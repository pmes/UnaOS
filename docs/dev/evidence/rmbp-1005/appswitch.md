# APPSWITCH (rmbp-ledger B428, MACPARITY row 10) — design

**Finding.** Cmd-Tab resolves to `Action::CycleWindow` (CRISPY table, usage 0x2B) and x86's `wc_focus_key`
answers it with WINCYCLE: `wm::cycle_pick` raises the least-recently-raised WINDOW, one press one window,
nothing on the glass. Two windows of one app are two stops; a minimised window is never a stop. The Mac
cycles APPS on one strip of icons while Cmd is held. Nothing in the tree groups the table by owner for a
switcher (WINDOWLIST's rows are per window), and no input path tells the desktop when Cmd is RELEASED
(`Event::Key` carries no modifier; the HID decoders keep the last modifier byte in `keymap::note_mods`).

**The seam.** Kernel — wm (the window system is the owner of focus and z; the strip is an overlay row like
LAUNCHER and SHORTCUTS). No new store: the apps are read from the live table through WINDOWLIST's
existing helpers (`wm::cycle_order` = visible rows z-descending, `wm::wl_rows` = every app row with its
minimised bit), names from `wm::app_name_of`, icons from APPRES (`appres::blit_key_icon`, generic fallback).
Activation is the existing focus primitive (`wm::focus_changed` raises every row of the owner, parked ones
included, i.e. DOCK2's minimised rows come back) then `wm::raise_one` back-to-front so the app keeps its own
stack and its frontmost window ends on top.

New file `unaos/crates/kernel/src/video/appswitch.rs` — `//! CHARTER: Kernel — wm`. x86 `wc` (the key door
and the overlay row are x86's, as LAUNCHER's). No knob: it replaces WINCYCLE's Cmd-Tab under `wc`.

**Doors (R88: edit and latch only; the acts run in the desktop pass).**
- `key_door` (asked right after `launcher::key_door` in `wc_route_event`): `CycleWindow` builds the MRU app
  list (first Tab) or moves the selection (Tab +1, Shift held -1); with fewer than two apps it is a consumed
  no-op. While the switcher is up every key/action is its own; Esc (`0x1B` / `Deselect`) cancels.
- `hid_edge` (from `xhci::hid_screenshot_chord_edge`, which both decoders call per report): the Cmd role's
  falling edge latches COMMIT; an Esc usage edge latches CANCEL. Atomics only.
- `service` (chained after `launcher::service`): CANCEL closes; COMMIT activates then closes; SHOW opens the
  strip (a quick tap commits before SHOW is served, so nothing flashes, as on the Mac); PAINT repaints.

**Order.** Apps by first appearance in `cycle_order` (z-descending = most recent activation); apps whose
every window is minimised follow, in table order. The focused app is index 0, so the first Tab selects 1.

**Milestones.** M1 the module (grouping core, strip paint, doors, service, activation) + wiring (route door,
HID hook, service chain, `mod` line, `tests appswitch` registration) + SHORTCUTS rows renamed. M2 the
fixture `tests appswitch` + MACPARITY row 10 folded.

**Witness.** `tests appswitch` → `:: APPSWITCH: apps=<n> order=mru strip=<ok|skip-oneapp|FAIL>
activate=<ok|skip-oneapp|FAIL> -> PASS ::` (the grouping core is proven on a synthetic table every run; the
live strip and activation only with two or more apps up). Glass wire: `[appswitch] show apps=<n> sel=<name>`,
`[appswitch] activate app=<name> windows=<n> restored=<n>`, `[appswitch] cancel`, `[appswitch] one-app`.

**Owed.** aarch64 keeps WINCYCLE (its router arm is untouched); a click on a strip icon; Q/H on the selected
app while held; kernel-owned windows (Settings, Quarry, the console) are not apps here, exactly as they are
not WINCYCLE stops; the strip is opaque (the overlay row has no alpha).
