# UnaOS Roadmap

> Working document — updated as arcs land. Session ground rules live in
> [`CLAUDE.md`](../CLAUDE.md); the security model and hardening ledger in
> [`SECURITY.md`](SECURITY.md); subsystem detail in [`dev/OS/`](dev/OS).
>
> Last major revision: **2026-07-10** — the multi-user capability chain is
> complete and metal-confirmed on both arches; focus moves to persistence,
> interrupt-driven storage, and the desktop surface.

## North star

A **functioning desktop**: the GUI shell spawns **capability-scoped programs,
loaded from disk into isolated address spaces**, with storage on UnaFS. Every
arc below serves that goal. The physical-world payloads — the printer
("Design → Make"), the ENDURO rover, Jetson depth perception — ride on top of
exactly this foundation and are sequenced after it. UnaOS will hold direct
authority over physical hardware, which is why the privilege/capability work
comes first rather than being retrofitted (a lesson learned from BeOS, which
deferred multi-user and paid for it).

## Where we are (2026-07-10)

| Platform | Track | State |
| :--- | :--- | :--- |
| x86_64 (2012 rMBP) | `hw-rmbp` | Boots on metal: xHCI USB (kbd/mouse/mass-storage incl. SuperSpeed recovery), GOP video, SMP + full scheduler toolkit, e1000+TCP (QEMU), FAT16/32 read+write. **The full capability chain U1a→U11→U6gx** (ring 3, per-page NX, per-process CR3, handle-table capabilities, cross-process transfer + revocation, RW files with grow/create/delete, per-name UnaFS ACL) — metal-confirmed. **STOR-1**: interrupt-driven storage runs read/write/grow/create/delete synchronously in-syscall behind the `irqstorage` knob (core transfer-IRQ mechanism metal-confirmed). |
| aarch64 (Pi 4 B) | `hw-pi4` | Bare-metal from microSD at EL1: GICv2 + timer, mailbox framebuffer GUI, 4-core SMP + scheduler, interrupt-driven input. **The capability chain (M6a–M6f, U4–U11, U6 owner/grants), SMP-hardened concurrent FS (F2 FAT-mutation + F3 namespace locks), and reboot-surviving ACL persistence foundation (K1)** — metal-confirmed. |
| aarch64 (Jetson Orin Nano) | `hw-jetson` | GICv3 + PSCI SMP; an interactive panel shell over a hub-attached USB disk with a read/write FAT path (`ls`/`cd`/`cat`/`touch`/`write`/`rm`) — metal-confirmed on real Orin silicon (file survives power-cycle). |

Development runs on all three platforms simultaneously (worktree sessions +
seat-spawned agents in hybrid mode — see `CLAUDE.md`): **rmbp leads** the shared
kernel-core arcs, **pi** pioneers and hardens the privilege boundary it largely
invented, **jetson** drives the Tegra bring-up and the workstation-shell surface.

---

## 1. The multi-user chain (active)

Security model: **capabilities first, POSIX-layerable** — detailed in
[`SECURITY.md`](SECURITY.md) and consistent with the long-standing design in
[`dev/OS/04_SECURITY_IMMUNITY/permission_model.md`](dev/OS/04_SECURITY_IMMUNITY/permission_model.md)
("no root; apps start with zero permissions; handles are passed, not paths").

> **STATUS (2026-07-10): the whole chain U1→U6 has LANDED and is metal-confirmed
> on both arches** — the rows below record the design intent as originally
> planned; per-milestone landing evidence is in [`MILESTONES.md`](MILESTONES.md).
> Active work has moved past the chain to *persistence* (pi4 K1 — reboot-surviving
> ACL) and *interrupt-driven storage* (x86 STOR-1). The next section (§1a) tracks
> those frontiers.

| Arc | Content | Lead | Port-back |
| :--- | :--- | :--- | :--- |
| **U1a** | x86 ring-3 round-trip: user GDT descriptors, TSS.RSP0, SYSCALL/SYSRET MSRs, `sys_write`/`sys_exit`, U/S page-walk demotion for the user window. Immediate hardening: EFER.NXE, NX on user data, CR4.SMEP | rmbp | design from Pi M6a (proven) |
| **U1b** | x86 fault→task-kill: PF/GP handlers kill the offending user task instead of halting; survivor demos + verdict | rmbp | design from Pi M6b |
| **U2** | Loadable programs: flat user blob read from FAT storage, loaded and run in ring 3 (ELF later) | rmbp | Pi as **M6c** via embedded blob (no Pi storage driver yet) |
| **U3** | Per-process address spaces: per-process PML4, CR3 switch in the scheduler | rmbp | Pi as **M6d** (per-task TTBR0 + ASID) |
| **U4** | Process model: PIDs, spawn/wait/exit status, **per-process handle table** — arch-neutral | shared | — |
| **U5** | **Capability model**: handles are capabilities; every syscall checked at the handle table; grant/attenuate/revoke; bandy handle-transfer semantics | shared | — |
| **U6** | UnaFS `owner`/`grants:*` enforcement checked at open — owned-by-default at `SYS_OPEN` + `SYS_FGRANT` delegation, in-kernel. **Landed on BOTH arches: aarch64 seam 2026-07-08 (U6), x86 twin U6gx** (owner-only unlink, generation-fenced grants); both metal-confirmed. On-disk attribute backing is the pi4 K1 persistence work (§1a) | pi led seam / rmbp twin | — |
| **H1** | Hardening ledger — folded into each track's next brief; tracked in `SECURITY.md` | all | — |

**Pi lane:** ✅ complete — M6c → M6e → M6d → U4/U5 → M6f all landed, plus the
F2/F3 SMP-concurrency hardening and the K1 persistence foundation (§1a).

**Jetson lane:** ✅ **JC1** GICv3 + **JC2** timer/PSCI SMP + scheduler landed and
Orin-metal-confirmed; the track now drives the workstation-shell surface (the
`ls`/`cat`/`touch`/`write`/`rm` panel over a hub-attached USB disk) rather than
the shared userspace port.

## 1a. Current frontiers (active — the chain above is done)

| Arc | Content | Track |
| :--- | :--- | :--- |
| **STOR-1** | Interrupt-driven x86 storage: a scheduled IF=1 service task owns the xHCI BOT pump; syscalls submit a block request + block on a per-request semaphore. Read/write/grow/create/delete synchronous in-syscall, behind the `irqstorage` knob. S1–S4 landed; core transfer-IRQ mechanism metal-confirmed. Next: S5 real shared backing for cross-process opens | rmbp |
| **K1+K2** | Reboot-surviving ACL: the U6 owner/grants persist to an on-disk `UNAFS.ATR` file (kernel-stamped `PrincipalRecord`, volume-fingerprint bound). Foundation + persistence landed (K1); **K2 turned cross-reboot enforcement LIVE** — three distinct launchable named programs on the card, the gate flipped, grow-repersist, and an end-to-end proof through REAL programs (`K2OWN.BIN` re-admitted by name after rebuild, `K2IMP.BIN` denied). **✅ METAL-CONFIRMED on real Pi 4 (2026-07-11): one-boot MBENCH 25/25, and a genuine two-boot power-cycle (`UNAOS_K2_LEAVE`) — the owned file's ACL survived a real power-cut and was enforced on the next boot** | pi4 |
| **UI/gfx** | Scale-aware UI metrics (no absolute pixel sizes), the in-kernel `pulse` monitor + `apps/pulse` host vessel, the `vug` software-rendered crystal engine, and the fbcon cached-RAM shadow that kills the uncached-VRAM scroll on x86 | rmbp/ux |

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

## 3a. The creative lane (host-native A/V userspace)

The audio and image/video half of userspace: `libs/resonance` + `handlers/stria`
(audio), `libs/lux` + `apps/facet` (image). Host-native only — zero kernel scope;
the kernel-side audio gap (x86 HDA, Pi HDMI/PWM) remains §6's separate row.
Direction set 2026-07-11 (dedicated A/V design session, ground-truthed on the Mac).

**Ground truth (2026-07-11):** resonance already makes sound — its cpal path drives
the default output device and its graph/oscillator/FFT are real and tested — but the
engine is dishonest at the edges: the graph's sample rate is hard-coded 44.1 kHz
while the device runs 48 kHz (pitch audibly sharp, confirmed by ear against a 440 Hz
reference), the command path drains and discards every message, and the interactive
example no longer compiles. stria's window skeleton references a windowing API that
no longer exists anywhere in the repo and is not a workspace member. lux decodes
Sony ARW only (no tests, no fixtures); facet is a README.

**Ordering: audio first.** The audio side is one honest arc from a felt,
controllable instrument; the image side needs decode capability built before its
vessel can exist.

| Arc | Content | Review tier |
| :--- | :--- | :--- |
| **AV-A1 phonolite** ✅ landed 2026-07-11 (`us-phonolite`; ear-witness attended-pending) | Make resonance honest (device sample rate into the graph, live command path, gain param, stop, nameable control handle, level readback) + the `apps/phonolite` tone vessel on the pulse pattern — start/stop, frequency/gain sliders (quartzite's first input-control idiom), level meter. Ear-witness gate: post-fix tone matches a 440 reference; pitch changes live | 2-lens |
| **AV-A2 stria** | Rewrite stria as a real bus-driven handler around the finished engine (`ignite(...)`, SMessage traffic); retire the vestigial window skeleton | 2-lens |
| **LUX-1** | Common-format decode via decoder crates (`png` + `zune-jpeg` or a trimmed `image`) feeding `RgbBuffer`; fixtures + tests; harden/fence the approximate ARW2 path | 2-lens |
| **FACET-1** | The viewer vessel: decode via lux, display in a quartzite view first (euclase textured-quad path later), pan/zoom/pixel readout after | direct read |

Policy decided: external decoder crates are in-bounds for lux (hand-rolling PNG/JPEG
is not the lane's value). `SMessage::AudioChunk`/`Spectrum` already exist — audio
arcs are expected to land with zero bandy changes.

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
| Audio (kernel) | x86 HDA; Pi HDMI/PWM audio — kernel playback, distinct from the host-native creative lane (§3a) | M each |
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
