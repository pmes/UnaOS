//! CHARTER: Kernel — wm
//!
//! NOTIFYPANE (rmbp-ledger B435, MACPARITY rows 24/26) — NOTIFY's per-app rules and Do Not Disturb's schedule,
//! as the kernel's CACHED CELL of Principia's keys (R79: a preference is Principia's; the rules are
//! `prefs_core::notify`, both rings). The cell holds the apps NOTIFY has seen (each post records its app) and
//! every stored stanza `system.notify.<app>.{allow,style,sound}`, loaded ONCE at login on NOTIFY's service pass.
//! Settings > Notifications changes a rule LIVE here and latches ONE store write per changed field, drained by
//! [`service`] (never a bus call in the click router). NOTIFY's pass reads the cell with `try_lock` at post time
//! ([`rule_at_post`], [`dnd_now`]) — no bus, no heap on the read.
//!
//! The alert sound: one per NOTIFY pass (a post, or a dialog's alert latched by [`alert`]), asked of the hda tone path ([`sound_for`] →
//! `drivers::hda::play::request_alert`), refused while the output is already sounding.
//!
//! Wire: `[notifypane] loaded apps=<n> dnd=<0|1> dnd_window=<f>-<u> via=login`, `[notifypane] set app=<a>
//! <field>=<v> applied=1`, `[notifypane] saved <key> ok=<0|1>`, `[notifypane] window <f>-<u> applied=1`,
//! `[notifypane] sound app=<a> -> requested|busy|nopath|fixture`, and `tests notifypane` →
//! `:: NOTIFYPANE: apps=<n> allow=<n> sound=<ok|none|busy> dnd_window=<f>-<u> -> PASS ::`.
//! Design: `docs/dev/evidence/rmbp-1005/notifypane.md`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use prefs_core::notify::{self as pn, Rule};
use prefs_core::PrefValue;

/// Apps the cell holds (the pane lists them; a post from an app beyond this reads the default rule).
pub const APPS: usize = 24;
/// Principia's schedule keys (namespace `system`).
pub const KEY_DND_FROM: &str = "notify.dnd_from";
pub const KEY_DND_UNTIL: &str = "notify.dnd_until";

#[derive(Clone, Copy)]
struct Ent {
    name: [u8; pn::APP_MAX],
    nl: u8,
    rule: Rule,
    /// Store writes owed: bit f = `pn::FIELDS[f]`.
    owed: u8,
}

impl Ent {
    const EMPTY: Ent = Ent { name: [0; pn::APP_MAX], nl: 0, rule: pn::DEFAULT, owed: 0 };
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.nl as usize]).unwrap_or("?")
    }
}

struct Cell {
    e: [Ent; APPS],
    n: usize,
}

static CELL: spin::Mutex<Cell> = spin::Mutex::new(Cell { e: [Ent::EMPTY; APPS], n: 0 });
static LOADED: AtomicBool = AtomicBool::new(false);
static FROM: AtomicU32 = AtomicU32::new(0);
static UNTIL: AtomicU32 = AtomicU32::new(0);
static WIN_OWED: AtomicBool = AtomicBool::new(false);
/// The pane's rows changed (a new app was seen, the store loaded): Settings repaints on its pass.
static STALE: AtomicBool = AtomicBool::new(false);
/// NOTIFY's fixtures hold the real rules out (`tests notify` must not read the user's choices).
static BYPASS_RULES: AtomicBool = AtomicBool::new(false);
static BYPASS_WINDOW: AtomicBool = AtomicBool::new(false);
static SOUNDS: AtomicU32 = AtomicU32::new(0);

/// A name the cell can key (one Principia key segment, not `dnd*`).
fn keyable(app: &[u8]) -> Option<&str> {
    let s = core::str::from_utf8(app).ok()?;
    (!s.is_empty() && s.len() <= pn::APP_MAX && prefs_core::valid_segment(s) && !pn::RESERVED.contains(&s)).then_some(s)
}

fn find(c: &Cell, app: &str) -> Option<usize> {
    c.e[..c.n].iter().position(|e| e.name() == app)
}

fn insert(c: &mut Cell, app: &str) -> Option<usize> {
    if let Some(i) = find(c, app) {
        return Some(i);
    }
    if c.n >= APPS {
        return None;
    }
    let i = c.n;
    c.e[i] = Ent::EMPTY;
    c.e[i].name[..app.len()].copy_from_slice(app.as_bytes());
    c.e[i].nl = app.len() as u8;
    c.n += 1;
    STALE.store(true, Ordering::Relaxed);
    Some(i)
}

/// NOTIFY's post-time read (its service pass): the app's rule, recording the app as seen. Contended or full =
/// the default rule (allowed, banner, sound).
pub fn rule_at_post(app: &[u8]) -> Rule {
    if BYPASS_RULES.load(Ordering::Relaxed) {
        return pn::DEFAULT;
    }
    let Some(a) = keyable(app) else { return pn::DEFAULT };
    let Some(mut c) = CELL.try_lock() else { return pn::DEFAULT };
    match insert(&mut c, a) {
        Some(i) => c.e[i].rule,
        None => pn::DEFAULT,
    }
}

/// The schedule `(from, until)` in local hours (equal = none).
pub fn window() -> (u32, u32) {
    (FROM.load(Ordering::Relaxed), UNTIL.load(Ordering::Relaxed))
}

/// The schedule is on and the local hour is inside it.
pub fn in_window_now() -> bool {
    if BYPASS_WINDOW.load(Ordering::Relaxed) {
        return false;
    }
    let (f, u) = window();
    f != u && super::appearance::local_hour().is_some_and(|h| pn::in_window(h, f, u))
}

/// Do Not Disturb now: the manual switch or the schedule.
pub fn dnd_now() -> bool {
    super::notify::dnd() || in_window_now()
}

/// The pane's rows: `(app, rule)` in first-seen order.
pub fn rows() -> Vec<(String, Rule)> {
    let c = CELL.lock();
    c.e[..c.n].iter().map(|e| (String::from(e.name()), e.rule)).collect()
}

/// Settings' change: flip field `f` (`pn::FIELDS`) of row `i` LIVE, latch its store write.
pub fn toggle(i: usize, f: usize) {
    let mut c = CELL.lock();
    if i >= c.n || f >= pn::FIELDS.len() {
        return;
    }
    let e = &mut c.e[i];
    match f {
        0 => e.rule.allow = !e.rule.allow,
        1 => e.rule.center = !e.rule.center,
        _ => e.rule.sound = !e.rule.sound,
    }
    e.owed |= 1 << f;
    serial_println!("[notifypane] set app={} {}={} applied=1", e.name(), pn::FIELDS[f], pn::value(&e.rule, f).to_literal());
}

/// Settings' schedule step: `which` 0 = from, 1 = until; `d` = ±1 hour (wrapping). LIVE, one store write latched.
pub fn step_window(which: usize, d: i32) {
    let a = if which == 0 { &FROM } else { &UNTIL };
    let v = (a.load(Ordering::Relaxed) as i32 + d).rem_euclid(24) as u32;
    a.store(v, Ordering::Relaxed);
    WIN_OWED.store(true, Ordering::Release);
    let (f, u) = window();
    serial_println!("[notifypane] window {}-{} applied=1 on={}", f, u, (f != u) as u8);
}

/// Settings asks: did the rows change since its last paint?
pub fn take_stale() -> bool {
    STALE.swap(false, Ordering::Relaxed)
}

/// NOTIFY asks for the alert sound for `app`'s post (at most once per its pass). `headless` = a fixture: counted,
/// never played.
pub fn sound_for(app: &[u8], headless: bool) -> &'static str {
    SOUNDS.fetch_add(1, Ordering::Relaxed);
    let r = if headless { "fixture" } else { request_tone() };
    serial_println!("[notifypane] sound app={} -> {}", core::str::from_utf8(app).unwrap_or("?"), r);
    r
}

fn request_tone() -> &'static str {
    #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
    {
        match crate::drivers::hda::play::request_alert() {
            Ok(()) => "requested",
            Err(why) => why,
        }
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "hda-tone")))]
    {
        "nopath"
    }
}

static ALERT_OWED: AtomicBool = AtomicBool::new(false);

/// A dialog (an alert) went up: latch its sound (queue-only — `dialog::post` calls this from any context).
pub fn alert() {
    ALERT_OWED.store(true, Ordering::Release);
}

/// The latched alert's sound, gated like a post of the app `system` (its Sound, DND).
fn alert_service() {
    if !ALERT_OWED.swap(false, Ordering::AcqRel) {
        return;
    }
    let (_, kept, snd) = pn::route(&rule_at_post(b"system"), true, dnd_now());
    if kept && snd {
        let _ = sound_for(b"system(alert)", false);
    } else {
        serial_println!("[notifypane] sound app=system(alert) -> {}", if !kept || !rule_at_post(b"system").sound { "off(app)" } else { "dnd" });
    }
}

/// The sounds asked for this boot (fixtures included).
pub fn sounds() -> u32 {
    SOUNDS.load(Ordering::Relaxed)
}

/// NOTIFY's fixtures: hold the user's rules and schedule out (`on`) or let them back.
pub fn bypass(rules: bool, window: bool) {
    BYPASS_RULES.store(rules, Ordering::Relaxed);
    BYPASS_WINDOW.store(window, Ordering::Relaxed);
}

/// The fixture's rules, LIVE only (never saved): `npoff` off, `npctr` center-only, `npsnd` the default.
pub fn fixture_rules(on: bool) {
    let mut c = CELL.lock();
    for (name, r) in [("npoff", Rule { allow: false, ..pn::DEFAULT }), ("npctr", Rule { center: true, ..pn::DEFAULT }), ("npsnd", pn::DEFAULT)] {
        if on {
            if let Some(i) = insert(&mut c, name) {
                c.e[i].rule = r;
                c.e[i].owed = 0;
            }
        } else if let Some(i) = find(&c, name) {
            let n = c.n;
            c.e.copy_within(i + 1..n, i);
            c.n -= 1;
        }
    }
    STALE.store(true, Ordering::Relaxed);
}

fn load_service() {
    if LOADED.load(Ordering::Relaxed) {
        return;
    }
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        if crate::fs::users::whoami(&mut nm).is_none() {
            return;
        }
        LOADED.store(true, Ordering::Relaxed);
        let f = crate::prefs_client::sys_int(KEY_DND_FROM, 0, 23).unwrap_or(0) as u32;
        let u = crate::prefs_client::sys_int(KEY_DND_UNTIL, 0, 23).unwrap_or(0) as u32;
        FROM.store(f, Ordering::Relaxed);
        UNTIL.store(u, Ordering::Relaxed);
        let list = crate::prefs_client::pref_list(Some(crate::prefs::NS)).unwrap_or_default();
        let mut c = CELL.lock();
        for (_, k, v) in list.iter() {
            let Some((app, fi)) = pn::parse_key(k) else { continue };
            if let Some(i) = insert(&mut c, app) {
                pn::apply(&mut c.e[i].rule, fi, v);
            }
        }
        let n = c.n;
        drop(c);
        serial_println!("[notifypane] loaded apps={} dnd={} dnd_window={}-{} via=login", n, super::notify::dnd() as u8, f, u);
    }
}

/// NOTIFY's service pass: the login load, then the latched store writes (bus calls here, never in a router).
pub fn service() {
    static REG: AtomicBool = AtomicBool::new(false);
    if !REG.swap(true, Ordering::Relaxed) {
        crate::tests::register("notifypane", test);
    }
    load_service();
    alert_service();
    if WIN_OWED.swap(false, Ordering::AcqRel) {
        let (f, u) = window();
        let a = crate::prefs_client::pref_set(crate::prefs::NS, KEY_DND_FROM, PrefValue::Int(f as i64));
        let b = crate::prefs_client::pref_set(crate::prefs::NS, KEY_DND_UNTIL, PrefValue::Int(u as i64));
        serial_println!("[notifypane] saved notify.dnd_window {}-{} ok={}", f, u, (a.is_ok() && b.is_ok()) as u8);
    }
    let owed: Vec<(String, usize, PrefValue)> = {
        let Some(mut c) = CELL.try_lock() else { return };
        let n = c.n;
        let mut v = Vec::new();
        for e in c.e[..n].iter_mut() {
            for f in 0..pn::FIELDS.len() {
                if e.owed & (1 << f) != 0 {
                    v.push((pn::key_of(e.name(), f), f, pn::value(&e.rule, f)));
                }
            }
            e.owed = 0;
        }
        v
    };
    for (k, _, v) in owed {
        let r = crate::prefs_client::pref_set(crate::prefs::NS, &k, v);
        serial_println!("[notifypane] saved {} ok={}", k, r.is_ok() as u8);
    }
}

/// `tests notifypane`: NOTIFY's real pass over the fixture's rules (an app turned off posts nothing, a
/// center-only app collects without a card, a sounding app asks once per pass, DND silences it), the schedule
/// rule, and ONE real alert sound through the tone path.
pub fn test() {
    let (block, center, gate) = super::notify::pane_leg();
    let window_ok = pn::in_window(23, 22, 7) && pn::in_window(6, 22, 7) && !pn::in_window(7, 22, 7) && !pn::in_window(12, 12, 12);
    let r = sound_for(b"tests", false);
    let sound = match r {
        "requested" => "ok",
        "nopath" => "none",
        x => x,
    };
    let rs = rows();
    let allow = rs.iter().filter(|(_, r)| r.allow).count();
    let (f, u) = window();
    let ok = block && center && gate && window_ok;
    serial_println!(
        ":: NOTIFYPANE: apps={} allow={} sound={} dnd_window={}-{} -> {} :: block={} center={} window={} sound_gate={} dnd_now={}",
        rs.len(), allow, sound, f, u, if ok { "PASS" } else { "FAIL" },
        if block { "ok" } else { "FAIL" }, if center { "ok" } else { "FAIL" }, if window_ok { "ok" } else { "FAIL" },
        if gate { "ok" } else { "FAIL" }, dnd_now() as u8
    );
}
