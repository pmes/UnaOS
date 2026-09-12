# DOCK — the taskbar strip and the pinned-app model

Status: **APPPIN landed on `exec-orin27-conreopen`** (orin 27, 2026-09-12), unflown. Module:
`unaos/crates/kernel/src/video/dock.rs`. Knobs: the dock exists on `UNAOS_WC=1` (x86) and on every
`desktop_firmware` aarch64 desktop (`UNAOS_PIDESK`, `UNAOS_DESKCASCADE`, `UNAOS_ORINRENDER`).

Peter's rulings, verbatim in `docs/dev/RULINGS.md`:

- R49 (2026-09-12): *"console should be an app that is pinned to the taskbar not this mystery thing
  that appears"* · *"same with shell"*.
- R50 (2026-09-12): *"quarry is our finder."* (and, the day Quarry was named, quarry.md:10: *"pinned to
  the left side of the taskbar/dock so it opens like Mac's Finder"*).
- Q10 (2026-08-09): the dock is a Mac-like window switcher; *"if looks a little off we will change it"*.

---

## 1. What a tile is

The strip is a **window switcher**: one tile per live window, kernel-owned rows included, and a press
raises and un-hides the window it names (`wm::focus_changed` + `wm::raise_one`, keyed by `(id, gen)`
so a stale tile raises nothing rather than the wrong thing — DOCKID). It carries no app grid.

Three tiles are **pinned apps** and are always on the strip, in the macOS order:

| tile | owner band | position | when the app has a window | when it has none |
|---|---|---|---|---|
| `quarry` | `quarry::OWNER` | leftmost (the Finder, R50) | the live row | a pin; press opens (`quarry::request_open`) |
| `console` | `wm::KERNEL_OWNER_CONSOLE` | where its row sat, left of the shell | the live row | a pin; press **launches** |
| `shell` | `wm::KERNEL_OWNER_DESKTOP` | the permanent tail | the live row | a pin; press **launches** |

A pin is a synthetic `DockEntry` with a sentinel id (`SHELL_PIN_ID`, `CONSOLE_PIN_ID`,
`QUARRY_PIN_ID`, one below the other from `u32::MAX`; the pulse instrument's `PULSE_PIN_ID` is the
same shape and is unchanged by this arc). The pip is lit for a live row and takes the minimised ink
for a pin. Painter (`compose`), router (`press_at`), occlusion registry (`strip_rect`), `wm::dock_tiles`
and `selftest` all build the model through the same chain, and `pins_applied` is the one pure fold the
two count-only readers use — so no reader can disagree about the tile count.

## 2. Launch

A press on a pin **posts a launch** for the app the pin names (`dock::PinnedApp::{Console, Shell}`,
`take_launch`). The post is a bit per app — a queue of one, carrying no window id, no generation and no
"was open" — because the router runs in the input band and may neither allocate a surface nor take a
blocking panel lock (LOCKFIX `7847ceea`); Quarry's tile defers the same way.

The body that **owns the app's instance** drains the post on its next pass and mints a fresh window
through the **same function its own boot bring-up used**:

| app | body | mint seam | what is fresh |
|---|---|---|---|
| console | every render body, via `console_launch_service` (arch-neutral, in `dock.rs`) | `fbcon::panel_console_window_open` | surface, row (id, gen), glyph route; the cell store (the app's document) is repainted |
| shell (x86) | `x86_render_service`, inline at the tail of its event drain | `open_shell_window` + `Screen::direct` | store, `Screen`, `TargetPal`, `Console`, row |
| shell (Pi) | `render_service`'s mint arm, opened by `shellwin_service_rearm` | `open_shell_window` | the same tuple, rebound by the arm |
| shell (cascaded scene) | the console pump, on its drain line | `tegra_shell_window_open(pw, ph)` | `TegraShellWin` (store + `Screen`), its `TargetPal`, the pump's `Console` |

An instance that is already live when the post is drained (the scan-to-press race) is **raised**, never
doubled — one live instance per pinned app. A console mint that declines for a transient reason (an
`FBCON.try_lock()` lost to the glyph painter) is retried for 32 passes before the decline is reported
with its count; `fbcon` names the reason on its own `[wc-x] console-window DECLINE` line each time.

The x86 launch arm is deliberately not gated on the desktop takeover: the tile exists wherever a dock
composites, and the QEMU witness gate drives the arm with no takeover at all (§5).

## 3. Quit

Closing a pinned app's window — the red disc, the app menu's **Quit**, `wc_close_furniture` — is a
**quit**. All three go through `wm::close`, which frees the row and, through the WINID holder registry,
clears every registered id cell (`fbcon::CONSOLE_WIN`, `SHELLWIN_ROW`). The owning body notices the row
gone on its next pass and tears the instance down:

- x86: frees the surface store, drops the `Console` (and everything typed into it), rebinds an empty
  `Screen`/`TargetPal`, publishes the slot-0 key sink as unbound. The generation recorded at the mint
  is the recycled-slot fence.
- cascaded scene: drops the `TargetPal`, then the `TegraShellWin` (the store is freed then and there).
  A keystroke arriving while the scene is up and no shell instance is live is **dropped** — before this
  arc it painted the pump's console over the desktop (the render4 SCREEN0 defect).
- Pi: clears the stale id and parks the mint arm shut; the old store is freed when the next launch
  rebinds the tuple (a teardown at the quit itself needs the mint arm's locals — pi's fold to take).
- console: the route is dropped; the cell store survives (the app's data outlives its window).

The tile stays: the pin appears in the same slot on the next composite, because no live row carries the
owner. Nothing remembers that a window existed.

## 4. The wire — one grammar, both apps, all three bodies

```text
[dock] press at (1077,1165) tile=3/5 app=shell -> launch
[dock] launch app=shell by=console-pump win=4 gen=3 tries=1 -> LAUNCHED
[wm-act] close-furniture win=4 owner=0xffffff02 closed=true route-dropped=false (…)
[dock] quit app=shell by=console-pump win=4 gen=3 -> TORN-DOWN
[dock] press at (1077,1165) tile=3/5 app=shell -> launch
[dock] launch app=shell by=console-pump win=4 gen=4 tries=1 -> LAUNCHED
[dock] press at (974,1157) tile=2/5 app=console -> launch
[dock] launch app=console by=orin_render_service win=1 gen=5 tries=1 -> LAUNCHED
[dock] launch app=console by=orin_render_service win=1 gen=5 tries=0 -> RAISED      (already live)
[dock] launch app=console by=orin_render_service win=0 gen=0 tries=32 -> DECLINE   (fbcon declined 32 passes)
```

`by=` names the pass that drained the post (`x86_render_service`, `render_service`,
`orin_render_service`, `console-pump`). The boot's own shell mint prints the same `LAUNCHED` line, so a
capture reads a boot as the first launch. The router's `band=dock` outcome words are `launch-shell` /
`launch-console` (`raise` and `background` are unchanged).

## 5. The fixture — `dock::apppin_selftest` (x86, `UNAOS_WC=1 ./arroyo test`, ladder tail)

Every leg drives the shipping seams: the press is `press_at` at the shell pin's own tile centre; the
launch is drained by the real `x86_render_service` on its own core and pass (the fixture only waits, up
to 4 s per step, on the launch/quit counters); the quit is `wm::close`; the teardown is the body's own.

```text
:: APPPIN: press1=launch-shell launch1=3:2 quit1=torn-down tile=kept press2=launch-shell launch2=3:3 fresh=true sprite planned=6 nohit=0 cleanup=quit :: PASS ::
```

`fresh` requires `(id, gen)` to differ between the two launches. The sprite leg parks the real pointer
at the relaunched window's centre (dmgovlp's `set_abs` idiom), presents the window six times and reads
`wm::cursor12_offer_counts`: the offer must have been **planned** at least once and `nohit` must not
have moved — the LOST POINTER of render13 boot 1 (`sprite_us=1`, `-> nohit` after the old re-mint)
stated as a gate. The fixture closes what it opened and waits for the quit, so the ladder ends on the
table it started with. Go-red: skipping the mint in the x86 launch arm reads
`launch1=0:0 quit1=no-teardown tile=lost … :: FAIL ::`.

## 6. What was retired (deleted, not `cfg`'d off)

`SHELL_REOPEN` / `take_shell_reopen`, `CONSOLE_REOPEN` / `take_console_reopen`, `CONSOLE_WINDOWED`,
`console_reopen_service`, `console_reopen_drain`, `orin_shell_reopen_drain`, `pi_shell_reopen_drain`,
`tegra_shell_note`, `tegra_shell_live_id`, both `tegra_shell_remint` copies and the SO10 recipe cells
(`TEGRA_SHELL_BASE/LEN/W/H/STRIDE/X/Y`); the `shell=pin -> reopen requested` and
`console-reopen … -> REOPEN` grammars; the `shell-reopen` / `console-reopen` outcome words. Ledger:
SO1, SO10, SO13, SO17 are ticked `dropped` by design under SO27; the drains S4 and SR6 counted are gone.

Left in place, as a STOP for the seat: `wm::shell_remint` (WCSER-REMINT) is the wedge-rescue's adoption
of a corpse row after a render-core death — not a dock route. Retiring it makes the rescue
close-and-recreate against the F4 drain barrier that dead cores can never settle (flight 3: 21 s, then
ABANDONED). The brief listed it; this arc did not take it.

## 7. Invariants

- ONE OS (R16): subsystem names only (`shellwin_*`, `console_launch_drain`, `[dock] launch app=`);
  no `target_arch` gate was added; the launch path is the app's own boot mint on every board.
- One live instance per pinned app; a launch that finds one live raises it.
- The pin tile and the live row never coexist for one owner (`pin_*_wanted` tests presence).
- Knob-off byte identity: every `main.rs` / `syscall.rs` edit is a same-line fold; the retired
  `main.rs` region is followed only by an empty knob-off twin; measured by `./arroyo knoboff wc`.
