//! Intl.NumberFormat (ECMA-402 §15, with the NumberFormat v3 additions of the 2023 edition: formatRange,
//! roundingMode / roundingIncrement / roundingPriority / trailingZeroDisplay, exact string input) and
//! Number.prototype.toLocaleString / BigInt.prototype.toLocaleString (§19.2.1, §19.3.1).
//!
//! Every value is rounded as an exact decimal digit string (big-integer arithmetic, never a double's): a
//! Number contributes its shortest round-tripping digits (the ICU behaviour every browser shows), a BigInt
//! or a numeric string its exact digits.
//!
//! INTLCLDR (SR137): the patterns (affixes, the negative subpattern, primary / secondary grouping), symbols,
//! numbering-system digits, minimum grouping digits, compact patterns (by magnitude and plural category),
//! currency symbols / names / digits and unit patterns come from the CLDR 46 tables of `crate::cldr`.

use super::*;
use crate::bignum::BigUint;
use crate::cldr::L;
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
    /// The CLDR data the formatter reads (INTLCLDR SR137).
    pub loc: L,
    /// The numbering system (a numeric system of `cldr::NUMSYS`).
    pub ns: &'static str,
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

impl NumFmt {
    /// A plain decimal formatter of locale `loc` (RelativeTimeFormat's and DurationFormat's numbers).
    pub fn plain(loc: L, locale: &str, ns: &'static str, dg: Digits) -> NumFmt {
        NumFmt {
            locale: String::from(locale),
            loc,
            ns,
            style: Style::Decimal,
            currency: String::new(),
            currency_display: String::from("symbol"),
            currency_sign: String::from("standard"),
            unit: String::new(),
            unit_display: String::from("short"),
            dg,
            notation: Notation::Standard,
            compact_long: false,
            grouping: Some("auto"),
            sign: String::from("auto"),
        }
    }
}

fn is_sanctioned(u: &str) -> bool {
    !u.contains("-per-") && crate::cldr::unit_ids().any(|x| x == u)
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
    let nu = get_string_option(vm, o, "numberingSystem", &[], "Intl.NumberFormat")?;
    if let Some(ns) = &nu {
        if !ns.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid numberingSystem : {ns}"));
        }
    }
    let r = resolve_locale(&requested, &["nu"]);
    let (ns, kw_kept) = numbering_system(&r, nu.as_deref());
    let kept: Vec<(&str, &str)> = if kw_kept { vec![("nu", ns)] } else { Vec::new() };
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
        let cd = crate::cldr::currency_digits(&currency);
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
        loc: r.loc,
        ns,
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

// ---- CLDR number patterns (UTS #35 Part 3 §3): affixes, grouping sizes, the negative subpattern

/// One affix element of a number pattern.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Lit(String),
    Minus,
    Plus,
    Percent,
    PerMille,
    Currency,
}

#[derive(Clone, Debug)]
struct Sub {
    pre: Vec<Tok>,
    post: Vec<Tok>,
    /// Grouping sizes (0: no grouping).
    primary: usize,
    secondary: usize,
}

fn push_lit(v: &mut Vec<Tok>, c: char) {
    if let Some(Tok::Lit(s)) = v.last_mut() {
        s.push(c);
    } else {
        v.push(Tok::Lit(String::from(c)));
    }
}

fn parse_sub(p: &str) -> Sub {
    let mut pre = Vec::new();
    let mut post = Vec::new();
    let mut num = String::new();
    let mut state = 0; // 0 prefix, 1 number, 2 suffix
    let mut quoted = false;
    let cs: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        i += 1;
        if c == '\'' {
            if i < cs.len() && cs[i] == '\'' {
                i += 1;
                push_lit(if state == 0 { &mut pre } else { &mut post }, '\'');
            } else {
                quoted = !quoted;
            }
            continue;
        }
        let numeric = !quoted && (c == '#' || c == ',' || c == '.' || c == '@' || c.is_ascii_digit() || (state == 1 && (c == 'E' || (c == '+' && num.ends_with('E')))));
        if state < 2 && numeric {
            state = 1;
            num.push(c);
            continue;
        }
        if state == 1 {
            state = 2;
        }
        let v = if state == 0 { &mut pre } else { &mut post };
        if quoted {
            push_lit(v, c);
            continue;
        }
        match c {
            '-' => v.push(Tok::Minus),
            '+' => v.push(Tok::Plus),
            '%' => v.push(Tok::Percent),
            '‰' => v.push(Tok::PerMille),
            '¤' => {
                if v.last() != Some(&Tok::Currency) {
                    v.push(Tok::Currency);
                }
            }
            _ => push_lit(v, c),
        }
    }
    let int_part = num.split(['.', 'E']).next().unwrap_or("");
    let groups: Vec<&str> = int_part.split(',').collect();
    let (primary, secondary) = if groups.len() >= 2 {
        let p = groups[groups.len() - 1].len();
        let s = if groups.len() >= 3 { groups[groups.len() - 2].len() } else { p };
        (p, s)
    } else {
        (0, 0)
    };
    Sub { pre, post, primary, secondary }
}

/// The positive subpattern and the explicit negative one, if any.
fn parse_pattern(p: &str) -> (Sub, Option<Sub>) {
    // A ';' inside quotes is literal; CLDR's patterns never quote one, so split on the first bare ';'.
    match p.split_once(';') {
        Some((a, b)) => (parse_sub(a), Some(parse_sub(b))),
        None => (parse_sub(p), None),
    }
}

/// The affixes for a sign: "-" the negative subpattern (explicit, or minus + positive), "+" the negative one with
/// its minus replaced by plus (or plus + positive when the negative subpattern has no minus), "" the positive one.
fn affixes(pat: &str, sign: &str) -> Sub {
    let (pos, neg) = parse_pattern(pat);
    match sign {
        "-" => match neg {
            Some(n) => Sub { primary: pos.primary, secondary: pos.secondary, ..n },
            None => {
                let mut pre = vec![Tok::Minus];
                pre.extend(pos.pre.iter().cloned());
                Sub { pre, ..pos }
            }
        },
        "+" => match neg {
            Some(n) if n.pre.contains(&Tok::Minus) || n.post.contains(&Tok::Minus) => {
                let swap = |v: Vec<Tok>| v.into_iter().map(|t| if t == Tok::Minus { Tok::Plus } else { t }).collect::<Vec<_>>();
                Sub { pre: swap(n.pre), post: swap(n.post), primary: pos.primary, secondary: pos.secondary }
            }
            _ => {
                let mut pre = vec![Tok::Plus];
                pre.extend(pos.pre.iter().cloned());
                Sub { pre, ..pos }
            }
        },
        _ => pos,
    }
}

/// Symbol indexes of `cldr::L::symbol`.
const SYM_DECIMAL: usize = 0;
const SYM_GROUP: usize = 1;
const SYM_PERCENT: usize = 2;
const SYM_PLUS: usize = 3;
const SYM_MINUS: usize = 4;
const SYM_APPROX: usize = 5;
const SYM_EXP: usize = 6;
const SYM_PERMILLE: usize = 7;
const SYM_INFINITY: usize = 8;
const SYM_NAN: usize = 9;

/// ASCII digits written in numbering system `ns`.
pub fn localize_digits(s: &str, ns: &str) -> String {
    if ns == "latn" {
        return String::from(s);
    }
    let Some(d) = crate::cldr::digits(ns) else { return String::from(s) };
    let ds: Vec<char> = d.chars().collect();
    s.chars().map(|c| if c.is_ascii_digit() && ds.len() == 10 { ds[(c as u8 - b'0') as usize] } else { c }).collect()
}

fn exp_for_mag(nf: &NumFmt, mag: i32) -> i32 {
    match nf.notation {
        Notation::Standard => 0,
        Notation::Scientific => mag,
        Notation::Engineering => mag.div_euclid(3) * 3,
        Notation::Compact => {
            // ComputeExponentForMagnitude from the locale's compact patterns: a pattern of z zeros for 10^m
            // shows the number scaled by 10^(m - z + 1); "0" means no compaction.
            if mag < 3 {
                return 0;
            }
            let m = mag.min(14) as u32;
            let style = if nf.compact_long { 1 } else { 0 };
            match nf.loc.compact(style, m, 5) {
                Some(p) => {
                    let z = p.chars().filter(|c| *c == '0').count() as i32;
                    if z == 0 || p.trim() == "0" { 0 } else { m as i32 - z + 1 }
                }
                None => 0,
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

/// The integer digits with the locale's group separator: `primary` digits in the last group, `secondary` in the
/// others, and no grouping below `primary + minimum grouping digits` digits.
fn push_integer(parts: &mut Vec<(String, String)>, nf: &NumFmt, int: &str, primary: usize, secondary: usize) {
    let min = match nf.grouping {
        None => usize::MAX,
        Some("always") => 1,
        Some("min2") => (nf.loc.min_grouping() as usize).max(2),
        Some(_) => nf.loc.min_grouping() as usize,
    };
    let int_l = localize_digits(int, nf.ns);
    if primary == 0 || min == usize::MAX || int.len() < primary + min {
        push(parts, "integer", &int_l);
        return;
    }
    let chars: Vec<char> = int_l.chars().collect();
    let n = chars.len();
    let mut cuts: Vec<usize> = Vec::new();
    let mut at = n - primary;
    cuts.push(at);
    let sec = if secondary == 0 { primary } else { secondary };
    while at > sec {
        at -= sec;
        cuts.push(at);
    }
    cuts.reverse();
    let group = nf.loc.symbol(nf.ns, SYM_GROUP);
    let mut start = 0;
    for c in cuts {
        let s: String = chars[start..c].iter().collect();
        push(parts, "integer", &s);
        push(parts, "group", group);
        start = c;
    }
    let s: String = chars[start..].iter().collect();
    push(parts, "integer", &s);
}

/// The CLDR plural category of a formatted number (UTS #35 operands from its digits and compact exponent).
pub fn plural_of(nf: &NumFmt, int: &str, frac: &str, exp: u32) -> &'static str {
    crate::cldr::plural(nf.loc.lang(), false, &crate::cldr::Operands::new(int, frac, exp))
}

fn is_space(c: char) -> bool {
    c == ' ' || c == '\u{a0}' || c == '\u{202f}' || c == '\u{2009}'
}

/// Text around a number in a unit / compact / currency-name pattern: spaces as "literal", the rest as `kind`.
fn push_text(parts: &mut Vec<(String, String)>, text: &str, kind: &str) {
    let mut cur = String::new();
    let mut cur_space = false;
    for c in text.chars() {
        let sp = is_space(c);
        if !cur.is_empty() && sp != cur_space {
            push(parts, if cur_space { "literal" } else { kind }, &cur);
            cur.clear();
        }
        cur_space = sp;
        cur.push(c);
    }
    if !cur.is_empty() {
        push(parts, if cur_space { "literal" } else { kind }, &cur);
    }
}

/// A CLDR pattern with quotes resolved (`Mio'.'` → `Mio.`).
fn unquote(p: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\'' {
            if i + 1 < cs.len() && cs[i + 1] == '\'' {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

/// The display pattern of a unit at plural category `cat` ("{0}" is the number).
fn unit_pattern(nf: &NumFmt, cat: &str) -> String {
    let w = match nf.unit_display.as_str() {
        "long" => 0,
        "narrow" => 2,
        _ => 1,
    };
    let ci = crate::cldr::cat_index(cat);
    let pick = |u: &str| -> Option<&'static str> { nf.loc.unit(u, w, ci).or_else(|| nf.loc.unit(u, w, 5)) };
    let u = nf.unit.as_str();
    if let Some(p) = pick(u) {
        return String::from(p);
    }
    let (a, b) = u.split_once("-per-").unwrap_or((u, ""));
    let Some(pa) = pick(a) else { return String::from("{0}") };
    if let Some(per) = nf.loc.unit(b, w, 6) {
        return per.replace("{0}", pa);
    }
    // The compound pattern "{0}/{1}" with the denominator's singular name ({0} removed from its "one" pattern).
    let one = nf.loc.unit(b, w, 1).or_else(|| pick(b)).unwrap_or("");
    let name = one.replace("{0}", "");
    let name = name.trim_matches(is_space);
    let comp = nf.loc.s(crate::cldr::slot::UNITPER_LONG + w);
    let comp = if comp.is_empty() { "{0}/{1}" } else { comp };
    comp.replace("{0}", pa).replace("{1}", name)
}

/// The symbol a currency shows.
fn currency_symbol(nf: &NumFmt) -> String {
    match nf.currency_display.as_str() {
        "code" => nf.currency.clone(),
        "narrowSymbol" => String::from(nf.loc.currency(&nf.currency, 2).unwrap_or(&nf.currency)),
        _ => String::from(nf.loc.currency(&nf.currency, 1).unwrap_or(&nf.currency)),
    }
}

/// Expand affix elements into parts.
fn push_affix(out: &mut Vec<(String, String)>, nf: &NumFmt, toks: &[Tok], sym: &str) {
    for t in toks {
        match t {
            Tok::Lit(s) => push(out, "literal", s),
            Tok::Minus => push(out, "minusSign", nf.loc.symbol(nf.ns, SYM_MINUS)),
            Tok::Plus => push(out, "plusSign", nf.loc.symbol(nf.ns, SYM_PLUS)),
            Tok::Percent => push(out, "percentSign", nf.loc.symbol(nf.ns, SYM_PERCENT)),
            Tok::PerMille => push(out, "perMille", nf.loc.symbol(nf.ns, SYM_PERMILLE)),
            Tok::Currency => push(out, "currency", sym),
        }
    }
}

/// PartitionNumberPattern (§15.5.4) over the CLDR patterns, symbols and digits of the formatter's locale.
pub fn format_parts(nf: &NumFmt, mv: &Mv) -> Vec<(String, String)> {
    let loc = nf.loc;
    let mut num: Vec<(String, String)> = Vec::new();
    let (neg, zero, nan);
    let mut int_s = String::new();
    let mut frac_s = String::new();
    let mut exp_used: i32 = 0;
    let mut compact_pat: Option<String> = None;
    let currency_compact = nf.style == Style::Currency && nf.notation == Notation::Compact && nf.currency_display != "name" && !nf.compact_long;
    let pat_index = match nf.style {
        Style::Percent => 1,
        Style::Currency if nf.currency_display != "name" => {
            if nf.currency_sign == "accounting" {
                5
            } else {
                3
            }
        }
        _ => 0,
    };
    let mut pattern: &str = loc.number_pattern(nf.ns, pat_index);
    let sym = if nf.style == Style::Currency { currency_symbol(nf) } else { String::new() };
    if nf.style == Style::Currency && nf.currency_display != "name" {
        // The alphaNextToNumber variant when the symbol's letter would touch the digits.
        let (p, _) = parse_pattern(pattern);
        let before = p.pre.contains(&Tok::Currency);
        let touching = if before { sym.chars().last() } else { sym.chars().next() };
        if touching.map(|c| c.is_alphabetic()).unwrap_or(false) {
            pattern = loc.number_pattern(nf.ns, pat_index + 1);
        }
    }
    let (pos, _) = parse_pattern(pattern);
    let (primary, secondary) = (pos.primary, pos.secondary);
    match mv {
        Mv::NaN => {
            push(&mut num, "nan", loc.symbol(nf.ns, SYM_NAN));
            neg = false;
            zero = false;
            nan = true;
        }
        Mv::Inf(n) => {
            push(&mut num, "infinity", loc.symbol(nf.ns, SYM_INFINITY));
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
            push_integer(&mut num, nf, &int, primary, secondary);
            if !frac.is_empty() {
                push(&mut num, "decimal", loc.symbol(nf.ns, SYM_DECIMAL));
                push(&mut num, "fraction", &localize_digits(&frac, nf.ns));
            }
            match nf.notation {
                Notation::Scientific | Notation::Engineering => {
                    push(&mut num, "exponentSeparator", loc.symbol(nf.ns, SYM_EXP));
                    if exp < 0 {
                        push(&mut num, "exponentMinusSign", loc.symbol(nf.ns, SYM_MINUS));
                    }
                    push(&mut num, "exponentInteger", &localize_digits(&format!("{}", exp.unsigned_abs()), nf.ns));
                }
                Notation::Compact if exp > 0 => {
                    let cat = crate::cldr::cat_index(plural_of(nf, &int, &frac, exp as u32));
                    let key = (exp + int.len() as i32 - 1).clamp(3, 14) as u32;
                    let style = if currency_compact {
                        2
                    } else if nf.compact_long {
                        1
                    } else {
                        0
                    };
                    let p = loc.compact(style, key, cat).or_else(|| loc.compact(if nf.compact_long { 1 } else { 0 }, key, cat));
                    compact_pat = p.map(unquote);
                }
                _ => {}
            }
            int_s = int;
            frac_s = frac;
            exp_used = exp;
        }
    }
    // Compact affixes around the number ("0K", "0 Mio.", "¤0K").
    let mut compact_has_currency = false;
    if let Some(cp) = &compact_pat {
        let first = cp.find('0').unwrap_or(0);
        let last = cp.rfind('0').map(|i| i + 1).unwrap_or(cp.len());
        let (pre, post) = (&cp[..first], &cp[last..]);
        let mut wrapped: Vec<(String, String)> = Vec::new();
        for (text, before) in [(pre, true), (post, false)] {
            let mut ps: Vec<(String, String)> = Vec::new();
            for (k, seg) in text.split('¤').enumerate() {
                if k > 0 {
                    push(&mut ps, "currency", &sym);
                    compact_has_currency = true;
                }
                push_text(&mut ps, seg, "compact");
            }
            if before {
                wrapped.extend(ps);
                wrapped.append(&mut num);
            } else {
                wrapped.extend(ps);
            }
        }
        num = wrapped;
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
    let outer = if compact_has_currency { loc.number_pattern(nf.ns, 0) } else { pattern };
    let a = affixes(outer, sign);
    push_affix(&mut out, nf, &a.pre, &sym);
    out.extend(num);
    push_affix(&mut out, nf, &a.post, &sym);
    let cat = if nan || matches!(mv, Mv::Inf(_)) { "other" } else { plural_of(nf, &int_s, &frac_s, exp_used.max(0) as u32) };
    match nf.style {
        Style::Currency if nf.currency_display == "name" => {
            let ci = crate::cldr::cat_index(cat);
            let name = loc
                .currency(&nf.currency, 3 + ci)
                .or_else(|| loc.currency(&nf.currency, 8))
                .or_else(|| loc.currency(&nf.currency, 0))
                .map(String::from)
                .unwrap_or_else(|| nf.currency.clone());
            let up = loc.get(crate::cldr::slot::CURUNIT_ZERO + ci).or_else(|| loc.get(crate::cldr::slot::CURUNIT_ZERO + 5)).unwrap_or("{0} {1}");
            wrap(up, out, |ps: &mut Vec<(String, String)>, k: &str| {
                if k == "1" {
                    push(ps, "currency", &name);
                }
            })
        }
        Style::Unit => {
            let up = unit_pattern(nf, cat);
            let (pre, post) = up.split_once("{0}").unwrap_or(("", ""));
            let mut res = Vec::new();
            push_text(&mut res, pre, "unit");
            res.extend(out);
            push_text(&mut res, post, "unit");
            res
        }
        _ => out,
    }
}

/// Substitute the number parts for {0} in `pattern`, `other` for the remaining {n}, the text as literals.
fn wrap(pattern: &str, number: Vec<(String, String)>, mut other: impl FnMut(&mut Vec<(String, String)>, &str)) -> Vec<(String, String)> {
    let mut res = Vec::new();
    let mut rest = pattern;
    let mut number = Some(number);
    while let Some(open) = rest.find('{') {
        if open > 0 {
            push(&mut res, "literal", &rest[..open]);
        }
        let close = rest[open..].find('}').map(|c| open + c).unwrap_or(rest.len() - 1);
        let k = &rest[open + 1..close];
        if k == "0" {
            if let Some(n) = number.take() {
                res.extend(n);
            }
        } else {
            other(&mut res, k);
        }
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        push(&mut res, "literal", rest);
    }
    res
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
                parts.push((String::from("approximatelySign"), String::from(nf.loc.symbol(nf.ns, SYM_APPROX))));
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
    let mut props: Vec<(&str, Value)> = vec![("locale", Value::str(&nf.locale)), ("numberingSystem", Value::str(nf.ns))];
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
