# SOFTY3 — SOFTY2's owed, outside its lane (rmbp-ledger B651)

Branch `exec-rmbp-softy3`, cut from `8da376cb` (wave34, seat cloud). Parent arc: SOFTY2 (B641, folded at `2038a7ff`);
its owed list is verbatim in `docs/dev/evidence/softy2/SOFTY2.md` "Owed (outside this arc's files)".

## The one compile (R107 / R113)

```
cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target \
  cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec \
  --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave34/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"
```

rc and the tail of its log: `compile.txt` beside this file. Not run (R107/R113): `./arroyo check`, the aarch64 leg, knoboff,
QEMU, `tests softy`, `tests statustray`, `tests notify` — the seat's at the fold.

## What landed

### (1) `tests statustray` reads the keymap's own table name (statusmenu.rs:399)

SOFTY2 made `keymap::active()` pick by look AND keyboard (R126): Una + PC keyboard is `una-pc`, not `pc`. The fixture's
input leg still asserted the literal `"pc"` after the toggle and `"crispy"` after the second, so under Una it read FAIL by
construction. The leg now records `active().name` before the first press (`set_pc(false)` has just run, so it is the
Mac table of the live look: `crispy` under Una, `softy-mac` under Softy), asserts `pc_selected()` flips and the name moves
away, then `pc_selected()` clears and the name comes back. No table name is typed in the fixture. One line, line-neutral
(`statusmenu.rs` is declared unconditionally, video/mod.rs:1236; the fixture is `witness`-gated but the line count holds).
`:355`'s `input=pc|us` census field is the KEYBOARD flag, not the table — left as it was (a spec token).

### (2) The clock opens notify's own Center — `notify::request_center` (the seam)

* **The seam** (notify.rs tail): `pub fn request_center(open: bool)` stores `REQ_CENTER` 4 (open) or 5 (close); atomics
  only, the press router's rule. `pass` acts on the next service pass: `4 if open => spent`, `1 | 4 => center_show` with
  the wire `[notify] center open items=<n> unread=<n> apps=<n> shown=<n> by=<bell|clock>` (the `by=` field is new, appended
  at the end), `5 => center_close(.., "clock")`.
* **The callers** (softy/startmenu.rs): `open(2)` requests the Center open once the calendar row is up (after its own
  `[softy] calendar open …` line); `close` of kind 2 requests it closed. So under Softy the Center's life is the
  calendar's — the clock press, the Esc edge, a press outside, a look switch (`close("switch")`) all take both down.
* **The rect follows the look** (`center_rect` → `softy_center_at`, notify.rs tail): under the UnaOS look the Center hangs
  under the menu bar as before; under Softy it sits above the taskbar (`softy::taskbar::top(pw, ph)`) behind a `FLY_GAP`
  band, at the right edge — and to the LEFT of the calendar flyout while that is open (`startmenu::rect()`, kind 2, when
  the flyout's x leaves room: `x = fx - gap - CW`), the two side by side as the Windows clock flyout shows the
  notifications beside the month. The panel height is `PANEL`'s (published by every pass, the same word the press
  router reads), so `center_rect(pw)` keeps its signature and its four callers.
* **The press router** (`notify::press_at`): with the Center open under Softy and the calendar open, a press outside the
  Center is NOT spent (it was `REQ_CENTER=2; return true`, which would have closed the Center and swallowed the clock
  press, leaving the calendar up): it returns `false` so `strip::press_route` falls through to the dock arm, which under
  Softy is `softy::taskbar::press_at` — inside the flyout it is the flyout's; on the clock it toggles the calendar closed;
  elsewhere it closes the calendar `by=outside`; each of the last two closes the Center through the seam. A press INSIDE
  the Center (Clear, an item's `x`, an item's action) is still notify's; an item's action closes the Center alone and
  leaves the calendar up (the Center's own close, as under the bell).
* **Legacy aliases → semantic roles** (notify.rs only; DESIGNFIX does winmenu.rs and crystal.rs and must not touch this
  file): 16 calls, `theme::chrome_face()`→`surface_window()`, `frame_line()`→`line_subtle()`, `button_face()`→
  `surface_raised()`, `content_text()`→`text_primary()`, `title_text_active()`→`text_primary()`,
  `title_text_inactive()`→`text_secondary()` — exactly the forwarding theme.rs:754-761 did, so no pixel moves. 0 legacy
  names remain in notify.rs (`grep -c` after the edit: 0).
* **Knob-off**: `notify` and `softy` are declared under `#[cfg(any(all(x86_64, wc), all(aarch64, desktop_firmware)))]`
  (video/mod.rs:1205, :1260), so neither file is lexed into the knob-off image (LAWS §5: a cfg'd-out `pub mod` is the
  exception); the new statements sit in those files and in no unconditional one except the statusmenu.rs line-neutral
  fold. The seat measures at the fold (R107).

### The fixture

`tests softy` (softy/mod.rs, leg 10, after the SOFTY2 legs and before the switch back): the clock pressed through
`taskbar::press_at` at `probe_hot(2)`, `notify::service()` driven once, then `startmenu::open_kind() == 2` and
`notify::center_open()` both true; pressed again and both false. The Center the operator had open before the fixture is
asked back (`request_center(true)`) at the end. A second line, after the SOFTY2 line:

```
:: SOFTY: clock->center open=ok close=ok -> PASS ::
```

Around it on the wire: `[softy] calendar open by=button … notify_unread=<n>`, `[notify] center open items=<n> unread=<n>
apps=<n> shown=<n> by=clock`, `[softy] calendar close by=button`, `[notify] center close by=clock`. `tests statustray`:
`:: STATUSTRAY-T: … input=ok … -> PASS ::`. No tests.rs change: the leg rides the existing `softy` registration (tests.rs:191).

Go-red (the seat's, by mutation): delete the `request_center(true)` call in `startmenu::open` → `open=no`; delete the
`request_center(false)` call in `startmenu::close` → `close=no`.

## Not done — the exact change, outside this arc's files (HEAD rule 3)

### (3) `Action::Start`

Needs `unaos/crates/kernel/src/video/keymap.rs` (not this arc's file): a variant `Start /* SOFTY3: the Windows key
alone — toggles Start under Softy, code 50 */` at the tail of `enum Action` (keymap.rs:151) and an arm
`Action::Start => "start"` in `Action::name` (keymap.rs:212); then, in this arc's files, `Action::Start` appended to
`smallfix3::ACTIONS` and `listed` (smallfix3.rs:28, :45), `Action::Start => 50` in `clipboard::action_code`
(clipboard.rs:421, the ring-3 ABI number, next free after Interrupt's 49) and `| Action::Start` in the `("ignored", 0)`
arm of `clipboard::terminal_action` (clipboard.rs:367). Nothing binds it: the tap reaches Start through
`keymap::take_start_taps` on the look's service pass (SOFTY2, keymap.rs:671), which is the design SOFTY2 chose because the
decoder runs where a window may not open. The Win-chords (Win+E, Win+D, Win+arrows) also need a WIN role bit and Softy
rows in `theme.rs` — a design question for the arc that takes them, not a line to fold.

### (4) `docs/dev/OS/05_USER_EXPERIENCE/SOFTY.md` "Not yet Windows-like (owed)" (SOFTY.md:164)

Not this arc's file. Replace the list's six bullets with this, dated 2026-10-13:

* DONE in SOFTY2 (B641): the Win key (a bare tap of the key playing the Windows role, R126), Settings > Appearance >
  Look, soft shadows under the flyouts (`paint::shadow`, elev.2), pressed states (caption buttons, taskbar), quick
  settings (sound, Wi-Fi, battery), dark Softy (`palette::dark(Sem)`).
* DONE in SOFTY3 (B651): the clock opens notify's own Center beside the calendar.
* OWED (2026-10-13): `Action::Start` and the Win-chords (Win+E, Win+D, Win+arrows: a WIN role bit and Softy rows in
  theme.rs); task view; snap layouts; jump lists; "All apps"; a search field that types in place (it opens the
  Launcher); desktop icons (no desktop icon layer exists in the tree); rounded bottom window corners; `look::service`
  repainting Settings on the Look click (SOFTY2 owed 5); the Center's rect under Softy on the Pi (`desktop_firmware`:
  the same code, unflown there).
