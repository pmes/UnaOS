// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALL-PI — the installer engine's FIRST live end-to-end execution on the Pi 4's emmc2 microSD
// target, provable in QEMU. Rung 4's Pi target from the installer line (docs/../unaos-installer.md):
// the arch-neutral engine (`crate::install`: GPT writer + FAT32 formatter + sha extent copy-verify)
// driven onto the seated microSD via the in-tree BCM2711 `drivers::emmc2` SDHCI driver.
//
// Why the Pi is the first LIVE install (no bench wait): QEMU's `raspi4b` MODELS the SD controller and
// attaches a real (emulated) card image, so the emmc2 read/write path this drives is genuinely
// exercised — GPT → FAT32 → payload copy → sha extent-verify all run and PASS against an emulated card.
// The x86 INSTALL-CORE witness proved the engine on a scratch disk; the Orin INSTALL-1 flow is
// metal-only (QEMU models no Tegra234 SDMMC). This arc is the first install whose FULL flow executes
// under CI-grade QEMU.
//
// DESIGN LAW — the seated card is sacred (inherited from the Orin flow, absolute):
//   On METAL the Pi's seated card holds the running system (it booted from it). So the destructive
//   install stands behind a THREE-GATE escalation mirroring the Orin sdmmc/sdmmc_arm/install_target
//   ladder, plus an ABOUT-TO-DESTROY announcement before the first repartitioning write:
//     Gate 1 — `piinstall`         : census the card (read-only) + announce its identity. No write.
//     Gate 2 — `piinstall_arm`     : arm the WRITE path — a NON-destructive scratch write/verify/restore
//                                    ladder on the card's LAST block (stashed first, restored after,
//                                    every step verified; REFUSED if a GPT is present, whose backup
//                                    header lives in that block). Proves the installer can write+verify
//                                    without repartitioning. No install.
//     Gate 3 — `piinstall_confirm` : the DESTRUCTIVE-confirmation gate — the about-to-destroy line, then
//                                    the full GPT → zero-ESP-metadata → FAT32 → payload → verify install.
//   Each feature implies the previous (`piinstall_confirm` ⇒ `piinstall_arm` ⇒ `piinstall` ⇒ `baremetal`),
//   so a plain build compiles NONE of this and every kernel8 image is byte-identical to baseline.
//
// QEMU witness safety: `raspi4b` has one SD slot, so the install-witness run is its OWN QEMU invocation
// with a DEDICATED BLANK SCRATCH image in the slot — NEVER the kernel8-test battery's fixture image
// (which carries HELLO.BIN / the unafs volume the battery reads back). The `./arroyo kernel8-install`
// mode generates that blank scratch image, boots against it, and host-verifies the result off the
// image from OUTSIDE the kernel. See docs/dev/OS/10_INSTALL/installer_engine.md §INSTALL-PI.

#![cfg(feature = "piinstall")]

use crate::drivers::emmc2;
use super::{InstallError, InstallTarget};

/// Stable serial prefix so the operator (and the bench runbook) can grep the whole install as one block.
const PS: &str = ":: PIINSTALL:";

const SECTOR: usize = 512;

/// An `InstallTarget` over the in-tree Pi `drivers::emmc2` SDHCI driver. Sector granularity; every method
/// loops the proven single-block CMD17/CMD24 primitives (`emmc2::read_block_512` / `write_block_512`) —
/// the same read path M6g census uses and the same write path U9/U10 exercised. Bounded multi-sector
/// looping (the engine hands whole-sector buffers), exactly as the Orin `SdInstallTarget` loops its
/// single-block primitives. No multi-block CMD18/CMD25 — the engine needs correctness, not throughput.
struct EmmcInstallTarget {
    num_blocks: u64,
}

impl EmmcInstallTarget {
    /// Bind to the registered emmc2 block device, or `None` if the microSD is not up (no card censused).
    fn bind() -> Option<Self> {
        let info = crate::drivers::block::info()?;
        if info.block_size != SECTOR as u32 {
            return None;
        }
        Some(Self { num_blocks: info.num_blocks })
    }
}

impl InstallTarget for EmmcInstallTarget {
    fn capacity_sectors(&self) -> u64 {
        self.num_blocks
    }

    fn id(&self) -> alloc::string::String {
        alloc::format!("Pi emmc2 microSD ({} x 512B sectors)", self.num_blocks)
    }

    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), InstallError> {
        debug_assert!(buf.len() % SECTOR == 0);
        for (i, chunk) in buf.chunks_mut(SECTOR).enumerate() {
            let n = emmc2::read_block_512(lba + i as u64, chunk).map_err(map_blk)?;
            if n < SECTOR {
                return Err(InstallError::Io);
            }
        }
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), InstallError> {
        debug_assert!(buf.len() % SECTOR == 0);
        for (i, chunk) in buf.chunks(SECTOR).enumerate() {
            emmc2::write_block_512(lba + i as u64, chunk).map_err(map_blk)?;
        }
        Ok(())
    }
}

fn map_blk(e: crate::drivers::block::BlockError) -> InstallError {
    match e {
        crate::drivers::block::BlockError::NotReady => InstallError::NotReady,
        crate::drivers::block::BlockError::BadLba => InstallError::BadLba,
        crate::drivers::block::BlockError::Io => InstallError::Io,
        // WEDGE-10 (F2): the storage device was momentarily loaned to another context, so this sector op
        // refused its bounded wait and touched nothing. `InstallError` has no transient/retry variant —
        // the installer engine is a one-shot boot-path ladder with no retry loop to feed — so this maps
        // to `Io`: COARSE BUT HONEST. The install fails cleanly and loudly rather than proceeding on a
        // sector it never actually read or wrote, which is the only property the ladder's verify stages
        // depend on. (Unreachable in practice here: the Pi installer runs on the BSP before any other
        // context can hold the loan.) If this path ever grows a retry, `Busy` is the one error worth
        // retrying — see the reaper's requeue arm in `arch::aarch64::syscall::free_orphan_chain`.
        crate::drivers::block::BlockError::Busy => InstallError::Io,
    }
}

/// Classify sector 0 by signature (the Orin `classify_sector0` rule, local copy): GPT-protective MBR,
/// FAT boot sector, generic MBR, or unknown. Drives the about-to-destroy line + the ladder's GPT refusal.
fn classify_sector0(buf: &[u8; SECTOR]) -> &'static str {
    let sig_55aa = buf[510] == 0x55 && buf[511] == 0xaa;
    let fat_jump = buf[0] == 0xeb || buf[0] == 0xe9;
    let fat_str = &buf[0x36..0x39] == b"FAT" || &buf[0x52..0x55] == b"FAT";
    if sig_55aa {
        if buf[450] == 0xee {
            return "GPT-protective MBR (0xEE partition; GPT header at LBA 1)";
        }
        if fat_jump && fat_str {
            return "FAT boot sector (0x55AA + jump + FAT type string)";
        }
        return "MBR (0x55AA boot signature; classic partition table)";
    }
    if fat_jump && fat_str {
        return "FAT boot sector (jump + FAT type string; no 0x55AA)";
    }
    "unknown (no recognised signature)"
}

/// INSTALL-PI entry (Pi BSP boot path, post-`emmc2::probe`). Runs the three-gate escalation: census +
/// announce (Gate 1), the armed scratch ladder (Gate 2), the destructive install (Gate 3). Each higher
/// gate is compiled in only under its feature, so a plain `piinstall` build ends at the census.
pub fn run() {
    serial_println!("{} INSTALL-PI — installer engine over the Pi emmc2 microSD ::", PS);

    // ── Gate 1: census the card (read-only) ──
    let Some(t) = EmmcInstallTarget::bind() else {
        serial_println!(
            "{} no emmc2 block device registered (microSD not up / not censused) — install SKIPPED ::",
            PS
        );
        return;
    };
    let cap = t.capacity_sectors();
    if cap == 0 {
        serial_println!("{} card reports 0 sectors — install SKIPPED ::", PS);
        return;
    }
    let mut sec0 = [0u8; SECTOR];
    if t.read_sectors(0, &mut sec0).is_err() {
        serial_println!("{} sector-0 read failed — install SKIPPED (no census) ::", PS);
        return;
    }
    let class = classify_sector0(&sec0);
    serial_println!(
        "{} Gate 1 census — target = {}, capacity {} blocks ({} MiB), sector-0 = {} ::",
        PS, t.id(), cap, cap * SECTOR as u64 / (1024 * 1024), class
    );

    // ── Gate 2: the armed scratch write/verify/restore ladder (non-destructive) ──
    #[cfg(feature = "piinstall_arm")]
    scratch_ladder(&t, &sec0);

    // ── Gate 3: the destructive install ──
    #[cfg(feature = "piinstall_confirm")]
    install_to_pi(t, &sec0);

    #[cfg(not(feature = "piinstall_arm"))]
    serial_println!(
        "{} Gate 1 only (UNAOS_PIINSTALL) — read-only census; the write path + install are gated (UNAOS_PIINSTALL_ARM / _CONFIRM) ::",
        PS
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// Gate 2 — the write-path ARM: a non-destructive scratch write/verify/restore ladder on the card's LAST
// block. Mirrors the Orin ORIN-SDMMC-2 paranoia ladder: stash → write stamped pattern → verify → restore
// → verify, REFUSING if a GPT is present (its backup header lives in the last LBA). Compiled out unless
// `piinstall_arm`, so a plain `piinstall` build ends at the Gate-1 census, byte-identical.
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "piinstall_arm")]
fn make_pattern(buf: &mut [u8; SECTOR], lba: u64) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i as u8) ^ 0x5a;
    }
    const MARK: &[u8] = b"UNAOS-PIINSTALL-SCRATCH";
    buf[..MARK.len()].copy_from_slice(MARK);
    buf[32..40].copy_from_slice(&lba.to_le_bytes());
}

#[cfg(feature = "piinstall_arm")]
fn scratch_ladder(t: &EmmcInstallTarget, sec0: &[u8; SECTOR]) {
    serial_println!(
        "{} Gate 2 ARMED (UNAOS_PIINSTALL_ARM) — non-destructive scratch write/verify/restore ladder on the LAST block ::",
        PS
    );

    // The last LBA holds a GPT backup header when a GPT is present — never our scratch region then.
    let class = classify_sector0(sec0);
    if class.starts_with("GPT") {
        serial_println!(
            "{}   ladder REFUSED — sector 0 is {}; a GPT backup header lives in the last LBA (our scratch region). No write. ::",
            PS, class
        );
        return;
    }
    let scratch_lba = t.capacity_sectors() - 1;

    // Stash the current contents.
    let mut stash = [0u8; SECTOR];
    if emmc2::read_block_512(scratch_lba, &mut stash).is_err() {
        serial_println!("{}   ladder FAIL (stash read) at LBA {} — REFUSING to write (nothing to restore) ::", PS, scratch_lba);
        return;
    }

    // Write a stamped pattern.
    let mut pattern = [0u8; SECTOR];
    make_pattern(&mut pattern, scratch_lba);
    if emmc2::write_block_512(scratch_lba, &pattern).is_err() {
        serial_println!("{}   ladder FAIL (write) at LBA {} ::", PS, scratch_lba);
        restore_or_warn(scratch_lba, &stash);
        return;
    }

    // Read back + verify against the pattern.
    let mut back = [0u8; SECTOR];
    if emmc2::read_block_512(scratch_lba, &mut back).is_err() || back != pattern {
        serial_println!("{}   ladder FAIL (verify) — read-back != written pattern at LBA {} ::", PS, scratch_lba);
        restore_or_warn(scratch_lba, &stash);
        return;
    }

    // Restore the stash + verify.
    if !restore_or_warn(scratch_lba, &stash) {
        return;
    }
    serial_println!(
        "{} Gate 2 scratch ladder — write/verify/restore/verify at LBA {} => PASS ::",
        PS, scratch_lba
    );
}

/// Re-write the stash and verify it. Returns whether the original was provably put back; warns loudly if
/// not (the block-layer write path already guards durability via CMD13, so a hex dump is not repeated here).
#[cfg(feature = "piinstall_arm")]
fn restore_or_warn(lba: u64, stash: &[u8; SECTOR]) -> bool {
    if emmc2::write_block_512(lba, stash).is_ok() {
        let mut chk = [0u8; SECTOR];
        if emmc2::read_block_512(lba, &mut chk).is_ok() && chk == *stash {
            return true;
        }
    }
    serial_println!("{}   ladder RESTORE FAILED at LBA {} — original scratch-block contents may be lost ::", PS, lba);
    false
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// Gate 3 — the DESTRUCTIVE install: the SELF-CLONE (INSTALL-PI-2). Compiled out unless `piinstall_confirm`.
// Drives the arch-neutral installer engine onto the seated microSD after the about-to-destroy announcement,
// with the payload being the running system's OWN boot media — the card's FAT boot partition (kernel8.img /
// config.txt / the firmware files the GPU ROM loads), cloned file-by-file onto a fresh GPT + FAT32 layout.
//
// SAME-DEVICE clone (the Pi analogue of the Orin §INSTALL-2, which clones a SEPARATE USB stick onto the SD):
// the Pi has ONE block device — the seated card is BOTH the source boot media AND the install target. So the
// source boot tree is SNAPSHOTTED WHOLE INTO MEMORY (`install::clone::snapshot`) BEFORE the first destructive
// write; only then does the GPT + zero + FAT32 + tree-write pass run, mirroring the buffered tree back via the
// engine's `TreeWriter`. Every cloned file is re-read off the card and sha-verified through the engine's own
// `verify_extents` — the installer's content-verify. Handles a NON-blank card: announce it, then re-establish
// the FAT blank-precondition by zeroing exactly the ESP metadata region before formatting.
//
// PARTITION RULING (byte-clone vs fresh-format): the boot FAT tree is cloned BY CONTENT (fresh FAT32 ESP,
// files re-written + per-file sha-verified) — never a raw byte-clone. A source data/unafs partition is NOT
// byte-copied: `write_gpt` lays a FRESH empty data partition. This follows the Orin §INSTALL-2 ruling and the
// engine's verify discipline — every cloned byte must be file-sha-verifiable, which a raw partition image is
// not, and the boot media the ROM needs is the FAT tree. See installer_engine.md §INSTALL-PI-2.
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "piinstall_confirm")]
fn install_to_pi(mut t: EmmcInstallTarget, sec0: &[u8; SECTOR]) {
    serial_println!(
        "{} Gate 3 THIRD GATE (UNAOS_PIINSTALL_CONFIRM) — self-cloning the running boot media onto the SEATED Pi microSD ::",
        PS
    );
    serial_println!(
        "{}   gates: [1] emmc2 census OK · [2] write path armed · [3] destructive-confirm — all satisfied ::",
        PS
    );

    // About-to-destroy announcement.
    let class = classify_sector0(sec0);
    serial_println!(
        "{} ABOUT TO DESTROY: microSD sector-0 = {} · capacity {} blocks ({} MiB) — the entire card is about to be repartitioned ::",
        PS, class, t.capacity_sectors(), t.capacity_sectors() * SECTOR as u64 / (1024 * 1024)
    );

    match install_flow(&mut t) {
        Ok(n) => serial_println!(":: INSTALL: pi emmc2 gpt+zero+fat32+clone({} files) verify => PASS ::", n),
        Err(e) => serial_println!(":: INSTALL: pi emmc2 install => FAIL ({:?}) ::", e),
    }
}

/// The engine driver: returns the number of files cloned (so `?` early-returns land on the single FAIL line
/// above). Snapshots the seated card's own boot tree into memory, then lays a fresh GPT + FAT32 and mirrors
/// the buffered tree back onto it, sha-verifying every cloned file off the card.
#[cfg(feature = "piinstall_confirm")]
fn install_flow(t: &mut EmmcInstallTarget) -> Result<usize, InstallError> {
    use super::{clone, fat32, gpt, verify_extents};

    // 0) SNAPSHOT the source boot tree WHOLE into memory BEFORE any destructive write. The seated card is
    //    both the source and the target, so once the GPT is rewritten the source is gone — buffer it first.
    //    `fs::fat::mount()` reads `drivers::block` (the same emmc2 card the census bound); its FAT boot
    //    partition is the running system's own boot media.
    let tree = {
        let src = crate::fs::fat::mount().map_err(|_| InstallError::NotReady)?;
        serial_println!("{}   INSTALL: mounted the card's own boot partition — {} ::", PS, src.describe());
        clone::snapshot(&src)?
    };
    if tree.file_count == 0 {
        // A boot partition with no files is not a self to clone — refuse rather than "PASS" a hollow card.
        serial_println!("{}   INSTALL: the card's boot partition carried no files to clone => FAIL ::", PS);
        return Err(InstallError::BadArg);
    }
    serial_println!(
        "{}   INSTALL: snapshotted the boot tree into memory — {} files, {} bytes (source now safe to overwrite) ::",
        PS, tree.file_count, tree.total_bytes
    );

    // 1) GPT: protective MBR + primary/backup + ESP + data, with the engine's own parse-back verify.
    let layout = gpt::write_gpt(t)?;
    serial_println!(
        "{}   INSTALL: GPT written + parse-back verified — ESP LBA {}..{}, data LBA {}..{} of {} sectors ::",
        PS, layout.esp_first_lba, layout.esp_last_lba, layout.data_first_lba, layout.data_last_lba,
        layout.total_sectors
    );

    // 2) Re-establish the FAT blank-precondition on a possibly-non-blank card: zero exactly the ESP
    //    reserved + FAT region (a stale FAT entry would forge an allocation). Data-area free clusters are
    //    left as-is (harmless) — the Orin install_flow discipline.
    let esp_sectors = layout.esp_last_lba - layout.esp_first_lba + 1;
    let blank_sectors = fat32::blank_region_sectors(esp_sectors)?;
    let zero = [0u8; SECTOR];
    for s in 0..blank_sectors {
        t.write_sectors(layout.esp_first_lba + s, &zero)?;
    }
    serial_println!(
        "{}   INSTALL: zeroed {} ESP metadata sectors (reserved + both FATs) to re-establish the blank-precondition ::",
        PS, blank_sectors
    );

    // 3) FAT32 format the ESP.
    let geom = fat32::format_esp(t, layout.esp_first_lba, esp_sectors)?;
    serial_println!(
        "{}   INSTALL: ESP formatted FAT32 — fat_sz={}sec clusters={} data@vol+{} ::",
        PS, geom.fat_sz, geom.count_of_clusters, geom.data_start
    );

    // 4) THE SELF-CLONE: mirror the buffered boot tree onto the freshly-formatted ESP, recording each
    //    file's extents for the content-verify below.
    let recs = clone::write_snapshot(t, geom, &tree)?;
    let data_extents: usize = recs.iter().map(|r| r.extents.len()).sum();
    serial_println!(
        "{}   INSTALL: cloned {} files ({} data extents) from the boot tree ::",
        PS, recs.len(), data_extents
    );

    // 5) Content-verify EVERY cloned file by re-reading its extents off the card and SHA-checking — the
    //    installer's content-verify IS the bench's content-verify, now native. Print the per-file manifest
    //    (the real manifest that replaces INSTALL-1's single UNAOS.IMG marker).
    for r in &recs {
        if !verify_extents(t, &r.extents, &r.sha)? {
            serial_println!("{}   INSTALL: {} extent sha-verify => FAIL ::", PS, r.path);
            return Err(InstallError::VerifyFailed);
        }
        serial_println!(
            "{}   INSTALL: {} ({} B, {} extents) sha256={} => VERIFIED ::",
            PS, r.path, r.size, r.extents.len(), clone::sha_hex(&r.sha)
        );
    }
    serial_println!(
        "{}   INSTALL: all {} cloned files re-read off the card + sha-verified => PASS ::",
        PS, recs.len()
    );
    Ok(recs.len())
}
