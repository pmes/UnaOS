// SPDX-License-Identifier: GPL-3.0-or-later
// SELFBUILD6 (B360): what GCC's libgcc_s.so.1 exports and LLVM libunwind (relinked -shared as libgcc_s.so.1) does not, as
// the musl rust-lld needs it:
// * `__popcountdi2@GCC_3.4` — imported strongly (libgcc.a's copy is hidden); clang lowers the builtin inline.
// * `__register_frame_info` / `__deregister_frame_info@GCC_3.0` — crtbegin's frame_dummy in a NON-PIC executable takes
//   their canonical PLT address (never 0) and calls it, so a weak-undefined one jumps to 0. libunwind finds every FDE
//   through dl_iterate_phdr + PT_GNU_EH_FRAME, so registering is a no-op here.
int __popcountdi2(long a) { return __builtin_popcountl(a); }
void __register_frame_info(const void *eh, void *ob) { (void)eh; (void)ob; }
void *__deregister_frame_info(const void *eh) { (void)eh; return 0; }
