# SMPBALK8 — the ERET-SCRUB verdict is stolen onto core 0 and never prints

**Arc:** SMPBALK8 (`exec-rmbp-smpbalk8`, parent `3e4b7aa1`), 2026-09-16. Lane: `UNAOS_QEMU_FULL=1 ./arroyo kernel8-test 300`
(QEMU raspi4b, `pi4-regression.spec`). Serial logs are read with `awk 'index($0,"[tag]")'`, never bare `grep`.

## 1. The three failing runs measured today (the seat's runs; this executor read the two surviving serials)

| run | tree | 1-min load | lines | verdict | `[smpbal] steal 'eret-verdict'` | ERET-SCRUB lines | U4 `-> PASS` |
|---|---|---|---|---|---|---|---|
| foldgate k8rerun (loaded) | `fa4dcf0b` | 15–20 | 15266 | 124/126 | 1 (seat's count) | 0 | present |
| foldgate k8rerun (quiet) | `fa4dcf0b` | 3.85 | 54048 | 124/126 | 1, at serial line 472 | 0 | present (line 556) |
| k8bisect0 (quiet) | `11ca67f1` | 1.04 | 60527 | 123/126 | 1, at serial line 482 | 0 | ABSENT |
| foldgate gate 6 (loaded) | `11ca67f1` | 9.55 | 27352 | 126/126 | serial overwritten, not countable | 2 (spec table) | present |
| SMPBALK8 gate (fix in) | `3e4b7aa1`+fix | 3.04 | 57496 | 126/126, rc=0 | 0 (verdict `caller-pinned` core 2; 12 steals total) | 2 | present |
| SMPBALK8 go-red (fix reverted) | `3e4b7aa1` | 2.45 | 61048 | 126/126, rc=0 | 0 — the race was not hit; verdict popped by core 2 itself | 2 | present |

## 2. The placement / steal / exit table, base run (`11ca67f1`, quiet), serial line numbers

```
477 :: M6e: EL0 preemptible — spinner completed=1 IRQs-taken-at-EL0=0 (…) ::            <- m6e-verdict, caller-pinned core 2
478 :: SCHED: task 'el0-eretentry' -> core 3 (policy: load-balanced EL0 residents=1, no-migrate) ::
479 :: SCHED: task 'el0-eretsvc'   -> core 0 (policy: load-balanced EL0 residents=1, no-migrate) ::
480 :: SCHED: task 'eret-verdict'  -> core 2 (policy: load-balanced, no-migrate; prio 1) ::
481 [el0stkhw] task=73:el0-eretsvc len=16384 hw=2320 headroom=14064 loguard=0           <- exited ON core 0 (exit-path print)
482 :: [smpbal] steal 'eret-verdict' c2->c0 (m=1) ::                                     <- thief = core 0, victim = core 2
548 [el0stkhw] task=72:el0-eretentry len=16384 hw=2280 headroom=14104 loguard=0          <- exited on core 3
730 :: [smpbal] steal 'orphan-reaper' c2->c0 (m=2) ::                                    <- core 0 idle again: its queue was EMPTY
887 :: [smpbal] steal 'el0-thread-w' c2->c0 (m=1) ::
1047 :: [smpbal] steal 'el0-thread-w' c1->c0 (m=2) ::
```
Lines carrying `ERET-SCRUB`: 0. Lines carrying `task=74` or `eret-verdict` after 482: 0. `[spin6]`, `[redzone]`, `[wedge4]`: 0.
`[skill]` kills: 10, all `pid=119..139 asid=1..3` (the shell's `bg` rows), none a kernel task.
Whole-run `[smpbal] steal` lines: 12 (= `STEAL_LOG_MAX`): `smpbal` x4 (the SMP-BAL spread witness), `vugfloor`,
`orphan-reaper` x2, `eret-verdict`, `el0-thread-w` x4.

Second failing serial (`fa4dcf0b`, quiet): identical placements (c3 / c0 / c2), `[el0stkhw] task=73` at 481, the steal line at
**472 — five lines BEFORE the M6e verdict line (477) and eight before its own placement line (480)**: `spawn_inner` pushes the
task onto the run queue (`sched.rs` `rq(cpu).push(task)`) before it prints the `SCHED: task` line, so a `CPU_AUTO` task is
stealable before its placement is on the wire. The steal of a `load-balanced` task is the balancer working as designed.

## 3. Core 0 on this lane

Core 0 is an ONLINE, SCHEDULING core: `kernel_main` ends in `run_bsp(0)` (`main.rs`, the Pi GUI+AP path), which calls
`mark_online(0)` and enters `run()`. Evidence on the wire: `el0-eretsvc` was PLACED on core 0 by `CPU_AUTO` (line 479) and
EXITED there (line 481, the exit-path `[el0stkhw]` print), four `bg-user` EL0 launches were load-balanced onto core 0 later in
the run, and core 0 stole three more times (730, 887, 1047). `:: SCHED: task … -> core 0` lines: 19 (base) / 21 (rerun),
14 of them the early caller-pinned boot fixtures (`demo`, `pm-*`, `skill-*`). The comment in `m6e_verdict` that said
`CPU_AUTO` "never" reaches "the unscheduled BSP" was stale since SMP-BAL wired `run_bsp(0)`.

## 4. The two runs on this worktree

The gate run with the fix and the go-red run without it are the last two rows of §1: 4 runs in which no steal of the verdict happened pass, 3 runs with the steal fail. The go-red is "failed to reproduce under load 2.45" (R19), so the go-red for the fix is the three failing runs.

## 5. What is and is not established

Established: 3/3 failing runs carry `steal 'eret-verdict' c2->c0` and 0 ERET-SCRUB lines; every sibling verdict spawned
caller-pinned onto `vcpu` (`m6b-verdict`, `m6d-verdict`, `m6f-verdict`, the `u5..u7` launchers) printed in 3/3 runs; core 0
dispatches (an EL0 task ran to exit there); the stolen task left core 0's queue without any drop/refuse/kill witness firing.
Not established: WHY a kernel task dispatched on core 0 after a steal produced no line. Also open: in the base run alone,
`u4-launch` (caller-pinned core 2, never stolen) printed neither its PASS nor its FAIL line although `el0-u4parent` (76) and
`el0-u4orphan` (77) both exited — a second silent poller, on a core that was not a steal target.
