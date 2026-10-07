# MENUBARSLOTS (rmbp-ledger B499) — the status items as ONE right-to-left flow

Cut from 668cdd95 (the merge19 fold + the flight-27 bench commits), branch `exec-rmbp-menubarslots`.

## Finding (the wire, then the code)

Flight 27 (`f27-boot1.log`): `[strip] paint tenant=menubar items=input,battery,clock battery=live` at login, then
`items=input,net,volume,battery,clock` once the tray published — and the bell is NOT in that list at all. Peter:
"menubar looks good but the wifi and notify icons overlap." The code says why: `video/menubar.rs` places every status
item by its own fixed arithmetic off the battery. `bell_slot` = battery − PAD − BELL_ITEM_W (NOTIFY B418) and
`tray_slot(VOLUME/NET/INPUT)` = battery − PAD − … (STATUSTRAY B426) both start from the SAME left edge, so the bell
lands on the volume glyph and the left of the WiFi label. Two arcs, one slot, no list that both read.

## The seam

Kernel (the bar is the kernel's `wm` furniture; no handler owns it). No new file, no new knob, no new store: the
fix is ONE function, `flow_slot`, over ONE ordered list `FLOW` — right to left, the Mac's order: clock, notify
(the bell, the Notification Center's item), battery, net (WiFi/ETH), volume, input. Each item measures its own
width (`flow_w`); each sits one `strip::PAD` left of the previous one. `clock_slot`, `batt_slot`, `bell_slot` and
`tray_slot` are now thin reads of it, so the painter, the press cells (`batt_box_abs`, `bell_box_abs`,
`tray_box_abs`), the titles' right limit and the volatile mask read one geometry. Slots stay RESERVED (a function
of the bar width alone — STATUSTRAY's rule: no item moves when another's fact comes or goes).
`status::ITEM_NOTIFY` names the bell on the wire; `status::ITEMS` (the statusmenu's dropdown items) is unchanged.

## Milestones

- M1 — `FLOW` + `flow_slot`; the five slot functions read it; MENUSTAT's seat leg reads "one PAD left of the
  bell" (the battery's right neighbour is now the bell, as on the Mac).
- M2 — the witness: `[strip] paint tenant=menubar items=input,volume,net,battery,notify,clock x=<x,…> battery=live`
  (left to right, each item's left x in bar pixels; re-printed on an edge of the item set, the battery word or
  the bar width). `tests menubar` → `:: MENUBAR: items=<n> overlaps=0 w=<bar w> x=<x,…> -> PASS ::` — every
  seated item's rect against every other, on the live bar width and the floor width.

## Witness (what flight 28 reads)

`awk 'index($0,"[strip] paint tenant=menubar items=")'` — the x list strictly increasing, each gap ≥ the item's
width; typed `tests menubar` → `:: MENUBAR: items=6 overlaps=0 … -> PASS ::`.

## Owed

- BRIGHTKEYS' transient level item (`bright_slot`, 9 cells left of the battery) still draws over the tray while it
  shows; it is a transient overlay (the bezel replaced its role) — not in this arc's list; a seat question.
- The Mac closes up absent items; this flow keeps reserved slots (STATUSTRAY's ruling). A seat question.
- Items toggled in Settings (MACPARITY row 3) — the flow is where such a toggle would land (skip an item).
