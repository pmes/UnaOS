# CLOCKCORE (rmbp-ledger B397) — the global clock off the BSP's tick

## Finding (the wire, then the code)
- f25-boots.log line 4377: `[play] dec spawn path=/system/test-f/TEST.FLAC jid=1 stack=65536 cpu=0` is the LAST
  kernel line until the seat's next mark — no `[vugfps]`, no PWR, no stall line (flight 24 card 3: the same).
- `arch::ms()` = `apic::ticks()` = `APIC_TICKS`, advanced ONLY by cpu 0's timer ISR (interrupts.rs, the
  `cpu_index == 0` arm). Every masked span on cpu 0 (`with_unafs_attempt` runs every UnaFS transaction under
  `without_interrupts`) stops the clock for the whole machine; deadman's `tick()` is BSP-only too. Every core
  already arms its own LAPIC timer (`apic::init` → `init_timer`, per-core `percpu.ticks`), so the per-core tick
  exists: what was missing is a clock that does not depend on any core taking an interrupt.
- `other_dispatching_cpu()` = lowest dispatching core other than the caller: always 0 from an AP.
- No HPET driver in this tree: the reference clock is the ACPI PM timer (the one `apic::calibrate` uses).

## Seam
Kernel (arch/x86_64 time) — `kernel-by-ruling` (B397: the seat's sched ruling). New file
`arch/x86_64/clockcore.rs`; one-line hooks in apic.rs (start at calibrate, witness after SMP), interrupts.rs
(the ISR hook), smp.rs (BSP publish / AP sync), mod.rs (`IrqMask` / `without_interrupts` record the site),
sched.rs (`other_dispatching_cpu`), deadman.rs (any core's tick), tests.rs (registration). No knob: the TSC clock
arms only when calibrated AND invariant (CPUID 0x8000_0007 EDX bit 8), else the BSP tick stays the clock.

## Milestones
- M1 `ms()`/`ticks()` from the invariant TSC: `BASE_MS + (tsc - BASE_TSC) * 1000 / hz` (a 64.64 multiplier),
  continuous with `APIC_TICKS` at the switch, globally monotonic (`LAST_MS` fetch_max); per-core offset measured at
  AP bring-up against the BSP's published TSC (min of 64 samples; within 10 us = synced = 0), applied only when an
  AP is out of sync. `report_tick_rate` keeps measuring the BSP heartbeat (`APIC_TICKS`) honestly.
- M2 masked sections NAMED: `without_interrupts`/`IrqMask` are `#[track_caller]`; the outermost mask records
  (cpu, t0, site); on exit a span > 10 ms prints `[irq] masked section fn=<name> ms=<n> cpu=<c> live=0` once per
  site; any OTHER core's timer tick scans every 50 ms and names a section still masked past 10 ms
  `[irq] masked section fn=<name> ms=<n> cpu=<c> live=1` (the wedge seen from outside). `fn=` is `with_unafs`
  for the UnaFS transaction, else `<file>:<line>`.
- M3 the BSP-only duties off cpu 0: deadman's tick runs on whichever core's tick reaches the deadline first (CAS);
  `other_dispatching_cpu()` returns a worker-pool core when one exists.
- M4 witness: boot `[clock] source=tsc hz=<n> invariant=1 per_core_tick=1 cores=<n> skew_max_cycles=<n>
  offsets=none (CLOCKCORE)`; `tests clock` → `:: CLOCKCORE: source=tsc tsc_ms_vs_hpet_ms=<d> ref=pm window_ms=250
  masked_clock_ms=<n>/20 masked_max_ms=<n> masked_sites=<n> cores_ticking=<n>/<n> -> PASS ::`.

## Witness (metal)
```
[clock] source=tsc hz=2693849305 invariant=1 per_core_tick=1 cores=8 skew_max_cycles=0 offsets=none (CLOCKCORE)
[irq] masked section fn=with_unafs ms=<n> cpu=<c> live=<0|1>        (only if a span crosses 10 ms)
:: CLOCKCORE: source=tsc tsc_ms_vs_hpet_ms=0 ref=pm window_ms=250 masked_clock_ms=20/20 masked_max_ms=20 … cores_ticking=8/8 -> PASS ::
```
And the FLAC wedge, if it recurs: the wire keeps talking (vugfps, PWR, the decoder guard on a live clock), with an
`[irq] masked section … live=1` naming the site if cpu 0 is stuck masked.

## Owed
- `wcser_overdue_probe` (witness knob) stays BSP-gated in the ISR; HPET is not driven (ref=pm).
- A raw spinlock spun on by a preempted holder's core is not a masked section: not named by this seam.
