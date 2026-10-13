//! Language tags (BCP 47 / UTS #35 Unicode locale identifiers: ECMA-402 §6.2 IsStructurallyValidLanguageTag
//! and CanonicalizeUnicodeLocaleId) and `Intl.Locale` (§14).

use super::*;

/// A parsed Unicode BCP 47 locale identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct LangTag {
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
    pub variants: Vec<String>,
    /// `-u-` keywords (key, value; value "" for a bare key), in canonical (sorted) order.
    pub keywords: Vec<(String, String)>,
    /// `-u-` attributes.
    pub attributes: Vec<String>,
    /// Other extensions and the private-use tail, already canonical (`-t-…`, `-x-…`).
    pub other: Vec<(char, String)>,
}

fn alpha(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic())
}
fn digit(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
fn alnum(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn lang_alias(l: &str) -> &str {
    match l {
        "iw" => "he",
        "in" => "id",
        "ji" => "yi",
        "jw" => "jv",
        "mo" => "ro",
        "tl" => "fil",
        "sh" => "sr-Latn",
        _ => l,
    }
}

impl LangTag {
    /// IsStructurallyValidLanguageTag + canonicalization; None when the tag is not well formed.
    pub fn parse(s: &str) -> Option<LangTag> {
        if s.is_empty() || !s.is_ascii() || s.starts_with('-') || s.ends_with('-') || s.contains("--") {
            return None;
        }
        let lower = s.to_ascii_lowercase();
        let subtags: Vec<&str> = lower.split(['-', '_']).collect();
        if lower.contains('_') {
            return None;
        }
        let mut i = 0;
        let lang = subtags[0];
        if !(alpha(lang) && (matches!(lang.len(), 2 | 3) || (5..=8).contains(&lang.len()))) {
            return None;
        }
        i += 1;
        let mut script = None;
        let mut region = None;
        let mut variants: Vec<String> = Vec::new();
        if i < subtags.len() && subtags[i].len() == 4 && alpha(subtags[i]) {
            let sc = subtags[i];
            script = Some(format!("{}{}", &sc[..1].to_ascii_uppercase(), &sc[1..]));
            i += 1;
        }
        if i < subtags.len() && ((subtags[i].len() == 2 && alpha(subtags[i])) || (subtags[i].len() == 3 && digit(subtags[i]))) {
            region = Some(subtags[i].to_ascii_uppercase());
            i += 1;
        }
        while i < subtags.len() {
            let v = subtags[i];
            let is_variant = ((5..=8).contains(&v.len()) && alnum(v)) || (v.len() == 4 && v.as_bytes()[0].is_ascii_digit() && alnum(v));
            if !is_variant {
                break;
            }
            if variants.iter().any(|x| x == v) {
                return None;
            }
            variants.push(v.to_string());
            i += 1;
        }
        variants.sort();
        let mut keywords: Vec<(String, String)> = Vec::new();
        let mut attributes: Vec<String> = Vec::new();
        let mut other: Vec<(char, String)> = Vec::new();
        let mut singletons_seen: Vec<char> = Vec::new();
        while i < subtags.len() {
            let st = subtags[i];
            if st.len() != 1 || !alnum(st) {
                return None;
            }
            let c = st.as_bytes()[0] as char;
            i += 1;
            if c == 'x' {
                let rest: Vec<&str> = subtags[i..].to_vec();
                if rest.is_empty() || rest.iter().any(|r| r.is_empty() || r.len() > 8 || !alnum(r)) {
                    return None;
                }
                other.push(('x', rest.join("-")));
                i = subtags.len();
                break;
            }
            if singletons_seen.contains(&c) {
                return None;
            }
            singletons_seen.push(c);
            let start = i;
            while i < subtags.len() && subtags[i].len() >= 2 && subtags[i].len() <= 8 && alnum(subtags[i]) {
                i += 1;
            }
            if i == start {
                return None;
            }
            let body = &subtags[start..i];
            if c == 'u' {
                let mut j = 0;
                while j < body.len() && body[j].len() >= 3 {
                    attributes.push(body[j].to_string());
                    j += 1;
                }
                while j < body.len() {
                    let key = body[j];
                    if key.len() != 2 || !key.as_bytes()[1].is_ascii_alphabetic() {
                        return None;
                    }
                    j += 1;
                    let vs = j;
                    while j < body.len() && body[j].len() >= 3 {
                        j += 1;
                    }
                    let mut val = body[vs..j].join("-");
                    if val == "true" {
                        val.clear();
                    }
                    if !keywords.iter().any(|(k, _)| k == key) {
                        keywords.push((key.to_string(), val));
                    }
                }
                keywords.sort_by(|a, b| a.0.cmp(&b.0));
            } else {
                other.push((c, body.join("-")));
            }
        }
        if i != subtags.len() {
            return None;
        }
        other.sort_by(|a, b| match (a.0 == 'x', b.0 == 'x') {
            (true, false) => core::cmp::Ordering::Greater,
            (false, true) => core::cmp::Ordering::Less,
            _ => a.0.cmp(&b.0),
        });
        let mut language = lang_alias(lang).to_string();
        if let Some(pos) = language.find('-') {
            // sh → sr-Latn
            if script.is_none() {
                script = Some(language[pos + 1..].to_string());
            }
            language.truncate(pos);
        }
        Some(LangTag { language, script, region, variants, keywords, attributes, other })
    }

    pub fn base_name(&self) -> String {
        let mut s = self.language.clone();
        if let Some(sc) = &self.script {
            s.push('-');
            s.push_str(sc);
        }
        if let Some(r) = &self.region {
            s.push('-');
            s.push_str(r);
        }
        for v in &self.variants {
            s.push('-');
            s.push_str(v);
        }
        s
    }

    pub fn keyword(&self, k: &str) -> Option<&str> {
        self.keywords.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }

    pub fn set_keyword(&mut self, k: &str, v: &str) {
        let v = if v == "true" { "" } else { v };
        if let Some(e) = self.keywords.iter_mut().find(|(a, _)| a == k) {
            e.1 = v.to_string();
        } else {
            self.keywords.push((k.to_string(), v.to_string()));
            self.keywords.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
}

impl core::fmt::Display for LangTag {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.base_name())?;
        let mut exts: Vec<(char, String)> = self.other.iter().filter(|(c, _)| *c != 'x').cloned().collect();
        if !self.keywords.is_empty() || !self.attributes.is_empty() {
            let mut u: Vec<String> = self.attributes.clone();
            for (k, v) in &self.keywords {
                u.push(k.clone());
                if !v.is_empty() {
                    u.push(v.clone());
                }
            }
            exts.push(('u', u.join("-")));
        }
        exts.sort_by(|a, b| a.0.cmp(&b.0));
        for (c, body) in exts {
            write!(f, "-{c}-{body}")?;
        }
        if let Some((_, x)) = self.other.iter().find(|(c, _)| *c == 'x') {
            write!(f, "-x-{x}")?;
        }
        Ok(())
    }
}

/// The tag without its extensions and private-use tail (§9.2.2 step 1 "removing extensions").
pub fn strip_extensions(tag: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for st in tag.split('-') {
        if st.len() == 1 {
            break;
        }
        out.push(st);
    }
    out.join("-")
}

/// Add Likely Subtags (UTS #35 §4.3) for the languages the data names; others keep their tag.
fn likely(t: &LangTag) -> (String, Option<String>, Option<String>) {
    let (sc, rg) = match t.language.as_str() {
        "en" => ("Latn", "US"),
        "de" => ("Latn", "DE"),
        "fr" => ("Latn", "FR"),
        "es" => ("Latn", "ES"),
        "it" => ("Latn", "IT"),
        "pt" => ("Latn", "BR"),
        "nl" => ("Latn", "NL"),
        "sv" => ("Latn", "SE"),
        "pl" => ("Latn", "PL"),
        "ru" => ("Cyrl", "RU"),
        "uk" => ("Cyrl", "UA"),
        "el" => ("Grek", "GR"),
        "he" => ("Hebr", "IL"),
        "ar" => ("Arab", "EG"),
        "hi" => ("Deva", "IN"),
        "ja" => ("Jpan", "JP"),
        "ko" => ("Kore", "KR"),
        "zh" => ("Hans", "CN"),
        "tr" => ("Latn", "TR"),
        "und" => ("Latn", "US"),
        _ => return (t.language.clone(), t.script.clone(), t.region.clone()),
    };
    let lang = if t.language == "und" { String::from("en") } else { t.language.clone() };
    (lang, Some(t.script.clone().unwrap_or_else(|| sc.to_string())), Some(t.region.clone().unwrap_or_else(|| rg.to_string())))
}

// ============================================================================================ Intl.Locale

pub fn init(vm: &mut Vm, intl: Obj) {
    let (c, p) = service(vm, intl, "Locale", 1, locale_ctor);
    // Locale has no supportedLocalesOf; remove the one `service` installs.
    vm.heap.get_mut(c).props.remove(&PropertyKey::from_str("supportedLocalesOf"));
    method(vm, p, "maximize", 0, loc_maximize);
    method(vm, p, "minimize", 0, loc_minimize);
    method(vm, p, "toString", 0, loc_to_string);
    method(vm, p, "getHourCycles", 0, loc_hour_cycles);
    method(vm, p, "getCalendars", 0, loc_calendars);
    method(vm, p, "getTextInfo", 0, loc_text_info);
    for (n, g) in [
        ("baseName", loc_base_name as NativeFn),
        ("calendar", loc_calendar),
        ("caseFirst", loc_case_first),
        ("collation", loc_collation),
        ("hourCycle", loc_hour_cycle),
        ("numeric", loc_numeric),
        ("numberingSystem", loc_numbering_system),
        ("language", loc_language),
        ("script", loc_script),
        ("region", loc_region),
        ("variants", loc_variants),
    ] {
        accessor(vm, p, PropertyKey::from_str(n), n, Some(g), None, C);
    }
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_locale_proto = p;
}

fn locale_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "Locale")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_locale_proto)?;
    let tv = vm.arg(ctx, 0);
    let tag = match &tv {
        Value::String(s) => s.to_rust(),
        Value::Object(o) => {
            if let Kind::Intl(i) = &vm.heap.get(*o).kind {
                if let IntlData::Locale(l) = &i.data {
                    l.to_string()
                } else {
                    vm.to_string(&tv)?.to_rust()
                }
            } else {
                vm.to_string(&tv)?.to_rust()
            }
        }
        _ => return vm.throw_type("First argument to Intl.Locale constructor can't be empty or missing"),
    };
    let ov = vm.arg(ctx, 1);
    let o = coerce_options(vm, &ov)?;
    let mut t = match LangTag::parse(&tag) {
        Some(t) => t,
        None => return vm.throw_range(&format!("Incorrect locale information provided: {tag}")),
    };
    if let Some(l) = get_string_option(vm, o, "language", &[], "Intl.Locale")? {
        if !(alpha(&l) && matches!(l.len(), 2 | 3 | 5..=8)) {
            return vm.throw_range("Incorrect locale information provided");
        }
        t.language = l.to_ascii_lowercase();
    }
    if let Some(s) = get_string_option(vm, o, "script", &[], "Intl.Locale")? {
        if !(s.len() == 4 && alpha(&s)) {
            return vm.throw_range("Incorrect locale information provided");
        }
        let s = s.to_ascii_lowercase();
        t.script = Some(format!("{}{}", &s[..1].to_ascii_uppercase(), &s[1..]));
    }
    if let Some(r) = get_string_option(vm, o, "region", &[], "Intl.Locale")? {
        if !((r.len() == 2 && alpha(&r)) || (r.len() == 3 && digit(&r))) {
            return vm.throw_range("Incorrect locale information provided");
        }
        t.region = Some(r.to_ascii_uppercase());
    }
    for (opt, key, allowed) in [
        ("calendar", "ca", &[][..]),
        ("collation", "co", &[][..]),
        ("hourCycle", "hc", &["h11", "h12", "h23", "h24"][..]),
        ("caseFirst", "kf", &["upper", "lower", "false"][..]),
    ] {
        if let Some(v) = get_string_option(vm, o, opt, allowed, "Intl.Locale")? {
            if allowed.is_empty() && !v.split('-').all(|p| (3..=8).contains(&p.len()) && alnum(p)) {
                return vm.throw_range(&format!("Incorrect locale information provided: {opt}"));
            }
            t.set_keyword(key, &v.to_ascii_lowercase());
        }
    }
    if let Some(b) = get_bool_option(vm, o, "numeric")? {
        t.set_keyword("kn", if b { "" } else { "false" });
    }
    if let Some(v) = get_string_option(vm, o, "numberingSystem", &[], "Intl.Locale")? {
        if !v.split('-').all(|p| (3..=8).contains(&p.len()) && alnum(p)) {
            return vm.throw_range("Incorrect locale information provided: numberingSystem");
        }
        t.set_keyword("nu", &v.to_ascii_lowercase());
    }
    Ok(Value::Object(alloc_intl(vm, p, IntlData::Locale(Box::new(t)))))
}

fn this_loc(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<LangTag> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::Locale(_)), what)?;
    match intl_ref(vm, o) {
        IntlData::Locale(l) => Ok((**l).clone()),
        _ => unreachable!(),
    }
}

fn new_loc(vm: &mut Vm, t: LangTag) -> Value {
    let p = vm.intr().intl_locale_proto;
    Value::Object(alloc_intl(vm, p, IntlData::Locale(Box::new(t))))
}

fn loc_maximize(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut t = this_loc(vm, ctx, "Intl.Locale.prototype.maximize")?;
    let (l, s, r) = likely(&t);
    t.language = l;
    t.script = s;
    t.region = r;
    Ok(new_loc(vm, t))
}
fn loc_minimize(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut t = this_loc(vm, ctx, "Intl.Locale.prototype.minimize")?;
    let (_, s, r) = likely(&LangTag { script: None, region: None, ..t.clone() });
    if t.script == s {
        t.script = None;
    }
    if t.region == r {
        t.region = None;
    }
    if t.language == "und" {
        t.language = String::from("en");
    }
    Ok(new_loc(vm, t))
}
fn loc_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "Intl.Locale.prototype.toString")?;
    Ok(Value::str(&t.to_string()))
}
fn opt_str(v: Option<&str>) -> Value {
    match v {
        Some(s) => Value::str(s),
        None => Value::Undefined,
    }
}
fn loc_base_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.baseName")?;
    Ok(Value::str(&t.base_name()))
}
fn loc_calendar(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.calendar")?;
    Ok(opt_str(t.keyword("ca")))
}
fn loc_case_first(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.caseFirst")?;
    Ok(opt_str(t.keyword("kf")))
}
fn loc_collation(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.collation")?;
    Ok(opt_str(t.keyword("co")))
}
fn loc_hour_cycle(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.hourCycle")?;
    Ok(opt_str(t.keyword("hc")))
}
fn loc_numeric(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.numeric")?;
    Ok(Value::Bool(matches!(t.keyword("kn"), Some("") | Some("true"))))
}
fn loc_numbering_system(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.numberingSystem")?;
    Ok(opt_str(t.keyword("nu")))
}
fn loc_language(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.language")?;
    Ok(Value::str(&t.language))
}
fn loc_script(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.script")?;
    Ok(opt_str(t.script.as_deref()))
}
fn loc_region(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.region")?;
    Ok(opt_str(t.region.as_deref()))
}
fn loc_variants(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "get Intl.Locale.prototype.variants")?;
    if t.variants.is_empty() {
        return Ok(Value::Undefined);
    }
    Ok(Value::str(&t.variants.join("-")))
}
fn loc_hour_cycles(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "Intl.Locale.prototype.getHourCycles")?;
    let hc = match t.keyword("hc") {
        Some(h) => String::from(h),
        None => String::from(if t.region.as_deref() == Some("GB") { "h23" } else { "h12" }),
    };
    Ok(Value::Object(vm.new_array(vec![Value::str(&hc)])))
}
fn loc_calendars(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "Intl.Locale.prototype.getCalendars")?;
    let ca = String::from(t.keyword("ca").unwrap_or("gregory"));
    Ok(Value::Object(vm.new_array(vec![Value::str(&ca)])))
}
fn loc_text_info(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_loc(vm, ctx, "Intl.Locale.prototype.getTextInfo")?;
    let rtl = matches!(t.language.as_str(), "ar" | "he" | "fa" | "ur" | "yi" | "ps" | "sd" | "ug" | "dv");
    let o = object_from(vm, vec![("direction", Value::str(if rtl { "rtl" } else { "ltr" }))]);
    Ok(Value::Object(o))
}
