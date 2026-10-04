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
    /// SELFINSTALL2: a UnaFS volume (by superblock magic) already lives on the target — `--write`
    /// then needs `--force`.
    pub unafs_present: bool,
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
        id: sel, port, model: model.clone(), sectors, gpt, parts, verdict, saw, unafs_present: false,
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
    // SELFINSTALL2: what a previous `install ssd --write` leaves — an ESP-typed FAT volume carrying
    // UnaOS plus (now) a UnaFS volume — is OURS, not a stranger. Before this the census counted our own
    // FAT ESP as `foreign` (FAT is somebody's filesystem in R25's model) and a UnaFS volume as `friend`,
    // so a re-install over our own SSD could never reach the Ours verdict. A foreign TYPE GUID anywhere,
    // a non-ESP FAT, or any unknown content still makes the whole disk a stranger.
    let unafs_present = c.rows.iter().any(|r| r.content == partition::Content::UnaFs);
    let ours_shaped = |r: &partition::CensusRow| {
        partition::foreign_type(&r.entry.type_guid).is_none()
            && ((r.entry.is_esp() && matches!(r.content, partition::Content::Fat | partition::Content::Esp))
                || r.content == partition::Content::UnaFs)
    };
    if c.rows.iter().all(ours_shaped) && c.rows.iter().any(|r| r.entry.is_esp()) && port_carries_unaos(port) {
        let mut pr = mk("valid", n, Verdict::Ours, saw);
        pr.unafs_present = unafs_present;
        return Ok(pr);
    }
    if c.foreign > 0 || c.friend > 0 {
        let why = alloc::format!("foreign={} friend={} [{}] — a stranger's OS lives here", c.foreign, c.friend, saw);
        let mut pr = mk("valid", n, Verdict::Stranger(why), saw);
        pr.unafs_present = unafs_present;
        return Ok(pr);
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
            print_plan(&p, out);
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
pub fn write_ssd(out: &mut dyn FnMut(&str), force: bool) {
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
    // SELFINSTALL2: the boot disk itself is refused here by name (selfguard), before the grant's own
    // refusal one step later, and an existing UnaFS volume is not overwritten without `--force`.
    if super::selfguard::refuses(sel) {
        serial_println!("[install] REFUSED target=ahci:{} reason=boot-device (selfguard) — nothing written ::", port);
        out("install ssd --write: REFUSED — this disk is the one the system booted from. Nothing was written.");
        return;
    }
    if p.unafs_present && !force {
        serial_println!("[install] REFUSED target=ahci:{} reason=unafs-present (a UnaFS volume is already on the target; --force replaces it) — nothing written ::", port);
        out("install ssd --write: REFUSED — the SSD already carries a UnaFS volume; `install ssd --write --force` replaces it. Nothing was written.");
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
    // SELFINSTALL2: the table comes from the shared core's Plan — ESP + (when the running system has
    // one) a UnaFS partition the size of the running volume.
    #[cfg(feature = "unafs")] let unafs_src = super::unafsmirror::source().filter(|s| !s.is_ahci()); #[cfg(not(feature = "unafs"))] let unafs_src: Option<()> = None;
    #[cfg(feature = "unafs")] let unafs_sectors = unafs_src.map(|s| s.sectors()); #[cfg(not(feature = "unafs"))] let unafs_sectors: Option<u64> = None;
    let plan = match ssd_plan(p.sectors, unafs_sectors) {
        Ok(pl) => pl,
        Err(e) => {
            serial_println!("[install] plan refused: {}", e);
            return fail(out, "the SSD is too small for the plan");
        }
    };
    for line in plan.lines() {
        serial_println!("[install] {}", line);
    }
    if let Err(e) = super::gpt::write_plan(&mut disk, &plan) {
        serial_println!("[install] GPT write failed err={:?}", e);
        return fail(out, "GPT write/verify failed");
    }
    let esp_part = plan.part(amber_core::PartKind::Esp).expect("plan carries an ESP");
    let layout = super::gpt::GptLayout {
        esp_first_lba: esp_part.first,
        esp_last_lba: esp_part.last,
        data_first_lba: 0,
        data_last_lba: 0,
        total_sectors: plan.disk_sectors,
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
    // SELFINSTALL2 M2: mirror the UnaFS volume (sector clone of the running span) and fsck the copy.
    #[cfg(feature = "unafs")] let (cloned_mib, fsck_tag, unafs_ok) = mirror_unafs(&mut disk, &plan, unafs_src, out); #[cfg(not(feature = "unafs"))] let (cloned_mib, fsck_tag, unafs_ok) = { let _ = unafs_src; (0u64, "skip", true) };
    let ms = crate::arch::ticks().wrapping_sub(t0);
    let pass = w.files > 0 && w.verified == w.files && unafs_ok;
    serial_println!(
        ":: SELFINSTALL: files={} bytes={} ms={} verified={} -> {} ::",
        w.files, w.bytes, ms, w.verified, if pass { "PASS" } else { "FAIL" }
    );
    witness(&plan, cloned_mib, fsck_tag, if pass { "PASS" } else { "FAIL" });
    out(&alloc::format!(
        "install ssd --write: {} files, {} bytes, verified {}/{}{}",
        w.files, w.bytes, w.verified, w.files,
        if pass { " — the SSD now carries UnaOS; reboot and pick it" } else { " — NOT trustworthy" }
    ));
}

#[cfg(not(feature = "ahci-write"))]
pub fn write_ssd(out: &mut dyn FnMut(&str), _force: bool) {
    serial_println!("[install] --write REFUSED reason=transport-write-disabled transport=ahci knob=UNAOS_AHCI_WRITE");
    out("install ssd --write: this build has no SATA write path (rebuild with UNAOS_AHCI_WRITE=1)");
}

/// The shell arm: `install ssd [--dry-run|--write]` (`args` is what follows `ssd`).
pub fn verb(out: &mut dyn FnMut(&str), args: &[&str]) {
    match args.first().copied() {
        Some("--dry-run") if args.len() == 1 => {
            let _ = dry_run(out);
        }
        Some("--write") if args.len() == 1 => write_ssd(out, false),
        Some("--write") if args.len() == 2 && args[1] == "--force" => write_ssd(out, true),
        _ => out("usage: install ssd --dry-run | install ssd --write [--force]   (the SSD install: ESP + UnaFS; --write needs UNAOS_AHCI_WRITE=1, refuses a stranger's disk, and needs --force over an existing UnaFS volume)"),
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

// ---------------------------------------------------------------------------------------------
// SELFINSTALL2 (rmbp-ledger B310) — the two-partition plan from the shared core, the dry run's plan
// print, the UnaFS mirror step, and the witness. File tail: nothing above moves.
// ---------------------------------------------------------------------------------------------

/// The SSD plan: ESP (512 MiB) and, when the running system has a UnaFS volume, a UnaFS partition of
/// exactly that volume's size — laid by `amber_core::Plan`, the same planner `tools/una-card` uses.
pub fn ssd_plan(disk_sectors: u64, unafs_sectors: Option<u64>) -> Result<amber_core::Plan, amber_core::PlanError> {
    use amber_core::{PartKind, PartReq, Size};
    let esp = PartReq { kind: PartKind::Esp, size: Size::Sectors(ESP_SECTORS), name: "UNAOS-ESP", seed: b"UNAOS-INSTALL-ESP" };
    match unafs_sectors {
        Some(n) if n > 0 => {
            let ufs = PartReq { kind: PartKind::UnaFS, size: Size::Sectors(n), name: "UNAOS-UNAFS", seed: b"UNAOS-INSTALL-UFS" };
            amber_core::Plan::layout(disk_sectors, super::gpt::INSTALL_DISK_SEED, &[esp, ufs])
        }
        _ => amber_core::Plan::layout(disk_sectors, super::gpt::INSTALL_DISK_SEED, &[esp]),
    }
}

/// The root the NEXT boot picks when the card and the SSD both carry this kernel (bootdisk `pick_root`).
fn boot_pick() -> &'static str {
    if cfg!(feature = "root-prefer-ahci") { "ahci" } else { "sdhc" }
}

/// `:: SELFINSTALL2: plan=<n parts> esp=<MiB> unafs=<MiB> cloned=<MiB> fsck=<ok|skip|fail> boot_pick=<ahci|sdhc> -> PASS|DRY|FAIL ::`
fn witness(plan: &amber_core::Plan, cloned_mib: u64, fsck: &str, verdict: &str) {
    let mib = |k| plan.part(k).map_or(0, |p: &amber_core::Part| amber_core::sectors_mib(p.sectors()));
    serial_println!(
        ":: SELFINSTALL2: plan={} esp={} unafs={} cloned={} fsck={} boot_pick={} -> {} ::",
        plan.parts.len(), mib(amber_core::PartKind::Esp), mib(amber_core::PartKind::UnaFS), cloned_mib, fsck, boot_pick(), verdict
    );
}

/// The plan the dry run (and the installer window) shows for the first AHCI disk, as display lines.
/// `None` when there is no AHCI disk, the probe fails, or the disk cannot hold the plan.
pub fn plan_lines() -> Option<alloc::vec::Vec<String>> {
    let (port, sel) = first_ahci()?;
    let p = probe(sel, port).ok()?;
    #[cfg(feature = "unafs")] let ufs = super::unafsmirror::source().filter(|s| !s.is_ahci()); #[cfg(not(feature = "unafs"))] let ufs: Option<()> = None;
    #[cfg(feature = "unafs")] let n = ufs.map(|s| s.sectors()); #[cfg(not(feature = "unafs"))] let n: Option<u64> = { let _ = ufs; None };
    let plan = ssd_plan(p.sectors, n).ok()?;
    let mut lines = plan.lines();
    #[cfg(feature = "unafs")]
    if let (Some(s), Some(dst)) = (ufs, plan.part(amber_core::PartKind::UnaFS)) {
        if let Some(cp) = amber_core::ClonePlan::into_part(s.span.base_lba, s.sectors(), dst) {
            lines.push(alloc::format!("  then: mirror the ESP files, {}", cp));
        }
    }
    lines.push(alloc::format!("  verdict={} — `install ssd --write` lays it", p.verdict.tag()));
    Some(lines)
}

/// Dry run: the whole plan from `amber_core::Plan`, the clone byte count, the ETA from a measured read.
fn print_plan(p: &Probe, out: &mut dyn FnMut(&str)) {
    #[cfg(feature = "unafs")] let ufs = super::unafsmirror::source().filter(|s| !s.is_ahci()); #[cfg(not(feature = "unafs"))] let ufs: Option<()> = None;
    #[cfg(feature = "unafs")] let n = ufs.map(|s| s.sectors()); #[cfg(not(feature = "unafs"))] let n: Option<u64> = { let _ = ufs; None };
    let plan = match ssd_plan(p.sectors, n) {
        Ok(pl) => pl,
        Err(e) => {
            serial_println!("[install] plan: none ({})", e);
            out(&alloc::format!("  plan: none ({})", e));
            return;
        }
    };
    for line in plan.lines() {
        serial_println!("[install] {}", line);
        out(&alloc::format!("  {}", line));
    }
    let mut cloned = 0u64;
    #[cfg(feature = "unafs")]
    match (ufs, plan.part(amber_core::PartKind::UnaFS)) {
        (Some(s), Some(dst)) => {
            if let Some(cp) = amber_core::ClonePlan::into_part(s.span.base_lba, s.sectors(), dst) {
                let rate = super::unafsmirror::measure_rate(&s, 2048);
                let eta = rate.and_then(|r| cp.eta_ms(r));
                serial_println!(
                    "[install] unafs mirror: {} src={} rate={}KiB/s eta_ms={}",
                    cp, super::unafsmirror::handle_name(s.handle), rate.map_or(0, |r| r / 1024), eta.unwrap_or(0)
                );
                out(&alloc::format!("  unafs: {} — about {} s at the measured {} KiB/s", cp, eta.unwrap_or(0) / 1000, rate.map_or(0, |r| r / 1024)));
                cloned = amber_core::sectors_mib(cp.sectors);
            }
        }
        _ => {
            serial_println!("[install] unafs mirror: none (the running system has no UnaFS root to mirror; the SSD gets the ESP only)");
            out("  unafs: none — the running system has no UnaFS root; the SSD gets the ESP only");
        }
    }
    let _ = &mut cloned;
    witness(&plan, cloned, "skip", "DRY");
}

/// `--write`'s UnaFS leg: sector-clone the running volume into the plan's UnaFS partition, then fsck
/// the copy through the grant. Returns `(cloned MiB, fsck tag, ok)`.
#[cfg(all(feature = "unafs", feature = "ahci-write"))]
fn mirror_unafs(
    disk: &mut AhciDisk,
    plan: &amber_core::Plan,
    src: Option<super::unafsmirror::Source>,
    out: &mut dyn FnMut(&str),
) -> (u64, &'static str, bool) {
    let (Some(s), Some(dst)) = (src, plan.part(amber_core::PartKind::UnaFS)) else {
        serial_println!("[install] unafs mirror: none (no UnaFS root on the running system)");
        return (0, "skip", true);
    };
    let Some(cp) = amber_core::ClonePlan::into_part(s.span.base_lba, s.sectors(), dst) else {
        serial_println!("[install] unafs mirror: the volume does not fit the planned partition");
        out("install ssd --write: the UnaFS volume does not fit its partition — NOT mirrored");
        return (0, "skip", false);
    };
    serial_println!("[install] unafs mirror: {} src={}", cp, super::unafsmirror::handle_name(s.handle));
    match super::unafsmirror::clone_span(disk, &s, &cp) {
        Ok(ms) => serial_println!("[install] unafs mirror done {} MiB ms={}", amber_core::sectors_mib(cp.sectors), ms),
        Err(e) => {
            serial_println!("[install] unafs mirror FAILED err={:?} (SourceMoved = the live volume committed during the copy; re-run when idle)", e);
            out("install ssd --write: the UnaFS mirror FAILED — the SSD's UnaFS partition is not trustworthy");
            return (0, "skip", false);
        }
    }
    let cloned = amber_core::sectors_mib(cp.sectors);
    match super::unafsmirror::fsck_target(disk, dst.first, s.span.block_count) {
        Ok(true) => (cloned, "ok", true),
        Ok(false) => {
            serial_println!("[install] unafs fsck of the copy: NOT clean");
            (cloned, "fail", false)
        }
        Err(why) => {
            serial_println!("[install] unafs fsck of the copy: {}", why);
            (cloned, "fail", false)
        }
    }
}

/// SELFINSTALL2 M4: `tests install` — the shared core's GPT encode/decode KATs and the planner on a
/// synthetic disk, IN-KERNEL and with no I/O, then the real dry run (which prints `-> DRY`, or SKIPs on
/// a machine with no SATA disk). Nothing is written on any path.
pub fn install_selftest() {
    let kat = amber_core::kat::run();
    // A synthetic 500 GB SSD and a 512 MiB running volume: the two-partition plan, 1 MiB aligned.
    let synth = ssd_plan(976_773_168, Some(1_048_576));
    let planner_ok = synth.as_ref().is_ok_and(|p| {
        p.parts.len() == 2
            && p.parts[0].first == 2048
            && p.parts[1].first == 2048 + ESP_SECTORS
            && p.parts[1].sectors() == 1_048_576
            && p.gpt().is_ok()
    });
    let tiny_refused = ssd_plan(500_000, Some(1_048_576)).is_err();
    let pass = kat.is_ok() && planner_ok && tiny_refused;
    serial_println!(
        ":: SELFINSTALL2: kat={} planner={} tiny={} -> {} ::",
        match kat { Ok(n) => alloc::format!("{}", n), Err(why) => alloc::format!("FAIL({})", why) },
        if planner_ok { "ok" } else { "bad" },
        if tiny_refused { "refused" } else { "ACCEPTED" },
        if pass { "PASS" } else { "FAIL" }
    );
    let mut sink = |_s: &str| {};
    if dry_run(&mut sink).is_none() {
        serial_println!(":: SELFINSTALL2: dry-run disks=0 -> SKIP (no AHCI disk on this machine) ::");
    }
}
