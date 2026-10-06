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
//! path under it re-resolves through the mount table on its next operation and errors (ENOENT); a
//! sector issued against the dead slot is refused by the block layer (`NotReady`) — never a hang.
//!
//! What is mounted: the FIRST FAT volume on the disk (superfloppy, GPT or MBR — `fat::mount_source`'s
//! one rule). A disk with no FAT volume is said on the wire with what it carries instead
//! (`fs=exfat`, `fs=ntfs`, `fs=unknown`): this tree has no exFAT reader, and a 64 GB SDXC card ships
//! exFAT — which is what flight 25's card most likely was (`0 FAT volume(s)` on it, f25:437).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

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
    /// `fat16` / `fat32`.
    fs: &'static str,
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
        mt.mount(&point, alloc::boxed::Box::new(FatBackend::new_source(&v.name, KERNEL_PRINCIPAL, true, v.source())));
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
        serial_println!("[volumes] unmounted /volumes/{} reason=detached slot={} ::", v.name, v.slot_id);
        changed = true;
    }
    REFUSED.lock().retain(|(s, n)| live.iter().any(|(_, d)| d.slot_id == *s && d.num_blocks == *n));

    let mut used = fixed_names(&survey);
    for v in MOUNTED.lock().iter() {
        used.push(v.name.clone());
    }
    for (ix, d) in live.iter() {
        let held = MOUNTED.lock().iter().any(|v| v.ix == *ix && v.slot_id == d.slot_id && v.num_blocks == d.num_blocks);
        let refused = REFUSED.lock().iter().any(|(s, n)| *s == d.slot_id && *n == d.num_blocks);
        if held || refused {
            continue;
        }
        let src = source_of_ix(*ix);
        match fat::mount_source(src) {
            Ok(fs) => {
                let (name, _) = crate::fs::bootdisk::sanitize_label(&fs.label_raw());
                let (uniq, _point) = crate::fs::bootdisk::next_volume_point(&mut used, &name);
                let kind = match fs.kind() {
                    fat::FatKind::Fat16 => "fat16",
                    fat::FatKind::Fat32 => "fat32",
                };
                serial_println!(
                    "[volumes] mounted /volumes/{} source={} slot={} fs={} removable=1 ::",
                    uniq, src.name(), d.slot_id, kind
                );
                MOUNTED.lock().push(RemVol { ix: *ix, slot_id: d.slot_id, num_blocks: d.num_blocks, name: uniq, fs: kind });
                changed = true;
            }
            Err(e) => {
                let (kind, ty) = sniff(*ix, d.num_blocks);
                let reason = match kind {
                    "exfat" => "no-exfat-reader",
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
    let ok = kept && !global_usb && pw == "ok" && refused == 0;
    let reason = if !kept {
        "boot-medium-not-pinned"
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
        if ok { "PASS" } else { "FAIL" },
        reason,
        refused
    );
}

/// `tests usbstor` — the same line, on demand.
pub fn selftest() {
    witness();
}
