# KVBLANK4 — the vblank ISR's ack re-arms the head enable (rmbp-ledger B192's falsifier; R71), 2026-09-24

Flights 13–15: `vector close head=0 irq=1 vbl_delta=62 … mode=irq deliver=msi` — ONE message per ~60-vblank
window. Prep: `docs/dev/evidence/rmbp-0924/prep/KVBLANK4.md`: the register identities (0x140 enable-host,
0x640 per-source mask, `DISP_INTR_HOST_HEAD_EN` per head) are right; the defect is two uncoupled clocks —
`kepler_vblank_isr` acks by writing the head enable back to its pre-arm value (a DISARM) and never re-arms,
while the intended re-arm lives in `rung3_run`, driven by the compositor's poll cadence. Peter (R71):
"whichever is the best edge or level, i'm not sure" — the seat's call.

## The fix (`drivers/gpu/kepler_vblank.rs`)

- `isr_head_en(entry) -> u32` = `entry | vblank bit`: the ISR's ack write keeps the vblank bit set on the SAME
  write. Pure, so the fixture drives the real function.
- Edge vs level: the line is RE-ARMED, not held. If the source proves level-behaved and the enable write alone
  does not deassert it, `IRQ_STORM_CAP` still drops `INTR_ENABLE_HOST` to its entry value and prints — the
  existing second floor. `rung3_run`'s poll-driven re-arm stays as the belt.
- `selftest_rearm()` (folded beside `selftest_period()`): 62 simulated vblanks; a message is delivered iff
  the head enable's vblank bit is set; every message runs the real `isr_head_en`; `broken` is the old ack
  (entry only) and must read exactly 1 — the flight shape. Pinned in `x86-witness.spec` (the bench replay
  spec; `UNAOS_KEPLER_VBLANK=1` arms the module).

## Proof (same wc-lane build as SMPLOAD, `UNAOS_KEPLER_VBLANK=1`)

```
:: KVBLANK4: irq=62 vbl=62 ratio_pct=100 fixed_isr=rearm broken=1 -> PASS ::
```
GO-RED BY MUTATION (`isr_head_en` returning `entry`; restored byte-identical):
```
:: KVBLANK4: irq=1 vbl=62 ratio_pct=1 fixed_isr=rearm broken=1 -> FAIL ::
```
QEMU has no Kepler, so the arithmetic is what QEMU proves; the metal decides edge vs level.

## Boot 16 reads

The R3 close line: `irq≈vbl_delta` (tens per window, not 1) with `deliver=msi`. `IRQ_STORMED`/the storm cap
firing means the source is level-behaved under this ack and the next rung is a status-side clear; a silent
`irq=1` again means the enable write is not what gates delivery.
