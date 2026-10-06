# STATUSTRAY (rmbp-ledger B426) — MACPARITY row 3: status items that are each a MENU

Cut from cbb9c7c6 (exec-rmbp-merge17), branch `exec-rmbp-statustray`.

## Finding (the tree, read before building)
- `video/status.rs` (MENUSTAT) models one item, the battery, fed by `status::poll` on the desktop
  service pass; `video/menubar.rs` reads `status::bar_item` and draws the cell left of the clock.
- The battery's menu already exists: POWERMENU M2 is a second MODE of the SHARD dropdown
  (`crystal.rs` panel mode, text lines from `powerui::TEXT`), opened by `crystal::press_at` on
  `menubar::batt_box_abs`. That panel is the ONE status drop-down this tree has; winmenu's tree widget
  is window-owned (`publish(owner, titles, on_pick)`) and is not a status-area surface.
- Volume: `status::VOL_LEVEL/VOL_MUTED` (BEZEL, Settings' `set_volume`); the sink is `hda::vol` (ready
  once `capture` found amps). Network: `usbnet::{kind, is_up, link_up}` (x86+usbnet), `wifi::status::d11_up`
  (x86+wifi), the lease in `smolnet` (`LEASED`, no address accessor). Input: `keymap::active()` —
  CRISPY (Cmd role = GUI) or PC (Cmd role = Alt, "selected by nothing"). Clock: `clock::try_unix_now`;
  the bar draws `--:--` unanchored since CLOCKBAR (that stays; the clock MENU says "not set").
- Neither winmenu nor the crystal has a motion/hover path (MACPARITY row 5 "unmeasured"; crystal.rs
  "DESIGNED, NOT BUILT — A HOVER HIGHLIGHT"). Switching menus is press-driven, as winmenu's titles are.

## The seam
Kernel — wm (desktop furniture). The MODEL stays `status.rs`: a `Tray` snapshot (volume, net, input,
clock) published by `status::tray_publish` on the SAME desktop service pass that runs `status::poll`
(the owners' atomics, read there, never in the paint). The bar reads `status::tray()` (relaxed loads).
The drop-down is the SAME panel mode of the SHARD dropdown the battery uses — `statusmenu.rs` only
decides WHICH item is open, builds that item's rows from the model, anchors the panel under the item
and acts on a row press through the owner's existing seam (`status::set_volume`, `keymap::set_pc`).
No second drop-down, no second store.

## Milestones
- M1 status.rs: the `Tray` model + `tray_publish` on the service pass; smolnet `lease_ip`; keymap PC toggle.
- M2 statusmenu.rs (new) + crystal panel glue: open per item, anchor under the item, row presses,
  switch on a press of another item, close on any other click; a drawn slider row for the volume.
- M3 menubar.rs: volume / network / input items drawn left of the battery in reserved slots (an
  unknown item is not drawn, the slots never move); the press cells; menus_right_limit and the date
  respect the tray; the signature folds the tray.
- M4 `tests statustray` + the witness; MACPARITY row 3 folded.

## Witness (the next flight reads)
`:: STATUSTRAY: items=5 drawn=<n> menus=<n> volume=<v/16|muted|none> net=<up|down|none> input=<us|pc> clock=<synced|unsynced> ::`
— once when the bar first paints the tray, again on a presence/state edge (not on a level tick).
`[statusmenu] open item=<name> rows=<n> x=<px>` / `[statusmenu] pick item=<name> row=<r> -> <what>`;
`tests statustray` -> `:: STATUSTRAY-T: … -> PASS|FAIL ::`.

## Owed
- Hover tracking (open one, slide across to the next without a press): no motion path exists for
  ANY bar menu (winmenu, crystal); it lands with row 5's motion arc, and statusmenu's `switch_to` is
  the function it will call.
- Slider DRAG: the volume row takes the press position (BRIGHTSLIDER's click seam); press-and-move is
  SLIDERDRAG's (R93), shared by every slider.
- Network Turn Off/On: no driver arm exists for usbnet or wifi; the row is shown disabled ("owed").
- The sink's name: hda::vol carries no codec name; the row says "HDA output".
- Items toggled in Settings (MACPARITY row 3 "settable"): SETTINGS' schema row, not this arc.
