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

## 6. The serial console always reaches the shell (SERIAL-FOCUS, the source split)
§5 promises the keyboard is never trapped by a *wedged* app. This is the companion promise for the *wire*: **input arriving over the serial line reaches the shell regardless of GUI focus**, and it does so without touching the ruling that a focused EL0 window owns the USB keyboard.

**The blocker.** On the bare-metal Pi, serial RX had exactly one destination — `main::input_service` posted each byte as an `Event::Key` into `GUI_CHANNEL`. `GUI_CHANNEL`'s only consumer is `render_service`, which parks inside `handle_key -> shell::dispatch_command` for the whole life of a foreground command, and `run <elf>` — the call that hands an EL0 window the keyboard via `user_input_set_active` — is one of those commands. So in precisely the state that matters the channel's consumer is asleep: the first 64 bytes queued where nothing would read them, and the 65th blocked the input task inside `Channel::send`, a semaphore wait with no deadline. There was a second door too — `pal::pump_and_poll`'s aarch64 arm put serial bytes into `pal::EVENT_QUEUE`, a second reader of the one PL011 FIFO, where a byte is indistinguishable from a decoded USB HID key by the time it meets the `[uvug9]` routing decision and is handed to `route_input_to_active_el0()`.

**The design: split by SOURCE, by construction rather than by a predicate.** There is no `source` field on `pal::Event`. A tag would be a thing every future router has to remember to test, and the router that forgets is a regression nobody sees until the bench. Instead the serial byte is **consumed before the focus decision**, into a carrier the focus decision cannot reach:

* `arch::aarch64::serial::shell_inbox` — a 512-byte bounded ring (bare-metal only), MPSC, no heap, `offer` total and non-blocking, drop-newest-and-count on overflow so what is delivered is always an exact in-order prefix of the arrival stream.
* `main::serial_to_shell` — the producer. `input_service` calls it instead of `gui_send`. `GUI_CHANNEL` now carries **no serial payload at all**; at most one *coalesced* wake token rides it, and only when a headroom check proves the `send` cannot block. A serial storm therefore cannot jam the GUI channel — the storm never travels on it.
* `render_service` — the consumer, drained through the same `handle_key` a USB keystroke reaches, into whichever surface SHELLWIN-PI's `windowed` predicate says the shell is on. The drain sits **after** the `match`, so the pass that returns from `dispatch_command` takes the whole backlog before parking: a command typed over the wire while an app owned the panel executes the instant that app exits.

USB HID keeps `EVENT_QUEUE` and every line of its focus routing untouched — this arc adds zero lines inside `pump_usb_into_gui`'s routing branches. "The focused EL0 window owns the USB keyboard" and "serial always reaches the shell" are now two statements about two disjoint carriers, and neither can be broken by editing the other. The click grammar is unchanged (click = SELECT + ack, SPACE = stop/start, focus never stops anything).

**Named cost.** A full-screen *kernel* app (`vug`, `pulse`) on the Pi no longer sees serial keystrokes in its own `pump_and_poll` drain. It only ever saw the ones it won off `input_service` in a coin toss, and its documented exit gestures are the USB key and the click.

**Witnesses.**
* `[serfocus] serial-in accepted=… delivered=… dropped=… held=… high=… cap=… focus=… app=…` — the live census, ~2 Hz, printed only when a byte is actually delivered. `accepted == delivered + held` always. `focus=<non-zero>` with `delivered` climbing *is* the claim, stated on the wire. Deliberately **not** `witness`-gated: it is silent by construction on every boot nobody is typing on, and the flashable `./arroyo kernel8` image an attended bench boots must carry the strings the bench is there to read.
* `[serfocus] split … :: PASS ::` — the QEMU fixture (`witness`-gated, `main::serial_focus_selftest`). raspi4b's `-serial file:` chardev is write-only, so nothing can be typed under QEMU; the fixture drives the pipeline from the `shell_inbox::offer` seam `input_service` calls, exactly as `input_router_selftest` drives the router from `push_event` rather than from a USB keypress. Four legs: focus does not divert (the real router routes 0), order preserved across a ring wrap, the storm bounded at `CAP` with a `GUI_SENT` delta of **zero**, and what survives a storm is the first `CAP` bytes in order.
