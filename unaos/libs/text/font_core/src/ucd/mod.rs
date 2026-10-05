//! Unicode Character Database 17.0.0 properties, generated into the crate by `oracle/gen_ucd.py` (data only; the
//! source files and their sha256 are named at the top of each table module and in `oracle/vectors.txt`).

#![allow(non_camel_case_types)]

pub(crate) mod bidi;
pub(crate) mod gc;
pub(crate) mod grapheme;
pub(crate) mod indic;
pub(crate) mod joining;
pub(crate) mod linebreak;
pub(crate) mod norm;
pub(crate) mod script;

/// Binary search a sorted, non-overlapping range table.
#[inline]
pub(crate) fn lookup<T: Copy>(t: &[(u32, u32, T)], cp: u32) -> Option<T> {
    let (mut lo, mut hi) = (0usize, t.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (s, e, v) = t[mid];
        if cp < s {
            hi = mid;
        } else if cp > e {
            lo = mid + 1;
        } else {
            return Some(v);
        }
    }
    None
}

#[inline]
pub(crate) fn in_set(t: &[(u32, u32)], cp: u32) -> bool {
    let (mut lo, mut hi) = (0usize, t.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (s, e) = t[mid];
        if cp < s {
            hi = mid;
        } else if cp > e {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// General_Category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gc {
    Lu, Ll, Lt, Lm, Lo, Mn, Mc, Me, Nd, Nl, No, Pc, Pd, Ps, Pe, Pi, Pf, Po, Sm, Sc, Sk, So, Zs, Zl, Zp, Cc, Cf, Cs, Co, Cn,
}

pub fn general_category(c: char) -> Gc {
    lookup(gc::GC, c as u32).unwrap_or(Gc::Cn)
}

/// Whether `c` is a combining mark (General_Category M*).
pub fn is_mark(c: char) -> bool {
    matches!(general_category(c), Gc::Mn | Gc::Mc | Gc::Me)
}

/// Grapheme_Cluster_Break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gcb {
    Other, CR, LF, Control, Extend, ZWJ, Regional_Indicator, Prepend, SpacingMark, L, V, T, LV, LVT,
}

/// Indic_Conjunct_Break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InCb {
    None, Linker, Consonant, Extend,
}

/// Joining_Type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jt {
    U, C, D, L, R, T,
}

pub fn joining_type(c: char) -> Jt {
    lookup(joining::JOINING, c as u32).unwrap_or(Jt::U)
}

/// Indic_Syllabic_Category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isc {
    Other, Avagraha, Bindu, Brahmi_Joining_Number, Cantillation_Mark, Consonant, Consonant_Dead, Consonant_Final,
    Consonant_Head_Letter, Consonant_Initial_Postfixed, Consonant_Killer, Consonant_Medial, Consonant_Placeholder,
    Consonant_Preceding_Repha, Consonant_Prefixed, Consonant_Subjoined, Consonant_Succeeding_Repha,
    Consonant_With_Stacker, Gemination_Mark, Invisible_Stacker, Joiner, Modifying_Letter, Non_Joiner, Nukta, Number,
    Number_Joiner, Pure_Killer, Register_Shifter, Reordering_Killer, Syllable_Modifier, Tone_Letter, Tone_Mark, Virama,
    Visarga, Vowel, Vowel_Dependent, Vowel_Independent,
}

/// Indic_Positional_Category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ipc {
    Not_Applicable, Bottom, Bottom_And_Left, Bottom_And_Right, Left, Left_And_Right, Overstruck, Right, Top,
    Top_And_Bottom, Top_And_Bottom_And_Left, Top_And_Bottom_And_Right, Top_And_Left, Top_And_Left_And_Right,
    Top_And_Right, Visual_Order_Left,
}

pub fn indic_syllabic_category(c: char) -> Isc {
    lookup(indic::ISC, c as u32).unwrap_or(Isc::Other)
}

pub fn indic_positional_category(c: char) -> Ipc {
    lookup(indic::IPC, c as u32).unwrap_or(Ipc::Not_Applicable)
}
