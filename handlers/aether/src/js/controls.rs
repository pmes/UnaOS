//! HTML §4.10 form controls, the DOM half (AETHERWPT, SR72): the input element's value modes and
//! per-type value sanitization (§4.10.5.1), the type-change steps, the date/time/number microsyntaxes
//! behind `valueAsNumber` / `valueAsDate` / `stepUp` / `stepDown`, the text selection APIs
//! (§4.10.18.8), the constraint validation API (§4.10.20: ValidityState, willValidate,
//! checkValidity/reportValidity, setCustomValidity, validationMessage), form owners and the `form`
//! attribute (§4.10.17.3), `form.elements`, `labels`, the cloning steps, and meter/progress.
//!
//! Installed after `html::install`; the names it defines supersede that file's simpler versions. The
//! dirty value and dirty checkedness live where the renderer reads them (`PageState::values` /
//! `checked`); the selection and the custom validity message live here.

use super::dom::{self, attr_value, remove_attr, set_attr, this_element, with_doc, wrap, wrap_opt};
use super::idl::*;
use super::page;
use html_core::{Document, Namespace, NodeId};
use js_core::vm::*;
use std::cell::RefCell;
use std::collections::HashMap;

pub const T_VALIDITY: u8 = 44;
pub const T_FILELIST: u8 = 45;

const SO_VALIDITY: u8 = 60;
const SO_LABELS: u8 = 61;
const SO_ELEMENTS: u8 = 62;
const SO_FILES: u8 = 63;
const SO_SELECTED: u8 = 64;
const T_COLL: u8 = 3;

#[derive(Clone, Copy, Default)]
struct Sel {
    start: u32,
    end: u32,
    /// 0 none, 1 forward, 2 backward.
    dir: u8,
}

thread_local! {
    static SELECTION: RefCell<HashMap<usize, Sel>> = RefCell::new(HashMap::new());
    static CUSTOM: RefCell<HashMap<usize, String>> = RefCell::new(HashMap::new());
    /// Forms whose "firing submission events" flag is set (§4.10.21.3 step 6).
    static INDETERMINATE: RefCell<std::collections::HashSet<usize>> = RefCell::new(std::collections::HashSet::new());
    static RESETTING: RefCell<std::collections::HashSet<usize>> = RefCell::new(std::collections::HashSet::new());
    static SUBMITTING: RefCell<std::collections::HashSet<usize>> = RefCell::new(std::collections::HashSet::new());
}

pub fn reset() {
    INDETERMINATE.with(|s| s.borrow_mut().clear());
    SUBMITTING.with(|s| s.borrow_mut().clear());
    SELECTION.with(|s| s.borrow_mut().clear());
    CUSTOM.with(|s| s.borrow_mut().clear());
}

// ------------------------------------------------------------------------------------------------ element helpers

fn html_local(d: &Document, el: usize) -> Option<String> {
    d.element(NodeId(el)).filter(|e| e.ns == Namespace::Html).map(|e| e.local.to_string())
}

fn local(el: usize) -> String {
    with_doc(|d| html_local(d, el)).unwrap_or_default()
}

const INPUT_TYPES: &[&str] = &[
    "hidden", "text", "search", "tel", "url", "email", "password", "date", "month", "week", "time", "datetime-local",
    "number", "range", "color", "checkbox", "radio", "file", "submit", "image", "reset", "button",
];

/// The input's type state from a `type` attribute value.
pub fn type_from(v: Option<&str>) -> &'static str {
    let t = v.unwrap_or("").to_ascii_lowercase();
    INPUT_TYPES.iter().find(|x| **x == t).copied().unwrap_or("text")
}

pub fn input_type(el: usize) -> &'static str {
    type_from(attr_value(el, "type").as_deref())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Value,
    Default,
    DefaultOn,
    Filename,
}

fn mode(ty: &str) -> Mode {
    match ty {
        "hidden" | "submit" | "image" | "reset" | "button" => Mode::Default,
        "checkbox" | "radio" => Mode::DefaultOn,
        "file" => Mode::Filename,
        _ => Mode::Value,
    }
}

/// Whether the selection APIs apply (§4.10.5.1 table: text, search, url, tel, password; and textarea).
fn selectable(el: usize) -> bool {
    match local(el).as_str() {
        "textarea" => true,
        "input" => matches!(input_type(el), "text" | "search" | "url" | "tel" | "password"),
        _ => false,
    }
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

fn strip_newlines(v: &str) -> String {
    v.chars().filter(|c| *c != '\n' && *c != '\r').collect()
}

fn trim_ws(v: &str) -> &str {
    v.trim_matches(is_ws)
}

// ------------------------------------------------------------------------------------------------ microsyntaxes

/// "valid floating-point number" (HTML §2.3.4.3) → its value.
pub fn parse_valid_float(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    let d0 = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i > d0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f0 = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == f0 {
            return None;
        }
    } else if !int_digits {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let e0 = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == e0 {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    let v: f64 = s.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some(if v == 0.0 { 0.0 } else { v })
}

fn digits(s: &str, n: usize) -> Option<u32> {
    if s.len() != n || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

/// Days from 1970-01-01 to y-m-d (proleptic Gregorian).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "YYYY-MM" (year ≥ 4 digits, > 0).
fn parse_ym(s: &str) -> Option<(i64, u32)> {
    let (y, m) = s.rsplit_once('-')?;
    if y.len() < 4 || !y.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let y: i64 = y.parse().ok()?;
    let m = digits(m, 2)?;
    if y <= 0 || !(1..=12).contains(&m) {
        return None;
    }
    Some((y, m))
}

fn parse_date(s: &str) -> Option<(i64, u32, u32)> {
    let (ym, d) = s.rsplit_once('-')?;
    let (y, m) = parse_ym(ym)?;
    let d = digits(d, 2)?;
    if d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some((y, m, d))
}

/// "HH:MM[:SS[.f{1,3}]]" → milliseconds since midnight.
fn parse_time(s: &str) -> Option<f64> {
    let mut parts = s.splitn(3, ':');
    let h = digits(parts.next()?, 2)?;
    let m = digits(parts.next()?, 2)?;
    if h > 23 || m > 59 {
        return None;
    }
    let mut ms = (h as f64 * 60.0 + m as f64) * 60_000.0;
    if let Some(sec) = parts.next() {
        let (ss, frac) = match sec.split_once('.') {
            Some((a, b)) => (a, Some(b)),
            None => (sec, None),
        };
        let ss = digits(ss, 2)?;
        if ss > 59 {
            return None;
        }
        ms += ss as f64 * 1000.0;
        if let Some(f) = frac {
            if f.is_empty() || f.len() > 3 || !f.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let v: f64 = f.parse().ok()?;
            ms += v * 10f64.powi(3 - f.len() as i32);
        }
    }
    Some(ms)
}

fn weeks_in_year(y: i64) -> u32 {
    // A year has 53 weeks when Jan 1 is a Thursday, or a Wednesday in a leap year.
    let wd = (days_from_civil(y, 1, 1) + 4).rem_euclid(7); // 0 = Sunday
    if wd == 4 || (wd == 3 && is_leap(y)) {
        53
    } else {
        52
    }
}

/// Monday of ISO week 1 of `y`, in days since the epoch.
fn week1_monday(y: i64) -> i64 {
    let jan4 = days_from_civil(y, 1, 4);
    let wd = (jan4 + 3).rem_euclid(7); // 0 = Monday
    jan4 - wd
}

fn parse_week(s: &str) -> Option<(i64, u32)> {
    let (y, w) = s.split_once("-W")?;
    if y.len() < 4 || !y.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let y: i64 = y.parse().ok()?;
    let w = digits(w, 2)?;
    if y <= 0 || w == 0 || w > weeks_in_year(y) {
        return None;
    }
    Some((y, w))
}

fn parse_local_dt(s: &str) -> Option<f64> {
    let i = s.find(['T', ' '])?;
    let (y, m, d) = parse_date(&s[..i])?;
    let t = parse_time(&s[i + 1..])?;
    Some(days_from_civil(y, m, d) as f64 * 86_400_000.0 + t)
}

fn fmt_year(y: i64) -> String {
    format!("{y:04}")
}

fn fmt_time(ms: f64) -> String {
    let ms = ms.rem_euclid(86_400_000.0).round() as i64;
    let (h, m, sec, f) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if sec == 0 && f == 0 {
        format!("{h:02}:{m:02}")
    } else if f == 0 {
        format!("{h:02}:{m:02}:{sec:02}")
    } else {
        format!("{h:02}:{m:02}:{sec:02}.{f:03}")
    }
}

/// "convert a string to a number" for the numeric/temporal types; None on error.
pub fn to_number(ty: &str, v: &str) -> Option<f64> {
    match ty {
        "number" | "range" => parse_valid_float(v),
        "date" => parse_date(v).map(|(y, m, d)| days_from_civil(y, m, d) as f64 * 86_400_000.0),
        "month" => parse_ym(v).map(|(y, m)| ((y - 1970) * 12 + m as i64 - 1) as f64),
        "week" => parse_week(v).map(|(y, w)| (week1_monday(y) + 7 * (w as i64 - 1)) as f64 * 86_400_000.0),
        "time" => parse_time(v),
        "datetime-local" => parse_local_dt(v),
        _ => None,
    }
}

/// "convert a number to a string".
fn from_number(vm: &mut Vm, ty: &str, n: f64) -> JsResult<String> {
    Ok(match ty {
        "date" => {
            let (y, m, d) = civil_from_days((n / 86_400_000.0).floor() as i64);
            format!("{}-{m:02}-{d:02}", fmt_year(y))
        }
        "month" => {
            let n = n.floor() as i64;
            let y = 1970 + n.div_euclid(12);
            let m = n.rem_euclid(12) + 1;
            format!("{}-{m:02}", fmt_year(y))
        }
        "week" => {
            let days = (n / 86_400_000.0).floor() as i64;
            let (mut y, _, _) = civil_from_days(days);
            if days >= week1_monday(y + 1) {
                y += 1;
            } else if days < week1_monday(y) {
                y -= 1;
            }
            let w = (days - week1_monday(y)) / 7 + 1;
            format!("{}-W{w:02}", fmt_year(y))
        }
        "time" => fmt_time(n),
        "datetime-local" => {
            let days = (n / 86_400_000.0).floor() as i64;
            let (y, m, d) = civil_from_days(days);
            format!("{}-{m:02}-{d:02}T{}", fmt_year(y), fmt_time(n - days as f64 * 86_400_000.0))
        }
        _ => vm.to_string(&Value::Number(n))?.to_rust(),
    })
}

fn valid_email(s: &str) -> bool {
    let Some((l, d)) = s.split_once('@') else { return false };
    if l.is_empty() || d.is_empty() {
        return false;
    }
    let lok = l.chars().all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c));
    let dok = d.split('.').all(|lab| {
        !lab.is_empty() && lab.len() <= 63 && !lab.starts_with('-') && !lab.ends_with('-') && lab.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    lok && dok
}

/// A CSS `<color>` (css_core) serialized as a valid lowercase simple color (alpha dropped).
fn css_color_hex(v: &str) -> Option<String> {
    let cv = css_core::parser::parse_component_values(trim_ws(v));
    match css_core::color::parse_color(&cv)? {
        css_core::color::Color::Rgba(c) => {
            let b = |x: f64| x.round().clamp(0.0, 255.0) as u8;
            Some(format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b)))
        }
        _ => None,
    }
}

fn is_simple_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|c| c.is_ascii_hexdigit())
}

// ------------------------------------------------------------------------------------------------ min / max / step

fn min_of(el: usize, ty: &str) -> Option<f64> {
    attr_value(el, "min").and_then(|v| to_number(ty, &v)).or(if ty == "range" { Some(0.0) } else { None })
}

fn max_of(el: usize, ty: &str) -> Option<f64> {
    let m = attr_value(el, "max").and_then(|v| to_number(ty, &v));
    if ty == "range" {
        let max = m.unwrap_or(100.0);
        let min = min_of(el, ty).unwrap_or(0.0);
        // A range whose maximum is below its minimum takes the minimum as its maximum.
        return Some(if max < min { min } else { max });
    }
    m
}

/// (default step, step scale, default step base).
fn step_params(ty: &str) -> Option<(f64, f64, f64)> {
    Some(match ty {
        "date" => (1.0, 86_400_000.0, 0.0),
        "month" => (1.0, 1.0, 0.0),
        "week" => (1.0, 604_800_000.0, -259_200_000.0),
        "time" | "datetime-local" => (60.0, 1000.0, 0.0),
        "number" | "range" => (1.0, 1.0, 0.0),
        _ => return None,
    })
}

/// The allowed value step (None: "any", or a type without steps).
fn allowed_step(el: usize, ty: &str) -> Option<f64> {
    let (def, scale, _) = step_params(ty)?;
    match attr_value(el, "step") {
        Some(v) if v.eq_ignore_ascii_case("any") => None,
        Some(v) => match crate::js::reflect::parse_float(&v).filter(|x| *x > 0.0 && parse_valid_float(trim_ws(&v)).is_some()) {
            Some(x) => {
                let x = if matches!(ty, "month" | "week" | "date") { x.round().max(1.0) } else { x };
                Some(x * scale)
            }
            None => Some(def * scale),
        },
        None => Some(def * scale),
    }
}

fn step_base(el: usize, ty: &str) -> f64 {
    if let Some(m) = attr_value(el, "min").and_then(|v| to_number(ty, &v)) {
        return m;
    }
    if let Some(v) = attr_value(el, "value").and_then(|v| to_number(ty, &v)) {
        return v;
    }
    step_params(ty).map(|p| p.2).unwrap_or(0.0)
}

fn aligned(v: f64, base: f64, step: f64) -> bool {
    let q = (v - base) / step;
    (q - q.round()).abs() < 1e-9 * q.abs().max(1.0)
}

// ------------------------------------------------------------------------------------------------ value

/// The value sanitization algorithm for type `ty` (§4.10.5.1.*).
fn sanitize(el: usize, ty: &str, v: &str) -> String {
    match ty {
        "text" | "search" | "tel" | "password" => strip_newlines(v),
        "url" => trim_ws(&strip_newlines(v)).to_string(),
        "email" => {
            let v = strip_newlines(v);
            if attr_value(el, "multiple").is_some() {
                v.split(',').map(trim_ws).collect::<Vec<_>>().join(",")
            } else {
                trim_ws(&v).to_string()
            }
        }
        "number" => {
            if parse_valid_float(v).is_some() {
                v.to_string()
            } else {
                String::new()
            }
        }
        "range" => sanitize_range(el, v),
        "color" => {
            if is_simple_color(v) {
                v.to_ascii_lowercase()
            } else {
                css_color_hex(v).unwrap_or_else(|| "#000000".into())
            }
        }
        "date" => if parse_date(v).is_some() { v.to_string() } else { String::new() },
        "month" => if parse_ym(v).is_some() { v.to_string() } else { String::new() },
        "week" => if parse_week(v).is_some() { v.to_string() } else { String::new() },
        "time" => if parse_time(v).is_some() { v.to_string() } else { String::new() },
        "datetime-local" => match parse_local_dt(v) {
            Some(n) => {
                let days = (n / 86_400_000.0).floor() as i64;
                let (y, m, d) = civil_from_days(days);
                format!("{}-{m:02}-{d:02}T{}", fmt_year(y), fmt_time(n - days as f64 * 86_400_000.0))
            }
            None => String::new(),
        },
        _ => v.to_string(),
    }
}

fn js_num_string(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n}");
        s
    }
}

fn sanitize_range(el: usize, v: &str) -> String {
    let min = min_of(el, "range").unwrap_or(0.0);
    let max = max_of(el, "range").unwrap_or(100.0);
    let mut n = match parse_valid_float(v) {
        Some(n) => n,
        None => {
            if max < min {
                min
            } else {
                min + (max - min) / 2.0
            }
        }
    };
    n = n.clamp(min, max.max(min));
    if let Some(step) = allowed_step(el, "range") {
        let base = step_base(el, "range");
        if !aligned(n, base, step) {
            let k = ((n - base) / step).round();
            let mut c = base + k * step;
            if c > max {
                c -= step;
            }
            if c < min {
                c += step;
            }
            n = c;
        }
    }
    js_num_string(n)
}

fn is_dirty(el: usize) -> bool {
    page(|p| p.values.contains_key(&el))
}

/// The element's value (input in value mode, textarea raw value with newlines normalized for the API).
pub fn value_of(el: usize) -> String {
    match local(el).as_str() {
        "input" => {
            let ty = input_type(el);
            match mode(ty) {
                Mode::Default => attr_value(el, "value").unwrap_or_default(),
                Mode::DefaultOn => attr_value(el, "value").unwrap_or_else(|| "on".into()),
                Mode::Filename => String::new(),
                Mode::Value => match page(|p| p.values.get(&el).cloned()) {
                    Some(v) => v,
                    None => sanitize(el, ty, &attr_value(el, "value").unwrap_or_default()),
                },
            }
        }
        "textarea" => {
            let raw = page(|p| p.values.get(&el).cloned()).unwrap_or_else(|| with_doc(|d| dom::text_content_of(d, el)));
            raw.replace("\r\n", "\n").replace('\r', "\n")
        }
        _ => String::new(),
    }
}

/// Sets the dirty value without the setter's cursor move (setRangeText, stepUp).
fn store_value(el: usize, v: String) {
    page(|p| p.values.insert(el, v));
    super::touch();
}

/// The `value` setter for input (value mode) and textarea: dirty, and the cursor moves to the end when
/// the value changed.
fn set_value(el: usize, v: &str) {
    let old = value_of(el);
    let new = if local(el) == "input" { sanitize(el, input_type(el), v) } else { v.replace("\r\n", "\n").replace('\r', "\n") };
    store_value(el, new.clone());
    if old != new {
        let l = utf16_len(&new);
        SELECTION.with(|s| s.borrow_mut().insert(el, Sel { start: l, end: l, dir: 0 }));
    }
}

fn input_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if input_type(el) == "file" {
        return Ok(s(""));
    }
    Ok(s(&value_of(el)))
}

fn input_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    match mode(input_type(el)) {
        Mode::Default | Mode::DefaultOn => set_attr(vm, el, "value", &v),
        Mode::Filename => {
            if !v.is_empty() {
                return throw_dom(vm, "InvalidStateError", "This input element accepts a filename, which may only be programmatically set to the empty string.");
            }
        }
        Mode::Value => set_value(el, &v),
    }
    Ok(Value::Undefined)
}

fn textarea_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&value_of(el)))
}

fn textarea_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    set_value(el, &v);
    Ok(Value::Undefined)
}

fn text_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(utf16_len(&value_of(el)) as f64))
}

fn input_type_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(input_type(el)))
}

fn input_type_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "type", &v);
    Ok(Value::Undefined)
}

/// The `type` attribute's change steps (§4.10.5): value-mode transitions, re-sanitization, and the
/// selection reset when the selection APIs start to apply. Called by `dom::attribute_changed`.
pub fn attribute_changed(el: usize, local_name: &str, old: &Option<String>, new: &Option<String>) {
    if local_name == "type" && local(el) == "input" {
        let (from, to) = (type_from(old.as_deref()), type_from(new.as_deref()));
        if from == to {
            return;
        }
        let was_selectable = matches!(from, "text" | "search" | "url" | "tel" | "password");
        let (fm, tm) = (mode(from), mode(to));
        if fm == Mode::Value && matches!(tm, Mode::Default | Mode::DefaultOn) {
            let v = page(|p| p.values.get(&el).cloned());
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                // Set the value content attribute (no script runs here: write it directly).
                dom::with_doc_mut(|d| {
                    if let Some(e) = d.element_mut(NodeId(el)) {
                        match e.attrs.iter_mut().find(|a| a.ns == Namespace::None && a.local == "value") {
                            Some(a) => a.value = v,
                            None => e.attrs.push(html_core::Attribute { prefix: None, ns: Namespace::None, local: "value".into(), value: v }),
                        }
                    }
                });
            }
            page(|p| p.values.remove(&el));
        } else if fm != Mode::Value && tm == Mode::Value {
            page(|p| p.values.remove(&el));
        } else if fm != Mode::Filename && tm == Mode::Filename {
            page(|p| p.values.remove(&el));
        } else if tm == Mode::Value {
            if let Some(v) = page(|p| p.values.get(&el).cloned()) {
                let sv = sanitize(el, to, &v);
                page(|p| p.values.insert(el, sv));
            }
        }
        let now_selectable = matches!(to, "text" | "search" | "url" | "tel" | "password");
        if !was_selectable && now_selectable {
            SELECTION.with(|s| s.borrow_mut().insert(el, Sel::default()));
        }
    }
    if local_name == "value" && local(el) == "input" && !is_dirty(el) && mode(input_type(el)) == Mode::Value {
        // The default value changed under a clean control: the value follows (lazily), the cursor
        // clamps on read.
    }
}

/// The HTML cloning steps for form controls: value, dirty flags and checkedness copy over.
pub fn cloned(src: usize, copy: usize) {
    page(|p| {
        if let Some(v) = p.values.get(&src).cloned() {
            p.values.insert(copy, v);
        }
        if let Some(c) = p.checked.get(&src).copied() {
            p.checked.insert(copy, c);
        }
    });
    if let Some(sel) = SELECTION.with(|s| s.borrow().get(&src).copied()) {
        SELECTION.with(|s| s.borrow_mut().insert(copy, sel));
    }
    if INDETERMINATE.with(|s| s.borrow().contains(&src)) {
        INDETERMINATE.with(|s| s.borrow_mut().insert(copy));
    }
}

pub fn set_indeterminate(el: usize, v: bool) {
    INDETERMINATE.with(|s| {
        if v {
            s.borrow_mut().insert(el);
        } else {
            s.borrow_mut().remove(&el);
        }
    });
}

fn indeterminate_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(Value::Bool(INDETERMINATE.with(|s| s.borrow().contains(&el))))
}

fn indeterminate_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    set_indeterminate(el, b);
    Ok(Value::Undefined)
}

/// The checked radio of `el`'s group, if any.
pub fn checked_radio_in_group(el: usize) -> Option<usize> {
    radio_group(el).into_iter().find(|r| checked(*r))
}

fn files_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if input_type(el) != "file" {
        return Ok(Value::Null);
    }
    Ok(dom::cached(vm, el, SO_FILES, |vm| {
        let p = dom::proto("FileList");
        host_obj(vm, p, T_FILELIST, vec![num(el as f64)])
    }))
}

fn files_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    // Setting null is ignored; a FileList from a DataTransfer would replace the selection (no picker here).
    Ok(Value::Undefined)
}

fn filelist_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if this_tagged(vm, &ctx.this, T_FILELIST).is_none() {
        return vm.throw_type("Illegal invocation");
    }
    Ok(num(0.0))
}

fn filelist_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if this_tagged(vm, &ctx.this, T_FILELIST).is_none() {
        return vm.throw_type("Illegal invocation");
    }
    Ok(Value::Null)
}

// ------------------------------------------------------------------------------------------------ valueAsNumber / valueAsDate / stepUp

fn numeric_type(ty: &str) -> bool {
    matches!(ty, "date" | "month" | "week" | "time" | "datetime-local" | "number" | "range")
}

fn value_as_number_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let ty = input_type(el);
    if !numeric_type(ty) {
        return Ok(num(f64::NAN));
    }
    Ok(num(to_number(ty, &value_of(el)).unwrap_or(f64::NAN)))
}

fn value_as_number_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let n = vm.to_number(&arg(vm, ctx, 0))?;
    if n.is_infinite() {
        return vm.throw_type("The value provided is infinite.");
    }
    let ty = input_type(el);
    if !numeric_type(ty) {
        return throw_dom(vm, "InvalidStateError", "This input element does not support Number values.");
    }
    if n.is_nan() {
        set_value(el, "");
    } else {
        let t = from_number(vm, ty, n)?;
        set_value(el, &t);
    }
    Ok(Value::Undefined)
}

fn date_type(ty: &str) -> bool {
    matches!(ty, "date" | "month" | "week" | "time")
}

fn value_as_date_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let ty = input_type(el);
    if !date_type(ty) {
        return Ok(Value::Null);
    }
    let n = match ty {
        "month" => parse_ym(&value_of(el)).map(|(y, m)| days_from_civil(y, m, 1) as f64 * 86_400_000.0),
        _ => to_number(ty, &value_of(el)),
    };
    match n {
        Some(ms) => {
            let date = super::idl::get(vm, vm.realm().global, "Date")?;
            vm.construct(&date, &[num(ms)], None)
        }
        None => Ok(Value::Null),
    }
}

fn value_as_date_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let ty = input_type(el);
    if !date_type(ty) {
        return throw_dom(vm, "InvalidStateError", "This input element does not support Date values.");
    }
    let v = arg(vm, ctx, 0);
    if v.is_null() {
        set_value(el, "");
        return Ok(Value::Undefined);
    }
    let is_date = matches!(&v, Value::Object(o) if matches!(vm.heap.get(*o).kind, Kind::Date(_)));
    if !is_date {
        return vm.throw_type("The provided value is not a Date.");
    }
    let Value::Object(o) = v else { unreachable!() };
    let ms = match vm.heap.get(o).kind {
        Kind::Date(t) => t,
        _ => f64::NAN,
    };
    if ms.is_nan() {
        set_value(el, "");
        return Ok(Value::Undefined);
    }
    let n = if ty == "month" {
        let (y, m, _) = civil_from_days((ms / 86_400_000.0).floor() as i64);
        ((y - 1970) * 12 + m as i64 - 1) as f64
    } else {
        ms
    };
    let t = from_number(vm, ty, n)?;
    set_value(el, &t);
    Ok(Value::Undefined)
}

fn step_impl(vm: &mut Vm, ctx: &CallCtx, down: bool) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let ty = input_type(el);
    let a = arg(vm, ctx, 0);
    let n = if a.is_undefined() { 1 } else { vm.to_int32(&a)? } as f64;
    if !numeric_type(ty) {
        return throw_dom(vm, "InvalidStateError", "This form element is not steppable.");
    }
    let Some(step) = allowed_step(el, ty) else {
        return throw_dom(vm, "InvalidStateError", "This form element does not have an allowed value step.");
    };
    let (min, max) = (min_of(el, ty), max_of(el, ty));
    if let (Some(mi), Some(ma)) = (min, max) {
        if mi > ma {
            return Ok(Value::Undefined);
        }
    }
    let base = step_base(el, ty);
    if let (Some(mi), Some(ma)) = (min, max) {
        let first = base + ((mi - base) / step).ceil() * step;
        if first > ma + 1e-9 {
            return Ok(Value::Undefined);
        }
    }
    let mut value = to_number(ty, &value_of(el)).unwrap_or(0.0);
    let before = value;
    if !aligned(value, base, step) {
        let k = (value - base) / step;
        value = base + if down { k.floor() } else { k.ceil() } * step;
    } else {
        let delta = step * n;
        value += if down { -delta } else { delta };
    }
    if let Some(mi) = min {
        if value < mi {
            value = base + ((mi - base) / step).ceil() * step;
        }
    }
    if let Some(ma) = max {
        if value > ma {
            value = base + ((ma - base) / step).floor() * step;
        }
    }
    if (down && value > before) || (!down && value < before) {
        return Ok(Value::Undefined);
    }
    // Round away floating noise against the step's precision.
    let value = (value * 1e9).round() / 1e9;
    let t = from_number(vm, ty, value)?;
    set_value(el, &t);
    Ok(Value::Undefined)
}

fn step_up(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    step_impl(vm, ctx, false)
}

fn step_down(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    step_impl(vm, ctx, true)
}

// ------------------------------------------------------------------------------------------------ selection

fn selection_of(el: usize) -> Sel {
    let len = utf16_len(&value_of(el));
    let mut s = SELECTION.with(|s| s.borrow().get(&el).copied()).unwrap_or_default();
    s.start = s.start.min(len);
    s.end = s.end.min(len);
    s
}

fn dir_name(d: u8) -> &'static str {
    match d {
        1 => "forward",
        2 => "backward",
        _ => "none",
    }
}

fn dir_from(s: &str) -> u8 {
    match s {
        "forward" => 1,
        "backward" => 2,
        _ => 0,
    }
}

/// "set the selection range"; a change queues a bubbling `select` event (§4.10.18.8).
fn set_selection(el: usize, start: u32, end: u32, dir: u8) {
    let len = utf16_len(&value_of(el));
    let end = end.min(len);
    let start = start.min(end);
    let old = selection_of(el);
    SELECTION.with(|s| s.borrow_mut().insert(el, Sel { start, end, dir }));
    if (old.start, old.end, old.dir) != (start, end, dir) {
        queue_select(el);
    }
}

fn queue_select(el: usize) {
    page(|p| p.tasks.push_back(super::Task::Event(super::TargetKey::Node(el), "select".into(), true)));
}

fn not_selectable<T>(vm: &mut Vm) -> JsResult<T> {
    throw_dom(vm, "InvalidStateError", "The input element's type does not support selection.")
}

fn selection_start_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if !selectable(el) {
        return Ok(Value::Null);
    }
    Ok(num(selection_of(el).start as f64))
}

fn selection_end_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if !selectable(el) {
        return Ok(Value::Null);
    }
    Ok(num(selection_of(el).end as f64))
}

fn selection_direction_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if !selectable(el) {
        return Ok(Value::Null);
    }
    Ok(s(dir_name(selection_of(el).dir)))
}

fn opt_ulong(vm: &mut Vm, v: &Value) -> JsResult<u32> {
    if v.is_null() {
        return Ok(0);
    }
    vm.to_uint32(v)
}

fn selection_start_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = opt_ulong(vm, &arg(vm, ctx, 0))?;
    if !selectable(el) {
        return not_selectable(vm);
    }
    let cur = selection_of(el);
    let end = cur.end.max(v);
    set_selection(el, v, end, cur.dir);
    Ok(Value::Undefined)
}

fn selection_end_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = opt_ulong(vm, &arg(vm, ctx, 0))?;
    if !selectable(el) {
        return not_selectable(vm);
    }
    let cur = selection_of(el);
    set_selection(el, cur.start, v, cur.dir);
    Ok(Value::Undefined)
}

fn selection_direction_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let d = if v.is_null() { 0 } else { dir_from(&string(vm, &v)?) };
    if !selectable(el) {
        return not_selectable(vm);
    }
    let cur = selection_of(el);
    set_selection(el, cur.start, cur.end, d);
    Ok(Value::Undefined)
}

fn set_selection_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "HTMLInputElement", "setSelectionRange")?;
    let a = vm.to_uint32(&arg(vm, ctx, 0))?;
    let b = vm.to_uint32(&arg(vm, ctx, 1))?;
    let dv = arg(vm, ctx, 2);
    let d = if dv.is_undefined() { 0 } else { dir_from(&string(vm, &dv)?) };
    if !selectable(el) {
        return not_selectable(vm);
    }
    set_selection(el, a, b, d);
    Ok(Value::Undefined)
}

fn select_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if selectable(el) {
        let len = utf16_len(&value_of(el));
        let before = selection_of(el);
        set_selection(el, 0, len, 0);
        if (before.start, before.end, before.dir) == (0, len, 0) {
            queue_select(el);
        }
    }
    Ok(Value::Undefined)
}

fn set_range_text(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "HTMLInputElement", "setRangeText")?;
    if !selectable(el) {
        return not_selectable(vm);
    }
    let repl = string(vm, &arg(vm, ctx, 0))?;
    let cur = selection_of(el);
    let (mut start, mut end) = (cur.start, cur.end);
    let mut mode_s = String::from("preserve");
    if ctx.argc >= 2 {
        if ctx.argc < 3 {
            return vm.throw_type("Failed to execute 'setRangeText': 3 arguments required.");
        }
        start = vm.to_uint32(&arg(vm, ctx, 1))?;
        end = vm.to_uint32(&arg(vm, ctx, 2))?;
        let m = arg(vm, ctx, 3);
        if !m.is_undefined() {
            mode_s = string(vm, &m)?;
            if !matches!(mode_s.as_str(), "select" | "start" | "end" | "preserve") {
                return vm.throw_type(&format!("The provided value '{mode_s}' is not a valid enum value of type SelectionMode."));
            }
        }
    }
    if start > end {
        return throw_dom(vm, "IndexSizeError", "The provided start value is larger than the end value.");
    }
    let v: Vec<u16> = value_of(el).encode_utf16().collect();
    let len = v.len() as u32;
    let end = end.min(len);
    let start = start.min(end);
    let (mut ss, mut se) = (cur.start, cur.end);
    let r16: Vec<u16> = repl.encode_utf16().collect();
    let mut out = v[..start as usize].to_vec();
    out.extend_from_slice(&r16);
    out.extend_from_slice(&v[end as usize..]);
    store_value(el, String::from_utf16_lossy(&out));
    let new_len = r16.len() as u32;
    let new_end = start + new_len;
    match mode_s.as_str() {
        "select" => {
            ss = start;
            se = new_end;
        }
        "start" => {
            ss = start;
            se = start;
        }
        "end" => {
            ss = new_end;
            se = new_end;
        }
        _ => {
            let old_len = end - start;
            let delta = new_len as i64 - old_len as i64;
            if ss > end {
                ss = (ss as i64 + delta) as u32;
            } else if ss > start {
                ss = start;
            }
            if se > end {
                se = (se as i64 + delta) as u32;
            } else if se > start {
                se = new_end;
            }
        }
    }
    let before = selection_of(el);
    set_selection(el, ss, se, 0);
    let after = selection_of(el);
    if (before.start, before.end, before.dir) == (after.start, after.end, after.dir) {
        queue_select(el);
    }
    Ok(Value::Undefined)
}

// ------------------------------------------------------------------------------------------------ form owner

const LISTED: &[&str] = &["button", "fieldset", "input", "object", "output", "select", "textarea"];

fn is_listed(d: &Document, el: usize) -> bool {
    html_local(d, el).is_some_and(|l| LISTED.contains(&l.as_str()))
}

fn root_in(d: &Document, n: usize) -> usize {
    let mut cur = NodeId(n);
    while let Some(p) = d.parent(cur) {
        cur = p;
    }
    cur.0
}

/// The form owner (§4.10.17.3): the `form` attribute's form in the same tree, else the nearest
/// ancestor form.
pub fn form_owner_in(d: &Document, el: usize) -> Option<usize> {
    let e = d.element(NodeId(el))?;
    if let Some(id) = e.attr("form") {
        let id = id.to_string();
        if id.is_empty() {
            return None;
        }
        let root = root_in(d, el);
        let hit = std::iter::once(NodeId(root)).chain(d.descendants(NodeId(root))).find(|n| d.element(*n).is_some_and(|x| x.attr("id") == Some(id.as_str())));
        return hit.filter(|n| html_local(d, n.0).as_deref() == Some("form")).map(|n| n.0);
    }
    let mut cur = d.parent(NodeId(el));
    while let Some(c) = cur {
        if html_local(d, c.0).as_deref() == Some("form") {
            return Some(c.0);
        }
        cur = d.parent(c);
    }
    None
}

pub fn form_owner(el: usize) -> Option<usize> {
    with_doc(|d| form_owner_in(d, el))
}

/// `form.elements` (§4.10.3): listed elements owned by `form`, in tree order, minus image buttons.
pub fn form_elements_in(d: &Document, form: usize) -> Vec<usize> {
    let root = root_in(d, form);
    d.descendants(NodeId(root))
        .map(|n| n.0)
        .filter(|&n| is_listed(d, n) && form_owner_in(d, n) == Some(form))
        .filter(|&n| !(html_local(d, n).as_deref() == Some("input") && d.element(NodeId(n)).and_then(|e| e.attr("type")).is_some_and(|t| t.eq_ignore_ascii_case("image"))))
        .collect()
}

fn form_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let f = form_owner(el);
    Ok(wrap_opt(vm, f))
}

fn form_elements(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, SO_ELEMENTS, |vm| dom::make_collection(vm, dom::CK_DOCFILTER, el, "formelements", "")))
}

fn form_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(with_doc(|d| form_elements_in(d, el)).len() as f64))
}

/// Labelable elements (§4.10.2).
fn labelable(d: &Document, el: usize) -> bool {
    match html_local(d, el).as_deref() {
        Some("button" | "meter" | "output" | "progress" | "select" | "textarea") => true,
        Some("input") => !d.element(NodeId(el)).and_then(|e| e.attr("type")).is_some_and(|t| t.eq_ignore_ascii_case("hidden")),
        _ => false,
    }
}

/// A label's labeled control (§4.10.4).
pub fn label_control_in(d: &Document, label: usize) -> Option<usize> {
    let e = d.element(NodeId(label))?;
    if let Some(f) = e.attr("for") {
        let f = f.to_string();
        let root = root_in(d, label);
        let hit = d.descendants(NodeId(root)).find(|n| d.element(*n).is_some_and(|x| x.attr("id") == Some(f.as_str())))?;
        return labelable(d, hit.0).then_some(hit.0);
    }
    d.descendants(NodeId(label)).map(|n| n.0).find(|&n| labelable(d, n))
}

/// `control.labels`: the label elements whose labeled control is `el`, in tree order.
pub fn labels_in(d: &Document, el: usize) -> Vec<usize> {
    if !labelable(d, el) {
        return Vec::new();
    }
    let root = root_in(d, el);
    d.descendants(NodeId(root)).map(|n| n.0).filter(|&n| html_local(d, n).as_deref() == Some("label") && label_control_in(d, n) == Some(el)).collect()
}

fn labels_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if !with_doc(|d| labelable(d, el)) {
        return Ok(Value::Null);
    }
    Ok(dom::cached(vm, el, SO_LABELS, |vm| dom::make_collection(vm, dom::CK_DOCFILTER, el, "labels", "")))
}

fn label_control(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let c = with_doc(|d| label_control_in(d, el));
    Ok(wrap_opt(vm, c))
}

fn label_form(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let f = with_doc(|d| label_control_in(d, el).filter(|c| is_listed(d, *c)).and_then(|c| form_owner_in(d, c)));
    Ok(wrap_opt(vm, f))
}

fn legend_form(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let f = with_doc(|d| {
        let p = d.parent(NodeId(el))?;
        (html_local(d, p.0).as_deref() == Some("fieldset")).then(|| form_owner_in(d, p.0)).flatten()
    });
    Ok(wrap_opt(vm, f))
}

// ------------------------------------------------------------------------------------------------ constraint validation

fn has(el: usize, a: &str) -> bool {
    attr_value(el, a).is_some()
}

/// Disabled (§4.10.18.5): the attribute, or a disabled fieldset ancestor outside its first legend.
pub fn disabled(el: usize) -> bool {
    let l = local(el);
    if matches!(l.as_str(), "button" | "input" | "select" | "textarea" | "fieldset" | "optgroup") && has(el, "disabled") {
        return true;
    }
    if l == "option" {
        return has(el, "disabled") || with_doc(|d| d.parent(NodeId(el)).is_some_and(|p| html_local(d, p.0).as_deref() == Some("optgroup") && d.element(p).is_some_and(|e| e.attr("disabled").is_some())));
    }
    with_doc(|d| {
        let mut child = NodeId(el);
        let mut cur = d.parent(child);
        while let Some(p) = cur {
            if html_local(d, p.0).as_deref() == Some("fieldset") && d.element(p).is_some_and(|e| e.attr("disabled").is_some()) {
                let first_legend = d.children(p).find(|c| html_local(d, c.0).as_deref() == Some("legend"));
                if first_legend != Some(child) {
                    return true;
                }
            }
            child = p;
            cur = d.parent(p);
        }
        false
    })
}

fn readonly_applies(ty: &str) -> bool {
    matches!(ty, "text" | "search" | "url" | "tel" | "email" | "password" | "date" | "month" | "week" | "time" | "datetime-local" | "number")
}

fn in_datalist(el: usize) -> bool {
    with_doc(|d| {
        let mut cur = d.parent(NodeId(el));
        while let Some(p) = cur {
            if html_local(d, p.0).as_deref() == Some("datalist") {
                return true;
            }
            cur = d.parent(p);
        }
        false
    })
}

/// Barred from constraint validation.
pub fn barred(el: usize) -> bool {
    let l = local(el);
    match l.as_str() {
        "input" => {
            let ty = input_type(el);
            matches!(ty, "hidden" | "reset" | "button") || has(el, "readonly") || disabled(el) || in_datalist(el)
        }
        "button" => {
            let t = attr_value(el, "type").unwrap_or_default().to_ascii_lowercase();
            matches!(t.as_str(), "reset" | "button") || disabled(el) || in_datalist(el)
        }
        "select" => disabled(el) || in_datalist(el),
        "textarea" => disabled(el) || has(el, "readonly") || in_datalist(el),
        _ => true,
    }
}

fn mutable(el: usize) -> bool {
    !disabled(el) && !(has(el, "readonly") && (local(el) == "textarea" || readonly_applies(input_type(el))))
}

fn checked(el: usize) -> bool {
    page(|p| p.checked.get(&el).copied()).unwrap_or_else(|| has(el, "checked"))
}

fn radio_group(el: usize) -> Vec<usize> {
    let Some(name) = attr_value(el, "name").filter(|n| !n.is_empty()) else { return vec![el] };
    let owner = form_owner(el);
    with_doc(|d| {
        let root = root_in(d, el);
        d.descendants(NodeId(root))
            .map(|n| n.0)
            .filter(|&n| {
                html_local(d, n).as_deref() == Some("input")
                    && d.element(NodeId(n)).is_some_and(|e| e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("radio")) && e.attr("name") == Some(name.as_str()))
                    && form_owner_in(d, n) == owner
            })
            .collect()
    })
}

fn option_selected(o: usize) -> bool {
    page(|p| p.checked.get(&o).copied()).unwrap_or_else(|| has(o, "selected"))
}

fn select_options(sel: usize) -> Vec<usize> {
    with_doc(|d| {
        let mut out = Vec::new();
        for c in d.children(NodeId(sel)) {
            match html_local(d, c.0).as_deref() {
                Some("option") => out.push(c.0),
                Some("optgroup") => out.extend(d.children(c).filter(|g| html_local(d, g.0).as_deref() == Some("option")).map(|g| g.0)),
                _ => {}
            }
        }
        out
    })
}

fn display_size(sel: usize) -> u32 {
    attr_value(sel, "size").and_then(|v| crate::js::reflect::parse_int(&v)).filter(|n| *n > 0).map(|n| n as u32).unwrap_or(if has(sel, "multiple") { 4 } else { 1 })
}

fn select_value_missing(sel: usize) -> bool {
    if !has(sel, "required") {
        return false;
    }
    let opts = select_options(sel);
    let multiple = has(sel, "multiple");
    let selected: Vec<usize> = if multiple {
        opts.iter().copied().filter(|o| option_selected(*o)).collect()
    } else {
        // Single select: the last selected, else the first non-disabled option (selectedness setting).
        match opts.iter().rev().find(|o| option_selected(**o)) {
            Some(o) => vec![*o],
            None => opts.iter().copied().find(|o| !disabled(*o)).into_iter().collect(),
        }
    };
    if selected.is_empty() {
        return true;
    }
    // The placeholder label option: first option, value "", parent is the select, single + size 1.
    if !multiple && display_size(sel) == 1 && selected.len() == 1 {
        let o = selected[0];
        let first = opts.first().copied() == Some(o);
        let val = attr_value(o, "value").unwrap_or_else(|| with_doc(|d| dom::text_content_of(d, o)));
        let parent_is_select = dom::parent_of(o) == Some(sel);
        if first && val.is_empty() && parent_is_select {
            return true;
        }
    }
    false
}

#[derive(Default, Clone, Copy)]
struct Flags {
    value_missing: bool,
    type_mismatch: bool,
    pattern_mismatch: bool,
    range_underflow: bool,
    range_overflow: bool,
    step_mismatch: bool,
    custom: bool,
}

impl Flags {
    fn valid(&self) -> bool {
        !(self.value_missing || self.type_mismatch || self.pattern_mismatch || self.range_underflow || self.range_overflow || self.step_mismatch || self.custom)
    }
}

fn pattern_matches(vm: &mut Vm, pattern: &str, value: &str) -> bool {
    let re = match super::idl::get(vm, vm.realm().global, "RegExp") {
        Ok(r) => r,
        Err(_) => return true,
    };
    if vm.construct(&re, &[s(pattern), s("v")], None).is_err() {
        return true;
    }
    let src = format!("^(?:{pattern})$");
    let Ok(rx) = vm.construct(&re, &[s(&src), s("v")], None) else {
        // An invalid pattern imposes no constraint.
        return true;
    };
    let Value::Object(rxo) = rx else { return true };
    let Ok(test) = super::idl::get(vm, rxo, "test") else { return true };
    match vm.call(&test, &rx, &[s(value)]) {
        Ok(v) => vm.to_boolean(&v),
        Err(_) => true,
    }
}

fn flags(vm: &mut Vm, el: usize) -> Flags {
    let mut f = Flags { custom: CUSTOM.with(|c| c.borrow().get(&el).is_some_and(|m| !m.is_empty())), ..Flags::default() };
    match local(el).as_str() {
        "input" => {
            let ty = input_type(el);
            let v = value_of(el);
            match ty {
                "checkbox" => f.value_missing = has(el, "required") && !checked(el),
                "radio" => {
                    let g = radio_group(el);
                    f.value_missing = g.iter().any(|r| has(*r, "required")) && g.iter().all(|r| !checked(*r));
                }
                "file" => f.value_missing = has(el, "required"),
                "hidden" | "submit" | "image" | "reset" | "button" | "color" | "range" => {}
                _ => f.value_missing = has(el, "required") && mutable(el) && v.is_empty(),
            }
            if !v.is_empty() {
                let multiple = ty == "email" && has(el, "multiple");
                if ty == "email" {
                    f.type_mismatch = if multiple { v.split(',').any(|x| !valid_email(x)) } else { !valid_email(&v) };
                }
                if ty == "url" {
                    f.type_mismatch = url::Url::parse(&v).is_err();
                }
                if matches!(ty, "text" | "search" | "url" | "tel" | "email" | "password") {
                    if let Some(p) = attr_value(el, "pattern") {
                        f.pattern_mismatch = if multiple { v.split(',').any(|x| !pattern_matches(vm, &p, x)) } else { !pattern_matches(vm, &p, &v) };
                    }
                }
                if numeric_type(ty) {
                    if let Some(n) = to_number(ty, &v) {
                        let (min, max) = (attr_value(el, "min").and_then(|x| to_number(ty, &x)), attr_value(el, "max").and_then(|x| to_number(ty, &x)));
                        if ty == "time" && matches!((min, max), (Some(a), Some(b)) if a > b) {
                            let (a, b) = (min.unwrap(), max.unwrap());
                            let out = n < a && n > b;
                            f.range_underflow = out;
                            f.range_overflow = out;
                        } else {
                            f.range_underflow = min.is_some_and(|m| n < m);
                            f.range_overflow = max.is_some_and(|m| n > m);
                        }
                        if let Some(step) = allowed_step(el, ty) {
                            f.step_mismatch = !aligned(n, step_base(el, ty), step);
                        }
                    }
                }
            }
        }
        "textarea" => f.value_missing = has(el, "required") && mutable(el) && value_of(el).is_empty(),
        "select" => f.value_missing = select_value_missing(el),
        _ => {}
    }
    f
}

fn this_validity(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    match this_tagged(vm, &ctx.this, T_VALIDITY) {
        Some(o) => Ok(slot_num(vm, o, 0) as usize),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn validity_flag(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_validity(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let f = flags(vm, el);
    Ok(Value::Bool(match which.as_str() {
        "valueMissing" => f.value_missing,
        "typeMismatch" => f.type_mismatch,
        "patternMismatch" => f.pattern_mismatch,
        "rangeUnderflow" => f.range_underflow,
        "rangeOverflow" => f.range_overflow,
        "stepMismatch" => f.step_mismatch,
        "customError" => f.custom,
        "valid" => f.valid(),
        _ => false,
    }))
}

fn validity_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, SO_VALIDITY, |vm| {
        let p = dom::proto("ValidityState");
        host_obj(vm, p, T_VALIDITY, vec![num(el as f64)])
    }))
}

fn will_validate(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(Value::Bool(!barred(el)))
}

fn validation_message(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if barred(el) {
        return Ok(s(""));
    }
    let f = flags(vm, el);
    if f.custom {
        return Ok(s(&CUSTOM.with(|c| c.borrow().get(&el).cloned()).unwrap_or_default()));
    }
    Ok(s(if f.value_missing {
        "Please fill out this field."
    } else if f.type_mismatch {
        "Please enter a valid value."
    } else if f.pattern_mismatch {
        "Please match the requested format."
    } else if f.range_underflow {
        "Value must be greater than or equal to the minimum."
    } else if f.range_overflow {
        "Value must be less than or equal to the maximum."
    } else if f.step_mismatch {
        "Please enter a valid value (step)."
    } else {
        ""
    }))
}

fn set_custom_validity(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "HTMLElement", "setCustomValidity")?;
    let m = string(vm, &arg(vm, ctx, 0))?;
    let m = m.replace("\r\n", "\n").replace('\r', "\n");
    CUSTOM.with(|c| c.borrow_mut().insert(el, m));
    Ok(Value::Undefined)
}

/// "check validity": fires a cancelable `invalid` at an invalid candidate.
fn check_one(vm: &mut Vm, el: usize) -> bool {
    if barred(el) || flags(vm, el).valid() {
        return true;
    }
    super::events::fire_simple_cancelable(vm, super::TargetKey::Node(el), "invalid");
    false
}

fn check_validity(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if local(el) == "form" {
        let els = with_doc(|d| form_elements_in(d, el));
        let mut ok = true;
        for e in els {
            ok &= check_one(vm, e);
        }
        return Ok(Value::Bool(ok));
    }
    if local(el) == "fieldset" {
        return Ok(Value::Bool(true));
    }
    Ok(Value::Bool(check_one(vm, el)))
}

// ------------------------------------------------------------------------------------------------ select / option

/// A select's list of options (§4.10.7): option children, and option children of optgroup children.
pub fn list_of_options_in(d: &Document, sel: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for c in d.children(NodeId(sel)) {
        match html_local(d, c.0).as_deref() {
            Some("option") => out.push(c.0),
            Some("optgroup") => out.extend(d.children(c).filter(|g| html_local(d, g.0).as_deref() == Some("option")).map(|g| g.0)),
            _ => {}
        }
    }
    out
}

fn option_selected_in(d: &Document, o: usize) -> bool {
    page(|p| p.checked.get(&o).copied()).unwrap_or_else(|| d.element(NodeId(o)).is_some_and(|e| e.attr("selected").is_some()))
}

/// `select.selectedOptions`: the options whose selectedness is true (a single select shows the one
/// its selectedness setting picks).
pub fn selected_options_in(d: &Document, sel: usize) -> Vec<usize> {
    let opts = list_of_options_in(d, sel);
    let multiple = d.element(NodeId(sel)).is_some_and(|e| e.attr("multiple").is_some());
    if multiple {
        return opts.into_iter().filter(|o| option_selected_in(d, *o)).collect();
    }
    let size = d.element(NodeId(sel)).and_then(|e| e.attr("size")).and_then(crate::js::reflect::parse_int).filter(|n| *n > 0).unwrap_or(1);
    match opts.iter().rev().find(|o| option_selected_in(d, **o)) {
        Some(o) => vec![*o],
        None if size == 1 => opts.into_iter().find(|o| !d.element(NodeId(*o)).is_some_and(|e| e.attr("disabled").is_some())).into_iter().collect(),
        None => Vec::new(),
    }
}

fn select_of_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    let t = super::idl::this_tagged(vm, &ctx.this, T_COLL);
    match t {
        Some(o) if slot_num(vm, o, 0) as u8 == dom::CK_OPTIONS => Ok(slot_num(vm, o, 1) as usize),
        _ => vm.throw_type("Illegal invocation"),
    }
}

fn this_select(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    if let Ok(s) = select_of_options(vm, ctx) {
        return Ok(s);
    }
    this_element(vm, ctx)
}

fn select_options_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, 35, |vm| dom::make_collection(vm, dom::CK_OPTIONS, el, "", "")))
}

fn selected_options_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, SO_SELECTED, |vm| dom::make_collection(vm, dom::CK_DOCFILTER, el, "selectedoptions", "")))
}

/// HTMLOptionsCollection/HTMLSelectElement `add(element, before)`.
fn options_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let sel = this_select(vm, ctx)?;
    need(vm, ctx, 1, "HTMLOptionsCollection", "add")?;
    let ev = arg(vm, ctx, 0);
    let Some(el) = dom::node_of(vm, &ev).filter(|n| matches!(local(*n).as_str(), "option" | "optgroup")) else {
        return vm.throw_type("Failed to execute 'add': The provided value is not of type '(HTMLOptGroupElement or HTMLOptionElement)'.");
    };
    let bv = arg(vm, ctx, 1);
    let before = if bv.is_nullish() {
        None
    } else if let Some(n) = dom::node_of(vm, &bv) {
        Some(n)
    } else {
        let i = vm.to_int32(&bv)?;
        with_doc(|d| list_of_options_in(d, sel)).get(i.max(0) as usize).copied().filter(|_| i >= 0)
    };
    if dom::with_doc(|d| d.is_inclusive_ancestor(NodeId(el), NodeId(sel))) {
        return throw_dom(vm, "HierarchyRequestError", "The new element contains the select.");
    }
    if let Some(b) = before {
        if !dom::with_doc(|d| d.is_inclusive_ancestor(NodeId(sel), NodeId(b))) {
            return throw_dom(vm, "NotFoundError", "The 'before' element is not a descendant of the select.");
        }
        if b == el {
            return Ok(Value::Undefined);
        }
    }
    let (parent, refn) = match before {
        Some(b) => (dom::parent_of(b).unwrap_or(sel), Some(b)),
        None => (sel, None),
    };
    dom::pre_insert(vm, el, parent, refn)?;
    Ok(Value::Undefined)
}

fn options_remove(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let sel = this_select(vm, ctx)?;
    if ctx.argc == 0 && select_of_options(vm, ctx).is_err() {
        // HTMLSelectElement.remove() with no argument is ChildNode.remove().
        dom::remove(vm, sel);
        return Ok(Value::Undefined);
    }
    let i = vm.to_int32(&arg(vm, ctx, 0))?;
    if i < 0 {
        return Ok(Value::Undefined);
    }
    if let Some(o) = with_doc(|d| list_of_options_in(d, sel)).get(i as usize).copied() {
        dom::remove(vm, o);
    }
    Ok(Value::Undefined)
}

fn options_length_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let sel = this_select(vm, ctx)?;
    let n = vm.to_uint32(&arg(vm, ctx, 0))? as usize;
    let opts = with_doc(|d| list_of_options_in(d, sel));
    if n > opts.len() {
        if n > 100_000 {
            return Ok(Value::Undefined);
        }
        let doc = dom::node_document(sel);
        for _ in opts.len()..n {
            let o = dom::create_element(doc, Namespace::Html, "option");
            dom::insert(vm, o, sel, None);
        }
    } else {
        for o in opts.into_iter().skip(n) {
            dom::remove(vm, o);
        }
    }
    Ok(Value::Undefined)
}

fn options_length_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let sel = this_select(vm, ctx)?;
    Ok(num(with_doc(|d| list_of_options_in(d, sel)).len() as f64))
}

/// The HTMLOptionsCollection indexed setter.
pub fn options_set_index(vm: &mut Vm, sel: usize, i: usize, v: &Value) -> JsResult<()> {
    let opts = with_doc(|d| list_of_options_in(d, sel));
    if v.is_null() {
        if let Some(o) = opts.get(i).copied() {
            dom::remove(vm, o);
        }
        return Ok(());
    }
    let Some(o) = dom::node_of(vm, v).filter(|n| local(*n) == "option") else {
        return vm.throw_type("The provided value is not of type 'HTMLOptionElement'.");
    };
    if i > opts.len() {
        let doc = dom::node_document(sel);
        for _ in opts.len()..i {
            let e = dom::create_element(doc, Namespace::Html, "option");
            dom::insert(vm, e, sel, None);
        }
    }
    match opts.get(i).copied() {
        Some(old) => {
            let p = dom::parent_of(old).unwrap_or(sel);
            dom::replace_child(vm, old, o, p)?;
        }
        None => {
            dom::pre_insert(vm, o, sel, None)?;
        }
    }
    Ok(())
}

fn input_list(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let Some(id) = attr_value(el, "list") else { return Ok(Value::Null) };
    let hit = with_doc(|d| {
        let root = root_in(d, el);
        d.descendants(NodeId(root)).find(|n| d.element(*n).is_some_and(|e| e.attr("id") == Some(id.as_str()))).map(|n| n.0)
    });
    let hit = hit.filter(|h| local(*h) == "datalist");
    Ok(wrap_opt(vm, hit))
}

fn option_index(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let sel = with_doc(|d| {
        let p = d.parent(NodeId(el))?;
        match html_local(d, p.0).as_deref() {
            Some("select") => Some(p.0),
            Some("optgroup") => d.parent(p).filter(|g| html_local(d, g.0).as_deref() == Some("select")).map(|g| g.0),
            _ => None,
        }
    });
    let i = sel.and_then(|s| with_doc(|d| list_of_options_in(d, s)).iter().position(|o| *o == el)).unwrap_or(0);
    Ok(num(i as f64))
}

fn option_text_of(el: usize) -> String {
    let t = with_doc(|d| dom::text_content_of(d, el));
    t.split(is_ws).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

fn option_label_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&attr_value(el, "label").unwrap_or_else(|| option_text_of(el))))
}

fn option_label_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "label", &v);
    Ok(Value::Undefined)
}

fn option_form(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let f = with_doc(|d| {
        let mut cur = d.parent(NodeId(el));
        while let Some(p) = cur {
            if html_local(d, p.0).as_deref() == Some("select") {
                return form_owner_in(d, p.0);
            }
            if html_local(d, p.0).as_deref() == Some("datalist") {
                return None;
            }
            cur = d.parent(p);
        }
        None
    });
    Ok(wrap_opt(vm, f))
}

// ------------------------------------------------------------------------------------------------ meter / progress

fn meter_vals(el: usize) -> (f64, f64, f64, f64, f64, f64) {
    let p = |a: &str| attr_value(el, a).and_then(|v| parse_valid_float(trim_ws(&v)).or_else(|| crate::js::reflect::parse_float(&v)));
    let min = p("min").unwrap_or(0.0);
    let max = p("max").unwrap_or(1.0).max(min);
    let value = p("value").unwrap_or(0.0).clamp(min, max);
    let low = p("low").unwrap_or(min).clamp(min, max);
    let high = p("high").unwrap_or(max).clamp(low, max);
    let optimum = p("optimum").unwrap_or((min + max) / 2.0).clamp(min, max);
    (value, min, max, low, high, optimum)
}

fn meter_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let (v, mi, ma, lo, hi, op) = meter_vals(el);
    Ok(num(match callee_str(vm, ctx).as_str() {
        "value" => v,
        "min" => mi,
        "max" => ma,
        "low" => lo,
        "high" => hi,
        _ => op,
    }))
}

fn double_attr_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let n = vm.to_number(&arg(vm, ctx, 0))?;
    if !n.is_finite() {
        return vm.throw_type("The provided double value is non-finite.");
    }
    let t = vm.to_string(&Value::Number(n))?.to_rust();
    let a = callee_str(vm, ctx);
    set_attr(vm, el, &a, &t);
    Ok(Value::Undefined)
}

fn progress_max(el: usize) -> f64 {
    attr_value(el, "max").and_then(|v| crate::js::reflect::parse_float(&v)).filter(|n| *n > 0.0).unwrap_or(1.0)
}

fn progress_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let max = progress_max(el);
    let v = attr_value(el, "value").and_then(|v| crate::js::reflect::parse_float(&v)).map(|n| n.max(0.0).min(max)).unwrap_or(0.0);
    Ok(num(v))
}

fn progress_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let n = vm.to_number(&arg(vm, ctx, 0))?;
    if !n.is_finite() {
        return vm.throw_type("The provided double value is non-finite.");
    }
    let t = vm.to_string(&Value::Number(n))?.to_rust();
    set_attr(vm, el, "value", &t);
    Ok(Value::Undefined)
}

fn progress_position(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let Some(v) = attr_value(el, "value").and_then(|v| crate::js::reflect::parse_float(&v)) else { return Ok(num(-1.0)) };
    let max = progress_max(el);
    Ok(num(v.max(0.0).min(max) / max))
}

// ------------------------------------------------------------------------------------------------ misc control IDL

fn control_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(match local(el).as_str() {
        "textarea" => "textarea",
        "fieldset" => "fieldset",
        "output" => "output",
        "select" => {
            if has(el, "multiple") {
                "select-multiple"
            } else {
                "select-one"
            }
        }
        _ => "",
    }))
}

fn fieldset_elements(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, SO_ELEMENTS, |vm| dom::make_collection(vm, dom::CK_DOCFILTER, el, "fieldsetelements", "")))
}

pub fn fieldset_elements_in(d: &Document, fs: usize) -> Vec<usize> {
    d.descendants(NodeId(fs)).map(|n| n.0).filter(|&n| is_listed(d, n)).collect()
}

fn output_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&with_doc(|d| dom::text_content_of(d, el))))
}

fn output_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    if attr_value(el, "data-aether-default").is_none() && page(|p| !p.values.contains_key(&el)) {
        let def = with_doc(|d| dom::text_content_of(d, el));
        page(|p| p.values.insert(el, def));
    }
    dom::set_text_content(vm, el, v)?;
    Ok(Value::Undefined)
}

fn output_default_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&page(|p| p.values.get(&el).cloned()).unwrap_or_else(|| with_doc(|d| dom::text_content_of(d, el)))))
}

fn output_default_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    if page(|p| p.values.contains_key(&el)) {
        page(|p| p.values.insert(el, v));
    } else {
        dom::set_text_content(vm, el, v)?;
    }
    Ok(Value::Undefined)
}

fn autocomplete_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = attr_value(el, "autocomplete").unwrap_or_default();
    let toks: Vec<String> = v.split(is_ws).filter(|t| !t.is_empty()).map(|t| t.to_ascii_lowercase()).collect();
    if local(el) == "input" && matches!(input_type(el), "hidden") {
        return Ok(s(&toks.join(" ")));
    }
    Ok(s(match toks.as_slice() {
        [one] if one == "on" || one == "off" => one,
        _ => {
            return Ok(s(&autofill_canonical(&toks)));
        }
    }))
}

/// The IDL value of an autofill detail token list ("" when it does not parse as one).
fn autofill_canonical(toks: &[String]) -> String {
    const FIELDS: &[&str] = &[
        "name", "honorific-prefix", "given-name", "additional-name", "family-name", "honorific-suffix", "nickname", "username", "new-password",
        "current-password", "one-time-code", "organization-title", "organization", "street-address", "address-line1", "address-line2",
        "address-line3", "address-level4", "address-level3", "address-level2", "address-level1", "country", "country-name", "postal-code",
        "cc-name", "cc-given-name", "cc-additional-name", "cc-family-name", "cc-number", "cc-exp", "cc-exp-month", "cc-exp-year", "cc-csc",
        "cc-type", "transaction-currency", "transaction-amount", "language", "bday", "bday-day", "bday-month", "bday-year", "sex", "url",
        "photo", "tel", "tel-country-code", "tel-national", "tel-area-code", "tel-local", "tel-local-prefix", "tel-local-suffix",
        "tel-extension", "email", "impp",
    ];
    const CONTACT: &[&str] = &["tel", "tel-country-code", "tel-national", "tel-area-code", "tel-local", "tel-local-prefix", "tel-local-suffix", "tel-extension", "email", "impp"];
    if toks.is_empty() {
        return String::new();
    }
    let mut i = toks.len();
    let mut out: Vec<&str> = Vec::new();
    // Optional trailing "webauthn".
    if toks[i - 1] == "webauthn" {
        i -= 1;
        if i == 0 {
            return String::new();
        }
    }
    let field = toks[i - 1].as_str();
    if !FIELDS.contains(&field) {
        return String::new();
    }
    i -= 1;
    let mut contact = None;
    if i > 0 && matches!(toks[i - 1].as_str(), "home" | "work" | "mobile" | "fax" | "pager") {
        if !CONTACT.contains(&field) {
            return String::new();
        }
        contact = Some(toks[i - 1].as_str());
        i -= 1;
    }
    let mut mode = None;
    if i > 0 && matches!(toks[i - 1].as_str(), "shipping" | "billing") {
        mode = Some(toks[i - 1].as_str());
        i -= 1;
    }
    let mut section = None;
    if i > 0 && toks[i - 1].starts_with("section-") {
        section = Some(toks[i - 1].as_str());
        i -= 1;
    }
    if i != 0 {
        return String::new();
    }
    out.extend(section);
    out.extend(mode);
    out.extend(contact);
    out.push(field);
    if toks.last().map(String::as_str) == Some("webauthn") {
        out.push("webauthn");
    }
    out.join(" ")
}

fn autocomplete_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "autocomplete", &v);
    Ok(Value::Undefined)
}

fn dimension_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let a = callee_str(vm, ctx);
    if local(el) == "input" && input_type(el) != "image" {
        return Ok(num(0.0));
    }
    Ok(num(attr_value(el, &a).and_then(|v| crate::js::reflect::parse_int(&v)).filter(|n| *n >= 0 && *n <= 2147483647).unwrap_or(0) as f64))
}

fn dimension_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let n = vm.to_uint32(&arg(vm, ctx, 0))? as i64;
    let a = callee_str(vm, ctx);
    set_attr(vm, el, &a, &(if n <= 2147483647 { n } else { 0 }).to_string());
    Ok(Value::Undefined)
}

fn form_reset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    reset_form(vm, el);
    Ok(Value::Undefined)
}

/// "reset" a form (§4.10.21.5): a cancelable, bubbling `reset`, then each control's reset algorithm.
pub fn reset_form(vm: &mut Vm, el: usize) {
    if RESETTING.with(|r| r.borrow().contains(&el)) {
        return;
    }
    RESETTING.with(|r| r.borrow_mut().insert(el));
    let _ = reset_form_inner(vm, el);
    RESETTING.with(|r| r.borrow_mut().remove(&el));
}

fn reset_form_inner(vm: &mut Vm, el: usize) -> JsResult<Value> {
    let ok = fire_form_event(vm, el, "reset", None);
    if !ok {
        return Ok(Value::Undefined);
    }
    let els = with_doc(|d| form_elements_in(d, el));
    for e in els {
        reset_control(vm, e)?;
    }
    super::touch();
    Ok(Value::Undefined)
}

/// The reset algorithm of one control (§4.10.21).
fn reset_control(vm: &mut Vm, el: usize) -> JsResult<()> {
    match local(el).as_str() {
        "input" | "textarea" => {
            page(|p| {
                p.values.remove(&el);
                p.checked.remove(&el);
            });
            let l = utf16_len(&value_of(el));
            SELECTION.with(|s| s.borrow_mut().insert(el, Sel { start: l, end: l, dir: 0 }));
        }
        "select" => {
            for o in select_options(el) {
                page(|p| p.checked.remove(&o));
            }
        }
        "output" => {
            if let Some(def) = page(|p| p.values.remove(&el)) {
                dom::set_text_content(vm, el, def)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn request_submit(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let sub = arg(vm, ctx, 0);
    if let Some(b) = dom::node_of(vm, &sub) {
        let is_submit = match local(b).as_str() {
            "button" => !matches!(attr_value(b, "type").unwrap_or_default().to_ascii_lowercase().as_str(), "reset" | "button"),
            "input" => matches!(input_type(b), "submit" | "image"),
            _ => false,
        };
        if !is_submit {
            return vm.throw_type("The specified element is not a submit button.");
        }
        if form_owner(b) != Some(el) {
            return throw_dom(vm, "NotFoundError", "The specified element is not owned by this form element.");
        }
    } else if !sub.is_nullish() {
        return vm.throw_type("The specified element is not a submit button.");
    }
    let submitter = dom::node_of(vm, &sub);
    submit_form(vm, el, submitter);
    Ok(Value::Undefined)
}

/// A bubbling, cancelable form event (`submit` is a SubmitEvent carrying its submitter).
fn fire_form_event(vm: &mut Vm, form: usize, ty: &str, submitter: Option<usize>) -> bool {
    let init = vm.new_plain_object();
    let iface = if ty == "submit" {
        let sv = wrap_opt(vm, submitter);
        let _ = vm.create_data_property(init, PropertyKey::from_str("submitter"), sv);
        "SubmitEvent"
    } else {
        "Event"
    };
    let e = super::events::new_event(vm, iface, ty, true, true, Some(init));
    let r = root(vm, Value::Object(e));
    let (ok, _) = super::events::dispatch(vm, e, super::TargetKey::Node(form));
    unroot(vm, r);
    ok
}

/// "submit" a form from a submitter (§4.10.21.3) up to the submit event; navigation is the shell's.
pub fn submit_form(vm: &mut Vm, el: usize, submitter: Option<usize>) {
    if SUBMITTING.with(|f| f.borrow().contains(&el)) {
        return;
    }
    SUBMITTING.with(|f| f.borrow_mut().insert(el));
    let _ = submit_events(vm, el, submitter);
    SUBMITTING.with(|f| f.borrow_mut().remove(&el));
}

fn submit_events(vm: &mut Vm, el: usize, submitter: Option<usize>) -> JsResult<Value> {
    let novalidate = submitter.is_some_and(|s| attr_value(s, "formnovalidate").is_some());
    if novalidate {
        fire_form_event(vm, el, "submit", submitter);
        return Ok(Value::Undefined);
    }
    if attr_value(el, "novalidate").is_none() {
        let els = with_doc(|d| form_elements_in(d, el));
        let mut ok = true;
        for e in els {
            ok &= check_one(vm, e);
        }
        if !ok {
            return Ok(Value::Undefined);
        }
    }
    fire_form_event(vm, el, "submit", submitter);
    Ok(Value::Undefined)
}

pub fn install(vm: &mut Vm) {
    let vs = interface(vm, "ValidityState", None, None);
    dom::register_iface("ValidityState", vs);
    for f in ["valueMissing", "typeMismatch", "patternMismatch", "tooLong", "tooShort", "rangeUnderflow", "rangeOverflow", "stepMismatch", "badInput", "customError", "valid"] {
        attr_with(vm, vs.proto, f, validity_flag, None, s(f));
    }

    let p = |n: &str| dom::iface(n).map(|i| i.proto);
    // The constraint validation API on every element that has it.
    for i in ["HTMLButtonElement", "HTMLFieldSetElement", "HTMLInputElement", "HTMLObjectElement", "HTMLOutputElement", "HTMLSelectElement", "HTMLTextAreaElement"] {
        let Some(pr) = p(i) else { continue };
        attr(vm, pr, "validity", validity_get, None);
        attr(vm, pr, "willValidate", will_validate, None);
        attr(vm, pr, "validationMessage", validation_message, None);
        op(vm, pr, "checkValidity", 0, check_validity);
        op(vm, pr, "reportValidity", 0, check_validity);
        op(vm, pr, "setCustomValidity", 1, set_custom_validity);
        attr(vm, pr, "form", form_get, None);
    }
    for i in ["HTMLButtonElement", "HTMLInputElement", "HTMLMeterElement", "HTMLOutputElement", "HTMLProgressElement", "HTMLSelectElement", "HTMLTextAreaElement"] {
        if let Some(pr) = p(i) {
            attr(vm, pr, "labels", labels_get, None);
        }
    }
    if let Some(f) = p("HTMLFormElement") {
        attr(vm, f, "elements", form_elements, None);
        attr(vm, f, "length", form_length, None);
        op(vm, f, "checkValidity", 0, check_validity);
        op(vm, f, "reportValidity", 0, check_validity);
        op(vm, f, "reset", 0, form_reset);
        op(vm, f, "requestSubmit", 0, request_submit);
    }
    if let Some(l) = p("HTMLLabelElement") {
        attr(vm, l, "control", label_control, None);
        attr(vm, l, "form", label_form, None);
    }
    if let Some(l) = p("HTMLLegendElement") {
        attr(vm, l, "form", legend_form, None);
    }
    if let Some(fs) = p("HTMLFieldSetElement") {
        attr(vm, fs, "elements", fieldset_elements, None);
        attr(vm, fs, "type", control_type, None);
    }
    if let Some(o) = p("HTMLOutputElement") {
        attr(vm, o, "value", output_value_get, Some(output_value_set));
        attr(vm, o, "defaultValue", output_default_get, Some(output_default_set));
        attr(vm, o, "type", control_type, None);
    }
    if let Some(t) = p("HTMLTextAreaElement") {
        attr(vm, t, "value", textarea_value_get, Some(textarea_value_set));
        attr(vm, t, "textLength", text_length, None);
        attr(vm, t, "type", control_type, None);
    }
    for i in ["HTMLInputElement", "HTMLTextAreaElement"] {
        let Some(pr) = p(i) else { continue };
        attr(vm, pr, "selectionStart", selection_start_get, Some(selection_start_set));
        attr(vm, pr, "selectionEnd", selection_end_get, Some(selection_end_set));
        attr(vm, pr, "selectionDirection", selection_direction_get, Some(selection_direction_set));
        op(vm, pr, "setSelectionRange", 2, set_selection_range);
        op(vm, pr, "setRangeText", 1, set_range_text);
        op(vm, pr, "select", 0, select_op);
    }
    for i in ["HTMLInputElement", "HTMLSelectElement", "HTMLTextAreaElement"] {
        if let Some(pr) = p(i) {
            attr(vm, pr, "autocomplete", autocomplete_get, Some(autocomplete_set));
        }
    }
    if let Some(sp) = p("HTMLSelectElement") {
        attr(vm, sp, "type", control_type, None);
        attr(vm, sp, "options", select_options_get, None);
        attr(vm, sp, "selectedOptions", selected_options_get, None);
        op(vm, sp, "add", 1, options_add);
        op(vm, sp, "remove", 0, options_remove);
        attr(vm, sp, "length", options_length_get, Some(options_length_set));
    }
    if let Some(hc) = dom::iface("HTMLCollection") {
        let oc = interface(vm, "HTMLOptionsCollection", Some(hc), None);
        dom::register_iface("HTMLOptionsCollection", oc);
        op(vm, oc.proto, "add", 1, options_add);
        op(vm, oc.proto, "remove", 1, options_remove);
        attr(vm, oc.proto, "length", options_length_get, Some(options_length_set));
    }
    if let Some(op_) = p("HTMLOptionElement") {
        attr(vm, op_, "index", option_index, None);
        attr(vm, op_, "label", option_label_get, Some(option_label_set));
        attr(vm, op_, "form", option_form, None);
    }
    let fl = interface(vm, "FileList", None, None);
    dom::register_iface("FileList", fl);
    attr(vm, fl.proto, "length", filelist_length, None);
    op(vm, fl.proto, "item", 1, filelist_item);
    if let Some(ip) = p("HTMLInputElement") {
        attr(vm, ip, "indeterminate", indeterminate_get, Some(indeterminate_set));
        attr(vm, ip, "list", input_list, None);
        attr(vm, ip, "files", files_get, Some(files_set));
        attr(vm, ip, "value", input_value_get, Some(input_value_set));
        attr(vm, ip, "type", input_type_get, Some(input_type_set));
        attr(vm, ip, "valueAsNumber", value_as_number_get, Some(value_as_number_set));
        attr(vm, ip, "valueAsDate", value_as_date_get, Some(value_as_date_set));
        op(vm, ip, "stepUp", 0, step_up);
        op(vm, ip, "stepDown", 0, step_down);
        attr_with(vm, ip, "width", dimension_get, Some(dimension_set), s("width"));
        attr_with(vm, ip, "height", dimension_get, Some(dimension_set), s("height"));
    }
    if let Some(mp) = p("HTMLMeterElement") {
        for a in ["value", "min", "max", "low", "high", "optimum"] {
            attr_with(vm, mp, a, meter_get, Some(double_attr_set), s(a));
        }
    }
    if let Some(pp) = p("HTMLProgressElement") {
        attr(vm, pp, "value", progress_value_get, Some(progress_value_set));
        attr(vm, pp, "position", progress_position, None);
    }
    let _ = wrap;
    let _ = remove_attr;
}
