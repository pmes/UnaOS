# ORIN-INSTALL-1 — landing report

**Arc:** INSTALL-1 — wire the installer engine to the Orin microSD: the first real installer flow. Boot from
the USB stick, install UnaOS onto the seated microSD from inside UnaOS. Rung 3 of the installer line
(`~/.claude/plans/unaos/future/unaos-installer.md`); rungs 1 (SDMMC-1 recon), 2 (SDMMC-2 armed write) and 3a
(INSTALL-CORE engine) are merged.

**Track:** hw-jetson. **Base:** hw-jetson, level with `main` (0 ahead / 0 behind at session start; clean).

## What landed

The first real installer flow: the arch-neutral installer engine (`crate::install`) driven onto the SEATED
microSD via the rung-2 armed single-block write path, behind a THIRD (destructive-confirmation) gate. The
engine's verified write/verify semantics are **untouched** — this arc adds the block target, a
metadata-zeroing pass, and the destructive announcement; the SD-specific glue lives entirely in
`arch/aarch64/sdmmc_tegra.rs`.

### Files

- `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs` — `SdInstallTarget` (an `InstallTarget` over the
  rung-2 CMD24/CMD17 primitives), `installer_marker_payload`, `install_to_sd` + `install_flow`, the virt
  witness line, and the `install_target`-gated call site. Plus a `cid: [u32;4]` field added to `Card`
  (`install_target`-gated) so the announce can re-decode the identity. ALL new code is
  `#[cfg(feature = "install_target")]`.
- `unaos/crates/kernel/src/install/mod.rs` — `verify_extents` made `pub` (additive).
- `unaos/crates/kernel/src/install/fat32.rs` — new pub `blank_region_sectors(esp_sectors)` helper (additive).
- `unaos/crates/kernel/src/lib.rs` — `pub mod install` gate widened to `any(installdemo, install_target)`.
- `unaos/crates/kernel/Cargo.toml` — `install_target = ["sdmmc_arm"]`.
- `unaos/arroyo` — `UNAOS_INSTALL_TARGET_SD=1` → `install_target,sdmmc_arm,sdmmc`.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — §ORIN-INSTALL-1 subsection.
- `unaos/docs/dev/OS/10_INSTALL/installer_engine.md` — §INSTALL-1 section.
- `unaos/scripts/orin-sdmmc1-bench.md` — the install leg (three-gate escalation, about-to-destroy line,
  PASS/FAIL shapes).
- `review/unaos-install1-LANDING.md` (this file).

## The three-gate escalation ladder

A real install is the most destructive act in the tree, so it stands behind three independent gates:

| gate | feature / knob | meaning |
|---|---|---|
| 1 | `sdmmc` / `UNAOS_SDMMC` | controller up and the card **census succeeded** (we hold a `Card`) |
| 2 | `sdmmc_arm` / `UNAOS_SDMMC_ARM` | the rung-2 armed CMD24 write path is compiled in |
| 3 | `install_target` / `UNAOS_INSTALL_TARGET_SD` | the explicit **destructive-confirmation** gate (knob stands in for the future operator UX) |

Under gate 3, before the first write, the flow prints the sector-0 classification, the capacity, and the card
**CID** it is about to destroy. Unlike the engine demo's blank-only `blank_check` refusal, a non-blank card is
**installable** — the flow announces it, then zeroes exactly the ESP metadata region
(`fat32::blank_region_sectors` = reserved + both FAT copies) to re-establish the formatter's blank-precondition
without altering the engine. The flow: GPT → zero-ESP-metadata → FAT32 → payload copy → sha extent-verify →
`ORIN-INSTALL-1 SD install — gpt+zero+fat32+copy verify => PASS`.

## M2 payload adjudication — self-read NOT reachable; honest fallback + flag

`install_to_sd` runs at the **pre-JB2b-xHCI-takeover EL2 census site** (`sdmmc_census`, invoked from
`main.rs:1388`, ahead of the `jb2b_attach` takeover at `~1483`). At that site `drivers::block::info()` is
`None` — the USB boot stick is not yet enumerated as a mass-storage block device (the tegra xHCI storage
bring-up in `xusb_tegra.rs` runs later, in the JB2b pump window). Self-read of the running boot volume is
therefore **not reachable** at the install site. Rather than fake a clone, v1 writes a **generated marker
payload** (`UNAOS.IMG`, ~4 KiB, self-describing) and the **self-clone is flagged as the named follow-up,
INSTALL-2** (which needs the boot media readable as a block device at the install position). The in-tree
`fs::fat::mount()` interop self-check the x86 engine witness runs is likewise skipped here (`mount()` reads the
USB block layer, not this armed SD target); the SD extent sha-verify is the by-content proof.

## Multi-block work — none added, by choice (single-block suffices)

No multi-block CMD25/CMD18 plumbing was added. `SdInstallTarget::{read,write}_sectors` loop the proven rung-2
single-block CMD17/CMD24 primitives (`read_block_at`/`write_block_at`), chunking the engine's multi-sector
buffers into 512-byte blocks. The engine needs correctness, not throughput, and single-block is the exact path
rung-2 verified on metal. The only sizable write is the bounded metadata-zero pass (~2064 sectors for a 64 MiB
ESP); CMD25 batching is recorded as a named perf follow-up, not a correctness gap.

## Gate results

- **`./arroyo check` across the knob matrix** — green both arches (x86_64 + aarch64) for: default, `UNAOS_SDMMC`,
  `UNAOS_SDMMC_ARM`, `UNAOS_INSTALL_TARGET_SD` (each × virt and × `UNAOS_TEGRA`), plus `UNAOS_INSTALLDEMO`. No
  new warnings in any touched file.
- **GICv3 CAPSTONE 6/6** (`UNAOS_GICV3=1 ./arroyo test-arm`) — `CAPSTONE COMPLETE — all 6 sync primitives
  verified in one boot`; Semaphore/Mutex/Channel/Condvar/RwLock/join all PASS; **0 FAIL**.
- **test-arm** (plain + `UNAOS_INSTALL_TARGET_SD=1 UNAOS_GICV3=1`) — **0 FAIL**; the install_target virt run
  prints the three SDMMC/INSTALL compiled-present-metal-only witness lines and still reaches CAPSTONE 6/6.
- **x86 `./arroyo test`** — **0 FAIL**.
- **`UNAOS_INSTALLDEMO=1 ./arroyo test`** — the engine witness still runs end-to-end:
  `:: INSTALL: gpt+fat32+copy verify => PASS ::`, **0 FAIL** (the `verify_extents`-pub and
  `blank_region_sectors` additions did not perturb the engine).
- **`./arroyo kernel8-test`** (Pi bare-metal gate) — **0 FAIL**; the full K-suite (K3/K4/K5/K9/FATDIRS/FATMOVE/
  IMG-SIG/…) PASS.
- **String-identity (rung-2 method).** The tegra `sdmmc_arm` binary contains **0** `ORIN-INSTALL` /
  `ABOUT TO DESTROY` strings; the `install_target` binary contains them (4). All installer code — including the
  `Card.cid` field — is `install_target`-gated, so an `sdmmc_arm`-without-`install_target` build is
  behavior-identical to the merged rung-2 ladder.

## Metal leg (owed — next Orin sitting, attended)

The full flow's **first execution is the attended Orin sitting**: `UNAOS_INSTALL_TARGET_SD=1 UNAOS_TEGRA=1
./arroyo esp-jetson`, on a card the operator is willing to erase — confirm the ABOUT-TO-DESTROY line, reach
`SD install … => PASS`, then re-seat the card and confirm a host reader sees a `UNAOS-ESP` FAT32 volume with
`UNAOS.IMG`. Runbook: `unaos/scripts/orin-sdmmc1-bench.md`, install leg.

## Flagged

- **INSTALL-2 (self-clone):** copy the running system's own boot volume as the payload — needs the boot media
  readable as a block device at the install position (a post-takeover install site or a second block backend).
- **Throughput:** multi-block CMD25/CMD18 on the SD path (single-block is correct but slower on the zero pass).
