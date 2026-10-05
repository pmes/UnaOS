//! CHARTER: Kernel — fs-core
//!
//! VOLUMES (rmbp-ledger B366) — Peter at the glass, flight 22 (items 4/5): "a duplicate home dir is created on the
//! fat boot partition, along with dup apps and system dirs. the boot partition should appear under volumes in place
//! of efi and only efi under volumes/boot. the mounted UnaFS root volume should also appear in volumes. maybe you can
//! find free samples for all the various file format we need tested?" and "we need test-f under system".
//!
//! * **The duplicate `home`** was the screenshot writer (`video::prtscr::ensure_capture_dir`) walking the FAT capture
//!   target; it now makes the folder through the mount table on a native root (prtscr's VOLUMES M1 block). The
//!   user store already did (`users::ensure_home_native`).
//! * **The Volumes view** is two MOUNTS, not a painter's trick, so the shell's `ls /volumes` and Quarry agree:
//!   [`bind_aliases`] binds `/volumes/boot` (the FAT boot partition, the same volume as `/boot`) and, on a native
//!   root, `/volumes/UnaOS` (the UnaFS root; the superblock carries no label, so the OS's name). Quarry's
//!   presentation ([`view`]) then shows the boot partition under Volumes only (no `boot` row at `/`) and only its
//!   `EFI` tree (`APPS/`, `SYSTEM/` stay on the medium where the loader needs them; `/apps` and `/system` are seen
//!   once, from the root). `/boot` itself stays bound — every verb, spec and loader path names it.
//! * **`system/test-f`** is staged by the builder from `builder/testf.list` (free samples, licence and sha256 per
//!   row, fetched at image time) onto the data volume, and by `arroyo esp-x86` onto the card's UnaFS root.
//!   [`testf_find`] is the one lookup (`/system/test-f`, then `/boot/system/test-f`); `tests play` asks it after
//!   `/home`, and `tests testf` scores staged vs claimed.
//!
//! Witnesses: `:: VOLUMES: boot=efi-only root=unafs home_on_fat=0 shown=<list> -> PASS ::` and
//! `:: TESTF: staged=<n>/<m> missing=<list> -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::vfs::{DirEnt, MountTable};

/// The Volumes entry for the FAT boot partition (Peter: "the boot partition should appear under volumes").
pub const BOOT_POINT: &str = "/volumes/boot";
/// The Volumes entry for the UnaFS root. The UnaFS superblock carries no label, so the OS's own name.
pub const ROOT_POINT: &str = "/volumes/UnaOS";
/// What Quarry shows of the boot partition: only its EFI tree.
pub const BOOT_SHOWN: &str = "EFI";

/// `system/test-f`, on the root first (the card's UnaFS root, or a FAT root), then on the boot partition's data tree.
pub const TESTF_DIRS: [&str; 2] = ["/system/test-f", "/boot/system/test-f"];
/// The formats the kernel claims, one sample each (`builder/testf.list` stages them under these names).
pub const TESTF_CLAIMED: [&str; 24] = [
    "TEST.WAV", "TEST.FLAC", "TEST.OPUS", "TEST.OGG", "TEST.MP3", "TEST.AAC", "TEST.M4A", "TEST.AIF",
    "TEST.PNG", "APNG.PNG", "TEST.JPG", "TEST.GIF", "LOSSLESS.WEBP", "LOSSY.WEBP", "ANIM.WEBP", "TEST.BMP",
    "TEST.QOI", "TEST.SVG", "TEST.TXT", "TEST.MD", "TEST.JSON", "TEST.CSV", "TEST.WEBM", "TEST.MP4",
];

static BOOT_ALIAS: AtomicBool = AtomicBool::new(false);
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// Is `/volumes/boot` bound (so Quarry may drop the root's `boot` row — the partition is shown under Volumes)?
pub fn boot_alias_bound() -> bool {
    BOOT_ALIAS.load(Ordering::Relaxed)
}

/// Bind the two Volumes entries over the root disk `bind` just bound. A point some other disk already claimed
/// (a stick labelled `boot`) is left to it and said once. Called from `bootdisk::bind` after the root.
pub fn bind_aliases(mt: &mut MountTable, src: crate::fs::fat::BlockSource, announce: bool) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    let say = announce && !ANNOUNCED.swap(true, Ordering::Relaxed);
    let taken = |mt: &MountTable, p: &str| mt.prefixes().iter().any(|q| *q == p);
    if taken(mt, BOOT_POINT) {
        if say {
            serial_println!("[vfs] volume alias /volumes/boot NOT bound — the point is another disk's ::");
        }
    } else {
        mt.mount(BOOT_POINT, alloc::boxed::Box::new(FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src)));
        BOOT_ALIAS.store(true, Ordering::Relaxed);
        if say {
            serial_println!("[vfs] volume alias /volumes/boot = fat boot volume source={} (Quarry shows EFI only) ::", src.name());
        }
    }
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    if mt.volume_name("/").map(|n| n == "native").unwrap_or(false) && !taken(mt, ROOT_POINT) {
        mt.mount(ROOT_POINT, alloc::boxed::Box::new(crate::fs::vfs::NativeBackend::new("native")));
        if say {
            serial_println!("[vfs] volume alias /volumes/UnaOS = native unafs root source={} ::", src.name());
        }
    }
}

fn is_boot_root(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    p == "/boot" || p == BOOT_POINT
}

/// Quarry's Volumes presentation of one listing (pure): at `/` the `boot` row goes when the partition is shown
/// under Volumes; at the boot partition's root only `EFI` is shown. Every other listing passes through.
pub fn view(path: &str, mut rows: Vec<DirEnt>, boot_under_volumes: bool) -> Vec<DirEnt> {
    if path == "/" && boot_under_volumes {
        rows.retain(|e| e.name != "boot");
    } else if is_boot_root(path) {
        rows.retain(|e| e.name.eq_ignore_ascii_case(BOOT_SHOWN));
    }
    rows
}

/// `tests play` and the viewer: `<dir>/<name>` in the first `system/test-f` that holds it.
pub fn testf_find(mt: &MountTable, name: &str) -> Option<String> {
    TESTF_DIRS.iter().map(|d| alloc::format!("{}/{}", d, name)).find(|p| mt.stat(p).is_ok())
}

fn names(path: &str, boot_under_volumes: bool) -> Option<Vec<String>> {
    match crate::shell::vfs_ls_collect(path) {
        Ok((true, rows)) => Some(view(path, rows, boot_under_volumes).into_iter().map(|e| e.name).collect()),
        _ => None,
    }
}

/// `tests volumes` — the layout Peter asked for, read off the live table and Quarry's own view of it.
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let native = mt.volume_name("/").map(|n| n == "native").unwrap_or(false);
    let pure_ok = {
        let d = |n: &str| DirEnt { name: String::from(n), kind: crate::fs::vfs::NodeKind::Dir, size: 0, mtime: None };
        let b = view("/volumes/boot", alloc::vec![d("APPS"), d("EFI"), d("HOME"), d("SYSTEM")], true);
        let r = view("/", alloc::vec![d("apps"), d("boot"), d("home")], true);
        b.len() == 1 && b[0].name == "EFI" && r.len() == 2 && !r.iter().any(|e| e.name == "boot")
    };
    if !native {
        serial_println!(":: VOLUMES: boot=- root=fat home_on_fat=- shown=- -> SKIP reason=fat-root (homes live on the FAT by design) :: pure={} ::", pure_ok);
        return;
    }
    let under = boot_alias_bound();
    let boot = names(BOOT_POINT, under);
    let boot_tok = match &boot {
        Some(v) if v.len() == 1 && v[0].eq_ignore_ascii_case(BOOT_SHOWN) => "efi-only",
        Some(v) if v.is_empty() => "empty",
        Some(_) => "more-than-efi",
        None => "missing",
    };
    let home_on_fat = ["/boot/HOME", "/boot/home"].iter().any(|p| mt.stat(p).is_ok()) as u32;
    let shown = names("/volumes", under).unwrap_or_default();
    let root = names("/", under).unwrap_or_default();
    let once = |w: &str| root.iter().filter(|n| n.eq_ignore_ascii_case(w)).count();
    let ok = pure_ok
        && boot_tok == "efi-only"
        && home_on_fat == 0
        && shown.iter().any(|n| n == "boot")
        && shown.iter().any(|n| alloc::format!("/volumes/{}", n) == ROOT_POINT)
        && once("apps") <= 1
        && once("system") <= 1
        && once("boot") == 0;
    serial_println!(
        ":: VOLUMES: boot={} root=unafs home_on_fat={} shown={} -> {} :: root_view={} pure={} ::",
        boot_tok,
        home_on_fat,
        if shown.is_empty() { String::from("-") } else { shown.join(",") },
        if ok { "PASS" } else { "FAIL" },
        root.join(","),
        pure_ok
    );
}

/// `tests testf` — what `system/test-f` holds against what the kernel claims.
pub fn testf_selftest() {
    let mt = crate::shell::vfs_mount_table();
    let m = TESTF_CLAIMED.len();
    let Some(dir) = TESTF_DIRS.iter().find(|d| mt.stat(d).is_ok()) else {
        serial_println!(":: TESTF: staged=0/{} missing=all -> FAIL :: reason=no-dir (looked in /system/test-f, /boot/system/test-f; the builder stages it from builder/testf.list) ::", m);
        return;
    };
    let missing: Vec<&str> = TESTF_CLAIMED.iter().copied().filter(|n| mt.stat(&alloc::format!("{}/{}", dir, n)).is_err()).collect();
    let manifest = mt.stat(&alloc::format!("{}/MANIFEST.txt", dir)).is_ok();
    serial_println!(
        ":: TESTF: staged={}/{} missing={} -> {} :: dir={} manifest={} ::",
        m - missing.len(),
        m,
        if missing.is_empty() { String::from("-") } else { missing.join(",") },
        if missing.is_empty() && manifest { "PASS" } else { "FAIL" },
        dir,
        if manifest { "yes" } else { "no" }
    );
}
