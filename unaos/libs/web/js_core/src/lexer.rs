//! ECMAScript lexical grammar (ECMA-262 §12): source text as UTF-16 code units, tokens on demand. The parser
//! drives goal-symbol choice: it asks for a regular expression or a template continuation where the
//! syntactic grammar allows one (§12 "InputElementRegExp" / "InputElementTemplateTail").

use crate::numconv;
use crate::string::{push_code_point, JsStr};
use crate::unicode;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

pub type Atom = Rc<str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P {
    LBrace, RBrace, LParen, RParen, LBracket, RBracket, Dot, Ellipsis, Semi, Comma, Lt, Gt, Le, Ge, Eq2, Ne,
    Eq3, Ne2, Plus, Minus, Star, Percent, Star2, Inc, Dec, Shl, Shr, UShr, Amp, Pipe, Caret, Bang, Tilde,
    And, Or, Nullish, Question, QDot, Colon, Assign, PlusEq, MinusEq, StarEq, PercentEq, Star2Eq, ShlEq,
    ShrEq, UShrEq, AmpEq, PipeEq, CaretEq, AndEq, OrEq, NullishEq, Arrow, Slash, SlashEq, At,
}

#[derive(Clone, Debug, PartialEq)]
pub enum T {
    Eof,
    /// IdentifierName (identifiers, keywords, contextual keywords). `escaped` marks a unicode escape.
    Name(Atom),
    Private(Atom),
    Num(f64),
    BigInt(Atom),
    Str(JsStr),
    /// Template piece. `cooked` is None for an invalid escape (allowed only in tagged templates).
    Template { cooked: Option<JsStr>, raw: JsStr, tail: bool },
    Regex { body: JsStr, flags: JsStr },
    Punct(P),
}

#[derive(Clone, Debug)]
pub struct Token {
    pub t: T,
    pub start: u32,
    pub end: u32,
    pub nl_before: bool,
    pub escaped: bool,
    /// Position of a legacy octal literal / octal or \8 \9 escape (a strict-mode early error).
    pub legacy_octal: bool,
}

#[derive(Clone, Debug)]
pub struct LexError {
    pub pos: u32,
    pub msg: String,
}

pub struct Lexer {
    pub src: Rc<[u16]>,
    pub pos: usize,
    pub module: bool,
    pub nl_before: bool,
    /// True at the beginning of the input or after a line terminator (for `-->` HTML close comments).
    line_start: bool,
}

fn is_lt(c: u32) -> bool {
    unicode::is_line_terminator(c)
}

impl Lexer {
    pub fn new(src: Rc<[u16]>, module: bool) -> Lexer {
        let mut l = Lexer { src, pos: 0, module, nl_before: false, line_start: true };
        // Hashbang comment (§12.5).
        if l.src.len() >= 2 && l.src[0] == b'#' as u16 && l.src[1] == b'!' as u16 {
            while l.pos < l.src.len() && !is_lt(l.src[l.pos] as u32) {
                l.pos += 1;
            }
        }
        l
    }

    fn peek_cp(&self) -> Option<(u32, usize)> {
        if self.pos >= self.src.len() {
            return None;
        }
        Some(crate::string::code_point_at(&self.src, self.pos))
    }
    fn at(&self, off: usize) -> u32 {
        self.src.get(self.pos + off).map(|&c| c as u32).unwrap_or(u32::MAX)
    }
    fn err<T>(&self, pos: usize, msg: &str) -> Result<T, LexError> {
        Err(LexError { pos: pos as u32, msg: String::from(msg) })
    }

    /// Skip whitespace and comments; sets nl_before.
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            let c = self.at(0);
            if c == u32::MAX {
                return Ok(());
            }
            if is_lt(c) {
                self.pos += 1;
                self.nl_before = true;
                self.line_start = true;
                continue;
            }
            if unicode::is_js_whitespace(c) {
                self.pos += 1;
                continue;
            }
            if c == b'/' as u32 {
                let d = self.at(1);
                if d == b'/' as u32 {
                    self.skip_line_comment();
                    continue;
                }
                if d == b'*' as u32 {
                    let start = self.pos;
                    self.pos += 2;
                    let mut closed = false;
                    while self.pos < self.src.len() {
                        let e = self.src[self.pos] as u32;
                        if e == b'*' as u32 && self.at(1) == b'/' as u32 {
                            self.pos += 2;
                            closed = true;
                            break;
                        }
                        if is_lt(e) {
                            self.nl_before = true;
                            self.line_start = true;
                        }
                        self.pos += 1;
                    }
                    if !closed {
                        return self.err(start, "unterminated comment");
                    }
                    // Annex B: a multi-line comment containing a line terminator followed by `-->` is a comment.
                    continue;
                }
                return Ok(());
            }
            if !self.module {
                // Annex B.1.1 HTML-like comments.
                if c == b'<' as u32 && self.at(1) == b'!' as u32 && self.at(2) == b'-' as u32 && self.at(3) == b'-' as u32 {
                    self.skip_line_comment();
                    continue;
                }
                if c == b'-' as u32 && self.at(1) == b'-' as u32 && self.at(2) == b'>' as u32 && self.line_start {
                    self.skip_line_comment();
                    continue;
                }
            }
            return Ok(());
        }
    }

    fn skip_line_comment(&mut self) {
        while self.pos < self.src.len() && !is_lt(self.src[self.pos] as u32) {
            self.pos += 1;
        }
    }

    pub fn next_token(&mut self) -> Result<Token, LexError> {
        self.nl_before = false;
        self.skip_trivia()?;
        let start = self.pos;
        let nl = self.nl_before;
        self.line_start = false;
        let mut tok = Token { t: T::Eof, start: start as u32, end: start as u32, nl_before: nl, escaped: false, legacy_octal: false };
        let c = match self.peek_cp() {
            None => return Ok(tok),
            Some((c, _)) => c,
        };
        let t = match c {
            0x22 | 0x27 => self.string(c, &mut tok)?,
            0x60 => {
                self.pos += 1;
                self.template_piece()?
            }
            0x30..=0x39 => self.number(&mut tok)?,
            0x2E if (0x30..=0x39).contains(&self.at(1)) => self.number(&mut tok)?,
            0x23 => {
                // PrivateIdentifier
                self.pos += 1;
                match self.peek_cp() {
                    Some((d, _)) if d == b'\\' as u32 || unicode::is_id_start(d) || d == b'$' as u32 || d == b'_' as u32 => {
                        let (name, esc) = self.ident_name()?;
                        tok.escaped = esc;
                        T::Private(name)
                    }
                    _ => return self.err(start, "invalid private name"),
                }
            }
            _ if c == b'\\' as u32 || c == b'$' as u32 || c == b'_' as u32 || unicode::is_id_start(c) => {
                let (name, esc) = self.ident_name()?;
                tok.escaped = esc;
                T::Name(name)
            }
            _ => T::Punct(self.punct()?),
        };
        tok.t = t;
        tok.end = self.pos as u32;
        Ok(tok)
    }

    fn punct(&mut self) -> Result<P, LexError> {
        let c = self.at(0);
        let c1 = self.at(1);
        let c2 = self.at(2);
        let c3 = self.at(3);
        let ch = |x: u32| char::from_u32(x).unwrap_or('\0');
        let (p, n) = match (ch(c), ch(c1), ch(c2), ch(c3)) {
            ('{', ..) => (P::LBrace, 1),
            ('}', ..) => (P::RBrace, 1),
            ('(', ..) => (P::LParen, 1),
            (')', ..) => (P::RParen, 1),
            ('[', ..) => (P::LBracket, 1),
            (']', ..) => (P::RBracket, 1),
            ('.', '.', '.', _) => (P::Ellipsis, 3),
            ('.', ..) => (P::Dot, 1),
            (';', ..) => (P::Semi, 1),
            (',', ..) => (P::Comma, 1),
            ('<', '<', '=', _) => (P::ShlEq, 3),
            ('<', '<', ..) => (P::Shl, 2),
            ('<', '=', ..) => (P::Le, 2),
            ('<', ..) => (P::Lt, 1),
            ('>', '>', '>', '=') => (P::UShrEq, 4),
            ('>', '>', '>', _) => (P::UShr, 3),
            ('>', '>', '=', _) => (P::ShrEq, 3),
            ('>', '>', ..) => (P::Shr, 2),
            ('>', '=', ..) => (P::Ge, 2),
            ('>', ..) => (P::Gt, 1),
            ('=', '=', '=', _) => (P::Eq3, 3),
            ('=', '=', ..) => (P::Eq2, 2),
            ('=', '>', ..) => (P::Arrow, 2),
            ('=', ..) => (P::Assign, 1),
            ('!', '=', '=', _) => (P::Ne2, 3),
            ('!', '=', ..) => (P::Ne, 2),
            ('!', ..) => (P::Bang, 1),
            ('+', '+', ..) => (P::Inc, 2),
            ('+', '=', ..) => (P::PlusEq, 2),
            ('+', ..) => (P::Plus, 1),
            ('-', '-', ..) => (P::Dec, 2),
            ('-', '=', ..) => (P::MinusEq, 2),
            ('-', ..) => (P::Minus, 1),
            ('*', '*', '=', _) => (P::Star2Eq, 3),
            ('*', '*', ..) => (P::Star2, 2),
            ('*', '=', ..) => (P::StarEq, 2),
            ('*', ..) => (P::Star, 1),
            ('%', '=', ..) => (P::PercentEq, 2),
            ('%', ..) => (P::Percent, 1),
            ('&', '&', '=', _) => (P::AndEq, 3),
            ('&', '&', ..) => (P::And, 2),
            ('&', '=', ..) => (P::AmpEq, 2),
            ('&', ..) => (P::Amp, 1),
            ('|', '|', '=', _) => (P::OrEq, 3),
            ('|', '|', ..) => (P::Or, 2),
            ('|', '=', ..) => (P::PipeEq, 2),
            ('|', ..) => (P::Pipe, 1),
            ('^', '=', ..) => (P::CaretEq, 2),
            ('^', ..) => (P::Caret, 1),
            ('~', ..) => (P::Tilde, 1),
            ('?', '?', '=', _) => (P::NullishEq, 3),
            ('?', '?', ..) => (P::Nullish, 2),
            ('?', '.', d, _) if !d.is_ascii_digit() => (P::QDot, 2),
            ('?', ..) => (P::Question, 1),
            (':', ..) => (P::Colon, 1),
            ('/', '=', ..) => (P::SlashEq, 2),
            ('/', ..) => (P::Slash, 1),
            ('@', ..) => (P::At, 1),
            _ => return self.err(self.pos, "unexpected character"),
        };
        self.pos += n;
        Ok(p)
    }

    /// IdentifierName with escapes. Returns (name, had_escape).
    fn ident_name(&mut self) -> Result<(Atom, bool), LexError> {
        let mut s = String::new();
        let mut escaped = false;
        let mut first = true;
        loop {
            let start = self.pos;
            let (c, n) = match self.peek_cp() {
                None => break,
                Some(x) => x,
            };
            let cp;
            if c == b'\\' as u32 {
                if self.at(1) != b'u' as u32 {
                    return self.err(start, "invalid escape in identifier");
                }
                self.pos += 2;
                cp = self.unicode_escape_body()?;
                escaped = true;
                let ok = if first {
                    cp == b'$' as u32 || cp == b'_' as u32 || unicode::is_id_start(cp)
                } else {
                    cp == b'$' as u32 || cp == 0x200C || cp == 0x200D || unicode::is_id_continue(cp)
                };
                if !ok {
                    return self.err(start, "invalid identifier escape");
                }
            } else {
                let ok = if first {
                    c == b'$' as u32 || c == b'_' as u32 || unicode::is_id_start(c)
                } else {
                    c == b'$' as u32 || c == 0x200C || c == 0x200D || unicode::is_id_continue(c)
                };
                if !ok {
                    break;
                }
                self.pos += n;
                cp = c;
            }
            match char::from_u32(cp) {
                Some(ch) => s.push(ch),
                None => return self.err(start, "invalid identifier"),
            }
            first = false;
        }
        Ok((Rc::from(s.as_str()), escaped))
    }

    /// After `\u`: XXXX or {X...}. Returns the code point.
    fn unicode_escape_body(&mut self) -> Result<u32, LexError> {
        let start = self.pos;
        if self.at(0) == b'{' as u32 {
            self.pos += 1;
            let mut v: u32 = 0;
            let mut any = false;
            while let Some(d) = numconv::digit_val(self.at(0)).filter(|&d| d < 16) {
                v = v.saturating_mul(16).saturating_add(d);
                if v > 0x10FFFF {
                    return self.err(start, "code point out of range");
                }
                any = true;
                self.pos += 1;
            }
            if !any || self.at(0) != b'}' as u32 {
                return self.err(start, "invalid unicode escape");
            }
            self.pos += 1;
            return Ok(v);
        }
        let mut v = 0;
        for i in 0..4 {
            match numconv::digit_val(self.at(i)).filter(|&d| d < 16) {
                Some(d) => v = v * 16 + d,
                None => return self.err(start, "invalid unicode escape"),
            }
        }
        self.pos += 4;
        Ok(v)
    }

    fn string(&mut self, quote: u32, tok: &mut Token) -> Result<T, LexError> {
        let start = self.pos;
        self.pos += 1;
        let mut v: Vec<u16> = Vec::new();
        loop {
            if self.pos >= self.src.len() {
                return self.err(start, "unterminated string");
            }
            let c = self.src[self.pos] as u32;
            if c == quote {
                self.pos += 1;
                break;
            }
            if c == 0x0A || c == 0x0D {
                return self.err(start, "unterminated string");
            }
            if c == b'\\' as u32 {
                self.pos += 1;
                match self.escape(&mut v, false)? {
                    EscapeKind::Ok => {}
                    EscapeKind::LegacyOctal => tok.legacy_octal = true,
                    EscapeKind::Invalid(m) => return self.err(self.pos, m),
                }
                continue;
            }
            v.push(c as u16);
            self.pos += 1;
        }
        Ok(T::Str(JsStr::from_units(v)))
    }

    /// Parse an escape after the backslash. In templates, octal escapes are invalid.
    fn escape(&mut self, v: &mut Vec<u16>, template: bool) -> Result<EscapeKind, LexError> {
        let c = self.at(0);
        if c == u32::MAX {
            return Ok(EscapeKind::Invalid("unterminated escape"));
        }
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        match ch {
            'n' => v.push(0x0A),
            't' => v.push(0x09),
            'r' => v.push(0x0D),
            'b' => v.push(0x08),
            'f' => v.push(0x0C),
            'v' => v.push(0x0B),
            '\r' => {
                self.pos += 1;
                if self.at(0) == 0x0A {
                    self.pos += 1;
                }
                return Ok(EscapeKind::Ok);
            }
            '\n' | '\u{2028}' | '\u{2029}' => {}
            '0' if !(0x30..=0x39).contains(&self.at(1)) => v.push(0),
            '0'..='7' => {
                if template {
                    return Ok(EscapeKind::Invalid("octal escape in template"));
                }
                // LegacyOctalEscapeSequence
                let mut n = c - 0x30;
                self.pos += 1;
                let max_len = if c <= b'3' as u32 { 3 } else { 2 };
                let mut len = 1;
                while len < max_len && (0x30..=0x37).contains(&self.at(0)) {
                    n = n * 8 + (self.at(0) - 0x30);
                    self.pos += 1;
                    len += 1;
                }
                v.push(n as u16);
                return Ok(EscapeKind::LegacyOctal);
            }
            '8' | '9' => {
                if template {
                    return Ok(EscapeKind::Invalid("\\8 or \\9 in template"));
                }
                v.push(c as u16);
                self.pos += 1;
                return Ok(EscapeKind::LegacyOctal);
            }
            'x' => {
                let a = numconv::digit_val(self.at(1)).filter(|&d| d < 16);
                let b = numconv::digit_val(self.at(2)).filter(|&d| d < 16);
                match (a, b) {
                    (Some(a), Some(b)) => {
                        v.push((a * 16 + b) as u16);
                        self.pos += 3;
                        return Ok(EscapeKind::Ok);
                    }
                    _ => return Ok(EscapeKind::Invalid("invalid hex escape")),
                }
            }
            'u' => {
                self.pos += 1;
                let save = self.pos;
                match self.unicode_escape_body() {
                    Ok(cp) => {
                        push_code_point(v, cp);
                        return Ok(EscapeKind::Ok);
                    }
                    Err(_) => {
                        self.pos = save;
                        return Ok(EscapeKind::Invalid("invalid unicode escape"));
                    }
                }
            }
            _ => {
                // NonEscapeCharacter (any source character, incl. a surrogate pair).
                let (cp, n) = crate::string::code_point_at(&self.src, self.pos);
                push_code_point(v, cp);
                self.pos += n;
                return Ok(EscapeKind::Ok);
            }
        }
        self.pos += 1;
        Ok(EscapeKind::Ok)
    }

    /// Scan a template piece starting after '`' or after the '}' that closes a substitution.
    pub fn template_piece(&mut self) -> Result<T, LexError> {
        let start = self.pos;
        let mut cooked: Vec<u16> = Vec::new();
        let mut raw: Vec<u16> = Vec::new();
        let mut valid = true;
        loop {
            if self.pos >= self.src.len() {
                return self.err(start, "unterminated template");
            }
            let c = self.src[self.pos] as u32;
            if c == 0x60 {
                self.pos += 1;
                let cooked = if valid { Some(JsStr::from_units(cooked)) } else { None };
                return Ok(T::Template { cooked, raw: JsStr::from_units(raw), tail: true });
            }
            if c == b'$' as u32 && self.at(1) == b'{' as u32 {
                self.pos += 2;
                let cooked = if valid { Some(JsStr::from_units(cooked)) } else { None };
                return Ok(T::Template { cooked, raw: JsStr::from_units(raw), tail: false });
            }
            if c == b'\\' as u32 {
                let esc_start = self.pos;
                self.pos += 1;
                match self.escape(&mut cooked, true)? {
                    EscapeKind::Ok => {}
                    _ => {
                        valid = false;
                        // Skip one character so scanning resumes sensibly.
                        if self.pos == esc_start + 1 && self.pos < self.src.len() {
                            self.pos += 1;
                        }
                    }
                }
                // Raw: the source text, with CR / CRLF normalised to LF.
                let mut i = esc_start;
                while i < self.pos {
                    let u = self.src[i];
                    if u == 0x0D {
                        raw.push(0x0A);
                        if i + 1 < self.pos && self.src[i + 1] == 0x0A {
                            i += 1;
                        }
                    } else {
                        raw.push(u);
                    }
                    i += 1;
                }
                continue;
            }
            if c == 0x0D {
                self.pos += 1;
                if self.at(0) == 0x0A {
                    self.pos += 1;
                }
                cooked.push(0x0A);
                raw.push(0x0A);
                continue;
            }
            cooked.push(c as u16);
            raw.push(c as u16);
            self.pos += 1;
        }
    }

    fn number(&mut self, tok: &mut Token) -> Result<T, LexError> {
        let start = self.pos;
        let c = self.at(0);
        let c1 = self.at(1);
        let radix = if c == b'0' as u32 {
            match char::from_u32(c1).unwrap_or('\0') {
                'x' | 'X' => 16,
                'o' | 'O' => 8,
                'b' | 'B' => 2,
                _ => 10,
            }
        } else {
            10
        };
        let result;
        if radix != 10 {
            self.pos += 2;
            let digits = self.digits(radix, true)?;
            if digits.is_empty() {
                return self.err(start, "missing digits");
            }
            if self.at(0) == b'n' as u32 {
                self.pos += 1;
                let b = crate::bignum::BigUint::from_digits(&digits, radix);
                result = T::BigInt(Rc::from(core::str::from_utf8(&b.to_string_radix(10)).unwrap()));
            } else {
                result = T::Num(numconv::parse_radix_int(&digits, radix));
            }
        } else if c == b'0' as u32 && (0x30..=0x39).contains(&c1) {
            // LegacyOctalIntegerLiteral or NonOctalDecimalIntegerLiteral (no separators allowed).
            tok.legacy_octal = true;
            self.pos += 1;
            let mut digits = Vec::new();
            let mut octal = true;
            while (0x30..=0x39).contains(&self.at(0)) {
                if self.at(0) >= b'8' as u32 {
                    octal = false;
                }
                digits.push(self.at(0) as u8);
                self.pos += 1;
            }
            if octal {
                result = T::Num(numconv::parse_radix_int(&digits, 8));
            } else {
                // NonOctalDecimalIntegerLiteral may continue as a decimal with fraction/exponent.
                let mut all = digits;
                let mut exp: i64 = 0;
                if self.at(0) == b'.' as u32 {
                    self.pos += 1;
                    while (0x30..=0x39).contains(&self.at(0)) {
                        all.push(self.at(0) as u8);
                        exp -= 1;
                        self.pos += 1;
                    }
                }
                exp += self.exponent()?;
                result = T::Num(numconv::decimal_to_f64(&all, exp));
            }
            if self.at(0) == b'n' as u32 {
                return self.err(start, "invalid BigInt literal");
            }
        } else {
            if c == b'0' as u32 && c1 == b'_' as u32 {
                return self.err(start, "numeric separator after leading zero");
            }
            let mut digits = self.digits(10, true)?;
            let int_len = digits.len();
            let mut exp: i64 = 0;
            let mut is_int = true;
            if self.at(0) == b'.' as u32 {
                is_int = false;
                self.pos += 1;
                if self.at(0) == b'_' as u32 {
                    return self.err(self.pos, "separator after dot");
                }
                let frac = self.digits(10, true)?;
                exp -= frac.len() as i64;
                digits.extend_from_slice(&frac);
            }
            if self.at(0) == b'e' as u32 || self.at(0) == b'E' as u32 {
                is_int = false;
            }
            exp += self.exponent()?;
            if self.at(0) == b'n' as u32 {
                if !is_int {
                    return self.err(start, "invalid BigInt literal");
                }
                self.pos += 1;
                let mut d = &digits[..int_len];
                while d.len() > 1 && d[0] == b'0' {
                    d = &d[1..];
                }
                result = T::BigInt(Rc::from(core::str::from_utf8(d).unwrap()));
            } else {
                result = T::Num(numconv::decimal_to_f64(&digits, exp));
            }
        }
        // The SourceCharacter immediately following a NumericLiteral must not be an IdentifierStart or digit.
        if let Some((d, _)) = self.peek_cp() {
            if d == b'\\' as u32 || d == b'$' as u32 || d == b'_' as u32 || unicode::is_id_start(d) || (0x30..=0x39).contains(&d) {
                return self.err(self.pos, "identifier starts immediately after numeric literal");
            }
        }
        Ok(result)
    }

    fn exponent(&mut self) -> Result<i64, LexError> {
        if self.at(0) != b'e' as u32 && self.at(0) != b'E' as u32 {
            return Ok(0);
        }
        let start = self.pos;
        self.pos += 1;
        let mut neg = false;
        if self.at(0) == b'+' as u32 || self.at(0) == b'-' as u32 {
            neg = self.at(0) == b'-' as u32;
            self.pos += 1;
        }
        let d = self.digits(10, true)?;
        if d.is_empty() {
            return self.err(start, "missing exponent");
        }
        let mut v: i64 = 0;
        for c in d {
            if v < 1_000_000_000 {
                v = v * 10 + (c - b'0') as i64;
            }
        }
        Ok(if neg { -v } else { v })
    }

    /// Digits with numeric separators.
    fn digits(&mut self, radix: u32, seps: bool) -> Result<Vec<u8>, LexError> {
        let mut out = Vec::new();
        let mut last_sep = false;
        loop {
            let c = self.at(0);
            if c == b'_' as u32 && seps {
                if out.is_empty() || last_sep {
                    return self.err(self.pos, "invalid numeric separator");
                }
                last_sep = true;
                self.pos += 1;
                continue;
            }
            match numconv::digit_val(c) {
                Some(d) if d < radix => {
                    out.push(c as u8);
                    last_sep = false;
                    self.pos += 1;
                }
                _ => break,
            }
        }
        if last_sep {
            return self.err(self.pos, "trailing numeric separator");
        }
        Ok(out)
    }

    /// Re-scan from `start` (a `/` or `/=` token) as a RegularExpressionLiteral.
    pub fn rescan_regex(&mut self, start: u32) -> Result<Token, LexError> {
        let s = start as usize;
        self.pos = s + 1;
        let mut in_class = false;
        let body_start = self.pos;
        loop {
            if self.pos >= self.src.len() {
                return self.err(s, "unterminated regular expression");
            }
            let c = self.src[self.pos] as u32;
            if is_lt(c) {
                return self.err(s, "unterminated regular expression");
            }
            if c == b'\\' as u32 {
                self.pos += 1;
                if self.pos >= self.src.len() || is_lt(self.src[self.pos] as u32) {
                    return self.err(s, "unterminated regular expression");
                }
                self.pos += 1;
                continue;
            }
            if c == b'[' as u32 {
                in_class = true;
            } else if c == b']' as u32 {
                in_class = false;
            } else if c == b'/' as u32 && !in_class {
                break;
            }
            self.pos += 1;
        }
        let body = JsStr::from_slice(&self.src[body_start..self.pos]);
        self.pos += 1;
        let flags_start = self.pos;
        while let Some((c, n)) = self.peek_cp() {
            if c == b'\\' as u32 {
                return self.err(self.pos, "escape in regular expression flags");
            }
            if !(c == b'$' as u32 || c == 0x200C || c == 0x200D || unicode::is_id_continue(c)) {
                break;
            }
            self.pos += n;
        }
        let flags = JsStr::from_slice(&self.src[flags_start..self.pos]);
        Ok(Token { t: T::Regex { body, flags }, start, end: self.pos as u32, nl_before: false, escaped: false, legacy_octal: false })
    }

    /// Re-scan from a `}` token as a TemplateMiddle / TemplateTail.
    pub fn rescan_template(&mut self, start: u32) -> Result<Token, LexError> {
        self.pos = start as usize + 1;
        let t = self.template_piece()?;
        Ok(Token { t, start, end: self.pos as u32, nl_before: false, escaped: false, legacy_octal: false })
    }
}

enum EscapeKind {
    Ok,
    LegacyOctal,
    Invalid(&'static str),
}
