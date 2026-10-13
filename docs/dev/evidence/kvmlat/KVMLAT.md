# KVMLAT — the QEMUIMAGE26 latency reds, read from the KVM boot's wire (rmbp-ledger B614)

Source: `docs/dev/evidence/qemu-image26/serial.log` (one KVM boot, 6 vCPUs, image-26 tree @62a3901a8; every
line number below is that file's). Arc branch `exec-rmbp-kvmlat`, cut from cb833ebb. Every lock and source site
named here was checked at both 62a3901a8 and cb833ebb (same lines). Nothing here was run: R107, the seat runs the
legs at the fold.

| row | red | cause | this arc |
|---|---|---|---|
| B598 STACKROOM | `ipi_flush=timeout shootdowns=371 timeouts=23` | the shootdown IPI is maskable and its wait was one 2 ms TSC bound; past it `shootdown` returned with the late cores unflushed (lines 81, 932: cpu 2; 974: cpus 0 and 5) | FIXED: NMI escalation, then complete or FAULT loudly (stackguard.rs tail, interrupts.rs `nmi_handler`) |
| B578 LOCKREG | `inversions=4 held_max_ms=357` | the four (lines 1123-1126) are the compositor's `try_lock` scratch pools (wm.rs:30196 vs :30381/:30383/:30384/:30385); a try cannot wait, so none is a real inversion | FIXED: `try_lock` records no order edge. PROBE: `held_max_at=` |
| B574 HIDSTALL | `key_queue_max_ms=2542 … hold_worst=?` | an injected key swallowed at the wake (dimidle) leaves lag's key timing open; the next routed key closes it on the old stamp. `?` = no current task on the pump core | PROBE landed: `sched:idle` / `sched:switch` name the `?`. FIX STOPPED (lag.rs, dimidle.rs) |
| B575 INPUTSTALL | `key_queue_max_ms=1520` | the same orphaned key timing (the `tests ehci` minute) | FIX STOPPED (same change) |
| B576 JOBSCAN | `on=worker chunks=0 bar_ms=425` | `chunks=0`: `/jobs` is absent on this image (`[jobs] volume=absent records=0`), nothing to chunk. The FAIL is `bar_ms=425 > 200` (jobs.rs:443), DESKTOPBUILT's login-ok→bar span, built on the usb-pump's device-service pass, the same loop that ran the HDA deferred bring-up first | FIX + PROBE STOPPED (hda.rs, desktopbuild.rs) |
| B579 LOGINRENDER | `zero_pass_max_ms=271 worst_what=quarry.settings:271 moved=wallpaper-probe:10,prefs-watch:167` | `prefs-watch:167` is 167 RUNS on the worker (loginrender.rs `moved`, its selftest prints `name:count`), not 167 ms; the prefs load ran on the login worker. The red is `settings::service` on the render task, 271 ms, sub-step not on the wire | PROBE STOPPED (settings.rs, loginrender.rs) |

## The stopped changes, exact

These need files outside this arc's list (HEAD item 3). Each is the change, not a suggestion of one.

### B574/B575 — end a key timing the router never saw

`unaos/crates/kernel/src/video/lag.rs`, FILE-TAIL append:

```rust
/// KVMLAT (B574/B575): a key/press consumed before the router (the idle wake's swallow at `pal::pop_event`) ends its
/// class timing here, uncounted as latency and counted `orphan=` — else the next routed key closes it on this stamp.
pub fn abandon(ev: &Event) {
    if !ON {
        return;
    }
    let Some((c, _)) = class_of(ev) else { return };
    if SLOTS[c].stage.compare_exchange(ISR, IDLE, AcqRel, Relaxed).is_ok() {
        ORPHANS.fetch_add(1, Relaxed);
    }
    pend_routed();
}
```

`unaos/crates/kernel/src/video/dimidle.rs` `gate`, the press-swallow return (same line, before `return true`):
`if let Event::Key(b) = ev { WAKE_KEY_UP.store(b as u32 | 0x100, Ordering::Relaxed); } crate::video::lag::abandon(&ev); // KVMLAT …`

Why it is the holder and not the bound: lag.rs:188 `on_enqueue` claims the class's one timing at the push, only
lag.rs:212 `route` moves it on, and a later key of the class COALESCES (lag.rs:202) — so a swallowed key's stamp is
inherited by the next key routed. The orphan sweep (lag.rs:475) ends such a slot only after the 3 s timeout, which
is why lines 4616/4623/4631 read 915, 1931, 3005 and then stop. Limit: a swallowed key that was itself coalesced
abandons the timing of a younger key (one sample lost, never a false stall).

### B578 — the users table across the KDF (only if `held_max_at=users.rs:364` on the next run)

`unaos/crates/kernel/src/fs/users.rs` `set_password` (users.rs:667): read the row and the calibrated count under
TABLE, drop the lock, run `password_hash`, re-take TABLE, check the row is unchanged (same `seq`), publish and flush.
Line 760 `[lock] held-too-long name=users.rs:364 at=users.rs:667 by=23@cpu2 ms=81` is the hold said at its first
50 ms scan; line 773 `kdf_ms=271` is its compute.

### B576 — the bar waits behind the HDA bring-up

`unaos/crates/kernel/src/drivers/hda.rs:2277` `probe_after_root`'s gate gains `|| !crate::video::desktopbuild::settled()`
with JOBSCAN's own fallback (jobs.rs:127, `BAR_WAIT_MS` 10 s after the services open), so the codec walk runs after
the bar's first paint and not inside it. Evidence for the ~200 ms it accounts for: `[boot] phase=desktop … at=12737ms`,
then `[hda] probe deferred-start at=12960ms` and the whole walk to `[hda] audit stage=end`, then
`[desktop] built at=login ok bar_ms=425` (12737 + 425 = 13162 ms); `hda::probe_after_root` runs in the usb-pump's
`net-hda` step (main.rs:6056) and `desktopbuild::service` in its `desktop-app` step (main.rs:5999), one loop.
The 12801→12960 ms span before it is NOT explained by the wire — PROBE (desktopbuild.rs `say`, same line):
`[desktop] built … held_ms=<furniture_held wait> batt_ms=<battery wait> gap_max_ms=<longest gap between two service passes while owed>`.

### B579 — which part of `settings::service` held the render task 271 ms

`unaos/crates/kernel/src/video/settings.rs` `service()` (settings.rs:353): one nested `loginrender::stamp` per part —
launcher+appswitch, `assoc::view_service`, `nameindex::service`, `prefs::service`, `loginitems::service`,
`settingsfiles::service`, `appearance::service`, `load_for_login`, `persist`, `mode_service`+`tp_service`+`bus_changes`;
`unaos/crates/kernel/src/video/loginrender.rs`: the ids and their `WHAT_NAMES` (`settings.launcher`, …). The stamp
nesting rule (loginrender.rs `Stamp::drop`: a child that took half its parent names the record) then makes
`worst_what=settings.<part>:<ms>`. What the wire already rules out: no `[lock] spin-wait` was said in that second
(the LOCKREG scan names any `Mutex` wait past 10 ms), and the session's prefs load ran on the login worker
(`[prefs] load clamped=0` sits between `[users] login ok` and `[login] submit … on=worker`, lines 765-773).
