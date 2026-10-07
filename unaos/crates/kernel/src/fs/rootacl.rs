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
    let anon_write = w && c && a && u;
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
