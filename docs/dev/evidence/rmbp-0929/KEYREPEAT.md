# KEYREPEAT

## Design
Finding: key auto-repeat ALREADY EXISTS (UVUG-5/6/9, PAL-TYPEMATIC): `pal.rs` `mod typematic` (report-level
tracker, `typematic_note_report`/`typematic_tick`), pumped by `main.rs` `x86_typematic_pump` (x86, `ehcihid`)
and the aarch64 service loop (~`main.rs:1677`). Constants are bench-tuned: `DELAY_MS=400`, `RATE_MS=40` (25/s),
not the 500/30 in the brief; left alone (P54b metal tuning). Missing piece: a TIMING witness — existing
`:: uvug6: typematic` proves logic with forced-due, never the delay/rate on a real clock.
Mechanism: `pal::keyrepeat_selftest` (pal.rs tail) holds 'k' 700 ms via report-level note + tick polling on
`arch::ms()`, releases with an empty report, polls 120 ms more. Spin-capped. Called from the x86 EHCI selftest
chain (`drivers/ehci/mod.rs` ~18029) and the aarch64 typematic selftest site (`main.rs:1409`).
Witness: `:: KEYREPEAT: delay_ms=<n> rate_hz=<n> first_repeat_ms=<n> repeats=<n> cancelled=<0|1> -> PASS|FAIL ::`
Spec pins: `unaos/scripts/specs/x86-wc.spec` tail (REQUIRE PASS w/ cancelled=1, FORBID FAIL). No knob.

## Written
M1: fixture + call sites + spec pins (single commit). Unverified by compiler.
