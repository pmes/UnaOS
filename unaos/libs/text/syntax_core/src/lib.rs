// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! SYNTAXCORE (LEDGER SR89) — UnaOS's own syntax core.
//!
//! CHARTER: Tabula — shared-core (Tabula owns Text, docs/CODEX.md §2; R105: tree-sitter is OUT and this
//! core is Tabula's syntax engine; R104: the GtkSourceView face stays beside it as the other-OS port).
//!
//! `no_std` + `alloc`, zero dependencies, no `unsafe`. What is here:
//!
//! * [`Language`] — a grammar: a per-line lexer that resumes from a [`LexState`] checkpoint and emits
//!   [`Token`]s of a fixed [`HighlightKind`]. Five grammars, each written from its specification
//!   ([`lang`]): Rust (the Reference's lexical structure), C (C17 §6.4), TOML v1.0.0, CommonMark 0.31
//!   (block structure, then inlines), POSIX shell with the bash extensions.
//! * [`Highlighter`] — the incremental engine. It stores, for every line, the lexer state the line STARTS
//!   in plus the line's tokens. An edit splices the line table ([`Highlighter::splice`], fed straight from
//!   an editor's changed-line range) and [`Highlighter::relex`] re-lexes from the first touched line,
//!   stopping as soon as a line ends in the state the next line already starts in — so a one-character edit
//!   re-lexes one line in the common case, and opening a block comment re-lexes exactly as far as it reaches.
//! * The shallow structural pass over those tokens: folding ranges ([`Highlighter::folds`]: multi-line
//!   bracket pairs, multi-line lexer modes such as block comments, fenced code and here-docs, and Markdown
//!   heading sections) and bracket matching ([`Highlighter::matching_bracket`]).
//! * [`Theme`] — kind → appearance-token NAME, in the Principia preference shape (`tabula.theme.<kind>`).
//!   No colour lives here: colours belong to the painter's theme (kernel `video/theme.rs`) or a host face.
//!
//! Text is read through [`Source`] (line count, line start, one line's content): the host adapts
//! edit_core's piece table to it, so the core never asks for the document as one `String`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod engine;
pub mod lang;
mod scan;
mod theme;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

pub use engine::{Fold, FoldKind, Highlighter};
pub use theme::{Theme, APPEARANCE_TOKENS, THEME_NS, THEME_PREFIX};

/// The fixed set of highlight kinds every grammar maps onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum HighlightKind {
    Keyword,
    Identifier,
    Type,
    Function,
    String,
    Char,
    Number,
    Comment,
    DocComment,
    Attribute,
    Macro,
    Punctuation,
    Operator,
    Lifetime,
    Escape,
    Heading,
    Emphasis,
    Link,
    Code,
    Variable,
    Builtin,
    Error,
}

impl HighlightKind {
    pub const COUNT: usize = 22;
    pub const ALL: [HighlightKind; HighlightKind::COUNT] = [
        HighlightKind::Keyword,
        HighlightKind::Identifier,
        HighlightKind::Type,
        HighlightKind::Function,
        HighlightKind::String,
        HighlightKind::Char,
        HighlightKind::Number,
        HighlightKind::Comment,
        HighlightKind::DocComment,
        HighlightKind::Attribute,
        HighlightKind::Macro,
        HighlightKind::Punctuation,
        HighlightKind::Operator,
        HighlightKind::Lifetime,
        HighlightKind::Escape,
        HighlightKind::Heading,
        HighlightKind::Emphasis,
        HighlightKind::Link,
        HighlightKind::Code,
        HighlightKind::Variable,
        HighlightKind::Builtin,
        HighlightKind::Error,
    ];

    /// The kind's stable name (the KAT files, the theme preference keys, a view's tag names).
    pub const fn name(self) -> &'static str {
        match self {
            HighlightKind::Keyword => "keyword",
            HighlightKind::Identifier => "identifier",
            HighlightKind::Type => "type",
            HighlightKind::Function => "function",
            HighlightKind::String => "string",
            HighlightKind::Char => "char",
            HighlightKind::Number => "number",
            HighlightKind::Comment => "comment",
            HighlightKind::DocComment => "doc-comment",
            HighlightKind::Attribute => "attribute",
            HighlightKind::Macro => "macro",
            HighlightKind::Punctuation => "punctuation",
            HighlightKind::Operator => "operator",
            HighlightKind::Lifetime => "lifetime",
            HighlightKind::Escape => "escape",
            HighlightKind::Heading => "heading",
            HighlightKind::Emphasis => "emphasis",
            HighlightKind::Link => "link",
            HighlightKind::Code => "code",
            HighlightKind::Variable => "variable",
            HighlightKind::Builtin => "builtin",
            HighlightKind::Error => "error",
        }
    }

    pub fn from_name(s: &str) -> Option<HighlightKind> {
        HighlightKind::ALL.iter().copied().find(|k| k.name() == s)
    }

    pub const fn index(self) -> usize {
        self as usize
    }
}

/// One lexeme of one line: line-relative byte range, its kind, and — filled by the engine for one-byte
/// punctuation/operator tokens — the bracket byte it is (`(`, `)`, `[`, `]`, `{`, `}`), else 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub start: u32,
    pub end: u32,
    pub kind: HighlightKind,
    pub bracket: u8,
}

/// A highlighted span of the document: an absolute byte range and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Highlight {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

/// The lexer state a line starts (or ends) in — the checkpoint the incremental engine stores per line.
/// Each grammar gives the fields its own meaning; `mode == 0` is the grammar's ground state (no
/// multi-line construct open), which the fold pass relies on. Two states compare equal exactly when a
/// line lexed from either produces the same tokens, which is what lets [`Highlighter::relex`] stop early.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct LexState {
    /// The open multi-line construct (grammar-defined; 0 = none).
    pub mode: u8,
    /// A count the mode needs (comment nesting depth, raw-string hashes, fence length).
    pub n: u32,
    /// Flags the grammar carries across lines without opening a construct (e.g. "inside a paragraph").
    pub flags: u32,
    /// A stack of open delimiters (TOML arrays, shell quoting/substitution contexts).
    pub stack: Vec<u8>,
    /// Text the mode needs (pending here-document delimiters).
    pub tag: String,
}

/// What a grammar writes for one line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineOut {
    pub tokens: Vec<Token>,
    /// Markdown: the heading level (1..=6) this line opens, else 0 — the heading-section fold.
    pub heading: u8,
}

impl LineOut {
    /// Append a token for `start..end` (empty ranges are dropped; an adjacent token of the same kind
    /// is extended instead of split).
    pub fn push(&mut self, kind: HighlightKind, start: usize, end: usize) {
        if end <= start {
            return;
        }
        if let Some(last) = self.tokens.last_mut() {
            if last.kind == kind
                && last.end as usize == start
                && !matches!(kind, HighlightKind::Punctuation | HighlightKind::Operator)
            {
                last.end = end as u32;
                return;
            }
        }
        self.tokens.push(Token { start: start as u32, end: end as u32, kind, bracket: 0 });
    }

    pub fn clear(&mut self) {
        self.tokens.clear();
        self.heading = 0;
    }
}

/// A grammar. Object-safe and `Send + Sync`, so a document can hold `Box<dyn Language>` across threads.
pub trait Language: Send + Sync {
    /// The language id this grammar serves (the sourceview-style id the host uses: "rust", "c", ...).
    fn id(&self) -> &'static str;
    /// Lex one line's content (no terminator) starting in `state`; leave in `state` the state the next
    /// line starts in. Tokens go to `out` in order, non-overlapping; whitespace is not emitted.
    fn lex_line(&self, line: &str, state: &mut LexState, out: &mut LineOut);
}

/// The document text as the core reads it: lines, never the whole text at once.
pub trait Source {
    /// Number of lines (one more than the number of line feeds).
    fn line_count(&self) -> usize;
    /// Absolute byte offset where `line` starts.
    fn line_start(&self, line: usize) -> usize;
    /// Append `line`'s content, WITHOUT its terminator (LF or CRLF), to `out`.
    fn read_line(&self, line: usize, out: &mut String);
}

/// A [`Source`] over a borrowed `&str` (tests, a ring-3 program holding a plain buffer). Lines end at LF;
/// a CR before the LF belongs to the terminator, as in edit_core.
pub struct StrSource<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> StrSource<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = Vec::new();
        starts.push(0);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        StrSource { text, starts }
    }
}

impl Source for StrSource<'_> {
    fn line_count(&self) -> usize {
        self.starts.len()
    }
    fn line_start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.text.len())
    }
    fn read_line(&self, line: usize, out: &mut String) {
        if line >= self.starts.len() {
            return;
        }
        let a = self.starts[line];
        let mut b = self.starts.get(line + 1).copied().unwrap_or(self.text.len());
        if b > a && self.text.as_bytes()[b - 1] == b'\n' {
            b -= 1;
            if b > a && self.text.as_bytes()[b - 1] == b'\r' {
                b -= 1;
            }
        }
        out.push_str(&self.text[a..b]);
    }
}

/// The grammar for a language id (the ids `TabulaDocument::language_for` answers), or `None` for a
/// language this core has no grammar for (the host then shows the text plain, or the sourceview face).
pub fn language_for_id(id: &str) -> Option<Box<dyn Language>> {
    match id {
        "rust" => Some(Box::new(lang::rust::Rust)),
        "c" | "cpp" | "chdr" => Some(Box::new(lang::c::C)),
        "toml" => Some(Box::new(lang::toml::Toml)),
        "markdown" | "md" => Some(Box::new(lang::markdown::Markdown)),
        "sh" | "bash" | "shell" => Some(Box::new(lang::shell::Shell)),
        _ => None,
    }
}

/// Lex a whole text in one pass (no engine, no checkpoints): every line's tokens, in order.
/// The KATs and a one-shot caller use it; an editor uses [`Highlighter`].
pub fn lex_all(lang: &dyn Language, text: &str) -> Vec<Vec<Token>> {
    let src = StrSource::new(text);
    let mut h = Highlighter::new();
    h.reset(lang, &src);
    (0..src.line_count()).map(|l| h.line_tokens(l).to_vec()).collect()
}
