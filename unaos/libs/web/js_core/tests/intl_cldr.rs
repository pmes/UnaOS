//! INTLCLDR (LEDGER SR137): known answers of the CLDR-driven Intl services, one test per family across six or
//! more locales. Every expected value is read off the CLDR 46 data by hand (the JSON of unicode-org/cldr-json
//! 46.0.0 and the collation rules of unicode-org/cldr release-46), not taken from another engine.

use js_core::vm::{NullHost, Value, Vm};

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

/// ResolveLocale over the CLDR locales, likely subtags, locale info.
#[test]
fn intl_cldr_locale_negotiation() {
    check(&[
        // de-AT has no data of its own: lookup falls back to de.
        ("new Intl.NumberFormat('de-AT').resolvedOptions().locale", "de"),
        ("new Intl.NumberFormat('zh-TW').resolvedOptions().locale", "zh-TW"),
        // likelySubtags zh-MO -> zh-Hant-MO: the best-fit step prefers the Hant data over lookup's zh (Hans).
        ("new Intl.NumberFormat('zh-MO').resolvedOptions().locale", "zh-Hant"),
        ("new Intl.NumberFormat('pt-BR').resolvedOptions().locale", "pt-BR"),
        ("Intl.NumberFormat.supportedLocalesOf(['fr', 'xx', 'ja-JP']).join()", "fr,ja-JP"),
        ("new Intl.Locale('zh-TW').maximize().toString()", "zh-Hant-TW"),
        ("new Intl.Locale('zh-Hant-TW').minimize().toString()", "zh-TW"),
        ("new Intl.Locale('ar').getTextInfo().direction", "rtl"),
        ("new Intl.Locale('he').getTextInfo().direction", "rtl"),
        // timeData JP: preferred "H".
        ("new Intl.Locale('ja').getHourCycles().join()", "h23"),
        // Calendars outside buddhist / gregory / japanese / roc are refused and resolve to the default.
        ("new Intl.DateTimeFormat('en-u-ca-islamic').resolvedOptions().calendar", "gregory"),
        ("Intl.supportedValuesOf('calendar').join()", "buddhist,gregory,japanese,roc"),
        ("Intl.supportedValuesOf('collation').join()", "phonebk,pinyin,trad"),
    ]);
}

/// NumberFormat: symbols, grouping (minimum grouping digits, Indian grouping), numbering systems, compact,
/// currency, percent.
#[test]
fn intl_cldr_numbers() {
    check(&[
        ("new Intl.NumberFormat('de').format(1234567.891)", "1.234.567,891"),
        ("new Intl.NumberFormat('de').format(-1234.5)", "-1.234,5"),
        ("new Intl.NumberFormat('fr').format(1234567.891)", "1\u{202f}234\u{202f}567,891"),
        // es minimumGroupingDigits 2.
        ("new Intl.NumberFormat('es').format(1234) + ' ' + new Intl.NumberFormat('es').format(12345)", "1234 12.345"),
        // hi "#,##,##0.###".
        ("new Intl.NumberFormat('hi').format(1234567)", "12,34,567"),
        ("new Intl.NumberFormat('ar-u-nu-arab').format(1234.5)", "١٬٢٣٤٫٥"),
        ("new Intl.NumberFormat('ar-u-nu-arab').resolvedOptions().numberingSystem", "arab"),
        ("new Intl.NumberFormat('ar-u-nu-arab').resolvedOptions().locale", "ar-u-nu-arab"),
        // bn defaults to the beng digits.
        ("new Intl.NumberFormat('bn').format(1234567)", "১২,৩৪,৫৬৭"),
        ("new Intl.NumberFormat('bn').resolvedOptions().numberingSystem", "beng"),
        ("new Intl.NumberFormat('ja', {notation: 'compact'}).format(12345)", "1.2万"),
        ("new Intl.NumberFormat('de', {notation: 'compact'}).format(1234567)", "1,2\u{a0}Mio."),
        // de's 10^3 compact pattern is "0": no compaction, and min2 grouping leaves 4 digits alone.
        ("new Intl.NumberFormat('de', {notation: 'compact'}).format(1234)", "1234"),
        ("new Intl.NumberFormat('de', {style: 'currency', currency: 'EUR'}).format(1234.5)", "1.234,50\u{a0}€"),
        ("new Intl.NumberFormat('ja', {style: 'currency', currency: 'JPY'}).format(1234)", "￥1,234"),
        ("new Intl.NumberFormat('ru', {style: 'percent'}).format(0.25)", "25\u{a0}%"),
    ]);
}

const D: &str = "var d = Date.UTC(2024, 0, 15, 15, 4, 5); ";

fn c(s: &str) -> String {
    format!("{D}{s}")
}

/// DateTimeFormat: availableFormats, date / time styles and the at-time glue, hour cycles, calendars,
/// interval formats.
#[test]
fn intl_cldr_dates() {
    let cases: Vec<(String, &str)> = vec![
        (c("new Date(d).toLocaleDateString('de', {timeZone: 'UTC'})"), "15.1.2024"),
        (c("new Date(d).toLocaleString('de', {timeZone: 'UTC'})"), "15.1.2024, 15:04:05"),
        (c("new Intl.DateTimeFormat('de', {dateStyle: 'long', timeZone: 'UTC'}).format(d)"), "15. Januar 2024"),
        (c("new Intl.DateTimeFormat('fr', {dateStyle: 'full', timeZone: 'UTC'}).format(d)"), "lundi 15 janvier 2024"),
        (c("new Intl.DateTimeFormat('fr', {dateStyle: 'long', timeStyle: 'short', timeZone: 'UTC'}).format(d)"), "15 janvier 2024 à 15:04"),
        (c("new Date(d).toLocaleString('ja', {timeZone: 'UTC'})"), "2024/1/15 15:04:05"),
        // ja's 12-hour cycle is h11 ("aK:mm").
        (c("new Intl.DateTimeFormat('ja', {hour: 'numeric', minute: '2-digit', hour12: true, timeZone: 'UTC'}).format(d)"), "午後3:04"),
        (c("new Intl.DateTimeFormat('ja', {hour: 'numeric', hour12: true, timeZone: 'UTC'}).resolvedOptions().hourCycle"), "h11"),
        (c("new Intl.DateTimeFormat('ja-u-ca-japanese', {dateStyle: 'long', timeZone: 'UTC'}).format(d)"), "令和6年1月15日"),
        (c("new Date(d).toLocaleDateString('ar', {timeZone: 'UTC'})"), "15\u{200f}/1\u{200f}/2024"),
        (c("new Intl.DateTimeFormat('ru', {dateStyle: 'long', timeZone: 'UTC'}).format(d)"), "15 января 2024\u{202f}г."),
        (c("new Intl.DateTimeFormat('hi', {dateStyle: 'full', timeZone: 'UTC'}).format(d)"), "सोमवार, 15 जनवरी 2024"),
        // th defaults to the buddhist calendar: 2024 + 543.
        (c("new Intl.DateTimeFormat('th', {dateStyle: 'long', timeZone: 'UTC'}).format(d)"), "15 มกราคม 2567"),
        (c("new Intl.DateTimeFormat('th').resolvedOptions().calendar"), "buddhist"),
        // intervalFormats MMMd / d.
        (c("new Intl.DateTimeFormat('en', {month: 'short', day: 'numeric', timeZone: 'UTC'}).formatRange(d, d + 864e5)"), "Jan 15\u{2009}–\u{2009}16"),
        (c("new Intl.DateTimeFormat('de', {month: 'short', day: 'numeric', timeZone: 'UTC'}).formatRange(d, d + 864e5)"), "15.–16. Jan."),
        (c("new Intl.DateTimeFormat('ja', {month: 'short', day: 'numeric', timeZone: 'UTC'}).formatRange(d, d + 864e5)"), "1月15日～16日"),
    ];
    let refs: Vec<(&str, &str)> = cases.iter().map(|(a, b)| (a.as_str(), *b)).collect();
    check(&refs);
}

/// RelativeTimeFormat: the relative phrases and the plural-category patterns of the dateFields.
#[test]
fn intl_cldr_relative_time() {
    check(&[
        ("new Intl.RelativeTimeFormat('de', {numeric: 'auto'}).format(-1, 'day')", "gestern"),
        ("new Intl.RelativeTimeFormat('de', {numeric: 'auto'}).format(-2, 'day')", "vorgestern"),
        ("new Intl.RelativeTimeFormat('de').format(3, 'day')", "in 3 Tagen"),
        ("new Intl.RelativeTimeFormat('fr').format(-3, 'hour')", "il y a 3 heures"),
        ("new Intl.RelativeTimeFormat('ru').format(5, 'day')", "через 5 дней"),
        ("new Intl.RelativeTimeFormat('ru').format(2, 'day')", "через 2 дня"),
        ("new Intl.RelativeTimeFormat('ru').format(-21, 'hour')", "21 час назад"),
        ("new Intl.RelativeTimeFormat('ja').format(-3, 'hour')", "3 時間前"),
        // ar "two": the pattern carries no number.
        ("new Intl.RelativeTimeFormat('ar').format(2, 'day')", "خلال يومين"),
        ("new Intl.RelativeTimeFormat('he', {numeric: 'auto'}).format(-1, 'day')", "אתמול"),
    ]);
}

/// PluralRules: the compiled CLDR rules, categories in CLDR order, plural ranges.
#[test]
fn intl_cldr_plural_rules() {
    check(&[
        ("[1, 2, 5, 11, 21, 1.5].map(n => new Intl.PluralRules('ru').select(n)).join()", "one,few,many,many,one,other"),
        ("[0, 1, 2, 3, 11, 100].map(n => new Intl.PluralRules('ar').select(n)).join()", "zero,one,two,few,many,other"),
        ("[0, 1, 1.5, 2, 1000000].map(n => new Intl.PluralRules('fr').select(n)).join()", "one,one,one,other,many"),
        ("new Intl.PluralRules('ja').select(1)", "other"),
        ("[1, 2, 5, 22, 25].map(n => new Intl.PluralRules('pl').select(n)).join()", "one,few,many,few,many"),
        ("new Intl.PluralRules('ru').resolvedOptions().pluralCategories.join()", "one,few,many,other"),
        ("[1, 2, 3, 4, 11, 22].map(n => new Intl.PluralRules('en', {type: 'ordinal'}).select(n)).join()", "one,two,few,other,other,two"),
        ("new Intl.PluralRules('fr').selectRange(0, 1)", "one"),
    ]);
}

/// ListFormat: the locale's templates and Spanish's contextual conjunctions.
#[test]
fn intl_cldr_lists() {
    check(&[
        ("new Intl.ListFormat('de').format(['a', 'b', 'c'])", "a, b und c"),
        ("new Intl.ListFormat('fr', {type: 'disjunction'}).format(['a', 'b', 'c'])", "a, b ou c"),
        ("new Intl.ListFormat('ja').format(['a', 'b', 'c'])", "a、b、c"),
        ("new Intl.ListFormat('ru').format(['a', 'b'])", "a и b"),
        ("new Intl.ListFormat('he').format(['א', 'ב'])", "א וב"),
        ("new Intl.ListFormat('es').format(['padre', 'hijo'])", "padre e hijo"),
        ("new Intl.ListFormat('es', {type: 'disjunction'}).format(['siete', 'ocho'])", "siete u ocho"),
    ]);
}

/// DisplayNames: regions, languages (dialect keys and the locale pattern), scripts, currencies, fields.
#[test]
fn intl_cldr_display_names() {
    check(&[
        ("new Intl.DisplayNames('de', {type: 'region'}).of('DE')", "Deutschland"),
        ("new Intl.DisplayNames('de', {type: 'language'}).of('fr-BE')", "Französisch (Belgien)"),
        ("new Intl.DisplayNames('fr', {type: 'language'}).of('en-GB')", "anglais britannique"),
        ("new Intl.DisplayNames('ja', {type: 'script'}).of('Latn')", "ラテン文字"),
        ("new Intl.DisplayNames('ru', {type: 'currency'}).of('EUR')", "евро"),
        ("new Intl.DisplayNames('ar', {type: 'region'}).of('EG')", "مصر"),
        ("new Intl.DisplayNames('de', {type: 'dateTimeField'}).of('day')", "Tag"),
    ]);
}

/// Collator: the CLDR root order across scripts and the tailorings.
#[test]
fn intl_cldr_collation() {
    check(&[
        ("['я', 'Ω', 'z', 'a'].sort(new Intl.Collator('en').compare).join()", "a,z,Ω,я"),
        ("['z', 'ä', 'b', 'a'].sort(new Intl.Collator('de').compare).join()", "a,ä,b,z"),
        ("['ö', 'z', 'å', 'a', 'ä'].sort(new Intl.Collator('sv').compare).join()", "a,z,å,ä,ö"),
        ("new Intl.Collator('de-u-co-phonebk', {sensitivity: 'base'}).compare('ä', 'ae')", "0"),
        ("new Intl.Collator('de-u-co-phonebk').resolvedOptions().collation", "phonebk"),
        ("['d', 'ch', 'cz', 'c'].sort(new Intl.Collator('es-u-co-trad').compare).join()", "c,cz,ch,d"),
        ("['o', 'ñ', 'n', 'nz'].sort(new Intl.Collator('es').compare).join()", "n,nz,ñ,o"),
        ("['j', 'ı', 'i', 'h'].sort(new Intl.Collator('tr').compare).join()", "h,ı,i,j"),
        ("['中', '爱', '阿', '把'].sort(new Intl.Collator('zh').compare).join()", "阿,爱,把,中"),
        ("['ア', 'あ'].sort(new Intl.Collator('ja').compare).join()", "あ,ア"),
        ("new Intl.Collator('da').resolvedOptions().caseFirst", "upper"),
        ("['a', 'A'].sort(new Intl.Collator('da').compare).join()", "A,a"),
    ]);
}
