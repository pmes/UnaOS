# KVBLANK7 — the vblank interrupt delivers ONCE, and the jitter is a polled-timestamp number

## Finding (boot 18, f18-boot1.log)

Peter: "the vugs are roller coastering; a minimized and restored vug runs really high fps for a little bit".

    [1098806ms] :: kepler: vblank head=0 count=65456 period_us=16669 jitter_us=16944 ... mode=irq vbwaits=748
                vbwait_us=5779826 ... seen=61293 tight=19434 vbwait_mode=poll vbwait_irq=0 vbwait_poll=1037
                isr_calls=1 acks=1 rearms=1 rearm_written=00000001 rearm_readback=1 ::
    [13020ms]   :: kepler: vblank-intr vector close ... irq=1 vbl_delta=60 rearms=1 ... mode=irq deliver=msi ... intr_or=04000000

Read against the code, NONE of hypotheses (a)-(c) as framed is what the log says:

* `isr_calls=1` for the WHOLE BOOT. The ISR (kepler_vblank.rs `kepler_vblank_isr`) ran ONCE in the 60-vblank window
  (`irq=1 vbl_delta=60`): delivery ratio 1/60, `keep` (>= 90%) false, `IRQ_LIVE` cleared at close. `mode=irq` is
  `IRQ_MODE`, set by the FIRST delivery (`deliver=msi`) — it says "the wire delivered once", not "the interrupt paces".
  `vbwait_mode=poll vbwait_irq=0` on every census agrees: every wait ran on the polled counter. So the KVBLANK4 and
  KVBLANK5 fixes (re-arm of the head enable, W1C of INTR_HOST_HEAD) did not make the wire deliver a SECOND message.
* `jitter_us` is not an ISR number at all. `VbAcc::fold` (`pmin`/`pmax` of `(now-tc)/(n-tn)` over consecutive TIGHT
  edges) takes `now` AFTER `scanout_beam`'s BAR0 read and AFTER the previous sample's stamp; a core preempted between the
  read and the `rdtsc` (the pump task, wcpar workers, the storm) stamps an edge up to a frame late, and the
  other core's `sample_cyc` makes it "tight". Jitter grew 5248 -> 7910 -> 16944 us with the load, mean period held at
  16669: a timestamp-lateness signature, but unproven, so M1 measures it.
* `beam::hold`'s wait arm (video/beam.rs ~:290) is a TEARING hold (it waits only when the raster is in the rect's hazard
  zone), not the frame pacer. The pacer is `present_pace` (syscall.rs), opt-in under `vsyncpace`.

Unproven but the best candidate for "exactly one delivery": NVIDIA MSI needs a RE-ARM after each message — nouveau's
`nvkm_pci_msi_rearm` (nv40 and later) writes 0xFF to PCI config byte 0x68. Nothing in this ISR does that.
Second finding (fps=2000 after restore): `set_hidden(.., false)` only wakes the app; `present_pace` has no state for
"just restored" (default build: an empty stub), so the first presents after the wake are un-gated.

## Mechanism / milestones

* M1 `kepler_vblank.rs`: 64-entry ISR ring `(tsc, vline_at_irq)` and a 64-entry EDGE ring `(tsc, n, from_pump)`;
  `dt_hist=[<8ms,8-24,24-40,>40]` for each, `missed=` (dt >= 25 ms) `doubled=` (dt < 8.3 ms) vs the 16.67 ms period,
  appended to the census line, plus `:: VBJITTER: period_us= jitter_us= missed= doubled= -> PASS|FAIL ::` (FAIL > 4 ms).
  Poll-edge pairs count only when both are non-pump samples one vblank apart: symmetric missed/doubled = lateness
  of the stamp (poll artefact); ISR histogram missed without doubled = lost messages.
* M2 ISR: ack W1C FIRST, read the latch back; if still set try INTR_HEAD_STATUS W1C, then the enable toggle, and record
  which one cleared (`ack_via=1|2|3|0`); then re-arm the head enable; then the MSI RE-ARM (config byte 0x68 = 0xFF,
  `msi_rearms=`, UNVERIFIED for GK107). The wait takes ONE source: the ISR counter only when the vector is kept live AND
  delivered within 3 periods; otherwise the beam sampler. A give-up on the irq source demotes to poll (printed once).
* M3 syscall.rs: `set_hidden(.., false)` arms `SLOT_REPACE[slot]`; the next present of that slot, before it takes any
  lock, waits for the next vblank (`wait_next_edge`, bounded 2 frames; else sleeps to the next 16 ms boundary) and, under
  `vsyncpace`, re-bases the window's deadline to now + one frame. Witness: `[wpace] restore win=N repaced=1`.

## Witness / pins

`:: VBJITTER: ... -> PASS|FAIL ::` once per census; `:: kepler: vblank head=0 ... dt_hist=[..] missed= doubled= ack_via= msi_rearms= ::`.
No QEMU pins (no Kepler, R78).

## Written

* M1/M2 `drivers/gpu/kepler_vblank.rs`: line `:: kepler: vblank-dt dt_hist=[<8ms:..,8-24:..,24-40:..,>40:..] poll_dt_hist=[..] isr_missed= isr_doubled= poll_missed= poll_doubled= isr_stuck= ack_via= msi_rearms= irq_demoted= ::`
  right after each `:: kepler: vblank head=` census, and `:: VBJITTER: period_us= jitter_us= missed= doubled= isr_calls= bound=jitter_us<=4000 -> PASS|FAIL ::`.
  Boot 19 reading: `isr_calls` far above 1 after the window and `msi_rearms` == `isr_calls` = the MSI re-arm was the missing step (hypothesis d'); `ack_via=1` the W1C
  guess was right, `2|3` the alternative that cleared it, `0` none did (stuck grows). `isr_missed>0, isr_doubled=0` = lost messages (c); poll missed/doubled
  symmetric with the ISR histogram clean = late timestamps, i.e. a measurement artefact (b). `wait-source demoted irq->poll` is printed once if an irq-trusting wait gave up.
* M3 `arch/x86_64/syscall.rs`: `[wpace] restore win=N slot=S repaced=1 via=vblank|sleep` on the first present after an unminimise
  (cap 64 lines). fps after a restore should start at the panel rate, not ~2000.
* Not pinned (no QEMU, R78; Kepler only).
