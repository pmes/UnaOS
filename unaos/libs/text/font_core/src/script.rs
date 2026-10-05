//! Script property (Scripts.txt), Script_Extensions (ScriptExtensions.txt) and the script-run itemizer (UAX #24
//! resolution as shaping engines do it): Common and Inherited characters join the run around them, a character
//! whose Script_Extensions contain the current run's script stays in it, and a closing paired bracket takes the
//! script of its opening bracket. Codes are ISO 15924 (`*b"Arab"`); [`ot_script_tag`] maps them to OpenType tags.

use crate::ucd::{self, lookup};
use alloc::vec::Vec;

/// ISO 15924 code.
pub type Script = [u8; 4];
pub const COMMON: Script = *b"Zyyy";
pub const INHERITED: Script = *b"Zinh";
pub const UNKNOWN: Script = *b"Zzzz";

fn id(c: char) -> u8 {
    lookup(ucd::script::SCRIPT, c as u32).unwrap_or(0)
}

/// The Script property of `c`.
pub fn script(c: char) -> Script {
    ucd::script::SCRIPT_CODES[id(c) as usize]
}

/// Script_Extensions of `c` (its Script alone when it has no explicit extensions).
pub fn script_extensions(c: char) -> Vec<Script> {
    match lookup(ucd::script::SCRIPT_EXT, c as u32) {
        Some(set) => ucd::script::SCX_SETS[set as usize].iter().map(|&i| ucd::script::SCRIPT_CODES[i as usize]).collect(),
        None => alloc::vec![script(c)],
    }
}

fn is_real(s: Script) -> bool {
    s != COMMON && s != INHERITED && s != UNKNOWN
}

/// Split text into (byte start, byte end, script) runs. Text with no real script at all is one `Zyyy` run.
pub fn itemize(text: &str) -> Vec<(usize, usize, Script)> {
    let mut runs: Vec<(usize, usize, Script)> = Vec::new();
    let mut cur = COMMON;
    let mut start = 0usize;
    // Open brackets: (paired closing char, script at the opener).
    let mut brackets: Vec<(char, Script)> = Vec::new();
    for (i, c) in text.char_indices() {
        let mut sc = script(c);
        if let Some((pair, open)) = crate::bidi::paired_bracket(c) {
            if open {
                if brackets.len() < 64 {
                    brackets.push((pair, cur));
                }
            } else if let Some(p) = brackets.iter().rposition(|&(cl, _)| cl == c) {
                let s = brackets[p].1;
                brackets.truncate(p);
                if is_real(s) {
                    sc = s;
                }
            }
        }
        if !is_real(sc) {
            // Common / Inherited / Unknown: narrow by Script_Extensions when the run has no script yet.
            if !is_real(cur) {
                let ext = script_extensions(c);
                if ext.len() == 1 && is_real(ext[0]) {
                    cur = ext[0];
                }
            }
            continue;
        }
        if sc == cur {
            continue;
        }
        if !is_real(cur) {
            cur = sc; // leading neutrals join the first real run
            for b in brackets.iter_mut() {
                if !is_real(b.1) {
                    b.1 = sc;
                }
            }
            continue;
        }
        // A character whose extensions include the current script stays in the run.
        if lookup(ucd::script::SCRIPT_EXT, c as u32).is_some() && script_extensions(c).contains(&cur) {
            continue;
        }
        runs.push((start, i, cur));
        start = i;
        cur = sc;
    }
    if !text.is_empty() {
        runs.push((start, text.len(), cur));
    }
    runs
}

/// The OpenType script tag(s) for an ISO 15924 code, preferred first (the Indic "v2" tags before the old ones).
pub fn ot_script_tags(s: Script) -> Vec<[u8; 4]> {
    let v2 = |a: &[u8; 4], b: &[u8; 4]| alloc::vec![*a, *b];
    match &s {
        b"Zyyy" | b"Zinh" | b"Zzzz" => alloc::vec![*b"DFLT"],
        b"Deva" => v2(b"dev2", b"deva"),
        b"Beng" => v2(b"bng2", b"beng"),
        b"Guru" => v2(b"gur2", b"guru"),
        b"Gujr" => v2(b"gjr2", b"gujr"),
        b"Orya" => v2(b"ory2", b"orya"),
        b"Taml" => v2(b"tml2", b"taml"),
        b"Telu" => v2(b"tel2", b"telu"),
        b"Knda" => v2(b"knd2", b"knda"),
        b"Mlym" => v2(b"mlm2", b"mlym"),
        b"Mymr" => v2(b"mym2", b"mymr"),
        b"Hira" | b"Kana" => alloc::vec![*b"kana"],
        b"Laoo" => alloc::vec![*b"lao "],
        b"Yiii" => alloc::vec![*b"yi  "],
        b"Nkoo" => alloc::vec![*b"nko "],
        b"Vaii" => alloc::vec![*b"vai "],
        b"Hang" => alloc::vec![*b"hang"],
        _ => {
            let mut t = s;
            t[0] = t[0].to_ascii_lowercase();
            alloc::vec![t]
        }
    }
}

/// The first OpenType tag of [`ot_script_tags`].
pub fn ot_script_tag(s: Script) -> [u8; 4] {
    ot_script_tags(s)[0]
}

/// Whether the script is written right to left (its letters are Bidi_Class R or AL).
pub fn is_rtl(s: Script) -> bool {
    matches!(
        &s,
        b"Arab" | b"Hebr" | b"Syrc" | b"Thaa" | b"Nkoo" | b"Samr" | b"Mand" | b"Adlm" | b"Rohg" | b"Yezi" | b"Mend"
            | b"Phnx" | b"Armi" | b"Avst" | b"Hatr" | b"Khar" | b"Lydi" | b"Mani" | b"Narb" | b"Nbat" | b"Orkh"
            | b"Palm" | b"Phli" | b"Phlp" | b"Prti" | b"Sarb" | b"Sogd" | b"Sogo" | b"Elym" | b"Chrs" | b"Ougr"
            | b"Cprt" | b"Hung" | b"Gara" | b"Todr"
    )
}
