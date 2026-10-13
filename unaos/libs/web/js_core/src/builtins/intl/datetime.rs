//! Intl.DateTimeFormat (ECMA-402 §11) and Date.prototype.toLocaleString / toLocaleDateString /
//! toLocaleTimeString (§20.4.1–3) over CLDR 46 (INTLCLDR, SR137): the requested components become a skeleton
//! matched against the locale's availableFormats (the field-distance match of UTS #35 Part 4 §2.6.2.1, field
//! widths then adjusted to the request), dateStyle / timeStyle read dateFormats / timeFormats, the two halves
//! join with the date-time glue of the date's width ("atTime" for full / long), a time zone name is appended
//! with the locale's appendItems pattern, fractional seconds follow the seconds with the locale's decimal
//! sign. Names (months, days, eras, day periods with the flexible dayPeriodRuleSet), the numbering system's
//! digits, the hour cycles (the locale's own, and its 12- and 24-hour variants for hour12) and the zone names
//! (metazones, zone-specific names, the localized GMT format) are the locale's. Calendars: gregory, and the
//! japanese (Meiji 1868 onwards; earlier dates show the Gregorian era), buddhist and roc calendars, whose
//! arithmetic is an era table or a year offset; every other calendar (islamic*, hebrew, chinese, dangi,
//! persian, indian, coptic, ethiopic) is refused by name: it resolves to the locale default.
//! formatRange uses the locale's intervalFormats with the greatest differing field (§11.5.9).

use super::*;
use crate::builtins::date::{date_from_time, day, month_from_time, week_day};
use crate::cldr::{self, slot, L};
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
    /// The locale's default hour cycle is a 24-hour one (what Temporal falls back to when no hour cycle is set).
    pub gb: bool,
    /// The CLDR data of the resolved locale.
    pub loc: L,
    /// The numbering system of the digits.
    pub ns: &'static str,
    /// "gregory", "japanese", "buddhist" or "roc".
    pub cal: &'static str,
    /// The locale's 12-hour and 24-hour cycles (what hour12 true / false select).
    pub hc12: &'static str,
    pub hc24: &'static str,
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
    /// The options named a component or a style (TEMPORAL SR117: a Temporal type with no explicit fields
    /// formats with its own defaults, ECMA-402 Temporal §11.1.x).
    pub explicit: bool,
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
/// The calendars whose arithmetic is here.
pub const CALENDARS: &[&str] = &["buddhist", "gregory", "japanese", "roc"];

/// The hour cycle of a pattern's first hour field.
fn hc_of_pattern(p: &str) -> Option<&'static str> {
    let mut quoted = false;
    for c in p.chars() {
        if c == '\'' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match c {
            'h' => return Some("h12"),
            'H' => return Some("h23"),
            'K' => return Some("h11"),
            'k' => return Some("h24"),
            _ => {}
        }
    }
    None
}

/// The locale's hour cycles: its default (the short time format's), its 12-hour one and its 24-hour one.
pub fn locale_hour_cycles(loc: L) -> (&'static str, &'static str, &'static str) {
    let def = hc_of_pattern(loc.time_format(3)).unwrap_or("h23");
    let h12 = loc.available_format("hm").and_then(hc_of_pattern).filter(|h| *h == "h11" || *h == "h12").unwrap_or("h12");
    let h24 = loc.available_format("Hm").and_then(hc_of_pattern).filter(|h| *h == "h23" || *h == "h24").unwrap_or("h23");
    (def, h12, h24)
}

/// The calendar a locale uses by default (CLDR calendarPreferenceData for the locales here: Thai → buddhist).
pub fn default_calendar(loc: L) -> &'static str {
    if loc.lang() == "th" { "buddhist" } else { "gregory" }
}

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
    let loc = r.loc;
    let (hc_default, hc12, hc24) = locale_hour_cycles(loc);
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
    // The calendar: the option, else the -u-ca- keyword, when it is one of CALENDARS; else the locale default.
    let supported = |c: &str| CALENDARS.iter().copied().find(|x| *x == c);
    let kw_ca = r.keyword("ca").and_then(supported);
    let cal_v = cal.as_deref().and_then(supported).or(kw_ca).unwrap_or(default_calendar(loc));
    let (ns, nu_kept) = numbering_system(&r, nu.as_deref());
    let mut f = DtFmt {
        locale: String::new(),
        gb: hc_default == "h23" || hc_default == "h24",
        loc,
        ns,
        cal: cal_v,
        hc12,
        hc24,
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
        explicit: false,
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
    f.explicit = explicit || date_style.is_some() || time_style.is_some();
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
    // The hour cycle (§11.1.2 steps 42–43): hour12 picks the locale's 12- or 24-hour cycle and overrides
    // hourCycle, which overrides -u-hc-, which overrides the locale's own.
    let kw_hc = r.keyword("hc").and_then(|k| ["h11", "h12", "h23", "h24"].into_iter().find(|h| *h == k));
    if f.hour.is_some() || f.time_style.is_some() {
        f.hc = Some(match (hour12, hc_opt, kw_hc) {
            (Some(true), _, _) => hc12,
            (Some(false), _, _) => hc24,
            (None, Some(h), _) => h,
            (None, None, Some(h)) => h,
            _ => hc_default,
        });
    }
    let mut kept: Vec<(&str, &str)> = Vec::new();
    if let Some(k) = kw_ca {
        if cal.as_deref().map(|c| c == k).unwrap_or(true) {
            kept.push(("ca", k));
        }
    }
    if let Some(h) = kw_hc {
        if hour12.is_none() && hc_opt.map(|x| x == h).unwrap_or(true) {
            kept.push(("hc", h));
        }
    }
    if nu_kept {
        kept.push(("nu", ns));
    }
    f.locale = r.locale_with(&kept);
    Ok(f)
}

// ============================================================================================ fields

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

/// The era (an index into the calendar's era names, with the name slot base) and the year shown in it.
fn era_and_year(f: &DtFmt, c: &Fields) -> (usize, usize, i64) {
    let y = c.year as i64;
    let greg = || if y <= 0 { (slot::ERA_WIDE_0, 0usize, 1 - y) } else { (slot::ERA_WIDE_0, 1usize, y) };
    match f.cal {
        "japanese" => match cldr::japanese_era(y, c.month as u32 + 1, c.date) {
            Some((i, ey)) => (slot::CAL_JAPANESE_ERA_WIDE_0, i, ey),
            None => greg(),
        },
        "buddhist" => (slot::CAL_BUDDHIST_ERA_WIDE_0, 0, y + 543),
        "roc" => {
            if y >= 1912 {
                (slot::CAL_ROC_ERA_WIDE_0, 1, y - 1911)
            } else {
                (slot::CAL_ROC_ERA_WIDE_0, 0, 1912 - y)
            }
        }
        _ => greg(),
    }
}

/// The era name of width 0 wide / 1 abbreviated / 2 narrow.
fn era_name(f: &DtFmt, c: &Fields, width: usize) -> &'static str {
    let (base, i, _) = era_and_year(f, c);
    let per = if base == slot::ERA_WIDE_0 {
        2
    } else if base == slot::CAL_JAPANESE_ERA_WIDE_0 {
        5
    } else if base == slot::CAL_BUDDHIST_ERA_WIDE_0 {
        1
    } else {
        2
    };
    f.loc.get(base + width * per + i).or_else(|| f.loc.get(base + per + i)).unwrap_or("")
}

// ============================================================================================ skeletons

/// The field type of a pattern letter (UTS #35 Part 4 Table "Date Field Symbol Table"), 255 for a non-field.
fn ftype(c: char) -> u8 {
    match c {
        'G' => 0,
        'y' | 'Y' | 'u' | 'U' | 'r' => 1,
        'Q' | 'q' => 2,
        'M' | 'L' => 3,
        'w' | 'W' => 4,
        'd' | 'D' | 'F' | 'g' => 5,
        'E' | 'c' | 'e' => 6,
        'a' | 'b' | 'B' => 7,
        'h' | 'H' | 'K' | 'k' => 8,
        'm' => 9,
        's' => 10,
        'S' | 'A' => 11,
        'z' | 'Z' | 'O' | 'v' | 'V' | 'X' | 'x' => 12,
        _ => 255,
    }
}

/// The letter runs of a skeleton or pattern outside quotes: (letter, length).
fn runs(p: &str) -> Vec<(char, usize)> {
    let mut out: Vec<(char, usize)> = Vec::new();
    let mut quoted = false;
    let mut prev: Option<char> = None;
    for c in p.chars() {
        if c == '\'' {
            quoted = !quoted;
            prev = None;
            continue;
        }
        if quoted || ftype(c) == 255 {
            prev = None;
            continue;
        }
        if prev == Some(c) {
            if let Some(l) = out.last_mut() {
                l.1 += 1;
            }
        } else {
            out.push((c, 1));
        }
        prev = Some(c);
    }
    out
}

/// The skeleton of the date components.
fn date_skeleton(f: &DtFmt) -> String {
    let mut s = String::new();
    if let Some(e) = f.era {
        s.push_str(match e {
            "long" => "GGGG",
            "narrow" => "GGGGG",
            _ => "G",
        });
    }
    if let Some(y) = f.year {
        s.push_str(if y == "2-digit" { "yy" } else { "y" });
    }
    if let Some(m) = f.month {
        s.push_str(match m {
            "numeric" => "M",
            "2-digit" => "MM",
            "short" => "MMM",
            "long" => "MMMM",
            _ => "MMMMM",
        });
    }
    if let Some(w) = f.weekday {
        s.push_str(match w {
            "long" => "EEEE",
            "narrow" => "EEEEE",
            _ => "E",
        });
    }
    if let Some(d) = f.day {
        s.push_str(if d == "2-digit" { "dd" } else { "d" });
    }
    s
}

fn hour_letter(hc: &str) -> char {
    match hc {
        "h11" => 'K',
        "h12" => 'h',
        "h24" => 'k',
        _ => 'H',
    }
}

fn twelve(hc: &str) -> bool {
    hc == "h11" || hc == "h12"
}

fn dp_letters(w: &str) -> &'static str {
    match w {
        "long" => "BBBB",
        "narrow" => "BBBBB",
        _ => "B",
    }
}

/// The skeleton of the time components (no time zone, no fractional seconds).
fn time_skeleton(f: &DtFmt) -> String {
    let mut s = String::new();
    let hc = f.hc.unwrap_or(if f.gb { "h23" } else { "h12" });
    if let Some(h) = f.hour {
        if twelve(hc) {
            if let Some(w) = f.day_period {
                s.push_str(dp_letters(w));
            }
        }
        let c = hour_letter(hc);
        s.push(c);
        if h == "2-digit" {
            s.push(c);
        }
    } else if let Some(w) = f.day_period {
        s.push_str(dp_letters(w));
    }
    if let Some(m) = f.minute {
        s.push_str(if m == "2-digit" { "mm" } else { "m" });
    }
    if let Some(sec) = f.second {
        s.push_str(if sec == "2-digit" { "ss" } else { "s" });
    }
    s
}

/// The distance between a requested skeleton and a candidate (UTS #35 Part 4 §2.6.2.1): a missing field costs
/// more than any width difference, an extra field more than a missing one.
fn distance(req: &[(char, usize)], cand: &[(char, usize)]) -> u32 {
    let mut d = 0u32;
    let req_hour12 = req.iter().any(|(c, _)| *c == 'h' || *c == 'K');
    for t in 0..13u8 {
        let r = req.iter().find(|(c, _)| ftype(*c) == t);
        let c = cand.iter().find(|(c, _)| ftype(*c) == t);
        match (r, c) {
            (None, None) => {}
            (Some(_), None) => d += 0x10000,
            (None, Some((cc, _))) => {
                if !(t == 7 && *cc == 'a' && req_hour12) {
                    d += 0x100000;
                }
            }
            (Some((rc, rl)), Some((cc, cl))) => {
                if t == 8 {
                    let a = matches!(rc, 'h' | 'K');
                    let b = matches!(cc, 'h' | 'K');
                    if a != b {
                        d += 0x1000;
                    } else if rc != cc {
                        d += 0x10;
                    }
                }
                if t == 7 && rc != cc {
                    d += 0x100;
                }
                if t == 3 && ((*rl >= 3) != (*cl >= 3)) {
                    d += 0x1000;
                }
                d += (*rl as i32 - *cl as i32).unsigned_abs();
            }
        }
    }
    d
}

/// The availableFormats pattern for a skeleton in the formatter's calendar: (the matched key, its pattern).
fn best_format(f: &DtFmt, skel: &str) -> (&'static str, String) {
    let req = runs(skel);
    let has_year = req.iter().any(|(c, _)| ftype(*c) == 1);
    let calendar = if f.cal == "gregory" { None } else { Some(["japanese", "buddhist", "roc"].iter().position(|c| *c == f.cal).unwrap_or(0)) };
    let mut best: Option<(u32, &'static str, &'static str)> = None;
    let mut consider = |key: &'static str, pat: &'static str| {
        let dd = distance(&req, &runs(key));
        if best.map(|b| dd < b.0).unwrap_or(true) {
            best = Some((dd, key, pat));
        }
    };
    match calendar {
        Some(ci) if has_year => {
            for &k in cldr::tables::CALAF_KEYS.iter() {
                if let Some(p) = f.loc.calendar_format(ci, k) {
                    consider(k, p);
                }
            }
            for &k in cldr::tables::AF_KEYS.iter().filter(|k| k.contains('G')) {
                if f.loc.calendar_format(ci, k).is_none() {
                    if let Some(p) = f.loc.available_format(k) {
                        consider(k, p);
                    }
                }
            }
        }
        _ => {
            for &k in cldr::tables::AF_KEYS.iter() {
                if let Some(p) = f.loc.available_format(k) {
                    consider(k, p);
                }
            }
        }
    }
    match best {
        Some((dd, k, p)) if dd < 0x10000 => (k, adjust(p, &req)),
        Some((_, k, p)) if req.len() > 1 => (k, adjust(p, &req)),
        _ => ("", String::from(skel)),
    }
}

/// Adjust a matched pattern's field widths to the request (UTS #35 Part 4 §2.6.2.2): text fields take the
/// requested width, numeric fields grow to it, a 2-digit year stays 2-digit; hour letters follow the request.
fn adjust(p: &str, req: &[(char, usize)]) -> String {
    let mut out = String::new();
    let cs: Vec<char> = p.chars().collect();
    let mut i = 0;
    let mut quoted = false;
    while i < cs.len() {
        let c = cs[i];
        if c == '\'' {
            quoted = !quoted;
            out.push(c);
            i += 1;
            continue;
        }
        if quoted || ftype(c) == 255 {
            out.push(c);
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < cs.len() && cs[i + n] == c {
            n += 1;
        }
        i += n;
        let t = ftype(c);
        let (mut letter, mut len) = (c, n);
        if let Some((rc, rl)) = req.iter().find(|(x, _)| ftype(*x) == t) {
            match t {
                3 | 6 | 0 => {
                    let text_req = t != 3 || *rl >= 3;
                    let text_pat = t != 3 || n >= 3;
                    if text_req == text_pat {
                        len = if text_req { *rl } else { n.max(*rl) };
                    }
                }
                1 => {
                    if *rl == 2 {
                        len = 2;
                    }
                }
                8 => {
                    letter = *rc;
                    len = n.max(*rl);
                }
                7 => {
                    if *rc == 'B' {
                        letter = 'B';
                        len = *rl;
                    }
                }
                5 | 9 | 10 => len = n.max(*rl),
                _ => {}
            }
        }
        for _ in 0..len {
            out.push(letter);
        }
    }
    out
}

/// Replace every hour field's letter by the hour cycle's.
fn with_hour_cycle(p: &str, hc: &str) -> String {
    let want = hour_letter(hc);
    let mut quoted = false;
    p.chars()
        .map(|c| {
            if c == '\'' {
                quoted = !quoted;
            }
            if !quoted && ftype(c) == 8 { want } else { c }
        })
        .collect()
}

/// Insert fractional seconds after the seconds field (or at the end), with the locale's decimal sign.
fn with_fraction(p: &str, n: u32, dec: &str) -> String {
    let frac: String = core::iter::repeat_n('S', n as usize).collect();
    let quoted_dec = format!("'{}'", dec.replace('\'', "''"));
    if let Some(pos) = p.rfind('s') {
        let mut s = String::from(&p[..=pos]);
        s.push_str(&quoted_dec);
        s.push_str(&frac);
        s.push_str(&p[pos + 1..]);
        return s;
    }
    if p.is_empty() {
        return frac;
    }
    format!("{p}{quoted_dec}{frac}")
}

/// The pattern letters of a time zone name style.
fn tz_letters(style: &str) -> &'static str {
    match style {
        "long" => "zzzz",
        "shortOffset" => "O",
        "longOffset" => "OOOO",
        "shortGeneric" => "v",
        "longGeneric" => "vvvv",
        _ => "z",
    }
}

fn style_index(s: &str) -> usize {
    match s {
        "full" => 0,
        "long" => 1,
        "medium" => 2,
        _ => 3,
    }
}

/// The date-time glue for a date part: the style of the date (or, for components, of its month and weekday
/// widths), "atTime" for full and long.
fn glue(f: &DtFmt, date_style: Option<usize>, dskel: &str) -> &'static str {
    let style = date_style.unwrap_or_else(|| {
        let r = runs(dskel);
        let month = r.iter().find(|(c, _)| ftype(*c) == 3).map(|x| x.1).unwrap_or(0);
        let wd = r.iter().find(|(c, _)| ftype(*c) == 6).map(|x| x.1).unwrap_or(0);
        if month >= 4 && wd >= 4 {
            0
        } else if month >= 4 {
            1
        } else if month == 3 {
            2
        } else {
            3
        }
    });
    f.loc.date_time_format(style, style <= 1)
}

fn join(glue: &str, date: &str, time: &str) -> String {
    glue.replace("{1}", date).replace("{0}", time)
}

/// The time pattern of a timeStyle under the formatter's hour cycle.
fn style_time_pattern(f: &DtFmt, ts: &str) -> String {
    let p = f.loc.time_format(style_index(ts));
    let hc = f.hc.unwrap_or(if f.gb { "h23" } else { "h12" });
    match hc_of_pattern(p) {
        Some(own) if twelve(own) != twelve(hc) => {
            // Rebuild through the skeleton: the hour field of the requested cycle, minutes, seconds, the zone.
            let r = runs(p);
            let mut skel = String::new();
            skel.push(hour_letter(hc));
            if r.iter().any(|(c, _)| *c == 'm') {
                skel.push_str("mm");
            }
            if r.iter().any(|(c, _)| *c == 's') {
                skel.push_str("ss");
            }
            let mut g = f.clone();
            g.hc = Some(hc);
            let (_, mut tp) = best_format(&g, &skel);
            if let Some((zc, zl)) = r.iter().find(|(c, _)| ftype(*c) == 12) {
                let z: String = core::iter::repeat_n(*zc, *zl).collect();
                tp = f.loc.s(slot::APPEND_TZ).replace("{0}", &tp).replace("{1}", &z);
            }
            with_hour_cycle(&tp, hc)
        }
        _ => with_hour_cycle(p, hc),
    }
}

/// The date pattern of a dateStyle in the formatter's calendar.
fn style_date_pattern(f: &DtFmt, ds: &str) -> String {
    let i = style_index(ds);
    let cal_slot = match f.cal {
        "japanese" => Some(slot::CAL_JAPANESE_DF_FULL),
        "buddhist" => Some(slot::CAL_BUDDHIST_DF_FULL),
        "roc" => Some(slot::CAL_ROC_DF_FULL),
        _ => None,
    };
    String::from(cal_slot.and_then(|s| f.loc.get(s + i)).unwrap_or(f.loc.date_format(i)))
}

/// The pattern a formatter writes: (date part, time part, glue) — empty parts absent.
fn pattern_parts(f: &DtFmt, bc: bool) -> (String, String, &'static str, &'static str, &'static str) {
    if f.date_style.is_some() || f.time_style.is_some() {
        let d = f.date_style.map(|s| style_date_pattern(f, s)).unwrap_or_default();
        let t = f.time_style.map(|s| style_time_pattern(f, s)).unwrap_or_default();
        let g = glue(f, f.date_style.map(style_index), "");
        return (d, t, g, "", "");
    }
    let mut dskel = date_skeleton(f);
    if bc && f.year.is_some() && f.era.is_none() && f.cal == "gregory" {
        dskel.insert(0, 'G');
    }
    let tskel = time_skeleton(f);
    let (dkey, d) = if dskel.is_empty() { ("", String::new()) } else { best_format(f, &dskel) };
    let (tkey, mut t) = if tskel.is_empty() { ("", String::new()) } else { best_format(f, &tskel) };
    if f.hour.is_some() {
        if let Some(hc) = f.hc {
            t = with_hour_cycle(&t, hc);
        }
    }
    if let Some(n) = f.fsd {
        t = with_fraction(&t, n, f.loc.symbol(f.ns, 0));
    }
    if let Some(z) = f.tzn {
        let letters = tz_letters(z);
        t = if t.is_empty() { String::from(letters) } else { f.loc.s(slot::APPEND_TZ).replace("{0}", &t).replace("{1}", letters) };
    }
    let g = glue(f, None, &dskel);
    (d, t, g, dkey, tkey)
}

/// The whole pattern.
fn full_pattern(f: &DtFmt, bc: bool) -> String {
    let (d, t, g, _, _) = pattern_parts(f, bc);
    if d.is_empty() {
        t
    } else if t.is_empty() {
        d
    } else {
        join(g, &d, &t)
    }
}

// ============================================================================================ zone names

/// The localized GMT format (UTS #35 Part 4 §7.3): long "GMT+01:00", short "GMT+1" / "GMT+5:30".
fn gmt(f: &DtFmt, off: f64, long: bool) -> String {
    let m = crate::numconv::libm_floor(off / 60_000.0 + 0.5) as i64;
    if m == 0 {
        let z = f.loc.s(slot::TZ_GMTZEROFORMAT);
        return String::from(if z.is_empty() { "GMT" } else { z });
    }
    let hf = f.loc.s(slot::TZ_HOURFORMAT);
    let hf = if hf.is_empty() { "+HH:mm;-HH:mm" } else { hf };
    let (pos, neg) = hf.split_once(';').unwrap_or((hf, hf));
    let p = if m < 0 { neg } else { pos };
    let a = m.unsigned_abs();
    let (h, mi) = (a / 60, a % 60);
    let body = if long {
        p.replace("HH", &format!("{h:02}")).replace("mm", &format!("{mi:02}"))
    } else {
        let p = p.replace("HH", "H");
        let p = if mi == 0 {
            // drop the minutes and the separator before them
            match p.find("mm") {
                Some(i) => {
                    let mut s = String::from(&p[..i]);
                    while s.ends_with(|c: char| !c.is_alphanumeric() && c != '+' && c != '-' && c != 'H') {
                        s.pop();
                    }
                    s.push_str(&p[i + 2..]);
                    s
                }
                None => p,
            }
        } else {
            p
        };
        p.replace('H', &format!("{h}")).replace("mm", &format!("{mi:02}"))
    };
    let body = super::number::localize_digits(&body, f.ns);
    let g = f.loc.s(slot::TZ_GMTFORMAT);
    let g = if g.is_empty() { "GMT{0}" } else { g };
    g.replace("{0}", &body)
}

/// Whether daylight-saving time is in effect for the formatter's zone at `t` with offset `off`.
fn in_dst(f: &DtFmt, off: f64) -> bool {
    match &f.zone {
        ZoneRule::Posix(p, _) => p.dst.is_some() && off != p.std_offset,
        _ => false,
    }
}

/// The CLDR zone id the names of `zone_id` are filed under.
fn cldr_zone_id(zone_id: &str) -> &str {
    if zone_id == "UTC" { "Etc/UTC" } else { zone_id }
}

fn zone_name(f: &DtFmt, style: &str, off: f64, _t: f64) -> String {
    let id = cldr_zone_id(&f.zone_id);
    let dst = in_dst(f, off);
    let kind = if dst { 2 } else { 1 };
    let meta = cldr::zone_meta(id);
    let utc = f.zone_id == "UTC";
    let named = |short: bool, k: usize| -> Option<&'static str> {
        f.loc.zone_name(id, short, k).or_else(|| if utc && k == 0 { f.loc.zone_name(id, short, 1) } else { None }).or_else(|| meta.and_then(|m| f.loc.metazone_name(m, short, k)))
    };
    let fixed = matches!(f.zone, ZoneRule::Fixed(_)) && !utc;
    let name = match style {
        "short" if !fixed => named(true, kind),
        "long" if !fixed => named(false, kind),
        "shortGeneric" if !fixed => named(true, 0),
        "longGeneric" if !fixed => named(false, 0),
        _ => None,
    };
    match name {
        Some(n) => String::from(n),
        None => gmt(f, off, matches!(style, "long" | "longOffset" | "longGeneric")),
    }
}

// ============================================================================================ formatting

fn p(parts: &mut Vec<(String, String)>, t: &str, v: &str) {
    parts.push((String::from(t), String::from(v)));
}

fn padded(v: i64, n: usize, ns: &str) -> String {
    let s = format!("{:0width$}", v, width = n);
    super::number::localize_digits(&s, ns)
}

/// FormatDateTimePattern (§11.5.5) of a CLDR pattern: (part type, value) for each field and literal.
fn interpret(f: &DtFmt, pat: &str, c: &Fields, off: f64, t: f64) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut lit = String::new();
    let cs: Vec<char> = pat.chars().collect();
    let mut i = 0;
    let mut quoted = false;
    let flush = |out: &mut Vec<(String, String)>, lit: &mut String| {
        if !lit.is_empty() {
            p(out, "literal", lit);
            lit.clear();
        }
    };
    while i < cs.len() {
        let ch = cs[i];
        if ch == '\'' {
            if i + 1 < cs.len() && cs[i + 1] == '\'' {
                lit.push('\'');
                i += 2;
                continue;
            }
            quoted = !quoted;
            i += 1;
            continue;
        }
        if quoted || ftype(ch) == 255 {
            lit.push(ch);
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < cs.len() && cs[i + n] == ch {
            n += 1;
        }
        i += n;
        let loc = f.loc;
        let (ty, val): (&str, String) = match ch {
            'G' => ("era", String::from(era_name(f, c, if n == 4 { 0 } else if n == 5 { 2 } else { 1 }))),
            'y' | 'Y' | 'u' => {
                let (_, _, y) = era_and_year(f, c);
                if n == 2 { ("year", padded(y.rem_euclid(100), 2, f.ns)) } else { ("year", padded(y, n, f.ns)) }
            }
            'M' | 'L' => {
                let ctx = if ch == 'L' { 1 } else { 0 };
                match n {
                    1 | 2 => ("month", padded(c.month as i64 + 1, n, f.ns)),
                    3 => ("month", String::from(loc.month(ctx, 0, c.month))),
                    4 => ("month", String::from(loc.month(ctx, 2, c.month))),
                    _ => ("month", String::from(loc.month(ctx, 1, c.month))),
                }
            }
            'd' => ("day", padded(c.date as i64, n, f.ns)),
            'E' | 'c' | 'e' => {
                let ctx = if ch == 'c' { 1 } else { 0 };
                let w = match n {
                    4 => 3,
                    5 => 1,
                    6 => 2,
                    _ => 0,
                };
                ("weekday", String::from(loc.day(ctx, w, c.weekday)))
            }
            'a' | 'b' | 'B' => {
                let w = match n {
                    4 => 2,
                    5 => 1,
                    _ => 0,
                };
                let ampm = if c.hour < 12 { "am" } else { "pm" };
                let id = match ch {
                    'B' => cldr::flexible_day_period(loc.lang(), c.hour, c.minute).unwrap_or(ampm),
                    'b' if c.minute == 0 && c.second == 0 && c.hour == 12 => "noon",
                    'b' if c.minute == 0 && c.second == 0 && c.hour == 0 => "midnight",
                    _ => ampm,
                };
                ("dayPeriod", String::from(loc.day_period(w, id).or_else(|| loc.day_period(w, ampm)).unwrap_or(ampm)))
            }
            'h' => ("hour", padded(if c.hour % 12 == 0 { 12 } else { c.hour % 12 } as i64, n, f.ns)),
            'K' => ("hour", padded((c.hour % 12) as i64, n, f.ns)),
            'k' => ("hour", padded(if c.hour == 0 { 24 } else { c.hour } as i64, n, f.ns)),
            'H' => ("hour", padded(c.hour as i64, n, f.ns)),
            'm' => ("minute", padded(c.minute as i64, n, f.ns)),
            's' => ("second", padded(c.second as i64, n, f.ns)),
            'S' => {
                let ms = format!("{:03}", c.ms);
                ("fractionalSecond", super::number::localize_digits(&ms[..n.min(3)], f.ns))
            }
            'z' | 'Z' | 'O' | 'v' | 'V' | 'X' | 'x' => {
                let style = match (ch, n >= 4) {
                    ('z', false) => "short",
                    ('z', true) => "long",
                    ('v', false) => "shortGeneric",
                    ('v', true) => "longGeneric",
                    ('O', false) => "shortOffset",
                    _ => "longOffset",
                };
                // A requested timeZoneName style wins over the letters of a style pattern.
                let style = f.tzn.unwrap_or(style);
                ("timeZoneName", zone_name(f, style, off, t))
            }
            _ => continue,
        };
        flush(&mut out, &mut lit);
        p(&mut out, ty, &val);
    }
    flush(&mut out, &mut lit);
    out
}

/// FormatDateTimePattern (§11.5.5) for the time value `t` shown at offset `off`.
pub fn format_parts(f: &DtFmt, t: f64, off: f64) -> Vec<(String, String)> {
    let c = fields(t + off);
    let pat = full_pattern(f, c.year <= 0.0);
    interpret(f, &pat, &c, off, t)
}

// ============================================================================================ ranges

/// The interval pattern split into its first and second halves: the second half starts at the first field
/// whose type the first half already showed.
fn split_interval(pat: &str) -> (String, String) {
    let cs: Vec<char> = pat.chars().collect();
    let mut seen: Vec<u8> = Vec::new();
    let mut i = 0;
    let mut quoted = false;
    while i < cs.len() {
        let c = cs[i];
        if c == '\'' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        if quoted || ftype(c) == 255 {
            i += 1;
            continue;
        }
        let t = ftype(c);
        if seen.contains(&t) {
            let a: String = cs[..i].iter().collect();
            let b: String = cs[i..].iter().collect();
            return (a, b);
        }
        seen.push(t);
        while i < cs.len() && cs[i] == c {
            i += 1;
        }
    }
    (String::from(pat), String::new())
}

/// The greatest field that differs between two instants (§11.5.9 Table 16 order) as a pattern letter, or None.
fn greatest_difference(f: &DtFmt, a: &Fields, b: &Fields, twelve_hour: bool) -> Option<char> {
    let (_, ea, ya) = era_and_year(f, a);
    let (_, eb, yb) = era_and_year(f, b);
    if ea != eb {
        return Some('G');
    }
    if ya != yb {
        return Some('y');
    }
    if a.month != b.month {
        return Some('M');
    }
    if a.date != b.date {
        return Some('d');
    }
    if twelve_hour && (a.hour < 12) != (b.hour < 12) {
        return Some('a');
    }
    if a.hour != b.hour {
        return Some(if twelve_hour { 'h' } else { 'H' });
    }
    if a.minute != b.minute {
        return Some('m');
    }
    if a.second != b.second {
        return Some('s');
    }
    if a.ms != b.ms {
        return Some('S');
    }
    None
}

fn tagged(parts: Vec<(String, String)>, src: &str, out: &mut Vec<(String, String)>, sources: &mut Vec<String>) {
    for x in parts {
        out.push(x);
        sources.push(String::from(src));
    }
}

/// PartitionDateTimeRangePattern (§11.5.9) for two time values.
pub fn range_value_parts(vm: &mut Vm, f: &DtFmt, x: f64, y: f64) -> (Vec<(String, String)>, Vec<String>) {
    let ox = zone_offset_of(vm, f, x);
    let oy = zone_offset_of(vm, f, y);
    let a = format_parts(f, x, ox);
    let b = format_parts(f, y, oy);
    let mut out = Vec::new();
    let mut src = Vec::new();
    if concat_parts(&a) == concat_parts(&b) {
        tagged(a, "shared", &mut out, &mut src);
        return (out, src);
    }
    let ca = fields(x + ox);
    let cb = fields(y + oy);
    let (d, t, g, dkey, tkey) = pattern_parts(f, ca.year <= 0.0 || cb.year <= 0.0);
    let twelve_hour = f.hc.map(twelve).unwrap_or(false) && f.hour.is_some();
    let diff = greatest_difference(f, &ca, &cb, twelve_hour);
    let styles = f.date_style.is_some() || f.time_style.is_some();
    let date_diff = matches!(diff, Some('G' | 'y' | 'M' | 'd'));
    // The skeleton whose intervalFormats apply: the date's when only a date is shown or the date differs.
    let interval = if styles {
        None
    } else if !d.is_empty() && (t.is_empty() || date_diff) {
        if t.is_empty() { diff.and_then(|c| f.loc.interval_format(dkey, c).map(|ip| (ip, true))) } else { None }
    } else if !t.is_empty() && !date_diff && f.tzn.is_none() && f.fsd.is_none() {
        diff.and_then(|c| f.loc.interval_format(tkey, c).map(|ip| (ip, false)))
    } else {
        None
    };
    match interval {
        Some((ip, is_date)) => {
            let req = runs(&if is_date { date_skeleton(f) } else { time_skeleton(f) });
            let mut ip = adjust(ip, &req);
            if !is_date {
                if let Some(hc) = f.hc {
                    ip = with_hour_cycle(&ip, hc);
                }
            }
            let (h1, h2) = split_interval(&ip);
            let first = interpret(f, &h1, &ca, ox, x);
            let second = interpret(f, &h2, &cb, oy, y);
            let mut inner = Vec::new();
            let mut inner_src = Vec::new();
            let n1 = first.len();
            for (k, part) in first.into_iter().enumerate() {
                let s = if k + 1 == n1 && part.0 == "literal" { "shared" } else { "startRange" };
                inner.push(part);
                inner_src.push(String::from(s));
            }
            let first_types: Vec<String> = inner.iter().filter(|p| p.0 != "literal").map(|p| p.0.clone()).collect();
            let srcs2: Vec<&str> = second.iter().map(|p| if p.0 == "literal" || first_types.contains(&p.0) { "endRange" } else { "shared" }).collect();
            for (k, part) in second.into_iter().enumerate() {
                let mut s = srcs2[k];
                if part.0 == "literal" {
                    s = srcs2[k + 1..].iter().copied().find(|_| true).unwrap_or("shared");
                }
                inner.push(part);
                inner_src.push(String::from(s));
            }
            if is_date || d.is_empty() {
                return (inner, inner_src);
            }
            // The date shown once, the time as an interval, joined by the glue.
            let date_parts = interpret(f, &d, &ca, ox, x);
            let gl = g;
            let (pre, rest) = gl.split_once("{1}").map(|(a, b)| (a, b)).unwrap_or(("", gl));
            let mut res = Vec::new();
            let mut rsrc = Vec::new();
            let lit = |s: &str, res: &mut Vec<(String, String)>, rsrc: &mut Vec<String>| {
                let txt = unquote_lit(s);
                if !txt.is_empty() {
                    res.push((String::from("literal"), txt));
                    rsrc.push(String::from("shared"));
                }
            };
            // glue "{1}…{0}" (date first) or "{0}…{1}"
            if let Some((mid, post)) = rest.split_once("{0}") {
                lit(pre, &mut res, &mut rsrc);
                tagged(date_parts, "shared", &mut res, &mut rsrc);
                lit(mid, &mut res, &mut rsrc);
                res.extend(inner);
                rsrc.extend(inner_src);
                lit(post, &mut res, &mut rsrc);
            } else {
                let (pre0, rest0) = gl.split_once("{0}").unwrap_or(("", gl));
                let (mid, post) = rest0.split_once("{1}").unwrap_or((rest0, ""));
                lit(pre0, &mut res, &mut rsrc);
                res.extend(inner);
                rsrc.extend(inner_src);
                lit(mid, &mut res, &mut rsrc);
                tagged(date_parts, "shared", &mut res, &mut rsrc);
                lit(post, &mut res, &mut rsrc);
            }
            (res, rsrc)
        }
        None => fallback_range(f, a, b),
    }
}

/// A pattern literal with its quotes resolved.
fn unquote_lit(s: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\'' {
            if i + 1 < cs.len() && cs[i + 1] == '\'' {
                out.push('\'');
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

/// The locale's intervalFormatFallback ("{0} – {1}") over the two full formats.
fn fallback_range(f: &DtFmt, a: Vec<(String, String)>, b: Vec<(String, String)>) -> (Vec<(String, String)>, Vec<String>) {
    let fb = f.loc.s(slot::IF_FALLBACK);
    let fb = if fb.is_empty() { "{0}\u{2009}–\u{2009}{1}" } else { fb };
    let mut out = Vec::new();
    let mut src = Vec::new();
    let mut rest = fb;
    let mut a = Some(a);
    let mut b = Some(b);
    while let Some(open) = rest.find('{') {
        if open > 0 {
            out.push((String::from("literal"), unquote_lit(&rest[..open])));
            src.push(String::from("shared"));
        }
        let k = rest.as_bytes().get(open + 1).copied();
        if k == Some(b'0') {
            if let Some(x) = a.take() {
                tagged(x, "startRange", &mut out, &mut src);
            }
        } else if let Some(x) = b.take() {
            tagged(x, "endRange", &mut out, &mut src);
        }
        rest = &rest[(open + 3).min(rest.len())..];
    }
    if !rest.is_empty() {
        out.push((String::from("literal"), unquote_lit(rest)));
        src.push(String::from("shared"));
    }
    (out, src)
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
    let off = zone_offset_of(vm, f, t);
    format_parts(f, t, off)
}

/// The formatter's offset at `t`: an IANA zone reads its historical transitions (TEMPORAL SR117,
/// `temporal::zone`), everything else its rule.
pub fn zone_offset_of(vm: &mut Vm, f: &DtFmt, t: f64) -> f64 {
    if let ZoneRule::Posix(..) = &f.zone {
        if let Some(z) = crate::builtins::temporal::zone::parse_identifier(&f.zone_id) {
            if t.is_finite() {
                let ns = (t as i128) * 1_000_000;
                return crate::builtins::temporal::zone::offset_ns(&z, ns) as f64 / 1_000_000.0;
            }
        }
    }
    zone_offset(vm, &f.zone, t)
}

/// format / formatToParts over a Temporal object or a time value.
fn format_any(vm: &mut Vm, f: &DtFmt, v: &Value) -> JsResult<Vec<(String, String)>> {
    if let Some(parts) = crate::builtins::temporal::format_hook(vm, f, v)? {
        return Ok(parts);
    }
    let t = time_arg(vm, v)?;
    Ok(format_value(vm, f, t))
}

fn dtf_format_getter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_dtf, "get Intl.DateTimeFormat.prototype.format")?;
    Ok(bound_getter(vm, o, "", 1, dtf_bound_format))
}

fn dtf_bound_format(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = bound_target(vm, ctx);
    let v = vm.arg(ctx, 0);
    let f = dtf_of(vm, o);
    let parts = format_any(vm, &f, &v)?;
    Ok(Value::str(&concat_parts(&parts)))
}

fn dtf_format_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_dtf, "Intl.DateTimeFormat.prototype.formatToParts")?;
    let v = vm.arg(ctx, 0);
    let f = dtf_of(vm, o);
    let parts = format_any(vm, &f, &v)?;
    Ok(parts_array(vm, &parts, None))
}

/// The range parts of formatRange / formatRangeToParts: time values through the locale's intervalFormats,
/// Temporal values (each formatted with its own defaults) through the interval fallback.
fn range_args(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<(Vec<(String, String)>, Vec<String>)> {
    let o = this_intl(vm, &ctx.this, is_dtf, what)?;
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    if a.is_undefined() || b.is_undefined() {
        return vm.throw_type("startDate or endDate is undefined");
    }
    let f = dtf_of(vm, o);
    let ta = crate::builtins::temporal::tdata(vm, &a).map(|d| d.index());
    let tb = crate::builtins::temporal::tdata(vm, &b).map(|d| d.index());
    if ta != tb {
        return vm.throw_type("formatRange needs two values of the same kind");
    }
    if ta.is_none() {
        let x = time_arg(vm, &a)?;
        let y = time_arg(vm, &b)?;
        return Ok(range_value_parts(vm, &f, x, y));
    }
    let pa = format_any(vm, &f, &a)?;
    let pb = format_any(vm, &f, &b)?;
    if concat_parts(&pa) == concat_parts(&pb) {
        let n = pa.len();
        return Ok((pa, (0..n).map(|_| String::from("shared")).collect()));
    }
    Ok(fallback_range(&f, pa, pb))
}

fn dtf_format_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (parts, _) = range_args(vm, ctx, "Intl.DateTimeFormat.prototype.formatRange")?;
    Ok(Value::str(&concat_parts(&parts)))
}

fn dtf_format_range_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (parts, src) = range_args(vm, ctx, "Intl.DateTimeFormat.prototype.formatRangeToParts")?;
    Ok(parts_array(vm, &parts, Some(("source", &src))))
}

fn dtf_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_dtf, "Intl.DateTimeFormat.prototype.resolvedOptions")?;
    let f = dtf_of(vm, o);
    let mut props: Vec<(&str, Value)> = vec![
        ("locale", Value::str(&f.locale)),
        ("calendar", Value::str(f.cal)),
        ("numberingSystem", Value::str(f.ns)),
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
