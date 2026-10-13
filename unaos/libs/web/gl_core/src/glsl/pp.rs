//! The GLSL ES 1.00 preprocessor (GLSL ES §3.4) and lexer (§3.1–§3.3, §3.6–§3.8), in one pass that hands the
//! parser a token stream with line numbers.
//!
//! Steps: comments become a space (newlines inside a block comment kept, so line numbers stay true; §3.3);
//! the WebGL 1.0 "Characters Outside the GLSL Source Character Set" character set is checked on what remains; each line is a directive (first non-blank
//! is `#`) or source. Source lines are tokenized into a buffer that is macro-expanded and flushed before
//! every directive (a directive changes the macro table, and a macro's meaning is the one at its use).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f32),
    /// An operator or punctuator (§3.8 plus `#`).
    P(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub t: Tok,
    pub line: u32,
}

/// Punctuators, longest first (so the scan is a greedy prefix match).
const PUNCT: &[&str] = &[
    "<<=", ">>=", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "^^", "+=", "-=", "*=", "/=", "%=", "&=", "^=", "|=", "(", ")", "[", "]",
    "{", "}", ".", ",", ";", "?", ":", "+", "-", "*", "/", "%", "<", ">", "=", "!", "~", "&", "|", "^", "#",
];

#[derive(Clone, Debug)]
struct Macro {
    params: Option<Vec<String>>,
    body: Vec<Tok>,
}

/// An `#extension` directive's effect on the shader.
#[derive(Clone, Debug, Default)]
pub struct ExtState {
    pub standard_derivatives: bool,
}

pub struct Output {
    pub tokens: Vec<Token>,
    pub errors: Vec<(u32, String)>,
    pub warnings: Vec<(u32, String)>,
    pub ext: ExtState,
}

/// WebGL 1.0 "Characters Outside the GLSL Source Character Set": the characters a shader may hold outside comments (the GLSL ES §3.1 set).
fn valid_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_.+-/*%<>[](){}^|&~=!:;,?# \t\n\r\x0b\x0c".contains(c)
}

/// Remove comments (each becomes one space; a block comment keeps its newlines). Returns the text and the
/// line of the first invalid character outside a comment, if any.
fn strip_comments(src: &str) -> (String, Option<(u32, char)>, bool) {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let (mut i, mut line, mut bad) = (0usize, 1u32, None);
    let mut unterminated = false;
    while i < b.len() {
        let c = b[i];
        if c == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            out.push(' ');
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            out.push(' ');
            let mut closed = false;
            while i < b.len() {
                if b[i] == '*' && i + 1 < b.len() && b[i + 1] == '/' {
                    i += 2;
                    closed = true;
                    break;
                }
                if b[i] == '\n' {
                    out.push('\n');
                    line += 1;
                }
                i += 1;
            }
            if !closed {
                unterminated = true;
            }
            continue;
        }
        if c == '\n' {
            line += 1;
        }
        if bad.is_none() && !valid_char(c) {
            bad = Some((line, c));
        }
        out.push(c);
        i += 1;
    }
    (out, bad, unterminated)
}

/// Tokenize one line. Errors are (message) for the caller to stamp with the line.
fn lex_line(s: &str, line: u32, out: &mut Vec<Token>, errs: &mut Vec<(u32, String)>) {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b' ' || c == b'\t' || c == b'\r' || c == 0x0b || c == 0x0c {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Token { t: Tok::Ident(s[st..i].to_string()), line });
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let st = i;
            let mut float = false;
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                i += 2;
                let ds = i;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let v = i64::from_str_radix(&s[ds..i], 16).ok();
                if ds == i || v.is_none() || (i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_')) {
                    errs.push((line, format!("'{}' : invalid hexadecimal constant", &s[st..i.min(b.len())])));
                    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                        i += 1;
                    }
                    continue;
                }
                out.push(Token { t: Tok::Int(v.unwrap()), line });
                continue;
            }
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' {
                float = true;
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let save = i;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                if i < b.len() && b[i].is_ascii_digit() {
                    float = true;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            let text = &s[st..i];
            if i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                // GLSL ES 1.00 has no literal suffixes (no `f`, no `u`).
                let e = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                errs.push((line, format!("'{}' : invalid number", &s[st..i.max(e)])));
                continue;
            }
            if float {
                out.push(Token { t: Tok::Float(parse_float(text)), line });
            } else if text.len() > 1 && text.starts_with('0') {
                match i64::from_str_radix(&text[1..], 8) {
                    Ok(v) => out.push(Token { t: Tok::Int(v), line }),
                    Err(_) => errs.push((line, format!("'{text}' : invalid octal constant"))),
                }
            } else {
                match text.parse::<i64>() {
                    Ok(v) if v <= u32::MAX as i64 => out.push(Token { t: Tok::Int(v), line }),
                    _ => errs.push((line, format!("'{text}' : integer constant overflow"))),
                }
            }
            continue;
        }
        let mut matched = false;
        for p in PUNCT {
            if s[i..].starts_with(p) {
                out.push(Token { t: Tok::P(p), line });
                i += p.len();
                matched = true;
                break;
            }
        }
        if !matched {
            errs.push((line, format!("'{}' : invalid character", c as char)));
            i += 1;
        }
    }
}

/// Decimal float text → f32 (correctly rounded through f64 digit accumulation; shader literals are short).
pub fn parse_float(t: &str) -> f32 {
    t.parse::<f64>().map(|v| v as f32).unwrap_or(0.0)
}

pub struct Pre<'a> {
    macros: BTreeMap<String, Macro>,
    errors: Vec<(u32, String)>,
    warnings: Vec<(u32, String)>,
    ext: ExtState,
    derivatives_supported: bool,
    out: Vec<Token>,
    pending: Vec<Token>,
    _src: &'a str,
}

/// The #if stack entry: (this group active, some group of the chain already taken, #else seen, parent active).
struct Cond {
    active: bool,
    taken: bool,
    seen_else: bool,
    parent: bool,
}

pub fn preprocess(src: &str, fragment: bool, derivatives_supported: bool) -> Output {
    let mut p = Pre {
        macros: BTreeMap::new(),
        errors: Vec::new(),
        warnings: Vec::new(),
        ext: ExtState::default(),
        derivatives_supported,
        out: Vec::new(),
        pending: Vec::new(),
        _src: src,
    };
    let obj = |v: i64| Macro { params: None, body: vec![Tok::Int(v)] };
    p.macros.insert("GL_ES".into(), obj(1));
    p.macros.insert("__VERSION__".into(), obj(100));
    if fragment {
        p.macros.insert("GL_FRAGMENT_PRECISION_HIGH".into(), obj(1));
    }
    let (text, bad, unterminated) = strip_comments(src);
    if let Some((l, c)) = bad {
        p.errors.push((l, format!("'{}' : invalid character in shader source (WebGL 1.0 'Characters Outside the GLSL Source Character Set')", c.escape_default())));
    }
    if unterminated {
        p.errors.push((1, "unterminated block comment".into()));
    }
    let mut conds: Vec<Cond> = Vec::new();
    let mut line_base: i64 = 0; // #line offset: reported line = physical + line_base
    let mut seen_code = false;
    for (idx, raw) in text.split('\n').enumerate() {
        let phys = idx as i64 + 1;
        let line = (phys + line_base).max(0) as u32;
        let active = conds.last().is_none_or(|c| c.active);
        let t = raw.trim_start_matches([' ', '\t', '\r', '\x0b', '\x0c']);
        if let Some(rest) = t.strip_prefix('#') {
            p.flush();
            let mut toks = Vec::new();
            let mut errs = Vec::new();
            lex_line(rest, line, &mut toks, &mut errs);
            let name = match toks.first() {
                Some(Token { t: Tok::Ident(n), .. }) => n.clone(),
                None => {
                    continue; // the null directive
                }
                _ => {
                    if active {
                        p.errors.push((line, "invalid directive".into()));
                    }
                    continue;
                }
            };
            let args = &toks[1..];
            match name.as_str() {
                "if" | "ifdef" | "ifndef" => {
                    let v = if !active {
                        false
                    } else if name == "if" {
                        if !errs.is_empty() {
                            p.errors.extend(errs);
                        }
                        p.eval_if(args, line)
                    } else {
                        match args {
                            [Token { t: Tok::Ident(n), .. }] => p.macros.contains_key(n) == (name == "ifdef"),
                            _ => {
                                p.errors.push((line, format!("#{name} : expected a macro name")));
                                false
                            }
                        }
                    };
                    conds.push(Cond { active: active && v, taken: v, seen_else: false, parent: active });
                }
                "elif" => {
                    let Some(c) = conds.last() else {
                        p.errors.push((line, "#elif without #if".into()));
                        continue;
                    };
                    if c.seen_else {
                        p.errors.push((line, "#elif after #else".into()));
                    }
                    let (parent, taken) = (c.parent, c.taken);
                    let v = parent && !taken && p.eval_if(args, line);
                    let c = conds.last_mut().unwrap();
                    c.active = v;
                    c.taken |= v;
                }
                "else" => {
                    let Some(c) = conds.last_mut() else {
                        p.errors.push((line, "#else without #if".into()));
                        continue;
                    };
                    if c.seen_else {
                        p.errors.push((line, "#else after #else".into()));
                    }
                    c.seen_else = true;
                    c.active = c.parent && !c.taken;
                    c.taken = true;
                }
                "endif" => {
                    if conds.pop().is_none() {
                        p.errors.push((line, "#endif without #if".into()));
                    }
                }
                _ if !active => {}
                "define" => {
                    p.errors.extend(errs);
                    p.define(rest, args, line);
                }
                "undef" => match args {
                    [Token { t: Tok::Ident(n), .. }] => {
                        if n.starts_with("GL_") || n == "__LINE__" || n == "__FILE__" || n == "__VERSION__" {
                            p.errors.push((line, format!("'{n}' : predefined macro can not be undefined")));
                        } else {
                            p.macros.remove(n);
                        }
                    }
                    _ => p.errors.push((line, "#undef : expected a macro name".into())),
                },
                "error" => {
                    p.errors.push((line, format!("#error : {}", rest.trim_start().trim_start_matches("error").trim())));
                }
                "pragma" => {}
                "extension" => p.extension(args, line),
                "version" => {
                    if seen_code || !p.out.is_empty() {
                        p.errors.push((line, "#version must occur before anything else".into()));
                    }
                    match args {
                        [Token { t: Tok::Int(100), .. }] => {}
                        _ => p.errors.push((line, "version not supported (WebGL 1.0 accepts #version 100)".into())),
                    }
                }
                "line" => {
                    let mut ex = Vec::new();
                    p.expand(args.to_vec(), &mut ex, &mut Vec::new());
                    match ex.first() {
                        Some(Token { t: Tok::Int(n), .. }) => line_base = *n - phys - 1,
                        _ => p.errors.push((line, "#line : expected a line number".into())),
                    }
                }
                _ => p.errors.push((line, format!("'{name}' : invalid directive"))),
            }
            seen_code = true;
            continue;
        }
        if !active {
            continue;
        }
        let mut errs = Vec::new();
        let before = p.pending.len();
        lex_line(raw, line, &mut p.pending, &mut errs);
        if p.pending.len() > before {
            seen_code = true;
        }
        p.errors.extend(errs);
    }
    p.flush();
    if !conds.is_empty() {
        p.errors.push((text.split('\n').count() as u32, "unexpected end of file: missing #endif".into()));
    }
    Output { tokens: p.out, errors: p.errors, warnings: p.warnings, ext: p.ext }
}

impl Pre<'_> {
    fn flush(&mut self) {
        let toks = core::mem::take(&mut self.pending);
        let mut out = Vec::new();
        self.expand(toks, &mut out, &mut Vec::new());
        self.out.extend(out);
    }

    fn define(&mut self, rest: &str, args: &[Token], line: u32) {
        let Some(Token { t: Tok::Ident(name), .. }) = args.first() else {
            self.errors.push((line, "#define : expected a macro name".into()));
            return;
        };
        if name.starts_with("GL_") || name == "defined" || name == "__LINE__" || name == "__FILE__" || name == "__VERSION__" {
            self.errors.push((line, format!("'{name}' : macro names beginning with GL_ are reserved (GLSL ES §3.4)")));
            return;
        }
        // Function-like only if `(` follows the name with no space between (C rule, §3.4).
        let after = rest.trim_start().trim_start_matches("define").trim_start();
        let function_like = after.strip_prefix(name.as_str()).is_some_and(|r| r.starts_with('('));
        let (params, body) = if function_like {
            let mut ps = Vec::new();
            let mut i = 2;
            loop {
                match args.get(i).map(|t| &t.t) {
                    Some(Tok::P(")")) if ps.is_empty() => {
                        i += 1;
                        break;
                    }
                    Some(Tok::Ident(p)) => {
                        if ps.contains(p) {
                            self.errors.push((line, format!("'{p}' : duplicate macro parameter")));
                        }
                        ps.push(p.clone());
                        i += 1;
                        match args.get(i).map(|t| &t.t) {
                            Some(Tok::P(",")) => i += 1,
                            Some(Tok::P(")")) => {
                                i += 1;
                                break;
                            }
                            _ => {
                                self.errors.push((line, "#define : bad macro parameter list".into()));
                                return;
                            }
                        }
                    }
                    _ => {
                        self.errors.push((line, "#define : bad macro parameter list".into()));
                        return;
                    }
                }
            }
            (Some(ps), args[i..].iter().map(|t| t.t.clone()).collect::<Vec<_>>())
        } else {
            (None, args[1..].iter().map(|t| t.t.clone()).collect::<Vec<_>>())
        };
        let m = Macro { params, body };
        if let Some(old) = self.macros.get(name) {
            if old.params != m.params || old.body != m.body {
                self.errors.push((line, format!("'{name}' : macro redefined")));
                return;
            }
        }
        self.macros.insert(name.clone(), m);
    }

    fn extension(&mut self, args: &[Token], line: u32) {
        let (name, behavior) = match args {
            [Token { t: Tok::Ident(n), .. }, Token { t: Tok::P(":"), .. }, Token { t: Tok::Ident(b), .. }] => (n.as_str(), b.as_str()),
            _ => {
                self.errors.push((line, "#extension : syntax error".into()));
                return;
            }
        };
        if !matches!(behavior, "require" | "enable" | "warn" | "disable") {
            self.errors.push((line, format!("'{behavior}' : unknown extension behavior")));
            return;
        }
        if name == "all" {
            if matches!(behavior, "require" | "enable") {
                self.errors.push((line, format!("'all' : extension behavior '{behavior}' is not allowed with 'all'")));
            } else {
                self.ext.standard_derivatives = false;
            }
            return;
        }
        let known = name == "GL_OES_standard_derivatives" && self.derivatives_supported;
        if !known {
            match behavior {
                "require" => self.errors.push((line, format!("'{name}' : extension is not supported"))),
                "enable" | "warn" => self.warnings.push((line, format!("'{name}' : extension is not supported"))),
                _ => {}
            }
            return;
        }
        self.ext.standard_derivatives = matches!(behavior, "require" | "enable" | "warn");
    }

    /// Expand macros in `toks` into `out`. `hide` lists the macros being expanded (no self-recursion).
    fn expand(&mut self, toks: Vec<Token>, out: &mut Vec<Token>, hide: &mut Vec<String>) {
        let mut i = 0;
        while i < toks.len() {
            let tk = &toks[i];
            let line = tk.line;
            let Tok::Ident(name) = &tk.t else {
                out.push(tk.clone());
                i += 1;
                continue;
            };
            if name == "__LINE__" {
                out.push(Token { t: Tok::Int(line as i64), line });
                i += 1;
                continue;
            }
            if name == "__FILE__" {
                out.push(Token { t: Tok::Int(0), line });
                i += 1;
                continue;
            }
            if hide.contains(name) || !self.macros.contains_key(name) {
                out.push(tk.clone());
                i += 1;
                continue;
            }
            let m = self.macros.get(name).unwrap().clone();
            let name = name.clone();
            match &m.params {
                None => {
                    let body: Vec<Token> = m.body.iter().map(|t| Token { t: t.clone(), line }).collect();
                    hide.push(name);
                    self.expand(body, out, hide);
                    hide.pop();
                    i += 1;
                }
                Some(params) => {
                    if !matches!(toks.get(i + 1).map(|t| &t.t), Some(Tok::P("("))) {
                        out.push(tk.clone());
                        i += 1;
                        continue;
                    }
                    // Collect the arguments: top-level commas split, parentheses nest.
                    let mut args: Vec<Vec<Token>> = vec![Vec::new()];
                    let mut depth = 0;
                    let mut j = i + 2;
                    let mut closed = false;
                    while j < toks.len() {
                        match &toks[j].t {
                            Tok::P("(") => {
                                depth += 1;
                                args.last_mut().unwrap().push(toks[j].clone());
                            }
                            Tok::P(")") if depth == 0 => {
                                closed = true;
                                break;
                            }
                            Tok::P(")") => {
                                depth -= 1;
                                args.last_mut().unwrap().push(toks[j].clone());
                            }
                            Tok::P(",") if depth == 0 => args.push(Vec::new()),
                            _ => args.last_mut().unwrap().push(toks[j].clone()),
                        }
                        j += 1;
                    }
                    if !closed {
                        self.errors.push((line, format!("'{name}' : unterminated macro invocation")));
                        return;
                    }
                    if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                        args.clear();
                    }
                    if args.len() != params.len() {
                        self.errors.push((line, format!("'{name}' : wrong number of macro arguments")));
                        i = j + 1;
                        continue;
                    }
                    // Arguments are fully expanded before substitution (C's prescan).
                    let mut expanded: Vec<Vec<Token>> = Vec::new();
                    for a in args {
                        let mut e = Vec::new();
                        self.expand(a, &mut e, hide);
                        expanded.push(e);
                    }
                    let mut body = Vec::new();
                    for t in &m.body {
                        if let Tok::Ident(n) = t {
                            if let Some(k) = params.iter().position(|p| p == n) {
                                body.extend(expanded[k].iter().map(|x| Token { t: x.t.clone(), line }));
                                continue;
                            }
                        }
                        body.push(Token { t: t.clone(), line });
                    }
                    hide.push(name);
                    self.expand(body, out, hide);
                    hide.pop();
                    i = j + 1;
                }
            }
        }
    }

    /// `#if` / `#elif`: `defined` resolved first, then macros expanded, then an integer expression (§3.4:
    /// an identifier left after expansion is an error).
    fn eval_if(&mut self, args: &[Token], line: u32) -> bool {
        let mut pre = Vec::new();
        let mut i = 0;
        while i < args.len() {
            if args[i].t == Tok::Ident("defined".into()) {
                let (n, step) = match (args.get(i + 1).map(|t| &t.t), args.get(i + 2).map(|t| &t.t), args.get(i + 3).map(|t| &t.t)) {
                    (Some(Tok::P("(")), Some(Tok::Ident(n)), Some(Tok::P(")"))) => (Some(n.clone()), 4),
                    (Some(Tok::Ident(n)), _, _) => (Some(n.clone()), 2),
                    _ => (None, 1),
                };
                match n {
                    Some(n) => pre.push(Token { t: Tok::Int(self.macros.contains_key(&n) as i64), line }),
                    None => {
                        self.errors.push((line, "'defined' : expected a macro name".into()));
                        return false;
                    }
                }
                i += step;
                continue;
            }
            pre.push(args[i].clone());
            i += 1;
        }
        let mut ex = Vec::new();
        self.expand(pre, &mut ex, &mut Vec::new());
        let mut ev = IfEval { t: &ex, i: 0, err: None };
        let v = ev.expr(0);
        if ev.err.is_none() && ev.i < ex.len() {
            ev.err = Some("unexpected token in preprocessor expression".into());
        }
        match ev.err {
            Some(e) => {
                self.errors.push((line, e));
                false
            }
            None => v != 0,
        }
    }
}

/// The #if expression grammar (C's integer operators, GLSL ES §3.4's precedence table).
struct IfEval<'a> {
    t: &'a [Token],
    i: usize,
    err: Option<String>,
}

impl IfEval<'_> {
    fn prec(op: &str) -> Option<u8> {
        Some(match op {
            "||" => 1,
            "&&" => 2,
            "|" => 3,
            "^" => 4,
            "&" => 5,
            "==" | "!=" => 6,
            "<" | ">" | "<=" | ">=" => 7,
            "<<" | ">>" => 8,
            "+" | "-" => 9,
            "*" | "/" | "%" => 10,
            _ => return None,
        })
    }

    fn primary(&mut self) -> i64 {
        let Some(tk) = self.t.get(self.i) else {
            self.err.get_or_insert("unexpected end of preprocessor expression".into());
            return 0;
        };
        self.i += 1;
        match &tk.t {
            Tok::Int(v) => *v,
            Tok::P("(") => {
                let v = self.expr(0);
                if !matches!(self.t.get(self.i).map(|t| &t.t), Some(Tok::P(")"))) {
                    self.err.get_or_insert("missing ')' in preprocessor expression".into());
                }
                self.i += 1;
                v
            }
            Tok::P("-") => self.primary().wrapping_neg(),
            Tok::P("+") => self.primary(),
            Tok::P("!") => (self.primary() == 0) as i64,
            Tok::P("~") => !self.primary(),
            Tok::Ident(n) => {
                self.err.get_or_insert(format!("'{n}' : undefined identifier in preprocessor expression"));
                0
            }
            _ => {
                self.err.get_or_insert("invalid preprocessor expression".into());
                0
            }
        }
    }

    fn expr(&mut self, min: u8) -> i64 {
        let mut lhs = self.primary();
        loop {
            let op = match self.t.get(self.i).map(|t| &t.t) {
                Some(Tok::P(op)) => *op,
                _ => break,
            };
            let Some(p) = Self::prec(op) else { break };
            if p <= min {
                break;
            }
            self.i += 1;
            let rhs = self.expr(p);
            lhs = match op {
                "||" => (lhs != 0 || rhs != 0) as i64,
                "&&" => (lhs != 0 && rhs != 0) as i64,
                "|" => lhs | rhs,
                "^" => lhs ^ rhs,
                "&" => lhs & rhs,
                "==" => (lhs == rhs) as i64,
                "!=" => (lhs != rhs) as i64,
                "<" => (lhs < rhs) as i64,
                ">" => (lhs > rhs) as i64,
                "<=" => (lhs <= rhs) as i64,
                ">=" => (lhs >= rhs) as i64,
                "<<" => lhs.wrapping_shl(rhs as u32),
                ">>" => lhs.wrapping_shr(rhs as u32),
                "+" => lhs.wrapping_add(rhs),
                "-" => lhs.wrapping_sub(rhs),
                "*" => lhs.wrapping_mul(rhs),
                "/" | "%" => {
                    if rhs == 0 {
                        self.err.get_or_insert("division by zero in preprocessor expression".into());
                        0
                    } else if op == "/" {
                        lhs.wrapping_div(rhs)
                    } else {
                        lhs.wrapping_rem(rhs)
                    }
                }
                _ => lhs,
            };
        }
        lhs
    }
}
