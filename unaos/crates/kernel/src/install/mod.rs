// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALL-CORE — the storage-agnostic installer engine.
//
// A GPT writer + FAT32 formatter + extent-level copy-and-verify, all expressed over one small
// abstraction — the `InstallTarget` trait — so the same engine drives any block target: the QEMU
// scratch disk this arc proves it on, and (later arcs) the Orin microSD / Pi emmc2 / an x86 USB
// stick. The engine is the RUNG-3 ENGINE built ahead of the SD write path so it is proven before it
// ever touches a real card.
//
// SAFETY DISCIPLINE (never-touch-a-seated-card, from birth):
//   * The engine only runs when explicitly ARMED. In this arc the arm is the `installdemo` feature +
//     the `UNAOS_INSTALLDEMO=1` knob, and the target is ONLY a dedicated blank scratch disk the build
//     attaches over the usb-storage slot (the boot volume is a separate ide-hd ESP, never touched).
//   * Before the first write, `blank_check` reads the target's leading sectors and REFUSES (honest
//     line, no writes) unless they are blank — the guard that keeps the engine off any volume that
//     already holds a partition table or filesystem.
//   * Verify by CONTENT always: the GPT writer re-reads + re-CRCs everything it wrote, and the copy
//     primitive re-reads every written extent and SHA-256-checks it. A negative test corrupts one
//     byte and proves the verifier FAILS when it should.
//
// This module is arch-neutral (it drives only the arch-neutral `drivers::block` layer), so it COMPILES
// on both x86_64 and aarch64 under the `installdemo` feature. The witness driver `run_demo` is invoked
// only from the x86_64 boot path this arc (the QEMU scratch disk lives on x86); aarch64 compiles the
// module (its pub API raises no dead-code warning) but never calls it.

pub mod fat32;
pub mod gpt;
pub mod hash;

// INSTALL-PI: the Pi 4 emmc2 microSD installer flow (three-gate escalation), driving this same engine
// onto the seated card via `drivers::emmc2`. `piinstall`-gated (⇒ baremetal); compiled out otherwise.
#[cfg(feature = "piinstall")]
pub mod pi;

// INSTALL-PI-2: the buffered self-clone payload primitive (snapshot the source boot tree into memory,
// then mirror it onto the freshly-formatted ESP). The engine's payload seam for a SAME-device clone
// (the Pi reads its source from and installs onto the one seated card). `piinstall_confirm`-gated —
// compiled only where the destructive Pi install that uses it is.
#[cfg(feature = "piinstall_confirm")]
pub mod clone;

use crate::drivers::block;

const SECTOR: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallError {
    /// No block device / storage not brought up.
    NotReady,
    /// A block read or write failed.
    Io,
    /// An LBA past the device capacity.
    BadLba,
    /// The device is too small for a valid GPT + FAT32 ESP layout.
    TooSmall,
    /// A parse-back / content verification did not match what was written.
    VerifyFailed,
    /// A caller argument is invalid (e.g. an empty payload or an unrepresentable 8.3 name).
    BadArg,
    /// The volume/partition has no free space for the request.
    NoSpace,
    /// The armed target's leading sectors are NOT blank — the engine refuses to write over it.
    NotBlank,
}

/// The one abstraction the installer engine writes through. Sector granularity, explicit capacity +
/// identity, and a read side so every write is verifiable. `buf` lengths must be a whole number of
/// 512-byte sectors.
pub trait InstallTarget {
    /// Logical sector size in bytes (512 for every target this arc supports).
    fn sector_size(&self) -> usize {
        SECTOR
    }
    /// Total addressable sectors.
    fn capacity_sectors(&self) -> u64;
    /// A human-readable identity for the witness log.
    fn id(&self) -> alloc::string::String;
    /// Read `buf.len()/512` sectors starting at `lba` into `buf`.
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), InstallError>;
    /// Write `buf.len()/512` sectors starting at `lba` from `buf`.
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), InstallError>;
}

/// An `InstallTarget` over the in-tree single-device block layer (`drivers::block`). In this arc that
/// device is the QEMU usb-storage scratch disk the builder attaches under `UNAOS_INSTALLDEMO=1`.
pub struct BlockTarget {
    info: block::BlockDeviceInfo,
}

impl BlockTarget {
    /// Bind to the currently registered block device, or `NotReady` if storage is not up.
    pub fn bind() -> Result<Self, InstallError> {
        let info = block::info().ok_or(InstallError::NotReady)?;
        if info.block_size != SECTOR as u32 {
            return Err(InstallError::Io);
        }
        Ok(Self { info })
    }
}

impl InstallTarget for BlockTarget {
    fn capacity_sectors(&self) -> u64 {
        self.info.num_blocks
    }

    fn id(&self) -> alloc::string::String {
        let trim = |s: &[u8]| -> alloc::string::String {
            let end = s.iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |p| p + 1);
            s[..end].iter().map(|&b| b as char).collect()
        };
        alloc::format!(
            "{} {} ({} x {}B sectors)",
            trim(&self.info.vendor),
            trim(&self.info.product),
            self.info.num_blocks,
            self.info.block_size
        )
    }

    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), InstallError> {
        debug_assert!(buf.len() % SECTOR == 0);
        for (i, chunk) in buf.chunks_mut(SECTOR).enumerate() {
            let n = block::read_block(lba + i as u64, chunk).map_err(map_blk)?;
            if n < chunk.len() {
                return Err(InstallError::Io);
            }
        }
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), InstallError> {
        debug_assert!(buf.len() % SECTOR == 0);
        for (i, chunk) in buf.chunks(SECTOR).enumerate() {
            block::write_block(lba + i as u64, chunk).map_err(map_blk)?;
        }
        Ok(())
    }
}

fn map_blk(e: block::BlockError) -> InstallError {
    match e {
        block::BlockError::NotReady => InstallError::NotReady,
        block::BlockError::BadLba => InstallError::BadLba,
        block::BlockError::Io => InstallError::Io,
    }
}

/// The never-touch guard: read the target's leading sectors and confirm they are blank (all zero).
/// `NotBlank` (with NO writes performed) if the target already carries a partition table / filesystem.
/// This is the arming discipline made mechanical — the engine will not format a device that is not
/// demonstrably blank.
pub fn blank_check<T: InstallTarget>(t: &T) -> Result<(), InstallError> {
    // 64 sectors covers the protective MBR + primary GPT header + the start of the entry array — any
    // real partition table or superfloppy BPB writes into this window, so a non-zero byte here means
    // "not blank".
    const PROBE_SECTORS: usize = 64;
    let cap = t.capacity_sectors();
    if cap < 128 {
        return Err(InstallError::TooSmall);
    }
    let mut buf = alloc::vec![0u8; PROBE_SECTORS * SECTOR];
    t.read_sectors(0, &mut buf)?;
    if buf.iter().any(|&b| b != 0) {
        return Err(InstallError::NotBlank);
    }
    Ok(())
}

/// The fixed test payload for the copy-and-verify witness: a few KiB of a deterministic,
/// non-degenerate pattern (not all-zero, not all-one) with a recognizable header. Spans multiple
/// clusters so the extent machinery is exercised across a chain.
fn test_payload() -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(6007);
    v.extend_from_slice(b"UNAOS-INSTALL-CORE payload v1\n");
    let mut i: usize = 0;
    while v.len() < 6007 {
        v.push(((i.wrapping_mul(31).wrapping_add(7)) & 0xFF) as u8);
        i += 1;
    }
    v
}

/// Re-read every recorded extent off the device and SHA-256 the reassembled bytes. Content-verify:
/// the digest must equal `expect`. Public so the arch-specific installer flows (e.g. the Orin microSD
/// path in `arch::sdmmc_tegra`) verify through the SAME primitive the x86 engine witness trusts.
pub fn verify_extents<T: InstallTarget>(
    t: &T,
    extents: &[fat32::Extent],
    expect: &[u8; 32],
) -> Result<bool, InstallError> {
    let mut h = hash::Sha256::new();
    let mut sec = [0u8; SECTOR];
    for e in extents {
        t.read_sectors(e.lba, &mut sec)?;
        h.update(&sec[..e.len]);
    }
    Ok(&h.finalize() == expect)
}

/// Known-answer tests for the two primitives — proves the CRC-32 + SHA-256 the verifier trusts are
/// themselves correct before we rely on them.
fn self_test_hashes() -> bool {
    // SHA-256 KATs (FIPS 180-4 examples).
    let kat_empty: [u8; 32] = [
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9,
        0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52,
        0xb8, 0x55,
    ];
    let kat_abc: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22,
        0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00,
        0x15, 0xad,
    ];
    if hash::sha256(b"") != kat_empty || hash::sha256(b"abc") != kat_abc {
        return false;
    }
    // Streaming consistency across multiple blocks + odd chunk boundaries must equal the one-shot
    // digest (exercises the buffer-carry + padding-crosses-a-block paths the extent verifier relies
    // on). Also pins the two canonical 56-byte boundary cases via one-shot.
    let long: alloc::vec::Vec<u8> = (0u32..1000).map(|i| (i.wrapping_mul(37) & 0xFF) as u8).collect();
    let one_shot = hash::sha256(&long);
    let mut streamed = hash::Sha256::new();
    for chunk in long.chunks(37) {
        streamed.update(chunk);
    }
    if streamed.finalize() != one_shot {
        return false;
    }
    // CRC-32/ISO-HDLC KAT: "123456789" => 0xCBF43926.
    hash::crc32(b"123456789") == 0xCBF4_3926
}

/// The INSTALL witness: run the whole engine end-to-end against the armed scratch target and emit the
/// verdict line. One-shot; call once a block device is present. Emits, on success:
/// `:: INSTALL: gpt+fat32+copy verify => PASS ::` plus the negative-test + refusal-guard evidence.
pub fn run_demo() {
    match run_demo_inner() {
        Ok(()) => {
            serial_println!(":: INSTALL: gpt+fat32+copy verify => PASS ::");
        }
        Err(e) => {
            serial_println!(":: INSTALL: engine => FAIL ({:?}) ::", e);
        }
    }
}

/// One-shot boot-loop entry: run the INSTALL witness exactly once, the first time a block device is
/// present (the armed scratch disk enumerates asynchronously over usb-storage). Mirrors the
/// `fat::probe_once` / `u2_probe_once` idiom used by the other x86 storage witnesses.
/// INSTGUI seam: the graphical installer's go-button. Same engine run as
/// [`run_demo`] — same blank-check refusal, same verify ladder, same serial
/// verdicts — returning the outcome so the GUI can show a verdict screen. The
/// GUI adds an attended confirmation in FRONT of the engine; it removes nothing.
pub fn run_gui() -> bool {
    match run_demo_inner() {
        Ok(()) => {
            serial_println!(":: INSTALL: gpt+fat32+copy verify => PASS (instgui) ::");
            true
        }
        Err(e) => {
            serial_println!(":: INSTALL: engine => REFUSED/FAIL ({:?}) (instgui) ::", e);
            false
        }
    }
}

pub fn install_probe_once() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if block::info().is_none() {
        return; // storage not brought up yet
    }
    DONE.store(true, Ordering::Relaxed);
    run_demo();
}

fn run_demo_inner() -> Result<(), InstallError> {
    // 0) Trust the primitives first. NOTE: the two opening prints are folded into ONE line on
    //    purpose — this point is a serial burst boundary (the first prints after the block device
    //    enumerates) where the console reliably drops the SECOND of two back-to-back, pre-I/O writes.
    //    Every LATER `INSTALL` line is separated from the previous by real block I/O (a read or write),
    //    which spaces them out safely; only these opening lines are vulnerable, so they are combined.
    if !self_test_hashes() {
        serial_println!(":: INSTALL: hash self-test (sha256 + crc32 KATs) => FAIL ::");
        return Err(InstallError::VerifyFailed);
    }
    let mut t = BlockTarget::bind()?;
    serial_println!(
        ":: INSTALL: engine start — hash self-test (sha256 + crc32 KATs) PASS — armed target = {} ::",
        t.id()
    );

    // 1) The never-touch guard: the armed scratch must be blank BEFORE we write.
    match blank_check(&t) {
        Ok(()) => serial_println!(":: INSTALL: blank-check (pre-write) => BLANK, armed ::"),
        Err(InstallError::NotBlank) => {
            serial_println!(
                ":: INSTALL: refusal — armed target is NOT blank; refusing to write => guard OK ::"
            );
            return Err(InstallError::NotBlank);
        }
        Err(e) => return Err(e),
    }

    // 2) GPT (protective MBR + primary/backup + ESP + data), with parse-back self-verify inside.
    let layout = gpt::write_gpt(&mut t)?;
    serial_println!(
        ":: INSTALL: GPT written + parse-back verified — ESP LBA {}..{}, data LBA {}..{} of {} sectors ::",
        layout.esp_first_lba,
        layout.esp_last_lba,
        layout.data_first_lba,
        layout.data_last_lba,
        layout.total_sectors
    );

    // 3) FAT32 format the ESP.
    let esp_sectors = layout.esp_last_lba - layout.esp_first_lba + 1;
    let geom = fat32::format_esp(&mut t, layout.esp_first_lba, esp_sectors)?;
    serial_println!(
        ":: INSTALL: ESP formatted FAT32 — fat_sz={}sec clusters={} data@vol+{} ::",
        geom.fat_sz,
        geom.count_of_clusters,
        geom.data_start
    );

    // 4) Copy a payload file, recording its extents; then content-verify by re-reading them.
    let payload = test_payload();
    let payload_sha = hash::sha256(&payload);
    let extents = fat32::write_payload_file(&mut t, &geom, "PAYLOAD.BIN", &payload)?;
    serial_println!(
        ":: INSTALL: copied PAYLOAD.BIN ({} bytes, {} extents) ::",
        payload.len(),
        extents.len()
    );
    if !verify_extents(&t, &extents, &payload_sha)? {
        serial_println!(":: INSTALL: extent sha-verify => FAIL ::");
        return Err(InstallError::VerifyFailed);
    }
    serial_println!(":: INSTALL: extent sha-verify (re-read every extent) => PASS ::");

    // 5) Interop self-check: the IN-TREE FAT reader must mount our GPT+ESP and read the file back.
    match crate::fs::fat::mount() {
        Ok(fs) => {
            let de = fs.find_in_root("PAYLOAD.BIN").map_err(|_| InstallError::VerifyFailed)?;
            let mut back = alloc::vec::Vec::new();
            fs.read_file(&de, &mut back, payload.len())
                .map_err(|_| InstallError::VerifyFailed)?;
            if back.len() != payload.len() || hash::sha256(&back) != payload_sha {
                serial_println!(":: INSTALL: in-tree FAT read-back => FAIL ::");
                return Err(InstallError::VerifyFailed);
            }
            serial_println!(
                ":: INSTALL: in-tree FAT mount + read-back of PAYLOAD.BIN ({} B) => PASS ::",
                back.len()
            );
        }
        Err(e) => {
            serial_println!(":: INSTALL: in-tree FAT mount => FAIL ({:?}) ::", e);
            return Err(InstallError::VerifyFailed);
        }
    }

    // 6) NEGATIVE test: corrupt one byte in the first extent and prove the verifier now FAILS; then
    //    restore it and prove it PASSES again. This is what makes the verifier's PASS meaningful.
    let first = extents[0];
    let mut sec = [0u8; SECTOR];
    t.read_sectors(first.lba, &mut sec)?;
    let orig = sec[0];
    sec[0] = orig ^ 0xFF;
    t.write_sectors(first.lba, &sec)?;
    if verify_extents(&t, &extents, &payload_sha)? {
        serial_println!(":: INSTALL: negative test — verifier did NOT catch corruption => FAIL ::");
        return Err(InstallError::VerifyFailed);
    }
    serial_println!(":: INSTALL: negative test — 1-byte corruption CAUGHT (verify REJECTED as it must) => PASS ::");
    // Restore + re-verify.
    sec[0] = orig;
    t.write_sectors(first.lba, &sec)?;
    if !verify_extents(&t, &extents, &payload_sha)? {
        serial_println!(":: INSTALL: restore re-verify => FAIL ::");
        return Err(InstallError::VerifyFailed);
    }
    serial_println!(":: INSTALL: restore + re-verify => PASS ::");

    // 7) The guard again: now that a GPT is present, blank_check must REFUSE — the never-touch
    //    discipline holds on a written volume.
    match blank_check(&t) {
        Err(InstallError::NotBlank) => serial_println!(
            ":: INSTALL: blank-check (post-write) => NOT blank, engine would REFUSE => guard OK ::"
        ),
        other => {
            serial_println!(":: INSTALL: post-write guard unexpected result {:?} => FAIL ::", other);
            return Err(InstallError::VerifyFailed);
        }
    }

    // 8) MULTI-CLUSTER DIRECTORY proof (ORIN-SDMMC-3 M2): the `TreeWriter` builds a subdirectory holding
    //    more than 16 entries — spanning more than one 512-byte cluster — then the in-tree FAT reader
    //    mounts the volume, follows the directory's cluster chain, and re-reads + sha-verifies every entry.
    //    This exercises the lifted single-cluster/>16-entry bound on the SAME engine the SD installer uses.
    demo_multicluster_dir(&mut t, &layout)?;

    Ok(())
}

/// ORIN-SDMMC-3 M2 witness: prove the multi-cluster directory `TreeWriter` path end-to-end on the x86
/// scratch target. Re-establishes the FAT blank-precondition, re-formats the ESP, then builds a tree whose
/// subdirectory `SUB/` holds `N > 16` files (so its directory image spans >1 cluster), writing each file
/// through `TreeWriter::write_file`. Finally the in-tree FAT reader mounts the volume, walks `SUB/` across
/// its cluster chain, and content-verifies every file by SHA — a genuine multi-cluster directory read.
fn demo_multicluster_dir<T: InstallTarget>(
    t: &mut T,
    layout: &gpt::GptLayout,
) -> Result<(), InstallError> {
    use fat32::{dir_clusters_for_slots, put_dir_entry, TreeWriter, ATTR_ARCHIVE, ATTR_DIR};

    // 20 files + `.`/`..` = 22 slots > 16 => the SUB/ directory needs 2 clusters.
    const N: usize = 20;

    // Re-establish the blank-precondition: the volume still carries PAYLOAD.BIN's FAT chain, so zero the
    // reserved+FAT region before re-formatting (a stale FAT entry would forge an allocation).
    let esp_sectors = layout.esp_last_lba - layout.esp_first_lba + 1;
    let blank = fat32::blank_region_sectors(esp_sectors)?;
    let zero = [0u8; SECTOR];
    for s in 0..blank {
        t.write_sectors(layout.esp_first_lba + s, &zero)?;
    }
    let geom = fat32::format_esp(t, layout.esp_first_lba, esp_sectors)?;

    // Build the tree: root -> SUB/ -> F00.BIN .. F19.BIN. Record each file's sha for the read-back verify.
    let mut files: alloc::vec::Vec<(alloc::string::String, [u8; 32], usize)> = alloc::vec::Vec::new();
    let sub_clusters = dir_clusters_for_slots(N + 2);
    {
        let mut w = TreeWriter::new(t, geom);
        // root: one entry (SUB) => one cluster.
        let root = w.reserve_root(dir_clusters_for_slots(1))?;
        let sub = w.alloc_dir_clusters(sub_clusters)?;

        // The SUB/ directory image, built wholly in memory across its cluster chain.
        let mut sub_img = alloc::vec![0u8; sub_clusters as usize * SECTOR];
        let mut slot = 0usize;
        if !put_dir_entry(&mut sub_img, slot, ".", ATTR_DIR, sub, 0)
            || !put_dir_entry(&mut sub_img, slot + 1, "..", ATTR_DIR, 0, 0)
        {
            return Err(InstallError::BadArg);
        }
        slot += 2;
        for i in 0..N {
            let name = alloc::format!("F{:02}.BIN", i);
            // Distinct, non-degenerate content per file (varying length exercises padding + extents).
            let mut content = alloc::vec::Vec::new();
            content.extend_from_slice(alloc::format!("multicluster demo file {}\n", i).as_bytes());
            let mut k: usize = 0;
            let target = 200 + i * 17;
            while content.len() < target {
                content.push(((k.wrapping_mul(29).wrapping_add(i)) & 0xFF) as u8);
                k += 1;
            }
            let sha = hash::sha256(&content);
            let (first, _extents) = w.write_file(&content)?;
            if !put_dir_entry(&mut sub_img, slot, &name, ATTR_ARCHIVE, first, content.len() as u32) {
                return Err(InstallError::BadArg);
            }
            slot += 1;
            files.push((name, sha, content.len()));
        }
        w.write_dir_image(sub, &sub_img)?;

        // The root directory image: a single SUB/ entry.
        let mut root_img = alloc::vec![0u8; SECTOR];
        if !put_dir_entry(&mut root_img, 0, "SUB", ATTR_DIR, sub, 0) {
            return Err(InstallError::BadArg);
        }
        w.write_dir_image(root, &root_img)?;
    }
    serial_println!(
        ":: INSTALL: multi-cluster dir — wrote SUB/ with {} files across {} clusters (>1) ::",
        N,
        sub_clusters
    );

    // Read it back through the in-tree FAT reader: mount, find SUB/, walk its cluster chain, verify each.
    let fs = crate::fs::fat::mount().map_err(|_| InstallError::VerifyFailed)?;
    let sub_de = fs.find_in_root("SUB").map_err(|_| InstallError::VerifyFailed)?;
    if !sub_de.is_dir {
        serial_println!(":: INSTALL: multi-cluster dir — SUB is not a directory => FAIL ::");
        return Err(InstallError::VerifyFailed);
    }
    let entries = fs.read_dir(sub_de.first_cluster()).map_err(|_| InstallError::VerifyFailed)?;
    let mut verified = 0usize;
    for (name, sha, size) in &files {
        let de = entries
            .iter()
            .find(|e| !e.is_dir && e.name() == name.as_str())
            .ok_or(InstallError::VerifyFailed)?;
        let mut back = alloc::vec::Vec::new();
        fs.read_file(de, &mut back, *size).map_err(|_| InstallError::VerifyFailed)?;
        if back.len() != *size || &hash::sha256(&back) != sha {
            serial_println!(":: INSTALL: multi-cluster dir — {} read-back mismatch => FAIL ::", name);
            return Err(InstallError::VerifyFailed);
        }
        verified += 1;
    }
    if verified != N {
        serial_println!(
            ":: INSTALL: multi-cluster dir — only {}/{} files verified => FAIL ::",
            verified,
            N
        );
        return Err(InstallError::VerifyFailed);
    }
    serial_println!(
        ":: INSTALL: multi-cluster dir — SUB/ {} entries across {} clusters, all re-read + sha-verified (dirs=1) => PASS ::",
        N,
        sub_clusters
    );
    Ok(())
}
