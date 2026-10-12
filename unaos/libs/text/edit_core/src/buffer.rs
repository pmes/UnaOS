// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! `Buffer`: the editor state of record — text, selections, undo/redo, edit operations and motion.

use crate::search::{self, SearchOpts};
use crate::text::{LineEnding, Pos, Text};
use crate::unicode;
use crate::viewport::{self, Viewport};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

/// One selection: `anchor` is where it was started, `head` is the moving end (the caret). Both are byte
/// offsets on char (and, after motion, grapheme) boundaries. A cursor is a selection with `anchor == head`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub const fn cursor(at: usize) -> Self {
        Selection { anchor: at, head: at }
    }
    pub const fn new(anchor: usize, head: usize) -> Self {
        Selection { anchor, head }
    }
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }
}

/// What a delete removes when the selection is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// One extended grapheme cluster (a CRLF is one cluster).
    Grapheme,
    /// To the previous word start / the next word end (UAX #29 word boundaries, whitespace skipped).
    Word,
    /// To the line start (backward) / the line end (forward); at that edge, the line ending itself.
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Backward,
    Forward,
}

/// A cursor motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    Up,
    Down,
    DocStart,
    DocEnd,
}

/// Indentation unit for auto-indent, indent and outdent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    Tabs,
    Spaces(u8),
}

/// One applied edit, as a view needs it: the old byte range `start..old_end` was replaced by `text`.
/// Positions are in the text as it stood immediately before (`start_pos`, `old_end_pos`) and after
/// (`new_end_pos`) THIS change; a list of changes is applied in order. Lines `first_line..=old_last_line`
/// of the old text became lines `first_line..=new_last_line` of the new — the only lines to re-render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub start: usize,
    pub old_end: usize,
    pub text: String,
    pub start_pos: Pos,
    pub old_end_pos: Pos,
    pub new_end_pos: Pos,
    pub first_line: usize,
    pub old_last_line: usize,
    pub new_last_line: usize,
}

impl Change {
    /// Byte offset where the inserted text ends in the new text.
    pub fn new_end(&self) -> usize {
        self.start + self.text.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Typing,
    Erasing,
    Other,
}

#[derive(Debug, Clone)]
struct Edit {
    at: usize,
    deleted: String,
    inserted: String,
}

#[derive(Debug, Clone)]
struct Group {
    kind: Kind,
    edits: Vec<Edit>,
    before: Vec<Selection>,
    after: Vec<Selection>,
}

/// The editor buffer of record.
#[derive(Clone)]
pub struct Buffer {
    text: Text,
    sels: Vec<Selection>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    /// The top undo group may still absorb typing of its kind.
    open: bool,
    indent: IndentStyle,
    /// Ending used for a newline typed on the last line (which has none): the file's first ending as loaded.
    default_ending: LineEnding,
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::new("")
    }
}

impl PartialEq for Buffer {
    /// Buffers compare by TEXT (the piece structure, cursors and history are not content).
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}
impl Eq for Buffer {}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Buffer")
            .field("text", &self.text)
            .field("selections", &self.sels)
            .field("undo", &self.undo.len())
            .field("redo", &self.redo.len())
            .finish()
    }
}

impl fmt::Display for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.text, f)
    }
}

impl Buffer {
    /// A buffer holding `s` exactly as given (line endings preserved), one cursor at 0, empty history.
    pub fn new(s: &str) -> Self {
        let text = Text::new(s);
        let default_ending = if text.line_count() > 1 { text.line_ending(0) } else { LineEnding::Lf };
        Buffer {
            text,
            sels: alloc::vec![Selection::cursor(0)],
            undo: Vec::new(),
            redo: Vec::new(),
            open: false,
            indent: IndentStyle::Spaces(4),
            default_ending,
        }
    }

    // ---- reading ----------------------------------------------------------------------------------------

    pub fn text(&self) -> &Text {
        &self.text
    }
    pub fn len(&self) -> usize {
        self.text.len()
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub fn line_count(&self) -> usize {
        self.text.line_count()
    }
    /// `line`'s content without its terminator.
    pub fn line(&self, line: usize) -> String {
        self.text.line(line)
    }
    pub fn line_ending(&self, line: usize) -> LineEnding {
        self.text.line_ending(line)
    }
    pub fn slice(&self, r: Range<usize>) -> String {
        self.text.slice(r.start, r.end)
    }
    pub fn offset_to_pos(&self, off: usize) -> Pos {
        self.text.offset_to_pos(off)
    }
    pub fn pos_to_offset(&self, pos: Pos) -> usize {
        self.text.pos_to_offset(pos)
    }
    pub fn indent_style(&self) -> IndentStyle {
        self.indent
    }
    pub fn set_indent_style(&mut self, s: IndentStyle) {
        self.indent = s;
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// Number of undo groups (each `undo()` reverts exactly one).
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    // ---- selections -------------------------------------------------------------------------------------

    /// The selections, sorted by start and never overlapping.
    pub fn selections(&self) -> &[Selection] {
        &self.sels
    }

    /// The first selection's caret.
    pub fn cursor(&self) -> usize {
        self.sels[0].head
    }

    /// Replace every selection (offsets snapped to char boundaries; sorted; overlaps merged). Closes the
    /// open undo group, as any cursor move does.
    pub fn set_selections(&mut self, sels: &[Selection]) {
        self.sels = sels
            .iter()
            .map(|s| Selection::new(self.text.floor_boundary(s.anchor), self.text.floor_boundary(s.head)))
            .collect();
        if self.sels.is_empty() {
            self.sels.push(Selection::cursor(0));
        }
        self.normalize();
        self.open = false;
    }

    /// One cursor at `at`.
    pub fn set_cursor(&mut self, at: usize) {
        self.set_selections(&[Selection::cursor(at)]);
    }

    /// Add a cursor at `at` (merged with any selection it touches).
    pub fn add_cursor(&mut self, at: usize) {
        let mut v = self.sels.clone();
        v.push(Selection::cursor(at));
        self.set_selections(&v);
    }

    fn normalize(&mut self) {
        self.sels.sort_by_key(|s| (s.start(), s.end()));
        let mut out: Vec<Selection> = Vec::with_capacity(self.sels.len());
        for s in self.sels.drain(..) {
            if let Some(last) = out.last_mut() {
                // Overlapping, or two cursors at one spot: merge. Touching non-empty selections stay apart.
                let touch_merge = s.start() == last.end() && (s.is_empty() || last.is_empty());
                if s.start() < last.end() || touch_merge {
                    let (a, b) = (last.start().min(s.start()), last.end().max(s.end()));
                    // keep the direction of the earlier selection
                    *last = if last.head >= last.anchor { Selection::new(a, b) } else { Selection::new(b, a) };
                    continue;
                }
            }
            out.push(s);
        }
        self.sels = out;
    }

    /// Close the open undo group: the caller's 1 s idle signal (a cursor move closes it on its own).
    pub fn close_group(&mut self) {
        self.open = false;
    }

    // ---- the one primitive edit ---------------------------------------------------------------------------

    /// Replace `a..b` with `s`, adjust every selection, record into the top group if `record`.
    fn raw_replace(&mut self, a: usize, b: usize, s: &str, record: bool) -> Option<Change> {
        let a = self.text.floor_boundary(a);
        let b = self.text.floor_boundary(b).max(a);
        if a == b && s.is_empty() {
            return None;
        }
        let start_pos = self.text.offset_to_pos(a);
        let old_end_pos = self.text.offset_to_pos(b);
        let deleted = self.text.slice(a, b);
        let ok = self.text.delete(a, b) && self.text.insert(a, s);
        debug_assert!(ok, "raw_replace on boundaries cannot fail");
        let n = s.len();
        let shift = |x: usize| -> usize {
            if x < a {
                x
            } else if x >= b {
                x - (b - a) + n
            } else {
                a + n
            }
        };
        for sel in &mut self.sels {
            sel.anchor = shift(sel.anchor);
            sel.head = shift(sel.head);
        }
        if record {
            if let Some(g) = self.undo.last_mut() {
                g.edits.push(Edit { at: a, deleted, inserted: String::from(s) });
            }
        }
        let new_end_pos = self.text.offset_to_pos(a + n);
        Some(Change {
            start: a,
            old_end: b,
            text: String::from(s),
            start_pos,
            old_end_pos,
            new_end_pos,
            first_line: start_pos.line,
            old_last_line: old_end_pos.line,
            new_last_line: new_end_pos.line,
        })
    }

    fn begin(&mut self, kind: Kind) {
        let coalesce = self.open && kind != Kind::Other && self.undo.last().map(|g| g.kind) == Some(kind);
        if !coalesce {
            self.undo.push(Group { kind, edits: Vec::new(), before: self.sels.clone(), after: Vec::new() });
        }
    }

    fn end(&mut self, kind: Kind, changes: &[Change]) {
        self.normalize();
        if let Some(g) = self.undo.last_mut() {
            if g.edits.is_empty() {
                self.undo.pop();
                return;
            }
            g.after = self.sels.clone();
        }
        if !changes.is_empty() {
            self.redo.clear();
        }
        self.open = kind != Kind::Other;
    }

    /// Apply one replacement per selection. `f` maps each selection to `(range, text)`; replacements are
    /// applied from the last selection to the first so earlier offsets stay valid, and a range that would
    /// overlap one already applied is clipped to stop where that one starts.
    fn edit_each<F>(&mut self, kind: Kind, mut f: F) -> Vec<Change>
    where
        F: FnMut(&Text, Selection) -> Option<(usize, usize, String)>,
    {
        self.begin(kind);
        let plans: Vec<Option<(usize, usize, String)>> = self.sels.clone().into_iter().map(|s| f(&self.text, s)).collect();
        let mut changes = Vec::new();
        let mut floor = usize::MAX;
        for p in plans.into_iter().rev().flatten() {
            let (a, mut b, s) = p;
            if a >= floor {
                continue;
            }
            b = b.min(floor);
            if let Some(c) = self.raw_replace(a, b, &s, true) {
                changes.push(c);
            }
            floor = a;
        }
        self.end(kind, &changes);
        changes
    }

    // ---- edit operations ----------------------------------------------------------------------------------

    /// Type `s` at every cursor, replacing each non-empty selection. Consecutive typing coalesces into ONE
    /// undo group until a cursor move, a non-typing edit, or [`Buffer::close_group`].
    pub fn insert(&mut self, s: &str) -> Vec<Change> {
        let s = String::from(s);
        self.edit_each(Kind::Typing, |_, sel| Some((sel.start(), sel.end(), s.clone())))
    }

    /// Replace `range` with `s` as ONE undo group of its own (paste, the host view's mirror, a tool's edit).
    /// Out-of-range ends are clamped and snapped to char boundaries; never panics.
    pub fn replace(&mut self, range: Range<usize>, s: &str) -> Vec<Change> {
        self.begin(Kind::Other);
        let changes: Vec<Change> = self.raw_replace(range.start, range.end, s, true).into_iter().collect();
        self.end(Kind::Other, &changes);
        changes
    }

    /// Replace the whole text with `s` through the smallest single edit (common prefix and suffix kept),
    /// as one undo group. Returns no change when the text is already `s`.
    pub fn set_text(&mut self, s: &str) -> Vec<Change> {
        let old = self.text.to_string();
        let ob = old.as_bytes();
        let nb = s.as_bytes();
        let mut p = 0;
        while p < ob.len() && p < nb.len() && ob[p] == nb[p] {
            p += 1;
        }
        while !old.is_char_boundary(p) || !s.is_char_boundary(p) {
            p -= 1;
        }
        let mut q = 0;
        while q < ob.len() - p && q < nb.len() - p && ob[ob.len() - 1 - q] == nb[nb.len() - 1 - q] {
            q += 1;
        }
        while !old.is_char_boundary(ob.len() - q) || !s.is_char_boundary(nb.len() - q) {
            q -= 1;
        }
        if p == ob.len() && p == nb.len() {
            return Vec::new();
        }
        self.replace(p..ob.len() - q, &s[p..nb.len() - q])
    }

    /// Delete at every selection: a non-empty selection is deleted whole; an empty one deletes one `unit`
    /// in `dir`. Backspacing graphemes coalesces like typing does.
    pub fn delete(&mut self, unit: Unit, dir: Direction) -> Vec<Change> {
        let kind = if unit == Unit::Grapheme { Kind::Erasing } else { Kind::Other };
        let this: &Buffer = self;
        let ranges: Vec<(usize, usize)> = this
            .sels
            .iter()
            .map(|sel| {
                if !sel.is_empty() {
                    return (sel.start(), sel.end());
                }
                let h = sel.head;
                match (unit, dir) {
                    (Unit::Grapheme, Direction::Backward) => (this.prev_grapheme(h), h),
                    (Unit::Grapheme, Direction::Forward) => (h, this.next_grapheme(h)),
                    (Unit::Word, Direction::Backward) => (this.word_left(h), h),
                    (Unit::Word, Direction::Forward) => (h, this.word_right(h)),
                    (Unit::Line, Direction::Backward) => {
                        let l = this.text.line_of(h);
                        let s = this.text.line_start(l);
                        if h > s { (s, h) } else if l > 0 { (this.text.line_end(l - 1), h) } else { (h, h) }
                    }
                    (Unit::Line, Direction::Forward) => {
                        let l = this.text.line_of(h);
                        let e = this.text.line_end(l);
                        if h < e { (h, e) } else { (h, this.text.line_end_full(l)) }
                    }
                }
            })
            .collect();
        let mut i = 0;
        self.edit_each(kind, |_, _| {
            let r = ranges[i];
            i += 1;
            Some((r.0, r.1, String::new()))
        })
    }

    /// Delete every line a selection touches, terminators included (the last line takes the terminator
    /// before it instead).
    pub fn delete_lines(&mut self) -> Vec<Change> {
        self.edit_each(Kind::Other, |t, sel| {
            let (l0, l1) = sel_lines(t, sel);
            let a = t.line_start(l0);
            let b = t.line_end_full(l1);
            if b == t.len() && l0 > 0 && t.line_ending(l1) == LineEnding::None {
                Some((t.line_end(l0 - 1), b, String::new()))
            } else {
                Some((a, b, String::new()))
            }
        })
    }

    /// Break the line at every cursor: the current line's own ending (CRLF stays CRLF) followed by the
    /// current line's leading whitespace (auto-indent), up to the cursor.
    pub fn newline(&mut self) -> Vec<Change> {
        let def = self.default_ending;
        self.edit_each(Kind::Other, |t, sel| {
            let l = t.line_of(sel.start());
            let ending = match t.line_ending(l) {
                LineEnding::None => def,
                e => e,
            };
            let ls = t.line_start(l);
            let upto = sel.start() - ls;
            let content = t.line(l);
            let ws: String = content.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let ws_len = ws.len().min(upto);
            let mut s = String::from(ending.as_str());
            s.push_str(&ws[..ws_len]);
            Some((sel.start(), sel.end(), s))
        })
    }

    fn indent_unit(&self) -> String {
        match self.indent {
            IndentStyle::Tabs => String::from("\t"),
            IndentStyle::Spaces(n) => " ".repeat(n.max(1) as usize),
        }
    }

    /// The distinct lines the selections touch, ascending.
    fn touched_lines(&self) -> Vec<usize> {
        let mut v: Vec<usize> = Vec::new();
        for s in &self.sels {
            let (a, b) = sel_lines(&self.text, *s);
            for l in a..=b {
                if v.last() != Some(&l) && !v.contains(&l) {
                    v.push(l);
                }
            }
        }
        v.sort_unstable();
        v
    }

    /// Indent every line a selection touches by one unit (tabs or spaces per [`IndentStyle`]); empty lines
    /// are left alone. One undo group.
    pub fn indent(&mut self) -> Vec<Change> {
        let unit = self.indent_unit();
        let lines = self.touched_lines();
        self.begin(Kind::Other);
        let mut changes = Vec::new();
        for &l in lines.iter().rev() {
            if self.text.line_end(l) == self.text.line_start(l) {
                continue;
            }
            let at = self.text.line_start(l);
            // A cursor exactly at the line start stays before the inserted indent only if it is an anchor
            // of a multi-line selection; the generic shift moves it after, which is what editors do.
            if let Some(c) = self.raw_replace(at, at, &unit, true) {
                changes.push(c);
            }
        }
        self.end(Kind::Other, &changes);
        changes
    }

    /// Remove up to one indent unit (one tab, or up to N spaces) from every line a selection touches.
    pub fn outdent(&mut self) -> Vec<Change> {
        let width = match self.indent {
            IndentStyle::Tabs => 4,
            IndentStyle::Spaces(n) => n.max(1) as usize,
        };
        let lines = self.touched_lines();
        self.begin(Kind::Other);
        let mut changes = Vec::new();
        for &l in lines.iter().rev() {
            let content = self.text.line(l);
            let cut = if content.starts_with('\t') {
                1
            } else {
                content.bytes().take(width).take_while(|&b| b == b' ').count()
            };
            if cut > 0 {
                let at = self.text.line_start(l);
                if let Some(c) = self.raw_replace(at, at + cut, "", true) {
                    changes.push(c);
                }
            }
        }
        self.end(Kind::Other, &changes);
        changes
    }

    // ---- undo / redo ---------------------------------------------------------------------------------------

    /// Revert the newest group; restores the selections it started with. `None` when there is nothing.
    pub fn undo(&mut self) -> Option<Vec<Change>> {
        let g = self.undo.pop()?;
        let mut changes = Vec::new();
        for e in g.edits.iter().rev() {
            if let Some(c) = self.raw_replace(e.at, e.at + e.inserted.len(), &e.deleted, false) {
                changes.push(c);
            }
        }
        self.sels = g.before.clone();
        self.redo.push(g);
        self.open = false;
        Some(changes)
    }

    /// Re-apply the newest undone group; restores the selections it ended with.
    pub fn redo(&mut self) -> Option<Vec<Change>> {
        let g = self.redo.pop()?;
        let mut changes = Vec::new();
        for e in &g.edits {
            if let Some(c) = self.raw_replace(e.at, e.at + e.deleted.len(), &e.inserted, false) {
                changes.push(c);
            }
        }
        self.sels = g.after.clone();
        self.undo.push(g);
        self.open = false;
        Some(changes)
    }

    // ---- motion ----------------------------------------------------------------------------------------------

    /// The next grapheme boundary after `off` (a line's CRLF is one step; the end of the text stays put).
    pub fn next_grapheme(&self, off: usize) -> usize {
        let off = self.text.floor_boundary(off);
        if off >= self.text.len() {
            return self.text.len();
        }
        let l = self.text.line_of(off);
        let ls = self.text.line_start(l);
        let full = self.text.line_full(l);
        ls + unicode::next_grapheme(&full, off - ls)
    }

    /// The previous grapheme boundary before `off` (from a line start, onto the previous line's content end).
    pub fn prev_grapheme(&self, off: usize) -> usize {
        let off = self.text.floor_boundary(off);
        if off == 0 {
            return 0;
        }
        let mut l = self.text.line_of(off);
        if off == self.text.line_start(l) {
            l -= 1;
        }
        let ls = self.text.line_start(l);
        let full = self.text.line_full(l);
        ls + unicode::prev_grapheme(&full, off - ls)
    }

    /// Word segments of line `l` as absolute ranges, whitespace-only and line-ending segments dropped.
    fn words_on_line(&self, l: usize) -> Vec<(usize, usize)> {
        let ls = self.text.line_start(l);
        let content = self.text.line(l);
        let b = unicode::word_boundaries(&content);
        b.windows(2)
            .filter(|w| !content[w[0]..w[1]].chars().all(char::is_whitespace))
            .map(|w| (ls + w[0], ls + w[1]))
            .collect()
    }

    /// The end of the first word (a non-whitespace UAX #29 segment) that ends after `off`, crossing lines;
    /// the text end when there is none.
    pub fn word_right(&self, off: usize) -> usize {
        let mut l = self.text.line_of(off);
        while l < self.text.line_count() {
            if let Some(&(_, e)) = self.words_on_line(l).iter().find(|&&(_, e)| e > off) {
                return e;
            }
            l += 1;
        }
        self.text.len()
    }

    /// The start of the last word that starts before `off`, crossing lines; `0` when there is none.
    pub fn word_left(&self, off: usize) -> usize {
        let mut l = self.text.line_of(off) as isize;
        while l >= 0 {
            if let Some(&(s, _)) = self.words_on_line(l as usize).iter().rev().find(|&&(s, _)| s < off) {
                return s;
            }
            l -= 1;
        }
        0
    }

    fn grapheme_col(&self, off: usize) -> usize {
        let l = self.text.line_of(off);
        let ls = self.text.line_start(l);
        let content = self.text.line(l);
        unicode::grapheme_boundaries(&content).iter().filter(|&&b| b > 0 && b <= off - ls).count()
    }

    fn at_grapheme_col(&self, l: usize, col: usize) -> usize {
        let ls = self.text.line_start(l);
        let content = self.text.line(l);
        let b = unicode::grapheme_boundaries(&content);
        ls + b.get(col).copied().unwrap_or(content.len())
    }

    fn motion_target(&self, sel: Selection, m: Motion, extend: bool) -> usize {
        let h = sel.head;
        match m {
            Motion::Left if !extend && !sel.is_empty() => sel.start(),
            Motion::Right if !extend && !sel.is_empty() => sel.end(),
            Motion::Left => self.prev_grapheme(h),
            Motion::Right => self.next_grapheme(h),
            Motion::WordLeft => self.word_left(h),
            Motion::WordRight => self.word_right(h),
            Motion::LineStart => self.text.line_start(self.text.line_of(h)),
            Motion::LineEnd => self.text.line_end(self.text.line_of(h)),
            Motion::Up | Motion::Down => {
                let l = self.text.line_of(h);
                let col = self.grapheme_col(h);
                if m == Motion::Up {
                    if l == 0 { 0 } else { self.at_grapheme_col(l - 1, col) }
                } else if l + 1 >= self.text.line_count() {
                    self.text.len()
                } else {
                    self.at_grapheme_col(l + 1, col)
                }
            }
            Motion::DocStart => 0,
            Motion::DocEnd => self.text.len(),
        }
    }

    /// Move every caret by `m`; with `extend` the anchors stay (a shift-motion). Closes the undo group.
    pub fn move_cursors(&mut self, m: Motion, extend: bool) {
        let v: Vec<Selection> = self
            .sels
            .iter()
            .map(|&s| {
                let t = self.motion_target(s, m, extend);
                if extend { Selection::new(s.anchor, t) } else { Selection::cursor(t) }
            })
            .collect();
        self.set_selections(&v);
    }

    // ---- search and view ----------------------------------------------------------------------------------------

    /// Every non-overlapping match of `needle`, left to right.
    pub fn find_all(&self, needle: &str, opts: SearchOpts) -> Vec<Range<usize>> {
        search::find_all(&self.text.to_string(), needle, opts)
    }

    /// The next match at/after `from` (forward) or the last match starting before `from` (backward),
    /// wrapping around the text end when `wrap`.
    pub fn find(&self, needle: &str, opts: SearchOpts, from: usize, dir: Direction, wrap: bool) -> Option<Range<usize>> {
        let all = self.find_all(needle, opts);
        match dir {
            Direction::Forward => {
                all.iter().find(|r| r.start >= from).cloned().or_else(|| if wrap { all.first().cloned() } else { None })
            }
            Direction::Backward => all
                .iter()
                .rev()
                .find(|r| r.start < from)
                .cloned()
                .or_else(|| if wrap { all.last().cloned() } else { None }),
        }
    }

    /// The visible rows for a pane: `rows` visual rows starting at the first row of `first_line`, each
    /// logical line soft-wrapped to `width` as measured by `measure` (the core knows no fonts).
    pub fn viewport(&self, first_line: usize, rows: usize, width: u32, measure: &dyn Fn(&str) -> u32) -> Viewport {
        viewport::build(&self.text, first_line, rows, width, measure)
    }
}

/// The first and last line a selection touches; a non-empty selection ending exactly at a line start does
/// not touch that line.
fn sel_lines(t: &Text, s: Selection) -> (usize, usize) {
    let a = t.line_of(s.start());
    let mut b = t.line_of(s.end());
    if !s.is_empty() && b > a && t.line_start(b) == s.end() {
        b -= 1;
    }
    (a, b)
}
