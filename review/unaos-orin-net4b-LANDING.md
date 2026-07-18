# ORIN-NET-4b — landing report (outbound-iATU fix-forward for FAULT-AT-M1)

**Branch:** `hw-jetson` (fast-forwarded to `main` @ `c6051e8` at session start; was 6 behind).
**Arc:** fix-forward after the NET-4 metal FAULT-AT-M1 — the first BAR register write raised a RAS
Uncorrectable (SNOC illegal-address / carveout, `a5a5a5a5` poison, DC-cut recovery). **Lane:** tegra
pcie/net files + the named docs. **QEMU cannot model any of this** — correctness by construction,
poison-honesty, and zero regression on every battery.

## M1 — adjudication (the bench observation was RIGHT)

The silicon record: BAR2 read back **`0x4000_4000`** — a **PCIe BUS address** (firmware assigns the
device's BARs inside controller-0's PCIe MEM window, PCI base `0x4000_0000`). With the DWC iATU
unprogrammed (the NET-2 finding) there is **no outbound CPU→PCIe MEM translation**, so the raw BAR
value is meaningless as a CPU physical address. The old path computed `bar_base = bar2 & !0xf =
0x4000_4000`, treated it as a CPU PA, and called `map_mmio_window(0x4000_4000, …)`. That address falls
in **GiB 1** — the SYSRAM/BPMP carveout `mmu_tegra::fill_table` maps Device-nGnRE — so `map_mmio_window`
returned `AlreadyMapped` **without complaint**, and the first register write (`CR` soft reset at
`0x4000_4000 + 0x37`) hit a protected Tegra carveout → the SNOC RAS fault + `a5a5…` poison. The shape
fully explains the fault; nothing else is needed. **Verdict: confirmed.**

## The ranges window found (resolved at runtime, not hardcoded)

Controller-0's DT `ranges` (rows of 7 cells: child PCI addr ×3, parent CPU addr ×2, size ×2) carries the
CPU aperture ↔ PCIe-address windows. The driver picks the MEM window (space code 2 = 32-bit / 3 = 64-bit)
whose `[pci_base, pci_base+size)` **contains** the firmware BAR2 value. For the observed BAR2
`0x4000_4000`: the 32-bit non-prefetch MEM window at PCIe `0x4000_0000` (size `0x1000_0000`), whose parent
CPU aperture base is the ~`0x32_…` (≈200 GiB) MMIO window NET-2 named. The exact `cpu_base` is read off
the live DTB on metal (poison-honest; refuse on a missing/foreign/disabled DTB) — no hardcoded guess.
ATU base = the `atu_dma` reg region (DWC-core `dbi + 0x30_0000` fallback documented).

## The ATU design chosen — keep + translate (DWC / pcie-tegra194 sequence-of-record)

1. **Program** DWC outbound iATU **region 0** for the whole MEM window (unrolled registers at
   `atu_base + N*0x200`: LOWER/UPPER BASE, LIMIT, TARGET, CTRL1 = TYPE_MEM, CTRL2 =
   `ENABLE|INCREASE_REGION_SIZE`) — every write announced `>>> ATU WRITE (M1-fix): …`, region enabled last.
2. **Translate, do not reassign.** Keep firmware's BAR assignment (already inside the ranges window NET-3
   sized it in) and drive the **CPU-side aperture** `cpu_addr = cpu_base + (bar_pci − pci_base)` — map
   *that* Device-nGnRE, never the raw BAR value. Reassigning the BAR would mean more fabric writes for no
   gain and diverge from the Linux DWC host model (programs outbound ATU from `ranges`, leaves enumerated
   BARs in place). **Choice recorded: keep + translate.**

Why this is safe where the raw BAR was fatal: `cpu_addr` targets the RC's own outbound MEM aperture
(RC-owned MMIO); a mistranslation or down link returns **UR / all-ones**, never a carveout. The iATU
writes hit the controller's own internal register block (GiB-0, always decoding on a powered RC —
NET-2/3 read `dbi`/`appl`/`ecam` there), not a carveout, so they carry none of the M1 fault's risk.

## M2/M3 — the guard, made law

- **Pre-write poison-honest readback.** After mapping the CPU aperture and **before any register write**,
  the driver reads **TCR** (`0x40`, chip-version bits a live RTL8168 always returns — `r8169` reads
  exactly this) and rejects poison. A poison readback ⇒ the window is not live ⇒ bring-up **REFUSED**
  cleanly, before any write, so the first-write fault can never recur.
- **`is_poison` now rejects `0xa5a5a5a5`** (the carveout fill the M1 fault left) in addition to
  `0xffffffff` / `0xdeadbeef`.
- **General rule** documented in the driver + arch_arm64.md §ORIN-NET-4b: **every new MMIO window earns a
  probe read before its first write** (the V3D-2 lesson transposed).

## What landed

| milestone | what |
|---|---|
| M1 | Adjudication (above) + design note folded into arch_arm64.md §ORIN-NET-4b. |
| M2 | `rtl8168_tegra.rs`: `resolve_atu_and_window` (DTB `ranges` + `atu_dma` resolution), `program_outbound_atu` (announced DWC unrolled-iATU writes), CPU-aperture translation, `probe_alive` TCR readback gate wired into `net4_bringup` between BAR2 resolution and the first write. |
| M3 | Guard generalized + documented; `is_poison` + `a5a5a5a5`; RAS fault signature + fix in arch_arm64.md §ORIN-NET-4b. |
| M4 | arch_arm64.md §ORIN-NET-4b fold; `scripts/orin-net4-bench.md` new expected serial chain (iATU + pre-write readback ritual + the refusal shape); this report. |

## Files touched (all in-lane)

- `unaos/crates/kernel/src/arch/aarch64/rtl8168_tegra.rs` — the fix (poison set, `probe_alive`, iATU
  block, `resolve_atu_and_window`, `program_outbound_atu`, rewired `net4_bringup`).
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — §ORIN-NET-4b.
- `unaos/scripts/orin-net4-bench.md` — updated serial chain + refusal verdict shape.
- `review/unaos-orin-net4b-LANDING.md` — this report.

## DONE gate (all green, pre-metal)

- `./arroyo check` **default** — x86_64 OK + aarch64 OK.
- `UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo check` — x86_64 OK + aarch64 OK (no rtl8168 warnings).
- `UNAOS_VNET=1 ./arroyo check` — x86_64 OK + aarch64 OK.
- `UNAOS_GICV3=1 ./arroyo test-arm 40` — CAPSTONE 6/6 + per-core idle/busy heartbeat PASS + VUG-HONESTY
  PASS.
- `UNAOS_NET4=1 UNAOS_GICV3=1 ./arroyo test-arm 40` — `PCIE4` witness line present + CAPSTONE COMPLETE 6/6.
- `./arroyo test-arm 22` — xHCI MISSION SUCCESS.
- `./arroyo test 22` (x86) — xHCI MISSION SUCCESS (unregressed).
- `./arroyo kernel8-test` (35s) — **0 FAIL**, CAPSTONE COMPLETE.

## Flagged

- **The iATU register model is correctness-by-construction and unexercised in QEMU.** The DWC unrolled
  offsets, `atu_dma`-as-ATU-base choice, and `INCREASE_REGION_SIZE` bit are from the Linux DWC /
  pcie-tegra194 model of record. The **pre-write TCR readback gate is the metal safety net**: if any of
  the ATU wiring is wrong, the readback returns UR/poison and the driver REFUSES before writing — it
  cannot fault on the first write. The attended sitting confirms the translation (live TCR + station MAC),
  not a QEMU run.
- **SMMU / cache-coherency unknowns from NET-4 are unchanged** (rings programmed with identity-physical
  addresses; `dsb sy`-only handover). Still the next metal questions after the first-write is proven.
- **No `main.rs` / lane-boundary changes** — the fix is entirely inside the driver + its named docs.
