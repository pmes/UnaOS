//! Intl.DateTimeFormat (ECMA-402 §11) and Date.prototype.toLocaleString / toLocaleDateString /
//! toLocaleTimeString (§20.4.1–3) for the `en-US` and `en-GB` data: components or dateStyle/timeStyle,
//! hour cycles h11/h12/h23/h24, fractional seconds, time zone names (short/long/offset/generic), the IANA
//! zones of `data::ZONES` as POSIX rules plus offset zones (`+05:30`) and the UTC aliases, formatToParts,
//! formatRange / formatRangeToParts. CLDR 42+'s spacing is reproduced: U+202F NARROW NO-BREAK SPACE before
//! the day period, U+2009 THIN SPACE around the range dash, " at " between a long date and its time.

use super::*;
use crate::builtins::date::{date_from_time, day, month_from_time, week_day};
use crate::tz::{year_of, PosixTz};

const MS_DAY: f64 = 86_400_000.0;

#[derive(Clone, Debug)]
pub enum ZoneRule {
    /// The embedding's local time (`Vm::tz` or `Host::tz_offset_ms`).
    Local,
    Fixed(f64),
    Posix(PosixTz, u8),
}

#[derive(Clone, Debug)]
pub struct DtFmt {
    pub locale: String,
    pub gb: bool,
    pub zone: ZoneRule,
    pub zone_id: String,
    pub hc: Option<&'static str>,
    pub weekday: Option<&'static str>,
    pub era: Option<&'static str>,
    pub year: Option<&'static str>,
    pub month: Option<&'static str>,
    pub day: Option<&'static str>,
    pub day_period: Option<&'static str>,
    pub hour: Option<&'static str>,
    pub minute: Option<&'static str>,
    pub second: Option<&'static str>,
    pub fsd: Option<u32>,
    pub tzn: Option<&'static str>,
    pub date_style: Option<&'static str>,
    pub time_style: Option<&'static str>,
}

fn pick(values: &[&'static str], s: Option<String>) -> Option<&'static str> {
    let s = s?;
    values.iter().copied().find(|v| *v == s)
}

/// An offset time zone string (§21.4.1.33.1 / ECMA-402 IsTimeZoneOffsetString): ±HH, ±HHMM, ±HH:MM.
fn parse_offset(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    let sign = match b.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let rest = &s[1..];
    let (h, m) = match rest.len() {
        2 => (rest, "00"),
        4 => (&rest[..2], &rest[2..]),
        5 if rest.as_bytes()[2] == b':' => (&rest[..2], &rest[3..]),
        _ => return None,
    };
    if !h.bytes().chain(m.bytes()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i32 = h.parse().ok()?;
    let m: i32 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(sign * (h * 60 + m))
}

fn offset_id(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let a = minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", a / 60, a % 60)
}

/// Validate and canonicalize a time zone identifier (§6.5.2 CanonicalizeTimeZoneName, ES2024: UTC aliases
/// canonicalize to "UTC").
pub fn parse_zone(s: &str) -> Option<(ZoneRule, String)> {
    if let Some(m) = parse_offset(s) {
        return Some((ZoneRule::Fixed(m as f64 * 60_000.0), offset_id(m)));
    }
    let u = s.to_ascii_uppercase();
    let utc = [
        "UTC", "ETC/UTC", "ETC/GMT", "GMT", "ETC/UCT", "UCT", "ETC/UNIVERSAL", "UNIVERSAL", "ETC/ZULU", "ZULU", "ETC/GREENWICH", "GREENWICH",
        "ETC/GMT0", "GMT0", "ETC/GMT+0", "ETC/GMT-0", "GMT+0", "GMT-0",
    ];
    if utc.contains(&u.as_str()) {
        return Some((ZoneRule::Fixed(0.0), String::from("UTC")));
    }
    if let Some(rest) = u.strip_prefix("ETC/GMT") {
        let (sign, num) = match rest.as_bytes().first() {
            Some(b'+') => (1i32, &rest[1..]),
            Some(b'-') => (-1i32, &rest[1..]),
            _ => return None,
        };
        let n: i32 = num.parse().ok()?;
        if num.is_empty() || num.len() > 2 || (sign == 1 && n > 12) || (sign == -1 && n > 14) || n == 0 {
            return None;
        }
        // Etc/GMT+5 is five hours WEST of Greenwich.
        return Some((ZoneRule::Fixed(-(sign * n) as f64 * 3_600_000.0), format!("Etc/GMT{}{}", if sign == 1 { '+' } else { '-' }, n)));
    }
    let z = data::zone(s)?;
    let rule = PosixTz::parse(z.posix)?;
    Some((ZoneRule::Posix(rule, z.meta), String::from(z.id)))
}

/// DefaultTimeZone (§21.4.1.24 SystemTimeZoneIdentifier): the host's IANA id when it names one of the
/// zones in the data, otherwise UTC or the current local offset.
pub fn default_zone(vm: &mut Vm) -> (ZoneRule, String) {
    if let Some(id) = vm.host.time_zone_id() {
        if let Some(z) = parse_zone(&id) {
            return z;
        }
    }
    let now = vm.host.now_ms();
    let off = crate::builtins::date::local_time(vm, now) - now;
    let has_dst = vm.tz.as_ref().map(|z| z.dst.is_some()).unwrap_or(false);
    if off == 0.0 && !has_dst {
        return (ZoneRule::Fixed(0.0), String::from("UTC"));
    }
    (ZoneRule::Local, offset_id((off / 60_000.0) as i32))
}

pub fn zone_offset(vm: &mut Vm, z: &ZoneRule, t: f64) -> f64 {
    match z {
        ZoneRule::Local => crate::builtins::date::local_time(vm, t) - t,
        ZoneRule::Fixed(o) => *o,
        ZoneRule::Posix(p, _) => p.offset_at_utc(t),
    }
}

const TEXT: &[&str] = &["narrow", "short", "long"];
const NUM: &[&str] = &["numeric", "2-digit"];

/// CreateDateTimeFormat (§11.1.2). `required`: "date" / "time" / "any"; `defaults`: "date" / "time" / "all".
pub fn create(vm: &mut Vm, locales: &Value, options: &Value, required: &str, defaults: &str) -> JsResult<DtFmt> {
    let requested = canonicalize_locale_list(vm, locales)?;
    let o = coerce_options(vm, options)?;
    let svc = "Intl.DateTimeFormat";
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], svc)?;
    let cal = get_string_option(vm, o, "calendar", &[], svc)?;
    if let Some(c) = &cal {
        if !c.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid calendar : {c}"));
        }
    }
    let nu = get_string_option(vm, o, "numberingSystem", &[], svc)?;
    if let Some(c) = &nu {
        if !c.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid numberingSystem : {c}"));
        }
    }
    let hour12 = get_bool_option(vm, o, "hour12")?;
    let hc_opt = pick(&["h11", "h12", "h23", "h24"], get_string_option(vm, o, "hourCycle", &["h11", "h12", "h23", "h24"], svc)?);
    let r = resolve_locale(&requested, &["ca", "hc", "nu"]);
    let gb = is_gb(r.data_locale);
    let tzv = get_prop(vm, o, "timeZone")?;
    let (zone, zone_id) = if tzv.is_undefined() {
        default_zone(vm)
    } else {
        let s = vm.to_string(&tzv)?.to_rust();
        match parse_zone(&s) {
            Some(z) => z,
            None => return vm.throw_range(&format!("Invalid time zone specified: {s}")),
        }
    };
    let weekday = pick(TEXT, get_string_option(vm, o, "weekday", TEXT, svc)?);
    let era = pick(TEXT, get_string_option(vm, o, "era", TEXT, svc)?);
    let year = pick(NUM, get_string_option(vm, o, "year", NUM, svc)?);
    let month_vals = &["numeric", "2-digit", "narrow", "short", "long"];
    let month = pick(month_vals, get_string_option(vm, o, "month", month_vals, svc)?);
    let dayv = pick(NUM, get_string_option(vm, o, "day", NUM, svc)?);
    let day_period = pick(TEXT, get_string_option(vm, o, "dayPeriod", TEXT, svc)?);
    let hour = pick(NUM, get_string_option(vm, o, "hour", NUM, svc)?);
    let minute = pick(NUM, get_string_option(vm, o, "minute", NUM, svc)?);
    let second = pick(NUM, get_string_option(vm, o, "second", NUM, svc)?);
    let fsd = get_number_option(vm, o, "fractionalSecondDigits", 1.0, 3.0)?;
    let tzn_vals = &["short", "long", "shortOffset", "longOffset", "shortGeneric", "longGeneric"];
    let tzn = pick(tzn_vals, get_string_option(vm, o, "timeZoneName", tzn_vals, svc)?);
    get_string_option(vm, o, "formatMatcher", &["basic", "best fit"], svc)?;
    let styles = &["full", "long", "medium", "short"];
    let date_style = pick(styles, get_string_option(vm, o, "dateStyle", styles, svc)?);
    let time_style = pick(styles, get_string_option(vm, o, "timeStyle", styles, svc)?);
    let mut f = DtFmt {
        locale: String::new(),
        gb,
        zone,
        zone_id,
        hc: None,
        weekday,
        era,
        year,
        month,
        day: dayv,
        day_period,
        hour,
        minute,
        second,
        fsd,
        tzn,
        date_style,
        time_style,
    };
    let explicit = weekday.is_some()
        || era.is_some()
        || year.is_some()
        || month.is_some()
        || dayv.is_some()
        || day_period.is_some()
        || hour.is_some()
        || minute.is_some()
        || second.is_some()
        || fsd.is_some()
        || tzn.is_some();
    if date_style.is_some() || time_style.is_some() {
        if explicit {
            return vm.throw_type("Can't set option dateStyle or timeStyle together with other date/time fields");
        }
        if required == "date" && time_style.is_some() {
            return vm.throw_type("Invalid option : timeStyle");
        }
        if required == "time" && date_style.is_some() {
            return vm.throw_type("Invalid option : dateStyle");
        }
    } else {
        let need_date = weekday.is_none() && year.is_none() && month.is_none() && dayv.is_none();
        let need_time = day_period.is_none() && hour.is_none() && minute.is_none() && second.is_none() && fsd.is_none();
        let need = match required {
            "date" => need_date,
            "time" => need_time,
            _ => need_date && need_time,
        };
        if need {
            if defaults == "date" || defaults == "all" {
                f.year = Some("numeric");
                f.month = Some("numeric");
                f.day = Some("numeric");
            }
            if defaults == "time" || defaults == "all" {
                f.hour = Some("numeric");
                f.minute = Some("numeric");
                f.second = Some("numeric");
            }
        }
    }
    // The hour cycle (§11.1.2 steps 42–43): hour12 overrides hourCycle, which overrides -u-hc-.
    let kw_hc = r.keyword("hc").and_then(|k| ["h11", "h12", "h23", "h24"].into_iter().find(|h| *h == k));
    let locale_hc = if gb { "h23" } else { "h12" };
    if f.hour.is_some() || f.time_style.is_some() {
        f.hc = Some(match (hour12, hc_opt, kw_hc) {
            (Some(true), _, _) => "h12",
            (Some(false), _, _) => "h23",
            (None, Some(h), _) => h,
            (None, None, Some(h)) => h,
            _ => locale_hc,
        });
    }
    let mut kept: Vec<(&str, &str)> = Vec::new();
    if r.keyword("ca") == Some("gregory") && cal.as_deref().map(|c| c == "gregory").unwrap_or(true) {
        kept.push(("ca", "gregory"));
    }
    if let Some(h) = kw_hc {
        if hour12.is_none() && hc_opt.map(|x| x == h).unwrap_or(true) {
            kept.push(("hc", h));
        }
    }
    if r.keyword("nu") == Some("latn") && nu.as_deref().map(|c| c == "latn").unwrap_or(true) {
        kept.push(("nu", "latn"));
    }
    f.locale = r.locale_with(&kept);
    Ok(f)
}

// ============================================================================================ formatting

struct Fields {
    year: f64,
    month: usize,
    date: u32,
    weekday: usize,
    hour: u32,
    minute: u32,
    second: u32,
    ms: u32,
}

fn fields(lt: f64) -> Fields {
    let d = day(lt);
    let tid = lt - d * MS_DAY;
    let tid = if tid < 0.0 { tid + MS_DAY } else { tid } as u64;
    Fields {
        year: year_of(lt),
        month: month_from_time(lt) as usize,
        date: date_from_time(lt) as u32,
        weekday: week_day(lt) as usize,
        hour: (tid / 3_600_000) as u32,
        minute: ((tid / 60_000) % 60) as u32,
        second: ((tid / 1000) % 60) as u32,
        ms: (tid % 1000) as u32,
    }
}

fn gmt_short(off: f64) -> String {
    let m = (off / 60_000.0) as i32;
    if m == 0 {
        return String::from("GMT");
    }
    let sign = if m < 0 { '-' } else { '+' };
    let a = m.unsigned_abs();
    if a % 60 == 0 { format!("GMT{sign}{}", a / 60) } else { format!("GMT{sign}{}:{:02}", a / 60, a % 60) }
}

fn gmt_long(off: f64) -> String {
    let m = (off / 60_000.0) as i32;
    if m == 0 {
        return String::from("GMT");
    }
    let sign = if m < 0 { '-' } else { '+' };
    let a = m.unsigned_abs();
    format!("GMT{sign}{:02}:{:02}", a / 60, a % 60)
}

fn zone_name(f: &DtFmt, style: &str, off: f64) -> String {
    if f.zone_id == "UTC" {
        return String::from(match style {
            "long" | "longGeneric" => "Coordinated Universal Time",
            "short" | "shortGeneric" => "UTC",
            _ => "GMT",
        });
    }
    let (meta, dst) = match &f.zone {
        ZoneRule::Posix(p, m) if *m != 0 => (Some(&data::METAS[*m as usize]), p.dst.is_some() && off != p.std_offset),
        _ => (None, false),
    };
    match (style, meta) {
        ("short", Some(m)) if (m.us_abbr && !f.gb) || (m.gb_abbr && f.gb) => String::from(if dst { m.abbr_dst } else { m.abbr_std }),
        ("long", Some(m)) => String::from(if dst { m.long_dst } else { m.long_std }),
        ("shortGeneric", Some(m)) if (m.us_abbr && !f.gb) || (m.gb_abbr && f.gb) => String::from(m.generic_short),
        ("longGeneric", Some(m)) => String::from(m.generic_long),
        ("long" | "longOffset" | "longGeneric", _) => gmt_long(off),
        _ => gmt_short(off),
    }
}

fn p(parts: &mut Vec<(String, String)>, t: &str, v: &str) {
    parts.push((String::from(t), String::from(v)));
}

fn num(v: u32, two: bool) -> String {
    if two { format!("{v:02}") } else { format!("{v}") }
}

/// The components a dateStyle / timeStyle stands for in the en data.
fn expand_styles(f: &DtFmt) -> DtFmt {
    let mut g = f.clone();
    if let Some(ds) = f.date_style {
        match (ds, f.gb) {
            ("full", _) => {
                g.weekday = Some("long");
                g.month = Some("long");
                g.day = Some("numeric");
                g.year = Some("numeric");
            }
            ("long", _) => {
                g.month = Some("long");
                g.day = Some("numeric");
                g.year = Some("numeric");
            }
            ("medium", _) => {
                g.month = Some("short");
                g.day = Some("numeric");
                g.year = Some("numeric");
            }
            (_, false) => {
                g.month = Some("numeric");
                g.day = Some("numeric");
                g.year = Some("2-digit");
            }
            (_, true) => {
                g.month = Some("2-digit");
                g.day = Some("2-digit");
                g.year = Some("numeric");
            }
        }
    }
    if let Some(ts) = f.time_style {
        g.hour = Some(if f.gb { "2-digit" } else { "numeric" });
        g.minute = Some("2-digit");
        if ts != "short" {
            g.second = Some("2-digit");
        }
        match ts {
            "full" => g.tzn = Some("long"),
            "long" => g.tzn = Some("short"),
            _ => {}
        }
    }
    g
}

fn day_period_phrase(h: u32, width: &str) -> &'static str {
    match (h, width) {
        (12, "narrow") => "n",
        (12, _) => "noon",
        (6..=11, _) => "in the morning",
        (13..=17, _) => "in the afternoon",
        (18..=20, _) => "in the evening",
        _ => "at night",
    }
}

/// FormatDateTimePattern (§11.5.5) over the en patterns.
pub fn format_parts(f0: &DtFmt, t: f64, off: f64) -> Vec<(String, String)> {
    let f = expand_styles(f0);
    let c = fields(t + off);
    let mut date: Vec<(String, String)> = Vec::new();
    let bc = c.year <= 0.0;
    let disp_year = if bc { 1.0 - c.year } else { c.year };
    let year_s = match f.year {
        Some("2-digit") => format!("{:02}", (disp_year as i64).rem_euclid(100)),
        Some(_) => format!("{}", disp_year as i64),
        None => String::new(),
    };
    let text_month = matches!(f.month, Some("narrow" | "short" | "long"));
    let wd_s = f.weekday.map(|w| match w {
        "narrow" => data::DAYS_NARROW[c.weekday],
        "short" => data::DAYS_SHORT[c.weekday],
        _ => data::DAYS_LONG[c.weekday],
    });
    let numeric_count = [f.year.is_some(), f.month.is_some(), f.day.is_some()].iter().filter(|x| **x).count();
    if !text_month {
        let gb_pad = f.gb && numeric_count >= 2;
        let mut pieces: Vec<(&str, String)> = Vec::new();
        let m_s = f.month.map(|m| num(c.month as u32 + 1, m == "2-digit" || gb_pad));
        let d_s = f.day.map(|d| num(c.date, d == "2-digit" || gb_pad));
        let order: [&str; 3] = if f.gb { ["day", "month", "year"] } else { ["month", "day", "year"] };
        for k in order {
            match k {
                "month" => {
                    if let Some(m) = &m_s {
                        pieces.push(("month", m.clone()));
                    }
                }
                "day" => {
                    if let Some(d) = &d_s {
                        pieces.push(("day", d.clone()));
                    }
                }
                _ => {
                    if f.year.is_some() {
                        pieces.push(("year", year_s.clone()));
                    }
                }
            }
        }
        if let Some(w) = wd_s {
            p(&mut date, "weekday", w);
            if !pieces.is_empty() {
                p(&mut date, "literal", ", ");
            }
        }
        for (i, (k, v)) in pieces.iter().enumerate() {
            if i > 0 {
                p(&mut date, "literal", "/");
            }
            p(&mut date, k, v);
        }
    } else {
        let m_name = match f.month {
            Some("narrow") => data::MONTHS_NARROW[c.month],
            Some("short") => data::MONTHS_SHORT[c.month],
            _ => data::MONTHS_LONG[c.month],
        };
        let d_s = f.day.map(|d| num(c.date, d == "2-digit"));
        if f.gb {
            if let Some(w) = wd_s {
                p(&mut date, "weekday", w);
                p(&mut date, "literal", if d_s.is_some() { " " } else { ", " });
            }
            if let Some(d) = &d_s {
                p(&mut date, "day", d);
                p(&mut date, "literal", " ");
            }
            p(&mut date, "month", m_name);
            if f.year.is_some() {
                p(&mut date, "literal", " ");
                p(&mut date, "year", &year_s);
            }
        } else {
            if let Some(w) = wd_s {
                p(&mut date, "weekday", w);
                p(&mut date, "literal", ", ");
            }
            p(&mut date, "month", m_name);
            if let Some(d) = &d_s {
                p(&mut date, "literal", " ");
                p(&mut date, "day", d);
            }
            if f.year.is_some() {
                p(&mut date, "literal", if d_s.is_some() { ", " } else { " " });
                p(&mut date, "year", &year_s);
            }
        }
    }
    if numeric_count == 0 && !text_month && wd_s.is_none() {
        date.clear();
    }
    let era_w = f.era.or(if bc && f.year.is_some() { Some("short") } else { None });
    if let Some(e) = era_w {
        let name = match (e, bc) {
            ("long", false) => "Anno Domini",
            ("long", true) => "Before Christ",
            ("narrow", false) => "A",
            ("narrow", true) => "B",
            (_, false) => "AD",
            (_, true) => "BC",
        };
        if !date.is_empty() {
            p(&mut date, "literal", " ");
        }
        p(&mut date, "era", name);
    }
    // ---- time
    let mut time: Vec<(String, String)> = Vec::new();
    let hc = f.hc.unwrap_or(if f.gb { "h23" } else { "h12" });
    let twelve = hc == "h11" || hc == "h12";
    if let Some(h) = f.hour {
        let hv = match hc {
            "h11" => c.hour % 12,
            "h12" => {
                if c.hour % 12 == 0 {
                    12
                } else {
                    c.hour % 12
                }
            }
            "h24" => {
                if c.hour == 0 {
                    24
                } else {
                    c.hour
                }
            }
            _ => c.hour,
        };
        p(&mut time, "hour", &num(hv, h == "2-digit" || !twelve));
    }
    if let Some(m) = f.minute {
        if !time.is_empty() {
            p(&mut time, "literal", ":");
        }
        p(&mut time, "minute", &num(c.minute, m == "2-digit" || f.hour.is_some() || f.second.is_some()));
    }
    if let Some(s) = f.second {
        if !time.is_empty() {
            p(&mut time, "literal", ":");
        }
        p(&mut time, "second", &num(c.second, s == "2-digit" || f.minute.is_some() || f.hour.is_some()));
    }
    if let Some(n) = f.fsd {
        let ms = format!("{:03}", c.ms);
        if !time.is_empty() {
            p(&mut time, "literal", ".");
        }
        p(&mut time, "fractionalSecond", &ms[..n as usize]);
    }
    if f.hour.is_some() && twelve {
        match f.day_period {
            Some(w) => {
                p(&mut time, "literal", " ");
                p(&mut time, "dayPeriod", day_period_phrase(c.hour, w));
            }
            None => {
                p(&mut time, "literal", "\u{202F}");
                p(&mut time, "dayPeriod", if c.hour < 12 { "AM" } else { "PM" });
            }
        }
    } else if f.hour.is_none() {
        if let Some(w) = f.day_period {
            if !time.is_empty() {
                p(&mut time, "literal", " ");
            }
            p(&mut time, "dayPeriod", day_period_phrase(c.hour, w));
        }
    }
    if let Some(z) = f.tzn {
        if !time.is_empty() || !date.is_empty() {
            p(&mut time, "literal", " ");
        }
        p(&mut time, "timeZoneName", &zone_name(&f, z, off));
    }
    if date.is_empty() {
        return time;
    }
    if time.is_empty() {
        return date;
    }
    let at = match f0.date_style {
        Some("full" | "long") => true,
        Some(_) => false,
        None => f.month == Some("long"),
    };
    // A lone time zone name joins with the space it already carries.
    if time.first().map(|x| x.0 == "literal").unwrap_or(false) {
        date.extend(time);
        return date;
    }
    p(&mut date, "literal", if at { " at " } else { ", " });
    date.extend(time);
    date
}

fn range_parts(f: &DtFmt, a: &[(String, String)], b: &[(String, String)]) -> (Vec<(String, String)>, Vec<String>) {
    let _ = f;
    let mut parts = Vec::new();
    let mut src = Vec::new();
    if concat_parts(a) == concat_parts(b) {
        for x in a {
            parts.push(x.clone());
            src.push(String::from("shared"));
        }
        return (parts, src);
    }
    for x in a {
        parts.push(x.clone());
        src.push(String::from("startRange"));
    }
    parts.push((String::from("literal"), String::from("\u{2009}–\u{2009}")));
    src.push(String::from("shared"));
    for x in b {
        parts.push(x.clone());
        src.push(String::from("endRange"));
    }
    (parts, src)
}

// ============================================================================================ JS surface

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "DateTimeFormat", 0, dtf_ctor);
    accessor(vm, p, PropertyKey::from_str("format"), "format", Some(dtf_format_getter), None, C);
    method(vm, p, "formatToParts", 1, dtf_format_to_parts);
    method(vm, p, "formatRange", 2, dtf_format_range);
    method(vm, p, "formatRangeToParts", 2, dtf_format_range_to_parts);
    method(vm, p, "resolvedOptions", 0, dtf_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_date_time_format_proto = p;
}

fn dtf_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let nt = if ctx.new_target.is_undefined() { Value::Object(ctx.callee) } else { ctx.new_target.clone() };
    let p = vm.get_prototype_from_ctor(&nt, |i| i.intl_date_time_format_proto)?;
    let l = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let f = create(vm, &l, &o, "any", "date")?;
    Ok(Value::Object(alloc_intl(vm, p, IntlData::DateTimeFormat(Box::new(f)))))
}

fn is_dtf(d: &IntlData) -> bool {
    matches!(d, IntlData::DateTimeFormat(_))
}

fn dtf_of(vm: &Vm, o: Obj) -> DtFmt {
    match intl_ref(vm, o) {
        IntlData::DateTimeFormat(f) => (**f).clone(),
        _ => unreachable!(),
    }
}

/// The time value a format call reads: undefined → now; otherwise ToNumber + TimeClip, NaN a RangeError.
fn time_arg(vm: &mut Vm, v: &Value) -> JsResult<f64> {
    let t = if v.is_undefined() { vm.host.now_ms() } else { vm.to_number(v)? };
    let t = crate::builtins::date::time_clip(t);
    if t.is_nan() {
        return vm.throw_range("Invalid time value");
    }
    Ok(t)
}

pub fn format_value(vm: &mut Vm, f: &DtFmt, t: f64) -> Vec<(String, String)> {
    let off = zone_offset(vm, &f.zone, t);
    format_parts(f, t, off)
}

fn dtf_format_getter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_dtf, "get Intl.DateTimeFormat.prototype.format")?;
    Ok(bound_getter(vm, o, "", 1, dtf_bound_format))
}

fn dtf_bound_format(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = bound_target(vm, ctx);
    let v = vm.arg(ctx, 0);
    let t = time_arg(vm, &v)?;
    let f = dtf_of(vm, o);
    let parts = format_value(vm, &f, t);
    Ok(Value::str(&concat_parts(&parts)))
}

fn dtf_format_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_dtf, "Intl.DateTimeFormat.prototype.formatToParts")?;
    let v = vm.arg(ctx, 0);
    let t = time_arg(vm, &v)?;
    let f = dtf_of(vm, o);
    let parts = format_value(vm, &f, t);
    Ok(parts_array(vm, &parts, None))
}

fn range_args(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<(DtFmt, Vec<(String, String)>, Vec<(String, String)>)> {
    let o = this_intl(vm, &ctx.this, is_dtf, what)?;
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    if a.is_undefined() || b.is_undefined() {
        return vm.throw_type("startDate or endDate is undefined");
    }
    let x = time_arg(vm, &a)?;
    let y = time_arg(vm, &b)?;
    let f = dtf_of(vm, o);
    let pa = format_value(vm, &f, x);
    let pb = format_value(vm, &f, y);
    Ok((f, pa, pb))
}

fn dtf_format_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (f, a, b) = range_args(vm, ctx, "Intl.DateTimeFormat.prototype.formatRange")?;
    let (parts, _) = range_parts(&f, &a, &b);
    Ok(Value::str(&concat_parts(&parts)))
}

fn dtf_format_range_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (f, a, b) = range_args(vm, ctx, "Intl.DateTimeFormat.prototype.formatRangeToParts")?;
    let (parts, src) = range_parts(&f, &a, &b);
    Ok(parts_array(vm, &parts, Some(("source", &src))))
}

fn dtf_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_dtf, "Intl.DateTimeFormat.prototype.resolvedOptions")?;
    let f = dtf_of(vm, o);
    let mut props: Vec<(&str, Value)> = vec![
        ("locale", Value::str(&f.locale)),
        ("calendar", Value::str("gregory")),
        ("numberingSystem", Value::str("latn")),
        ("timeZone", Value::str(&f.zone_id)),
    ];
    if let Some(h) = f.hc {
        props.push(("hourCycle", Value::str(h)));
        props.push(("hour12", Value::Bool(h == "h11" || h == "h12")));
    }
    if f.date_style.is_none() && f.time_style.is_none() {
        for (k, v) in [
            ("weekday", f.weekday),
            ("era", f.era),
            ("year", f.year),
            ("month", f.month),
            ("day", f.day),
            ("dayPeriod", f.day_period),
            ("hour", f.hour),
            ("minute", f.minute),
            ("second", f.second),
        ] {
            if let Some(v) = v {
                // In a pattern with an hour, minutes and seconds are always two digits.
                let v = if (k == "minute" || k == "second") && f.hour.is_some() { "2-digit" } else { v };
                props.push((k, Value::str(v)));
            }
        }
        if let Some(n) = f.fsd {
            props.push(("fractionalSecondDigits", Value::Number(n as f64)));
        }
        if let Some(z) = f.tzn {
            props.push(("timeZoneName", Value::str(z)));
        }
    }
    if let Some(s) = f.date_style {
        props.push(("dateStyle", Value::str(s)));
    }
    if let Some(s) = f.time_style {
        props.push(("timeStyle", Value::str(s)));
    }
    let r = object_from(vm, props);
    Ok(Value::Object(r))
}

/// Date.prototype.toLocaleString / toLocaleDateString / toLocaleTimeString (§20.4.1–3).
pub fn date_to_locale(vm: &mut Vm, t: f64, locales: &Value, options: &Value, required: &str, defaults: &str) -> JsResult<Value> {
    if t.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let f = create(vm, locales, options, required, defaults)?;
    let parts = format_value(vm, &f, t);
    Ok(Value::str(&concat_parts(&parts)))
}
