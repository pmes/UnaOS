# BP-shape — rings, the tree, arches (read at 1719fe54)

## 1. What it is
- One monorepo, two layers. Ring 0 = `unaos/` (its own cargo workspace, `unaos/Cargo.toml`). Ring 3 = two things:
  host-native userspace at the root (root `Cargo.toml` workspace) and on-UnaOS ring-3 ELFs (the `user-*` crates in `unaos/crates/`, each its own workspace root).
- Shared `no_std` cores live under `unaos/libs/` (`fs`, `media`, `ml`, `net`, `sys`, `text`, `vcs`, `web`) as `<name>_core`; the root workspace builds them, the kernel takes them as path deps.
- Arches: x86_64 (rMBP metal; QEMU q35) and aarch64 (Pi 4 bare-metal `kernel8.img`; Jetson Orin via UEFI, feature `tegra`; QEMU `virt`, `raspi4b`).
- The kernel has three entry shapes: x86 UEFI (`unaos/crates/kernel/src/main.rs:14`), aarch64 UEFI (`unaos/crates/kernel/src/main.rs:21`), aarch64 bare-metal Pi (`unaos/crates/kernel/src/main.rs:31-77`). All reach `kernel_main` (`unaos/crates/kernel/src/main.rs:80`).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-shape-1 | The syscall ABI is declared once, in `una-abi`; both kernels and every `user-*` crate import it. No crate re-types a number. | `unaos/crates/una-abi/src/lib.rs:4-17`; `unaos/Cargo.toml` members comment | compile (a divergence is a type error) |
| BP-shape-2 | A capability both rings need is ONE `no_std` core in `unaos/libs/`, linked by the kernel and the host/ring-3 owner (seam `shared-core`). | `Cargo.toml:9-60`; `unaos/crates/kernel/Cargo.toml:3897-3996` | `gates` arch row `sharedcore` leg (`unaos/scripts/arch-check.py:17`) |
| BP-shape-3 | Kernel targets: x86_64 is `+soft-float` with MMX/SSE/AVX off; aarch64 is `+v8a,+strict-align,+neon`. | `unaos/x86_64-unaos.json:8`; `unaos/aarch64-unaos.json:7` | build |
| BP-shape-4 | Names in any file both arches compile are subsystem names, never board names (R16). | `docs/dev/LAWS.md:185` | `gates` row `neutral` (`tools/neutral-check.py`, exceptions `tools/neutral.allow`, SR166) |
| BP-shape-5 | ONE OS: experience-layer code gates on its feature knob, never on `target_arch` without a stated hardware reason. | `docs/dev/LAWS.md:186-188` | unheld (review) |
| BP-shape-6 | The desktop family is one cfg: x86 `wc` OR aarch64 `desktop_firmware`. A desktop file compiles under exactly that. | `unaos/crates/kernel/src/video/mod.rs:691`; `unaos/crates/kernel/src/splash.rs:708-716` | `./arroyo check` both arches |
| BP-shape-7 | No board, bus, slot, serial or card geometry in kernel source; the root is found by content. | `docs/dev/LAWS.md:188-189` `docs/dev/LAWS.md:190-228` | unheld (review); see BP-storage-3 |
| BP-shape-8 | Every new kernel file in `unaos/crates/kernel/src/video/`, `unaos/crates/kernel/src/fs/`, `unaos/crates/kernel/src/install/`, `unaos/crates/kernel/src/selfhost/` and the shell files starts with a `CHARTER:` line naming its owning handler and seam (`wm`/`driver`/`fs-core`/`shared-core`/`fulfiller`/`kernel-by-ruling R<n>`/`owed B<n>`). | `docs/dev/EXECUTOR-BRIEF.md:44-52`; `docs/dev/LAWS.md:35-38` | `gates` row `charter` (`unaos/scripts/charter-check.sh`) |
| BP-shape-9 | Handler purpose comes from `docs/CODEX.md` §2, never from a README. | `docs/dev/LAWS.md:33-34` | review |
| BP-shape-10 | Dependencies: built in UnaOS when possible, else latest stable; a crate doing a claimed capability is owed a core (R83). | `docs/dev/LAWS.md:288` | `gates` row `deps` (`tools/deps-audit/run.py`) |
| BP-shape-11 | GPL-3.0-or-later; no vendor firmware in the tree; vendor sets are staged on media with digests pinned in-tree (R52, R127). | `docs/dev/LAWS.md:292-299`; `unaos/firmware/b43.pins` `unaos/firmware/kepler.pins` `unaos/firmware/fonts.pins` | `gates` row `firmware` (`tools/firmware-census.py`, the `git ls-files` census, SR166) |
| BP-shape-12 | The Jetson has no QEMU model: an aarch64 `tegra` green means compiles and links, never runs. | `docs/dev/LAWS.md:171-177` | `./arroyo check` (tegra leg) |
| BP-shape-13 | The hardware census is ONE table read from the drivers' own registries (HWREPORT B725, BP-bar-25): the PCI function table the boot's PCI init records once (config reads, MF-gated), the xHCI slot table (a non-waiting loan), the EHCI endpoint table (`try_lock`), each class driver's own slot or id, the SATA and SD block registries, the SMC and the MADT count. It issues no transfer and no bus walk of its own; a held registry marks the census busy, never guessed. Driver = the claiming module's name or `none`; state = `up` / `held` / `absent` / `refused:<why>`. | `unaos/crates/kernel/src/hw.rs:88` (`census`); `unaos/crates/kernel/src/arch/x86_64/pci.rs:1218` (`functions`) `unaos/crates/kernel/src/arch/x86_64/pci.rs:1173` (the one recording call); `unaos/crates/kernel/src/drivers/xhci/mod.rs:18006` (`census_slots`); `unaos/crates/kernel/src/drivers/ehci/mod.rs:19989` (`census`) | `tests hwreport` (`:: HWREPORT: … rows_ok=1 -> PASS ::`: per-bus rows equal the registries' counts) |

## 3. Seams
- Ring 0 may call: the `unaos/libs/` cores, `una-abi`, `unaos/crates/net`, `unaos/crates/rast`, `unaos/crates/boot-info`, `unaos/crates/xusb-fw`. Never `libs/`, `handlers/`, `vessels/` (`unaos/Cargo.toml` `exclude = ["libs"]`).
- Ring-3 ELFs may call: `una-abi`, the `unaos/libs/` cores. Host userspace may call the cores and `libs/`.
- `unaos/libs/sys/helm` is a root-workspace member; the kernel does not link it at 1719fe54 (absent from `unaos/crates/kernel/Cargo.toml` path deps).

## 4. Top-level tree
| dir | purpose (read from its manifest/README at 1719fe54) |
|---|---|
| `unaos/` | Ring 0: kernel, loader (`crates/bootloader`), builder, `una-abi`, ring-3 ELF crates, shared cores, `arroyo`, scripts, firmware pins |
| `libs/` | host libraries: `gneiss_pal` (platform layer), `bandy` (bus), `quartzite` (GUI), `elessar` (workspace), `euclase` (wgpu), `lux` (images), `resonance` (audio), `text_host`, `input`/`pwm` (rover lane) |
| `handlers/` | the CODEX handlers as host crates (22 dirs: the 21 + `sonde`) — see handlers.md |
| `vessels/` | host apps: `lumen`, `una`, `pulse`, `phonolite`, `facet-view`, `aether-shell` |
| `tools/` | host CLIs and gate scripts (`status-check.py`, `deps-audit`, `unafs`, `una-card`, …) |
| `docs/` | CODEX, ROADMAP, PRODUCT, SECURITY; `docs/dev/` rules, ledgers, queues, evidence |
| `kits/`, `system/`, `fonts/`, `scripts/` | theme kit record, trust bundle, vendored faces, Pi V3D probes (purpose of each `unverified` beyond listing) |

## 5. Ruled, unbuilt
- CODEX Amendment IV (Sonde as the 22nd handler) is drafted, not signed: `docs/CODEX.md:15` still says The 21 (R112).
- The firmware census (BP-shape-11) runs as the `firmware` gates row; its call from ledger-check is owed (SR166).

## 6. Read further
- `docs/dev/LAWS.md` §3 — the cross-cutting rules this sheet cites.
- `docs/CODEX.md` — the canon (handlers, Gneiss, zones).
- `docs/dev/OS/01_BOOT_HAL/arch_x86_64.md`, `arch_arm64.md` — per-arch HAL.
- `docs/dev/review/ARCH-2026-10-06.md` — the architecture review GATE-ARCH came from.
