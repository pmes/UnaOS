# KVMLAT2 — the three KVMLAT stops, fixed (rmbp-ledger B617)

Spec: `docs/dev/evidence/kvmlat/KVMLAT.md` "The stopped changes, exact" (B578, B576, B579). Wire cited from
`docs/dev/evidence/qemu-image26/serial.log` (one KVM boot, image-26 tree @62a3901a8). Arc branch `exec-rmbp-kvmlat2`,
cut from 37adc602. One commit per row, each compiled once on the x86 kernel line (kfeats24); the Orin line once at
the end. Nothing here was run: R107, the seat runs the legs at the fold.

| row | cause (wire) | fix | the wire the fold should read |
|---|---|---|---|
| B578 LOCKREG | `[lock] held-too-long name=users.rs:364 at=users.rs:667 … ms=81` (760), `kdf calibrated iters=266666` (759), `kdf_ms=271` (773): `set_password` held `TABLE` across the calibration, the PBKDF2 stretch and the flush | `fs/users.rs` `set_password`: snapshot row, `seq` and the cached count under `TABLE`; drop it; calibrate (`kdf_measure`, split out of `calibrated_iters`) and hash off the lock; re-take, publish only if `seq` and the row are unchanged (else re-hash, at most 3, then `Volume`) | no `[lock] held-too-long name=users.rs:364 at=users.rs:6xx`; `:: LOCKREG: … -> PASS :: held_max_at=<file>:<line>` with the file not `users.rs` (the flush alone stays under the lock) |
| B576 JOBSCAN | `[boot] phase=desktop … at=12737ms`, `[hda] probe deferred-start at=12960ms` and the codec walk, then `[desktop] built at=login ok bar_ms=425` (12737 + 425 = 13162): `hda::probe_after_root` (main.rs:6056, the usb-pump's `net-hda` step) ran inside the bar's span on the same loop as `desktopbuild::service` (main.rs:5999, `desktop-app`); `:: JOBSCAN: … bar_ms=425 -> FAIL` (bound 200, fs/jobs.rs:443) | `drivers/hda.rs:2277` gate gains `\|\| bar_holds()` (LINE-NEUTRAL fold before the `//`; `bar_holds` at the file tail): no bring-up until `desktopbuild::settled()`, bounded by `fs::jobs::BAR_WAIT_MS` (now `pub`, 10 s) from the gate's first open. PROBE: `video/desktopbuild.rs` `[desktop] built` gains `held_ms= batt_ms= gap_max_ms=` | `[desktop] built at=login ok bar_ms=<n> … held_ms=<n> batt_ms=<n> gap_max_ms=<n>` BEFORE `[hda] probe after-bar waited_ms=<n> why=settled` and `[hda] probe deferred-start`; `:: JOBSCAN: … bar_ms=<200 … -> PASS`. If bar_ms stays over 200, the probe splits it: held_ms (the stage), batt_ms (the battery wait, ≤ 500), the rest the paint read-back, and gap_max_ms the longest pass gap (the render task's `settings::service`, B579, is the other candidate on the same second: serial.log 838) |
| B579 LOGINRENDER | `[lag] stall at_ms=12716 … stage=render-handler stage_ms=271 … handler_what=quarry.settings:271` (838), `:: LOGINRENDER: … zero_pass_max_ms=271 worst_what=quarry.settings:271`: `video::settings::service` (settings.rs:353, stamped whole at video/quarry/live.rs:2902) held the render task, part unnamed | `video/settings.rs` `service`: one nested stamp per part (LINE-NEUTRAL, each on its own line): settings.launcher (launcher + appswitch), .assoc, .nameindex, .prefs, .loginitems, .files, .appearance, .load (`load_for_login`), .persist, .mode (mode + tp + bus_changes), .session (`prefs::user_name_in`). `video/loginrender.rs`: the names (ids 11-21, `W_S_*` at the file tail) and the nesting rule made recursive (`Stamp.2` sets the parent's children aside, so a grandchild that took half its parent names the record through every level; the old rule stopped at depth 2 and would have kept `quarry.settings`) | `:: LOGINRENDER: … worst_what=settings.<part>:<ms> …` and `[lag] stall … handler_what=settings.<part>:<ms>`. The part is then moved off the render task on that wire (the PREFSWATCH shape); this arc does not move it blind |

## B579 — the move is owed to the wire, not made here

The arc asked for the part to go off the render path "if it is a store walk". Read at 37adc602, no part of
`settings::service` is a store walk on its face: `prefs::service`'s 2 s delete watch already runs on a worker
(loginrender.rs `W_PREFSWATCH`, `moved=prefs-watch:167` is its run count), the session's prefs file was loaded on the
login worker (`[prefs] load clamped=0`, serial.log 765-773), `settingsfiles::service` is an idle latch, and
`nameindex::service` is a chunked build. The one candidate the wire places in that second is `load_for_login`:
`[settings] loaded n=0 user=una store=/home/una/settings`, `[backlight] login keep …`, `[settings] trackpad loaded …`
print immediately before the stall line (serial.log 832-838). It is seven PrefGets in `from_prefs` and five in `tp_load`, over the prefs bus
(`prefs_client::pref_get` → `call`), the backlight readback, the trackpad load, `mode_at_login` and
`appearance::load_at_login` — bus round trips and display work, not a walk. Moving it to a worker is a design change
(what of it must stay on the render task: the scale, the backlight) that one named measurement should decide; the
stamps make the next run name it (`settings.load` if this reading is right).

