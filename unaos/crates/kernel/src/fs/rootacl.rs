//! CHARTER: Kernel — fs-core
//!
//! ROOTACL (rmbp-ledger B456; SECREVIEW F2's owed sibling, docs/dev/review/SEC-2026-10-06.md; R94, R100). The root
//! volume's system trees — `/system`, `/system/types`, `/apps`, `/lib` — carry ONE owner value, [`SYSTEM_OWNER`], in
//! the native ACL the VFS already evaluates. No second store, no path list in the ACL: an object is system-owned when
//! its NEAREST `owner` row (walking the v6 parent chain) is `system`, so a file `arroyo` put under `/lib/rustc/…`
//! inherits its tree's row. A system-owned object is written (create under it, write, truncate, unlink, rename, rmdir,
//! attribute set/remove) only by `kernel` and by a principal whose user row is the administrator (the installer and
//! the updater, R100); it reads for everyone. `vfs::native_write_authz` asks [`write_verdict`]; `unafs::read_authz`
//! treats `system` as public. The stamps: `arroyo` after ROOTDISK2's put loop, and [`stamp`] at FILETYPES' registry
//! build (`assoc::seed_once`). Witnesses: `[rootacl] owner=system trees=<n> stamped=<m> root=<vol>` once per boot;
//! `tests rootacl` → `:: ROOTACL: owner=system trees=4 anon_write=refused admin_write=ok -> PASS :: …`.

use alloc::string::String;
use crate::fs::vfs::{AttrValue, MountTable, NodeKind, VfsError, KERNEL_PRINCIPAL};

/// The owner value of the system trees. Never matched as a NAME (a program or user called `system` is not it).
pub const SYSTEM_OWNER: &str = "system";

/// The trees [`stamp`] owns on the root volume (the order `tests rootacl` reports them in).
pub const TREES: [&str; 4] = ["/system", "/system/types", "/apps", "/lib"];

/// The parent chain is walked at most this deep (a cycle on a damaged volume stops here, failing to "not system").
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
const WALK_MAX: usize = 64;

/// Is the object `ino` (already read) system-owned — its nearest `owner` row, up the parent chain, `system`?
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn system_owned(fs: &mut crate::fs::unafs::KernelUnaFS, ino: &::unafs::inode::Inode) -> bool {
    use ::unafs::inode::AttributeValue;
    let verdict = |i: &::unafs::inode::Inode| match i.attributes.get("owner") {
        Some(AttributeValue::String(s)) => Some(s == SYSTEM_OWNER),
        Some(_) => Some(false),
        None => None,
    };
    if let Some(v) = verdict(ino) {
        return v;
    }
    let mut parent = ino.parent;
    let mut seen = ino.id;
    for _ in 0..WALK_MAX {
        if parent == 0 || parent == seen {
            return false;
        }
        let Ok(p) = fs.read_inode(parent) else { return false };
        if let Some(v) = verdict(&p) {
            return v;
        }
        seen = p.id;
        parent = p.parent;
    }
    false
}

/// Is the live object `id` system-owned? (`create`'s parent: an admin's new object takes no personal owner row.)
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn under_system(fs: &mut crate::fs::unafs::KernelUnaFS, id: u64) -> bool {
    match fs.read_inode(id) {
        Ok(ino) => system_owned(fs, &ino),
        Err(_) => false,
    }
}

/// Is `principal` the administrator (R100)? The principal's own user row, not the console's session: `user:<name>`
/// with an optional `#<uid>` (x86 `attrsurf_principal`, aarch64 `user_native_string`).
pub fn admin_principal(principal: &str) -> bool {
    let Some(rest) = principal.strip_prefix("user:") else { return false };
    let name = rest.split('#').next().unwrap_or("");
    #[cfg(feature = "login")] { !name.is_empty() && crate::fs::users::role_of(name.as_bytes()) == Some(crate::fs::users::Role::Admin) }
    #[cfg(not(feature = "login"))] { let _ = name; false } // no user store without `login`: nobody is the administrator
}

/// `native_write_authz`'s question for a non-kernel `principal` on `ino`: `None` = not system-owned (the ordinary
/// owner/grants rule answers); `Some(Ok)` = the administrator; `Some(Err(Denied))` = everyone else, said once.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn write_verdict(fs: &mut crate::fs::unafs::KernelUnaFS, ino: &::unafs::inode::Inode, principal: &str) -> Option<Result<(), VfsError>> {
    if !system_owned(fs, ino) {
        return None;
    }
    if admin_principal(principal) {
        return Some(Ok(()));
    }
    serial_println!("[rootacl] refused id={} principal={} reason=system-owned (B456: kernel and the administrator write here)", ino.id, principal);
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] let _ = crate::video::dialog::refused_by(crate::video::dialog::WHAT_SYSTEM_FILES, alloc::format!("write id={} principal={} reason=system-owned", ino.id, principal).as_bytes(), crate::video::dialog::caller_owner()); // REFUSALUI (B468): the one refusal surface, moved here from attrsys F2 at the ROOTACL fold
    Some(Err(VfsError::Denied))
}

/// Does `path` carry the `system` owner row itself?
fn stamped(mt: &MountTable, path: &str) -> bool {
    matches!(mt.get_attr(path, "owner", KERNEL_PRINCIPAL), Ok(AttrValue::Str(s)) if s == SYSTEM_OWNER)
}

/// Stamp [`TREES`] that exist as directories on the root and lack the row. `(trees, stamped)`: the trees now
/// carrying `system`, and how many this call wrote. Idempotent; kernel authority. One line, every call.
pub fn stamp(mt: &MountTable) -> (usize, usize) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static REG: AtomicBool = AtomicBool::new(false);
    if !REG.swap(true, Ordering::AcqRel) {
        crate::tests::register("rootacl", selftest);
    }
    let (mut trees, mut wrote) = (0usize, 0usize);
    for t in TREES {
        if !matches!(mt.stat(t), Ok(s) if matches!(s.kind, NodeKind::Dir)) {
            continue;
        }
        if !stamped(mt, t) && mt.set_attr(t, "owner", AttrValue::Str(String::from(SYSTEM_OWNER)), KERNEL_PRINCIPAL).is_ok() {
            wrote += 1;
        }
        if stamped(mt, t) {
            trees += 1;
        }
    }
    let vol = mt.volume_name("/").unwrap_or_else(|_| String::from("-"));
    crate::bootlog_println!("[rootacl] owner=system trees={} stamped={} root={}", trees, wrote, vol);
    (trees, wrote)
}

/// `tests rootacl`: on the live root, a scratch file under `/system/types` (kernel-made, no row of its own: it
/// inherits) is refused to `anon` for write, create-beside, attribute set and unlink, read by `anon`, and written by
/// the administrator's principal; then removed.
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let (trees, _) = stamp(&mt);
    let owner = if stamped(&mt, "/system") { SYSTEM_OWNER } else { "none" };
    const F: &str = "/system/types/.rootacl-scratch";
    const G: &str = "/system/types/.rootacl-anon";
    let _ = mt.unlink(F, KERNEL_PRINCIPAL);
    let _ = mt.unlink(G, KERNEL_PRINCIPAL);
    let made = mt.create(F, NodeKind::File, KERNEL_PRINCIPAL).is_ok() && mt.write(F, 0, b"rootacl", KERNEL_PRINCIPAL).is_ok();
    let anon = "anon";
    let tok = |r: bool| if r { "refused" } else { "ADMITTED" };
    let w = matches!(mt.write(F, 0, b"x", anon), Err(VfsError::Denied));
    let c = matches!(mt.create(G, NodeKind::File, anon), Err(VfsError::Denied));
    let a = matches!(mt.set_attr(F, una_abi::attr_keys::OPENER, AttrValue::Str(String::from("X")), anon), Err(VfsError::Denied));
    let u = matches!(mt.unlink(F, anon), Err(VfsError::Denied));
    let r = mt.open_read(F, anon).is_ok();
    #[cfg(feature = "login")] let mut nb = [0u8; crate::fs::users::NAME_MAX];
    #[cfg(feature = "login")] let admin = crate::fs::users::admin_name(&mut nb).and_then(|n| core::str::from_utf8(&nb[..n]).ok().map(String::from));
    #[cfg(not(feature = "login"))] let admin: Option<String> = None;
    let admin_tok = match &admin {
        Some(n) => {
            let p = alloc::format!("user:{}#0", n);
            if mt.write(F, 0, b"admin", &p).is_ok() { "ok" } else { "REFUSED" }
        }
        None => "no-admin",
    };
    let _ = mt.unlink(G, KERNEL_PRINCIPAL);
    let _ = mt.unlink(F, KERNEL_PRINCIPAL);
    let anon_write = w && c && a && u; #[cfg(target_arch = "x86_64")] let _ = machine_store_leg(&mt); // MACHINESTOREACL (rmbp-ledger B1150): the no-session settings folder's ring-3 refusal leg (file tail); its own summary line, not in ROOTACL's PASS. LINE-NEUTRAL append, code first
    let pass = made && owner == SYSTEM_OWNER && trees == TREES.len() && anon_write && r && admin_tok != "REFUSED";
    serial_println!(
        ":: ROOTACL: owner={} trees={} anon_write={} admin_write={} -> {} :: anon_read={} anon_create={} anon_unlink={} anon_attr={} admin={} scratch={} ::",
        owner,
        trees,
        if anon_write { "refused" } else { "ADMITTED" },
        admin_tok,
        if pass { "PASS" } else { "FAIL" },
        if r { "ok" } else { "REFUSED" },
        tok(c),
        tok(u),
        tok(a),
        admin.as_deref().unwrap_or("-"),
        if made { "made" } else { "unmade" },
    );
}

// ── MACHINESTOREACL (rmbp-ledger B1150, BP-storage-11) — TAIL-APPENDED ─────────────────────────────────────────────────
// The no-session settings folder (prefs' MACHINE store, VEIL-REDUCE-BOOT-STORE B1140: `/settings/general` is the boot
// screen's copy) carries the `system` owner row above. `write_verdict`'s walk then refuses every write, create, rename,
// unlink and attribute set in it to all but `kernel` and the administrator (the swap's row-less `general` inherits the
// folder's row); reads stay public (`unafs::read_authz`). Before B1150 the folder was a public object (a kernel create
// stamps no row) and ring 3's `SYS_PATH_WRITE` wrote it as `anon`. prefs.rs stamps it from its two writers
// (`ensure_dirs`, `machine_mirror`) through [`stamp_store`]; a home's `<home>/settings` is never stamped. `tests rootacl`
// runs the MACHINESTORE leg (x86): the ring-3 path door's body (`arch::syscall::path_write_as`) as `anon`.
// Wire: `[rootacl] store dir=<d> owner=<system|none> stamped=<1|0>` (a write, or the first failure) ·
// `:: MACHINESTORE: owner= write= create= read= -> PASS|FAIL :: by=anon target=<general|scratch> errno= made_dir= ::`.

/// The no-session settings folder, spelt as prefs.rs's `path()` with no home and uninstall's `settings_dirs` spell it.
fn machine_store_dir() -> String {
    alloc::format!("/{}", prefs_core::files::DIR)
}

/// Stamp the machine store folder `dir` with [`SYSTEM_OWNER`] at kernel authority when it lacks the row; any other
/// `dir` (a home's settings folder) is left alone with no I/O. `true` = the folder carries the row.
pub fn stamp_store(mt: &MountTable, dir: &str) -> bool {
    use core::sync::atomic::{AtomicBool, Ordering};
    static FAIL_SAID: AtomicBool = AtomicBool::new(false);
    if machine_store_dir() != dir || !matches!(mt.stat(dir), Ok(s) if matches!(s.kind, NodeKind::Dir)) {
        return false;
    }
    if stamped(mt, dir) {
        return true;
    }
    let ok = mt.set_attr(dir, "owner", AttrValue::Str(String::from(SYSTEM_OWNER)), KERNEL_PRINCIPAL).is_ok() && stamped(mt, dir);
    if ok || !FAIL_SAID.swap(true, Ordering::AcqRel) {
        serial_println!("[rootacl] store dir={} owner={} stamped={}", dir, if ok { SYSTEM_OWNER } else { "none" }, ok as u8);
    }
    ok
}

/// MACHINESTORE leg of `tests rootacl`: the folder laid at kernel authority only when absent (removed after if still
/// empty), stamped by [`stamp_store`]; then, through the ring-3 path door's body as `anon`, a write (a trailing newline
/// at EOF of `general`, else of a kernel-laid scratch file; a wrongly admitted byte is truncated back) and a create
/// beside it must both be `-EACCES`, and an `anon` read of the target must still open. One summary line.
#[cfg(target_arch = "x86_64")]
fn machine_store_leg(mt: &MountTable) -> bool {
    use crate::arch::syscall::path_write_as;
    let dir = machine_store_dir();
    let made_dir = mt.stat(&dir).is_err() && mt.create(&dir, NodeKind::Dir, KERNEL_PRINCIPAL).is_ok();
    let owned = stamp_store(mt, &dir);
    let general = alloc::format!("{}/general", dir);
    let scratch = alloc::format!("{}/machinestore-probe", dir);
    let anon_new = alloc::format!("{}/machinestore-anon", dir);
    let _ = mt.unlink(&anon_new, KERNEL_PRINCIPAL);
    let is_general = matches!(mt.stat(&general), Ok(s) if matches!(s.kind, NodeKind::File));
    let target = if is_general { general } else { scratch };
    let laid = !is_general && {
        let _ = mt.unlink(&target, KERNEL_PRINCIPAL);
        mt.create(&target, NodeKind::File, KERNEL_PRINCIPAL).is_ok() && mt.write(&target, 0, b"probe\n", KERNEL_PRINCIPAL).is_ok()
    };
    let size = mt.stat(&target).map(|s| s.size as u64).unwrap_or(0);
    let we = path_write_as(&target, 0, size, b"\n", "anon");
    if we >= 0 {
        let _ = mt.truncate(&target, size, KERNEL_PRINCIPAL);
    }
    let ce = path_write_as(&anon_new, una_abi::PATH_W_TRUNC, 0, b"x", "anon");
    if ce >= 0 {
        let _ = mt.unlink(&anon_new, KERNEL_PRINCIPAL);
    }
    let read = mt.open_read(&target, "anon").is_ok();
    // MACHINESTOREPLANT (rmbp-ledger B1156): plant as `anon` before the stamp (the folder's row lifted at kernel authority),
    // re-stamp, then prefs' ONE reader (`read_all` via `prefs::store_read`) must refuse the planted file. Named in-place hunk.
    let plant = alloc::format!("{}/machinestore-plant", dir);
    let _ = mt.unlink(&plant, KERNEL_PRINCIPAL);
    let _ = mt.remove_attr(&dir, "owner", KERNEL_PRINCIPAL);
    let pe = path_write_as(&plant, una_abi::PATH_W_TRUNC, 0, b"[system]\n", "anon");
    let restamped = stamp_store(mt, &dir);
    let plant_tok = if pe < 0 { "unplanted" } else if crate::prefs::store_read(&plant).is_none() { "refused" } else { "READ" };
    let _ = mt.unlink(&plant, KERNEL_PRINCIPAL);
    // MACHINESTORESTAMPRING3 (rmbp-ledger B1162): the boot stamp's body (`prefs::lay_machine_store`, no latch) leaves the
    // folder stamped, so the same plant as `anon` is now refused at CREATE (`-EACCES`). Named in-place hunk.
    let (_, boot_stamped) = crate::prefs::lay_machine_store(mt);
    let be = path_write_as(&plant, una_abi::PATH_W_TRUNC, 0, b"[system]\n", "anon");
    if be >= 0 {
        let _ = mt.unlink(&plant, KERNEL_PRINCIPAL);
    }
    let boot_tok = if !boot_stamped { "unstamped" } else if be == una_abi::EACCES { "refused-at-create" } else if be >= 0 { "ADMITTED" } else { "error" };
    if laid {
        let _ = mt.unlink(&target, KERNEL_PRINCIPAL);
    }
    if made_dir {
        let _ = mt.remove_dir(&dir, KERNEL_PRINCIPAL);
    }
    let tok = |e: i64| if e == una_abi::EACCES { "refused" } else if e >= 0 { "ADMITTED" } else { "error" };
    let pass = owned && we == una_abi::EACCES && ce == una_abi::EACCES && read && restamped && plant_tok == "refused" && boot_tok == "refused-at-create"; // MACHINESTOREPLANT (B1156): the plant leg joins this leg's PASS; MACHINESTORESTAMPRING3 (B1162): so does the boot step
    serial_println!(
        ":: MACHINESTORE: owner={} write={} create={} read={} plant={} boot={} -> {} :: by=anon target={} errno={} made_dir={} ::",
        if owned { SYSTEM_OWNER } else { "none" },
        tok(we),
        tok(ce),
        if read { "ok" } else { "REFUSED" },
        plant_tok,
        boot_tok,
        if pass { "PASS" } else { "FAIL" },
        if is_general { "general" } else { "scratch" },
        we,
        made_dir as u8,
    );
    pass
}

// ── MACHINESTOREPLANT (rmbp-ledger B1156, BP-storage-11) — TAIL-APPENDED ───────────────────────────────────────────────
// A file a program planted in the machine store before the folder's first stamp keeps its OWN owner row (a ring-3 create
// stamps `owner=<principal>`, fs/vfs.rs create; `owner` is reserved, so ring 3 cannot shed it), and `system_owned`
// stops at the nearest row: the folder's `system` row never reaches it. prefs' one reader (`read_all`) asks
// [`store_file_refused`] and reads a refused file as absent (the kernel's next swap of that domain replaces it).
// Wire (said by prefs): `[prefs] machine settings/<d> refused: owner=<p>`.

/// Is the machine-store file `path` NOT the kernel's? Its nearest owner row must be [`SYSTEM_OWNER`]: its own row
/// `system`, or no row of its own in a [`stamped`] folder. `Some(owner)` = refuse (`none` for a row-less file in an
/// unstamped folder); `None` = read it — also for a path not directly in the store, and for a volume with no attributes
/// (a FAT root answers `Unsupported`: MACHINE-STORE-FATROOT stays owed).
pub fn store_file_refused(mt: &MountTable, path: &str) -> Option<String> {
    let dir = machine_store_dir();
    let leaf = path.strip_prefix(dir.as_str()).and_then(|r| r.strip_prefix('/'))?;
    if leaf.is_empty() || leaf.contains('/') {
        return None;
    }
    match mt.get_attr(path, "owner", KERNEL_PRINCIPAL) {
        Ok(AttrValue::Str(s)) if s == SYSTEM_OWNER => None,
        Ok(AttrValue::Str(s)) => Some(s),
        Ok(_) => Some(String::from("?")),
        Err(VfsError::Backend("no-attr")) => {
            if stamped(mt, &dir) { None } else { Some(String::from("none")) }
        }
        Err(_) => None,
    }
}
