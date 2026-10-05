//! Backtracking matcher (ECMA-262 §22.2.2): the pattern tree is compiled to a small instruction program run by a
//! loop with an explicit backtrack stack (branch points plus undo records for captures and loop registers), so
//! match depth never touches the native stack. Lookarounds run as nested sub-matches (bounded by pattern nesting).

use super::parser::{CharSet, ModFlags, Node, Regex};
use crate::unicode;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
enum Inst {
    /// Literal (already canonicalized when `icase`).
    Char { c: u32, icase: bool, back: bool },
    Any { dotall: bool, back: bool },
    Set { idx: usize, neg: bool, icase: bool, back: bool },
    LineStart { multi: bool },
    LineEnd { multi: bool },
    WordB { neg: bool, ui: bool },
    Split(usize, usize),
    Jmp(usize),
    Save(usize),
    RepInit(usize),
    RepLoop { reg: usize, min: u32, max: u32, greedy: bool, exit: usize },
    RepStart { posreg: usize, first: usize, last: usize },
    RepEnd { reg: usize, posreg: usize, min: u32, lp: usize },
    BackRef { groups: Box<[usize]>, icase: bool, back: bool },
    /// Greedy repetition of a single-character matcher with no captures (gives back one character at a time).
    Star { atom: Box<Inst>, min: u32, max: u32 },
    Look { neg: bool, end: usize },
    Match,
}

/// A compiled pattern.
#[derive(Debug)]
pub struct Program {
    code: Vec<Inst>,
    sets: Vec<CharSet>,
    nregs: usize,
    pub ncaps: usize,
    unicode: bool,
    /// Inverse canonicalization: (canonical, other) for every code point whose Canonicalize differs from itself.
    inv: Vec<(u32, u32)>,
}

#[derive(Clone, Copy)]
struct Ctx {
    i: bool,
    m: bool,
    s: bool,
    back: bool,
}

struct Compiler<'a> {
    code: Vec<Inst>,
    sets: Vec<CharSet>,
    nregs: usize,
    unicode: bool,
    any_icase: bool,
    _re: &'a Regex,
}

pub fn canonicalize(c: u32, unicode: bool) -> u32 {
    if unicode {
        return unicode::simple_fold(c);
    }
    if c < 128 {
        return (c as u8).to_ascii_uppercase() as u32;
    }
    let mut v = Vec::new();
    unicode::to_upper_full(c, &mut v);
    if v.len() != 1 {
        return c;
    }
    let u = v[0];
    if u > 0xFFFF || u < 128 {
        return c;
    }
    u
}

impl Compiler<'_> {
    fn emit(&mut self, i: Inst) -> usize {
        self.code.push(i);
        self.code.len() - 1
    }
    fn canon(&self, c: u32, cx: Ctx) -> u32 {
        if cx.i { canonicalize(c, self.unicode) } else { c }
    }
    fn node(&mut self, n: &Node, cx: Ctx) {
        match n {
            Node::Empty => {}
            Node::Char(c) => {
                let c = self.canon(*c, cx);
                self.emit(Inst::Char { c, icase: cx.i, back: cx.back });
            }
            Node::Dot => {
                self.emit(Inst::Any { dotall: cx.s, back: cx.back });
            }
            Node::Set(set, neg) => {
                if cx.i {
                    self.any_icase = true;
                }
                if set.strings.is_empty() {
                    let idx = self.sets.len();
                    let mut set = (**set).clone();
                    // Unicode + ignoreCase: \w gains U+017F and U+212A, \W loses them (§22.2.2.9.4 WordCharacters).
                    if cx.i && self.unicode {
                        let w = CharSet::from_ranges(vec![(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)]);
                        let extra = CharSet::from_ranges(vec![(0x17F, 0x17F), (0x212A, 0x212A)]);
                        if set == w {
                            set.add_set(&extra);
                        } else if set == w.complement() {
                            set = set.subtract(&extra);
                        }
                    }
                    self.sets.push(set);
                    self.emit(Inst::Set { idx, neg: *neg, icase: cx.i, back: cx.back });
                } else {
                    // v-mode class of strings: longest strings first, then single characters (§22.2.2.9).
                    let mut strs: Vec<&Vec<u32>> = set.strings.iter().collect();
                    strs.sort_by(|a, b| b.len().cmp(&a.len()));
                    let mut alts: Vec<Node> = strs.iter().map(|s| Node::Seq(s.iter().map(|&c| Node::Char(c)).collect())).collect();
                    let mut single = (**set).clone();
                    single.strings.clear();
                    if !single.ranges.is_empty() {
                        alts.push(Node::Set(Box::new(single), false));
                    }
                    self.alt(&alts, cx);
                }
            }
            Node::LineStart => {
                self.emit(Inst::LineStart { multi: cx.m });
            }
            Node::LineEnd => {
                self.emit(Inst::LineEnd { multi: cx.m });
            }
            Node::WordBoundary(neg) => {
                self.emit(Inst::WordB { neg: *neg, ui: cx.i && self.unicode });
            }
            Node::Seq(v) => {
                if cx.back {
                    for x in v.iter().rev() {
                        self.node(x, cx);
                    }
                } else {
                    for x in v {
                        self.node(x, cx);
                    }
                }
            }
            Node::Alt(v) => self.alt(v, cx),
            Node::Group(x, idx) => match idx {
                Some(i) => {
                    let (a, b) = if cx.back { (2 * i + 1, 2 * i) } else { (2 * i, 2 * i + 1) };
                    self.emit(Inst::Save(a));
                    self.node(x, cx);
                    self.emit(Inst::Save(b));
                }
                None => self.node(x, cx),
            },
            Node::Look { behind, negated, node, .. } => {
                let at = self.emit(Inst::Look { neg: *negated, end: 0 });
                self.node(node, Ctx { back: *behind, ..cx });
                self.emit(Inst::Match);
                let end = self.code.len();
                self.code[at] = Inst::Look { neg: *negated, end };
            }
            Node::Repeat { node, min, max, greedy, first_group, last_group } => {
                let max = max.unwrap_or(u32::MAX);
                if max == 0 {
                    return;
                }
                if *min == 1 && max == 1 {
                    self.node(node, cx);
                    return;
                }
                let single = match &**node {
                    Node::Char(_) | Node::Dot => true,
                    Node::Set(set, _) => set.strings.is_empty(),
                    _ => false,
                };
                if single && *greedy {
                    let at = self.code.len();
                    self.node(node, cx);
                    if self.code.len() == at + 1 {
                        let atom = Box::new(self.code.pop().unwrap());
                        self.emit(Inst::Star { atom, min: *min, max });
                        return;
                    }
                    self.code.truncate(at);
                }
                let reg = self.nregs;
                let posreg = self.nregs + 1;
                self.nregs += 2;
                self.emit(Inst::RepInit(reg));
                let lp = self.emit(Inst::RepLoop { reg, min: *min, max, greedy: *greedy, exit: 0 });
                self.emit(Inst::RepStart { posreg, first: *first_group, last: *last_group });
                self.node(node, cx);
                self.emit(Inst::RepEnd { reg, posreg, min: *min, lp });
                let exit = self.code.len();
                self.code[lp] = Inst::RepLoop { reg, min: *min, max, greedy: *greedy, exit };
            }
            Node::BackRef(groups) => {
                self.emit(Inst::BackRef { groups: groups.clone().into_boxed_slice(), icase: cx.i, back: cx.back });
                if cx.i {
                    self.any_icase = true;
                }
            }
            Node::NamedRef(_) => {}
            Node::Modifiers { add, remove, node } => {
                let f = |c: bool, a: bool, r: bool| if a { true } else if r { false } else { c };
                let ModFlags { i: ai, m: am, s: as_ } = *add;
                let ModFlags { i: ri, m: rm, s: rs } = *remove;
                let ncx = Ctx { i: f(cx.i, ai, ri), m: f(cx.m, am, rm), s: f(cx.s, as_, rs), back: cx.back };
                if ncx.i {
                    self.any_icase = true;
                }
                self.node(node, ncx);
            }
        }
    }
    fn alt(&mut self, v: &[Node], cx: Ctx) {
        if v.is_empty() {
            return;
        }
        let mut jumps = Vec::new();
        for (k, x) in v.iter().enumerate() {
            if k + 1 < v.len() {
                let sp = self.emit(Inst::Split(0, 0));
                self.node(x, cx);
                jumps.push(self.emit(Inst::Jmp(0)));
                let next = self.code.len();
                self.code[sp] = Inst::Split(sp + 1, next);
            } else {
                self.node(x, cx);
            }
        }
        let end = self.code.len();
        for j in jumps {
            self.code[j] = Inst::Jmp(end);
        }
    }
}

pub fn compile(re: &Regex) -> Program {
    let unicode = re.flags.unicode();
    let mut c = Compiler { code: Vec::new(), sets: Vec::new(), nregs: 0, unicode, any_icase: re.flags.i, _re: re };
    let cx = Ctx { i: re.flags.i, m: re.flags.m, s: re.flags.s, back: false };
    c.emit(Inst::Save(0));
    c.node(&re.node, cx);
    c.emit(Inst::Save(1));
    c.emit(Inst::Match);
    let mut inv = Vec::new();
    if c.any_icase {
        if unicode {
            for (x, f) in unicode::fold_pairs() {
                if x != f {
                    inv.push((f, x));
                }
            }
        } else {
            for (x, _) in unicode::upper_pairs() {
                if x > 0xFFFF {
                    continue;
                }
                let k = canonicalize(x, false);
                if k != x {
                    inv.push((k, x));
                }
            }
        }
        inv.sort_unstable();
        inv.dedup();
    }
    Program { code: c.code, sets: c.sets, nregs: c.nregs, ncaps: re.ngroups + 1, unicode, inv }
}

enum Bt {
    Branch(usize, usize),
    /// Greedy single-character loop: resume at `pc` after giving back one character, while `cur` != `lo`.
    Star { pc: usize, lo: usize, cur: usize, back: bool },
    Cap(usize, isize),
    Reg(usize, usize),
}

struct M<'a> {
    p: &'a Program,
    s: &'a [u16],
    caps: Vec<isize>,
    regs: Vec<usize>,
    bt: Vec<Bt>,
    steps: u64,
    limit: u64,
    aborted: bool,
}

fn is_lt(c: u32) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

fn is_word(c: u32, ui: bool) -> bool {
    matches!(c, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A) || (ui && (c == 0x17F || c == 0x212A))
}

impl M<'_> {
    #[inline]
    fn next(&self, pos: usize) -> Option<(u32, usize)> {
        if pos >= self.s.len() {
            return None;
        }
        if self.p.unicode {
            let (c, n) = crate::string::code_point_at(self.s, pos);
            Some((c, pos + n))
        } else {
            Some((self.s[pos] as u32, pos + 1))
        }
    }
    #[inline]
    fn prev(&self, pos: usize) -> Option<(u32, usize)> {
        if pos == 0 {
            return None;
        }
        let lo = self.s[pos - 1];
        if self.p.unicode && (0xDC00..0xE000).contains(&lo) && pos >= 2 {
            let hi = self.s[pos - 2];
            if (0xD800..0xDC00).contains(&hi) {
                return Some((0x10000 + (((hi as u32) - 0xD800) << 10) + (lo as u32 - 0xDC00), pos - 2));
            }
        }
        Some((lo as u32, pos - 1))
    }
    #[inline]
    fn read(&self, pos: usize, back: bool) -> Option<(u32, usize)> {
        if back { self.prev(pos) } else { self.next(pos) }
    }
    fn canon(&self, c: u32) -> u32 {
        canonicalize(c, self.p.unicode)
    }
    fn set_has(&self, idx: usize, c: u32, icase: bool) -> bool {
        let set = &self.p.sets[idx];
        if set.contains(c) {
            return true;
        }
        if !icase {
            return false;
        }
        let t = self.canon(c);
        if t != c && set.contains(t) && self.canon(t) == t {
            return true;
        }
        let inv = &self.p.inv;
        let mut k = inv.partition_point(|e| e.0 < t);
        while k < inv.len() && inv[k].0 == t {
            if set.contains(inv[k].1) {
                return true;
            }
            k += 1;
        }
        false
    }
    /// Match one single-character instruction at `pos`.
    #[inline]
    fn step1(&self, inst: &Inst, pos: usize) -> Option<usize> {
        match inst {
            Inst::Char { c, icase, back } => match self.read(pos, *back) {
                Some((ch, np)) if ch == *c || (*icase && self.canon(ch) == *c) => Some(np),
                _ => None,
            },
            Inst::Any { dotall, back } => match self.read(pos, *back) {
                Some((ch, np)) if *dotall || !is_lt(ch) => Some(np),
                _ => None,
            },
            Inst::Set { idx, neg, icase, back } => match self.read(pos, *back) {
                Some((ch, np)) if self.set_has(*idx, ch, *icase) != *neg => Some(np),
                _ => None,
            },
            _ => None,
        }
    }
    fn set_cap(&mut self, slot: usize, v: isize) {
        let old = self.caps[slot];
        self.bt.push(Bt::Cap(slot, old));
        self.caps[slot] = v;
    }
    fn set_reg(&mut self, r: usize, v: usize) {
        let old = self.regs[r];
        self.bt.push(Bt::Reg(r, old));
        self.regs[r] = v;
    }

    /// Run from `pc` at `pos` until a `Match`; returns the end position, leaving undo records on the stack.
    fn run(&mut self, mut pc: usize, mut pos: usize) -> Option<usize> {
        let base = self.bt.len();
        loop {
            self.steps += 1;
            if self.steps > self.limit {
                self.aborted = true;
                self.unwind(base);
                return None;
            }
            let ok = match &self.p.code[pc] {
                Inst::Char { c, icase, back } => match self.read(pos, *back) {
                    Some((ch, np)) if ch == *c || (*icase && self.canon(ch) == *c) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Any { dotall, back } => match self.read(pos, *back) {
                    Some((ch, np)) if *dotall || !is_lt(ch) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Set { idx, neg, icase, back } => match self.read(pos, *back) {
                    Some((ch, np)) if self.set_has(*idx, ch, *icase) != *neg => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::LineStart { multi } => {
                    if pos == 0 || (*multi && is_lt(self.s[pos - 1] as u32)) {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::LineEnd { multi } => {
                    if pos == self.s.len() || (*multi && is_lt(self.s[pos] as u32)) {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::WordB { neg, ui } => {
                    let a = pos > 0 && is_word(self.s[pos - 1] as u32, *ui);
                    let b = pos < self.s.len() && is_word(self.s[pos] as u32, *ui);
                    if (a != b) != *neg {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Split(a, b) => {
                    self.bt.push(Bt::Branch(*b, pos));
                    pc = *a;
                    true
                }
                Inst::Jmp(a) => {
                    pc = *a;
                    true
                }
                Inst::Save(slot) => {
                    let slot = *slot;
                    self.set_cap(slot, pos as isize);
                    pc += 1;
                    true
                }
                Inst::RepInit(r) => {
                    let r = *r;
                    self.set_reg(r, 0);
                    pc += 1;
                    true
                }
                Inst::RepLoop { reg, min, max, greedy, exit } => {
                    let c = self.regs[*reg] as u64;
                    if c < *min as u64 {
                        pc += 1;
                    } else if c >= *max as u64 {
                        pc = *exit;
                    } else if *greedy {
                        self.bt.push(Bt::Branch(*exit, pos));
                        pc += 1;
                    } else {
                        self.bt.push(Bt::Branch(pc + 1, pos));
                        pc = *exit;
                    }
                    true
                }
                Inst::RepStart { posreg, first, last } => {
                    let (posreg, first, last) = (*posreg, *first, *last);
                    self.set_reg(posreg, pos);
                    for g in first..=last {
                        if g == 0 {
                            continue;
                        }
                        if self.caps[2 * g] != -1 || self.caps[2 * g + 1] != -1 {
                            self.set_cap(2 * g, -1);
                            self.set_cap(2 * g + 1, -1);
                        }
                    }
                    pc += 1;
                    true
                }
                Inst::RepEnd { reg, posreg, min, lp } => {
                    let (reg, lp) = (*reg, *lp);
                    if self.regs[reg] as u64 >= *min as u64 && pos == self.regs[*posreg] {
                        false
                    } else {
                        let c = self.regs[reg];
                        self.set_reg(reg, c.saturating_add(1));
                        pc = lp;
                        true
                    }
                }
                Inst::BackRef { groups, icase, back } => {
                    let mut range = None;
                    for &g in groups.iter() {
                        let (a, b) = (self.caps[2 * g], self.caps[2 * g + 1]);
                        if a >= 0 && b >= 0 {
                            range = Some((a as usize, b as usize));
                            break;
                        }
                    }
                    match range {
                        None => {
                            pc += 1;
                            true
                        }
                        Some((a, b)) => {
                            let len = b - a;
                            let (ok, np) = if *back {
                                if pos < len { (false, 0) } else { (self.eq_range(a, pos - len, len, *icase), pos - len) }
                            } else if pos + len > self.s.len() {
                                (false, 0)
                            } else {
                                (self.eq_range(a, pos, len, *icase), pos + len)
                            };
                            if ok {
                                pos = np;
                                pc += 1;
                            }
                            ok
                        }
                    }
                }
                Inst::Star { atom, min, max } => {
                    let back = matches!(&**atom, Inst::Char { back: true, .. } | Inst::Any { back: true, .. } | Inst::Set { back: true, .. });
                    let mut n = 0u32;
                    let mut p = pos;
                    let mut lo = pos;
                    let mut ok = true;
                    while n < *max {
                        match self.step1(atom, p) {
                            Some(np) => {
                                p = np;
                                n += 1;
                                if n == *min {
                                    lo = p;
                                }
                            }
                            None => break,
                        }
                    }
                    self.steps += n as u64;
                    if n < *min {
                        ok = false;
                    } else {
                        if *min == 0 {
                            lo = pos;
                        }
                        if p != lo {
                            self.bt.push(Bt::Star { pc: pc + 1, lo, cur: p, back });
                        }
                        pos = p;
                        pc += 1;
                    }
                    ok
                }
                Inst::Look { neg, end } => {
                    let (neg, end) = (*neg, *end);
                    let b2 = self.bt.len();
                    let r = self.run(pc + 1, pos);
                    if self.aborted {
                        self.unwind(base);
                        return None;
                    }
                    match (r.is_some(), neg) {
                        (true, false) => {
                            // Keep capture/register undo records, drop branch points (lookarounds are atomic).
                            let mut w = b2;
                            for k in b2..self.bt.len() {
                                if !matches!(self.bt[k], Bt::Branch(..) | Bt::Star { .. }) {
                                    self.bt.swap(w, k);
                                    w += 1;
                                }
                            }
                            self.bt.truncate(w);
                            pc = end;
                            true
                        }
                        (true, true) => {
                            self.unwind(b2);
                            false
                        }
                        (false, false) => false,
                        (false, true) => {
                            pc = end;
                            true
                        }
                    }
                }
                Inst::Match => return Some(pos),
            };
            if !ok {
                loop {
                    if self.bt.len() == base {
                        return None;
                    }
                    match self.bt.pop().unwrap() {
                        Bt::Branch(p, q) => {
                            pc = p;
                            pos = q;
                            break;
                        }
                        Bt::Star { pc: p, lo, cur, back } => {
                            // Give back one character (the inverse direction of the loop's reads).
                            let np = if back { self.next(cur).map(|x| x.1).unwrap_or(lo) } else { self.prev(cur).map(|x| x.1).unwrap_or(lo) };
                            if np != lo {
                                self.bt.push(Bt::Star { pc: p, lo, cur: np, back });
                            }
                            pc = p;
                            pos = np;
                            break;
                        }
                        Bt::Cap(s, v) => self.caps[s] = v,
                        Bt::Reg(r, v) => self.regs[r] = v,
                    }
                }
            }
        }
    }
    fn unwind(&mut self, base: usize) {
        while self.bt.len() > base {
            match self.bt.pop().unwrap() {
                Bt::Branch(..) | Bt::Star { .. } => {}
                Bt::Cap(s, v) => self.caps[s] = v,
                Bt::Reg(r, v) => self.regs[r] = v,
            }
        }
    }
    fn eq_range(&self, a: usize, b: usize, len: usize, icase: bool) -> bool {
        if !icase {
            return self.s[a..a + len] == self.s[b..b + len];
        }
        let (mut i, mut j) = (a, b);
        while i < a + len {
            let (c1, n1) = self.next(i).unwrap();
            let (c2, n2) = match self.next(j) {
                Some(x) => x,
                None => return false,
            };
            if c1 != c2 && self.canon(c1) != self.canon(c2) {
                return false;
            }
            i = n1;
            j = n2;
        }
        j == b + len
    }
}

/// Outcome of one match attempt at a fixed start position.
pub enum MatchResult {
    /// Capture slots (code-unit indices, -1 = undefined), 2 per group, group 0 first.
    Match(Vec<isize>),
    NoMatch,
    /// The step budget was exhausted.
    Aborted,
}

/// Match `prog` against `s` anchored at `start` (the caller loops over start positions for non-sticky searches).
pub fn match_at(prog: &Program, s: &[u16], start: usize, limit: u64) -> MatchResult {
    let mut m = M { p: prog, s, caps: vec![-1; prog.ncaps * 2], regs: vec![0; prog.nregs], bt: Vec::new(), steps: 0, limit, aborted: false };
    match m.run(0, start) {
        Some(_) => MatchResult::Match(m.caps),
        None if m.aborted => MatchResult::Aborted,
        None => MatchResult::NoMatch,
    }
}

/// Search forward from `start`; returns the first match. `budget` is shared across start positions.
pub fn search(prog: &Program, s: &[u16], start: usize, sticky: bool, limit: u64) -> MatchResult {
    let mut m = M { p: prog, s, caps: vec![-1; prog.ncaps * 2], regs: vec![0; prog.nregs], bt: Vec::new(), steps: 0, limit, aborted: false };
    let mut pos = start;
    loop {
        if pos > s.len() {
            return MatchResult::NoMatch;
        }
        if m.run(0, pos).is_some() {
            return MatchResult::Match(m.caps);
        }
        if m.aborted {
            return MatchResult::Aborted;
        }
        if sticky {
            return MatchResult::NoMatch;
        }
        // AdvanceStringIndex
        pos += if prog.unicode && pos < s.len() { crate::string::code_point_at(s, pos).1 } else { 1 };
    }
}
