# BOOTVERDICTS2 — the six boot verdicts, the serial door's route word, the frozen LOGTS stamp (rmbp-ledger B616)

Arc branch `exec-rmbp-bootverdicts2`, cut from 37adc602. Source capture: `docs/dev/evidence/qemu-image26/serial.log`
(the image-26 KVM boot; line numbers below are that file's). Nothing here was run (R107): each commit compiled the
x86 kernel line once; the seat runs the legs at the fold.

## 1. The six verdicts before the announce (B597 SMALLFIX7, B605 VERDICTS)

What the wire said. `tests::tally` (tests.rs:44) counts every `-> PASS`/`-> FAIL` the verdict tap sees
(`selftest::capture`, selftest.rs:248) until `TESTS: deferred=` is announced or the first `tests` run. Six landed in
that window: `sdhc: w2 … -> PASS` (serial.log:70), `DRAINCAP` (:213), `SERWIT-1B` (:279), `PWRDRAIN` (:345),
`SERWIRE` (:429), `SERIALTX` (:439); the announce is :443 (`deferred=94 … at_boot=0`, so not a `tests-at-boot` build).
On the rMBP the five ring fixtures print SKIP (no 16550, `uart_absent`) and `w2` does not reach PASS, so metal reads
`verdicts=0:-` and QEMU `6:…`.

Mechanism. The five ring fixtures are one-shot calls on `serial_ring::mirror_service`'s fixture line (serial_ring.rs:1780
and :1787), each guarded by its own `*_DONE` latch (draincap :1952, backpressure :2099, pwrdrain :2241, serwire :3000,
sertx :3435). `w2` is `drivers/sdhc.rs` `mb_verdict` (:3892), reached from `write_multiblock_selftest` (:3937) after
`w1`'s ladder passes (:3328), inside `bring_up` BEFORE the card is published to the block layer (:3401 onward).

The change.
* The five ring fixtures: each signature line now reads `fn X_selftest() { if crate::tests::defer_polled("<name>",
  X_body, &X_DONE) { return; } X_body() } fn X_body() {` (LINE-NEUTRAL fold; the old body is `X_body`'s). The poll
  never runs the body: `defer_polled` (tests.rs tail) registers `X_body` once, using the fixture's own latch, and
  returns `true`. The registered function is NOT the polled one, so another core's `mirror_service` pass during
  `tests <name>` cannot run the body beside it (the hazard `defer_fast` has, since `running_is` reads a global
  `CUR`). The body's latch line became `if X_DONE.swap(..) && cfg!(feature = "tests-at-boot")`: under `tests-at-boot`
  `defer_polled` returns `false` and the poll runs the body once, as before; otherwise the body runs every time
  `tests` runs it. Names: `draincap`, `serwit1b`, `pwrdrain`, `serwire`, `serialtx` (x86). Registration order is the
  poll's order, so `tests` (all) keeps the order the fixture line's comment settles (sinkdrain, already deferred,
  between pwrdrain and serwire).
* `w2`: the WRITE stays at boot. It is the instrument's ordering (Step 9b: after `w1`'s PASS on the same card, and
  before `bring_up` publishes the card — on demand it would write the scratch window under a live filesystem, which
  the gates were not designed for). `mb_verdict` banks its fields (`w2_bank`, sdhc.rs tail) and its boot line prints
  `tests::boot_word` (`ok`/`NOT-OK` until the operator's first `tests` run, PASS/FAIL under `tests-at-boot`;
  `REFUSED` unchanged); `tests sdhcw2` prints the banked fields with PASS/FAIL and `at=boot`.
* `tests verdicts`: `moved=` gains the five (17 → 22), `reworded=` the one (18 → 19).

No spec REQUIREs any of the six boot lines (`grep -n` over unaos/scripts/specs/: the only rows naming these fixtures'
tags are the `[sertx]` census rows, x86-default.spec:246, x86-wc.spec:684, x86-witness.spec:1577, which are the
unconditional `tx_rollup`, not SERIALTX's fixture, and are untouched).

Expected wire (QEMU, not `tests-at-boot`): `:: sdhc: w2 armed=1 lba=… n=4 verify=ok busy_ms=0 restore=IDENTICAL
reason=none -> ok ::` at boot; no DRAINCAP/SERWIT-1B/PWRDRAIN/SERWIRE/SERIALTX line before the announce;
`:: VERDICTS: before_deferred=0 moved=22 reworded=19 announced=1 tags=- -> PASS ::`; `tests smallfix7` →
`verdicts=0:-`; `tests draincap` → `:: TESTS: run draincap ::` then the DRAINCAP line.

## 2. The serial door's route word (B567's wording item)

What the wire said. serial.log:953-958: `[serialdoor] key=printable win_focus=0x0 ring=0x0 -> shell(headless)` on a QEMU
boot with no Kepler takeover, then `tests door` read `headless=0 ran=0 window=0 lines=0`: the bytes never reached the
headless door.

Mechanism. The witness is printed by `wc_route_event` (arch/x86_64/syscall.rs:7312) with `serialdoor::route_word()`
(serialdoor.rs:44), which read only `SHELL_WIN`. The byte's real route is decided by the render service's key arm
(main.rs:6814-6830): `desktop` (main.rs:6380, `desktop_owns_backdrop()` = `desktop_uefi::is_active()`, main.rs:6225)
false hands it to the BACKDROP console's `handle_key`; true with a shell window to that window; true with none to
`serialdoor::key` (the headless door).

The change. `route_word` asks `desktop_uefi::is_active()` first: false → `backdrop-console`, else `window`/`headless`
as before (one line, LINE-NEUTRAL). The witness reads `-> shell(backdrop-console)` on QEMU. The bare form
`-> backdrop-console` would need the format string in arch/x86_64/syscall.rs:7312 (`-> shell({})`), outside this arc's
list; the exact change, if wanted: `"[serialdoor] key={} win_focus={:#x} ring={:#x} -> {}"` with `route_word()`
returning `shell(window)`, `shell(headless)`, `backdrop-console`. Limit: `desktop` is latched once at the render
service's entry (main.rs:6380) and `is_active` is read per key; they differ only if the takeover lands after the
render service started, which the boot order (takeover during PCI enumeration) does not do.

Specs. The only row on this witness is x86-witness.spec:1490, `PENDING \[serialdoor\] key=0x[0-9a-f]+ … -> shell`
(PENDING, not REQUIRE; its `key=0x…` already does not match the current `key=printable` class word). The new word
keeps the `-> shell` prefix, so the row's reading is unchanged. x86-wc.spec:229/230/234 pin `:: SERIALDOOR:` (the
fixture's verdict), not this witness.
