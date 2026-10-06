//! CHARTER: Kernel — fs-core
//!
//! BOOTFATSEAM (rmbp-ledger B453, SECREVIEW F3, R99): the kernel's own stores — the users table (`USERS.DAT`) and
//! the holocron store (`BTBOND.DAT`) — live on the UnaFS root as KERNEL-OWNED leaves of `/system` once the boot FAT
//! is sacred (an x86 native root, [`crate::fs::bootfat::sacred`]). One mount table (`shell::vfs_mount_table`), one
//! ACL: every leaf written here is stamped `owner=kernel`, so the native evaluators (`unafs::read_authz`,
//! `vfs::native_write_authz`) refuse every other principal; a leaf that does NOT carry `owner=kernel` is not adopted
//! (a program may have planted it in the still-public `/system` before the migration — ROOTACL B456 owns the
//! directory's own ACL).
//!
//! Publish discipline (the FAT stores' own): stage the temp, read it back, drop the live leaf, rename the temp over
//! it. A failure before the rename leaves the previous generation live.

use alloc::string::String;
use alloc::vec::Vec;

/// The directory every kernel-owned leaf lives in.
pub const DIR: &str = "/system";

/// `/system/<leaf>`.
pub fn path(leaf: &str) -> String {
    alloc::format!("{}/{}", DIR, leaf)
}

/// Are the kernel's stores on UnaFS this boot? (The boot FAT is sacred = an x86 native root is bound.)
pub fn active() -> bool {
    crate::fs::bootfat::sacred()
}

#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn owner_is_kernel(id: u64) -> bool {
    use ::unafs::inode::AttributeValue;
    crate::fs::unafs::with_unafs(|fs| match fs.read_inode(id) {
        Ok(ino) => matches!(ino.attributes.get("owner"), Some(AttributeValue::String(s)) if s == crate::fs::vfs::KERNEL_PRINCIPAL),
        Err(_) => false,
    })
    .unwrap_or(false)
}

#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn stamp_kernel(id: u64) -> bool {
    use ::unafs::inode::AttributeValue;
    crate::fs::unafs::with_unafs(|fs| {
        fs.set_attribute(id, String::from("owner"), AttributeValue::String(String::from(crate::fs::vfs::KERNEL_PRINCIPAL))).is_ok()
    })
    .unwrap_or(false)
}

#[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
fn owner_is_kernel(_id: u64) -> bool {
    false
}

#[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
fn stamp_kernel(_id: u64) -> bool {
    false
}

/// What a read of a kernel-owned leaf found.
pub enum Leaf {
    /// The bytes of a leaf that carries `owner=kernel`.
    Kernel(Vec<u8>),
    /// No such leaf.
    Absent,
    /// A leaf at that name WITHOUT `owner=kernel` — refused, never adopted.
    Foreign,
    /// The volume answered with an error (or a leaf larger than `max`).
    Error(&'static str),
}

/// Read `/system/<leaf>` (bounded by `max`) at kernel authority, adopted only when it is kernel-owned.
pub fn read(leaf: &str, max: usize) -> Leaf {
    use crate::fs::vfs::NodeKind;
    let mt = crate::shell::vfs_mount_table();
    let p = path(leaf);
    let st = match mt.stat(&p) {
        Ok(s) => s,
        Err(_) => return Leaf::Absent,
    };
    if st.kind != NodeKind::File {
        return Leaf::Foreign;
    }
    if !st.id.map(owner_is_kernel).unwrap_or(false) {
        return Leaf::Foreign;
    }
    if st.size as usize > max {
        return Leaf::Error("oversize");
    }
    match mt.read(&p, 0, st.size as usize) {
        Ok(b) if b.len() == st.size as usize => Leaf::Kernel(b),
        Ok(_) => Leaf::Error("short-read"),
        Err(_) => Leaf::Error("read"),
    }
}

/// Remove `/system/<leaf>` at kernel authority. Absent is success.
pub fn unlink(leaf: &str) -> Result<(), &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let p = path(leaf);
    if mt.stat(&p).is_err() {
        return Ok(());
    }
    mt.unlink(&p, crate::fs::vfs::KERNEL_PRINCIPAL).map_err(|_| "unlink")
}

/// Whole-leaf rewrite of `/system/<leaf>` (the primitive; [`publish`] is the swap). Creates `/system` when absent.
pub fn write(leaf: &str, data: &[u8]) -> Result<(), &'static str> {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    let mt = crate::shell::vfs_mount_table();
    if mt.stat(DIR).is_err() {
        mt.create(DIR, NodeKind::Dir, KERNEL_PRINCIPAL).map_err(|_| "mkdir-system")?;
    }
    let p = path(leaf);
    if mt.stat(&p).is_ok() {
        mt.unlink(&p, KERNEL_PRINCIPAL).map_err(|_| "unlink-old")?;
    }
    let st = mt.create(&p, NodeKind::File, KERNEL_PRINCIPAL).map_err(|_| "create")?;
    // Owned BEFORE a byte lands: there is no window in which the leaf is public and holds data.
    if !st.id.map(stamp_kernel).unwrap_or(false) {
        let _ = mt.unlink(&p, KERNEL_PRINCIPAL);
        return Err("owner-stamp");
    }
    let n = mt.write(&p, 0, data, KERNEL_PRINCIPAL).map_err(|_| "write")?;
    if n != data.len() {
        return Err("short-write");
    }
    Ok(())
}

/// Publish `data` as `/system/<live>`: stage `/system/<tmp>`, prove it reads back byte-equal and kernel-owned, then
/// drop the live leaf and rename the temp over it.
pub fn publish(live: &str, tmp: &str, data: &[u8]) -> Result<(), &'static str> {
    write(tmp, data)?;
    match read(tmp, data.len()) {
        Leaf::Kernel(b) if b[..] == data[..] => {}
        _ => return Err("readback"),
    }
    let mt = crate::shell::vfs_mount_table();
    unlink(live)?;
    mt.rename(&path(tmp), &path(live), crate::fs::vfs::KERNEL_PRINCIPAL).map_err(|_| "rename")
}

/// Rename `/system/<from>` to `/system/<to>`, replacing `to` (the quarantine move).
pub fn rename(from: &str, to: &str) -> Result<(), &'static str> {
    let mt = crate::shell::vfs_mount_table();
    unlink(to)?;
    mt.rename(&path(from), &path(to), crate::fs::vfs::KERNEL_PRINCIPAL).map_err(|_| "rename")
}
