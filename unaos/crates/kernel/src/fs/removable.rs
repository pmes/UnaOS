// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! CHARTER: Kernel — fs-core
//!
//! USBSTOR (rmbp-ledger B384, R95) — a removable disk is a volume while it is inserted.
//!
//! Flight 25: the card in the hub's USB reader was claimed after login (`STORSLOT: claim slot=6 …
//! diag=armed`) and NOTHING mounted it — `fs::bootdisk::survey` caches the first walk that bound a
//! root, so HOMESOIL's `others` is a boot-time list and nothing listened for an attach after it.
//!
//! The seam: no second store. The block registry (`drivers::block::USB_DISKS`) stays the one record
//! of what is attached and `USB_PUBLISH_GEN` its change signal (it moves on every publish AND every
//! retraction); the mount table stays rebuilt per verb by `fs::bootdisk::bind`, which asks [`bind`]
//! here for the removable mounts and no longer takes USB disks from the boot-time cache ([`owns`]);
//! Quarry's `volume_gen` (publish gen + `fs::NS_GEN`) carries the live update, and [`service`] bumps
//! `NS_GEN` after each change so a listing taken between the publish and the mount is re-read.
//!
//! Detach: the retraction empties the registry entry, the next pass drops the mount, and every open
//! path under it re-resolves through the mount table on its next operation and errors (-ENODEV: UNPLUG,
//! B703 — the name is DEPARTED until the same stick comes back and takes it again); a sector issued
//! against the dead slot is refused by the block layer (`NotReady`) — never a hang. A pull says
//! `[storage] pull vol=<n> -> unsafe dirty=<0|1>`; Quarry's sidebar Eject flushes first
//! (`[storage] eject vol=<n> flushed=<n> -> safe`). Fixture: `tests unplug`.
//!
//! What is mounted: the FIRST FAT volume on the disk (superfloppy, GPT or MBR — `fat::mount_source`'s
//! one rule). A disk with no FAT volume is said on the wire with what it carries instead
//! (`fs=exfat`, `fs=ntfs`, `fs=unknown`); an exFAT disk then mounts through `fs/exfat.rs` (EXFAT, B392) — a 64 GB SDXC card ships
//! exFAT — which is what flight 25's card most likely was (`0 FAT volume(s)` on it, f25:437).

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use crate::sync::Mutex;

use crate::fs::fat::{self, BlockSource};

/// One removable volume currently mounted under `/volumes`.
#[derive(Clone)]
struct RemVol {
    /// Registry index (0 is spelled `BlockSource::Usb`).
    ix: usize,
    /// The enumerator's identity — a replug lands on a new slot and reads as a new disk.
    slot_id: u8,
    num_blocks: u64,
    /// The unique leaf under `/volumes`.
    name: String,
    /// `fat16` / `fat32` / `exfat` (EXFAT, B392).
    fs: &'static str,
    /// UNPLUG (B703): the FAT volume's fingerprint `(BS_VolID, clusters)` — a replug of the same stick
    /// takes back the name it left under ([`DEPARTED`]). `None` on exFAT (named by its label alone).
    serial: Option<(u32, u32)>,
    /// UNPLUG (B703): where FAT[1]'s clean-shutdown bit lives on this volume. `None` on exFAT (the
    /// read-only backend writes nothing) or when the boot sector did not read back sane.
    fat1: Option<Fat1>,
    /// UNPLUG (B703): the write state every per-verb backend over this mount shares.
    st: Arc<VolState>,
}

impl RemVol {
    fn source(&self) -> BlockSource {
        source_of_ix(self.ix)
    }
}

/// The live removable mounts. Touched only from the main loop ([`service`]) and the per-verb table
/// builder ([`bind`]); never from an interrupt handler.
static MOUNTED: Mutex<Vec<RemVol>> = Mutex::new(Vec::new());
/// Disks seen but not mountable: `(slot_id, num_blocks)`, so a no-FAT card is said once per insert.
static REFUSED: Mutex<Vec<(u8, u64)>> = Mutex::new(Vec::new());
/// The `USB_PUBLISH_GEN` this module last reconciled against. `u64::MAX` = never.
static SEEN_GEN: AtomicU64 = AtomicU64::new(u64::MAX);
/// Witness facts.
static STICK_AT_BOOT: AtomicBool = AtomicBool::new(false);
static HOTPLUG: AtomicU8 = AtomicU8::new(0); // 0 none, 1 a post-boot mount/unmount landed
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// A USB disk published before this uptime is "in from power-on": the rMBP's xHCI enumerates a
/// cold-plugged disk ~7.5 s into the boot (f25 `BPACE: pci-usb t=7521ms`), and a hand plugging one
/// inside 20 s of the power button is not the case this separates.
const AT_BOOT_MS: u64 = 20_000;

fn source_of_ix(ix: usize) -> BlockSource {
    if ix == 0 { BlockSource::Usb } else { BlockSource::UsbN(ix as u8) }
}

/// Does the removable path own this source, so `bootdisk::bind` must not mount it from its boot-time
/// cache? x86 only: on aarch64 nothing calls [`service`] (the Pi's loop runs `piusb27_service`), so
/// the cache keeps its USB disks there exactly as before.
pub fn owns(source: BlockSource) -> bool {
    if !cfg!(target_arch = "x86_64") {
        return false;
    }
    match source {
        BlockSource::Usb | BlockSource::UsbN(_) => true,
        BlockSource::Default => fat::source_unit(BlockSource::Default).is_some(),
        #[allow(unreachable_patterns)]
        _ => false,
    }
}

/// `bootdisk::bind`'s half: mount every live removable volume into the per-verb table. Rebuilt from
/// the list [`service`] reconciles, so a detached disk is simply absent from the next table.
pub fn bind(mt: &mut crate::fs::vfs::MountTable) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    if !cfg!(target_arch = "x86_64") {
        return;
    }
    let vols = MOUNTED.lock().clone();
    for v in vols.iter() {
        let point = alloc::format!("{}/{}", crate::fs::bootdisk::VOLUMES, v.name);
        if v.fs == "exfat" { mt.mount(&point, alloc::boxed::Box::new(crate::fs::exfat::ExfatBackend::new(&v.name, v.source()))); continue; } // EXFAT (B392): the shared core's read-only backend
        let inner = FatBackend::new_source(&v.name, KERNEL_PRINCIPAL, true, v.source());
        mt.mount(&point, alloc::boxed::Box::new(RemFat { inner, ix: v.ix, fat1: v.fat1, st: v.st.clone() })); // UNPLUG (B703): the FAT backend, counted (the pull's `dirty=`) and dirty-marked on disk at its first write
    }
}

/// The names already taken under `/volumes` by the FIXED disks: the R89 aliases and HOMESOIL's
/// non-USB volumes. A removable volume takes the next free spelling (`Untitled 1`, the macOS shape).
fn fixed_names(s: &crate::fs::bootdisk::Survey) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    for n in ["UnaOS", "boot", "data"] {
        used.push(String::from(n));
    }
    for h in s.others.iter() {
        if !owns(h.source) {
            used.push(h.name.clone());
        }
    }
    used
}

/// What a disk with no FAT volume carries, by the OEM id of its first volume boot sector (LBA 0 for
/// a superfloppy, else MBR partition 1's start). A NAME for the wire, never a mount decision.
fn sniff(ix: usize, blocks: u64) -> (&'static str, u8) {
    let mut sec = [0u8; 512];
    if crate::drivers::block::read_block_usb_ix(ix, 0, &mut sec).is_err() {
        return ("unreadable", 0);
    }
    let oem = |s: &[u8]| -> Option<&'static str> {
        match &s[3..11] {
            b"EXFAT   " => Some("exfat"),
            b"NTFS    " => Some("ntfs"),
            _ => None,
        }
    };
    if let Some(k) = oem(&sec) {
        return (k, 0);
    }
    let Some(t) = crate::drivers::block::decode_mbr(&sec, blocks) else { return ("unknown", 0) };
    let Some(p) = t.iter().next() else { return ("unknown", 0) };
    let mut pbs = [0u8; 512];
    if crate::drivers::block::read_block_usb_ix(ix, p.start_lba, &mut pbs).is_err() {
        return ("unreadable", p.type_byte);
    }
    (oem(&pbs).unwrap_or(if p.type_byte == 0xEE { "gpt-nofat" } else { "unknown" }), p.type_byte)
}

/// The main-loop half. Cheap on the idle path (two atomic loads); on a registry change it re-reads
/// the registry, mounts what arrived, drops what left and says each on the wire. MUST run unmasked
/// and lock-free (sector reads) — `flight_recorder::service` is its caller, on every x86 loop.
pub fn service() {
    crate::drivers::block::pin_boot_medium_once();
    if !REGISTERED.swap(true, Ordering::Relaxed) {
        let _ = crate::tests::defer("usbstor", selftest);
        let _ = crate::tests::defer("exfat", crate::fs::exfat::selftest); // EXFAT (B392): `tests exfat`, never at boot (R80)
    }
    let g = crate::drivers::block::usb_publish_gen();
    if SEEN_GEN.swap(g, Ordering::AcqRel) == g {
        return;
    }
    reconcile();
}

fn reconcile() {
    let now = crate::arch::ms();
    let at_boot = now < AT_BOOT_MS;
    // Live registry, minus the disk this kernel is running from (QEMU's USB test disk, a card in a USB
    // reader): that one is `/`, `/boot`, never a removable volume.
    let survey = crate::fs::bootdisk::survey();
    let root_dev = survey.root.as_ref().and_then(|f| fat::source_device(f.source));
    let mut live: Vec<(usize, crate::drivers::block::BlockDeviceInfo)> = Vec::new();
    for ix in 0..crate::drivers::block::MAX_USB_DISKS {
        if let Some(d) = crate::drivers::block::usb_info_ix(ix) {
            if root_dev.map(|r| fat::same_device(&r, &d)).unwrap_or(false) {
                continue;
            }
            live.push((ix, d));
        }
    }
    if at_boot && !live.is_empty() {
        STICK_AT_BOOT.store(true, Ordering::Relaxed);
    }
    let mut changed = false;

    // Detach first, so a replug that reuses index 0 frees its old name before taking one.
    let gone: Vec<RemVol> = {
        let mut m = MOUNTED.lock();
        let (keep, gone): (Vec<RemVol>, Vec<RemVol>) = m.drain(..).partition(|v| {
            live.iter().any(|(ix, d)| *ix == v.ix && d.slot_id == v.slot_id && d.num_blocks == v.num_blocks)
        });
        *m = keep;
        gone
    };
    for v in gone.iter() {
        // UNPLUG (B703): a pull without Eject. `dirty` = a write reached this volume since its mount (or since
        // the last flush) — the FAT's own clean bit was cleared on disk at that first write, so the next OS to
        // mount the stick sees the same answer. The name departs: every path under it answers -ENODEV until a
        // replug of the same stick takes the name back.
        let dirty = v.st.writes.load(Ordering::Acquire) > 0;
        serial_println!("[storage] pull vol={} slot={} -> unsafe dirty={}", v.name, v.slot_id, dirty as u8);
        *LAST_PULL.lock() = Some((v.name.clone(), dirty));
        depart(&v.name, v.serial);
        serial_println!("[volumes] unmounted /volumes/{} reason=detached slot={} ::", v.name, v.slot_id); crate::video::notify(b"quarry", b"Disk ejected", alloc::format!("{} was removed", v.name).as_bytes(), b"", 0, b""); // NOTIFY (B418): the detach is a notification
        changed = true;
    }
    REFUSED.lock().retain(|(s, n)| live.iter().any(|(_, d)| d.slot_id == *s && d.num_blocks == *n));

    let mut used = fixed_names(&survey);
    for v in MOUNTED.lock().iter() {
        used.push(v.name.clone());
    }
    for (ix, d) in live.iter() {
        let held = MOUNTED.lock().iter().any(|v| v.ix == *ix && v.slot_id == d.slot_id && v.num_blocks == d.num_blocks);
        let refused = REFUSED.lock().iter().any(|(s, n)| *s == d.slot_id && *n == d.num_blocks) || ejected(d.slot_id, d.num_blocks); // QUARRY3 (B413): an ejected disk stays unmounted until it is replugged
        if held || refused {
            continue;
        }
        let src = source_of_ix(*ix);
        match fat::mount_source(src) {
            Ok(fs) => {
                let (name, _) = crate::fs::bootdisk::sanitize_label(&fs.label_raw());
                let fp = fs.volume_fingerprint();
                // UNPLUG (B703): the same stick back (its fingerprint departed) takes back its own name when free.
                let prior = DEPARTED.lock().iter().find(|(n, s)| *s == Some(fp) && !used.iter().any(|u| u == n)).map(|(n, _)| n.clone());
                let uniq = match prior {
                    Some(n) => {
                        used.push(n.clone());
                        n
                    }
                    None => crate::fs::bootdisk::next_volume_point(&mut used, &name).0,
                };
                undepart(&uniq);
                let fat1 = fat1_locate(*ix, &fs);
                let came_dirty = fat1.map(|f| fat1_clean(*ix, &f) == Some(false)).unwrap_or(false);
                let st = Arc::new(VolState { writes: AtomicU64::new(0), marked: AtomicBool::new(false), came_dirty });
                let kind = match fs.kind() {
                    fat::FatKind::Fat16 => "fat16",
                    fat::FatKind::Fat32 => "fat32",
                };
                serial_println!(
                    "[volumes] mounted /volumes/{} source={} slot={} fs={} removable=1 ::",
                    uniq, src.name(), d.slot_id, kind
                );
                crate::video::notify(b"quarry", b"Disk mounted", alloc::format!("{} is in Volumes", uniq).as_bytes(), b"Open", 2, alloc::format!("{}/{}", crate::fs::bootdisk::VOLUMES, uniq).as_bytes()); MOUNTED.lock().push(RemVol { ix: *ix, slot_id: d.slot_id, num_blocks: d.num_blocks, name: uniq, fs: kind, serial: Some(fp), fat1, st }); // NOTIFY (B418): the mount is a notification, `Open` = Quarry at the volume
                changed = true;
            }
            Err(e) => {
                let (kind, ty) = sniff(*ix, d.num_blocks);
                // EXFAT (B392): no FAT volume and the boot sector says exFAT — the shared core mounts it.
                let ex = if kind == "exfat" { Some(crate::fs::exfat::probe(*ix)) } else { None };
                if let Some(Ok(name)) = ex.as_ref() {
                    let (uniq, _point) = crate::fs::bootdisk::next_volume_point(&mut used, name);
                    undepart(&uniq); // UNPLUG (B703): the name is live again
                    serial_println!(
                        "[volumes] mounted /volumes/{} source={} slot={} fs=exfat removable=1 ::",
                        uniq, src.name(), d.slot_id
                    );
                    crate::video::notify(b"quarry", b"Disk mounted", alloc::format!("{} is in Volumes", uniq).as_bytes(), b"Open", 2, alloc::format!("{}/{}", crate::fs::bootdisk::VOLUMES, uniq).as_bytes()); MOUNTED.lock().push(RemVol { ix: *ix, slot_id: d.slot_id, num_blocks: d.num_blocks, name: uniq, fs: "exfat", serial: None, fat1: None, st: Arc::new(VolState { writes: AtomicU64::new(0), marked: AtomicBool::new(false), came_dirty: false }) }); // NOTIFY (B418)
                    changed = true;
                    continue;
                }
                let reason = match kind {
                    "exfat" => match ex { Some(Err(r)) => r, _ => "no-exfat-reader" },
                    "ntfs" => "no-ntfs-reader",
                    _ => match e { fat::FatError::NotFat => "no-fat-volume", fat::FatError::Io => "io", fat::FatError::Unsupported => "sector-size", _ => "mount-error" },
                };
                serial_println!(
                    "[volumes] not mounted source={} slot={} fs={} part_type={} reason={} ::",
                    src.name(), d.slot_id, kind, ty, reason
                );
                REFUSED.lock().push((d.slot_id, d.num_blocks));
                changed = true;
            }
        }
    }
    if changed {
        if !at_boot {
            HOTPLUG.store(1, Ordering::Relaxed);
        }
        crate::fs::NS_GEN.fetch_add(1, Ordering::Release); // Quarry's volume_gen moves: the sidebar and the Volumes listing re-read
        witness();
    }
}

/// `:: USBSTOR: … ::` — one line per removable change (an event line, R80) and by `tests usbstor`.
fn witness() {
    let kept = crate::drivers::block::boot_medium_kept();
    #[cfg(feature = "login")]
    let store = crate::fs::users::boot80_store_probe();
    #[cfg(not(feature = "login"))]
    let store = "-";
    let pw = if store == "dat" { "ok" } else { store };
    let names: Vec<String> = MOUNTED.lock().iter().map(|v| alloc::format!("{}:{}", v.name, v.fs)).collect();
    let refused = REFUSED.lock().len();
    let global_usb = fat::source_unit(BlockSource::Default).is_some();
    let ok = kept && !global_usb && pw == "ok" && refused == 0; let nsd = !kept && crate::drivers::block::boot_medium_not_sdhc(); let skip = nsd && refused == 0; // QEMUCLASS1 (B604): the pin's subject is a boot from the Sdhc card (R95); a boot the pin found elsewhere has no medium to keep — SKIP, unless a disk was refused (a red on any boot)
    let reason = if !kept {
        if skip { "boot-medium-not-sdhc" } else if nsd { "unmountable-disk" } else { "boot-medium-not-pinned" }
    } else if global_usb {
        "usb-in-global-slot"
    } else if pw != "ok" {
        "users-store"
    } else if refused > 0 {
        "unmountable-disk"
    } else {
        "-"
    };
    serial_println!(
        ":: USBSTOR: boot_medium_kept={} stick_at_boot={} pw_write={} mounted={} hotplug={} -> {} :: reason={} refused={} ::",
        kept as u8,
        STICK_AT_BOOT.load(Ordering::Relaxed) as u8,
        pw,
        if names.is_empty() { String::from("-") } else { names.join(",") },
        if HOTPLUG.load(Ordering::Relaxed) == 1 { "ok" } else { "-" },
        if ok { "PASS" } else if skip { "SKIP reason=boot-medium-not-sdhc" } else { "FAIL" },
        reason,
        refused
    );
}

/// `tests usbstor` — the same line, on demand.
pub fn selftest() {
    witness();
}

/// EXFAT (B392): the exFAT removables mounted now, `(registry index, name under /volumes)` — what
/// `tests exfat` audits.
pub fn exfat_mounts() -> Vec<(usize, String)> {
    MOUNTED.lock().iter().filter(|v| v.fs == "exfat").map(|v| (v.ix, v.name.clone())).collect()
}

// ── QUARRY3 (rmbp-ledger B413) — the sidebar's Locations and its eject ──────────────────────────────

/// Disks the person ejected while they stay inserted: `(slot_id, num_blocks)`. A replug is a new slot id and
/// mounts fresh; the list is bounded (the oldest record goes first).
static EJECTED: Mutex<Vec<(u8, u64)>> = Mutex::new(Vec::new());

fn ejected(slot_id: u8, num_blocks: u64) -> bool {
    EJECTED.lock().iter().any(|(s, n)| *s == slot_id && *n == num_blocks)
}

/// The removable volumes mounted now — their leaves under `/volumes` (Quarry's sidebar marks them ejectable).
pub fn mounted_names() -> Vec<String> {
    MOUNTED.lock().iter().map(|v| v.name.clone()).collect()
}

/// **Eject** `name` (a leaf under `/volumes`): the detach path's unmount — the mount leaves the table on the
/// next per-verb rebuild (`bind` reads [`MOUNTED`]), `NS_GEN` moves so every listing re-reads, and the disk is
/// parked so [`reconcile`] does not mount it again while it stays in. `false` = no such removable volume.
/// `[volumes] unmounted /volumes/<name> reason=eject slot=<n> ::`.
pub fn eject(name: &str) -> bool {
    let v = {
        let mut m = MOUNTED.lock();
        let Some(i) = m.iter().position(|v| v.name == name) else { return false };
        m.remove(i)
    };
    {
        let mut e = EJECTED.lock();
        if e.len() >= 8 {
            e.remove(0);
        }
        e.push((v.slot_id, v.num_blocks));
    }
    // UNPLUG (B703) M2: safe removal. The mount is already out of MOUNTED (no new per-verb table binds it), so
    // the flush is the last writer: the deferred dir entry, then FAT[1]'s clean bit set back in every FAT copy
    // when THIS mount cleared it, read back from copy 0. `flushed` = sectors the flush wrote.
    let (flushed, safe) = flush(&v);
    serial_println!(
        "[storage] eject vol={} flushed={} -> {} :: arrived_dirty={} writes={} ::",
        v.name, flushed, if safe { "safe" } else { "unsafe" }, v.st.came_dirty as u8, v.st.writes.load(Ordering::Acquire)
    );
    *LAST_EJECT.lock() = Some((v.name.clone(), flushed, safe));
    depart(&v.name, v.serial);
    serial_println!("[volumes] unmounted /volumes/{} reason=eject slot={} ::", v.name, v.slot_id);
    crate::fs::NS_GEN.fetch_add(1, Ordering::Release);
    true
}

// ── UNPLUG (rmbp-ledger B703) — detach is an unmount; Eject is a flush ────────────────────────────────────────

/// One mount's write state, shared (`Arc`) by every per-verb backend [`bind`] builds over the volume and by
/// [`MOUNTED`]'s record, so the pull line and the Eject read what the writers counted.
struct VolState {
    /// Successful mutations through the mount since it was mounted (or since the last flush).
    writes: AtomicU64,
    /// This mount cleared FAT[1]'s clean bit on disk (at its first write) and owes setting it back.
    marked: AtomicBool,
    /// The volume arrived with the clean bit already clear (another OS's unsafe pull, or ours): it stays
    /// dirty until something that checks it says otherwise — an Eject never claims it clean.
    came_dirty: bool,
}

/// Where FAT[1]'s clean-shutdown bit lives: the sector of the FIRST FAT holding entry 1, the stride to the next
/// copy, the copy count, the byte offset of the entry and the bit (FAT16 bit 15, FAT32 bit 27; set = clean).
#[derive(Clone, Copy)]
struct Fat1 {
    lba0: u64,
    stride: u64,
    copies: u32,
    off: usize,
    mask: u32,
    wide: bool,
}

/// Read the volume's boot sector off its own registry entry and place FAT[1]. Cross-checked against the mount's
/// own geometry (`num_fats`); anything that does not read back sane is `None` and the volume is simply never
/// dirty-marked (its writes are still counted).
fn fat1_locate(ix: usize, fs: &fat::FatFs) -> Option<Fat1> {
    let part_lba = fs.extent().0;
    let mut bs = [0u8; 512];
    crate::drivers::block::read_block_usb_ix(ix, part_lba, &mut bs).ok()?;
    let le16 = |o: usize| u16::from_le_bytes([bs[o], bs[o + 1]]) as u64;
    let le32 = |o: usize| u32::from_le_bytes([bs[o], bs[o + 1], bs[o + 2], bs[o + 3]]) as u64;
    if le16(11) != 512 {
        return None;
    }
    let rsvd = le16(14);
    let copies = bs[16] as u32;
    let fat_sz = if le16(22) != 0 { le16(22) } else { le32(36) };
    if rsvd == 0 || fat_sz == 0 || copies == 0 || copies != fs.num_fats() {
        return None;
    }
    let wide = matches!(fs.kind(), fat::FatKind::Fat32);
    Some(Fat1 {
        lba0: part_lba + rsvd,
        stride: fat_sz,
        copies,
        off: if wide { 4 } else { 2 },
        mask: if wide { 0x0800_0000 } else { 0x8000 },
        wide,
    })
}

fn fat1_word(sec: &[u8; 512], f: &Fat1) -> u32 {
    if f.wide {
        u32::from_le_bytes([sec[f.off], sec[f.off + 1], sec[f.off + 2], sec[f.off + 3]])
    } else {
        u16::from_le_bytes([sec[f.off], sec[f.off + 1]]) as u32
    }
}

/// Copy 0's clean bit: `Some(true)` = clean. `None` = unreadable (the stick is gone).
fn fat1_clean(ix: usize, f: &Fat1) -> Option<bool> {
    let mut sec = [0u8; 512];
    crate::drivers::block::read_block_usb_ix(ix, f.lba0, &mut sec).ok()?;
    Some(fat1_word(&sec, f) & f.mask != 0)
}

/// Set (`clean`) or clear the bit in EVERY FAT copy (a one-copy edit is a disagreeing volume), read-modify-write
/// of the one sector per copy; the volume's write posture is asked first (BP-storage-5: `write_veto` is the one
/// definition). Returns the sectors written.
fn fat1_set(ix: usize, f: &Fat1, clean: bool) -> Result<u32, ()> {
    if source_of_ix(ix).write_veto().is_some() {
        return Err(());
    }
    let mut n = 0u32;
    for c in 0..f.copies as u64 {
        let lba = f.lba0 + c * f.stride;
        let mut sec = [0u8; 512];
        crate::drivers::block::read_block_usb_ix(ix, lba, &mut sec).map_err(|_| ())?;
        let w = fat1_word(&sec, f);
        let nw = if clean { w | f.mask } else { w & !f.mask };
        if nw == w {
            continue;
        }
        if f.wide {
            sec[f.off..f.off + 4].copy_from_slice(&nw.to_le_bytes());
        } else {
            sec[f.off..f.off + 2].copy_from_slice(&(nw as u16).to_le_bytes());
        }
        crate::drivers::block::write_block_usb_ix(ix, lba, &sec).map_err(|_| ())?;
        n += 1;
    }
    Ok(n)
}

/// Eject's flush: `(sectors written, safe)`. Safe = everything this mount wrote is on the medium and, when this
/// mount cleared the clean bit, copy 0 reads it set again. An exFAT mount (read-only) has nothing to flush.
fn flush(v: &RemVol) -> (u32, bool) {
    let mut n = 0u32;
    if fat::dirent_pending() {
        fat::flush_pending_dirent_by("eject");
        if !fat::dirent_pending() {
            n += 1;
        }
    }
    let safe = match v.fat1 {
        Some(f) if v.st.marked.load(Ordering::Acquire) => match fat1_set(v.ix, &f, true) {
            Ok(w) => {
                n += w;
                v.st.marked.store(false, Ordering::Release);
                fat1_clean(v.ix, &f) == Some(true)
            }
            Err(()) => false,
        },
        _ => crate::drivers::block::usb_disk_present(v.ix),
    };
    if safe {
        v.st.writes.store(0, Ordering::Release);
    }
    (n, safe)
}

/// The FAT backend over a removable mount: every verb is the FAT backend's own; a MUTATING verb first clears the
/// on-disk clean bit (once per mount, before the first sector moves — so a pull mid-write leaves the volume
/// marked dirty for whoever mounts it next) and counts its success.
struct RemFat {
    inner: crate::fs::vfs::FatBackend,
    ix: usize,
    fat1: Option<Fat1>,
    st: Arc<VolState>,
}

impl RemFat {
    fn mutate<T>(&self, f: impl FnOnce() -> Result<T, crate::fs::vfs::VfsError>) -> Result<T, crate::fs::vfs::VfsError> {
        use crate::fs::vfs::VfsBackend;
        if let Some(f1) = self.fat1 {
            if !self.st.came_dirty
                && self.inner.write_veto().is_none()
                && self.st.marked.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
                && fat1_set(self.ix, &f1, false).is_err()
            {
                self.st.marked.store(false, Ordering::Release);
            }
        }
        let r = f();
        if r.is_ok() {
            self.st.writes.fetch_add(1, Ordering::AcqRel);
        }
        r
    }
}

impl crate::fs::vfs::VfsBackend for RemFat {
    fn volume_name(&self) -> &str { self.inner.volume_name() }
    fn mount_root(&self) -> &str { self.inner.mount_root() }
    fn on_volume(&self, rel: &str) -> String { self.inner.on_volume(rel) }
    fn read_dir(&self, rel: &str) -> Result<Vec<crate::fs::vfs::DirEnt>, crate::fs::vfs::VfsError> { self.inner.read_dir(rel) }
    fn read_dir_kinds(&self, rel: &str) -> Result<Vec<(String, crate::fs::vfs::NodeKind)>, crate::fs::vfs::VfsError> { self.inner.read_dir_kinds(rel) }
    fn stat(&self, rel: &str) -> Result<crate::fs::vfs::Stat, crate::fs::vfs::VfsError> { self.inner.stat(rel) }
    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, crate::fs::vfs::VfsError> { self.inner.read(rel, offset, len) }
    fn authorize_read(&self, rel: &str, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.inner.authorize_read(rel, p) }
    fn open_read(&self, rel: &str, p: &str) -> Result<crate::fs::vfs::Stat, crate::fs::vfs::VfsError> { self.inner.open_read(rel, p) }
    fn authorize_write(&self, rel: &str, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.inner.authorize_write(rel, p) }
    fn create(&self, rel: &str, kind: crate::fs::vfs::NodeKind, p: &str) -> Result<crate::fs::vfs::Stat, crate::fs::vfs::VfsError> { self.mutate(|| self.inner.create(rel, kind, p)) }
    fn write(&self, rel: &str, offset: u64, data: &[u8], p: &str) -> Result<usize, crate::fs::vfs::VfsError> { self.mutate(|| self.inner.write(rel, offset, data, p)) }
    fn truncate(&self, rel: &str, size: u64, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.mutate(|| self.inner.truncate(rel, size, p)) }
    fn unlink(&self, rel: &str, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.mutate(|| self.inner.unlink(rel, p)) }
    fn write_veto(&self) -> Option<&'static str> { self.inner.write_veto() }
    fn rename(&self, from: &str, to: &str, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.mutate(|| self.inner.rename(from, to, p)) }
    fn remove_dir(&self, rel: &str, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.mutate(|| self.inner.remove_dir(rel, p)) }
    fn remove_attr(&self, rel: &str, key: &str, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.mutate(|| self.inner.remove_attr(rel, key, p)) }
    fn set_attr(&self, rel: &str, key: &str, value: crate::fs::vfs::AttrValue, p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.mutate(|| self.inner.set_attr(rel, key, value, p)) }
    fn set_attrs(&self, rel: &str, kv: &[(String, Option<crate::fs::vfs::AttrValue>)], p: &str) -> Result<(), crate::fs::vfs::VfsError> { self.mutate(|| self.inner.set_attrs(rel, kv, p)) }
    fn create_files_batch(&self, rel: &str, files: Vec<(String, Vec<(String, crate::fs::vfs::AttrValue)>)>, p: &str) -> Result<usize, crate::fs::vfs::VfsError> { self.mutate(|| self.inner.create_files_batch(rel, files, p)) }
    fn get_attr(&self, rel: &str, key: &str, p: &str) -> Result<crate::fs::vfs::AttrValue, crate::fs::vfs::VfsError> { self.inner.get_attr(rel, key, p) }
    fn list_attrs(&self, rel: &str, p: &str) -> Result<Vec<(String, crate::fs::vfs::AttrValue)>, crate::fs::vfs::VfsError> { self.inner.list_attrs(rel, p) }
    fn query(&self, expr: &str, p: &str) -> Result<Vec<(u64, String)>, crate::fs::vfs::VfsError> { self.inner.query(expr, p) }
    fn volume_bytes(&self) -> Option<u64> { self.inner.volume_bytes() }
    fn describe(&self) -> Option<String> { self.inner.describe() }
    fn volume_id(&self) -> Option<u64> { self.inner.volume_id() }
}

/// Names that left (a pull, or an Eject) and have not come back: `(leaf, FAT fingerprint)`. Bounded (the oldest
/// goes first). [`departed_name`] is what `vfs::MountTable::resolve` asks, so a path under one answers
/// `NoSuchVolume` (-ENODEV) instead of falling through to the root's -ENOENT.
static DEPARTED: Mutex<Vec<(String, Option<(u32, u32)>)>> = Mutex::new(Vec::new());
/// `DEPARTED.len()`, so the resolver's idle path is one atomic load and never a lock.
static DEPARTED_N: AtomicUsize = AtomicUsize::new(0);
/// The last pull `(name, dirty)` and the last Eject `(name, flushed, safe)`, for `tests unplug`.
static LAST_PULL: Mutex<Option<(String, bool)>> = Mutex::new(None);
static LAST_EJECT: Mutex<Option<(String, u32, bool)>> = Mutex::new(None);

fn depart(name: &str, serial: Option<(u32, u32)>) {
    let mut d = DEPARTED.lock();
    d.retain(|(n, _)| n != name);
    if d.len() >= 8 {
        d.remove(0);
    }
    d.push((String::from(name), serial));
    DEPARTED_N.store(d.len(), Ordering::Release);
}

fn undepart(name: &str) {
    let mut d = DEPARTED.lock();
    d.retain(|(n, _)| n != name);
    DEPARTED_N.store(d.len(), Ordering::Release);
}

/// The departed volume `path` lies on (`/volumes/<name>` or below), if any.
pub fn departed_name(path: &str) -> Option<String> {
    if DEPARTED_N.load(Ordering::Acquire) == 0 {
        return None;
    }
    let rest = path.strip_prefix(crate::fs::bootdisk::VOLUMES)?.strip_prefix('/')?;
    let leaf = rest.split('/').next().unwrap_or("");
    if leaf.is_empty() {
        return None;
    }
    DEPARTED.lock().iter().find(|(n, _)| n == leaf).map(|(n, _)| n.clone())
}

/// `MountTable::resolve`'s question: is `path` on a volume that left?
pub fn departed(path: &str) -> bool {
    departed_name(path).is_some()
}

/// `tests unplug`: the first FAT removable mounted now, `(name, registry index)`.
pub fn unplug_target() -> Option<(String, usize)> {
    MOUNTED.lock().iter().find(|v| v.fs != "exfat").map(|v| (v.name.clone(), v.ix))
}

/// `tests unplug`: the last pull `(name, dirty)`.
pub fn last_pull() -> Option<(String, bool)> {
    LAST_PULL.lock().clone()
}

/// `tests unplug`: the last Eject `(name, flushed, safe)`.
pub fn last_eject() -> Option<(String, u32, bool)> {
    LAST_EJECT.lock().clone()
}
