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
    /// AHCIROOT (B332): a Stranger the OPERATOR cleared by typing the confirmation token the refusal
    /// printed (`--erase-stranger ERASE-<port>-<sectors>`). Carries what was seen, for the wire.
    #[cfg(feature = "ahciroot")]
    Confirmed(String),
}

impl Verdict {
    pub fn tag(&self) -> &'static str {
        match self {
            Verdict::Blank => "blank",
            Verdict::Ours => "ours",
            Verdict::Stranger(_) => "stranger",
            #[cfg(feature = "ahciroot")]
            Verdict::Confirmed(_) => "confirmed",
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

/// INSTALL4 (B619): the SATA disk `install ssd` names — the first one that is NOT the boot disk, else the
/// first one (so the refusal still names it). Index 0 alone named the q35 boot ESP (`ide.0` = AHCI port 0)
/// on every QEMU lane and never the virtual SSD beside it; on the rMBP (SSD = port 0, boot = the SD card)
/// the answer is the one it always was.
fn first_ahci() -> Option<(u8, BlockDeviceId)> {
    let mut first = None;
    for ix in 0..block::MAX_AHCI_DISKS {
        if let Some(d) = block::ahci_disk(ix) {
            let id = d.info.id(BlockHandle::Ahci { port: d.port });
            if !is_boot_disk(id) {
                return Some((d.port, id));
            }
            first = first.or(Some((d.port, id)));
        }
    }
    first
}

/// INSTALL4 (B619): is this SATA disk the one the system booted from? `selfguard` answers for the handles
/// it enumerates; for a SATA handle the installer's own serial check (`partition::sata_is_boot_device`).
fn is_boot_disk(id: BlockDeviceId) -> bool {
    #[cfg(feature = "ahci-write")]
    if partition::sata_is_boot_device(id.handle) {
        return true;
    }
    super::selfguard::refuses(id)
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

/// INSTALL4 (B619): is the running volume on the TARGET disk? Only that is refused: a system booted from
/// one SATA disk (every QEMU lane, an rMBP booted off an installed SSD) may clone onto ANOTHER one.
fn source_on_port(s: BlockSource, port: u8) -> bool {
    matches!(s, BlockSource::Ahci(p) if p == port)
}

fn plan_text(p: &Probe) -> String {
    if let Verdict::Stranger(_) = &p.verdict {
        return beside_text(p); // INSTALL4 (B619): the stranger's plan is BESIDE, or a named refusal — never a wipe
    }
    if !p.verdict.writable() {
        return String::from("REFUSE(stranger: R20, no destructive work on a disk that is not ours)");
    }
    let Some(src) = running_source() else { return String::from("NO-SOURCE(running volume not found)") };
    if source_on_port(src, p.port) {
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
            if matches!(p.verdict, Verdict::Stranger(_)) { print_beside(&p, out); } else { print_plan(&p, out); } // INSTALL4 (B619): a stranger's disk never gets the whole-disk layout printed as its plan
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
    write_ssd_confirmed(out, force, None)
}

/// AHCIROOT (B332): `install ssd --write [--force] [--erase-stranger <token>]` — `confirm` is the token
/// the operator typed; it clears a Stranger verdict only when it equals [`stranger_token`] for THIS disk.
#[cfg(feature = "ahci-write")]
pub fn write_ssd_confirmed(out: &mut dyn FnMut(&str), force: bool, _confirm: Option<&str>) {
    let _ = write_ssd_inner(out, force, _confirm, None);
}

/// The one body behind `install ssd --write` (glass = `None`) and the installer window's confirm screen
/// (INSTALL3, B342: glass = `Some`, naming the operator's disk, layout and dry-run choice and receiving
/// the stage lines). Every refusal, the grant and the write are this function's for both callers.
#[cfg(feature = "ahci-write")]
fn write_ssd_inner(out: &mut dyn FnMut(&str), force: bool, _confirm: Option<&str>, mut glass: Option<Glass<'_>>) -> GlassOutcome {
    let (gport, glayout, dry) = match &glass {
        Some(g) => (Some(g.port), g.layout, g.dry_run),
        None => (None, None, false),
    };
    let mut stage = |name: &str, what: &str| {
        serial_println!("[install] stage={} {}", name, what);
        if let Some(g) = glass.as_mut() {
            (g.stage)(name, what);
        }
    };
    let oc = |grant: bool, pass: bool, why: &str| GlassOutcome { grant, pass, reason: String::from(why) };
    let target = match gport {
        Some(gp) => block::ahci_info_port(gp).map(|i| (gp, i.id(BlockHandle::Ahci { port: gp }))),
        None => first_ahci(),
    };
    let Some((port, sel)) = target else {
        out("install ssd: no AHCI disk registered");
        stage("probe", "refused (no AHCI disk on that port)");
        return oc(false, false, "no AHCI disk on that port");
    };
    #[cfg_attr(not(feature = "ahciroot"), allow(unused_mut))] let mut p = match probe(sel, port) {
        Ok(p) => p,
        Err(e) => {
            serial_println!("[install] target=ahci:{} probe err={:?} — nothing written", port, e);
            out("install ssd --write: cannot read the disk; nothing written");
            stage("probe", "refused (the disk cannot be read)");
            return oc(false, false, "the disk cannot be read");
        }
    };
    stage("probe", &alloc::format!("ok ahci:{} verdict={}", port, p.verdict.tag()));
    #[cfg(feature = "ahciroot")] if let Some((gp, _, _, crate::drivers::block::GrantKind::Root)) = block::ahci_live_grant() { serial_println!("[install] REFUSED target=ahci:{} reason=ssd-is-live-root (port {} holds the boot's root grant) — nothing written ::", port, gp); out("install ssd --write: REFUSED — the SSD's UnaFS is the running root; boot without it to reinstall. Nothing was written."); stage("guard", "refused (the SSD is the running root)"); return oc(false, false, "the SSD's UnaFS is the running root"); } // AHCIROOT (B332)
    #[cfg(feature = "ahciroot")] if let (Verdict::Stranger(why), Some(tok)) = (&p.verdict, _confirm) { if tok == stranger_token(port, p.sectors).as_str() { serial_println!("[install] target=ahci:{} verdict=stranger CONFIRMED by the operator token — the disk will be erased (R25: the operator's word, typed)", port); p.verdict = Verdict::Confirmed(why.clone()); } } // AHCIROOT (B332)
    serial_println!(
        "[install] target=ahci:{} model={} sectors={} gpt={} verdict={} saw={}",
        p.port, p.model, p.sectors, p.gpt, p.verdict.tag(), p.saw
    );
    if let Verdict::Stranger(why) = &p.verdict {
        // R20: no destructive work on a disk that is not ours.
        serial_println!("[install] REFUSED target=ahci:{} verdict=stranger saw: {} — nothing was written (R20) ::", port, why);
        out(&alloc::format!("install ssd --write: REFUSED — this disk is not ours ({}). Nothing was written.", why));
        #[cfg(feature = "ahciroot")] out(&alloc::format!("  to install BESIDE it (every partition on it kept): `install ssd --dry-run` prints the plan, then install ssd --write --beside {}", beside_token(port, p.sectors))); // INSTALL4 (B619): the additive path first
        #[cfg(feature = "ahciroot")] out(&alloc::format!("  to ERASE the WHOLE disk instead (every OS and file on it is lost): install ssd --write --erase-stranger {}", stranger_token(port, p.sectors))); // AHCIROOT (B332): the glass asks; the operator types
        stage("guard", "refused (not ours, and not confirmed)");
        return oc(false, false, "the disk is not ours and the erase was not confirmed");
    }
    // SELFINSTALL2: the boot disk itself is refused here by name (selfguard), before the grant's own
    // refusal one step later, and an existing UnaFS volume is not overwritten without `--force`.
    if super::selfguard::refuses(sel) {
        serial_println!("[install] REFUSED target=ahci:{} reason=boot-device (selfguard) — nothing written ::", port);
        out("install ssd --write: REFUSED — this disk is the one the system booted from. Nothing was written.");
        stage("guard", "refused (the boot disk)");
        return oc(false, false, "the disk is the one the system booted from");
    }
    if p.unafs_present && !force {
        serial_println!("[install] REFUSED target=ahci:{} reason=unafs-present (a UnaFS volume is already on the target; --force replaces it) — nothing written ::", port);
        out("install ssd --write: REFUSED — the SSD already carries a UnaFS volume; `install ssd --write --force` replaces it. Nothing was written.");
        stage("guard", "refused (a UnaFS volume is present; --force)");
        return oc(false, false, "a UnaFS volume is present");
    }
    stage("guard", &alloc::format!("ok verdict={}", p.verdict.tag()));
    #[cfg(feature = "unafs")] let unafs_src = super::unafsmirror::source().filter(|s| !s.is_ahci()); #[cfg(not(feature = "unafs"))] let unafs_src: Option<()> = None;
    #[cfg(feature = "unafs")] let unafs_sectors = unafs_src.map(|s| s.sectors()); #[cfg(not(feature = "unafs"))] let unafs_sectors: Option<u64> = None;
    let plan_for = |sectors: u64| match glayout {
        Some((esp, ufs)) => ssd_plan_layout(sectors, esp, ufs, unafs_sectors),
        None => ssd_plan(sectors, unafs_sectors),
    };
    if dry {
        // INSTALL3 (B342): the glass's DRY RUN — the judgment above ran in full, the grant is minted from
        // the verdict exactly as below and dropped unheld; no stage past this one touches the disk.
        if AhciDisk::new_quiet(sel, &p.verdict).is_err() {
            stage("grant", "refused (boot device, or the disk is too small)");
            return oc(false, false, "the write grant was refused");
        }
        stage("grant", "issued (dry run: minted, never held)");
        match plan_for(p.sectors) {
            Ok(pl) => {
                for line in pl.lines() {
                    serial_println!("[install] {}", line);
                }
                witness(&pl, 0, "skip", "DRY");
            }
            Err(e) => {
                stage("gpt", &alloc::format!("fail (plan: {})", e));
                return oc(true, false, "the SSD is too small for the plan");
            }
        }
        for s in ["snapshot", "gpt", "esp", "unafs", "fsck", "grow"] {
            stage(s, "dry (not run)");
        }
        stage("done", "dry run — nothing written");
        return oc(true, true, "dry run — nothing written");
    }
    let t0 = crate::arch::ticks();
    // Source: the running card's volume, buffered whole BEFORE any destructive write.
    let Some(src_id) = running_source() else { fail(out, "running volume not found"); stage("snapshot", "fail (running volume not found)"); return oc(false, false, "running volume not found") };
    if source_on_port(src_id, port) {
        fail(out, "already running from the SSD; boot the card to clone");
        stage("snapshot", "fail (already running from the SSD)");
        return oc(false, false, "already running from the SSD");
    }
    let tree = match fat::mount_source(src_id)
        .map_err(|_| InstallError::Io)
        .and_then(|fs| super::clone::snapshot_capped(&fs, SNAP_MAX_FILE, SNAP_MAX_TOTAL))
    {
        Ok(t) => t,
        Err(e) => {
            serial_println!("[install] snapshot of {} failed err={:?}", src_id.name(), e);
            fail(out, "snapshot of the card volume failed");
            stage("snapshot", "fail");
            return oc(false, false, "snapshot of the card volume failed");
        }
    };
    if !tree_has_boot_files(&tree) {
        fail(out, "the card volume lacks EFI/BOOT/BOOTX64.EFI or kernel.elf; not a bootable UnaOS volume");
        stage("snapshot", "fail (not a bootable UnaOS volume)");
        return oc(false, false, "the card volume is not bootable");
    }
    serial_println!("[install] snapshot src={} files={} bytes={}", src_id.name(), tree.file_count, tree.total_bytes);
    stage("snapshot", &alloc::format!("ok files={} bytes={}", tree.file_count, tree.total_bytes));
    let mut disk = match AhciDisk::new(sel, &p.verdict) {
        Ok(d) => d,
        Err(e) => {
            serial_println!("[install] write grant refused err={:?} (boot-device or too small) — nothing written", e);
            fail(out, "write grant refused (boot device, or disk too small)");
            stage("grant", "refused");
            return oc(false, false, "the write grant was refused");
        }
    };
    #[cfg(feature = "ahciroot")] let _hold = match block::hold_ahci_grant(disk.grant, block::GrantKind::Install) { Some(h) => h, None => { fail(out, "the SATA write grant slot is held (the SSD is the live root, or a test holds it)"); stage("grant", "refused (the grant slot is held)"); return oc(false, false, "the grant slot is held") } }; // AHCIROOT (B332) M1: the kernel-held grant, dropped (and the drive flushed) when this verb returns
    stage("grant", "issued (held for this install)");
    // SELFINSTALL2: the table comes from the shared core's Plan — ESP + (when the running system has
    // one) a UnaFS partition the size of the running volume.
    let plan = match plan_for(p.sectors) {
        Ok(pl) => pl,
        Err(e) => {
            serial_println!("[install] plan refused: {}", e);
            fail(out, "the SSD is too small for the plan");
            stage("gpt", "fail (the SSD is too small for the plan)");
            return oc(true, false, "the SSD is too small for the plan");
        }
    };
    for line in plan.lines() {
        serial_println!("[install] {}", line);
    }
    if let Err(e) = super::gpt::write_plan(&mut disk, &plan) {
        serial_println!("[install] GPT write failed err={:?}", e);
        fail(out, "GPT write/verify failed");
        stage("gpt", "fail");
        return oc(true, false, "GPT write/verify failed");
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
    stage("gpt", &alloc::format!("ok {} parts", plan.parts.len()));
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
            fail(out, "ESP format/clone failed");
            stage("esp", "fail");
            return oc(true, false, "ESP format/clone failed");
        }
    };
    stage("esp", &alloc::format!("ok files={} verified={}", w.files, w.verified));
    // SELFINSTALL2 M2: mirror the UnaFS volume (sector clone of the running span) and fsck the copy.
    #[cfg(feature = "unafs")] let (cloned_mib, fsck_tag, unafs_ok) = mirror_unafs(&mut disk, &plan, unafs_src, out); #[cfg(not(feature = "unafs"))] let (cloned_mib, fsck_tag, unafs_ok) = { let _ = unafs_src; (0u64, "skip", true) };
    #[cfg(feature = "ahciroot")] let (cloned_mib, fsck_tag, unafs_ok) = if unafs_src.is_none() { fresh_unafs(&mut disk, &plan, port, out) } else { (cloned_mib, fsck_tag, unafs_ok) }; // AHCIROOT (B332) M3
    stage("unafs", if unafs_ok { "ok" } else { "fail" });
    stage("fsck", fsck_tag);
    #[cfg(feature = "unafs")] let unafs_ok = if unafs_ok && fsck_tag == "ok" { drop(tree); let (what, ok) = grow_unafs(&mut disk, &plan); stage("grow", &what); ok } else { stage("grow", "skip (the volume did not fsck clean)"); unafs_ok }; // UNAFSGROW (B347) M2: the volume grows to p2 (the snapshot buffer is freed first: the grown map needs the heap)
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
    stage("done", if pass { "PASS" } else { "FAIL" });
    oc(true, pass, if pass { "the SSD carries UnaOS" } else { "the install is NOT trustworthy" })
}

#[cfg(not(feature = "ahci-write"))]
pub fn write_ssd(out: &mut dyn FnMut(&str), _force: bool) {
    serial_println!("[install] --write REFUSED reason=transport-write-disabled transport=ahci knob=UNAOS_AHCI_WRITE");
    out("install ssd --write: this build has no SATA write path (rebuild with UNAOS_AHCI_WRITE=1)");
}

/// The shell arm: `install ssd [--dry-run|--write]` (`args` is what follows `ssd`).
pub fn verb(out: &mut dyn FnMut(&str), args: &[&str]) {
    #[cfg(feature = "login")]
    if args.first().copied() == Some("--write") && crate::fs::users::admin_authority("install-ssd-write").is_err() {
        return out("install ssd --write: an administrator's session is required (R100: log in as the administrator)"); // FIRSTUSER (B409)
    }
    match args.first().copied() {
        Some("--dry-run") if args.len() == 1 => {
            let _ = dry_run(out);
        }
        Some("--write") if args.len() == 1 => write_ssd(out, false),
        Some("--write") if args.len() == 2 && args[1] == "--force" => write_ssd(out, true),
        #[cfg(feature = "ahciroot")]
        Some("--write") if args.len() == 3 && args[1] == "--erase-stranger" => write_ssd_confirmed(out, false, Some(args[2])),
        #[cfg(feature = "ahciroot")]
        Some("--write") if args.len() == 3 && args[1] == "--beside" => write_beside(out, args[2]), // INSTALL4 (B619)
        #[cfg(not(feature = "ahciroot"))]
        Some("--write") if args.len() == 3 && args[1] == "--beside" => out("install ssd --write --beside: this build has no beside install (rebuild with UNAOS_AHCIROOT=1)"),
        _ => out("usage: install ssd --dry-run | install ssd --write [--force] | install ssd --write --beside <token>   (the SSD install: ESP + UnaFS; --write needs UNAOS_AHCI_WRITE=1, refuses a stranger's disk — the dry run prints the beside plan and its token — and needs --force over an existing UnaFS volume)"),
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
    #[cfg(feature = "ahciroot")] let unafs_sectors = Some(unafs_sectors.filter(|&n| n > 0).unwrap_or(super::ahciroot::FRESH_UNAFS_MIB * 2048) + super::ahciroot::SCRATCH_SECTORS); // AHCIROOT (B332) M3: p2 always (cloned or fresh), plus the 64-sector scratch tail outside the volume
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
    if matches!(p.verdict, Verdict::Stranger(_)) {
        return Some(beside_lines(&p)); // INSTALL4 (B619)
    }
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
            #[cfg(feature = "ahciroot")] { serial_println!("[install] unafs: fresh {} MiB volume (UnaFS::format, the `unafs init` code) + {} scratch sectors — the running system has no UnaFS root to mirror", super::ahciroot::FRESH_UNAFS_MIB, super::ahciroot::SCRATCH_SECTORS); out("  unafs: a fresh UnaFS volume (UnaFS::format) — the running system has no UnaFS root to mirror"); }
            #[cfg(not(feature = "ahciroot"))] { serial_println!("[install] unafs mirror: none (the running system has no UnaFS root to mirror; the SSD gets the ESP only)");
            out("  unafs: none — the running system has no UnaFS root; the SSD gets the ESP only"); }
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
            && p.parts[1].sectors() == 1_048_576 + PLAN_SCRATCH
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

// ---------------------------------------------------------------------------------------------
// AHCIROOT (rmbp-ledger B332) — the operator's confirmation token for a Stranger disk.
// ---------------------------------------------------------------------------------------------

/// The token the refusal prints and `--erase-stranger` must repeat: the port and the disk's exact
/// sector count, so a token copied from another machine (or another disk) does not match.
#[cfg(feature = "ahciroot")]
pub fn stranger_token(port: u8, sectors: u64) -> String {
    alloc::format!("ERASE-{}-{}", port, sectors)
}

/// AHCIROOT (B332): the scratch tail the plan adds to p2 (0 without the knob) — `tests install`'s
/// planner check reads it so the synthetic plan is asserted in both polarities.
#[cfg(feature = "ahciroot")]
const PLAN_SCRATCH: u64 = super::ahciroot::SCRATCH_SECTORS;
#[cfg(not(feature = "ahciroot"))]
const PLAN_SCRATCH: u64 = 0;

/// AHCIROOT (B332) M3: `--write`'s UnaFS leg when there is no running volume to mirror — format p2 with
/// `UnaFS::format` through the held install grant, then fsck the result. Returns `(0, fsck tag, ok)`.
#[cfg(feature = "ahciroot")]
fn fresh_unafs(disk: &mut AhciDisk, plan: &amber_core::Plan, port: u8, out: &mut dyn FnMut(&str)) -> (u64, &'static str, bool) {
    let Some(dst) = plan.part(amber_core::PartKind::UnaFS) else { return (0, "skip", false) };
    let mib = super::ahciroot::FRESH_UNAFS_MIB;
    serial_println!("[install] unafs fresh: UnaFS::format {} MiB at lba={} (p2 {}..{}, scratch tail {} sectors)", mib, dst.first, dst.first, dst.last, super::ahciroot::SCRATCH_SECTORS);
    let blocks = match super::ahciroot::format_fresh(port, dst.first, mib) {
        Ok(n) => n,
        Err(why) => {
            serial_println!("[install] unafs fresh FAILED: {}", why);
            out("install ssd --write: formatting the SSD's UnaFS partition FAILED");
            return (0, "skip", false);
        }
    };
    match super::unafsmirror::fsck_target(disk, dst.first, blocks) {
        Ok(true) => (0, "ok", true),
        Ok(false) => (0, "fail", false),
        Err(why) => {
            serial_println!("[install] unafs fsck of the fresh volume: {}", why);
            (0, "fail", false)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// INSTALL3 (rmbp-ledger B342) — the installer window's door into `write_ssd_inner`. File tail.
// ---------------------------------------------------------------------------------------------

/// What the installer window hands the write body: the disk the operator chose (by port), the layout
/// they chose (`(esp_sectors, Some(p2 sectors incl. scratch) | None = the rest)`, or `None` for the
/// verb's own plan), the dry-run choice, and where the stage lines go.
#[cfg(feature = "ahci-write")]
pub struct Glass<'a> {
    pub port: u8,
    pub layout: Option<(u64, Option<u64>)>,
    pub dry_run: bool,
    pub stage: &'a mut dyn FnMut(&str, &str),
}

/// What the write body answers the window: was the grant issued (minted from the verdict), did the
/// install (or the dry run) pass, and the reason in words for the result screen.
#[cfg(feature = "ahci-write")]
pub struct GlassOutcome {
    pub grant: bool,
    pub pass: bool,
    pub reason: String,
}

/// The installer window's confirm: `confirm` is the shell token (`stranger_token`) the window derived
/// from the disk name the operator typed, or `None` (an empty or ours disk). The operator's click IS the
/// consent `--force` stands for, so an existing UnaFS volume on an ours disk is replaced.
#[cfg(feature = "ahci-write")]
pub fn write_ssd_glass(out: &mut dyn FnMut(&str), confirm: Option<&str>, glass: Glass<'_>) -> GlassOutcome {
    write_ssd_inner(out, true, confirm, Some(glass))
}

/// The smallest p2 the plan accepts: the volume it will carry (the running one mirrored, or the fresh
/// one) plus the scratch tail. The window's layout screen never offers less.
pub fn p2_min_sectors(unafs_sectors: Option<u64>) -> u64 {
    #[cfg(feature = "ahciroot")]
    return unafs_sectors.filter(|&n| n > 0).unwrap_or(super::ahciroot::FRESH_UNAFS_MIB * 2048) + super::ahciroot::SCRATCH_SECTORS;
    #[cfg(not(feature = "ahciroot"))]
    return unafs_sectors.unwrap_or(0);
}

/// The operator's layout through the same planner: ESP of `esp_sectors`, then p2 of `p2` sectors (the
/// scratch tail included) or the rest of the disk. Refused when p2 cannot hold [`p2_min_sectors`].
pub fn ssd_plan_layout(disk_sectors: u64, esp_sectors: u64, p2: Option<u64>, unafs_sectors: Option<u64>) -> Result<amber_core::Plan, amber_core::PlanError> {
    use amber_core::{PartKind, PartReq, Size};
    let min = p2_min_sectors(unafs_sectors);
    if p2.is_some_and(|n| n < min) {
        return Err(amber_core::PlanError::TooSmall);
    }
    let esp = PartReq { kind: PartKind::Esp, size: Size::Sectors(esp_sectors), name: "UNAOS-ESP", seed: b"UNAOS-INSTALL-ESP" };
    let ufs = PartReq { kind: PartKind::UnaFS, size: p2.map_or(Size::Rest, Size::Sectors), name: "UNAOS-UNAFS", seed: b"UNAOS-INSTALL-UFS" };
    let plan = amber_core::Plan::layout(disk_sectors, super::gpt::INSTALL_DISK_SEED, &[esp, ufs])?;
    match plan.part(PartKind::UnaFS) {
        Some(u) if u.sectors() >= min => Ok(plan),
        _ => Err(amber_core::PlanError::TooSmall),
    }
}

/// The running UnaFS volume's sectors when there is one to mirror (the layout screen's minimum).
pub fn mirror_source_sectors() -> Option<u64> {
    #[cfg(feature = "unafs")]
    return super::unafsmirror::source().filter(|s| !s.is_ahci()).map(|s| s.sectors());
    #[cfg(not(feature = "unafs"))]
    return None;
}

// ---------------------------------------------------------------------------------------------
// UNAFSGROW (rmbp-ledger B347) M2 — the copied (or fresh) volume grows to its partition. File tail.
// ---------------------------------------------------------------------------------------------

/// The largest volume the installer grows to: the paged UnaFS format's own wall (2^31 blocks = 8 TiB),
/// i.e. the partition. UNAFSMAP (B354) made the refcount map a paged tree — RAM is 16 B per 4 MiB leaf
/// plus a 4 MiB leaf cache (a 500 GB volume mounts in about 2 MiB of heap, see unafs `map_bench`) and a
/// commit writes only its dirty leaves — so the 16 GiB cap UNAFSGROW (B347) needed is gone.
pub const GROW_CAP_BLOCKS: u64 = 1 << 31;
// The installer's cap IS the crate's wall (a literal above: this file also builds without `unafs`).
#[cfg(feature = "unafs")]
const _: () = assert!(GROW_CAP_BLOCKS == ::unafs::superblock::MAX_BLOCK_COUNT_PAGED);

/// The block count the installer grows p2's volume to: the partition minus the scratch tail, in whole
/// 4 KiB blocks, capped at [`GROW_CAP_BLOCKS`]. Pure; `tests unafsgrow` prints it for a synthetic plan.
pub fn grow_target_blocks(p2_sectors: u64) -> u64 {
    (p2_sectors.saturating_sub(PLAN_SCRATCH) / 8).min(GROW_CAP_BLOCKS)
}

/// `--write`'s grow leg, after the volume was copied (or formatted) and fsck'd clean: grow it to
/// [`grow_target_blocks`] through the held grant and fsck it again. Returns the stage text and whether
/// the volume is still trustworthy (a refused grow writes nothing and keeps the old size: still `true`).
#[cfg(all(feature = "unafs", feature = "ahci-write"))]
fn grow_unafs(disk: &mut AhciDisk, plan: &amber_core::Plan) -> (String, bool) {
    let Some(dst) = plan.part(amber_core::PartKind::UnaFS) else { return (String::from("skip (no UnaFS partition)"), true) };
    let want = grow_target_blocks(dst.sectors());
    match super::unafsmirror::grow_target(disk, dst.first, dst.sectors() / 8, want) {
        Ok((from, to, true)) => {
            serial_println!("[install] unafs grow: from={} to={} blocks ({} MiB -> {} MiB) fsck=ok", from, to, from / 256, to / 256);
            (alloc::format!("from={} to={} ok", from, to), true)
        }
        Ok((from, to, false)) => {
            serial_println!("[install] unafs grow: from={} to={} fsck NOT clean", from, to);
            (alloc::format!("from={} to={} fail (fsck not clean)", from, to), false)
        }
        Err(why) => {
            serial_println!("[install] unafs grow: {} — the volume keeps its size", why);
            (alloc::format!("from=? to={} fail ({}; the volume keeps its size)", want, why), true)
        }
    }
}

/// UNAFSGROW (B347) M3: the dry run's grant — the same judgment as [`AhciDisk::new`], minted silently
/// (`partition::mint_disk_grant_quiet`) and dropped by the caller unheld.
#[cfg(all(target_arch = "x86_64", feature = "ahci-write"))]
impl AhciDisk {
    pub fn new_quiet(sel: BlockDeviceId, v: &Verdict) -> Result<Self, InstallError> {
        let rd = super::BlockTarget::bind_id(sel)?;
        let grant = partition::mint_disk_grant_quiet(sel, rd.capacity_sectors(), v).ok_or(InstallError::NotBlank)?;
        Ok(Self { rd, grant })
    }
}

// ---------------------------------------------------------------------------------------------
// INSTALL4 (rmbp-ledger B619) — BESIDE A STRANGER, never over it. R25: a non-UnaOS disk changes only by
// the operator's explicit act; the arc's rule on top of it: a stranger's disk gets an ADDITIVE plan (an
// ESP and a UnaFS partition in free space, every existing partition kept) or a named refusal — the
// whole-disk layout is never printed as its plan. The write takes the typed `BESIDE-<port>-<sectors>`
// token, mints grants that cover only free space and the two GPT tables (`partition::mint_beside_*`),
// adds two entries in empty slots (`gpt::add_entries`), then runs the SAME ESP and UnaFS legs the
// whole-disk install runs, through an `AhciDisk` whose grant is the new partitions only. File tail.
// ---------------------------------------------------------------------------------------------

/// The token `install ssd --write --beside` must repeat: the port and the disk's exact sector count.
pub fn beside_token(port: u8, sectors: u64) -> String {
    alloc::format!("BESIDE-{}-{}", port, sectors)
}

/// The additive plan: two partitions laid in the largest free extent, in two empty slots.
pub struct BesidePlan {
    pub plan: amber_core::Plan,
    pub free: (u64, u64),
    pub slots: [u32; 2],
    pub existing: usize,
}

/// The beside plan for a table, or the named refusal (`<token> (<words>)`). The ESP is 512 MiB at the
/// free extent's first 1 MiB boundary, the UnaFS partition runs from the next boundary to the extent's
/// end and must hold [`p2_min_sectors`] (the volume to mirror, or the fresh one, plus the scratch tail).
pub fn beside_plan(rt: &super::gpt::RawTable, unafs_sectors: Option<u64>) -> Result<BesidePlan, String> {
    use amber_core::gpt::{derive_guid, ALIGN};
    use amber_core::{sectors_mib, Part, PartKind};
    let slots = rt.empty_slots();
    if slots.len() < 2 {
        return Err(String::from("no-free-slot (the partition table has fewer than two empty entries)"));
    }
    let need = p2_min_sectors(unafs_sectors).max(1);
    let want_mib = sectors_mib(ESP_SECTORS + need + 2 * ALIGN);
    let align = |x: u64| x.div_ceil(ALIGN) * ALIGN;
    let Some((a, b)) = rt.free_extents().into_iter().max_by_key(|&(a, b)| b - a) else {
        return Err(alloc::format!("no-free-space (have=0 MiB need={} MiB; make room in macOS first: diskutil apfs resizeContainer <container> <smaller size>)", want_mib));
    };
    let esp_first = align(a);
    let esp_last = esp_first.saturating_add(ESP_SECTORS - 1);
    let ufs_first = align(esp_last.saturating_add(1));
    if ufs_first > b || b - ufs_first + 1 < need {
        return Err(alloc::format!(
            "no-free-space (have={} MiB need={} MiB; make room in macOS first: diskutil apfs resizeContainer <container> <smaller size>)",
            sectors_mib(b - a + 1), want_mib
        ));
    }
    let esp = Part { kind: PartKind::Esp, first: esp_first, last: esp_last, guid: derive_guid(b"UNAOS-BESIDE-ESP"), name: String::from("UNAOS-ESP") };
    let ufs = Part { kind: PartKind::UnaFS, first: ufs_first, last: b, guid: derive_guid(b"UNAOS-BESIDE-UFS"), name: String::from("UNAOS-UNAFS") };
    let plan = amber_core::Plan { disk_sectors: rt.total_sectors, disk_guid: rt.primary.disk_guid, parts: alloc::vec![esp, ufs] };
    Ok(BesidePlan { plan, free: (a, b), slots: [slots[0], slots[1]], existing: rt.entries.len() })
}

/// The beside plan for the probed disk: its raw table read and cross-checked, then [`beside_plan`].
fn beside_for(p: &Probe) -> Result<BesidePlan, String> {
    if p.unafs_present {
        return Err(String::from("beside-present (a UnaFS partition is already on this disk; a re-install beside a stranger is owed)"));
    }
    let t = super::BlockTarget::bind_id(p.id).map_err(|e| alloc::format!("unreadable ({:?})", e))?;
    let rt = super::gpt::read_raw_table(&t)
        .map_err(|_| String::from("gpt-not-standard (no self-consistent primary + backup table pair, or an array where a standard table does not keep it)"))?;
    beside_plan(&rt, mirror_source_sectors())
}

fn beside_reason(why: &str) -> &str {
    why.split(' ').next().unwrap_or(why)
}

/// The `plan=` field of the dry run's target line for a stranger.
fn beside_text(p: &Probe) -> String {
    match beside_for(p) {
        Ok(b) => alloc::format!(
            "beside(free={}..{} {} MiB: esp {} MiB + unafs {} MiB in empty slots {},{}; the {} existing partitions untouched) then --write --beside {}",
            b.free.0, b.free.1, amber_core::sectors_mib(b.free.1 - b.free.0 + 1),
            amber_core::sectors_mib(b.plan.parts[0].sectors()), amber_core::sectors_mib(b.plan.parts[1].sectors()),
            b.slots[0], b.slots[1], b.existing, beside_token(p.port, p.sectors)
        ),
        Err(why) => alloc::format!("REFUSE(stranger: {} — never a wipe)", why),
    }
}

/// The dry run's beside block: the extent, the plan's lines, the token, the witness.
fn print_beside(p: &Probe, out: &mut dyn FnMut(&str)) {
    match beside_for(p) {
        Ok(b) => {
            serial_println!(
                "[install] beside free={}..{} ({} MiB) slots={},{} existing={} — LBA 0 and every existing partition stay outside every write",
                b.free.0, b.free.1, amber_core::sectors_mib(b.free.1 - b.free.0 + 1), b.slots[0], b.slots[1], b.existing
            );
            out(&alloc::format!("  beside: the {} existing partitions stay as they are; UnaOS goes in the free space", b.existing));
            for line in b.plan.lines() {
                serial_println!("[install] {}", line);
                out(&alloc::format!("  {}", line));
            }
            let tok = beside_token(p.port, p.sectors);
            serial_println!("[install] beside token={}", tok);
            out(&alloc::format!("  to install it (administrator; UNAOS_AHCIROOT build): install ssd --write --beside {}", tok));
            serial_println!(
                ":: INSTALL4: beside port={} free={}..{} esp={} unafs={} slots={},{} existing={} -> DRY ::",
                p.port, b.free.0, b.free.1,
                amber_core::sectors_mib(b.plan.parts[0].sectors()), amber_core::sectors_mib(b.plan.parts[1].sectors()),
                b.slots[0], b.slots[1], b.existing
            );
        }
        Err(why) => {
            serial_println!(":: INSTALL4: beside port={} reason={} -> REFUSED ::", p.port, beside_reason(&why));
            serial_println!("[install] beside refused: {}", why);
            out(&alloc::format!("  beside: REFUSED — {}", why));
        }
    }
}

/// The installer window's plan view for a stranger.
fn beside_lines(p: &Probe) -> alloc::vec::Vec<String> {
    match beside_for(p) {
        Ok(b) => {
            let mut v = b.plan.lines();
            v.push(alloc::format!("  beside: the {} existing partitions stay; then install ssd --write --beside {}", b.existing, beside_token(p.port, p.sectors)));
            v
        }
        Err(why) => alloc::vec![alloc::format!("REFUSE(stranger: {} — never a wipe)", why)],
    }
}

/// The operator's typed consent to an install BESIDE a stranger's partitions on ONE disk. The fields are
/// private and [`beside_consent`] is the only constructor: the beside grants take one (`partition.rs`).
#[cfg(feature = "ahciroot")]
pub struct BesideConsent {
    port: u8,
    sectors: u64,
}

#[cfg(feature = "ahciroot")]
impl BesideConsent {
    pub fn port(&self) -> u8 {
        self.port
    }
    pub fn sectors(&self) -> u64 {
        self.sectors
    }
}

#[cfg(feature = "ahciroot")]
fn beside_consent(port: u8, sectors: u64, typed: &str) -> Option<BesideConsent> {
    (typed == beside_token(port, sectors).as_str()).then_some(BesideConsent { port, sectors })
}

/// The table editor's target: reads through the disk, writes ONLY through the two table grants.
#[cfg(feature = "ahciroot")]
struct TableTarget {
    rd: super::BlockTarget,
    head: block::WriteGrant,
    tail: block::WriteGrant,
}

#[cfg(feature = "ahciroot")]
impl InstallTarget for TableTarget {
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
        let g = if self.head.allows(lba) { &self.head } else { &self.tail };
        block::write_sectors_granted(g, lba, buf).map_err(|_| InstallError::Io)
    }
}

/// SHA-256 of LBA 0 (slot `u32::MAX`) and of every in-use partition's first 4 KiB — read before and after.
#[cfg(feature = "ahciroot")]
fn part_heads<T: InstallTarget>(t: &T, rt: &super::gpt::RawTable) -> Option<alloc::vec::Vec<(u32, [u8; 32])>> {
    let mut v = alloc::vec::Vec::new();
    let mut lba0 = [0u8; 512];
    t.read_sectors(0, &mut lba0).ok()?;
    v.push((u32::MAX, super::hash::sha256(&lba0)));
    for (slot, e) in &rt.entries {
        let n = core::cmp::min(8, e.last_lba - e.first_lba + 1) as usize;
        let mut buf = alloc::vec![0u8; n * 512];
        t.read_sectors(e.first_lba, &mut buf).ok()?;
        v.push((*slot, super::hash::sha256(&buf)));
    }
    Some(v)
}

#[cfg(feature = "ahciroot")]
fn beside_refuse(out: &mut dyn FnMut(&str), why: &str) {
    serial_println!(":: INSTALL4: beside REFUSED reason={} — nothing written ::", beside_reason(why));
    serial_println!("[install] beside refused: {}", why);
    out(&alloc::format!("install ssd --write --beside: REFUSED — {}. Nothing was written.", why));
}

/// `install ssd --write --beside <token>`: every refusal before the first write, then table → ESP →
/// UnaFS → the neighbours read back. The verb's `login` gate (R100) runs before this.
#[cfg(feature = "ahciroot")]
pub fn write_beside(out: &mut dyn FnMut(&str), typed: &str) {
    let Some((port, sel)) = first_ahci() else { return beside_refuse(out, "no-disk (no AHCI disk is registered)") };
    let p = match probe(sel, port) {
        Ok(p) => p,
        Err(_) => return beside_refuse(out, "unreadable (the disk cannot be read)"),
    };
    serial_println!("[install] beside target=ahci:{} model={} sectors={} gpt={} verdict={} saw={}", port, p.model, p.sectors, p.gpt, p.verdict.tag(), p.saw);
    if !matches!(p.verdict, Verdict::Stranger(_)) {
        return beside_refuse(out, &alloc::format!("not-a-stranger (verdict={}; a blank or ours disk takes `install ssd --write`)", p.verdict.tag()));
    }
    if is_boot_disk(sel) {
        return beside_refuse(out, "boot-device (this disk is the one the system booted from)");
    }
    if let Some((gp, _, _, k)) = block::ahci_live_grant() {
        return beside_refuse(out, &alloc::format!("grant-slot-held (port {} kind={})", gp, k.tag()));
    }
    let Some(consent) = beside_consent(port, p.sectors, typed) else {
        return beside_refuse(out, "token-mismatch (`install ssd --dry-run` prints this disk's token)");
    };
    if p.unafs_present {
        return beside_refuse(out, "beside-present (a UnaFS partition is already on this disk)");
    }
    let rd = match super::BlockTarget::bind_id(sel) {
        Ok(t) => t,
        Err(_) => return beside_refuse(out, "target-gone (the disk left the registry)"),
    };
    let rt = match super::gpt::read_raw_table(&rd) {
        Ok(rt) => rt,
        Err(_) => return beside_refuse(out, "gpt-not-standard (no self-consistent primary + backup table pair)"),
    };
    let unafs_src = super::unafsmirror::source().filter(|s| !s.is_ahci());
    let bp = match beside_plan(&rt, unafs_src.map(|s| s.sectors())) {
        Ok(b) => b,
        Err(why) => return beside_refuse(out, &why),
    };
    let (esp, ufs) = (bp.plan.parts[0].clone(), bp.plan.parts[1].clone());
    for line in bp.plan.lines() {
        serial_println!("[install] {}", line);
    }
    let Some(before) = part_heads(&rd, &rt) else { return beside_refuse(out, "unreadable (a partition head could not be read)") };
    // The payload, buffered whole BEFORE the first write.
    let Some(src) = running_source() else { return beside_refuse(out, "no-source (the running volume was not found)") };
    if source_on_port(src, port) {
        return beside_refuse(out, "source-on-target (the running volume is on this disk)");
    }
    let tree = match fat::mount_source(src).map_err(|_| InstallError::Io).and_then(|fs| super::clone::snapshot_capped(&fs, SNAP_MAX_FILE, SNAP_MAX_TOTAL)) {
        Ok(t) if tree_has_boot_files(&t) => t,
        Ok(_) => return beside_refuse(out, "not-bootable (the running volume lacks EFI/BOOT/BOOTX64.EFI or kernel.elf)"),
        Err(_) => return beside_refuse(out, "snapshot-failed (the running volume could not be read whole)"),
    };
    serial_println!("[install] snapshot src={} files={} bytes={}", src.name(), tree.file_count, tree.total_bytes);
    // The grants: free space (both new partitions; the UnaFS one alone for the slot) and the two tables.
    let Some(pg) = partition::mint_beside_part_grant(&consent, &rt, esp.first, ufs.last) else { return beside_refuse(out, "grant-refused (the planned extent is not free space)") };
    let Some(ug) = partition::mint_beside_part_grant(&consent, &rt, ufs.first, ufs.last) else { return beside_refuse(out, "grant-refused (the UnaFS extent is not free space)") };
    let Some((hg, tg)) = partition::mint_beside_table_grants(&consent, &rt) else { return beside_refuse(out, "grant-refused (the tables are not outside the usable range)") };
    let Some(_hold) = block::hold_ahci_grant(ug, block::GrantKind::Install) else { return beside_refuse(out, "grant-slot-held") };
    let t0 = crate::arch::ticks();
    let tt_rd = match super::BlockTarget::bind_id(sel) {
        Ok(t) => t,
        Err(_) => return beside_refuse(out, "target-gone (the disk left the registry)"),
    };
    let mut tt = TableTarget { rd: tt_rd, head: hg, tail: tg };
    if let Err(e) = super::gpt::add_entries(&mut tt, &rt, &[(bp.slots[0], esp.entry()), (bp.slots[1], ufs.entry())]) {
        serial_println!(":: INSTALL4: beside port={} table=fail err={:?} (the table read before the edit was written back) -> FAIL ::", port, e);
        out("install ssd --write --beside: the partition-table edit FAILED; the table was put back as it was read and nothing else was written");
        return;
    }
    serial_println!("[install] beside table=ok slots={},{} esp={}..{} unafs={}..{} (both headers, both arrays, CRCs, re-read)", bp.slots[0], bp.slots[1], esp.first, esp.last, ufs.first, ufs.last);
    let mut disk = AhciDisk { rd, grant: pg };
    let entry = super::gpt::GptEntryView { index: bp.slots[0], type_guid: super::gpt::ESP_TYPE_GUID, first_lba: esp.first, last_lba: esp.last };
    let w = match partition::write_partition(&mut disk, &entry, &tree, partition::no_grant()) {
        Ok(w) => w,
        Err(e) => {
            serial_println!(":: INSTALL4: beside port={} table=ok esp=fail err={:?} -> FAIL ::", port, e);
            out("install ssd --write --beside: the ESP format/clone FAILED — the new partitions are not trustworthy (the stranger's partitions were not written)");
            return;
        }
    };
    let (cloned_mib, fsck_tag, unafs_ok) = if unafs_src.is_some() { mirror_unafs(&mut disk, &bp.plan, unafs_src, out) } else { fresh_unafs(&mut disk, &bp.plan, port, out) };
    drop(tree);
    let unafs_ok = if unafs_ok && fsck_tag == "ok" {
        let (what, ok) = grow_unafs(&mut disk, &bp.plan);
        serial_println!("[install] beside grow {}", what);
        ok
    } else {
        unafs_ok
    };
    let after = part_heads(&disk.rd, &rt);
    let untouched = after.as_ref().map_or(0, |a| before.iter().filter(|&h| a.contains(h)).count());
    let ms = crate::arch::ticks().wrapping_sub(t0);
    let pass = w.files > 0 && w.verified == w.files && unafs_ok && fsck_tag == "ok" && untouched == before.len();
    serial_println!(
        ":: INSTALL4: beside port={} table=ok old={}/{} untouched={}/{} esp={} unafs={} cloned={} files={} verified={} fsck={} ms={} -> {} ::",
        port, rt.entries.len(), rt.entries.len(), untouched, before.len(),
        amber_core::sectors_mib(esp.sectors()), amber_core::sectors_mib(ufs.sectors()), cloned_mib,
        w.files, w.verified, fsck_tag, ms, if pass { "PASS" } else { "FAIL" }
    );
    serial_println!(
        ":: SELFINSTALL: files={} bytes={} ms={} verified={} -> {} ::",
        w.files, w.bytes, ms, w.verified, if pass { "PASS" } else { "FAIL" }
    );
    out(&alloc::format!(
        "install ssd --write --beside: {} files verified {}/{}, UnaFS fsck={}, the {} existing partitions {}{}",
        w.files, w.verified, w.files, fsck_tag, rt.entries.len(),
        if untouched == before.len() { "byte-identical" } else { "CHANGED" },
        if pass { " — reboot holding Option and pick EFI Boot" } else { " — NOT trustworthy" }
    ));
}
