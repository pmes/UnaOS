# LIDSLEEP — rmbp-ledger B431 (MACPARITY row 34: "the lid closes to sleep")

Cut from 4ead840a (merge17). Driver arc: structured by DRIVERS-METHOD §6. Knob `UNAOS_LIDSLEEP=1` → feature
`lidsleep` (implies `smc`; x86 only — the k8-reach NA row). Module: `unaos/crates/kernel/src/video/lidsleep.rs`
(`//! CHARTER: Kernel — driver`), polled from `dimidle::service` (every desktop service pass), 1 s throttle.

**Finding.** The tree has no lid read and no sleep state. The SMC driver reads keys (battery sweep, `read_key`); the
backlight has one writer (`video/backlight.rs`, gmux index 0x74) that never writes 0 (BRIGHTFLOOR: 0 = OFF).
MSLD has never been READ on this machine: it is only named, by the index walk.

## 1. Capture
- The only capture: the `#KEY` index walk (flights 13, 14, 15, 18, 19, 20): `:: SMC-SCOUT: idx 255 = MSLD ::`
  (f13-boot1.log line 864; neighbours MSLB, MSLC, MSLF, MSLG, MSLP, MSLS, MSLT). Its VALUE, type and length are
  uncaptured. Flights 24/25 carry no `[smc]` lid line (none exists): `SMC-BATT … busy=… late=0` only.
- Plan: this arc's rung 0 IS the capture — read-only, the value byte printed at boot and on every change.

## 2. Rung ledger
| rung | hypothesis | writes | discriminator (confirm / refute) | status | alternatives |
|---|---|---|---|---|---|
| 0 premise | MSLD is the lid switch on THIS SMC and flips with the lid; nonzero = closed [ONE-SOURCE: Apple SMC key-name convention, MS* = power-management state; the polarity is unpinned] | none (SMC READ_CMD 0x10 only) | confirm: `[smc] lid=… raw=` changes when Peter closes and opens the lid (flips ≥ 2, raw differs between the two); refute: `lid=no-key`, or raw constant across a close | open | (a) MSLD is a sleep-request latch, not the switch (then it flips only after the OS acks — reads constant); (b) polarity is zero = closed (rung 0's raw-by-glass decides it); (c) the lid is MSLC/MSLS/MSLT (next boot reads those three beside it if (a)) |
| 1 blank | gmux index 0x74 := 0 turns the panel backlight off and the prior raw restores it | gmux 0x74 (the BRIGHTSLIDER register) | confirm: `[backlight] off … readback=0` then `[lidsleep] wake … readback=<prior>`, glass dark then lit; refute: readback ≠ 0, or the panel stays lit | open (exercised by `tests lidsleep` and `sleep_request`) | the gmux may floor a 0 (then readback ≠ 0: the line shows it) |
| 2 ladder on lid | lid closed → rung 1 blank, input ignored; lid open → restore | as rung 1 | applied ONLY after rung 0 is confirmed: `LADDER_ARMED` const is false; the wire prints `[lidsleep] ladder unapplied lid=<s> would=<backlight-off or restore>` | parked: reopens when rung 0 is confirmed on metal | — |
| 3 S3 | — | — | — | parked: NOT this arc (walls below) | — |

## 3. This boot's tree (one boot, read-only on the SMC)
- `[smc] lid=<open or closed> raw=<byte> len=<n> at=boot` once. `lid=no-key` → rung 0 refuted for MSLD; alt (c) next.
- Peter closes the lid ~5 s and opens it: each change prints `[smc] lid=<s> raw=<byte> flips=<n> ms=<t>`; the raw read
  while the lid was shut names the polarity (rung 0 alt (b)).
- `tests lidsleep` → `:: LIDSLEEP: msld=<ok|no-key|stuck|unbuilt> raw=<b> flips=<n> backlight_off=<ok|unarmed|fail> prior=<raw> off_rb=<rb> restore_rb=<rb> -> PASS|FAIL|SKIP ::`.
- `[lidsleep] sleep_request via=<who> backlight_off=<…>` (LOGINWINDOW's Sleep), then `[lidsleep] wake via=input restored=<raw> readback=<rb>` on the next key/pointer event.

## 4. Walls known and unapplied (not this arc)
- ACPI S3 on this firmware: no AML interpreter in the tree (smc.rs head: "no ACPI interpreter involved").
- The Kepler's state across S3 (the display engine is left RUNNING this arc; no PM rungs).
- xHCI resume (save/restore of the controller across S3; xHCI spec §4.23.2 save/restore state).
- Input ignored while closed: the input seam is `dimidle::gate`; the lid ladder's swallow joins it when rung 2 arms.

## 5. Constants
| constant | value | sources |
|---|---|---|
| MSLD key name | `MSLD` | capture: f13 `idx 255 = MSLD` · [ONE-SOURCE: capture] (meaning: rung 0) |
| MSLD read length | 1 byte | [ONE-SOURCE: none for the type — a 1-byte read returns the first value byte of any longer key; rung 0 prints `len=`] |
| gmux backlight register | index 0x74, 0 = off | upstream apple-gmux `GMUX_PORT_BRIGHTNESS` (cited at igpu.rs gmux_set_brightness) · capture f19 (0 written → dark, BRIGHTFLOOR B312) |
| poll period | 1000 ms | the brief (rung 0: a 1 s poll); the battery sweep's REFRESH_MS shape |

## 6. Unwind
- SMC: nothing written (READ_CMD only, through `smc::read_key`, serialized by its TXN lock).
- gmux 0x74: pre-image = the register's readback (`backlight::readback_now`) before the 0; restore = `set_raw_via(prior)`
  on wake / at the end of the fixture. Nothing destructive.

## 7. What the next flight reads (in order)
1. `awk 'index($0,"[smc] lid=")'` — the boot value, then the flips with Peter's close/open. Two raws that differ → rung 0
   confirmed (polarity from which raw was read with the lid shut); constant raw → alt (a)/(c); `no-key` → refuted.
2. `tests lidsleep` → `:: LIDSLEEP:` line; glass: dark for the fixture's blink, lit after.
3. `[lidsleep] sleep_request` / `[lidsleep] wake` from LOGINWINDOW's Sleep button (when B430 wires it).
4. `[lidsleep] ladder unapplied` lines — must appear once per lid change and never write.

Owed: rung 2 armed (after rung 0), input swallow while closed, Settings > Display (nothing this arc), S3.
