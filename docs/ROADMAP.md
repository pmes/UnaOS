# UnaOS Roadmap

> Working document — updated as arcs land. Session ground rules live in
> [`CLAUDE.md`](../CLAUDE.md); the security model and hardening ledger in
> [`SECURITY.md`](SECURITY.md); subsystem detail in [`dev/OS/`](dev/OS).
>
> Last major revision: **2026-07-02** — the multi-user-first redirect.

## North star

A **functioning desktop**: the GUI shell spawns **capability-scoped programs,
loaded from disk into isolated address spaces**, with storage on UnaFS. Every
arc below serves that goal. The physical-world payloads — the printer
("Design → Make"), the ENDURO rover, Jetson depth perception — ride on top of
exactly this foundation and are sequenced after it. UnaOS will hold direct
authority over physical hardware, which is why the privilege/capability work
comes first rather than being retrofitted (a lesson learned from BeOS, which
deferred multi-user and paid for it).

## Where we are (2026-07-02)

| Platform | Track | State |
| :--- | :--- | :--- |
| x86_64 (2012 rMBP) | `hw-rmbp` | Boots on metal: xHCI USB (kbd/mouse/mass-storage incl. SuperSpeed recovery), GOP video, SMP + full scheduler toolkit, e1000+TCP (QEMU), FAT16/32 read. **Ring 3: not started** (per-CPU GDT/TSS ready; no user segments, syscalls, U/S or NX bits). |
| aarch64 (Pi 4 B) | `hw-pi4` | Bare-metal from microSD at EL1: GICv2 + timer, mailbox framebuffer GUI, 4-core SMP + scheduler, interrupt-driven input, **EL0 userspace with SVC syscalls, per-page permissions, and fault→task-kill (M6a/M6b)** — the project's first privilege boundary. |
| aarch64 (Jetson Orin Nano) | `hw-jetson` | Tegra UART serial only. No interrupts yet (needs GICv3). |

Development runs on all three platforms simultaneously (three executor
sessions, one per worktree — see `CLAUDE.md`): **rmbp leads** the core arcs,
**pi follows** with the aarch64 port-backs it largely pioneered, **jetson runs
catchup** (GICv3 → scheduler) until it joins the shared code.

---

## 1. The multi-user chain (active)

Security model: **capabilities first, POSIX-layerable** — detailed in
[`SECURITY.md`](SECURITY.md) and consistent with the long-standing design in
[`dev/OS/04_SECURITY_IMMUNITY/permission_model.md`](dev/OS/04_SECURITY_IMMUNITY/permission_model.md)
("no root; apps start with zero permissions; handles are passed, not paths").

| Arc | Content | Lead | Port-back |
| :--- | :--- | :--- | :--- |
| **U1a** | x86 ring-3 round-trip: user GDT descriptors, TSS.RSP0, SYSCALL/SYSRET MSRs, `sys_write`/`sys_exit`, U/S page-walk demotion for the user window. Immediate hardening: EFER.NXE, NX on user data, CR4.SMEP | rmbp | design from Pi M6a (proven) |
| **U1b** | x86 fault→task-kill: PF/GP handlers kill the offending user task instead of halting; survivor demos + verdict | rmbp | design from Pi M6b |
| **U2** | Loadable programs: flat user blob read from FAT storage, loaded and run in ring 3 (ELF later) | rmbp | Pi as **M6c** via embedded blob (no Pi storage driver yet) |
| **U3** | Per-process address spaces: per-process PML4, CR3 switch in the scheduler | rmbp | Pi as **M6d** (per-task TTBR0 + ASID) |
| **U4** | Process model: PIDs, spawn/wait/exit status, **per-process handle table** — arch-neutral | shared | — |
| **U5** | **Capability model**: handles are capabilities; every syscall checked at the handle table; grant/attenuate/revoke; bandy handle-transfer semantics | shared | — |
| **U6** | UnaFS enforcement: `owner`/`grants:*` typed attributes checked on open (rides on K2/K3 below) | rmbp+host | — |
| **H1** | Hardening ledger — folded into each track's next brief; tracked in `SECURITY.md` | all | — |

**Pi lane:** M6c loadable blob → M6e preemptible EL0 (metal-gated) → M6d
(port of U3) → consume U4/U5 → M6f `copy_from_user`.

**Jetson lane:** **JC1** GICv3 (developed against QEMU `virt
-machine gic-version=3`, then Orin metal; parameterizes the shared GIC driver)
→ **JC2** generic timer + PSCI CPU_ON SMP (6 cores) + scheduler → **JC3** joins
the shared userspace code.

## 2. UnaFS: meeting and surpassing BeFS

`libs/unafs` already exceeds BeFS on one axis — typed attributes including
`Vector` embeddings with cosine-similarity queries. What remains to meet and
beat it (details and caveats in [`libs/unafs/README.md`](../libs/unafs/README.md)):

| Arc | Content |
| :--- | :--- |
| F1 | Journal **rollback/replay** (recovery is detect-only today) |
| F2 | `unlink`/`rename`/`remove_attribute` + catalog entry removal |
| F3 | Generic on-disk **B+tree** (one implementation for indexes and directories) |
| F4 | **Attribute indexes** — log-time equality + true range queries; retires the O(n) catalog scan/rewrite |
| F5 | **Live queries** — persistent queries emitting add/remove deltas over bandy; the query-driven UI (the BeOS crown jewel, plus similarity) |
| F6–F8 | B+tree directories; metadata checksums (beyond BeFS); extent trees for very large files |
| K1 | `no_std + alloc` port of the core |
| K2 | Block adapter (kernel 512 B sectors ↔ unafs 4096 B blocks) + GPT/MBR partition offsets |
| K3 | **Kernel read-only mount** of a real USB volume — `ls`/`cat`/`query` on metal |
| K4 | Journaled kernel writes + a minimal VFS (retires read-only FAT as the storage story) |

Order: F1 → F2 → K1/K2 → **K3 (metal milestone)** → F3 → F4 → **F5 (BeFS
surpassed)** → K4. The F-arcs are host-native — ideal parallel work needing no
hardware session.

## 3. Design → Make (printer)

Canon: [`CODEX.md`](CODEX.md) §5 — Vug computes toolpaths, Comscan pumps
G-code to hardware. Sequenced routes:

1. **PrusaLink HTTP** (~2 arcs): HTTP client grows PUT + API-key auth over the
   existing TCP stack → upload G-code to the Prusa CORE One+ and start a print.
   *"UnaOS prints a part"* early; also a real Go-Back-N stress test.
2. **USB CDC-ACM + Comscan** (2 arcs): modest USB class driver (two bulk
   endpoints + SET_LINE_CODING) on the existing xHCI stack; stream G-code with
   ok/ack flow control from the rMBP on metal. Comscan's first real capability.
3. **Vug native CAM**: full slicing is far-future; the Vug→Comscan G-code
   interface gets defined during (1)/(2) so they remain the permanent "Make"
   backend. Optional interim: 2.5D polygon-extrusion slicing for flat parts.

## 4. ENDURO — the rover (deferred until the desktop chain matures)

Architecture settled 2026-07-02. The OS is the **vehicle computer between the
RC receiver and the actuators**; the transmitter remains the human override
and estop at every stage of autonomy.

```
FlySky NB4 Plus ──AFHDS3──▶ FGr8B ──i-BUS (UART 115200)──▶ UnaOS drive stack ──PWM──▶ steering servo
   (transmitter)            (receiver)                      (Jetson Orin Nano)    └──▶ Hobbywing AXE R3 FOC ESC
```

- **i-BUS in**: 32-byte frames every ~7 ms (`0x20 0x40`, 14× LE u16
  1000–2000 µs, checksum `0xFFFF − sum`) on a second Tegra UART (never the
  TCU-owned debug UART). Verify the FGr8B's i-BUS output option on the bench.
- **PWM out**: Tegra's native PWM is 8-bit duty (insufficient at 50 Hz) →
  either a 333 Hz frame or (recommended) a **PCA9685 over I2C** (12-bit; the
  I2C driver is reusable for sensors).
- **Drive service** (arch-neutral, designed): bounded command channel, 20 ms
  control loop, ARM/DISARM state machine, 500 ms deadman, software throttle
  cap, cross-core watchdog, panic-handler-forces-neutral. A 3-position
  transmitter channel selects DISARM / MANUAL / AUTO.
- **Power**: Orin barrel input is 7–20 V → 3S LiPo direct.
- Milestones: GICv3/scheduler (shared with the Jetson catchup lane) → i-BUS
  decode demo ("UnaOS sees the transmitter") → actuation gate → drive service →
  first crawl, each behind a written safety-interlock checklist.
- A Pi-4 fast path (BCM2711 GPIO/PWM, fully specified) is preserved in the
  planning archive if a second vehicle or an earlier wheel-turn is ever wanted.

## 5. Jetson perception (after ENDURO drives)

Tegra XUSB as a platform (non-PCI) xHCI controller — *unknown: whether UEFI
leaves the XUSB firmware loaded* → SuperSpeed enumeration (the rMBP
SS-recovery machinery reuses directly) → UVC class driver (D400-series
RealSense streams over **bulk** endpoints on USB3, so isochronous support is
likely deferrable — verify for the D435i) → depth frames + IMU → obstacle-stop
and follow-me demos feeding the drive service's AUTO mode.

## 6. Other load-bearing gaps (honest inventory)

| Gap | Note | Size |
| :--- | :--- | :--- |
| aarch64 networking | Pi: PCIe RC + VL805 xHCI (unlocks all Pi USB) or GENET MAC driver | M–L |
| Pi runtime storage | SD/EMMC driver (needed for loadable programs from disk on Pi) | S–M |
| RTC + NTP | Cheap and load-bearing — land before UnaFS kernel writes (real mtimes) | S |
| Entropy | RDRAND/jitter; prerequisite for any TLS/WPA future | S |
| VFS layer | Forced by K3/K4 (FAT + UnaFS coexistence) — plan, don't stumble | M |
| Audio | x86 HDA; Pi HDMI/PWM audio | M each |
| Power management | Backlight/battery/idle — Lazarus-machine credibility | S–M each |
| WiFi/BT | SDIO + firmware + supplicant — distant; Ethernet first | L++ |
| GPU acceleration | Modesetting beyond GOP is L; real 3D is years — own the CPU/SIMD-render choice near-term | L++ |

## Sequencing principles

- One focused arc per session; every arc has a written brief with an exact
  DONE gate.
- QEMU-green ≠ correct: hardware verification at arc boundaries; adversarial
  review before metal and before merge.
- No worktree runs more than one arc ahead of `main`; the integrator merges
  reviewed arcs and rebases the other worktrees immediately.
- Core elements are built once on the lead platform; followers port the
  platform nuances back promptly so the shared code never forks.
