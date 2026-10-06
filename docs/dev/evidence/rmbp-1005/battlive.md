# BATTLIVE (rmbp-ledger B492) — R103 §1: the battery dropdown live, the power coming in, the item never dropped

Cut from 21521a53 (merge19) on `exec-rmbp-battlive`. No new knob (rides `smc`).

## Finding (flight 26, read with awk)
- Every boot: `:: SMC-BATT: present=true soc=100% … amp=0mA … ac=derived:idle` — plugged in and full, and the
  dropdown said `State: discharging` (`powerui::battery_lines` has two states: charging / discharging).
- The dropdown is filled ONCE, at open (`crystal::open_battery_panel` -> `powerui::panel_fill`); nothing refills it
  while it is down, and the model it reads is swept every 10 s (`status::POLL_MS`). Hence "not live updated".
- Adapter power: the 2012 rMBP has NO `AC-W` (SMC-SCOUT flight 14: `key AC-W absent`). Its own key enumeration
  (flight 14, `SMC-SCOUT: idx …`) carries `PDTR` (idx 344, DC-in total power), `ID0R` (idx 196, DC-in current),
  `VD0R` (idx 474, DC-in voltage) and `ACIN` (idx 9). None is read today.
- BATTGONE: boot 2 `[desktop] built … battery=painted`, then the item gone with no line. The only path that takes
  the item off the bar is `status::bar_item()` -> `None` once the held reading is older than `STALE_MS` (60 s) —
  i.e. the device-service pass that sweeps it went quiet for a minute (boot 2's wire: `[status] poll` and `:: PWR:`
  both stop at 14:34:24/14:34:31 under the hid stalls). The item vanished silently because the bar's paint says
  nothing about which items it drew.

## Seam
`video::status` (the desktop STATUS MODEL, both arches) stays the one source the bar and the menus read; the SMC
is touched only by the driver (`drivers::smc::adapter`, new tail module: the DC-in keys, bounded reads) and only
from the device-service pass. The tray's item list is the source of truth (STATUSTRAY B426): a reading that has
gone stale is HELD on the bar and said so, never dropped silently.

## Milestones
- M1 — live dropdown: `status::live_tick` on the service pass (inside `status::poll`): while the battery menu is
  open, sweep each second (battery keys + `PDTR`, falling back to `ID0R`x`VD0R`), store, rebuild the panel rows,
  bump the menu generation (the crystal repaints). State: charging / fully charged / not charging / discharging;
  `Power source: Power Adapter (NN.N W in)`; the bar's glyph reads the same store, so it tracks.
  Witness `[battery] live pct=<n> state=<s> watts_in=<w> src=smc:<reg>` once a second while open.
- M2 — BATTGONE: `bar_item` holds a stale SMC reading (the item stays); `[strip] paint tenant=menubar
  items=<list> battery=<live|held|none|noseat>` on every change of the drawn item set (edge, not periodic).
- M3 — `tests battery` -> `:: BATTERY: live=ok watts_src=<reg or none> strip_kept=ok -> PASS ::` (registered on the
  desktop pass, R80); the tests table is full on metal (flight 26: `table full (cap=128)`), so CAP 128 -> 160.

## Witness (metal)
Open the battery item: `[statusmenu] open item=battery`, then once a second
`[battery] live pct=100 state=fully-charged watts_in=<w> src=smc:PDTR`; `tests battery` -> `:: BATTERY: … -> PASS ::`.

## Owed
- `PDTR`'s type is decoded as `sp96` (2 bytes, watts = raw/64) from Apple's published key catalogue, and `ID0R`/`VD0R`
  as `sp5a`/`sp4b`; no key-info (0x13) read exists in the driver, so the raw bytes ride the first live line
  (`raw=`) for the flight to confirm the decode.
- Why the service pass went quiet for a minute is HIDSTALL's, not this arc's.
