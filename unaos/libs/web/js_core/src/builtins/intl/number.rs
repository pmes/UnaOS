//! Intl.NumberFormat (ECMA-402 §15, with the NumberFormat v3 additions of the 2023 edition: formatRange,
//! roundingMode / roundingIncrement / roundingPriority / trailingZeroDisplay, exact string input) and
//! Number.prototype.toLocaleString / BigInt.prototype.toLocaleString (§19.2.1, §19.3.1).
//!
//! Every value is rounded as an exact decimal digit string (big-integer arithmetic, never a double's): a
//! Number contributes its shortest round-tripping digits (the ICU behaviour every browser shows), a BigInt
//! or a numeric string its exact digits.

use super::*;
use crate::bignum::BigUint;
use core::cmp::Ordering;

// ============================================================================================ decimals

/// A finite decimal `±int(d) × 10^e`; `d` holds ASCII digits with no leading or trailing zeros (empty = 0).
#[derive(Clone, Debug, PartialEq)]
pub struct Dec {
    pub neg: bool,
    pub d: Vec<u8>,
    pub e: i32,
}

/// An Intl mathematical value (§15.5.16 ToIntlMathematicalValue).
#[derive(Clone, Debug)]
pub enum Mv {
    NaN,
    Inf(bool),
    Fin(Dec),
}

impl Dec {
    pub fn zero(neg: bool) -> Dec {
        Dec { neg, d: Vec::new(), e: 0 }
    }
    pub fn is_zero(&self) -> bool {
        self.d.is_empty()
    }
    /// The exponent of the leading digit (floor(log10 |x|)); 0 for zero.
    pub fn mag(&self) -> i32 {
        if self.d.is_empty() { 0 } else { self.d.len() as i32 + self.e - 1 }
    }
    pub fn normalize(mut self) -> Dec {
        let lead = self.d.iter().take_while(|&&c| c == b'0').count();
        self.d.drain(..lead);
        while self.d.last() == Some(&b'0') {
            self.d.pop();
            self.e += 1;
        }
        if self.d.is_empty() {
            self.e = 0;
        }
        self
    }
    pub fn from_digits(neg: bool, d: Vec<u8>, e: i32) -> Dec {
        Dec { neg, d, e }.normalize()
    }
    /// The decimal a finite double formats from: its shortest round-tripping digits (Number::toString's),
    /// as ICU-based engines do — `1.005` rounds to "1.01" at two fraction digits, as on every browser,
    /// where the double's exact binary value (1.00499999999999989…) would give "1".
    pub fn from_f64(x: f64) -> Dec {
        if x == 0.0 {
            return Dec::zero(x.is_sign_negative());
        }
        let (d, k) = crate::numconv::shortest_digits(x.abs());
        let n = d.len() as i32;
        Dec::from_digits(x < 0.0, d, k - n)
    }

    /// The exact decimal value of a finite double (every binary digit written out).
    pub fn from_f64_exact(x: f64) -> Dec {
        let bits = x.to_bits();
        let neg = bits >> 63 == 1;
        let be = ((bits >> 52) & 0x7ff) as i32;
        let mut m = bits & ((1u64 << 52) - 1);
        let e2 = if be == 0 {
            -1074
        } else {
            m |= 1u64 << 52;
            be - 1075
        };
        if m == 0 {
            return Dec::zero(neg);
        }
        let (n, e) = if e2 >= 0 {
            (BigUint::from_u64(m).shl(e2 as u64), 0)
        } else {
            (BigUint::from_u64(m).mul(&BigUint::pow_small(5, (-e2) as u32)), e2)
        };
        Dec::from_digits(neg, n.to_string_radix(10), e)
    }
    pub fn is_integer(&self) -> bool {
        self.d.is_empty() || self.e >= 0
    }
    /// The integer and fraction digit strings, the fraction padded to `min_frac`, the integer to `min_int`.
    pub fn to_strings(&self, min_frac: usize, min_int: usize) -> (String, String) {
        let (mut int, mut frac) = if self.d.is_empty() {
            (String::from("0"), String::new())
        } else if self.e >= 0 {
            let mut s = String::from(core::str::from_utf8(&self.d).unwrap());
            for _ in 0..self.e {
                s.push('0');
            }
            (s, String::new())
        } else {
            let point = self.d.len() as i32 + self.e;
            let ds = core::str::from_utf8(&self.d).unwrap();
            if point <= 0 {
                let mut f = String::new();
                for _ in 0..(-point) {
                    f.push('0');
                }
                f.push_str(ds);
                (String::from("0"), f)
            } else {
                (String::from(&ds[..point as usize]), String::from(&ds[point as usize..]))
            }
        };
        while frac.len() < min_frac {
            frac.push('0');
        }
        while int.len() < min_int {
            int.insert(0, '0');
        }
        (int, frac)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Ceil,
    Floor,
    Expand,
    Trunc,
    HalfCeil,
    HalfFloor,
    HalfExpand,
    HalfTrunc,
    HalfEven,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "ceil" => Mode::Ceil,
            "floor" => Mode::Floor,
            "expand" => Mode::Expand,
            "trunc" => Mode::Trunc,
            "halfCeil" => Mode::HalfCeil,
            "halfFloor" => Mode::HalfFloor,
            "halfTrunc" => Mode::HalfTrunc,
            "halfEven" => Mode::HalfEven,
            _ => Mode::HalfExpand,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Ceil => "ceil",
            Mode::Floor => "floor",
            Mode::Expand => "expand",
            Mode::Trunc => "trunc",
            Mode::HalfCeil => "halfCeil",
            Mode::HalfFloor => "halfFloor",
            Mode::HalfExpand => "halfExpand",
            Mode::HalfTrunc => "halfTrunc",
            Mode::HalfEven => "halfEven",
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Rest {
    Zero,
    Below,
    Half,
    Above,
}

/// Round `x` to a multiple of `inc × 10^pos` (§15.5.8 ApplyUnsignedRoundingMode on the exact value).
pub fn round_at(x: &Dec, pos: i32, mode: Mode, inc: u32) -> Dec {
    if x.is_zero() {
        return x.clone();
    }
    let len = x.d.len() as i32;
    let drop = pos - x.e;
    let (kept, rest): (Vec<u8>, Rest) = if drop <= 0 {
        let mut k = x.d.clone();
        for _ in 0..(-drop) {
            k.push(b'0');
        }
        (k, Rest::Zero)
    } else if drop > len {
        (vec![b'0'], Rest::Below)
    } else {
        let k = if drop == len { vec![b'0'] } else { x.d[..(len - drop) as usize].to_vec() };
        let r = &x.d[(len - drop) as usize..];
        let rest = if r.is_empty() {
            Rest::Zero
        } else {
            match r[0].cmp(&b'5') {
                Ordering::Less => Rest::Below,
                Ordering::Greater => Rest::Above,
                Ordering::Equal => {
                    if r.len() == 1 { Rest::Half } else { Rest::Above }
                }
            }
        };
        (k, rest)
    };
    let k = BigUint::from_digits(&kept, 10);
    let mut q = k.clone();
    let r = q.div_small_assign(inc.max(1)) as u64;
    let inc2 = inc.max(1) as u64;
    let a = 2 * r;
    let half = match rest {
        Rest::Zero => a.cmp(&inc2),
        Rest::Below => {
            if a < inc2 { Ordering::Less } else { Ordering::Greater }
        }
        Rest::Half => (a + 1).cmp(&inc2),
        Rest::Above => {
            if a + 2 <= inc2 { Ordering::Less } else { Ordering::Greater }
        }
    };
    let result = if r == 0 && rest == Rest::Zero {
        k
    } else {
        let neg = x.neg;
        let tie = |up_on_tie: bool| match half {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => up_on_tie,
        };
        let up = match mode {
            Mode::Ceil => !neg,
            Mode::Floor => neg,
            Mode::Expand => true,
            Mode::Trunc => false,
            Mode::HalfCeil => tie(!neg),
            Mode::HalfFloor => tie(neg),
            Mode::HalfExpand => tie(true),
            Mode::HalfTrunc => tie(false),
            Mode::HalfEven => tie(q.low_u64() & 1 == 1),
        };
        let base = if up { q.add(&BigUint::from_u64(1)) } else { q };
        base.mul_small(inc.max(1))
    };
    let digits = if result.is_zero() { Vec::new() } else { result.to_string_radix(10) };
    Dec::from_digits(x.neg, digits, pos)
}

// ============================================================================================ digit options

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RType {
    Frac,
    Sig,
    More,
    Less,
}

#[derive(Clone, Debug)]
pub struct Digits {
    pub min_int: u32,
    pub min_frac: u32,
    pub max_frac: u32,
    pub min_sig: u32,
    pub max_sig: u32,
    pub rtype: RType,
    pub increment: u32,
    pub mode: Mode,
    pub priority: &'static str,
    pub strip_if_integer: bool,
}

impl Digits {
    pub fn fixed(min_frac: u32, max_frac: u32) -> Digits {
        Digits {
            min_int: 1,
            min_frac,
            max_frac,
            min_sig: 1,
            max_sig: 21,
            rtype: RType::Frac,
            increment: 1,
            mode: Mode::HalfExpand,
            priority: "auto",
            strip_if_integer: false,
        }
    }
}

const INCREMENTS: [u32; 15] = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000];

/// SetNumberFormatDigitOptions (§15.1.3).
pub fn digit_options(vm: &mut Vm, o: Option<Obj>, mnfd_default: u32, mxfd_default: u32, compact: bool) -> JsResult<Digits> {
    let min_int = get_number_option(vm, o, "minimumIntegerDigits", 1.0, 21.0)?.unwrap_or(1);
    let mnfd = get_prop(vm, o, "minimumFractionDigits")?;
    let mxfd = get_prop(vm, o, "maximumFractionDigits")?;
    let mnsd = get_prop(vm, o, "minimumSignificantDigits")?;
    let mxsd = get_prop(vm, o, "maximumSignificantDigits")?;
    let increment = get_number_option(vm, o, "roundingIncrement", 1.0, 5000.0)?.unwrap_or(1);
    if !INCREMENTS.contains(&increment) {
        return vm.throw_range("roundingIncrement value is out of range.");
    }
    let mode = get_string_option(
        vm,
        o,
        "roundingMode",
        &["ceil", "floor", "expand", "trunc", "halfCeil", "halfFloor", "halfExpand", "halfTrunc", "halfEven"],
        "Intl.NumberFormat",
    )?;
    let mode = Mode::parse(mode.as_deref().unwrap_or("halfExpand"));
    let priority = get_string_option(vm, o, "roundingPriority", &["auto", "morePrecision", "lessPrecision"], "Intl.NumberFormat")?;
    let priority: &'static str = match priority.as_deref() {
        Some("morePrecision") => "morePrecision",
        Some("lessPrecision") => "lessPrecision",
        _ => "auto",
    };
    let tzd = get_string_option(vm, o, "trailingZeroDisplay", &["auto", "stripIfInteger"], "Intl.NumberFormat")?;
    let strip_if_integer = tzd.as_deref() == Some("stripIfInteger");
    let mxfd_default = if increment != 1 { mnfd_default } else { mxfd_default };
    let has_sd = !mnsd.is_undefined() || !mxsd.is_undefined();
    let has_fd = !mnfd.is_undefined() || !mxfd.is_undefined();
    let mut need_sd = true;
    let mut need_fd = true;
    if priority == "auto" {
        need_sd = has_sd;
        if need_sd || (!has_fd && compact) {
            need_fd = false;
        }
    }
    let mut d = Digits { min_int, ..Digits::fixed(mnfd_default, mxfd_default) };
    d.increment = increment;
    d.mode = mode;
    d.priority = priority;
    d.strip_if_integer = strip_if_integer;
    if need_sd {
        if has_sd {
            let mn = default_number_option(vm, &mnsd, 1.0, 21.0, "minimumSignificantDigits")?.unwrap_or(1);
            let mx = default_number_option(vm, &mxsd, mn as f64, 21.0, "maximumSignificantDigits")?.unwrap_or(21);
            d.min_sig = mn;
            d.max_sig = mx;
        } else {
            d.min_sig = 1;
            d.max_sig = 21;
        }
    }
    if need_fd {
        if has_fd {
            let mn = default_number_option(vm, &mnfd, 0.0, 100.0, "minimumFractionDigits")?;
            let mx = default_number_option(vm, &mxfd, 0.0, 100.0, "maximumFractionDigits")?;
            match (mn, mx) {
                (None, Some(mx)) => {
                    d.min_frac = mnfd_default.min(mx);
                    d.max_frac = mx;
                }
                (Some(mn), None) => {
                    d.min_frac = mn;
                    d.max_frac = mxfd_default.max(mn);
                }
                (Some(mn), Some(mx)) => {
                    if mn > mx {
                        return vm.throw_range("maximumFractionDigits value is out of range.");
                    }
                    d.min_frac = mn;
                    d.max_frac = mx;
                }
                (None, None) => {}
            }
        } else {
            d.min_frac = mnfd_default;
            d.max_frac = mxfd_default;
        }
    }
    if !need_sd && !need_fd {
        d.min_frac = 0;
        d.max_frac = 0;
        d.min_sig = 1;
        d.max_sig = 2;
        d.rtype = RType::More;
        d.priority = "morePrecision";
    } else if priority == "morePrecision" {
        d.rtype = RType::More;
    } else if priority == "lessPrecision" {
        d.rtype = RType::Less;
    } else if has_sd {
        d.rtype = RType::Sig;
    } else {
        d.rtype = RType::Frac;
    }
    if increment != 1 {
        if d.rtype != RType::Frac {
            return vm.throw_type("roundingIncrement requires fraction-digit rounding");
        }
        if d.max_frac != d.min_frac {
            return vm.throw_range("roundingIncrement requires maximumFractionDigits equal to minimumFractionDigits");
        }
    }
    Ok(d)
}

/// FormatNumericToString (§15.5.3): the rounded value and its integer / fraction digit strings.
pub fn format_numeric(dg: &Digits, x: &Dec) -> (Dec, String, String) {
    let sig_minf = |r: &Dec| -> usize {
        if r.is_zero() {
            dg.min_sig.saturating_sub(1) as usize
        } else {
            (dg.min_sig as i32 - (r.mag() + 1)).max(0) as usize
        }
    };
    let frac_round = || round_at(x, -(dg.max_frac as i32), dg.mode, dg.increment);
    let sig_round = || round_at(x, x.mag() - dg.max_sig as i32 + 1, dg.mode, 1);
    let (r, mut minf) = match dg.rtype {
        RType::Frac => (frac_round(), dg.min_frac as usize),
        RType::Sig => {
            let r = sig_round();
            let m = sig_minf(&r);
            (r, m)
        }
        RType::More | RType::Less => {
            let s_pos = if x.is_zero() { 1 - dg.max_sig as i32 } else { x.mag() - dg.max_sig as i32 + 1 };
            let f_pos = -(dg.max_frac as i32);
            let pick_sig = if dg.rtype == RType::More { s_pos <= f_pos } else { s_pos > f_pos };
            if pick_sig {
                let r = sig_round();
                let m = sig_minf(&r);
                (r, m)
            } else {
                (frac_round(), dg.min_frac as usize)
            }
        }
    };
    if dg.strip_if_integer && r.is_integer() {
        minf = 0;
    }
    let (i, f) = r.to_strings(minf, dg.min_int as usize);
    (r, i, f)
}

// ============================================================================================ the formatter

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    Decimal,
    Percent,
    Currency,
    Unit,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Notation {
    Standard,
    Scientific,
    Engineering,
    Compact,
}

#[derive(Clone, Debug)]
pub struct NumFmt {
    pub locale: String,
    pub gb: bool,
    pub style: Style,
    pub currency: String,
    pub currency_display: String,
    pub currency_sign: String,
    pub unit: String,
    pub unit_display: String,
    pub dg: Digits,
    pub notation: Notation,
    pub compact_long: bool,
    /// "always" / "auto" / "min2", or None for false.
    pub grouping: Option<&'static str>,
    pub sign: String,
}

fn is_sanctioned(u: &str) -> bool {
    data::unit(u).is_some()
}

/// IsWellFormedUnitIdentifier (§6.6.1).
fn well_formed_unit(u: &str) -> bool {
    if is_sanctioned(u) {
        return true;
    }
    match u.split_once("-per-") {
        Some((a, b)) => is_sanctioned(a) && is_sanctioned(b),
        None => false,
    }
}

/// InitializeNumberFormat (§15.1.2) as a Rust value (shared by the constructor and toLocaleString).
pub fn create(vm: &mut Vm, locales: &Value, options: &Value) -> JsResult<NumFmt> {
    let requested = canonicalize_locale_list(vm, locales)?;
    let o = coerce_options(vm, options)?;
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], "Intl.NumberFormat")?;
    if let Some(ns) = get_string_option(vm, o, "numberingSystem", &[], "Intl.NumberFormat")? {
        if !ns.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid numberingSystem : {ns}"));
        }
    }
    let r = resolve_locale(&requested, &["nu"]);
    let kept: Vec<(&str, &str)> = match r.keyword("nu") {
        Some("latn") => vec![("nu", "latn")],
        _ => Vec::new(),
    };
    let locale = r.locale_with(&kept);
    // SetNumberFormatUnitOptions (§15.1.4)
    let style = get_string_option(vm, o, "style", &["decimal", "percent", "currency", "unit"], "Intl.NumberFormat")?;
    let style = match style.as_deref() {
        Some("percent") => Style::Percent,
        Some("currency") => Style::Currency,
        Some("unit") => Style::Unit,
        _ => Style::Decimal,
    };
    let currency = get_string_option(vm, o, "currency", &[], "Intl.NumberFormat")?;
    if let Some(c) = &currency {
        if !(c.len() == 3 && c.bytes().all(|b| b.is_ascii_alphabetic())) {
            return vm.throw_range(&format!("Invalid currency code : {c}"));
        }
    }
    let currency_display = get_string_option(vm, o, "currencyDisplay", &["code", "symbol", "narrowSymbol", "name"], "Intl.NumberFormat")?
        .unwrap_or_else(|| String::from("symbol"));
    let currency_sign =
        get_string_option(vm, o, "currencySign", &["standard", "accounting"], "Intl.NumberFormat")?.unwrap_or_else(|| String::from("standard"));
    let unit = get_string_option(vm, o, "unit", &[], "Intl.NumberFormat")?;
    if let Some(u) = &unit {
        if !well_formed_unit(u) {
            return vm.throw_range(&format!("Invalid unit argument for Intl.NumberFormat() '{u}'"));
        }
    }
    let unit_display =
        get_string_option(vm, o, "unitDisplay", &["short", "narrow", "long"], "Intl.NumberFormat")?.unwrap_or_else(|| String::from("short"));
    if style == Style::Currency && currency.is_none() {
        return vm.throw_type("Currency code is required with currency style.");
    }
    if style == Style::Unit && unit.is_none() {
        return vm.throw_type("Unit is required with unit style.");
    }
    let currency = currency.map(|c| c.to_ascii_uppercase()).unwrap_or_default();
    let notation = get_string_option(vm, o, "notation", &["standard", "scientific", "engineering", "compact"], "Intl.NumberFormat")?;
    let notation = match notation.as_deref() {
        Some("scientific") => Notation::Scientific,
        Some("engineering") => Notation::Engineering,
        Some("compact") => Notation::Compact,
        _ => Notation::Standard,
    };
    let (mnfd_d, mxfd_d) = if style == Style::Currency && notation == Notation::Standard {
        let cd = data::currency(&currency).map(|c| c.digits).unwrap_or(2);
        (cd, cd)
    } else {
        (0, if style == Style::Percent { 0 } else { 3 })
    };
    let dg = digit_options(vm, o, mnfd_d, mxfd_d, notation == Notation::Compact)?;
    let cd = get_string_option(vm, o, "compactDisplay", &["short", "long"], "Intl.NumberFormat")?;
    let compact_long = cd.as_deref() == Some("long");
    let default_grouping: &'static str = if notation == Notation::Compact { "min2" } else { "auto" };
    // GetBooleanOrStringNumberFormatOption (§15.1.5)
    let ug = get_prop(vm, o, "useGrouping")?;
    let grouping: Option<&'static str> = match &ug {
        Value::Undefined => Some(default_grouping),
        Value::Bool(true) => Some("always"),
        v if !vm.to_boolean(v) => None,
        v => {
            let s = vm.to_string(v)?.to_rust();
            match s.as_str() {
                "min2" => Some("min2"),
                "auto" => Some("auto"),
                "always" => Some("always"),
                "true" | "false" => Some(default_grouping),
                _ => return vm.throw_range(&format!("Value {s} out of range for Intl.NumberFormat options property useGrouping")),
            }
        }
    };
    let sign = get_string_option(vm, o, "signDisplay", &["auto", "never", "always", "exceptZero", "negative"], "Intl.NumberFormat")?
        .unwrap_or_else(|| String::from("auto"));
    Ok(NumFmt {
        gb: is_gb(&locale),
        locale,
        style,
        currency,
        currency_display,
        currency_sign,
        unit: unit.unwrap_or_default(),
        unit_display,
        dg,
        notation,
        compact_long,
        grouping,
        sign,
    })
}

/// ToIntlMathematicalValue (§15.5.16).
pub fn to_mv(vm: &mut Vm, v: &Value) -> JsResult<Mv> {
    let p = vm.to_primitive(v, 1)?;
    match &p {
        Value::BigInt(b) => {
            let digits = if b.mag.is_zero() { Vec::new() } else { b.mag.to_string_radix(10) };
            Ok(Mv::Fin(Dec::from_digits(b.neg, digits, 0)))
        }
        Value::String(s) => Ok(match parse_decimal_string(&s.to_rust()) {
            Some(m) => m,
            None => Mv::NaN,
        }),
        _ => {
            let n = vm.to_number(&p)?;
            Ok(mv_of(n))
        }
    }
}

pub fn mv_of(n: f64) -> Mv {
    if n.is_nan() {
        Mv::NaN
    } else if n.is_infinite() {
        Mv::Inf(n < 0.0)
    } else {
        Mv::Fin(Dec::from_f64(n))
    }
}

/// StringIntlMV: the exact value of a StringNumericLiteral (§7.1.4.1.1 grammar).
pub fn parse_decimal_string(s: &str) -> Option<Mv> {
    let t: &str = s.trim_matches(|c: char| crate::unicode::is_str_whitespace(c as u32));
    if t.is_empty() {
        return Some(Mv::Fin(Dec::zero(false)));
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' && matches!(b[1], b'x' | b'X' | b'o' | b'O' | b'b' | b'B') {
        let radix = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            _ => 2,
        };
        let digits = &b[2..];
        if !digits.iter().all(|&c| (c as char).to_digit(radix).is_some()) {
            return None;
        }
        let n = BigUint::from_digits(digits, radix);
        let d = if n.is_zero() { Vec::new() } else { n.to_string_radix(10) };
        return Some(Mv::Fin(Dec::from_digits(false, d, 0)));
    }
    let (neg, rest) = match b[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if rest == "Infinity" {
        return Some(Mv::Inf(neg));
    }
    let rb = rest.as_bytes();
    let mut i = 0;
    let mut digits: Vec<u8> = Vec::new();
    let mut e: i64 = 0;
    let mut any = false;
    while i < rb.len() && rb[i].is_ascii_digit() {
        digits.push(rb[i]);
        any = true;
        i += 1;
    }
    if i < rb.len() && rb[i] == b'.' {
        i += 1;
        while i < rb.len() && rb[i].is_ascii_digit() {
            digits.push(rb[i]);
            e -= 1;
            any = true;
            i += 1;
        }
    }
    if !any {
        return None;
    }
    if i < rb.len() && (rb[i] == b'e' || rb[i] == b'E') {
        i += 1;
        let eneg = match rb.get(i) {
            Some(b'-') => {
                i += 1;
                true
            }
            Some(b'+') => {
                i += 1;
                false
            }
            _ => false,
        };
        let st = i;
        let mut ev: i64 = 0;
        while i < rb.len() && rb[i].is_ascii_digit() {
            ev = (ev * 10 + (rb[i] - b'0') as i64).min(1 << 40);
            i += 1;
        }
        if i == st {
            return None;
        }
        e += if eneg { -ev } else { ev };
    }
    if i != rb.len() {
        return None;
    }
    let d = Dec::from_digits(neg, digits, e.clamp(-(1 << 30), 1 << 30) as i32);
    // Values beyond the double range format as the spec's mathematical value; very large exponents are
    // treated as infinities to keep the digit strings bounded.
    if !d.is_zero() && d.mag() > 400 {
        return Some(Mv::Inf(neg));
    }
    if !d.is_zero() && d.mag() < -400 {
        return Some(Mv::Fin(Dec::zero(neg)));
    }
    Some(Mv::Fin(d))
}

fn exp_for_mag(nf: &NumFmt, mag: i32) -> i32 {
    match nf.notation {
        Notation::Standard => 0,
        Notation::Scientific => mag,
        Notation::Engineering => mag.div_euclid(3) * 3,
        Notation::Compact => {
            if mag < 3 {
                0
            } else {
                (mag.div_euclid(3) * 3).min(12)
            }
        }
    }
}

/// ComputeExponent (§15.5.6).
fn compute_exponent(nf: &NumFmt, x: &Dec) -> i32 {
    if x.is_zero() {
        return 0;
    }
    let mag = x.mag();
    let exp = exp_for_mag(nf, mag);
    let mut y = x.clone();
    y.e -= exp;
    let (r, _, _) = format_numeric(&nf.dg, &y);
    if r.is_zero() || r.mag() == mag - exp {
        return exp;
    }
    exp_for_mag(nf, mag + 1)
}

fn push(parts: &mut Vec<(String, String)>, t: &str, v: &str) {
    parts.push((String::from(t), String::from(v)));
}

fn push_integer(parts: &mut Vec<(String, String)>, int: &str, grouping: Option<&str>) {
    let group = match grouping {
        None => false,
        Some("min2") => int.len() >= 5,
        Some(_) => int.len() > 3,
    };
    if !group {
        push(parts, "integer", int);
        return;
    }
    let first = int.len() % 3;
    let mut i = 0;
    if first > 0 {
        push(parts, "integer", &int[..first]);
        i = first;
    }
    while i < int.len() {
        if i > 0 {
            push(parts, "group", ",");
        }
        push(parts, "integer", &int[i..i + 3]);
        i += 3;
    }
}

fn compact_name(exp: i32, long: bool) -> &'static str {
    match (exp, long) {
        (3, false) => "K",
        (6, false) => "M",
        (9, false) => "B",
        (12, false) => "T",
        (3, true) => "thousand",
        (6, true) => "million",
        (9, true) => "billion",
        (12, true) => "trillion",
        _ => "",
    }
}

/// The CLDR `en` cardinal plural category of a formatted number (i = integer digits, v = fraction digits).
pub fn en_cardinal(int: &str, frac: &str) -> &'static str {
    if int == "1" && frac.is_empty() { "one" } else { "other" }
}

fn unit_pattern(nf: &NumFmt, one: bool) -> String {
    let u = nf.unit.as_str();
    let pick = |x: &data::Unit| -> &'static str {
        match nf.unit_display.as_str() {
            "long" => {
                if one {
                    x.long_one
                } else {
                    x.long_other
                }
            }
            "narrow" => x.narrow,
            _ => {
                if one {
                    x.short_one
                } else {
                    x.short_other
                }
            }
        }
    };
    if let Some(x) = data::unit(u) {
        return String::from(pick(x));
    }
    if let Some((s, n, lo, lp)) = data::compound_unit(u) {
        return String::from(match nf.unit_display.as_str() {
            "long" => {
                if one {
                    lo
                } else {
                    lp
                }
            }
            "narrow" => n,
            _ => s,
        });
    }
    let (a, b) = u.split_once("-per-").unwrap_or((u, ""));
    let (ua, ub) = match (data::unit(a), data::unit(b)) {
        (Some(x), Some(y)) => (x, y),
        _ => return String::from("{0}"),
    };
    match nf.unit_display.as_str() {
        "long" => {
            let per = ub.long_one.replace("{0} ", "");
            format!("{} per {}", if one { ua.long_one } else { ua.long_other }, per)
        }
        "narrow" => format!("{}/{}", ua.narrow, ub.narrow.replace("{0}", "")),
        _ => format!("{}/{}", if one { ua.short_one } else { ua.short_other }, ub.short_one.replace("{0} ", "").replace("{0}", "")),
    }
}

/// PartitionNumberPattern (§15.5.4).
pub fn format_parts(nf: &NumFmt, mv: &Mv) -> Vec<(String, String)> {
    let mut num: Vec<(String, String)> = Vec::new();
    let (neg, zero, nan);
    let mut int_s = String::new();
    let mut frac_s = String::new();
    match mv {
        Mv::NaN => {
            push(&mut num, "nan", "NaN");
            neg = false;
            zero = false;
            nan = true;
        }
        Mv::Inf(n) => {
            push(&mut num, "infinity", "∞");
            neg = *n;
            zero = false;
            nan = false;
        }
        Mv::Fin(x) => {
            let mut x = x.clone();
            if nf.style == Style::Percent && !x.is_zero() {
                x.e += 2;
            }
            let exp = compute_exponent(nf, &x);
            if !x.is_zero() {
                x.e -= exp;
            }
            let (r, int, frac) = format_numeric(&nf.dg, &x);
            neg = x.neg;
            zero = r.is_zero();
            nan = false;
            push_integer(&mut num, &int, nf.grouping);
            if !frac.is_empty() {
                push(&mut num, "decimal", ".");
                push(&mut num, "fraction", &frac);
            }
            match nf.notation {
                Notation::Scientific | Notation::Engineering => {
                    push(&mut num, "exponentSeparator", "E");
                    if exp < 0 {
                        push(&mut num, "exponentMinusSign", "-");
                    }
                    push(&mut num, "exponentInteger", &format!("{}", exp.unsigned_abs()));
                }
                Notation::Compact if exp > 0 => {
                    if nf.compact_long {
                        push(&mut num, "literal", " ");
                    }
                    push(&mut num, "compact", compact_name(exp, nf.compact_long));
                }
                _ => {}
            }
            int_s = int;
            frac_s = frac;
            if exp != 0 {
                // A scaled value ("1K", "1E3") is never CLDR `one` (its operand n is not 1).
                frac_s.push('c');
            }
        }
    }
    let sign: &str = match nf.sign.as_str() {
        "always" => {
            if neg {
                "-"
            } else {
                "+"
            }
        }
        "never" => "",
        "exceptZero" => {
            if zero || nan {
                ""
            } else if neg {
                "-"
            } else {
                "+"
            }
        }
        "negative" => {
            if neg && !zero {
                "-"
            } else {
                ""
            }
        }
        _ => {
            if neg {
                "-"
            } else {
                ""
            }
        }
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let push_sign = |out: &mut Vec<(String, String)>| match sign {
        "-" => push(out, "minusSign", "-"),
        "+" => push(out, "plusSign", "+"),
        _ => {}
    };
    let one = en_cardinal(&int_s, &frac_s) == "one";
    match nf.style {
        Style::Decimal => {
            push_sign(&mut out);
            out.extend(num);
        }
        Style::Percent => {
            push_sign(&mut out);
            out.extend(num);
            push(&mut out, "percentSign", "%");
        }
        Style::Currency => {
            let c = data::currency(&nf.currency);
            if nf.currency_display == "name" {
                push_sign(&mut out);
                out.extend(num);
                push(&mut out, "literal", " ");
                let name = match c {
                    Some(c) => String::from(if one { c.one } else { c.other }),
                    None => nf.currency.clone(),
                };
                push(&mut out, "currency", &name);
            } else {
                let sym: String = match (nf.currency_display.as_str(), c) {
                    ("code", _) | (_, None) => nf.currency.clone(),
                    ("narrowSymbol", Some(c)) => String::from(c.narrow),
                    (_, Some(c)) => String::from(if nf.gb { data::gb_symbol(c.code).unwrap_or(c.symbol) } else { c.symbol }),
                };
                let spaced = sym.chars().last().map(|ch| ch.is_alphabetic()).unwrap_or(false);
                let accounting = nf.currency_sign == "accounting" && sign == "-";
                if accounting {
                    push(&mut out, "literal", "(");
                } else {
                    push_sign(&mut out);
                }
                push(&mut out, "currency", &sym);
                if spaced {
                    push(&mut out, "literal", "\u{a0}");
                }
                out.extend(num);
                if accounting {
                    push(&mut out, "literal", ")");
                }
            }
        }
        Style::Unit => {
            let pat = unit_pattern(nf, one);
            let (pre, post) = pat.split_once("{0}").unwrap_or(("", ""));
            if !pre.is_empty() {
                push(&mut out, "unit", pre);
            }
            push_sign(&mut out);
            out.extend(num);
            if !post.is_empty() {
                let trimmed = post.trim_start_matches(' ');
                let spaces = post.len() - trimmed.len();
                if spaces > 0 {
                    push(&mut out, "literal", &post[..spaces]);
                }
                if !trimmed.is_empty() {
                    push(&mut out, "unit", trimmed);
                }
            }
        }
    }
    out
}

pub fn format_to_string(nf: &NumFmt, mv: &Mv) -> String {
    concat_parts(&format_parts(nf, mv))
}

/// PartitionNumberRangePattern (§15.5.21) / FormatApproximately (§15.5.22).
fn range_parts(nf: &NumFmt, x: &Mv, y: &Mv) -> (Vec<(String, String)>, Vec<String>) {
    let a = format_parts(nf, x);
    let b = format_parts(nf, y);
    let mut parts = Vec::new();
    let mut src = Vec::new();
    if concat_parts(&a) == concat_parts(&b) {
        let mut inserted = false;
        for p in a {
            if !inserted && matches!(p.0.as_str(), "integer" | "nan" | "infinity" | "minusSign" | "plusSign" | "currency" | "literal") {
                parts.push((String::from("approximatelySign"), String::from("~")));
                src.push(String::from("shared"));
                inserted = true;
            }
            parts.push(p);
            src.push(String::from("shared"));
        }
        return (parts, src);
    }
    for p in a {
        parts.push(p);
        src.push(String::from("startRange"));
    }
    parts.push((String::from("literal"), String::from(if nf.style == Style::Decimal { "–" } else { " – " })));
    src.push(String::from("shared"));
    for p in b {
        parts.push(p);
        src.push(String::from("endRange"));
    }
    (parts, src)
}

// ============================================================================================ JS surface

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "NumberFormat", 0, nf_ctor);
    accessor(vm, p, PropertyKey::from_str("format"), "format", Some(nf_format_getter), None, C);
    method(vm, p, "formatToParts", 1, nf_format_to_parts);
    method(vm, p, "formatRange", 2, nf_format_range);
    method(vm, p, "formatRangeToParts", 2, nf_format_range_to_parts);
    method(vm, p, "resolvedOptions", 0, nf_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_number_format_proto = p;
}

fn nf_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    // Callable without `new` (§15.1.1 step 1: NewTarget undefined → the active function).
    let nt = if ctx.new_target.is_undefined() { Value::Object(ctx.callee) } else { ctx.new_target.clone() };
    let p = vm.get_prototype_from_ctor(&nt, |i| i.intl_number_format_proto)?;
    let l = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let nf = create(vm, &l, &o)?;
    Ok(Value::Object(alloc_intl(vm, p, IntlData::NumberFormat(Box::new(nf)))))
}

fn is_nf(d: &IntlData) -> bool {
    matches!(d, IntlData::NumberFormat(_))
}

fn nf_of(vm: &Vm, o: Obj) -> NumFmt {
    match intl_ref(vm, o) {
        IntlData::NumberFormat(n) => (**n).clone(),
        _ => unreachable!(),
    }
}

fn nf_format_getter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_nf, "get Intl.NumberFormat.prototype.format")?;
    Ok(bound_getter(vm, o, "", 1, nf_bound_format))
}

fn nf_bound_format(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = bound_target(vm, ctx);
    let v = vm.arg(ctx, 0);
    let mv = to_mv(vm, &v)?;
    let nf = nf_of(vm, o);
    Ok(Value::str(&format_to_string(&nf, &mv)))
}

fn nf_format_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_nf, "Intl.NumberFormat.prototype.formatToParts")?;
    let v = vm.arg(ctx, 0);
    let mv = to_mv(vm, &v)?;
    let nf = nf_of(vm, o);
    let parts = format_parts(&nf, &mv);
    Ok(parts_array(vm, &parts, None))
}

fn range_args(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<(NumFmt, Mv, Mv)> {
    let o = this_intl(vm, &ctx.this, is_nf, what)?;
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    if a.is_undefined() || b.is_undefined() {
        return vm.throw_type("start or end is undefined");
    }
    let x = to_mv(vm, &a)?;
    let y = to_mv(vm, &b)?;
    if matches!(x, Mv::NaN) || matches!(y, Mv::NaN) {
        return vm.throw_range("Invalid number range: NaN");
    }
    Ok((nf_of(vm, o), x, y))
}

fn nf_format_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (nf, x, y) = range_args(vm, ctx, "Intl.NumberFormat.prototype.formatRange")?;
    let (parts, _) = range_parts(&nf, &x, &y);
    Ok(Value::str(&concat_parts(&parts)))
}

fn nf_format_range_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (nf, x, y) = range_args(vm, ctx, "Intl.NumberFormat.prototype.formatRangeToParts")?;
    let (parts, src) = range_parts(&nf, &x, &y);
    Ok(parts_array(vm, &parts, Some(("source", &src))))
}

fn nf_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_nf, "Intl.NumberFormat.prototype.resolvedOptions")?;
    let nf = nf_of(vm, o);
    let mut props: Vec<(&str, Value)> = vec![("locale", Value::str(&nf.locale)), ("numberingSystem", Value::str("latn"))];
    props.push((
        "style",
        Value::str(match nf.style {
            Style::Decimal => "decimal",
            Style::Percent => "percent",
            Style::Currency => "currency",
            Style::Unit => "unit",
        }),
    ));
    if nf.style == Style::Currency {
        props.push(("currency", Value::str(&nf.currency)));
        props.push(("currencyDisplay", Value::str(&nf.currency_display)));
        props.push(("currencySign", Value::str(&nf.currency_sign)));
    }
    if nf.style == Style::Unit {
        props.push(("unit", Value::str(&nf.unit)));
        props.push(("unitDisplay", Value::str(&nf.unit_display)));
    }
    let d = &nf.dg;
    props.push(("minimumIntegerDigits", Value::Number(d.min_int as f64)));
    if d.rtype != RType::Sig {
        props.push(("minimumFractionDigits", Value::Number(d.min_frac as f64)));
        props.push(("maximumFractionDigits", Value::Number(d.max_frac as f64)));
    }
    if d.rtype != RType::Frac {
        props.push(("minimumSignificantDigits", Value::Number(d.min_sig as f64)));
        props.push(("maximumSignificantDigits", Value::Number(d.max_sig as f64)));
    }
    props.push(("useGrouping", match nf.grouping {
        Some(g) => Value::str(g),
        None => Value::Bool(false),
    }));
    props.push((
        "notation",
        Value::str(match nf.notation {
            Notation::Standard => "standard",
            Notation::Scientific => "scientific",
            Notation::Engineering => "engineering",
            Notation::Compact => "compact",
        }),
    ));
    if nf.notation == Notation::Compact {
        props.push(("compactDisplay", Value::str(if nf.compact_long { "long" } else { "short" })));
    }
    props.push(("signDisplay", Value::str(&nf.sign)));
    props.push(("roundingIncrement", Value::Number(d.increment as f64)));
    props.push(("roundingMode", Value::str(d.mode.name())));
    props.push(("roundingPriority", Value::str(d.priority)));
    props.push(("trailingZeroDisplay", Value::str(if d.strip_if_integer { "stripIfInteger" } else { "auto" })));
    let r = object_from(vm, props);
    Ok(Value::Object(r))
}

/// Number.prototype.toLocaleString (§19.2.1) and BigInt.prototype.toLocaleString (§19.3.1).
pub fn to_locale_string(vm: &mut Vm, x: Mv, locales: &Value, options: &Value) -> JsResult<Value> {
    let nf = create(vm, locales, options)?;
    Ok(Value::str(&format_to_string(&nf, &x)))
}
