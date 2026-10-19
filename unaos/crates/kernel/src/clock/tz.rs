// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — owed B701 (the zone table is kernel-side until a no_std lib both rings link carries it)
//!
//! TZLOCALE (rmbp-ledger B701) — THE ZONE TABLE. The civil anchor is UTC (`clock::set_anchor`); a zone is applied
//! only when a second is PRINTED (`clock::locale`). Each zone is a standard offset in minutes east of UTC and one of
//! five FIXED daylight-saving rules; the edges of any year are derived, never listed. A zone whose law is not a fixed
//! rule (Egypt, Israel, Iran, Chile, Lord Howe's half hour) is left out rather than approximated. `no_std`, no heap:
//! a painter on the composite tail may call [`offset`]. Held by `tests tzlocale` (`:: TZLOCALE: … dst_edges_ok= ::`).

/// A fixed daylight-saving rule; every rule moves the clock +60 min.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    /// No daylight saving.
    None,
    /// US and Canada: 2nd Sunday of March 02:00 standard -> 1st Sunday of November 02:00 daylight.
    Us,
    /// The EU, the UK and their neighbours: last Sunday of March 01:00 UTC -> last Sunday of October 01:00 UTC.
    Eu,
    /// South-east Australia: 1st Sunday of October 02:00 standard -> 1st Sunday of April 03:00 daylight.
    Au,
    /// New Zealand: last Sunday of September 02:00 standard -> 1st Sunday of April 03:00 daylight.
    Nz,
}

/// One edge of a rule: the `week`-th Sunday (5 = the last) of `month`, `min` minutes after its midnight, on the UTC
/// clock (`utc`) or the zone's STANDARD clock.
#[derive(Clone, Copy)]
struct Edge {
    month: u32,
    week: u32,
    min: i64,
    utc: bool,
}

const fn edge(month: u32, week: u32, min: i64, utc: bool) -> Edge {
    Edge { month, week, min, utc }
}

impl Rule {
    /// `(start, end)` of daylight time, or `None` for a zone without it. An end written on the daylight clock is
    /// stored on the standard clock (one hour earlier): US 02:00 daylight = 01:00 standard, AU/NZ 03:00 = 02:00.
    fn edges(self) -> Option<(Edge, Edge)> {
        match self {
            Rule::None => None,
            Rule::Us => Some((edge(3, 2, 120, false), edge(11, 1, 60, false))),
            Rule::Eu => Some((edge(3, 5, 60, true), edge(10, 5, 60, true))),
            Rule::Au => Some((edge(10, 1, 120, false), edge(4, 1, 120, false))),
            Rule::Nz => Some((edge(9, 5, 120, false), edge(4, 1, 120, false))),
        }
    }
}

/// One zone: its IANA name, its standard offset (minutes east of UTC) and its rule.
#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub name: &'static str,
    pub std_min: i32,
    pub rule: Rule,
}

const fn z(name: &'static str, std_min: i32, rule: Rule) -> Zone {
    Zone { name, std_min, rule }
}

/// The zones, west to east. Index 0 is `UTC`, the zero state (`system.locale.tz` unset).
pub const ZONES: [Zone; 52] = [
    z("UTC", 0, Rule::None),
    z("Pacific/Honolulu", -600, Rule::None),
    z("America/Anchorage", -540, Rule::Us),
    z("America/Los_Angeles", -480, Rule::Us),
    z("America/Vancouver", -480, Rule::Us),
    z("America/Denver", -420, Rule::Us),
    z("America/Phoenix", -420, Rule::None),
    z("America/Chicago", -360, Rule::Us),
    z("America/Mexico_City", -360, Rule::None),
    z("America/New_York", -300, Rule::Us),
    z("America/Toronto", -300, Rule::Us),
    z("America/Bogota", -300, Rule::None),
    z("America/Halifax", -240, Rule::Us),
    z("America/St_Johns", -210, Rule::Us),
    z("America/Sao_Paulo", -180, Rule::None),
    z("America/Argentina/Buenos_Aires", -180, Rule::None),
    z("Atlantic/Reykjavik", 0, Rule::None),
    z("Europe/London", 0, Rule::Eu),
    z("Europe/Dublin", 0, Rule::Eu),
    z("Europe/Lisbon", 0, Rule::Eu),
    z("Africa/Lagos", 60, Rule::None),
    z("Europe/Amsterdam", 60, Rule::Eu),
    z("Europe/Berlin", 60, Rule::Eu),
    z("Europe/Madrid", 60, Rule::Eu),
    z("Europe/Paris", 60, Rule::Eu),
    z("Europe/Rome", 60, Rule::Eu),
    z("Europe/Stockholm", 60, Rule::Eu),
    z("Europe/Warsaw", 60, Rule::Eu),
    z("Europe/Zurich", 60, Rule::Eu),
    z("Africa/Johannesburg", 120, Rule::None),
    z("Europe/Athens", 120, Rule::Eu),
    z("Europe/Helsinki", 120, Rule::Eu),
    z("Europe/Kyiv", 120, Rule::Eu),
    z("Europe/Istanbul", 180, Rule::None),
    z("Europe/Moscow", 180, Rule::None),
    z("Africa/Nairobi", 180, Rule::None),
    z("Asia/Dubai", 240, Rule::None),
    z("Asia/Karachi", 300, Rule::None),
    z("Asia/Kolkata", 330, Rule::None),
    z("Asia/Dhaka", 360, Rule::None),
    z("Asia/Bangkok", 420, Rule::None),
    z("Asia/Jakarta", 420, Rule::None),
    z("Asia/Shanghai", 480, Rule::None),
    z("Asia/Singapore", 480, Rule::None),
    z("Australia/Perth", 480, Rule::None),
    z("Asia/Seoul", 540, Rule::None),
    z("Asia/Tokyo", 540, Rule::None),
    z("Australia/Adelaide", 570, Rule::Au),
    z("Australia/Brisbane", 600, Rule::None),
    z("Australia/Sydney", 600, Rule::Au),
    z("Australia/Melbourne", 600, Rule::Au),
    z("Pacific/Auckland", 720, Rule::Nz),
];

/// The longest name a zone may carry (the schema's `system.locale.tz` string bound).
pub const NAME_MAX: usize = 40;

/// The zone named `name` (exact IANA spelling), by index.
pub fn index_of(name: &str) -> Option<usize> {
    ZONES.iter().position(|z| z.name == name)
}

/// Zone `i`, or `UTC` for an index outside the table.
pub fn at(i: usize) -> &'static Zone {
    ZONES.get(i).unwrap_or(&ZONES[0])
}

/// Days since 1970-01-01 of a civil date (Hinnant, through `clock::unix_from_civil`).
fn days(y: i64, m: u32, d: u32) -> i64 {
    (super::unix_from_civil(y, m, d, 0, 0, 0) / 86_400) as i64
}

/// Day of the week of a day number, 0 = Sunday (1970-01-01 was a Thursday).
fn dow(day: i64) -> i64 {
    (day + 4).rem_euclid(7)
}

/// The day of the month of the `week`-th Sunday (5 = the last) of `m` in `y`.
fn sunday(y: i64, m: u32, week: u32) -> u32 {
    if week >= 5 {
        let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        let last = days(ny, nm, 1) - 1; // the month's last day
        let ld = (last - days(y, m, 1)) as u32 + 1;
        ld - dow(last) as u32
    } else {
        let first = dow(days(y, m, 1)) as u32;
        1 + (7 - first) % 7 + 7 * (week - 1)
    }
}

/// The UTC second of `e` in year `y` for a zone of standard offset `std_min`.
fn edge_unix(y: i64, e: Edge, std_min: i32) -> i64 {
    let d = days(y, e.month, sunday(y, e.month, e.week));
    d * 86_400 + e.min * 60 - if e.utc { 0 } else { std_min as i64 * 60 }
}

/// The zone's offset (minutes east of UTC) at the UTC second `unix`, and whether daylight time is in force. The
/// year is the UTC year of `unix`: no rule has an edge within a day of New Year, so the local year never differs.
pub fn offset(z: &Zone, unix: u64) -> (i32, bool) {
    let Some((s, e)) = z.rule.edges() else { return (z.std_min, false) };
    let y = super::civil_from_unix(unix).0;
    let (s, e, t) = (edge_unix(y, s, z.std_min), edge_unix(y, e, z.std_min), unix as i64);
    let dst = if s < e { t >= s && t < e } else { t >= s || t < e };
    (z.std_min + if dst { 60 } else { 0 }, dst)
}

/// The UTC second of the start (`start = true`) or end of zone `z`'s daylight time in `y`; `None` without a rule.
/// The fixture's edges come from here AND from literal UTC seconds, so a wrong edge cannot agree with itself.
pub fn edge_of(z: &Zone, y: i64, start: bool) -> Option<i64> {
    let (s, e) = z.rule.edges()?;
    Some(edge_unix(y, if start { s } else { e }, z.std_min))
}

/// The table is well formed: `UTC` first at offset 0, names unique, printable and at most [`NAME_MAX`] bytes,
/// offsets within +/-14 h. Returns the number of bad rows (0 = well formed).
pub fn table_faults() -> usize {
    let mut bad = (ZONES[0].name != "UTC" || ZONES[0].std_min != 0 || ZONES[0].rule != Rule::None) as usize;
    for (i, z) in ZONES.iter().enumerate() {
        let name_ok = !z.name.is_empty() && z.name.len() <= NAME_MAX && z.name.bytes().all(|b| (0x21..=0x7e).contains(&b));
        let dup = ZONES[..i].iter().any(|o| o.name == z.name);
        if !name_ok || dup || z.std_min.abs() > 14 * 60 {
            bad += 1;
        }
    }
    bad
}
