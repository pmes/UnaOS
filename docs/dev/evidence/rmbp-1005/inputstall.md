# INPUTSTALL (rmbp-ledger B375) — R88 + VUGFITS, from flight 23's wire

Branch `exec-rmbp-inputstall`, cut from a60219de. Knob: none (rides `wc` + `beam`, both in the x86 metal line).

## Finding (read from the wire and the code, not flown)
1. **The key queue is the render task.** On the SCHED-X86 split, `input_service` (svc core) only forwards
   events into `GUI_CHANNEL_X86`; the ONE consumer is the `render` task (`x86_render_service`), which routes
   (`wc_route_event`), runs every kernel-surface handler (login `submit()` = KDF 250 ms + UnaFS adduser, the
   1.6 s / 2.2 s Enters), AND composites — and, holding `COMP_GATE` unmasked, it is the task that RE-RUNS the
   pass for every ring-3 present that folded behind it (`COMP_RERUN_MAX` rounds plus the trailing
   re-acquire). With six vugs folding presents into it, the render task composites on their behalf while a
   keystroke waits in the channel: `queue=2424.6`. The "lock the key route waits on" is the channel's single
   consumer being inside someone else's frame.
2. **The beam hold is per BAND, two frames each, IRQ-masked on syscall presents.** `beam::hold` brackets every
   band of every window/strip/fill present with a `GIVEUP_FRAMES = 2` budget; a banded present can spin
   several frames inside `COMP_GATE` (`BEAMHOLD: holds=600 held_us=3324041 gaveup=1 -> FAIL`, 5.5 ms per
   hold). A core spinning masked dispatches nothing: a vug worker released onto it does not start inside the
   parent's `BARRIER_SPIN_YIELDS` — that IS `strand=` (one frame in nine). Same minutes, same cause as (1).
3. **The setter's `comp=200–450 ms` per key is NOT named by reading.** For a kernel surface `comp` runs from
   the router's return to the next `wm::composite()` entry: the login handler's repaint + `wm::present`'s
   pre-composite path. M1 splits it (`draw=` handler→present request, `pre=` request→pass) so flight 24
   names it. A press that draws nothing (`control=none`) is charged until the next unrelated frame — an
   instrument limit, said here.

## The seam
Kernel compositor (`wm`/`beam`) and the instrument (`video/lag.rs`, CHARTER `Kernel — wm`). The vug's strand
count reaches the kernel through the profiler's existing verb (`SYS_PROF` op `OP_NOTE`, PROFILE2's syscall —
no new number): the app reports, the kernel lines it up. No new file, no new knob, no new store.

## Milestones
- **M1 chart** — `[lag] stall at_ms= span_ms= stage= stage_ms= queue= wm= app= comp= present= render=<route|handler|composite> render_ms= draw= pre= passes= pass_ms_max= rows= full= beam_ms= beam_max_ms= capped= reruns_yielded= valve= hid_gap_ms= strand=<s>/<frames>`
  once per second in which any stage, render phase or HID gap exceeds 50 ms; vug notes frames/strands via
  `SYS_PROF(OP_NOTE)`; `boot::hid_pass` feeds the per-second HID gap.
- **M2 beam** — one frame of beam wait per composite pass per core (all bands share it; `capped=` counts the
  bands that went unheld once it was spent); a stand-alone strip/fill bracket gets the same budget per
  two-frame window.
- **M3 input first** — the composite gate holder does not re-run folded presents while an input event waits
  for the render task (`reruns_yielded=`); the damage stays on the table (`COMP_PENDING`) and the next pass
  — the render task's own burst present right after the drain, or the folding app's next present — takes it.
- **M4 witness** — once a minute with input in it: `:: INPUTSTALL: key_queue_max_ms=<n> comp_max_ms=<n> hid_gap_max_ms=<n> strand_pct=<n> bound=50 -> PASS|FAIL ::`
  (PASS iff the three ms ≤ 50 and strand_pct ≤ 1).

## Witness lines flight 24 reads (x86 metal shape, no extra knob)
`[lag] stall …` (any second over 50 ms), `:: INPUTSTALL: … ::` (per minute of input), the existing `[lag]`,
`:: LAG:`, `:: BEAMHOLD:` (now with `capped=`), and the vug's `:: VUGART: … strand=` beside them.

## Owed
- Kernel-surface handlers still run on the render task: login `submit()` (KDF + adduser) and the window
  router's click work (`wm=1.4 s` clicks at 09:25) should run off it — needs the login state machine to
  advance from a worker; not done here.
- The setter per-key `comp=` cause — M1 names it on flight 24.
- `hid_gap_max_ms=3604` under the setter: M1's `hid_gap_ms=` puts the pump's gap beside the other columns.

## The seat's question (BRIGHTSLIDER's finding: bus/fs work on the render thread)
Read: per drained burst the render loop runs `instgui::service`, the dock's posted launches and, on its 5 s
clock, the load/smpload/rtwit witness lines; the net tick left it in GLASSLAG M2 (`net-tick` task, 5 s).
Nothing periodic at sub-second cadence writes the bus or UnaFS from it, so the sub-second strand stall is NOT
the render loop's own periodic work. A press handler that writes (the slider PrefSet BRIGHTSLIDER debounced,
`click→shown … wm=1471.4`) is exactly what `render=route|handler render_ms=` now names, per second, on the
stall line.

## As built
One commit carries M1–M4: the milestones share `lag.rs`, `beam.rs` and the `wm.rs` gate lines, so they were
written and compile-proved together. Files: `video/lag.rs` (stall line, render phases, input-first, the
minute witness), `video/beam.rs` (pass budget, `capped=`, per-second census), `video/wm.rs` (three same-line
folds: the `present` request, the re-run `while`, the trailing re-acquire; tail `inputstall_valve`),
`main.rs` (one same-line fold: `render_idle` at the park), `boot.rs` (`hid_pass` feeds `lag::hid_gap`),
`prof.rs` + `una-abi` (`SYS_PROF` op `OP_NOTE`, kind `NOTE_FRAME`), `user-vug` (`frame_note` per frame).

## Seat's answers (2026-10-05) and M4b–M5
(a) strand bound 1 %: kept. (b) **M4b**: before `phase=desktop` a stall second is counted, not printed; the first
roll at the desktop prints ONE `[lag] stall boot_suppressed=<n> worst_stage=<s> worst_ms=<n>`. (c) M3 accepted.

**M5 — input is never consumed by the task that runs handlers** (scoped to the two measured cases):
- **Login submit** (`video/login.rs`): Enter/the button copies the form, paints "Working...", and hands the slow
  half (`users::set_password_checked` / `installer_create_user` / `verify` / `login` / `login_root` — KDF,
  adduser, UnaFS home) to a `login-submit` kernel task on `smp::worker_cpu(0)` (never the render core). The
  worker only computes; its outcome is posted and the RENDER task applies it (same messages, take-down,
  session, installer advance) from `submit_drain` — on the 250 ms pulse (`main.rs` same-line fold beside
  `instgui::service`) or ahead of the next key. Keys during the submit are swallowed by the busy form; a
  worker silent past 15 s releases the form with a message. Headless forms (the `loginst` fixtures) and
  builds without a worker core run inline (`on=render`).
  Witness: `[login] submit kdf_ms=<n> adduser_ms=<n> on=worker input_blocked_ms=0 kind=<root|user|setpw|create|unlock>`.
- **Settings PrefSet** (`prefs_client.rs`, the seam — not `settings.rs`, which BRIGHTSLIDER owns on merge16):
  `sys_set` called from the render task queues the write (latest value per key) and a `prefs-flush` task on
  the worker core runs the PrefSets; `sys_get` answers a queued key from the queue (read-your-writes).
  Witness: `[prefsbus] flush n=<keys> ms=<n> on=worker queued_ms=<n> (INPUTSTALL M5 …)`.
- Owed (M6): the wm-stage clicks (`click→shown … wm=1445.6`), still inside `wc_route_event` on the render task.
- Flight 24 reads: across a login, `:: INPUTSTALL: key_queue_max_ms=<50 …`; `[lag] key→echo` for the Enter no
  longer carries the KDF (`comp=` ≈ one repaint).
