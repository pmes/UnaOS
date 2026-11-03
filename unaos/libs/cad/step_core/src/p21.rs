//! ISO 10303-21, the clear-text encoding of the exchange structure (the public grammar, cited by its sections):
//! - the file is `ISO-10303-21;` HEADER `ENDSEC;` DATA `ENDSEC;` `END-ISO-10303-21;`;
//! - the HEADER holds FILE_DESCRIPTION, FILE_NAME and FILE_SCHEMA, each a record `NAME(params);`;
//! - the DATA section holds entity instances `#n = NAME(params);` (simple) or `#n = (A(params) B(params) ...);`
//!   (complex, one partial record per supertype), a third-edition DATA may carry `(params)` after the keyword;
//! - a parameter is an INTEGER, a REAL (`1.`, `-2.5E-3`: the decimal point is required), a STRING (`'...'`), an
//!   ENUMERATION (`.T.`), a BINARY (`"0FF"`), an instance reference `#n`, `$` (null), `*` (derived), a list `( ... )`
//!   or a typed parameter `NAME(param)`;
//! - a STRING doubles its apostrophe (`''`) and its backslash (`\\`) and spells other characters by the control
//!   directives `\S\c` (c + 128 in the current page), `\P?\` (the page: A, ISO 8859-1, is the only one read here),
//!   `\X\hh` (one ISO 8859-1 octet), `\X2\hhhh...\X0\` (UCS-2) and `\X4\hhhhhhhh...\X0\` (UCS-4);
//! - comments `/* ... */` and white space, line breaks included, may stand between any two tokens, so a long instance
//!   continues over lines; a line break inside a string is not part of its value (files wrapped at 80 columns).
//!
//! Every refusal names its 1-based line and column. The writer half spells REALs and STRINGs back.

use crate::Refusal;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// One parameter value.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    Bin(String),
    /// A reference to instance `id`, at its position in the text.
    Ref { id: u64, line: u32, col: u32 },
    /// `$`.
    Null,
    /// `*`.
    Derived,
    List(Vec<Param>),
    /// `NAME(param)`.
    Typed(String, Box<Param>),
}

/// `NAME(params)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub name: String,
    pub params: Vec<Param>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Simple(Record),
    /// The partial records of a complex instance, in file order.
    Complex(Vec<Record>),
}

/// `#id = body;` at (line, col) of its `#`.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub id: u64,
    pub line: u32,
    pub col: u32,
    pub body: Body,
}

impl Instance {
    /// The simple record named `name`, or the partial of that name in a complex instance.
    pub fn record(&self, name: &str) -> Option<&Record> {
        match &self.body {
            Body::Simple(r) => (r.name == name).then_some(r),
            Body::Complex(rs) => rs.iter().find(|r| r.name == name),
        }
    }

    /// True when the instance is (or has a partial) named `name`.
    pub fn is(&self, name: &str) -> bool {
        self.record(name).is_some()
    }

    /// The simple record's name, or `(A B C)` for a complex instance.
    pub fn name(&self) -> String {
        match &self.body {
            Body::Simple(r) => r.name.clone(),
            Body::Complex(rs) => {
                let mut s = String::from("(");
                for (i, r) in rs.iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    s.push_str(&r.name);
                }
                s.push(')');
                s
            }
        }
    }

    /// Every record (one for a simple instance).
    pub fn records(&self) -> &[Record] {
        match &self.body {
            Body::Simple(r) => core::slice::from_ref(r),
            Body::Complex(rs) => rs,
        }
    }
}

/// The exchange structure: the HEADER records and the DATA instances by number.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Table {
    pub header: Vec<Record>,
    pub data: BTreeMap<u64, Instance>,
}

impl Table {
    pub fn get(&self, id: u64) -> Option<&Instance> {
        self.data.get(&id)
    }

    /// The header record named `name`.
    pub fn header(&self, name: &str) -> Option<&Record> {
        self.header.iter().find(|r| r.name == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Kw(String),
    Inst(u64),
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    Bin(String),
    Dollar,
    Star,
    LParen,
    RParen,
    Comma,
    Eq,
    Semi,
    Eof,
}

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
    line: u32,
    col: u32,
}

fn syntax(line: u32, col: u32, what: &'static str) -> Refusal {
    Refusal::Syntax { line, col, what }
}

fn hex(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        _ => None,
    }
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Lexer { b: text.as_bytes(), i: 0, line: 1, col: 1 }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn at(&self, k: usize) -> Option<u8> {
        self.b.get(self.i + k).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else if c & 0xC0 != 0x80 {
            // A UTF-8 continuation byte does not start a new column.
            self.col += 1;
        }
        Some(c)
    }

    /// White space and comments.
    fn skip(&mut self) -> Result<(), Refusal> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => {
                    self.bump();
                }
                Some(b'/') if self.at(1) == Some(b'*') => {
                    let (l, c) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    loop {
                        match self.bump() {
                            None => return Err(syntax(l, c, "unterminated comment")),
                            Some(b'*') if self.peek() == Some(b'/') => {
                                self.bump();
                                break;
                            }
                            Some(_) => {}
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// The next token and its position.
    fn next(&mut self) -> Result<(Tok, u32, u32), Refusal> {
        self.skip()?;
        let (l, c) = (self.line, self.col);
        let Some(ch) = self.peek() else { return Ok((Tok::Eof, l, c)) };
        let t = match ch {
            b'(' => {
                self.bump();
                Tok::LParen
            }
            b')' => {
                self.bump();
                Tok::RParen
            }
            b',' => {
                self.bump();
                Tok::Comma
            }
            b'=' => {
                self.bump();
                Tok::Eq
            }
            b';' => {
                self.bump();
                Tok::Semi
            }
            b'$' => {
                self.bump();
                Tok::Dollar
            }
            b'*' => {
                self.bump();
                Tok::Star
            }
            b'#' => {
                self.bump();
                let mut n: u64 = 0;
                let mut any = false;
                while let Some(d @ b'0'..=b'9') = self.peek() {
                    self.bump();
                    any = true;
                    n = n.checked_mul(10).and_then(|n| n.checked_add((d - b'0') as u64)).ok_or(syntax(l, c, "instance number too large"))?;
                }
                if !any {
                    return Err(syntax(l, c, "instance number"));
                }
                Tok::Inst(n)
            }
            b'\'' => Tok::Str(self.string(l, c)?),
            b'"' => {
                self.bump();
                let mut s = String::new();
                loop {
                    match self.bump() {
                        Some(b'"') => break,
                        Some(d) if hex(d).is_some() => s.push(d as char),
                        Some(_) => return Err(syntax(l, c, "binary digit")),
                        None => return Err(syntax(l, c, "unterminated binary")),
                    }
                }
                Tok::Bin(s)
            }
            b'.' => {
                self.bump();
                let mut s = String::new();
                while let Some(d) = self.peek() {
                    if d.is_ascii_alphanumeric() || d == b'_' {
                        self.bump();
                        s.push(d.to_ascii_uppercase() as char);
                    } else {
                        break;
                    }
                }
                if s.is_empty() || self.bump() != Some(b'.') {
                    return Err(syntax(l, c, "enumeration"));
                }
                Tok::Enum(s)
            }
            b'0'..=b'9' | b'+' | b'-' => self.number(l, c)?,
            b'!' | b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                let mut s = String::new();
                if ch == b'!' {
                    self.bump();
                    s.push('!');
                }
                while let Some(d) = self.peek() {
                    if d.is_ascii_alphanumeric() || d == b'_' || d == b'-' {
                        self.bump();
                        s.push(d.to_ascii_uppercase() as char);
                    } else {
                        break;
                    }
                }
                if s.contains('-') && s != "ISO-10303-21" && s != "END-ISO-10303-21" {
                    return Err(syntax(l, c, "keyword"));
                }
                Tok::Kw(s)
            }
            _ => return Err(syntax(l, c, "unexpected character")),
        };
        Ok((t, l, c))
    }

    fn number(&mut self, l: u32, c: u32) -> Result<Tok, Refusal> {
        let start = self.i;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.bump();
        }
        let mut digits = 0;
        while let Some(b'0'..=b'9') = self.peek() {
            self.bump();
            digits += 1;
        }
        if digits == 0 {
            return Err(syntax(l, c, "number"));
        }
        let mut real = false;
        if self.peek() == Some(b'.') {
            real = true;
            self.bump();
            while let Some(b'0'..=b'9') = self.peek() {
                self.bump();
            }
        }
        if matches!(self.peek(), Some(b'E' | b'e')) {
            real = true;
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            let mut ed = 0;
            while let Some(b'0'..=b'9') = self.peek() {
                self.bump();
                ed += 1;
            }
            if ed == 0 {
                return Err(syntax(l, c, "real exponent"));
            }
        }
        let text = core::str::from_utf8(&self.b[start..self.i]).map_err(|_| syntax(l, c, "number"))?;
        if real {
            text.parse::<f64>().map(Tok::Real).map_err(|_| syntax(l, c, "real"))
        } else {
            text.parse::<i64>().map(Tok::Int).map_err(|_| syntax(l, c, "integer"))
        }
    }

    /// A STRING from its opening apostrophe at (l, c), its control directives decoded to UTF-8.
    fn string(&mut self, l: u32, c: u32) -> Result<String, Refusal> {
        self.bump();
        let mut out: Vec<u8> = Vec::new();
        let unterminated = Refusal::UnterminatedString { line: l, col: c };
        loop {
            let Some(ch) = self.bump() else { return Err(unterminated) };
            match ch {
                b'\'' => {
                    if self.peek() == Some(b'\'') {
                        self.bump();
                        out.push(b'\'');
                    } else {
                        break;
                    }
                }
                b'\r' | b'\n' => {}
                b'\\' => {
                    let (dl, dc) = (self.line, self.col);
                    let bad = syntax(dl, dc, "string control directive");
                    match self.bump() {
                        Some(b'\\') => out.push(b'\\'),
                        Some(b'S') => {
                            if self.bump() != Some(b'\\') {
                                return Err(bad);
                            }
                            let Some(x) = self.bump() else { return Err(unterminated) };
                            if !(0x20..0x7F).contains(&x) {
                                return Err(bad);
                            }
                            push_char(&mut out, char::from(x + 128));
                        }
                        Some(b'P') => {
                            let page = self.bump();
                            if self.bump() != Some(b'\\') {
                                return Err(bad);
                            }
                            if page != Some(b'A') {
                                return Err(syntax(dl, dc, "code page"));
                            }
                        }
                        Some(b'X') => match self.bump() {
                            Some(b'\\') => {
                                let (h, lo) = (self.bump().and_then(hex), self.bump().and_then(hex));
                                let (Some(h), Some(lo)) = (h, lo) else { return Err(bad) };
                                push_char(&mut out, char::from((h * 16 + lo) as u8));
                            }
                            Some(w @ (b'2' | b'4')) => {
                                if self.bump() != Some(b'\\') {
                                    return Err(bad);
                                }
                                let n = if w == b'2' { 4 } else { 8 };
                                loop {
                                    if self.peek() == Some(b'\\') {
                                        self.bump();
                                        if self.bump() != Some(b'X') || self.bump() != Some(b'0') || self.bump() != Some(b'\\') {
                                            return Err(bad);
                                        }
                                        break;
                                    }
                                    let mut v: u32 = 0;
                                    for _ in 0..n {
                                        let Some(d) = self.bump().and_then(hex) else { return Err(bad) };
                                        v = v * 16 + d;
                                    }
                                    let Some(ch) = char::from_u32(v) else { return Err(bad) };
                                    push_char(&mut out, ch);
                                }
                            }
                            _ => return Err(bad),
                        },
                        None => return Err(unterminated),
                        Some(_) => return Err(bad),
                    }
                }
                _ => out.push(ch),
            }
        }
        String::from_utf8(out).map_err(|_| syntax(l, c, "string encoding"))
    }
}

struct Parser<'a> {
    lx: Lexer<'a>,
    tok: Tok,
    line: u32,
    col: u32,
}

const MAX_DEPTH: u32 = 64;

impl<'a> Parser<'a> {
    fn advance(&mut self) -> Result<(), Refusal> {
        let (t, l, c) = self.lx.next()?;
        self.tok = t;
        self.line = l;
        self.col = c;
        Ok(())
    }

    fn err(&self, what: &'static str) -> Refusal {
        syntax(self.line, self.col, what)
    }

    fn expect(&mut self, t: Tok, what: &'static str) -> Result<(), Refusal> {
        if self.tok != t {
            return Err(self.err(what));
        }
        self.advance()
    }

    fn keyword(&mut self, k: &str, what: &'static str) -> Result<(), Refusal> {
        if self.tok != Tok::Kw(String::from(k)) {
            return Err(self.err(what));
        }
        self.advance()
    }

    /// `( p, p, ... )` with the current token on the `(`.
    fn params(&mut self, depth: u32) -> Result<Vec<Param>, Refusal> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting deeper than 64"));
        }
        self.expect(Tok::LParen, "'('")?;
        let mut v = Vec::new();
        if self.tok == Tok::RParen {
            self.advance()?;
            return Ok(v);
        }
        loop {
            v.push(self.param(depth)?);
            match self.tok {
                Tok::Comma => self.advance()?,
                Tok::RParen => {
                    self.advance()?;
                    return Ok(v);
                }
                _ => return Err(self.err("',' or ')'")),
            }
        }
    }

    fn param(&mut self, depth: u32) -> Result<Param, Refusal> {
        let p = match core::mem::replace(&mut self.tok, Tok::Eof) {
            Tok::Int(i) => Param::Int(i),
            Tok::Real(r) => Param::Real(r),
            Tok::Str(s) => Param::Str(s),
            Tok::Enum(s) => Param::Enum(s),
            Tok::Bin(s) => Param::Bin(s),
            Tok::Inst(id) => Param::Ref { id, line: self.line, col: self.col },
            Tok::Dollar => Param::Null,
            Tok::Star => Param::Derived,
            Tok::LParen => {
                self.tok = Tok::LParen;
                return Ok(Param::List(self.params(depth + 1)?));
            }
            Tok::Kw(name) => {
                self.advance()?;
                if self.tok != Tok::LParen {
                    return Err(self.err("'(' after a typed parameter's name"));
                }
                self.advance()?;
                let inner = self.param(depth + 1)?;
                self.expect(Tok::RParen, "')' closing a typed parameter")?;
                return Ok(Param::Typed(name, Box::new(inner)));
            }
            t => {
                self.tok = t;
                return Err(self.err("parameter"));
            }
        };
        self.advance()?;
        Ok(p)
    }

    fn record(&mut self) -> Result<Record, Refusal> {
        let Tok::Kw(name) = core::mem::replace(&mut self.tok, Tok::Eof) else { return Err(self.err("entity name")) };
        self.advance()?;
        let params = self.params(0)?;
        Ok(Record { name, params })
    }
}

/// Parse a Part 21 file into its instance table. After the parse every reference is resolved, and the first one
/// (in file order) to an undefined instance refuses with its own position.
pub fn parse(text: &str) -> Result<Table, Refusal> {
    let mut p = Parser { lx: Lexer::new(text), tok: Tok::Eof, line: 1, col: 1 };
    p.advance()?;
    p.keyword("ISO-10303-21", "'ISO-10303-21'")?;
    p.expect(Tok::Semi, "';'")?;
    p.keyword("HEADER", "'HEADER'")?;
    p.expect(Tok::Semi, "';'")?;
    let mut t = Table::default();
    while p.tok != Tok::Kw(String::from("ENDSEC")) {
        if !matches!(p.tok, Tok::Kw(_)) {
            return Err(p.err("header record"));
        }
        let r = p.record()?;
        p.expect(Tok::Semi, "';'")?;
        t.header.push(r);
    }
    for need in ["FILE_DESCRIPTION", "FILE_NAME", "FILE_SCHEMA"] {
        if t.header(need).is_none() {
            return Err(p.err(match need {
                "FILE_DESCRIPTION" => "header: FILE_DESCRIPTION missing",
                "FILE_NAME" => "header: FILE_NAME missing",
                _ => "header: FILE_SCHEMA missing",
            }));
        }
    }
    p.advance()?;
    p.expect(Tok::Semi, "';'")?;
    let mut sections = 0;
    while p.tok == Tok::Kw(String::from("DATA")) {
        sections += 1;
        p.advance()?;
        if p.tok == Tok::LParen {
            p.params(0)?;
        }
        p.expect(Tok::Semi, "';'")?;
        loop {
            let tok = p.tok.clone();
            match tok {
                Tok::Kw(ref k) if k == "ENDSEC" => {
                    p.advance()?;
                    p.expect(Tok::Semi, "';'")?;
                    break;
                }
                Tok::Inst(id) => {
                    let (line, col) = (p.line, p.col);
                    p.advance()?;
                    p.expect(Tok::Eq, "'='")?;
                    let body = match p.tok.clone() {
                        Tok::Kw(_) => Body::Simple(p.record()?),
                        Tok::LParen => {
                            p.advance()?;
                            let mut rs = Vec::new();
                            while p.tok != Tok::RParen {
                                if !matches!(p.tok, Tok::Kw(_)) {
                                    return Err(p.err("partial entity name"));
                                }
                                rs.push(p.record()?);
                            }
                            if rs.is_empty() {
                                return Err(p.err("empty complex instance"));
                            }
                            p.advance()?;
                            Body::Complex(rs)
                        }
                        _ => return Err(p.err("entity")),
                    };
                    p.expect(Tok::Semi, "';'")?;
                    if t.data.contains_key(&id) {
                        return Err(Refusal::DuplicateInstance { instance: id, line, col });
                    }
                    t.data.insert(id, Instance { id, line, col, body });
                }
                _ => return Err(p.err("instance or 'ENDSEC'")),
            }
        }
    }
    if sections == 0 {
        return Err(p.err("'DATA'"));
    }
    p.keyword("END-ISO-10303-21", "'END-ISO-10303-21'")?;
    p.expect(Tok::Semi, "';'")?;
    check_refs(&t)?;
    Ok(t)
}

fn dangling(t: &Table, p: &Param, worst: &mut Option<(u32, u32, u64)>) {
    match p {
        Param::Ref { id, line, col } => {
            if !t.data.contains_key(id) && worst.is_none_or(|(l, c, _)| (*line, *col) < (l, c)) {
                *worst = Some((*line, *col, *id));
            }
        }
        Param::List(v) => v.iter().for_each(|q| dangling(t, q, worst)),
        Param::Typed(_, q) => dangling(t, q, worst),
        _ => {}
    }
}

fn check_refs(t: &Table) -> Result<(), Refusal> {
    let mut worst = None;
    for inst in t.data.values() {
        for r in inst.records() {
            for p in &r.params {
                dangling(t, p, &mut worst);
            }
        }
    }
    match worst {
        Some((line, col, instance)) => Err(Refusal::Undefined { instance, line, col }),
        None => Ok(()),
    }
}

/// A REAL in Part 21's spelling: Rust's shortest round-trip digits (so a written value reads back bit-exact), with
/// the decimal point the grammar requires and an upper-case exponent ("1000.0", "1.E-7", "-2.5E21"). The caller
/// refuses non-finite values first.
pub fn real(x: f64) -> String {
    let s = format!("{:?}", x);
    match s.split_once('e') {
        Some((m, e)) => {
            if m.contains('.') {
                format!("{m}E{e}")
            } else {
                format!("{m}.E{e}")
            }
        }
        None => {
            if s.contains('.') {
                s
            } else {
                format!("{s}.")
            }
        }
    }
}

/// A STRING in Part 21's spelling, quotes included: `'` and `\` doubled, printable ASCII as is, other characters
/// as `\X2\hhhh\X0\` (or `\X4\hhhhhhhh\X0\` past the basic plane).
pub fn string(s: &str) -> String {
    let mut o = String::from("'");
    for ch in s.chars() {
        match ch {
            '\'' => o.push_str("''"),
            '\\' => o.push_str("\\\\"),
            ' '..='~' => o.push(ch),
            _ => {
                let v = ch as u32;
                if v <= 0xFFFF {
                    o.push_str(&format!("\\X2\\{v:04X}\\X0\\"));
                } else {
                    o.push_str(&format!("\\X4\\{v:08X}\\X0\\"));
                }
            }
        }
    }
    o.push('\'');
    o
}
