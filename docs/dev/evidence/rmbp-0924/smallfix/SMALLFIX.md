# SMALLFIX — B74 (the strip's forgotten debt), B70 (the uncounted refusal), S32 (judged) — 2026-09-24

Cloud session, QEMU 8.2.2 TCG (`UNAOS_QEMU_MACHINE=pc-q35-8.2`). Gate = x86-wc.spec's own line:
`UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_QEMU_FULL=1 ./arroyo test 240`, then
`mbench.py --replay target/serial.log --spec scripts/specs/x86-wc.spec --platform x86`. Serial read
with awk. Every run's rc=1 is the container's `:: SOCK-3: ring-3 tcp round-trip FAIL` (environmental,
proven on the untouched tip earlier this session); the spec replay is the verdict here.

## ON tree — both fixtures PASS, spec 38/38 (was 36 pins; +2)
```
:: PRTSCR: refused — capture in flight (another task holds the capture door; a key request is re-armed and runs after it) ::
:: PRTSCR-REFUSE: inflight door -> named=1 counted=1 slicing=0 -> PASS ::
:: PRTSCR-DIR-FIX: theme=crispy dir=/home/una/Desktop home=/home/una -> HOME/UNA/DESKTOP (want HOME/UNA/DESKTOP; "Desktop" is 7 chars and the FAT create path is 8.3 ONLY, so legal83 is the necessary c
:: PRTSCR-DIR-FIX: no session -> REFUSED reason=no-session plan_none=true live=no-login-built captures 0->0 bytes=0 (no name chosen, no volume touched, no directory entry made) -> PASS ::
:: STRIPVAC: box=16x8+0+792 -> 8x8+0+792 uncovered=1 restored=1 flat=0 owed_px=64 scene=no :: PASS ::
[strip] settle tenant=stripvac box=16x8+0+792 erased=yes -> DEBT-PAID
:: STRIPVAC-DEBT: planted=1 paid=1 settled=1 owed_after=0x0 -> PASS ::
════════════ MBENCH VERDICT — x86-wc.spec vs target/serial.log ════════════
  ✅ MBENCH PASS — 38/38 required witnesses, 0 forbidden hit(s), 3070 lines scanned [full wall 258.9s]
```
`[strip] settle … -> DEBT-PAID` is the mechanism's one-shot line: the fixture planted a debt for its own
tenant and `settle` paid it. `dock.rs` and `menubar.rs` call `strip::settle` ahead of every paint pass
and in their hide arms, so a real declined erase (`-> UNERASED` / `-> STALE-ENDS`) is retried on the next
pass instead of forgotten. On this leg no real decline occurred (the counts above are the fixture's).

## Go-red B74 — `settle` mutated not to clear the debt
```
:: STRIPVAC-DEBT: planted=1 paid=1 settled=1 owed_after=0x318000000080010 -> FAIL ::
════════════ MBENCH VERDICT — x86-wc.spec vs target/serial.log ════════════
  ❌ MBENCH FAIL — 36/38 required witnesses, 3 forbidden hit(s), 3068 lines scanned [full wall 263.0s]
```
## Go-red B70 — the door's `refuse(..)` reverted to a bare `.report()`
```
:: PRTSCR-REFUSE: inflight door -> named=1 counted=0 slicing=0 -> FAIL ::
════════════ MBENCH VERDICT — x86-wc.spec vs target/serial.log ════════════
  ❌ MBENCH FAIL — 35/38 required witnesses, 3 forbidden hit(s), 3066 lines scanned [full wall 262.4s]
```
(That go-red build was also the first with the fixture moved into `dir_fixture`, and the move had
dragged `selftest_once`'s three latches along — which is why its two PRTSCR-DIR-FIX pins also read
missing there. Put back before the ON run above, which has all four PRTSCR lines.)

## The first ON attempt (fixture in `selftest_once`)
`:: PRTSCR-REFUSE:` never printed: `selftest_once` runs only behind the `prtscrst` feature, which the
wc leg does not carry. `dir_fixture` runs from `service()` on every leg, so the fixture lives there.
Spec 37/38 on that run — the one missing pin was this one.

## S32 — does not reproduce
The row said four furniture `rollup(scope)` functions had no callers. At the tip:
```
unaos/crates/kernel/src/video/dock.rs:1148:    LEDGER.rollup("dock", scope, dock_tail!());
unaos/crates/kernel/src/video/dock.rs:1388:    rollup("selftest"); dockid_selftest(); // DOCKID — the tile-IDENTITY ba
unaos/crates/kernel/src/video/crystal.rs:1193:    rollup("selftest");
unaos/crates/kernel/src/video/winmenu.rs:1890:        rollup("selftest");
unaos/crates/kernel/src/video/winmenu.rs:2012:        rollup("selftest");
unaos/crates/kernel/src/video/winmenu.rs:2032:    rollup("selftest"); #[cfg(any(all(target_arch = "x86_64", feature = "w
unaos/crates/kernel/src/video/winmenu.rs:2280:    pulsewin::rollup("pulsequit");
unaos/crates/kernel/src/video/menubar.rs:2244:    rollup("selftest");
unaos/crates/kernel/src/video/cursor.rs:1328:    cursor11_rollup("desk");
```
Each module's selftest calls its own rollup at its end; LEDGER S32 → dropped, with this listing.
