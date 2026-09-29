# WCPAR — spread the compositor's per-pass work across the APs

## Finding (boot 16, f16.log)
`[wcpar] cores=1 total=297 max=297 c0=297 c1..c7=0 span=5049ms` on every rollup; `:: SMPLOAD: busy=[12,0,0,0,0,0,0,3] stealable=[0,...] -> PASS ::`.
`[wcpar]` (`video/wm.rs` `wcpar_emit`) counts the core each window compose STAGED on. The compose is one serial pass under `COMP_GATE`;
`[comp2] pass_us=8477 blit_us=8469` says the pass is the row-by-row BAR1 blit. No x86 band-worker pool existed (grep: none), so nothing was stealable.

## Mechanism
- `video/wcpar.rs` (new): `start()` at ignition (`desktop_uefi.rs` before `[wc-x] desktop-clear`) spawns one PINNED `wcpar-band` task per online AP (`arch::sched::spawn`, named core = pinned, `sched.rs` pin contract).
- `wm.rs stage_window` no-clip blit loop: `par_blit` fans rows into disjoint bands; the presenter claims bands too (packed `gen<<32|next` CAS) and joins on `J_DONE`; workers read the staging buffer and write only their rows, `sfence` before done. No window table / lock is touched by workers. Clipped (overlap) rows keep the serial loop.
- Workers spin hot 20 ms after the last band, then `sleep_ms(1)`; the presenter always contributes, so a sleeping pool costs latency, never correctness.

## Milestones
- M1: `[wcpar] pool=N workers=W cpus_online=C reason=...` at ignition.
- M2: fan-out + `:: WCPAR: cores= workers= bands= pass_us= serial_us= speedup_pct= -> PASS ::` per rollup (`serial_us` = sum of band times, `pass_us` = wall inside `par_blit`).

## Pins
`x86-wc.spec` REQUIRE `:: WCPAR: cores=` and `[wcpar] pool=`. QEMU lane is `-smp 6` (`builder/src/main.rs:1633`), so the pin is honest if a >=32-row unclipped window composes in that lane; if not, drop the first REQUIRE.

## Written
Boot 17 should show `[wcpar] pool=8 workers=7 ...` at ignition, then `:: WCPAR: cores=>1 ... -> PASS ::` and `[wcpar] cores=>1` under a vug storm.
Compiler notes: `crate::arch::smp::online_aps`, `crate::arch::sched::{spawn,sleep_ms,PRIO_LOW,online_cpu_count}`, `wcg::cycles_to_us` (pub(super), witness-gated module: `emit` is witness-only, and `wcpar.rs` uses no wcg outside `emit`).
