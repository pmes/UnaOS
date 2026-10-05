//! CSSOM §2.1 serialization of identifiers and strings, and component values back to CSS text.

use alloc::string::String;

use crate::parser::{BlockKind, CV};
use crate::tokenizer::{Num, Token};

/// "serialize an identifier"
pub fn identifier(s: &str, out: &mut String) {
    let chars: alloc::vec::Vec<char> = s.chars().collect();
    if chars.len() == 1 && chars[0] == '-' {
        out.push_str("\\-");
        return;
    }
    for (i, &c) in chars.iter().enumerate() {
        let u = c as u32;
        if u == 0 {
            out.push('\u{FFFD}');
        } else if (1..=0x1F).contains(&u) || u == 0x7F || (i == 0 && c.is_ascii_digit()) || (i == 1 && c.is_ascii_digit() && chars[0] == '-') {
            out.push_str(&alloc::format!("\\{:x} ", u));
        } else if u >= 0x80 || c == '-' || c == '_' || c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('\\');
            out.push(c);
        }
    }
}

/// "serialize a string" (double quotes)
pub fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        let u = c as u32;
        if u == 0 {
            out.push('\u{FFFD}');
        } else if (1..=0x1F).contains(&u) || u == 0x7F {
            out.push_str(&alloc::format!("\\{:x} ", u));
        } else if c == '"' || c == '\\' {
            out.push('\\');
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out.push('"');
}

/// A number as CSS text (shortest round-trip form, no exponent for ordinary magnitudes).
pub fn number(n: &Num, out: &mut String) {
    number_f64(n.value, out)
}

pub fn number_f64(v: f64, out: &mut String) {
    if v == (v as i64) as f64 && v.abs() < 1e15 {
        out.push_str(&alloc::format!("{}", v as i64));
    } else {
        out.push_str(&alloc::format!("{}", v));
    }
}

pub fn token(t: &Token, out: &mut String) {
    match t {
        Token::Ident(s) => identifier(s, out),
        Token::Function(s) => {
            identifier(s, out);
            out.push('(');
        }
        Token::AtKeyword(s) => {
            out.push('@');
            identifier(s, out);
        }
        Token::Hash { value, .. } => {
            out.push('#');
            for c in value.chars() {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c as u32 >= 0x80 {
                    out.push(c);
                } else {
                    out.push('\\');
                    out.push(c);
                }
            }
        }
        Token::String(s) => string(s, out),
        Token::BadString => out.push_str("\"\n"),
        Token::Url(s) => {
            out.push_str("url(");
            string(s, out);
            out.push(')');
        }
        Token::BadUrl => out.push_str("url(\u{FFFD})"),
        Token::Delim(c) => out.push(*c),
        Token::Number(n) => number(n, out),
        Token::Percentage(n) => {
            number(n, out);
            out.push('%');
        }
        Token::Dimension(n, u) => {
            number(n, out);
            // a unit starting with e/E followed by a digit or sign would re-tokenize as an exponent
            if u.starts_with(['e', 'E']) {
                out.push_str("\\65 ");
                identifier(&u[1..], out);
            } else {
                identifier(u, out);
            }
        }
        Token::Whitespace => out.push(' '),
        Token::Cdo => out.push_str("<!--"),
        Token::Cdc => out.push_str("-->"),
        Token::Colon => out.push(':'),
        Token::Semicolon => out.push(';'),
        Token::Comma => out.push(','),
        Token::LeftBracket => out.push('['),
        Token::RightBracket => out.push(']'),
        Token::LeftParen => out.push('('),
        Token::RightParen => out.push(')'),
        Token::LeftBrace => out.push('{'),
        Token::RightBrace => out.push('}'),
        Token::IncludeMatch => out.push_str("~="),
        Token::DashMatch => out.push_str("|="),
        Token::PrefixMatch => out.push_str("^="),
        Token::SuffixMatch => out.push_str("$="),
        Token::SubstringMatch => out.push_str("*="),
    }
}

/// Component values back to CSS text.
pub fn to_css(v: &[CV]) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

fn write(v: &[CV], out: &mut String) {
    for c in v {
        match c {
            CV::Token(t) => token(t, out),
            CV::Function(n, args) => {
                identifier(n, out);
                out.push('(');
                write(args, out);
                out.push(')');
            }
            CV::Block(k, inner) => {
                let (a, b) = match k {
                    BlockKind::Paren => ('(', ')'),
                    BlockKind::Bracket => ('[', ']'),
                    BlockKind::Brace => ('{', '}'),
                };
                out.push(a);
                write(inner, out);
                out.push(b);
            }
        }
    }
}
