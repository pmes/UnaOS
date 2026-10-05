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
hook that runs constructors after libc is up, plus `dlopen` / `dlsym` / `dlclose` / `dlerror` / `dl_iterate_phdr` /
`__tls_get_addr`. On the kernel `dlopen` / `dlsym` / `dlclose` reach `ldso.rs` through three private syscalls. On the host they
reach the runner's functions.

**Milestones.**
- M1: the probe, written down below before any code.
- M2: `ldso_core` plus the host runner, proven on `dyn` (a hand-built PIE plus one `.so` against musl).
- M3: the probes in order on the host (`dyn`, `lld_dyn`, `rustc --version`, rustc hello, proc-macro), then the kernel fulfiller
  and `tests selfbuild6`.
- M4: the numbers and the honest NEXT.

**Witness.** `:: SELFBUILD6: dyn=ok lld_dyn=<ok|skip(window)> rustc_version=ok rustc_hello=<ok|oom> proc_macro=<ok|oom> relocs=<n> ms=<n> -> PASS ::`

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
