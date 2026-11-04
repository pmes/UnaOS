// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! PREFS (rmbp-ledger B300; AUDIT B287, R79) — the kernel reads and writes PRINCIPIA'S preference store.
//! There is ONE store (since R98 the `<home>/settings/` folder below; PRINCIPIAFILES B445 put Principia on it), the one `handlers/principia` owns on the
//! host, in Principia's format (one TOML table per namespace, dotted keys, four scalar types). The value
//! model and the codec are `unaos/libs/sys/prefs_core` — the same crate Principia links — so the two rings
//! cannot drift: what the kernel writes, Principia reads unchanged, and the reverse (the cross-tests in
//! `handlers/principia/src/prefs.rs` pin it). Before this file the kernel kept two private stores in two
//! private formats in the home; their one-shot import ran at every login from B300 to B445 and is RETIRED
//! (PRINCIPIAFILES, ARCHREVIEW F1): no kernel code opens them any more.
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
//! The render is PARSED (it must be the tree that was serialized), then `<d>.new` is written and READ BACK byte-equal
//! (`MountTable::put_file_atomic`), and only then does the old file go and the temp get renamed over it. A load that finds no
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
//! # Decisions (REASK, rmbp-ledger B681, R130 law 2)
//!
//! An answer the user gave to an ASK (apptrust's foreign-volume Open, a refusal alert, FWPIN's toast) is the user's
//! word: it is stored ONCE here, as `system.decisions.<domain>.<key>` (the `general` domain file), consulted before
//! the ask is raised again, and dropped only by a Revoke (Settings > Privacy). See [`decide`] / [`decided`] /
//! [`revoke`] below.
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
    /// APPEARANCE (B408): Light / Dark / Auto, the accent, the highlight (`prefs_core::appearance`).
    pub const APPEARANCE_MODE: &str = "appearance.mode";
    pub const APPEARANCE_ACCENT: &str = "appearance.accent";
    pub const APPEARANCE_HIGHLIGHT: &str = "appearance.highlight";
    /// TRACKPADPANE (B412): the Trackpad pane (rules: `prefs_core::trackpad`).
    pub const TP_SPEED: &str = "trackpad.speed";
    pub const TP_TAP: &str = "trackpad.tap_to_click";
    pub const TP_NATURAL: &str = "trackpad.natural_scroll";
    pub const TP_SECONDARY: &str = "trackpad.secondary_click";
    pub const TP_THREE_DRAG: &str = "trackpad.three_finger_drag";
    /// DRAGDROP (B440): the folders the user dropped on Quarry's sidebar Favorites (R79: a preference is Principia's).
    pub const QUARRY_FAVORITES: &str = "quarry.favorites"; pub const A11Y_REDUCE_MOTION: &str = "accessibility.reduce_motion"; pub const A11Y_INCREASE_CONTRAST: &str = "accessibility.increase_contrast"; pub const A11Y_CURSOR_SIZE: &str = "accessibility.cursor_size"; pub const A11Y_TEXT_SIZE: &str = "accessibility.text_size"; pub const LOOK: &str = "look"; pub const NOTIFY_DND: &str = "notify.dnd"; pub const NOTIFY_DND_FROM: &str = "notify.dnd_from"; pub const NOTIFY_DND_UNTIL: &str = "notify.dnd_until"; pub const PRIVACY_REMEMBER: &str = "privacy.remember"; pub const LOCALE_TZ: &str = "locale.tz"; pub const LOCALE_REGION: &str = "locale.region"; pub const LOCALE_FIRST_DAY: &str = "locale.first_day"; pub const ALL: [&str; 49] = [BRIGHTNESS, IDLE_MIN, WALLPAPER, VOLUME, MUTE, POINTER, LOWBAT_PCT, DOCK_PINS, SETTINGS_TAB, FONT, FONT_SIZE, LOGIN_ITEMS, DISPLAY_MODE, DOCK_POSITION, DOCK_AUTOHIDE, APPEARANCE_MODE, APPEARANCE_ACCENT, APPEARANCE_HIGHLIGHT, TP_SPEED, TP_TAP, TP_NATURAL, TP_SECONDARY, TP_THREE_DRAG, QUARRY_FAVORITES, A11Y_REDUCE_MOTION, A11Y_INCREASE_CONTRAST, A11Y_CURSOR_SIZE, A11Y_TEXT_SIZE, LOOK, NOTIFY_DND, NOTIFY_DND_FROM, NOTIFY_DND_UNTIL, PRIVACY_REMEMBER, LOCALE_TZ, LOCALE_REGION, LOCALE_FIRST_DAY, A11Y_CVD, A11Y_INVERT, A11Y_ZOOM, A11Y_ZOOM_FOLLOW, A11Y_SHOW_PERF, LOCK_AFTER_IDLE_MIN, LOCK_ON_LID, LOCK_ON_SLEEP, KBD_LAYOUT, KBD_COMPOSE, AUDIO_OUTPUT, A11Y_SHIMMER, A11Y_REDUCE_TRANSPARENCY]; pub const AUDIO_OUTPUT: &str = "audio.output"; pub const KBD_LAYOUT: &str = "keyboard.layout"; pub const KBD_COMPOSE: &str = "keyboard.compose"; pub const A11Y_CVD: &str = "accessibility.cvd"; pub const A11Y_INVERT: &str = "accessibility.invert"; pub const A11Y_ZOOM: &str = "accessibility.zoom"; pub const A11Y_ZOOM_FOLLOW: &str = "accessibility.zoom_follow"; pub const A11Y_SHOW_PERF: &str = "accessibility.show_performance"; pub const LOCK_AFTER_IDLE_MIN: &str = "lock.after_idle_min"; pub const LOCK_ON_LID: &str = "lock.on_lid"; pub const LOCK_ON_SLEEP: &str = "lock.on_sleep"; pub const DISPLAY_ICC: &str = "display.icc"; pub const A11Y_SHIMMER: &str = "accessibility.shimmer"; pub const A11Y_REDUCE_TRANSPARENCY: &str = "accessibility.reduce_transparency"; // VEIL-REDUCE-ROW (rmbp-ledger B1128): A11Y_REDUCE_TRANSPARENCY, IN ALL since VEIL-REDUCE-FACE (rmbp-ledger B1134) gave it Settings > Accessibility row 1 (its check beside Increase contrast). DISPLAYICCFILE (rmbp-ledger B972): DISPLAY_ICC, the absolute path of an installed display profile `video::colour::latch` takes over the EDID, OUT of ALL until DISPLAY-ICC-FACE gives it a Settings row (ALL is the keys Settings rows; `settings::selftest_map` reds an unrowed one). TZLOCALE (B701): the three system.locale.* keys, Settings > Date & Time's rows. REASK (B681): PRIVACY_REMEMBER, Settings > Privacy's row (does a new answer get kept). SETTINGSMAP (B673): the keys Settings owns a row for (LOOK = prefs_core::appearance::LOOK_KEY, NOTIFY_* = notify/notifypane's, held equal by `tests settingsmap`) and ALL, the iterable table of every key here (LINE-NEUTRAL fold, code first). A11YPANE (B655): the Accessibility pane's four keys (LINE-NEUTRAL fold, code first: this file is lexed into every image)
}

static TREE: crate::sync::Mutex<PrefTree> = crate::sync::Mutex::new(PrefTree::new());
/// The session user the tree was loaded for (`None` = never loaded; `Some("")` = no session).
static LOADED_FOR: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None);
static LOADED_N: AtomicU32 = AtomicU32::new(0);
static SAVED_N: AtomicU32 = AtomicU32::new(0);
/// The file on disk was refused at load: saves are held so it is never overwritten.
static HELD: AtomicBool = AtomicBool::new(false);
/// File-size ceiling for a read (a preferences file is a few hundred bytes).
const READ_MAX: usize = 64 * 1024;

// ── Where ────────────────────────────────────────────────────────────────────────────────────

/// PERFREVIEW F3 (B443): the session name read into `buf` — the per-pass compares (prefs, Settings, login items)
/// allocate nothing (the String-returning `user_name` it replaced allocated per pass).
pub(crate) const WHO_BUF: usize = 64;
pub(crate) fn user_name_in(buf: &mut [u8; WHO_BUF]) -> Option<&str> {
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        let n = crate::fs::users::whoami(&mut nm)?.min(WHO_BUF);
        buf[..n].copy_from_slice(&nm[..n]);
        return core::str::from_utf8(&buf[..n]).ok();
    }
    #[cfg(not(feature = "login"))]
    {
        let _ = buf;
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
    alloc::format!("{}/{}", base(), prefs_core::files::LEGACY)
}

// ── VFS helpers (mount table, kernel principal) ─────────────────────────────────────────────────

fn read_all(p: &str) -> Option<Vec<u8>> {
    let mt = store_mt();
    let st = mt.stat(p).ok()?; if machine_refused(&mt, p) { return None; } // MACHINESTOREPLANT (rmbp-ledger B1156): a machine-store file whose nearest owner row is not ROOTACL's `system` is read as absent (file tail). LINE-NEUTRAL append, code first
    if st.size as usize > READ_MAX {
        return None;
    }
    if st.size == 0 {
        return Some(Vec::new());
    }
    mt.read(p, 0, st.size as usize).ok()
}

fn write_all(p: &str, b: &[u8]) -> Result<(), String> {
    store_mt().put_file(p, b, crate::fs::vfs::KERNEL_PRINCIPAL).map(|_| ()).map_err(|e| alloc::format!("write {}: {:?}", p, e)) // VFSPUTONE (rmbp-ledger B964): prefs' binding (its store, its String error) on the ONE VFS whole-file writer
}

/// `<home>/settings` — a plain folder in the home (R98: no link, no dot).
fn ensure_dirs() {
    use crate::fs::vfs::NodeKind;
    let mt = store_mt();
    let d = path();
    if lay_machine_dir(&mt, &d, "save").is_none() && mt.stat(&d).is_err() { // MACHINESTOREONELAY (rmbp-ledger B1167): the no-session folder is laid and stamped by the ONE lay (file tail; was MACHINESTOREACL B1150's own create + stamp); a home's folder keeps this create. Named in-place hunk
        let _ = mt.create(&d, NodeKind::Dir, crate::fs::vfs::KERNEL_PRINCIPAL);
    }
}

// ── SETTINGSFILES state ─────────────────────────────────────────────────────────────────────────

/// Domains whose file refused at load: their saves are HELD so the user's file is never overwritten.
static HELD_D: crate::sync::Mutex<Vec<String>> = crate::sync::Mutex::new(Vec::new());
/// Domains whose file is on the volume (at the last load / save) — the reset-on-delete watch list.
static ON_DISK: crate::sync::Mutex<Vec<String>> = crate::sync::Mutex::new(Vec::new());
/// Who made the write in flight when it was not the domain's own pane (the `pref` verb, the fixture).
static BY: crate::sync::Mutex<Option<&'static str>> = crate::sync::Mutex::new(None);
/// One save (or the delete watch) at a time: the swap's unlink window must not read as a user delete.
static SAVE_LOCK: crate::sync::Mutex<()> = crate::sync::Mutex::new(());
/// The programs' declared stanzas (`PrefDeclare`, `app.<name>.*`).
static DECLARED: crate::sync::Mutex<prefs_core::declare::Registry> = crate::sync::Mutex::new(prefs_core::declare::Registry::new());
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
    prefs_core::files::writer_of(d)
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
        path(), ON_DISK.lock().len(), LOADED_N.load(Ordering::Relaxed), SAVED_N.load(Ordering::Relaxed), NS, if ok { "loaded" } else { "refused" }
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
    if let Ok(ents) = store_mt().read_dir(&path()) {
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
    let mut tree = PrefTree::new(); crate::fs::rootacl::stamp_store(&store_mt(), &path()); // MACHINESTOREPLANT (rmbp-ledger B1156): the no-session store's folder is stamped before its first read (a home's path is a no-I/O no-op). LINE-NEUTRAL append, code first
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
            let mt = store_mt();
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
}

/// PREFSKERNEL M3: values the last load clamped into their schema range.
static LOAD_CLAMPED: AtomicU32 = AtomicU32::new(0);

/// Load once per login (and once with no session). Cheap when nothing changed.
pub fn ensure_loaded() { let (sk, sq) = PREFS_SEEN.check(); if sq.is_some() { return; } PREFS_SEEN.mark(sk, true); // SESSIONGEN (B462): the session and this load's state are unchanged since the last compare — one load, no lock
    let mut wb = [0u8; WHO_BUF];
    let u = user_name_in(&mut wb).unwrap_or(""); // PERFREVIEW F3 (B443): compared on the stack; a String only when the session changed
    let (fresh, was) = { // PREFSRELOAD (B763): `was` = a tree was loaded for another session (a login or logout moved SESSION_GEN) — that switch is diffed and posted below (LINE-NEUTRAL fold)
        let mut g = LOADED_FOR.lock();
        if g.as_deref() != Some(u) {
            let was = g.is_some(); *g = Some(String::from(u));
            (true, was)
        } else {
            (false, false)
        }
    };
    if fresh {
        let (t0, old) = (crate::arch::ms(), if was { Some(TREE.lock().clone()) } else { None }); load(); if let Some(old) = old { let _ = reload_post(&old, t0); } // PREFSRELOAD (B763): the old session's tree against the new store, one PrefChanged per differing key (same-line fold, code first)
    }
}

/// The desktop service passes call this (settings, dock): the per-login load happens here, off the
/// click paths — and the SETTINGSFILES delete watch, and REASK's (B681) owed decisions (persisted off the press route).
pub fn service() {
    ensure_loaded();
    if DECIDE_OWED.load(Ordering::Acquire) {
        let _ = decisions_flush();
    }
    let now = crate::arch::ms();
    if now.saturating_sub(WATCH_MS.load(Ordering::Relaxed)) >= WATCH_EVERY_MS {
        WATCH_MS.store(now, Ordering::Relaxed);
        // SMALLFIX10 (B539 item 3): flight 31's LOGINRENDER `quarry:225` was THIS watch on the render task (two
        // `stat`s per domain under SAVE_LOCK, slowed by the registry build holding the volume masked). On a worker
        // core now, the wallpaper probe's shape (wallpaper.rs); inline where there is no worker core.
        #[cfg(target_arch = "x86_64")]
        if let Some(cpu) = crate::arch::smp::worker_cpu(0) {
            if !WATCHING.swap(true, Ordering::AcqRel) {
                crate::arch::sched::spawn_stack("prefs-watch", watch_task, cpu, cpu, crate::arch::sched::PRIO_NORMAL, crate::arch::sched::RENDER_PATH_STACK_SIZE);
            }
            return;
        }
        let _ = watch_deleted();
    }
}

/// SMALLFIX10 (B539 item 3): one delete-watch run in flight on the worker (never two).
static WATCHING: AtomicBool = AtomicBool::new(false);

/// SMALLFIX10 (B539 item 3): the delete watch's worker task — counted as moved off the render task.
#[cfg(target_arch = "x86_64")]
fn watch_task(_cpu: usize) {
    crate::video::loginrender::moved(crate::video::loginrender::W_PREFSWATCH);
    let _ = watch_deleted();
    WATCHING.store(false, Ordering::Release);
}

/// R98: a domain file the user deleted resets that domain to the schema defaults (the store never holds a
/// default, so the keys simply leave the tree and every reader answers its default). Returns the domains reset.
fn watch_deleted() -> Vec<String> {
    let Some(_g) = SAVE_LOCK.try_lock() else { return Vec::new() };
    let mt = store_mt(); say_stat_once(&mt); // DONGLEBOOT (B658): the store's own volume, never a mount-table build per pass (LINE-NEUTRAL fold, code first)
    let gone: Vec<String> = ON_DISK.lock().iter().filter(|d| mt.stat(&domain_path(d)).is_err() && mt.stat(&alloc::format!("{}.new", domain_path(d))).is_err()).cloned().collect(); let mut cleared: Vec<(String, String)> = Vec::new(); // PREFSDELETED (B755): the keys a delete clears (LINE-NEUTRAL fold, code first)
    for d in &gone {
        let keys = prefs_core::files::clear(&mut TREE.lock(), d);
        ON_DISK.lock().retain(|x| x != d);
        serial_println!("[prefs] settings/{} absent -> defaults (deleted by the user) keys={}", d, keys.len()); cleared.extend(keys); // PREFSDELETED (B755): collected, posted below (LINE-NEUTRAL fold, code first)
    }
    drop(_g); deleted_post(&cleared); gone // PREFSDELETED (B755): SAVE_LOCK and the TREE lock released, then one PrefChanged per cleared key (file tail). LINE-NEUTRAL fold, code first.
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
    // Parse the render: it must BE the domain's tree. The swap's read-back then proves the file is these bytes (VFSPUTREST B969).
    match PrefTree::parse(&text) {
        Ok(t) if t == want || t.to_toml() == want.to_toml() => {} // re-emit equality covers a NaN value
        _ => return Err(String::from("render does not parse to the tree")),
    }
    ensure_dirs();
    // `<d>.new` written, read back byte-equal, then renamed over the domain file; a volume with no rename is written direct.
    if !store_mt().put_file_atomic(&p, text.as_bytes(), crate::fs::vfs::KERNEL_PRINCIPAL).map_err(|e| alloc::format!("swap {}: {:?}", p, e))? {
        serial_println!("[prefs] swap=direct (the volume has no rename)");
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
    /// SETTINGSFILES: the domain this write saves (`None` = every domain).
    domain: Option<String>,
}

impl prefs_core::wire::Persist for KernelPersist {
    fn with_tree<R>(&mut self, f: impl FnOnce(&mut PrefTree) -> R) -> R {
        f(&mut TREE.lock())
    }
    fn save(&mut self) -> Result<(), ()> {
        // SETTINGSFILES (R98): a key's write rewrites ONLY its domain's file.
        let r = match self.domain.take() {
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
    } else if ns == NS && prefs_core::notify::parse_key(k).is_some() {
        // NOTIFYPANE (B435): `system.notify.<app>.<field>` is checked by the per-app stanza (`prefs_core::notify`).
        match prefs_core::notify::check(k, v.clone()) {
            Ok(a) => a.value,
            Err(x) => {
                serial_println!("[prefs] set {}.{}={} ok=0 why={:?} (notify stanza)", ns, k, v, x);
                return Err(SetError::Refused(x));
            }
        }
    } else {
        v
    };
    let mut kp = KernelPersist { err: None, domain: Some(prefs_core::files::domain_of(ns, k)) };
    let mut r = prefs_core::wire::persisted_set(&mut kp, ns, k, v.clone());
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
///   or:   <ns> "." <key> NUL     (PREFSUNSET SR169: the key left the store with no preference-level default; wire::unset_body)
/// ```
///
/// (the PREF_SET request body, unsolicited). Also the ATTRSURF hook: the key is mirrored as an attribute
/// on the preferences file once the VFS can set one ([`mirror_attr`]).
fn changed(ns: &str, k: &str, a: &prefs_core::schema::Applied) {
    let v = &a.value;
    serial_println!("[prefs] changed {}.{}", ns, k); #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))] crate::prefs_client::on_changed(ns, k, Some(a)); // SETTINGSBUS (B337): the verb-19 frame to the kernel's subscribers. PREFSKERNEL (B345): it carries `clamped`. ARMNOEL0: widened with the prefs_client cfg (lib.rs).
    mirror_attr(ns, k, v); machine_mirror(ns, k, Some(v)); // VEIL-REDUCE-BOOT-STORE (rmbp-ledger B1140): a machine key the session committed is copied to the no-session store (file tail). LINE-NEUTRAL append, code first
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
        ["unset", a] => unset_verb(a, out), ["list"] | ["list", _] => { // PREFS-UNSET-VERB (rmbp-ledger B1161): the operator unset (file tail). LINE-NEUTRAL fold, code first
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
        _ => out("usage: pref get <ns.key> | pref set <ns.key> <value> | pref unset <ns.key> | pref list [<ns>]"),
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
    } fn unset(&mut self, ns: &str, k: &str) -> Result<bool, prefs_core::wire::SetFail> { store_unset(ns, k) } // PREFS-UNSET-BUS (rmbp-ledger B1166): the bus UNSET's store arm is B1161's `unset` (file tail). LINE-NEUTRAL fold, code first
    /// SETTINGSFILES (B407): PrefDeclare — the program's stanza is held for its `app.<name>.*` writes and
    /// its descriptions head the keys in `settings/<name>`.
    fn declare(&mut self, name: &str, keys: Vec<prefs_core::declare::DeclKey>) -> i64 {
        let n = keys.len();
        let st = prefs_core::declare::insert(&mut DECLARED.lock(), name, keys); // PREFSCAP (B454): capped at 64 programs
        if st != 0 {
            serial_println!("[prefs] declare app.{} refused: {} programs held (PREFSCAP cap)", name, prefs_core::declare::MAX_PROGRAMS);
            return st;
        }
        serial_println!("[prefs] declared app.{} keys={} -> settings/{} (R98)", name, n, name);
        0
    }
}

// The shared wire's numbers are the kernel bus's (prefs_core's dev-test pins them to una-abi too).
#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))] // ARMNOEL0: widened with the `crate::bus` cfg (lib.rs)
const _: () = assert!(
    prefs_core::wire::BODY_MAX == crate::bus::BUS_BODY_MAX
        && prefs_core::wire::VERB_GET == una_abi::BUS_VERB_PREF_GET
        && prefs_core::wire::VERB_SET == una_abi::BUS_VERB_PREF_SET
        && prefs_core::wire::VERB_LIST == una_abi::BUS_VERB_PREF_LIST
        && prefs_core::wire::VERB_CHANGED == una_abi::BUS_VERB_PREF_CHANGED && prefs_core::wire::VERB_UNSET == una_abi::BUS_VERB_PREF_UNSET // PREFS-UNSET-BUS (rmbp-ledger B1166): verb 29 pinned. LINE-NEUTRAL fold, code first
);

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))] // the `crate::bus` cfg (lib.rs): no bus on this build, no bus fulfiller (merge9 fold); ARMNOEL0: widened with it so the aarch64 desktop without EL0 reads the store
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

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))] // the `crate::bus` cfg (lib.rs): no bus on this build, no bus fulfiller (merge9 fold); ARMNOEL0: widened with it
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
    if let Some(k) = crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &path()) { serial_println!(":: PREFS-FIXTURE: dir={} -> SKIP reason=home-not-writable({}) ::", path(), k); serial_println!(":: SETTINGSFILES: dir={} -> SKIP reason=home-not-writable({}) ::", path(), k); return; } ensure_loaded(); // QEMUSKIP (B613): every leg saves to <home>/settings — a refused volume is SKIP (the run's `[prefs] set … ok=0 why=Io`)
    #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))] let codec = codec_selftest(); // ARMNOEL0: widened (with its twin) to the `crate::bus` cfg
    #[cfg(not(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware"))))] let codec = true; // no bus on this build: the codec leg is vacuous, not failed
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
    #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))] let ks = bus_fulfil(prefs_core::wire::VERB_SET, wire_body, true, &mut kt); // ARMNOEL0: widened (with its twin) to the bus_fulfil cfg
    #[cfg(not(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware"))))] let ks = prefs_core::wire::fulfil(&mut KernelStore, prefs_core::wire::VERB_SET, wire_body, true, &mut kt);
    let mut rt = Vec::new();
    let rs = prefs_core::wire::fulfil(&mut prefs_core::wire::TreeStore::default(), prefs_core::wire::VERB_SET, wire_body, true, &mut rt);
    let wire_ok = ks == 0 && ks == rs && kt == rt && kt == b"100\x00clamped=true";
    // Leg L, the load clamp: a file holding an out-of-range value loads clamped, once, and is re-saved so.
    let _ = write_all(&p, b"[system]\npower.lowbat_shutdown_pct = 250\n");
    *LOADED_FOR.lock() = None; PREFS_SEEN.forget(); // SESSIONGEN (B462): after the clear
    ensure_loaded();
    let refile = read_all(&p).and_then(|b| String::from_utf8(b).ok()).and_then(|t| PrefTree::parse(&t).ok());
    let loadclamp_ok = LOAD_CLAMPED.load(Ordering::Relaxed) == 1
        && get(NS, lk) == Some(PrefValue::Int(100))
        && refile.as_ref().and_then(|t| t.get(NS, lk).cloned()) == Some(PrefValue::Int(100));
    // Leg 3 — a malformed file: refused with its line, nothing adopted, defaults hold, saves held.
    let bad = b"[system]\ndisplay.brightness = 3\nrecents = [\"a\"]\n";
    let _ = write_all(&p, bad);
    *LOADED_FOR.lock() = None; PREFS_SEEN.forget(); // SESSIONGEN (B462): after the clear
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
    settingsfiles_selftest(); unset_selftest(); unset_bus_selftest(); prefscap_path_selftest(); // PREFS-UNSET-VERB (rmbp-ledger B1161): the unset KAT (file tail). PREFS-UNSET-BUS (rmbp-ledger B1166): the bus UNSET KAT (file tail). LINE-NEUTRAL fold, code first PREFSCAP-PATH (rmbp-ledger B1170): the path-keyed cap KAT (file tail).
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

/// WINMEMORY (B429, MACPARITY row 12): declare ONE key of `app.<name>` on the program's behalf — the window
/// manager owns the frame, so it declares `window.frame` at the app's first window. MERGES: a stanza the
/// program declared itself (Lumen's) keeps its rows; a key already there is left as it is. `line` is one
/// PrefDeclare line (`<key>\t<spec>\t<default>\t<doc>`). Returns whether the key was added.
pub fn declare_app_key(name: &str, line: &str) -> bool {
    let mut body = Vec::from(name.as_bytes());
    body.push(0);
    body.extend_from_slice(line.as_bytes());
    let Some((n, keys)) = prefs_core::declare::parse(&body) else { return false };
    let mut reg = DECLARED.lock();
    let stanza = reg.entry(n).or_default();
    let mut added = 0usize;
    for k in keys {
        if !stanza.iter().any(|d| d.key == k.key) && stanza.len() < prefs_core::declare::MAX_KEYS {
            stanza.push(k);
            added += 1;
        }
    }
    let total = stanza.len();
    drop(reg);
    if added > 0 {
        serial_println!("[prefs] declared app.{} keys={} -> settings/{} (R98, by the window manager)", name, total, name);
    }
    added > 0
}
// ── LAUNCHERPREFS (B451): the kernel's own programs declare and retire through the one store ──────

/// LAUNCHERPREFS (B451, ARCHREVIEW F7): a kernel-resident program (the Launcher) declares its `app.<name>.*`
/// stanza — the PrefDeclare body (`prefs_core::declare::body`) — into the same registry the bus verb fills,
/// as the kernel (no frame; PREFSCAP B454's caller-bound declare names this the kernel's row). `false` = a
/// malformed stanza. Idempotent: a second declare replaces the first.
pub fn declare_kernel(body: &[u8]) -> bool {
    let Some((name, keys)) = prefs_core::declare::parse(body) else {
        serial_println!("[prefs] declare (kernel) refused: malformed stanza");
        return false;
    };
    let n = keys.len();
    serial_println!("[prefs] declared app.{} keys={} -> settings/{} (kernel)", name, n, name);
    DECLARED.lock().insert(name, keys);
    true
}

/// LAUNCHERPREFS (B451): drop every key of namespace `ns` from the tree (no save: the caller's next
/// [`set`] in the same domain rewrites that domain's file without them). Answers the keys removed.
pub fn retire(ns: &str) -> usize {
    let mut t = TREE.lock();
    let gone: Vec<String> = t.list(ns).into_iter().map(|(k, _)| String::from(k)).collect();
    for k in &gone {
        t.remove(ns, k);
    }
    gone.len()
}

// ── PREFSCAP (rmbp-ledger B454, SECREVIEW F4): the ring-3 transport stamps the caller ─────────────────
//
// Both syscall dispatchers call [`bus_fulfil_from`] with the kernel-stamped wm owner of the calling slot
// (x86 slot + 1, aarch64 the asid — the key the dialog verbs use); the program is named from
// `wm::app_name_of(owner)`, the launcher-armed name, never from the body. `prefs_core::cap` decides (one
// decision, both rings): `app.<own>.*`, the namespace named after the program, the schema's `ring 3` rows.
// The kernel's own client (`prefs_client`) keeps [`bus_fulfil`] = the trusted caller.

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
/// The stamped program name of wm `owner`, lowercased into `buf`; `None` = nothing armed.
fn caller_name(owner: u64, buf: &mut [u8; crate::video::wm::MAX_TITLE]) -> Option<&str> {
    let n = crate::video::wm::app_name_of(owner, buf); #[cfg(feature = "busreg")] let n = prefscap_early_name(owner, n, buf); // PREFSCAP-EARLY-NAME (rmbp-ledger B1182): no launcher-armed name -> wm's early record at the row's current generation, the cap road only (file tail). LINE-NEUTRAL fold, code first
    buf[..n].make_ascii_lowercase();
    core::str::from_utf8(&buf[..n]).ok().filter(|s| !s.is_empty())
}

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
/// [`bus_fulfil`] for a RING-3 caller: the same fulfiller under the caller the kernel stamped.
pub fn bus_fulfil_from(verb: u8, body: &[u8], in_session: bool, owner: u64, text: &mut Vec<u8>) -> i64 {
    ensure_loaded();
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    let caller = match caller_name(owner, &mut nb) {
        Some(p) => prefs_core::cap::Caller::Program(p),
        None => prefs_core::cap::Caller::Anon,
    }; let (caller, demoted) = prefscap_path(owner, caller); // PREFSCAP-PATH (rmbp-ledger B1170): `prefs` is Principia only from /apps/PREFS.ELF, else Anon (file tail). LINE-NEUTRAL fold, code first
    if verb == prefs_core::wire::VERB_SET && in_session {
        if let Some((ns, k, _)) = prefs_core::wire::parse_set(body) {
            if !prefs_core::cap::may_set(caller, ns, k) {
                serial_println!("[prefs] set {}.{} refused: program={} may not (PREFSCAP)", ns, k, prefs_caller_text(caller));
            }
        }
    }
    if verb == prefs_core::wire::VERB_DECLARE && in_session {
        if let Some((name, _)) = prefs_core::declare::parse(body) {
            if !prefs_core::cap::may_declare(caller, &name) {
                serial_println!("[prefs] declare app.{} refused: program={} may not (PREFSCAP)", name, prefs_caller_text(caller));
            }
        }
    }
    if (verb == prefs_core::wire::VERB_SET || verb == prefs_core::wire::VERB_DECLARE) && !in_session {
        if let Some((ns, k, _)) = prefs_core::wire::parse_set(body) {
            serial_println!("[prefs] set {}.{} refused: caller is not the session user", ns, k);
        }
    }
    prefscap_set_note(verb, body, demoted, unset_bus_note(verb, body, caller, prefs_core::wire::fulfil_as(&mut KernelStore, verb, body, in_session, caller, text))) // PREFS-UNSET-BUS (rmbp-ledger B1166): a ring-3 UNSET is said with its caller (file tail). LINE-NEUTRAL fold, code first PREFSCAP-PATH (rmbp-ledger B1170): a demoted caller's SET is said with path=foreign (file tail).
}

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
fn prefs_caller_text(c: prefs_core::cap::Caller<'_>) -> &str {
    match c {
        prefs_core::cap::Caller::Program(p) => p,
        prefs_core::cap::Caller::Kernel => "kernel",
        prefs_core::cap::Caller::Anon => "anon",
    }
}

/// PREFSCAP (B454) — `tests prefscap` (never at boot, R80): a program stamped `pcaptest` sets its own
/// `app.pcaptest.*` and is refused `vein.endpoint`, `system.login.items`, `system.display.mode` and another
/// program's stanza; it declares its own stanza and not another's; the declared registry refuses the 65th
/// program; the bus route refuses a ring-3 `R3PREF_SET` (the deputy). Everything it made is removed.
/// `:: PREFSCAP: own=ok foreign=refused system=<n>-settable declare_cap=64 deputy=<refused/skip> -> PASS ::`.
#[cfg(feature = "witness")]
pub fn prefscap_selftest() {
    use prefs_core::cap::Caller;
    use prefs_core::wire::{fulfil_as, EACCES, ENOSPC, VERB_DECLARE, VERB_SET};
    ensure_loaded();
    const ME: Caller = Caller::Program("pcaptest"); if let Some(k) = crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &path()) { serial_println!(":: PREFSCAP: dir={} -> SKIP reason=home-not-writable({}) ::", path(), k); return; } // QEMUSKIP (B613): `own=` is a save to <home>/settings
    let mut o = Vec::new();
    let own = fulfil_as(&mut KernelStore, VERB_SET, b"app.pcaptest.level\x003", true, ME, &mut o) == 0
        && get(prefs_core::files::APP_NS, "pcaptest.level") == Some(PrefValue::Int(3));
    let before: Vec<Option<PrefValue>> = [("vein", "endpoint"), ("system", "login.items"), ("system", "display.mode"), ("app", "lumen.window.frame")]
        .iter().map(|(n, k)| get(n, k)).collect();
    let foreign = [
        b"vein.endpoint\x00\"https://pcaptest.invalid/\"".as_slice(),
        b"system.login.items\x00\"/apps/PCAPTEST.ELF\"",
        b"system.display.mode\x00\"1x1\"",
        b"app.lumen.window.frame\x00\"0,0,1,1\"",
    ]
    .iter()
    .all(|b| fulfil_as(&mut KernelStore, VERB_SET, b, true, ME, &mut o) == EACCES)
        && [("vein", "endpoint"), ("system", "login.items"), ("system", "display.mode"), ("app", "lumen.window.frame")]
            .iter().map(|(n, k)| get(n, k)).collect::<Vec<_>>() == before;
    let st = |n: &str| alloc::format!("{}\0level\tint:0:10\t5\tPREFSCAP fixture\n", n).into_bytes();
    let held_before = DECLARED.lock().len();
    let decl_own = fulfil_as(&mut KernelStore, VERB_DECLARE, &st("pcaptest"), true, ME, &mut o) == 0;
    let decl_foreign = fulfil_as(&mut KernelStore, VERB_DECLARE, &st("pcapother"), true, ME, &mut o) == EACCES
        && !DECLARED.lock().contains_key("pcapother");
    // Fill to the cap with throwaway names, ask once more, then remove every name this leg added.
    let mut added: Vec<String> = alloc::vec![String::from("pcaptest")];
    let mut i = 0usize;
    while DECLARED.lock().len() < prefs_core::declare::MAX_PROGRAMS && i < 2 * prefs_core::declare::MAX_PROGRAMS {
        let n = alloc::format!("pcapfill{}", i);
        if !DECLARED.lock().contains_key(n.as_str()) && fulfil_as(&mut KernelStore, VERB_DECLARE, &st(&n), true, Caller::Kernel, &mut o) == 0 {
            added.push(n);
        }
        i += 1;
    }
    let capped = DECLARED.lock().len() == prefs_core::declare::MAX_PROGRAMS
        && fulfil_as(&mut KernelStore, VERB_DECLARE, &st("pcapover"), true, Caller::Kernel, &mut o) == ENOSPC;
    {
        let mut d = DECLARED.lock();
        for n in &added {
            d.remove(n.as_str());
        }
    }
    // The fixture's own domain file goes the way SETTINGSFILES' does: unlinked, then the watch resets it.
    let _ = crate::shell::vfs_mount_table().unlink(&domain_path("pcaptest"), crate::fs::vfs::KERNEL_PRINCIPAL);
    WATCH_MS.store(0, Ordering::Relaxed);
    let _ = watch_deleted();
    let _ = TREE.lock().remove(prefs_core::files::APP_NS, "pcaptest.level");
    let restored = DECLARED.lock().len() == held_before.min(prefs_core::declare::MAX_PROGRAMS);
    #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
    let deputy = if crate::bus_route::r3pref_set_refused() { "refused" } else { "OPEN" };
    #[cfg(not(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64"))))]
    let deputy = "skip";
    let n = prefs_core::cap::settable_count();
    let ok = own && foreign && decl_own && decl_foreign && capped && restored && deputy != "OPEN";
    serial_println!(
        ":: PREFSCAP: own={} foreign={} system={}-settable declare_cap={} declare_own={} declare_foreign={} deputy={} -> {} ::",
        if own { "ok" } else { "fail" }, if foreign { "refused" } else { "WRITTEN" }, n,
        if capped { prefs_core::declare::MAX_PROGRAMS } else { 0 }, decl_own as u8, if decl_foreign { "refused" } else { "HELD" },
        deputy, if ok { "PASS" } else { "FAIL" }
    );
}

// ── SESSIONGEN (rmbp-ledger B462, PERFREVIEW F3's follow-up) ─────────────────────────────────────────────────

/// The login generation (`fs::users::SESSION_GEN`); 0 without `login`.
pub fn session_gen() -> u32 {
    #[cfg(feature = "login")]
    {
        crate::fs::users::session_gen_read() // SESSIONGENPOLL (B782): the one counted read (`users::session_gen_reads`)
    }
    #[cfg(not(feature = "login"))]
    {
        0
    }
}

/// One per-pass session consumer's last compare, keyed by (login generation, this consumer's forget epoch).
/// [`SessionSeen::check`] is read BEFORE the compare and [`SessionSeen::mark`]ed with that key, so a login,
/// a logout or a [`SessionSeen::forget`] (the consumer cleared its own record — call it AFTER the clear) that
/// lands mid-pass leaves the key moved and the next pass compares again. Stale marks only ever match a key
/// that nothing has moved since.
pub(crate) struct SessionSeen {
    seen: core::sync::atomic::AtomicU64,
    epoch: AtomicU32,
}

static PREFS_SEEN: SessionSeen = SessionSeen::new();

impl SessionSeen {
    pub(crate) const fn new() -> Self {
        SessionSeen { seen: core::sync::atomic::AtomicU64::new(0), epoch: AtomicU32::new(0) }
    }

    /// The pass's key, and — when the last compare was made under the same key — whether a session was open.
    pub(crate) fn check(&self) -> (u64, Option<bool>) {
        let k = ((session_gen() as u64) << 32) | (((self.epoch.load(Ordering::Acquire) as u64) & 0x3FFF_FFFF) << 2) | 2;
        let s = self.seen.load(Ordering::Acquire);
        (k, if s & !1 == k { Some(s & 1 == 1) } else { None })
    }

    pub(crate) fn mark(&self, k: u64, some: bool) {
        self.seen.store(k | some as u64, Ordering::Release);
    }

    /// Move this consumer's key (call AFTER clearing its record): its next [`check`](Self::check) compares again.
    pub(crate) fn forget(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
}

// ── DONGLEBOOT (rmbp-ledger B658) — the store's table is the store's volume ───────────────────────────────────
//
// Flight 35 boot 2: `[lock] held-too-long name=prefs.rs:224 at=prefs.rs:443` (this file's `watch_deleted` holding
// SAVE_LOCK) for 37522 ms over `name=ahci.rs:249 at=ahci.rs:680` (the SATA port lock) — a stat of a domain file in
// the home on the CARD reached the internal SSD. Not through the stat: `MountTable::resolve` is longest-prefix and
// `<home>/settings/<d>` resolves to `/` (the native UnaFS on the card) whatever else is mounted, the EFI volume of
// `ahci0` included (it is withdrawn from `/volumes` anyway, volumes.rs `withdraw_foreign_esp`). Through the TABLE:
// every `shell::vfs_mount_table()` runs `bootdisk::bind` → `bind_root(…, unafs_state(src), …)` → under `ahciroot`
// (on the image line since INSTALL4) `install::ahciroot::ensure_root_grant()`, which caches only a POSITIVE answer
// and re-probes the SSD's GPT and partitions (`selfinstall::probe`, `partition::census`) on every call when the
// SSD is not OURS — the bench's SSD carries macOS, so every table build read it (every prefs watch pass on boots 1
// and 3 shows the same pair at 53–99 ms). The watch built a table every 2 s.
//
// The fix in this file: the store's reads, writes and stats resolve against ONE mount — `/` as the native UnaFS
// backend, the same `NativeBackend` `bootdisk::bind_root` mounts there — once the root is bound and the store's
// path is claimed by `/` alone (decided once per boot from one full table). A FAT root keeps the full table (the
// homes live on the FAT there; nothing changes). `[prefs] stat domain=<d> mounts_walked=<n> device=<handle>` once
// per boot at the first watch pass, and from `tests dongleboot`. The SSD probe that every OTHER table build still
// runs is ahciroot.rs's (outside this arc's files; the one-line change is in the commit body).

/// 0 = undecided, 1 = the store is on the native root alone, 2 = the full table (a FAT root, or a prefix over it).
static STORE_KIND: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static STAT_SAID: AtomicBool = AtomicBool::new(false);

/// The table every store read, write and stat resolves against.
fn store_mt() -> crate::fs::vfs::MountTable {
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        match STORE_KIND.load(Ordering::Acquire) {
            1 => {
                let mut t = crate::fs::vfs::MountTable::new();
                t.mount("/", alloc::boxed::Box::new(crate::fs::vfs::NativeBackend::new("native")));
                return t;
            }
            2 => return crate::shell::vfs_mount_table(),
            _ => {}
        }
        let mt = crate::shell::vfs_mount_table();
        if matches!(crate::fs::bootdisk::locate(), crate::fs::bootdisk::Verdict::Bound(_)) {
            let p = path();
            let native = mt.volume_name(&p).map(|n| n == "native").unwrap_or(false);
            let alone = mt.prefixes().iter().filter(|q| **q != "/").all(|q| !(p == *q || (p.starts_with(*q) && p.as_bytes().get(q.len()) == Some(&b'/'))));
            STORE_KIND.store(if native && alone { 1 } else { 2 }, Ordering::Release);
        }
        mt
    }
    #[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
    {
        crate::shell::vfs_mount_table()
    }
}

/// The device the store's table reads: the shared UnaFS mount's handle on a native store, else the found root's
/// source (`-` before either is known).
fn store_device(mt: &crate::fs::vfs::MountTable) -> String {
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    if mt.volume_name("/").map(|n| n == "native").unwrap_or(false) {
        if let Some(h) = crate::fs::unafs::mount_bound_handle() {
            return alloc::format!("{:?}", h).to_ascii_lowercase().replace(' ', "");
        }
    }
    let _ = mt;
    match crate::fs::bootdisk::locate() {
        crate::fs::bootdisk::Verdict::Bound(f) => String::from(f.source.name()),
        crate::fs::bootdisk::Verdict::None_(_) => String::from("-"),
    }
}

/// `[prefs] stat …`, once per boot, at the watch's first pass.
fn say_stat_once(mt: &crate::fs::vfs::MountTable) {
    if STAT_SAID.swap(true, Ordering::AcqRel) {
        return;
    }
    let d = ON_DISK.lock().first().cloned().unwrap_or_else(|| String::from("-"));
    serial_println!(
        "[prefs] stat domain={} mounts_walked={} device={} store={} (DONGLEBOOT: the delete watch stats the store's volume only)",
        d, mt.prefixes().len(), store_device(mt), match STORE_KIND.load(Ordering::Acquire) { 1 => "native-root", 2 => "full-table", _ => "undecided" }
    );
}

/// `tests dongleboot` leg 1: the watch's stats, run the watch's way (one table, every domain on the watch list and
/// its `.new`). Returns `(mounts_walked, device, first domain, stats, native_store)`; prints the `[prefs] stat` line.
pub fn dongleboot_stat_probe() -> (usize, String, String, u32, bool) {
    ensure_loaded();
    let mt = store_mt();
    let mut ds: Vec<String> = ON_DISK.lock().clone();
    if ds.is_empty() {
        ds.push(String::from("display"));
    }
    let mut n = 0u32;
    for d in &ds {
        let _ = mt.stat(&domain_path(d));
        let _ = mt.stat(&alloc::format!("{}.new", domain_path(d)));
        n += 2;
    }
    let walked = mt.prefixes().len();
    let dev = store_device(&mt);
    let native = STORE_KIND.load(Ordering::Acquire) == 1;
    serial_println!("[prefs] stat domain={} mounts_walked={} device={} stats={} store={}", ds[0], walked, dev, n, if native { "native-root" } else { "full-table" });
    (walked, dev, ds[0].clone(), n, native)
}

// ── REASK (rmbp-ledger B681, R130 law 2) — a decision the user made is kept in THIS store and never asked again ──────
//
// Peter, R130: "everything else is ASK with the risk said once, then the user's word is final and Principia remembers
// it." Every ASK seam records the answer here and consults it first; nothing expires (R130 law 1: no date the user does
// not control) — only a Revoke drops one.
//
// * Key: `system.decisions.<domain>.<stem>-<fnv64>` — `<domain>` one of [`decide`]'s words; `<stem>` the subject's
//   leaf in key characters (readable in `settings/general`), `<fnv64>` FNV-1a over the WHOLE subject (one key per
//   subject). `prefs_core::files::domain_of` files `system.decisions.*` in `settings/general`.
// * Value (a TOML string): `<answer> <at> <bind> <who>` — the answer word, the civil second it was given (0 = the clock
//   was unset), what the answer is BOUND to (`-` = nothing; apptrust: the program's volume and stamp, so another
//   program at the same path asks again), and the subject itself (the last field, may hold spaces).
// * Recording is QUEUE-ONLY ([`decide`]: no I/O; the answers arrive on the press route) and the queue answers
//   [`decided`] at once; [`service`] persists it through [`set`] (the one swap). [`revoke`] removes and saves.
// * Wire: `[decision] record domain= key= answer= at= queued=1`, `[decision] consult domain= key= answer= age_d= ->
//   remembered`, `[decision] stale domain= key= was= now= -> ask`, `[decision] persist key= ok=`, `[decision] revoke
//   key= ok=`.

/// The decisions' first key segment under `system`.
pub const DECISIONS: &str = "decisions";

/// The ASK seams that record a decision — the key's `<domain>` segment, one word each.
pub mod decide {
    /// `fs::apptrust`: a program on a foreign volume, answered `Open`.
    pub const APPTRUST: &str = "apptrust";
    /// `video::dialog::WHAT_OPENER`: a file's preferred opener refused.
    pub const OPENER: &str = "opener";
    /// `video::dialog::WHAT_FIRMWARE`: FWPIN's toast.
    pub const FIRMWARE: &str = "firmware";
    /// `video::dialog::WHAT_SYSTEM_FILES`: a write to the system trees refused.
    pub const SYSFILES: &str = "sysfiles";
    /// Any other refusal routed through `dialog::refused`.
    pub const NOTICE: &str = "notice"; /** `fs::apptrust::net_gate`: a ring-3 program's outbound reach, answered `always`/`never` (NETGRANT B707; the key's subject is the program's name). */ pub const NET: &str = "net"; // NETGRANT (B707): LINE-NEUTRAL append, code before this comment (this file is lexed into every image)
}

/// A subject is kept to this many bytes (the value stays one short line in `settings/general`).
const WHO_MAX: usize = 160;
/// The readable stem of a key.
const STEM_MAX: usize = 24;

/// One decision the user made.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    /// The full key under `system` (`decisions.<domain>.<stem>-<hash>`) — what [`revoke`] takes.
    pub key: String,
    pub domain: String,
    pub answer: String,
    /// Civil seconds when it was given (0 = the clock was unset).
    pub at: u64,
    pub bind: String,
    pub who: String,
}

/// Decisions given and not yet persisted (`(key, value)`), answered by [`decided`] at once.
static DECIDE_Q: crate::sync::Mutex<Vec<(String, String)>> = crate::sync::Mutex::new(Vec::new());
/// The queue holds a decision the store has not seen ([`service`] persists it).
static DECIDE_OWED: AtomicBool = AtomicBool::new(false);
/// A fixture's civil second (`0` = the live clock). CONSOLEFIX M1: a fixture never sets the live clock — `tests reask`
/// plants a `clock::FixtureClock` and hands its reading here.
static DECIDE_NOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The decisions' view of the civil clock: the fixture's when one is planted, else the live clock (`try_lock`).
fn decision_now() -> Option<u64> {
    match DECIDE_NOW.load(Ordering::Acquire) {
        0 => crate::clock::try_unix_now(),
        n => Some(n),
    }
}

/// `tests reask`: plant (`Some`) or drop (`None`) the fixture's civil second.
pub fn decision_clock(fixture: Option<u64>) {
    DECIDE_NOW.store(fixture.unwrap_or(0), Ordering::Release);
}

/// `s` cut to at most `n` bytes on a character boundary.
fn clip(s: &str, n: usize) -> &str {
    let mut e = s.len().min(n);
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    &s[..e]
}

/// The subject as stored: clipped, printable (a byte outside 0x20..0x7e reads `?`).
fn subject(who: &str) -> String {
    clip(who, WHO_MAX).chars().map(|c| if (' '..='~').contains(&c) { c } else { '?' }).collect()
}

/// A value field with no blank in it (the answer, the bind).
fn field_word(s: &str) -> String {
    let w: String = s.chars().map(|c| if c.is_ascii_graphic() { c } else { '_' }).collect();
    if w.is_empty() { String::from("-") } else { w }
}

/// The key a `(domain, who)` decision lives at: `decisions.<domain>.<stem>-<fnv64 of who>`.
pub fn decision_key(domain: &str, who: &str) -> String {
    let leaf = who.trim_end_matches('/').rsplit('/').next().unwrap_or(who);
    let mut stem: String = leaf.bytes().take(STEM_MAX).map(|b| if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' { b as char } else { '_' }).collect();
    if stem.is_empty() {
        stem.push('x');
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in who.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let dom = if prefs_core::valid_segment(domain) { domain } else { decide::NOTICE };
    alloc::format!("{}.{}.{}-{:016x}", DECISIONS, dom, stem, h)
}

/// Parse a stored decision (`None` = not a decision's key or value).
fn parse_decision(key: &str, v: &str) -> Option<Decision> {
    let rest = key.strip_prefix(DECISIONS)?.strip_prefix('.')?;
    let domain = rest.split('.').next()?;
    let mut it = v.splitn(4, ' ');
    let answer = it.next()?;
    let at = it.next()?.parse::<u64>().ok()?;
    let bind = it.next()?;
    let who = it.next().unwrap_or("");
    Some(Decision { key: String::from(key), domain: String::from(domain), answer: String::from(answer), at, bind: String::from(bind), who: String::from(who) })
}

/// The stored value at `key`: the queue first (given, not yet persisted), then the tree. `blocking = false` takes
/// every lock with `try_lock` (a busy lock reads as no decision: the caller asks, it never spins).
fn decision_value(key: &str, blocking: bool) -> Option<String> {
    let q = if blocking { Some(DECIDE_Q.lock()) } else { DECIDE_Q.try_lock() };
    if let Some(v) = q.as_ref().and_then(|q| q.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone())) {
        return Some(v);
    }
    drop(q);
    let t = if blocking { Some(TREE.lock()) } else { TREE.try_lock() };
    t?.get(NS, key).and_then(|v| v.as_str().map(String::from))
}

fn consult(domain: &str, who: &str, bind: &str, blocking: bool) -> Option<String> {
    let key = decision_key(domain, who);
    let d = parse_decision(&key, &decision_value(&key, blocking)?)?;
    if d.who != subject(who) {
        serial_println!("[decision] collision domain={} key={} -> ask", domain, key);
        return None;
    }
    let bw = field_word(bind);
    if d.bind != bw {
        serial_println!("[decision] stale domain={} key={} was={} now={} -> ask", domain, key, d.bind, bw);
        return None;
    }
    let age = match decision_now() {
        Some(n) if d.at != 0 => alloc::format!("{}", n.saturating_sub(d.at) / 86_400),
        _ => String::from("?"),
    };
    serial_println!("[decision] consult domain={} key={} answer={} age_d={} -> remembered (R130)", domain, key, d.answer, age);
    Some(d.answer)
}

/// The user's answer to the `(domain, who)` ask, if they gave one bound to `bind` — consulted BEFORE the ask is raised.
/// No expiry: the civil clock only dates it (`age_d=` on the wire). Never does I/O.
pub fn decided(domain: &str, who: &str, bind: &str) -> Option<String> {
    consult(domain, who, bind, true)
}

/// [`decided`] for a path that must never spin (the refusal router, reached from the VFS): every lock `try_lock`.
pub fn decided_quiet(domain: &str, who: &str, bind: &str) -> Option<String> {
    consult(domain, who, bind, false)
}

/// Record the user's `answer` to the `(domain, who)` ask, bound to `bind` (`-` = nothing) — unless the user switched
/// `system.privacy.remember` off (then nothing is kept and the ask asks every time). QUEUE-ONLY (no I/O: the
/// press route calls it); [`decided`] answers it at once and [`service`] persists it. Replaces an earlier answer.
pub fn decide(domain: &str, who: &str, bind: &str, answer: &str) {
    let key = decision_key(domain, who);
    // The user's own switch (Settings > Privacy, `system.privacy.remember`, default on): off = this answer is not kept.
    // try_lock: a busy tree reads as the default (this runs on the press route and the refusal router).
    if TREE.try_lock().and_then(|t| t.get(NS, key::PRIVACY_REMEMBER).and_then(|v| v.as_bool())) == Some(false) {
        serial_println!("[decision] record domain={} key={} answer={} kept=0 (privacy.remember=false)", domain, key, field_word(answer));
        return;
    }
    let at = decision_now().unwrap_or(0);
    let aw = field_word(answer);
    let v = alloc::format!("{} {} {} {}", aw, at, field_word(bind), subject(who));
    {
        let mut q = DECIDE_Q.lock();
        q.retain(|(k, _)| *k != key);
        q.push((key.clone(), v));
    }
    DECIDE_OWED.store(true, Ordering::Release);
    serial_println!("[decision] record domain={} key={} answer={} at={} queued=1", domain, key, aw, at);
}

/// Persist the queued decisions through [`set`] (the domain file's swap). A decision whose save fails stays queued (it
/// still answers this boot) and is said. Returns the decisions persisted.
pub fn decisions_flush() -> usize {
    DECIDE_OWED.store(false, Ordering::Release);
    let q = DECIDE_Q.lock().clone();
    let mut n = 0usize;
    for (k, v) in q {
        match set(NS, &k, PrefValue::Str(v.clone())) {
            Ok(()) => {
                DECIDE_Q.lock().retain(|(qk, qv)| !(*qk == k && *qv == v));
                n += 1;
            }
            Err(e) => serial_println!("[decision] persist key={} ok=0 why={} (kept in RAM for this boot)", k, e),
        }
    }
    if n > 0 {
        serial_println!("[decision] persist n={} ok=1", n);
    }
    n
}

/// Every decision the user made (the store's, then any not yet persisted) — Settings > Privacy's rows.
pub fn decisions() -> Vec<Decision> {
    let mut out: Vec<Decision> = {
        let t = TREE.lock();
        t.list(NS)
            .into_iter()
            .filter_map(|(k, v)| if k.starts_with(DECISIONS) { v.as_str().and_then(|s| parse_decision(k, s)) } else { None })
            .collect()
    };
    if let Some(q) = DECIDE_Q.try_lock() {
        for (k, v) in q.iter() {
            if !out.iter().any(|d| d.key == *k) {
                if let Some(d) = parse_decision(k, v) {
                    out.push(d);
                }
            }
        }
    }
    out
}

/// Revoke the decision at `key` (a [`Decision::key`]): it leaves the queue and the tree and its domain file is saved, so
/// the next ask asks. A failed save puts it back (store and RAM never disagree). `true` = a decision was dropped.
pub fn revoke(key: &str) -> bool {
    ensure_loaded();
    let queued = {
        let mut q = DECIDE_Q.lock();
        let n = q.len();
        q.retain(|(k, _)| k != key);
        n != q.len()
    };
    let old = TREE.lock().remove(NS, key);
    if let Some(v) = &old {
        if let Err(e) = save_domain(&prefs_core::files::domain_of(NS, key)) {
            let _ = TREE.lock().set(NS, key, v.clone());
            serial_println!("[decision] revoke key={} ok=0 why={} (kept)", key, e);
            return false;
        }
    }
    let ok = queued || old.is_some();
    serial_println!("[decision] revoke key={} ok={} -> the next ask asks", key, ok as u8);
    ok
}

/// `tests reask`'s reboot: the RAM a reboot loses (the tree, the unpersisted queue, the session's load record) is
/// cleared and the store is read back from its files by the one [`load`]. Returns the keys loaded.
///
/// PREFSRELOAD (B763): the tree the RAM held is diffed against the store read back ([`reload_post`]), so a value the
/// store no longer holds (an unpersisted write) reaches its subscribers as a PrefChanged like any other move.
pub fn reload_from_store() -> usize {
    let t0 = crate::arch::ms();
    DECIDE_Q.lock().clear();
    DECIDE_OWED.store(false, Ordering::Release);
    let old = core::mem::replace(&mut *TREE.lock(), PrefTree::new()); // PREFSDELETED (B755): the tree before the reload, diffed after it (LINE-NEUTRAL fold, code first)
    *LOADED_FOR.lock() = None;
    PREFS_SEEN.forget();
    ensure_loaded(); let _ = reload_post(&old, t0); // PREFSDELETED (B755): one PrefChanged per key the reload moved (file tail). LINE-NEUTRAL fold, code first.
    TREE.lock().len()
}

// ══════════════ PREFSDELETED (rmbp-ledger B755, BP-security-18, BP-storage-15) — TAIL-APPENDED ══════════════
// A deleted domain file (R98's delete watch, `watch_deleted`) and `reload_from_store` reset keys in the tree without a
// write, so no `changed()` ran and no PrefChanged was made: the lock (`lock::prefs_pass`, which re-reads only on a
// PrefChanged naming its keys, `prefs_client::changed_since`) and Settings (`bus_changes_inner`) kept the old value.
// Now each cleared key posts the SAME frame of record `changed()` posts (`prefs_client::on_changed`, under its cfg),
// carrying the value the key answers once unset: its declared default — the schema's static default, a derived one
// evaluated against the store (the kernel has no environment variables and no local models, so those rule inputs
// answer no), or the program's `PrefDeclare` default for `app.<name>.<key>`. A key with no preference-level default
// (`Default::Consumer`, an undeclared key) has no value the frame can carry: PREFSUNSET (SR169) posts it in verb 19's
// unset form (`prefs_core::wire::unset_body`, `<ns>.<key>` NUL), counted `unset=`, so `unsent=` is 0.

/// The kernel's rule inputs: the store's stored values, no environment variable, no local model.
struct StoreEnv;

impl prefs_core::rules::RuleEnv for StoreEnv {
    fn pref(&self, ns: &str, key: &str) -> Option<PrefValue> {
        TREE.lock().get(ns, key).cloned()
    }
    fn env_set(&self, _var: &str) -> bool {
        false
    }
    fn local_model_installed(&self, _name: &str) -> bool {
        false
    }
}

/// The value `ns`.`k` answers once it leaves the store, as the PrefChanged's `Applied` (never clamped). `None` = no
/// preference-level default (the consumer computes it, or the key is undeclared).
fn default_applied(ns: &str, k: &str) -> Option<prefs_core::schema::Applied> {
    let value = if ns == prefs_core::files::APP_NS {
        prefs_core::declare::lookup(&DECLARED.lock(), k).map(|d| d.default.clone())
    } else {
        prefs_core::schema::default_of(ns, k, &StoreEnv).map(|(v, _)| v)
    }?;
    Some(prefs_core::schema::Applied { value, clamped: false })
}

/// Post one PrefChanged per `(ns, key, value)`; a `None` value posts the unset form (PREFSUNSET SR169). Called with no
/// prefs lock held (`on_changed` takes the bus queue's and the ring-3 subscribers' locks only). Returns the frames
/// handed to `on_changed`.
fn post_changes(changes: &[(String, String, Option<prefs_core::schema::Applied>)]) -> usize {
    for (ns, k, a) in changes {
        #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))]
        crate::prefs_client::on_changed(ns, k, a.as_ref()); // the `changed()` cfg: no bus on this build, no frame
        let _ = (ns, k, a); machine_mirror(ns, k, a.as_ref().map(|x| &x.value)); // VEIL-REDUCE-BOOT-STORE (rmbp-ledger B1140): a session switch catches the machine copy up (file tail). LINE-NEUTRAL append, code first
    }
    changes.len()
}

/// `watch_deleted`'s keys, after its locks are dropped: each posts SR169's unset form (the consumer re-reads its
/// default), said once per run.
fn deleted_post(cleared: &[(String, String)]) {
    if cleared.is_empty() {
        return;
    }
    let out: Vec<(String, String, Option<prefs_core::schema::Applied>)> = cleared.iter().map(|(ns, k)| (ns.clone(), k.clone(), None)).collect(); // UNSET-POST-ROADS (rmbp-ledger B1179) + UNSET-POSTED-INLINE (rmbp-ledger B1189): the delete road posts SR169's unset form on this line. LINE-NEUTRAL fold, code first
    let unset = out.iter().filter(|c| c.2.is_none()).count();
    let sent = post_changes(&out);
    deleted_note(cleared.len(), sent);
    serial_println!("[prefs] deleted -> changed keys={} sent={} unsent={} unset={}", cleared.len(), sent, cleared.len() - sent, unset);
}

/// `reload_from_store`'s diff: every key whose value the reload moved (added, changed, or gone -> its default).
fn reload_post(old: &PrefTree, t0: u64) -> usize {
    let mut out: Vec<(String, String, Option<prefs_core::schema::Applied>)> = Vec::new();
    let mut gone: Vec<(String, String)> = Vec::new();
    {
        let t = TREE.lock();
        for (ns, k, v) in t.entries() {
            if old.get(ns, k) != Some(v) {
                out.push((String::from(ns), String::from(k), Some(prefs_core::schema::Applied { value: v.clone(), clamped: false })));
            }
        }
        for (ns, k, _) in old.entries() {
            if t.get(ns, k).is_none() {
                gone.push((String::from(ns), String::from(k)));
            }
        }
    }
    let n = out.len() + gone.len();
    let unset = gone.len(); out.extend(gone.iter().map(|(ns, k)| (ns.clone(), k.clone(), None))); // UNSET-POST-ROADS (rmbp-ledger B1179) + UNSET-POSTED-INLINE (rmbp-ledger B1189): the reload road posts SR169's unset form on this line. LINE-NEUTRAL fold, code first
    let sent = post_changes(&out);
    serial_println!("[prefs] reload gen={} changed={} sent={} unsent={} unset={} ms={}", session_gen(), n, sent, n - sent, unset, crate::arch::ms().saturating_sub(t0)); // PREFSRELOAD (B763) + PREFSDELETED (B755) + PREFSUNSET (SR169): the ONE reload line
    n
}

/// `tests autolock`'s `deleted=` leg (tests.rs tail): the domain `ns`.`k` lives in.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] // the fixture's cfg (tests.rs `autolock_selftest`)
pub fn domain_of_key(ns: &str, k: &str) -> String {
    prefs_core::files::domain_of(ns, k)
}

/// `tests autolock`'s `deleted=` leg: the domain file's bytes as they are now (`None` = not on disk, or held).
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] // the fixture's cfg (tests.rs `autolock_selftest`)
pub fn domain_bytes(d: &str) -> Option<Vec<u8>> {
    ensure_loaded();
    if held(d) || !ON_DISK.lock().iter().any(|x| x == d) {
        return None;
    }
    read_all(&domain_path(d))
}

/// `tests autolock`'s `deleted=` leg: delete the domain file as the user's Quarry delete would (the file and any
/// `.new`), then run THE delete watch until the domain has left the tree (a worker-core watch may take it first;
/// bounded). `true` = the domain is off disk and out of the tree.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] // the fixture's cfg (tests.rs `autolock_selftest`)
pub fn delete_domain_watched(d: &str) -> bool {
    let p = domain_path(d);
    let mt = store_mt();
    let k = crate::fs::vfs::KERNEL_PRINCIPAL;
    if mt.unlink(&p, k).is_err() {
        return false;
    }
    let _ = mt.unlink(&alloc::format!("{}.new", p), k);
    let t0 = crate::arch::ms();
    loop {
        let _ = watch_deleted();
        if !ON_DISK.lock().iter().any(|x| x == d) {
            return true;
        }
        if crate::arch::ms().wrapping_sub(t0) > 5_000 {
            return false;
        }
        crate::arch::sched::yield_now();
    }
}

/// `tests autolock`'s `deleted=` leg: put the domain file back byte for byte, merge its keys into the tree and post
/// their PrefChanged (the restore is a change too). `true` = written and parsed.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] // the fixture's cfg (tests.rs `autolock_selftest`)
pub fn restore_domain(d: &str, bytes: &[u8]) -> bool {
    let Some(Ok(t)) = core::str::from_utf8(bytes).ok().map(PrefTree::parse) else { return false };
    {
        let _g = SAVE_LOCK.lock();
        ensure_dirs();
        if write_all(&domain_path(d), bytes).is_err() {
            return false;
        }
        prefs_core::files::merge(&mut TREE.lock(), &t);
        let mut od = ON_DISK.lock();
        if !od.iter().any(|x| x == d) {
            od.push(String::from(d));
        }
    }
    let out: Vec<(String, String, Option<prefs_core::schema::Applied>)> =
        t.entries().map(|(ns, k, v)| (String::from(ns), String::from(k), Some(prefs_core::schema::Applied { value: v.clone(), clamped: false }))).collect();
    let _ = post_changes(&out);
    true
}

// ══════════════ PREFSUNSET (LEDGER SR169, BP-storage-15) — TAIL-APPENDED ══════════════
// The last non-empty delete-watch run's counts (`deleted_post`): keys cleared and frames handed to `on_changed`. With
// the unset form every cleared key posts, so `tests autolock`'s `deleted=` leg expects sent == keys (unsent=0).
static DELETED_KEYS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static DELETED_SENT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// `deleted_post`'s half: one run's counts.
fn deleted_note(keys: usize, sent: usize) {
    DELETED_KEYS.store(keys, Ordering::Relaxed);
    DELETED_SENT.store(sent, Ordering::Relaxed);
}

/// `tests autolock`'s `deleted=` leg: `(keys, sent)` of the last delete-watch run that cleared keys.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] // the fixture's cfg (tests.rs `autolock_selftest`)
pub fn deleted_counts() -> (usize, usize) {
    (DELETED_KEYS.load(Ordering::Relaxed), DELETED_SENT.load(Ordering::Relaxed))
}

// ── PREFSRELOAD (rmbp-ledger B763) — a session switch is a set of PrefChanged ──────────────────────────────────
//
// A login or a logout moves `fs::users::SESSION_GEN` and changes the session user; the next [`ensure_loaded`] reads the
// new user's store into the tree. Before this arc that load was silent: Settings, the lock, the clock and the look each
// polled SESSION_GEN to notice. Now the load is followed by ONE diff of the tree the old session held against the tree
// the new store gave, and every key whose value differs is posted through [`changed`] — the same `[prefs] changed`
// line and the same verb-19 frame (`prefs_client::on_changed`) a write sends — schema rows first in `SCHEMA` order, then
// the keys no row declares in tree order. THE DIFF IS [`reload_post`] (PREFSDELETED's, the one that knows the unset form
// of PREFSUNSET): a key the new store does not hold carries verb 19's unset body (UNSET-POST-RULE B1185). One line per
// reload: `[prefs] reload gen=<SESSION_GEN> changed=<n> sent=<n> unsent=<n> unset=<n> ms=<n>`.

/// `tests settingsmap`'s reload leg (B763): the session switch with `old` as the tree the previous session held — the
/// store is re-read by the one [`load`] and every key that differs from `old` is posted. Returns the keys posted.
pub fn reload_session(old: &PrefTree) -> usize {
    let t0 = crate::arch::ms();
    ensure_loaded();
    load();
    reload_post(old, t0)
}

// ── VEIL-REDUCE-BOOT-STORE (rmbp-ledger B1140, BP-storage-11, BP-video-11) — TAIL-APPENDED ─────────────────────────────
// The boot login screen has no session user, so its GET (`crystal::login_reduce_load` -> the bus -> [`ensure_loaded`]) reads
// the NO-SESSION store, `/settings/<domain>` ([`path`] with an empty home). That store is the MACHINE copy: while a session
// holds the store, every accepted change of a key in [`MACHINE_KEYS`] ([`changed`]) and every session switch's post
// ([`post_changes`]) is mirrored into `/settings/<domain>` by [`machine_mirror`] — read, the one key set, rendered by the
// same `prefs_core::files::render`, the render parsed, written by the same swap (`put_file_atomic`), under SAVE_LOCK; the
// session TREE is never taken. No session (or no `login`: the store IS `/settings`) = nothing to mirror. A machine file
// prefs_core refuses is never overwritten. Wire: `[prefs] machine settings/<d> system.<k>=<v> wrote=<0|1>` (MACHINE-UNSET B1154: `<v>` = `unset` when a None removed the key).

/// The keys the boot screen reads before any login (the machine copy's whole content from the session side).
pub const MACHINE_KEYS: &[&str] = prefs_core::files::MACHINE_KEYS; // VEIL-BOOT-STORE-R3 (rmbp-ledger B1146): the shared list (prefs_core), one for both rings; A11Y-BOOT-SET (rmbp-ledger B1147): the accessibility keys a boot-screen consumer reads (contrast, cursor, filter, shimmer and motion, the lens); `login::veil_reduce_boot` reads them all through `settings::a11y_load("machine")`. Named in-place hunk

/// Mirror the session's committed `ns`/`k` = `v` into the no-session store's domain file (see the block above).
fn machine_mirror(ns: &str, k: &str, v: Option<&PrefValue>) {
    let shown = v.map_or(String::from("unset"), |v| alloc::format!("{}", v)); // MACHINE-UNSET (rmbp-ledger B1154): None = the key left the session store with no value to carry; it leaves the machine copy by this same RMW. Named in-place hunk
    if ns != NS || !MACHINE_KEYS.contains(&k) {
        return;
    }
    match home() {
        Some(h) if !h.is_empty() => {}
        _ => return,
    }
    let d = prefs_core::files::domain_of(ns, k);
    let dir = alloc::format!("/{}", prefs_core::files::DIR);
    let p = alloc::format!("{}/{}", dir, d);
    let _g = SAVE_LOCK.lock();
    let mut t = match read_all(&p) {
        Some(b) => match parse_said(&p, &b) {
            Ok(t) => t,
            Err(_) => {
                serial_println!("[prefs] machine settings/{} refused: not mirrored", d);
                return;
            }
        },
        None => PrefTree::new(),
    };
    if t.get(ns, k) == v {
        serial_println!("[prefs] machine settings/{} {}.{}={} wrote=0", d, ns, k, shown);
        return;
    }
    if match v { Some(v) => t.set(ns, k, v.clone()).is_err(), None => t.remove(ns, k).is_none() } {
        return;
    }
    let text = prefs_core::files::render(&d, &t, &now_iso(), "the session store's machine mirror (VEIL-REDUCE-BOOT-STORE)", &doc_of);
    match PrefTree::parse(&text) {
        Ok(r) if r == t || r.to_toml() == t.to_toml() => {}
        _ => return,
    }
    let mt = store_mt();
    let _ = lay_machine_dir(&mt, &dir, "mirror"); let ok = mt.put_file_atomic(&p, text.as_bytes(), crate::fs::vfs::KERNEL_PRINCIPAL).is_ok(); // MACHINESTOREONELAY (rmbp-ledger B1167): the machine folder is laid and stamped ROOTACL `system` by the ONE lay before its swap (file tail; was MACHINESTOREACL B1150's own create + stamp). Named in-place hunk
    serial_println!("[prefs] machine settings/{} {}.{}={} wrote={}", d, ns, k, shown, ok as u8);
}

// ── MACHINESTOREPLANT (rmbp-ledger B1156, BP-storage-11) — TAIL-APPENDED ───────────────────────────────────────────────
// A file planted in the no-session store before its folder's first stamp keeps its own owner row; [`read_all`] (the ONE
// reader under `load`, `read_domain`'s `.new` adoption, [`machine_mirror`]'s read-modify-write and `domain_bytes`)
// refuses it here through ROOTACL's owner read (`rootacl::store_file_refused`) and reads it as absent, so the boot
// screen never takes it and the mirror never launders it; the kernel's next swap of that domain replaces it.
// Wire: `[prefs] machine settings/<d> refused: owner=<p>`.

/// Is `p` a machine-store file ROOTACL says is not the kernel's? Said once per refused read.
fn machine_refused(mt: &crate::fs::vfs::MountTable, p: &str) -> bool {
    match crate::fs::rootacl::store_file_refused(mt, p) {
        Some(owner) => {
            serial_println!("[prefs] machine settings/{} refused: owner={}", p.rsplit('/').next().unwrap_or(p), owner);
            true
        }
        None => false,
    }
}

/// `tests rootacl`'s MACHINESTORE plant leg reads through the one reader.
pub(crate) fn store_read(p: &str) -> Option<Vec<u8>> {
    read_all(p)
}

// ── PREFS-UNSET-VERB (rmbp-ledger B1161, BP-storage-11) — TAIL-APPENDED ────────────────────────────────────────────────
// The operator's `pref unset <ns.key>`: the key leaves the session store by THE ONE RULE the delete watch and the session
// reload already keep — removed from the tree (never the default written into the user's file), its domain saved by the
// same swap (`KernelPersist` -> `save_domain`, rolled back on a failed save), then ONE post through [`post_changes`] of
// SR169's unset form (`None`, on its own line, UNSET-POSTED-INLINE B1189) for EVERY key (MACHINE-KEY-DEFAULT-PIN B1173, UNSET-POST-ROADS B1179,
// UNSET-POST-RULE B1185), so the mirror removes a machine key (B1154's `=unset`); [`default_applied`] answers the
// operator's `reads <lit>` text only. The bus unset is PREFS-UNSET-BUS (B1166, `store_unset` below). The
// writers column admits the operator for every key (`prefs_core::schema::Writer`), so there is no new check.
// Wire: `[prefs] unset <ns>.<key> ok=1` (+ `[prefs] changed <ns>.<key>`), `ok=1 was=unset`, `ok=0 why=<e>`.

/// Unset `ns`/`k` and persist its domain (see the block above). `Ok(true)` = a value left the store and was posted;
/// `Ok(false)` = it was unset already (nothing saved, nothing posted).
pub fn unset(ns: &str, k: &str) -> Result<bool, SetError> {
    use prefs_core::wire::Persist as _;
    ensure_loaded();
    prefs_core::validate_ns(ns).map_err(SetError::Name)?;
    prefs_core::validate_key(k).map_err(SetError::Name)?;
    let mut kp = KernelPersist { err: None, domain: Some(prefs_core::files::domain_of(ns, k)) };
    let Some(old) = kp.with_tree(|t| t.remove(ns, k)) else {
        serial_println!("[prefs] unset {}.{} ok=1 was=unset", ns, k);
        return Ok(false);
    };
    if kp.save().is_err() {
        kp.with_tree(|t| {
            let _ = t.set(ns, k, old);
        });
        let e = kp.err.take().unwrap_or_else(|| String::from("save failed"));
        serial_println!("[prefs] unset {}.{} ok=0 why={}", ns, k, e);
        return Err(SetError::Io(e));
    }
    serial_println!("[prefs] unset {}.{} ok=1", ns, k);
    serial_println!("[prefs] changed {}.{}", ns, k);
    let _ = post_changes(&[(String::from(ns), String::from(k), None)]); // MACHINE-KEY-DEFAULT-PIN (rmbp-ledger B1173) + UNSET-POSTED-INLINE (rmbp-ledger B1189): SR169's unset form for every key, posted on this line. LINE-NEUTRAL fold, code first
    Ok(true)
}

/// The `pref unset <ns.key>` arm of [`verb`].
fn unset_verb(a: &str, out: &mut dyn FnMut(&str)) {
    let Some((ns, k)) = split_addr(a) else {
        out("pref: address is <ns>.<key> ([A-Za-z0-9_-] segments)");
        return;
    };
    *BY.lock() = Some("the pref verb");
    let r = unset(ns, k);
    *BY.lock() = None;
    match r {
        Ok(true) => match default_applied(ns, k) {
            Some(d) => out(&alloc::format!("{} unset (reads {})", a, d.value.to_literal())),
            None => out(&alloc::format!("{} unset (reads its consumer's default)", a)),
        },
        Ok(false) => out(&alloc::format!("pref: {} was already unset", a)),
        Err(e) => out(&alloc::format!("pref: {} refused: {}", a, e)),
    }
}

/// `tests prefs`' unset line (B1161): a fixture key leaves the tree and its file; a second unset writes nothing; a
/// Consumer-default key posts the unset form; a static-default key is removed (never written as its default) and reads
/// the default (it posts the unset form, UNSET-POST-ROADS B1179); a malformed address is refused. The tree and every domain file touched are restored byte for byte.
/// `:: PREFS-UNSET: removed=1 again=noop consumer=unset static=0 name=refused restored=1 -> PASS ::`.
#[cfg(feature = "witness")]
fn unset_selftest() {
    let fp = domain_path("fixturens");
    let doms = [prefs_core::files::domain_of(NS, key::LOWBAT_PCT), prefs_core::files::domain_of(NS, key::SETTINGS_TAB)];
    let saved: Vec<(String, Option<Vec<u8>>)> = doms.iter().map(|d| (domain_path(d), read_all(&domain_path(d)))).collect();
    let tree0 = TREE.lock().clone();
    let (on_disk0, saved_n0) = (ON_DISK.lock().clone(), SAVED_N.load(Ordering::Relaxed));
    *BY.lock() = Some("tests prefs");
    // Leg 1 — a fixture key leaves the tree and its domain file.
    let fu = "fixture.u";
    let removed = set("fixturens", fu, PrefValue::Int(1)).is_ok()
        && matches!(unset("fixturens", fu), Ok(true))
        && get("fixturens", fu).is_none()
        && read_all(&fp).and_then(|b| String::from_utf8(b).ok()).and_then(|t| PrefTree::parse(&t).ok()).is_some_and(|t| t.get("fixturens", fu).is_none())
        && read_all(&alloc::format!("{}.new", fp)).is_none();
    // Leg 2 — unset again: nothing saved.
    let n_before = SAVED_N.load(Ordering::Relaxed);
    SAVED_N.store(u32::MAX, Ordering::Relaxed);
    let again = matches!(unset("fixturens", fu), Ok(false)) && SAVED_N.load(Ordering::Relaxed) == u32::MAX;
    SAVED_N.store(n_before, Ordering::Relaxed);
    // Leg 3 — a Consumer-default key: removed, posted as the unset form.
    let lk = key::LOWBAT_PCT;
    let consumer = set(NS, lk, PrefValue::Int(40)).is_ok() && matches!(unset(NS, lk), Ok(true)) && get(NS, lk).is_none() && default_applied(NS, lk).is_none();
    // Leg 4 — a static-default key: removed (the store never writes the default), posted with the default.
    let tk = key::SETTINGS_TAB;
    let st_default = default_applied(NS, tk).map(|a| a.value);
    let static_ok = set(NS, tk, PrefValue::Int(3)).is_ok() && matches!(unset(NS, tk), Ok(true)) && get(NS, tk).is_none() && st_default == Some(PrefValue::Int(0));
    // Leg 5 — a malformed address.
    let name = matches!(unset("bad ns", "k"), Err(SetError::Name(_)));
    // Restore the tree and the files.
    let mt = crate::shell::vfs_mount_table();
    for (p, b) in saved.iter() {
        match b {
            Some(b) => {
                let _ = write_all(p, b);
            }
            None => {
                let _ = mt.unlink(p, crate::fs::vfs::KERNEL_PRINCIPAL);
            }
        }
    }
    let _ = mt.unlink(&fp, crate::fs::vfs::KERNEL_PRINCIPAL);
    *TREE.lock() = tree0;
    *ON_DISK.lock() = on_disk0;
    SAVED_N.store(saved_n0, Ordering::Relaxed);
    *BY.lock() = None;
    let restored = saved.iter().all(|(p, b)| read_all(p) == *b) && read_all(&fp).is_none();
    let ok = removed && again && consumer && static_ok && name && restored;
    serial_println!(
        ":: PREFS-UNSET: removed={} again={} consumer={} static={} name={} restored={} -> {} ::",
        removed as u8, if again { "noop" } else { "wrote" }, if consumer { "unset" } else { "FAIL" },
        st_default.map(|v| v.to_literal()).unwrap_or_else(|| String::from("none")), if name { "refused" } else { "admitted" },
        restored as u8, if ok { "PASS" } else { "FAIL" }
    );
}

// ── MACHINESTORESTAMPRING3 (rmbp-ledger B1162, BP-storage-11) — TAIL-APPENDED ──────────────────────────────────────────
// The no-session folder is laid and stamped ROOTACL `system` BEFORE the first ring-3 program runs, so a plant is refused
// at CREATE (`rootacl::write_verdict` on the stamped folder) rather than read as content later (B1156's reader refusal
// stays as the second net, and `load`'s lazy stamp stays). There is no single post-bind pre-spawn boot point (x86 binds
// the root on a service-loop pass, aarch64 lazily), so the call rides each arch's ring-3 image mapper
// (`map_image_into_slot`, every program's one road to a slot), gated on `unafs::mount_bound_handle()` — a masked read-only
// peek that never runs the lazy bind or a survey walk (UNLOCKSTACK B1092): no native volume mounted = no-op, retried at
// the next spawn; latched after the ONE attempt. No store read, no TREE, no SAVE_LOCK (create and stamp are idempotent).
// Wire, once per boot: `[prefs] machine store dir=/settings owner=<system|none> made=<0|1> at=spawn` (ROOTACL's own
// `[rootacl] store dir=/settings owner=system stamped=1` prints only on the boot that WRITES the row).

/// Latched after the one boot attempt (a native volume was mounted at a spawn).
static MACHINE_LAID: AtomicBool = AtomicBool::new(false);

/// The ring-3 mapper's call: lay and stamp the machine store folder once per boot, before the first program's slot.
pub fn stamp_machine_store() {
    if MACHINE_LAID.load(Ordering::Acquire) {
        return;
    }
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        if crate::fs::unafs::mount_bound_handle().is_none() {
            return;
        }
        if MACHINE_LAID.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = lay_native(&store_mt(), "spawn"); // MACHINESTOREONELAY (rmbp-ledger B1167): the once-per-boot wire is said by the ONE lay (file tail). Named in-place hunk
    }
}

/// Lay `/settings` at kernel authority when absent and stamp it through `rootacl::stamp_store`; no I/O unless the folder
/// resolves to the native volume. `(made, stamped)`. `tests rootacl`'s MACHINESTORE boot step drives it (no latch).
pub(crate) fn lay_machine_store(mt: &crate::fs::vfs::MountTable) -> (bool, bool) {
    lay_native(mt, "tests") // MACHINESTOREONELAY (rmbp-ledger B1167): the body is the ONE lay behind B1162's native gate (file tail). Named in-place hunk
}

// ── MACHINESTOREONELAY (rmbp-ledger B1167, BP-storage-11) — TAIL-APPENDED ──────────────────────────────────────────────
// The no-session folder `/settings` has ONE lay, [`lay_machine_dir`]: create-if-absent at kernel authority, then
// `rootacl::stamp_store`. Three roads call it: `ensure_dirs` (the save road; `path()` is `<home>/settings` in a session,
// whose create stays in the caller, and the lay answers `None` with no I/O for any dir but `/settings`),
// `machine_mirror` (the session's machine copy) and the spawn road (`stamp_machine_store`, plus `tests rootacl`'s BOOT
// step through `lay_machine_store`), the last two behind [`lay_native`], B1162's native-volume gate. The lay itself is
// gate-free because a FAT root's `/settings` is still kernel-written (B1160 refuses only ring 3's SYS_OPEN there): the
// save and mirror roads must lay it on any volume. `load`'s stamp (B1156) stays stamp-only: a read never creates.
// MACHINE-STORE-PREB1150, measured and ACCEPTED: a ring-3 create carries `owner=<principal>` (fs/vfs.rs create) and is
// refused by `rootacl::store_file_refused`; the residual is a kernel-created row-less file a pre-B1150 image let ring 3
// rewrite in place. A write adds no row, native UnaFS keeps no mtime, and the `# auto-saved` header is content the
// writer chose, so nothing on disk tells it apart; a rewrite from the session stores needs a session the boot screen
// does not have. Bounded to volumes that ran a pre-B1150 image; the kernel's next swap of that domain replaces the file.
// No lock (create and stamp are idempotent); callers keep SAVE_LOCK where they hold it.
// Wire, once per boot from whichever road lays first:
// `[prefs] machine store dir=/settings owner=<system|none> made=<0|1> at=<spawn|save|mirror|tests>`.

/// The one-per-boot wire of [`lay_machine_dir`] was said.
static MACHINE_SAID: AtomicBool = AtomicBool::new(false);

/// B1162's spawn-road gate: lay the machine folder only when it resolves to the native volume. `(made, stamped)`.
fn lay_native(mt: &crate::fs::vfs::MountTable, at: &str) -> (bool, bool) {
    let dir = alloc::format!("/{}", prefs_core::files::DIR);
    if !mt.volume_name(&dir).map(|n| n == "native").unwrap_or(false) {
        return (false, false);
    }
    lay_machine_dir(mt, &dir, at).unwrap_or((false, false))
}

/// THE lay of the no-session folder: `None` (no I/O) unless `dir` is `/settings`; else create it at kernel authority when
/// absent and stamp it through `rootacl::stamp_store`. `Some((made, stamped))`; the first lay per boot says the wire.
fn lay_machine_dir(mt: &crate::fs::vfs::MountTable, dir: &str, at: &str) -> Option<(bool, bool)> {
    if dir.strip_prefix('/') != Some(prefs_core::files::DIR) {
        return None;
    }
    let made = mt.stat(dir).is_err() && mt.create(dir, crate::fs::vfs::NodeKind::Dir, crate::fs::vfs::KERNEL_PRINCIPAL).is_ok();
    let stamped = crate::fs::rootacl::stamp_store(mt, dir);
    if !MACHINE_SAID.swap(true, Ordering::AcqRel) {
        serial_println!(
            "[prefs] machine store dir={} owner={} made={} at={}",
            dir,
            if stamped { crate::fs::rootacl::SYSTEM_OWNER } else { "none" },
            made as u8,
            at
        );
    }
    Some((made, stamped))
}

// ── PREFS-UNSET-BUS (rmbp-ledger B1166, BP-storage-11) — TAIL-APPENDED ─────────────────────────────────────────────────
// The bus UNSET (una-abi `BUS_VERB_PREF_UNSET` 29): `prefs_core::wire::fulfil_as`'s UNSET arm (`wire::unset_as`: the GET
// address, SET's gate — -EACCES outside the session or past `cap::may_set` for the caller `bus_fulfil_from` stamped — then
// `Store::unset`) over `KernelStore`, whose arm is B1161's [`unset`]: the one rule (removed from the tree, its domain saved
// by the same swap, rolled back on a failed save) and the ONE `post_changes` of the unset form. Reply: 0 + SR169's unset
// form, -ENOENT already unset, -EINVAL malformed, -EIO the write. Principia's host store answers the same verb through the
// same `fulfil_as` (`PrefStore::unset`). No relay: R3PREF_UNSET is owed (PREFS-UNSET-RELAY). Wire, after B1161's
// `[prefs] unset <ns>.<key> ok=1` + `[prefs] changed <ns>.<key>`: `[prefs] unset <ns>.<key> ok=1 via=bus by=<prog>`,
// `ok=1 was=unset via=bus …`, `ok=0 why=<refused|malformed|io> via=bus …`.

/// `KernelStore::unset`: B1161's [`unset`] in the wire's failure words.
fn store_unset(ns: &str, k: &str) -> Result<bool, prefs_core::wire::SetFail> {
    unset(ns, k).map_err(|e| match e {
        SetError::Refused(r) => prefs_core::wire::SetFail::Refused(r),
        SetError::Name(n) => prefs_core::wire::SetFail::Name(n),
        SetError::Io(_) => prefs_core::wire::SetFail::Io,
    })
}

#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
/// [`bus_fulfil_from`]'s note: a ring-3 UNSET is said with its outcome and the stamped caller. Returns `st` unchanged.
fn unset_bus_note(verb: u8, body: &[u8], caller: prefs_core::cap::Caller<'_>, st: i64) -> i64 {
    if verb == prefs_core::wire::VERB_UNSET {
        let a = prefs_core::wire::parse_get(body).map(|_| core::str::from_utf8(body).unwrap_or("?")).unwrap_or("<malformed>");
        let who = prefs_caller_text(caller);
        if st == 0 {
            serial_println!("[prefs] unset {} ok=1 posted=unset via=bus by={}", a, who); // MACHINE-KEY-DEFAULT-PIN (rmbp-ledger B1173) + UNSET-POSTED-INLINE (rmbp-ledger B1189): the one post is the unset form, a literal. Named in-place hunk
        } else if st == prefs_core::wire::ENOENT {
            serial_println!("[prefs] unset {} ok=1 was=unset via=bus by={}", a, who);
        } else {
            let why = if st == prefs_core::wire::EACCES {
                "refused"
            } else if st == prefs_core::wire::EINVAL {
                "malformed"
            } else {
                "io"
            };
            serial_println!("[prefs] unset {} ok=0 why={} via=bus by={}", a, why, who);
        }
    }
    st
}

/// `tests prefs`' bus UNSET line (B1166): a scratch wm owner named `pcaptest` SETs then UNSETs its own
/// `app.pcaptest.level` through [`bus_fulfil_from`] (0 + the frozen unset form, the key gone), unsets it again
/// (-ENOENT), is refused `vein.endpoint` (-EACCES, kept) and an unset outside the session (-EACCES, kept); a bare
/// namespace is -EINVAL. The name, the tree, ON_DISK and `settings/pcaptest` are restored.
/// `:: PREFS-UNSET-BUS: unset=0 body=frozen again=enoent foreign=refused outside=refused name=einval restored=1 -> PASS ::`.
#[cfg(feature = "witness")]
fn unset_bus_selftest() {
    #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
    {
        unset_bus_legs();
    }
    #[cfg(not(any(feature = "aarch64_el0", target_arch = "x86_64")))]
    {
        serial_println!(":: PREFS-UNSET-BUS: SKIP (no ring-3 bus on this build) ::");
    }
}

#[cfg(all(feature = "witness", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn unset_bus_legs() {
    use prefs_core::wire::{EACCES, EINVAL, ENOENT, VERB_SET, VERB_UNSET};
    const OWNER: u64 = u64::MAX - 0x5F5; // beside smallfix4's SCRATCH: no launcher hands out an owner this high
    const KEY: &str = "pcaptest.level";
    let app = prefs_core::files::APP_NS;
    if let Some(k) = crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &path()) {
        serial_println!(":: PREFS-UNSET-BUS: dir={} -> SKIP reason=home-not-writable({}) ::", path(), k);
        return;
    }
    ensure_loaded();
    if get(app, KEY).is_some() || crate::video::wm::app_name_armed(OWNER) {
        serial_println!(":: PREFS-UNSET-BUS: SKIP (app.{} held or the scratch owner named) ::", KEY);
        return;
    }
    let dp = domain_path(&prefs_core::files::domain_of(app, KEY));
    let saved = read_all(&dp);
    let on_disk0 = ON_DISK.lock().clone();
    let _ = crate::video::wm::app_name_arm(OWNER, "/apps/PCAPTEST.ELF");
    let call = |verb: u8, body: &[u8], sess: bool, o: &mut Vec<u8>| -> i64 {
        o.clear();
        bus_fulfil_from(verb, body, sess, OWNER, o)
    };
    let mut o = Vec::new();
    let set = call(VERB_SET, b"app.pcaptest.level\x003", true, &mut o) == 0;
    let unset_st = call(VERB_UNSET, b"app.pcaptest.level", true, &mut o);
    let body = o == prefs_core::wire::unset_body(app, KEY) && o.as_slice() == b"app.pcaptest.level\x00" && get(app, KEY).is_none();
    let again = call(VERB_UNSET, b"app.pcaptest.level", true, &mut o) == ENOENT;
    let ep = get("vein", "endpoint");
    let foreign = call(VERB_UNSET, b"vein.endpoint", true, &mut o) == EACCES && get("vein", "endpoint") == ep;
    let outside = call(VERB_SET, b"app.pcaptest.level\x003", true, &mut o) == 0
        && call(VERB_UNSET, b"app.pcaptest.level", false, &mut o) == EACCES
        && get(app, KEY).is_some();
    let name = call(VERB_UNSET, b"app", true, &mut o) == EINVAL; machine_unset_leg(OWNER); // MACHINE-KEY-DEFAULT-PIN (rmbp-ledger B1173): the machine-key bus unset leg, while the scratch name is armed (file tail). LINE-NEUTRAL append, code first
    // Restore the name, the tree, ON_DISK and the domain file.
    crate::video::wm::app_name_forget(OWNER);
    let _ = TREE.lock().remove(app, KEY);
    let mt = crate::shell::vfs_mount_table();
    match &saved {
        Some(b) => {
            let _ = write_all(&dp, b);
        }
        None => {
            let _ = mt.unlink(&dp, crate::fs::vfs::KERNEL_PRINCIPAL);
        }
    }
    *ON_DISK.lock() = on_disk0;
    let restored = read_all(&dp) == saved && get(app, KEY).is_none() && !crate::video::wm::app_name_armed(OWNER);
    let ok = set && unset_st == 0 && body && again && foreign && outside && name && restored;
    serial_println!(
        ":: PREFS-UNSET-BUS: unset={} body={} again={} foreign={} outside={} name={} restored={} -> {} ::",
        unset_st, if body { "frozen" } else { "MOVED" }, if again { "enoent" } else { "FAIL" }, if foreign { "refused" } else { "UNSET" },
        if outside { "refused" } else { "UNSET" }, if name { "einval" } else { "FAIL" }, restored as u8, if ok { "PASS" } else { "FAIL" }
    );
}

// =================================================================================================
// PREFSCAP-PATH (rmbp-ledger B1170) — TAIL-APPENDED. PREFSCAP named a ring-3 caller from its launcher-armed NAME
// only (`caller_name`), so any image called PREFS.ELF was `prefs`, Principia's fulfiller, and `prefs_core::cap`
// let it SET every key (vein.endpoint, system.login.items) through this fulfiller. B1163 (BUSREG-TAG-PATH) gave
// wm the PATH each launcher loaded the image from (`wm::app_path_with`) and `bus_route` the rule that judges it
// (`path_names` against `TAG_IMAGE`). [`bus_fulfil_from`] now asks it ONCE: a caller stamped `prefs` keeps
// Principia's rights only when its path is `TAG_IMAGE`'s Principia image (`/apps/` byte-exact, the leaf any
// case); from any other path, or with no path (the spelling never promotes), it is `Caller::Anon` — an
// impostor, not a program owed a namespace: the schema's ring-3 rows, no declare. `prefs_core` stays path-free
// and untouched. Knob-off (no `busreg`): no path record exists, [`prefscap_path`] is the identity.
// PREFSCAP-NS-PATH (rmbp-ledger B1177): the same verdict, generalised in place, binds EVERY stamped name to its image
// `/apps/<NAME>.ELF` (a namespace's owner is named only by the caller's name: `cap`'s stanza, owner and declare arms),
// so an image called VEIN.ELF from `/home` is `Anon` and may not SET `vein.*`; `prefs` is the general case.
// Witness: `tests prefs` → `[prefs] set vein.endpoint ok=0 why=refused via=bus by=prefs path=foreign` and
// `:: PREFSCAP-PATH: … -> PASS ::`.
// =================================================================================================

/// PREFSCAP-PATH — `c` as PREFSCAP may see it, and the stamped name it was demoted from (`None` = kept). Takes only
/// wm's leaf `APP_PATHS` (then, with no late path, `APP_NAMES_EARLY`; `bus_route::path_names` takes no lock).
/// PREFSCAP-NS-PATH (rmbp-ledger B1177): EVERY program, not only `prefs`, is its name only when its path is
/// `/apps/<NAME>.ELF` (B1163's rule; `prefs` is the general case, its image TAG_IMAGE's `/apps/PREFS.ELF`), else `Anon`.
#[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn prefscap_path<'a>(owner: u64, c: prefs_core::cap::Caller<'a>) -> (prefs_core::cap::Caller<'a>, Option<&'a str>) {
    match c {
        prefs_core::cap::Caller::Program(p) => {
            let img = alloc::format!("/apps/{}.ELF", p); // built before any leaf lock is taken
            let bound = crate::video::wm::app_path_with(owner, |path| crate::bus_route::path_names(path, &img)).or_else(|| prefscap_early(owner, &img)) // PREFSCAP-ARM-EARLY (rmbp-ledger B1176): no late path -> wm's early record at the caller row's current generation, B1164's two-source rule as `tagpath_id` (file tail); rides PREFSCAP-NS-PATH's (B1177) per-name image. LINE-NEUTRAL fold, code first
                == Some(true);
            if bound {
                (c, None)
            } else {
                (prefs_core::cap::Caller::Anon, Some(p))
            }
        }
        _ => (c, None),
    }
}

/// PREFSCAP-PATH knob-off: no path record, nothing demoted (today's caller, unchanged).
#[cfg(all(not(feature = "busreg"), any(feature = "aarch64_el0", target_arch = "x86_64")))]
#[inline(always)]
fn prefscap_path<'a>(_owner: u64, c: prefs_core::cap::Caller<'a>) -> (prefs_core::cap::Caller<'a>, Option<&'a str>) {
    (c, None)
}

/// PREFSCAP-PATH — a DEMOTED caller's SET is said with its stamped name (any program since B1177) and `path=foreign` (B1166's why words);
/// every other verb or caller passes `st` through silently.
#[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))]
#[inline(always)]
fn prefscap_set_note(verb: u8, body: &[u8], demoted: Option<&str>, st: i64) -> i64 {
    if let (Some(by), true) = (demoted, verb == prefs_core::wire::VERB_SET) {
        let ok = (st == 0) as u8;
        let why = match st {
            0 => "",
            prefs_core::wire::EACCES => " why=refused",
            prefs_core::wire::EINVAL => " why=malformed",
            _ => " why=io",
        };
        match prefs_core::wire::parse_set(body) {
            Some((ns, k, _)) => {
                serial_println!("[prefs] set {}.{} ok={}{} via=bus by={} path=foreign", ns, k, ok, why, by)
            }
            None => serial_println!("[prefs] set <malformed> ok={}{} via=bus by={} path=foreign", ok, why, by),
        }
    }
    st
}

/// `tests prefs`' PREFSCAP-PATH line (B1170): a scratch wm owner named PREFS from `/home/una/PREFS.ELF` SETs
/// `vein.endpoint` through [`bus_fulfil_from`] (-EACCES, the value kept) and is demoted to `Anon` (the schema's
/// ring-3 rows only, no declare: pure cap checks, nothing written); `/apps/PREFS.ELF` and `/apps/prefs.elf` keep
/// Principia, `/APPS/PREFS.ELF` and a pathless `PREFS.ELF` are demoted, a non-prefs name is never touched. The
/// scratch name and path are forgotten.
/// `:: PREFSCAP-PATH: foreign=refused anon=held apps=admitted case=admitted upper=demoted pathless=demoted early=admitted stale=demoted earlyname=admitted other=demoted ns=refused nsown=admitted restored=1 -> PASS ::`
/// (`early=` / `stale=`: PREFSCAP-ARM-EARLY B1176, [`prefscap_early_legs`]; `earlyname=`: PREFSCAP-EARLY-NAME B1182, [`prefscap_earlyname_leg`]; `other=demoted` / `ns=` / `nsown=`: PREFSCAP-NS-PATH
/// B1177 — a `/home/una/LUMEN.ELF` is demoted, a `/home/una/VEIN.ELF` SET of `vein.endpoint` is -EACCES with the value kept
/// (`[prefs] set vein.endpoint ok=0 why=refused via=bus by=vein path=foreign`), `/apps/VEIN.ELF` keeps `vein.*` and only it).
#[cfg(feature = "witness")]
fn prefscap_path_selftest() {
    #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
    {
        prefscap_path_legs();
    }
    #[cfg(not(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64"))))]
    {
        serial_println!(":: PREFSCAP-PATH: SKIP (no busreg path record on this build) ::");
    }
}

#[cfg(all(feature = "witness", feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn prefscap_path_legs() {
    use prefs_core::cap::{may_declare, may_set, Caller};
    const OWNER: u64 = u64::MAX - 0x5F6; // beside B1166's 0x5F5: no launcher hands out an owner this high
    ensure_loaded();
    if crate::video::wm::app_name_armed(OWNER) {
        serial_println!(":: PREFSCAP-PATH: SKIP (the scratch owner is named) ::");
        return;
    }
    // Arm the scratch owner from `path`; `Some(demoted)` for the stamped caller, `None` = nothing armed.
    let demoted = |path: &str| -> Option<bool> {
        let _ = crate::video::wm::app_name_arm(OWNER, path);
        let mut nb = [0u8; crate::video::wm::MAX_TITLE];
        caller_name(OWNER, &mut nb).map(|p| {
            let (c, d) = prefscap_path(OWNER, Caller::Program(p));
            d.is_some() == (c == Caller::Anon) && d.is_some()
        })
    };
    // L1 + L2: the foreign PREFS — demoted, its SET refused on the wire, the value kept; Anon's rows only.
    let foreign_demoted = demoted("/home/una/PREFS.ELF") == Some(true);
    let ep = get("vein", "endpoint");
    let mut o = Vec::new();
    let st = bus_fulfil_from(prefs_core::wire::VERB_SET, b"vein.endpoint\x00\"https://foreign.example/\"", true, OWNER, &mut o);
    let foreign = foreign_demoted && st == prefs_core::wire::EACCES && get("vein", "endpoint") == ep;
    let a = Caller::Anon;
    let anon = foreign_demoted
        && !may_set(a, "system", "login.items")
        && !may_set(a, "app", "prefs.x")
        && may_set(a, "system", "audio.volume")
        && !may_declare(a, "prefs");
    // L3: the Principia image keeps its rights; the directory is byte-exact, the leaf any case.
    let apps = demoted("/apps/PREFS.ELF") == Some(false);
    let case = demoted("/apps/prefs.elf") == Some(false);
    let upper = demoted("/APPS/PREFS.ELF") == Some(true);
    // L4: a relative arm clears the path row: the spelling alone never promotes.
    let pathless = demoted("PREFS.ELF") == Some(true);
    let (early, stale, early_gone) = prefscap_early_legs(OWNER, &demoted); // PREFSCAP-ARM-EARLY (rmbp-ledger B1176): the early record admits a pathless PREFS.ELF at the current generation only (file tail)
    // L5 (PREFSCAP-NS-PATH B1177): every program is its name only from `/apps/<NAME>.ELF`: a non-prefs image from
    // `/home` is demoted; VEIN from `/home` is refused its own namespace on the wire; `/apps/VEIN.ELF` owns `vein.*`.
    let other = demoted("/home/una/LUMEN.ELF") == Some(true);
    let ns_demoted = demoted("/home/una/VEIN.ELF") == Some(true);
    let ep = get("vein", "endpoint");
    o.clear();
    let st = bus_fulfil_from(prefs_core::wire::VERB_SET, b"vein.endpoint\x00\"https://foreign.example/\"", true, OWNER, &mut o);
    let ns = ns_demoted && st == prefs_core::wire::EACCES && get("vein", "endpoint") == ep;
    let nsown = demoted("/apps/VEIN.ELF") == Some(false) && {
        let mut nb = [0u8; crate::video::wm::MAX_TITLE];
        caller_name(OWNER, &mut nb).is_some_and(|p| {
            let (c, _) = prefscap_path(OWNER, Caller::Program(p));
            c == Caller::Program("vein") && may_set(c, "vein", "x") && !may_set(c, "system", "login.items")
        })
    };
    crate::video::wm::app_name_forget(OWNER); let earlyname = prefscap_earlyname_leg(OWNER); // PREFSCAP-EARLY-NAME (rmbp-ledger B1182): no late name, the early record names the caller `prefs` (file tail). Named in-place hunk
    let restored = early_gone && !crate::video::wm::app_name_armed(OWNER) && crate::video::wm::app_path_with(OWNER, |_| true).is_none();
    let ok = foreign && anon && apps && case && upper && pathless && early && stale && earlyname && other && ns && nsown && restored; // PREFSCAP-EARLY-NAME (rmbp-ledger B1182): `earlyname` (named in-place hunk)
    serial_println!(
        ":: PREFSCAP-PATH: foreign={} anon={} apps={} case={} upper={} pathless={} early={} stale={} earlyname={} other={} ns={} nsown={} restored={} -> {} ::",
        if foreign { "refused" } else { "ADMITTED" }, if anon { "held" } else { "FAIL" }, if apps { "admitted" } else { "DEMOTED" },
        if case { "admitted" } else { "DEMOTED" }, if upper { "demoted" } else { "ADMITTED" }, if pathless { "demoted" } else { "ADMITTED" },
        if early { "admitted" } else { "DEMOTED" }, if stale { "demoted" } else { "ADMITTED" }, if earlyname { "admitted" } else { "DEMOTED" },
        if other { "demoted" } else { "KEPT" }, if ns { "refused" } else { "ADMITTED" }, if nsown { "admitted" } else { "DEMOTED" }, restored as u8, if ok { "PASS" } else { "FAIL" }
    );
}

// ── MACHINE-KEY-DEFAULT-PIN (rmbp-ledger B1173, BP-storage-11) — TAIL-APPENDED ─────────────────────────────────────────
// What [`unset`]'s ONE post carries. A key in [`MACHINE_KEYS`] posts SR169's unset form (`None`): the store no longer
// holds it (GET answers -ENOENT), Principia's host store posts the same form for the same verb, every consumer re-reads
// (the kernel's `changes_drain`, ring 3's theme HINT) or falls back per key (`settings::a11y_load("machine")`) and takes
// the static default from `prefs_core::schema` (B1160 pins it static), and no consumer reads the frame's value; so
// `machine_mirror` removes the key from the machine copy by B1154's None arm instead of writing the default literal.
// UNSET-POST-ROADS (B1179): ONE rule on the three roads ([`unset`], the delete watch's [`deleted_post`], the reload's
// [`reload_post`]), and a STATIC-default key (`schema::default_of` -> `Source::Default`) posts the unset form too, as the
// host ring does. UNSET-POST-RULE (B1185): so do a Rule-default key (vein's provider rebuild re-reads on the namespace) and a declared
// `app.*` key (no reader binds the value; the declarer holds its default): EVERY unset posts the unset form, [`default_applied`] answers reads only. Wire: `[prefs] machine settings/<d> system.<k>=unset wrote=1`, `[prefs] unset <a> ok=1 posted=unset via=bus …`.
// UNSET-POSTED-INLINE (rmbp-ledger B1189): the constant `unset_posted` and its `posted=` word fn are retired; [`unset`], [`deleted_post`] and [`reload_post`] post `None` on their own lines and [`unset_bus_note`] says `posted=unset` as a literal.

/// `tests prefs`' machine-key bus unset leg (B1173), from [`unset_bus_legs`] while its scratch owner is armed: SET then
/// UNSET `system.accessibility.reduce_motion` (a ring-3 settable row) through [`bus_fulfil_from`]; the post is the unset
/// form, the key is gone, and (in a session) the machine copy no longer holds it. The session domain file, the machine
/// file, the tree and ON_DISK are restored byte for byte.
/// `:: MACHINE-KEY-UNSET: set=1 unset=0 posted=unset roads=3 rule=unset app=unset mirror=<removed|nosession> restored=1 -> PASS ::`.
#[cfg(all(feature = "witness", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn machine_unset_leg(owner: u64) {
    use prefs_core::wire::{VERB_SET, VERB_UNSET};
    const K: &str = "accessibility.reduce_motion";
    let d = prefs_core::files::domain_of(NS, K);
    let sp = domain_path(&d);
    let mp = alloc::format!("/{}/{}", prefs_core::files::DIR, d);
    let (s0, m0) = (read_all(&sp), read_all(&mp));
    let (tree0, on_disk0) = (TREE.lock().clone(), ON_DISK.lock().clone());
    let mut o = Vec::new();
    let set = bus_fulfil_from(VERB_SET, b"system.accessibility.reduce_motion\x00true", true, owner, &mut o) == 0;
    o.clear();
    let st = bus_fulfil_from(VERB_UNSET, b"system.accessibility.reduce_motion", true, owner, &mut o);
    let posted = get(NS, K).is_none(); // UNSET-POSTED-INLINE (rmbp-ledger B1189): the post is the literal unset form; its effect is `mirror=removed` and `roads=3`. Named in-place hunk
    let session = home().is_some_and(|h| !h.is_empty());
    let held = read_all(&mp).and_then(|b| String::from_utf8(b).ok()).and_then(|t| PrefTree::parse(&t).ok()).is_some_and(|t| t.get(NS, K).is_some());
    let mirror = if !session {
        "nosession"
    } else if held {
        "KEPT"
    } else {
        "removed"
    };
    let (del, rel) = unset_roads_leg(&sp, &mp, session); let roads = (posted && mirror != "KEPT") as u8 + del as u8 + rel as u8; let (rule, app) = unset_rule_leg(); // UNSET-POST-ROADS (rmbp-ledger B1179): the delete and reload roads, before the restore below (file tail). Named in-place hunk UNSET-POST-RULE (rmbp-ledger B1185): the Rule-default and declared app.* legs (file tail).
    // Restore the files, the tree and ON_DISK.
    let mt = crate::shell::vfs_mount_table();
    for (p, b) in [(&sp, &s0), (&mp, &m0)] {
        match b {
            Some(b) => {
                let _ = write_all(p, b);
            }
            None => {
                let _ = mt.unlink(p, crate::fs::vfs::KERNEL_PRINCIPAL);
            }
        }
    }
    *TREE.lock() = tree0;
    *ON_DISK.lock() = on_disk0;
    let restored = read_all(&sp) == s0 && read_all(&mp) == m0;
    let ok = set && st == 0 && posted && mirror != "KEPT" && restored && roads == 3 && rule && app; // UNSET-POST-ROADS (rmbp-ledger B1179): `roads=` (named in-place hunk). UNSET-POST-RULE (rmbp-ledger B1185): `rule=` `app=`
    serial_println!(
        ":: MACHINE-KEY-UNSET: set={} unset={} posted={} roads={} rule={} app={} mirror={} restored={} -> {} ::",
        set as u8, st, if posted { "unset" } else { "VALUE" }, roads, if rule { "unset" } else { "VALUE" }, if app { "unset" } else { "VALUE" }, mirror, restored as u8, if ok { "PASS" } else { "FAIL" }
    );
}

// =================================================================================================
// PREFSCAP-ARM-EARLY (rmbp-ledger B1176) — TAIL-APPENDED, `busreg` only (knob-off: no symbol, no byte). B1170's
// [`prefscap_path`] read only the LATE path (`wm::app_path_with`), so the real PREFS.ELF was demoted while it had a
// name and no path: `wm::app_name_arm_launch` arms the name through `app_name_arm` (which clears the path, the name
// being bare) and arms the path after it, and a relative late arm leaves that state for good. B1164 keeps the EARLY
// record (`wm::app_path_early_of`: the arch spawn body's absolute argv[0] or SYS_SPAWN_PATH's kernel-opened path,
// stamped with the row's bus generation at the mint). The verdict now reads the late path OR ELSE the early record at
// the caller row's CURRENT generation: B1164's two-source rule, `bus_route::tagpath_id`'s order (a late path decides,
// foreign stays foreign), so the cap verdict is the fulfiller verdict. The row is the owner's through
// `wm::owner_of_launch`'s own offset (x86 slot + 1, aarch64 the asid), never an arch cfg here. Locks: wm's two leaves,
// one at a time (`app_path_with`'s guard is gone before `or_else` runs); `rgen` is an atomic load.
// Witness: `tests prefs` → `:: PREFSCAP-PATH: … pathless=demoted early=admitted stale=demoted … -> PASS ::`.
// =================================================================================================

/// PREFSCAP-ARM-EARLY — the bus generation of wm `owner`'s row now (`owner_of_launch(row) == owner`); `None` = an
/// owner below every row's (x86 owner 0).
#[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn prefscap_gen(owner: u64) -> Option<u64> {
    let row = owner.checked_sub(crate::video::wm::owner_of_launch(0))?;
    Some((crate::arch::syscall::busreg_ops().rgen)(row as usize))
}

/// PREFSCAP-ARM-EARLY — is `owner`'s early path, at its row's current generation, the image `img`? `None` = no
/// absolute early record at that generation (the verdict stays demoted). Takes only wm's leaf `APP_NAMES_EARLY`.
#[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn prefscap_early(owner: u64, img: &str) -> Option<bool> {
    crate::video::wm::app_path_early_of(owner, prefscap_gen(owner)?, |p| crate::bus_route::path_names(p, img))
}

/// `prefscap_path_legs`' early legs on the scratch `owner` (name relative, so no late path): `/apps/PREFS.ELF`
/// early-armed at [`prefscap_gen`] is admitted; the same record at the next generation (a dead tenant's) is demoted;
/// the record is then forgotten. `(early, stale, gone)`.
#[cfg(all(feature = "witness", feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn prefscap_early_legs(owner: u64, demoted: &dyn Fn(&str) -> Option<bool>) -> (bool, bool, bool) {
    let Some(g) = prefscap_gen(owner) else { return (false, false, false) };
    let _ = crate::video::wm::app_name_arm_early(owner, g, "/apps/PREFS.ELF");
    let early = demoted("PREFS.ELF") == Some(false);
    let _ = crate::video::wm::app_name_arm_early(owner, g.wrapping_add(1), "/apps/PREFS.ELF");
    let stale = demoted("PREFS.ELF") == Some(true);
    crate::video::wm::app_name_early_forget(owner);
    let gone = crate::video::wm::app_path_early_of(owner, g, |_| true).is_none()
        && crate::video::wm::app_path_early_of(owner, g.wrapping_add(1), |_| true).is_none();
    (early, stale, gone)
}

// ── UNSET-POST-ROADS (rmbp-ledger B1179, BP-storage-11) — TAIL-APPENDED ────────────────────────────────────────────────
// [`machine_unset_leg`]'s second and third roads for `system.accessibility.reduce_motion`, run before that leg's restore
// (which puts the session domain file, the machine file, the tree and ON_DISK back byte for byte). The delete road: the
// key is set, the session domain file holding it is deleted, [`watch_deleted`] clears the domain and [`deleted_post`]
// posts SR169's unset form (`None`), so the machine copy loses the key. The reload road: the key is set, the store is
// saved without it while RAM still holds it, [`reload_session`] re-reads the store and [`reload_post`] posts the unset
// form for the key that left. Each road passes when the key is gone and (in a session) the machine copy no longer holds it.
// Wire: `[prefs] machine settings/general system.accessibility.reduce_motion=unset wrote=1` once per road, then
// `[prefs] deleted -> changed … unset=<n>` / `[prefs] reload … changed=1 sent=1 unsent=0 unset=1 …`.

/// The delete and reload roads (see the block above): `(delete_ok, reload_ok)`. `sp` = the session domain file, `mp` = the
/// machine copy's file, `session` = a session home is mounted.
#[cfg(all(feature = "witness", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn unset_roads_leg(sp: &str, mp: &str, session: bool) -> (bool, bool) {
    const K: &str = "accessibility.reduce_motion";
    let d = prefs_core::files::domain_of(NS, K);
    let kept = || read_all(mp).and_then(|b| String::from_utf8(b).ok()).and_then(|t| PrefTree::parse(&t).ok()).is_some_and(|t| t.get(NS, K).is_some());
    let mt = crate::shell::vfs_mount_table();
    // Road 2 — the delete watch: the session domain file holding the key is deleted.
    let set2 = set(NS, K, PrefValue::Bool(true)).is_ok();
    let _ = mt.unlink(sp, crate::fs::vfs::KERNEL_PRINCIPAL);
    let _ = watch_deleted();
    let del = set2 && !ON_DISK.lock().iter().any(|x| *x == d) && get(NS, K).is_none() && !(session && kept());
    // Road 3 — the reload: RAM holds the key, the store does not.
    let set3 = set(NS, K, PrefValue::Bool(true)).is_ok();
    let old = TREE.lock().clone();
    let _ = TREE.lock().remove(NS, K);
    let saved = save_domain(&d).is_ok();
    let _ = reload_session(&old);
    let rel = set3 && saved && get(NS, K).is_none() && !(session && kept());
    (del, rel)
}

// =================================================================================================
// PREFSCAP-EARLY-NAME (rmbp-ledger B1182) — TAIL-APPENDED, `busreg` only (knob-off: no symbol, no byte). B1176 let
// [`prefscap_path`] read wm's early PATH record, but [`caller_name`] named the caller from `wm::app_name_of` only, and
// B1164 keeps the early record invisible to it by design (titles and NOTICE must not read it). So a ring-3
// SYS_SPAWN_PATH launch of `/apps/PREFS.ELF` (early-armed at the mint, B1172) that REGISTERs and SETs before its
// launcher's late arm, or with no late arm at all, was `Anon` before the path was asked. [`caller_name`]'s callers are
// the cap road only ([`bus_fulfil_from`] and the PREFSCAP-PATH witness), so its ONE fold falls back to
// `wm::app_name_early_of` at the row's CURRENT generation ([`prefscap_gen`], B1176's inverse, reused) when no late name
// answers: `bus_route::row_name`'s own early fallback (`armearly_name`), mirrored. A late name always wins; a row with
// neither record stays `Anon`; an early name from a non-`/apps` or relative argv[0] is named, then demoted by
// [`prefscap_path`] (`path=foreign`); a record from another generation answers nothing. Locks: wm's leaf `APP_NAMES`
// is dropped before `rgen` (an atomic load) and the leaf `APP_NAMES_EARLY`, taken alone; no prefs lock is held.
// Witness: `tests prefs` → `:: PREFSCAP-PATH: … stale=demoted earlyname=admitted … -> PASS ::`.
// =================================================================================================

/// PREFSCAP-EARLY-NAME — `n` when a launcher-armed name exists, else the length of `owner`'s early program name at its
/// row's current generation, copied into `buf` (0 = no record at that generation, or an owner below every row's).
#[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn prefscap_early_name(owner: u64, n: usize, buf: &mut [u8; crate::video::wm::MAX_TITLE]) -> usize {
    if n != 0 {
        return n;
    }
    prefscap_gen(owner).map_or(0, |g| crate::video::wm::app_name_early_of(owner, g, buf))
}

/// `prefscap_path_legs`' early-name leg on the scratch `owner` (no late name armed): `/apps/PREFS.ELF` early-armed at
/// [`prefscap_gen`] makes [`caller_name`] `prefs`, [`prefscap_path`] keeps it (nothing demoted) and it may set
/// `app.prefs.x` and declare `prefs` (pure cap checks, nothing written); the record forgotten, the row has neither
/// record and [`caller_name`] answers none (`Anon`).
#[cfg(all(feature = "witness", feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn prefscap_earlyname_leg(owner: u64) -> bool {
    use prefs_core::cap::{may_declare, may_set, Caller};
    let Some(g) = prefscap_gen(owner) else { return false };
    if crate::video::wm::app_name_armed(owner) {
        return false;
    }
    let _ = crate::video::wm::app_name_arm_early(owner, g, "/apps/PREFS.ELF");
    let admitted = {
        let mut nb = [0u8; crate::video::wm::MAX_TITLE];
        caller_name(owner, &mut nb).is_some_and(|p| {
            let (c, d) = prefscap_path(owner, Caller::Program(p));
            p == "prefs" && d.is_none() && c == Caller::Program("prefs") && may_set(c, "app", "prefs.x") && may_declare(c, "prefs")
        })
    };
    crate::video::wm::app_name_early_forget(owner);
    let anon = {
        let mut nb = [0u8; crate::video::wm::MAX_TITLE];
        caller_name(owner, &mut nb).is_none()
    };
    let gone = crate::video::wm::app_path_early_of(owner, g, |_| true).is_none();
    admitted && anon && gone
}

// ── UNSET-POST-RULE (rmbp-ledger B1185, BP-storage-11) — TAIL-APPENDED ─────────────────────────────────────────────────
// [`machine_unset_leg`]'s `rule=` and `app=` words. A Rule-default key (`vein.embed.provider`: `schema::default_of` answers
// `Source::Rule`, and under the kernel's [`StoreEnv`] the embedder rule always yields a value) and a declared `app.*` key
// (a scratch `app.unsetrule.k`, declared through [`declare_kernel`] only when no `unsetrule` stanza is held, removed after)
// each have a literal [`default_applied`] would answer (the premise: the roads' unset form is posted over a value, not for lack of one; UNSET-POSTED-INLINE B1189 retired `unset_posted`, the post is the literal `None` on each road's line). No store write,
// no tree change; TREE and DECLARED are taken one at a time, with no prefs lock held by the caller.

/// The Rule-default and declared-app legs (see the block above): `(rule_ok, app_ok)`.
#[cfg(all(feature = "witness", any(feature = "aarch64_el0", target_arch = "x86_64")))]
fn unset_rule_leg() -> (bool, bool) {
    const RK: &str = "embed.provider";
    let rule = matches!(prefs_core::schema::default_of("vein", RK, &StoreEnv), Some((_, prefs_core::schema::Source::Rule(_))))
        && default_applied("vein", RK).is_some(); // UNSET-POSTED-INLINE (rmbp-ledger B1189): the premise only, the post is the sites' literal. Named in-place hunk
    let held = DECLARED.lock().contains_key("unsetrule");
    let declared = !held && declare_kernel(b"unsetrule\x00k\tint:0:9\t4\tUNSET-POST-RULE scratch");
    let app = declared
        && default_applied(prefs_core::files::APP_NS, "unsetrule.k").map(|a| a.value) == Some(PrefValue::Int(4)); // UNSET-POSTED-INLINE (rmbp-ledger B1189): the premise only. Named in-place hunk
    if declared {
        DECLARED.lock().remove("unsetrule");
    }
    (rule, app)
}
