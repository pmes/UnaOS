//! TEMPORAL (LEDGER SR117): known-answer checks of Temporal in js_core, one test per type, the cases taken
//! from the proposal's documentation examples (tc39/proposal-temporal docs/*.md) and the spec's worked
//! algorithms; the zone cases read the historical table (tzdata 2025b). Written for the fold (R107).

use js_core::vm::{NullHost, Value, Vm};

const PRELUDE: &str = "function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } ";

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
        let full = format!("{PRELUDE}{src}");
        let r = vm.run_script_str(&full);
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

#[test]
fn temporal_instant() {
    check(&[
        ("Temporal.Instant.from('2019-03-30T01:45:00+01:00').toString()", "2019-03-30T00:45:00Z"),
        ("Temporal.Instant.fromEpochMilliseconds(1553906700000).toString()", "2019-03-30T00:45:00Z"),
        ("String(Temporal.Instant.from('2019-03-30T00:45Z').epochNanoseconds)", "1553906700000000000"),
        ("Temporal.Instant.from('2019-03-30T00:45Z').toString({ timeZone: 'Europe/Berlin' })", "2019-03-30T01:45:00+01:00"),
        ("Temporal.Instant.from('2019-03-30T02:45:59.999999999Z').round({ smallestUnit: 'second' }).toString()", "2019-03-30T02:46:00Z"),
        ("Temporal.Instant.from('1969-07-20T20:17Z').until('2019-07-20T20:17Z', { largestUnit: 'hour' }).toString()", "PT438288H"),
        ("Temporal.Instant.from('2020-01-01T00:00Z').add({ hours: 1, minutes: 30 }).toString()", "2020-01-01T01:30:00Z"),
        ("err(() => Temporal.Instant.from('2020-01-01T00:00Z').add({ days: 1 }))", "RangeError"),
        ("err(() => Temporal.Instant.from('2020-01-01T00:00'))", "RangeError"),
        ("String(Temporal.Instant.compare('2020-01-01T00:00Z', '2020-01-01T00:00:00.000000001Z'))", "-1"),
        ("new Temporal.Instant(0n).toString({ smallestUnit: 'millisecond' })", "1970-01-01T00:00:00.000Z"),
        ("Object.prototype.toString.call(new Temporal.Instant(0n))", "[object Temporal.Instant]"),
        ("err(() => new Temporal.Instant(0n).valueOf())", "TypeError"),
    ]);
}

#[test]
fn temporal_zoned_date_time() {
    check(&[
        (
            "Temporal.ZonedDateTime.from('1995-12-07T03:24:30.000003500-08:00[America/Los_Angeles]').toString()",
            "1995-12-07T03:24:30.0000035-08:00[America/Los_Angeles]",
        ),
        (
            "Temporal.ZonedDateTime.from({ timeZone: 'America/Los_Angeles', year: 1995, month: 12, day: 7, hour: 3, minute: 24, second: 30, millisecond: 0, microsecond: 3, nanosecond: 500 }).toString()",
            "1995-12-07T03:24:30.0000035-08:00[America/Los_Angeles]",
        ),
        ("String(Temporal.ZonedDateTime.from('2020-03-08T12:00[America/Los_Angeles]').hoursInDay)", "23"),
        ("Temporal.ZonedDateTime.from('2020-03-08T01:00[America/Los_Angeles]').add({ hours: 1 }).toString()", "2020-03-08T03:00:00-07:00[America/Los_Angeles]"),
        ("Temporal.ZonedDateTime.from('2020-03-07T02:30[America/Los_Angeles]').add({ days: 1 }).toString()", "2020-03-08T03:30:00-07:00[America/Los_Angeles]"),
        ("Temporal.ZonedDateTime.from('2020-03-08T00:00[America/Los_Angeles]').until('2020-03-09T00:00[America/Los_Angeles]', { largestUnit: 'hour' }).toString()", "PT23H"),
        ("Temporal.ZonedDateTime.from('2020-03-08T00:00[America/Los_Angeles]').until('2020-03-09T00:00[America/Los_Angeles]', { largestUnit: 'day' }).toString()", "P1D"),
        // Historical offsets: British Standard Time (+01:00 all year, 1968-1971) and New York local mean time.
        ("Temporal.ZonedDateTime.from('1970-01-15T12:00[Europe/London]').offset", "+01:00"),
        ("Temporal.ZonedDateTime.from('1800-01-01T00:00[America/New_York]').offset", "-04:56:02"),
        ("Temporal.ZonedDateTime.from('2020-01-01T00:00[America/New_York]').getTimeZoneTransition('next').toString()", "2020-03-08T03:00:00-04:00[America/New_York]"),
        ("Temporal.ZonedDateTime.from('2020-03-08T12:00[America/New_York]').startOfDay().toString()", "2020-03-08T00:00:00-05:00[America/New_York]"),
        ("err(() => Temporal.ZonedDateTime.from('2020-03-08T02:30-08:00[America/Los_Angeles]'))", "RangeError"),
        ("Temporal.ZonedDateTime.from('2020-11-01T01:30-08:00[America/Los_Angeles]').offset", "-08:00"),
        ("Temporal.ZonedDateTime.from('2020-01-01T12:00[America/New_York]').toLocaleString('en-US')", "1/1/2020, 12:00:00\u{202F}PM EST"),
        ("err(() => new Intl.DateTimeFormat('en-US').format(Temporal.ZonedDateTime.from('2020-01-01T12:00[UTC]')))", "TypeError"),
    ]);
}

#[test]
fn temporal_plain_date() {
    check(&[
        ("Temporal.PlainDate.from('2006-08-24').toString()", "2006-08-24"),
        ("String(new Temporal.PlainDate(2006, 8, 24).dayOfWeek)", "4"),
        ("String(Temporal.PlainDate.from('2006-08-24').weekOfYear)", "34"),
        ("Temporal.PlainDate.from({ year: 2006, month: 8, day: 24 }).add({ months: 1 }).toString()", "2006-09-24"),
        ("Temporal.PlainDate.from('2001-01-31').add({ months: 1 }).toString()", "2001-02-28"),
        ("err(() => Temporal.PlainDate.from('2001-01-31').add({ months: 1 }, { overflow: 'reject' }))", "RangeError"),
        ("Temporal.PlainDate.from('2006-08-24').until('2019-01-31').toString()", "P4543D"),
        ("Temporal.PlainDate.from('2006-08-24').until('2019-01-31', { largestUnit: 'year' }).toString()", "P12Y5M7D"),
        ("Temporal.PlainDate.from('2006-08-24').toString({ calendarName: 'always' })", "2006-08-24[u-ca=iso8601]"),
        ("Temporal.PlainDate.from('2006-08-24').with({ day: 1 }).toString()", "2006-08-01"),
        ("err(() => Temporal.PlainDate.from('2006-08-24').withCalendar('japanese'))", "RangeError"),
        ("Temporal.PlainDate.from('2006-08-24').monthCode + ' ' + Temporal.PlainDate.from('2006-08-24').calendarId", "M08 iso8601"),
        ("Temporal.PlainDate.from('2020-02-01').toPlainDateTime('12:30').toString()", "2020-02-01T12:30:00"),
    ]);
}

#[test]
fn temporal_plain_time() {
    check(&[
        ("Temporal.PlainTime.from('19:39:09.068346205').toString()", "19:39:09.068346205"),
        ("Temporal.PlainTime.from('19:39:09.068346205').round({ smallestUnit: 'hour' }).toString()", "20:00:00"),
        ("Temporal.PlainTime.from('19:39:09.068346205').round({ roundingIncrement: 30, smallestUnit: 'minute' }).toString()", "19:30:00"),
        ("Temporal.PlainTime.from('19:39:09.068346205').add({ hours: 5, minutes: 30 }).toString()", "01:09:09.068346205"),
        ("Temporal.PlainTime.from('20:13:20.971398099').until('22:39:09.068346205').toString()", "PT2H25M48.096948106S"),
        ("Temporal.PlainTime.from('19:39:09.068346205').toString({ smallestUnit: 'minute' })", "19:39"),
        ("Temporal.PlainTime.from('19:39:09.068346205').toString({ fractionalSecondDigits: 4 })", "19:39:09.0683"),
        ("Temporal.PlainTime.from('15:30').toLocaleString('en-US')", "3:30:00\u{202F}PM"),
    ]);
}

#[test]
fn temporal_plain_date_time() {
    check(&[
        ("Temporal.PlainDateTime.from('1995-12-07T03:24:30.000003500').toString()", "1995-12-07T03:24:30.0000035"),
        ("Temporal.PlainDateTime.from('1995-12-07T03:24:30.000003500').round({ smallestUnit: 'hour' }).toString()", "1995-12-07T03:00:00"),
        ("Temporal.PlainDateTime.from('1995-12-07T03:24:30.000003500').with({ year: 2015, second: 31 }).toString()", "2015-12-07T03:24:31.0000035"),
        ("Temporal.PlainDateTime.from('1995-12-07T03:24:30.000003500').add({ years: 20, months: 4, nanoseconds: 500 }).toString()", "2016-04-07T03:24:30.000004"),
        ("Temporal.PlainDateTime.from('1995-12-07T03:24:30.000003500').until('2019-01-31T15:30').toString()", "P8456DT12H5M29.9999965S"),
        ("Temporal.PlainDateTime.from('1995-12-07T03:24:30.000003500').until('2019-01-31T15:30', { largestUnit: 'year' }).toString()", "P23Y1M24DT12H5M29.9999965S"),
        ("Temporal.PlainDateTime.from('2020-03-08T02:30').toZonedDateTime('America/Los_Angeles').toString()", "2020-03-08T03:30:00-07:00[America/Los_Angeles]"),
        ("err(() => Temporal.PlainDateTime.from('2020-01-01T12:00Z'))", "RangeError"),
    ]);
}

#[test]
fn temporal_plain_year_month() {
    check(&[
        ("Temporal.PlainYearMonth.from('2019-06').toString()", "2019-06"),
        ("String(Temporal.PlainYearMonth.from('2019-06').daysInMonth)", "30"),
        ("Temporal.PlainYearMonth.from('2019-06').add({ months: 7 }).toString()", "2020-01"),
        ("Temporal.PlainYearMonth.from('2019-06').until('2020-03').toString()", "P9M"),
        ("Temporal.PlainYearMonth.from('2019-06').toPlainDate({ day: 24 }).toString()", "2019-06-24"),
        ("Temporal.PlainYearMonth.from({ year: 2019, month: 6 }).toString({ calendarName: 'always' })", "2019-06-01[u-ca=iso8601]"),
        ("Temporal.PlainYearMonth.from('2020-05[u-ca=iso8601]').toString()", "2020-05"),
    ]);
}

#[test]
fn temporal_plain_month_day() {
    check(&[
        ("Temporal.PlainMonthDay.from('08-24').toString()", "08-24"),
        ("Temporal.PlainMonthDay.from({ monthCode: 'M02', day: 29 }).toPlainDate({ year: 2021 }).toString()", "2021-02-28"),
        ("Temporal.PlainMonthDay.from('--12-25').monthCode", "M12"),
        ("err(() => Temporal.PlainMonthDay.from({ month: 2, day: 29 }))", "TypeError"),
        ("Temporal.PlainMonthDay.from({ month: 2, day: 29, year: 2020 }).toString()", "02-29"),
    ]);
}

#[test]
fn temporal_duration() {
    check(&[
        ("String(Temporal.Duration.from({ hours: 130, minutes: 20 }).total({ unit: 'second' }))", "469200"),
        ("Temporal.Duration.from('PT130H20M').round({ largestUnit: 'day' }).toString()", "P5DT10H20M"),
        ("Temporal.Duration.from({ hours: 1756 }).round({ relativeTo: '2020-01-01', largestUnit: 'year' }).toString()", "P2M13DT4H"),
        ("String(Temporal.Duration.from('P1M').total({ unit: 'day', relativeTo: '2020-02-01' }))", "29"),
        ("String(Temporal.Duration.compare({ hours: 25 }, { days: 1 }))", "1"),
        ("Temporal.Duration.from({ seconds: 1, milliseconds: 500 }).toString()", "PT1.5S"),
        ("String(Temporal.Duration.from('-P1Y2M').sign)", "-1"),
        ("err(() => new Temporal.Duration(1, -1))", "RangeError"),
        ("Temporal.Duration.from({ minutes: 80 }).round({ largestUnit: 'hour' }).toString()", "PT1H20M"),
        ("Temporal.Duration.from({ hours: 1, minutes: 30 }).round({ smallestUnit: 'hour', roundingMode: 'halfEven' }).toString()", "PT2H"),
        ("String(Temporal.Duration.from({ days: 1 }).total({ unit: 'hour', relativeTo: '2020-03-08T00:00[America/Los_Angeles]' }))", "23"),
        ("err(() => Temporal.Duration.from({ months: 1 }).total({ unit: 'day' }))", "RangeError"),
        ("Temporal.Duration.from('PT1.5H').toString()", "PT1H30M"),
        ("err(() => Temporal.Duration.from('P1Y').add({ months: 1 }))", "RangeError"),
    ]);
}

#[test]
fn temporal_now() {
    check(&[
        ("typeof Temporal.Now.instant().epochNanoseconds", "bigint"),
        ("Temporal.Now.timeZoneId()", "UTC"),
        ("Temporal.Now.plainDateISO('UTC').toString()", "1970-01-01"),
        ("Temporal.Now.zonedDateTimeISO('Asia/Tokyo').toString()", "1970-01-01T09:00:00+09:00[Asia/Tokyo]"),
        ("Object.prototype.toString.call(Temporal.Now)", "[object Temporal.Now]"),
    ]);
}

#[test]
fn temporal_parse_grammar() {
    check(&[
        ("Temporal.PlainDate.from('20200101').toString()", "2020-01-01"),
        ("Temporal.ZonedDateTime.from('2020-01-01T00:00+01:00[Europe/Paris][u-ca=iso8601]').toString()", "2020-01-01T00:00:00+01:00[Europe/Paris]"),
        ("err(() => Temporal.PlainDate.from('2020-01-01[!foo=bar]'))", "RangeError"),
        ("Temporal.PlainDate.from('2020-01-01[foo=bar]').toString()", "2020-01-01"),
        ("err(() => Temporal.PlainDate.from('2020-01-01[u-ca=gregory]'))", "RangeError"),
        ("Temporal.Instant.from('2020-01-01T00:00:00.123456789+05:30:15').toString()", "2019-12-31T18:29:45.123456789Z"),
        ("Temporal.PlainTime.from('T1230').toString()", "12:30:00"),
        ("err(() => Temporal.PlainTime.from('1230'))", "RangeError"),
        ("err(() => Temporal.Duration.from('P1Y1.5M'))", "RangeError"),
        ("String(Temporal.PlainDate.from('+002020-01-01').year)", "2020"),
        ("err(() => Temporal.PlainDate.from('-000000-01-01'))", "RangeError"),
        ("Temporal.PlainDateTime.from('2020-01-01 23:59:60').toString()", "2020-01-01T23:59:59"),
    ]);
}

#[test]
fn temporal_intl_date_time_format() {
    check(&[
        ("new Intl.DateTimeFormat('en-US').format(Temporal.PlainDate.from('2006-08-24'))", "8/24/2006"),
        ("new Intl.DateTimeFormat('en-US', { timeZone: 'UTC' }).format(Temporal.Instant.from('2020-01-01T00:00Z'))", "1/1/2020, 12:00:00\u{202F}AM"),
        ("new Intl.DateTimeFormat('en-GB', { dateStyle: 'long' }).format(Temporal.PlainDateTime.from('2020-07-04T10:00'))", "4 July 2020"),
        ("err(() => new Intl.DateTimeFormat('en-US', { timeStyle: 'short' }).format(Temporal.PlainDate.from('2020-07-04')))", "TypeError"),
        // Date's own formatter reads the historical table too: London was on +01:00 in January 1970.
        ("new Date(Date.UTC(1970, 0, 15, 12)).toLocaleString('en-GB', { timeZone: 'Europe/London', timeStyle: 'short' })", "13:00"),
    ]);
}
