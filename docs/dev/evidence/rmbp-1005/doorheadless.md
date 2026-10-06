# DOORHEADLESS (rmbp-ledger B487) — the serial door's headless shell

**Finding (flight 26 boot 2, `f26-boot2.log`).** 62 `[serialdoor]` lines, every one `win_focus=0x0 ring=0x0 -> shell
(the wire is a console)`, zero `[midden]`. `wc_route_event` (arch/x86_64/syscall.rs) hands a door byte back as a plain
`Key`; `x86_render_service`'s key arm (main.rs) gives it to `handle_key` only `if shell_id != WIN_NONE` — on the bare
desktop (R88) there is no shell window, so the byte was dropped with no word on the wire. Boot 3 had a window
(`win_focus=0xffffff02`) and the read list ran.

**Seam.** SHELLTASK's job task and transcript queue (B458/B474, `shelltask.rs`) are the shell; the door gets its own
CONSOLE, not a second shell: `serialdoor.rs` holds one headless `Console` (marked in-window so `shelltask::submit`/
`interrupt` take it; `set_output_sink` = the wire) and a 16x16 scratch pal (the job's own shape). A door byte with no
shell window goes to `serialdoor::key` (the line editor: printable, Backspace, Ctrl-C, the login prompt); Enter runs
the line exactly as the window's Enter does — a routed verb on the job task, the rest inline on the render task — and
`serialdoor::service` (each render pass, no shell window) drains the job's transcript through `shelltask::service` into
that console, whose sink prints it on the wire. A shell window, when one exists, keeps taking the door's lines.
No new knob; no new verb (`tests door` is a fixture under the existing `tests`).

**Milestones.** M1 the design. M2 `serialdoor.rs` + the route (wc_route_event says `-> shell(window)` /
`-> shell(headless)`; main.rs's dropped arm calls `serialdoor::key`; the render pass calls `serialdoor::service`).
M3 `tests door`.

**Witness (the next flight reads).** Bare desktop, typed on the wire: `[serialdoor] key=printable … -> shell(headless)`,
on Enter `[serialdoor] line verb=<v> -> shell(headless) on=<task|render>`, then `:: [midden] cmd="…" ::` and the
transcript lines; `tests door` → `:: DOOR: headless=1 ran=1 window=0 -> PASS ::`. With a shell window: `-> shell(window)`.

**Owed.** A door line that takes the screen (the pager, `vug`) has no glass to take headless — it runs on the scratch
pal and waits for keys the door then supplies; the door does not echo typed bytes (the window paints them; the wire
stays quiet, the `[serialdoor] key=` witness is the echo). x86 only (aarch64 has no `wc` shell window to lack).
