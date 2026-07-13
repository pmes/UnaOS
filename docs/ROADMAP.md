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

## 1b. Networking: sockets on the mature stack (direction, 2026-07-12)

**Decision (Peter): adopt the mature TCP/IP crate — [smoltcp](https://github.com/smoltcp-rs/smoltcp)
(0.13.x, 0BSD, `no_std`, heap-optional) — and retire the hand-rolled protocol
line to "possible future items."** What exists today and stays: the e1000e
driver (`drivers/e1000.rs` — PCI, MSI RX on vector 0x41, DMA rings) is the
device layer smoltcp binds to; the hand-rolled `crates/net` protocol crate
(ARP/ICMP/UDP/DHCP + the Go-Back-N TCP engine) keeps working knob-off and
remains in-tree as reference until the smoltcp line fully replaces its shell
surface. There is **no net syscall surface yet** (ring 3 cannot reach the
network); the socket syscall family opened at number 19 (SOCK-2 landed 19–22, SOCK-3 landed 23–25; next free number: 26).

| Arc | Content | Track |
| :--- | :--- | :--- |
| SOCK-1 ✅ 🔬 | **Landed (round 9, `net-sock1`).** smoltcp 0.13.1 (0BSD, `no_std`, static buffers) + the `E1000Phy` `Device` adapter over the e1000e rings, behind `UNAOS_SMOLNET` (knob-off byte-identical, both arches). Shell `ping`/`arp`/`netinfo` ride smoltcp's ICMP socket + interface knob-on; the boot witness pings slirp's gateway ×4 (`:: SOCK-1: … 4/4 replies — witness OK ::`). No syscalls. See [`08_NET/networking.md`](../unaos/docs/dev/OS/08_NET/networking.md). | x86 first (aarch64 has no wired NIC) |
| SOCK-2 ✅ 🔬 | **Landed (round 9, `net-sock1`).** The UDP socket syscall family over a persistent smoltcp `SocketSet`, behind `UNAOS_SMOLNET` (knob-off byte-identical, both arches): `sys_socket`(19)/`sys_bind`(20)/`sys_sendto`(21)/`sys_recvfrom`(22). A socket is a capability (`KIND_SOCKET`, send=`CAP_WRITE`/recv=`CAP_READ` at the File `handle_resolve` CHECK); `recvfrom` is non-blocking (`-EAGAIN`). A ring-3 fixture completes a datagram round-trip to slirp's DNS (`:: SOCK-2: ring-3 udp round-trip … -> PASS ::`) + a kernel witness (`:: SOCK-2: … — witness OK ::`). Next free syscall number: **23**. See [`08_NET/networking.md`](../unaos/docs/dev/OS/08_NET/networking.md) + [`SECURITY.md`](SECURITY.md). | x86 |
| SOCK-3 ✅ 🔬 | **Landed (round 10, `net-sock1`).** TCP **client** sockets over the same persistent smoltcp stack, behind `UNAOS_SMOLNET` (knob-off byte-identical, both arches): `sys_connect`(23)/`sys_send`(24)/`sys_sock_recv`(25); `sys_socket` gains `SOCK_STREAM`. A TCP socket is the same `KIND_SOCKET` capability (connect/send=`CAP_WRITE`, recv=`CAP_READ`); `connect` is non-blocking with a ring-3 poll model (`0`/`-EINPROGRESS`/`-ECONNREFUSED`), `recv` non-blocking (`-EAGAIN`/`0`-EOF). Carries the two SOCK-2-review REQUIRED folds: the socket handle value word is now **gen-fenced** `(gen<<32)|(sid+1)` (recycled-slot UAF closed for all socket kinds), and every TCP pump releases the `STACK` lock between chunks. A ring-3 fixture completes a byte-stream round-trip to slirp's resolver over DNS-over-TCP (`:: SOCK-3: ring-3 tcp round-trip … -> PASS ::`) + a kernel witness (`:: SOCK-3: … — witness OK ::`). Next free syscall number: **26**. See [`08_NET/networking.md`](../unaos/docs/dev/OS/08_NET/networking.md) + [`SECURITY.md`](SECURITY.md). | x86 |
| SOCK-4+ | TCP server/listen sockets; transferable sockets (the gen fence is in place); DHCP via smoltcp; aarch64 NIC bring-up joins here (§6 row) | later |

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
| K1 | **✅ `no_std + alloc` port of the core (landed).** `libs/unafs` compiles `#![no_std]` under `--no-default-features` (host and the kernel's `aarch64-unknown-none-softfloat` target); the host-native surface — `FileDevice`, the mmap reader, the bandy event bus, the `sqrt`-using query engine — sits behind a default-on `std` feature so every downstream consumer builds unchanged. bincode 1.3 → 2.x (its `legacy()` config) with the **on-disk byte layout preserved and pinned by golden-vector KATs** (`tests/kat_vectors.rs`) |
| K2 | **✅ Block adapter (landed).** `libs/unafs/adapter.rs` presents a 512 B-sector kernel device (a generic `SectorDevice` trait, host-tested with `MemSectorDevice`) as unafs's 4096 B `BlockDevice` — one block ↔ eight contiguous sectors at `base_lba + block*8`, all arithmetic `checked_*`. `parse_partitions` reads GPT (protective MBR + LBA-1 header) and MBR tables with signature/entry-size/LBA-order/extent bound checks; `locate_unafs` finds the volume by its `UNAFS` superblock magic. `no_std` + `alloc`; 16 adapter tests + synthetic GPT/MBR fixtures |
| K3 | **✅ Kernel read-only mount (landed + METAL-CONFIRMED 2026-07-12: `K3-mount PASS [w=0x1ff]` on real Pi 4 silicon ×5 boots, interactive `uls`/`ucat` panel-witnessed).** `fs/unafs.rs` wraps the kernel's 512 B block layer as the K2 `SectorDevice` (`write_sector` a deliberate `Io` stub — writes are K4's), `locate_unafs` finds the volume by superblock magic (staged as MBR partition 2 on the Pi image), `UnaFS::mount` gives a live RO mount; shell `uls`/`ucat` + a byte-verified `K3-mount` witness (`w=0x1ff`: superblock, root `ls`, single- and multi-block reads, negative resolve, RO-seam refusal). The dirty-mount warning now reaches the kernel console via a `no_std` warn hook. `query` deferred: the engine is `std`/FP-`sqrt`-gated |
| K4 | Journaled kernel writes + a minimal VFS (retires read-only FAT as the storage story) |

Order: F1 → F2 → K1/K2 → **K3 (metal milestone)** → F3 → F4 → **F5 (BeFS
surpassed)** → K4. The F-arcs are host-native — ideal parallel work needing no
hardware session.

The **security-K4 residual** ([`SECURITY.md`](SECURITY.md) §K1 — migrate the
kernel's `UNAFS.ATR` FAT-bridge sidecar onto native unafs typed attributes,
then delete it) sits on top of this chain: it needs a unafs filesystem the
kernel can mount, i.e. this K1 (`no_std` core, ✅ landed) → K2 (block adapter,
✅ landed) → K3 (RO mount) → K4 (journaled writes + the migrate pass). The K4-ready
projection codec (pi4, 2026-07-12) is the layer that migrate pass will call.
The three K-numbering schemes are distinct — this is BeFS-K1, not the security
ACL K1 (survive-reboot) nor the U-chain.

## 3. Design → Make (printer)

Canon: [`CODEX.md`](CODEX.md) §5 — Vug computes toolpaths, Comscan pumps
G-code to hardware. Sequenced routes:

1. **PrusaLink HTTP** (~2 arcs): HTTP client grows PUT + API-key auth over the
   TCP stack (now the smoltcp line — §1b) → upload G-code to the Prusa CORE
   One+ and start a print. *"UnaOS prints a part"* early.
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
| **AV-A2 stria** ✅ landed 2026-07-11 (`us-stria`) | Rewrote stria as a real bus-driven handler around the finished engine: `StriaHandler::ignite(synapse)` owns the graph + engine lifecycle, a real `BandyMember` publishes `SMessage::Spectrum` level beats (and `AudioChunk` frames) on the Synapse, and a single-owner control task drives frequency/gain/running-state respecting the stop/start ordering contract; vestigial `gneiss_pal::WaylandApp` skeleton retired (crate is now a library). Folds the AV-A1 review notes on ordering, no-re-entry, and liveness-desync | 2-lens |
| **LUX-1** ✅ landed 2026-07-11 (`us-lux1`) | Common-format decode via `png` + `zune-jpeg` feeding `RgbBuffer`: `decode`/`sniff_format` dispatch on magic bytes, PNG (palette/grayscale/16-bit normalized, alpha dropped) and JPEG (forced RGB) both converted sRGB→linear to honor the linear-`RgbBuffer` contract; tiny committed fixtures + round-trip/fail-closed tests. Fenced the ARW path: tag-named dimensions bounded (`≤ 512 MP`, non-zero) before any allocation, plus fail-closed tests on garbage input | 2-lens |
| **FACET-1** ✅ landed + merged 2026-07-12 (`main`; eye-witness passed) | The `apps/facet` viewer vessel on the pulse/phonolite single-view idiom: `facet <image>` reads a file, decodes via `lux` (`decode` dispatch on magic bytes), packs the linear `RgbBuffer` through the sRGB OETF to 8-bit RGBA, and shows it aspect-fit/centered in a quartzite window via a new additive `platforms::macos::image_view` module (CPU blit through `NSBitmapImageRep`, tagged sRGB for color-managed display; rescales with the window) | direct read |
| **FACET-2** ✅ landed 2026-07-12 (`us-facet2`; eye-witness attended-pending) | Interaction for the viewer: `FacetImageView` gains a zoom/pan transform and handles its own pointer input directly on the `NSView` (quartzite's first pointer idiom, the twin of tone_panel's control idiom) — scroll-wheel / trackpad-pinch **zoom about the cursor**, click-drag **pan**, `0`/`f` **reset-to-fit**, and a live **pixel readout** overlay (`NSTrackingArea` mouse-move → source pixel coords + packed 8-bit sRGB in decimal/hex + the source linear RGB `lux` decoded). Still an additive extension of `image_view` — `NSEvent`/`NSTrackingArea` reached via `class!`+`msg_send!`, quartzite `Cargo.toml` zero-diff. Euclase textured-quad (GPU) path is the remaining later arc | 2-lens |

Policy decided: external decoder crates are in-bounds for lux (hand-rolling PNG/JPEG
is not the lane's value). `SMessage::AudioChunk`/`Spectrum` already exist — audio
arcs are expected to land with zero bandy changes.

## 3b. The on-UnaOS program story: bandy-on-metal (direction, 2026-07-11)

Decided in design discussion (Peter + seat, 2026-07-11). The question "what is a
real program on UnaOS?" is **not** answered by porting POSIX conventions
(exec/argv/stdout/exit). Host userspace already rejects that model: the unit of
behavior is a **handler speaking `SMessage` on the bus**, and executables
(vessels) are wiring and lifecycle, not domain logic. The on-UnaOS program story
is therefore: **port the bus, not the binary convention.**

Principles, in force for every arc that touches this seam:

1. **Smart commands, not apps.** Shell commands (`cp`, `cat`, `ls`, …) are
   *verbs*: midden parses text into typed messages; whoever owns the capability
   fulfills them — the kernel's FAT machinery on metal, amber_bytes/geode on the
   host. No process spawn, no PATH, no byte-stream reparsing between pipeline
   stages. The in-kernel shell's command table is the proto-form; it factors
   into "midden's verb set, fulfilled by the capability owner."
2. **Every message carries the caller's principal.** The K1/K2 named-principal
   ACL (pi4 track) is the security model for smart commands: a verb fulfilled
   in-kernel runs with the *invoker's* grants, never ambient authority. This is
   designed in from message one — principal-stamping is not retrofittable.
3. **The verb/fulfiller seam stays honest.** A command is addressable the same
   way whether fulfilled in-kernel, by a handler, or by a spawned vessel —
   "is it an app?" is a deployment detail, not an interface difference.
   Third-party commands later register as fulfillers.
4. **Interfaces are verb *generators*, and they are interchangeable.** midden
   (text), vug and the future spatial desktop (graphical/3D), and an AI (vein,
   or a small local model managing elessar spaces) all *generate* the same
   verbs. The AI is one more fulfiller/generator on the bus — never a required
   layer. AI-off configurations (tinkerer builds, hardened builds) are
   first-class: unregister vein and everything still works, with every action
   principal-attributed and ACL-checked.
5. **Elessar is the declarative source of truth.** A workspace (with principia
   settings) *exports* to platform apps — `apps/una` on macOS today; on-UnaOS
   midden+vug is just another export target once the bus runs there. "Shed the
   weight of apps": the desktop becomes a scene of live capabilities viewed
   over the bus, not a window manager for monoliths.

First concrete arc (when sequenced): define the on-UnaOS `SMessage` transport
seam — syscall-backed send/receive, principal-stamped, same wire shape bandy
uses — and round-trip midden's first verbs (`ls`, `cat`, `cp`) through kernel
fulfillment in QEMU. K2's named programs supply the principal stamping. Midden
becomes program #3 — the first real (non-fixture) on-UnaOS program.

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
  I2C driver is reusable for sensors). **The actuation codec now exists
  host-native** in [`libs/pca9685`](../libs/pca9685) (`#![no_std]`,
  zero-dependency): prescale + per-channel duty register writes, frozen by
  datasheet KATs, with the `drive`↔`pca9685` µs→duty seam proven host-side. What
  remains is the I2C transport + the attended actuation gate.
- **Drive service** (arch-neutral): bounded command channel, 20 ms control
  loop, ARM/DISARM state machine, 500 ms deadman, software throttle cap,
  cross-core watchdog, panic-handler-forces-neutral. A 3-position transmitter
  channel selects DISARM / MANUAL / AUTO. **The arch-neutral core now exists
  host-native** in [`libs/drive`](../libs/drive) (`#![no_std]`, embeds in the
  kernel later); its safety invariants I1–I8 are proven by the test battery in
  `libs/drive/tests/invariants.rs`.
- **Power**: Orin barrel input is 7–20 V → 3S LiPo direct.
- Milestones: GICv3/scheduler (shared with the Jetson catchup lane) → i-BUS
  decode demo **(host-side landed — the `libs/ibus` codec + format-freeze KATs;
  synthetic until real FGr8B captures are appended, then silicon)** → actuation
  codec **(host-side landed — the `libs/pca9685` prescale/duty codec + datasheet
  KATs; the I2C transport + attended actuation gate remain)** → drive service →
  first crawl, each behind a written safety-interlock
  checklist ([`docs/dev/USERLAND/ENDURO_SAFETY.md`](dev/USERLAND/ENDURO_SAFETY.md)).
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
