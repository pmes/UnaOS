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

## Written
Boot 17 (x86, `UNAOS_LINUXABI=1`, builder-staged media: `APPS/{HELLO,PIPE,LS}.LNX`), desktop shell `tests linuxabi2`:
```
[linuxabi] load path=/apps/PIPE.LNX segs=1 entry=0x400078 ...
ping
[linuxabi] load path=/apps/LS.LNX ...
:: LINUXABI2: fork_ok=1 pipe_ok=1 dents=<n>=3+ stdin=1 -> PASS ::
```
(`-> SKIP (fixtures not staged)` when APPS/ lacks them.) `fork_ok` = >=1 fork and the child exited 0 having seen the parent's rbx/r12 canaries;
`pipe_ok` = parent read "ping\n" through the pipe, wait4 status 0, exit_group(0); `stdin` = the line pre-queued with `stdin_push` came back
first on stdout; `dents` = names `getdents64("/")` returned (incl `.`/`..`). Interactive: `linux /apps/LS.LNX` then type a line + Enter.
`tests linuxabi` (HELLO) is unchanged. No new knob, no spec pin (x86-wc.spec does not pin `tests linuxabi`).

**Not done / limits.** Never compiled or run (R76/R78). Work-list items not done: `select`, `ftruncate`, `readlinkat`, `futex`, `rt_sig*` delivery
(kill = end the task), real threads (`clone` with CLONE_VM|CLONE_THREAD -> -ENOSYS), `#!` scripts in execve, `/proc`, symlinks, `getrandom` is a
clock xorshift (not crypto). fork is an EAGER copy (<=64 MiB), not CoW — there is no CoW fault hook in `page_fault`. The FS_BASE/pinning limit stays:
FS_BASE is per-core, so every process is pinned to the verb's core; `sched.rs` now re-asserts it per switch-in via `linuxabi::on_dispatch`.
Ring 3 has no SSE (CR4.OSFXSR=0): a stock musl/busybox build (SSE2 memcpy/float) will #UD until that is lifted or the toolchain targets `-mno-sse`.
stdout reaches the shell window per completed LINE (a bare prompt appears when its line completes); stdin echo is likewise per line.
Files are slurped at open (a 100 MB file costs 100 MB); writes are write-through per 4 KiB through `MountTable::write` with the kernel principal,
only under `/home/<user>/`. A failed `execve` AFTER the point of no return returns into unmapped memory (SIGSEGV zombie).

## Notes for the compiler executor (guessed signatures / cfgs)
- `syscall.rs` stub: `crate::linuxabi_save!()` folded onto the `linuxabi_r8!` template line, `crate::linuxabi_restore!()` onto the `call {dispatch}` line;
  macros at `percpu.rs` tail (multi-line string literals "push rbx\npush rbp..."). If `global_asm!` rejects a macro-produced multi-line literal, split into six macros.
- `sched.rs`: `user_task_trampoline` fork-child asm block (explicit `in("rax")`, `in(reg)` operands consumed by the pushes before rbx..r15 are loaded) and the
  `{ ... on_dispatch(uc) ... }` fold on the `target_cr3` line. `take_fork_regs` returns `Option<[u64; 6]>`.
- Guessed APIs: `crate::arch::sched::{current_user_cr3, KillSwitch::{new,request,is_reaped}, exit, yield_now, kill_check_current}`, `crate::pal::{pump_and_poll, Event::Key(u8)}`,
  `crate::shell::{cwd_now, vfs_path, vfs_mount_table}`, `MountTable::{stat,read,read_dir,create,write,unlink,remove_dir,rename,prefixes}`, `fs::users::whoami` under `feature = "login"`.
- `percpu::KERNEL_RSP_OFFSET` as an asm `const` operand (same shape as `USER_RSP_OFFSET`).
