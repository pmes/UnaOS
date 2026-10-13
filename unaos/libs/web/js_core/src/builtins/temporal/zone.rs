//! Time zones for Temporal (proposal-temporal 2025 §11): UTC and its aliases, offset zones (±HH:MM, minute
//! precision), Etc/GMT±N, and every Zone and Link of the IANA database (INTLCLDR SR137: `crate::tzdata`, tzdata
//! 2025b) with its HISTORICAL offsets — every transition through 2037, and `tz::PosixTz` (the zone's TZif
//! footer rule) after the last one. A Link keeps its own identifier and compares equal to its primary
//! identifier. GetPossibleEpochNanoseconds,
//! DisambiguatePossibleEpochNanoseconds, GetStartOfDay and GetNamedTimeZoneNext/PreviousTransition live here.

use super::iso::*;
use crate::tz::PosixTz;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

const NS_PER_S: i128 = 1_000_000_000;

#[derive(Clone, Debug, PartialEq)]
pub enum TzKind {
    /// UTC or one of its aliases.
    Utc,
    /// A fixed offset in nanoseconds east of UTC (offset zones and Etc/GMT±N).
    Fixed(i64),
    /// The primary identifier (`tzdata::Found::primary`): a Zone index, or `ZONES.len()` + a primary Link's index.
    Named(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimeZone {
    /// The case-normalized identifier (what `timeZoneId` reports).
    pub id: String,
    pub kind: TzKind,
    /// True for an offset time zone identifier (±HH:MM).
    pub is_offset: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disambiguation {
    Compatible,
    Earlier,
    Later,
    Reject,
}

/// An offset zone identifier: ±HH, ±HHMM or ±HH:MM (§11.1.x IsOffsetTimeZoneIdentifier) → minutes.
pub fn parse_offset_id(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let sign = match b.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let r = &b[1..];
    let (h, m) = match r.len() {
        2 => (&r[0..2], &b"00"[..]),
        4 => (&r[0..2], &r[2..4]),
        5 if r[2] == b':' => (&r[0..2], &r[3..5]),
        _ => return None,
    };
    if !h.iter().chain(m.iter()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h = ((h[0] - b'0') * 10 + (h[1] - b'0')) as i64;
    let m = ((m[0] - b'0') * 10 + (m[1] - b'0')) as i64;
    if h > 23 || m > 59 {
        return None;
    }
    Some(sign * (h * 60 + m))
}

pub fn offset_minutes_id(minutes: i64) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let a = minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", a / 60, a % 60)
}

const UTC_ALIASES: &[&str] = &[
    "UTC", "Etc/UTC", "Etc/GMT", "GMT", "Etc/UCT", "UCT", "Etc/Universal", "Universal", "Etc/Zulu", "Zulu", "Etc/Greenwich", "Greenwich", "Etc/GMT0",
    "GMT0", "Etc/GMT+0", "Etc/GMT-0", "GMT+0", "GMT-0",
];

/// A time zone identifier (offset or available named zone), case-normalized; `None` when unknown.
pub fn parse_identifier(s: &str) -> Option<TimeZone> {
    if let Some(m) = parse_offset_id(s) {
        return Some(TimeZone { id: offset_minutes_id(m), kind: TzKind::Fixed(m * 60_000_000_000), is_offset: true });
    }
    if let Some(a) = UTC_ALIASES.iter().find(|a| a.eq_ignore_ascii_case(s)) {
        return Some(TimeZone { id: String::from(*a), kind: TzKind::Utc, is_offset: false });
    }
    let u = s.to_ascii_uppercase();
    if let Some(rest) = u.strip_prefix("ETC/GMT") {
        let (sign, num) = match rest.as_bytes().first() {
            Some(b'+') => (1i64, &rest[1..]),
            Some(b'-') => (-1i64, &rest[1..]),
            _ => return None,
        };
        if num.is_empty() || num.len() > 2 || !num.bytes().all(|c| c.is_ascii_digit()) || num.starts_with('0') {
            return None;
        }
        let n: i64 = num.parse().ok()?;
        if (sign == 1 && n > 12) || (sign == -1 && n > 14) {
            return None;
        }
        // Etc/GMT+5 is five hours WEST of Greenwich.
        return Some(TimeZone {
            id: format!("Etc/GMT{}{}", if sign == 1 { '+' } else { '-' }, n),
            kind: TzKind::Fixed(-sign * n * 3_600_000_000_000),
            is_offset: false,
        });
    }
    let f = crate::tzdata::find(s)?;
    Some(TimeZone { id: String::from(f.id), kind: TzKind::Named(f.primary as usize), is_offset: false })
}

impl TimeZone {
    pub fn utc() -> TimeZone {
        TimeZone { id: String::from("UTC"), kind: TzKind::Utc, is_offset: false }
    }
    /// TimeZoneEquals: the same primary identifier (all UTC aliases are one zone).
    pub fn equals(&self, o: &TimeZone) -> bool {
        match (&self.kind, &o.kind) {
            (TzKind::Utc, TzKind::Utc) => true,
            (TzKind::Named(a), TzKind::Named(b)) => a == b,
            (TzKind::Fixed(a), TzKind::Fixed(b)) => a == b && self.is_offset == o.is_offset && self.id == o.id,
            _ => false,
        }
    }
}

// ============================================================================================ named zones

struct Named {
    initial: i64,
    trans: Vec<(i128, i64)>,
    rule: Option<PosixTz>,
}

/// Decode a zone's history: (transition instant in ns, offset in ns after it).
fn named(idx: usize) -> Named {
    let data = crate::tzdata::by_primary(idx as u16).data;
    let rule = PosixTz::parse(crate::tzdata::ZONES[data as usize].posix);
    let initial = crate::tzdata::initial(data).offset as i64 * 1_000_000_000;
    // Offset changes only: a transition that flips the daylight-saving flag alone is no Temporal transition.
    let mut trans: Vec<(i128, i64)> = Vec::new();
    let mut cur = initial;
    for (t, k) in crate::tzdata::transitions(data) {
        let off = k.offset as i64 * 1_000_000_000;
        if off != cur {
            trans.push((t as i128 * NS_PER_S, off));
            cur = off;
        }
    }
    Named { initial, trans, rule }
}

impl Named {
    fn rule_offset(&self, ns: i128) -> i64 {
        match &self.rule {
            Some(r) => {
                let ms = ns.div_euclid(1_000_000) as f64;
                (r.offset_at_utc(ms) as i64) * 1_000_000
            }
            None => self.initial,
        }
    }
    fn offset_at(&self, ns: i128) -> i64 {
        match self.trans.last() {
            None => self.rule_offset(ns),
            Some((last, _)) if ns >= *last => self.rule_offset(ns),
            _ => {
                let mut off = self.initial;
                for (t, o) in &self.trans {
                    if *t > ns {
                        break;
                    }
                    off = *o;
                }
                off
            }
        }
    }
    /// Rule transitions (UTC ns) of the local years around `ns`.
    fn rule_transitions(&self, ns: i128, span: i64) -> Vec<i128> {
        let mut v = Vec::new();
        if let Some(r) = &self.rule {
            let y = datetime_from_ns(ns).0.year;
            for yy in (y - span)..=(y + span) {
                if let Some((a, b)) = r.transitions_utc(yy as f64) {
                    v.push(a as i128 * 1_000_000);
                    v.push(b as i128 * 1_000_000);
                }
            }
        }
        v.sort();
        v
    }
    fn next(&self, ns: i128) -> Option<i128> {
        let last = self.trans.last().map(|x| x.0);
        if let Some(t) = self.trans.iter().map(|x| x.0).find(|t| *t > ns) {
            return Some(t);
        }
        let floor = last.unwrap_or(i128::MIN);
        self.rule_transitions(ns, 1).into_iter().find(|t| *t > ns && *t > floor)
    }
    fn previous(&self, ns: i128) -> Option<i128> {
        let last = self.trans.last().map(|x| x.0);
        let floor = last.unwrap_or(i128::MIN);
        if last.map(|l| ns > l).unwrap_or(true) {
            if let Some(t) = self.rule_transitions(ns, 1).into_iter().rev().find(|t| *t < ns && *t > floor) {
                return Some(t);
            }
        }
        self.trans.iter().map(|x| x.0).filter(|t| *t < ns).last()
    }
}

/// GetOffsetNanosecondsFor.
pub fn offset_ns(tz: &TimeZone, ns: i128) -> i64 {
    match &tz.kind {
        TzKind::Utc => 0,
        TzKind::Fixed(o) => *o,
        TzKind::Named(i) => named(*i).offset_at(ns),
    }
}

/// GetISODateTimeFor.
pub fn local(tz: &TimeZone, ns: i128) -> (IsoDate, Time) {
    datetime_from_ns(ns + offset_ns(tz, ns) as i128)
}

/// GetPossibleEpochNanoseconds (§11.6.x): the instants whose wall-clock reading is `d`/`t`, ascending.
pub fn possible(tz: &TimeZone, d: IsoDate, t: Time) -> Result<Vec<i128>, &'static str> {
    let utc = utc_ns(d, t);
    let mut out = Vec::new();
    match &tz.kind {
        TzKind::Utc => out.push(utc),
        TzKind::Fixed(o) => out.push(utc - *o as i128),
        TzKind::Named(i) => {
            let z = named(*i);
            let mut cands: Vec<i64> = Vec::new();
            for probe in [utc - NS_PER_DAY, utc, utc + NS_PER_DAY] {
                let o = z.offset_at(probe);
                if !cands.contains(&o) {
                    cands.push(o);
                }
            }
            for o in cands {
                let e = utc - o as i128;
                if z.offset_at(e) == o && !out.contains(&e) {
                    out.push(e);
                }
            }
            out.sort();
        }
    }
    for e in &out {
        if !valid_epoch_ns(*e) {
            return Err("date-time outside the representable range");
        }
    }
    Ok(out)
}

/// GetEpochNanosecondsFor = DisambiguatePossibleEpochNanoseconds(GetPossibleEpochNanoseconds(...)).
pub fn epoch_ns_for(tz: &TimeZone, d: IsoDate, t: Time, dis: Disambiguation) -> Result<i128, &'static str> {
    let p = possible(tz, d, t)?;
    disambiguate(&p, tz, d, t, dis)
}

/// DisambiguatePossibleEpochNanoseconds (§11.6.x).
pub fn disambiguate(p: &[i128], tz: &TimeZone, d: IsoDate, t: Time, dis: Disambiguation) -> Result<i128, &'static str> {
    if p.len() == 1 {
        return Ok(p[0]);
    }
    if !p.is_empty() {
        return match dis {
            Disambiguation::Earlier | Disambiguation::Compatible => Ok(p[0]),
            Disambiguation::Later => Ok(p[p.len() - 1]),
            Disambiguation::Reject => Err("ambiguous wall-clock time rejected (disambiguation: reject)"),
        };
    }
    if dis == Disambiguation::Reject {
        return Err("skipped wall-clock time rejected (disambiguation: reject)");
    }
    let utc = utc_ns(d, t);
    let before = utc - NS_PER_DAY;
    let after = utc + NS_PER_DAY;
    if !valid_epoch_ns(before) || !valid_epoch_ns(after) {
        return Err("date-time outside the representable range");
    }
    let ob = offset_ns(tz, before) as i128;
    let oa = offset_ns(tz, after) as i128;
    let gap = oa - ob;
    if dis == Disambiguation::Earlier {
        let (days, tt) = add_time(t, -gap);
        let dd = d.add_days(days);
        let q = possible(tz, dd, tt)?;
        return q.first().copied().ok_or("no instant for wall-clock time");
    }
    let (days, tt) = add_time(t, gap);
    let dd = d.add_days(days);
    let q = possible(tz, dd, tt)?;
    q.last().copied().ok_or("no instant for wall-clock time")
}

/// GetStartOfDay (§11.6.x).
pub fn start_of_day(tz: &TimeZone, d: IsoDate) -> Result<i128, &'static str> {
    let p = possible(tz, d, Time::MIDNIGHT)?;
    if let Some(x) = p.first() {
        return Ok(*x);
    }
    let before = utc_ns(d, Time::MIDNIGHT) - NS_PER_DAY;
    match next_transition(tz, before) {
        Some(t) => Ok(t),
        None => Err("no start of day"),
    }
}

/// GetNamedTimeZoneNextTransition (null for UTC and fixed zones, and past nsMaxInstant).
pub fn next_transition(tz: &TimeZone, ns: i128) -> Option<i128> {
    match &tz.kind {
        TzKind::Named(i) => named(*i).next(ns).filter(|t| valid_epoch_ns(*t)),
        _ => None,
    }
}

pub fn previous_transition(tz: &TimeZone, ns: i128) -> Option<i128> {
    match &tz.kind {
        TzKind::Named(i) => named(*i).previous(ns).filter(|t| valid_epoch_ns(*t)),
        _ => None,
    }
}
