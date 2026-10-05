//! UAX #14 (Unicode Line Breaking Algorithm, Unicode 17.0) for Latin, Greek and Cyrillic text.
//!
//! The class table covers the ranges in [`table::RANGES`]; the rules implemented are LB1–LB25, LB28–LB30
//! and LB31 — every rule that can fire on characters from those ranges. The rules that only concern
//! scripts outside them (LB20 CB, LB23a ID/EB/EM, LB26/27 Hangul, LB28a Brahmic, LB30a RI, LB30b emoji,
//! the East Asian halves of LB19a/LB30) are owed with those scripts. `tests/linebreak.rs` runs the official
//! LineBreakTest-17.0.0.txt over every test line whose code points all fall inside the supported ranges.

mod table;

use alloc::vec::Vec;

/// Line_Break property values (all of Unicode 17.0's).
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lb {
    AI, AK, AL, AP, AS, B2, BA, BB, BK, CB, CJ, CL, CM, CP, CR, EB, EM, EX, GL, H2, H3, HH, HL, HY, ID, IN, IS, JL,
    JT, JV, LF, NL, NS, NU, OP, PO, PR, QU, RI, SA, SG, SP, SY, VF, VI, WJ, XX, ZW, ZWJ,
}

/// Raw Line_Break class of a code point (`XX` outside the supported ranges).
pub fn class(c: char) -> Lb {
    let cp = c as u32;
    let t = table::TABLE;
    let (mut lo, mut hi) = (0usize, t.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (s, e, k) = t[mid];
        if cp < s {
            hi = mid;
        } else if cp > e {
            lo = mid + 1;
        } else {
            return k;
        }
    }
    Lb::XX
}

/// Whether `c` lies inside the ranges this module's class table covers exactly.
pub fn supported(c: char) -> bool {
    let cp = c as u32;
    table::RANGES.iter().any(|&(a, b)| cp >= a && cp <= b)
}

/// Quotation marks with General_Category Pi / Pf (needed by LB15a/LB15b) inside the supported ranges.
fn is_pi(c: char) -> bool {
    matches!(c as u32, 0x00AB | 0x2018 | 0x201B | 0x201C | 0x201F | 0x2039)
}
fn is_pf(c: char) -> bool {
    matches!(c as u32, 0x00BB | 0x2019 | 0x201D | 0x203A)
}

/// LB1: resolve AI/XX (and SG/SA/CJ, absent here) to their default classes.
fn resolve(k: Lb) -> Lb {
    match k {
        Lb::AI | Lb::XX => Lb::AL,
        k => k,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Break {
    /// No break allowed before this position.
    None,
    /// A break opportunity.
    Allowed,
    /// A mandatory break (after BK/CR/LF/NL, and at end of text).
    Mandatory,
}

/// A "unit": a base character with its attached combining marks (LB9).
#[derive(Clone, Copy)]
struct Unit {
    cls: Lb,
    pi: bool,
    pf: bool,
    zwj_end: bool,
}

/// Break decisions for every char boundary: `out[i]` is the decision BEFORE char `i` (char index);
/// `out[0]` is `None` (LB2) and `out[n]` is `Mandatory` (LB3).
pub fn breaks(text: &str) -> Vec<Break> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = alloc::vec![Break::None; n + 1];
    if n == 0 {
        return out;
    }
    out[n] = Break::Mandatory;
    // LB9 attaches CM/ZWJ to the preceding base unless that is BK CR LF NL SP ZW; LB10 makes a lone one AL.
    let mut units: Vec<Unit> = Vec::with_capacity(n);
    let mut unit_start: Vec<usize> = Vec::with_capacity(n);
    for (i, &c) in chars.iter().enumerate() {
        let k = resolve(class(c));
        if matches!(k, Lb::CM | Lb::ZWJ) {
            if let Some(u) = units.last_mut() {
                if !matches!(u.cls, Lb::BK | Lb::CR | Lb::LF | Lb::NL | Lb::SP | Lb::ZW) {
                    u.zwj_end = k == Lb::ZWJ;
                    out[i] = Break::None;
                    continue;
                }
            }
            units.push(Unit { cls: Lb::AL, pi: false, pf: false, zwj_end: k == Lb::ZWJ });
            unit_start.push(i);
            continue;
        }
        units.push(Unit { cls: k, pi: k == Lb::QU && is_pi(c), pf: k == Lb::QU && is_pf(c), zwj_end: false });
        unit_start.push(i);
    }
    for j in 1..units.len() {
        out[unit_start[j]] = decide(&units, j);
    }
    out
}

fn decide(u: &[Unit], j: usize) -> Break {
    use Lb::*;
    let a = u[j - 1].cls;
    let b = u[j].cls;
    let cls = |k: usize| u[k].cls;
    // The last unit at or before `from` that is not SP.
    let before_spaces = |from: usize| -> Option<usize> {
        let mut k = from as isize;
        while k >= 0 && cls(k as usize) == SP {
            k -= 1;
        }
        if k < 0 { None } else { Some(k as usize) }
    };
    // LB4, LB5
    if a == BK {
        return Break::Mandatory;
    }
    if a == CR && b == LF {
        return Break::None;
    }
    if matches!(a, CR | LF | NL) {
        return Break::Mandatory;
    }
    // LB6, LB7
    if matches!(b, BK | CR | LF | NL | SP | ZW) {
        return Break::None;
    }
    // LB8: ZW SP* ÷
    let bs = before_spaces(j - 1);
    if let Some(k) = bs {
        if cls(k) == ZW {
            return Break::Allowed;
        }
    }
    // LB8a: ZWJ ×
    if u[j - 1].zwj_end {
        return Break::None;
    }
    // LB11
    if a == WJ || b == WJ {
        return Break::None;
    }
    // LB12, LB12a
    if a == GL {
        return Break::None;
    }
    if b == GL && !matches!(a, SP | BA | HY | HH) {
        return Break::None;
    }
    // LB13
    if matches!(b, CL | CP | EX | SY) {
        return Break::None;
    }
    // LB14: OP SP* ×
    if let Some(k) = bs {
        if cls(k) == OP {
            return Break::None;
        }
    }
    // LB15a: (sot | BK | CR | LF | NL | OP | QU | GL | SP | ZW) [Pi&QU] SP* ×
    if let Some(k) = bs {
        if u[k].pi && (k == 0 || matches!(cls(k - 1), BK | CR | LF | NL | OP | QU | GL | SP | ZW)) {
            return Break::None;
        }
    }
    // LB15b: × [Pf&QU] (SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW | eot)
    if u[j].pf
        && (j + 1 == u.len()
            || matches!(cls(j + 1), SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW))
    {
        return Break::None;
    }
    // LB15c: SP ÷ IS NU
    if a == SP && b == IS && j + 1 < u.len() && cls(j + 1) == NU {
        return Break::Allowed;
    }
    // LB15d: × IS
    if b == IS {
        return Break::None;
    }
    // LB16: (CL | CP) SP* × NS
    if b == NS {
        if let Some(k) = bs {
            if matches!(cls(k), CL | CP) {
                return Break::None;
            }
        }
    }
    // LB17: B2 SP* × B2
    if b == B2 {
        if let Some(k) = bs {
            if cls(k) == B2 {
                return Break::None;
            }
        }
    }
    // LB18
    if a == SP {
        return Break::Allowed;
    }
    // LB19 / LB19a (every supported character is non-East-Asian, so QU binds both ways).
    if b == QU || a == QU {
        return Break::None;
    }
    // LB20a: (sot | BK | CR | LF | NL | SP | ZW | CB | GL) (HY | HH) × (AL | HL)
    if matches!(a, HY | HH)
        && matches!(b, AL | HL)
        && (j == 1 || matches!(cls(j - 2), BK | CR | LF | NL | SP | ZW | GL))
    {
        return Break::None;
    }
    // LB21
    if matches!(b, BA | HH | HY | NS) || a == BB {
        return Break::None;
    }
    // LB21a: HL (HY | HH) × [^HL]
    if matches!(a, HY | HH) && j >= 2 && cls(j - 2) == HL && b != HL {
        return Break::None;
    }
    // LB21b
    if a == SY && b == HL {
        return Break::None;
    }
    // LB22
    if b == IN {
        return Break::None;
    }
    // LB23
    if (matches!(a, AL | HL) && b == NU) || (a == NU && matches!(b, AL | HL)) {
        return Break::None;
    }
    // LB24
    if (matches!(a, PR | PO) && matches!(b, AL | HL)) || (matches!(a, AL | HL) && matches!(b, PR | PO)) {
        return Break::None;
    }
    // LB25 (Unicode 15.1+):
    //   (PR | PO) × (OP | HY)? IS? NU ;  (OP | HY) × IS? NU ;  IS × NU
    //   NU (NU | SY | IS)* × (NU | SY | IS | CL | CP)
    //   NU (NU | SY | IS)* (CL | CP)? × (PO | PR)
    let nu_at = |k: usize| k < u.len() && cls(k) == NU;
    let nu_ahead = |k: usize| nu_at(k) || (k < u.len() && cls(k) == IS && nu_at(k + 1));
    if matches!(a, PR | PO) && (nu_ahead(j) || (matches!(b, OP | HY) && nu_ahead(j + 1))) {
        return Break::None;
    }
    if matches!(a, OP | HY) && nu_ahead(j) {
        return Break::None;
    }
    if a == IS && b == NU {
        return Break::None;
    }
    let num_before = |end: usize| -> bool {
        let mut k = end as isize;
        while k >= 0 && matches!(cls(k as usize), SY | IS) {
            k -= 1;
        }
        k >= 0 && cls(k as usize) == NU
    };
    if matches!(b, NU | SY | IS | CL | CP) && num_before(j - 1) {
        return Break::None;
    }
    if matches!(b, PO | PR) && (num_before(j - 1) || (matches!(a, CL | CP) && j >= 2 && num_before(j - 2))) {
        return Break::None;
    }
    // LB28
    if matches!(a, AL | HL) && matches!(b, AL | HL) {
        return Break::None;
    }
    // LB29
    if a == IS && matches!(b, AL | HL) {
        return Break::None;
    }
    // LB30 (no East Asian OP/CP in the supported ranges)
    if matches!(a, AL | HL | NU) && b == OP {
        return Break::None;
    }
    if a == CP && matches!(b, AL | HL | NU) {
        return Break::None;
    }
    Break::Allowed // LB31
}
