# TPMODE (rmbp-ledger B459) — the trackpad's two 2 s mode-switch timeouts, every boot

Cut from merge18 @ f4e0613c. PERF-2026-10-06 §F2. Structured by DRIVERS-METHOD §6.

**Finding (read from the captured wire, flights 24 and 25, six boots):** `bcm5974_mode_switch` tries the HID 1.11
conformant `wIndex = intf` (=1) FIRST. On every boot the device accepts the SETUP and NAKs the DATA stage until the
2000 ms `hw_wait_budget` expires, for the GET and again for the SET (the readback is skipped because the SET failed):
`STOP-NOTE EP0 DATA timeout addr=9 req=0xa1/0x01 token=0x80088d82` (status 0x82: Active, IN PID, CERR 3 — still being
NAKed), then `req=0x21/0x09 token=0x80088c82`, 2 s apart. The second attempt at the legacy `wIndex = 0` then latches
silently (its witness is `bootlog_println!`, off the metal wire); `[tp] mode-mismatch readback=vendor …` (f24 13:04:51)
proves a readback of the vendor selector, which only the index-0 attempt could have produced. `kbd-armed` 1.92–2.29 s,
`trackpad-armed` 6.00–6.29 s: the 4 s is the dead index-1 attempt and nothing else.

**Seam:** none new — the EHCI driver (`drivers/ehci/mod.rs`, kernel-by-ruling). No new file, no knob, no verb.

## 1. Capture
The working state is captured: flight 11 (`GET_REPORT(feature) addr=8 intf=1` at `wIndex=0`, `got=8b byte0=0x08`, the
stream after the SET), and flights 24/25 (the index-0 attempt latches on 6 of 6 boots; index 1 times out 6 of 6).

## 2. Rung ledger
| rung | hypothesis | discriminator | status |
|---|---|---|---|
| R0 (premise) | the 4 s is the index-1 attempt's two data-stage NAK timeouts, and index 0 answers | confirmed by: two STOP-NOTE DATA timeouts at `widx=1` per boot, no failure line at `widx=0`, `readback=vendor` | confirmed from f24/f25 (quoted above); a flight line under this arc's dump is owed (§7) |
| R1 | the device owns the mode Feature report at request index 0 (wsp.c TYPE2 "request index 0"), so index 0 first costs no timeout | refutes: `[tp] mode req=1 … widx=0 status=timeout`; confirms: three `status=ok` lines, `TPMODE … timeouts=0` | unflown (this boot) |
| R2 (alt) | the vendor interface wants the request after SET_IDLE | would need index 0 to fail too — it does not; parked, reopens if R1 refutes | parked |
| R3 (alt) | the 2 s is a wait on a response never sent (bound the wait to 100 ms) | the device NAKs; a bound would still cost 2 × 100 ms per boot and still never latch at index 1; parked as the fallback's cost, reopens if index 0 ever fails to latch | parked |

## 3. This boot's tree
One candidate (R1): the index that latched is tried first; the conformant index is the fallback, reached only when
index 0 does not latch. Dump: one `[tp] mode req=<n>` line per EP0 request (seq, bmRequestType/bRequest, if, widx,
wValue, status ok/stall/timeout/hse, bytes, ms), then `:: TPMODE: requests= timeouts= first=index0 latched= armed_ms= …`.
Reading: `req=1..3 widx=0 status=ok` and `timeouts=0` → R1 confirmed; `req=1 widx=0 status=timeout` → R1 refuted,
reopen R2/R3; `widx=1` lines present → index 0 did not latch, read its readback byte in the `[tp] mode wrote=` line.

## 4. Walls known and unapplied
None for this wall. The index-1 NAK (a conformant HID device STALLs an unsupported request, this one NAKs) is a
device fact, recorded, not a wall we own.

## 5. Constants
| constant | value | sources |
|---|---|---|
| `BCM5974_MODE_REQ_INDEX` | 0 | wsp.c TYPE2 parameter block (request index 0, BSD-2) · capture f11 + f24/f25 (latched at 0, 6/6) |
| `TPMODE_ARMED_BOUND_MS` | 2500 | PERF-2026-10-06 §F2 (≤ 2.5 s) · capture f24 `kbd-armed` 1.92–2.29 s [ONE-SOURCE: review + capture] |

## 6. Unwind
No new register write. The mode report write is the same read-modify-write as before, at the index that was already
written on every boot; only the order of the two attempts changes. Not destructive.

## 7. What the next flight reads
1. `awk 'index($0,"[tp] mode req=")'` — three lines, `widx=0`, `status=ok`, `ms=` single digits.
2. `awk 'index($0,":: TPMODE:")'` — `requests=3 timeouts=0 first=index0 latched=yes armed_ms=<n> bound=2500 -> PASS`.
3. `awk 'index($0,"trackpad-armed")'` — within ~0.1 s of the second `kbd-armed` (≈ 2.1–2.4 s).
4. No `STOP-NOTE EP0 DATA timeout … req=0xa1/0x01` line on the boot.

Owed: R0's own quoted line under this dump (the next flight); the index-1 fallback still costs 4 s if index 0 ever
fails to latch (R3, parked).
