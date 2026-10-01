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

// --- the wire: AttrValue <-> una-abi value wire ---------------------------------------------------

/// Append `v`'s value wire (header + payload) to `out`.
pub fn value_to_wire(v: &AttrValue, out: &mut Vec<u8>) {
    let (tag, len) = match v {
        AttrValue::Int(_) => (una_abi::ATTR_TAG_INT, 8),
        AttrValue::Float(_) => (una_abi::ATTR_TAG_FLOAT, 8),
        AttrValue::Str(s) => (una_abi::ATTR_TAG_STR, s.len()),
        AttrValue::Blob(b) => (una_abi::ATTR_TAG_BLOB, b.len()),
        AttrValue::Vector(x) => (una_abi::ATTR_TAG_VECTOR, x.len() * 4),
    };
    out.extend_from_slice(&una_abi::AttrWireHdr { tag, _rsv: [0; 3], len: len as u32 }.to_bytes());
    match v {
        AttrValue::Int(i) => out.extend_from_slice(&i.to_le_bytes()),
        AttrValue::Float(f) => out.extend_from_slice(&f.to_le_bytes()),
        AttrValue::Str(s) => out.extend_from_slice(s.as_bytes()),
        AttrValue::Blob(b) => out.extend_from_slice(b),
        AttrValue::Vector(x) => {
            for f in x {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
}

/// Decode a validated `(tag, payload)` (from `una_abi::attr_wire_parse`) into a value.
pub fn value_from_wire(tag: u8, p: &[u8]) -> Result<AttrValue, i64> {
    let w8 = |p: &[u8]| { let mut a = [0u8; 8]; a.copy_from_slice(&p[..8]); a };
    Ok(match tag {
        una_abi::ATTR_TAG_INT => AttrValue::Int(i64::from_le_bytes(w8(p))),
        una_abi::ATTR_TAG_FLOAT => AttrValue::Float(f64::from_le_bytes(w8(p))),
        una_abi::ATTR_TAG_STR => AttrValue::Str(String::from(core::str::from_utf8(p).map_err(|_| una_abi::EINVAL)?)),
        una_abi::ATTR_TAG_BLOB => AttrValue::Blob(p.to_vec()),
        una_abi::ATTR_TAG_VECTOR => AttrValue::Vector(p.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()),
        _ => return Err(una_abi::EINVAL),
    })
}

/// A caller-supplied namespace path: UTF-8, absolute, no `.`/`..` component, no NUL. Anything else is
/// `-EINVAL` before the mount table is consulted (the resolver is lexical; ring 3 does not get to
/// lean on it).
fn user_path(b: &[u8]) -> Result<&str, i64> {
    let s = core::str::from_utf8(b).map_err(|_| una_abi::EINVAL)?;
    if s.is_empty() || s.len() > una_abi::ATTR_PATH_MAX || !s.starts_with('/') || s.contains('\0')
        || s.split('/').any(|c| c == "." || c == "..")
    {
        return Err(una_abi::EINVAL);
    }
    Ok(s)
}

fn user_key(b: &[u8]) -> Result<&str, i64> {
    let k = core::str::from_utf8(b).map_err(|_| una_abi::EINVAL)?;
    if k.is_empty() || k.contains('\0') {
        return Err(una_abi::EINVAL);
    }
    Ok(k)
}

macro_rules! tryi {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(n) => return n,
        }
    };
}

/// The namespace every consumer resolves through — the shell's table, so a program and the shell
/// see the same mounts.
fn table() -> crate::fs::vfs::MountTable {
    crate::shell::vfs_mount_table()
}

/// `SYS_ATTR_SET` / `BUS_VERB_ATTR_SET`: `req` is a request whose remainder is one value wire.
pub fn do_set(req: &[u8], principal: &str) -> i64 {
    let (p, k, rest) = tryi!(una_abi::attr_req_parse(req));
    let (tag, payload, used) = tryi!(una_abi::attr_wire_parse(rest));
    if used != rest.len() {
        return una_abi::EINVAL;
    }
    let (path, key) = (tryi!(user_path(p)), tryi!(user_key(k)));
    let v = tryi!(value_from_wire(tag, payload));
    match table().set_attr(path, key, v, principal) {
        Ok(()) => 0,
        Err(e) => errno_of(&e),
    }
}

/// `SYS_ATTR_GET` / `BUS_VERB_ATTR_GET`: `out` receives one value wire.
pub fn do_get(req: &[u8], principal: &str, out: &mut Vec<u8>) -> i64 {
    let (p, k, rest) = tryi!(una_abi::attr_req_parse(req));
    if !rest.is_empty() {
        return una_abi::EINVAL;
    }
    let (path, key) = (tryi!(user_path(p)), tryi!(user_key(k)));
    match table().get_attr(path, key, principal) {
        Ok(v) => { value_to_wire(&v, out); 0 }
        Err(e) => errno_of(&e),
    }
}

/// `SYS_ATTR_LIST` / `BUS_VERB_ATTR_LIST`: `out` receives `[key_len u16][key][value wire]` rows.
pub fn do_list(path: &[u8], principal: &str, out: &mut Vec<u8>) -> i64 {
    let path = tryi!(user_path(path));
    match table().list_attrs(path, principal) {
        Ok(rows) => {
            for (k, v) in rows {
                out.extend_from_slice(&(k.len() as u16).to_le_bytes());
                out.extend_from_slice(k.as_bytes());
                value_to_wire(&v, out);
            }
            0
        }
        Err(e) => errno_of(&e),
    }
}

/// `SYS_QUERY` / `BUS_VERB_ATTR_QUERY`: `out` receives `[id u64][path_len u16][path]` rows.
pub fn do_query(expr: &[u8], principal: &str, out: &mut Vec<u8>) -> i64 {
    let Ok(expr) = core::str::from_utf8(expr) else { return una_abi::EINVAL };
    if expr.trim().is_empty() || expr.len() > una_abi::ATTR_VALUE_MAX {
        return una_abi::EINVAL;
    }
    match table().query(expr, principal) {
        Ok(hits) => {
            for (id, p) in hits {
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&(p.len() as u16).to_le_bytes());
                out.extend_from_slice(p.as_bytes());
            }
            0
        }
        Err(e) => errno_of(&e),
    }
}

/// `SYS_STAT` / `BUS_VERB_ATTR_STAT`: the VFS OPEN contract (authorize first, then stat — a denied
/// principal never learns size or kind), answered as one `UserStat`.
pub fn do_stat(path: &[u8], principal: &str, out: &mut Vec<u8>) -> i64 {
    let path = tryi!(user_path(path));
    match table().open_read(path, principal) {
        Ok(st) => {
            let mut flags = 0;
            if st.id.is_some() { flags |= una_abi::STAT_HAS_ID; }
            if st.mtime.is_some() { flags |= una_abi::STAT_HAS_MTIME; }
            let u = una_abi::UserStat {
                kind: if matches!(st.kind, crate::fs::vfs::NodeKind::Dir) { 1 } else { 0 },
                flags,
                size: st.size,
                id: st.id.unwrap_or(0),
                mtime: st.mtime.unwrap_or(0),
            };
            out.extend_from_slice(&u.to_bytes());
            0
        }
        Err(e) => errno_of(&e),
    }
}

/// The syscall body, arch-neutral: `inb` is the already-copied-in first buffer (request / path /
/// expression). Returns `Ok(out)` for the arch arm to copy out, or the errno. The out ceiling is
/// `ATTR_OUT_MAX`; `cap` (the caller's buffer) smaller than the answer is `-ERANGE`, checked here
/// so both arches refuse identically.
pub fn syscall_fulfil(nr: u64, inb: &[u8], principal: &str, cap: usize) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    let r = match nr {
        una_abi::SYS_ATTR_SET => do_set(inb, principal),
        una_abi::SYS_ATTR_GET => do_get(inb, principal, &mut out),
        una_abi::SYS_ATTR_LIST => do_list(inb, principal, &mut out),
        una_abi::SYS_QUERY => do_query(inb, principal, &mut out),
        una_abi::SYS_STAT => do_stat(inb, principal, &mut out),
        _ => una_abi::ENOSYS,
    };
    if r < 0 {
        return Err(r);
    }
    if out.len() > una_abi::ATTR_OUT_MAX || out.len() > cap {
        return Err(una_abi::ERANGE);
    }
    Ok(out)
}

/// Upper bound on the first (input) buffer of any attribute syscall — one bus body.
pub const IN_MAX: usize = una_abi::BUS_BODY_MAX;

/// The bus arm, both arches: validate the typed body (fail-closed `-EINVAL`, the frame rule), then
/// the SAME function the syscall calls. The reply body is the syscall's output bytes.
#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
pub fn bus_fulfil(verb: u8, body: &[u8], principal: &str, text: &mut Vec<u8>) -> i64 {
    if crate::bus::attr::attr_body_parse(verb, body).is_err() {
        return una_abi::EINVAL;
    }
    let nr = match verb {
        una_abi::BUS_VERB_ATTR_SET => una_abi::SYS_ATTR_SET,
        una_abi::BUS_VERB_ATTR_GET => una_abi::SYS_ATTR_GET,
        una_abi::BUS_VERB_ATTR_LIST => una_abi::SYS_ATTR_LIST,
        una_abi::BUS_VERB_ATTR_QUERY => una_abi::SYS_QUERY,
        _ => una_abi::SYS_STAT,
    };
    match syscall_fulfil(nr, body, principal, una_abi::ATTR_OUT_MAX) {
        Ok(out) => { *text = out; 0 }
        Err(e) => e,
    }
}

// --- M4: `tests attr` -----------------------------------------------------------------------------

/// `tests attr` — the surface end to end on a real volume, through the mount table (the path every
/// consumer takes): create a file, set int/string/vector attributes as its owner, get each back
/// typed, list, query by equality and by vector similarity, deny a foreign principal (get, set, and
/// the query hit is dropped), remove one and confirm the query no longer finds it, clean up.
///
/// The target directory is the first of `/home`, `/` whose volume carries typed attributes. A tree
/// with none (FAT only — today's rMBP until UNAFSX86) prints SKIP with `reason=no-unafs-volume`,
/// never FAIL.
///
/// Witness: `:: ATTRSURF: set=<n> get=<n> list=<n> query=<n> denied=<n> dir=<d> -> PASS|FAIL ::`.
pub fn selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as K};
    const A: &str = "user:attra#9001";
    const B: &str = "user:attrb#9002";
    let mt = table();
    let dir = ["/home", "/"].iter().copied().find(|d| {
        !matches!(mt.list_attrs(d, K), Err(VfsError::Unsupported) | Err(VfsError::NoSuchVolume) | Err(VfsError::NoSuchPath))
    });
    let Some(dir) = dir else {
        serial_println!(":: ATTRSURF: set=0 get=0 list=0 query=0 denied=0 reason=no-unafs-volume -> SKIP ::");
        return;
    };
    let path = if dir == "/" { String::from("/ATTRSURF.T") } else { alloc::format!("{}/ATTRSURF.T", dir) };
    let _ = mt.unlink(&path, K);
    // Created by the kernel, then GIVEN an owner — the one place a reserved key is set, and only the
    // kernel may (a non-kernel `owner` write is refused by the adapter).
    if mt.create(&path, NodeKind::File, K).is_err() || mt.set_attr(&path, "owner", AttrValue::Str(String::from(A)), K).is_err() {
        serial_println!(":: ATTRSURF: set=0 get=0 list=0 query=0 denied=0 dir={} reason=create -> FAIL ::", dir);
        return;
    }
    let vals = [
        ("attrsurf.n", AttrValue::Int(42)),
        ("attrsurf.s", AttrValue::Str(String::from("hello"))),
        ("attrsurf.v", AttrValue::Vector(alloc::vec![1.0, 0.0, 0.0])),
    ];
    let mut set = 0u32;
    let mut get = 0u32;
    for (k, v) in vals.iter() {
        if mt.set_attr(&path, k, v.clone(), A).is_ok() {
            set += 1;
        }
    }
    for (k, v) in vals.iter() {
        if mt.get_attr(&path, k, A).as_ref() == Ok(v) {
            get += 1;
        }
    }
    let list = match mt.list_attrs(&path, A) {
        Ok(rows) => rows.iter().filter(|(k, v)| vals.iter().any(|(vk, vv)| vk == k && vv == v)).count() as u32,
        Err(_) => 0,
    };
    let hits = |expr: &str, who: &str| mt.query(expr, who).map(|h| h.iter().any(|(_, p)| *p == path)).unwrap_or(false);
    let eq = "attrsurf.s == \"hello\"";
    let sim = "similarity(attrsurf.v, [1.0, 0.0, 0.0]) > 0.9";
    let query = hits(eq, A) as u32 + hits(sim, A) as u32;
    let denied = matches!(mt.get_attr(&path, "attrsurf.n", B), Err(VfsError::Denied)) as u32
        + matches!(mt.set_attr(&path, "attrsurf.n", AttrValue::Int(7), B), Err(VfsError::Denied)) as u32
        + (!hits(eq, B)) as u32;
    let removed = mt.remove_attr(&path, "attrsurf.s", A).is_ok() && !hits(eq, A);
    let _ = mt.unlink(&path, K);
    let pass = set == 3 && get == 3 && list == 3 && query == 2 && denied == 3 && removed;
    serial_println!(
        ":: ATTRSURF: set={} get={} list={} query={} denied={} removed={} dir={} -> {} ::",
        set, get, list, query, denied, removed as u8, dir, if pass { "PASS" } else { "FAIL" }
    );
}
