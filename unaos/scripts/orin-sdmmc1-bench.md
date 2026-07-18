# ORIN-SDMMC-1 bench runbook — Tegra234 microSD-slot SDMMC READ-ONLY recon (attended; the installer line's first rung)

ORIN-SDMMC-1 is the **first rung of the installer line** (the Orin is the "mule" whose microSD slot we
ultimately want to write from a booted UnaOS). This rung is the NET-1 house pattern: **read-only census
before any touch.** It resolves the microSD-slot SDMMC controller from the live DTB, brings the SDHCI engine
up to card-identification, reads CID/CSD/capacity, and reads sector 0 to classify its partition signature.
It **writes nothing to the card** — the write path (SDMMC-2) is a separate arc behind its own arm flag.

QEMU models no Tegra234 SDMMC controller, so the whole MMIO/identification path is **attended-metal** — this
sitting. See `arch/aarch64/sdmmc_tegra.rs` (the recon), `drivers/emmc2.rs` (the proven Pi 4 SDHCI model it
mirrors), `arch/aarch64/fdt_tegra.rs` (the DTB walker), and arch_arm64.md §ORIN-SDMMC for the design, the
Tegra vendor-quirk assumptions, and the read-only-by-construction argument.

## The image (one knob)

- **`UNAOS_SDMMC=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`** — the recon image. The `sdmmc` feature is standalone
  (does NOT imply `tegra`/`pcie2`); combined with `UNAOS_TEGRA=1` the controller MMIO is compiled in and the
  recon runs on the metal Orin during `tegra_early_stop`, before the JB2b xHCI work. Knob-off, the module +
  call sites vanish, so the tegra loadable image is byte-identical to baseline; zero `SDMMC` strings
  knob-off.

Stage the built ESP tar to `~/unaos-bench/flash/orin/` per the flash-staging rule (stamp + sha256 +
MANIFEST); flash the staged tar, never a `target/` path. Validate tegra media by `tegra:` count/hash, never
by size.

**Seat a microSD card** in the devkit slot before the sitting — any card (the census reports whatever is
there). An empty slot is a valid run too (it yields the honest "no card seated" line).

## Hard rules for this bench (the read-only boundary is load-bearing)

- **This rung is READ-ONLY to the card. There must be NO card-storage write on the wire.** The recon issues
  only the identification ladder + a CMD17 single-block READ (CMD0/8/55/41/2/3/9/7/16/17). If the serial
  shows **any** announced write to card storage — a `CMD24`/WRITE, an erase, an ACMD6 bus-width write — that
  is a **STOP**: record it and report, do not continue. (The SDHCI controller-register writes the recon does
  make — SRST, clock, power, command-issue — are the machinery every read needs; they are not card writes.)
- **Poison is ABSENT, never present** (the NET-4b law). The `CAPABILITIES` probe read happens **before any
  write**; a poison value (`0xffffffff` / `0xdeadbeef` / `0xa5a5a5a5`) ⇒ the recon **refuses** cleanly (no
  reset, no writes) — an honest result, recorded, not a bug to work around.
- **Any RAS/SError signature is a STOP.** The `mmu_tegra` Part-C / healed `exceptions.rs` vectors capture the
  syndrome (recorded + spin); record it and report.
- **The Tegra clock/pad assumption is the metal unknown.** The recon drives only the standard SDHCI internal
  divider and assumes the firmware/BPMP left the sdmmc1 module clock + pad power up (the bootloader read the
  card). If `M2: internal clock never stabilised … the input clock is gated` appears, that is the honest
  BPMP-clock finding — record it and scope the next arc; do **not** improvise a CAR/BPMP clock write this
  sitting.
- One serial reader only (`lsof` the port; screen(1) at 115200 is the proven rig). The recon is a boot-time
  sequence — capture the serial, no interaction needed.

## What to expect on the wire (grep `SDMMC`)

```
:: SDMMC: ORIN-SDMMC-1 Tegra234 microSD READ-ONLY recon (DTB @0x… size=0x…) ::
:: SDMMC:   M1: candidate /bus@0/mmc@3400000 reg=0x03400000(size 0x10000) status=okay removable cd-gpios compat='nvidia,tegra234-sdhci|' ::
:: SDMMC:   M1: candidate …/mmc@3460000 reg=0x03460000(size 0x10000) status=… non-removable … compat='nvidia,tegra234-sdhci|'   (the on-module eMMC, if present — logged, not picked)
:: SDMMC:   M1: picked /bus@0/mmc@3400000 @ 0x03400000 (size 0x10000) as the microSD slot ::
:: SDMMC:   M1: controller window 0x03400000(+0x10000) is in the GiB-0 device window (already Device-nGnRE) ::
:: SDMMC:   M1: live SDHCI — CAPABILITIES=0x……… (base-clk … MHz, 8-bit=…, ADMA2=…), spec-version reg=0x… (SDHCI 4.0) ::
:: SDMMC:   M2: SRST_ALL (controller software reset) ::
:: SDMMC:   M2: card detected (Present State 0x………) ::
:: SDMMC:   M2: CID manufacturer(MID)=0x… OEM(OID)='..' product(PNM)='.....' rev=0x. serial(PSN)=0x……… date=M/YYYY ::
:: SDMMC:   M2: capacity … blocks (… MiB, CSD v2), addressing block (SDHC/SDXC), v2 (CMD8 ok) ::
:: SDMMC:   M2: identified — RCA 0x…, bus 1-bit, default-speed (<=25 MHz) [4-bit/HS negotiation deferred] ::
:: SDMMC:   M3: sector 0 first 16 bytes = xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx xx ::
:: SDMMC:   M3: sector-0 signature = GPT-protective MBR (…) ::
:: SDMMC: ORIN-SDMMC-1 DONE — microSD censused: … blocks (… MiB, CSD v2), sector-0 … (READ-ONLY; no card write) ::
```

### What to record

- **The CID** — manufacturer (MID), product name (PNM), serial (PSN), manufacture date (the first real
  identity read off the Orin's microSD card).
- **The capacity** (blocks + MiB) and CSD version, and whether it matched the card's labelled size.
- **The sector-0 signature** (GPT-protective / FAT / MBR / unknown) and the first-16-bytes hex.
- **The `CAPABILITIES` value + SDHCI spec version** (proves the register space is live, not open-bus), and
  whether the base-clock field was nonzero or the 200 MHz assumption kicked in.

### The verdict shapes

- **Card censused** (the money shot) — CID + capacity + sector-0 signature printed, `DONE` line reached.
  First Orin microSD identity + content read; the installer line has its target bring-up. Record the CID and
  the signature.
- **No card seated** — `M2: no card seated … census done, nothing to identify`. A valid run (empty slot);
  re-seat a card and re-boot to census one.
- **`M1: CAPABILITIES … = POISON … recon REFUSED`** — the register window is not a live SDHCI through the
  mapped aperture. The read-before-write guard doing its job: an honest clean refusal, no fault. Record the
  candidate/base lines above it and report.
- **`M2: internal clock never stabilised … input clock is gated`** — the BPMP-clock finding; scope the next
  arc (a BPMP module-clock MRQ for sdmmc1). Do not improvise a CAR write.

The box proceeds to CAPSTONE (JM6) exactly as a normal tegra boot — the recon is a prologue. Restore the
boot-stick default at the end of the sitting per the standing rule.
