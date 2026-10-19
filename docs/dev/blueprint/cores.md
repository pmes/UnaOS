# BP-cores — SMP, scheduling, service roles, the compositor gate, watchdogs, locks (read at 1719fe54)

## 1. What it is
- x86: the BSP brings up APs by INIT-SIPI-SIPI (`unaos/crates/kernel/src/arch/x86_64/smp.rs:1-16`); each core runs its own preemptive per-CPU round-robin scheduler (`unaos/crates/kernel/src/arch/x86_64/sched.rs:1-45`).
- At the GUI handoff the BSP publishes ONE split word `(render, service)` = (first AP, last AP), spawns `usb-pump` + `input` on the service core and `render` on the render core, then JOINS the scheduler (`unaos/crates/kernel/src/main.rs:1513-1591`).
- The compositor pass on x86 is serialised by one gate, `COMP_GATE` (`unaos/crates/kernel/src/video/wm.rs:9469`). A holder past 1500 ms is declared dead and the gate is stolen (W5).
- Watches on sibling cores (pump watch, render watch, deadman) exist because the instruments on a wedged core die with it.
- aarch64: a parallel scheduler (`unaos/crates/kernel/src/arch/aarch64/sched.rs:1-20`); the Pi BSP runs boot duties then `run_bsp(0)` (`unaos/crates/kernel/src/main.rs:1494`). The aarch64 compositor uses its own `comp_gate_pass` (`unaos/crates/kernel/src/video/wm.rs:27530`).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-cores-1 | Placement is asked by name, never re-derived: `smp::worker_cpu(n)` (not the render core) and `smp::xhci_worker_cpu(n)` (neither render nor service; `None` = DECLINE, never co-locate). | `unaos/crates/kernel/src/arch/x86_64/smp.rs:208-243` `unaos/crates/kernel/src/arch/x86_64/smp.rs:341-358` | `:: SCHED-X86 PLACE: … tier= pool= ::` line |
| BP-cores-2 | The split is ONE atomic word, published once, before any service spawn; readers take it with one load. | `unaos/crates/kernel/src/arch/x86_64/smp.rs:245-275` `unaos/crates/kernel/src/arch/x86_64/smp.rs:407-409`; `unaos/crates/kernel/src/main.rs:1534` | unheld (statement order in `kernel_main`) |
| BP-cores-3 | Two preemptible takers of a raw spinlock (`XHCI_CONTROLLER`) never share a core: render and service cores must be DISTINCT or the handoff declines. | `unaos/crates/kernel/src/main.rs:1511-1520`; `unaos/crates/kernel/src/arch/x86_64/smp.rs:344-352` | unheld (code review) |
| BP-cores-4 | No core is reserved by policy: every core dispatches (`sched=all-cores`); placement exclusions place named work, they hold no capacity idle. | `docs/dev/LAWS.md:189`; `unaos/crates/kernel/src/arch/x86_64/smp.rs:388-405` | the `sched=all-cores` term on the PLACE line |
| BP-cores-5 | Core 0 is the sole advancer of the global ms-clock: never place a cooperative (IF=0) ring-3 task there. | `unaos/crates/kernel/src/arch/x86_64/sched.rs:2666-2672` | unheld |
| BP-cores-6 | Scheduler lock order is `cv.locked -> mutex.sem.locked -> run-queue -> heap`; the run-queue lock is held with IF=0 and never across a switch; IRQ handlers only read `current`. | `unaos/crates/kernel/src/arch/x86_64/sched.rs:20-45` | unheld (review) |
| BP-cores-7 | Only `wm::composite()` takes `COMP_GATE` on x86 (the other writer is the knob-off `wedgeinj` park). | `unaos/crates/kernel/src/video/wm.rs:4816` `unaos/crates/kernel/src/video/wm.rs:9469`; `docs/dev/evidence/gatesteal/GATESTEAL.md` §2.4 | unheld |
| BP-cores-8 | A gate holder in-pass for `COMP_GATE_STEAL_MS` = 1500 ms is dead; its gate is taken; a revenant never releases a gate it no longer owns. | `unaos/crates/kernel/src/video/wm.rs:4826-4850` `unaos/crates/kernel/src/video/wm.rs:9494-9502` | `:: [wcser] GATE STOLEN …` line; `tests gatesteal` |
| BP-cores-9 | The SERVICE core is a ONE-PASS compositor holder (no re-run rounds, no re-acquire); a MASKED composite on the service core DECLINES before the CAS. | `unaos/crates/kernel/src/video/wm.rs:4930` `unaos/crates/kernel/src/video/wm.rs:5026` (GATESTEAL B657) | `tests gatesteal`; `[wcser] … declined=` |
| BP-cores-10 | The PAYGO read-back runs on a `paygo-take` task on a sibling core, never on the pump. | `unaos/crates/kernel/src/sessionwedge.rs:614`; `unaos/crates/kernel/src/bootpace.rs:343` | `tests gatesteal` |
| BP-cores-11 | A dead core's roles are re-homed by the scheduler from a LIVE core: the pump watch calls `sched::rehome_dead_core(dead, live)`; the service role follows the pump; the dead core is parked by name and skipped by `pick_cpu`. | `unaos/crates/kernel/src/sessionwedge.rs:135`; `unaos/crates/kernel/src/arch/x86_64/sched.rs:3075` `unaos/crates/kernel/src/arch/x86_64/sched.rs:3518` `unaos/crates/kernel/src/arch/x86_64/sched.rs:7943` `unaos/crates/kernel/src/arch/x86_64/sched.rs:7974` | `[pump] REHOME:` line; PUMPWEDGE `rehome=` |
| BP-cores-12 | Every wait on the render path is bounded and named; its watch runs on a sibling core. | `unaos/crates/kernel/src/video/renderwedge.rs:1-25` (B520) | `tests renderwedge` |
| BP-cores-13 | The pump is watched from a sibling core armed at the ignition; a pump silent 2000 ms is a PUMPWEDGE that prints without the pump. | `unaos/crates/kernel/src/sessionwedge.rs:38-40` `unaos/crates/kernel/src/sessionwedge.rs:135`; `unaos/crates/kernel/src/boot.rs:154` | `:: PUMPWEDGE: … -> FAIL ::` |
| BP-cores-14 | A tracked lock (`sink`, `uart`, `unafs`) records its holder's task id; a stack-overflow kill releases only locks the dead task is KNOWN to hold. Off on aarch64. | `unaos/crates/kernel/src/lockowner.rs:4-31` | `[stack] overflow … released=[…]` |
| BP-cores-15 | A shared resource reached from the render task is held only through a bounded, masked accessor (the AHCI port: `dongle::with_port`, render task refused at once, 1 s bound); the render path never does per-row disk I/O under a UI lock (facts read on the service pass). | commits `715c69ff` (DONGLEBOOT B658), `331cd2a8` (QUARRYIO B660) | `tests dongleboot`, `tests quarryio` |

## 3. Seams
- Callers of placement: ring-3 launchers (`syscall::bg_place_cpu` is the one unconverted site, `unaos/crates/kernel/src/arch/x86_64/smp.rs:237`), `irqstorage`, fixtures.
- `composite()` is called by the render task, the pump's service pass (one-pass), ring-3 `SYS_WIN_PRESENT`, and drive tasks; a decline leaves damage on the table (`service_damage`).
- `sessionwedge::look` may call `sched::rehome_dead_core` and `smp::publish_sched_split(render, live)`; nothing else re-publishes the split.

## 4. Wire
`:: SCHED-X86 PLACE: aps= rsvc=c<n> svc=c<n> worker=[…] xhci=[…] tier= pool= sched=all-cores ::` ·
`:: SCHED-X86: RENDER on core <r> + INPUT/usb-pump on core <s> …` · `:: W5: nmi core=c<n> taken=<y|n> rip= if= …` (`unaos/crates/kernel/src/arch/x86_64/interrupts.rs:1095-1137`) ·
`:: [wcser] GATE STOLEN from c<a> by c<b> after <n>ms …` · `:: PUMPWEDGE: age_ms= … rehome=<ok(pump=<t>@cpu<n>)|none>` · `[pump] census why=wedge …` · `[deadman] gate=…` · `[lag] stall … holder= masked_ms=`.

## 5. Ruled, unbuilt / open
- `unaos/crates/kernel/src/arch/x86_64/sched.rs:9-10` and `unaos/crates/kernel/src/arch/x86_64/smp.rs:15-16` still say the BSP is not scheduled and drives xHCI; the code since SCHED-X86 joins the BSP (`unaos/crates/kernel/src/main.rs:1591`) and runs the pump on the last AP. Stale header comments (LAWS §5 "wire beats comment"): owed a repair.
- WCPAR (parallel per-core compose, serial present) is designed, not landed (`unaos/crates/kernel/src/video/wm.rs:9455-9467`).
- GATESTEAL §2.5: the instruction the wedged holder sat on is not named in-tree (no symbol table in the image).

## 6. Read further
- `docs/dev/OS/02_KERNEL_CORE/scheduler.md` — the scheduler design.
- `docs/dev/evidence/gatesteal/GATESTEAL.md` — the steal, the dead mailbox, the re-home.
- `docs/dev/OS/08_VIDEO/engine.md` §8 — WCPAR.
- `docs/dev/OS/rmbp-ledger.md` rows B520, B525, B553, B657, B658, B660.
