# LINUXABI1 — run a static Linux x86_64 ELF as a ring-3 program (self-hosting rung 1)

## Design
**Direction.** Peter: "self-hosting so we can build the OS with the OS and then boot the OS"; ROADMAP §1c SH-5 "requires a native
toolchain story". The realistic road is a Linux-syscall compatibility layer running musl-static busybox, then a static toolchain.
This is rung 1: load + a first syscall table + a verb. Nothing in the boot-18 log bears on it (a feature arc, not a finding).

**How UnaOS runs ring-3 today.** `syscall.rs:17017` `spawn_user_image_bg_inner` -> `load_program_common` -> `elf::map_image_into_slot`
puts the image in a static SLOT (`memory.rs:937 alloc_user_space`): ONE 16 KiB window at USER_BASE (1 TiB, PML4[2]) + the FB hole, so a
Linux binary linked at 0x400000 with a real stack/heap cannot live there. `sched::spawn_user_preemptible(name, entry, rsp, cpu, cr3, kill)`
(`sched.rs:2631`) takes ANY cr3, and `free_user_space_by_cr3` ignores a cr3 that is not a slot root, so a private PML4 is supported.
The SYSCALL stub (`syscall.rs:2169`) hands `syscall_dispatch` 5 C args (nr, a0..a2, a3=r10) and clobbers r8 with r10; r9 survives.

**Mechanism** (`arch/x86_64/linuxabi/`):
- `mod.rs` `AddrSpace`: own PML4 = copy of the kernel's (PML4[2] zeroed); `leaf_slot` clones/splits kernel tables on write so the ELF can
  overlay 0x400000 while the kernel half stays shared. All user memory access is a SOFTWARE walk (`copy_in`/`copy_out`: page must be
  present+USER(+W), touched through its identity frame) -> bad pointer = -EFAULT. `run_path` loads, spawns task `linuxabi` PINNED to the
  calling core (FS_BASE is per-core and not context-switched), waits (DONE/FAULT/deadline), frees.
- `elf.rs`: ET_EXEC/EM_X86_64/no PT_INTERP; segments < 16 MiB and `region_is_usable` (they REPLACE the kernel's identity view under that CR3);
  W+X refused; System V stack (argc, argv, envp, auxv AT_PHDR/PHENT/PHNUM/PAGESZ/BASE/FLAGS/ENTRY/UID../CLKTCK/SECURE/RANDOM/EXECFN/NULL), 16-aligned.
- `sys.rs`: the Linux table (read write open openat close fstat newfstatat lseek mmap mprotect munmap brk rt_sig* ioctl writev getpid gettid uname
  getcwd readlink* arch_prctl set_tid_address clock_gettime get*id; exit/exit_group in `mod.rs`). Unknown -> -ENOSYS + once-per-nr `[linuxabi] enosys nr=`.
- Wire: `syscall_dispatch` (signature line) asks `is_linux_task()` first; the stub parks user r8 via the `linuxabi_r8!` template macro (`percpu.rs`
  tail; empty line knob-off => stub byte-identical); `record_ring3_kill` tells the verb about a fault; `shell.rs` `linux` arm; `tests.rs` registers `linuxabi`.

**Milestones.** M1 loader+stack+entry (`[linuxabi] load path= segs= entry= stack= brk=`). M2 syscalls. M3 verb + fixture `HELLO.LNX`
(crate `user-linux-hello`, a host generator writing 174 bytes; builder stages it into APPS/ beside VUG.ELF). All in one commit-set.

**Knob.** `UNAOS_LINUXABI=1` -> cargo feature `linuxabi` (three places: `arroyo`, `builder/src/main.rs`, `scripts/banner-cert.sh` row + `k8-reach.registry` NA row).

## Written
Boot 17 (x86, `UNAOS_LINUXABI=1`, builder-staged media), shell `linux /apps/HELLO.LNX` or `tests linuxabi`:
```
[linuxabi] load path=/apps/HELLO.LNX segs=1 entry=0x400078 stack=0x1007ffffxxx brk=0x100010000000   (path as the VFS resolves it)
hello from linux abi
:: LINUXABI: path=/apps/HELLO.LNX exit=0 syscalls=2 enosys=[] ms=<n> -> PASS ::
```
Spec pin: NOT added to x86-wc.spec — staging of HELLO.LNX into the test lane's data volume is unverified (the builder stages it into the ESP APPS/;
`tests linuxabi` prints `-> SKIP (fixture not staged)` when absent). Pin `:: LINUXABI: .* exit=0 ` once a lane shows it staged.

**Not done / limits.** musl-static busybox untried (needs clone/futex-free paths only; getdents64, readv, access, dup, pipe, nanosleep, getrandom,
prlimit64, sched_yield, madvise, set_robust_list, rseq will show up as `enosys nr=` — that list is the next arc's work list). No stdin, no file writes,
no fork/exec/threads/signals. File-backed mmap needs user r8 (arg 4) from the stub scratch. `linuxabi` page-table copy of low memory does not see kernel
mapping edits made while the program runs.
