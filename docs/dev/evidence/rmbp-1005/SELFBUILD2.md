# SELFBUILD2 — threads, futex and the rest of the rustc surface for the Linux ABI shim (ledger B349)

## Design
**Finding.** SELFBUILD1 (B344) ran a static tcc on the shim and measured the host rustc: 64 distinct syscalls, of which the shim
answers 49. The unanswered ones are the threaded surface every modern toolchain needs: `clone3`/`clone(CLONE_VM|CLONE_THREAD)`
(-ENOSYS), `futex` (1593 calls), `statx` (152), `ftruncate`, `flock`, `eventfd2`/`epoll_*`, `socketpair`/`recvfrom`, `rt_sigreturn`.
The shim's process model keys EVERYTHING per address space (`ProcInfo` found by CR3, `FS_TAB`, the FXSAVE slot, `FORK_REGS`), so a
second task on the same PML4 had nowhere to keep its own FS_BASE, x87/XMM file or first-entry registers.

**Seam.** `CHARTER: Kernel — driver` (the compatibility box, as LINUXABI3's `fpu.rs` and SELFBUILD1's `sys2.rs`). No handler owns
the Linux ABI. New kernel files under `arch/x86_64/linuxabi/`: `thread.rs` (threads + futex), `sys3.rs` (statx, ftruncate,
fallocate, flock, eventfd2, epoll, socketpair, sendfile), `signal.rs` (per-process handler table, delivery, rt_sigreturn),
`selfbuild2.rs` (`tests selfbuild2`). Existing files get routing lines only.

**The thread key.** A thread is a scheduler task (`spawn_user_preemptible`, TASK_NAME, the process PML4, pinned to the session's
core). Every per-task table keeps its shape and is keyed by a THREAD KEY instead of the bare CR3: the leader's key is the PML4
(unchanged for every single-threaded process), thread *i*'s key is `PML4 + i` (the PML4 is page-aligned, so the low bits are free).
`thread::key_for(cr3)` maps the CURRENT scheduler task id to its key through a lock-free 32-entry table (the `FS_TAB` shape); the
three hooks (`on_dispatch` FS_BASE, `fpu::dispatch` FXSAVE slot, `take_fork_regs` first-entry registers) call it once. Process state
(`LinuxProc`: address space, fd table, brk/mmap) is shared because every thread's `cur_info()` resolves the same `ProcInfo` by CR3.

**Milestones.** M1 threads (clone/clone3 CLONE_THREAD, tid/tgid, set_tid_address, CHILD_CLEARTID wake, gettid, exit vs exit_group)
+ futex (WAIT/WAKE/WAIT_BITSET/WAKE_BITSET/REQUEUE/CMP_REQUEUE, timeouts off `arch::ms`). M2 statx, ftruncate/truncate/fallocate,
flock, eventfd2 + epoll, socketpair(AF_UNIX) + sendto/recvfrom, sendfile. M3 rt_sigaction table, rt_sigprocmask mask, delivery at
the syscall boundary through a 21-byte signal trampoline page, rt_sigreturn, rt_sigsuspend; `tests selfbuild2` (SYSKAT2.LNX + the
busybox probe). M4 this doc.

**Witness.** `:: LINUXABI-KAT2: threads=4 counter=400000 futex_waits=<n> epoll=ok statx=ok sigreturn=ok -> PASS ::` and
`:: SELFBUILD2: kat2=ok probe=ok hi=1 cat_lines=<n> probe_syscalls=<n> probe_missing=[] -> PASS ::`. No new knob — rides
`UNAOS_LINUXABI=1` (the x86 metal shape carries `linuxabi`).

**Owed (SELFBUILD3).** mmap-backed (streamed, not slurped) files and the 96 MiB per-process cap; a lazy/fault-in anonymous mmap.

## M1 — threads + futex (written; `thread.rs`, routing in `mod.rs`/`proc.rs`/`sys.rs`/`fpu.rs`/`sched.rs`)
- `clone(56)` with CLONE_THREAD and `clone3(435)` (CLONE_THREAD = thread, else the fork-shaped clone with the struct's fields):
  requires VM|FS|FILES|SIGHAND|THREAD (glibc's set adds SYSVSEM|SETTLS|PARENT_SETTID|CHILD_CLEARTID); CHILD_SETTID honoured;
  `stack == 0` = the caller's rsp; clone3's `stack + stack_size`. The child enters ring 3 at the caller's return rip with rax 0,
  the caller's rbx/rbp/r12-r15 AND rdi/rsi/rdx/r10/r8/r9 (the trampoline in `sched.rs` now loads 12 registers; a fork child's
  six extra are zero, so fork is unchanged) — glibc's `clone3` keeps the thread function in rdx and its argument in r8.
- Thread key `PML4 + i` (i in 1..31): `FS_TAB` (CLONE_SETTLS, or the creator's FS), an FXSAVE slot seeded from the creator's live
  registers (`fpu::fork_into(key)`), `FORK_REGS`. `key_for` is a lock-free 32-entry task-id table read at the dispatch site.
  At most 12 live threads per session (FS_TAB and the FXSAVE table are 32 entries, shared with up to 16 processes).
- `gettid` = tid (leader: pid), `set_tid_address` remembered per thread (a leader's before its first clone is carried over),
  `exit(60)` of a thread with live siblings: zero `clear_child_tid`, FUTEX_WAKE 1 on it, release the key, `sched::exit` — the
  process lives on; the LAST live thread's `exit` or any `exit_group(231)` ends the process (siblings killed at their next kill
  boundary). gc frees the address space only when every thread task is exited or reaped (`all_gone`); `kill`, a fatal signal and
  a ring-3 fault take the group; `tkill`/`tgkill` resolve a tid to its process; `execve` kills the other threads; `fork` gives the
  child the CALLING thread's FS_BASE. The nanosleep/poll/epoll deadline (`LinuxProc::sleep_until`) is swapped per thread.
- `futex(202)`: WAIT, WAKE (Linux's "at least one" for val <= 0), REQUEUE / CMP_REQUEUE (returns woken + requeued, as the host
  kernel did: 2 for wake 0 / requeue 2), WAIT_BITSET (absolute deadline), WAKE_BITSET; PRIVATE and CLOCK_REALTIME flags accepted
  (every clock id reads `arch::ms`, so absolute deadlines match `clock_gettime`). Waiters are RETRY loops keyed by scheduler task
  id; the value check and the enqueue run under the process mutex with IF masked, all threads on one core: no lost wake-up. Other
  ops (WAKE_OP, LOCK_PI, …) answer -ENOSYS with a `[linuxabi] futex op=<n> unanswered` line.

## M2 — the file / IPC surface (written; `sys3.rs`, one `fd::Kind::Ext` variant)
`statx(332)` (STATX_BASIC_STATS from the same VFS helpers as stat/fstat, mtime from the VFS stamp, AT_EMPTY_PATH on an fd),
`ftruncate(77)`/`truncate(76)` (through `MountTable::truncate`; a backend without in-place shrink gets a rewrite: empty + write the
kept prefix; grow zero-fills), `fallocate(285)` (mode 0 grows, KEEP_SIZE a no-op, punch/collapse -EOPNOTSUPP), `flock(73)` (advisory,
per open file description, keyed by VFS path, released with the description; LOCK_NB = -EWOULDBLOCK, else the call blocks),
`eventfd2(290)`/`eventfd(284)` (counter + EFD_SEMAPHORE/NONBLOCK/CLOEXEC), `epoll_create1(291)`/`epoll_create(213)`,
`epoll_ctl(233)` (ADD/DEL/MOD, EEXIST/ENOENT), `epoll_wait(232)`/`epoll_pwait(281)` (level-triggered over `poll_ready`; ONESHOT
disarms; EPOLLET served as level; timeouts per thread), `socketpair(53)` AF_UNIX (two crossed 64 KiB pipe rings; STREAM/DGRAM/
SEQPACKET all as a byte stream), `sendto(44)`/`recvfrom(45)` on those, and `sendfile(40)` (busybox `cat` — measured below; the
write body of `sys.rs` became `write_desc` so sendfile shares it).

## M3 — signals + `tests selfbuild2` (written; `signal.rs`, `selfbuild2.rs`)
`rt_sigaction` keeps a per-process 64-entry table (old written back; SIG_IGN drops a pending one), `rt_sigprocmask` a real mask
(per process), `kill`/`tgkill` to a catching process queue the signal instead of the default action, a child's exit posts SIGCHLD
to a catching parent, fork inherits table + mask, execve resets caught handlers. Delivery at every syscall exit: Linux's
`rt_sigframe` (pretcode = SA_RESTORER's restorer or the trampoline's own; `ucontext` with the full `sigcontext`; `siginfo`
SI_USER; a 512-byte FXSAVE image of the live x87/SSE file, `fpu::save_live`) below the red zone; the SYSCALL return frame is
rewritten to enter a 21-byte trampoline page at `STACK_TOP + 4096` (mapped R+X on first delivery) because the stub scrubs
rdi/rsi/rdx on the way out — `mov rdi,rbx; mov rsi,r12; mov rdx,r13; jmp r14`. `rt_sigreturn(15)` restores rip/rsp/rflags
(user-modifiable bits only), rbx/rbp/r12-r15, rax, the mask and the FP image (MXCSR masked to the CPU's MXCSR_MASK before
`fxrstor`, which would #GP at CPL 0). A blocking syscall is ended with -EINTR by a deliverable signal unless the handler has
SA_RESTART (then it keeps blocking and the handler runs when it completes — the stub zeroes rdi..r9, so a re-executed syscall
could not carry its arguments); `rt_sigsuspend(130)` and `pause(34)` always end -EINTR.

`tests selfbuild2` runs `SYSKAT2.LNX /apps/HELLO.C <size the VFS reports> <home>kat2.tmp` (20 s), then the busybox probe.

## Host proof (R78: the only executions)
- `SYSKAT2.LNX` (gcc 13.3 `-static -nostdlib -fno-pie -no-pie -O1 -ffreestanding -mno-red-zone`, 15024 bytes, sha256
  `f3c6e4e2b8e4bdb45abf320b09eb2b0a3e3620f12ece3e167166bb046b8cc314`) on the host Linux 6.18 kernel: `syskat2 ok threads=4
  counter=400000 futex_waits=<4..3523> epoll=ok statx=ok sigreturn=ok checks=26` on every run, all cores and pinned to one core
  (`taskset -c 0`: futex_waits 1..100 — the shim's shape). `strace -f`: clone x5, clone3 x3, futex 278, rt_sigaction 2, kill 2,
  rt_sigreturn 2 (one per delivery), statx 2, ftruncate 2, fallocate, flock 5, eventfd2, epoll_create1, epoll_ctl 2,
  epoll_wait 2, socketpair, sendfile, pread64.
- The probe, `busybox sh -c "echo hi; cat <HELLO.C>"` (PROBE.LNX = SELFBUILD1's busybox 1.37 glibc-static, sha256
  `3752671d…ed122871`), on the host: exit 0, prints `hi` then the 23 lines of HELLO.C. 49 syscalls after the first execve,
  22 distinct: brk 10, rt_sigaction 6 (ash installs SIGCHLD/SIGINT handlers, SIGQUIT ignored), prctl 3, set_tid_address/
  set_robust_list/rseq/readlinkat/prlimit64/newfstatat/mprotect/getuid/getrandom/arch_prctl 2 each, **sendfile 2** (`cat`; was
  unanswered by the shim before this arc), execve 2, write/uname/openat/getppid/getpid/close/exit_group 1. NOTE: ash `exec`s the
  LAST command of `-c` in place (`execve("/proc/self/exe", ["cat", …])`) — no fork and no wait4 on this exact command line; fork +
  wait + SIGCHLD delivery are exercised by SELFBUILD1's probe line (`echo probe | wc -c; …`, which now also gets ash's SIGCHLD
  handler run through rt_sigreturn, as the host does 5 times).

## What a metal boot should print (`./arroyo esp-x86`, metal shape with `linuxabi`, logged in as `<user>`)
```
tests selfbuild2
[linuxabi] thread pid=1 tid=2 key=+1 rip=<…> sp=<…> tls=<…>        (x8 over the session: tids 2..9)
[linuxabi] signal 10 pid=1 -> handler <…> frame=<…>                 (x2)
:: LINUXABI: path=/apps/SYSKAT2.LNX exit=0 syscalls=<~400> enosys=[] ms=<n> -> PASS ::
[selfbuild2] kernel: threads_spawned=8 futex_blocked=<n> signals_delivered=2 sigreturns=2
:: LINUXABI-KAT2: threads=4 counter=400000 futex_waits=<n> epoll=ok statx=ok sigreturn=ok -> PASS ::
[selfbuild2] linux /apps/PROBE.LNX sh -c "echo hi; cat /apps/HELLO.C"
:: LINUXABI: path=/apps/PROBE.LNX exit=0 syscalls=<~49> enosys=[] ms=<n> -> PASS ::
[selfbuild2] probe: hi
[selfbuild2] probe: /* SPDX-License-Identifier: GPL-3.0-or-later
:: SELFBUILD2: kat2=ok probe=ok hi=1 cat_lines=23 probe_syscalls=<~49> probe_missing=[] -> PASS ::
```
On a KAT failure: `:: LINUXABI-KAT2: … fail=<id> exit=<id> blocked=<syscall> -> FAIL ::` — the id is the check table in
`syskat2.c`'s header (19-21 need a writable `<home>`: no session user = `/home/` = refused writes). `tests linuxabi` (SYSKAT.LNX,
30 checks) and `tests selfbuild` are unchanged in their wire; SYSKAT's checks 18/19 now read the real table/mask (same answers).

## Owed
- **SELFBUILD3:** mmap-backed files (files are still slurped whole at open, shared descriptions see their own copy), lazy /
  fault-in anonymous memory and the 96 MiB per-process cap (`MAX_PAGES`; rustc + LLVM map hundreds of MiB), file-backed MAP_SHARED.
- Per-thread signal masks and thread-directed signals (`tgkill` to one thread delivers to whichever thread of the process syscalls
  first); queued real-time signals; sigaltstack delivery; a blocked SIG_DFL signal is not held pending; delivery only at a syscall
  boundary; `futex` WAKE_OP / PI ops; futexes shared across fork; EPOLLET edge semantics; `shutdown`/`sendmsg`/`recvmsg`;
  SA_RESTART restarts by not interrupting.
- Then (SELFBUILD1's list): a static linker on the volume and a static rustc (needs a musl host toolchain to build).

## Not done / limits
- NEVER run (R78: no QEMU). The host runs are the only executions; the kernel side is compile-proven (x86 metal shape with
  `linuxabi`, aarch64 leg).
- All threads of a session are pinned to the verb's core (FS_BASE / FXSAVE are per-core state; same as LINUXABI2): threads give
  rustc concurrency, not parallelism, until the shim can migrate a Linux task.
