//! UAX #14 (Unicode Line Breaking Algorithm, Unicode 17.0.0), every class and every rule LB1–LB31 including
//! LB15a–d, LB19/LB19a with East_Asian_Width, LB20 CB, LB20a/LB21a (HY/HH), LB23a, LB25, LB26/27 (Hangul),
//! LB28a (Brahmic orthographic syllables, U+25CC), LB30 (non-East-Asian OP/CP), LB30a (regional indicators) and
//! LB30b (emoji modifiers, unassigned Extended_Pictographic). Line_Break and East_Asian_Width come from the UCD
//! (`ucd::linebreak`, generated); SA is resolved by LB1 (no dictionary segmentation for Thai/Lao/Khmer/Myanmar
//! — the conformance file assumes none). `tests/linebreak.rs` runs LineBreakTest-17.0.0.txt in full.

use crate::grapheme::is_extended_pictographic;
use crate::ucd::{self, general_category, in_set, lookup, Gc};

use alloc::vec::Vec;

/// Line_Break property values (all of Unicode 17.0's).
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lb {
    AI, AK, AL, AP, AS, B2, BA, BB, BK, CB, CJ, CL, CM, CP, CR, EB, EM, EX, GL, H2, H3, HH, HL, HY, ID, IN, IS, JL,
    JT, JV, LF, NL, NS, NU, OP, PO, PR, QU, RI, SA, SG, SP, SY, VF, VI, WJ, XX, ZW, ZWJ,
}

/// Raw Line_Break class of a code point (LineBreak-17.0.0.txt; unlisted code points are `XX`).
pub fn class(c: char) -> Lb {
    lookup(ucd::linebreak::LINE_BREAK, c as u32).unwrap_or(Lb::XX)
}

/// Every code point is covered since FONTBIDI (kept for callers of the FONTCORE-era API).
pub fn supported(_c: char) -> bool {
    true
}

/// East_Asian_Width F, W or H (`$EastAsian` in LB19a and LB30).
pub fn east_asian(c: char) -> bool {
    in_set(ucd::linebreak::EAST_ASIAN_FWH, c as u32)
}

/// LB1: resolve AI, SG, XX to AL; SA to CM (General_Category Mn/Mc) or AL; CJ to NS.
fn resolve(k: Lb, c: char) -> Lb {
    match k {
        Lb::AI | Lb::SG | Lb::XX => Lb::AL,
        Lb::SA => {
            if matches!(general_category(c), Gc::Mn | Gc::Mc) {
                Lb::CM
            } else {
                Lb::AL
            }
        }
        Lb::CJ => Lb::NS,
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

/// A "unit": a base character with its attached combining marks (LB9), carrying the base's properties.
#[derive(Clone, Copy)]
struct Unit {
    cls: Lb,
    pi: bool,
    pf: bool,
    /// East_Asian_Width F/W/H.
    ea: bool,
    /// U+25CC DOTTED CIRCLE (LB28a).
    dotted: bool,
    /// Extended_Pictographic and unassigned (LB30b).
    pict_cn: bool,
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
        let k = resolve(class(c), c);
        if matches!(k, Lb::CM | Lb::ZWJ) {
            if let Some(u) = units.last_mut() {
                if !matches!(u.cls, Lb::BK | Lb::CR | Lb::LF | Lb::NL | Lb::SP | Lb::ZW) {
                    u.zwj_end = k == Lb::ZWJ;
                    out[i] = Break::None;
                    continue;
                }
            }
            units.push(Unit {
                cls: Lb::AL,
                pi: false,
                pf: false,
                ea: east_asian(c),
                dotted: false,
                pict_cn: false,
                zwj_end: k == Lb::ZWJ,
            });
            unit_start.push(i);
            continue;
        }
        let gc = general_category(c);
        units.push(Unit {
            cls: k,
            pi: k == Lb::QU && gc == Gc::Pi,
            pf: k == Lb::QU && gc == Gc::Pf,
            ea: east_asian(c),
            dotted: c == '\u{25CC}',
            pict_cn: gc == Gc::Cn && is_extended_pictographic(c),
            zwj_end: false,
        });
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
    let n = u.len();
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
        && (j + 1 == n || matches!(cls(j + 1), SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW))
    {
        return Break::None;
    }
    // LB15c: SP ÷ IS NU
    if a == SP && b == IS && j + 1 < n && cls(j + 1) == NU {
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
    // LB19: × [QU - Pi] ; [QU - Pf] ×
    if b == QU && !u[j].pi {
        return Break::None;
    }
    if a == QU && !u[j - 1].pf {
        return Break::None;
    }
    // LB19a: [^EA] × QU ; × QU ([^EA] | eot) ; QU × [^EA] ; (sot | [^EA]) QU ×
    if b == QU && (!u[j - 1].ea || j + 1 == n || !u[j + 1].ea) {
        return Break::None;
    }
    if a == QU && (!u[j].ea || j == 1 || !u[j - 2].ea) {
        return Break::None;
    }
    // LB20: ÷ CB ; CB ÷
    if a == CB || b == CB {
        return Break::Allowed;
    }
    // LB20a: (sot | BK | CR | LF | NL | SP | ZW | CB | GL) (HY | HH) × (AL | HL)
    if matches!(a, HY | HH)
        && matches!(b, AL | HL)
        && (j == 1 || matches!(cls(j - 2), BK | CR | LF | NL | SP | ZW | CB | GL))
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
    // LB23a
    if (a == PR && matches!(b, ID | EB | EM)) || (matches!(a, ID | EB | EM) && b == PO) {
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
    let nu_at = |k: usize| k < n && cls(k) == NU;
    let nu_ahead = |k: usize| nu_at(k) || (k < n && cls(k) == IS && nu_at(k + 1));
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
    // LB26
    if a == JL && matches!(b, JL | JV | H2 | H3) {
        return Break::None;
    }
    if matches!(a, JV | H2) && matches!(b, JV | JT) {
        return Break::None;
    }
    if matches!(a, JT | H3) && b == JT {
        return Break::None;
    }
    // LB27
    if (matches!(a, JL | JV | JT | H2 | H3) && b == PO) || (a == PR && matches!(b, JL | JV | JT | H2 | H3)) {
        return Break::None;
    }
    // LB28
    if matches!(a, AL | HL) && matches!(b, AL | HL) {
        return Break::None;
    }
    // LB28a: AP × (AK | ◌ | AS) ; (AK | ◌ | AS) × (VF | VI) ; (AK | ◌ | AS) VI × (AK | ◌) ;
    //        (AK | ◌ | AS) × (AK | ◌ | AS) VF
    let ak = |k: usize| matches!(cls(k), AK | AS) || u[k].dotted;
    let ak_nos = |k: usize| cls(k) == AK || u[k].dotted;
    if a == AP && ak(j) {
        return Break::None;
    }
    if ak(j - 1) && matches!(b, VF | VI) {
        return Break::None;
    }
    if a == VI && j >= 2 && ak(j - 2) && ak_nos(j) {
        return Break::None;
    }
    if ak(j - 1) && ak(j) && j + 1 < n && cls(j + 1) == VF {
        return Break::None;
    }
    // LB29
    if a == IS && matches!(b, AL | HL) {
        return Break::None;
    }
    // LB30: (AL | HL | NU) × [OP - EA] ; [CP - EA] × (AL | HL | NU)
    if matches!(a, AL | HL | NU) && b == OP && !u[j].ea {
        return Break::None;
    }
    if a == CP && !u[j - 1].ea && matches!(b, AL | HL | NU) {
        return Break::None;
    }
    // LB30a: an odd number of RI before × RI
    if a == RI && b == RI {
        let mut k = j;
        let mut cnt = 0;
        while k > 0 && cls(k - 1) == RI {
            cnt += 1;
            k -= 1;
        }
        if cnt % 2 == 1 {
            return Break::None;
        }
    }
    // LB30b: EB × EM ; [ExtPict & Cn] × EM
    if b == EM && (a == EB || u[j - 1].pict_cn) {
        return Break::None;
    }
    Break::Allowed // LB31
}
