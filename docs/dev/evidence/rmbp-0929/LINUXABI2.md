# LINUXABI2 — rung 2 of self-hosting: what a static busybox/musl shell needs (files, pipes, fork/exec/wait)

## Design
**Direction.** Peter: "self-hosting so we can build the OS with the OS". LINUXABI1 (`docs/dev/evidence/rmbp-0929/LINUXABI1.md`) loads ONE
static Linux ELF and answers ~25 syscalls; its "Design limits" list is this arc's work list: no stdin, no writes, no fork/exec/pipe, no getdents64.
Nothing in the boot-18 log bears on it (a feature arc, not a finding).

**Mechanism** (all under `unaos/crates/kernel/src/arch/x86_64/linuxabi/`):
- `mod.rs` — `LP` (one global process) becomes a PROCESS TABLE (`proc.rs` `TABLE`: `Arc<ProcInfo>` = pid/ppid/pgid/state atomics + `Arc<Mutex<LinuxProc>>`).
  The calling process is found by its CR3 (`sched::current_user_cr3`, each process owns one PML4). `dispatch` is a RETRY loop: `sys::handle`
  is a non-blocking try-op and returns `RETRY` (i64::MIN) to mean "would block"; `dispatch` drops every lock, `yield_now`s, re-runs it.
  So pipe read/write, stdin read, wait4, nanosleep, poll all block without holding a lock.
- `AddrSpace::fork_copy` (EAGER copy of every private data page, bounded 16384 pages = 64 MiB; there is no CoW fault hook in this kernel's
  page-fault path) and `AddrSpace::reset` (execve: same PML4 so `Task.user_cr3` stays valid; kernel half restored, user tables/frames freed).
- SYSCALL stub: two new template macros (`percpu.rs` tail, `linuxabi_save!`/`linuxabi_restore!`, EMPTY knob-off) push/pop rbx rbp r12-r15 beside
  rcx/r11/user-rsp on the task's kernel stack, so (a) `fork` can hand the child the parent's callee-saved registers and (b) `execve` can
  rewrite the return frame (rip/rsp/regs). Frame layout from `ktop` (= `gs:[KERNEL_RSP_OFFSET]` read first thing in `dispatch`): -8/-16 user rsp,
  -24 r11, -32 rcx(rip), -40 rbx, -48 rbp, -56 r12, -64 r13, -72 r14, -80 r15.
- Fork child: `spawn_user_preemptible(TASK_NAME, parent_rip, parent_rsp, this_cpu, child_pml4, kill)`; `user_task_trampoline` (sched.rs) asks
  `linuxabi::take_fork_regs(cr3)` and, when it hits, enters ring 3 through a second iretq block that loads rbx/rbp/r12-r15 and rax=0.
- FS_BASE: still per-core and not context-switched, so tasks stay PINNED to one core (the LINUXABI1 limit stays), but two processes now share
  that core, so the scheduler dispatch site calls `linuxabi::on_dispatch(cr3)` (lock-free table cr3 -> fs_base) and re-asserts FS_BASE per switch-in.
- Files (`sys.rs`/`fd.rs`): descriptors are `Arc<Desc>` (shared offset across dup/fork); kinds Console, File (slurped for reads, WRITE-THROUGH via
  the mount table), Dir (listing snapshot + synthetic mount-prefix dirs), PipeR/PipeW (`Pipe`: 64 KiB ring, reader/writer counts in `Drop`).
  Writes/creates/unlink/rename/mkdir only under the session's `/home/<user>/` (else -EACCES), principal = kernel like the editor's save.
- stdin: `read(0)` pops a LINE from `STDIN` (blocks via RETRY). The shell verb is `dispatch_command` inside `handle_key`, i.e. the render service is
  blocked in the verb, so the line editor is not running; the verb loop therefore PUMPS keys itself (`pal::pump_and_poll`, the seam `vug`/`pulse`
  use), edits a line (BS, Enter, Ctrl-C = kill session) and pushes it to `STDIN`. stdout/stderr go to serial AND an `OUT` buffer the verb drains
  into the shell window per completed line (a prompt with no newline appears when the line completes — documented limit).
- M3 fixtures `PIPE.LNX`/`LS.LNX`: crate `user-linux-hello` now writes all three (code assembled with `as`, embedded as bytes).

**Milestones.** M1 files/dirs/stdin/dup/pipe/poll/fcntl/etc; M2 fork/execve/wait4/kill/pgid/clone; M3 fixtures + `tests linuxabi2`.
No new knob (rides `UNAOS_LINUXABI=1`).

**Witness.** `:: LINUXABI2: fork_ok=<0|1> pipe_ok=<0|1> dents=<n> stdin=<0|1> -> PASS|FAIL ::` (SKIP when fixtures are not staged).
