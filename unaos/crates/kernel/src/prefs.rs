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
//! # SETTINGSFILES (B407, R98) — the store is a FOLDER of files, one per domain
//!
//! Since R98 the ONE store above lives as `<home>/settings/<domain>` files (`display`, `login`, `desktop`,
//! `sound`, `trackpad`, `general`, and `<program>` for `app.<program>.*` / a program's own namespace) —
//! `prefs_core::files::domain_of` is the rule. The tree, the keys, the schema and the bus are unchanged; a
//! write rewrites ONLY its domain's file (the swap below, per file), human-readable (`# auto-saved <ISO> by
//! <who>`, the schema's description above each key). The old `preferences.toml` is migrated once and
//! deleted (`[prefs] migrated preferences.toml -> settings/<n> files`); a file the user deletes resets its
//! domain at the next service read (`[prefs] settings/<d> absent -> defaults (deleted by the user)`). The
//! paragraphs below that say "the file" mean each domain file.
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
//! The ranges are the schema's (`prefs_core::schema::SCHEMA`, docs/dev/PREFS-SCHEMA.md; PREFSKERNEL B345:
//! every set clamps through `schema::check`, every load through `schema::clamp_tree`).
//! `display.brightness` (1..16) · `display.idle_min` (minutes, 0 = never) · `display.wallpaper` (a path,
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
    /// KERNELFONT (B359) schema rows, written by the Settings Font picker since KERNELFONT2 (B363).
    pub const FONT: &str = "display.font";
    pub const FONT_SIZE: &str = "display.font_size";
    /// PREFSUI (B389): R91's login items and R93's Resolution dropdown.
    pub const LOGIN_ITEMS: &str = "login.items";
    pub const DISPLAY_MODE: &str = "display.mode";
    /// DOCK2 (B394): the dock's edge and auto-hide.
    pub const DOCK_POSITION: &str = "dock.position";
    pub const DOCK_AUTOHIDE: &str = "dock.autohide";
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

/// SETTINGSFILES (B407, R98): `<home>/settings` — the folder of domain files (`settings/display`, …), or
/// `/settings` with no session. The store IS this folder; there is no other file.
pub fn path() -> String {
    alloc::format!("{}/{}", base(), prefs_core::files::DIR)
}

/// `<home>/settings/<domain>`.
pub fn domain_path(d: &str) -> String {
    alloc::format!("{}/{}", path(), d)
}

/// The single file before R98 (Principia's path), migrated ONCE into the domain files and deleted.
pub fn legacy_path() -> String {
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

/// `<home>/settings` — a plain folder in the home (R98: no link, no dot).
fn ensure_dirs() {
    use crate::fs::vfs::NodeKind;
    let mt = crate::shell::vfs_mount_table();
    let d = path();
    if mt.stat(&d).is_err() {
        let _ = mt.create(&d, NodeKind::Dir, crate::fs::vfs::KERNEL_PRINCIPAL);
    }
}

// ── SETTINGSFILES state ─────────────────────────────────────────────────────────────────────────

/// Domains whose file refused at load: their saves are HELD so the user's file is never overwritten.
static HELD_D: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());
/// Domains whose file is on the volume (at the last load / save) — the reset-on-delete watch list.
static ON_DISK: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());
/// The domain the write in flight saves (set by [`set_applied`], read by `KernelPersist::save`).
static PENDING: spin::Mutex<Option<String>> = spin::Mutex::new(None);
/// Who made the write in flight when it was not the domain's own pane (the `pref` verb, the fixture).
static BY: spin::Mutex<Option<&'static str>> = spin::Mutex::new(None);
/// One save (or the delete watch) at a time: the swap's unlink window must not read as a user delete.
static SAVE_LOCK: spin::Mutex<()> = spin::Mutex::new(());
/// The programs' declared stanzas (`PrefDeclare`, `app.<name>.*`).
static DECLARED: spin::Mutex<prefs_core::declare::Registry> = spin::Mutex::new(prefs_core::declare::Registry::new());
/// The old single file was migrated at this boot.
static MIGRATED: AtomicBool = AtomicBool::new(false);
static WATCH_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The delete watch runs at most this often (a service-pass read, never a timer).
const WATCH_EVERY_MS: u64 = 2000;

fn held(d: &str) -> bool {
    HELD_D.lock().iter().any(|h| h == d)
}

fn on_disk_add(d: &str) {
    let mut g = ON_DISK.lock();
    if !g.iter().any(|x| x == d) {
        g.push(String::from(d));
    }
}

/// The writer named in the file's `# auto-saved … by <who>` line: the explicit one, else the pane that owns
/// the domain (`system.*`), else the program (`app.<name>`, `vein`).
fn by_of(d: &str) -> String {
    if let Some(b) = *BY.lock() {
        return String::from(b);
    }
    if prefs_core::files::SYSTEM_DOMAINS.contains(&d) {
        String::from(prefs_core::files::pane_of(d))
    } else {
        alloc::format!("the program {}", d)
    }
}

fn now_iso() -> String {
    let mut b = [0u8; 32];
    match crate::clock::iso8601_now(&mut b) {
        Some(n) => String::from(core::str::from_utf8(&b[..n]).unwrap_or("?")),
        None => alloc::format!("(clock unsynced, uptime {} ms)", crate::arch::ms()),
    }
}

/// The comment above a key: the schema's `doc`, or the program's declared one.
fn doc_of(ns: &str, k: &str) -> Option<String> {
    if ns == prefs_core::files::APP_NS {
        return prefs_core::declare::lookup(&DECLARED.lock(), k).map(|d| d.doc.clone()).filter(|d| !d.is_empty());
    }
    prefs_core::files::schema_doc(ns, k)
}

// ── Load ─────────────────────────────────────────────────────────────────────────────────────

/// TESTFIX3: set by `tests prefs` around its DELIBERATE malformed reload, so that expected refusal is said
/// as a `[prefs]` line and not as a `:: PREFS: … -> FAIL` the scorer counts (FLIGHT 19 counted it).
static FIXTURE_QUIET: AtomicBool = AtomicBool::new(false);

fn witness(ok: bool) {
    if FIXTURE_QUIET.load(Ordering::Acquire) {
        serial_println!("[prefs] fixture malformed reload loaded={} held={} (expected refusal)", LOADED_N.load(Ordering::Relaxed), !ok as u8);
        return;
    }
    serial_println!(
        ":: PREFS: path={} domains={} loaded={} saved={} ns={} -> {} ::",
        path(), ON_DISK.lock().len(), LOADED_N.load(Ordering::Relaxed), SAVED_N.load(Ordering::Relaxed), NS, if ok { "PASS" } else { "FAIL" }
    );
}

/// Parse `text`; a refusal is said with its line.
fn parse_said(p: &str, text: &[u8]) -> Result<PrefTree, prefs_core::ParseError> {
    let s = core::str::from_utf8(text).map_err(|_| prefs_core::ParseError { line: 1, why: "not UTF-8" })?;
    PrefTree::parse(s).inspect_err(|e| serial_println!("[prefs] refused path={} line={} why={}", p, e.line, e.why))
}

/// The domains in `<home>/settings` (a `<d>.new` alone counts: its swap was interrupted).
fn domains_on_volume() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Ok(ents) = crate::shell::vfs_mount_table().read_dir(&path()) {
        for e in ents {
            if !matches!(e.kind, crate::fs::vfs::NodeKind::File) {
                continue;
            }
            let d = e.name.strip_suffix(".new").unwrap_or(&e.name);
            if prefs_core::files::valid_domain(d) && !v.iter().any(|x| x == d) {
                v.push(String::from(d));
            }
        }
    }
    v
}

/// Read one domain file (or its orphaned `.new`): `Ok(None)` = absent, `Err` = refused.
fn read_domain(d: &str) -> Result<Option<PrefTree>, ()> {
    let p = domain_path(d);
    let tmp = alloc::format!("{}.new", p);
    match read_all(&p) {
        Some(t) => parse_said(&p, &t).map(Some).map_err(|_| ()),
        None => match read_all(&tmp).map(|t| parse_said(&tmp, &t)) {
            Some(Ok(tr)) => {
                serial_println!("[prefs] adopted {} (the swap was interrupted)", tmp);
                Ok(Some(tr))
            }
            _ => Ok(None),
        },
    }
}

/// Read every domain file into the tree (or migrate the old single file, once), then the private-store import.
fn load() {
    let mut tree = PrefTree::new();
    let (mut held_v, mut disk) = (Vec::new(), Vec::new());
    for d in domains_on_volume() {
        match read_domain(&d) {
            Ok(Some(t)) => {
                prefs_core::files::merge(&mut tree, &t);
                disk.push(d);
            }
            Ok(None) => {}
            Err(()) => held_v.push(d),
        }
    }
    // R98: the old single file, migrated ONCE — only when no domain file exists yet.
    let mut migrate_legacy = false;
    if disk.is_empty() && held_v.is_empty() {
        let lp = legacy_path();
        if let Some(t) = read_all(&lp) {
            match parse_said(&lp, &t) {
                Ok(tr) => {
                    tree = tr;
                    migrate_legacy = true;
                }
                Err(_) => serial_println!("[prefs] {} refused: not migrated, left in place (R98)", lp),
            }
        }
    }
    let ok = held_v.is_empty();
    for d in &held_v {
        serial_println!("[prefs] settings/{} refused: its saves are held; fix or delete it", d);
    }
    // PREFSKERNEL M3 (B345): a value outside its schema range (a hand edit, a file older than the
    // schema) is clamped ONCE, here, by the shared rule — and re-saved, so the file holds the clamp.
    let clamped = prefs_core::schema::clamp_tree(&mut tree);
    LOAD_CLAMPED.store(clamped as u32, Ordering::Relaxed);
    serial_println!("[prefs] load clamped={}", clamped);
    HELD.store(!ok, Ordering::Release);
    *HELD_D.lock() = held_v;
    *ON_DISK.lock() = disk;
    LOADED_N.store(tree.len() as u32, Ordering::Relaxed);
    SAVED_N.store(0, Ordering::Relaxed);
    *TREE.lock() = tree;
    witness(ok);
    if migrate_legacy {
        let saved = save();
        let n = ON_DISK.lock().len();
        if saved.is_ok() {
            let mt = crate::shell::vfs_mount_table();
            let lp = legacy_path();
            let _ = mt.unlink(&lp, crate::fs::vfs::KERNEL_PRINCIPAL);
            let _ = mt.unlink(&alloc::format!("{}.new", lp), crate::fs::vfs::KERNEL_PRINCIPAL);
            MIGRATED.store(true, Ordering::Release);
            serial_println!("[prefs] migrated preferences.toml -> settings/<n> files n={} (R98)", n);
        } else {
            serial_println!("[prefs] migrate preferences.toml -> settings/ failed: {:?}; the old file stays (R98)", saved.err());
        }
    } else if clamped > 0 {
        let _ = save();
    }
    migrate();
}

/// PREFSKERNEL M3: values the last load clamped into their schema range.
static LOAD_CLAMPED: AtomicU32 = AtomicU32::new(0);

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
/// click paths — and the SETTINGSFILES delete watch.
pub fn service() {
    ensure_loaded();
    let now = crate::arch::ms();
    if now.saturating_sub(WATCH_MS.load(Ordering::Relaxed)) >= WATCH_EVERY_MS {
        WATCH_MS.store(now, Ordering::Relaxed);
        let _ = watch_deleted();
    }
}

/// R98: a domain file the user deleted resets that domain to the schema defaults (the store never holds a
/// default, so the keys simply leave the tree and every reader answers its default). Returns the domains reset.
fn watch_deleted() -> Vec<String> {
    let Some(_g) = SAVE_LOCK.try_lock() else { return Vec::new() };
    let mt = crate::shell::vfs_mount_table();
    let gone: Vec<String> = ON_DISK.lock().iter().filter(|d| mt.stat(&domain_path(d)).is_err() && mt.stat(&alloc::format!("{}.new", domain_path(d))).is_err()).cloned().collect();
    for d in &gone {
        let keys = prefs_core::files::clear(&mut TREE.lock(), d);
        ON_DISK.lock().retain(|x| x != d);
        serial_println!("[prefs] settings/{} absent -> defaults (deleted by the user) keys={}", d, keys.len());
    }
    gone
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

/// Save EVERY domain (the load-time clamp, the migrations, the fixture). `Ok(keys written)`.
pub fn save() -> Result<usize, String> {
    let mut ds: Vec<String> = prefs_core::files::split(&TREE.lock()).into_keys().collect();
    for d in ON_DISK.lock().iter() {
        if !ds.contains(d) {
            ds.push(d.clone());
        }
    }
    let mut n = 0usize;
    let mut first_err = None;
    for d in &ds {
        match save_domain(d) {
            Ok(k) => n += k,
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(n),
    }
}

/// R98: write ONE domain's file by the swap (`<d>.new`, read back, parse = the domain's tree, rename) —
/// human-readable, the auto-saved line on top and the schema's description above each key. `Ok(keys)`.
pub fn save_domain(d: &str) -> Result<usize, String> {
    if held(d) {
        serial_println!("[prefs] save held: settings/{} was refused at load; fix or delete it", d);
        return Err(alloc::format!("held: settings/{} was refused at load", d));
    }
    let _g = SAVE_LOCK.lock();
    let want = prefs_core::files::part(&TREE.lock(), d);
    let by = by_of(d);
    let text = prefs_core::files::render(d, &want, &now_iso(), &by, &doc_of);
    let n = want.len();
    let p = domain_path(d);
    let tmp = alloc::format!("{}.new", p);
    ensure_dirs();
    write_all(&tmp, text.as_bytes())?;
    // Read back and parse: the temp must BE the domain's tree before the real file is touched.
    let back = read_all(&tmp).ok_or_else(|| String::from("read-back failed"))?;
    if back != text.as_bytes() {
        return Err(String::from("read-back differs"));
    }
    match core::str::from_utf8(&back).ok().map(PrefTree::parse) {
        Some(Ok(t)) if t == want || t.to_toml() == want.to_toml() => {} // re-emit equality covers a NaN value
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
    on_disk_add(d);
    SAVED_N.store(n as u32, Ordering::Relaxed);
    serial_println!("[prefs] saved settings/{} keys={} by={}", d, n, by);
    witness(true);
    Ok(n)
}

/// A copy of the in-memory tree.
pub fn snapshot() -> PrefTree {
    TREE.lock().clone()
}

/// The files on the volume, read through the VFS, parsed with prefs_core and merged: `None` = no file,
/// `Some(Err)` = a file refused.
pub fn read_file() -> Option<Result<PrefTree, prefs_core::ParseError>> {
    let mut t = PrefTree::new();
    let mut any = false;
    for d in domains_on_volume() {
        let Some(b) = read_all(&domain_path(&d)) else { continue };
        any = true;
        match core::str::from_utf8(&b).map(PrefTree::parse) {
            Ok(Ok(x)) => prefs_core::files::merge(&mut t, &x),
            Ok(Err(e)) => return Some(Err(e)),
            Err(_) => return Some(Err(prefs_core::ParseError { line: 1, why: "not UTF-8" })),
        }
    }
    any.then_some(Ok(t))
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

/// Why a [`set_applied`] failed.
#[derive(Clone, Debug, PartialEq)]
pub enum SetError {
    /// The schema refused the value (`prefs_core::schema::check`: wrong type, enum, length, NaN).
    Refused(prefs_core::schema::Refusal),
    /// A malformed name or a leaf collision.
    Name(prefs_core::PrefError),
    /// The save failed or is held; the tree was rolled back.
    Io(String),
}

impl core::fmt::Display for SetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SetError::Refused(r) => write!(f, "{}", r),
            SetError::Name(e) => f.write_str(e.as_str()),
            SetError::Io(e) => f.write_str(e),
        }
    }
}

/// PREFSKERNEL (B345): the kernel store as `prefs_core::wire::Persist` — the lock is `TREE`, held for the
/// closure only (the save runs unlocked, as before); the save is [`save`]'s swap. Its error text is kept
/// for the `[prefs] set` line.
struct KernelPersist {
    err: Option<String>,
}

impl prefs_core::wire::Persist for KernelPersist {
    fn with_tree<R>(&mut self, f: impl FnOnce(&mut PrefTree) -> R) -> R {
        f(&mut TREE.lock())
    }
    fn save(&mut self) -> Result<(), ()> {
        // SETTINGSFILES (R98): a key's write rewrites ONLY its domain's file.
        let r = match PENDING.lock().take() {
            Some(d) => save_domain(&d),
            None => save(),
        };
        r.map(|_| ()).map_err(|e| self.err = Some(e))
    }
}

/// Set `ns`/`key` and persist — PRINCIPIA'S RULE, not a kernel copy of it (PREFSKERNEL, B345): this is
/// `prefs_core::wire::persisted_set`, the function the host reference store runs: the name is validated,
/// `prefs_core::schema::check` clamps (the CLAMP is stored and answered: `Applied { value, clamped }`) or
/// refuses, an unchanged value is not re-written, and a failed save rolls the tree back (cache and file
/// never disagree). Prints the set line, and on a change the PrefChanged (which carries `clamped`).
pub fn set_applied(ns: &str, k: &str, v: PrefValue) -> Result<prefs_core::schema::Applied, SetError> {
    ensure_loaded();
    // SETTINGSFILES (B407): `app.<name>.<key>` is checked by the program's declared stanza (its clamp kept).
    let mut pre_clamp = false;
    let v = if ns == prefs_core::files::APP_NS {
        match prefs_core::declare::check(&DECLARED.lock(), k, v.clone()) {
            Ok(a) => {
                pre_clamp = a.clamped;
                a.value
            }
            Err(x) => {
                serial_println!("[prefs] set {}.{}={} ok=0 why={:?} (declared stanza)", ns, k, v, x);
                return Err(SetError::Refused(x));
            }
        }
    } else {
        v
    };
    *PENDING.lock() = Some(prefs_core::files::domain_of(ns, k));
    let mut kp = KernelPersist { err: None };
    let mut r = prefs_core::wire::persisted_set(&mut kp, ns, k, v.clone());
    *PENDING.lock() = None;
    if let Ok(st) = r.as_mut() {
        st.applied.clamped |= pre_clamp;
    }
    match &r {
        Ok(st) if st.applied.clamped => serial_println!("[prefs] set {}.{}={} ok=1 clamped=1 sent={}", ns, k, st.applied.value, v),
        Ok(st) => serial_println!("[prefs] set {}.{}={} ok=1", ns, k, st.applied.value),
        Err(e) => serial_println!("[prefs] set {}.{}={} ok=0 why={:?}", ns, k, v, e),
    }
    match r {
        Ok(st) => {
            if st.changed {
                changed(ns, k, &st.applied);
            }
            Ok(st.applied)
        }
        Err(prefs_core::wire::SetFail::Refused(x)) => Err(SetError::Refused(x)),
        Err(prefs_core::wire::SetFail::Name(e)) => Err(SetError::Name(e)),
        Err(prefs_core::wire::SetFail::Io) => Err(SetError::Io(kp.err.unwrap_or_else(|| String::from("save failed")))),
    }
}

/// [`set_applied`] for callers that only need accepted / refused (the refusal as text).
pub fn set(ns: &str, k: &str, v: PrefValue) -> Result<(), String> {
    set_applied(ns, k, v).map(|_| ()).map_err(|e| alloc::format!("{}", e))
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
///         [NUL "clamped=true"]   (PREFSKERNEL: when the schema clamped the write; wire::changed_body_applied)
/// ```
///
/// (the PREF_SET request body, unsolicited). Also the ATTRSURF hook: the key is mirrored as an attribute
/// on the preferences file once the VFS can set one ([`mirror_attr`]).
fn changed(ns: &str, k: &str, a: &prefs_core::schema::Applied) {
    let v = &a.value;
    serial_println!("[prefs] changed {}.{}", ns, k); #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))] crate::prefs_client::on_changed(ns, k, a); // SETTINGSBUS (B337): the verb-19 frame to the kernel's subscribers. PREFSKERNEL (B345): it carries `clamped`.
    mirror_attr(ns, k, v);
}

/// ATTRSURF hook: stamp `system.*` keys as attributes on the preferences file (so `query` finds them on
/// UnaFS). The VFS has `remove_attr` only — `set_attr` is ATTRSURF's, not in this tree. OWED ATTRSURF:
/// when it lands this body becomes `mt.set_attr(&path(), &alloc::format!("{}.{}", ns, k), v)`.
fn mirror_attr(ns: &str, k: &str, v: &PrefValue) {
    let _ = (ns, k, v);
}

/// Split `ns.key` (the verb's and the bus's address form) and validate both halves — the shared
/// `prefs_core::wire::split_addr` (PREFSKERNEL, B345).
pub fn split_addr(a: &str) -> Option<(&str, &str)> {
    prefs_core::wire::split_addr(a)
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
                *BY.lock() = Some("the pref verb");
                let r = set_applied(ns, k, PrefValue::infer(&raw));
                *BY.lock() = None;
                match r {
                    Ok(ap) if ap.clamped => out(&alloc::format!("{} = {} (clamped to the schema's range)", a, ap.value.to_literal())),
                    Ok(_) => out(&alloc::format!("{} = {}", a, get(ns, k).map(|v| v.to_literal()).unwrap_or_default())),
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

// PREFSKERNEL (B345): the bodies, the statuses and the fulfiller are `prefs_core::wire`'s — ONE
// implementation for both rings (Principia on the host answers through the same `fulfil`). What stays
// here is the kernel's `Store`: the tree behind `set_applied` (= `wire::persisted_set` + the swap).

/// PREF_GET body: `<ns>.<key>`. Pure; `None` = BadBody (-EINVAL). (`prefs_core::wire::parse_get`.)
pub fn get_body_parse(body: &[u8]) -> Option<(&str, &str)> {
    prefs_core::wire::parse_get(body)
}
/// PREF_SET body: `<ns>.<key>` NUL `<TOML scalar literal>`. (`prefs_core::wire::parse_set`.)
pub fn set_body_parse(body: &[u8]) -> Option<(&str, &str, PrefValue)> {
    prefs_core::wire::parse_set(body)
}
/// PREF_LIST body: `<ns>` or empty (every namespace). (`prefs_core::wire::parse_list`.)
pub fn list_body_parse(body: &[u8]) -> Option<Option<&str>> {
    prefs_core::wire::parse_list(body)
}

/// The kernel's store as the shared wire sees it: reads are the tree, a write is [`set_applied`].
#[allow(dead_code)] // a no-bus, no-witness build has no caller
struct KernelStore;

impl prefs_core::wire::Store for KernelStore {
    fn get(&self, ns: &str, k: &str) -> Option<PrefValue> {
        get(ns, k)
    }
    fn set(&mut self, ns: &str, k: &str, v: PrefValue) -> Result<prefs_core::schema::Applied, prefs_core::wire::SetFail> {
        set_applied(ns, k, v).map_err(|e| match e {
            SetError::Refused(r) => prefs_core::wire::SetFail::Refused(r),
            SetError::Name(n) => prefs_core::wire::SetFail::Name(n),
            SetError::Io(_) => prefs_core::wire::SetFail::Io,
        })
    }
    fn list(&self, ns: &str) -> Vec<(String, PrefValue)> {
        list(ns)
    }
    fn namespaces(&self) -> Vec<String> {
        namespaces()
    }
    /// SETTINGSFILES (B407): PrefDeclare — the program's stanza is held for its `app.<name>.*` writes and
    /// its descriptions head the keys in `settings/<name>`.
    fn declare(&mut self, name: &str, keys: Vec<prefs_core::declare::DeclKey>) -> i64 {
        let n = keys.len();
        DECLARED.lock().insert(String::from(name), keys);
        serial_println!("[prefs] declared app.{} keys={} -> settings/{} (R98)", name, n, name);
        0
    }
}

// The shared wire's numbers are the kernel bus's (prefs_core's dev-test pins them to una-abi too).
#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
const _: () = assert!(
    prefs_core::wire::BODY_MAX == crate::bus::BUS_BODY_MAX
        && prefs_core::wire::VERB_GET == una_abi::BUS_VERB_PREF_GET
        && prefs_core::wire::VERB_SET == una_abi::BUS_VERB_PREF_SET
        && prefs_core::wire::VERB_LIST == una_abi::BUS_VERB_PREF_LIST
        && prefs_core::wire::VERB_CHANGED == una_abi::BUS_VERB_PREF_CHANGED
);

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))] // the `crate::bus` cfg (lib.rs): no bus on this build, no bus fulfiller (merge9 fold)
/// Fulfil one PREF verb — PREFSKERNEL (B345): a thin call into `prefs_core::wire::fulfil` over the kernel
/// store, the function Principia answers with on the host. `in_session`: the caller runs in the open
/// session (the transport decides, from the stamped principal). Reply body into `text`; returns the status.
/// GET: the literal, -ENOENT unset. SET: empty, or `<stored>` NUL `clamped=true` when the schema clamped;
/// -EACCES outside the session; -EINVAL malformed or refused by the schema; -EIO collision / save failed
/// or held. LIST: `<ns>.<key> = <literal>` lines, -E2BIG past the 4 KiB body ceiling.
pub fn bus_fulfil(verb: u8, body: &[u8], in_session: bool, text: &mut Vec<u8>) -> i64 {
    ensure_loaded();
    if (verb == prefs_core::wire::VERB_SET || verb == prefs_core::wire::VERB_DECLARE) && !in_session {
        if let Some((ns, k, _)) = prefs_core::wire::parse_set(body) {
            serial_println!("[prefs] set {}.{} refused: caller is not the session user", ns, k);
        }
    }
    prefs_core::wire::fulfil(&mut KernelStore, verb, body, in_session, text)
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
        // TESTFIX3: verb byte 16 = PREF_GET since the merge9 fold moved PREFS to 16..=18 (11 is ATTR_SET now);
        // the stale 11 was the one KAT short on the metal (`kats=8/9`).
        b'U', b'B', b'S', b'1', 1, 1, 16, 0, 21, 0, 0, 0, 0, 0, 0, 0, //
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
    // SETTINGSFILES (R98): the legs below run on the `general` domain file (power.lowbat_shutdown_pct's)
    // and the fixture's own `settings/fixturens`; every other domain file is untouched.
    let p = domain_path("general");
    let fp = domain_path("fixturens");
    let original = read_all(&p);
    let (held_d0, on_disk0) = (HELD_D.lock().clone(), ON_DISK.lock().clone());
    *BY.lock() = Some("tests prefs");
    let tree0 = TREE.lock().clone();
    let load_clamped0 = LOAD_CLAMPED.load(Ordering::Relaxed);
    let held0 = HELD.load(Ordering::Acquire);
    let (loaded0, saved0) = (LOADED_N.load(Ordering::Relaxed), SAVED_N.load(Ordering::Relaxed));
    HELD.store(false, Ordering::Release);
    HELD_D.lock().clear();
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
    let file = read_all(&fp);
    let reread = file.as_deref().and_then(|b| core::str::from_utf8(b).ok()).map(PrefTree::parse);
    let file_ok = match &reread {
        Some(Ok(t)) => vals.iter().all(|(k, v)| t.get("fixturens", k) == Some(v)) && *t == prefs_core::files::part(&TREE.lock(), "fixturens"),
        _ => false,
    };
    let no_temp = read_all(&alloc::format!("{}.new", fp)).is_none();
    // PREFSKERNEL (B345) — Leg C, the set clamp: an out-of-range write stores the schema's clamp and says
    // so. The key is `power.lowbat_shutdown_pct` (0..=100): read by the power check only, so the fixture
    // dims no panel and moves no window.
    let lk = key::LOWBAT_PCT;
    let clamp_ok = matches!(set_applied(NS, lk, PrefValue::Int(250)), Ok(ref a) if a.clamped && a.value == PrefValue::Int(100))
        && get(NS, lk) == Some(PrefValue::Int(100))
        && matches!(set_applied(NS, lk, PrefValue::Int(40)), Ok(ref a) if !a.clamped)
        && set_applied(NS, lk, PrefValue::Str(String::from("loud"))).is_err()
        && get(NS, lk) == Some(PrefValue::Int(40));
    // Leg W, the wire is the shared one: the kernel's fulfiller and prefs_core's reference store answer the
    // same clamped SET with the same status and the same bytes (`100` NUL `clamped=true`).
    let wire_body: &[u8] = b"system.power.lowbat_shutdown_pct\x00250";
    let mut kt = Vec::new();
    #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))] let ks = bus_fulfil(prefs_core::wire::VERB_SET, wire_body, true, &mut kt);
    #[cfg(not(any(feature = "aarch64_el0", target_arch = "x86_64")))] let ks = prefs_core::wire::fulfil(&mut KernelStore, prefs_core::wire::VERB_SET, wire_body, true, &mut kt);
    let mut rt = Vec::new();
    let rs = prefs_core::wire::fulfil(&mut prefs_core::wire::TreeStore::default(), prefs_core::wire::VERB_SET, wire_body, true, &mut rt);
    let wire_ok = ks == 0 && ks == rs && kt == rt && kt == b"100\x00clamped=true";
    // Leg L, the load clamp: a file holding an out-of-range value loads clamped, once, and is re-saved so.
    let _ = write_all(&p, b"[system]\npower.lowbat_shutdown_pct = 250\n");
    *LOADED_FOR.lock() = None;
    ensure_loaded();
    let refile = read_all(&p).and_then(|b| String::from_utf8(b).ok()).and_then(|t| PrefTree::parse(&t).ok());
    let loadclamp_ok = LOAD_CLAMPED.load(Ordering::Relaxed) == 1
        && get(NS, lk) == Some(PrefValue::Int(100))
        && refile.as_ref().and_then(|t| t.get(NS, lk).cloned()) == Some(PrefValue::Int(100));
    // Leg 3 — a malformed file: refused with its line, nothing adopted, defaults hold, saves held.
    let bad = b"[system]\ndisplay.brightness = 3\nrecents = [\"a\"]\n";
    let _ = write_all(&p, bad);
    *LOADED_FOR.lock() = None;
    FIXTURE_QUIET.store(true, Ordering::Release);
    ensure_loaded();
    FIXTURE_QUIET.store(false, Ordering::Release);
    let refused = HELD.load(Ordering::Acquire) && held("general") && get(NS, lk).is_none();
    let held = save_domain("general").is_err() && read_all(&p).as_deref() == Some(&bad[..]);
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
    let _ = mt.unlink(&fp, crate::fs::vfs::KERNEL_PRINCIPAL);
    *TREE.lock() = tree0;
    *HELD_D.lock() = held_d0;
    *ON_DISK.lock() = on_disk0;
    *BY.lock() = None;
    HELD.store(held0, Ordering::Release);
    LOADED_N.store(loaded0, Ordering::Relaxed);
    LOAD_CLAMPED.store(load_clamped0, Ordering::Relaxed);
    SAVED_N.store(saved0, Ordering::Relaxed);
    let restored = read_all(&p) == original && read_all(&fp).is_none();
    let rows = prefs_core::schema::SCHEMA.len();
    let ok = codec && set_ok && file_ok && no_temp && refused && held && restored && loadclamp_ok && clamp_ok && wire_ok;
    serial_println!(
        ":: PREFS-FIXTURE: codec={} types={} file={} temp_gone={} malformed_refused={} save_held={} restored={} loadclamp={} clamp={} wire={} schema_rows={} -> {} ::",
        codec as u8, set_ok as u8, file_ok as u8, no_temp as u8, refused as u8, held as u8, restored as u8,
        loadclamp_ok as u8, clamp_ok as u8, if wire_ok { "shared" } else { "diverged" }, rows, if ok { "PASS" } else { "FAIL" }
    );
    settingsfiles_selftest();
}

/// SETTINGSFILES (B407, R98) — `tests prefs`' second line (never at boot, R80): a program declares a stanza
/// over the wire (`PrefDeclare`), its out-of-range write is clamped by the DECLARED range and lands in
/// `settings/<name>` alone; the file is human-readable (the auto-saved line, the declared description above
/// the key) and parses back to the domain; deleting it resets the domain at the next watch. Then everything
/// the leg made is removed. `:: SETTINGSFILES: domains=<n> files=<n> migrated=<0/1> readable=1
/// reset_on_delete=ok app_ns=ok -> PASS ::`.
#[cfg(feature = "witness")]
fn settingsfiles_selftest() {
    const NAME: &str = "sftest";
    const DOC: &str = "the fixture's level (SETTINGSFILES)";
    let dp = domain_path(NAME);
    let mt = crate::shell::vfs_mount_table();
    let others: Vec<(String, Option<Vec<u8>>)> = ON_DISK.lock().iter().map(|d| (d.clone(), read_all(&domain_path(d)))).collect();
    // The stanza as a program sends it (`tools/prefs-schema-check.py` R12 reads the literal as the declaration).
    const STANZA: &[u8] = b"sftest\0level\tint:0:10\t5\tthe fixture's level (SETTINGSFILES)\n";
    let body = prefs_core::declare::parse(STANZA).map(|(n, k)| prefs_core::declare::body(&n, &k)).unwrap_or_default();
    let mut out = Vec::new();
    let declared = prefs_core::wire::fulfil(&mut KernelStore, prefs_core::wire::VERB_DECLARE, &body, true, &mut out) == 0
        && prefs_core::wire::fulfil(&mut KernelStore, prefs_core::wire::VERB_DECLARE, &body, false, &mut out) == prefs_core::wire::EACCES;
    let set = set_applied(prefs_core::files::APP_NS, "sftest.level", PrefValue::Int(99));
    let app_ok = declared
        && matches!(set, Ok(ref a) if a.clamped && a.value == PrefValue::Int(10))
        && set_applied(prefs_core::files::APP_NS, "sftest.level", PrefValue::Bool(true)).is_err();
    let text = read_all(&dp).and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
    let parsed = PrefTree::parse(&text).ok();
    let readable = prefs_core::files::stamp_of(&text).is_some()
        && text.contains(&alloc::format!("# {}\nsftest.level = 10\n", DOC))
        && parsed.as_ref().and_then(|t| t.get(prefs_core::files::APP_NS, "sftest.level").cloned()) == Some(PrefValue::Int(10))
        && parsed.as_ref().map(|t| t.namespaces().len()) == Some(1);
    // Only `settings/sftest` changed: every other domain file is byte-for-byte what it was.
    let alone = others.iter().all(|(d, b)| read_all(&domain_path(d)) == *b);
    let _ = mt.unlink(&dp, crate::fs::vfs::KERNEL_PRINCIPAL);
    WATCH_MS.store(0, Ordering::Relaxed);
    let reset = watch_deleted().iter().any(|d| d == NAME) && get(prefs_core::files::APP_NS, "sftest.level").is_none();
    DECLARED.lock().remove(NAME);
    let files = domains_on_volume().len();
    let domains = ON_DISK.lock().len();
    let ok = app_ok && readable && alone && reset && read_all(&dp).is_none();
    serial_println!(
        ":: SETTINGSFILES: domains={} files={} migrated={} readable={} reset_on_delete={} app_ns={} alone={} dir={} -> {} ::",
        domains, files, MIGRATED.load(Ordering::Acquire) as u8, readable as u8, if reset { "ok" } else { "fail" },
        if app_ok { "ok" } else { "fail" }, alone as u8, path(), if ok { "PASS" } else { "FAIL" }
    );
}
