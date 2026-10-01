# PTRSTUTTER — the trackpad goes dark under the vug storm

## Finding (boot 17, `docs/dev/evidence/rmbp-0915/flight17/f17-boot1.log`)
- `:: EHCI-HID: ISRARM armed=1 refused=0 irq=5679 isr_rearm=5382 poll_rearm=462 depth_max=8 ringfull=272 …` — the 8-deep completion ring filled 272 times; 462 polled re-arms.
- `:: EHCI-HID: [1] EHCIDARK addr=9 ep=IN1 kind=vendor-mt … dark=76312ms max=19009ms missed<=70796`.
- `:: SMPLOAD: … busy=[97,97,89,92,97,98,91,97]` with WCPAR's seven PRIO_LOW band workers pinned on every AP.

## Mechanism
- Producer: `isr_service_ep` / `interrupt_ack` (drivers/ehci/mod.rs) lifts a completion into `ISR_RINGS`; at full ring it DECLINED, leaving the endpoint un-re-armed (dark until the pass came).
- Consumer: `service_ehci_hid` (the pass: main.rs input/usb-pump on the service core, `pal::pump_and_poll`). It decodes into `Controller`/pal state under `EHCI_HID`, so it cannot run from the ISR; the ISR-direct option is refused (decode takes locks).
- Starver: `wcpar::start()` pinned a PRIO_LOW spinner (20 ms hot, then `sleep_ms(1)`) on EVERY AP, including `smp::service_cpu()` where `usb-pump`/`input` run at PRIO_NORMAL.

## Milestones
- M1 ring: `ISR_RING` 8 -> 64; full ring = DROP NEWEST, counted in `ISR_DROPPED`, endpoint re-armed (never dark behind a starved consumer; ISR never blocks). `ISRARM` gains `ring=64 dropped=`. Self-test updated (overflow report dropped + endpoint re-armed; `ringfull-refuses=true` token kept).
- M2 consumer (REVISED after ruling "THERE IS NO RESERVING CORES"): workers on ALL online APs; worker hot-spin 20 ms -> <= 1 ms then `sleep_ms(1)`, `yield_now()` after each band. The scheduler already preempts on a higher-priority wake (sched.rs need_resched/wake path), so a PRIO_NORMAL input service should get the core within a tick. NEXT ARC if boot 18 still shows dark_max_ms > 500: a bounded `par_blit` that yields inside a band (bands are not interruptible mid-band).
- M3 verdict: `:: WCPAR: … load=idle|busy -> PASS|FAIL ::`; speedup floor (20%) only when idle; the old `done == bands` race (swap vs in-flight band) no longer fails it.

## Witness
`:: PTRSTUTTER: ring=64 dropped= dark_ms= dark_max_ms= reserved_cpu= -> PASS|FAIL ::` on the EHCIDARK cadence; FAIL = dark_max_ms > 500.

## Pins
None new (QEMU cannot reproduce the storm). x86-witness.spec:1566 `ringfull-refuses=true` row still matches; no spec row pins the ISRARM rollup field order or `[wcpar] … reason=one-per-online-ap` (checked x86-wc.spec:908 `:: WCPAR: cores=` prefix only).

## Written
Boot 18 should show `:: PTRSTUTTER: ring=64 dropped=N dark_ms=… dark_max_ms=… reserved_cpu=none(ruling) -> PASS ::` and `[wcpar] pool=8`.
