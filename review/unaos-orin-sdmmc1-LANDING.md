# ORIN-SDMMC-1 — landing report

**Arc:** ORIN-SDMMC-1 — Tegra234 microSD-slot SDMMC controller bring-up, READ-ONLY recon. The installer
line's first rung (`~/.claude/plans/unaos/future/unaos-installer.md`), the NET-1 read-only-census house
pattern.

**Track:** hw-jetson. **Base:** hw-jetson fast-forwarded to `main` (`05eafaf`) at session start (was 4
commits behind, ff-only, clean).

## What landed

A new `sdmmc`-feature-gated, tegra-MMIO-gated module `arch/aarch64/sdmmc_tegra.rs` that censuses the Orin
devkit's microSD card **read-only**, plus its knob/feature/doc wiring:

- **M1 — FDT census + poison-honest probe.** Read-only DTB walk (reuses `fdt_tegra::Fdt`) enumerates every
  SDMMC/SDHCI-compatible node (compatible `nvidia,tegra234-sdhci` or a `mmc@`/`sdhci@` name), logging each
  candidate's `reg` base/size, `status`, `non-removable`/`cd-gpios`, and a bounded `compatible` ASCII view.
  Picks the **enabled removable** instance (the microSD slot; eMMC is `non-removable`), first-enabled as a
  documented fallback. **No hardcoded base** — the DTB decides. A mapped-GiB guard confirms the window is in
  the GiB-0 device window `mmu_tegra` already maps (never deref unmapped). Then, **before any write** (NET-4b
  law), `CAPABILITIES`/Host-Version are read + poison-checked; poison ⇒ honest refusal (no reset, no writes).
- **M2 — SDHCI identification (READ-ONLY).** Mirrors the proven Pi 4 `drivers::emmc2` register/bit model.
  Reset, status-latch, 3.3 V power, card-detect (absent ⇒ "no card seated" line, never a hang), 400 kHz,
  then CMD0/8/55-41/2/3/9/7/16 → 25 MHz default-speed. Prints CID (MID/OID/PNM/PRV/PSN/MDT), CSD-derived
  capacity (blocks/MiB, CSD v1/v2), bus width/speed (1-bit, default-speed). All waits CNTPCT-bounded.
- **M3 — sector-0 read census.** CMD17 single-block READ into a 512-byte stack buffer; dumps the first 16
  bytes hex + classifies GPT-protective MBR / FAT boot sector / MBR / unknown by signature.
- **Witness half** (`sdmmc` without `tegra`, the QEMU-virt build): one honest compiled-present line, zero MMIO.

### Files

- `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs` (new) — the recon module.
- `unaos/crates/kernel/src/arch/aarch64/mod.rs` — `#[cfg(feature = "sdmmc")] pub mod sdmmc_tegra;`.
- `unaos/crates/kernel/src/main.rs` — two call sites mirroring net4: virt witness (`all(sdmmc, not(tegra))`,
  after the net4 witness) and metal (`all(sdmmc, tegra)`, after the net4 metal call in `tegra_early_stop`).
- `unaos/crates/kernel/Cargo.toml` — `sdmmc = []` feature (standalone; does not imply tegra/pcie2).
- `unaos/arroyo` — `[ -n "${UNAOS_SDMMC:-}" ] && _feats="${_feats}sdmmc,"`.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — new §ORIN-SDMMC.
- `unaos/scripts/orin-sdmmc1-bench.md` (new) — the bench runbook.
- `review/unaos-orin-sdmmc1-LANDING.md` (this file).

## The DT resolution logic (no hardcoded base)

1. Walk the DTB; for each `reg` property, cheap-pre-filter on the node leaf name (`mmc@`/`sdhci@`/`sdmmc`),
   else confirm via `compatible` (contains `sdhci`/`tegra234-sdhci`). Collect unique candidate paths
   (bounded, `MAX_CAND = 8`).
2. For each candidate: resolve `reg` → (base, size) as (addr:2, size:2) cells; read `status` (absent ⇒
   okay), `non-removable` presence (⇒ eMMC, not the slot), `cd-gpios` presence. **Log every candidate.**
3. Pick the **first enabled + removable** instance as the microSD slot; fall back to the first enabled
   instance (documented) if none is marked removable; else refuse. Log the pick.
4. Guard: the picked base must lie in a mapped GiB (GiB-0 device window — always Device-nGnRE on tegra — or
   a RAM GiB from the mask) before any deref; else honest refusal (a controller outside the already-mapped
   windows would need the pcie2 `map_mmio_window` path this standalone feature does not pull).

The Orin Nano devkit slot is commonly `sdmmc1` (`mmc@3400000`), but the code lets the DTB decide and logs
every candidate found (including the on-module eMMC `sdmmc4` when present, which is skipped as
`non-removable`).

## Read-only-by-construction evidence

Commands issued by the module (grep `cmd\([0-9]+\)` of the source, excluding comments):
**CMD0, CMD2, CMD3, CMD7, CMD8, CMD9, CMD16, CMD17, CMD41, CMD55** — the identification ladder + a
single-block READ. There is **no** `cmd(24)`/WRITE_SINGLE_BLOCK, `cmd(25)`, `write_block`, ACMD6 bus-width
write, erase, or CMD6 switch in the code — the only `cmd(24)` literal in the file is inside a comment
documenting its absence. The controller-register writes the module does make (SRST, clock divider, bus
power, command-issue register) are the SDHCI machinery every read requires; **none targets card storage**.
No block backend is registered, no `drivers::block` write seam is touched. Read-only by construction.

## Gate results (verbatim)

- `./arroyo check` (default, both arches): `✅ x86_64 OK` / `✅ aarch64 OK` (pre-existing warnings only; no
  sdmmc_tegra diagnostics).
- `UNAOS_SDMMC=1 UNAOS_TEGRA=1 ./arroyo check`: `✅ x86_64 OK` / `✅ aarch64 OK`.
- `UNAOS_SDMMC=1 ./arroyo check` (virt): `✅ x86_64 OK` / `✅ aarch64 OK`.
- knob-off `UNAOS_GICV3=1 ./arroyo test-arm 40`: CAPSTONE 6/6 PASS (Semaphore/Mutex/Channel/Condvar/RwLock/
  join) + `CAPSTONE COMPLETE`; `✅ aarch64 test complete`. Zero `SDMMC` strings knob-off.
- `./arroyo test-arm 22`: `✅ aarch64 test complete`; 0 uppercase FAIL (the 7 lowercase `fail` are the
  standard UEFI/TPM boot noise — `Tpm2GetCapabilityPcrs fail!`, `failed to load Boot0001`, `failed to find
  range`).
- `./arroyo test 22` (x86): `✅ Test run complete`; 0 FAIL.
- `./arroyo kernel8-test`: `✅ Flashable image` + all U-chain/CAPSTONE PASS lines; 0 uppercase FAIL.
- knob-on virt `UNAOS_SDMMC=1 UNAOS_GICV3=1 ./arroyo test-arm 40`: the witness line
  `:: SDMMC: ORIN-SDMMC-1 Tegra234 microSD recon compiled; no Tegra234 SDMMC on this build (QEMU virt) —
  recon is metal-only (UNAOS_SDMMC=1 UNAOS_TEGRA=1) ::` printed, and CAPSTONE 6/6 intact.

## Flagged / metal-pending

- **QEMU cannot model the Tegra SDMMC** — the M1 (post-CAPS)/M2/M3 metal path is code-complete-prior-to-
  metal. Correctness rests on `arroyo check`, the QEMU non-regression (tegra code compiled out on virt), and
  faithful SD-spec/SDHCI adherence + the emmc2 model. The metal leg (census a seated card) is the next Orin
  sitting; runbook `scripts/orin-sdmmc1-bench.md`.
- **Tegra vendor-quirk assumptions (documented in-source + arch_arm64.md §ORIN-SDMMC):** (1) firmware/BPMP
  left the sdmmc1 module clock + pad power up — the module drives only the standard SDHCI internal divider,
  never the CAR/BPMP clock or the Tegra vendor pad registers (≥ 0x100); a never-stabilising internal clock
  surfaces as the honest "input clock gated" line (a BPMP-clock arc, not worked around). (2) A zero
  `CAPABILITIES` base-clock field falls back to an assumed 200 MHz (logged). (3) 4-bit/high-speed
  negotiation deferred (not needed to census).
- **Lane:** stayed in-lane — new tegra sdmmc module + arroyo/Cargo knob + named docs. Did NOT touch the Pi
  `emmc2` driver, pcie/net files, sched, or xhci.

## Commit

`sdmmc: ORIN-SDMMC-1 — Tegra234 microSD READ-ONLY census (UNAOS_SDMMC)` on `hw-jetson`. Not merged, not
pushed (per track rules — the integrator merges after review).
