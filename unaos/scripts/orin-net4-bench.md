# ORIN-NET-4 bench runbook — RTL8168/8111 GbE driver + smoltcp bind (attended; the Orin's first network path)

NET-3's consolidated sitting answered the two recon questions the driver stands on: the device at
controller-0 bus1:dev0:fn0 is a **Realtek RTL8168/8111** (`0x10ec:0x8168`), and the **link was observed UP**
(gen1 x1 — the `LINK-UP-pre-LTSSM` observation, DLL-active before the LTSSM write even landed). NET-4 is the
driver that turns "device identified" into "packets move": claim the device, enable its BAR decode +
bus-master, map BAR2, reset the MAC, bring up the C+ RX/TX descriptor rings, read the station MAC, and bind
a smoltcp `phy::Device` over the rings. QEMU models no Tegra234 RC, so the whole MMIO/DMA + smoltcp layer is
**attended-metal** — this sitting.

**NET-4b (fix-forward) is folded in.** The first NET-4 sitting FAULTED at the first BAR register write:
BAR2 read back `0x4000_4000` — a **PCIe bus address** — and the driver deref'd it as a CPU physical address,
hitting a Tegra carveout (RAS Uncorrectable, `a5a5a5a5` poison, DC-cut recovery). NET-4b programs an
**outbound iATU region** from controller-0's DT `ranges` and drives the **CPU-side aperture** address
(`cpu_base + (bar_pci − pci_base)`), never the raw BAR value, and gates the first write behind a
**poison-honest TCR readback** through the new window. See arch_arm64.md §ORIN-NET-4b for the adjudication
and the ATU design.

See `arch/aarch64/rtl8168_tegra.rs` (the driver + smoltcp adapter), `arch/aarch64/pcie_probe.rs` (the NET-3
`census2`/`net3_*` recon this runs after), `arch/aarch64/mmu_tegra.rs` (`map_mmio_window` + the PS widen),
and arch_arm64.md §ORIN-NET-4 for the design, the programming model, the DMA identity-map invariant, and the
SMMU metal risk.

## The image (one knob)

- **`UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`** — the driver image. `net4` implies `pcie3` (which
  implies `pcie2`): the NET-3 `census2` runs first (PS widen + LTSSM enable + ECAM enumeration), then
  NET-4's `net4_bringup` claims the device it found. The driver's MMIO/DMA + the smoltcp adapter are
  additionally `tegra`-gated. Knob-off, the module + call sites vanish **and the smoltcp dep is not pulled**,
  so the tegra loadable image is byte-identical to baseline; zero `PCIE4` strings knob-off.

Stage the built ESP tar to `~/unaos-bench/flash/orin/` per the flash-staging rule (stamp + sha256 +
MANIFEST); flash the staged tar, never a `target/` path. Validate tegra media by `tegra:` count/hash, never
by size.

## Hard rules for this bench (the write-scope boundary is load-bearing)

- **The driver DOES the writes NET-3 refused — but only these, each logged before issue.** The `COMMAND`
  register MEM-space + bus-master decode-enable (one config write to bus1:dev0:fn0), then the RTL8168
  control/ring register program (`CFG9346` unlock/lock, `RDSAR`/`TNPDS` ring bases, `RMS`/`MTPS`/`TCR`/`RCR`,
  `CR` RxEnb|TxEnb) and the soft reset. Every register write prints a `>>> REG WRITE (Mx): …` or
  `>>> CONFIG WRITE (M1): …` line first. If the serial shows a write outside this set — a write to any
  **other** controller or config function, an MSI/MSI-X setup, a PERST/PHY reprogram, a BPMP power/clock
  MRQ — that is a **STOP**: record it and report, do not improvise.
- **Poison is ABSENT, never present** (PI-V3D-1). The device-identity read and the `TCR` init readback reject
  `0xffffffff` / `0xdeadbeef`. A poison identity ⇒ the claim skips (link down / no device); a poison `TCR`
  readback ⇒ the ring bring-up fails by design (the controller stopped answering) — both are honest results,
  recorded, not bugs to work around.
- **Any RAS/SError signature is a STOP.** A BAR/config/register write to a device the firmware quiesced could
  fault; the `mmu_tegra` Part-C / healed `exceptions.rs` vectors capture the syndrome (recorded + spin).
- **The DMA identity-map + SMMU question is THE metal unknown.** The rings are programmed with
  identity-physical addresses (`mmu_tegra` maps RAM VA==PA). If RX/TX never advances (descriptors stay
  NIC-owned / OWN never clears on TX), the likely cause is the **SMMU translating controller-0's stream ID**
  so the NIC's DMA does not reach our identity-physical rings — record it as the SMMU-bypass finding (the
  next arc programs an SMMU identity/bypass stream mapping). Do **not** improvise SMMU writes this sitting.
- One serial reader only (`lsof` the port; screen(1) at 115200 is the proven rig). The bring-up is a
  boot-time sequence — capture the serial, no interaction needed.

## What to expect on the wire (grep `PCIE4` — the NET-4 sub-block; the NET-2/3 recon carries `PCIE2`/`PCIE3`)

The NET-3 `census2` preamble runs first (PS-widen, LTSSM enable, ECAM enumeration + BAR sizing). NET-4's own
lines then take over:

```
:: PCIE4: ORIN-NET-4 RTL8168/8111 GbE bring-up (DTB @0x… size=0x…) ::
:: PCIE4:   ecam 0x2e20000000 mapped Device-nGnRE (via the PS-widened regime) ::
:: PCIE4:   bus1:dev0:fn0 vendor=0x10ec device=0x8168 ::
:: PCIE4:   >>> CONFIG WRITE (M1): COMMAND[0x4] 0x……… -> 0x……… (set MEM-space + bus-master) — issuing ::
:: PCIE4:   register BAR2 = 0x40004000 (32-bit mem) — this is a PCIe BUS address (needs outbound iATU translation) ::
:: PCIE4:   M1-fix: BAR2 0x40004000 in ranges MEM window PCIe [0x40000000..0x50000000) -> CPU base 0x32… ; ATU base 0x2a04… ::
:: PCIE4:   M1-fix: outbound iATU region 0 @ 0x2a04… — CPU [0x32…..0x32…] -> PCIe 0x40000000 (type MEM) ::
:: PCIE4:   >>> ATU WRITE (M1-fix): BASE lo/hi = 0x…/0x… ::
:: PCIE4:   >>> ATU WRITE (M1-fix): LIMIT lo/hi = 0x…/0x… ::
:: PCIE4:   >>> ATU WRITE (M1-fix): TARGET lo/hi = 0x40000000/0x00000000 ::
:: PCIE4:   >>> ATU WRITE (M1-fix): REGION_CTRL1 = TYPE_MEM ::
:: PCIE4:   >>> ATU WRITE (M1-fix): REGION_CTRL2 = ENABLE|INCREASE_REGION_SIZE — arming region ::
:: PCIE4:   BAR2 CPU aperture 0x32… (+0x1000) mapped Device-nGnRE — registers reachable via iATU ::
:: PCIE4:   M1-fix readback: TCR = 0x……… (live, non-poison) — register window confirmed; first write is now safe ::
:: PCIE4:   >>> REG WRITE (M1): CR[0x37] |= RST (soft reset) — issuing ::
:: PCIE4:   CR.RST cleared after N spins — reset complete ::
:: PCIE4:   station MAC = xx:xx:xx:xx:xx:xx ::
:: PCIE4:   M2 ring bring-up (C+ mode; RTL8168 programming-guide order) ::
:: PCIE4:   >>> REG WRITE (M2): CFG9346[0x50] = 0xc0 (unlock config) ::
:: PCIE4:   CPlusCmd[0xe0] = 0x…… (C+ engine) ::
:: PCIE4:   >>> REG WRITE (M2): RDSAR[0xe4] = 0x……… (RX ring, 32 desc) ::
:: PCIE4:   >>> REG WRITE (M2): TNPDS[0x20] = 0x……… (TX ring, 8 desc) ::
:: PCIE4:   >>> REG WRITE (M2): RMS[0xda] = 0x0800; MTPS[0xec] = 0x3b ::
:: PCIE4:   >>> REG WRITE (M2): TCR[0x40] = 0x……… ::
:: PCIE4:   >>> REG WRITE (M2): CR[0x37] = 0x0c (RxEnb | TxEnb) ::
:: PCIE4:   >>> REG WRITE (M2): RCR[0x44] = 0x……… (promiscuous bring-up) ::
:: PCIE4:   rings up: RX @ 0x……… (32 desc) TX @ 0x……… (8 desc); TCR readback 0x……… (live) ::
:: PCIE4:   RTL8168 @ BAR2 PCIe 0x40004000 (CPU aperture 0x32…), MAC read, C+ rings up + RX/TX enabled; PHY link UP/DOWN ::
:: PCIE4:   smoltcp 0.13 Interface BOUND over RTL8168: MAC set, 192.168.1.2/24 + default gw 192.168.1.1, medium=ethernet, polled OK; link … — live ICMP/ARP is attended-metal ::
:: PCIE4: ORIN-NET-4 DONE — RTL8168 driver up + smoltcp bound (live traffic = attended metal) ::
```

### What to record

- **The station MAC** (the first real MAC read off the Orin's NIC).
- **`PHY link UP/DOWN`** and the `TCR readback` value (proves the register space is live, not open-bus).
- **Whether the smoltcp poll advances the rings** — on a live link with a peer, `rx_count`/`tx_count` should
  move; the honest pre-subnet state is an empty ring (no traffic). If a peer is on the link, the `smoltcp
  Interface BOUND` line is followed by ARP for the gateway on the wire.
- **Any `[tx] descriptor N never completed` or a stuck RX ring** — the SMMU-translation candidate (see the
  hard rules); record and stop, do not improvise SMMU writes.

### The verdict shapes

- **Driver up, rings live, no traffic** (likely, pre-subnet-config) — "RTL8168 claimed + MAC read + rings
  enabled + smoltcp bound; link `X`; no peer/subnet yet." The next step is the link's real subnet + a live
  ARP/ICMP peer.
- **Driver up, RX/TX advances** (the money shot) — "packets move on the Orin." Record `rx_count`/`tx_count`
  and any ARP reply. First Orin network I/O.
- **Rings never advance despite a live link** — the **SMMU-bypass finding**; scope the next arc (program an
  SMMU identity/bypass stream mapping for controller-0).
- **`M1-fix readback: TCR … = POISON … bring-up REFUSED before any write`** — the register window is not
  live through the iATU (mistranslation, link down, or the device quiesced). This is the guard doing its
  job: an **honest clean refusal, no fault**, not a bug to work around. Record the `ranges`/ATU/CPU-aperture
  values printed just above it (they scope whether the `ranges` resolution or the iATU program is at fault)
  and report — do **not** improvise a raw-BAR fallback (that is the retired FAULT-AT-M1 path).

The box proceeds to CAPSTONE (JM6) exactly as a normal tegra boot — the bring-up is a prologue. Restore the
boot-stick default at the end of the sitting per the standing rule.
