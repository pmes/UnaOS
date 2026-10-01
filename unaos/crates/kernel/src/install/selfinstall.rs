// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFINSTALL (ROADMAP §1c rung SH-3) — the card UnaOS booted from clones itself onto the rMBP's
// INTERNAL SATA SSD without a host PC, and the machine then boots from the SSD.
//
//   install ssd --dry-run   M1: enumerate the AHCI disk, classify it (blank | ours | stranger), print
//                           the plan, write NOTHING.
//   install ssd --write     M2: on blank|ours ONLY (and a build with `ahci-write`): lay a GPT with one
//                           512 MiB ESP and mirror the running card's volume onto it, sha-verifying
//                           every file off the SSD. A stranger verdict REFUSES and says what it saw (R20).
//
// The write path is the engine's own, unforked: `gpt::write_gpt_sized` -> `partition::write_partition`
// (zero FAT metadata, `format_esp`, `clone::write_snapshot`, `verify_extents`). What is new is the
// whole-disk capability: [`AhciDisk`] is an `InstallTarget` whose writes go through
// `block::write_sectors_granted` with a `WriteGrant` that `partition::mint_disk_grant` only produces
// from a [`Verdict`] of Blank/Ours. `install/mod.rs`'s plain `BlockTarget` Ahci write arm is untouched
// and still refuses in every cfg (B91).

use super::{partition, InstallError, InstallTarget};
use crate::drivers::block::{self, BlockDeviceId, BlockHandle};
use crate::fs::fat::{self, BlockSource};
use alloc::string::String;

/// The ESP the self-install lays: 512 MiB of 512-byte sectors.
pub const ESP_SECTORS: u64 = 512 * 1024 * 1024 / 512;
/// The x86 heap is 256 MiB; the card's volume (kernel + SRC.TGZ + blobs) is buffered whole.
const SNAP_MAX_FILE: usize = 128 * 1024 * 1024;
const SNAP_MAX_TOTAL: usize = 160 * 1024 * 1024;

/// What the self-guard says about the disk. The write grant is minted from this and nothing else.
pub enum Verdict {
    /// Zero head (first 64 sectors), or a valid GPT with no partitions: nothing to lose.
    Blank,
    /// A GPT whose every partition is an ESP carrying UnaOS (EFI/BOOT/BOOTX64.EFI + kernel.elf).
    Ours,
    /// Anything else — somebody's OS, an unreadable table over data, a foreign volume. Carries what was seen.
    Stranger(String),
}

impl Verdict {
    pub fn tag(&self) -> &'static str {
        match self {
            Verdict::Blank => "blank",
            Verdict::Ours => "ours",
            Verdict::Stranger(_) => "stranger",
        }
    }
    pub fn writable(&self) -> bool {
        !matches!(self, Verdict::Stranger(_))
    }
}

/// Everything the dry run prints.
pub struct Probe {
    pub id: BlockDeviceId,
    pub port: u8,
    pub model: String,
    pub sectors: u64,
    pub gpt: &'static str,
    pub parts: usize,
    pub verdict: Verdict,
    pub saw: String,
}

fn first_ahci() -> Option<(u8, BlockDeviceId)> {
    for ix in 0..block::MAX_AHCI_DISKS {
        if let Some(d) = block::ahci_disk(ix) {
            return Some((d.port, d.info.id(BlockHandle::Ahci { port: d.port })));
        }
    }
    None
}

fn has(entries: &[crate::fs::fat::DirEntry], name: &str, dir: bool) -> bool {
    entries.iter().any(|e| e.is_dir == dir && e.name().eq_ignore_ascii_case(name))
}

/// Does the FAT volume on this AHCI port carry UnaOS (the card's ESP shape)?
fn port_carries_unaos(port: u8) -> bool {
    let Ok(fs) = fat::mount_source(BlockSource::Ahci(port)) else { return false };
    let Ok(root) = fs.read_root() else { return false };
    if !has(&root, "kernel.elf", false) {
        return false;
    }
    let Some(efi) = root.iter().find(|e| e.is_dir && e.name().eq_ignore_ascii_case("EFI")) else { return false };
    let Ok(efi_rows) = fs.read_dir(efi.first_cluster()) else { return false };
    let Some(boot) = efi_rows.iter().find(|e| e.is_dir && e.name().eq_ignore_ascii_case("BOOT")) else { return false };
    let Ok(boot_rows) = fs.read_dir(boot.first_cluster()) else { return false };
    has(&boot_rows, "BOOTX64.EFI", false)
}

/// M1 core: READ-ONLY classification of the AHCI disk. Writes nothing, mints nothing.
pub fn probe(sel: BlockDeviceId, port: u8) -> Result<Probe, InstallError> {
    let t = super::BlockTarget::bind_id(sel)?;
    let (v, p) = t.vendor_product();
    let model: String = alloc::format!("{} {}", v, p).trim().into();
    let sectors = t.capacity_sectors();
    let mut head = alloc::vec![0u8; 64 * 512];
    t.read_sectors(0, &mut head)?;
    let zero_head = head.iter().all(|&b| b == 0);
    let mk = |gpt: &'static str, parts: usize, verdict: Verdict, saw: String| Probe {
        id: sel, port, model: model.clone(), sectors, gpt, parts, verdict, saw,
    };
    let table = match super::gpt::read_table(&t) {
        Ok(tb) => tb,
        Err(_) if zero_head => return Ok(mk("none", 0, Verdict::Blank, String::from("first 64 sectors all zero"))),
        Err(_) => {
            return Ok(mk(
                "unreadable",
                0,
                Verdict::Stranger(String::from("no readable GPT over a non-zero head (MBR/superfloppy/foreign table)")),
                String::from("non-zero head, no GPT"),
            ))
        }
    };
    let c = partition::census(&t)?;
    let mut saw = String::new();
    for r in &c.rows {
        if !saw.is_empty() {
            saw.push(',');
        }
        saw.push_str(&alloc::format!("part{}:{}", r.entry.index, r.content.tag()));
    }
    if table.entries.is_empty() {
        return Ok(mk("valid", 0, Verdict::Blank, String::from("valid GPT, zero partitions")));
    }
    if saw.is_empty() {
        saw.push('-');
    }
    let n = c.rows.len();
    if c.foreign > 0 || c.friend > 0 {
        let why = alloc::format!("foreign={} friend={} [{}] — a stranger's OS lives here", c.foreign, c.friend, saw);
        return Ok(mk("valid", n, Verdict::Stranger(why), saw));
    }
    let all_esp = c.rows.iter().all(|r| r.entry.is_esp());
    if all_esp && port_carries_unaos(port) {
        return Ok(mk("valid", n, Verdict::Ours, saw));
    }
    let why = alloc::format!("partitions [{}] but no UnaOS ESP (EFI/BOOT/BOOTX64.EFI + kernel.elf) was found", saw);
    Ok(mk("valid", n, Verdict::Stranger(why), saw))
}

/// Count files and bytes of the card's volume without reading any data (for the plan line).
fn tree_size(fs: &fat::FatFs, rows: &[crate::fs::fat::DirEntry], depth: u32, files: &mut usize, bytes: &mut u64) {
    if depth > 6 {
        return;
    }
    for e in rows {
        let n = e.name();
        if n == "." || n == ".." {
            continue;
        }
        if e.is_dir {
            if let Ok(sub) = fs.read_dir(e.first_cluster()) {
                tree_size(fs, &sub, depth + 1, files, bytes);
            }
        } else {
            *files += 1;
            *bytes += e.size as u64;
        }
    }
}

/// Where the clone would come from: the running card's volume, found by CONTENT (bootdisk).
fn running_source() -> Option<BlockSource> {
    match crate::fs::bootdisk::locate() {
        crate::fs::bootdisk::Verdict::Bound(f) => Some(f.source),
        _ => None,
    }
}

fn source_is_ahci(s: BlockSource) -> bool {
    matches!(s, BlockSource::Ahci(_))
}

fn plan_text(p: &Probe) -> String {
    if !p.verdict.writable() {
        return String::from("REFUSE(stranger: R20, no destructive work on a disk that is not ours)");
    }
    let Some(src) = running_source() else { return String::from("NO-SOURCE(running volume not found)") };
    if source_is_ahci(src) {
        return String::from("NO-SOURCE(already running from the SSD)");
    }
    let (mut files, mut bytes) = (0usize, 0u64);
    if let Ok(fs) = fat::mount_source(src) {
        if let Ok(rows) = fs.read_root() {
            tree_size(&fs, &rows, 0, &mut files, &mut bytes);
        }
    }
    alloc::format!("gpt+esp512M+clone(src={} files={} bytes={}) then --write", src.name(), files, bytes)
}

/// M1: print the `[install] target=…` plan line. Returns the probe so a fixture can assert on it.
pub fn dry_run(out: &mut dyn FnMut(&str)) -> Option<Probe> {
    let Some((port, sel)) = first_ahci() else {
        serial_println!("[install] target=ahci:- no AHCI disk is registered (build needs UNAOS_AHCI=1 and a SATA disk) plan=none");
        out("install ssd: no AHCI disk registered");
        return None;
    };
    match probe(sel, port) {
        Err(e) => {
            serial_println!("[install] target=ahci:{} probe err={:?} verdict=stranger plan=REFUSE(unreadable: nothing is claimed about it)", port, e);
            out("install ssd: cannot read the disk; refusing");
            None
        }
        Ok(p) => {
            let plan = plan_text(&p);
            serial_println!(
                "[install] target=ahci:{} model={} sectors={} gpt={} verdict={} plan={}",
                p.port, p.model, p.sectors, p.gpt, p.verdict.tag(), plan
            );
            if let Verdict::Stranger(why) = &p.verdict {
                serial_println!("[install] saw: {}", why);
            }
            out(&alloc::format!(
                "install ssd (dry run, nothing written): ahci:{} {} sectors={} gpt={} verdict={}",
                p.port, p.model, p.sectors, p.gpt, p.verdict.tag()
            ));
            out(&alloc::format!("  plan: {}", plan));
            #[cfg(feature = "unafs")] { serial_println!("[install] unafs-root mirror=NOT-YET (UNAFSX86 owed: --write copies the ESP files only; the SSD gets no UnaFS partition and boots with a FAT root)"); out("  unafs: the UnaFS root partition is NOT mirrored yet — the installed SSD boots with a FAT root"); }
            Some(p)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// M2 — the write.
// ---------------------------------------------------------------------------------------------

/// The whole-disk `InstallTarget` for the WRITE path: reads through the (read-open) `BlockTarget`,
/// writes ONLY through a `WriteGrant` minted from a Blank/Ours verdict.
#[cfg(feature = "ahci-write")]
pub struct AhciDisk {
    rd: super::BlockTarget,
    grant: block::WriteGrant,
}

#[cfg(feature = "ahci-write")]
impl AhciDisk {
    pub fn new(sel: BlockDeviceId, v: &Verdict) -> Result<Self, InstallError> {
        let rd = super::BlockTarget::bind_id(sel)?;
        let grant = partition::mint_disk_grant(sel, rd.capacity_sectors(), v).ok_or(InstallError::NotBlank)?;
        Ok(Self { rd, grant })
    }
}

#[cfg(feature = "ahci-write")]
impl InstallTarget for AhciDisk {
    fn capacity_sectors(&self) -> u64 {
        self.rd.capacity_sectors()
    }
    fn id(&self) -> String {
        self.rd.id()
    }
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), InstallError> {
        self.rd.read_sectors(lba, buf)
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), InstallError> {
        block::write_sectors_granted(&self.grant, lba, buf).map_err(|_| InstallError::Io)
    }
}

#[cfg(feature = "ahci-write")]
fn tree_has_boot_files(tree: &super::clone::SnapTree) -> bool {
    let kernel = tree.root.files.iter().any(|f| f.name.eq_ignore_ascii_case("kernel.elf"));
    let efi = tree.root.subdirs.iter().find(|(n, _)| n.eq_ignore_ascii_case("EFI"));
    let boot = efi.and_then(|(_, d)| d.subdirs.iter().find(|(n, _)| n.eq_ignore_ascii_case("BOOT")));
    let loader = boot.map_or(false, |(_, d)| d.files.iter().any(|f| f.name.eq_ignore_ascii_case("BOOTX64.EFI")));
    kernel && loader
}

#[cfg(feature = "ahci-write")]
fn fail(out: &mut dyn FnMut(&str), why: &str) {
    serial_println!(":: SELFINSTALL: files=0 bytes=0 ms=0 verified=0 reason={} -> FAIL ::", why);
    out(&alloc::format!("install ssd --write: {}", why));
}

/// M2: `install ssd --write`.
#[cfg(feature = "ahci-write")]
pub fn write_ssd(out: &mut dyn FnMut(&str)) {
    let Some((port, sel)) = first_ahci() else {
        out("install ssd: no AHCI disk registered");
        return;
    };
    let p = match probe(sel, port) {
        Ok(p) => p,
        Err(e) => {
            serial_println!("[install] target=ahci:{} probe err={:?} — nothing written", port, e);
            out("install ssd --write: cannot read the disk; nothing written");
            return;
        }
    };
    serial_println!(
        "[install] target=ahci:{} model={} sectors={} gpt={} verdict={} saw={}",
        p.port, p.model, p.sectors, p.gpt, p.verdict.tag(), p.saw
    );
    if let Verdict::Stranger(why) = &p.verdict {
        // R20: no destructive work on a disk that is not ours.
        serial_println!("[install] REFUSED target=ahci:{} verdict=stranger saw: {} — nothing was written (R20) ::", port, why);
        out(&alloc::format!("install ssd --write: REFUSED — this disk is not ours ({}). Nothing was written.", why));
        return;
    }
    let t0 = crate::arch::ticks();
    // Source: the running card's volume, buffered whole BEFORE any destructive write.
    let Some(src_id) = running_source() else { return fail(out, "running volume not found") };
    if source_is_ahci(src_id) {
        return fail(out, "already running from the SSD; boot the card to clone");
    }
    let tree = match fat::mount_source(src_id)
        .map_err(|_| InstallError::Io)
        .and_then(|fs| super::clone::snapshot_capped(&fs, SNAP_MAX_FILE, SNAP_MAX_TOTAL))
    {
        Ok(t) => t,
        Err(e) => {
            serial_println!("[install] snapshot of {} failed err={:?}", src_id.name(), e);
            return fail(out, "snapshot of the card volume failed");
        }
    };
    if !tree_has_boot_files(&tree) {
        return fail(out, "the card volume lacks EFI/BOOT/BOOTX64.EFI or kernel.elf; not a bootable UnaOS volume");
    }
    serial_println!("[install] snapshot src={} files={} bytes={}", src_id.name(), tree.file_count, tree.total_bytes);
    let mut disk = match AhciDisk::new(sel, &p.verdict) {
        Ok(d) => d,
        Err(e) => {
            serial_println!("[install] write grant refused err={:?} (boot-device or too small) — nothing written", e);
            return fail(out, "write grant refused (boot device, or disk too small)");
        }
    };
    let layout = match super::gpt::write_gpt_sized(&mut disk, ESP_SECTORS, false) {
        Ok(l) => l,
        Err(e) => {
            serial_println!("[install] GPT write failed err={:?}", e);
            return fail(out, "GPT write/verify failed");
        }
    };
    serial_println!(
        "[install] gpt written esp={}..{} sectors={}",
        layout.esp_first_lba, layout.esp_last_lba, layout.esp_last_lba - layout.esp_first_lba + 1
    );
    let entry = super::gpt::GptEntryView {
        index: 0,
        type_guid: super::gpt::ESP_TYPE_GUID,
        first_lba: layout.esp_first_lba,
        last_lba: layout.esp_last_lba,
    };
    let w = match partition::write_partition(&mut disk, &entry, &tree, partition::no_grant()) {
        Ok(w) => w,
        Err(e) => {
            serial_println!("[install] ESP write failed err={:?}", e);
            return fail(out, "ESP format/clone failed");
        }
    };
    let ms = crate::arch::ticks().wrapping_sub(t0);
    let pass = w.files > 0 && w.verified == w.files;
    serial_println!(
        ":: SELFINSTALL: files={} bytes={} ms={} verified={} -> {} ::",
        w.files, w.bytes, ms, w.verified, if pass { "PASS" } else { "FAIL" }
    );
    out(&alloc::format!(
        "install ssd --write: {} files, {} bytes, verified {}/{}{}",
        w.files, w.bytes, w.verified, w.files,
        if pass { " — the SSD now carries UnaOS; reboot and pick it" } else { " — NOT trustworthy" }
    ));
}

#[cfg(not(feature = "ahci-write"))]
pub fn write_ssd(out: &mut dyn FnMut(&str)) {
    serial_println!("[install] --write REFUSED reason=transport-write-disabled transport=ahci knob=UNAOS_AHCI_WRITE");
    out("install ssd --write: this build has no SATA write path (rebuild with UNAOS_AHCI_WRITE=1)");
}

/// The shell arm: `install ssd [--dry-run|--write]` (`args` is what follows `ssd`).
pub fn verb(out: &mut dyn FnMut(&str), args: &[&str]) {
    match args.first().copied() {
        Some("--dry-run") if args.len() == 1 => {
            let _ = dry_run(out);
        }
        Some("--write") if args.len() == 1 => write_ssd(out),
        _ => out("usage: install ssd --dry-run | install ssd --write   (the SSD install; --write needs UNAOS_AHCI_WRITE=1 and refuses a stranger's disk)"),
    }
}

/// `tests selfinstall` = the dry run. Prints the plan on the lane's AHCI disk, or SKIPs without one.
pub fn selftest() {
    let mut sink = |_s: &str| {};
    match dry_run(&mut sink) {
        None => serial_println!(":: SELFINSTALL: dry-run disks=0 -> SKIP (no AHCI disk on this lane) ::"),
        Some(p) => serial_println!(
            ":: SELFINSTALL: dry-run target=ahci:{} verdict={} gpt={} parts={} -> PASS ::",
            p.port, p.verdict.tag(), p.gpt, p.parts
        ),
    }
}
