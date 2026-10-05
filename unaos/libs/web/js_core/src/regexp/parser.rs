//! RegExp pattern grammar (ECMA-262 §22.2.1, with Annex B.1.2 for non-Unicode patterns): parses a pattern into
//! a node tree with resolved character sets, and reports every early error.

use crate::unicode;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub d: bool,
    pub g: bool,
    pub i: bool,
    pub m: bool,
    pub s: bool,
    pub u: bool,
    pub v: bool,
    pub y: bool,
}

impl Flags {
    pub fn parse(units: &[u16]) -> Result<Flags, &'static str> {
        let mut f = Flags::default();
        for &c in units {
            let slot = match c {
                0x64 => &mut f.d,
                0x67 => &mut f.g,
                0x69 => &mut f.i,
                0x6D => &mut f.m,
                0x73 => &mut f.s,
                0x75 => &mut f.u,
                0x76 => &mut f.v,
                0x79 => &mut f.y,
                _ => return Err("invalid flag"),
            };
            if *slot {
                return Err("duplicate flag");
            }
            *slot = true;
        }
        if f.u && f.v {
            return Err("u and v flags are exclusive");
        }
        Ok(f)
    }
    pub fn unicode(&self) -> bool {
        self.u || self.v
    }
}

/// A set of code points (sorted, merged inclusive ranges) plus multi-code-point strings (v-mode).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharSet {
    pub ranges: Vec<(u32, u32)>,
    pub strings: Vec<Vec<u32>>,
}

impl CharSet {
    pub fn new() -> CharSet {
        CharSet::default()
    }
    pub fn from_ranges(mut r: Vec<(u32, u32)>) -> CharSet {
        normalize(&mut r);
        CharSet { ranges: r, strings: Vec::new() }
    }
    pub fn from_flat(t: &[u32]) -> CharSet {
        let r = t.chunks(2).map(|c| (c[0], c[1])).collect();
        CharSet { ranges: r, strings: Vec::new() }
    }
    pub fn add(&mut self, a: u32, b: u32) {
        self.ranges.push((a, b));
        normalize(&mut self.ranges);
    }
    pub fn add_set(&mut self, o: &CharSet) {
        self.ranges.extend_from_slice(&o.ranges);
        normalize(&mut self.ranges);
        for s in &o.strings {
            if !self.strings.contains(s) {
                self.strings.push(s.clone());
            }
        }
    }
    pub fn add_string(&mut self, s: Vec<u32>) {
        if s.len() == 1 {
            self.add(s[0], s[0]);
        } else if !self.strings.contains(&s) {
            self.strings.push(s);
        }
    }
    pub fn contains(&self, c: u32) -> bool {
        let r = &self.ranges;
        let (mut lo, mut hi) = (0, r.len());
        while lo < hi {
            let m = (lo + hi) / 2;
            if c < r[m].0 {
                hi = m;
            } else if c > r[m].1 {
                lo = m + 1;
            } else {
                return true;
            }
        }
        false
    }
    pub fn complement(&self) -> CharSet {
        let mut out = Vec::new();
        let mut next = 0u32;
        for &(a, b) in &self.ranges {
            if a > next {
                out.push((next, a - 1));
            }
            next = b + 1;
        }
        if next <= 0x10FFFF {
            out.push((next, 0x10FFFF));
        }
        CharSet { ranges: out, strings: Vec::new() }
    }
    pub fn intersect(&self, o: &CharSet) -> CharSet {
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < o.ranges.len() {
            let (a1, b1) = self.ranges[i];
            let (a2, b2) = o.ranges[j];
            let a = a1.max(a2);
            let b = b1.min(b2);
            if a <= b {
                out.push((a, b));
            }
            if b1 < b2 {
                i += 1;
            } else {
                j += 1;
            }
        }
        let strings = self.strings.iter().filter(|s| o.strings.contains(s)).cloned().collect();
        CharSet { ranges: out, strings }
    }
    pub fn subtract(&self, o: &CharSet) -> CharSet {
        let mut r = self.intersect(&o.complement());
        r.strings = self.strings.iter().filter(|s| !o.strings.contains(s)).cloned().collect();
        r
    }
}

fn normalize(r: &mut Vec<(u32, u32)>) {
    r.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(r.len());
    for &(a, b) in r.iter() {
        if let Some(last) = out.last_mut() {
            if a <= last.1.saturating_add(1) {
                if b > last.1 {
                    last.1 = b;
                }
                continue;
            }
        }
        out.push((a, b));
    }
    *r = out;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ModFlags {
    pub i: bool,
    pub m: bool,
    pub s: bool,
}

#[derive(Clone, Debug)]
pub enum Node {
    Empty,
    Char(u32),
    Dot,
    Set(Box<CharSet>, bool /*negated*/),
    LineStart,
    LineEnd,
    WordBoundary(bool /*negated*/),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    /// Capturing group (index >= 1) or non-capturing (None).
    Group(Box<Node>, Option<usize>),
    Look { behind: bool, negated: bool, node: Box<Node>, first_group: usize, last_group: usize },
    Repeat { node: Box<Node>, min: u32, max: Option<u32>, greedy: bool, first_group: usize, last_group: usize },
    BackRef(Vec<usize>),
    /// Parse-time only: `\k<name>`, resolved to `BackRef` once all groups are known.
    NamedRef(String),
    Modifiers { add: ModFlags, remove: ModFlags, node: Box<Node> },
}

#[derive(Debug)]
pub struct Regex {
    pub node: Node,
    /// Number of capture groups (excluding group 0).
    pub ngroups: usize,
    /// Group names (index -> name) for named groups.
    pub names: Vec<(String, usize)>,
    pub flags: Flags,
}

struct P<'a> {
    s: Vec<u32>,
    pos: usize,
    u: bool,
    v: bool,
    named: bool,
    total_groups: usize,
    ngroups: usize,
    names: Vec<(String, usize, Vec<(usize, usize)>)>,
    backref_names: Vec<String>,
    path: Vec<(usize, usize)>,
    disj_counter: usize,
    /// Unicode + ignoreCase: WordCharacters gains U+017F and U+212A (§22.2.2.9.4).
    ui: bool,
    _src: &'a [u16],
}

type R<T> = Result<T, String>;

fn err<T>(m: &str) -> R<T> {
    Err(String::from(m))
}

pub fn parse(pattern: &[u16], flags: Flags) -> R<Regex> {
    let u = flags.unicode();
    let mut s = Vec::with_capacity(pattern.len());
    let mut i = 0;
    while i < pattern.len() {
        if u {
            let (c, n) = crate::string::code_point_at(pattern, i);
            s.push(c);
            i += n;
        } else {
            s.push(pattern[i] as u32);
            i += 1;
        }
    }
    let (total, named) = prescan(&s);
    let mut p = P {
        s,
        pos: 0,
        u,
        v: flags.v,
        named: named || u,
        total_groups: total,
        ngroups: 0,
        names: Vec::new(),
        backref_names: Vec::new(),
        path: Vec::new(),
        disj_counter: 0,
        ui: u && flags.i,
        _src: pattern,
    };
    let node = p.disjunction()?;
    if p.pos < p.s.len() {
        return err(if p.s[p.pos] == ')' as u32 { "unmatched ')'" } else { "unexpected character" });
    }
    for n in &p.backref_names {
        if !p.names.iter().any(|(m, _, _)| m == n) {
            return err("invalid named reference");
        }
    }
    let names = p.names.iter().map(|(n, i, _)| (n.clone(), *i)).collect();
    // Named backreferences are resolved after parsing: rewrite them to group indices.
    let mut node = node;
    resolve_named(&mut node, &p.names);
    Ok(Regex { node, ngroups: p.ngroups, names, flags })
}

fn resolve_named(n: &mut Node, names: &[(String, usize, Vec<(usize, usize)>)]) {
    match n {
        Node::NamedRef(name) => {
            let idx = names.iter().filter(|(n, _, _)| n == name).map(|(_, i, _)| *i).collect();
            *n = Node::BackRef(idx);
        }
        Node::Seq(v) | Node::Alt(v) => v.iter_mut().for_each(|x| resolve_named(x, names)),
        Node::Group(x, _) => resolve_named(x, names),
        Node::Look { node, .. } | Node::Repeat { node, .. } | Node::Modifiers { node, .. } => resolve_named(node, names),
        _ => {}
    }
}

/// Count capturing groups and detect named groups.
fn prescan(s: &[u32]) -> (usize, bool) {
    let mut n = 0;
    let mut named = false;
    let mut i = 0;
    let mut in_class = false;
    while i < s.len() {
        let c = s[i];
        if c == '\\' as u32 {
            i += 2;
            continue;
        }
        if in_class {
            if c == ']' as u32 {
                in_class = false;
            }
        } else if c == '[' as u32 {
            in_class = true;
        } else if c == '(' as u32 {
            if i + 1 < s.len() && s[i + 1] == '?' as u32 {
                if i + 2 < s.len() && s[i + 2] == '<' as u32 && i + 3 < s.len() && s[i + 3] != '=' as u32 && s[i + 3] != '!' as u32 {
                    n += 1;
                    named = true;
                }
            } else {
                n += 1;
            }
        }
        i += 1;
    }
    (n, named)
}

fn is_syntax_char(c: u32) -> bool {
    matches!(char::from_u32(c), Some('^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'))
}

enum ClassAtom {
    Char(u32),
    Set(CharSet),
}

impl P<'_> {
    fn peek(&self) -> Option<u32> {
        self.s.get(self.pos).copied()
    }
    fn peek_at(&self, k: usize) -> Option<u32> {
        self.s.get(self.pos + k).copied()
    }
    fn is(&self, c: char) -> bool {
        self.peek() == Some(c as u32)
    }
    fn eat(&mut self, c: char) -> bool {
        if self.is(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn disjunction(&mut self) -> R<Node> {
        let id = self.disj_counter;
        self.disj_counter += 1;
        let mut alts = Vec::new();
        let mut k = 0;
        loop {
            self.path.push((id, k));
            let a = self.alternative();
            self.path.pop();
            alts.push(a?);
            if !self.eat('|') {
                break;
            }
            k += 1;
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }

    fn alternative(&mut self) -> R<Node> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' as u32 || c == ')' as u32 {
                break;
            }
            terms.push(self.term()?);
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().unwrap(),
            _ => Node::Seq(terms),
        })
    }

    fn term(&mut self) -> R<Node> {
        let c = self.peek().unwrap();
        let group_start = self.ngroups;
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        let (atom, quantifiable) = match ch {
            '^' => {
                self.pos += 1;
                (Node::LineStart, false)
            }
            '$' => {
                self.pos += 1;
                (Node::LineEnd, false)
            }
            '\\' if self.peek_at(1) == Some('b' as u32) => {
                self.pos += 2;
                (Node::WordBoundary(false), false)
            }
            '\\' if self.peek_at(1) == Some('B' as u32) => {
                self.pos += 2;
                (Node::WordBoundary(true), false)
            }
            '(' => {
                self.pos += 1;
                if self.eat('?') {
                    if self.eat('=') || self.is('!') {
                        let negated = if self.is('!') {
                            self.pos += 1;
                            true
                        } else {
                            false
                        };
                        let first = self.ngroups;
                        let d = self.disjunction()?;
                        if !self.eat(')') {
                            return err("unterminated group");
                        }
                        // Annex B: lookaheads are quantifiable in non-Unicode patterns.
                        (Node::Look { behind: false, negated, node: Box::new(d), first_group: first + 1, last_group: self.ngroups }, !self.u)
                    } else if self.is('<') && (self.peek_at(1) == Some('=' as u32) || self.peek_at(1) == Some('!' as u32)) {
                        let negated = self.peek_at(1) == Some('!' as u32);
                        self.pos += 2;
                        let first = self.ngroups;
                        let d = self.disjunction()?;
                        if !self.eat(')') {
                            return err("unterminated group");
                        }
                        (Node::Look { behind: true, negated, node: Box::new(d), first_group: first + 1, last_group: self.ngroups }, false)
                    } else if self.eat(':') {
                        let d = self.disjunction()?;
                        if !self.eat(')') {
                            return err("unterminated group");
                        }
                        (Node::Group(Box::new(d), None), true)
                    } else if self.is('<') {
                        self.pos += 1;
                        let name = self.group_name()?;
                        self.ngroups += 1;
                        let idx = self.ngroups;
                        // Duplicate names are allowed only in different alternatives.
                        for (n, _, path) in &self.names {
                            if *n == name && !paths_disjoint(path, &self.path) {
                                return err("duplicate capture group name");
                            }
                        }
                        self.names.push((name, idx, self.path.clone()));
                        let d = self.disjunction()?;
                        if !self.eat(')') {
                            return err("unterminated group");
                        }
                        (Node::Group(Box::new(d), Some(idx)), true)
                    } else {
                        // Modifiers (?ims-ims:…)
                        let (add, remove) = self.modifiers()?;
                        let d = self.disjunction()?;
                        if !self.eat(')') {
                            return err("unterminated group");
                        }
                        (Node::Modifiers { add, remove, node: Box::new(d) }, true)
                    }
                } else {
                    self.ngroups += 1;
                    let idx = self.ngroups;
                    let d = self.disjunction()?;
                    if !self.eat(')') {
                        return err("unterminated group");
                    }
                    (Node::Group(Box::new(d), Some(idx)), true)
                }
            }
            ')' => return err("unmatched ')'"),
            '*' | '+' | '?' => return err("nothing to repeat"),
            '{' => {
                if self.u {
                    return err("lone quantifier bracket");
                }
                // Annex B: a `{` that forms a valid quantifier with nothing to repeat is an error.
                let save = self.pos;
                if self.braced_quantifier()?.is_some() {
                    return err("nothing to repeat");
                }
                self.pos = save + 1;
                (Node::Char(c), true)
            }
            '}' | ']' => {
                if self.u {
                    return err("lone quantifier bracket");
                }
                self.pos += 1;
                (Node::Char(c), true)
            }
            '.' => {
                self.pos += 1;
                (Node::Dot, true)
            }
            '[' => {
                self.pos += 1;
                (self.class()?, true)
            }
            '\\' => {
                self.pos += 1;
                (self.atom_escape()?, true)
            }
            _ => {
                self.pos += 1;
                (Node::Char(c), true)
            }
        };
        // Quantifier
        let q = match self.peek().and_then(char::from_u32) {
            Some('*') => {
                self.pos += 1;
                Some((0, None))
            }
            Some('+') => {
                self.pos += 1;
                Some((1, None))
            }
            Some('?') => {
                self.pos += 1;
                Some((0, Some(1)))
            }
            Some('{') => {
                let save = self.pos;
                match self.braced_quantifier()? {
                    Some(q) => Some(q),
                    None => {
                        if self.u {
                            return err("incomplete quantifier");
                        }
                        self.pos = save;
                        None
                    }
                }
            }
            _ => None,
        };
        if let Some((min, max)) = q {
            if !quantifiable {
                return err("nothing to repeat");
            }
            if let Some(m) = max {
                if m < min {
                    return err("numbers out of order in quantifier");
                }
            }
            let greedy = !self.eat('?');
            return Ok(Node::Repeat { node: Box::new(atom), min, max, greedy, first_group: group_start + 1, last_group: self.ngroups });
        }
        Ok(atom)
    }

    /// `{n}`, `{n,}`, `{n,m}` at the current `{`; returns None (position unspecified) if not a quantifier.
    fn braced_quantifier(&mut self) -> R<Option<(u32, Option<u32>)>> {
        self.pos += 1;
        let min = match self.decimal() {
            Some(v) => v,
            None => return Ok(None),
        };
        let max = if self.eat(',') {
            if self.is('}') {
                None
            } else {
                match self.decimal() {
                    Some(v) => Some(v),
                    None => return Ok(None),
                }
            }
        } else {
            Some(min)
        };
        if !self.eat('}') {
            return Ok(None);
        }
        Ok(Some((min, max)))
    }

    fn decimal(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut v: u64 = 0;
        while let Some(c) = self.peek() {
            if !(0x30..=0x39).contains(&c) {
                break;
            }
            v = (v * 10 + (c - 0x30) as u64).min(u32::MAX as u64);
            self.pos += 1;
        }
        if self.pos == start {
            None
        } else {
            Some(v as u32)
        }
    }

    fn modifiers(&mut self) -> R<(ModFlags, ModFlags)> {
        let mut add = ModFlags::default();
        let mut remove = ModFlags::default();
        let mut seen = 0;
        let mut removing = false;
        let mut any_remove_flag = false;
        loop {
            let c = match self.peek().and_then(char::from_u32) {
                Some(c) => c,
                None => return err("invalid group"),
            };
            match c {
                ':' => {
                    self.pos += 1;
                    break;
                }
                '-' if !removing => {
                    removing = true;
                    self.pos += 1;
                }
                'i' | 'm' | 's' => {
                    let bit = match c {
                        'i' => 1,
                        'm' => 2,
                        _ => 4,
                    };
                    if seen & bit != 0 {
                        return err("repeated flag in modifiers");
                    }
                    seen |= bit;
                    let tgt = if removing { &mut remove } else { &mut add };
                    if removing {
                        any_remove_flag = true;
                    }
                    match c {
                        'i' => tgt.i = true,
                        'm' => tgt.m = true,
                        _ => tgt.s = true,
                    }
                    self.pos += 1;
                }
                _ => return err("invalid group"),
            }
        }
        if removing && !any_remove_flag && seen == 0 {
            return err("invalid modifiers");
        }
        if !removing && seen == 0 {
            return err("invalid group");
        }
        Ok((add, remove))
    }

    fn group_name(&mut self) -> R<String> {
        let mut name = String::new();
        let mut first = true;
        loop {
            let c = match self.peek() {
                Some(c) => c,
                None => return err("invalid capture group name"),
            };
            if c == '>' as u32 {
                self.pos += 1;
                break;
            }
            let cp = if c == '\\' as u32 {
                self.pos += 1;
                if !self.eat('u') {
                    return err("invalid capture group name");
                }
                match self.unicode_escape(true)? {
                    Some(cp) => cp,
                    None => return err("invalid capture group name"),
                }
            } else {
                self.pos += 1;
                // In non-Unicode patterns the pattern is code units: join a literal surrogate pair.
                if !self.u && (0xD800..0xDC00).contains(&c) {
                    if let Some(d) = self.peek() {
                        if (0xDC00..0xE000).contains(&d) {
                            self.pos += 1;
                            0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00)
                        } else {
                            c
                        }
                    } else {
                        c
                    }
                } else {
                    c
                }
            };
            let ok = if first {
                cp == '$' as u32 || cp == '_' as u32 || unicode::is_id_start(cp)
            } else {
                cp == '$' as u32 || cp == 0x200C || cp == 0x200D || unicode::is_id_continue(cp)
            };
            if !ok {
                return err("invalid capture group name");
            }
            match char::from_u32(cp) {
                Some(ch) => name.push(ch),
                None => return err("invalid capture group name"),
            }
            first = false;
        }
        if name.is_empty() {
            return err("invalid capture group name");
        }
        Ok(name)
    }

    /// After `\u`: returns Some(code point) or None if not a valid escape (caller decides).
    /// `name_mode`: in group names, `\u{…}` and surrogate pairs are allowed regardless of flags.
    fn unicode_escape(&mut self, name_mode: bool) -> R<Option<u32>> {
        let start = self.pos;
        if self.is('{') && (self.u || name_mode) {
            self.pos += 1;
            let mut v: u32 = 0;
            let mut any = false;
            while let Some(d) = self.peek().and_then(crate::numconv::digit_val).filter(|&d| d < 16) {
                v = v.saturating_mul(16).saturating_add(d);
                if v > 0x10FFFF {
                    return err("invalid unicode escape");
                }
                any = true;
                self.pos += 1;
            }
            if !any || !self.eat('}') {
                if self.u {
                    return err("invalid unicode escape");
                }
                self.pos = start;
                return Ok(None);
            }
            return Ok(Some(v));
        }
        let h = self.hex4();
        match h {
            None => {
                self.pos = start;
                if self.u {
                    return err("invalid unicode escape");
                }
                Ok(None)
            }
            Some(hi) => {
                if (self.u || name_mode) && (0xD800..0xDC00).contains(&hi) && self.is('\\') && self.peek_at(1) == Some('u' as u32) {
                    let save = self.pos;
                    self.pos += 2;
                    if let Some(lo) = self.hex4() {
                        if (0xDC00..0xE000).contains(&lo) {
                            return Ok(Some(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)));
                        }
                    }
                    self.pos = save;
                }
                Ok(Some(hi))
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let mut v = 0;
        for k in 0..4 {
            let d = self.peek_at(k).and_then(crate::numconv::digit_val).filter(|&d| d < 16)?;
            v = v * 16 + d;
        }
        self.pos += 4;
        Some(v)
    }

    /// After a backslash outside a class.
    fn atom_escape(&mut self) -> R<Node> {
        let c = match self.peek() {
            Some(c) => c,
            None => return err("\\ at end of pattern"),
        };
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        match ch {
            '1'..='9' => {
                let save = self.pos;
                let n = self.decimal().unwrap() as usize;
                if n <= self.total_groups {
                    return Ok(Node::BackRef(vec![n]));
                }
                if self.u {
                    return err("invalid escape");
                }
                self.pos = save;
                // Annex B: legacy octal escape or identity escape for 8 / 9.
                if ch >= '8' {
                    self.pos += 1;
                    return Ok(Node::Char(c));
                }
                return Ok(Node::Char(self.legacy_octal()));
            }
            'k' => {
                if self.named {
                    self.pos += 1;
                    if !self.eat('<') {
                        return err("invalid named reference");
                    }
                    let name = self.group_name()?;
                    self.backref_names.push(name.clone());
                    return Ok(Node::NamedRef(name));
                }
                self.pos += 1;
                return Ok(Node::Char(c));
            }
            _ => {}
        }
        match self.class_escape(false)? {
            ClassAtom::Char(c) => Ok(Node::Char(c)),
            ClassAtom::Set(s) => Ok(Node::Set(Box::new(s), false)),
        }
    }

    fn legacy_octal(&mut self) -> u32 {
        // LegacyOctalEscapeSequence: up to three octal digits, value <= 0o377.
        let c0 = self.peek().unwrap() - 0x30;
        self.pos += 1;
        let mut v = c0;
        let max = if c0 <= 3 { 2 } else { 1 };
        for _ in 0..max {
            match self.peek() {
                Some(d) if (0x30..=0x37).contains(&d) => {
                    v = v * 8 + (d - 0x30);
                    self.pos += 1;
                }
                _ => break,
            }
        }
        v
    }

    /// Escapes common to atoms and classes (the backslash is consumed). `in_class` enables \b and \- forms.
    fn class_escape(&mut self, in_class: bool) -> R<ClassAtom> {
        let c = match self.peek() {
            Some(c) => c,
            None => return err("\\ at end of pattern"),
        };
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        self.pos += 1;
        Ok(ClassAtom::Char(match ch {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                let set = match ch.to_ascii_lowercase() {
                    'd' => CharSet::from_ranges(vec![(0x30, 0x39)]),
                    's' => space_set(),
                    _ => CharSet::from_ranges(vec![(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)]),
                };
                let set = if ch.is_ascii_uppercase() { set.complement() } else { set };
                return Ok(ClassAtom::Set(set));
            }
            'p' | 'P' if self.u => {
                let s = self.property(ch == 'P')?;
                return Ok(ClassAtom::Set(s));
            }
            'f' => 0x0C,
            'n' => 0x0A,
            'r' => 0x0D,
            't' => 0x09,
            'v' => 0x0B,
            'b' if in_class => 0x08,
            '-' if in_class && self.u => '-' as u32,
            'c' => match self.peek() {
                Some(l) if (l as u8 as u32 == l) && (l as u8).is_ascii_alphabetic() => {
                    self.pos += 1;
                    l % 32
                }
                Some(l) if in_class && !self.u && ((0x30..=0x39).contains(&l) || l == '_' as u32) => {
                    self.pos += 1;
                    l % 32
                }
                _ => {
                    if self.u {
                        return err("invalid unicode escape");
                    }
                    // Annex B: `\c` not followed by a letter is a literal backslash; `c` is re-read.
                    self.pos -= 1;
                    '\\' as u32
                }
            },
            '0' => {
                if let Some(d) = self.peek() {
                    if (0x30..=0x39).contains(&d) {
                        if self.u {
                            return err("invalid decimal escape");
                        }
                        self.pos -= 1;
                        return Ok(ClassAtom::Char(self.legacy_octal()));
                    }
                }
                0
            }
            '1'..='9' if in_class => {
                if self.u {
                    return err("invalid class escape");
                }
                self.pos -= 1;
                if ch >= '8' {
                    self.pos += 1;
                    c
                } else {
                    self.legacy_octal()
                }
            }
            'x' => {
                let a = self.peek().and_then(crate::numconv::digit_val).filter(|&d| d < 16);
                let b = self.peek_at(1).and_then(crate::numconv::digit_val).filter(|&d| d < 16);
                match (a, b) {
                    (Some(a), Some(b)) => {
                        self.pos += 2;
                        a * 16 + b
                    }
                    _ => {
                        if self.u {
                            return err("invalid escape");
                        }
                        'x' as u32
                    }
                }
            }
            'u' => match self.unicode_escape(false)? {
                Some(cp) => cp,
                None => 'u' as u32,
            },
            _ => {
                if self.u {
                    if is_syntax_char(c) || c == '/' as u32 {
                        c
                    } else {
                        return err("invalid escape");
                    }
                } else if ch == 'k' && self.named {
                    return err("invalid named reference");
                } else {
                    c
                }
            }
        }))
    }

    fn property(&mut self, negated: bool) -> R<CharSet> {
        if !self.eat('{') {
            return err("invalid property name");
        }
        let mut name = String::new();
        let mut value: Option<String> = None;
        loop {
            let c = match self.peek().and_then(char::from_u32) {
                Some(c) => c,
                None => return err("invalid property name"),
            };
            self.pos += 1;
            if c == '}' {
                break;
            }
            if c == '=' && value.is_none() {
                value = Some(String::new());
                continue;
            }
            if !(c.is_ascii_alphanumeric() || c == '_') {
                return err("invalid property name");
            }
            match &mut value {
                Some(v) => v.push(c),
                None => name.push(c),
            }
        }
        let set = match value {
            Some(v) => {
                if v.is_empty() {
                    return err("invalid property name");
                }
                match name.as_str() {
                    "General_Category" | "gc" => match unicode::gc_value(&v) {
                        Some(mask) => CharSet::from_flat(&unicode::gc_ranges(mask)),
                        None => return err("invalid property name"),
                    },
                    "Script" | "sc" | "Script_Extensions" | "scx" => match unicode::script_value(&v) {
                        Some(sc) => CharSet::from_flat(&unicode::script_ranges(sc, name.starts_with("Script_") || name == "scx")),
                        None => return err("invalid property name"),
                    },
                    _ => return err("invalid property name"),
                }
            }
            None => {
                if let Some(mask) = unicode::gc_value(&name) {
                    CharSet::from_flat(&unicode::gc_ranges(mask))
                } else if let Some(t) = unicode::binary_property(&name) {
                    CharSet::from_flat(t)
                } else if self.v && unicode::string_property(&name).is_some() {
                    if negated {
                        return err("negated property of strings");
                    }
                    let t = unicode::string_property(&name).unwrap();
                    let mut s = CharSet::new();
                    let mut i = 0;
                    while i < t.len() {
                        let n = t[i] as usize;
                        s.add_string(t[i + 1..i + 1 + n].to_vec());
                        i += 1 + n;
                    }
                    s
                } else if self.v && name == "RGI_Emoji" {
                    let mut s = CharSet::new();
                    for (_, t) in unicode::emoji::STRING_PROPERTIES.iter() {
                        let mut i = 0;
                        while i < t.len() {
                            let n = t[i] as usize;
                            s.add_string(t[i + 1..i + 1 + n].to_vec());
                            i += 1 + n;
                        }
                    }
                    if negated {
                        return err("negated property of strings");
                    }
                    s
                } else {
                    return err("invalid property name");
                }
            }
        };
        Ok(if negated { set.complement() } else { set })
    }

    // ------------------------------------------------------------------------------------- classes

    fn class(&mut self) -> R<Node> {
        let negated = self.eat('^');
        if self.v {
            let set = self.class_set_expression()?;
            if !self.eat(']') {
                return err("unterminated character class");
            }
            if negated && !set.strings.is_empty() {
                return err("negated character class may contain strings");
            }
            return Ok(Node::Set(Box::new(set), negated));
        }
        let mut set = CharSet::new();
        loop {
            match self.peek() {
                None => return err("unterminated character class"),
                Some(c) if c == ']' as u32 => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            let a = self.class_atom()?;
            if self.is('-') && self.peek_at(1).is_some() && self.peek_at(1) != Some(']' as u32) {
                self.pos += 1;
                let b = self.class_atom()?;
                match (a, b) {
                    (ClassAtom::Char(x), ClassAtom::Char(y)) => {
                        if x > y {
                            return err("range out of order in character class");
                        }
                        set.add(x, y);
                    }
                    (a, b) => {
                        if self.u {
                            return err("invalid character class range");
                        }
                        for atom in [a, ClassAtom::Char('-' as u32), b] {
                            match atom {
                                ClassAtom::Char(x) => set.add(x, x),
                                ClassAtom::Set(s) => set.add_set(&s),
                            }
                        }
                    }
                }
            } else {
                match a {
                    ClassAtom::Char(x) => set.add(x, x),
                    ClassAtom::Set(s) => set.add_set(&s),
                }
            }
        }
        Ok(Node::Set(Box::new(set), negated))
    }

    fn class_atom(&mut self) -> R<ClassAtom> {
        let c = self.peek().unwrap();
        self.pos += 1;
        if c == '\\' as u32 {
            if self.is('B') && !self.u {
                self.pos += 1;
                return Ok(ClassAtom::Char('B' as u32));
            }
            if self.is('k') && !self.u {
                self.pos += 1;
                return Ok(ClassAtom::Char('k' as u32));
            }
            return self.class_escape(true);
        }
        Ok(ClassAtom::Char(c))
    }

    // v-mode (UnicodeSets) class contents.
    fn class_set_expression(&mut self) -> R<CharSet> {
        if self.is(']') {
            return Ok(CharSet::new());
        }
        let first = self.class_set_operand_or_range(true)?;
        if self.is_double('&') {
            let mut acc = first.0;
            if first.1 {
                return err("invalid set operation");
            }
            while self.is_double('&') {
                self.pos += 2;
                if self.is('&') {
                    return err("invalid set operation");
                }
                let (o, was_range) = self.class_set_operand_or_range(false)?;
                if was_range {
                    return err("invalid set operation");
                }
                acc = acc.intersect(&o);
            }
            if !self.is(']') {
                return err("invalid set operation");
            }
            return Ok(acc);
        }
        if self.is_double('-') {
            let mut acc = first.0;
            if first.1 {
                return err("invalid set operation");
            }
            while self.is_double('-') {
                self.pos += 2;
                let (o, was_range) = self.class_set_operand_or_range(false)?;
                if was_range {
                    return err("invalid set operation");
                }
                acc = acc.subtract(&o);
            }
            if !self.is(']') {
                return err("invalid set operation");
            }
            return Ok(acc);
        }
        let mut acc = first.0;
        while !self.is(']') {
            if self.peek().is_none() {
                return err("unterminated character class");
            }
            if self.is_double('&') || self.is_double('-') {
                return err("invalid set operation");
            }
            let (o, _) = self.class_set_operand_or_range(true)?;
            acc.add_set(&o);
        }
        Ok(acc)
    }

    fn is_double(&self, c: char) -> bool {
        self.peek() == Some(c as u32) && self.peek_at(1) == Some(c as u32)
    }

    /// Returns (set, was_a_range).
    fn class_set_operand_or_range(&mut self, allow_range: bool) -> R<(CharSet, bool)> {
        let c = match self.peek() {
            Some(c) => c,
            None => return err("unterminated character class"),
        };
        if c == '[' as u32 {
            self.pos += 1;
            let negated = self.eat('^');
            let s = self.class_set_expression()?;
            if !self.eat(']') {
                return err("unterminated character class");
            }
            if negated {
                if !s.strings.is_empty() {
                    return err("negated character class may contain strings");
                }
                return Ok((s.complement(), false));
            }
            return Ok((s, false));
        }
        if c == '\\' as u32 {
            if self.peek_at(1) == Some('q' as u32) {
                self.pos += 2;
                if !self.eat('{') {
                    return err("invalid escape");
                }
                let mut s = CharSet::new();
                let mut cur: Vec<u32> = Vec::new();
                loop {
                    match self.peek() {
                        None => return err("unterminated class string disjunction"),
                        Some(x) if x == '}' as u32 => {
                            self.pos += 1;
                            if cur.len() == 1 {
                                s.add(cur[0], cur[0]);
                            } else {
                                s.add_string(cur);
                            }
                            break;
                        }
                        Some(x) if x == '|' as u32 => {
                            self.pos += 1;
                            let w = core::mem::take(&mut cur);
                            if w.len() == 1 {
                                s.add(w[0], w[0]);
                            } else {
                                s.add_string(w);
                            }
                        }
                        _ => cur.push(self.class_set_character()?),
                    }
                }
                return Ok((s, false));
            }
            // Character class escapes produce sets.
            if let Some(n) = self.peek_at(1).and_then(char::from_u32) {
                if matches!(n, 'd' | 'D' | 's' | 'S' | 'w' | 'W' | 'p' | 'P') {
                    self.pos += 1;
                    return match self.class_escape(true)? {
                        ClassAtom::Set(s) => Ok((s, false)),
                        ClassAtom::Char(x) => Ok((CharSet::from_ranges(vec![(x, x)]), false)),
                    };
                }
            }
        }
        let a = self.class_set_character()?;
        if allow_range && self.is('-') && !self.is_double('-') {
            self.pos += 1;
            let b = self.class_set_character()?;
            if a > b {
                return err("range out of order in character class");
            }
            return Ok((CharSet::from_ranges(vec![(a, b)]), true));
        }
        Ok((CharSet::from_ranges(vec![(a, a)]), false))
    }

    fn class_set_character(&mut self) -> R<u32> {
        let c = match self.peek() {
            Some(c) => c,
            None => return err("unterminated character class"),
        };
        if c == '\\' as u32 {
            self.pos += 1;
            let n = match self.peek() {
                Some(n) => n,
                None => return err("\\ at end of pattern"),
            };
            // ClassSetReservedPunctuator
            if matches!(char::from_u32(n), Some('&' | '-' | '!' | '#' | '%' | ',' | ':' | ';' | '<' | '=' | '>' | '@' | '`' | '~')) {
                self.pos += 1;
                return Ok(n);
            }
            return match self.class_escape(true)? {
                ClassAtom::Char(x) => Ok(x),
                ClassAtom::Set(_) => err("invalid escape"),
            };
        }
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        if matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | '/' | '-' | '|') {
            return err("invalid character in character class");
        }
        // ClassSetReservedDoublePunctuator
        if matches!(ch, '&' | '!' | '#' | '$' | '%' | '*' | '+' | ',' | '.' | ':' | ';' | '<' | '=' | '>' | '?' | '@' | '^' | '`' | '~')
            && self.peek_at(1) == Some(c)
        {
            return err("invalid set operation in character class");
        }
        self.pos += 1;
        Ok(c)
    }
}

fn paths_disjoint(a: &[(usize, usize)], b: &[(usize, usize)]) -> bool {
    for (x, y) in a.iter().zip(b.iter()) {
        if x == y {
            continue;
        }
        return x.0 == y.0 && x.1 != y.1;
    }
    false
}

pub fn space_set() -> CharSet {
    let mut r: Vec<(u32, u32)> = vec![(9, 13), (0xFEFF, 0xFEFF), (0x2028, 0x2029)];
    let zs = unicode::gc_ranges(1 << unicode::gc_index("Zs"));
    for c in zs.chunks(2) {
        r.push((c[0], c[1]));
    }
    CharSet::from_ranges(r)
}
