//! CHARTER: Kernel — fs-core
//!
//! R99 (Peter, 2026-10-06): "the boot fat partition is sacred and should even be difficult for root to do much with let
//! alone have system apps and libs there to confuse things." — ROOTDISK2 (rmbp-ledger B401).
//!
//! * **Read-only for every principal, root (the shell's `kernel` principal) included.** Once `bootdisk::bind_root`
//!   binds an x86 native UnaFS root ([`arm`]), every write verb the VFS hands a FAT mount of the boot volume (`/boot`,
//!   `/volumes/boot` — the backends named `boot`) is refused in `FatBackend::authorize_write` ([`refuse`]) with
//!   `[boot] fat write refused path=… by=<principal> (R99: sacred) ::` and `-EROFS`-shaped `Unsupported`.
//! * **The one writer**: the installer/updater path, through [`fat_unlock`] — a named guard that prints
//!   `[boot] fat unlocked by=<installer|updater> for=<write> …` and relocks (with a line) when dropped.
//! * **Nothing else lives there**: `/apps`, `/lib` (ROOTDISK2), `/var` (incl. the boot log, `fs::bootlog`) and `/home`
//!   are on UnaFS; the ESP keeps the loader, the kernel, the boot configuration and the firmware blobs.
//! * A FAT-root boot (no UnaFS on the boot disk: the two-device and QEMU shapes) is NOT armed — its FAT is `/`.
//!
//! Witness: `tests bootfat` → `:: BOOTFAT: write_as_root=refused … unlock_path=installer relocked=1 -> PASS ::`.

use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static SACRED: AtomicBool = AtomicBool::new(false);
static ANNOUNCED: AtomicBool = AtomicBool::new(false);
/// Open unlock guards (the installer and the updater may nest; the gate is open while any is held).
static UNLOCKS: AtomicU32 = AtomicU32::new(0);
static REFUSED: AtomicU32 = AtomicU32::new(0);
/// The backend volume name every mount of the boot FAT carries (`bind_root`, `volumes::bind_aliases`).
const BOOT_VOLUME: &str = "boot";

/// `bootdisk::bind_root` on an x86 native root: the boot FAT becomes sacred for the rest of the boot.
pub fn arm(announce: bool) {
    SACRED.store(true, Ordering::Relaxed);
    if announce && !ANNOUNCED.swap(true, Ordering::Relaxed) {
        serial_println!("[boot] fat sacred: /boot and /volumes/boot are read-only for every principal; writes only through fat_unlock (R99) ::");
    }
}

/// Is the boot FAT read-only now?
pub fn sacred() -> bool {
    SACRED.load(Ordering::Relaxed)
}

/// Would a write to the boot FAT pass the gate right now?
pub fn allows() -> bool {
    !sacred() || UNLOCKS.load(Ordering::Acquire) > 0
}

/// `FatBackend::authorize_write`, FIRST: `true` = refuse this write (said on the wire). Only the boot volume's
/// mounts are governed; every other FAT volume (a stick, another disk) keeps its own posture.
pub fn refuse(volume: &str, root: &str, rel: &str, principal: &str) -> bool {
    if volume != BOOT_VOLUME || allows() {
        return false;
    }
    REFUSED.fetch_add(1, Ordering::Relaxed);
    serial_println!("[boot] fat write refused path=/boot{}{} by={} (R99: sacred) ::", root, rel, principal);
    true
}

/// The named unlock. Held only by the installer/updater for one write; dropped = relocked.
pub struct FatUnlock {
    by: &'static str,
}

/// Open the boot FAT for ONE named write (`by` = `installer` or `updater`, `for_what` = what is written).
pub fn fat_unlock(by: &'static str, for_what: &str) -> FatUnlock {
    UNLOCKS.fetch_add(1, Ordering::AcqRel);
    serial_println!("[boot] fat unlocked by={} for={} (R99: installer/updater only; relocks on drop) ::", by, for_what);
    FatUnlock { by }
}

impl Drop for FatUnlock {
    fn drop(&mut self) {
        UNLOCKS.fetch_sub(1, Ordering::AcqRel);
        serial_println!("[boot] fat relocked by={} ::", self.by);
    }
}

/// Quarry: does `name` in `dir` draw the read-only lock (the boot volume, under Volumes)?
pub fn shows_lock(dir: &str, name: &str) -> bool {
    sacred() && dir.trim_end_matches('/') == "/volumes" && name == BOOT_VOLUME
}

/// `tests bootfat` — one write as root (the shell's principal) at both spellings of the boot FAT, read back as a
/// refusal with nothing created; then the installer's unlock opens the gate and its drop closes it again.
pub fn selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    if !sacred() {
        serial_println!(":: BOOTFAT: write_as_root=- unlock_path=- -> SKIP :: reason=fat-root (no UnaFS root: the FAT is /) ::");
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    let before = REFUSED.load(Ordering::Relaxed);
    let mut toks: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    let mut ok = true;
    for p in ["/volumes/boot/R99PROBE.TXT", "/boot/R99PROBE.TXT"] {
        let r = mt.create(p, NodeKind::File, KERNEL_PRINCIPAL);
        let absent = mt.stat(p).is_err();
        let t = match r {
            Err(e) => alloc::format!("refused({:?})", e),
            Ok(_) => String::from("WRITTEN"),
        };
        ok &= t.starts_with("refused") && absent;
        toks.push(t);
    }
    let said = REFUSED.load(Ordering::Relaxed) - before;
    let gate = {
        let u = fat_unlock("installer", "tests bootfat gate (no write)");
        let open = allows();
        drop(u);
        open && !allows()
    };
    ok &= said >= 2 && gate;
    serial_println!(
        ":: BOOTFAT: write_as_root={} unlock_path={} relocked={} -> {} :: refusals_said={} ::",
        if toks.iter().all(|t| t.starts_with("refused")) { "refused" } else { "WRITTEN" },
        if gate { "installer" } else { "broken" },
        !allows() as u8,
        if ok { "PASS" } else { "FAIL" },
        said
    );
    for t in toks {
        serial_println!("[boot] bootfat leg {} ::", t);
    }
}

/// `tests rootdisk` (ROOTDISK2 line): `readonly` when a create as root under `/volumes/boot` is refused and nothing
/// was made; `writable` / `unarmed` otherwise.
pub fn posture(mt: &crate::fs::vfs::MountTable) -> &'static str {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    if !sacred() {
        return "unarmed";
    }
    let p = "/volumes/boot/R99PROBE.TXT";
    match mt.create(p, NodeKind::File, KERNEL_PRINCIPAL) {
        Err(_) if mt.stat(p).is_err() => "readonly",
        _ => "writable",
    }
}

/// `tests rootdisk`: the installer's unlock opens the gate and its drop closes it (`installer`), else `broken`.
pub fn unlock_path() -> &'static str {
    let u = fat_unlock("installer", "tests rootdisk gate (no write)");
    let open = allows();
    drop(u);
    if open && !allows() { "installer" } else { "broken" }
}
