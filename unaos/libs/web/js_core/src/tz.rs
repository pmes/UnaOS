//! Time zones as data: a POSIX TZ rule (`std offset [dst [offset] [,start[/time],end[/time]]]`, IEEE 1003.1
//! §8.3) gives LocalTZA without any OS time-zone database. The embedding sets the rule string; the default is UTC.

use alloc::string::String;

const MS_HOUR: f64 = 3_600_000.0;
const MS_DAY: f64 = 86_400_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Rule {
    /// Jn: day 1..=365, February 29 never counted.
    Julian1(u32),
    /// n: day 0..=365, leap days counted.
    Julian0(u32),
    /// Mm.w.d: month 1..=12, week 1..=5 (5 = last), weekday 0..=6 (Sunday = 0).
    Month(u32, u32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PosixTz {
    pub std_name: String,
    /// Offset east of UTC in ms (the POSIX string writes hours west of UTC).
    pub std_offset: f64,
    pub dst: Option<Dst>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dst {
    pub name: String,
    pub offset: f64,
    start: (Rule, f64),
    end: (Rule, f64),
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn name(&mut self) -> Option<String> {
        let start = self.i;
        if self.peek() == Some(b'<') {
            self.i += 1;
            let b = self.i;
            while let Some(c) = self.peek() {
                if c == b'>' {
                    let n = core::str::from_utf8(&self.s[b..self.i]).ok()?.into();
                    self.i += 1;
                    return Some(n);
                }
                self.i += 1;
            }
            return None;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        if self.i - start < 3 {
            return None;
        }
        Some(core::str::from_utf8(&self.s[start..self.i]).ok()?.into())
    }
    fn num(&mut self) -> Option<u32> {
        let start = self.i;
        let mut v: u32 = 0;
        while let Some(c) = self.peek() {
            if !c.is_ascii_digit() {
                break;
            }
            v = v.checked_mul(10)?.checked_add((c - b'0') as u32)?;
            self.i += 1;
        }
        if self.i == start { None } else { Some(v) }
    }
    /// `[+-]hh[:mm[:ss]]` in ms.
    fn time(&mut self) -> Option<f64> {
        let neg = match self.peek() {
            Some(b'-') => {
                self.i += 1;
                true
            }
            Some(b'+') => {
                self.i += 1;
                false
            }
            _ => false,
        };
        let h = self.num()? as f64;
        let mut t = h * MS_HOUR;
        for scale in [60_000.0, 1000.0] {
            if self.peek() == Some(b':') {
                self.i += 1;
                t += self.num()? as f64 * scale;
            } else {
                break;
            }
        }
        Some(if neg { -t } else { t })
    }
    fn rule(&mut self) -> Option<(Rule, f64)> {
        let r = match self.peek()? {
            b'J' => {
                self.i += 1;
                Rule::Julian1(self.num()?)
            }
            b'M' => {
                self.i += 1;
                let m = self.num()?;
                if self.peek() != Some(b'.') {
                    return None;
                }
                self.i += 1;
                let w = self.num()?;
                if self.peek() != Some(b'.') {
                    return None;
                }
                self.i += 1;
                let d = self.num()?;
                if !(1..=12).contains(&m) || !(1..=5).contains(&w) || d > 6 {
                    return None;
                }
                Rule::Month(m, w, d)
            }
            _ => Rule::Julian0(self.num()?),
        };
        let t = if self.peek() == Some(b'/') {
            self.i += 1;
            self.time()?
        } else {
            2.0 * MS_HOUR
        };
        Some((r, t))
    }
}

impl PosixTz {
    pub fn utc() -> PosixTz {
        PosixTz { std_name: String::from("UTC"), std_offset: 0.0, dst: None }
    }

    /// Parse a POSIX TZ rule string, e.g. `CET-1CEST,M3.5.0,M10.5.0/3` or `EST5EDT,M3.2.0,M11.1.0`.
    pub fn parse(s: &str) -> Option<PosixTz> {
        let mut p = P { s: s.as_bytes(), i: 0 };
        let std_name = p.name()?;
        let std_offset = -p.time()?;
        if p.peek().is_none() {
            return Some(PosixTz { std_name, std_offset, dst: None });
        }
        let dst_name = p.name()?;
        let dst_offset = if matches!(p.peek(), Some(c) if c == b'+' || c == b'-' || c.is_ascii_digit()) { -p.time()? } else { std_offset + MS_HOUR };
        let (start, end) = if p.peek() == Some(b',') {
            p.i += 1;
            let a = p.rule()?;
            if p.peek() != Some(b',') {
                return None;
            }
            p.i += 1;
            let b = p.rule()?;
            (a, b)
        } else {
            // POSIX leaves the default implementation-defined; use the current US rules.
            ((Rule::Month(3, 2, 0), 2.0 * MS_HOUR), (Rule::Month(11, 1, 0), 2.0 * MS_HOUR))
        };
        if p.peek().is_some() {
            return None;
        }
        Some(PosixTz { std_name, std_offset, dst: Some(Dst { name: dst_name, offset: dst_offset, start, end }) })
    }

    /// Offset (ms east of UTC) in effect at UTC time `t`.
    pub fn offset_at_utc(&self, t: f64) -> f64 {
        let d = match &self.dst {
            None => return self.std_offset,
            Some(d) => d,
        };
        if !t.is_finite() {
            return self.std_offset;
        }
        let year = year_of(t + self.std_offset);
        // Transition instants in UTC: start is given in standard local time, end in daylight local time.
        let s = rule_day_start(year, d.start.0) + d.start.1 - self.std_offset;
        let e = rule_day_start(year, d.end.0) + d.end.1 - d.offset;
        let in_dst = if s < e { t >= s && t < e } else { !(t >= e && t < s) };
        if in_dst { d.offset } else { self.std_offset }
    }

    /// Offset to subtract from local time `t` to get UTC: repeated local times take the offset before the
    /// transition (daylight time), skipped ones are read with the offset before the transition (standard time).
    pub fn offset_for_local(&self, t: f64) -> f64 {
        let d = match &self.dst {
            None => return self.std_offset,
            Some(d) => d,
        };
        if self.offset_at_utc(t - d.offset) == d.offset {
            return d.offset;
        }
        self.std_offset
    }

    /// The abbreviation in effect at UTC time `t`.
    pub fn name_at_utc(&self, t: f64) -> &str {
        match &self.dst {
            Some(d) if self.offset_at_utc(t) == d.offset && d.offset != self.std_offset => &d.name,
            _ => &self.std_name,
        }
    }
}

fn floor(x: f64) -> f64 {
    crate::numconv::libm_floor(x)
}

pub fn day_from_year(y: f64) -> f64 {
    365.0 * (y - 1970.0) + floor((y - 1969.0) / 4.0) - floor((y - 1901.0) / 100.0) + floor((y - 1601.0) / 400.0)
}

pub fn is_leap(y: f64) -> bool {
    let y = y as i64;
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn year_of(t: f64) -> f64 {
    let mut y = floor(t / (MS_DAY * 365.2425)) + 1970.0;
    while day_from_year(y) * MS_DAY > t {
        y -= 1.0;
    }
    while day_from_year(y + 1.0) * MS_DAY <= t {
        y += 1.0;
    }
    y
}

const CUM: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// Local midnight (as ms from the epoch, in the zone's own clock) of the rule's day in `year`.
fn rule_day_start(year: f64, r: Rule) -> f64 {
    let leap = is_leap(year);
    let y0 = day_from_year(year);
    let day = match r {
        Rule::Julian1(n) => {
            let n = n.clamp(1, 365);
            let extra = if leap && n >= 60 { 1 } else { 0 };
            (n - 1 + extra) as f64
        }
        Rule::Julian0(n) => n.min(365) as f64,
        Rule::Month(m, w, wd) => {
            let mi = (m - 1) as usize;
            let first = CUM[mi] + if leap && m > 2 { 1 } else { 0 };
            let dim = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][mi];
            let first_wd = ((y0 + first as f64 + 4.0) % 7.0 + 7.0) % 7.0;
            let mut d = (wd as f64 - first_wd + 7.0) % 7.0 + 7.0 * (w as f64 - 1.0);
            while d >= dim as f64 {
                d -= 7.0;
            }
            first as f64 + d
        }
    };
    (y0 + day) * MS_DAY
}
