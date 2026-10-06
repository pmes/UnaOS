# SHELLTASK2 (rmbp-ledger B474) — Ctrl-C to the shell job, the dock and `run` on the task

**Finding (flights 24/25, the wire).** `tests play flac` → `[play] dec spawn … jid=1` and nothing more; the next
typed lines reached the door (`[serialdoor] key=printable … -> shell`) and never ran. Since SHELLTASK (B458) that
wedge holds only the `shell-job` task, but nothing reaches the task: Ctrl-C (`shellux` 0x03) only clears the edit
line ("the shell tracks no foreground child at this seam"). The DOCKPIN verb arm (`main.rs`, beside `S_SHELL`)
and `run`/exec still dispatch on the render task; a foreground `run` holds it up to 5 s
(`run_user_image_argv`'s yield loop).

**The seam.** The kernel's own kill: the job is spawned with a TEARDOWN-1 `KillSwitch`
(`sched::spawn_stack_killable`, the `spawn_inner` path every kernel task takes, the switch the scheduler already
honours at a preemption, a yield and a sleep park). Ctrl-C / Cmd-. (`Action::Interrupt`, ⌘. on the CRISPY table)
on the shell window arms it; the render pass (`shelltask::service`) waits for `is_reaped`, then frees what the
dead task held the STACKGUARD2 way — `stackguard::release_held` (sink, UART, UnaFS) then LOCKREG's
`sync::release_task` — drains the transcript the task wrote, clears `busy`, KEEPS the queued lines, prints `^C`.
A foreground ring-3 program is the job's child: Ctrl-C kills the CHILD (`syscall::fg_interrupt`, the `run`'s own
`KillSwitch`) and the job returns normally. A job parked where no kill boundary reaches (a kernel `Semaphore`) is
ABANDONED after 2 s: named (`sync::name_task`), its generation retired (it never pops another line), the shell freed;
its kill stays armed, so it retires at its next boundary.

**Routing.** `run`, `bg`, `jobs`, `kill`, `storm` and any word that is not a verb (`midden_core::is_verb` — the
bare-name/program-path exec) go on the task: their waits are the task's own. DOCKPIN lines go through the task's
queue (`submit_glass`, origin Glass); a glass line whose verb opens a kernel window (`activity`, `settings`, `edit`,
…) is POSTED back to the render pass (`RENDER_POST`), the way the transcript is; a program path runs on the task.

**Audit — what stays on the render task, one line each.** `view`/`edit`/`dialog`/`activity`/`settings`/`top`/
`batmon`/`wallpaper`/`screenshot`/`shot`: they build kernel windows whose state the render pass owns. The input
band (`wc_route_event` → a focused ring-3 window's queue): it never waits on a program (non-blocking, LOCKFIX-B1),
nothing to move. `login`/`passwd` prompts: the prompt is the render task's key path. Bare-name `cd`/`clear`/
`history`: the view's own state.

**Milestones.** M1 the kill seam (`spawn_stack_killable`, `syscall::fg_*`), the abort in `shelltask`, the Ctrl-C and
⌘. hooks. M2 DOCKPIN + `run`/exec routing (`submit_glass`, `RENDER_POST`). M3 `tests shelltask2`.

**Witness.** `[shelltask] abort verb=<v> tid=<t> after_ms=<n> locks_released=<n> queued_kept=<q> how=task|child|abandoned`;
`[shelltask] dockpin verb=<v> -> task queued=<q>`; `[shelltask] post verb=<v> -> render (window)`.
`tests shelltask2` → `:: SHELLTASK2: abort=ok locks=0 queued_kept=<n> dockpin=task run=task -> PASS ::` (a scratch
`shell-job` holding a lock and spinning is aborted through the same path).
On metal: type `tests play flac`, then Ctrl-C → `^C` and the abort line; the next typed line runs.

**Owed.** The aborted job's heap (its console, its 16x16 pal) is leaked, not freed (a killed task runs no `Drop`).
The global `origin` scope a killed job swapped is not restored. `play-dec` itself is not stopped by the abort
(DECJOBHANG/DECSTALL own the decoder). aarch64 and the backdrop console are unchanged.
