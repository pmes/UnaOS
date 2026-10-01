# SERIALLOCK — a line lock on the wire

## Finding (boot 17, `docs/dev/evidence/rmbp-0915/flight17/f17-boot1.log`)
197 wire lines carry two witnesses merged: `[ 120009ms] :: VUGART: frames=1 … thr=streak<=1:: VUGART: frames=2 …`
and `… severity=2:: kepler: vblank head=0 …`. Every awk read of the capture undercounts; spec lanes can miss pins.

## Mechanism
- `arch/x86_64/serial.rs` `_print` is atomic per sink call (staging ring slot, `ftdi::mirror` under `RING`),
  but a line was never one call: `serial_print!` fragments end in a separate `serial_println!`, and
  there was no lock spanning the sinks (UART stage, fbcon, ftdi mirror, selftest, flight recorder).
- The merged first half lacks its `\n` and the second half lacks its logts prefix (bare staged fold,
  `drivers/xhci/ftdi.rs` `drain_staged_into`) — two writers' bytes met inside one capture line.

## Milestones
- M1 `src/serial_line.rs` `emit(args, nl)`: format the whole line into a 512-byte stack buffer
  (`…` on truncation, `[serial] trunc` once), then ONE critical section (`LINE_BUSY`) around `_print`.
  Masked callers (ISR, deadman tick) spin 4096 then PARK in an 8-slot deferred ring drained by the next
  unmasked print (`deferred=`); unmasked callers bypass after 2^22 spins (`bypass=`) rather than
  deadlock on a nested print; panic mode bypasses entirely. Macros in both arches' `serial.rs` call it.
- M2 `:: SERIAL: lines= merged_fixed= trunc= deferred= defer_lost= bypass= -> PASS ::` once a second from
  `serial_line::census_poll`, folded onto `serial_ring::mirror_service` (every lane, both arches).
  `merged_fixed` = lines that had to wait on the lock, i.e. that would have been exposed to interleave.
- M3 `unaos/scripts/serial-merged.sh <log>` counts lines with two `:: NAME:` heads (boot 17: 197; want 0).

## Spec pins
`x86-default.spec`: REQUIRE `:: SERIAL: lines=[1-9]… -> PASS ::`. No knob.

## Written
Boot 18 should show `:: SERIAL: lines=N merged_fixed=M trunc=T deferred=D defer_lost=0 bypass=0 -> PASS ::`
roughly once a second, and `serial-merged.sh` should print 0.
Known limits: the deferred ring is global (not per-CPU); a `serial_print!` fragment is atomic itself but
fragment+println pairs on different cores can still interleave BETWEEN the two calls (convert such
pairs to one `serial_println!`); lines over 508 bytes are now cut (were 1536 in the staging slot).
