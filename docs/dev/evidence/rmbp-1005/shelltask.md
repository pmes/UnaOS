# SHELLTASK (rmbp-ledger B458) — the shell off the render task

**Finding (PERFREVIEW F1, the flown wire).** `main.rs` `handle_key` runs `shell::dispatch_command` inline on the
render task (x86 `x86_render_service`, the shell window's console), and the dock's verb arm does the same. Every
typed `tests` verb holds the compositor: flights 24/25 carry 21 `[lag] stall … render=handler` seconds (64.5 s),
key-to-echo mean 237 ms (max 2960 ms). DECJOBHANG's wedge (`tests play flac`) took the render handler with it.

**The seam.** The render task composes; the shell runs on its own kernel task (`shell-job`, the DECJOB shape:
`spawn_stack` with its own measured stack under STACKGUARD2's guard page, on a worker-pool core that is neither
the render core nor the BSP, every lock a `sync::Mutex` so LOCKREG names it). The transcript is the seam: the
task's `Console` is a PRODUCER console (`task_out`) whose `println` lands in `shelltask`'s line queue
(`SHELL_OUT`, the termring contract — the view's owner drains — with a task-side producer that may wait, so it
backpressures instead of dropping); the render pass drains it into the shell window's console with `try_lock`
(never waits on the shell) and paints. A line is routed when its verb is on the task list (`ROUTED`: the long
console-only verbs — `tests`, the file, net, census/prof/play/wifi/linux verbs); a line that needs the view or the
glass (`clear`, `history`, `selftest`, `login`/`passwd`, window openers, ring-3 `run`/exec) stays on the render
task as today. While a job runs, typed lines queue (FIFO, 16) and run in order; a queued render-bound line is
dispatched by the render pass when the task is idle. A fixture that touches the glass calls the compositor the way
a ring-3 syscall does (`wm::composite`'s `COMP_GATE`), never draws on the task's pal (a 16x16 heap throwaway, the
`witness_capture` shape).

**Milestones.** M1 `shelltask.rs` (queue, task, producer console, render-side drain) + the `Console` field.
M2 the hooks: `handle_key`'s CR arm (same-line fold), the render pass's drain beside `S_SHELL`, `lag.rs`'s
handler-stall counter. M3 `tests shelltask`.

**Witness.** Per routed line: `[shelltask] line verb=<v> job=<n> cpu=<c> key_us=<n> queued=<q>` and
`[shelltask] done verb=<v> tid=<t> ms=<n> lines=<n>`. `tests shelltask` (on the task itself, spinning 1.5 s like a
long verb): `:: SHELLTASK: shell_task=<tid> render_tid=<tid> render_stalls=0 render_passes=<n> key_us=<n> -> PASS ::`.
On metal, with a typed `tests usbnet7`: no `[lag] stall … render=handler` line beside it.

**Owed.** The dock verb arm (`main.rs` DOCKPIN) still dispatches inline (a pinned verb is a short launch). Ring-3
foreground `run`/exec, window-opening and prompting verbs stay on the render task. A wedged job (DECJOBHANG) now
wedges only the shell: queued lines wait; no Ctrl-C to the task yet. aarch64 and the backdrop console are unchanged.
`[perf] key_us` (PERFREVIEW's line) is not on this tip; `key_us` here is the Enter key's cost on the render task.
