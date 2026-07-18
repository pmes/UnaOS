# INSTALL-PI — landing report

**Arc:** INSTALL-PI — the installer engine's first LIVE end-to-end install, on the Pi 4 emmc2 microSD
target, provable in QEMU `raspi4b`.
**Branch:** `us-installpi` (worktree `../UnaOS-installpi`). **Date:** 2026-07-18. **Model:** Claude Opus 4.8.

## What landed

The installer engine's **first full end-to-end execution that needs no bench.** QEMU's `raspi4b` models the
BCM2711 SD controller and attaches a real emulated card image, so the in-tree `drivers::emmc2` read/write path
is genuinely exercised and the whole engine flow (GPT → FAT32 → payload copy → sha extent-verify) runs and
PASSes under CI-grade QEMU — unlike ORIN-INSTALL-1, which is metal-only (QEMU models no Tegra234 SDMMC).

- **M1 — `EmmcInstallTarget`** (`crates/kernel/src/install/pi.rs`): an `InstallTarget` over `drivers::emmc2`,
  `read_sectors` / `write_sectors` looping the proven single-block `read_block_512` / `write_block_512`
  (CMD17/CMD24) primitives (bounded multi-sector loop, exactly as the Orin `SdInstallTarget`). The engine
  (`write_gpt` / `format_esp` / `write_payload_file` / `verify_extents`) runs verbatim — no engine change.
- **M2 — the flow call**, on the Pi BSP boot path (`main.rs`, immediately after `emmc2::probe`, where the
  card is up). Three-gate escalation mirroring `sdmmc / sdmmc_arm / install_target`:
  - Gate 1 `piinstall` — census (read-only) + announce identity/capacity/sector-0 class.
  - Gate 2 `piinstall_arm` — non-destructive scratch write/verify/restore ladder on the card's LAST block
    (stashed + restored, verified; refused if a GPT is present).
  - Gate 3 `piinstall_confirm` — the destructive-confirm gate: ABOUT-TO-DESTROY line, then
    GPT → zero-ESP-metadata → FAT32 → payload → sha extent-verify.
- **M3 — the QEMU-live witness**: `./arroyo kernel8-install [secs]` (function `install_pi`) arms all three
  gates, generates a **dedicated BLANK 128 MiB scratch image** (never the `kernel8-test` battery fixture),
  boots `raspi4b` with it in the single SD slot, captures the in-kernel PASS, then **re-reads the scratch
  image on the host** and checks the GPT + FAT32 structures from outside the kernel.
- **M4 — docs**: `installer_engine.md §INSTALL-PI`, `arch_arm64.md ## INSTALL-PI`, this landing report.

## The scratch-image mechanism

`raspi4b` has one SD slot, so the install-witness run is its own QEMU invocation. `install_pi` `dd`s a blank
all-zero `target/pi-install-scratch.img` (128 MiB, `UNAOS_PIINSTALL_MIB` overridable) and attaches it via
`-drive if=sd` — separate from the `kernel8-test` battery, which attaches the fixture flashable image. The
kernel boots via QEMU `-kernel` (not from the card), so the card is a pure throwaway; the emmc2 driver's
legacy-Arasan fallback leg (the QEMU card path) drives it.

## Payload adjudication (M2)

A Pi "install" payload is ultimately the boot volume's FAT files (kernel8.img / start4.elf / config.txt — what
the GPU ROM loads). At the pre-shell BSP call site those are not reachable as a readable clone source, so v1
writes a generated `UNAOS.IMG` marker (honest-and-sufficient for the QEMU witness). **Metal follow-up:** the
self-clone of the boot FAT files (the Pi analogue of INSTALL-2). On real hardware the seated card IS the
running system's card — the three gates + about-to-destroy line are that guard; a metal install leg wants a
dedicated erasable card, never the boot card.

## Live witness + host verification (verbatim)

```
:: PIINSTALL: INSTALL-PI — installer engine over the Pi emmc2 microSD ::
:: PIINSTALL: Gate 1 census — target = Pi emmc2 microSD (262144 x 512B sectors), capacity 262144 blocks (128 MiB), sector-0 = unknown (no recognised signature) ::
:: PIINSTALL: Gate 2 ARMED (UNAOS_PIINSTALL_ARM) — non-destructive scratch write/verify/restore ladder on the LAST block ::
:: PIINSTALL: Gate 2 scratch ladder — write/verify/restore/verify at LBA 262143 => PASS ::
:: PIINSTALL: Gate 3 THIRD GATE (UNAOS_PIINSTALL_CONFIRM) — installing UnaOS onto the SEATED Pi microSD ::
:: PIINSTALL:   gates: [1] emmc2 census OK · [2] write path armed · [3] destructive-confirm — all satisfied ::
:: PIINSTALL: ABOUT TO DESTROY: microSD sector-0 = unknown (no recognised signature) · capacity 262144 blocks (128 MiB) — the entire card is about to be repartitioned ::
:: PIINSTALL:   GPT written + parse-back verified — ESP LBA 2048..133119, data LBA 133120..262110 of 262144 sectors ::
:: PIINSTALL:   zeroed 2064 ESP metadata sectors (reserved + both FATs) to re-establish the blank-precondition ::
:: PIINSTALL:   ESP formatted FAT32 — fat_sz=1016sec clusters=129008 data@vol+2064 ::
:: PIINSTALL:   copied UNAOS.IMG (4096 bytes, 8 extents) ::
:: PIINSTALL:   extent sha-verify (re-read every written extent off the card) => PASS ::
:: INSTALL: pi emmc2 gpt+fat32+copy verify => PASS ::
✅ in-kernel: install PASS line present
── INSTALL-PI host-side verification (target/pi-install-scratch.img) ──
  PASS protective MBR 0x55AA signature
  PASS protective MBR 0xEE GPT partition type @450
  PASS primary GPT header "EFI PART" @LBA1
  PASS GPT header revision 1.0
  PASS FAT32 boot sector "FAT32" fs-type @ESP+82
  PASS FAT32 volume label "UNAOS" @ESP+71
  PASS FAT32 boot sector 0x55AA @ESP+510
HOST-VERIFY: PASS
```

## DONE gate

- `./arroyo check` both arches — GREEN (default, no piinstall).
- Knob matrix (`piinstall` / `piinstall_arm` / `piinstall_confirm`) — all compile clean on aarch64.
- Knob-off `./arroyo kernel8-test 35` — **0 FAIL**, `CAPSTONE COMPLETE — all 6 sync primitives verified`.
  Functional identity: `piinstall*` default OFF ⇒ the module + call site + engine all compile out.
- M3 live witness — in-kernel PASS + host-side `HOST-VERIFY: PASS` (above).
- `./arroyo test-arm 22` (exit 0) + `UNAOS_GICV3=1 ./arroyo test-arm 40` — **0 FAIL**, CAPSTONE 6/6.
- `./arroyo test 22` (x86) — **0 FAIL**.
- `UNAOS_INSTALLDEMO=1 ./arroyo test 22` — engine witness `:: INSTALL: gpt+fat32+copy verify => PASS ::`
  (negative test + blank-check guards all green) — unregressed.

## Lane / flags

New Pi install glue (`install/pi.rs`), `install/` additive only, `lib.rs` + `main.rs` additive cfg-gated
lines, arroyo run-mode wiring + `UNAOS_PIINSTALL` knob family, named docs. The emmc2 driver's read/write
primitives, the battery fixtures, piusb, v3d, and sched are untouched. Nothing flagged.
