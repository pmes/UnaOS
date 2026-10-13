//! AETHERJS2 (LEDGER SR106): known-answer checks of the 2026 surface — ECMA-402 Intl for en-US / en-GB, the
//! ES2026 built-ins, modules with top-level await / dynamic import() / import.meta.resolve, and the ES2022–
//! ES2025 language features pages rely on. Each case asserts the value Chromium prints for the same program.

use js_core::vm::{Host, NullHost, Value, Vm};

fn show(vm: &mut Vm, v: Result<Value, Value>) -> String {
    match v {
        Ok(Value::String(s)) => s.to_rust(),
        Ok(other) => vm.error_string(&other),
        Err(e) => format!("THROW {}", vm.error_string(&e)),
    }
}

/// Run `setup`, drain the job queue, then evaluate `expr`.
fn after_jobs(vm: &mut Vm, setup: &str, expr: &str) -> String {
    if let Err(e) = vm.run_script_str(setup) {
        return format!("THROW {}", vm.error_string(&e));
    }
    let _ = vm.run_jobs();
    let r = vm.run_script_str(expr);
    show(vm, r)
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

const D: &str = "var d = new Date(Date.UTC(2024, 0, 15, 15, 4, 5, 123)); ";

/// ECMA-402 §15: Intl.NumberFormat, Number/BigInt toLocaleString.
#[test]
fn intl_number_format() {
    check(&[
        ("new Intl.NumberFormat('en-US').format(1234567.891)", "1,234,567.891"),
        ("(1234.5).toLocaleString('en-US', {style: 'currency', currency: 'USD'})", "$1,234.50"),
        ("(1234.5678).toLocaleString()", "1,234.568"),
        ("new Intl.NumberFormat('en-US', {style: 'percent'}).format(0.256)", "26%"),
        ("new Intl.NumberFormat('en-US', {notation: 'compact'}).format(1234)", "1.2K"),
        ("new Intl.NumberFormat('en-US', {notation: 'compact'}).format(1234567)", "1.2M"),
        ("new Intl.NumberFormat('en-US', {notation: 'compact', compactDisplay: 'long'}).format(1500000)", "1.5 million"),
        ("new Intl.NumberFormat('en-US', {maximumSignificantDigits: 3}).format(123456)", "123,000"),
        ("new Intl.NumberFormat('en-US', {minimumFractionDigits: 2}).format(1)", "1.00"),
        ("new Intl.NumberFormat('en-US', {maximumFractionDigits: 2}).format(1.005)", "1.01"),
        ("new Intl.NumberFormat('en-US', {maximumFractionDigits: 0, roundingMode: 'halfEven'}).format(2.5)", "2"),
        ("new Intl.NumberFormat('en-US', {style: 'unit', unit: 'kilometer-per-hour'}).format(50)", "50 km/h"),
        ("new Intl.NumberFormat('en-US', {style: 'unit', unit: 'liter', unitDisplay: 'long'}).format(2)", "2 liters"),
        ("new Intl.NumberFormat('en-US', {style: 'currency', currency: 'EUR', currencySign: 'accounting'}).format(-5)", "(€5.00)"),
        ("new Intl.NumberFormat('en-US', {style: 'currency', currency: 'JPY'}).format(1234)", "¥1,234"),
        ("new Intl.NumberFormat('en-US', {style: 'currency', currency: 'USD', currencyDisplay: 'name'}).format(1)", "1.00 US dollars"),
        ("new Intl.NumberFormat('en', {notation: 'scientific'}).format(123456)", "1.235E5"),
        ("new Intl.NumberFormat('en-US').format(12345678901234567890n)", "12,345,678,901,234,567,890"),
        ("new Intl.NumberFormat('en-US').format('1234567890123456789.987')", "1,234,567,890,123,456,789.987"),
        ("[0, 5, -0].map(x => new Intl.NumberFormat('en', {signDisplay: 'exceptZero'}).format(x)).join(' ')", "0 +5 0"),
        ("new Intl.NumberFormat().formatToParts(-1234.5).map(p => p.type).join()", "minusSign,integer,group,integer,decimal,fraction"),
        ("new Intl.NumberFormat('en').formatRange(3, 5)", "3–5"),
        ("new Intl.NumberFormat('en-US', {style: 'currency', currency: 'USD'}).resolvedOptions().maximumFractionDigits", "2"),
        ("new Intl.NumberFormat('de-DE').resolvedOptions().locale", "en-US"),
        ("try { new Intl.NumberFormat('en', {style: 'currency'}) } catch (e) { e.name }", "TypeError"),
    ]);
}

/// ECMA-402 §11: Intl.DateTimeFormat and Date.prototype.toLocale*String.
#[test]
fn intl_date_time_format() {
    let c = |s: &str| format!("{D}{s}");
    let cases: Vec<(String, &str)> = vec![
        (c("d.toLocaleString('en-US', {timeZone: 'UTC'})"), "1/15/2024, 3:04:05\u{202F}PM"),
        (c("d.toLocaleDateString('en-US', {timeZone: 'UTC', weekday: 'long', year: 'numeric', month: 'long', day: 'numeric'})"), "Monday, January 15, 2024"),
        (c("d.toLocaleDateString('en-GB', {timeZone: 'UTC'})"), "15/01/2024"),
        (c("d.toLocaleTimeString('en-GB', {timeZone: 'UTC'})"), "15:04:05"),
        (c("new Intl.DateTimeFormat('en-US', {dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC'}).format(d)"), "Jan 15, 2024, 3:04\u{202F}PM"),
        (c("new Intl.DateTimeFormat('en-US', {dateStyle: 'long', timeZone: 'America/New_York'}).format(d)"), "January 15, 2024"),
        (c("new Intl.DateTimeFormat('en-US', {hour: 'numeric', minute: '2-digit', timeZoneName: 'short', timeZone: 'America/Los_Angeles'}).format(d)"), "7:04\u{202F}AM PST"),
        ("new Intl.DateTimeFormat('en-US', {hour: 'numeric', minute: '2-digit', timeZoneName: 'short', timeZone: 'America/New_York'}).format(Date.UTC(2024, 6, 4, 16, 0))".into(), "12:00\u{202F}PM EDT"),
        (c("new Intl.DateTimeFormat('en-US', {timeZone: 'Asia/Kolkata', hour: 'numeric', minute: 'numeric', hour12: false}).format(d)"), "20:34"),
        (c("new Intl.DateTimeFormat('en-US', {minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3, timeZone: 'UTC'}).format(d)"), "04:05.123"),
        (c("new Intl.DateTimeFormat('en-US', {timeZone: 'UTC'}).formatToParts(d).map(p => p.type).join()"), "month,literal,day,literal,year"),
        (c("new Intl.DateTimeFormat('en-US', {timeZone: 'UTC'}).formatRange(d, Date.UTC(2024, 0, 16))"), "1/15/2024\u{2009}–\u{2009}1/16/2024"),
        ("new Intl.DateTimeFormat().resolvedOptions().timeZone".into(), "UTC"),
        ("new Intl.DateTimeFormat('en-US', {timeZone: 'europe/paris'}).resolvedOptions().timeZone".into(), "Europe/Paris"),
        ("new Date(NaN).toLocaleString()".into(), "Invalid Date"),
        ("try { new Intl.DateTimeFormat('en', {timeZone: 'Mars/Olympus'}) } catch (e) { e.name }".into(), "RangeError"),
        ("try { new Intl.DateTimeFormat('en', {dateStyle: 'short', hour: 'numeric'}) } catch (e) { e.name }".into(), "TypeError"),
    ];
    let refs: Vec<(&str, &str)> = cases.iter().map(|(a, b)| (a.as_str(), *b)).collect();
    check(&refs);
}

/// ECMA-402 §10 / §19.1.1: Intl.Collator and String.prototype.localeCompare.
#[test]
fn intl_collator() {
    check(&[
        ("['b', 'A', 'a', 'B', '\\u00e1'].sort(new Intl.Collator('en').compare).join()", "a,A,á,b,B"),
        ("new Intl.Collator('en', {sensitivity: 'base'}).compare('a', '\\u00c1')", "0"),
        ("['10', '9', '2'].sort(new Intl.Collator('en', {numeric: true}).compare).join()", "2,9,10"),
        ("['a', 'A'].sort(new Intl.Collator('en', {caseFirst: 'upper'}).compare).join()", "A,a"),
        ("['b', '1', '-'].sort(new Intl.Collator('en').compare).join()", "-,1,b"),
        ("'a'.localeCompare('b')", "-1"),
        ("'\\u00e9'.localeCompare('e\\u0301')", "0"),
        ("new Intl.Collator().resolvedOptions().sensitivity", "variant"),
    ]);
}

/// ECMA-402 §16: Intl.PluralRules.
#[test]
fn intl_plural_rules() {
    check(&[
        ("[0, 1, 2].map(n => new Intl.PluralRules('en-US').select(n)).join()", "other,one,other"),
        (
            "[1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111].map(n => new Intl.PluralRules('en', {type: 'ordinal'}).select(n)).join()",
            "one,two,few,other,other,other,other,one,two,few,one,other",
        ),
        ("new Intl.PluralRules('en', {minimumFractionDigits: 1}).select(1)", "other"),
        ("new Intl.PluralRules('en').resolvedOptions().pluralCategories.join()", "one,other"),
    ]);
}

/// ECMA-402 §13 / §17 / §18 / §14: ListFormat, RelativeTimeFormat, Segmenter, Locale and the namespace.
#[test]
fn intl_list_relative_segmenter_locale() {
    check(&[
        ("new Intl.ListFormat('en', {type: 'conjunction'}).format(['a', 'b', 'c'])", "a, b, and c"),
        ("new Intl.ListFormat('en').format(['a', 'b'])", "a and b"),
        ("new Intl.ListFormat('en', {type: 'disjunction'}).format(['a', 'b', 'c'])", "a, b, or c"),
        ("new Intl.ListFormat('en-GB').format(['a', 'b', 'c'])", "a, b and c"),
        ("new Intl.RelativeTimeFormat('en').format(-1, 'day')", "1 day ago"),
        ("new Intl.RelativeTimeFormat('en').format(3, 'hours')", "in 3 hours"),
        ("new Intl.RelativeTimeFormat('en', {numeric: 'auto'}).format(-1, 'day')", "yesterday"),
        ("new Intl.RelativeTimeFormat('en', {numeric: 'auto'}).format(0, 'second')", "now"),
        ("new Intl.RelativeTimeFormat('en', {style: 'short'}).format(3, 'month')", "in 3 mo."),
        ("new Intl.RelativeTimeFormat('en').formatToParts(100, 'day').map(p => p.type).join()", "literal,integer,literal"),
        ("[...new Intl.Segmenter('en', {granularity: 'grapheme'}).segment('e\\u0301\\u{1F44D}\\u{1F3FD}\\u{1F1FA}\\u{1F1F8}')].length", "3"),
        (
            "[...new Intl.Segmenter('en', {granularity: 'word'}).segment(\"Hello, world! It's 3.14\")].filter(s => s.isWordLike).map(s => s.segment).join('|')",
            "Hello|world|It's|3.14",
        ),
        (
            "[...new Intl.Segmenter('en', {granularity: 'sentence'}).segment('Hi there. How are you? Fine.')].map(s => s.segment).join('|')",
            "Hi there. |How are you? |Fine.",
        ),
        ("new Intl.Segmenter('en', {granularity: 'word'}).segment('foo bar').containing(5).segment", "bar"),
        ("new Intl.Locale('en-Latn-US-u-hc-h23').hourCycle", "h23"),
        ("new Intl.Locale('en').maximize().toString()", "en-Latn-US"),
        ("Intl.getCanonicalLocales('EN-us').join()", "en-US"),
        ("Intl.NumberFormat.supportedLocalesOf(['fr', 'en-GB']).join()", "en-GB"),
        ("Object.prototype.toString.call(Intl)", "[object Intl]"),
    ]);
}

/// ES2026: Array.fromAsync, Error.isError, Math.sumPrecise, Uint8Array base64/hex, Iterator.concat, upsert,
/// Error.captureStackTrace.
#[test]
fn es2026_builtins() {
    check(&[
        ("[Error.isError(new TypeError()), Error.isError({})].join()", "true,false"),
        ("Math.sumPrecise([1e20, 0.1, -1e20])", "0.1"),
        ("Object.is(Math.sumPrecise([]), -0)", "true"),
        ("new Uint8Array([72, 101, 108, 108, 111]).toBase64()", "SGVsbG8="),
        ("Uint8Array.fromBase64('SGVsbG8=').join()", "72,101,108,108,111"),
        ("new Uint8Array([72, 101, 108, 108, 111]).toHex()", "48656c6c6f"),
        ("Uint8Array.fromHex('cafe').join()", "202,254"),
        ("new Uint8Array([251, 255]).toBase64({alphabet: 'base64url', omitPadding: true})", "-_8"),
        ("var a = new Uint8Array(2); var r = a.setFromHex('abcdef'); [r.read, r.written, a.join()].join()", "4,2,171,205"),
        ("try { Uint8Array.fromBase64('S') } catch (e) { e.name }", "SyntaxError"),
        ("[...Iterator.concat([1, 2], new Set([3]))].join()", "1,2,3"),
        ("var m = new Map(); m.getOrInsert('a', 1); [m.getOrInsert('a', 2), m.getOrInsertComputed('b', k => k + '!')].join()", "1,b!"),
        ("var o = {}; Error.captureStackTrace(o); typeof o.stack", "string"),
    ]);
    let mut vm = Vm::new(Box::new(NullHost));
    let got = after_jobs(
        &mut vm,
        "var out = []; Array.fromAsync([1, Promise.resolve(2), 3], x => x * 2).then(a => out.push(a.join()));\
         async function* g() { yield 1; yield 2; } Array.fromAsync(g()).then(a => out.push(a.join()));\
         Array.fromAsync({length: 2, 0: 'a', 1: Promise.resolve('b')}).then(a => out.push(a.join()));\
         Array.fromAsync(5, 'nope').catch(e => out.push(e.name));",
        "out.sort().join('|')",
    );
    assert_eq!(got, "1,2|2,4,6|TypeError|a,b");
}

/// ES2022–ES2025 language and library surface the 2026 web leans on (already in the engine; held here).
#[test]
fn es2025_language_surface() {
    check(&[
        (
            "class A { static #c = 0; #x = 1; static { A.init = true; } #m() { return this.#x + 1; } get v() { return this.#m(); } static has(o) { return #x in o; } } [new A().v, A.init, A.has(new A()), A.has({})].join()",
            "2,true,true,false",
        ),
        ("typeof new WeakRef({}).deref() + ',' + typeof FinalizationRegistry", "object,function"),
        ("[[1, 2, 3].at(-1), [1, 2, 3, 4].findLast(x => x % 2), [3, 1, 2].toSorted().join(''), [1, 2].toReversed().join(''), [1, 2].with(0, 9).join('')].join()", "3,3,123,21,92"),
        ("Object.groupBy([1, 2, 3], x => x % 2 ? 'odd' : 'even').odd.join()", "1,3"),
        ("/[\\p{L}--[a-z]]/v.test('A') + ',' + /[\\p{L}--[a-z]]/v.test('a')", "true,false"),
        ("'2024-05'.match(/(?<y>\\d+)-(?<m>\\d+)/).groups.m", "05"),
        ("/(?<y>\\d{4})-\\d{2}|\\d{2}-(?<y>\\d{4})/.exec('05-2024').groups.y", "2024"),
        ("new Set([1, 2]).union(new Set([3])).size", "3"),
        ("typeof Promise.withResolvers().resolve", "function"),
    ]);
    let mut vm = Vm::new(Box::new(NullHost));
    let got = after_jobs(
        &mut vm,
        "var r = []; (async () => { for await (const x of [Promise.resolve(1), 2]) r.push(x); })();\
         Promise.try(() => 5).then(v => r.push(v));",
        "r.join()",
    );
    // The for-await body resumes one job after the async-from-sync unwrap: Promise.try's reaction runs first.
    assert_eq!(got, "5,1,2");
}

/// A module host: sources by URL, relative resolution, and import.meta.resolve.
struct ModHost;
impl Host for ModHost {
    fn load_module(&mut self, referrer: Option<&str>, specifier: &str) -> Result<(String, String), String> {
        let url = self.resolve_module_specifier(referrer, specifier)?;
        let src = match url.as_str() {
            "https://x.test/dep.js" => "export const v = await Promise.resolve(40); export default function add(n) { return v + n; }",
            "https://x.test/lazy.js" => "export const lazy = 'loaded';",
            _ => return Err(format!("404 {url}")),
        };
        Ok((url, src.to_string()))
    }
    fn has_import_meta_resolve(&self) -> bool {
        true
    }
    fn resolve_module_specifier(&mut self, _referrer: Option<&str>, specifier: &str) -> Result<String, String> {
        match specifier.strip_prefix("./") {
            Some(rest) => Ok(format!("https://x.test/{rest}")),
            None => Err(format!("Failed to resolve module specifier \"{specifier}\"")),
        }
    }
}

/// ES modules end to end in the engine: a static import of a module with top-level await, dynamic import(),
/// import.meta.url / resolve.
#[test]
fn modules_tla_dynamic_import_meta() {
    let mut vm = Vm::new(Box::new(ModHost));
    let src = "import add, { v } from './dep.js';\
               globalThis.sum = add(2);\
               globalThis.meta = import.meta.url + ' ' + import.meta.resolve('./lazy.js');\
               const m = await import('./lazy.js');\
               globalThis.lazy = m.lazy;";
    if let Err(e) = vm.run_module("https://x.test/main.js", src) {
        panic!("module threw: {}", vm.error_string(&e));
    }
    let _ = vm.run_jobs();
    let r = vm.run_script_str("[sum, meta, lazy].join('|')");
    let got = show(&mut vm, r);
    assert_eq!(got, "42|https://x.test/main.js https://x.test/lazy.js|loaded");
}
