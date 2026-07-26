# UI Guidelines: Functional Beauty

## 1. The "16ms" Rule (60fps)
Responsiveness is the #1 feature.
* **The Promise:** If a user clicks, the screen MUST update within 16ms. Even if the app is frozen, the window manager must respond (move, minimize, close).
* **Implementation:** The Window Server runs on a dedicated high-priority thread (Real-Time Class). It never waits for an application to finish thinking.

## 2. Information Density (The "Data" Aesthetic)
We reject modern "white space" trends. We prefer **High Signal-to-Noise Ratio**.
* **Tabs:** Like BeOS, windows use distinctive tabs that are easy to grab.
* **Metadata First:** In the file browser, we don't just show icons. We show resolution, frame rate (for videos), and EXIF data (for photos) directly in the list view.
* **Typography:** We use a custom, high-legibility monospace font for system data (like `JetBrains Mono` or `Fira Code`) to emphasize precision.

## 3. The "Workspace" Metaphor
unaOS is a workbench, not a consumption device.
* **Spatial Organization:** Windows remember exactly where you put them. If you leave a text editor in the top-right corner, it stays there after reboot.
* **Virtual Desktops:** Deeply integrated. One workspace for "Kernel Dev," one for "Music," one for "Communication."

## 4. Dark Mode by Default (Ecology)
* **OLED Black:** The default theme uses true black (`#000000`) to turn off pixels on OLED screens (Pixel 10, modern laptops). This saves energy.
* **Accent Colors:** Used strictly to indicate status (Green = Good, Yellow = Busy, Red = Error). No decorative colors.

## 5. The App-Input Watchdog (never trap the keyboard)
A concrete enforcement of the §1 promise "even if the app is frozen … the window manager must respond." While a full-screen app owns the screen (the GUI-CLICK-2 `SCREEN_APP_ACTIVE` gate in `main.rs`), the Pi input router stops forwarding events into `GUI_CHANNEL` and leaves them in `pal::EVENT_QUEUE` for the app's own drain. That gate is correct while the app is *live*, but on its own it has no escape hatch: a wedged app would trap the keyboard until reboot.

`kernel/src/gui_watchdog.rs` is that escape hatch — a self-contained, both-arch state machine (no dependency on the router internals in `main.rs`/`pal.rs`):
* **Mode-transition witnesses** — `[gui] app-enter t=<s>s` and `[gui] app-exit t=<s>s dur=<s>s wedged=<bool>`, timestamped from the monotonic `clock::uptime_secs()` seam, make every screen hand-off self-dating on serial.
* **Liveness heartbeat** — the active app's drain loop calls `note_progress()` each pass.
* **The watchdog** — `poll()`, run on the status/pump cadence, returns `true` (and prints a latched `[gui] watchdog app wedged <n>s … — returning input to shell`) once the app has made no drain progress for `WATCHDOG_TIMEOUT_SECS` (5 s). The caller clears `SCREEN_APP_ACTIVE`, and the router resumes delivering input to the shell.

The state machine and witnesses landed with the module (GUI-CARRY); the call-site hooks are now wired (GUI-WIRE): `on_app_enter`/`on_app_exit` bracket `dispatch_command` in `main.rs` `handle_key`, `poll()` runs on the 1 Hz `status_tick` cadence (clearing `SCREEN_APP_ACTIVE` when it fires — the escape hatch), and `note_progress()` runs once per drain pass in `pal::pump_and_poll`. A healthy `vug` enter/exit prints `[gui] app-enter`/`[gui] app-exit` on serial; no watchdog fires during a live session.
