# ORIN-SDMMC bench runbook — Tegra234 microSD READ-ONLY recon + the ARMED write ladder + the INSTALL leg (attended)

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

## Install leg (ORIN-INSTALL-2) — the THIRD, destructive gate, now a real self-clone

**⚠ This leg REPARTITIONS AND REFORMATS the entire seated microSD. It is destructive by design — it clones the
running UnaOS boot payload onto the card.** It runs only behind a THIRD gate on top of the armed ladder, and
only on a card you are willing to erase. Do **not** run it on a card whose contents matter.

- **Image:** `UNAOS_INSTALL_TARGET_SD=1 UNAOS_TEGRA=1 ./arroyo esp-jetson` (the knob pulls `install_target`
  ⇒ `sdmmc_arm` ⇒ `sdmmc`). A plain `UNAOS_SDMMC_ARM=1` image (the armed-ladder image above) does **not**
  contain the installer — string-identity: zero `ORIN-INSTALL` strings in it, and it rebuilds byte-for-byte.
- **The three-gate ladder:** gate 1 = `sdmmc` census OK; gate 2 = `sdmmc_arm` write path; gate 3 =
  `install_target` destructive-confirm.
- **The install is DEFERRED (INSTALL-2).** The read-only census runs pre-JB2b as before but only **stashes** the
  card identity; the destructive install runs **after the JB2b USB pump**, once the boot stick has enumerated as
  a block device — the position where the running boot payload is readable so the installer can clone *the real
  files*, not a synthetic marker. So the install lines appear on the wire AFTER the JB2b keyboard/storage lines,
  not right after the census.
- **Before the first write it ANNOUNCES what it will destroy** — the sector-0 classification, capacity, and the
  card CID identity. Read that line and confirm it is the card you intend to erase before letting the boot
  proceed.

**Expected on the wire (grep `INSTALL`), on a card the install proceeds on** (the `EFI/BOOT/BOOTAA64.EFI` +
`kernel.elf` boot tree is illustrative — the flow clones whatever the stick's ESP actually carries):

```
:: SDMMC: ORIN-INSTALL-2 card identity stashed; destructive install DEFERRED to the post-JB2b USB-enumerated site (self-clone needs the boot stick readable) ::
   … (JB2b keyboard/storage enumeration lines) …
:: SDMMC: ORIN-INSTALL-2 THIRD GATE (UNAOS_INSTALL_TARGET_SD=1) — cloning the running boot payload onto the SEATED microSD ::
:: SDMMC:   gates: [1] sdmmc census OK · [2] sdmmc_arm write path armed · [3] install_target destructive-confirm — all satisfied ::
:: SDMMC:   ABOUT TO DESTROY: microSD sector-0 = <class> · capacity <N> blocks (<M> MiB) — the entire card is about to be repartitioned ::
:: SDMMC:   M2: CID manufacturer(MID)=… product(PNM)='…' serial(PSN)=… date=…/… ::
:: SDMMC:   INSTALL: UNAFS SIZING-GATE-1 (pre-GPT whole-card upper bound) => OK — cap 131072 blk (512 MiB), planned 131072 blk, refmap 1048576 B = 2.0% of the 50331648 B heap (limit 25% = 12582900 B) ::
:: SDMMC:   INSTALL: GPT written + parse-back verified — ESP LBA 2048..… , data LBA …..… of <N> sectors ::
:: SDMMC:   INSTALL: zeroed <K> ESP metadata sectors (reserved + both FATs) to re-establish the blank-precondition ::
:: SDMMC:   INSTALL: ESP formatted FAT32 — fat_sz=…sec clusters=… data@vol+… ::
:: SDMMC:   INSTALL: UNAFS SIZING-GATE-2 (p2 UNAOS-DATA span) => OK — cap 131072 blk (512 MiB), planned 131072 blk, refmap 1048576 B = 2.0% of the 50331648 B heap (limit 25% = 12582900 B) ::
:: SDMMC:   INSTALL: UNAFS formatting p2 UNAOS-DATA — LBA 133120..62333918 = 62200799 sectors = 7775099 blk; volume CAPPED to 131072 blk (512 MiB), refmap 1048576 B ::
:: SDMMC:   INSTALL: UNAFS p2 volume MOUNTED BACK off the card — v5 magic ok, 131072 blk (512 MiB) at LBA 133120, refmap 1048576 B (2.0% of the 50331648 B heap) rebuilt from 128 leaf blk + 1 index blk (single-level), root gen 1, 130933 free blk, root dir lists 0 entries => UNAFS-VERIFIED ::
:: SDMMC:   INSTALL: mounted USB boot stick — <fs describe> ::
:: SDMMC:   INSTALL: cloned <F> files (<C> data clusters) from the boot tree ::
:: SDMMC:   INSTALL: kernel.elf (<B> B, <E> extents) sha256=<64-hex> => VERIFIED ::
:: SDMMC:   INSTALL: EFI/BOOT/BOOTAA64.EFI (<B> B, <E> extents) sha256=<64-hex> => VERIFIED ::
:: SDMMC:   INSTALL: all <F> cloned files re-read off the card + sha-verified => PASS ::
:: SDMMC: ORIN-INSTALL-2 SD install — gpt+zero+fat32+clone(<F> files)+unafs verify => PASS — p1 UNAOS-ESP = FAT32 (<F> files sha-verified) · p2 UNAOS-DATA = UnaFS v5 (131072 blk / 512 MiB, mounted back off the card) ::
```

The final `=> PASS` line is the only success; any engine/read error is a single `ORIN-INSTALL-2 SD install =>
FAIL (<Reason>)` line naming the cause (GPT/format/read/clone/verify each fail closed, no partial write left
unaccounted). A **verify-FAIL** shape is `INSTALL: <path> extent sha-verify => FAIL` immediately followed by the
`… SD install => FAIL (VerifyFailed)` verdict — the card's content did not match what was read off the stick.
Honest **SKIP** shapes (no destruction): `ORIN-INSTALL-2 deferred install SKIPPED — no card identity stashed`
(census found no controller/card) or `… SKIPPED — the USB boot stick did not enumerate as a block device (no
self to clone)`.

**Note (payload):** INSTALL-2 clones the RUNNING system's real boot payload read off the USB stick's ESP
(`fs::fat::mount()` over the JB2b-enumerated block device), each file sha-extent-verified on the card. This
replaces INSTALL-1's generated `UNAOS.IMG` marker. Verify-by-content is the per-file SD extent sha-verify.

**Note (p2 native volume — TEGRA-UNAFS-FMT):** the flow no longer leaves `UNAOS-DATA` raw. Partition 2 is
**formatted as a native UnaFS volume and then MOUNTED back off the card** before the verdict, so the numbers
above are all measured, not derived from what was held in RAM. Three things to read at the bench:

- The two **`SIZING-GATE-n`** lines are the heap bound, and each carries its own token so they are never
  confused: gate 1 runs on the whole-card capacity **before the first byte of the GPT is written** (a refusal
  there leaves the card exactly as it was found), gate 2 runs on the real p2 span. A refusal reads
  `UNAFS SIZING-GATE-n (…) => REFUSED — … NOTHING was written to the card ::`. Note what these gates do NOT
  prove: they compare against the heap's **total** `HEAP_SIZE`, never its free bytes, so a format can still
  fail on heap pressure later — that failure is fail-closed and named where it lands.
- The volume is **capped at 512 MiB (131,072 blocks)** inside the much larger partition, so `CAPPED to` is the
  expected word on the bench card. On a card whose data partition is smaller than the cap the same line reads
  `volume UNCAPPED at the full <N> blk` instead — both are normal. The cap keeps the volume single-level
  (128 refmap leaves) and keeps the every-boot rung-4 probe-mount at ~1,000 polled CMD17 reads rather than
  ~8,200.
- **`=> UNAFS-VERIFIED ::`** is a distinct terminator from the per-file `=> VERIFIED ::` above it — grep for
  `UNAFS-VERIFIED` to address the volume proof alone. `root gen 1` is the format commit; `root dir lists 0
  entries` is correct for a freshly formatted volume (nothing has written to it yet).

**Honest p2 SKIP shape** (not a failure — the install continues and still ends `=> PASS`):
`INSTALL: UNAFS p2 SKIPPED — the GPT layout's UNAOS-DATA span is <S> sectors = <B> blk, under the 12 blk floor
a UnaFS format needs; p2 left raw, the install continues ::`. That line is reachable on a card of ~65 MiB,
where the GPT's 1 MiB alignment can leave the data partition a single sector; the verdict then ends
`· p2 UNAOS-DATA = NO NATIVE VOLUME (span absent or below the format floor) ::`.

**Virt witness** (no metal — `UNAOS_INSTALL_TARGET_SD=1 UNAOS_GICV3=1 ./arroyo test-arm`), after the two SDMMC
witness lines:

```
:: SDMMC: ORIN-INSTALL-2 third gate (UNAOS_INSTALL_TARGET_SD=1) compiled-present but metal-only — no Tegra234 SDMMC on this build (QEMU virt); no install here ::
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
- **(Install leg)** the ABOUT-TO-DESTROY line (class + capacity + CID), the ESP/data LBAs, the zeroed-metadata
  sector count, the mounted-stick describe line, the per-file `sha256=… VERIFIED` manifest, and whether the flow
  reached `ORIN-INSTALL-2 SD install … => PASS` or a named `FAIL`/`SKIP`. After a PASS, re-seat the card and
  confirm a host reader sees a `UNAOS` FAT32 ESP carrying the cloned boot tree (`/EFI/BOOT/BOOTAA64.EFI` +
  `/kernel.elf`) — and, ideally, that each file's host-side sha256 matches the serial manifest.

The box proceeds to CAPSTONE (JM6) exactly as a normal tegra boot — the recon/ladder is a prologue. Restore
the boot-stick default at the end of the sitting per the standing rule.
