# PRIO-MIX landing — the dedicated priority-mix stress witness

Track: priomix · Branch: `us-priomix` · Base: `main` `51d7ec1`.
Lane: `arch/aarch64/sched.rs` (witness + its two call sites) + `docs/dev/OS/01_BOOT_HAL/arch_arm64.md`
+ this report. x86 files, `smp.rs`/`smp_virt.rs`, and `main.rs` untouched.

## The gap this closes

The AARCH64-PRIO landing proved fixed priority + anti-starvation aging in ONE combined scenario
(`priority_aging_witness`, on the `virt` GICv3 boot core only) and explicitly **deferred** a dedicated
*priority-mix* witness — the Pi metal ledger recorded it as "mix witness deferred", the future metal
verification of a genuine priority mix on silicon. This arc delivers that witness and, crucially, wires
it into the **Pi kernel8 battery** (the deferred accrual) as well as the existing virt path.

## Scope delivered (M1–M3)

- **M1 — the witness (`prio_mix_witness(cpu)`, sched.rs).** Two bounded, self-contained sub-scenarios
  drained cooperatively via `run_until_empty`, reported independently on one line:
  - **strict** — from a *drained* queue: `PM_STRICT_HIGH`=3 `PRIO_HIGH` short tasks (run-to-completion,
    no yield) + 1 `PRIO_LOW` short task. A monotonic completion-ORDER counter (`PM_SEQ`) proves every
    high task finished before the low one (low's finish index == the high count). An **ordering** claim
    — valid only on a cooperative drained start, which is how the witness always runs; deliberately not
    asserted under preemption.
  - **aged-rescue** — from a drained queue: `PM_AGE_HIGH`=2 `PRIO_HIGH` loaders each yielding
    `PM_AGE_ITERS`=40 times (top level continuously ready) + 1 `PRIO_LOW` no-yield canary. Asserts the
    canary ran **while high load was still active** (`under_load`) — the anti-starvation proof. A
    **bounded-rescue** claim (completion before the finite load drains), not an ordering claim, so it
    stays honest under real preemption on Pi metal: the aging clock is dispatch passes
    (`SchedCpu::age_passes`), which advance on cooperative and preemptive dispatch alike. Same proven
    2-loader × 40-iter shape as `priority_aging_witness`.
  - Emits `:: AARCH64 SCHED: prio-mix witness (strict=..., aged-rescue=...) => PASS/FAIL ::`. Bounded +
    never hangs a battery: every task does finite work and neither low task yields, so `run_until_empty`
    always drains — a broken scheduler FAILs loudly (strict: low not last; aged-rescue: low ran only
    after the load drained), never wedging the core. That finite-work guarantee is the watchdog bound;
    no timer needed. Telemetry statics are lock-free relaxed (owning-core-only within a cooperative drain).
- **M2 — wired into BOTH witness paths (additive):**
  - `run_capstone_boot_core` (virt GICv3 boot core + Orin metal) — one line after the untouched
    `priority_aging_witness(cpu)` call; the existing `priority+aging PASS` line is unchanged.
  - `demo_cooperative` (Pi kernel8 battery) — appended after the cooperative-demo "complete" line, on
    boot core 0, still cooperatively (runs BEFORE `start_aps` flips `SCHED_ACTIVE`, so preemption is
    provably off and the strict sub-scenario is validly asserted). This is the deferred Pi accrual.
  - No double run: the `virt`/Orin boot core diverges into `run_capstone_boot_core` before reaching
    `demo_cooperative`, so each platform runs the witness exactly once. No `main.rs`/`smp*.rs` edits
    were needed — both call sites are sched.rs functions `main.rs` already invokes.
- **M3 — docs.** `arch_arm64.md` AARCH64-PRIO section grew a PRIO-MIX witness subsection; this report.

## Gates (verbatim)

- `./arroyo check` — `✅ x86_64 OK` / `✅ aarch64 OK` (no new warnings from this diff; the pre-existing
  `own_load`/`RING_SIZE`/etc. warnings are unrelated to sched.rs).
- `UNAOS_GICV3=1 ./arroyo test-arm 40`:
  - `:: AARCH64 SCHED: priority+aging witness — 2 PRIO_HIGH loaders vs 1 PRIO_LOW candidate on cpu 0 ::`
  - `:: AARCH64 SCHED: priority+aging PASS ::` (existing witness, untouched)
  - `:: AARCH64 SCHED: prio-mix witness — 3 PRIO_HIGH short + 1 PRIO_LOW (strict), then 2 PRIO_HIGH loaders + 1 PRIO_LOW (aged-rescue) on cpu 0 ::`
  - `:: AARCH64 SCHED: prio-mix witness (strict=PASS, aged-rescue=PASS) => PASS ::`  ← the new witness
  - `:: AARCH64 SMP: 3/3 secondaries online via PSCI CPU_ON on the GICv3 path ::`
  - `:: AARCH64 SMP: per-core idle heartbeat PASS ...` + `... per-core busy heartbeat PASS ...`
  - `:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::` (6/6 unchanged)
- `./arroyo test-arm 22`: `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`
- `./arroyo kernel8-test 35`: **56 PASS / 0 FAIL**, `CAPSTONE COMPLETE`, and the new
  `:: AARCH64 SCHED: prio-mix witness (strict=PASS, aged-rescue=PASS) => PASS ::` in the battery (the
  deferred Pi accrual). 0 FAIL is the invariant; the PASS count is boot-to-boot demo variance.
- `./arroyo test 22` (x86): `MISSION SUCCESS`, 4/4 SMP online, all U*/SOCK tests PASS, 0 FAIL — x86
  sched untouched, unregressed.

## Lens (ONE, at landing) — verdict PASS

- **Existing contracts preserved.** The `priority+aging PASS` line and every SMP heartbeat / CAPSTONE
  line are unchanged (busy-heartbeat `busy=8`, idle+busy heartbeats PASS, CAPSTONE 6/6). The new witness
  stages + drains its own tasks and leaves the queue empty, so it perturbs neither the CAPSTONE that
  follows on virt nor the AP workload that follows on Pi.
- **Cannot hang / cannot cheat.** Every task does finite work; neither low task yields, so
  `run_until_empty` always drains even if aging/priority is broken. strict FAILs if the low task is not
  last; aged-rescue FAILs if the canary ran only after the load drained (`under_load == false`). Both
  are loud, never a wedge.
- **Preemption honesty.** The strict (ordering) claim is asserted only on a cooperative drained start —
  both call sites run before preemption is enabled. The aged-rescue claim is a bounded-rescue assertion
  keyed off the dispatch-pass aging clock, which is valid under cooperative AND preemptive dispatch, so
  it remains honest when read on Pi metal (where the AP timer preempts).
- **Determinism.** With 3 short high + 1 short low from a drained queue, the first aging sweep at pass 4
  ages the low task by 4 < `AGE_TICKS`=16 (no promotion), so all 3 high complete first and the low runs
  last — strict is deterministic in QEMU. The aged-rescue params are byte-identical to the proven
  `priority_aging_witness`, so under_load holds deterministically.
- **Statics.** `PM_*` are lock-free relaxed and touched only on the owning core inside a cooperative
  drain; reset at the top of each sub-scenario. No cross-CPU sharing.

No MUST/SHOULD-FIX outstanding.

## Flagged / ledger

- The witness runs COOPERATIVELY on both paths (before preemption). On QEMU raspi4b the Pi has no
  Group-1 IRQ delivery, so there is no preemption to exercise there regardless; a genuine *preemptive*
  mix on silicon is still read at the next Pi sitting off the battery line — the aged-rescue assertion is
  phrased (bounded rescue, dispatch-pass clock) to be valid there. This closes the ledger's "mix witness
  deferred" by putting the self-checking witness into the Pi battery; the metal read is the accrual.
- `PRIO_RT` (level 3) remains defined-but-unused; the top level is intentionally never a promotion
  target, so the mix witness does not seed it (consistent with the AARCH64-PRIO landing).
