//! CSS Syntax Level 3, §7.1: the `<urange>` production (`@font-face` `unicode-range`).
//!
//! The grammar is matched over tokens, then the source text of the tokens after `u` is re-read as
//! `+<hex>[?…][-<hex>]`, exactly as the specification describes.

use alloc::vec::Vec;

use crate::tokenizer::{Token, Tokenizer};

/// Parse a comma-separated list of `<urange>`; each item is `Some((start, end))` or `None` when invalid.
pub fn parse_urange_list(input: &str) -> Vec<Option<(u32, u32)>> {
    let mut tz = Tokenizer::new(input);
    let mut toks = Vec::new();
    while let Some(t) = tz.next_token() {
        toks.push(t);
    }
    let src = tz.source().to_vec();
    let mut out = Vec::new();
    for item in toks.split(|(t, _, _)| *t == Token::Comma) {
        out.push(parse_one(item, &src));
    }
    out
}

fn parse_one(item: &[(Token, usize, usize)], src: &[char]) -> Option<(u32, u32)> {
    let mut a = 0;
    let mut b = item.len();
    while a < b && item[a].0 == Token::Whitespace {
        a += 1;
    }
    while b > a && item[b - 1].0 == Token::Whitespace {
        b -= 1;
    }
    let t = &item[a..b];
    match t.first() {
        Some((Token::Ident(u), _, _)) if u.eq_ignore_ascii_case("u") => {}
        _ => return None,
    }
    let rest = &t[1..];
    let q = |r: &[(Token, usize, usize)]| r.iter().all(|(t, _, _)| *t == Token::Delim('?'));
    let ok = match rest {
        [(Token::Delim('+'), _, _), (Token::Ident(_), _, _), tail @ ..] => q(tail),
        [(Token::Delim('+'), _, _), tail @ ..] => !tail.is_empty() && q(tail),
        [(Token::Dimension(..), _, _), tail @ ..] => q(tail),
        [(Token::Number(_), _, _), (Token::Dimension(..), _, _)] => true,
        [(Token::Number(_), _, _), (Token::Number(_), _, _)] => true,
        [(Token::Number(_), _, _), tail @ ..] => q(tail),
        _ => false,
    };
    if !ok {
        return None;
    }
    let text: Vec<char> = src[t[0].2..t[t.len() - 1].2].to_vec();
    concatenated(&text)
}

fn concatenated(text: &[char]) -> Option<(u32, u32)> {
    let mut i = 0;
    if text.first() != Some(&'+') {
        return None;
    }
    i += 1;
    let hs = i;
    while i < text.len() && text[i].is_ascii_hexdigit() {
        i += 1;
    }
    let he = i;
    while i < text.len() && text[i] == '?' {
        i += 1;
    }
    if i - hs > 6 || i == hs {
        return None;
    }
    let hex = |s: &[char], fill: Option<char>| -> u32 {
        s.iter().fold(0u32, |v, &c| {
            let c = if c == '?' { fill.unwrap() } else { c };
            v * 16 + c.to_digit(16).unwrap()
        })
    };
    let (start, end) = if i > he {
        if i != text.len() {
            return None;
        }
        (hex(&text[hs..i], Some('0')), hex(&text[hs..i], Some('F')))
    } else {
        let start = hex(&text[hs..he], None);
        if i == text.len() {
            (start, start)
        } else {
            if text[i] != '-' {
                return None;
            }
            i += 1;
            let es = i;
            while i < text.len() && text[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i == es || i - es > 6 || i != text.len() {
                return None;
            }
            (start, hex(&text[es..i], None))
        }
    };
    if end > 0x10FFFF || start > end {
        return None;
    }
    Some((start, end))
}
