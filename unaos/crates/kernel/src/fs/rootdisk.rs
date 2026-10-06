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
//! * **`/lib`.** The ring-3 libraries leave the program directory (`/apps/LIB` → `/lib`). On a FAT root [`bind_lib`]
//!   mounts the boot FAT's `LIB/` there (the `/apps` shape); the one-image `/apps/LIB` compat is gone (ROOTDISK2).
//! * **Links drawn as links.** [`mark`]: at `/` (and `/volumes/UnaOS`) the root's special entries take `ls -F`'s `@`.
//!
//! ROOTDISK2 (rmbp-ledger B401) — R94 in full, "where the new folder is for real": on a NATIVE root `/apps` and `/lib`
//! are not mounted at all — they are directories of the UnaFS volume (`arroyo` puts them there; `bootdisk::bind_root`
//! skips the FAT mounts), so the root's only links are `boot` and `volumes`. [`program_source`] is the one place a
//! ring-0 launcher reads a program: `/apps/<NAME>` through the mount table, whichever volume `/apps` is.
//!
//! Witnesses: `tests rootdisk` → `:: ROOTDISK: root=UnaOS lib=/lib apps_alias=same-inode
//! user_dir_at_root=visible-under-volume -> PASS :: …` and `:: ROOTDISK2: apps=unafs lib=unafs var=unafs
//! boot_fat=readonly unlock_path=installer rmdir=ok image_mb=<n> -> PASS :: links=volumes-only …` (R99: `fs::bootfat`). Owed: R94's hot swap (docs/dev/evidence/rmbp-1005/rootdisk2.md, design only).

use alloc::string::String;
use alloc::vec::Vec;
use crate::fs::vfs::{MountTable, NodeKind, VfsBackend, VfsError};
use crate::fs::volumes::ROOT_POINT;

/// Where the ring-3 libraries are seen (R94: "apps/lib should be moved to lib").
pub const LIB_POINT: &str = "/lib";
/// Where programs are seen (`shell::EXEC_ROOT`).
pub const APPS_POINT: &str = "/apps";
/// Their directory at the boot partition's root, on a FAT root (the builder stages `target/LIB` there).
pub const LIB_DIR: &str = "LIB";
/// The root's special entries (ROOTDISK2: `apps` and `lib` are real directories of the root volume, not links).
pub const LINKS: [&str; 2] = ["boot", "volumes"];
/// The root links that are NOT on the volume (another volume, or the namespace of volumes itself).
const OFF_VOLUME: [&str; 2] = ["boot", "volumes"];
/// The directory `tests rootdisk` makes and removes to prove `rmdir` (ROOTDISK2).
const RMDIR_PROBE: &str = "rootdisk2-probe";

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

/// Mount `/lib` — the boot FAT rooted at `LIB/` — beside `/apps`. Called from `bootdisk::bind_root` on a FAT root only
/// (ROOTDISK2: on a native root `/lib` is the UnaFS volume's own directory).
pub fn bind_lib(mt: &mut MountTable, src: crate::fs::fat::BlockSource, announce: bool) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    mt.mount(
        LIB_POINT,
        alloc::boxed::Box::new(FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src).rooted(LIB_DIR)),
    );
    if announce {
        serial_println!(
            "[vfs] lib mount /lib = fat boot volume source={} rooted={} (R94: fat root) ::",
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
    let stale = mt.stat("/boot/APPS/LIB").is_ok() || mt.stat("/boot/LIB").is_ok() && !mt.prefixes().iter().any(|p| *p == LIB_POINT);
    let lib_tok = match (lib_rows.is_empty(), stale) {
        (false, false) => "/lib",
        (false, true) => "/lib+stale-esp-LIB",
        (true, true) => "absent(still-on-esp)",
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
        .find(|(n, d)| *d && !LINKS.iter().chain(["apps", "home", "lib", "system", "var", RMDIR_PROBE].iter()).any(|l| l.eq_ignore_ascii_case(n)))
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
        ":: ROOTDISK: root={} lib={} apps_alias={} user_dir_at_root={} -> {} :: made={} user={} links={} ::",
        root_tok,
        lib_tok,
        apps_tok,
        user_tok,
        if ok { "PASS" } else { "FAIL" },
        made,
        user.as_deref().unwrap_or("-"),
        if links.is_empty() { String::from("-") } else { links.join(",") }
    );
    rootdisk2(&mt, if made == "newfolder" { Some("newfolder") } else { None });
}

// ── ROOTDISK2 (rmbp-ledger B401, R94 in full) ─────────────────────────────────────────────────────────────────────

/// Does this root carry `/apps` and `/lib` as its own directories? x86 with a native UnaFS root (the rMBP card, the
/// installed SSD). The Pi and Orin native roots keep the FAT program source by design (their cards stage APPS/ on FAT).
pub fn apps_on_root(native_root: bool) -> bool {
    native_root && cfg!(target_arch = "x86_64")
}

/// `bootdisk::bind_root`, on a NATIVE root: `/apps` and `/lib` are the UnaFS volume's own directories, so nothing is
/// mounted for them — said once, where the FAT shape says `[vfs] apps mount …` and `[vfs] lib mount …`.
pub fn announce_native(src: crate::fs::fat::BlockSource) {
    serial_println!(
        "[vfs] apps+lib on the native root: /apps /lib are directories of / source={} (R94: real, no links) ::",
        src.name()
    );
}

/// A program as [`program_source`] found it: the `FatFs::find_app` row's two facts the launchers read.
pub struct AppEnt {
    pub size: u32,
    pub is_dir: bool,
    path: String,
}

/// The one program source for the ring-0 launchers and witnesses (Lumen, the desktop app, WINX-2/8, PULSE-W, the
/// loginst STAT fixtures, shotmask): `/apps/<NAME>` through the mount table — the UnaFS directory on a native root,
/// the boot FAT's `APPS/` on a FAT root. It keeps `FatFs`'s method names so each caller swaps one call on one line.
pub struct Programs {
    mt: MountTable,
}

/// `Err` when no table is bound yet (before any disk enumerated) — the callers' "no program volume" arm.
pub fn program_source() -> Result<Programs, ()> {
    let mt = crate::shell::vfs_mount_table();
    if mt.prefixes().is_empty() {
        return Err(());
    }
    Ok(Programs { mt })
}

impl Programs {
    /// `name` in `/apps`, matched exactly first and then case-insensitively (UnaFS names are case-sensitive; the
    /// callers spell the staged upper-case 8.3 names, and a FAT `APPS/` answers either way).
    pub fn find_app(&self, name: &str) -> Result<AppEnt, ()> {
        let exact = alloc::format!("{}/{}", APPS_POINT, name);
        let path = if self.mt.stat(&exact).is_ok() {
            exact
        } else {
            let rows = self.mt.read_dir(APPS_POINT).map_err(|_| ())?;
            let hit = rows.iter().find(|r| r.name.eq_ignore_ascii_case(name)).ok_or(())?;
            alloc::format!("{}/{}", APPS_POINT, hit.name)
        };
        let st = self.mt.stat(&path).map_err(|_| ())?;
        Ok(AppEnt { size: st.size.min(u32::MAX as u64) as u32, is_dir: matches!(st.kind, NodeKind::Dir), path })
    }

    /// The whole file (at most `max_bytes`) REPLACING `out` — `FatFs::read_file`'s contract (FATREAD-1).
    pub fn read_file(&self, de: &AppEnt, out: &mut Vec<u8>, max_bytes: usize) -> Result<(), ()> {
        out.clear();
        let want = (de.size as usize).min(max_bytes);
        while out.len() < want {
            let chunk = self.mt.read(&de.path, out.len() as u64, want - out.len()).map_err(|_| ())?;
            if chunk.is_empty() {
                break;
            }
            out.extend_from_slice(&chunk);
        }
        Ok(())
    }
}

/// Which volume a root entry is: `unafs` (a directory of the native root, no mount of its own), `fat-link` (a mount
/// of the boot FAT over the root), `absent`.
fn home_of(mt: &MountTable, point: &str) -> &'static str {
    if mt.prefixes().iter().any(|p| *p == point) {
        return "fat-link";
    }
    match mt.stat(point) {
        Ok(st) if matches!(st.kind, NodeKind::Dir) && mt.volume_name(point).map(|n| n == "native").unwrap_or(false) => "unafs",
        Ok(_) => "not-native",
        Err(_) => "absent",
    }
}

/// `tests rootdisk`, second line: R94 in full — `/apps` and `/lib` are UnaFS directories, the root's only link is
/// `volumes` (Quarry's view: `boot` is under Volumes), a directory made at `/` is seen under `/volumes/UnaOS` and is
/// REMOVED by the VFS `rmdir` (`UnaFS::rmdir`, one CoW transaction) from both paths, and the volume's size.
/// `made` is the folder ROOTDISK's leg made (removed here too, so the test leaves the root as it found it).
fn rootdisk2(mt: &MountTable, made: Option<&str>) {
    use crate::fs::vfs::KERNEL_PRINCIPAL;
    let apps = home_of(mt, APPS_POINT);
    let lib = home_of(mt, LIB_POINT);
    let progs = names(APPS_POINT).iter().filter(|(_, d)| !*d).count();
    let root = crate::fs::volumes::view("/", names_ents("/"), crate::fs::volumes::boot_alias_bound());
    let links: Vec<&str> = root.iter().filter(|e| matches!(e.kind, NodeKind::Dir) && is_link("/", &e.name)).map(|e| e.name.as_str()).collect();
    let links_tok = match links.as_slice() {
        ["volumes"] => String::from("volumes-only"),
        [] => String::from("-"),
        l => l.join("+"),
    };
    let probe = alloc::format!("/{}", RMDIR_PROBE);
    let under = alloc::format!("{}/{}", ROOT_POINT, RMDIR_PROBE);
    let rmdir_tok: String = match mt.create(&probe, NodeKind::Dir, KERNEL_PRINCIPAL) {
        Err(e) => alloc::format!("mkdir-failed({:?})", e),
        Ok(_) => {
            let seen = mt.stat(&under).is_ok();
            match mt.remove_dir(&probe, KERNEL_PRINCIPAL) {
                Err(e) => alloc::format!("rmdir-failed({:?})", e),
                Ok(()) if !seen => String::from("unseen-under-volume"),
                Ok(()) if mt.stat(&probe).is_ok() || mt.stat(&under).is_ok() => String::from("still-there"),
                Ok(()) => String::from("ok"),
            }
        }
    };
    let cleaned = match made {
        None => "-",
        Some(m) => match mt.remove_dir(&alloc::format!("/{}", m), KERNEL_PRINCIPAL) {
            Ok(()) => "newfolder",
            Err(_) => "left",
        },
    };
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    let image_mb = crate::fs::unafs::with_unafs(|fs| fs.superblock.block_count).map(|b| (b * 4096) >> 20).unwrap_or(0);
    #[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
    let image_mb = 0u64;
    let var = home_of(mt, "/var");
    let boot_fat = crate::fs::bootfat::posture(mt);
    let unlock = crate::fs::bootfat::unlock_path();
    let ok = apps == "unafs"
        && lib == "unafs"
        && var == "unafs"
        && boot_fat == "readonly"
        && unlock == "installer"
        && links_tok == "volumes-only"
        && rmdir_tok == "ok"
        && cleaned != "left";
    serial_println!(
        ":: ROOTDISK2: apps={} lib={} var={} boot_fat={} unlock_path={} rmdir={} image_mb={} -> {} :: links={} progs={} cleaned={} ::",
        apps,
        lib,
        var,
        boot_fat,
        unlock,
        rmdir_tok,
        image_mb,
        if ok { "PASS" } else { "FAIL" },
        links_tok,
        progs,
        cleaned
    );
}

fn names_ents(path: &str) -> Vec<crate::fs::vfs::DirEnt> {
    match crate::shell::vfs_ls_collect(path) {
        Ok((true, rows)) => rows,
        _ => Vec::new(),
    }
}
