# DOCKSHELL2 — no shell window for a GUI action (rmbp-ledger B621, R115)

Branch `exec-rmbp-dockshell2`, parent `d616bf98`. Only file changed in code: `unaos/crates/kernel/src/video/dock.rs`.

## The wire (docs/dev/evidence/rmbp-0915/flight34/f34-boot1.log)

```
[15:05:31Z] [dock] press at (700,1698) tile=2/11 app=settings -> launch verb-posted
[15:05:31Z] [dock] launch app=shell by=x86_render_service win=2 gen=3 tries=1 -> LAUNCHED
[15:05:31Z] [dock] dockpin verb settings
[15:05:31Z] [shelltask] post verb=settings -> render (window)
[15:05:31Z] [dock] tile add win=3 gen=1 owner=0xffffff07 seq=7 label=Settings
[15:05:31Z] [dock] census tiles=12 win:gen=1:9,3:1,activity:pin,settings:pin,...
[15:05:32Z] [settings] open win=3
[15:05:41Z] [dock] launch app=settings first_window_ms=timeout bound_ms=10000
[15:05:41Z] [notify] show ... title=Dock line=settings did not open a window in 10 s.
```

Verdict on the brief's question: the verb RAN and the window OPENED (`[settings] open win=3`, 1 s after the press).
Nothing was lost in the drain. The timeout came from the dock's first-window join, not from the window. Settings and
Activity are KERNEL windows (`settings.rs:43` and `activity.rs:31` both set `OWNER = KERNEL_OWNER_BASE + 7`). The pins'
`DpKind::Ring3Program` join (`dp_owner_is`'s `_` arm: `DP_PIN_OWNER + i` or `DP_PROG_OWNER[i]`) is filled by
`dp_refresh`, and `dp_refresh` skips kernel owners (`wm::is_kernel_owner`). So `launch_landed(4)` could never be true.
The census still shows `settings:pin` beside the live `3:1` window, and the bound raised a false "did not open" notice.

## GUI actions converted

| action | before (wire) | after (wire) |
|---|---|---|
| dock tile Settings | `[dock] press … app=settings -> launch verb-posted`, `[dock] launch app=shell … LAUNCHED`, `[dock] dockpin verb settings` | `[dock] press … app=settings -> launch direct-posted`, `[dock] direct app=settings window=settings by=x86_render_service open_ms=<n> -> opened`, `[dock] launch app=settings by=dock direct=1 first_window_ms=<n>` |
| dock tile Activity | `… app=activity -> launch verb-posted` + shell LAUNCHED + `dockpin verb activity` | `… app=activity -> launch direct-posted`, `[dock] direct app=activity window=activity by=… -> opened`, `[dock] launch app=activity by=dock direct=1 first_window_ms=<n>` |
| login items `activity` / `settings` (`loginitems.rs` -> `launch_named` -> `dp_launch`) | `[login] item launch name=settings how=verb-posted` + a shell window | `[login] item launch name=settings how=direct-posted`, no shell |
| launcher pick of a table app (`launch_named`) | as the tile | as the tile |
| system menu Force Quit (`sysmenu.rs:212` `post_line_launch("activity")`) | `[dock] line launch posted line=activity` + shell LAUNCHED | `[dock] line launch line=activity app=activity -> direct-posted`, then `[dock] direct app=activity … -> opened` |
| launcher pick of a program path (`launcher.rs:846` `post_line_launch(path)`) | `[dock] line launch posted line=/apps/X.ELF` + shell LAUNCHED | `[dock] line launch line=/apps/X.ELF -> direct-posted`, `[dock] direct app=X.ELF path=/apps/X.ELF … -> started` (a pin's path takes the pin's own direct arm) |

Already direct, not changed: the dock tiles lumen/aether/midden/una/stria (DOCKSHELL B555), editor
(`textedit::request_open`), quarry, console. Settings from the system menu (`sysmenu.rs` `open_settings` ->
`settings::request_open`) was already direct. `menubar.rs` has no launch arm (the system menu's items live in
`sysmenu.rs`), so it is unchanged. `desktop.rs` does not exist. The desktop's double-click opener is Quarry's
`live.rs` launch. A windowed program detaches there, and Quarry posts `run <path>` only for a CONSOLE program, which
needs a console. That line and a console-program pin's fallback are the two shell-seam uses left, kept on purpose.

Still reads "line-posted" (not in this arc's files): `launcher.rs:846` builds its own `how` word
`line-posted path=…` from `post_line_launch`'s `true`. Exact change for its owner: `"direct-posted path={} origin=glass"`
when the path starts with `/` and has no whitespace, otherwise keep the old word.

## DOORSPLIT

The door's route is chosen PER BYTE, not per line. `wc_route_event` (syscall.rs:7312) prints
`serialdoor::route_word()` for each key, and that reads `SHELL_WIN`, which the render pass refreshes every pass. The
render key arm hands each byte to whichever console exists at that byte: the shell window's `shell_console`, or
`serialdoor::key`'s headless `HEAD` console. Flight 34 at 15:05:38 shows this: four printable bytes go
`-> shell(window)`, then the dock-opened shell window closes (`[wm-act] close-furniture win=2`, `[dock] quit app=shell
… TORN-DOWN`), and the rest of the line and its Enter go `-> shell(headless)`, which runs `verb=una` from only the
second half of the typed line.

After DOCKSHELL2 no GUI action opens a shell window, so the flight-34 cause is gone: a shell the dock opened for a verb
and then closed mid-line. DOORSPLIT CAN STILL OCCUR whenever a shell window the OPERATOR opened (the shell tile, Cmd-N,
a quit) appears or disappears while a door line is half typed. The exact change, in `serialdoor.rs` and `main.rs`
(neither is this arc's file):

1. `serialdoor.rs`: `static LINE_AT: AtomicU8` (0 idle, 1 window, 2 headless), latched from `SHELL_WIN` on a line's
   FIRST byte (when it is 0) and cleared on CR/LF/^C. `route_word()` returns the latched route while a line is open.
2. `main.rs` (the render key arm that consults `take_door`): a door byte goes to the latched console. If the latched
   route is `window` and the shell window is gone, the window console's `current_input` is moved into the headless
   `HEAD` console (`serialdoor::adopt(&str)`, a new fn) before the byte is keyed. A line that started headless stays
   headless when a window appears.
