# UnaOS

A clean-sheet, Rust-based operating system and creative environment, built on a
single philosophy: **the OS should be fast, spatial, and fun.**

UnaOS rejects the "document model" of modern computing in favor of a
low-latency, spatial architecture with two complementary goals:

1. **The Formula 1 Car** — a real-time kernel for high-end creative workflows,
   where the OS gets out of the way.
2. **The Lazarus Machine** — a stripped-down, bare-metal runtime that revives
   "obsolete" hardware by removing decades of legacy bloat.

---

## Architecture

UnaOS is a monorepo with two layers that build independently.

### Ring 0 — the kernel (`unaos/`)
A bare-metal Rust kernel for **x86_64 and aarch64**. UEFI boot, a pure local-APIC
interrupt system (no legacy PIC), an SMP preemptive scheduler, and drivers for
USB/xHCI storage, Intel e1000 networking, and the UEFI-GOP framebuffer. See
[`docs/dev/OS/`](docs/dev/OS) and [`unaos/crates/kernel`](unaos/crates/kernel).

### Ring 3 — the userspace

**Core libraries** ([`libs/`](libs)):

| Crate | Role |
| :--- | :--- |
| [gneiss_pal](libs/gneiss_pal) | Platform abstraction layer (fs, net, geometry, DSP, windowing). |
| [bandy](libs/bandy) | The message bus: the `SMessage` enum + the `Synapse` broadcast channel. |
| [quartzite](libs/quartzite) | The native multi-platform GUI API (macOS AppKit, Linux GTK4, Qt). |
| [elessar](libs/elessar) | Workspace/context detection. |
| [euclase](libs/euclase) | WGPU rendering. |
| [resonance](libs/resonance) | Audio engine & DSP. |
| [unafs](unaos/libs/fs/unafs) | Virtual filesystem client. |
| [lux](libs/lux) | Image decoding (incl. camera RAW). |

**Layout convention.** Following Linux precedent, a device-class or domain grows
its own directory under `libs/` — the directory names the class, the leaf names
the crate/part: `unaos/libs/fs/unafs`, `libs/input/ibus`, `libs/pwm/pca9685` (with
`net/`, `video/`, `media/` to come). A crate stays flat at `libs/<crate>` until a
second family member joins it, at which point the pair moves under a class
directory. Crate names never change with the move — only paths.

**Handlers** ([`handlers/`](handlers)) — domain services, each owning one
capability area. Implemented today: [vein](handlers/vein) (AI/LLM),
[matrix](handlers/matrix) (spatial files), [midden](handlers/midden) (shell),
[principia](handlers/principia) (system config), [tabula](handlers/tabula) (text),
[vaire](handlers/vaire) (version control), [aule](handlers/aule) (build),
[amber_bytes](handlers/amber_bytes) (disk forensics), [stria](handlers/stria) (A/V).
Design-stage: [aether](handlers/aether), [comscan](handlers/comscan),
[geode](handlers/geode), [holocron](handlers/holocron), [mica](handlers/mica),
[obsidian](handlers/obsidian), [xenolith](handlers/xenolith),
[zircon](handlers/zircon), [vug](handlers/vug). The full manifest is in
[`docs/CODEX.md`](docs/CODEX.md).

**Vessels** ([`vessels/`](vessels)) — the executables: [lumen](vessels/lumen) (the AI
companion and reference GUI vessel), [facet](vessels/facet) (raster graphics),
[pulse](vessels/pulse) (the system monitor — live per-core CPU bars, a BeOS Pulse
homage, fed through a `PulseSource` seam a kernel telemetry feed will later
back), `una` (IDE, currently parked), and the CLI tools under
[`tools/`](tools).

The userspace architecture is documented in
[`docs/dev/USERLAND/ARCHITECTURE.md`](docs/dev/USERLAND/ARCHITECTURE.md).

---

## Kernel status

| Subsystem | Status |
| :--- | :--- |
| UEFI boot + GOP framebuffer (both arches) | ✅ |
| Local-APIC interrupts (timer / xHCI MSI / NIC MSI / IPI) | ✅ |
| Memory: page tables + heap | ✅ |
| SMP: preemptive per-CPU scheduler + sync primitives (x86 x2APIC/MADT; aarch64 4-core Pi + PSCI CPU_ON on Jetson) | ✅ |
| USB/xHCI: interrupt-driven, Bulk-Only-Transport mass storage, HID input | ✅ |
| Network: e1000 (MSI RX) + hand-rolled TCP/IP (ARP/ICMP/DHCP/UDP/TCP) | ✅ |
| Storage: FAT16/32 read + write / grow / create / delete; interrupt-driven on x86 (STOR-1, behind the `irqstorage` knob) | ✅ |
| Video: scale-aware `FrameBuffer` + double-buffered `Screen` + boot/panic console (cached-RAM fbcon shadow on x86) | ✅ |

The USB+scheduler, network, and video tracks were developed in parallel and are
**integrated and verified booting together** on `main`, exercised each merge by
the `./arroyo battery` suite (both arches, QEMU x86 + `virt` GICv2/v3 + `raspi4b`).

### Privilege & security chain — the current focus

The capability-isolated userspace (see *Current direction* below) advanced
arc-by-arc, x86 leading the shared kernel-core and aarch64 (the pioneer of the
privilege boundary) porting each rung. **The whole chain below is now landed and
metal-confirmed on both arches.** ✅ metal-confirmed · 🔬 QEMU-green, metal pending.

| Arc | What it lands | x86 (`hw-rmbp`) | aarch64 (`hw-pi4`) |
| :--- | :--- | :---: | :---: |
| Privilege round-trip | ring 3 / EL0 with syscalls | ✅ U1a | ✅ M6a |
| Per-page perms + fault→kill | W^X/NX, faults kill the task not the kernel | ✅ U1b | ✅ M6b |
| Loadable program | a program run from an on-disk blob | ✅ U2 | ✅ M6c |
| Preemptible userspace | timer preempts a running user task | ✅ U3.5 | ✅ M6e |
| Per-process address space | isolated per-task page tables | ✅ U3 *(CR3)* | ✅ M6d *(TTBR0+ASID)* |
| Process model + handle table | PIDs, spawn/wait/exit, per-process handles | ✅ U4x | ✅ U4 |
| Capabilities | handles are capabilities; grant/attenuate/revoke | ✅ U5x | ✅ U5 |
| Cross-process transfer + revocation | inbox transfer, generation-tagged revoke trees | ✅ U7x/U8x | ✅ U7/U8 |
| File writes → grow / create / delete | RW files on FAT + full lifecycle | ✅ U9x–U11x | ✅ U9–U11 |
| UnaFS owner/grants ACL | owner-by-default at `open` + `SYS_FGRANT` delegation | ✅ U6gx | ✅ U6 |

Milestone-by-milestone history with test evidence:
[`docs/MILESTONES.md`](docs/MILESTONES.md); hardening ledger in
[`docs/SECURITY.md`](docs/SECURITY.md).

**Current frontiers** (building on the chain above):

- **Interrupt-driven x86 storage (STOR-1)** — read / write / grow / create /
  delete run synchronously *in the syscall* via a scheduled storage service
  task, behind the `irqstorage` knob; the core transfer-IRQ mechanism is
  metal-confirmed on the real Panther Point xHCI.
- **ACL persistence (pi4 K1)** — the owner/grants ACL survives reboot via an
  on-disk `UNAFS.ATR` file; the persistent-principal foundation has landed
  (cross-reboot *enforcement* is proven and gated pending a second launchable
  named program).
- **UI + graphics** — a scale-aware UI metrics layer (no absolute pixel sizes),
  an in-kernel `pulse` monitor, and the `vug` software-rendered crystal engine.

Beyond `main`, development runs on three hardware tracks in parallel:
**`hw-rmbp`** (2012 MacBook Pro, x86_64 — boots on metal with USB input and mass
storage, native video, and the full capability chain through disk-backed RW
files with a per-name ACL), **`hw-pi4`** (Raspberry Pi 4, bare-metal aarch64 —
SMP scheduler, GUI, the capability chain, SMP-hardened concurrent FS, and
reboot-surviving ACL persistence), and **`hw-jetson`** (Jetson Orin Nano —
GICv3 + PSCI SMP, an interactive panel shell with a read/write FAT path,
metal-confirmed on real Orin silicon). Per-platform debugging setup:
[`docs/dev/DEBUGGING.md`](docs/dev/DEBUGGING.md).

**Current direction — multi-user first.** UnaOS is designed to hold direct
authority over physical hardware (printers, vehicles, GPIO), so privilege
separation and a capability-based security model are being built *now*, not
retrofitted: a per-process handle table where handles are capabilities, with
principals and grants stored as UnaFS typed attributes. The near-term goal is
a functioning desktop — a shell that spawns capability-scoped programs loaded
from disk into isolated address spaces. See [`docs/ROADMAP.md`](docs/ROADMAP.md)
and [`docs/SECURITY.md`](docs/SECURITY.md).

Per-subsystem detail: [`docs/dev/OS/`](docs/dev/OS).

---

## Building & running

From `unaos/`:

```
./arroyo check          # type-check the kernel for both arches
./arroyo test [secs]    # headless x86 boot; serial -> target/serial.log
./arroyo x86            # x86 GUI in QEMU
./arroyo arm            # aarch64 GUI in QEMU
cargo test -p net       # network-stack host unit tests
```

The host-native userspace builds with a normal `cargo build` from the repo root.

## Try UnaOS in a VM

`./arroyo vm-image` (from `unaos/`) packages the same x86 boot products the
`esp-x86` path proves into **one self-contained, distributable disk image**:

```
./arroyo vm-image       # -> target/vm/unaos-x86-<git7>.img  (+ printed sha256)
```

The `.img` is a GPT disk with a FAT32 EFI System Partition carrying the boot
tree (`EFI/BOOT/BOOTX64.EFI` + `kernel.elf`) and a `README-VM.txt`. It builds in
pure Rust (the [`fatfs`](https://crates.io/crates/fatfs) crate for the
filesystem, a hand-written GPT + protective MBR in `builder/src/vm_image.rs`) —
no `mkfs.vfat`/`hdiutil`, and the disk/partition GUIDs are derived
deterministically from the git hash. It boots on any UEFI/EFI VM with **zero
UnaOS tooling on the consumer's machine**:

```
qemu-system-x86_64 -machine q35 -m 1G \
  -drive if=pflash,format=raw,readonly=on,file=<OVMF_CODE.fd> \
  -drive if=none,id=unastick,format=raw,file=target/vm/unaos-x86-<git7>.img \
  -device qemu-xhci -device usb-storage,drive=unastick,bootindex=0 \
  -serial stdio
```

The disk is attached over **USB** on purpose: UnaOS then writes its whole boot
log to a plain file, **`UNAOS.LOG`**, in the root of the image while it runs (the
"flight recorder" — the kernel drives USB storage but not IDE/SATA, so the log is
written back only over a USB attachment; any attachment still boots). After the VM
shuts down, mount the `.img` and copy `UNAOS.LOG` off it — that is the boot log to
send back, with no serial capture on the tester's side.

`README-VM.txt` inside the image has the UTM (macOS), VirtualBox, and VMware
click-paths (enable EFI, attach the disk — as a USB disk to capture `UNAOS.LOG`).
The boot log appears on the serial console and is saved to `UNAOS.LOG`; the shell
appears in the VM's graphical window.

---

*Est. 2026 — The Architect & Una*
