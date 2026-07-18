# DEFAULT-QUIET landing — gate the re-proof fixture battery behind a `witness` feature

Track: quietboot · Branch: `us-quietboot` · Base: `main` `adb8de3`.
Lane (integrator-granted, cross-cutting): `main.rs` launcher call sites, `arroyo`, `builder/src/main.rs`,
one Cargo feature, minimal per-module `cfg` additions. No fixture/test bodies touched; no sched logic,
driver internals, or protection touched. One arc, 4 milestones (M1 inventory / M2 feature+gating /
M3 verify / M4 doc).

## The directive this closes

Peter, verbatim intent: **"NO MORE TESTING THINGS WE HAVE TESTED 10s OF TIMES OR MORE"** on default
boots. A default boot emitted a large battery of EL0/kernel fixture `-> PASS` lines re-proving
long-metal-confirmed facts on *every* boot. This gates that battery behind a new **`witness`** cargo
feature (default OFF); the QEMU regression commands auto-arm it so their coverage is unchanged, while
boot/media builds reach the shell with the **boot-honesty lines only**.

## Classification model (M1)

- **(a) battery fixtures re-proving long-metal-confirmed facts → GATE** behind `witness`.
- **(b) boot-honesty — live hardware state discovered THIS boot → KEEP** (device found, link/calibration,
  SD-card census, `MISSION SUCCESS`, the `CAPSTONE` scheduler terminus, net-stack liveness witnesses).
- **(c) recent-arc witnesses still earning first metal verdicts → LEAVE** (SLEEPMS/JOINTMO/AGEREF/CVCAP
  on `sched_demo`, already knob-gated; prio-mix + busy/idle heartbeat, part of the `start_aps` CAPSTONE
  workload / boot terminus).
- **(d) already knob-gated → LEAVE** (NET-1..4, VNET, RAST, SMC, irqstorage, videobench, EHCI probes …).

## Inventory table (family → path → class → disposition)

| Family / call site | Boot path | Class | Disposition |
|---|---|---|---|
| `nmi_self_fire` + `canonical_guard_selftest` (U2-0c) | x86 pre-sched | (a) | gated `witness` |
| `U1a` ring-3 round-trip + `sched::enable()` | x86 pre-sched | (a) | gated `witness` |
| `U1b` fault-isolation (3 fixtures) | x86 pre-sched | (a) | gated `witness` |
| `U2-0a` TF+SYSCALL DoS | x86 pre-sched | (a) | gated `witness` |
| `U3` per-process CR3 + `U3.5` preemptible ring-3 | x86 pre-sched | (a) | gated `witness` |
| `u2_probe_once` (FAT loader → ring 3) | x86 loops (both) | (a) | gated `witness` |
| `u4x`/`u5x`/`u6x`/`u6bx`_probe_once | x86 loops (both) | (a) | gated `witness` |
| `u7x`→`u8x`→`u9x`→`u10x`→`u6gx` + ring-3 `SOCK-2/3/4` | x86 (cascade from `u6bx`) | (a) | gated (via cascade root) |
| `clock_x1_witness` (CLOCK-X1 calibration) | x86 pre-sched | (b) | KEPT |
| `fat::probe_once` / `flight_recorder` / `log_summary_once` | x86 loops | (b) | KEPT |
| `MISSION SUCCESS` (xHCI BOT/CSW), APIC calibrate/tick-rate | x86 | (b) | KEPT |
| `SOCK-1/2/3/6/7` smoltcp net-stack liveness (`— witness OK/PENDING`) | x86 (smolnet) | (b/d) | KEPT |
| `M6b`/`M6e`/`M6d`/`M6f`/`M6g`-loader EL0 fixtures | aarch64 baremetal (Pi) | (a) | gated `witness` |
| `U4`/`U5`/`U6`/`U6b`/`U7` EL0 launchers | aarch64 baremetal | (a) | gated `witness` |
| `U9`/`U10`/`U10c`/`U10d`/`U11`/`U11-defer`/`U11-reap` | aarch64 (cascade from `U7`) | (a) | gated (via cascade root) |
| `K1`..`K9`/`F2`/`F3`/`BANDY-*`/`unafs K3..K8c`/`FATDIRS`/`FATMOVE`/img-sig | aarch64 (cascade from `U7`) | (a) | gated (via cascade root) |
| `emmc2::probe` (SD-card census, "M6g Part B") | aarch64 baremetal | (b) | KEPT |
| `demo_cooperative` / `start_aps` (CAPSTONE workload) | aarch64 baremetal | (b/c) | KEPT (terminus + prio-mix/heartbeat) |
| `run_capstone_boot_core` → `CAPSTONE COMPLETE` | aarch64 virt/tegra | (b) | KEPT (boot terminus) |
| aarch64 `virt`/`tegra` non-baremetal path | — | — | no class-(a) fixtures present (nothing to gate) |

**Gating discipline:** for the two long chains (x86 `u6bx`→`u7x`..`u6gx`; aarch64 `U7`→`U9`..`unafs`)
the cascade root call site is gated, so the whole tail vanishes with it — no fixture body edited. On x86
both the usbdebug service loop and the GUI main loop carry the five `*_probe_once` sites; both were gated.

## Design

- **New feature** `witness = []` (kernel `Cargo.toml`), default OFF. Gates CALL SITES only.
- **`main.rs`:** x86 pre-scheduler fixtures wrapped in `#[cfg(feature = "witness")]` (keeping
  `clock_x1_witness` outside); the ten x86 `*_probe_once` sites changed
  `#[cfg(target_arch = "x86_64")]` → `#[cfg(all(target_arch = "x86_64", feature = "witness"))]`;
  the aarch64-baremetal `if let Some(&cpu) = online.first() { … }` fixture block (M6b..U7) gated
  `#[cfg(feature = "witness")]`. `sched::enable()` lives inside the gated x86 block — the opt-in
  `sched_demo` path self-enables via `start_demo`, so a quiet default boot simply leaves the APs idle.
- **`arroyo`:** a top-of-script `case` auto-**exports** `UNAOS_WITNESS=1` for
  `test`/`test-fat`/`test-arm`/`kernel8-test` (so the x86 `builder` subprocess, which re-derives features
  from env, also pushes it); `_feats` gains `witness`; `kernel8()` adds `witness` to `K8_FEATS` under the
  same knob. `battery` is **not** armed at the top — each sub-invocation (`"$0" test` …) re-enters the
  case and self-arms, while its `"$0" esp-jetson` sub-invocation does not (jetson media stays
  witness-free — byte-identity preserved).
- **`builder/src/main.rs`:** `if UNAOS_WITNESS.is_ok() { feats.push("witness") }` (kept in sync with arroyo).
- **aarch64 byte-identity:** all `witness` call sites are x86- or `baremetal`-scoped, so the `virt`/`tegra`
  compile never sees one. `esp-jetson` does not arm the knob → the jetson image is unchanged.

## Verification (M3) — all gates green

Commands run in the worktree; verdicts read from `target/serial*.log` with `awk`.

```
./arroyo check                    → ✅ x86_64 OK  ✅ aarch64 OK   (features: ehcihid,smolnet — NO witness)
UNAOS_WITNESS=1 ./arroyo check    → ✅ x86_64 OK  ✅ aarch64 OK   (features: witness,ehcihid,smolnet)
./arroyo test 22                  → features witness,ehcihid,smolnet · MISSION SUCCESS · 20 -> PASS
UNAOS_CPU=qemu64 ./arroyo test 22 → MISSION SUCCESS · 21 -> PASS   (xAPIC path)
./arroyo test-arm 22              → MISSION SUCCESS   (virt v2; witness is a no-op on aarch64)
UNAOS_GICV3=1 ./arroyo test-arm 40→ CAPSTONE COMPLETE · 6/6 primitives PASS · prio-mix witness PASS
./arroyo kernel8-test 35          → "UNAOS_WITNESS: kernel8 carries the M6b..U7 fixture battery" ·
                                     CAPSTONE COMPLETE · 29 -> PASS (pi log) · 0 FAIL ·
                                     M6b/U7/U10/K2-liveenf all present
```

Witness families INTACT in the batteries — witness-ON reproduces the pre-change call sites exactly, so
the `-> PASS` tallies and `CAPSTONE COMPLETE` are unchanged (zero witness families lost).

## Quiet-boot evidence + line-count delta (M3)

Built witness-OFF and booted headless to compare against the witness-ON battery run:

| Default boot | witness-ON (battery) | witness-OFF (quiet) | Δ |
|---|---|---|---|
| **x86** total serial lines | 376 | 323 | **−53** |
| x86 `-> PASS` fixture verdicts | 20 | **0** | −20 |
| **Pi (raspi4b)** total serial lines | 191 | 69 | **−122** |
| Pi `-> PASS` / `: PASS` lines | 29 | 6 (CAPSTONE terminus only) | −23 |

Quiet x86 boot: **zero** `U1a`/`U1b`/`U2`/`U3`/`U7x`..`U6gx`/ring-3-`SOCK` fixture lines; keeps APIC
calibration, `MISSION SUCCESS`, FAT/USB honesty, smoltcp net-liveness — reaches the shell. Quiet Pi boot:
**zero** `M6b`/`M6d`/`M6f`/`U4`..`U7`/`U9`/`U10`/`K*`/`BANDY` fixture lines; keeps the SD-card census +
`prio-mix` + `CAPSTONE COMPLETE` (all 6 primitives) — reaches the terminus.

## Flagged

- Pre-existing (not introduced here): `const U10D_PATTERN` in `arch/{aarch64,x86_64}/syscall.rs` is
  unreferenced in-tree — a dead-code warning that is independent of this arc (those files were not touched).
- On aarch64 `virt`/`tegra`, arming `witness` (for `test-arm`) changes only the cargo `-Cmetadata`
  feature-hash, not emitted code — a functional no-op that reflects intent, exactly as documented.
