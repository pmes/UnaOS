// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The piece table and its line index. See the crate header for why a piece table.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// A position as the API speaks it: a 0-based line and a UTF-8 byte offset within that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Pos {
    pub line: usize,
    pub byte: usize,
}

impl Pos {
    pub const fn new(line: usize, byte: usize) -> Self {
        Pos { line, byte }
    }
}

/// How a line ends, as loaded (or as typed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// The last line: no terminator.
    None,
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::None => "",
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Src {
    Orig,
    Add,
}

#[derive(Debug, Clone, Copy)]
struct Piece {
    src: Src,
    start: usize,
    len: usize,
}

/// UTF-8 text as a piece table over an immutable original buffer and an append-only add buffer, with a
/// line index (the byte offset of every line start) maintained incrementally on every edit.
#[derive(Clone)]
pub struct Text {
    orig: String,
    add: String,
    pieces: Vec<Piece>,
    len: usize,
    line_starts: Vec<usize>,
}

impl Default for Text {
    fn default() -> Self {
        Text::new("")
    }
}

impl PartialEq for Text {
    /// Equal when the TEXT is equal, whatever the piece structure.
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.chunks().flat_map(str::bytes).eq(other.chunks().flat_map(str::bytes))
    }
}
impl Eq for Text {}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Text")
            .field("len", &self.len)
            .field("lines", &self.line_starts.len())
            .field("pieces", &self.pieces.len())
            .finish()
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.chunks() {
            f.write_str(c)?;
        }
        Ok(())
    }
}

fn scan_lines(s: &str, base: usize, out: &mut Vec<usize>) {
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            out.push(base + i + 1);
        }
    }
}

impl Text {
    /// A text whose original buffer is `s` (one piece; nothing is copied into nodes).
    pub fn new(s: &str) -> Self {
        let mut line_starts = Vec::new();
        line_starts.push(0);
        scan_lines(s, 0, &mut line_starts);
        let mut pieces = Vec::new();
        if !s.is_empty() {
            pieces.push(Piece { src: Src::Orig, start: 0, len: s.len() });
        }
        Text { orig: String::from(s), add: String::new(), pieces, len: s.len(), line_starts }
    }

    fn src(&self, p: &Piece) -> &str {
        let b = match p.src {
            Src::Orig => &self.orig,
            Src::Add => &self.add,
        };
        &b[p.start..p.start + p.len]
    }

    /// The text as its contiguous pieces, in order.
    pub fn chunks(&self) -> impl Iterator<Item = &str> + '_ {
        self.pieces.iter().map(move |p| self.src(p))
    }

    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Number of pieces (a fragmentation measure; the fuzz watches it).
    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// The byte at `off` (`None` past the end).
    pub fn byte_at(&self, off: usize) -> Option<u8> {
        let mut acc = 0;
        for p in &self.pieces {
            if off < acc + p.len {
                return Some(self.src(p).as_bytes()[off - acc]);
            }
            acc += p.len;
        }
        None
    }

    /// True when `off` is `0`, `len`, or the first byte of a UTF-8 sequence.
    pub fn is_char_boundary(&self, off: usize) -> bool {
        if off == 0 || off == self.len {
            return true;
        }
        match self.byte_at(off) {
            Some(b) => (b as i8) >= -0x40,
            None => false,
        }
    }

    /// Round `off` down to a char boundary within `0..=len`.
    pub fn floor_boundary(&self, off: usize) -> usize {
        let mut o = off.min(self.len);
        while !self.is_char_boundary(o) {
            o -= 1;
        }
        o
    }

    /// The bytes `a..b` as a `String`. Both ends are clamped to `len` and snapped down to char boundaries.
    pub fn slice(&self, a: usize, b: usize) -> String {
        let a = self.floor_boundary(a);
        let b = self.floor_boundary(b).max(a);
        let mut out = String::with_capacity(b - a);
        let mut acc = 0;
        for p in &self.pieces {
            let (ps, pe) = (acc, acc + p.len);
            acc = pe;
            if pe <= a {
                continue;
            }
            if ps >= b {
                break;
            }
            let s = self.src(p);
            out.push_str(&s[a.max(ps) - ps..b.min(pe) - ps]);
        }
        out
    }

    /// Locate `off`: (piece index, offset within it). `off == len` gives `(pieces.len(), 0)`.
    fn locate(&self, off: usize) -> (usize, usize) {
        let mut acc = 0;
        for (i, p) in self.pieces.iter().enumerate() {
            if off < acc + p.len {
                return (i, off - acc);
            }
            acc += p.len;
        }
        (self.pieces.len(), 0)
    }

    /// Insert `s` at byte `off`. Returns `false` (and changes nothing) if `off` is not a char boundary
    /// in `0..=len`.
    pub fn insert(&mut self, off: usize, s: &str) -> bool {
        if off > self.len || !self.is_char_boundary(off) {
            return false;
        }
        if s.is_empty() {
            return true;
        }
        let add_start = self.add.len();
        self.add.push_str(s);
        let new = Piece { src: Src::Add, start: add_start, len: s.len() };
        let (i, inner) = self.locate(off);
        if inner == 0 {
            // At a piece boundary: extend the previous piece if it ends where the add buffer did.
            if i > 0 {
                let prev = &mut self.pieces[i - 1];
                if prev.src == Src::Add && prev.start + prev.len == add_start {
                    prev.len += s.len();
                    self.after_insert(off, s);
                    return true;
                }
            }
            self.pieces.insert(i, new);
        } else {
            let p = self.pieces[i];
            let left = Piece { len: inner, ..p };
            let right = Piece { start: p.start + inner, len: p.len - inner, ..p };
            self.pieces.splice(i..=i, [left, new, right]);
        }
        self.after_insert(off, s);
        true
    }

    fn after_insert(&mut self, off: usize, s: &str) {
        self.len += s.len();
        let k = self.line_starts.partition_point(|&st| st <= off);
        for st in &mut self.line_starts[k..] {
            *st += s.len();
        }
        let mut new = Vec::new();
        scan_lines(s, off, &mut new);
        if !new.is_empty() {
            self.line_starts.splice(k..k, new);
        }
    }

    /// Delete bytes `a..b`. Returns `false` (and changes nothing) unless `a <= b <= len` are both char
    /// boundaries.
    pub fn delete(&mut self, a: usize, b: usize) -> bool {
        if a > b || b > self.len || !self.is_char_boundary(a) || !self.is_char_boundary(b) {
            return false;
        }
        if a == b {
            return true;
        }
        let mut out: Vec<Piece> = Vec::with_capacity(self.pieces.len() + 1);
        let mut acc = 0;
        for p in &self.pieces {
            let (ps, pe) = (acc, acc + p.len);
            acc = pe;
            if pe <= a || ps >= b {
                out.push(*p);
                continue;
            }
            if ps < a {
                out.push(Piece { len: a - ps, ..*p });
            }
            if pe > b {
                out.push(Piece { start: p.start + (b - ps), len: pe - b, ..*p });
            }
        }
        self.pieces = out;
        self.len -= b - a;
        let lo = self.line_starts.partition_point(|&st| st <= a);
        let hi = self.line_starts.partition_point(|&st| st <= b);
        self.line_starts.drain(lo..hi);
        for st in &mut self.line_starts[lo..] {
            *st -= b - a;
        }
        true
    }

    // ---- the line index ------------------------------------------------------------------------------

    /// Number of lines: one more than the number of LFs.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte offset where `line` starts (`len` past the last line).
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts.get(line).copied().unwrap_or(self.len)
    }

    /// Byte offset where `line` ends, terminator INCLUDED (= the next line's start, or `len`).
    pub fn line_end_full(&self, line: usize) -> usize {
        self.line_starts.get(line + 1).copied().unwrap_or(self.len)
    }

    /// The ending `line` carries.
    pub fn line_ending(&self, line: usize) -> LineEnding {
        if line + 1 >= self.line_starts.len() {
            return LineEnding::None;
        }
        let e = self.line_starts[line + 1]; // just past the LF
        if e >= 2 && e - 2 >= self.line_start(line) && self.byte_at(e - 2) == Some(b'\r') {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }

    /// Byte offset where `line`'s content ends (terminator EXCLUDED).
    pub fn line_end(&self, line: usize) -> usize {
        self.line_end_full(line) - self.line_ending(line).as_str().len()
    }

    /// `line`'s content, without its terminator.
    pub fn line(&self, line: usize) -> String {
        self.slice(self.line_start(line), self.line_end(line))
    }

    /// `line`'s content WITH its terminator.
    pub fn line_full(&self, line: usize) -> String {
        self.slice(self.line_start(line), self.line_end_full(line))
    }

    /// The line containing byte `off` (offsets past the end map to the last line).
    pub fn line_of(&self, off: usize) -> usize {
        self.line_starts.partition_point(|&st| st <= off).max(1) - 1
    }

    /// Byte offset → `Pos`.
    pub fn offset_to_pos(&self, off: usize) -> Pos {
        let off = off.min(self.len);
        let line = self.line_of(off);
        Pos { line, byte: off - self.line_starts[line] }
    }

    /// `Pos` → byte offset. A line past the end clamps to `len`; a byte past the line's content clamps to
    /// the content end (never inside a CRLF), then down to a char boundary.
    pub fn pos_to_offset(&self, pos: Pos) -> usize {
        if pos.line >= self.line_starts.len() {
            return self.len;
        }
        let s = self.line_starts[pos.line];
        self.floor_boundary((s + pos.byte).min(self.line_end(pos.line)))
    }

    /// The index as the naive scan would build it — the invariant the fuzz checks.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if self.pieces.iter().any(|p| p.len == 0) {
            return Err("empty piece");
        }
        if self.pieces.iter().map(|p| p.len).sum::<usize>() != self.len {
            return Err("piece lengths do not sum to len");
        }
        let mut naive = Vec::new();
        naive.push(0);
        let mut acc = 0;
        for c in self.chunks() {
            scan_lines(c, acc, &mut naive);
            acc += c.len();
        }
        if naive != self.line_starts {
            return Err("line index differs from a naive scan");
        }
        for c in self.chunks() {
            if core::str::from_utf8(c.as_bytes()).is_err() {
                return Err("piece is not UTF-8");
            }
        }
        Ok(())
    }
}
