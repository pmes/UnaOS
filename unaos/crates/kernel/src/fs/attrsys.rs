//! CHARTER: Kernel — fs-core
//!
//! ATTRSURF (B299) — the ONE fulfiller of the typed-attribute surface above the VFS.
//!
//! Every consumer reaches attributes through here or through `MountTable` directly: the shell verbs
//! (`setfattr`, `getfattr`, `query`, `stat`), `SYS_ATTR_SET/GET/LIST`, `SYS_QUERY`, `SYS_STAT` on both
//! arches, and the bus verbs `BUS_VERB_ATTR_*` on both arches. The syscall arm and the bus arm of a
//! verb call the SAME function here with the SAME bytes (una-abi's ATTRSURF block: the syscall input
//! is the bus request body, the syscall output is the bus reply body), so the two cannot drift, and
//! the errno a program gets is the errno the shell verb prints ([`errno_of`] / [`errno_name`]).
//!
//! Design and wire: `docs/dev/evidence/rmbp-1001/ATTRSURF.md`.
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::{AttrValue, VfsError};

/// The errno a VFS refusal becomes on the wire. `Backend` spellings the attribute surface owns are
/// mapped by name; every other backend string is `-EIO`.
pub fn errno_of(e: &VfsError) -> i64 {
    match e {
        VfsError::NoSuchVolume => una_abi::ENODEV,
        VfsError::NoSuchPath => una_abi::ENOENT,
        VfsError::NotADirectory => una_abi::ENOTDIR,
        VfsError::IsADirectory => una_abi::EISDIR,
        VfsError::Denied => una_abi::EACCES,
        VfsError::Unsupported => una_abi::ENOTSUP,
        VfsError::Backend("no-attr") => una_abi::ENODATA,
        VfsError::Backend("bad-query") | VfsError::Backend("bad-key") | VfsError::Backend("attr-too-big") => una_abi::EINVAL,
        VfsError::Backend(_) => una_abi::EIO,
    }
}

/// The errno's name as the verbs print it (`-ENOTSUP`, ...).
pub fn errno_name(e: i64) -> &'static str {
    match e {
        una_abi::ENODEV => "-ENODEV",
        una_abi::ENOENT => "-ENOENT",
        una_abi::ENOTDIR => "-ENOTDIR",
        una_abi::EISDIR => "-EISDIR",
        una_abi::EACCES => "-EACCES",
        una_abi::ENOTSUP => "-ENOTSUP",
        una_abi::ENODATA => "-ENODATA",
        una_abi::EINVAL => "-EINVAL",
        una_abi::ERANGE => "-ERANGE",
        una_abi::EFAULT => "-EFAULT",
        _ => "-EIO",
    }
}

/// One operator line for a refusal: `<reason> (<errno>)`.
pub fn refusal(e: &VfsError) -> String {
    let n = errno_of(e);
    let why = match n {
        una_abi::ENODEV => "no such volume",
        una_abi::ENOENT => "no such file or directory",
        una_abi::ENOTDIR => "not a directory",
        una_abi::EISDIR => "is a directory",
        una_abi::EACCES => "permission denied",
        una_abi::ENOTSUP => "this volume carries no typed attributes",
        una_abi::ENODATA => "no such attribute",
        una_abi::EINVAL => match e { VfsError::Backend(s) => s, _ => "invalid argument" },
        _ => match e { VfsError::Backend(s) => s, _ => "i/o error" },
    };
    alloc::format!("{} ({})", why, errno_name(n))
}
