// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! PREFS (rmbp-ledger B300; AUDIT B287, R79) — the kernel reads and writes PRINCIPIA'S preference store.
//! There is ONE store: `<home>/.config/unaos/preferences.toml`, the file `handlers/principia` owns on the
//! host, in Principia's format (one TOML table per namespace, dotted keys, four scalar types). The value
//! model and the codec are `unaos/libs/sys/prefs_core` — the same crate Principia links — so the two rings
//! cannot drift: what the kernel writes, Principia reads unchanged, and the reverse (the cross-tests in
//! `handlers/principia/src/prefs.rs` pin it). Before this file the kernel kept `<home>/.settings` and
//! `<home>/.dock`, two private stores in two private formats; their reading is DELETED (a one-shot import
//! on first load is the only thing that still opens them, and it deletes them: `[prefs] migrated=<n>`).
//!
//! # Where and when
//!
//! The home comes from `fs::users::home_of` for the session user (never a `/home/<name>` literal — the
//! UnaFS root on x86 spells it differently); no session = `/.config/unaos/preferences.toml`. The file is
//! read ONCE per login ([`service`], chained from the desktop service passes; the verb, the bus and the
//! fixture call [`ensure_loaded`] themselves). [`get`] never does I/O, so it is safe on any path.
//!
//! # The write: the USERS.DAT swap (fs/users.rs), through the mount table
//!
//! `preferences.toml.new` is written, READ BACK and PARSED (it must be the tree that was serialized), and
//! only then does the old file go and the temp get renamed over it. A load that finds no
//! `preferences.toml` but a parseable `.new` adopts it. The missing directories are created.
//!
//! # A refused file
//!
//! A file `prefs_core` refuses (anything outside the subset, with its line number) is NOT adopted: the
//! defaults hold, and saves are HELD (`[prefs] save held`) so the operator's file is never overwritten by a
//! tree that does not contain it — Principia's own rule (a malformed file is an error, not a silent wipe).
//!
//! # Keys (namespace `system`)
//!
//! `display.brightness` (1..16; BRIGHTFLOOR clamps a stored 0 on load and save) · `display.idle_min` (minutes, 0 = never) · `display.wallpaper` (a path,
//! empty = off) · `audio.volume` (0..16) · `audio.mute` (bool) · `pointer.speed` (0 slow, 1 normal, 2 fast)
//! · `power.lowbat_shutdown_pct` (0 = off) · `dock.pins` (comma-joined app names — TOML arrays are outside
//! the subset) · `settings.tab` (0..3). Defaults live with each consumer, as in Principia.
//!
//! # Bus (M3)
//!
//! `PREF_GET` / `PREF_SET` / `PREF_LIST` (una-abi 11/12/13), fulfilled by [`bus_fulfil`] under the
//! caller's principal: any principal may READ; only a program running in the open session (x86: its slot
//! carries the session's uid; aarch64: its principal is the session's `user:` record) may SET, in any
//! namespace — the file is the session user's. PrefChanged: see [`changed`].
//!
//! Witness: `:: PREFS: path=<p> loaded=<n> saved=<n> ns=system -> PASS ::` on load and on save;
//! `[prefs] set <ns>.<key>=<value> ok=<0|1>` per change; `[prefs] changed <ns>.<key>` per accepted change.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub use prefs_core::{PrefTree, PrefValue};

/// The kernel's namespace.
pub const NS: &str = "system";

/// The `system` keys the kernel reads and writes.
pub mod key {
    pub const BRIGHTNESS: &str = "display.brightness";
    pub const IDLE_MIN: &str = "display.idle_min";
    pub const WALLPAPER: &str = "display.wallpaper";
    pub const VOLUME: &str = "audio.volume";
    pub const MUTE: &str = "audio.mute";
    pub const POINTER: &str = "pointer.speed";
    pub const LOWBAT_PCT: &str = "power.lowbat_shutdown_pct";
    pub const DOCK_PINS: &str = "dock.pins";
    pub const SETTINGS_TAB: &str = "settings.tab";
}

static TREE: spin::Mutex<PrefTree> = spin::Mutex::new(PrefTree::new());
/// The session user the tree was loaded for (`None` = never loaded; `Some("")` = no session).
static LOADED_FOR: spin::Mutex<Option<String>> = spin::Mutex::new(None);
static LOADED_N: AtomicU32 = AtomicU32::new(0);
static SAVED_N: AtomicU32 = AtomicU32::new(0);
/// The file on disk was refused at load: saves are held so it is never overwritten.
static HELD: AtomicBool = AtomicBool::new(false);
/// File-size ceiling for a read (a preferences file is a few hundred bytes).
const READ_MAX: usize = 64 * 1024;

// ── Where ────────────────────────────────────────────────────────────────────────────────────

fn user_name() -> Option<String> {
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        let n = crate::fs::users::whoami(&mut nm)?;
        return core::str::from_utf8(&nm[..n]).ok().map(String::from);
    }
    #[cfg(not(feature = "login"))]
    {
        None
    }
}

/// The session user's home (from the users table — never a literal), without a trailing `/`.
pub fn home() -> Option<String> {
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        let n = crate::fs::users::whoami(&mut nm)?;
        let mut hb = [0u8; crate::fs::users::HOME_MAX];
        let hn = crate::fs::users::home_of(&nm[..n], &mut hb)?;
        return core::str::from_utf8(&hb[..hn]).ok().map(|h| String::from(h.trim_end_matches('/')));
    }
    #[cfg(not(feature = "login"))]
    {
        None
    }
}

fn base() -> String {
    home().unwrap_or_default()
}

/// `<home>/.config/unaos/preferences.toml` (Principia's path), or `/.config/unaos/preferences.toml`.
pub fn path() -> String {
    alloc::format!("{}/.config/unaos/preferences.toml", base())
}

// ── VFS helpers (mount table, kernel principal) ─────────────────────────────────────────────────

fn read_all(p: &str) -> Option<Vec<u8>> {
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(p).ok()?;
    if st.size as usize > READ_MAX {
        return None;
    }
    if st.size == 0 {
        return Some(Vec::new());
    }
    mt.read(p, 0, st.size as usize).ok()
}

fn write_all(p: &str, b: &[u8]) -> Result<(), String> {
    use crate::fs::vfs::NodeKind;
    let mt = crate::shell::vfs_mount_table();
    let k = crate::fs::vfs::KERNEL_PRINCIPAL;
    let _ = mt.unlink(p, k);
    mt.create(p, NodeKind::File, k).map_err(|e| alloc::format!("create {}: {:?}", p, e))?;
    let mut off = 0usize;
    while off < b.len() {
        let w = mt.write(p, off as u64, &b[off..], k).map_err(|e| alloc::format!("write {}: {:?}", p, e))?;
        if w == 0 {
            return Err(alloc::format!("write {}: zero", p));
        }
        off += w;
    }
    Ok(())
}

fn ensure_dirs() {
    use crate::fs::vfs::NodeKind;
    let mt = crate::shell::vfs_mount_table();
    let b = base();
    for d in [alloc::format!("{}/.config", b), alloc::format!("{}/.config/unaos", b)] {
        if mt.stat(&d).is_err() {
            let _ = mt.create(&d, NodeKind::Dir, crate::fs::vfs::KERNEL_PRINCIPAL);
        }
    }
}

// ── Load ─────────────────────────────────────────────────────────────────────────────────────

fn witness(ok: bool) {
    serial_println!(
        ":: PREFS: path={} loaded={} saved={} ns={} -> {} ::",
        path(), LOADED_N.load(Ordering::Relaxed), SAVED_N.load(Ordering::Relaxed), NS, if ok { "PASS" } else { "FAIL" }
    );
}

/// Parse `text`; a refusal is said with its line.
fn parse_said(p: &str, text: &[u8]) -> Result<PrefTree, prefs_core::ParseError> {
    let s = core::str::from_utf8(text).map_err(|_| prefs_core::ParseError { line: 1, why: "not UTF-8" })?;
    PrefTree::parse(s).inspect_err(|e| serial_println!("[prefs] refused path={} line={} why={}", p, e.line, e.why))
}

/// Read the file (or an orphaned `.new`) into the tree, then run the one-shot import.
fn load() {
    let p = path();
    let tmp = alloc::format!("{}.new", p);
    let (tree, ok) = match read_all(&p) {
        Some(t) => match parse_said(&p, &t) {
            Ok(tr) => (tr, true),
            Err(_) => (PrefTree::new(), false),
        },
        None => match read_all(&tmp).map(|t| parse_said(&tmp, &t)) {
            Some(Ok(tr)) => {
                serial_println!("[prefs] adopted {} (the swap was interrupted)", tmp);
                (tr, true)
            }
            _ => (PrefTree::new(), true),
        },
    };
    HELD.store(!ok, Ordering::Release);
    LOADED_N.store(tree.len() as u32, Ordering::Relaxed);
    SAVED_N.store(0, Ordering::Relaxed);
    *TREE.lock() = tree;
    witness(ok);
    if ok {
        migrate();
    }
}

/// Load once per login (and once with no session). Cheap when nothing changed.
pub fn ensure_loaded() {
    let u = user_name().unwrap_or_default();
    let fresh = {
        let mut g = LOADED_FOR.lock();
        if g.as_deref() != Some(u.as_str()) {
            *g = Some(u);
            true
        } else {
            false
        }
    };
    if fresh {
        load();
    }
}

/// The desktop service passes call this (settings, dock): the per-login load happens here, off the
/// click paths.
pub fn service() {
    ensure_loaded();
}

// ── The one-shot import of the two retired private stores ───────────────────────────────────────

/// `<home>/.settings` (`key=value`) and `<home>/.dock` (a name per line), if either still exists: every
/// key the tree does not already hold is imported, the files are DELETED, the tree is saved. Their
/// reading exists only here, and only until they are gone.
fn migrate() {
    let b = base();
    let (sp, dp) = (alloc::format!("{}/.settings", b), alloc::format!("{}/.dock", b));
    let (st, dt) = (read_all(&sp), read_all(&dp));
    if st.is_none() && dt.is_none() {
        return;
    }
    let mut n = 0u32;
    {
        let mut t = TREE.lock();
        let mut put = |k: &str, v: PrefValue| {
            if t.get(NS, k).is_none() && t.set(NS, k, v).is_ok() {
                n += 1;
            }
        };
        if let Some(text) = st.as_deref().and_then(|s| core::str::from_utf8(s).ok()) {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                let (k, v) = (k.trim(), v.trim());
                let num = v.parse::<i64>().ok();
                match (k, num) {
                    ("brightness", Some(x)) => put(key::BRIGHTNESS, PrefValue::Int(x)),
                    ("volume", Some(x)) => put(key::VOLUME, PrefValue::Int(x)),
                    ("mute", Some(x)) => put(key::MUTE, PrefValue::Bool(x != 0)),
                    ("idle_min", Some(x)) => put(key::IDLE_MIN, PrefValue::Int(x)),
                    ("pointer", Some(x)) => put(key::POINTER, PrefValue::Int(x)),
                    ("tab", Some(x)) => put(key::SETTINGS_TAB, PrefValue::Int(x)),
                    ("wallpaper", _) => put(key::WALLPAPER, PrefValue::Str(String::from(v))),
                    _ => {}
                }
            }
        }
        if let Some(text) = dt.as_deref().and_then(|s| core::str::from_utf8(s).ok()) {
            let names: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            if !names.is_empty() {
                put(key::DOCK_PINS, PrefValue::Str(names.join(",")));
            }
        }
    }
    let saved = save();
    if saved.is_ok() {
        let mt = crate::shell::vfs_mount_table();
        let k = crate::fs::vfs::KERNEL_PRINCIPAL;
        if st.is_some() {
            let _ = mt.unlink(&sp, k);
        }
        if dt.is_some() {
            let _ = mt.unlink(&dp, k);
        }
    }
    serial_println!("[prefs] migrated={} from={}{} deleted={}", n, if st.is_some() { ".settings " } else { "" }, if dt.is_some() { ".dock" } else { "" }, saved.is_ok() as u8);
}

// ── Save ─────────────────────────────────────────────────────────────────────────────────────

/// Serialize the tree and replace the file by the swap. `Ok(keys written)`.
pub fn save() -> Result<usize, String> {
    if HELD.load(Ordering::Acquire) {
        serial_println!("[prefs] save held: {} was refused at load; fix or remove it", path());
        return Err(String::from("held: the file on disk was refused at load"));
    }
    let (text, want, n) = {
        let t = TREE.lock();
        (t.to_toml(), t.clone(), t.len())
    };
    let p = path();
    let tmp = alloc::format!("{}.new", p);
    ensure_dirs();
    write_all(&tmp, text.as_bytes())?;
    // Read back and parse: the temp must BE the tree before the real file is touched.
    let back = read_all(&tmp).ok_or_else(|| String::from("read-back failed"))?;
    if back != text.as_bytes() {
        return Err(String::from("read-back differs"));
    }
    match core::str::from_utf8(&back).ok().map(PrefTree::parse) {
        Some(Ok(t)) if t == want || t.to_toml() == text => {} // re-emit equality covers a NaN value
        _ => return Err(String::from("read-back does not parse to the tree")),
    }
    let mt = crate::shell::vfs_mount_table();
    let k = crate::fs::vfs::KERNEL_PRINCIPAL;
    let _ = mt.unlink(&p, k);
    match mt.rename(&tmp, &p, k) {
        Ok(()) => {}
        Err(crate::fs::vfs::VfsError::Unsupported) => {
            // A backend with no rename: write the real file directly (said), then drop the temp.
            write_all(&p, text.as_bytes())?;
            let _ = mt.unlink(&tmp, k);
            serial_println!("[prefs] swap=direct (the volume has no rename)");
        }
        Err(e) => return Err(alloc::format!("rename: {:?}", e)),
    }
    SAVED_N.store(n as u32, Ordering::Relaxed);
    witness(true);
    Ok(n)
}

/// A copy of the in-memory tree.
pub fn snapshot() -> PrefTree {
    TREE.lock().clone()
}

/// The file on the volume, read through the VFS and parsed with prefs_core: `None` = no file,
/// `Some(Err)` = refused.
pub fn read_file() -> Option<Result<PrefTree, prefs_core::ParseError>> {
    read_all(&path()).map(|b| match core::str::from_utf8(&b) {
        Ok(t) => PrefTree::parse(t),
        Err(_) => Err(prefs_core::ParseError { line: 1, why: "not UTF-8" }),
    })
}

// ── Get / set / list ─────────────────────────────────────────────────────────────────────────

/// The value of `ns`/`key`, or `None`. Never does I/O.
pub fn get(ns: &str, k: &str) -> Option<PrefValue> {
    TREE.lock().get(ns, k).cloned()
}

/// `system.<k>` as an integer in `lo..=hi`, or `None` (unset, another type, or out of range).
pub fn int(k: &str, lo: i64, hi: i64) -> Option<i64> {
    get(NS, k).and_then(|v| v.as_int()).filter(|x| (lo..=hi).contains(x))
}

/// [`int`] without waiting: `None` when the tree is busy. For paths that must never spin (the
/// device-service task's power check).
pub fn peek_int(k: &str, lo: i64, hi: i64) -> Option<i64> {
    TREE.try_lock()?.get(NS, k).and_then(|v| v.as_int()).filter(|x| (lo..=hi).contains(x))
}

/// `system.<k>` as a bool.
pub fn flag(k: &str) -> Option<bool> {
    get(NS, k).and_then(|v| v.as_bool())
}

/// `system.<k>` as a string.
pub fn text(k: &str) -> Option<String> {
    get(NS, k).and_then(|v| v.as_str().map(String::from))
}

/// Every `(key, value)` in `ns`.
pub fn list(ns: &str) -> Vec<(String, PrefValue)> {
    TREE.lock().list(ns).into_iter().map(|(k, v)| (String::from(k), v.clone())).collect()
}

/// Every namespace holding a value.
pub fn namespaces() -> Vec<String> {
    TREE.lock().namespaces().into_iter().map(String::from).collect()
}

/// Set `ns`/`key` and persist. An unchanged value is not re-written. On a refused name or a failed save
/// the tree is rolled back (cache and file never disagree — Principia's rule). Prints the set line, and on
/// success the PrefChanged stand-in.
pub fn set(ns: &str, k: &str, v: PrefValue) -> Result<(), String> {
    ensure_loaded();
    let prev = TREE.lock().set(ns, k, v.clone());
    let r = match prev {
        Err(e) => Err(String::from(e.as_str())),
        Ok(Some(ref old)) if *old == v => Ok(false),
        Ok(old) => match save() {
            Ok(_) => Ok(true),
            Err(e) => {
                let mut t = TREE.lock();
                match old {
                    Some(o) => {
                        let _ = t.set(ns, k, o);
                    }
                    None => {
                        t.remove(ns, k);
                    }
                }
                Err(e)
            }
        },
    };
    serial_println!("[prefs] set {}.{}={} ok={}", ns, k, v, r.is_ok() as u8);
    match r {
        Ok(true) => {
            changed(ns, k, &v);
            Ok(())
        }
        Ok(false) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Set a `system` key; a failure is already said on the serial line, so callers that only persist
/// (the settings window, the dock, the brightness keys) ignore it.
pub fn set_sys(k: &str, v: PrefValue) {
    let _ = set(NS, k, v);
}

/// PrefChanged. Principia broadcasts `PrincipiaCommand::PrefChanged { ns, key, value }` on the host
/// bus; the kernel has no interest registration yet (BANDY-3 builds fulfiller registration), so the
/// signal is the serial line `[prefs] changed <ns>.<key>`, and the mailbox delivery is OWED BANDY-3.
/// The frame BANDY-3 carries, to every ring-3 program that registered interest:
///
/// ```text
/// header: kind = REPLY (2), verb = BUS_VERB_PREF_CHANGED (19), corr = 0, status = 0,
///         principal = the kernel reply record, body_len = n
/// body:   <ns> "." <key> NUL <value as a TOML literal (prefs_core::PrefValue::to_literal)>
/// ```
///
/// (the PREF_SET request body, unsolicited). Also the ATTRSURF hook: the key is mirrored as an attribute
/// on the preferences file once the VFS can set one ([`mirror_attr`]).
fn changed(ns: &str, k: &str, v: &PrefValue) {
    serial_println!("[prefs] changed {}.{}", ns, k);
    mirror_attr(ns, k, v);
}

/// ATTRSURF hook: stamp `system.*` keys as attributes on the preferences file (so `query` finds them on
/// UnaFS). The VFS has `remove_attr` only — `set_attr` is ATTRSURF's, not in this tree. OWED ATTRSURF:
/// when it lands this body becomes `mt.set_attr(&path(), &alloc::format!("{}.{}", ns, k), v)`.
fn mirror_attr(ns: &str, k: &str, v: &PrefValue) {
    let _ = (ns, k, v);
}

/// Split `ns.key` (the verb's and the bus's address form) and validate both halves.
pub fn split_addr(a: &str) -> Option<(&str, &str)> {
    let (ns, k) = a.split_once('.')?;
    (prefs_core::validate_ns(ns).is_ok() && prefs_core::validate_key(k).is_ok()).then_some((ns, k))
}

// ── The `pref` verb ──────────────────────────────────────────────────────────────────────────

/// `pref get <ns.key>` · `pref set <ns.key> <value>` · `pref list [<ns>]`. A value is a TOML literal
/// when it is one (`12`, `true`, `1.5`, `"a b"`), else the text as a string.
pub fn verb(args: &[&str], out: &mut dyn FnMut(&str)) {
    ensure_loaded();
    match args {
        ["get", a] => match split_addr(a) {
            Some((ns, k)) => match get(ns, k) {
                Some(v) => out(&alloc::format!("{} = {}", a, v)),
                None => out(&alloc::format!("pref: {} is unset", a)),
            },
            None => out("pref: address is <ns>.<key> ([A-Za-z0-9_-] segments)"),
        },
        ["set", a, rest @ ..] if !rest.is_empty() => match split_addr(a) {
            Some((ns, k)) => {
                let raw = rest.join(" ");
                match set(ns, k, PrefValue::infer(&raw)) {
                    Ok(()) => out(&alloc::format!("{} = {}", a, get(ns, k).map(|v| v.to_literal()).unwrap_or_default())),
                    Err(e) => out(&alloc::format!("pref: {} refused: {}", a, e)),
                }
            }
            None => out("pref: address is <ns>.<key> ([A-Za-z0-9_-] segments)"),
        },
        ["list"] | ["list", _] => {
            let nss = match args.get(1) {
                Some(ns) => alloc::vec![String::from(*ns)],
                None => namespaces(),
            };
            let mut any = false;
            for ns in nss {
                for (k, v) in list(&ns) {
                    out(&alloc::format!("{}.{} = {}", ns, k, v));
                    any = true;
                }
            }
            if !any {
                out("pref: nothing set");
            }
            out(&alloc::format!("({})", path()));
        }
        _ => out("usage: pref get <ns.key> | pref set <ns.key> <value> | pref list [<ns>]"),
    }
}

// ── Bus (M3): PREF_GET / PREF_SET / PREF_LIST ─────────────────────────────────────────────────

const ENOENT: i64 = -2;
const E2BIG: i64 = -7;
const EIO: i64 = -5;
const EACCES: i64 = -13;
const EINVAL: i64 = -22;

/// PREF_GET body: `<ns>.<key>` (ASCII). PREF_LIST body: `<ns>` or empty (every namespace). PREF_SET body:
/// `<ns>.<key>` NUL `<TOML scalar literal>`. Pure; `None` = BadBody (-EINVAL).
pub fn get_body_parse(body: &[u8]) -> Option<(&str, &str)> {
    split_addr(core::str::from_utf8(body).ok()?)
}
pub fn set_body_parse(body: &[u8]) -> Option<(&str, &str, PrefValue)> {
    let z = body.iter().position(|&b| b == 0)?;
    let (ns, k) = get_body_parse(&body[..z])?;
    let lit = core::str::from_utf8(&body[z + 1..]).ok()?;
    Some((ns, k, PrefValue::from_literal(lit).ok()?))
}
pub fn list_body_parse(body: &[u8]) -> Option<Option<&str>> {
    if body.is_empty() {
        return Some(None);
    }
    let ns = core::str::from_utf8(body).ok()?;
    prefs_core::validate_ns(ns).is_ok().then_some(Some(ns))
}

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))] // the `crate::bus` cfg (lib.rs): no bus on this build, no bus fulfiller (merge9 fold)
/// Fulfil one PREF verb. `in_session`: the caller runs in the open session (the transport decides, from
/// the stamped principal). Reply body into `text`; returns the status (0 or a negative errno).
/// GET: the value as a TOML literal, -ENOENT unset. SET: empty, -EACCES outside the session, -EIO when the
/// save failed or is held. LIST: `<ns>.<key> = <literal>` lines, -E2BIG past the 4 KiB body ceiling.
pub fn bus_fulfil(verb: u8, body: &[u8], in_session: bool, text: &mut Vec<u8>) -> i64 {
    ensure_loaded();
    match verb {
        una_abi::BUS_VERB_PREF_GET => match get_body_parse(body) {
            Some((ns, k)) => match get(ns, k) {
                Some(v) => {
                    text.extend_from_slice(v.to_literal().as_bytes());
                    0
                }
                None => ENOENT,
            },
            None => EINVAL,
        },
        una_abi::BUS_VERB_PREF_SET => match set_body_parse(body) {
            Some((ns, k, v)) => {
                if !in_session {
                    serial_println!("[prefs] set {}.{} refused: caller is not the session user", ns, k);
                    EACCES
                } else if set(ns, k, v).is_ok() {
                    0
                } else {
                    EIO
                }
            }
            None => EINVAL,
        },
        una_abi::BUS_VERB_PREF_LIST => match list_body_parse(body) {
            Some(which) => {
                let nss = match which {
                    Some(ns) => alloc::vec![String::from(ns)],
                    None => namespaces(),
                };
                for ns in nss {
                    for (k, v) in list(&ns) {
                        text.extend_from_slice(alloc::format!("{}.{} = {}\n", ns, k, v).as_bytes());
                    }
                }
                if text.len() > crate::bus::BUS_BODY_MAX {
                    text.clear();
                    E2BIG
                } else {
                    0
                }
            }
            None => EINVAL,
        },
        _ => EINVAL,
    }
}

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))] // the `crate::bus` cfg (lib.rs): no bus on this build, no bus fulfiller (merge9 fold)
/// PREFS-CODEC KATs (M3): the three request bodies through the frozen v1 frame — build, `frame_parse`
/// (the verb gate admits them), body-parse — plus one frozen golden and the refusals. Pure, in RAM.
/// Prints `:: PREFS-CODEC: kats=<n> -> PASS ::`.
pub fn codec_selftest() -> bool {
    use crate::bus::{build_request, frame_parse, BUS_HDR_LEN};
    let mut f = [0u8; 128];
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut kat = |ok: bool| {
        total += 1;
        pass += ok as u32;
    };
    // GET: frozen golden — `system.audio.mute`, corr 21.
    const GOLDEN_REQ_PREF_GET: &[u8] = &[
        b'U', b'B', b'S', b'1', 1, 1, 11, 0, 21, 0, 0, 0, 0, 0, 0, 0, //
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
        17, 0, 0, 0, b's', b'y', b's', b't', b'e', b'm', b'.', b'a', b'u', b'd', b'i', b'o', b'.', b'm', b'u', b't', b'e',
    ];
    let n = build_request(una_abi::BUS_VERB_PREF_GET, 21, b"system.audio.mute", &mut f);
    kat(&f[..n] == GOLDEN_REQ_PREF_GET);
    kat(matches!(frame_parse(&f[..n]), Ok(h) if h.verb == una_abi::BUS_VERB_PREF_GET)
        && get_body_parse(&f[BUS_HDR_LEN..n]) == Some(("system", "audio.mute")));
    // SET: `system.display.brightness` NUL `9`.
    let n = build_request(una_abi::BUS_VERB_PREF_SET, 22, b"system.display.brightness\x009", &mut f);
    kat(matches!(frame_parse(&f[..n]), Ok(h) if h.verb == una_abi::BUS_VERB_PREF_SET)
        && set_body_parse(&f[BUS_HDR_LEN..n]) == Some(("system", "display.brightness", PrefValue::Int(9))));
    kat(set_body_parse(b"system.display.wallpaper\x00\"/home/ann/SKY.PNG\"") == Some(("system", "display.wallpaper", PrefValue::Str(String::from("/home/ann/SKY.PNG")))));
    // LIST: empty and one namespace.
    let n = build_request(una_abi::BUS_VERB_PREF_LIST, 23, b"", &mut f);
    kat(matches!(frame_parse(&f[..n]), Ok(h) if h.verb == una_abi::BUS_VERB_PREF_LIST) && list_body_parse(b"") == Some(None));
    kat(list_body_parse(b"system") == Some(Some("system")));
    // Refusals: no dot, bad segment, no NUL, a value outside the subset, a bad namespace.
    kat(get_body_parse(b"system") .is_none() && get_body_parse(b"system.a..b").is_none() && get_body_parse(b"sys tem.a").is_none());
    kat(set_body_parse(b"system.a=1").is_none() && set_body_parse(b"system.a\x00[1]").is_none() && set_body_parse(b"system.a\x001 2").is_none());
    kat(list_body_parse(b"a.b").is_none());
    let ok = pass == total;
    serial_println!(":: PREFS-CODEC: kats={}/{} -> {} ::", pass, total, if ok { "PASS" } else { "FAIL" });
    ok
}

// ── The fixture (M4) ─────────────────────────────────────────────────────────────────────────

/// `tests prefs`: set/get each of the four types, save, re-read the FILE through the VFS, parse it with
/// prefs_core and compare; then a malformed file is refused (no panic) and the defaults hold, and the
/// operator's own file is restored byte for byte. Prints `:: PREFS-FIXTURE: ... -> PASS ::`.
#[cfg(feature = "witness")]
pub fn selftest() {
    ensure_loaded();
    #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))] let codec = codec_selftest();
    #[cfg(not(any(feature = "aarch64_el0", target_arch = "x86_64")))] let codec = true; // no bus on this build: the codec leg is vacuous, not failed
    let p = path();
    let original = read_all(&p);
    let tree0 = TREE.lock().clone();
    let held0 = HELD.load(Ordering::Acquire);
    HELD.store(false, Ordering::Release);
    // Leg 1 — the four types, through set (which saves by the swap).
    let vals = [
        ("fixture.s", PrefValue::Str(String::from("a \"quoted\" path\\x"))),
        ("fixture.i", PrefValue::Int(-42)),
        ("fixture.f", PrefValue::Float(2.5)),
        ("fixture.b", PrefValue::Bool(true)),
    ];
    let mut set_ok = true;
    for (k, v) in vals.iter() {
        set_ok &= set("fixturens", k, v.clone()).is_ok() && get("fixturens", k).as_ref() == Some(v);
    }
    // Leg 2 — the file on the volume IS the tree.
    let file = read_all(&p);
    let reread = file.as_deref().and_then(|b| core::str::from_utf8(b).ok()).map(PrefTree::parse);
    let file_ok = match &reread {
        Some(Ok(t)) => vals.iter().all(|(k, v)| t.get("fixturens", k) == Some(v)) && *t == *TREE.lock(),
        _ => false,
    };
    let no_temp = read_all(&alloc::format!("{}.new", p)).is_none();
    // Leg 3 — a malformed file: refused with its line, nothing adopted, defaults hold, saves held.
    let bad = b"[system]\ndisplay.brightness = 3\nrecents = [\"a\"]\n";
    let _ = write_all(&p, bad);
    *LOADED_FOR.lock() = None;
    ensure_loaded();
    let refused = HELD.load(Ordering::Acquire) && TREE.lock().is_empty() && int(key::BRIGHTNESS, 0, 16).is_none();
    let held = save().is_err() && read_all(&p).as_deref() == Some(&bad[..]);
    // Restore the operator's file and tree exactly.
    let mt = crate::shell::vfs_mount_table();
    match &original {
        Some(b) => {
            let _ = write_all(&p, b);
        }
        None => {
            let _ = mt.unlink(&p, crate::fs::vfs::KERNEL_PRINCIPAL);
        }
    }
    *TREE.lock() = tree0;
    HELD.store(held0, Ordering::Release);
    let restored = read_all(&p) == original;
    let ok = codec && set_ok && file_ok && no_temp && refused && held && restored;
    serial_println!(
        ":: PREFS-FIXTURE: codec={} types={} file={} temp_gone={} malformed_refused={} save_held={} restored={} -> {} ::",
        codec as u8, set_ok as u8, file_ok as u8, no_temp as u8, refused as u8, held as u8, restored as u8, if ok { "PASS" } else { "FAIL" }
    );
}
