//! CSS Syntax Level 3, §6: the An+B microsyntax, parsed over component values (§6.2).

use crate::parser::{trim_ws, CV};
use crate::tokenizer::Token;

fn signless_int(c: Option<&CV>) -> Option<i32> {
    match c {
        Some(CV::Token(Token::Number(n))) if n.is_integer && !n.has_sign => Some(n.int()),
        _ => None,
    }
}
fn signed_int(c: Option<&CV>) -> Option<i32> {
    match c {
        Some(CV::Token(Token::Number(n))) if n.is_integer && n.has_sign => Some(n.int()),
        _ => None,
    }
}

/// `n-<digits>` (case-insensitive `n`) → the digits as a negative B.
fn ndashdigits(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.len() < 3 || !(b[0] == b'n' || b[0] == b'N') || b[1] != b'-' || !b[2..].iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut v: i64 = 0;
    for d in &b[2..] {
        v = (v * 10 + (d - b'0') as i64).min(i32::MAX as i64);
    }
    Some(-(v as i32))
}

/// After an `An` part: optionally ` <signed-integer>`, `['+'|'-'] <signless-integer>`, or nothing.
fn rest_b(v: &[CV], mut i: usize, a: i32) -> Option<(i32, i32)> {
    let skip = |i: &mut usize| {
        while *i < v.len() && v[*i].is_whitespace() {
            *i += 1;
        }
    };
    skip(&mut i);
    if i >= v.len() {
        return Some((a, 0));
    }
    if let Some(b) = signed_int(v.get(i)) {
        i += 1;
        skip(&mut i);
        return if i == v.len() { Some((a, b)) } else { None };
    }
    let sign = if v[i].is_delim('+') {
        1
    } else if v[i].is_delim('-') {
        -1
    } else {
        return None;
    };
    i += 1;
    skip(&mut i);
    let b = signless_int(v.get(i))?;
    i += 1;
    skip(&mut i);
    if i == v.len() { Some((a, sign * b)) } else { None }
}

/// Parse `<an+b>` from component values (leading / trailing whitespace allowed). `(A, B)` or `None`.
pub fn parse_anb(cvs: &[CV]) -> Option<(i32, i32)> {
    let v = trim_ws(cvs);
    let first = v.first()?;
    let only = v.len() == 1;
    match first {
        CV::Token(Token::Ident(s)) => {
            let l = ascii_lower(s);
            match l.as_str() {
                "odd" if only => Some((2, 1)),
                "even" if only => Some((2, 0)),
                _ => ident_form(&l, v, 1, 1),
            }
        }
        CV::Token(Token::Number(n)) if n.is_integer && only => Some((0, n.int())),
        CV::Token(Token::Dimension(n, unit)) if n.is_integer => {
            let u = ascii_lower(unit);
            let a = n.int();
            if u == "n" {
                rest_b(v, 1, a)
            } else if u == "n-" {
                // <ndash-dimension> <signless-integer>
                let mut i = 1;
                while i < v.len() && v[i].is_whitespace() {
                    i += 1;
                }
                let b = signless_int(v.get(i))?;
                if i + 1 == v.len() { Some((a, -b)) } else { None }
            } else if let Some(b) = ndashdigits(&u) {
                if only { Some((a, b)) } else { None }
            } else {
                None
            }
        }
        CV::Token(Token::Delim('+')) => match v.get(1) {
            // '+' immediately followed by an n-ident (no whitespace between).
            Some(CV::Token(Token::Ident(s))) => {
                let l = ascii_lower(s);
                if l.starts_with('-') {
                    return None;
                }
                ident_form(&l, v, 2, 1)
            }
            _ => None,
        },
        _ => None,
    }
}

/// The ident forms: `n`, `-n`, `n-`, `-n-`, `n-<digits>`, `-n-<digits>`, each with a following B part.
/// `l` is the lowercased ident, `next` the index after it, `sign` the A sign carried by a leading `+`.
fn ident_form(l: &str, v: &[CV], next: usize, sign: i32) -> Option<(i32, i32)> {
    let (a, rest) = if let Some(r) = l.strip_prefix('-') { (-sign, r) } else { (sign, l) };
    match rest {
        "n" => rest_b(v, next, a),
        "n-" => {
            let mut i = next;
            while i < v.len() && v[i].is_whitespace() {
                i += 1;
            }
            let b = signless_int(v.get(i))?;
            if i + 1 == v.len() { Some((a, -b)) } else { None }
        }
        r => {
            let b = ndashdigits(r)?;
            if next == v.len() { Some((a, b)) } else { None }
        }
    }
}

fn ascii_lower(s: &str) -> alloc::string::String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

/// `true` when the 1-based `index` is matched by An+B (some n ≥ 0 with A·n + B = index).
pub fn anb_matches(a: i32, b: i32, index: i32) -> bool {
    let (a, b, index) = (a as i64, b as i64, index as i64);
    if a == 0 {
        index == b
    } else {
        let d = index - b;
        d % a == 0 && d / a >= 0
    }
}
