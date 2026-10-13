//! INTLCLDR (LEDGER SR137): the IANA time zone database (tzdata 2025b, `js_core::tzdata`) through
//! `tz::PosixTz::named`, Date, Intl.DateTimeFormat and Temporal. The expected offsets are read off the tz
//! source rules by hand: America/New_York's 2007 US rule change (Energy Policy Act of 2005), Europe/Moscow's
//! permanent +04 of 2011-03-27 and return to +03 on 2014-10-26, Asia/Tokyo without daylight time since 1951,
//! Australia/Lord_Howe's 30-minute daylight shift, Pacific/Apia skipping 2011-12-30 across the date line.

use js_core::tz::PosixTz;
use js_core::vm::{Host, NullHost, Value, Vm};

const H: f64 = 3_600_000.0;

/// Epoch milliseconds of a proleptic Gregorian UTC date-time.
fn utc(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> f64 {
    // days_from_civil (H. Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 86_400 + h * 3600 + mi * 60 + s) * 1000) as f64
}

fn zone(id: &str) -> PosixTz {
    PosixTz::named(id).unwrap_or_else(|| panic!("{id} missing from tzdata"))
}

#[test]
fn tz_new_york_2007_rule_change() {
    let z = zone("America/New_York");
    // 2006: first Sunday of April to last Sunday of October; 2007 on: second Sunday of March to first of November.
    assert_eq!(z.offset_at_utc(utc(2006, 3, 15, 12, 0, 0)), -5.0 * H);
    assert_eq!(z.offset_at_utc(utc(2007, 3, 15, 12, 0, 0)), -4.0 * H);
    assert_eq!(z.offset_at_utc(utc(2006, 11, 1, 12, 0, 0)), -5.0 * H);
    assert_eq!(z.offset_at_utc(utc(2007, 11, 1, 12, 0, 0)), -4.0 * H);
    // 2007-03-11 02:00 EST = 07:00 UTC.
    assert_eq!(z.offset_at_utc(utc(2007, 3, 11, 6, 59, 59)), -5.0 * H);
    assert_eq!(z.offset_at_utc(utc(2007, 3, 11, 7, 0, 0)), -4.0 * H);
    assert!(z.is_dst_at_utc(utc(2007, 3, 15, 12, 0, 0)));
    assert!(!z.is_dst_at_utc(utc(2006, 3, 15, 12, 0, 0)));
}

#[test]
fn tz_moscow_2011_and_2014() {
    let z = zone("Europe/Moscow");
    // 2011-03-27 02:00 MSK (+03) became 03:00 +04 for good: 2011-03-26T23:00Z.
    assert_eq!(z.offset_at_utc(utc(2011, 3, 26, 22, 59, 59)), 3.0 * H);
    assert_eq!(z.offset_at_utc(utc(2011, 3, 26, 23, 0, 0)), 4.0 * H);
    // +04 all year 2012-2014, as standard time.
    assert_eq!(z.offset_at_utc(utc(2012, 7, 1, 0, 0, 0)), 4.0 * H);
    assert_eq!(z.offset_at_utc(utc(2013, 1, 1, 0, 0, 0)), 4.0 * H);
    assert!(!z.is_dst_at_utc(utc(2012, 7, 1, 0, 0, 0)));
    // 2014-10-26 02:00 +04 went back to 01:00 +03: 2014-10-25T22:00Z.
    assert_eq!(z.offset_at_utc(utc(2014, 10, 25, 21, 59, 59)), 4.0 * H);
    assert_eq!(z.offset_at_utc(utc(2014, 10, 25, 22, 0, 0)), 3.0 * H);
    assert_eq!(z.offset_at_utc(utc(2020, 7, 1, 0, 0, 0)), 3.0 * H);
}

#[test]
fn tz_tokyo_without_daylight_time() {
    let z = zone("Asia/Tokyo");
    for (y, m) in [(1970, 1), (2000, 7), (2020, 1), (2020, 7), (2040, 7)] {
        assert_eq!(z.offset_at_utc(utc(y, m, 1, 0, 0, 0)), 9.0 * H);
        assert!(!z.is_dst_at_utc(utc(y, m, 1, 0, 0, 0)));
    }
    // A local time reads back the same instant.
    let local = utc(2020, 7, 1, 9, 0, 0);
    assert_eq!(z.offset_for_local(local), 9.0 * H);
}

#[test]
fn tz_lord_howe_half_hour_daylight_time() {
    let z = zone("Australia/Lord_Howe");
    // +10:30 standard, +11:00 daylight (the only 30-minute shift in the database).
    assert_eq!(z.offset_at_utc(utc(2024, 1, 15, 0, 0, 0)), 11.0 * H);
    assert_eq!(z.offset_at_utc(utc(2024, 7, 15, 0, 0, 0)), 10.5 * H);
    assert!(z.is_dst_at_utc(utc(2024, 1, 15, 0, 0, 0)));
    // After 2037 the POSIX rule answers.
    assert_eq!(z.offset_at_utc(utc(2050, 1, 15, 0, 0, 0)), 11.0 * H);
    assert_eq!(z.offset_at_utc(utc(2050, 7, 15, 0, 0, 0)), 10.5 * H);
    check(&[(
        "new Intl.DateTimeFormat('en-US', {timeZone: 'Australia/Lord_Howe', timeZoneName: 'longOffset'}).format(Date.UTC(2024, 0, 15))",
        "1/15/2024, GMT+11:00",
    )]);
}

#[test]
fn tz_apia_2011_date_line_jump() {
    let z = zone("Pacific/Apia");
    // 2011-12-29 24:00 at -10 (daylight) became 2011-12-31 00:00 at +14: 2011-12-30T10:00Z.
    assert_eq!(z.offset_at_utc(utc(2011, 12, 30, 9, 59, 59)), -10.0 * H);
    assert_eq!(z.offset_at_utc(utc(2011, 12, 30, 10, 0, 0)), 14.0 * H);
    check(&[
        ("new Date(Date.UTC(2011, 11, 30, 10)).toLocaleString('en-US', {timeZone: 'Pacific/Apia'})", "12/31/2011, 12:00:00\u{202f}AM"),
        ("Temporal.ZonedDateTime.from('2011-12-29T23:59:59[Pacific/Apia]').add({seconds: 1}).toString()", "2011-12-31T00:00:00+14:00[Pacific/Apia]"),
    ]);
}

#[test]
fn tz_names_links_and_primary_identifiers() {
    check(&[
        ("new Intl.DateTimeFormat('en-US', {timeZone: 'America/New_York', timeZoneName: 'short'}).format(Date.UTC(2007, 2, 15, 12))", "3/15/2007, EDT"),
        ("new Intl.DateTimeFormat('en-US', {timeZone: 'America/New_York', timeZoneName: 'short'}).format(Date.UTC(2006, 2, 15, 12))", "3/15/2006, EST"),
        (
            "new Intl.DateTimeFormat('en-US', {timeZone: 'Europe/Moscow', timeZoneName: 'long', hour: 'numeric'}).format(Date.UTC(2012, 6, 1))",
            "4\u{202f}AM Moscow Standard Time",
        ),
        // A Link resolves to its primary identifier; a Link listed under its own country is primary itself.
        ("new Intl.DateTimeFormat('en', {timeZone: 'Asia/Calcutta'}).resolvedOptions().timeZone", "Asia/Kolkata"),
        ("new Intl.DateTimeFormat('en', {timeZone: 'Europe/Oslo'}).resolvedOptions().timeZone", "Europe/Oslo"),
        ("Temporal.ZonedDateTime.from('2020-01-01T00:00[Asia/Calcutta]').timeZoneId", "Asia/Calcutta"),
        ("Temporal.ZonedDateTime.from('2020-01-01T00:00[Asia/Calcutta]').offset", "+05:30"),
        ("Intl.supportedValuesOf('timeZone').includes('Pacific/Apia')", "true"),
    ]);
}

struct MoscowHost;
impl Host for MoscowHost {
    fn time_zone_id(&mut self) -> Option<String> {
        Some(String::from("Europe/Moscow"))
    }
}

#[test]
fn tz_date_reads_the_host_zone() {
    let cases = [
        ("new Date(Date.UTC(2012, 6, 1)).getTimezoneOffset()", "-240"),
        ("new Date(Date.UTC(2015, 0, 1)).getTimezoneOffset()", "-180"),
        ("new Date(Date.UTC(2012, 6, 1)).toString()", "Sun Jul 01 2012 04:00:00 GMT+0400 (Moscow Standard Time)"),
        // 2014-10-26 01:30 happened twice (+04, then +03): the earlier instant.
        ("new Date(2014, 9, 26, 1, 30).getTime()", "1414272600000"),
    ];
    let mut fails = Vec::new();
    for (src, want) in cases {
        let mut vm = Vm::new(Box::new(MoscowHost));
        let r = vm.run_script_str(src);
        let got = show(&mut vm, r);
        if got != want {
            fails.push(format!("{src:?}: got {got:?}, want {want:?}"));
        }
    }
    for f in &fails {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} failures", fails.len());
}

fn show(vm: &mut Vm, v: Result<Value, Value>) -> String {
    match v {
        Ok(Value::String(s)) => s.to_rust(),
        Ok(other) => vm.error_string(&other),
        Err(e) => format!("THROW {}", vm.error_string(&e)),
    }
}

fn check(cases: &[(&str, &str)]) {
    let mut fails = Vec::new();
    for (src, want) in cases {
        let mut vm = Vm::new(Box::new(NullHost));
        let r = vm.run_script_str(src);
        let got = show(&mut vm, r);
        if got != *want {
            fails.push(format!("{src:?}: got {got:?}, want {want:?}"));
        }
    }
    for f in &fails {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} failures", fails.len());
}
