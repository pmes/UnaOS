# SELFBUILD5 — mremap, the alternate signal stack, and UnaOS links a Rust program (ledger B357)

## Design
**Finding.** SELFBUILD4 (B356) ran the first Rust program and named the next walls in order: `mremap` (musl's `realloc` past
~128 KiB answered -ENOSYS), SA_ONSTACK delivery, and a LINKER on UnaOS before a compiler. tcc cannot link Rust objects; the
toolchain's `rust-lld` is a dynamic program over `libLLVM.so`, which the shim cannot load.

**Seam.** `CHARTER: Kernel — driver` (the compatibility box, as SELFBUILD1–4; no handler owns the Linux ABI). New kernel files:
`arch/x86_64/linuxabi/remap.rs` (mremap, per-thread sigaltstack, getcpu) and `selfbuild5.rs` (`tests selfbuild5`). Existing files
get routing only: `sys.rs` (three arms), `signal.rs` (the frame's top and `uc_stack` from `remap::sig_stack`, re-arm at
`rt_sigreturn`, forget at exec/exit), `vm.rs` (five helpers made `pub(super)`), `thread.rs`/`proc.rs` (a new thread starts
without an alternate stack, a fork child inherits it), `mod.rs` (two `mod` lines, three names), `tests.rs` (register). Host side:
`crates/user-linux-hello/c/syskat5.c`, arroyo `build_selfbuild5_x86` (a static `ld.lld` built from the pinned LLVM release, the
RUST.LNX objects and the std rlibs staged as data), builder rows. No new knob: it rides `UNAOS_LINUXABI=1`.

**Milestones.** M1 mremap + sigaltstack at delivery + getcpu, proven by SYSKAT5 on the host kernel. M2 the LLD.LNX probe (build,
strace on Linux, staging). M3 `tests selfbuild5`. M4 this doc with the honest NEXT.

**Witness.** `:: SELFBUILD5: mremap=ok altstack=ok lld=ok link_ms=<n> link_peak_mib=<n> relink_runs=1 -> PASS ::`, after
`:: LINUXABI-KAT5: … -> PASS ::`, `[selfbuild5] link: …` and `[selfbuild5] kernel: …`.

**Stays owed.** Signal delivery at a FAULT (so a real stack-overflow handler runs on the alternate stack), rustc itself.

## M1 — mremap, the alternate signal stack, the small answers (written; proven by SYSKAT5 on the host kernel)
- **`mremap` (25)** (`remap.rs`), over SELFBUILD3's VMAs:
  - A **shrink** unmaps the tail (written back first if it is MAP_SHARED) and answers the old address.
  - A **grow in place** happens when the range ends its VMA and the pages after it are free and legal (`vm::fixed_ok`). Only the
    VMA's end moves. No page is touched.
  - Otherwise, with `MREMAP_MAYMOVE`, the range **moves**. A new VMA with the same protection, file and offset is placed first-fit
    over the two mmap windows, or at `new_addr` with `MREMAP_FIXED` (whatever was there is unmapped; ranges that overlap are
    refused). Every resident leaf of the old range, present or PROT_NONE-resident, is **re-homed by rewriting its PTE**, so the
    frame, the copy-on-write bits (COW/CW) and the dirty bit go with it. No byte is copied and no frame changes hands, so
    `m.frames`, `REFS` and the budget stay as they were. A shared page from a COW fork keeps its sharing at the new address.
  - `MREMAP_DONTUNMAP` (equal lengths only) moves the pages and keeps the old VMA. It then faults zero (anonymous) or re-reads its
    file, as on Linux.
  - Answers follow Linux: an unaligned old address, FIXED or DONTUNMAP without MAYMOVE, or DONTUNMAP with a length change gives
    -EINVAL. An old range not inside ONE mapping gives -EFAULT. A blocked grow without MAYMOVE gives -ENOMEM. `old_len == 0` (the
    duplicate-a-shared-mapping form) gives -EINVAL, which is not answered. The brk heap VMA itself gives -EINVAL, since it is
    `brk`'s to size.
  - Counters: `MREMAP_CALLS`, `MREMAP_INPLACE`, `MREMAP_MOVED`, `MREMAP_PAGES`.
- **`sigaltstack` (131), per THREAD** (keyed by `thread::key_for`, the key FS_BASE and FXSAVE already use):
  - Install, disable and query work. A query while on the stack reports `SS_ONSTACK`. Changing the stack there gives -EPERM.
    `SS_AUTODISARM` is honoured (disarmed at delivery, re-armed from the frame's `uc_stack` at `rt_sigreturn`). A size under
    MINSIGSTKSZ (2048) gives -ENOMEM.
  - `signal::deliver` asks `remap::sig_stack` for the frame's top. With `SA_ONSTACK`, an armed stack and the thread not already on
    it, the frame goes at `ss_sp + ss_size`; otherwise it goes 128 bytes under the user rsp, as before. The frame's
    `ucontext.uc_stack` now carries the real `{ss_sp, ss_flags, ss_size}` instead of a constant SS_DISABLE.
  - A new thread starts without one (`thread.rs`). A fork child inherits the forking thread's (`proc.rs`). `execve`, exit and a
    new session drop them (`signal.rs`).
- **The small answers.** `getcpu` (309) answers CPU 0, node 0. `madvise(MADV_HUGEPAGE)` was already accepted as a hint by
  SELFBUILD3's `madvise`, and `sched_yield` was already 0. Both are now KAT-checked. `mremap`, `getcpu` and `madvise` are named in
  `sys_name`.
- **Stays owed (stated).** Delivery is still at the syscall boundary only. A fault on a guard page therefore kills the group with
  status 11, as before, instead of running Rust std's overflow handler on the alternate stack. To run it, the #PF entry would
  have to save the full ring-3 register file (today's `x86-interrupt` handler does not expose it) and enter the handler through
  a second trampoline. That is SELFBUILD4 M4 item 6.

**SYSKAT5.LNX** (`crates/user-linux-hello/c/syskat5.c`) is freestanding, built with gcc 13.3 `-static -nostdlib -fno-pie -no-pie
-O1 -ffreestanding -mno-red-zone`. It is 14544 bytes, sha256 `b4215177b8133b531265ce498f63fd096ed02cc972d3fb273da949d532b7b4df`,
and it takes no arguments. Its groups:
- **grow** 1-4: 2 pages grown to 4 in place; the bytes kept; the new pages zero and writable; a shrink to 1 page with the dropped
  page unmapped.
- **move** 10-16: -ENOMEM without MAYMOVE when the next mapping blocks the grow. With MAYMOVE, a new address; the bytes moved;
  the tail zero; the old range unmapped; the neighbour untouched. Then realloc's shape: 1 MiB stamped every 4 KiB, grown to
  4 MiB.
- **fixed** 20-22: onto a reserved range.
- **dontunmap** 30-32: the old range still mapped, zero and writable.
- **errors** 40-43.
- **altstack** 50-59: install and read back; SA_ONSTACK; `kill(self)` runs the handler once with its local variable inside the
  alternate stack; SS_ONSTACK in the handler; -EPERM to change it there; `uc_stack.ss_sp`; flags 0 after return; a handler
  without SA_ONSTACK on the normal stack; SS_DISABLE.
- **small** 60-62.

Host result (Linux 6.18): `syskat5 ok grow=ok move=ok fixed=ok dontunmap=ok errors=ok altstack=ok small=ok checks=42 fail=none`,
exit 0. `strace -f -c` counts 90 calls: write 34, mremap 11 (5 deliberate errors), munmap 10, sigaltstack 9 (1 EPERM), mmap 9,
mincore 4, rt_sigaction 3, rt_sigreturn 2, kill 2, getcpu, madvise, sched_yield, mprotect, getpid, execve.

## M2 — LLD.LNX, the link probe (built; host-proven; staged by arroyo)
**The linker.** I took the first route the row offers: build `lld` alone from the pinned release.
- **Rejected options.** The toolchain's `rust-lld` (11.6 MB) is dynamic over `libLLVM.so.22.1-rust-1.99.0-nightly`, libz and
  glibc, so the shim cannot load it (no PT_INTERP). The LLVM GitHub prebuilts are glibc-dynamic too.
- **The build.** `nightly rustc 1.99.0 (da80ed070 2026-07-14)` reports **LLVM 22.1.8**. Its source tarball is
  `llvm-project-22.1.8.src.tar.xz`, 167061596 bytes, sha256 `922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888`
  (GitHub release asset; pinned in arroyo as `SELFBUILD_LLVM_SHA256`). Only `llvm/ lld/ cmake/ third-party/ libunwind/include`
  are extracted, without tests (236 MB). The build is CMake + Ninja, Release, `LLVM_TARGETS_TO_BUILD=X86`, `LLVM_BUILD_STATIC=ON`,
  `-fno-pie` / `-static -no-pie`, with no PIC, plugins, zlib, zstd, libxml2, libedit, libpfm, assertions, RTTI or VC revision.
- **Not musl.** The binary is **static, but glibc, not musl**. The host has no musl C++ runtime, so a musl lld would need a musl
  cross g++ first. glibc-static is the shape TCC.LNX already runs under the shim. The link warns about `getpwuid_r` in
  `home_directory` (only `~` expansion, which lld does not hit).
- **Build cost.** 1960 ninja steps for the `lld` target. Here, at a load average of 15–26 from the other executors, it took
  about 2 h 20 min at `-j3`. The 2-hour background job hit its time limit at step 1697/1960; ninja resumed and finished the last
  93 steps in under 10 minutes. On an idle 4-core host it should take about 40–60 minutes. arroyo builds it ONCE and keeps
  `target/LLD.LNX`; `UNAOS_LLD_LNX=<path>` stages an operator's prebuilt instead.
- **Result.** `LLD.LNX` is the stripped `bin/lld`: **68506176 bytes**, sha256
  `e1e3c681d3ea846453f293fa32458b2455caeb614a64ba59deaf58bdbaab4722`. It is ET_EXEC, statically linked, with no INTERP.
  `LLD.LNX -flavor gnu --version` prints `LLD 22.1.8 (compatible with GNU linkers)`. That sha comes from ONE build; a second
  clean build was not affordable in the disk and CPU allowance, so reproducibility is not proven.

**The objects.** arroyo rebuilds `crates/user-linux-rust` (RUST.LNX's crate, same flags) with `-C linker=<recording wrapper around
rust-lld> -C linker-flavor=ld.lld -C link-self-contained=yes`. The wrapper writes the argument list and copies every `.o`
input. A python step then maps it into `target/LIB/rust/` (31 files, 16028 KiB, staged as `/apps/LIB/rust`):
- the probe's objects: `probe-0.o` (the CGU, 58904 bytes), `probe-1.o` (allocator shim) and `probe-sym.o` (rustc's
  `symbols.o`);
- 18 std rlibs, renamed `lib<crate>.rlib`. `lib.rmeta` is dropped and `--strip-debug` applied, taking 22 MB down to 13 MB.
  lld reads only the members it needs;
- musl `crt1.o crti.o crtbegin.o crtend.o crtn.o`, `libc.a` (2.7 MB after strip-debug) and `libunwind.a` from the target's
  `self-contained/`;
- `link.rsp`: rustc's own ld.lld line, one argument per line, with `/apps/LIB/rust/` paths. `-flavor` and `-o` are left to the
  caller. sha256 `46f044349852bb0f0b2d6e9ec12409aacecde11e64c56f9bbba47583a11516ec`.

**Host proof** (the only execution, R78). `LLD.LNX -flavor gnu @host.rsp -o rust2`, pinned to one CPU (`taskset -c 0`, which is
what UnaOS's `sched_getaffinity` answers), exits 0 in **38 ms wall** with a **79 MiB** peak RSS (VmHWM 78904 KiB). The output is
637672 bytes, ET_EXEC, sha256 `3514853f052b2e3fc32e39dcdeab3ff54324b2ce29834a8ffcee1a7eae8cd9f1`. It runs and prints
`rust ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 args=2 file_bytes=897 file_lines=23 cpus=4 ms=9`, exit 0. The
same response file linked by `rust-lld` also runs; arroyo checks the staged linker, or `rust-lld` when LLD.LNX is absent.

**What the link asks of the kernel** (`strace -f`, one CPU, the static LLD.LNX): **931 calls**, 22 of them errors, over 2 tasks.

| syscall | calls | what it is | the shim |
|---|---|---|---|
| mprotect | 354 | glibc's per-thread arena: a 128 MiB PROT_NONE reservation made RW as it grows | answered (PROT_NONE VMAs, SELFBUILD3) |
| futex | 256 | WAKE_PRIVATE 193, WAIT_BITSET_PRIVATE\|CLOCK_REALTIME 54 (no timeout), WAIT_PRIVATE 9 — the lld thread pool's condvar | answered (SELFBUILD2: WAIT, WAKE, WAIT_BITSET) |
| rt_sigprocmask 66 · rt_sigaction 16 · sigaltstack 2 | 84 | glibc thread start-up and lld's crash handler | answered; sigaltstack is real now |
| openat 33 · fstat 33 · close 33 · pread64 14 · lseek 2 | 115 | the response file (read twice), 31 inputs, the output | answered |
| mmap | 23 | inputs mapped `PROT_READ, MAP_PRIVATE\|MAP_NORESERVE` (libstd.rlib 6.7 MB the largest), a thread stack (8 MiB PROT_NONE), the arena, the OUTPUT `MAP_SHARED` RW | lazy file VMAs; MAP_SHARED write-back at munmap (SELFBUILD3) |
| brk | 24 | the main arena | answered |
| getrandom | 15 | temp-file names, hash seeds | answered |
| newfstatat 9 · access 4 · readlink 2 · readlinkat 1 | 16 | `realpath` of inputs (`readlink /proc/self/fd/N`), `/proc/self/exe` | `/proc/self/fd/N` answers -ENOENT, and LLVM leaves the real path empty (it is only used in diagnostics); `/proc/self/exe` answered (SELFBUILD1) |
| ftruncate 2 · unlink 1 · rename 1 | 4 | `FileOutputBuffer`: a 1-byte MAP_SHARED probe file (mapped, unlinked, unmapped), then `<out>.tmpXXXXXXX` O_EXCL, ftruncate to size, mapped, written, **munmap before rename** | answered (the write-back happens on munmap, before the rename, so the VMA's path is still right) |
| clone3 1 · set_robust_list 2 · rseq 2 · sched_getaffinity 3 | 8 | one pool worker even on one CPU; rseq -ENOSYS (deliberate) | answered |
| munmap 4 · prlimit64 · ioctl(TCGETS) · arch_prctl · set_tid_address · exit_group · execve | 10 | | answered |

Where the row's expectations differ from what was measured: there is **no `mremap`** (glibc never grows these blocks by mremap
in a link this size), **no `fallocate`** (lld uses `ftruncate`), and **no parallel threads to speak of**. On one CPU lld runs
one worker, and with an existing output a second one (the background unlink). Every call above was answered by the shim before
this arc, except sigaltstack, which is now real. So the link should run on UnaOS with no new syscall. The risk is time: each
file-page fault does a VFS `stat` and a 4 KiB read, and a 16 MB input set is about 4000 such faults at most.

## M3 — `tests selfbuild5` (written; `selfbuild5.rs`, registered on the LINUXABI line of `tests.rs`)
1. `SYSKAT5.LNX` (30 s) gives `:: LINUXABI-KAT5: grow= move= fixed= dontunmap= errors= altstack= small= fail= exit= -> … ::`.
   `mremap=ok` needs the five mremap groups; `altstack=ok` needs the altstack group.
2. `LLD.LNX -flavor gnu @/apps/LIB/rust/link.rsp -o <home>rust2.lnx` (600 s; the output is unlinked first). `vm::PEAK_RESIDENT`
   is reset before the run, giving `link_peak_mib`; the session's `ms` gives `link_ms`. It prints
   `[selfbuild5] link: out= out_bytes= link_ms= peak_resident_pages= exit=`.
3. `<home>rust2.lnx /apps/HELLO.C` (30 s) gives `relink_runs=1` when it prints `rust ok threads=4 counter=400000 …` and exits 0.
4. `[selfbuild5] kernel: mremap_calls= mremap_inplace= mremap_moved= mremap_pages= altstack_deliveries= faults_file= faults_anon=
   peak_resident=`.

The verdict is PASS when mremap and altstack are ok, lld is ok and relink_runs=1. Any fail gives FAIL. LLD.LNX or link.rsp not
staged with a passing KAT gives SKIP.

**What a metal boot should print** (`./arroyo esp-x86`, metal shape with `linuxabi`, logged in as `<user>`):
```
tests selfbuild5
:: LINUXABI: path=/apps/SYSKAT5.LNX exit=0 syscalls=<~90> enosys=[] ms=<n> -> PASS ::
:: LINUXABI-KAT5: grow=ok move=ok fixed=ok dontunmap=ok errors=ok altstack=ok small=ok fail=none exit=0 -> PASS ::
[selfbuild5] linux /apps/LLD.LNX -flavor gnu @/apps/LIB/rust/link.rsp -o /home/<user>/rust2.lnx
[linuxabi] thread pid=<p> tid=<t> key=+1 …
:: LINUXABI: path=/apps/LLD.LNX exit=0 syscalls=<~930> enosys=[] ms=<n> -> PASS ::
[selfbuild5] link: out=/home/<user>/rust2.lnx out_bytes=637672 link_ms=<n> peak_resident_pages=<~20000> exit=0
[selfbuild5] linux /home/<user>/rust2.lnx /apps/HELLO.C
[selfbuild5] rust2: rust: panic raised (expected; caught)
[selfbuild5] rust2: rust ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 args=2 file_bytes=<n> file_lines=23 cpus=1 ms=<n>
:: LINUXABI: path=/home/<user>/rust2.lnx exit=0 syscalls=<~160> enosys=[] ms=<n> -> PASS ::
[selfbuild5] kernel: mremap_calls=11 mremap_inplace=<1-2> mremap_moved=<4-5> mremap_pages=<n> altstack_deliveries=1 faults_file=<n> faults_anon=<n> peak_resident=<n>
:: SELFBUILD5: mremap=ok altstack=ok lld=ok link_ms=<n> link_peak_mib=<~80> relink_runs=1 -> PASS ::
```
(rseq answers -ENOSYS without being noted, which is deliberate in the shim, so `enosys=[]`.)

**Reading a failure:**
- `lld=fail(exit=…)` with a `[linuxabi] fault … NoVma|Prot` line points at the glibc arena (mprotect over PROT_NONE) or a
  thread stack.
- `lld=fail(load)` with `not ET_EXEC` means a PIE lld was staged.
- `out_bytes=0` with exit 0 means the MAP_SHARED write-back or the rename went wrong. Look for `<home>rust2.lnx.tmp*`.
- `relink_runs=0` with lld ok means the output is wrong. Compare its size with 637672.
- KAT5 `fail=<id>`: look the id up in the table in the C header (14 = the old range still mapped after a move, 31 = DONTUNMAP's
  old range not zero, 53 = the frame not on the alternate stack).

## M4 — the honest NEXT rung: rustc itself
Reached by construction (unflown): a growing Rust program's `realloc` gets a real `mremap`, and a static LLVM 22.1.8 linker
links a Rust program's objects against std and musl on the volume. What `rustc` on UnaOS still needs, in the order it would
bite:
1. **A static rustc.** The host rustc is `librustc_driver-*.so` (151 MB) + `libLLVM.so` + glibc, all dynamic. A static rustc
   means a rustc whose HOST is `x86_64-unknown-linux-musl` with LLVM linked in statically. Upstream ships a `rustc` for that host
   through rustup. Two ways to get it:
   - **(a) Download the upstream musl-host rustc** for the pinned nightly (`rustc-nightly-x86_64-unknown-linux-musl.tar.xz`,
     to be measured). As far as I know it still links `librustc_driver` as a shared object against musl's dynamic loader, so it
     would need a dynamic loader on UnaOS. That is unverified here: the next arc's first probe is `readelf -d` on it.
   - **(b) Bootstrap from source** with `x.py` for `--host x86_64-unknown-linux-musl` and `-C target-feature=+crt-static`,
     `rustc.link-shared=false`, `llvm.link-shared=false`, `llvm.static-libstdcpp=true`. That is a stage-1 build of the compiler,
     plus LLVM (which the stage would build again unless `download-ci-llvm` fits). On this container it is several hours and
     about 30–40 GB. It does not fit the shared disk allowance, and it is not feasible inside arroyo on an image build.
   **Feasibility:** (b) is possible as a once-per-toolchain arroyo step on a dedicated build host, not here. (a) is cheaper if the
   shim grows a dynamic loader.
2. **Proc-macros.** A static rustc cannot `dlopen` a proc-macro `.so`. Either the shim gets a dynamic-loader path (PT_INTERP to
   `ld-musl`, ET_DYN at a chosen base, PROT_EXEC file mappings with W^X kept, `dlopen` working through it), or every crate built
   on UnaOS avoids proc-macros (no serde-derive, no thiserror). For the kernel's own crates, check what they pull: a
   `#![no_std]` kernel tree with no proc-macro dependencies would build with (1b) alone. That is the question to answer next, by
   listing the dependency graph.
3. **The driver chain.** rustc execs the linker. On UnaOS that becomes `-C linker=/apps/LLD.LNX -C linker-flavor=ld.lld` (plus
   `-Z unstable-options -C link-self-contained=+linker` when the flavor needs `-flavor gnu`, or a hard link named `ld.lld`). This
   arc's response file is exactly the line rustc would produce, so the linker half is proven to need nothing more.
4. **Signals at a fault.** rustc's `stacker` and std's overflow handler need SIGSEGV delivered at the faulting instruction onto
   the alternate stack (M1's owed item). Without it, a deep recursion in rustc dies as an ordinary SIGSEGV.
5. **Memory and time.** rustc on a small crate is about 300–600 MB resident: past the 96 MiB heap share, and so on the user frame
   pool, which is still unflown. With one CPU, codegen units bring no speed.
6. **The sysroot.** The full musl `rust-std` (35 MB of rlibs, with rmeta, which rustc DOES read, unlike lld) plus `rustc`'s own
   rlibs for the target go on the volume. Lazy file VMAs fault one page at a time with a `stat` each, so the shared page cache
   (still owed) becomes a speed item there.

The cheaper NEXT rung is to run the musl-host **rustc from (1a) behind a minimal ld-musl loader** (ET_DYN + PT_INTERP + a
handful of relocations, which musl's loader does itself once it is mapped). That one piece answers both (1) and (2).

## Not done / limits
- NEVER run (R76/R78: no QEMU). The kernel side is compile-proven only, on the x86 metal shape + linuxabi + selfdiag, ahciroot,
  btc, and on the aarch64 leg.
- Signal delivery at a fault (overflow handlers on the alternate stack) is owed. `mremap` does not answer the `old_len == 0`
  duplicate form or the brk heap.
- LLD.LNX is glibc-static, not musl, and was built once (its sha is not proven reproducible). About 68 MB of linker plus 16 MB of
  `/apps/LIB/rust` go on the ESP (`APPS/`), nothing in git (0 bytes of payload in the tree).
- The link's speed on UnaOS is unmeasured: the file VMAs fault through the VFS with a `stat` per page.
