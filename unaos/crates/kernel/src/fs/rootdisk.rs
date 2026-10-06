//! CHARTER: Kernel — fs-core
//!
//! ROOTDISK (rmbp-ledger B390) — R94, Peter at the glass, flight 24: "apps/lib should be moved to lib or even
//! system/lib? where's volumes/UnaOS/apps? /apps should not exist without there being a /volumes/(UnaOS system boot
//! disk)/apps … even if a user creates a directory at root that root is the primary boot disk and where the new
//! folder is for real. obviously root is a special place where invisible links exist."
//!
//! The mount table stays the ONE place a name is decided (no second store, no copy):
//! * **`/volumes/UnaOS` IS `/`.** [`redirect`] (asked first by `MountTable::resolve`) resolves a path under the root's
//!   Volumes entry as the same path without the prefix, through the WHOLE table — so `/volumes/UnaOS/apps` reaches the
//!   `/apps` mount (the same backend object and the same relative path: one inode, two paths), a directory made at `/`
//!   is the one seen under `/volumes/UnaOS/`, and the root's own links (`volumes`, `boot`) are refused under it.
//!   `shell::vfs_ls_collect` lists the aliased path as `/` ([`unalias`]) minus those links ([`present`]).
//! * **`/lib`.** The ring-3 libraries leave the program directory: the builder stages `LIB/` at the boot partition's
//!   root, [`bind_lib`] mounts it at `/lib` (the `/apps` shape: the boot FAT rooted at a directory, volume name `boot`).
//!   For ONE image `/apps/LIB/…` still resolves on `/lib` ([`compat_rel`]) and says so once per subtree, naming the task.
//! * **Links drawn as links.** [`mark`]: at `/` (and `/volumes/UnaOS`) the root's special entries take `ls -F`'s `@`.
//!
//! Witness: `tests rootdisk` → `:: ROOTDISK: root=UnaOS lib=/lib apps_alias=same-inode
//! user_dir_at_root=visible-under-volume -> PASS :: …`. Owed (docs/dev/evidence/rmbp-1005/rootdisk.md): `apps`/`lib`
//! physically on UnaFS, and R94's hot-swap of the system disk.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::fs::vfs::{MountTable, NodeKind, VfsBackend, VfsError};
use crate::fs::volumes::ROOT_POINT;

/// Where the ring-3 libraries are seen (R94: "apps/lib should be moved to lib").
pub const LIB_POINT: &str = "/lib";
/// Their directory at the boot partition's root (the builder stages `target/LIB` there).
pub const LIB_DIR: &str = "LIB";
/// The old spelling, resolved on `/lib` for ONE image.
pub const COMPAT: &str = "/apps/LIB";
/// The root's special entries: mounts of the boot partition's directories and the Volumes namespace.
pub const LINKS: [&str; 4] = ["apps", "boot", "lib", "volumes"];
/// The root links that are NOT on the volume (another volume, or the namespace of volumes itself).
const OFF_VOLUME: [&str; 2] = ["boot", "volumes"];

static COMPAT_HITS: AtomicU32 = AtomicU32::new(0);
static COMPAT_SAID: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());

/// `path` below `prefix` at a component boundary: the remainder (`/` for the prefix itself).
fn under<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    match rest.as_bytes().first() {
        None => Some("/"),
        Some(b'/') => Some(rest),
        Some(_) => None,
    }
}

fn first(p: &str) -> &str {
    p.trim_start_matches('/').split('/').next().unwrap_or("")
}

fn off_volume(inner: &str) -> bool {
    let h = first(inner);
    OFF_VOLUME.iter().any(|l| l.eq_ignore_ascii_case(h))
}

/// Is the root's Volumes entry live — `/` the native UnaFS volume and `/volumes/UnaOS` bound over it?
fn alias_live(mt: &MountTable) -> bool {
    mt.prefixes().iter().any(|p| *p == ROOT_POINT) && mt.volume_name("/").map(|n| n == "native").unwrap_or(false)
}

/// `/apps/LIB…` (FAT spelling, any case) → the remainder below it.
pub fn compat_rel(path: &str) -> Option<&str> {
    let head = path.get(..COMPAT.len())?;
    if !head.eq_ignore_ascii_case(COMPAT) {
        return None;
    }
    let rest = &path[COMPAT.len()..];
    match rest.as_bytes().first() {
        None | Some(b'/') => Some(rest),
        Some(_) => None,
    }
}

fn compat_note(path: &str, rest: &str) {
    COMPAT_HITS.fetch_add(1, Ordering::Relaxed);
    let sub = String::from(first(rest));
    let mut said = COMPAT_SAID.lock();
    if said.iter().any(|s| *s == sub) {
        return;
    }
    said.push(sub);
    drop(said);
    serial_println!(
        "[rootdisk] compat /apps/LIB -> /lib path={} task={} (R94: one image, then it goes) ::",
        path,
        current_task()
    );
}

fn current_task() -> &'static str {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        crate::arch::sched::current_name().unwrap_or("kernel")
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "kernel"
    }
}

/// Asked FIRST by `MountTable::resolve`: `Some` when the root disk's rules decide the path, `None` to resolve as before.
pub fn redirect<'a>(mt: &'a MountTable, path: &'a str) -> Option<Result<(&'a dyn VfsBackend, &'a str), VfsError>> {
    if let Some(inner) = under(path, ROOT_POINT) {
        if !alias_live(mt) {
            return None;
        }
        if off_volume(inner) {
            return Some(Err(VfsError::NoSuchPath));
        }
        return Some(mt.resolve(inner));
    }
    if let Some(rest) = compat_rel(path) {
        if !mt.prefixes().iter().any(|p| *p == LIB_POINT) {
            return None;
        }
        let r = mt.resolve(LIB_POINT).map(|(b, _)| (b, rest));
        compat_note(path, rest);
        return Some(r);
    }
    None
}

/// `shell::vfs_ls_collect`: the root path a `/volumes/UnaOS/…` listing is (None: list `path` itself).
pub fn unalias(mt: &MountTable, path: &str) -> Option<String> {
    let inner = under(path.trim_end_matches('/'), ROOT_POINT).or_else(|| under(path, ROOT_POINT))?;
    if !alias_live(mt) || off_volume(inner) {
        return None;
    }
    Some(String::from(inner))
}

/// `shell::vfs_ls_collect`: the listing of `/volumes/UnaOS` is `/`'s without the root's off-volume links.
pub fn present(orig: &str, mut rows: Vec<crate::fs::vfs::DirEnt>) -> Vec<crate::fs::vfs::DirEnt> {
    if orig.trim_end_matches('/') == ROOT_POINT {
        rows.retain(|e| !OFF_VOLUME.iter().any(|l| l.eq_ignore_ascii_case(&e.name)));
    }
    rows
}

/// Is `name` in directory `dir` one of the root's links?
pub fn is_link(dir: &str, name: &str) -> bool {
    let d = dir.trim_end_matches('/');
    (d.is_empty() || d == ROOT_POINT) && LINKS.iter().any(|l| l.eq_ignore_ascii_case(name))
}

/// Quarry's directory mark (`ls -F`): `@` for a root link, `/` for every other directory.
pub fn mark(dir: &str, name: &str) -> u8 {
    if is_link(dir, name) { b'@' } else { b'/' }
}

/// Mount `/lib` — the boot FAT rooted at `LIB/` — beside `/apps`. Called from `bootdisk::bind_root`.
pub fn bind_lib(mt: &mut MountTable, src: crate::fs::fat::BlockSource, announce: bool) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    mt.mount(
        LIB_POINT,
        alloc::boxed::Box::new(FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src).rooted(LIB_DIR)),
    );
    if announce {
        serial_println!(
            "[vfs] lib mount /lib = fat boot volume source={} rooted={} (R94: was /apps/LIB) ::",
            src.name(),
            LIB_DIR
        );
    }
}

fn names(path: &str) -> Vec<(String, bool)> {
    match crate::shell::vfs_ls_collect(path) {
        Ok((true, rows)) => rows.into_iter().map(|e| (e.name, matches!(e.kind, NodeKind::Dir))).collect(),
        _ => Vec::new(),
    }
}

/// `tests rootdisk` — R94 read off the live table: the root's Volumes entry is `/`, `/lib` holds the libraries,
/// `/volumes/UnaOS/apps` is the `/apps` directory itself, and a directory at `/` is the one under `/volumes/UnaOS`
/// (made as `/newfolder` when no user directory stands at `/`).
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    if !mt.volume_name("/").map(|n| n == "native").unwrap_or(false) {
        serial_println!(":: ROOTDISK: root=fat lib=- apps_alias=- user_dir_at_root=- -> SKIP :: reason=fat-root (no UnaFS volume is /) ::");
        return;
    }
    let root_tok = if alias_live(&mt) { "UnaOS" } else { "unaliased" };
    let lib_rows = names(LIB_POINT);
    let stale = mt.stat("/boot/APPS/LIB").is_ok();
    let lib_tok = match (lib_rows.is_empty(), stale) {
        (false, false) => "/lib",
        (false, true) => "/lib+stale-apps-LIB",
        (true, true) => "absent(still-apps-LIB)",
        (true, false) => "absent(unstaged)",
    };
    let alias_rows = names(ROOT_POINT);
    let apps_tok = {
        let a = mt.resolve("/apps").ok();
        let b = mt.resolve("/volumes/UnaOS/apps").ok();
        let same = match (a, b) {
            (Some((x, rx)), Some((y, ry))) => core::ptr::addr_eq(x as *const dyn VfsBackend, y as *const dyn VfsBackend) && rx == ry,
            _ => false,
        };
        let home = match (mt.stat("/home"), mt.stat("/volumes/UnaOS/home")) {
            (Ok(x), Ok(y)) => x.id.is_some() && x.id == y.id,
            _ => false,
        };
        let listed = alias_rows.iter().any(|(n, _)| n == "apps") && !alias_rows.iter().any(|(n, _)| n == "volumes");
        if !same {
            "different"
        } else if !home {
            "home-differs"
        } else if !listed {
            "unlisted"
        } else {
            "same-inode"
        }
    };
    let root_rows = names("/");
    let mut made = String::from("-");
    let user = root_rows
        .iter()
        .find(|(n, d)| *d && !LINKS.iter().chain(["home", "system", "var"].iter()).any(|l| l.eq_ignore_ascii_case(n)))
        .map(|(n, _)| n.clone());
    let user = match user {
        Some(u) => Ok(u),
        None => match mt.create("/newfolder", NodeKind::Dir, crate::fs::vfs::KERNEL_PRINCIPAL) {
            Ok(_) => {
                made = String::from("newfolder");
                Ok(String::from("newfolder"))
            }
            Err(e) => Err(alloc::format!("mkdir-failed({:?})", e)),
        },
    };
    let user_tok = match &user {
        Ok(u) => {
            let p = alloc::format!("/{}", u);
            let q = alloc::format!("{}/{}", ROOT_POINT, u);
            let ids = matches!((mt.stat(&p), mt.stat(&q)), (Ok(x), Ok(y)) if x.id.is_some() && x.id == y.id);
            let listed = names(ROOT_POINT).iter().any(|(n, _)| n == u);
            if ids && listed { String::from("visible-under-volume") } else { alloc::format!("missing-under-volume({})", u) }
        }
        Err(e) => e.clone(),
    };
    let links: Vec<&str> = root_rows.iter().filter(|(n, d)| *d && is_link("/", n)).map(|(n, _)| n.as_str()).collect();
    let ok = root_tok == "UnaOS" && lib_tok == "/lib" && apps_tok == "same-inode" && user_tok == "visible-under-volume";
    serial_println!(
        ":: ROOTDISK: root={} lib={} apps_alias={} user_dir_at_root={} -> {} :: made={} user={} compat_hits={} links={} ::",
        root_tok,
        lib_tok,
        apps_tok,
        user_tok,
        if ok { "PASS" } else { "FAIL" },
        made,
        user.as_deref().unwrap_or("-"),
        COMPAT_HITS.load(Ordering::Relaxed),
        if links.is_empty() { String::from("-") } else { links.join(",") }
    );
}
