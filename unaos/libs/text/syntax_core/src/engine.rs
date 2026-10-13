// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The incremental engine: per-line lexer checkpoints, the splice/relex cycle, and the shallow
//! structural pass (folds, bracket matching) over the stored tokens.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use crate::{Highlight, HighlightKind, Language, LexState, LineOut, Source, Token};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    /// The state this line starts in (the checkpoint).
    start: LexState,
    tokens: Vec<Token>,
    heading: u8,
    /// Spliced in and not yet lexed.
    dirty: bool,
}

impl Line {
    fn placeholder(start: LexState) -> Line {
        Line { start, tokens: Vec::new(), heading: 0, dirty: true }
    }
}

/// What a fold groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    /// A bracket pair whose ends are on different lines.
    Bracket,
    /// A multi-line lexer mode: a block comment, a multi-line string, fenced code, a here-document.
    Block,
    /// A Markdown heading's section (to the line before the next heading of the same or a higher level).
    Heading,
}

/// A folding range: lines `start_line..=end_line`, the first staying visible when folded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: FoldKind,
}

/// The incremental highlighter for one document. It holds no grammar: the caller passes the
/// [`Language`] on every call (the document owns it as `Box<dyn Language>`); a change of grammar is
/// noticed by id and lexes the document afresh.
#[derive(Clone, Debug, Default)]
pub struct Highlighter {
    lines: Vec<Line>,
    /// The state the last line ends in.
    end: LexState,
    lang: &'static str,
    scratch: String,
    out: LineOut,
    /// Lines lexed since creation (a KAT reads it to prove the one-line relex).
    lexed: u64,
}

fn opener_of(close: u8) -> u8 {
    match close {
        b')' => b'(',
        b']' => b'[',
        _ => b'{',
    }
}

fn closer_of(open: u8) -> u8 {
    match open {
        b'(' => b')',
        b'[' => b']',
        _ => b'}',
    }
}

impl Highlighter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The id of the grammar the stored tokens came from ("" before the first lex).
    pub fn language(&self) -> &'static str {
        self.lang
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Lines lexed since creation.
    pub fn lex_count(&self) -> u64 {
        self.lexed
    }

    /// Lex the whole document afresh with `lang`.
    pub fn reset(&mut self, lang: &dyn Language, src: &dyn Source) -> Range<usize> {
        self.lang = lang.id();
        let n = src.line_count().max(1);
        self.lines.clear();
        for _ in 0..n {
            self.lines.push(Line::placeholder(LexState::default()));
        }
        self.end = LexState::default();
        self.relex_dirty(lang, src)
    }

    /// Record an edit: lines `first..=old_last` of the old text became lines `first..=new_last` of the
    /// new one (edit_core's `Change::{first_line, old_last_line, new_last_line}` exactly). Several
    /// splices may be recorded, in order, before one [`Self::relex`]. The first new line keeps the
    /// checkpoint of the old line it replaces (the text before it did not change).
    pub fn splice(&mut self, first: usize, old_last: usize, new_last: usize) {
        let n = self.lines.len();
        if n == 0 {
            return;
        }
        let first = first.min(n - 1);
        let old_end = (old_last.max(first) + 1).min(n);
        let new_n = new_last.max(first) + 1 - first;
        let start = self.lines[first].start.clone();
        let mut repl = Vec::with_capacity(new_n);
        for k in 0..new_n {
            repl.push(Line::placeholder(if k == 0 { start.clone() } else { LexState::default() }));
        }
        self.lines.splice(first..old_end, repl);
    }

    /// Re-lex what the recorded splices touched, propagating a changed end state downward until a line
    /// ends in the state the next line already starts in. Returns the lines whose tokens were rewritten
    /// (the range a view repaints). A different grammar, or a line table that does not match
    /// `src.line_count()`, lexes the whole document.
    pub fn relex(&mut self, lang: &dyn Language, src: &dyn Source) -> Range<usize> {
        if lang.id() != self.lang || src.line_count().max(1) != self.lines.len() {
            return self.reset(lang, src);
        }
        self.relex_dirty(lang, src)
    }

    fn relex_dirty(&mut self, lang: &dyn Language, src: &dyn Source) -> Range<usize> {
        let n = self.lines.len();
        let first = match self.lines.iter().position(|l| l.dirty) {
            Some(f) => f,
            None => return 0..0,
        };
        let last_dirty = self.lines.iter().rposition(|l| l.dirty).unwrap_or(first);
        let mut state = if first == 0 { LexState::default() } else { self.lines[first].start.clone() };
        let mut i = first;
        loop {
            self.lines[i].start = state.clone();
            self.lex_one(lang, src, i, &mut state);
            i += 1;
            if i >= n {
                self.end = state;
                break;
            }
            if i > last_dirty && self.lines[i].start == state {
                break;
            }
        }
        first..i
    }

    fn lex_one(&mut self, lang: &dyn Language, src: &dyn Source, i: usize, state: &mut LexState) {
        self.scratch.clear();
        src.read_line(i, &mut self.scratch);
        self.out.clear();
        lang.lex_line(&self.scratch, state, &mut self.out);
        let bytes = self.scratch.as_bytes();
        let len = bytes.len() as u32;
        let line = &mut self.lines[i];
        line.tokens.clear();
        for t in &self.out.tokens {
            let mut t = *t;
            t.end = t.end.min(len);
            if t.start >= t.end {
                continue;
            }
            t.bracket = 0;
            if t.end == t.start + 1 && matches!(t.kind, HighlightKind::Punctuation | HighlightKind::Operator) {
                let b = bytes[t.start as usize];
                if matches!(b, b'(' | b')' | b'[' | b']' | b'{' | b'}') {
                    t.bracket = b;
                }
            }
            line.tokens.push(t);
        }
        line.heading = self.out.heading;
        line.dirty = false;
        self.lexed += 1;
    }

    /// The tokens of `line` (line-relative byte ranges); empty past the end.
    pub fn line_tokens(&self, line: usize) -> &[Token] {
        self.lines.get(line).map(|l| l.tokens.as_slice()).unwrap_or(&[])
    }

    /// The checkpoint `line` starts in.
    pub fn line_state(&self, line: usize) -> Option<&LexState> {
        self.lines.get(line).map(|l| &l.start)
    }

    /// Absolute-offset spans for `lines` (clamped to the document).
    pub fn highlights(&self, src: &dyn Source, lines: Range<usize>) -> Vec<Highlight> {
        let mut v = Vec::new();
        let end = lines.end.min(self.lines.len());
        for li in lines.start.min(end)..end {
            let base = src.line_start(li);
            for t in &self.lines[li].tokens {
                v.push(Highlight { range: base + t.start as usize..base + t.end as usize, kind: t.kind });
            }
        }
        v
    }

    /// The folding ranges, sorted by start line (outer before inner), duplicates removed.
    pub fn folds(&self) -> Vec<Fold> {
        let n = self.lines.len();
        let mut out = Vec::new();
        // Bracket pairs across lines.
        let mut stack: Vec<(u8, usize)> = Vec::new();
        for (li, l) in self.lines.iter().enumerate() {
            for t in &l.tokens {
                match t.bracket {
                    b'(' | b'[' | b'{' => stack.push((t.bracket, li)),
                    b')' | b']' | b'}' => {
                        let open = opener_of(t.bracket);
                        if let Some(pos) = stack.iter().rposition(|&(b, _)| b == open) {
                            let sl = stack[pos].1;
                            stack.truncate(pos);
                            if li > sl {
                                out.push(Fold { start_line: sl, end_line: li, kind: FoldKind::Bracket });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Multi-line lexer modes: a line that starts in the ground state and ends outside it opens a
        // block; the block closes on the first later line that ends back in the ground state.
        let ends_open = |j: usize| -> bool {
            if j + 1 < n { self.lines[j + 1].start.mode != 0 } else { self.end.mode != 0 }
        };
        let mut i = 0;
        while i < n {
            if self.lines[i].start.mode == 0 && ends_open(i) {
                let mut j = i + 1;
                while j < n && ends_open(j) {
                    j += 1;
                }
                let end = j.min(n - 1);
                if end > i {
                    out.push(Fold { start_line: i, end_line: end, kind: FoldKind::Block });
                }
                i = end + 1;
            } else {
                i += 1;
            }
        }
        // Heading sections.
        let heads: Vec<(usize, u8)> =
            self.lines.iter().enumerate().filter(|(_, l)| l.heading > 0).map(|(i, l)| (i, l.heading)).collect();
        for (k, &(li, lv)) in heads.iter().enumerate() {
            let end = heads[k + 1..].iter().find(|&&(_, l2)| l2 <= lv).map(|&(i2, _)| i2 - 1).unwrap_or(n - 1);
            if end > li {
                out.push(Fold { start_line: li, end_line: end, kind: FoldKind::Heading });
            }
        }
        out.sort_by(|a, b| a.start_line.cmp(&b.start_line).then(b.end_line.cmp(&a.end_line)));
        out.dedup_by(|a, b| a.start_line == b.start_line && a.end_line == b.end_line);
        out
    }

    /// The bracket matching the one whose token starts at (`line`, `byte`): its (line, byte), or `None`
    /// when no bracket token starts there or it is unmatched. Brackets inside strings and comments are
    /// not tokens of their own, so they never match.
    pub fn matching_bracket(&self, line: usize, byte: usize) -> Option<(usize, usize)> {
        let l = self.lines.get(line)?;
        let t = l.tokens.iter().find(|t| t.start as usize == byte && t.bracket != 0)?;
        let b = t.bracket;
        let forward = matches!(b, b'(' | b'[' | b'{');
        let (open, close) = if forward { (b, closer_of(b)) } else { (opener_of(b), b) };
        let mut depth = 0usize;
        if forward {
            for li in line..self.lines.len() {
                for tk in &self.lines[li].tokens {
                    if li == line && tk.start as usize <= byte {
                        continue;
                    }
                    if tk.bracket == open {
                        depth += 1;
                    } else if tk.bracket == close {
                        if depth == 0 {
                            return Some((li, tk.start as usize));
                        }
                        depth -= 1;
                    }
                }
            }
        } else {
            for li in (0..=line).rev() {
                for tk in self.lines[li].tokens.iter().rev() {
                    if li == line && tk.start as usize >= byte {
                        continue;
                    }
                    if tk.bracket == close {
                        depth += 1;
                    } else if tk.bracket == open {
                        if depth == 0 {
                            return Some((li, tk.start as usize));
                        }
                        depth -= 1;
                    }
                }
            }
        }
        None
    }
}
