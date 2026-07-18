# ORIN-SDMMC-3 — landing report

**Arc:** ORIN-SDMMC-3 — multi-block SD transfers + the small install follow-ups. Closes the two INSTALL-2
follow-ups (`review/unaos-install2-LANDING.md` §Flagged): single-block-only SD transfers (throughput) and
single-cluster directories (the >16-entry bound). Same `UNAOS_INSTALL_TARGET_SD=1` gate.

**Track:** hw-jetson. **Base:** hw-jetson, fast-forwarded to `main` at session start (was 2 behind; ff-only,
now level). Clean.

## What landed

Multi-block CMD18/CMD25 SD transfer primitives (auto-CMD12 completion), used by `SdInstallTarget` for
whole-file writes with single-block retained as the fallback; and multi-cluster FAT32 directory support in the
`TreeWriter`, lifting the INSTALL-2 >16-entry `NoSpace` bound. The x86 `installdemo` witness gains a
multi-cluster-directory proof (a `SUB/` directory of 20 files spanning 2 clusters, re-read + sha-verified
through the in-tree FAT reader). All additive; every knob-off battery unregressed; the `sdmmc_arm` (unarmed)
jetson binary rebuilds byte-for-byte identical.

### Files

- `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs` — `read_blocks_at` (CMD18) / `write_blocks_at` (CMD25)
  multi-block primitives + Transfer-Mode consts + `MULTIBLOCK_CHUNK_BLOCKS` (all `install_target`-gated);
  `SdInstallTarget::{read,write}_sectors` rewritten to loop the bounded chunk with a single-block tail
  fallback; `copy_dir` rewritten for multi-cluster directories (in-memory image sized up front from the source
  entry count, `write_dir_image`); `count_nondot` helper; `install_flow` reserves the root cluster chain via
  `reserve_root` before any file allocation.
- `unaos/crates/kernel/src/install/fat32.rs` — **additive** `TreeWriter` methods `alloc_dir_clusters`,
  `reserve_root`, `write_dir_image` (replacing the single-cluster `alloc_dir_cluster`/`write_dir_cluster`);
  `dir_clusters_for_slots`; `put_dir_entry` generalized from `&mut [u8; 512]` to `&mut [u8]`;
  `write_file` batches its data write into one contiguous multi-sector call (per-cluster extents unchanged).
- `unaos/crates/kernel/src/install/mod.rs` — the x86 `installdemo` witness (`run_demo_inner`) gains step 8,
  `demo_multicluster_dir`, exercising the multi-cluster directory path end-to-end.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — new §ORIN-SDMMC-3 subsection; the stale "single-block, by choice"
  and "directories fit one cluster" notes in §ORIN-INSTALL-1/2 updated to point forward.
- `unaos/docs/dev/OS/10_INSTALL/installer_engine.md` — new §ORIN-SDMMC-3 section + header line; the throughput
  owed-item retired.
- `review/unaos-orin-sdmmc3-LANDING.md` (this file).

## M1 — multi-block CMD18/CMD25 + the two design choices

- **Chunk size = 64 blocks (32 KiB).** `SdInstallTarget::{read,write}_sectors` loop `MULTIBLOCK_CHUNK_BLOCKS =
  64`, taking the min of the chunk and the remaining blocks each pass; a 1-block tail (and every 512-byte
  metadata call) drops to the retained single-block CMD17/CMD24 primitive. 64 keeps one transfer's PIO drain +
  bounded-wait budget modest while collapsing a run into ~1/64 the command count.
- **Completion = auto-CMD12 (not explicit CMD12).** The Transfer-Mode field sets Auto-CMD12-Enable so the host
  controller issues `STOP_TRANSMISSION` itself at the counted transfer's end — no second command round-trip and
  no separate CMD12 error-handling path; normal `INT_DATA_DONE` still fires. `BLKSIZECNT[31:16]` carries the
  count, Transfer-Mode sets Block-Count-Enable + Multi-Block-Select, and reads add the card→host direction bit.
- **Gating.** Both primitives are `install_target`-gated (⇒ `sdmmc_arm` ⇒ `sdmmc`). This satisfies the arc's
  "multi-block WRITES exist only under `sdmmc_arm`" floor and keeps plain `sdmmc`/`sdmmc_arm` builds
  byte-identical (they are the only builds that use them). The **rung-2 witness ladder is untouched** — still
  single-block CMD24 — so the metal-verified witness does not shift.
- **`TreeWriter::write_file`** now writes each file's contiguous cluster chain in one `write_sectors` call, so
  on the SD target a whole file rides multi-block CMD25 (per-cluster extents retained for verify granularity).

## M2 — multi-cluster directories + the >16-entry witness

`TreeWriter` builds a directory's image wholly in memory across its whole cluster chain, then writes it once
(the no-stale-byte discipline, now spanning >1 cluster). Each directory is sized up front from its source entry
count (`dir_clusters_for_slots`): subdirectories via `alloc_dir_clusters`, the root via `reserve_root` (called
before any file allocation so cluster 2's chain stays physically contiguous with the extension clusters). The
INSTALL-2 >16-entry `NoSpace` is lifted — `NoSpace` now means the volume is genuinely full.

The x86 `UNAOS_INSTALLDEMO` witness proves it: after the existing engine run it re-establishes the
blank-precondition, re-formats, builds `SUB/` with 20 files (22 slots → 2 clusters) through the `TreeWriter`,
then the in-tree FAT reader mounts the volume, follows `SUB/`'s cluster chain, and re-reads + SHA-verifies every
file. The verdict line gained a `dirs=1` field per the brief; existing line meanings are unchanged.

## Gate results (verbatim)

- **`./arroyo check` — default (both arches):** 0 errors (only the pre-existing `own_load`/`RING_SIZE`/… warnings
  in unrelated files; none in the touched files).
- **`UNAOS_INSTALLDEMO=1 ./arroyo check` (both arches):** ✅ x86_64 OK, ✅ aarch64 OK; 0 errors, no warnings in
  the touched files.
- **`UNAOS_INSTALL_TARGET_SD=1 ./arroyo check` — virt (both arches):** ✅ both OK.
- **`UNAOS_INSTALL_TARGET_SD=1 UNAOS_TEGRA=1 ./arroyo check` — tegra (both arches):** ✅ both OK, no warnings in
  the touched files.
- **`UNAOS_INSTALLDEMO=1 ./arroyo test 22` (x86 engine):** engine end-to-end PASS, including the new proof —
  `:: INSTALL: multi-cluster dir — SUB/ 20 entries across 2 clusters, all re-read + sha-verified (dirs=1) => PASS ::`
  then `:: INSTALL: gpt+fat32+copy verify => PASS ::`; 0 FAIL.
- **`./arroyo test 22` (x86):** 0 FAIL.
- **`./arroyo test-arm 22` (plain GICv2):** 0 FAIL.
- **`UNAOS_GICV3=1 ./arroyo test-arm 22` (CAPSTONE):** `:: CAPSTONE COMPLETE — all 6 sync primitives verified in
  one boot ::`; 0 FAIL.
- **`./arroyo kernel8-test 35` (Pi bare-metal gate):** `:: CAPSTONE COMPLETE … ::`; 0 FAIL.
- **String-identity (esp-jetson builds, established method):** the unarmed `UNAOS_SDMMC_ARM=1` jetson kernel.elf
  contains **0** `ORIN-SDMMC-3` / `READ_MULTIPLE` / `CMD18` / `CMD25` / `mb: CMD` / `multi-cluster` strings and
  **rebuilds byte-for-byte identical** (sha256 `68599c33…9581` twice). The `UNAOS_INSTALL_TARGET_SD=1` jetson
  kernel.elf contains the multi-block strings (`mb: CMD18` ×2, `mb: CMD25` ×2).

## Lane note

M2/the DONE gate require the x86 `installdemo` witness to prove the >16-entry directory; that witness lives in
`install/mod.rs`, which the LANE line ("`sdmmc_tegra.rs`, `install/fat32.rs` (additive), named docs") did not
name explicitly. The general multi-cluster capability is confined to `fat32.rs` (in lane); `mod.rs` gained only
the demo driver that exercises it (one new step + `demo_multicluster_dir`), the minimum the DONE gate demands.
Flagging per the lane discipline — no other file outside the lane was touched.

## Metal leg (owed — next Orin sitting, attended)

The multi-block SD path (CMD18/CMD25) is compiled + `arroyo check`-verified only (QEMU models no Tegra234
SDMMC); its first metal exercise is the attended Orin self-clone sitting, alongside the INSTALL-2 flow. Runbook:
`unaos/scripts/orin-sdmmc1-bench.md`, install leg.

## Flagged

- **Bootability** (unchanged from INSTALL-2): making the Orin actually boot the cloned card (ESP type GUID /
  attributes / firmware boot-order) is the next rung's metal question.
- **Metal SD throughput measurement:** the CMD25 win is structural (fewer commands); the actual metal
  read/write rates are first observable at the attended sitting.
