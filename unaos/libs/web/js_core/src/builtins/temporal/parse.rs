//! The Temporal ISO 8601 string grammar (proposal-temporal 2025 §13.x, "ISO 8601 grammar") with RFC 9557
//! annotations: dates (`YYYY-MM-DD`, `YYYYMMDD`, `±YYYYYY`), times with fractions of up to nine digits,
//! UTC offsets with sub-minute precision, the time zone annotation `[Zone]` / `[±HH:MM]`, key-value
//! annotations (`[u-ca=iso8601]`, critical `!`), year-month and month-day forms, and ISO 8601 durations.
//! Every function returns `Err(message)` for a string outside the grammar; the caller throws a RangeError.

use super::iso::*;
use alloc::string::String;

pub type PResult<T> = Result<T, &'static str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offset {
    pub ns: i64,
    /// The offset was written with seconds (sub-minute precision).
    pub sub_minute: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub year: i64,
    pub month: u8,
    /// 0 when the form had no day (year-month).
    pub day: u8,
    pub has_date: bool,
    pub time: Option<Time>,
    pub z: bool,
    pub offset: Option<Offset>,
    pub tz: Option<String>,
    pub calendar: Option<String>,
}

struct C<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> C<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn at(&self, k: usize) -> Option<u8> {
        self.s.get(self.i + k).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn done(&self) -> bool {
        self.i >= self.s.len()
    }
    fn digits(&mut self, n: usize) -> Option<i64> {
        if self.i + n > self.s.len() {
            return None;
        }
        let mut v = 0i64;
        for k in 0..n {
            let c = self.s[self.i + k];
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as i64;
        }
        self.i += n;
        Some(v)
    }
    fn is_digit_at(&self, k: usize) -> bool {
        self.at(k).map(|c| c.is_ascii_digit()).unwrap_or(false)
    }
}

fn year(c: &mut C) -> PResult<i64> {
    match c.peek() {
        Some(b'+') | Some(b'-') => {
            let neg = c.peek() == Some(b'-');
            c.i += 1;
            let y = c.digits(6).ok_or("invalid extended year")?;
            if neg && y == 0 {
                return Err("-000000 is not a valid year");
            }
            Ok(if neg { -y } else { y })
        }
        _ => c.digits(4).ok_or("invalid year"),
    }
}

/// Date: extended (`-` separators) or basic, never mixed.
fn date(c: &mut C) -> PResult<(i64, u8, u8)> {
    let y = year(c)?;
    let ext = c.eat(b'-');
    let m = c.digits(2).ok_or("invalid month")?;
    if ext {
        if !c.eat(b'-') {
            return Err("invalid date");
        }
    } else if c.peek() == Some(b'-') {
        return Err("mixed basic and extended date format");
    }
    let d = c.digits(2).ok_or("invalid day")?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m as u8) as i64 {
        return Err("invalid ISO date");
    }
    Ok((y, m as u8, d as u8))
}

/// Fraction digits after `.` or `,`: nanoseconds.
fn fraction(c: &mut C) -> PResult<Option<i64>> {
    if c.peek() == Some(b'.') || c.peek() == Some(b',') {
        c.i += 1;
        let start = c.i;
        let mut v = 0i64;
        while let Some(d) = c.peek() {
            if !d.is_ascii_digit() {
                break;
            }
            if c.i - start >= 9 {
                return Err("more than nine fractional digits");
            }
            v = v * 10 + (d - b'0') as i64;
            c.i += 1;
        }
        let n = c.i - start;
        if n == 0 {
            return Err("empty fraction");
        }
        for _ in n..9 {
            v *= 10;
        }
        return Ok(Some(v));
    }
    Ok(None)
}

/// TimeSpec: HH[:MM[:SS[.f]]] or HH[MM[SS[.f]]].
fn time(c: &mut C) -> PResult<Time> {
    let h = c.digits(2).ok_or("invalid hour")?;
    let mut mi = 0;
    let mut s = 0;
    let mut frac = 0;
    let ext = c.peek() == Some(b':');
    if ext || c.is_digit_at(0) {
        if ext {
            c.i += 1;
        }
        mi = c.digits(2).ok_or("invalid minute")?;
        let more = if ext { c.peek() == Some(b':') } else { c.is_digit_at(0) };
        if more {
            if ext {
                c.i += 1;
            }
            s = c.digits(2).ok_or("invalid second")?;
            if let Some(f) = fraction(c)? {
                frac = f;
            }
        } else if !ext && c.peek() == Some(b':') {
            return Err("mixed basic and extended time format");
        }
    }
    if h > 23 || mi > 59 || s > 60 {
        return Err("invalid time");
    }
    if s == 60 {
        s = 59;
    }
    Ok(Time {
        hour: h as u8,
        minute: mi as u8,
        second: s as u8,
        ms: (frac / 1_000_000) as u16,
        us: ((frac / 1000) % 1000) as u16,
        ns: (frac % 1000) as u16,
    })
}

/// UTCOffset with optional sub-minute precision.
fn utc_offset(c: &mut C, allow_sub_minute: bool) -> PResult<Offset> {
    let sign = match c.peek() {
        Some(b'+') => 1i64,
        Some(b'-') => -1,
        _ => return Err("expected an offset"),
    };
    c.i += 1;
    let h = c.digits(2).ok_or("invalid offset hour")?;
    let mut m = 0;
    let mut s = 0;
    let mut frac = 0;
    let mut sub = false;
    let ext = c.peek() == Some(b':');
    if ext || c.is_digit_at(0) {
        if ext {
            c.i += 1;
        }
        m = c.digits(2).ok_or("invalid offset minute")?;
        let more = if ext { c.peek() == Some(b':') } else { c.is_digit_at(0) };
        if more {
            if !allow_sub_minute {
                return Err("offset with seconds is not allowed here");
            }
            if ext {
                c.i += 1;
            }
            s = c.digits(2).ok_or("invalid offset second")?;
            sub = true;
            if let Some(f) = fraction(c)? {
                frac = f;
            }
        }
    }
    if h > 23 || m > 59 || s > 59 {
        return Err("invalid offset");
    }
    let ns = ((h * 60 + m) * 60 + s) * 1_000_000_000 + frac;
    Ok(Offset { ns: sign * ns, sub_minute: sub })
}

fn is_tz_char(c: u8, first: bool) -> bool {
    c.is_ascii_alphabetic() || c == b'.' || c == b'_' || (!first && (c.is_ascii_digit() || c == b'-' || c == b'+'))
}

fn annotations(c: &mut C, p: &mut Parsed, allow_tz: bool) -> PResult<()> {
    let mut first = true;
    let mut cal_critical = false;
    let mut cal_count = 0;
    while c.peek() == Some(b'[') {
        c.i += 1;
        let critical = c.eat(b'!');
        let start = c.i;
        while let Some(x) = c.peek() {
            if x == b']' {
                break;
            }
            c.i += 1;
        }
        if c.done() {
            return Err("unterminated annotation");
        }
        let body = &c.s[start..c.i];
        c.i += 1;
        let text = core::str::from_utf8(body).map_err(|_| "invalid annotation")?;
        if let Some(eq) = text.find('=') {
            let (k, v) = (&text[..eq], &text[eq + 1..]);
            let kb = k.as_bytes();
            if kb.is_empty() || !(kb[0].is_ascii_lowercase() || kb[0] == b'_') || !kb.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-') {
                return Err("invalid annotation key");
            }
            if v.is_empty() || !v.split('-').all(|part| (3..=8).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_alphanumeric())) {
                return Err("invalid annotation value");
            }
            if k == "u-ca" {
                cal_count += 1;
                if cal_count == 1 {
                    p.calendar = Some(String::from(v));
                    cal_critical = critical;
                } else if critical || cal_critical {
                    return Err("conflicting critical calendar annotations");
                }
            } else if critical {
                return Err("unknown critical annotation");
            }
            first = false;
        } else {
            if !first || !allow_tz || p.tz.is_some() {
                return Err("misplaced time zone annotation");
            }
            let ok = if text.starts_with('+') || text.starts_with('-') {
                let mut cc = C { s: text.as_bytes(), i: 0 };
                utc_offset(&mut cc, false).is_ok() && cc.done()
            } else {
                !text.is_empty()
                    && text.split('/').all(|seg| {
                        let b = seg.as_bytes();
                        !b.is_empty() && seg != "." && seg != ".." && is_tz_char(b[0], true) && b.iter().all(|x| is_tz_char(*x, false))
                    })
            };
            if !ok {
                return Err("invalid time zone annotation");
            }
            p.tz = Some(String::from(text));
            first = false;
        }
    }
    let _ = first;
    Ok(())
}

/// Date [T Time] [Offset|Z] [TimeZoneAnnotation] [Annotations]: the shared core of the date-time forms.
fn date_time(c: &mut C, p: &mut Parsed) -> PResult<()> {
    let (y, m, d) = date(c)?;
    p.year = y;
    p.month = m;
    p.day = d;
    p.has_date = true;
    if matches!(c.peek(), Some(b'T') | Some(b't') | Some(b' ')) && c.is_digit_at(1) {
        c.i += 1;
        p.time = Some(time(c)?);
    }
    if p.time.is_some() {
        match c.peek() {
            Some(b'Z') | Some(b'z') => {
                c.i += 1;
                p.z = true;
            }
            Some(b'+') | Some(b'-') => {
                p.offset = Some(utc_offset(c, true)?);
            }
            _ => {}
        }
    }
    annotations(c, p, true)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// PlainDate / PlainDateTime: no `Z`.
    DateTime,
    /// ZonedDateTime: a time zone annotation is required.
    Zoned,
    /// Instant: an offset or `Z` is required, with a time.
    Instant,
    /// Duration.round/total/compare relativeTo: plain or zoned.
    Relative,
}

fn finish(c: &C) -> PResult<()> {
    if c.done() { Ok(()) } else { Err("trailing characters") }
}

pub fn parse_date_time(s: &str, form: Form) -> PResult<Parsed> {
    let mut c = C { s: s.as_bytes(), i: 0 };
    let mut p = Parsed::default();
    date_time(&mut c, &mut p)?;
    finish(&c)?;
    match form {
        Form::DateTime => {
            if p.z {
                return Err("a UTC designator is not allowed for a plain date-time");
            }
        }
        Form::Zoned => {
            if p.tz.is_none() {
                return Err("a time zone annotation is required");
            }
        }
        Form::Instant => {
            if !p.z && p.offset.is_none() {
                return Err("an offset or Z is required for an instant");
            }
            if p.time.is_none() {
                return Err("a time is required for an instant");
            }
        }
        Form::Relative => {
            if p.z && p.tz.is_none() {
                return Err("a UTC designator requires a time zone annotation");
            }
        }
    }
    Ok(p)
}

/// TemporalTimeString: `T`-prefixed or bare time (refusing strings that are also month-day or year-month
/// forms), or a date-time with a time.
pub fn parse_time(s: &str) -> PResult<Parsed> {
    let b = s.as_bytes();
    let mut p = Parsed::default();
    // A date-time string carrying a time.
    {
        let mut c = C { s: b, i: 0 };
        let mut q = Parsed::default();
        if date_time(&mut c, &mut q).is_ok() && c.done() {
            if q.time.is_none() {
                return Err("a time is required");
            }
            if q.z {
                return Err("a UTC designator is not allowed for a plain time");
            }
            return Ok(q);
        }
    }
    let mut c = C { s: b, i: 0 };
    let designated = c.eat(b'T') || c.eat(b't');
    let t = time(&mut c)?;
    match c.peek() {
        Some(b'Z') | Some(b'z') => return Err("a UTC designator is not allowed for a plain time"),
        Some(b'+') | Some(b'-') => {
            p.offset = Some(utc_offset(&mut c, true)?);
        }
        _ => {}
    }
    annotations(&mut c, &mut p, true)?;
    finish(&c)?;
    if !designated {
        // Ambiguity with DateSpecMonthDay / DateSpecYearMonth (with whatever followed).
        let core_end = b.iter().position(|x| *x == b'[').unwrap_or(b.len());
        let core = &s[..core_end];
        if parse_month_day_core(core).is_ok() || parse_year_month_core(core).is_ok() {
            return Err("ambiguous time string: add a T designator");
        }
    }
    p.time = Some(t);
    Ok(p)
}

fn parse_month_day_core(s: &str) -> PResult<(u8, u8)> {
    let mut c = C { s: s.as_bytes(), i: 0 };
    if c.peek() == Some(b'-') && c.at(1) == Some(b'-') {
        c.i += 2;
    }
    let m = c.digits(2).ok_or("invalid month")?;
    c.eat(b'-');
    let d = c.digits(2).ok_or("invalid day")?;
    finish(&c)?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(1972, m as u8) as i64 {
        return Err("invalid month-day");
    }
    Ok((m as u8, d as u8))
}

fn parse_year_month_core(s: &str) -> PResult<(i64, u8)> {
    let mut c = C { s: s.as_bytes(), i: 0 };
    let y = year(&mut c)?;
    c.eat(b'-');
    let m = c.digits(2).ok_or("invalid month")?;
    finish(&c)?;
    if !(1..=12).contains(&m) {
        return Err("invalid month");
    }
    Ok((y, m as u8))
}

/// TemporalYearMonthString.
pub fn parse_year_month(s: &str) -> PResult<Parsed> {
    if let Ok(p) = parse_date_time(s, Form::DateTime) {
        return Ok(p);
    }
    let end = s.find('[').unwrap_or(s.len());
    let (y, m) = parse_year_month_core(&s[..end])?;
    let mut p = Parsed { year: y, month: m, day: 0, has_date: false, ..Default::default() };
    let mut c = C { s: s.as_bytes(), i: end };
    annotations(&mut c, &mut p, true)?;
    finish(&c)?;
    Ok(p)
}

/// TemporalMonthDayString.
pub fn parse_month_day(s: &str) -> PResult<Parsed> {
    if let Ok(p) = parse_date_time(s, Form::DateTime) {
        return Ok(p);
    }
    let end = s.find('[').unwrap_or(s.len());
    let (m, d) = parse_month_day_core(&s[..end])?;
    let mut p = Parsed { year: 1972, month: m, day: d, has_date: false, ..Default::default() };
    let mut c = C { s: s.as_bytes(), i: end };
    annotations(&mut c, &mut p, true)?;
    finish(&c)?;
    Ok(p)
}

/// ParseDateTimeUTCOffset: an offset string (`offset` property, sub-minute precision allowed).
pub fn parse_offset_string(s: &str) -> PResult<Offset> {
    let mut c = C { s: s.as_bytes(), i: 0 };
    let o = utc_offset(&mut c, true)?;
    finish(&c)?;
    Ok(o)
}

/// What a time zone string names: a bare identifier, or the zone of an ISO string (annotation, Z, offset).
pub enum TzString {
    Id(String),
    Offset(i64),
}

pub fn parse_time_zone_string(s: &str) -> PResult<TzString> {
    if super::zone::parse_identifier(s).is_some() {
        return Ok(TzString::Id(String::from(s)));
    }
    let mut c = C { s: s.as_bytes(), i: 0 };
    let mut p = Parsed::default();
    if date_time(&mut c, &mut p).is_err() || !c.done() {
        let mut c2 = C { s: s.as_bytes(), i: 0 };
        let mut q = Parsed::default();
        let t = if c2.eat(b'T') || c2.eat(b't') { time(&mut c2).is_ok() } else { false };
        if !t {
            return Err("invalid time zone");
        }
        match c2.peek() {
            Some(b'Z') | Some(b'z') => {
                c2.i += 1;
                q.z = true;
            }
            Some(b'+') | Some(b'-') => q.offset = Some(utc_offset(&mut c2, true)?),
            _ => {}
        }
        annotations(&mut c2, &mut q, true)?;
        finish(&c2)?;
        p = q;
    }
    if let Some(tz) = p.tz {
        return Ok(TzString::Id(tz));
    }
    if p.z {
        return Ok(TzString::Id(String::from("UTC")));
    }
    if let Some(o) = p.offset {
        if o.sub_minute {
            return Err("an offset time zone has minute precision");
        }
        return Ok(TzString::Offset(o.ns / 60_000_000_000));
    }
    Err("invalid time zone")
}

/// TemporalDurationString (ISO 8601 durations with the Temporal restrictions on fractions).
pub fn parse_duration(s: &str) -> PResult<Dur> {
    let b = s.as_bytes();
    let mut c = C { s: b, i: 0 };
    let neg = match c.peek() {
        Some(b'-') => {
            c.i += 1;
            true
        }
        Some(b'+') => {
            c.i += 1;
            false
        }
        _ => false,
    };
    if !(c.eat(b'P') || c.eat(b'p')) {
        return Err("a duration starts with P");
    }
    let mut f = [0f64; 10];
    let mut any = false;
    let mut in_time = false;
    let mut last_rank: i32 = -1;
    let mut frac_seen = false;
    let mut time_extra: i128 = 0;
    loop {
        if c.done() {
            break;
        }
        if !in_time && (c.peek() == Some(b'T') || c.peek() == Some(b't')) {
            c.i += 1;
            in_time = true;
            if c.done() {
                return Err("empty time part");
            }
            continue;
        }
        if frac_seen {
            return Err("a fraction is allowed only on the smallest unit");
        }
        let start = c.i;
        while c.peek().map(|x| x.is_ascii_digit()).unwrap_or(false) {
            c.i += 1;
        }
        if c.i == start {
            return Err("expected digits");
        }
        let whole = core::str::from_utf8(&b[start..c.i]).unwrap();
        let fr = fraction(&mut c)?;
        let unit = c.peek().ok_or("missing unit designator")?.to_ascii_uppercase();
        c.i += 1;
        let rank: i32 = match (in_time, unit) {
            (false, b'Y') => 0,
            (false, b'M') => 1,
            (false, b'W') => 2,
            (false, b'D') => 3,
            (true, b'H') => 4,
            (true, b'M') => 5,
            (true, b'S') => 6,
            _ => return Err("invalid duration unit"),
        };
        if rank <= last_rank {
            return Err("duration units out of order");
        }
        last_rank = rank;
        let v: f64 = whole.parse().map_err(|_| "invalid number")?;
        f[rank as usize] = v;
        any = true;
        if let Some(fr) = fr {
            if !in_time {
                return Err("a fraction is allowed only on time units");
            }
            frac_seen = true;
            let unit_ns: i128 = match rank {
                4 => 3_600_000_000_000,
                5 => 60_000_000_000,
                _ => 1_000_000_000,
            };
            time_extra = fr as i128 * (unit_ns / 1_000_000_000);
        }
    }
    if !any {
        return Err("empty duration");
    }
    if in_time && last_rank < 4 {
        return Err("empty time part");
    }
    // Distribute the fraction of the last time unit over the smaller units.
    if time_extra != 0 {
        let mut t = time_extra;
        let start = (last_rank + 1) as usize;
        let lens: [i128; 10] = [0, 0, 0, 0, 0, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
        for k in start..10 {
            let q = t / lens[k];
            t %= lens[k];
            f[k] += q as f64;
        }
    }
    if neg {
        for v in f.iter_mut() {
            if *v != 0.0 {
                *v = -*v;
            }
        }
    }
    let d = Dur { f };
    if !d.is_valid() {
        return Err("duration out of range");
    }
    Ok(d)
}
