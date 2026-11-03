// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The reader: a byte cursor over one line at a time (pdf_core `Lexer`'s shape: a position, comments to the end of the
//! line, backslash escapes inside strings) producing `<verb> <id>? key=value...`, then the typing of each statement
//! by its verb (timeline_core `edl::parse`'s shape: line-numbered, every refusal named). References are checked HERE
//! only for existence, order (declared on an earlier line), sketch membership and statement kind; whether an entity is
//! of the kind a constraint needs is sketch_core's `Constraint::check`, reported by replay.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use make_core::{Aabb3, Axis, Feeds, Fixture, Op, Power, Stock, StockShape, Tool, Vec2, Vec3};

use crate::{
    AXES, Cons, Doc, FORMAT, KINEMATICS, MACHINE_KINDS, MachineStmt, POWER_KINDS, PRef, Pose, Refusal, SIDES, Stmt,
    TOOL_KINDS, Tol, UNITS, VERSION, What, of_word,
};

/// A lexed value and the column it starts at.
#[derive(Clone, Debug)]
struct V {
    val: Val,
    col: u32,
}

#[derive(Clone, Debug)]
enum Val {
    Str(String),
    Word(String),
    List(Vec<V>),
}

struct Item {
    key: String,
    kcol: u32,
    v: V,
}

struct Toks {
    verb: String,
    vcol: u32,
    id: Option<(String, u32)>,
    items: Vec<Item>,
}

/// A word ends at white space or one of these.
fn is_stop(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'"' | b'[' | b']' | b'#' | b'=')
}

fn valid_verb(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

fn valid_key(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

struct Cur<'a> {
    s: &'a str,
    pos: usize,
    line: u32,
}

impl<'a> Cur<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }
    fn col(&self) -> u32 {
        self.pos as u32 + 1
    }
    fn at(&self, col: u32, what: What) -> Refusal {
        Refusal { line: self.line, col, what }
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.pos += 1;
        }
    }
    /// The end of the statement: the end of the line or a comment.
    fn at_end(&self) -> bool {
        matches!(self.peek(), None | Some(b'#'))
    }
    fn word(&mut self) -> &'a str {
        let st = self.pos;
        while let Some(b) = self.peek() {
            if is_stop(b) {
                break;
            }
            self.pos += 1;
        }
        &self.s[st..self.pos]
    }
    /// A string, the cursor on its opening quote: the ONE escaping rule.
    fn string(&mut self) -> Result<String, Refusal> {
        let open = self.col();
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.s[self.pos..].chars().next() else {
                return Err(self.at(open, What::Unterminated));
            };
            match c {
                '"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                '\\' => {
                    let ecol = self.col();
                    self.pos += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.at(open, What::Unterminated));
                    };
                    self.pos += 1;
                    match e {
                        b'\\' => out.push('\\'),
                        b'"' => out.push('"'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            if self.peek() != Some(b'{') {
                                return Err(self.at(ecol, What::BadEscape));
                            }
                            self.pos += 1;
                            let st = self.pos;
                            while matches!(self.peek(), Some(h) if h.is_ascii_hexdigit()) {
                                self.pos += 1;
                            }
                            let hex = &self.s[st..self.pos];
                            if hex.is_empty() || hex.len() > 6 || self.peek() != Some(b'}') {
                                return Err(self.at(ecol, What::BadEscape));
                            }
                            self.pos += 1;
                            let ch = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
                            match ch {
                                Some(ch) => out.push(ch),
                                None => return Err(self.at(ecol, What::BadEscape)),
                            }
                        }
                        _ => return Err(self.at(ecol, What::BadEscape)),
                    }
                }
                c => {
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }
    /// A value for `key`: a string, a list or a word.
    fn value(&mut self, key: &str) -> Result<V, Refusal> {
        let col = self.col();
        match self.peek() {
            Some(b'"') => Ok(V { val: Val::Str(self.string()?), col }),
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    match self.peek() {
                        None | Some(b'#') => return Err(self.at(col, What::Unterminated)),
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(V { val: Val::List(items), col });
                        }
                        _ => items.push(self.value(key)?),
                    }
                }
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    return Err(self.at(col, What::BadValue { key: key.to_string() }));
                }
                Ok(V { val: Val::Word(w.to_string()), col })
            }
        }
    }
}

/// One line to its tokens, or None for a blank / comment-only line.
fn lex_line(s: &str, line: u32) -> Result<Option<Toks>, Refusal> {
    let mut c = Cur { s, pos: 0, line };
    c.ws();
    if c.at_end() {
        return Ok(None);
    }
    let vcol = c.col();
    let verb = c.word();
    if !valid_verb(verb) {
        return Err(c.at(vcol, What::BadVerb));
    }
    let mut t = Toks { verb: verb.to_string(), vcol, id: None, items: Vec::new() };
    loop {
        let before = c.pos;
        c.ws();
        if c.at_end() {
            break;
        }
        if c.pos == before {
            return Err(c.at(c.col(), What::Trailing));
        }
        let hcol = c.col();
        let head = c.word();
        if c.peek() == Some(b'=') {
            if !valid_key(head) {
                return Err(c.at(hcol, What::Trailing));
            }
            c.pos += 1;
            if t.items.iter().any(|i| i.key == head) {
                return Err(c.at(hcol, What::DuplicateKey(head.to_string())));
            }
            let v = c.value(head)?;
            t.items.push(Item { key: head.to_string(), kcol: hcol, v });
        } else {
            if head.is_empty() || t.id.is_some() || !t.items.is_empty() {
                return Err(c.at(hcol, What::Trailing));
            }
            if !valid_id(head) {
                return Err(c.at(hcol, What::BadId));
            }
            t.id = Some((head.to_string(), hcol));
        }
    }
    Ok(Some(t))
}

/// Read a document. The first statement is the header; every later line is one statement or blank / comment.
pub fn read(text: &str) -> Result<Doc, Refusal> {
    let mut doc: Option<Doc> = None;
    let mut st = State { decls: BTreeMap::new(), machine: false, stock: false };
    for (n, raw) in text.split('\n').enumerate() {
        let line = n as u32 + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let Some(t) = lex_line(raw, line)? else { continue };
        if let Some(d) = doc.as_mut() {
            let s = statement(t, line, &mut st)?;
            d.stmts.push(s);
            d.lines.push(line);
        } else {
            doc = Some(header(t, line)?);
        }
    }
    doc.ok_or(Refusal { line: 1, col: 1, what: What::NoHeader })
}

fn header(t: Toks, line: u32) -> Result<Doc, Refusal> {
    if t.verb != FORMAT {
        return Err(Refusal { line, col: t.vcol, what: What::NoHeader });
    }
    if let Some((_, col)) = t.id {
        return Err(Refusal { line, col, what: What::Trailing });
    }
    let mut f = Fields { line, vcol: t.vcol, items: t.items };
    let vv = f.req("version")?;
    let version = int(&vv, "version", line)?;
    if version != VERSION as u64 {
        return Err(Refusal { line, col: vv.col, what: What::Version(version.min(u32::MAX as u64) as u32) });
    }
    let lin = f.f64("lin")?;
    let ang = f.f64("ang")?;
    f.done()?;
    Ok(Doc::new(Tol { lin, ang }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Sketch,
    Entity,
    Constraint,
    Feature,
    Machine,
    Op,
    Scan,
}

struct State {
    /// Every declared id: its kind and, for an entity or constraint, its sketch.
    decls: BTreeMap<String, (Kind, Option<String>)>,
    machine: bool,
    stock: bool,
}

impl State {
    fn declare(&mut self, id: (String, u32), line: u32, kind: Kind, sketch: Option<String>) -> Result<String, Refusal> {
        if self.decls.contains_key(&id.0) {
            return Err(Refusal { line, col: id.1, what: What::DuplicateId(id.0) });
        }
        self.decls.insert(id.0.clone(), (kind, sketch));
        Ok(id.0)
    }

    /// A reference to an earlier statement of kind `want` (`sketch=`, `on=`).
    fn target(&self, v: &V, key: &str, line: u32, want: Kind) -> Result<String, Refusal> {
        let name = ref_word(v, key, line)?;
        match self.decls.get(name) {
            None => Err(Refusal { line, col: v.col, what: What::UnknownRef(name.to_string()) }),
            Some((k, _)) if *k != want => Err(Refusal { line, col: v.col, what: What::WrongTarget(name.to_string()) }),
            Some(_) => Ok(name.to_string()),
        }
    }

    /// An entity of sketch `sk` named by `name` (at `col`).
    fn entity_name(&self, name: &str, col: u32, line: u32, sk: &str) -> Result<String, Refusal> {
        match self.decls.get(name) {
            None => Err(Refusal { line, col, what: What::UnknownRef(name.to_string()) }),
            Some((Kind::Entity, Some(s))) if s == sk => Ok(name.to_string()),
            Some((Kind::Entity, _)) => Err(Refusal { line, col, what: What::CrossSketch(name.to_string()) }),
            Some(_) => Err(Refusal { line, col, what: What::WrongTarget(name.to_string()) }),
        }
    }

    fn entity(&self, v: &V, key: &str, line: u32, sk: &str) -> Result<String, Refusal> {
        let name = ref_word(v, key, line)?;
        self.entity_name(name, v.col, line, sk)
    }

    fn pref(&self, v: &V, key: &str, line: u32, sk: &str) -> Result<PRef, Refusal> {
        let Val::Word(w) = &v.val else {
            return Err(Refusal { line, col: v.col, what: What::BadValue { key: key.to_string() } });
        };
        let (base, suffix) = match w.split_once('.') {
            Some((b, s)) => (b, Some(s)),
            None => (w.as_str(), None),
        };
        if !valid_id(base) {
            return Err(Refusal { line, col: v.col, what: What::BadValue { key: key.to_string() } });
        }
        let id = self.entity_name(base, v.col, line, sk)?;
        match suffix {
            None => Ok(PRef::Point(id)),
            Some("centre") => Ok(PRef::Centre(id)),
            Some("start") => Ok(PRef::Start(id)),
            Some("end") => Ok(PRef::End(id)),
            Some(_) => Err(Refusal { line, col: v.col, what: What::BadValue { key: key.to_string() } }),
        }
    }
}

/// A word that is a valid id.
fn ref_word<'v>(v: &'v V, key: &str, line: u32) -> Result<&'v str, Refusal> {
    match &v.val {
        Val::Word(w) if valid_id(w) => Ok(w),
        _ => Err(Refusal { line, col: v.col, what: What::BadValue { key: key.to_string() } }),
    }
}

fn bad(v: &V, key: &str, line: u32) -> Refusal {
    Refusal { line, col: v.col, what: What::BadValue { key: key.to_string() } }
}

fn num(v: &V, key: &str, line: u32) -> Result<f64, Refusal> {
    let Val::Word(w) = &v.val else { return Err(bad(v, key, line)) };
    let x: f64 = w.parse().map_err(|_| Refusal { line, col: v.col, what: What::BadNumber })?;
    if !x.is_finite() {
        return Err(Refusal { line, col: v.col, what: What::NonFinite });
    }
    Ok(x)
}

fn int(v: &V, key: &str, line: u32) -> Result<u64, Refusal> {
    let Val::Word(w) = &v.val else { return Err(bad(v, key, line)) };
    w.parse::<u64>().map_err(|_| Refusal { line, col: v.col, what: What::BadNumber })
}

fn int_max(v: &V, key: &str, line: u32, max: u64) -> Result<u64, Refusal> {
    let n = int(v, key, line)?;
    if n > max {
        return Err(Refusal { line, col: v.col, what: What::BadNumber });
    }
    Ok(n)
}

fn list<'v>(v: &'v V, key: &str, line: u32) -> Result<&'v [V], Refusal> {
    match &v.val {
        Val::List(xs) => Ok(xs),
        _ => Err(bad(v, key, line)),
    }
}

fn nums<const N: usize>(v: &V, key: &str, line: u32) -> Result<[f64; N], Refusal> {
    let xs = list(v, key, line)?;
    if xs.len() != N {
        return Err(bad(v, key, line));
    }
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(xs) {
        *o = num(x, key, line)?;
    }
    Ok(out)
}

fn vec3(v: &V, key: &str, line: u32) -> Result<Vec3, Refusal> {
    let [x, y, z] = nums::<3>(v, key, line)?;
    Ok(Vec3 { x, y, z })
}

fn aabb(v: &V, key: &str, line: u32) -> Result<Aabb3, Refusal> {
    let xs = list(v, key, line)?;
    if xs.len() != 2 {
        return Err(bad(v, key, line));
    }
    Ok(Aabb3 { min: vec3(&xs[0], key, line)?, max: vec3(&xs[1], key, line)? })
}

fn pts(v: &V, key: &str, line: u32) -> Result<Vec<Vec2>, Refusal> {
    let mut out = Vec::new();
    for p in list(v, key, line)? {
        let [x, y] = nums::<2>(p, key, line)?;
        out.push(Vec2 { x, y });
    }
    Ok(out)
}

fn polys(v: &V, key: &str, line: u32) -> Result<Vec<Vec<Vec2>>, Refusal> {
    let mut out = Vec::new();
    for p in list(v, key, line)? {
        out.push(pts(p, key, line)?);
    }
    Ok(out)
}

fn text(v: &V, key: &str, line: u32) -> Result<String, Refusal> {
    match &v.val {
        Val::Str(s) => Ok(s.clone()),
        _ => Err(bad(v, key, line)),
    }
}

fn pick<T: Copy>(v: &V, key: &str, line: u32, table: &[(&'static str, T)]) -> Result<T, Refusal> {
    match &v.val {
        Val::Word(w) => of_word(table, w).ok_or_else(|| bad(v, key, line)),
        _ => Err(bad(v, key, line)),
    }
}

/// The key=value items of one statement, taken by name; whatever is left is an unknown key.
struct Fields {
    line: u32,
    vcol: u32,
    items: Vec<Item>,
}

impl Fields {
    fn opt(&mut self, k: &str) -> Option<V> {
        let i = self.items.iter().position(|it| it.key == k)?;
        Some(self.items.remove(i).v)
    }
    fn req(&mut self, k: &'static str) -> Result<V, Refusal> {
        self.opt(k).ok_or(Refusal { line: self.line, col: self.vcol, what: What::MissingKey(k) })
    }
    fn f64(&mut self, k: &'static str) -> Result<f64, Refusal> {
        let v = self.req(k)?;
        num(&v, k, self.line)
    }
    fn u32(&mut self, k: &'static str) -> Result<u32, Refusal> {
        let v = self.req(k)?;
        Ok(int_max(&v, k, self.line, u32::MAX as u64)? as u32)
    }
    fn nums<const N: usize>(&mut self, k: &'static str) -> Result<[f64; N], Refusal> {
        let v = self.req(k)?;
        nums::<N>(&v, k, self.line)
    }
    fn vec3(&mut self, k: &'static str) -> Result<Vec3, Refusal> {
        let v = self.req(k)?;
        vec3(&v, k, self.line)
    }
    fn pts(&mut self, k: &'static str) -> Result<Vec<Vec2>, Refusal> {
        let v = self.req(k)?;
        pts(&v, k, self.line)
    }
    fn polys(&mut self, k: &'static str) -> Result<Vec<Vec<Vec2>>, Refusal> {
        let v = self.req(k)?;
        polys(&v, k, self.line)
    }
    fn text(&mut self, k: &'static str) -> Result<String, Refusal> {
        let v = self.req(k)?;
        text(&v, k, self.line)
    }
    fn pick<T: Copy>(&mut self, k: &'static str, table: &[(&'static str, T)]) -> Result<T, Refusal> {
        let v = self.req(k)?;
        pick(&v, k, self.line, table)
    }
    fn target(&mut self, st: &State, k: &'static str, want: Kind) -> Result<String, Refusal> {
        let v = self.req(k)?;
        st.target(&v, k, self.line, want)
    }
    fn entity(&mut self, st: &State, k: &'static str, sk: &str) -> Result<String, Refusal> {
        let v = self.req(k)?;
        st.entity(&v, k, self.line, sk)
    }
    fn pref(&mut self, st: &State, k: &'static str, sk: &str) -> Result<PRef, Refusal> {
        let v = self.req(k)?;
        st.pref(&v, k, self.line, sk)
    }
    fn done(self) -> Result<(), Refusal> {
        match self.items.into_iter().next() {
            None => Ok(()),
            Some(it) => Err(Refusal { line: self.line, col: it.kcol, what: What::UnknownKey(it.key) }),
        }
    }
}

fn need_id(t_id: Option<(String, u32)>, line: u32, vcol: u32) -> Result<(String, u32), Refusal> {
    t_id.ok_or(Refusal { line, col: vcol, what: What::BadId })
}

fn no_id(t_id: &Option<(String, u32)>, line: u32) -> Result<(), Refusal> {
    match t_id {
        Some((_, col)) => Err(Refusal { line, col: *col, what: What::BadId }),
        None => Ok(()),
    }
}

/// One statement by its verb.
fn statement(t: Toks, line: u32, st: &mut State) -> Result<Stmt, Refusal> {
    let Toks { verb, vcol, id: tid, items } = t;
    let mut f = Fields { line, vcol, items };
    let s = match verb.as_str() {
        "sketch" => {
            let id = need_id(tid, line, vcol)?;
            let o = f.nums::<3>("o")?;
            let z = f.nums::<3>("z")?;
            let x = f.nums::<3>("x")?;
            f.done()?;
            Stmt::Sketch { id: st.declare(id, line, Kind::Sketch, None)?, o, z, x }
        }
        "point" | "line" | "circle" | "arc" => {
            let id = need_id(tid, line, vcol)?;
            let sketch = f.target(st, "sketch", Kind::Sketch)?;
            let s = match verb.as_str() {
                "point" => {
                    let x = f.f64("x")?;
                    let y = f.f64("y")?;
                    Stmt::Point { id: String::new(), sketch: sketch.clone(), at: [x, y] }
                }
                "line" => {
                    let a = f.entity(st, "a", &sketch)?;
                    let b = f.entity(st, "b", &sketch)?;
                    Stmt::Line { id: String::new(), sketch: sketch.clone(), a, b }
                }
                "circle" => {
                    let centre = f.entity(st, "centre", &sketch)?;
                    let r = f.f64("r")?;
                    Stmt::Circle { id: String::new(), sketch: sketch.clone(), centre, r }
                }
                _ => {
                    let centre = f.entity(st, "centre", &sketch)?;
                    let r = f.f64("r")?;
                    let start = f.f64("start")?;
                    let end = f.f64("end")?;
                    Stmt::Arc { id: String::new(), sketch: sketch.clone(), centre, r, start, end }
                }
            };
            f.done()?;
            let name = st.declare(id, line, Kind::Entity, Some(sketch))?;
            with_id(s, name)
        }
        "extrude" | "revolve" => {
            let id = need_id(tid, line, vcol)?;
            let sketch = f.target(st, "sketch", Kind::Sketch)?;
            let outer = f.entity(st, "outer", &sketch)?;
            let s = if verb == "extrude" {
                let dir = f.nums::<3>("dir")?;
                let length = f.f64("length")?;
                Stmt::Extrude { id: String::new(), sketch, outer, dir, length }
            } else {
                let axis_o = f.nums::<2>("axis_o")?;
                let axis_d = f.nums::<2>("axis_d")?;
                let angle = f.f64("angle")?;
                Stmt::Revolve { id: String::new(), sketch, outer, axis_o, axis_d, angle }
            };
            f.done()?;
            let name = st.declare(id, line, Kind::Feature, None)?;
            with_id(s, name)
        }
        "machine" => {
            let id = need_id(tid, line, vcol)?;
            if st.machine {
                return Err(Refusal { line, col: vcol, what: What::SecondMachine });
            }
            let name = f.text("name")?;
            let kind = f.pick("kind", &MACHINE_KINDS)?;
            let kinematics = f.pick("kinematics", &KINEMATICS)?;
            let units = f.pick("units", &UNITS)?;
            let ev = f.req("envelope")?;
            let envelope = aabb(&ev, "envelope", line)?;
            let safe_z = f.f64("safe_z")?;
            let pk = f.pick("power", &POWER_KINDS)?;
            let max = f.f64("power_max")?;
            let ask_above = f.f64("ask_above")?;
            let loaded = match f.opt("loaded") {
                Some(v) => Some(int_max(&v, "loaded", line, u32::MAX as u64)? as u32),
                None => None,
            };
            f.done()?;
            st.machine = true;
            let id = st.declare(id, line, Kind::Machine, None)?;
            Stmt::Machine(MachineStmt {
                id,
                name,
                kind,
                kinematics,
                units,
                envelope,
                safe_z,
                power: Power { kind: pk, max, ask_above },
                loaded,
            })
        }
        "axis" | "tool" | "stock" | "fixture" => {
            no_id(&tid, line)?;
            if !st.machine {
                return Err(Refusal { line, col: vcol, what: What::Orphan });
            }
            let s = match verb.as_str() {
                "axis" => Stmt::Axis(Axis {
                    id: f.pick("axis", &AXES)?,
                    min: f.f64("min")?,
                    max: f.f64("max")?,
                    max_feed: f.f64("max_feed")?,
                    max_accel: f.f64("max_accel")?,
                    rapid: f.f64("rapid")?,
                }),
                "tool" => {
                    let number = f.u32("number")?;
                    let kind = f.pick("kind", &TOOL_KINDS)?;
                    let dc = f.f64("dc")?;
                    let lf = f.f64("lf")?;
                    let apmx = f.f64("apmx")?;
                    let nv = f.req("nof")?;
                    let nof = int_max(&nv, "nof", line, u8::MAX as u64)? as u8;
                    let sig = f.f64("sig")?;
                    Stmt::Tool(Tool { number, kind, dc, lf, apmx, nof, sig })
                }
                "stock" => {
                    if st.stock {
                        return Err(Refusal { line, col: vcol, what: What::SecondStock });
                    }
                    let sv = f.req("shape")?;
                    let shape = match &sv.val {
                        Val::Word(w) if w == "box" => StockShape::Box { size: f.vec3("size")? },
                        Val::Word(w) if w == "cylinder" => {
                            StockShape::Cylinder { radius: f.f64("radius")?, height: f.f64("height")? }
                        }
                        _ => return Err(bad(&sv, "shape", line)),
                    };
                    let origin = f.vec3("origin")?;
                    st.stock = true;
                    Stmt::Stock(Stock { shape, origin })
                }
                _ => {
                    let name = f.text("name")?;
                    let kv = f.req("keepout")?;
                    let mut keepout = Vec::new();
                    for b in list(&kv, "keepout", line)? {
                        keepout.push(aabb(b, "keepout", line)?);
                    }
                    Stmt::Fixture(Fixture { name, keepout })
                }
            };
            f.done()?;
            s
        }
        "drill" | "contour" | "pocket" | "face" | "print_layers" | "laser_cut" => {
            let id = need_id(tid, line, vcol)?;
            let on = f.target(st, "on", Kind::Feature)?;
            let op = op_stmt(&verb, &mut f)?;
            f.done()?;
            Stmt::Op { id: st.declare(id, line, Kind::Op, None)?, on, op }
        }
        "scan" => {
            let id = need_id(tid, line, vcol)?;
            let path = f.text("path")?;
            let pv = f.req("pose")?;
            let pose = match &pv.val {
                Val::Word(w) if w == "identity" => Pose::Identity,
                Val::Word(w) if w == "turntable" => Pose::Turntable,
                Val::Word(w) if w == "file" => Pose::File(f.text("poses")?),
                Val::Word(w) if w == "icp" => Pose::Icp,
                _ => return Err(bad(&pv, "pose", line)),
            };
            let confidence = f.f64("confidence")?;
            f.done()?;
            Stmt::Scan { id: st.declare(id, line, Kind::Scan, None)?, path, pose, confidence }
        }
        _ => match constraint(&verb, &mut f, st)? {
            Some((sketch, c)) => {
                let id = need_id(tid, line, vcol)?;
                f.done()?;
                let name = st.declare(id, line, Kind::Constraint, Some(sketch.clone()))?;
                Stmt::Constraint { id: name, sketch, c }
            }
            None => return Err(Refusal { line, col: vcol, what: What::UnknownVerb(verb.clone()) }),
        },
    };
    Ok(s)
}

/// The statement with its declared id filled in.
fn with_id(mut s: Stmt, name: String) -> Stmt {
    match &mut s {
        Stmt::Point { id, .. }
        | Stmt::Line { id, .. }
        | Stmt::Circle { id, .. }
        | Stmt::Arc { id, .. }
        | Stmt::Extrude { id, .. }
        | Stmt::Revolve { id, .. } => *id = name,
        _ => {}
    }
    s
}

/// A constraint verb's keys, or None when `verb` is no constraint.
fn constraint(verb: &str, f: &mut Fields, st: &State) -> Result<Option<(String, Cons)>, Refusal> {
    const VERBS: [&str; 19] = [
        "coincident",
        "on_line",
        "on_circle",
        "horizontal",
        "vertical",
        "parallel",
        "perpendicular",
        "tangent_line_circle",
        "tangent_circles",
        "equal_length",
        "equal_radius",
        "distance",
        "distance_point_line",
        "angle",
        "radius",
        "diameter",
        "fix",
        "midpoint",
        "symmetric",
    ];
    if !VERBS.contains(&verb) {
        return Ok(None);
    }
    let sk = f.target(st, "sketch", Kind::Sketch)?;
    let s = sk.as_str();
    let c = match verb {
        "coincident" => Cons::Coincident(f.pref(st, "a", s)?, f.pref(st, "b", s)?),
        "on_line" => Cons::PointOnLine(f.pref(st, "p", s)?, f.entity(st, "line", s)?),
        "on_circle" => Cons::PointOnCircle(f.pref(st, "p", s)?, f.entity(st, "circle", s)?),
        "horizontal" => Cons::Horizontal(f.entity(st, "line", s)?),
        "vertical" => Cons::Vertical(f.entity(st, "line", s)?),
        "parallel" => Cons::Parallel(f.entity(st, "a", s)?, f.entity(st, "b", s)?),
        "perpendicular" => Cons::Perpendicular(f.entity(st, "a", s)?, f.entity(st, "b", s)?),
        "tangent_line_circle" => Cons::TangentLineCircle(f.entity(st, "line", s)?, f.entity(st, "circle", s)?),
        "tangent_circles" => Cons::TangentCircles(f.entity(st, "a", s)?, f.entity(st, "b", s)?),
        "equal_length" => Cons::EqualLength(f.entity(st, "a", s)?, f.entity(st, "b", s)?),
        "equal_radius" => Cons::EqualRadius(f.entity(st, "a", s)?, f.entity(st, "b", s)?),
        "distance" => Cons::Distance(f.pref(st, "a", s)?, f.pref(st, "b", s)?, f.f64("d")?),
        "distance_point_line" => Cons::DistancePointLine(f.pref(st, "p", s)?, f.entity(st, "line", s)?, f.f64("d")?),
        "angle" => Cons::Angle(f.entity(st, "a", s)?, f.entity(st, "b", s)?, f.f64("angle")?),
        "radius" => Cons::Radius(f.entity(st, "circle", s)?, f.f64("r")?),
        "diameter" => Cons::Diameter(f.entity(st, "circle", s)?, f.f64("d")?),
        "fix" => Cons::Fix(f.pref(st, "p", s)?, f.nums::<2>("at")?),
        "midpoint" => Cons::Midpoint(f.pref(st, "p", s)?, f.entity(st, "line", s)?),
        _ => Cons::Symmetric(f.pref(st, "a", s)?, f.pref(st, "b", s)?, f.entity(st, "line", s)?),
    };
    Ok(Some((sk, c)))
}

/// One Op by its verb (the compose site for VUGMAKEPOCKET / VUGMAKESLICE: a new field is a new key).
fn op_stmt(verb: &str, f: &mut Fields) -> Result<Op, Refusal> {
    let line = f.line;
    Ok(match verb {
        "drill" => Op::Drill {
            points: f.pts("points")?,
            top: f.f64("top")?,
            depth: f.f64("depth")?,
            peck: f.f64("peck")?,
            tool: f.u32("tool")?,
            feeds: Feeds { feed: f.f64("feed")?, plunge: f.f64("plunge")?, rpm: f.f64("rpm")? },
        },
        "contour" => Op::Contour {
            profile: f.pts("profile")?,
            top: f.f64("top")?,
            depth: f.f64("depth")?,
            side: f.pick("side", &SIDES)?,
            tool: f.u32("tool")?,
            feeds: Feeds { feed: f.f64("feed")?, plunge: f.f64("plunge")?, rpm: f.f64("rpm")? },
        },
        "pocket" => Op::Pocket2_5D {
            outline: f.pts("outline")?,
            islands: f.polys("islands")?,
            depth: f.f64("depth")?,
            stepover: f.f64("stepover")?,
            tool: f.u32("tool")?,
        },
        "face" => Op::Face { area: f.pts("area")?, depth: f.f64("depth")? },
        "print_layers" => {
            let sv = f.req("slices")?;
            let mut slices = Vec::new();
            for s in list(&sv, "slices", line)? {
                let parts = list(s, "slices", line)?;
                if parts.len() != 2 {
                    return Err(bad(s, "slices", line));
                }
                slices.push((num(&parts[0], "slices", line)?, polys(&parts[1], "slices", line)?));
            }
            Op::PrintLayers { slices, tool: f.u32("tool")? }
        }
        _ => Op::LaserCut { polylines: f.polys("polylines")?, passes: f.u32("passes")? },
    })
}
