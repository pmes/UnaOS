# SELFBUILD6 — a dynamic loader inside the shim, and rustc on UnaOS (ledger B360)

## Design
**Finding.** SELFBUILD5 (B357) linked a Rust program on UnaOS with a static `ld.lld` and named the wall: `rustc` is a DYNAMIC
program. It is a 9 KB PIE over `librustc_driver-*.so` (301 MB), and it `dlopen`s every proc-macro. No static build of it is
shipped, and a source bootstrap does not fit this disk.

**Seam.** `CHARTER: Kernel — shared-core`. The loader is ONE implementation in a new `no_std` crate,
`unaos/libs/sys/ldso_core`. It parses, maps, lays out static TLS, resolves by GNU hash with versions, relocates, orders
init/fini and keeps the `dl_iterate_phdr` table. It works against a `Space` trait (read, write, map file, map anon, reserve). Two
fulfillers link it:
- the kernel's `arch/x86_64/linuxabi/ldso.rs` (`CHARTER: Kernel — shared-core`), over the process's VMAs (SELFBUILD3/4);
- a host runner (`ldso_core/examples/ldrun.rs`), over `mmap` on Linux. It is the only EXECUTION proof this arc has (R78: no QEMU),
  and it runs the same relocation code on the real payload.

It is not musl's `ld.so` (C, R79). The program's libc IS musl, the same `libc.a` every static Rust program on UnaOS already
links. arroyo re-links it as `libc.so` at image time, because rustc's DT_NEEDED asks for it. The ring-3 half of the loader is
one page of hand-written machine code (`ldso_core/src/tramp.S`, bytes checked by a host test). It holds the `__libc_start_main`
hook that runs constructors after libc is up, plus `dlopen` / `dlsym` / `dlclose` / `dladdr` / `dlerror` / `dl_iterate_phdr` /
`__tls_get_addr`. On the kernel `dlopen` / `dlsym` / `dlclose` / `dladdr` reach `ldso.rs` through four private syscalls
(7504..7507). On the host they reach the runner's functions. (`dladdr` was added in M2: rustc finds its sysroot by
`dladdr` on a driver function, and the static `libc.a` stub answers nothing.)

**Milestones.**
- M1: the probe, written down below before any code.
- M2: `ldso_core` plus the host runner, proven on `dyn` (a hand-built PIE plus one `.so` against musl).
- M3: the probes in order on the host (`dyn`, `lld_dyn`, `rustc --version`, rustc hello, proc-macro), then the kernel fulfiller
  and `tests selfbuild6`.
- M4: the numbers and the honest NEXT.

**Witness.** `:: SELFBUILD6: dyn=ok lld_dyn=<ok|skip> rustc_version=<ok|oom> rustc_hello=<ok|oom> proc_macro=<ok|oom> relocs=<n> ms=<n> -> PASS ::`
(`skip` for `lld_dyn` is the image window refusing the 141 MB `ET_EXEC`; `oom` carries `rss_mib=<n>`.)

**Stays owed.** RELRO is not re-protected after relocation. Constructors of a `dlopen`ed object's TLS reach only the calling
thread. ASLR is off (first-fit bases).

## M1 — the probe (static.rust-lang.org answered; nothing here is guessed)
**Payload.** `rustc-nightly-x86_64-unknown-linux-musl.tar.xz` for `dist/2026-07-15`, the date of the pinned nightly
`rustc 1.99.0-nightly (da80ed070 2026-07-14)`.
- Size: 108341620 bytes.
- sha256 `706b319322fe2d737c6c56ac5bf7ec0a4ce2e76c49c12220099c0a3f41aa4c10`, which matches upstream's `.sha256`.

The matching std is `rust-std-nightly-x86_64-unknown-linux-musl.tar.xz`, 40480036 bytes, sha256
`778d9ca7f9cfed6accc3174e2f75a7a0d670e62d8e010f5cbff59b8af35eaae1`. It is the same artifact the host toolchain already carries
under `rustlib/x86_64-unknown-linux-musl`.

| object | type | PT_INTERP | DT_NEEDED | relocations (count, type) | TLS | other |
|---|---|---|---|---|---|---|
| `bin/rustc` (9528 B) | ET_DYN (PIE), BIND_NOW | `/lib/ld-musl-x86_64.so.1` | `librustc_driver-3a02c43c96fe1fbd.so`, `libc.so` | 7 GLOB_DAT, 6 RELATIVE, 1 JUMP_SLOT | none | RPATH `$ORIGIN/../lib`; init_array 1, fini_array 1 |
| `lib/librustc_driver-*.so` (300886528 B) | ET_DYN, BIND_NOW, **STATIC_TLS** | — | `libgcc_s.so.1`, `libc.so` | 249004 RELATIVE, 10679 GLOB_DAT, 4741 R_X86_64_64, **235 TPOFF64**, 144 JUMP_SLOT, **7 DTPMOD64, 3 DTPOFF64** | PT_TLS memsz 0x3428 (tdata 240 B), align 8 | GNU_HASH + SysV HASH; VERNEED → `libgcc_s.so.1` GCC_3.0 / GCC_3.3 / GCC_4.2.0; init_array **688 entries**; 746 undefined dynamic symbols; VA span 232 MB |
| `rustlib/…/bin/rust-lld` | **ET_EXEC** at 0x400000, span 0x400000..0x8d34000 (141 MB) | `/lib/ld-musl-x86_64.so.1` | `libgcc_s.so.1`, `libc.so` | — | PT_TLS memsz 0x4c0 | |

- **IFUNC / IRELATIVE: none** (0 `STT_GNU_IFUNC` symbols, 0 IRELATIVE). **No DT_RELR, no TEXTREL, no COPY.** The loader handles
  exactly RELATIVE, GLOB_DAT, JUMP_SLOT, 64, TPOFF64, DTPMOD64 and DTPOFF64, and refuses anything else by name.
- **TLS model:** initial-exec (TPOFF64) for 235 symbols, plus 7 general-dynamic slots (DTPMOD64/DTPOFF64 → `__tls_get_addr`). No
  TLSDESC. The DF_STATIC_TLS flag says the object must sit in the static TLS block, so the x86-64 variant II static layout
  covers it. `__tls_get_addr` is then `fs:0 + module_tpoff + offset`: the loader writes the module's TP offset into the
  DTPMOD64 slot, which only it and its own `__tls_get_addr` read.
- **What the tarball does NOT ship:** `libc.so` and `libgcc_s.so.1`. On a musl system `libc.so` IS `ld-musl` (loader and libc in
  one file). The Rust target's `self-contained/` dir has `libc.a` (musl) and `libunwind.a` (LLVM). Both turn out to be PIC:
  `ld.lld -shared --whole-archive` links them into a 1.7 MB `libc.so` and a 94 KB `libgcc_s.so.1`, with no text relocations.
  Their relocations are RELATIVE / GLOB_DAT / JUMP_SLOT only. Every one of the 376 symbols rustc and the driver import is
  defined by those two. The 14 left over are WEAK (`_ITM_*`, `__register_frame_info`, `__cxa_thread_atexit_impl`,
  `pidfd_*`) and resolve to 0.
- **What musl's static libc expects at start-up** (the loader must provide this, because libc is entered without its own
  `ld.so` stages):
  - `__init_tls` finds ONE PT_TLS in AT_PHDR's table. The loader gives it a SYNTHETIC table (PT_PHDR, one PT_TLS spanning
    every object's block plus a 16 KiB surplus for `dlopen`, PT_GNU_STACK) whose PT_TLS points at a template it builds. No
    PT_DYNAMIC is listed, because musl would read libc.so's own `_DYNAMIC` against it.
  - musl's static `__libc_start_init` runs only libc's own constructors. The loader therefore resolves the program's
    `__libc_start_main` to its trampoline, which swaps `main` for a hook. The hook runs every other object's DT_INIT and
    init_array (dependencies first) once libc and TLS are up, registers the fini list with `atexit`, then calls the real `main`.
  - `dl_iterate_phdr` / `dlopen` / `dlsym` in libc.a are WEAK static stubs (main program only; "Dynamic loading not
    supported"). The loader's table defines those names first in lookup order. LLVM libunwind (inside `libgcc_s.so.1`) finds
    every object's `.eh_frame_hdr` through `dl_iterate_phdr`, which is what lets a panic inside rustc unwind.
- **The UnaOS image window:** an ET_EXEC must fit `0x10000..16 MiB` (elf.rs `IMAGE_LIMIT`; above it is the kernel heap). The
  musl `rust-lld` is an ET_EXEC spanning 141 MB at 0x400000, so the kernel cannot place it. `lld_dyn` is a host-runner proof
  only, and the kernel wire says `lld_dyn=skip(window)`. The same rule refuses SELFBUILD5's 68 MB static `LLD.LNX` (ET_EXEC at
  0x400000). That is an unflown bug in B357, recorded for the seat under "Not done". PIE and shared objects carry no fixed
  address, so they go to first-fit bases in the 448 GiB second mmap window (`vm::MMAP2_*`).


## M2 — `ldso_core` and the host runner (written; `dyn=ok` on the host kernel)
**The crate** (`unaos/libs/sys/ldso_core`, `no_std` + `alloc`, no dependencies, a member of the root workspace; the kernel pulls
it with `default-features = false`):
- `src/elf.rs`: the header, program headers, dynamic tags, `Elf64_Sym`, the GNU and SysV hash functions, the seven relocation
  numbers and the refusal names.
- `src/tls.rs`: the variant II layout ([the formula below](#the-tls-formula-musl-needs)), with unit tests against lld's rule and
  musl's.
- `src/lib.rs`: the `Space` trait (open, read_file, reserve, map_file, map_anon, map_code, write, read) and the `Loader`:
  - **map**: each object is reserved whole (first fit; an `ET_EXEC` exactly at its addresses), every `PT_LOAD` mapped
    privately from the file, the page holding a segment's file end zeroed past it when `.bss` follows, `.bss` pages anonymous,
    and the holes between segments `PROT_NONE`. Metadata (dynamic section, symbol and string tables, hash tables, versions,
    relocations) is read from the FILE, never through the mapping, so a lazy mapping is only faulted where a relocation lands.
    Segments that share a page are refused (none in the payload).
  - **dependencies**: breadth-first over `DT_NEEDED`; `DT_RPATH` (the needer's, then the executable's) or `DT_RUNPATH`, with
    `$ORIGIN`; then the fulfiller's search list.
  - **lookup**: the loader's own eight names first (`__libc_start_main dlopen dlsym dlclose dladdr dlerror __tls_get_addr
    dl_iterate_phdr`), then the global scope in load order (plus the object's `dlopen` group). GNU hash with the bloom filter,
    SysV hash as the fallback. Versions: a reference's `DT_VERSYM` index names its `DT_VERNEED` entry; a definer with
    `DT_VERDEF` must match by name; a definer without version tables satisfies any version (that is how the relinked
    `libgcc_s.so.1` serves the driver's `GCC_3.0` / `GCC_3.3` / `GCC_4.2.0` and rust-lld's `GCC_3.4`). Hidden versions answer
    only a versioned reference. A strong miss is an error naming `symbol@version`; a weak miss is 0.
  - **relocation**: `RELATIVE`, `GLOB_DAT`, `JUMP_SLOT`, `64`, `TPOFF64` (`S + A - d(module)`), `DTPMOD64` (the module's TP
    offset, which only the loader's `__tls_get_addr` reads), `DTPOFF64`; anything else is refused by name and number. A
    per-object cache resolves each symbol once. Tables stream from the file 2730 entries at a time.
  - **constructors / destructors**: post-order over the dependency graph (dependencies first, the executable last): `DT_INIT`
    then `DT_INIT_ARRAY` of every object except libc.so (musl's static `__libc_start_main` runs libc's own); the fini list is
    the reverse (`DT_FINI_ARRAY` backwards, then `DT_FINI`), registered with libc's `atexit` by the trampoline hook.
  - **arena**: the TLS template, the synthetic program headers, the init/fini lists, the `dl_phdr_info` table (64-byte
    records; republished, table pointer before count, on every `dlopen`), object names and `dlerror` strings, bump-allocated
    in RW pages of the program.
  - **dlopen** (`RTLD_LOCAL`/`RTLD_GLOBAL`/`RTLD_NOLOAD`, `NULL` = the program): maps the object and its new dependencies as one
    group, places their TLS in the surplus and writes each initial image into the template and into the CALLING thread's
    block (`tp` comes with the gate), relocates against global + group, leaves the constructor list in the trampoline's
    PENDING slot (the trampoline runs it on return). **dlsym** searches the handle's group breadth-first (or the global scope
    for `RTLD_DEFAULT` / `RTLD_NEXT` / the program); TLS symbols are not served. **dlclose** drops a reference (objects stay
    mapped). **dladdr** fills `Dl_info` with the object's path and base and the nearest dynamic symbol.
- `src/tramp.S` + `tools/gen_tramp.py` + `src/tramp_bytes.rs` (ADOPTED from the untracked M1 draft, extended with `dladdr` and
  the 7507 gate; `gen_tramp.py` now refuses an assembly with relocations): 384 bytes of position-independent code; the data
  page slots are `DATA_*` in `lib.rs`.

### The TLS formula musl needs
musl's STATIC `__init_tls` reads ONE `PT_TLS {vaddr I, filesz, memsz S, align A}` from `AT_PHDR` and places the block at
`tp - S'` with `S' = S + ((-S - I) & (A-1))`, copying `filesz` bytes; `tp` is `A`-aligned. The loader builds a template of
`S` bytes at a page-aligned `I`, `S` a multiple of `A = 64`, so `S' = S`. Each module's distance below `tp`:
- the executable first: `d = memsz + ((-memsz - p_vaddr) & (align-1))` — exactly lld's x86-64 `getTlsTpOffset`, so its
  local-exec code agrees (the dyn probe's PIE and rust-lld have executable TLS);
- each shared object after it: `c = d(prev) + memsz; d = c + ((-c - p_vaddr) & (align-1))`, so `tp - d ≡ p_vaddr (mod align)`;
- then 16 KiB of surplus for `dlopen`.
Module `m`'s image sits at template offset `S - d(m)`; its TP offset is `-d(m)`. The synthetic table is `PT_PHDR` (so musl's
base is 0), that `PT_TLS`, and `PT_GNU_STACK`. No `PT_DYNAMIC` is listed: libc.so's own `_DYNAMIC` is non-zero, and musl would
compute its base from it. For rustc: the driver's block is `d = 0x3428`, the template 29760 bytes.

**The host runner** (`src/bin/ldrun.rs`, feature `host`): `ldrun [-L dir]... [--stats] <program> [args...]`. It fulfils `Space`
with `mmap` (`MAP_FIXED_NOREPLACE` for an `ET_EXEC`), builds the System V stack (the synthetic `AT_PHDR`/`AT_PHNUM`,
`AT_BASE` = the trampoline, `AT_RANDOM`, `AT_HWCAP`, the real uids), and jumps to the entry on its own thread. Two host-only
cares: its global allocator is `mmap`-backed (glibc and musl's mallocng must not share the program break), and each gate swaps
`FS` to the runner's thread pointer for the call and back (the program's `FS` is also the `tp` a late TLS image is written
under).

**The dyn probe** (`fixtures/dyn`: `hello.c` PIE, `libdyn.c`, `libplug.c`, `build.sh`; clang `--target=x86_64-unknown-linux-musl
-nostdinc -ffreestanding` + ld.lld against the Rust musl target's self-contained crt objects — there is no musl-gcc here). It
checks, in order: constructors before main (1), `JUMP_SLOT` (2), `R_X86_64_64` and `GLOB_DAT` agree (3), `TPOFF64` and
`DTPMOD64`/`DTPOFF64` initial images (4), the executable's local-exec block (5), initial-exec from the executable (6), a
`pthread_create`d thread gets the INITIAL images while the main thread's copy was changed (7–9), `dl_iterate_phdr` counts 3 (10),
`dlopen("libplug.so")` + `dlsym` + the plug's constructor + its TLS + its reference into libdyn.so (11–13), `dlerror` after a
miss (14), `dladdr` names libdyn.so (15), the table grows to 4 (16); a destructor prints at exit. Its relocations cover all
seven types (1 × 64, 2 × DTPMOD64, 2 × DTPOFF64, 32 GLOB_DAT, 569 JUMP_SLOT, 107 RELATIVE, 2 TPOFF64, counted over the five
objects).

**Host results** (`cargo test -p ldso_core`: 6 unit tests + `tramp_bytes_match_source` + `dyn_probe`, all ok):
`dyn=ok objects=3 after_dlopen=4 …`, `dyn: destructor ran`, exit 0; `[ldrun] loaded objects=3 pages_mapped=197 relocs=663`.

**Found on the way — the relink recipe M1 wrote down was not enough** (each fixed in `fixtures/dyn/build.sh`, which arroyo runs):
1. `libc.a --whole-archive` pulls musl's complex functions, which need `__mulsc3` / `__muldc3` / `__mulxc3`. musl links its own
   libc.so with `-lgcc`; so does this one (`gcc -print-libgcc-file-name`, members pulled only as needed). libc.so then has no
   undefined symbol.
2. Without `--eh-frame-hdr` neither relinked object has `PT_GNU_EH_FRAME`. LLVM libunwind finds FDEs through `dl_iterate_phdr`
   + that header, so it could not unwind its OWN frames and the first rustc error (raised as a panic) aborted with "failed to
   initiate panic, error 5" (`_URC_END_OF_STACK`). With the flag, a type error prints its diagnostic and rustc exits 1.
3. The musl rust-lld imports `__popcountdi2@GCC_3.4` (libgcc.a's copy is hidden, so it cannot be re-exported), and its
   crtbegin `frame_dummy` — non-PIC code — takes `__register_frame_info`'s canonical PLT address, which is never 0, so a weak
   miss jumps to address 0. `fixtures/dyn/gcc_helpers.c` exports `__popcountdi2` and no-op `__register_frame_info` /
   `__deregister_frame_info` (libunwind finds every FDE by the header anyway).
4. rustc links proc-macros with `-lgcc_s`: `libgcc_s.so` (a copy of `libgcc_s.so.1`) is staged beside it.

## M3 — the probes (host-proven under `ldrun`; the kernel fulfiller and `tests selfbuild6` written)
**Host, the staged tree, pinned to one CPU** (`taskset -c 0`, what UnaOS's `sched_getaffinity` answers; peak RSS from
`getrusage(RUSAGE_CHILDREN)`):

| probe | command (under `ldrun -L LIB/dyn`) | result | ms | peak RSS |
|---|---|---|---|---|
| dyn | `LIB/dyn/hello` | `dyn=ok objects=3 after_dlopen=4`; 663 relocations, 197 pages mapped | 20 | 9.9 MiB |
| lld_dyn | `rust-lld -flavor gnu --version` | `LLD 22.1.8 (…) (compatible with GNU linkers)`; 3 objects, 912 relocations, 35337 pages (138 MiB span) | 13 | 22.2 MiB |
| lld_dyn link | rustc hello with `-C linker=<ldrun rust-lld>` | the output runs: `hello from rustc on ldso_core: sum=55` | 241 | 160 MiB |
| rustc_version | `rustc --version` | `rustc 1.99.0-nightly (da80ed070 2026-07-14)`; 4 objects, **265481 relocations**, 13882 lookups, 57014 pages (223 MiB) mapped, 9.6 MB of tables read, load 72 ms | 96 | **39.7 MiB** |
| rustc_hello | `rustc -C codegen-units=1 -O hello.rs --target x86_64-unknown-linux-musl -C linker=ld.lld -C linker-flavor=ld.lld -C link-self-contained=yes -C relocation-model=static -C target-feature=+crt-static` | 4813520-byte ET_EXEC; runs: `hello from rustc on ldso_core: sum=55` | 302 (6055 cold) | **161 MiB** |
| proc_macro (build) | `rustc --crate-type proc-macro pm.rs … -L LIB/dyn` | `libpm.so` (5.7 MB; NEEDED libgcc_s.so.1 + libc.so; PT_TLS 0xd8) | 654 | 157 MiB |
| proc_macro | `rustc user.rs --extern pm=libpm.so …` | `dlopen(libpm.so, RTLD_LAZY)` then `dlsym(__rustc_proc_macro_decls_…__)` through the gate; runs: `proc_macro: derived Hello for Probe` | 199 | 141 MiB |
| panic path | `rustc bad.rs` (a type error) | E0308 diagnostic, exit 1 (the unwinder works through every object) | 130 | 131 MiB |

The driver's relocation counts match M1's `readelf` exactly (264813 in the driver + 14 + 616 + 38).

**The kernel fulfiller** (`arch/x86_64/linuxabi/ldso.rs`, `CHARTER: Kernel — shared-core`):
- `KSpace` implements `Space` over the process's `AddrSpace`: `open`/`read_file` through the VFS; `reserve` first-fit in
  `vm::MMAP2_BASE..MMAP2_LIMIT`, or for an `ET_EXEC` inside the image window (`0x10000..16 MiB`, as `elf.rs`; outside it
  answers `skip(window): …`); `map_file`/`map_anon` insert lazy VMAs (W^X refused); `map_code` read-populates then stores;
  `write` is `copy_out(force)` after read-populating any page a write-populate would refuse.
- `run_inner` (`linux`, every `tests` run) and `execve` ask `ldso::wants` (a `PT_INTERP` in the head) and take
  `ldso::load`, which returns the `elf::Plan` `build_stack` takes (entry + the synthetic headers). Static programs keep the
  SELFBUILD4 path; a static-PIE is still refused by `elf.rs`.
- The loader state is a map PML4 → `Loader` (fork copies it, execve and session end drop it). `sys.rs` routes 7504..7507 to
  `ldso::gate` (dlopen reads the caller's `FS_BASE` as `tp`); the process lock already serialises one process's gates.
- `[ldso] load path= objects= pages_mapped= relocs= lookups= read= ms= entry= tramp=` and one line per object on serial;
  `[ldso] dlopen <path> flags= -> <handle> objects=` per dlopen.

**`tests selfbuild6`** (`selfbuild6.rs`, registered on the LINUXABI line of `tests.rs`): the five probes above, in order, each
a `run_path` session with `vm::PEAK_RESIDENT` reset; the two compiles run under busybox `sh -c "TMPDIR=<home> rustc …"`
(rustc writes temporaries; only the home is writable; `run_path`'s environment is fixed) with `-C linker=/apps/LLD.LNX`, then
the output runs. A failed probe while `vm::REFUSALS` grew reads `oom rss_mib=<n>`; any other failure `fail(…)` → FAIL.

**arroyo** `build_selfbuild6_x86` (called after `build_selfbuild5_x86`) + builder rows (APPS/LIB/dyn, APPS/LIB/rustc): see its
comment. `LIB/dyn` is always built when the musl target exists. `LIB/rustc` (530 MB: the stripped driver 222 MiB, rust-lld
157 MiB, the musl std 147 MiB after dropping the sanitizer runtimes and the profiler) only with `UNAOS_SELFBUILD6_RUSTC=1`, the
rustc tarball fetched and sha-checked at image time, and staged only when rustc --version, hello and the proc-macro crate all
run under ldrun. One run of the whole function took 27 s (the tarball cached). sha256 of that run: `bin/rustc`
`8b29ba2ebb045e2beeba90c362c67892ecac63c4fd331444c28a91764679799d`, the stripped driver
`ee58664c6683b71517c68d7640366c4f037ada197bd4a32db55fc4d31e0fd3fb`, rust-lld
`287f1ef844fec811560a5879b47db810d6446cc586c181d6902b934634a8d11b`, `pm/libpm.so`
`5e36883b8f69fb4f69bd84820045edfee40e7206c35fd23e6f193b743a48c300` (one run; reproducibility not proven).

**What a metal boot should print** (`UNAOS_SELFBUILD6_RUSTC=1 ./arroyo esp-x86`, metal shape with `linuxabi`, logged in):
```
tests selfbuild6
[selfbuild6] linux /apps/LIB/dyn/hello
[ldso] load path=/apps/LIB/dyn/hello objects=3 pages_mapped=197 relocs=663 lookups=570 read=<~106000> ms=<n> entry=<va> tramp=<va>
[selfbuild6] dyn: dyn=ok objects=3 after_dlopen=4 argv0=/apps/LIB/dyn/hello
[selfbuild6] dyn: dyn: destructor ran
:: LINUXABI: path=/apps/LIB/dyn/hello exit=0 syscalls=<n> enosys=[] ms=<n> -> PASS ::
[selfbuild6] linux /apps/LIB/rustc/lib/rustlib/x86_64-unknown-linux-musl/bin/rust-lld -flavor gnu --version
[selfbuild6] lld_dyn: load: /apps/LIB/rustc/…/rust-lld: skip(window): ET_EXEC at 0x400000..0x8d34000 is outside the image window 0x10000..0x1000000
[selfbuild6] linux /apps/LIB/rustc/bin/rustc --version
[ldso] load path=/apps/LIB/rustc/bin/rustc objects=4 pages_mapped=57014 relocs=265481 …
[selfbuild6] rustc_version: rustc 1.99.0-nightly (da80ed070 2026-07-14)
[selfbuild6] rustc load: objects=4 pages_mapped=57014 relocs=265481 load_ms=<n> read_bytes=<~9.6 MB>
[selfbuild6] linux sh -c TMPDIR=/home/<user> /apps/LIB/rustc/bin/rustc -C codegen-units=1 -O /apps/LIB/rustc/hello.rs … -o /home/<user>/hello6.lnx
[selfbuild6] rustc_hello: hello from rustc on ldso_core: sum=55          (or: verdict=oom rss_mib=<~96>)
[selfbuild6] proc_macro: proc_macro: derived Hello for Probe             (or: verdict=oom rss_mib=<~96>)
[selfbuild6] kernel: loads=<n> dlopens=<n> dlsyms=<n> faults_file=<n> faults_anon=<n> refusals=<n> pool_peak=<n>
:: SELFBUILD6: dyn=ok lld_dyn=skip rustc_version=ok rustc_hello=<ok|oom rss_mib=n> proc_macro=<ok|oom rss_mib=n> relocs=265481 ms=<n> -> PASS ::
```
Without `UNAOS_SELFBUILD6_RUSTC=1`: `… lld_dyn=skip rustc_version=skip rustc_hello=skip proc_macro=skip relocs=0 … -> SKIP ::`
(dyn still ran).

**Reading a failure:** `dyn=fail(exit=<k>)` — `<k>` is the probe's check number above (1 constructors, 4 TLS, 7–9 threads, 11–13
dlopen, 15 dladdr). `fail(load)` with `[selfbuild6] …: <path>: undefined symbol X@V` — a relink lost an export;
`relocation R_X86_64_<T> refused` — a payload outside the probed set. `rustc_version=fail(exit=SIG11)` with a
`[linuxabi] fault va=…` line in a `[ldso]` object span: compare the address with the object bases printed at load.
`rustc_hello=fail(exit=1)` with a linker message: `/apps/LLD.LNX` (B357) is an `ET_EXEC` at 0x400000 of 68 MB and the
SAME window refuses it until B361 relinks it PIE.

## M4 — what rustc's 301 MB driver needs beyond the lazy mappings (B361 WINDOW2 is in flight)
- **Address space: nothing more.** The driver is a shared object: 232 MB of span placed first-fit in the 448 GiB MMAP2 window
  as lazy file VMAs, like every object here. `USER_WINDOW_BYTES` (B361) is the native ring-3 slot window and does not bound a
  Linux process. What B361 does change for this arc: (a) `IMAGE_LIMIT` — the musl rust-lld is an `ET_EXEC` spanning
  0x400000..0x8d34000 (141 MB), which a 64 MiB window still refuses, so `lld_dyn` stays `skip` (the host proves it); (b) the
  PIE relink of `LLD.LNX`, which `rustc_hello` and `proc_macro` need as their linker (68 MB `ET_EXEC` at 0x400000 today).
  Once LLD.LNX is a PIE it is an `ET_DYN` without `PT_INTERP` — a static-PIE, which `elf.rs` refuses; B361 has to load it at a
  base itself (its own relocations are RELATIVE only), or link it `-no-pie` below the raised limit.
- **Resident memory: the frame pool.** Host peak RSS: `rustc --version` 39.7 MiB — inside the 96 MiB heap share
  (`vm::HEAP_SHARE` 24576 pages); `rustc hello.rs` 161 MiB and the proc-macro compile 141 MiB — past it. They need
  `vm::limit_pages()` = heap share + the user frame pool, which is still unflown; without it the honest wire is
  `rustc_hello=oom rss_mib=<~96>`. The unblocking work is the pool flying (and, behind it, page tables from the pool — today
  every table still comes from the kernel heap).
- **RELRO and data faults at load: ~4.4 MB, eager.** The driver's RW segment holds 4.4 MB of file bytes
  (`.data.rel.ro` 4.36 MB + `.got` 105 KB + `.data` 80 KB) plus 0.8 MB of `.bss`. Its 249004 RELATIVE + 10679 GLOB_DAT +
  4741 `64` + 235 TPOFF64 stores touch nearly every page of it, so about 1100 file pages fault in (each a VFS `stat` + a 4 KiB
  read at a deep offset of a 232 MB FAT file) and become private dirty frames before `main`. Text faults after that are
  per touched page; `--version` ends with about 10000 resident pages in all (its RSS). Speed items, both owed: a shared page cache, and
  FAT cluster-chain lookups that do not walk from the file's start.
- **Kernel heap per dynamic process:** the resident lookup tables (driver: 2.5 MB dynstr + 485 KB dynsym + 116 KB GNU hash +
  40 KB versym), copied at fork (rustc's `posix_spawn` of the linker is a COW vfork; the copy dies at the child's execve).

## Not done / limits
- NEVER run under UnaOS (R76/R78: no QEMU). The kernel side is compile-proven only (x86 metal shape + linuxabi + selfdiag,
  ahciroot, btc; the aarch64 leg). The only executions are the host runner's, on the real payload.
- RELRO is not re-protected after relocation. `dlclose` never unmaps. A `dlopen`ed object's TLS image reaches the calling
  thread and later threads, not threads already running (rustc dlopens proc-macros on the thread that runs them). ASLR is off.
- `dlsym` of a TLS symbol answers 0 with `dlerror`. No lazy binding: every reference resolves at load (BIND_NOW semantics,
  which is what rustc and the driver ask for anyway).
- The kernel's `lld_dyn` is `skip` by construction (the window); `rustc_hello` / `proc_macro` on the kernel also need a linker
  the shim can load (B361's LLD.LNX) and the frame pool (above).
- The ESP payload with `UNAOS_SELFBUILD6_RUSTC=1` is 530 MB (opt-in; not in git). `LIB/dyn` is 1.9 MB.
- FINDING for B357 (from M1, unchanged): `elf.rs` `IMAGE_LIMIT` refuses the 68 MB static LLD.LNX, so `tests selfbuild5` prints
  "segment outside the loadable window" on metal until B361.
