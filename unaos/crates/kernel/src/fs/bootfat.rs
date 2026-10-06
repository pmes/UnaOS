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
    if !rel.contains("R99PROBE") { WRITERS.fetch_add(1, Ordering::Relaxed); } // SMALLFIX3 (B416): a refusal that is not `tests bootfat`'s own probe is a fixture writing to /boot
    serial_println!("[boot] fat write refused path=/boot{}{} by={} (R99: sacred) ::", root, rel, principal);
    true
}

/// The named unlock. Held only by the installer/updater for one write; dropped = relocked.
pub struct FatUnlock {
    by: &'static str,
    /// SMALLFIX3 (B416): `false` when the administrator's authority refused it — the gate stays shut.
    held: bool,
}

/// Open the boot FAT for ONE named write (`by` = `installer` or `updater`, `for_what` = what is written).
pub fn fat_unlock(writer: Writer, for_what: &str) -> FatUnlock {
    let by = writer.name(); // BOOTFATSEAM (B453): the caller names itself by TYPE — only the installer and the updater can
    // SMALLFIX3 (B416, R100): the one writer path asks the administrator's authority once (`[auth] admin=… for=boot-fat-write`).
    #[cfg(feature = "login")]
    AUTH_ASKS.fetch_add(1, Ordering::Relaxed); // BOOTFATSEAM (B453): `tests bootfatseam` reads that the gate asked
    #[cfg(feature = "login")]
    if crate::fs::users::admin_authority("boot-fat-write").is_err() {
        serial_println!("[boot] fat unlock refused by={} for={} (R100: not the administrator) ::", by, for_what);
        return FatUnlock { by, held: false };
    }
    UNLOCKS.fetch_add(1, Ordering::AcqRel);
    serial_println!("[boot] fat unlocked by={} for={} (R99: installer/updater only; relocks on drop) ::", by, for_what);
    FatUnlock { by, held: true }
}

impl Drop for FatUnlock {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
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
        let u = fat_unlock(Writer::Installer, "tests bootfat gate (no write)");
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
    let u = fat_unlock(Writer::Installer, "tests rootdisk gate (no write)");
    let open = allows();
    drop(u);
    if open && !allows() { "installer" } else { "broken" }
}

// SMALLFIX3 (rmbp-ledger B416) — the boot FAT's refusals, asked BEFORE a write and counted after one. Tail.
/// Refusals of a write that was NOT `tests bootfat`'s own `R99PROBE` — a fixture that still writes to `/boot`.
static WRITERS: AtomicU32 = AtomicU32::new(0);

/// `FatBackend::write_veto`, first: the reason a write to `volume` would be refused now, without saying it on
/// the wire (the veto is a question, the refusal in [`refuse`] is the event).
pub fn veto(volume: &str) -> Option<&'static str> {
    if volume == BOOT_VOLUME && !allows() { Some("the boot FAT is sacred (R99): read-only for every principal") } else { None }
}

/// `tests smallfix3`'s `boot_writers=`: `None` when the gate is not armed (a FAT-root boot: the FAT is `/`).
pub fn writers() -> Option<u32> {
    if sacred() { Some(WRITERS.load(Ordering::Relaxed)) } else { None }
}

// =========================================================================================
// BOOTFATSEAM (rmbp-ledger B453, SECREVIEW F3, R99) — the gate at the FAT layer itself (tail)
// =========================================================================================
// The VFS gate above (`refuse`, `veto`) governs the FAT MOUNTS named `boot`. Code that holds a raw `FatFs` never
// passes it — the users store, the holocron store, the FAT-LFN witness, `src extract`. So `bind_root` hands this
// module the boot FAT's `BlockSource` ([`arm_on`]) and the FAT layer asks it twice: `FatFs::write_veto` (the
// question every raw writer already asks: [`veto_source`]) and `fat::write_sector` / `write_sectors` (the one place
// every FAT write passes: [`raw_guard`] — a refusal there is a writer that did not ask, counted and said).
// `fat_unlock` opens both layers; it takes a [`Writer`], so only the installer and the updater can name themselves.

/// The two writers R99 admits. Neither writes the boot FAT on x86 today (the installer writes the TARGET disk at the
/// block layer); the type is the rule, so a third caller is a compile-time decision, not a string.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    Installer,
    Updater,
}

impl Writer {
    pub fn name(self) -> &'static str {
        match self {
            Writer::Installer => "installer",
            Writer::Updater => "updater",
        }
    }
}

/// The refusal string of a raw write to the sacred boot FAT (FAT-LFN compares against it for `reason=r99-sacred`).
pub const SACRED_VETO: &str = "the boot FAT is sacred (R99): read-only for every principal, root included; writes only through fat_unlock";

/// The boot FAT's source key (0 = none bound: a FAT-root boot, or before `bind_root`).
static BOOT_SRC: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
/// Raw writes refused at `write_sector` / `write_sectors` — writers that did not ask `write_veto` first.
static RAW_REFUSED: AtomicU32 = AtomicU32::new(0);
/// Of those, the ones `tests bootfatseam`'s own probe caused (every run's).
static PROBE_REFUSED: AtomicU32 = AtomicU32::new(0);
/// `fat_unlock` calls that asked `admin_authority`.
static AUTH_ASKS: AtomicU32 = AtomicU32::new(0);
/// The FAT-LFN witness skipped on the sacred FAT (`reason=r99-sacred`).
static LFN_SKIPPED: AtomicBool = AtomicBool::new(false);

/// One number per medium. `Usb` IS `UsbN(0)` (the same registry entry), so both spellings key alike.
fn key(s: crate::fs::fat::BlockSource) -> u16 {
    use crate::fs::fat::BlockSource as B;
    match s {
        B::Default => 1,
        B::Usb => 0x200,
        B::UsbN(n) => 0x200 | n as u16,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        B::Sdhc => 3,
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        B::SdMmc => 4,
        #[cfg(all(target_arch = "x86_64", feature = "ahci"))]
        B::Ahci(p) => 0x400 | p as u16,
    }
}

/// `bootdisk::bind_root` on a native root: the boot FAT is `src`'s, and it is sacred from now on.
pub fn arm_on(src: crate::fs::fat::BlockSource, announce: bool) {
    BOOT_SRC.store(key(src), Ordering::Relaxed);
    arm(announce);
}

fn governs(src: crate::fs::fat::BlockSource) -> bool {
    let k = BOOT_SRC.load(Ordering::Relaxed);
    k != 0 && k == key(src) && !allows()
}

/// `FatFs::write_veto`, FIRST: the R99 refusal for a raw `FatFs` on the boot FAT's medium while the gate is shut.
pub fn veto_source(src: crate::fs::fat::BlockSource) -> Option<&'static str> {
    if governs(src) { Some(SACRED_VETO) } else { None }
}

/// `fat::write_sector` / `write_sectors`, FIRST: refuse a write to the sacred boot FAT that reached the medium
/// layer without asking. Said on the wire (the first eight; every one is counted).
pub fn raw_guard(src: crate::fs::fat::BlockSource, site: &str, lba: u64) -> Result<(), crate::fs::fat::FatError> {
    if !governs(src) {
        return Ok(());
    }
    if RAW_REFUSED.fetch_add(1, Ordering::Relaxed) < 8 {
        serial_println!("[boot] fat raw write refused source={} lba={} site={} (R99: sacred; a direct writer) ::", src.name(), lba, site);
    }
    Err(crate::fs::fat::FatError::Unsupported)
}

/// FAT-LFN's skip on the sacred FAT.
pub fn note_lfn_skip() {
    LFN_SKIPPED.store(true, Ordering::Relaxed);
}

/// `tests bootfatseam` — the line the arc is read by:
/// `:: BOOTFATSEAM: direct_writers=0 users=unafs holocron=unafs lfn=skip fat_unlock=authority -> PASS ::`.
/// A raw `FatFs` write at the boot FAT (a create of `R99RAW.TXT`) must be refused by `write_veto` AND, asked
/// without it, by the sector layer — nothing created; the stores must be on UnaFS; the FAT-LFN witness must
/// skip; `fat_unlock` must ask the administrator's authority. Run on demand only (R80).
pub fn bootfatseam_selftest() {
    if !sacred() {
        serial_println!(":: BOOTFATSEAM: direct_writers=- users=- holocron=- lfn=- fat_unlock=- -> SKIP :: reason=fat-root (no UnaFS root: the FAT is /) ::");
        return;
    }
    let k = BOOT_SRC.load(Ordering::Relaxed);
    let src = [crate::fs::fat::BlockSource::Default, crate::fs::fat::BlockSource::Usb]
        .into_iter()
        .chain(boot_candidates())
        .find(|s| key(*s) == k);
    let raw_before = RAW_REFUSED.load(Ordering::Relaxed);
    // Leg 1: the question a raw writer asks, and the sector layer under a writer that does not ask.
    let (asked, unasked) = match src.and_then(|s| crate::fs::fat::mount_source(s).ok()) {
        Some(fs) => {
            let asked = fs.write_veto() == Some(SACRED_VETO);
            let made = fs.create_in_dir(0, "R99RAW.TXT", 0x20).is_ok();
            let absent = fs.locate_in_dir(0, "R99RAW.TXT").is_err();
            (asked, !made && absent)
        }
        None => (false, false),
    };
    let probe = RAW_REFUSED.load(Ordering::Relaxed) - raw_before;
    let probes = PROBE_REFUSED.fetch_add(probe, Ordering::Relaxed) + probe;
    let direct = RAW_REFUSED.load(Ordering::Relaxed) - probes; // refusals that were NOT this fixture's own probes
    #[cfg(feature = "login")]
    let users = crate::fs::users::seat_word();
    #[cfg(not(feature = "login"))]
    let users = "unbuilt";
    #[cfg(feature = "holocron")]
    let holocron = crate::fs::holocron::seat_word();
    #[cfg(not(feature = "holocron"))]
    let holocron = "unbuilt";
    #[cfg(feature = "witness")]
    let lfn = if LFN_SKIPPED.load(Ordering::Relaxed) || src.map(|s| veto_source(s).is_some()).unwrap_or(false) { "skip" } else { "RUNS" };
    #[cfg(not(feature = "witness"))]
    let lfn = "unbuilt";
    let asks_before = AUTH_ASKS.load(Ordering::Relaxed);
    drop(fat_unlock(Writer::Installer, "tests bootfatseam gate (no write)"));
    let authority = if AUTH_ASKS.load(Ordering::Relaxed) > asks_before { "authority" } else { "UNASKED" };
    let ok = asked && unasked && direct == 0 && users != "fat" && holocron != "fat" && lfn != "RUNS" && authority == "authority" && !allows();
    serial_println!(
        ":: BOOTFATSEAM: direct_writers={} users={} holocron={} lfn={} fat_unlock={} -> {} :: veto_asked={} raw_refused={} source={} ::",
        direct, users, holocron, lfn, authority, if ok { "PASS" } else { "FAIL" },
        if asked { "r99" } else { "MISSING" },
        if unasked { probe } else { 0 },
        src.map(|s| s.name()).unwrap_or("?")
    );
}

/// The media a native root can be bound on, beside `Default`/`Usb` (the cfg-gated arms of `BlockSource`).
fn boot_candidates() -> alloc::vec::Vec<crate::fs::fat::BlockSource> {
    #[allow(unused_mut)]
    let mut v = alloc::vec::Vec::new();
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    v.push(crate::fs::fat::BlockSource::Sdhc);
    #[cfg(all(target_arch = "x86_64", feature = "ahci"))]
    for p in 0..32u8 {
        v.push(crate::fs::fat::BlockSource::Ahci(p));
    }
    for n in 1..8u8 {
        v.push(crate::fs::fat::BlockSource::UsbN(n));
    }
    v
}

