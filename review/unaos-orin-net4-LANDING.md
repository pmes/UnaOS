# ORIN-NET-4 — landing report

**Branch:** `hw-jetson` (base `main` @ `32e6c34`). **Knob:** `UNAOS_NET4=1` (feature `net4 = ["pcie3", "dep:smoltcp"]`).
**Arc:** the Orin's FIRST network path — a Realtek RTL8168/8111 GbE driver (device claim + BAR map + MAC
read (M1); C+ RX/TX descriptor rings + init (M2)) bound to smoltcp (M3), plus a NET-2/3 recon cosmetic pass
(M0) and the doc/runbook (M4). Code-complete-prior-to-metal by design (QEMU models no Tegra234 RC).

## The ground truth this stands on (NET-1/2/3 recon; not re-litigated)

Controller-0 (`/bus@0/pcie@140a0000`, domain 8) link UP gen1 x1; device at bus1:dev0:fn0 = **Realtek
RTL8168/8111 (`0x10ec:0x8168`)**; BARs sized: I/O `0x100`, **BAR2 mem `0x1000`** (the driver's 4 KiB
register window), mem `0x4000` (MSI-X). The `LINK-UP-pre-LTSSM` observation (link trained before the M2
write) means NET-4 claims the device NET-3 found rather than re-fighting bring-up.

## What landed (commits on `hw-jetson`)

| commit | milestone | what |
|---|---|---|
| `f04333f` | M0 | cosmetic-nit pass over the NET-2/3 recon (`pcie_probe.rs`): census2's interrupt-map line `P`→`P2` (an `awk '/PCIE2/'` sweep dropped it); a garbled `dump_words` comment reworded. Both under pcieprobe/pcie2/pcie3 (OFF default) — default media unaffected. |
| `59b6a1a` | M1 | driver skeleton (`arch/aarch64/rtl8168_tegra.rs`): resolve controller-0's ecam from the live DTB (tegra-RC + firmware-okay gated), map it via the PS-widened `map_mmio_window`, read bus1:dev0:fn0, poison-reject the identity, confirm `0x10ec:0x8168`; enable `COMMAND` MEM-space + bus-master (the write NET-3 refused); resolve + map BAR2 (64-bit aware); soft-reset the MAC; read + print the station MAC. |
| `ac7e81f` | M2 | C+ RX(32)/TX(8) descriptor rings + DMA buffers from `alloc_zeroed` (identity map ⇒ pointer == physical addr, the e1000 invariant); `init_rings` in RTL8168 programming-guide / `r8169` `rtl_hw_start` order (CFG9346 unlock, RDSAR/TNPDS, RMS/MTPS/TCR, CR=RxEnb\|TxEnb, RCR last, CFG9346 lock, IMR=0/ISR clear); poison-honest TCR readback (fails bring-up on open-bus); NET4_DEVICE registry. |
| `1607a15` | M3 | transmit / rx_frame_raw (OWN\|FS\|LS + TPPoll.NPQ; length-clamped RX recycle); raw_rx/raw_tx/hw_addr accessors; a smoltcp 0.13 `phy::Device` (`Rtl8168Phy`) over the rings; `bind_smoltcp` builds an Interface (MAC + static bring-up CIDR + default route) + ICMP socket + bounded poll — the x86 e1000/smolnet seam transposed. |
| `811ac8a` | (fold) | scoped the metal-only helpers (Fdt import, is_poison, vendor/device consts) into the tegra `metal` submodule so the net4/virt build has zero dead-code warnings. |
| M4 | (this commit) | arch_arm64.md §ORIN-NET-4 (design + the r21c/NET-3 metal-facts fold: RTL8168 identity + LINK-UP-pre-LTSSM, both load-bearing) + `scripts/orin-net4-bench.md` + this report. Cargo/arroyo `UNAOS_NET4` wiring. |

## Write ledger (what the driver does that NET-3 refused)

The driver, being a driver, DOES the writes NET-3 held back — but only these, each announced on serial before
issue: (1) the `COMMAND` register MEM-space + bus-master decode-enable on bus1:dev0:fn0; (2) the RTL8168
soft reset + control/ring register program (CFG9346, RDSAR/TNPDS, RMS/MTPS/TCR/RCR, CR). It touches ONLY
controller-0's downstream device and its own BAR2 — no other controller/function, no MSI/MSI-X, no
PERST/PHY. The BAR sizing NET-3 did is not repeated (NET-3 restored the originals; the driver reads the
firmware-assigned BAR value).

## DMA identity-map invariant + the one metal risk

`mmu_tegra` maps RAM identity (VA==PA), so a heap allocation's pointer is the physical address the NIC DMAs
against (the x86 e1000 UEFI-1:1 invariant, transposed). The rings are programmed with those identity-physical
addresses. **The unknown QEMU cannot settle:** whether the SMMU (`smmu_tegra`) is translating or bypassing
controller-0's PCIe stream IDs. If, on metal, the rings never advance despite a live link, that IS the
SMMU-bypass finding (the next arc's scope) — documented in the bench runbook as the leading STOP candidate,
not something to improvise this sitting.

## Gate results (all green; metal explicitly deferred)

| gate | result |
|---|---|
| `UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo check` (both arches) | OK, **zero net-new warnings** (aarch64 = 20 = the pcie3+tegra baseline; the module emits none) |
| `UNAOS_NET4=1 ./arroyo check` (virt, both arches) | OK (aarch64 = 15 = default/pcie3 baseline) |
| `./arroyo check` (default, both arches) | OK (unchanged) |
| `./arroyo test-arm 22` (GICv2 MISSION) | **MISSION SUCCESS** (unregressed) |
| `UNAOS_GICV3=1 ./arroyo test-arm 40` (knob-off) | **CAPSTONE COMPLETE 6/6** + VUG-HONESTY PASS (unregressed) |
| `UNAOS_NET4=1 UNAOS_GICV3=1 ./arroyo test-arm 40` | the `PCIE4` witness line fires (`RTL8168 driver compiled; no Tegra234 RC … metal-only`) **and** CAPSTONE still COMPLETE 6/6 — the net4 virt build does not perturb the GICv3 run |

## Byte-identity (knob-off)

`net4` is default-OFF, armed only by `UNAOS_NET4=1`, and NOT stripped by `arm_features` (it is a real aarch64
feature, unlike the x86-only `smolnet`). With it off, the module + both call sites are compiled out **and the
smoltcp dep is not pulled** (declared optional under `net4`), so the default tegra/virt media are
byte-identical to baseline. (Full objcopy per-section verification is an esp-jetson-build step, deferred with
metal — the arc did not build metal media.)

## Metal-pending (attended sitting — NOT this arc's gate)

The full claim → rings → bind sequence, the live MAC, PHY link state, whether RX/TX advances, and the
SMMU-bypass question are all attended-metal. Runbook: `scripts/orin-net4-bench.md` (image
`UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`; stage to `~/unaos-bench/flash/orin/` per the flash-staging
rule). Joins the NET-3 recon accruals for the next Orin sitting.

## Flagged

- **DMA/SMMU** is the one honest pre-metal unknown (above) — the leading metal STOP candidate, documented.
- Static bring-up IP (`192.168.1.2/24`, gw `.1`) is a placeholder so the smoltcp interface can bind
  pre-metal; the link's real subnet is a metal input (revisited then).
- Committed on `hw-jetson` only; **not merged/pushed** (integrator merges, Peter pushes).
