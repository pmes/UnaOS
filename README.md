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
| [unafs](libs/unafs) | Virtual filesystem client. |
| [lux](libs/lux) | Image decoding (incl. camera RAW). |

**Handlers** ([`handlers/`](handlers)) — domain services, each owning one
capability area. Implemented today: [vein](handlers/vein) (AI/LLM),
[matrix](handlers/matrix) (spatial files), [midden](handlers/midden) (shell),
[principia](handlers/principia) (system config), [tabula](handlers/tabula) (text),
[vaire](handlers/vaire) (version control), [aule](handlers/aule) (build),
[amber_bytes](handlers/amber_bytes) (storage), [stria](handlers/stria) (A/V).
Design-stage: [aether](handlers/aether), [comscan](handlers/comscan),
[geode](handlers/geode), [holocron](handlers/holocron), [mica](handlers/mica),
[obsidian](handlers/obsidian), [xenolith](handlers/xenolith),
[zircon](handlers/zircon), [vug](handlers/vug). The full manifest is in
[`docs/CODEX.md`](docs/CODEX.md).

**Vessels** ([`apps/`](apps)) — the executables: [lumen](apps/lumen) (the AI
companion and reference GUI vessel), [facet](apps/facet) (raster graphics),
`una` (IDE, currently parked), and the CLI tools under [`apps/cli/`](apps/cli).

The userspace architecture is documented in
[`docs/dev/USERLAND/ARCHITECTURE.md`](docs/dev/USERLAND/ARCHITECTURE.md).

---

## Kernel status

| Subsystem | Status |
| :--- | :--- |
| UEFI boot + GOP framebuffer (both arches) | ✅ |
| Local-APIC interrupts (timer / xHCI MSI / NIC MSI / IPI) | ✅ |
| Memory: page tables + heap | ✅ |
| SMP: x2APIC + ACPI MADT + AP startup; preemptive per-CPU scheduler + sync primitives | ✅ (x86_64) |
| USB/xHCI: interrupt-driven, Bulk-Only-Transport mass storage, HID input | ✅ |
| Network: e1000 (MSI RX) + hand-rolled TCP/IP (ARP/ICMP/DHCP/UDP/TCP) | ✅ |
| Video: `FrameBuffer` + double-buffered `Screen` + boot/panic console | ✅ |

The USB+scheduler, network, and video tracks were developed in parallel and are
**integrated and verified booting together** on the `c01-int_combined` branch.

Beyond `main`, development runs on three hardware tracks in parallel:
**`hw-rmbp`** (2012 MacBook Pro, x86_64 — boots on metal with USB input and
mass storage, native video, FAT read), **`hw-pi4`** (Raspberry Pi 4,
bare-metal aarch64 — SMP scheduler, GUI, and the project's first privilege
boundary: EL0 userspace with syscalls, per-page permissions, and
fault→task-kill), and **`hw-jetson`** (Jetson Orin Nano — early bring-up).

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

---

*Est. 2026 — The Architect & Una*
