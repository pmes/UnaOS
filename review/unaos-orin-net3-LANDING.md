# ORIN-NET-3 — landing report

**Branch:** `us-orinnet3` (base `main` @ `3fe218a`). **Knob:** `UNAOS_PCIE3=1` (feature `pcie3 = ["pcie2"]`).
**Arc:** PS widen (M1) + controller-0 link bring-up (M2) + device enumeration / BAR sizing (M3) + QEMU
witness + doc. The lane's **first deliberate fabric-write arc**.

## What landed

- **M1 — TCR PS/IPS widen 36→40-bit (knob-gated).** `mmu_tegra` `TCR_EL2_ACTIVE`/`TCR_EL1_ACTIVE` and
  `boot_tegra` `TCR_EL1_ACTIVE` fold to the NET-2 literal knob-off, flip PS `0b001→0b010` under `pcie3`.
  `map_mmio_window`'s ceiling split into `PS_OUTPUT_CEILING_GIB` (64 knob-off / 1024 `pcie3`) **and** a
  standing `L1_GIB_EXTENT = 512` array-safety guard (the 512-entry L1 table's VA extent — the binding
  limit after the widen, and the guard that keeps `l1.add(gi)` in bounds). ECAM (`0x2e_2000_0000`,
  ~184 GiB) now maps; the audit of the old `PS_GIB_CEILING=64` (grep) confirmed no other MMIO-ceiling
  site (the remaining `<64` bounds are RAM/framebuffer widths, left unchanged).
- **M2 — appl LTSSM enable (controller 0 only).** `net3_link_bringup`: one `APPL_CTRL |= LTSSM_EN`
  read-modify-write (Linux `pcie-tegra194` = documentation of record), announced before issue, then a
  finite-backstop poll of DLL-active (DBI) + `APPL_LINK_STATUS.RDLH` + `APPL_DEBUG` LTSSM state. A
  still-down link is recorded as an honest hardware result.
- **M3 — enumerate + BAR sizing.** `net3_enumerate_and_size`: walks bus1:dev0:fn0 through the now-mapped
  ECAM (`ecam_base + (1<<20)` — no iATU CFG-region write needed), poison-rejecting the identity read,
  then the all-ones/readback BAR-sizing ritual restoring each original immediately (32- and 64-bit),
  each write announced.
- **QEMU witness — `ps_widen_witness`** on the GICv3 virt path: inverts NET-2's regression (ECAM now
  REACHABLE) and asserts refusal persists at 512 GiB and 1 TiB. `mmu_tegra` module un-gated to
  `any(tegra, pcie3)` so the witness reaches the real `map_mmio_window` (L1 statics inert on virt).
- **Docs:** arch_arm64.md §ORIN-NET-3 (write ledger, two-ceiling reasoning, byte-identity) + bench
  runbook `scripts/orin-net3-bench.md`. Cargo/arroyo `UNAOS_PCIE3` wiring.

## Fabric-write ledger (the complete set this arc adds)

1. **M1:** `TCR_EL2`/`TCR_EL1` PS/IPS `0b001→0b010` at MMU-enable (system-register, knob-gated) + one
   Device-nGnRE **page-table descriptor** for the ECAM GiB (via `map_mmio_window`, bounded to the asked
   window).
2. **M2:** one `APPL_CTRL |= LTSSM_EN` read-modify-write on controller 0.
3. **M3:** per enumerated BAR, an all-ones probe write **and** an immediate restore write (≤2 per 32-bit
   BAR, ≤4 per 64-bit pair). Guarded against a 64-bit type in BAR slot 5 (would write past the BAR array).

Nothing else touches fabric/config/system registers. No bus-master/MEM decode, no MSI, no DMA, no driver
bind, no writes to any other controller, no PERST/PHY reprogram.

## Gate results (all green)

| gate | result |
|---|---|
| `check` × {off, PCIE3, PCIE3+TEGRA} both arches | OK, **zero net-new warnings** (off/PCIE3 aarch64 = 15; TEGRA/PCIE3+TEGRA aarch64 = 20; x86 = 17) |
| `test-arm 22` (GICv2 MISSION) | MISSION SUCCESS |
| `UNAOS_GICV3=1 test-arm 40` (knob-off) | CAPSTONE COMPLETE 6/6 |
| `UNAOS_PCIE3=1 UNAOS_GICV3=1 test-arm 40` | **PS-widen witness PASS** (ECAM REACHABLE; refusal @512GiB & @1TiB true) + census2 graceful skip + CAPSTONE 6/6 + VUG-HONESTY PASS |
| `kernel8-test 35` (Pi4 raspi4b) | 0 FAIL, CAPSTONE COMPLETE |
| `esp-jetson` (knob-off + PCIE3+TEGRA) | links |
| **byte-identity (knob-off vs `3fe218a`)** | `.text`/`.rodata`/`.data`/`.got` **identical**; `.data.rel.ro` **1 byte** (`Location` line `0x7a→0x87`) — the ratified class |

## Security-tier lens (self-run, folded)

One MEDIUM write-scope finding, **fixed inline**: a 64-bit BAR type in slot 5 would drive the high-half
probe to config offset `0x28` (outside the BARs-only write class) — now guarded (records malformed, no
write past the BAR array). Cleared: array-safety of the widen (L1_GIB_EXTENT guard), every fabric write
gated+logged, widened regime maps only the asked window, no protection weakened, refusal preserved.

## Metal-pending (attended sitting — not this arc's gate)

QEMU models no Tegra234 RC, so all link/device answers are attended-metal. The recon ESP is staged:
`~/unaos-bench/flash/orin/orin-net3-recon-20260717-214247.tar` (+ `.MANIFEST`, kernel.elf sha
`a005fdf3c5c71a8c…`). Pre-registered branches in the runbook: link stays DOWN (likely — record LTSSM
state) vs comes UP (enumerate + BAR sizes). Joins the RAST-PACE spin + NET-2 DBI accruals for the next
consolidated sitting.

## Flagged

- `arch/aarch64/mod.rs`: one gate line changed (`mmu_tegra` → `any(tegra, pcie3)`) so the virt witness
  can reach the real `map_mmio_window`. Minimal, tegra-module-scoped; knob-off unaffected (mmu_tegra only
  compiles on virt when `pcie3` is on). Noting it as a near-lane touch.
- Committed on `us-orinnet3` only; **not merged/pushed** (integrator merges, Peter pushes).
