# KVBLANK8 — why the Kepler vblank interrupt delivers once, proven on one boot, then fixed

CHARTER: Kernel — driver. Ledger B318. Branch `exec-rmbp-kvblank8`. Code: the tail of
`unaos/crates/kernel/src/drivers/gpu/kepler_vblank.rs` (section `KVBLANK8`), one helper at the tail of
`arch/x86_64/apic.rs`, one rollup line in `video/wcg.rs`, the `tests kvblank8` registration in `tests.rs`.

## Finding (boot 19, `rmbp-0915/flight19/f19-boots.log`)

`:: VBJITTER: period_us=16669 jitter_us=183 … isr_calls=1 … -> PASS ::`: the period is the POLLED
`HEAD_STAT.VERT` counter; the ISR ran once for the whole boot. Read off the same log:
`[MSI] Enabled on 1:0.0: cap@0x68 … MsgCtl=0x0081` (MSI enable on, 64-bit, per-vector masking NOT
capable, so no MSI mask or pending bits exist on this function), the vector close
`irq=1 vbl_delta=60 … intr_or=04000000 line_or=00000000`, the ISR books `acks=1 ack_via=1 rearm_readback=1
msi_rearms=1`. So the one message was the ALREADY-LATCHED status (status was `02000003` through the whole
read-only census), the W1C ack cleared it, the head enable read back armed, the KVBLANK7 MSI re-arm ran,
and no second message came. The ISR does call `apic::eoi()`, and the xHCI/NIC vectors 0x40/0x41 share
0x44's priority class and keep working, so a missing EOI is unlikely but unproven: nothing has ever read
the LAPIC ISR/IRR bits for 0x44, PMC_INTR_0 after the ack, or the PMC line enable across the ISR.
Nothing in the ISR toggles `INTR_ENABLE_HOST` (0x140 bit 0): if any PDISPLAY source stays asserted after
the vblank ack (`INTR_HEAD_STATUS` also carries bits 1 and 25), PMC bit 26 never falls, the MSI logic
never sees a new rising edge, and that is "fires once" exactly.

## Milestones

* **M1 the instrument.** `kv8_snapshot(at)` prints ONE line of the whole path read back from hardware:
  head 0/1 `INTR_HOST_HEAD_EN`/`INTR_HOST_HEAD`/`INTR_HEAD_STATUS`, `INTR_SUMMARY`/`INTR_HOST_SUMMARY`,
  `PMC_INTR_0`, `INTR_ENABLE_HOST`, `INTR_MASK_HOST`, `INTR_LINE_HOST`, the MSI capability (MsgCtl,
  address pair, data, mask/pending when per-vector capable, else `none`), PCI COMMAND bus-master and
  INTx-disable, STATUS INTx bit, the LAPIC ISR/IRR bit for the vector plus TPR/PPR on the TARGET cpu
  (`lapic_valid=1` only when read on the cpu the MSI address names), IOAPIC `na` on the MSI wire, and the
  ISR's books (entries, acks, head re-arms, PMC re-arms, MSI re-arms, EOIs checked/missed, PMC_INTR_0 and
  host summary after the last ack). At takeover (`rung3_arm`), 100 ms after the first ISR, and at 2 s.
  Then a 60 Hz census for 1 s: per sample, is the display status SET while the ISR has not run, is PMC bit
  26 set, is the LAPIC IRR/ISR bit set. Verdict
  `:: KVBLANK8: lost_at=display|pmc|msi|apic|none isr_calls=<n> ::`. Knob `UNAOS_KVBLANK_TRACE=1`
  (feature `kvblank_trace`) runs it at takeover from a task pinned to the MSI's target cpu and holds the
  rung-3 window open until it is done; otherwise it runs only under `tests kvblank8` (R80).
* **M2 the fix (unconditional under `nvidia-kepler-vblank`).** In the ISR, in this order: EOI CHECKED
  (after `eoi()` the LAPIC in-service bit for the vector is read back, `eoi=ok|missing`), and the PMC
  RE-EDGE: `INTR_ENABLE_HOST` hw bit cleared at ISR entry, the acks done, `PMC_INTR_0` read back, the
  MSI re-arm also written through the BAR0 PCI-config mirror (`0x088000 + 0x68`, envytools PPCI), and the
  hw bit set again with a read-back — a 0→1 on the line enable re-presents any still-pending source as a
  fresh edge. A same-frame storm guard (64 entries without `HEAD_STAT` vblank_count moving) cuts at PMC.
  The trace's `lost_at=` says which of these the bench boot needed; a residual `lost_at=` names the next.
* **M3 the pacing.** `irq_source()` refuses the ISR counter on a core that is the MSI target with IF=0
  (the IRQ-masked `beam::hold` could never see its own ISR, and one give-up demoted the source for the
  boot); `tests kvblank8` PASS keeps the vector live and clears the demotion, so `wait_next_edge` counts
  ISR events instead of sampling BAR0. `[wc-h] vbl win=<id> vbl_isr=<per-second> vbl_src=irq|poll`
  rides each rollup.
* **M4 `tests kvblank8`.** Re-opens the window on the shell's cpu (MSI re-targeted there), runs the M1
  instrument for 1 s, asserts `isr_calls >= 30 && lost_at=none`. Witness
  `:: KVBLANK8: lost_at=<x> isr_calls=<n> eoi=<ok|missing> rearm=<ok|missing> -> PASS|FAIL ::`.
  PASS leaves the vector live (M3 then paces on it); FAIL restores every captured word with read-back.

## What a metal boot should print

`tests kvblank8` on the desktop: `:: KVBLANK8: snap at=takeover …`, `… at=isr+100ms …`, `… at=t+2s …`,
`:: KVBLANK8: census samples=60 …`, `:: KVBLANK8: lost_at=none isr_calls=60 ::`, then
`:: KVBLANK8: lost_at=none isr_calls=60 eoi=ok rearm=ok -> PASS ::`, and the rollups then carry
`[wc-h] vbl win=… vbl_isr=60 vbl_src=irq`. A FAIL names the stage; that is the boot's finding.

## Owed

The fix is the M1-predicted one and flown by nobody; if the boot prints `lost_at=` other than `none`,
the named stage is the next arc. IOAPIC redirection-entry readback for the INTx wire is not printed
(this machine runs MSI). No QEMU pin (no Kepler, R78).
