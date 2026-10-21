// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — owed B701 (the locale is Principia's preference; this is its kernel reader and formatter)
//!
//! TZLOCALE (rmbp-ledger B701, BP-bar-17, research C §10: the locale is ONE authority, not per app). Three keys in
//! Principia's store say how a civil second is PRINTED: `system.locale.tz` (an IANA name from [`super::tz::ZONES`]),
//! `system.locale.region` (the date order and the hour cycle, [`REGIONS`]) and `system.locale.first_day`
//! ([`FIRST_DAYS`]). The anchor stays UTC; every painter asks [`fmt_time`] / [`fmt_date`] (or [`fmt_stamp`], the two
//! joined) and never splits a second itself.
//!
//! * The live locale is three `AtomicU8` (zero = UTC, `ymd-24`, `mon`): a painter on the composite tail reads it with
//!   relaxed loads and never a lock.
//! * [`service`] — one call on the settings service pass: a Settings choice's owed store write (never in the click
//!   router), the login generation's read and PrefChanged's re-read (look.rs's shape). Each applied change prints
//!   `[locale] tz=<name> off_min=<n> dst=<0|1> region=<r> first_day=<d> via=<login|bus|settings>`.
//! * The pure `_in(&Locale, …)` twins are the same arithmetic on a caller's [`Locale`]: `tests tzlocale` drives them,
//!   so no fixture writes the live locale or the store.
//! * [`wire`] extends the `[clock]` boot line: ` at=<local ISO-8601|unset> tz=<name> off_min=<n> dst=<0|1>`.

use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering};

use alloc::string::String;

use super::tz;

/// `system.locale.region`: the date order (`ymd` `YYYY-MM-DD`, `dmy` `DD/MM/YYYY`, `mdy` `MM/DD/YYYY`) and the hour
/// cycle (24 or 12). Index = order * 2 + (12 h); index 0 is the default. Mirrors the schema's enum.
pub const REGIONS: [&str; 6] = ["ymd-24", "ymd-12", "dmy-24", "dmy-12", "mdy-24", "mdy-12"];
/// `system.locale.first_day`: the week's first day. Index 0 is the default. Mirrors the schema's enum.
pub const FIRST_DAYS: [&str; 3] = ["mon", "sun", "sat"];

/// How a civil second is printed: a zone index, a region index, a first-day index (each clamped on read).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Locale {
    pub tz: u8,
    pub region: u8,
    pub first_day: u8,
}

impl Locale {
    /// UTC, `ymd-24`, `mon`: the zero state, and what an unset or unknown key reads as.
    pub const DEFAULT: Locale = Locale { tz: 0, region: 0, first_day: 0 };

    /// The three key values as the store holds them; an absent or unknown spelling keeps that field's default.
    /// The service's parse and the fixture's are this one fn.
    pub fn parse(tz: Option<&str>, region: Option<&str>, first_day: Option<&str>) -> Locale {
        let pos = |t: &[&str], v: Option<&str>| v.and_then(|s| t.iter().position(|x| *x == s)).unwrap_or(0) as u8;
        Locale { tz: tz.and_then(tz::index_of).unwrap_or(0) as u8, region: pos(&REGIONS, region), first_day: pos(&FIRST_DAYS, first_day) }
    }
    pub fn zone(&self) -> &'static tz::Zone {
        tz::at(self.tz as usize)
    }
    pub fn region_name(&self) -> &'static str {
        REGIONS[(self.region as usize).min(REGIONS.len() - 1)]
    }
    pub fn first_day_name(&self) -> &'static str {
        FIRST_DAYS[(self.first_day as usize).min(FIRST_DAYS.len() - 1)]
    }
    /// 0 `ymd`, 1 `dmy`, 2 `mdy`.
    pub fn order(&self) -> u8 {
        (self.region.min(REGIONS.len() as u8 - 1)) / 2
    }
    pub fn h24(&self) -> bool {
        self.region % 2 == 0
    }
    /// The week's first day as a weekday number (Sunday = 0): `mon` 1, `sun` 0, `sat` 6.
    pub fn first_wd(&self) -> u8 {
        [1u8, 0, 6][(self.first_day as usize).min(FIRST_DAYS.len() - 1)]
    }
    /// The calendar column (0..7) of weekday `wd` (Sunday = 0) in a week that starts on [`Self::first_wd`].
    pub fn week_col(&self, wd: u8) -> usize {
        ((wd % 7) as usize + 7 - self.first_wd() as usize) % 7
    }
    /// The weekday (Sunday = 0) a calendar column `col` shows.
    pub fn col_wd(&self, col: usize) -> u8 {
        ((self.first_wd() as usize + col) % 7) as u8
    }
    /// The local civil second (UTC + the zone's offset now), the offset in minutes and the daylight bit.
    pub fn local(&self, unix: u64) -> (u64, i32, bool) {
        let (off, dst) = tz::offset(self.zone(), unix);
        ((unix as i64 + off as i64 * 60).max(0) as u64, off, dst)
    }
}

static TZ: AtomicU8 = AtomicU8::new(0);
static REGION: AtomicU8 = AtomicU8::new(0);
static FIRST: AtomicU8 = AtomicU8::new(0);

/// The live locale (three relaxed loads; never a lock).
#[inline]
pub fn current() -> Locale {
    Locale { tz: TZ.load(Ordering::Relaxed), region: REGION.load(Ordering::Relaxed), first_day: FIRST.load(Ordering::Relaxed) }
}

/// The painters that print a civil second through this file. The bit of each lands in [`painted`] when it draws.
/// CLOCKPAINTERS (B719) repointed the seven that still split the UTC second themselves (`Bar` … `Quarry` are
/// TZLOCALE's three; `BarDate` … `Login` are B719's). CIVILREST (B738) repointed the last three: `Taken` … `Appearance`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Painter {
    /// The Una menu bar's clock (`video/menubar.rs` `clock_hhmm`).
    Bar = 0,
    /// The Softy taskbar's clock and date (`video/softy/taskbar.rs`).
    Taskbar = 1,
    /// Quarry's Modified column (`video/quarry/live.rs` `mtime_field`).
    Quarry = 2,
    /// The Una menu bar's date on a wide bar (`video/menubar.rs` `clockbar_date`).
    BarDate = 3,
    /// The clock item's menu: weekday, long date, time with the zone and offset, the month glance
    /// (`video/statusmenu.rs` `clock_lines`).
    ClockMenu = 4,
    /// The Softy calendar flyout (`video/softy/startmenu.rs` `paint_calendar`).
    Calendar = 5,
    /// The shell's `date` verb (`shell.rs`), and the local seed `date -s` takes.
    DateVerb = 6,
    /// Quarry's Get Info panel: the modified time and each attribute's change time (`video/quarry/getinfo.rs`).
    Info = 7,
    /// The notification centre's item times, an hour and older (`video/notify.rs` `age`).
    Notify = 8,
    /// The login screen's civil line under the card (`video/login.rs`).
    Login = 9,
    /// A photo's Taken attribute as a person reads it: Get Info's row and Quarry's column (`fs/attrfacts.rs` `fmt_value`).
    Taken = 10,
    /// The Privacy tab's kept decisions, the date each was answered (`video/settings.rs` `paint_privacy`).
    Privacy = 11,
    /// The local hour `auto` appearance and the notification quiet hours read (`video/appearance.rs` `local_hour`).
    Appearance = 12,
}
/// How many painters [`Painter`] names.
pub const PAINTERS: usize = 13;
static PAINTED: AtomicU16 = AtomicU16::new(0);

/// The painters that have drawn through this file this boot (bit = `Painter as u8`).
pub fn painted() -> u16 {
    PAINTED.load(Ordering::Relaxed)
}

#[inline]
fn mark(p: Painter) {
    PAINTED.fetch_or(1u16 << p as u8, Ordering::Relaxed);
}

/// Two decimal digits.
#[inline]
fn d2(v: u32) -> [u8; 2] {
    [b'0' + ((v / 10) % 10) as u8, b'0' + (v % 10) as u8]
}

/// **The clock text** of `unix` in the live locale, RIGHT-aligned and blank-padded into `out`: `HH:MM` (24 h) or
/// `h:MM AM` (12 h; a slot narrower than 8 drops the meridiem: `h:MM`). Returns the text's length.
pub fn fmt_time(p: Painter, unix: u64, out: &mut [u8]) -> usize {
    mark(p);
    fmt_time_in(&current(), unix, out)
}

/// **The date text** of `unix` in the live locale, left-aligned into `out` (10 bytes: `YYYY-MM-DD`, `DD/MM/YYYY` or
/// `MM/DD/YYYY`). Returns its length.
pub fn fmt_date(p: Painter, unix: u64, out: &mut [u8]) -> usize {
    mark(p);
    fmt_date_in(&current(), unix, out)
}

/// The date and the full clock text joined by a space (`2026-10-19 14:05`, `10/19/2026 2:05 PM`).
pub fn fmt_stamp(p: Painter, unix: u64) -> String {
    mark(p);
    fmt_stamp_in(&current(), unix)
}

/// [`fmt_time`] on locale `l` (pure).
pub fn fmt_time_in(l: &Locale, unix: u64, out: &mut [u8]) -> usize {
    let (ls, _, _) = l.local(unix);
    let (_, _, _, h, mi, _) = super::civil_from_unix(ls);
    let mut b = [b' '; 8];
    let mut n = 0usize;
    let mut put = |s: &[u8], b: &mut [u8; 8]| {
        for &c in s {
            if n < b.len() {
                b[n] = c;
                n += 1;
            }
        }
    };
    if l.h24() {
        put(&d2(h), &mut b);
    } else {
        let h12 = match h % 12 { 0 => 12, x => x };
        if h12 >= 10 { put(&d2(h12), &mut b) } else { put(&[b'0' + h12 as u8], &mut b) }
    }
    put(b":", &mut b);
    put(&d2(mi), &mut b);
    if !l.h24() && out.len() >= 8 {
        put(if h < 12 { b" AM" } else { b" PM" }, &mut b);
    }
    let k = n.min(out.len());
    let pad = out.len() - k;
    for c in out[..pad].iter_mut() {
        *c = b' ';
    }
    out[pad..].copy_from_slice(&b[..k]);
    k
}

/// [`fmt_date`] on locale `l` (pure).
pub fn fmt_date_in(l: &Locale, unix: u64, out: &mut [u8]) -> usize {
    let (ls, _, _) = l.local(unix);
    let (y, mo, d, _, _, _) = super::civil_from_unix(ls);
    let y = y.clamp(0, 9999) as u32;
    let (yh, yl) = (d2(y / 100), d2(y % 100));
    let (m, dd) = (d2(mo), d2(d));
    let b: [u8; 10] = match l.order() {
        1 => [dd[0], dd[1], b'/', m[0], m[1], b'/', yh[0], yh[1], yl[0], yl[1]],
        2 => [m[0], m[1], b'/', dd[0], dd[1], b'/', yh[0], yh[1], yl[0], yl[1]],
        _ => [yh[0], yh[1], yl[0], yl[1], b'-', m[0], m[1], b'-', dd[0], dd[1]],
    };
    let k = b.len().min(out.len());
    out[..k].copy_from_slice(&b[..k]);
    k
}

/// [`fmt_stamp`] on locale `l` (pure).
pub fn fmt_stamp_in(l: &Locale, unix: u64) -> String {
    let (mut d, mut t) = ([0u8; 10], [0u8; 8]);
    let dn = fmt_date_in(l, unix, &mut d);
    let tn = fmt_time_in(l, unix, &mut t);
    let mut s = String::with_capacity(dn + 1 + tn);
    s.push_str(core::str::from_utf8(&d[..dn]).unwrap_or(""));
    s.push(' ');
    s.push_str(core::str::from_utf8(&t[t.len() - tn..]).unwrap_or(""));
    s
}

/// CLOCKPAINTERS (B719): the local civil fields of one second, for a painter that lays out a calendar or a long date
/// (weekday Sunday = 0, the offset in minutes, the daylight bit). The split is this file's, never the painter's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Civil {
    pub y: i64,
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub s: u32,
    pub wd: u8,
    pub off: i32,
    pub dst: bool,
}

/// [`Civil`] of `unix` in the live locale.
pub fn civil(p: Painter, unix: u64) -> Civil {
    mark(p);
    civil_in(&current(), unix)
}

/// [`civil`] on locale `l` (pure).
pub fn civil_in(l: &Locale, unix: u64) -> Civil {
    let (ls, off, dst) = l.local(unix);
    let (y, mo, d, h, mi, s) = super::civil_from_unix(ls);
    Civil { y, mo: mo.clamp(1, 12), d, h, mi, s, wd: ((ls / 86_400 + 4) % 7) as u8, off, dst } // 1970-01-01 was a Thursday
}

/// The long form the shell's `date` prints, in the live locale: the date, the clock with seconds, the zone and its
/// offset (`2026-10-19 14:05:09 Europe/Berlin UTC+02:00`, `10/19/2026 2:05:09 PM America/New_York UTC-04:00`).
pub fn fmt_long(p: Painter, unix: u64) -> String {
    mark(p);
    fmt_long_in(&current(), unix)
}

/// [`fmt_long`] on locale `l` (pure).
pub fn fmt_long_in(l: &Locale, unix: u64) -> String {
    let c = civil_in(l, unix);
    let mut d = [0u8; 10];
    let dn = fmt_date_in(l, unix, &mut d);
    let o = off_text(c.off);
    let clock = if l.h24() {
        alloc::format!("{:02}:{:02}:{:02}", c.h, c.mi, c.s)
    } else {
        alloc::format!("{}:{:02}:{:02} {}", match c.h % 12 { 0 => 12, x => x }, c.mi, c.s, if c.h < 12 { "AM" } else { "PM" })
    };
    alloc::format!("{} {} {} UTC{}", core::str::from_utf8(&d[..dn]).unwrap_or(""), clock, l.zone().name, core::str::from_utf8(&o).unwrap_or("+00:00"))
}

/// The UTC second a LOCAL civil second `local` (read as if it were UTC) names in the live locale: what `date -s`
/// seeds, so the anchor stays UTC (BP-boot-18).
pub fn utc_of_local(p: Painter, local: u64) -> u64 {
    mark(p);
    utc_of_local_in(&current(), local)
}

/// [`utc_of_local`] on locale `l` (pure): the offset is read at the standard-time guess, then once more at the
/// result, so a second inside daylight time lands on daylight time. A skipped second (the spring gap) and a repeated
/// one (the autumn hour) both read as standard time.
pub fn utc_of_local_in(l: &Locale, local: u64) -> u64 {
    let z = l.zone();
    let guess = (local as i64 - z.std_min as i64 * 60).max(0) as u64;
    let (o1, _) = tz::offset(z, guess);
    let u1 = (local as i64 - o1 as i64 * 60).max(0) as u64;
    let (o2, _) = tz::offset(z, u1);
    (local as i64 - o2 as i64 * 60).max(0) as u64
}

/// `+HH:MM` / `-HH:MM` for an offset in minutes.
pub fn off_text(off: i32) -> [u8; 6] {
    let a = off.unsigned_abs();
    let (h, m) = (d2(a / 60), d2(a % 60));
    [if off < 0 { b'-' } else { b'+' }, h[0], h[1], b':', m[0], m[1]]
}

/// Local ISO-8601 with its offset, `2026-10-19T14:05:00+02:00`, on locale `l` (pure). 25 bytes.
pub fn iso_in(l: &Locale, unix: u64) -> String {
    let (ls, off, _) = l.local(unix);
    let (y, mo, d, h, mi, s) = super::civil_from_unix(ls);
    let o = off_text(off);
    alloc::format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}", y.clamp(0, 9999), mo, d, h, mi, s, core::str::from_utf8(&o).unwrap_or("+00:00"))
}

/// The `[clock]` line's tail: ` at=<local ISO-8601|unset> tz=<name> off_min=<n> dst=<0|1>` (never blocks: the
/// anchor is read with `try_unix_now`; a contended or unset clock prints `unset`).
pub fn wire() -> String {
    let l = current();
    match super::try_unix_now() {
        Some(u) => {
            let (_, off, dst) = l.local(u);
            alloc::format!(" at={} tz={} off_min={} dst={}", iso_in(&l, u), l.zone().name, off, dst as u8)
        }
        None => {
            let (off, dst) = tz::offset(l.zone(), 0);
            alloc::format!(" at=unset tz={} off_min={} dst={}", l.zone().name, off, dst as u8)
        }
    }
}

/// Put `l` on the glass and say so.
fn apply(l: Locale, via: &str) {
    TZ.store(l.tz.min(tz::ZONES.len() as u8 - 1), Ordering::Relaxed);
    REGION.store(l.region.min(REGIONS.len() as u8 - 1), Ordering::Relaxed);
    FIRST.store(l.first_day.min(FIRST_DAYS.len() as u8 - 1), Ordering::Relaxed);
    let l = current();
    let (_, off, dst) = l.local(super::try_unix_now().unwrap_or(0));
    serial_println!(
        "[locale] tz={} off_min={} dst={} region={} first_day={} via={}",
        l.zone().name, off, dst as u8, l.region_name(), l.first_day_name(), via
    );
}

// ── Settings > Date & Time: a choice is applied at once and its store write is owed to [`service`] ───────────────
const OWE_TZ: u8 = 1;
const OWE_REGION: u8 = 2;
const OWE_FIRST: u8 = 4;
static OWED: AtomicU8 = AtomicU8::new(0);
/// The session generation the keys were last read for (`u32::MAX` = never), and the PrefChanged count read at.
static READ_GEN: AtomicU32 = AtomicU32::new(u32::MAX);
static SEEN_CHANGES: AtomicU32 = AtomicU32::new(0);

/// Settings chose zone `i` (wrapped into the table).
pub fn choose_tz(i: usize) {
    let mut l = current();
    l.tz = (i % tz::ZONES.len()) as u8;
    apply(l, "settings");
    OWED.fetch_or(OWE_TZ, Ordering::Release);
}

/// Settings chose region `i` ([`REGIONS`] index).
pub fn choose_region(i: usize) {
    let mut l = current();
    l.region = i.min(REGIONS.len() - 1) as u8;
    apply(l, "settings");
    OWED.fetch_or(OWE_REGION, Ordering::Release);
}

/// Settings chose first day `i` ([`FIRST_DAYS`] index).
pub fn choose_first(i: usize) {
    let mut l = current();
    l.first_day = i.min(FIRST_DAYS.len() - 1) as u8;
    apply(l, "settings");
    OWED.fetch_or(OWE_FIRST, Ordering::Release);
}

/// The settings service pass: write a Settings choice's owed keys; read the three keys at a new login generation
/// and on PrefChanged. `true` = the live locale moved this pass (the caller repaints an open Date & Time pane).
/// Cheap when nothing moved: three relaxed loads.
pub fn service() -> bool {
    use crate::prefs::{key, PrefValue as P};
    let o = OWED.swap(0, Ordering::AcqRel);
    if o != 0 {
        let l = current();
        if o & OWE_TZ != 0 { crate::prefs_client::sys_set(key::LOCALE_TZ, P::Str(String::from(l.zone().name))); }
        if o & OWE_REGION != 0 { crate::prefs_client::sys_set(key::LOCALE_REGION, P::Str(String::from(l.region_name()))); }
        if o & OWE_FIRST != 0 { crate::prefs_client::sys_set(key::LOCALE_FIRST_DAY, P::Str(String::from(l.first_day_name()))); }
        SEEN_CHANGES.store(locale_mark(), Ordering::Relaxed); // PREFSCLIENTKEYS (B779): the ring's mark (file tail)
    }
    let g = crate::prefs::session_gen();
    let bus = locale_changed(); // PREFSCLIENTKEYS (B779): only a change naming a `system.locale.*` key (file tail)
    let login = READ_GEN.load(Ordering::Relaxed) != g;
    if !(login || bus) {
        return false;
    }
    let mut wb = [0u8; crate::prefs::WHO_BUF];
    if crate::prefs::user_name_in(&mut wb).is_none() {
        return false;
    }
    READ_GEN.store(g, Ordering::Relaxed);
    // the edge was claimed by `locale_changed` (PREFSCLIENTKEYS B779)
    let (t, r, f) = (crate::prefs_client::sys_text(key::LOCALE_TZ), crate::prefs_client::sys_text(key::LOCALE_REGION), crate::prefs_client::sys_text(key::LOCALE_FIRST_DAY));
    let l = Locale::parse(t.as_deref(), r.as_deref(), f.as_deref());
    if l == current() && !login {
        return false;
    }
    apply(l, if login { "login" } else { "bus" });
    true
}

// ── PREFSCLIENTKEYS (rmbp-ledger B779) — the keyed PrefChanged edge — TAIL-APPENDED, so no line above moves ─────────
// [`service`] re-reads its three keys only when a PrefChanged since its mark named one of them (`prefs_client::
// changed_take`, the last-changed ring kept beside `CHANGED_N`); any other key's change moves the mark at zero GETs.
// Outside the desktop families there is no kernel subscriber (Settings and the lock register one), so no PrefChanged
// edge ever moved there: the twins answer 0 / false, as `CHANGED_N` did.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn locale_mark() -> u32 {
    crate::prefs_client::changed_mark()
}
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn locale_changed() -> bool {
    crate::prefs_client::changed_take(&SEEN_CHANGES, |ns, k| ns == crate::prefs::NS && k.starts_with("locale."))
}
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn locale_mark() -> u32 {
    0
}
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn locale_changed() -> bool {
    false
}
