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

/// The built-in v1 payload — a small, self-describing marker/manifest blob. HONEST BY DESIGN (M2
/// adjudication): a Pi "install" payload is ultimately the boot volume's FAT files (kernel8.img +
/// start4.elf + config.txt — what the GPU ROM loads), i.e. the running system's own boot media. At this
/// pre-shell BSP call site those files are not reachable as a readable clone source, so — rather than
/// fake one — we write a marker the in-tree FAT reader can find, and FLAG the self-clone (copy the boot
/// FAT files) as the named metal follow-up (mirrors the Orin INSTALL-2 self-clone). Spans several sectors
/// so the extent machinery is exercised across a chain.
#[cfg(feature = "piinstall_confirm")]
fn installer_marker_payload() -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(4096);
    v.extend_from_slice(
        b"UNAOS-INSTALL-PI marker payload v1\n\
          Written by the booted UnaOS installer onto the Pi's emmc2 microSD.\n\
          The v1 self-clone (copy the boot FAT files: kernel8.img/start4.elf/config.txt) is the\n\
          named metal follow-up.\n",
    );
    let mut i: usize = 0;
    while v.len() < 4096 {
        v.push(((i.wrapping_mul(37).wrapping_add(13)) & 0xFF) as u8);
        i += 1;
    }
    v
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
// Gate 3 — the DESTRUCTIVE install. Compiled out unless `piinstall_confirm`. Drives the arch-neutral
// installer engine onto the seated microSD after the about-to-destroy announcement. Handles a NON-blank
// card (unlike the engine's blank-only demo): announce it, then re-establish the FAT blank-precondition
// by zeroing exactly the ESP metadata region before formatting — the Orin `install_flow` shape.
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "piinstall_confirm")]
fn install_to_pi(mut t: EmmcInstallTarget, sec0: &[u8; SECTOR]) {
    serial_println!(
        "{} Gate 3 THIRD GATE (UNAOS_PIINSTALL_CONFIRM) — installing UnaOS onto the SEATED Pi microSD ::",
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
        Ok(()) => serial_println!(":: INSTALL: pi emmc2 gpt+fat32+copy verify => PASS ::"),
        Err(e) => serial_println!(":: INSTALL: pi emmc2 install => FAIL ({:?}) ::", e),
    }
}

/// The engine driver, factored out so `?` early-returns land on the single FAIL line above.
#[cfg(feature = "piinstall_confirm")]
fn install_flow(t: &mut EmmcInstallTarget) -> Result<(), InstallError> {
    use super::{fat32, gpt, hash, verify_extents};

    // 1) GPT: protective MBR + primary/backup + ESP + data, with the engine's own parse-back verify.
    let layout = gpt::write_gpt(t)?;
    serial_println!(
        "{}   GPT written + parse-back verified — ESP LBA {}..{}, data LBA {}..{} of {} sectors ::",
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
        "{}   zeroed {} ESP metadata sectors (reserved + both FATs) to re-establish the blank-precondition ::",
        PS, blank_sectors
    );

    // 3) FAT32 format the ESP.
    let geom = fat32::format_esp(t, layout.esp_first_lba, esp_sectors)?;
    serial_println!(
        "{}   ESP formatted FAT32 — fat_sz={}sec clusters={} data@vol+{} ::",
        PS, geom.fat_sz, geom.count_of_clusters, geom.data_start
    );

    // 4) Copy the v1 marker payload, recording extents; then content-verify by re-reading them off the card.
    let payload = installer_marker_payload();
    let payload_sha = hash::sha256(&payload);
    let extents = fat32::write_payload_file(t, &geom, "UNAOS.IMG", &payload)?;
    serial_println!(
        "{}   copied UNAOS.IMG ({} bytes, {} extents) ::",
        PS, payload.len(), extents.len()
    );
    if !verify_extents(t, &extents, &payload_sha)? {
        serial_println!("{}   extent sha-verify => FAIL ::", PS);
        return Err(InstallError::VerifyFailed);
    }
    serial_println!("{}   extent sha-verify (re-read every written extent off the card) => PASS ::", PS);
    Ok(())
}
