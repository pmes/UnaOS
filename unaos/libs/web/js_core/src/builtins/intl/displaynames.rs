//! Intl.DisplayNames (ECMA-402 §12) with CLDR 46 `en` display names for the common languages (incl. the
//! dialect names), regions, scripts, currencies (from `data::CURRENCIES`), calendars and date-time fields.
//! A code the data lacks falls back to the canonicalized code (`fallback: "code"`) or undefined ("none").

use super::locale::LangTag;
use super::*;

#[derive(Clone, Debug)]
pub struct DnFmt {
    pub locale: String,
    pub style: &'static str,
    pub kind: &'static str,
    pub fallback: &'static str,
    pub language_display: &'static str,
}

static LANGUAGES: &[(&str, &str)] = &[
    ("af", "Afrikaans"), ("am", "Amharic"), ("ar", "Arabic"), ("az", "Azerbaijani"), ("be", "Belarusian"), ("bg", "Bulgarian"),
    ("bn", "Bangla"), ("bs", "Bosnian"), ("ca", "Catalan"), ("cs", "Czech"), ("cy", "Welsh"), ("da", "Danish"), ("de", "German"),
    ("el", "Greek"), ("en", "English"), ("eo", "Esperanto"), ("es", "Spanish"), ("et", "Estonian"), ("eu", "Basque"), ("fa", "Persian"),
    ("fi", "Finnish"), ("fil", "Filipino"), ("fr", "French"), ("ga", "Irish"), ("gl", "Galician"), ("gu", "Gujarati"), ("ha", "Hausa"),
    ("he", "Hebrew"), ("hi", "Hindi"), ("hr", "Croatian"), ("hu", "Hungarian"), ("hy", "Armenian"), ("id", "Indonesian"), ("ig", "Igbo"),
    ("is", "Icelandic"), ("it", "Italian"), ("ja", "Japanese"), ("jv", "Javanese"), ("ka", "Georgian"), ("kk", "Kazakh"), ("km", "Khmer"),
    ("kn", "Kannada"), ("ko", "Korean"), ("ky", "Kyrgyz"), ("la", "Latin"), ("lo", "Lao"), ("lt", "Lithuanian"), ("lv", "Latvian"),
    ("mk", "Macedonian"), ("ml", "Malayalam"), ("mn", "Mongolian"), ("mr", "Marathi"), ("ms", "Malay"), ("mt", "Maltese"),
    ("my", "Burmese"), ("nb", "Norwegian Bokmål"), ("ne", "Nepali"), ("nl", "Dutch"), ("nn", "Norwegian Nynorsk"), ("no", "Norwegian"),
    ("pa", "Punjabi"), ("pl", "Polish"), ("ps", "Pashto"), ("pt", "Portuguese"), ("ro", "Romanian"), ("ru", "Russian"), ("si", "Sinhala"),
    ("sk", "Slovak"), ("sl", "Slovenian"), ("so", "Somali"), ("sq", "Albanian"), ("sr", "Serbian"), ("sv", "Swedish"), ("sw", "Swahili"),
    ("ta", "Tamil"), ("te", "Telugu"), ("th", "Thai"), ("tr", "Turkish"), ("uk", "Ukrainian"), ("ur", "Urdu"), ("uz", "Uzbek"),
    ("vi", "Vietnamese"), ("yi", "Yiddish"), ("yo", "Yoruba"), ("zh", "Chinese"), ("zu", "Zulu"),
];

static DIALECTS: &[(&str, &str)] = &[
    ("de-AT", "Austrian German"), ("de-CH", "Swiss High German"), ("en-AU", "Australian English"), ("en-CA", "Canadian English"),
    ("en-GB", "British English"), ("en-US", "American English"), ("es-419", "Latin American Spanish"), ("es-ES", "European Spanish"),
    ("es-MX", "Mexican Spanish"), ("fr-CA", "Canadian French"), ("fr-CH", "Swiss French"), ("nl-BE", "Flemish"),
    ("pt-BR", "Brazilian Portuguese"), ("pt-PT", "European Portuguese"), ("zh-Hans", "Simplified Chinese"), ("zh-Hant", "Traditional Chinese"),
];

static REGIONS: &[(&str, &str, &str)] = &[
    ("001", "world", ""), ("150", "Europe", ""), ("419", "Latin America", ""), ("AE", "United Arab Emirates", "UAE"), ("AR", "Argentina", ""),
    ("AT", "Austria", ""), ("AU", "Australia", ""), ("BD", "Bangladesh", ""), ("BE", "Belgium", ""), ("BG", "Bulgaria", ""),
    ("BR", "Brazil", ""), ("CA", "Canada", ""), ("CH", "Switzerland", ""), ("CL", "Chile", ""), ("CN", "China", ""), ("CO", "Colombia", ""),
    ("CZ", "Czechia", ""), ("DE", "Germany", ""), ("DK", "Denmark", ""), ("EG", "Egypt", ""), ("ES", "Spain", ""), ("EU", "European Union", ""),
    ("FI", "Finland", ""), ("FR", "France", ""), ("GB", "United Kingdom", "UK"), ("GR", "Greece", ""), ("HK", "Hong Kong SAR China", "Hong Kong"),
    ("HU", "Hungary", ""), ("ID", "Indonesia", ""), ("IE", "Ireland", ""), ("IL", "Israel", ""), ("IN", "India", ""), ("IS", "Iceland", ""),
    ("IT", "Italy", ""), ("JP", "Japan", ""), ("KE", "Kenya", ""), ("KR", "South Korea", ""), ("MA", "Morocco", ""), ("MX", "Mexico", ""),
    ("MY", "Malaysia", ""), ("NG", "Nigeria", ""), ("NL", "Netherlands", ""), ("NO", "Norway", ""), ("NZ", "New Zealand", ""),
    ("PE", "Peru", ""), ("PH", "Philippines", ""), ("PK", "Pakistan", ""), ("PL", "Poland", ""), ("PT", "Portugal", ""), ("RO", "Romania", ""),
    ("RU", "Russia", ""), ("SA", "Saudi Arabia", ""), ("SE", "Sweden", ""), ("SG", "Singapore", ""), ("SK", "Slovakia", ""), ("TH", "Thailand", ""),
    ("TR", "Türkiye", ""), ("TW", "Taiwan", ""), ("UA", "Ukraine", ""), ("UN", "United Nations", "UN"), ("US", "United States", "US"),
    ("VE", "Venezuela", ""), ("VN", "Vietnam", ""), ("ZA", "South Africa", ""),
];

static SCRIPTS: &[(&str, &str)] = &[
    ("Arab", "Arabic"), ("Cyrl", "Cyrillic"), ("Deva", "Devanagari"), ("Grek", "Greek"), ("Hang", "Hangul"), ("Hans", "Simplified Han"),
    ("Hant", "Traditional Han"), ("Hebr", "Hebrew"), ("Hira", "Hiragana"), ("Jpan", "Japanese"), ("Kana", "Katakana"), ("Kore", "Korean"),
    ("Latn", "Latin"), ("Thai", "Thai"),
];

static CALENDARS: &[(&str, &str)] = &[
    ("buddhist", "Buddhist Calendar"), ("chinese", "Chinese Calendar"), ("gregory", "Gregorian Calendar"), ("hebrew", "Hebrew Calendar"),
    ("islamic", "Hijri Calendar"), ("iso8601", "ISO-8601 Calendar"), ("japanese", "Japanese Calendar"), ("persian", "Persian Calendar"),
];

/// (field, long, short, narrow).
static FIELDS: &[(&str, &str, &str, &str)] = &[
    ("era", "era", "era", "era"), ("year", "year", "yr.", "yr"), ("quarter", "quarter", "qtr.", "qtr"), ("month", "month", "mo.", "mo"),
    ("weekOfYear", "week", "wk.", "wk"), ("weekday", "day of the week", "day of wk.", "day of wk."), ("day", "day", "day", "day"),
    ("dayPeriod", "AM/PM", "AM/PM", "AM/PM"), ("hour", "hour", "hr.", "hr"), ("minute", "minute", "min.", "min"),
    ("second", "second", "sec.", "sec"), ("timeZoneName", "time zone", "zone", "zone"),
];

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn region_name(code: &str, style: &str) -> Option<String> {
    REGIONS.iter().find(|(c, _, _)| *c == code).map(|(_, l, s)| String::from(if style != "long" && !s.is_empty() { *s } else { *l }))
}

fn language_name(t: &LangTag, dialect: bool) -> Option<String> {
    let mut base_used: Vec<&str> = Vec::new();
    let mut name: Option<String> = None;
    if dialect {
        let with_region = t.region.as_ref().map(|r| format!("{}-{}", t.language, r));
        let with_script = t.script.as_ref().map(|s| format!("{}-{}", t.language, s));
        if let Some(n) = with_region.as_deref().and_then(|k| DIALECTS.iter().find(|(c, _)| *c == k)) {
            name = Some(String::from(n.1));
            base_used.push("region");
        } else if let Some(n) = with_script.as_deref().and_then(|k| DIALECTS.iter().find(|(c, _)| *c == k)) {
            name = Some(String::from(n.1));
            base_used.push("script");
        }
    }
    let name = match name {
        Some(n) => n,
        None => String::from(LANGUAGES.iter().find(|(c, _)| *c == t.language)?.1),
    };
    let mut extra: Vec<String> = Vec::new();
    if let Some(s) = &t.script {
        if !base_used.contains(&"script") {
            extra.push(SCRIPTS.iter().find(|(c, _)| c == s).map(|(_, n)| String::from(*n)).unwrap_or_else(|| s.clone()));
        }
    }
    if let Some(r) = &t.region {
        if !base_used.contains(&"region") {
            extra.push(region_name(r, "long").unwrap_or_else(|| r.clone()));
        }
    }
    for v in &t.variants {
        extra.push(v.to_ascii_uppercase());
    }
    Some(if extra.is_empty() { name } else { format!("{name} ({})", extra.join(", ")) })
}

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "DisplayNames", 2, dn_ctor);
    method(vm, p, "of", 1, dn_of);
    method(vm, p, "resolvedOptions", 0, dn_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_display_names_proto = p;
}

fn dn_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "DisplayNames")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_display_names_proto)?;
    let l = vm.arg(ctx, 0);
    let requested = canonicalize_locale_list(vm, &l)?;
    let ov = vm.arg(ctx, 1);
    let o = match ov {
        Value::Object(o) => Some(o),
        _ => return vm.throw_type("Intl.DisplayNames requires an options object with a type"),
    };
    let svc = "Intl.DisplayNames";
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], svc)?;
    let r = resolve_locale(&requested, &[]);
    let style = get_string_option(vm, o, "style", &["narrow", "short", "long"], svc)?;
    let kind = get_string_option(vm, o, "type", &["language", "region", "script", "currency", "calendar", "dateTimeField"], svc)?;
    let Some(kind) = kind else { return vm.throw_type("Intl.DisplayNames: the type option is required") };
    let fallback = get_string_option(vm, o, "fallback", &["code", "none"], svc)?;
    let ld = get_string_option(vm, o, "languageDisplay", &["dialect", "standard"], svc)?;
    let f = DnFmt {
        locale: String::from(r.data_locale),
        style: match style.as_deref() {
            Some("narrow") => "narrow",
            Some("short") => "short",
            _ => "long",
        },
        kind: match kind.as_str() {
            "language" => "language",
            "region" => "region",
            "script" => "script",
            "currency" => "currency",
            "calendar" => "calendar",
            _ => "dateTimeField",
        },
        fallback: if fallback.as_deref() == Some("none") { "none" } else { "code" },
        language_display: if ld.as_deref() == Some("standard") { "standard" } else { "dialect" },
    };
    Ok(Value::Object(alloc_intl(vm, p, IntlData::DisplayNames(Box::new(f)))))
}

fn dn_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::DisplayNames(_)), "Intl.DisplayNames.prototype.of")?;
    let f = match intl_ref(vm, o) {
        IntlData::DisplayNames(f) => (**f).clone(),
        _ => unreachable!(),
    };
    let cv = vm.arg(ctx, 0);
    let code = vm.to_string(&cv)?.to_rust();
    let bad = |vm: &mut Vm| -> JsResult<Value> { vm.throw_range(&format!("invalid_argument: {code}")) };
    let (canon, name): (String, Option<String>) = match f.kind {
        "language" => {
            let t = match LangTag::parse(&code) {
                Some(t) if t.keywords.is_empty() && t.attributes.is_empty() && t.other.is_empty() => t,
                _ => return bad(vm),
            };
            (t.to_string(), language_name(&t, f.language_display == "dialect"))
        }
        "region" => {
            let ok = (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic())) || (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()));
            if !ok {
                return bad(vm);
            }
            let c = code.to_ascii_uppercase();
            let n = region_name(&c, f.style);
            (c, n)
        }
        "script" => {
            if !(code.len() == 4 && code.bytes().all(|b| b.is_ascii_alphabetic())) {
                return bad(vm);
            }
            let l = code.to_ascii_lowercase();
            let c = format!("{}{}", l[..1].to_ascii_uppercase(), &l[1..]);
            let n = SCRIPTS.iter().find(|(k, _)| *k == c).map(|(_, n)| String::from(*n));
            (c, n)
        }
        "currency" => {
            if !(code.len() == 3 && code.bytes().all(|b| b.is_ascii_alphabetic())) {
                return bad(vm);
            }
            let c = code.to_ascii_uppercase();
            let n = data::currency(&c).map(|x| title_case(x.one));
            (c, n)
        }
        "calendar" => {
            if !code.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
                return bad(vm);
            }
            let c = code.to_ascii_lowercase();
            let c = if c == "gregorian" { String::from("gregory") } else { c };
            let n = CALENDARS.iter().find(|(k, _)| *k == c).map(|(_, n)| String::from(*n));
            (c, n)
        }
        _ => {
            let Some(x) = FIELDS.iter().find(|(k, ..)| *k == code) else { return bad(vm) };
            let n = match f.style {
                "short" => x.2,
                "narrow" => x.3,
                _ => x.1,
            };
            (code.clone(), Some(String::from(n)))
        }
    };
    Ok(match name {
        Some(n) => Value::str(&n),
        None if f.fallback == "code" => Value::str(&canon),
        None => Value::Undefined,
    })
}

fn dn_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::DisplayNames(_)), "Intl.DisplayNames.prototype.resolvedOptions")?;
    let f = match intl_ref(vm, o) {
        IntlData::DisplayNames(f) => (**f).clone(),
        _ => unreachable!(),
    };
    let mut props = vec![
        ("locale", Value::str(&f.locale)),
        ("style", Value::str(f.style)),
        ("type", Value::str(f.kind)),
        ("fallback", Value::str(f.fallback)),
    ];
    if f.kind == "language" {
        props.push(("languageDisplay", Value::str(f.language_display)));
    }
    let r = object_from(vm, props);
    Ok(Value::Object(r))
}
