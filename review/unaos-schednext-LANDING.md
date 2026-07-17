# SCHED-NEXT landing — ORIN-SMP busy-heartbeat (cooperative scheduled work on the virt secondaries)

Track: hw-jetson · Branch: hw-jetson · Base: `534c55b` (ff'd to local main at session start).
Lane: aarch64 sched/task + smp_virt + arch_arm64.md. One arc, 2 code milestones + doc.

## Audit (MILESTONE 0)

The future doc `~/.claude/plans/unaos/future/unaos-scheduler-next.md` is the **x86** SMP-scheduler
brief. Audited against the aarch64 module (`arch/aarch64/sched.rs`, 1833 lines): the full sync toolkit
(sem/mutex/condvar/channel/rwlock/join) = CAPSTONE 6/6 is DONE (JC3 virt / JM6b Orin metal-confirmed);
SMP bring-up is DONE (JC2/JM5, 3/3 via PSCI CPU_ON on GICv3, idle-heartbeat just merged). The x86
priority/aging/PI candidates are ABSENT on aarch64 (flat round-robin) but deferred as large + out of
track momentum. The honest, momentum-aligned next arc — the doc's own named "SMP scheduling on virt,
a later step" — is running **scheduled work on the virt secondaries**, which today only park idle.
Scoped to `~/.claude/plans/unaos/queue/unaos-schednext.md`.

## Scope delivered

The idle-heartbeat proved a parked online secondary reads honest **idle**. This lands its other half:
an online secondary can **run scheduled work and read busy** — the QEMU-testable (cooperative) slice
of SMP scheduling on `virt`. Preemptive multi-core stays metal-only (no per-core timer at EL2).

- **sched.rs** — `secondary_probe_body` (cooperative yield/exit task), `stage_secondary_work`,
  `secondary_work_go`, `secondary_work_done`, `run_secondary_work(cpu)` (BOUNDED release wait →
  `run_until_empty` → `SECWORK_DONE`). Each dispatch bumps `CPU_BUSY[cpu]`. Additive; no scheduling
  path or existing counter changed.
- **smp_virt.rs** — `__secondary_rust_virt` runs one `run_secondary_work(core)` pass before its idle
  park; the BSP (`start_secondaries`) stages 2 probe tasks per online AP + releases after the ping
  proofs, waits (bounded) on completion, then the witness upgrades to assert **`busy>0` AND `idle>0`**
  and emits a new `per-core busy heartbeat PASS` line (idle-heartbeat line kept — both must PASS).

## Gates (verbatim)

- `./arroyo check` — `✅ x86_64 OK` / `✅ aarch64 OK` (no new warnings).
- `UNAOS_GICV3=1 ./arroyo test-arm 40`:
  - `:: AARCH64 SMP: 3/3 secondaries online via PSCI CPU_ON on the GICv3 path ::`
  - `:: AARCH64 SMP: AP {1,2,3} pulse (busy=8, idle=2) ran+idle ::` (busy=8 = 2 tasks × 4 dispatches)
  - `:: AARCH64 SMP: per-core idle heartbeat PASS — 3 online APs report idle (not pinned) ::`
  - `:: AARCH64 SMP: per-core busy heartbeat PASS — 3 online APs ran cooperative scheduled work ::`
  - `:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::` (6/6 unchanged)
- `./arroyo test-arm 22`: `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`
- `./arroyo kernel8-test`: **43 PASS / 0 FAIL**, `CAPSTONE COMPLETE` (shared sched.rs unregressed on
  the Pi — its APs run the full `run()` loop via `start_aps`, an untouched path; PASS count is
  boot-to-boot demo variance, 0 FAIL is the invariant).

## Lens (ONE, at landing) — verdict PASS, 1 MUST-FIX caught + folded pre-commit

Adversarial self-review of the diff:
- **MUST-FIX (caught + fixed before commit): shared-tail hang.** `__secondary_rust_virt` is the shared
  real-entry for `start_secondaries` (virt, releases the gate), `start_secondaries_tegra` (real Orin),
  AND the smpprobe legs — the latter two never call `secondary_work_go`. The first-draft **unbounded**
  release spin would have hung every metal + probe secondary. Fixed: `run_secondary_work`'s wait is
  **bounded** (~20 ms one-shot ceiling) — virt releases in microseconds (no added latency); tegra/probe
  elapse the ceiling once at bring-up and park as before (empty-queue drain = no-op); can never hang.
  Doc corrected to not overclaim metal busy-bars.
- Witness honesty: `busy>0` requires real `dispatch_next` dispatch (only bump site) — uncheatable.
  `SECWORK_DONE` Release/Acquire covers the relaxed `CPU_BUSY` writes (BSP reads the settled count).
- Isolation: per-CPU run queues; secondary work runs entirely within BSP bring-up, before the
  EL2→EL1 drop + boot-core CAPSTONE. Ping proofs read before staging, so `poke_cpu` SGIs don't skew.
- EL2 cooperative-task safety: `switch_context` EL-neutral; tasks yield/exit only, no timer/blocking.

No remaining MUST/SHOULD-FIX.

## Ledger / flagged (metal, accrues — not this gate)

- Real Orin secondaries still park **idle** (tegra path stages no work), so metal shows the idle bar.
  A **live vug busy bar on a metal secondary** needs the tegra bring-up to also stage + release
  cooperative work — a small **attended** follow-up (staging cooperative EL2 work on real Orin
  secondaries wants a metal sitting to confirm). Optional refinement: an early `secondary_work_go()`
  in `start_secondaries_tegra` would zero the ~20 ms bounded-wait on the metal path.
