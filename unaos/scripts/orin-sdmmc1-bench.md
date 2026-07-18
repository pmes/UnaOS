# ORIN-SDMMC bench runbook — Tegra234 microSD READ-ONLY recon + the ARMED write ladder (attended)

The installer line makes the Orin the "mule" whose microSD slot we write from a booted UnaOS. This runbook
covers **two rungs against the same driver** (`arch/aarch64/sdmmc_tegra.rs`):

- **UNARMED (ORIN-SDMMC-1, `UNAOS_SDMMC=1`)** — the NET-1 house pattern: **read-only census before any
  touch.** Resolve the microSD-slot SDMMC controller from the live DTB, bring the SDHCI engine up to
  card-identification, read CID/CSD/capacity, read sector 0, classify its partition signature. **Writes
  nothing to the card.**
- **ARMED (ORIN-SDMMC-2, `UNAOS_SDMMC=1 UNAOS_SDMMC_ARM=1`)** — the write path behind a **paranoia ladder**:
  re-census → pick a provably-safe scratch region → stash it → write a stamped pattern → verify → restore →
  verify. The only card writes UnaOS makes, and only to a stashed-then-restored scratch block.

QEMU models no Tegra234 SDMMC controller, so the whole MMIO/identification/write path is **attended-metal** —
this sitting. See `drivers/emmc2.rs` (the proven Pi 4 SDHCI model it mirrors), `arch/aarch64/fdt_tegra.rs`
(the DTB walker), and arch_arm64.md §ORIN-SDMMC for the design, the Tegra vendor-quirk assumptions, the
read-only-by-construction argument (unarmed), and the double-gating + scratch-region rule (armed).

## The two images (double-gated)

- **`UNAOS_SDMMC=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`** — the **unarmed recon** image. Read-only census; the
  card is untouched.
- **`UNAOS_SDMMC=1 UNAOS_SDMMC_ARM=1 UNAOS_TEGRA=1 ./arroyo esp-jetson`** — the **armed ladder** image.
  Writes require BOTH `sdmmc` AND the separate `sdmmc_arm` arm; the recon knob alone never writes the card.

`UNAOS_SDMMC=1` **without** `UNAOS_SDMMC_ARM=1` is byte-identical in behavior to the merged ORIN-SDMMC-1
recon (zero SDMMC-2 / `ladder` strings in the unarmed kernel; the write path is entirely `sdmmc_arm`-gated).
Knob-off entirely, the module + call sites vanish and the tegra image is byte-identical to baseline.

Stage the built ESP tar to `~/unaos-bench/flash/orin/` per the flash-staging rule (stamp + sha256 +
MANIFEST); flash the staged tar, never a `target/` path. Validate tegra media by `tegra:` count/hash, never
by size.

**Seat a microSD card** in the devkit slot before the sitting — any card (the census reports whatever is
there). An empty slot is a valid unarmed run too (it yields the honest "no card seated" line).

## Hard rules for this bench

### Unarmed run (READ-ONLY boundary is load-bearing)

- **The unarmed rung is READ-ONLY to the card. There must be NO card-storage write on the wire.** The recon
  issues only the identification ladder + a CMD17 single-block READ (CMD0/8/55/41/2/3/9/7/16/17). Any
  announced write to card storage on serial — a `CMD24`/WRITE, an erase, an ACMD6 bus-width write — under the
  UNARMED image is a **STOP**: record it and report. (The SDHCI controller-register writes the recon makes —
  SRST, clock, power, command-issue — are the machinery every read needs; they are not card writes.)
- **Poison is ABSENT, never present** (the NET-4b law). The `CAPABILITIES` probe read happens **before any
  write**; a poison value (`0xffffffff` / `0xdeadbeef` / `0xa5a5a5a5`) ⇒ the recon **refuses** cleanly (no
  reset, no writes) — an honest result, recorded, not a bug to work around.

### Armed run (the seated card is sacred)

- **The armed ladder writes ONLY a scratch region it stashed first and restores after.** The scratch region
  is the card's **last block** (LBA `capacity-1`), and only when sector 0 shows **no GPT**. A `write ladder
  REFUSED (GPT present …)` line is the CORRECT, expected result on a GPT card (a GPT backup header lives in
  the last LBA) — not a failure; record it and re-seat a non-GPT card if you want to exercise the write.
- **A `PASS` line is the only success.** `:: SDMMC: write ladder — write/verify/restore/verify => PASS ::`
  means all seven steps verified AND the scratch block was restored byte-identical. Anything else is a
  `ladder FAIL step N (…)` line — record it verbatim.
- **A restore failure dumps the stash as hex.** If step 6/7 (restore write / restore verify) fails, or a
  mid-ladder emergency restore fails, the driver prints the stashed original as 32 `stash[0xNN]:` hex rows so
  the data is never silently lost. Capture those rows — they are the recovery copy.

### Both runs

- **Any RAS/SError signature is a STOP.** The `mmu_tegra` Part-C / healed `exceptions.rs` vectors capture the
  syndrome (recorded + spin); record it and report.
- **The Tegra clock/pad assumption is the metal unknown.** The driver drives only the standard SDHCI internal
  divider and assumes the firmware/BPMP left the sdmmc1 module clock + pad power up (the bootloader read the
  card). If `internal clock never stabilised … the input clock is gated` appears, that is the honest
  BPMP-clock finding — record it and scope the next arc; do **not** improvise a CAR/BPMP clock write.
- One serial reader only (`lsof` the port; screen(1) at 115200 is the proven rig). The recon/ladder is a
  boot-time sequence — capture the serial, no interaction needed.

## What to expect on the wire (grep `SDMMC`)

### Unarmed recon (ORIN-SDMMC-1)

```
:: SDMMC: ORIN-SDMMC-1 Tegra234 microSD READ-ONLY recon (DTB @0x… size=0x…) ::
:: SDMMC:   M1: candidate /bus@0/mmc@3400000 reg=0x03400000(size 0x10000) status=okay removable cd-gpios compat='nvidia,tegra234-sdhci|' ::
:: SDMMC:   M1: picked /bus@0/mmc@3400000 @ 0x03400000 (size 0x10000) as the microSD slot ::
:: SDMMC:   M1: live SDHCI — CAPABILITIES=0x……… (base-clk … MHz, 8-bit=…, ADMA2=…), spec-version reg=0x… (SDHCI 4.0) ::
:: SDMMC:   M2: card detected (Present State 0x………) ::
:: SDMMC:   M2: CID manufacturer(MID)=0x… OEM(OID)='..' product(PNM)='.....' rev=0x. serial(PSN)=0x……… date=M/YYYY ::
:: SDMMC:   M2: capacity … blocks (… MiB, CSD v2), addressing block (SDHC/SDXC), v2 (CMD8 ok) ::
:: SDMMC:   M3: sector 0 first 16 bytes = xx xx xx … ::
:: SDMMC:   M3: sector-0 signature = GPT-protective MBR (…) ::
:: SDMMC: ORIN-SDMMC-1 DONE — microSD censused: … blocks (… MiB, CSD v2), sector-0 … (READ-ONLY; no card write) ::
```

### Armed ladder (ORIN-SDMMC-2) — the recon lines above, THEN:

On a **non-GPT** card (the write actually runs):

```
:: SDMMC: ORIN-SDMMC-2 ARMED (UNAOS_SDMMC_ARM=1) — paranoia write ladder on the SEATED card (scratch region, stashed + restored) ::
:: SDMMC:   ladder step 1/7: re-reading sector 0 (rung-1 read census) before any write ::
:: SDMMC:   ladder step 1: read census stable (sector 0 re-read byte-identical) ::
:: SDMMC:   ladder step 2/7: no GPT (sector 0 = …) — scratch region = the last 1 block(s), LBA … (card's last LBA) ::
:: SDMMC:   ladder step 3/7: reading + stashing scratch LBA … current contents ::
:: SDMMC:   ladder step 3: stashed 512 bytes from LBA … (first 8: xx xx …) ::
:: SDMMC:   ladder step 4/7: CMD24 single-block WRITE of stamped pattern to LBA … ::
:: SDMMC:   ladder step 5/7: reading back LBA … + byte-comparing to the stamped pattern ::
:: SDMMC:   ladder step 5: write verified (read-back byte-identical to the stamped pattern) ::
:: SDMMC:   ladder step 6/7: RESTORING original stashed contents to LBA … ::
:: SDMMC:   ladder step 7/7: reading back LBA … + byte-comparing to the stash (restore verify) ::
:: SDMMC:   ladder step 7: restore verified (LBA … byte-identical to the original stash) ::
:: SDMMC: write ladder — write/verify/restore/verify => PASS ::
```

On a **GPT** card (the expected refusal — no write):

```
:: SDMMC:   ladder step 2/7: sector 0 is GPT-protective MBR (…) — a GPT BACKUP header lives in the card's LAST LBA, exactly where our scratch region sits; REFUSING all scratch writes this arc (no provably-safe region) ::
:: SDMMC: ORIN-SDMMC-2 write ladder REFUSED (GPT present — the seated card is sacred; no write) ::
```

The virt witness (no metal, for reference — `UNAOS_SDMMC=1 UNAOS_SDMMC_ARM=1 UNAOS_GICV3=1 ./arroyo test-arm`):

```
:: SDMMC: ORIN-SDMMC-1 Tegra234 microSD recon compiled; no Tegra234 SDMMC on this build (QEMU virt) — recon is metal-only (UNAOS_SDMMC=1 UNAOS_TEGRA=1) ::
:: SDMMC: ORIN-SDMMC-2 write ladder ARMED (UNAOS_SDMMC_ARM=1) but metal-only — no Tegra234 SDMMC on this build (QEMU virt); zero card writes here ::
```

### The failure shapes (armed) — each a distinct honest line, record verbatim

- `ladder FAIL step 1 (re-census): …` — sector 0 unreadable or changed since the census; REFUSES to write.
- `ladder FAIL step 3 (stash read): …` — scratch LBA unreadable; REFUSES to write (nothing to restore from).
- `ladder FAIL step 4 (write): …` — CMD24 failed; an emergency restore of the stash runs.
- `ladder FAIL step 5 (verify …): read-back != written pattern …` — the write did not land as issued;
  emergency restore runs.
- `ladder FAIL step 6 (restore write): … original …-byte data below as hex …` — the RESTORE itself failed;
  the `stash[0xNN]:` hex rows follow (the recovery copy).
- `ladder FAIL step 7 (restore verify …): … original data below as hex` — restore read-back mismatched; hex
  rows follow.
- `ladder: EMERGENCY RESTORE FAILED — original …-byte stash below as hex …` — a post-fault restore could not
  be verified; the hex rows are the last copy of the original data.

### What to record

- **The CID** — manufacturer (MID), product name (PNM), serial (PSN), manufacture date.
- **The capacity** (blocks + MiB) and CSD version, whether it matched the labelled size.
- **The sector-0 signature** and first-16-bytes hex.
- **The `CAPABILITIES` value + SDHCI spec version**, and whether the base-clock field was nonzero or the
  200 MHz assumption kicked in.
- **(Armed)** the scratch LBA, whether the ladder reached `PASS` or `REFUSED (GPT)`, and any `FAIL`/hex rows.

The box proceeds to CAPSTONE (JM6) exactly as a normal tegra boot — the recon/ladder is a prologue. Restore
the boot-stick default at the end of the sitting per the standing rule.
