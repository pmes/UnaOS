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

## 7. A window title is a NAME (WINTITLE)

Peter's ruling at the glass, render11, 2026-09-08: *"VUG WINDOW NAMES ARE DUMB AND WINDOW TITLES
NUMERICALLY SEQUENCED IS FOR UNTITLED DOCS ETC"*.

**The defect.** Both arch window seams (`arch/x86_64/syscall.rs` and `arch/aarch64/syscall.rs`, in
`mod wc_shim`'s `create`) built a title out of their own table row index:

```rust
let title = [b'e', b'l', b'0', b' ', b'w', b'i', b'n', b' ', b'0' + (id as u8 % 10)];
```

So every program launched from the shell came up titled `el0 win 0`, `el0 win 1`, `el0 win 2`. That
is a generated label carrying a sequence number — not the application's name, and wearing the one
piece of typography reserved for a nameless document.

**The rule.** A title is a name, resolved in this order at exactly one site
(`video::wm::mint_title`, whose only caller is `create_inner`, so a window cannot be titled by any
other path):

| # | `from=` | title |
|---|---|---|
| 1 | `program` | the launched program's own name: the launch path's basename with its extension dropped, in the case the operator wrote it (`/apps/VUG.ELF` → `VUG`) |
| 2 | `unnamed` | `Application` — an EL0 window whose launcher armed no name. A noun, **never** a number, so several unnamed programs read the same word |
| 3 | `document` | `Untitled`, `Untitled 1`, `Untitled 2` (`wm::untitled_document`) — **the only numbered titles in the system**, and only for a document with no name |
| 4 | `declared` | the name the creating module declared at its `create` / `create_at` call site. Built-in tenants are their own declarers: `Console`, `Shell`, `Pulse`, `Quarry`, `Install UnaOS` |

**An app window never carries a numeric suffix**, however many instances of that program are open:
two VUG windows are both `VUG`. Numbering belongs to documents.

**There is no ELF-declared name, and no manifest format was invented to make one.** The ring-3 ABI
(`crates/una-abi/src/lib.rs`) carries syscall numbers and info-page offsets and nothing else;
`SYS_WIN_CREATE` takes `(w, h)` — an app never supplies its own title, deliberately, so a program
cannot paint something that looks like another window's frame. No ELF note or section is read on the
load path. Clause 1 is therefore the program's FILE name, which is the name the operator typed. If
an ABI ever grows a declared-name field it becomes clause 0 and reports `from=declared`; the witness
vocabulary already has the word.

**Naming a launch.** A launcher that resolves a path to an image calls
`wm::app_name_arm(wm::owner_of_launch(handle), path)` immediately after the spawn returns, and
`wm::app_name_forget(...)` when it retires the job. Armed today: the shell's `bg` and its bare-name
launch (`shell.rs`), Quarry's double-click (`video/quarry/live.rs`), and the x86 desktop app
(`video/desktop_uefi.rs`). ⚠ `owner_of_launch` exists because **the two arches return different
things from `spawn_user_image_bg`** — x86 returns `mapped.slot` (0-based) while aarch64 returns
`ttbr0 >> 48`, which is `slot + 1` — so the same handle is off by one against the compositor's owner
namespace. Normalising the two seams is owed (LEDGER SO22).

**The wire.** The source of every title on the glass is readable from a capture rather than believed
from a screenshot — and it rides the `[wm] alloc` line the window system already prints once per
create, appended to it:

```
[wm] alloc win=1 gen=4 owner=0x1 title="VUG" from=program
```

⚠ **Appended, not printed beside it, and that is a measured constraint rather than a preference.**
The first cut of this arc added a second `serial_println!` next to the alloc witness. One extra
routed-console print per create is one extra composite driven from inside `wm::create_inner`, and it
cost the furniture its compose passes for the rest of the boot: on `UNAOS_WC=1 ./arroyo test`,
`[dock] selftest passes=44` fell to `passes=4` and `[crystal] selftest passes=7` to `passes=0`,
reddening `:: DOCK: … vacate=false` and `:: WINMENU: … app_box=false` — two fixtures with nothing to
do with titles — while the title fixture itself passed. A/B at the same sha: the pre-arc tree exits
0 on that command, the two-print tree exits 1. **A per-create fact goes on that line, never on a new
one.** It therefore inherits that witness's terms: the first `WINID_LOG_MAX` (32) creates of a boot,
and only on a build that has window furniture (`wc` on x86, `desktop_firmware` on aarch64) — which is
every build that has a title bar for a title to appear in.

**The fixture.** `wm::wintitle_selftest`, `witness`-gated and folded at the tail of
`hittest_selftest` — the one `wm` battery both arch selftest drivers run, so one fold covers x86 and
aarch64. Seven legs: the `program_name` derivation; the document sequence; the seam label resolving
to a noun with **no digit in it**; an armed owner taking its program's name; **two windows of one
program carrying the same title**; a declared name surviving verbatim with the document form still
recognised; and the wiring end-to-end — a real `create` over a seam label, read back out of the
window ROW rather than out of a helper's return value. `app_name_forget` is asserted too: the owner
falls back to the unnamed clause it started at. Its verdict:

```
:: WINTITLE: program_name=1 document=1 label=1 seam="el0 win " program=1 no-seq=1 declared=1 row=1 forget=1 PASS ::
```

A full window table makes `row=skip` — that leg alone, never the verdict, so the six legs already
measured are not thrown away and the wire is never silent about a fixture that ran.
