# POWERMENU (R75) — how Peter turns the machine off, and sees what the battery is doing

## Finding
- The Crystal menu already carries `Restart` and `Shut Down` rows (`video/crystal.rs` `ROWS`), but one click fires them
  with no confirm; x86 Restart is `unimplemented: Restart (no reboot path)` although `power::reboot` is a real FADT/8042 ladder
  (`power.rs` `platform_reboot`); x86 Shut Down calls `acpi_power::poweroff()` DIRECTLY, bypassing `power::shutdown` (so no dirent
  flush, no census, no window close, no HDA stop).
- The menubar battery item (MENUBATT, `video/menubar.rs`, model `video/status.rs`) is paint-only: a click on it goes nowhere.
- `drivers/smc.rs` `battery::snapshot` reads BRSC/B0AV/B0AC/B0FC/B0RM; `B0CT` (cycles) and `B0DC` (design capacity) are never read.
- No `battery` shell verb. No low-battery action.

## Mechanism
- M1: `crystal::fire` Restart/ShutDown -> `power::reboot()/shutdown()`; `crystal::press_at` gates both rows through
  `powerui::confirm` (first click arms for 5 s, the row re-reads "Click again to shut down|restart", menu stays open; second click
  within 5 s fires). `power::prelude(action)` = flush dirent + `wm::close_all_furniture` + `hda::powerdown::stop` ; `power::going(action)`
  prints the `:: POWER: ... -> going ::` line immediately before the ACPI/PSCI call. Pi: rows exist, honest stub (no PSCI).
- M2: `menubar::batt_box_abs` + `crystal::press_at` closed arm -> `crystal::open_battery_panel` (a second MODE of the crystal dropdown:
  same transient surface/occlusion/erase machinery, `powerui::panel_*` text). `battery` verb prints the same lines. `smc::battery::extras`
  reads B0CT/B0DC.
- M3: `powerui::lowbat_service` (called beside `status::poll` in `desktop_app_service`): NOTICE at 10 % and 5 % (once per
  threshold per boot, discharging only); `UNAOS_LOWBAT_SHUTDOWN=<pct>` (feature `lowbat_shutdown`, value via `option_env!`) runs `power::shutdown()`.

## Witness
- `:: POWER: action=shutdown|reboot windows_closed=N flushed=0|1 hda_stopped=0|1 -> going ::` (last witness before the firmware call)
- `:: POWER-UI: panel_ok= notice_ok= -> PASS ::` (`tests power`, forced percent, no real shutdown)

## Pins
- x86-wc.spec REQUIRE `:: POWER-UI: panel_ok=true notice_ok=true -> PASS ::`, FORBID `-> FAIL`.

## Knob
`UNAOS_LOWBAT_SHUTDOWN=<pct>` : arroyo `_feats+=lowbat_shutdown`, builder pushes feature, banner-cert row, value via `option_env!`.

## Written
All three milestones are written (uncompiled, per R76). Boot 17 should show, from the desktop shell:
- `tests power` -> `:: POWER-UI: panel_ok=true notice_ok=true -> PASS ::` (plus `:: POWER-UI: battery panel open rows=6 ::`).
- Crystal -> Shut Down (first click): `:: POWER-UI: armed verb=shutdown window_ms=5000 ::`, row reads "Click again to shut down"; second click:
  `:: POWER-UI: confirmed verb=shutdown ::`, `[pwrshutoff] ...`, then `:: POWER: action=shutdown windows_closed=N flushed=0|1 hda_stopped=0|1 -> going ::`
  (the last witness before the ACPI call; the firmware-side `ring drained`/`ftdi flushed` lines of acpi_power follow it, unavoidable).
- Click on the menubar battery item -> `:: POWER-UI: battery panel open rows=6 ::`; `battery` verb -> the same lines + `:: BATTERY: lines= first= ::`.
- Low battery: `:: POWER-UI: lowbat notice level=10|5 pct= ::`; with `UNAOS_LOWBAT_SHUTDOWN=<pct>`: `:: LOWBAT-SHUTDOWN armed: pct= <= <pct> ::` then the clean shutdown.
Known limits: the armed label reverts on the next composite after 5 s (no timer repaint); `going` is not printed on the Pi (no PSCI, rows are honest stubs there);
the NOTICE needs the `login` feature (notice surface); the `shutdown` shell verb now also closes windows (it runs the same prelude).
