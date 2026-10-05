//! CSS Syntax Module Level 3, §3.3 preprocessing and §4 tokenization.
//!
//! The tokenizer works over the preprocessed input as a `Vec<char>` (CR / FF / CRLF → LF, NUL → U+FFFD) and
//! yields each token with its `[start, end)` span in that char vector, so productions that need the source
//! text of their tokens (the `<urange>` production, §7.1) can recover it.

use alloc::string::String;
use alloc::vec::Vec;

/// A numeric value as the tokenizer produced it (§4.3.12 "consume a number").
#[derive(Clone, Debug, PartialEq)]
pub struct Num {
    /// The value. CSS numbers are IEEE doubles here (Chromium's representation).
    pub value: f64,
    /// `true` for the "integer" type flag, `false` for "number".
    pub is_integer: bool,
    /// `true` when the source began with an explicit `+` or `-` (An+B and `<urange>` need it).
    pub has_sign: bool,
}

impl Num {
    /// The value as an integer, clamped to `i32` (for the "integer"-typed productions).
    pub fn int(&self) -> i32 {
        let v = self.value;
        if v >= i32::MAX as f64 {
            i32::MAX
        } else if v <= i32::MIN as f64 {
            i32::MIN
        } else {
            v as i32
        }
    }
}

/// One token (§4).
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Ident(String),
    Function(String),
    AtKeyword(String),
    /// `<hash-token>`; `is_id` is the "id" type flag (the name would start an ident sequence).
    Hash { value: String, is_id: bool },
    String(String),
    BadString,
    Url(String),
    BadUrl,
    Delim(char),
    Number(Num),
    Percentage(Num),
    Dimension(Num, String),
    Whitespace,
    Cdo,
    Cdc,
    Colon,
    Semicolon,
    Comma,
    LeftBracket,
    RightBracket,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    /// `~=` `|=` `^=` `$=` `*=`: the attribute-selector match tokens of the 2014 CR (css-parsing-tests
    /// still distinguish them from two delims; the Selectors parser accepts both forms).
    IncludeMatch,
    DashMatch,
    PrefixMatch,
    SuffixMatch,
    SubstringMatch,
}

/// §3.3: CR, FF and CRLF become LF; NUL becomes U+FFFD. (Surrogates cannot occur in a Rust `str`.)
pub fn preprocess(input: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(input.len());
    let mut it = input.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                out.push('\n');
            }
            '\x0C' => out.push('\n'),
            '\0' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

fn is_ws(c: char) -> bool {
    c == '\n' || c == '\t' || c == ' '
}
fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c as u32 >= 0x80
}
fn is_ident_char(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit() || c == '-'
}
fn is_non_printable(c: char) -> bool {
    matches!(c as u32, 0..=8 | 0x0B | 0x0E..=0x1F | 0x7F)
}

/// The §4 tokenizer over a preprocessed char buffer.
pub struct Tokenizer {
    s: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    pub fn new(input: &str) -> Self {
        Tokenizer { s: preprocess(input), pos: 0 }
    }

    /// The preprocessed input (token spans index into it).
    pub fn source(&self) -> &[char] {
        &self.s
    }

    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }
    fn cur(&self, k: usize) -> Option<char> {
        self.at(self.pos + k)
    }

    /// §4.3.8: two code points at `i` are a valid escape.
    fn valid_escape_at(&self, i: usize) -> bool {
        self.at(i) == Some('\\') && self.at(i + 1) != Some('\n')
    }

    /// §4.3.9: three code points at `i` would start an ident sequence.
    fn starts_ident_at(&self, i: usize) -> bool {
        match self.at(i) {
            Some('-') => match self.at(i + 1) {
                Some(c) if is_ident_start(c) || c == '-' => true,
                _ => self.valid_escape_at(i + 1),
            },
            Some('\\') => self.valid_escape_at(i),
            Some(c) => is_ident_start(c),
            None => false,
        }
    }

    /// §4.3.10: three code points at `i` would start a number.
    fn starts_number_at(&self, i: usize) -> bool {
        match self.at(i) {
            Some('+') | Some('-') => match self.at(i + 1) {
                Some(c) if c.is_ascii_digit() => true,
                Some('.') => matches!(self.at(i + 2), Some(c) if c.is_ascii_digit()),
                _ => false,
            },
            Some('.') => matches!(self.at(i + 1), Some(c) if c.is_ascii_digit()),
            Some(c) => c.is_ascii_digit(),
            None => false,
        }
    }

    /// §4.3.7: consume an escaped code point (the backslash already consumed).
    fn consume_escape(&mut self) -> char {
        match self.cur(0) {
            None => '\u{FFFD}',
            Some(c) if c.is_ascii_hexdigit() => {
                let mut v: u32 = 0;
                let mut n = 0;
                while n < 6 {
                    match self.cur(0) {
                        Some(h) if h.is_ascii_hexdigit() => {
                            v = v * 16 + h.to_digit(16).unwrap();
                            self.pos += 1;
                            n += 1;
                        }
                        _ => break,
                    }
                }
                if matches!(self.cur(0), Some(w) if is_ws(w)) {
                    self.pos += 1;
                }
                if v == 0 || (0xD800..=0xDFFF).contains(&v) || v > 0x10FFFF {
                    '\u{FFFD}'
                } else {
                    char::from_u32(v).unwrap_or('\u{FFFD}')
                }
            }
            Some(c) => {
                self.pos += 1;
                c
            }
        }
    }

    /// §4.3.11: consume an ident sequence.
    fn consume_ident_seq(&mut self) -> String {
        let mut out = String::new();
        loop {
            match self.cur(0) {
                Some(c) if is_ident_char(c) => {
                    out.push(c);
                    self.pos += 1;
                }
                Some('\\') if self.valid_escape_at(self.pos) => {
                    self.pos += 1;
                    out.push(self.consume_escape());
                }
                _ => return out,
            }
        }
    }

    /// §4.3.12: consume a number.
    fn consume_number(&mut self) -> Num {
        let start = self.pos;
        let mut is_integer = true;
        let has_sign = matches!(self.cur(0), Some('+') | Some('-'));
        if has_sign {
            self.pos += 1;
        }
        while matches!(self.cur(0), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.cur(0) == Some('.') && matches!(self.cur(1), Some(c) if c.is_ascii_digit()) {
            is_integer = false;
            self.pos += 1;
            while matches!(self.cur(0), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.cur(0), Some('e') | Some('E')) {
            let exp = match self.cur(1) {
                Some(c) if c.is_ascii_digit() => Some(1),
                Some('+') | Some('-') if matches!(self.cur(2), Some(c) if c.is_ascii_digit()) => Some(2),
                _ => None,
            };
            if let Some(k) = exp {
                is_integer = false;
                self.pos += k;
                while matches!(self.cur(0), Some(c) if c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
        }
        // §4.3.13 "convert a string to a number": the exact decimal value, correctly rounded.
        let repr: String = self.s[start..self.pos].iter().collect();
        let value = repr.parse::<f64>().unwrap_or(0.0);
        Num { value, is_integer, has_sign }
    }

    /// §4.3.3: consume a numeric token.
    fn consume_numeric(&mut self) -> Token {
        let n = self.consume_number();
        if self.starts_ident_at(self.pos) {
            let unit = self.consume_ident_seq();
            Token::Dimension(n, unit)
        } else if self.cur(0) == Some('%') {
            self.pos += 1;
            Token::Percentage(n)
        } else {
            Token::Number(n)
        }
    }

    /// §4.3.4: consume an ident-like token.
    fn consume_ident_like(&mut self) -> Token {
        let name = self.consume_ident_seq();
        if name.eq_ignore_ascii_case("url") && self.cur(0) == Some('(') {
            self.pos += 1;
            while matches!(self.cur(0), Some(c) if is_ws(c)) && matches!(self.cur(1), Some(c) if is_ws(c)) {
                self.pos += 1;
            }
            let q = |c: Option<char>| c == Some('"') || c == Some('\'');
            if q(self.cur(0)) || (matches!(self.cur(0), Some(c) if is_ws(c)) && q(self.cur(1))) {
                Token::Function(name)
            } else {
                self.consume_url()
            }
        } else if self.cur(0) == Some('(') {
            self.pos += 1;
            Token::Function(name)
        } else {
            Token::Ident(name)
        }
    }

    /// §4.3.6: consume a url token (after `url(`).
    fn consume_url(&mut self) -> Token {
        let mut out = String::new();
        while matches!(self.cur(0), Some(c) if is_ws(c)) {
            self.pos += 1;
        }
        loop {
            match self.cur(0) {
                Some(')') => {
                    self.pos += 1;
                    return Token::Url(out);
                }
                None => return Token::Url(out),
                Some(c) if is_ws(c) => {
                    while matches!(self.cur(0), Some(c) if is_ws(c)) {
                        self.pos += 1;
                    }
                    match self.cur(0) {
                        Some(')') => {
                            self.pos += 1;
                            return Token::Url(out);
                        }
                        None => return Token::Url(out),
                        _ => {
                            self.consume_bad_url_remnants();
                            return Token::BadUrl;
                        }
                    }
                }
                Some('"') | Some('\'') | Some('(') => {
                    self.consume_bad_url_remnants();
                    return Token::BadUrl;
                }
                Some(c) if is_non_printable(c) => {
                    self.consume_bad_url_remnants();
                    return Token::BadUrl;
                }
                Some('\\') => {
                    if self.valid_escape_at(self.pos) {
                        self.pos += 1;
                        out.push(self.consume_escape());
                    } else {
                        self.consume_bad_url_remnants();
                        return Token::BadUrl;
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// §4.3.14
    fn consume_bad_url_remnants(&mut self) {
        loop {
            match self.cur(0) {
                None => return,
                Some(')') => {
                    self.pos += 1;
                    return;
                }
                Some('\\') if self.valid_escape_at(self.pos) => {
                    self.pos += 1;
                    self.consume_escape();
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    /// §4.3.5: consume a string token (the opening quote already consumed).
    fn consume_string(&mut self, end: char) -> Token {
        let mut out = String::new();
        loop {
            match self.cur(0) {
                None => return Token::String(out),
                Some(c) if c == end => {
                    self.pos += 1;
                    return Token::String(out);
                }
                Some('\n') => return Token::BadString,
                Some('\\') => match self.cur(1) {
                    None => self.pos += 1,
                    Some('\n') => self.pos += 2,
                    Some(_) => {
                        self.pos += 1;
                        out.push(self.consume_escape());
                    }
                },
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// §4.3.2: consume comments.
    fn consume_comments(&mut self) {
        while self.cur(0) == Some('/') && self.cur(1) == Some('*') {
            self.pos += 2;
            loop {
                match self.cur(0) {
                    None => return,
                    Some('*') if self.cur(1) == Some('/') => {
                        self.pos += 2;
                        break;
                    }
                    _ => self.pos += 1,
                }
            }
        }
    }

    /// §4.3.1: consume a token. `None` at EOF.
    pub fn next_token(&mut self) -> Option<(Token, usize, usize)> {
        self.consume_comments();
        let start = self.pos;
        let c = self.cur(0)?;
        let t = match c {
            c if is_ws(c) => {
                while matches!(self.cur(0), Some(c) if is_ws(c)) {
                    self.pos += 1;
                }
                Token::Whitespace
            }
            '"' | '\'' => {
                self.pos += 1;
                self.consume_string(c)
            }
            '#' => {
                if matches!(self.cur(1), Some(n) if is_ident_char(n)) || self.valid_escape_at(self.pos + 1) {
                    self.pos += 1;
                    let is_id = self.starts_ident_at(self.pos);
                    let value = self.consume_ident_seq();
                    Token::Hash { value, is_id }
                } else {
                    self.pos += 1;
                    Token::Delim('#')
                }
            }
            '(' => self.one(Token::LeftParen),
            ')' => self.one(Token::RightParen),
            '[' => self.one(Token::LeftBracket),
            ']' => self.one(Token::RightBracket),
            '{' => self.one(Token::LeftBrace),
            '}' => self.one(Token::RightBrace),
            ',' => self.one(Token::Comma),
            ':' => self.one(Token::Colon),
            ';' => self.one(Token::Semicolon),
            '+' | '.' => {
                if self.starts_number_at(self.pos) {
                    self.consume_numeric()
                } else {
                    self.one(Token::Delim(c))
                }
            }
            '-' => {
                if self.starts_number_at(self.pos) {
                    self.consume_numeric()
                } else if self.cur(1) == Some('-') && self.cur(2) == Some('>') {
                    self.pos += 3;
                    Token::Cdc
                } else if self.starts_ident_at(self.pos) {
                    self.consume_ident_like()
                } else {
                    self.one(Token::Delim('-'))
                }
            }
            '<' => {
                if self.cur(1) == Some('!') && self.cur(2) == Some('-') && self.cur(3) == Some('-') {
                    self.pos += 4;
                    Token::Cdo
                } else {
                    self.one(Token::Delim('<'))
                }
            }
            '@' => {
                if self.starts_ident_at(self.pos + 1) {
                    self.pos += 1;
                    Token::AtKeyword(self.consume_ident_seq())
                } else {
                    self.one(Token::Delim('@'))
                }
            }
            '\\' => {
                if self.valid_escape_at(self.pos) {
                    self.consume_ident_like()
                } else {
                    self.one(Token::Delim('\\'))
                }
            }
            '~' | '|' | '^' | '$' | '*' if self.cur(1) == Some('=') => {
                self.pos += 2;
                match c {
                    '~' => Token::IncludeMatch,
                    '|' => Token::DashMatch,
                    '^' => Token::PrefixMatch,
                    '$' => Token::SuffixMatch,
                    _ => Token::SubstringMatch,
                }
            }
            c if c.is_ascii_digit() => self.consume_numeric(),
            c if is_ident_start(c) => self.consume_ident_like(),
            c => self.one(Token::Delim(c)),
        };
        Some((t, start, self.pos))
    }

    fn one(&mut self, t: Token) -> Token {
        self.pos += 1;
        t
    }
}

/// Tokenize a whole string (spans dropped).
pub fn tokenize(input: &str) -> Vec<Token> {
    let mut t = Tokenizer::new(input);
    let mut out = Vec::new();
    while let Some((tok, _, _)) = t.next_token() {
        out.push(tok);
    }
    out
}
