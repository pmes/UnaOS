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
