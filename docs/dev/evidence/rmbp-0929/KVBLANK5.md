# KVBLANK5 — the Kepler vblank ISR re-arms on the metal (boot 16 finding)

## Design
Finding (f16.log): KVBLANK4 fixture `irq=62 vbl=62 ... PASS` at 7390ms, yet the live close at 12544ms reads `irq=1 vbl_delta=60 rearms=0 ... deliver=msi intr_or=04000000 line_or=00000000`; census `vbwaits=180 vbwait_us=1591253`.

Mechanism (drivers/gpu/kepler_vblank.rs):
- `rearms=` was `WIN_SEEN`, incremented only in `rung3_run`'s POLL re-arm branch (en&vb==0). It stayed 0 because the ISR's re-arm DID keep the enable set; it never counted ISR re-arms. Not an early return, not mode-gated.
- The real cause of irq=1: the ISR never acked the PDISPLAY latch. `intr_or=04000000` = PMC bit 26 stays raised; MSI is an edge on assertion, so no further message while the source stays asserted. The fixture models "enable set => message", which the metal does not.
- vbwait 8.8 ms/wait is a half-frame average, not proof of polling: `hold` runs IRQ-masked so the ISR cannot advance the counter inside a wait; the polled sampler does. Also the interrupt was disarmed at close, so nothing fed the counter afterward.

Milestones: M1 ISR books (`isr_calls acks rearms rearm_written rearm_readback pending_pre stuck poll_rearms kept_live`, line `vblank-isr books`, and on the census line); M2 ISR = read INTR_HOST_HEAD, disarm, ack (write vblank bit to INTR_HOST_HEAD, W1C per nvkm gf119 — unverified on this die; `stuck=`/`pending_pre=` are the witness), re-arm, readback; storm now = 4096 consecutive un-cleared acks, not 4096 entries (60 Hz would have cut the line after 68 s); M3 close keeps the interrupt armed when irq/vbl_delta >= 90%; `vbwait_mode=irq|vbwait_irq=|vbwait_poll=` on the census line (irq if the counter moved via an ISR during the wait).

Edge vs level (R71): seat's call = edge with explicit latch ack; storm floor remains.

## Written
Boot 17 close line should read `irq~=60 vbl_delta=60 rearms>0`, `:: kepler: vblank-isr books isr_calls=60 acks=60 rearms=60 rearm_written=00000001 rearm_readback=60 pending_pre=60 stuck=0 ... kept_live=1 ::`. If `stuck` grows the W1C guess is wrong (read `pending_pre`/`stuck`). Spec: QEMU fixture line `KVBLANK4 ... rearm_written=00000001 vbwait_mode=poll` in x86-witness.spec.
