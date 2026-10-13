// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Vein — the Lumen window (ring 3); the grammars are Tabula's shared core (syntax_core, SR89).
//
// LUMEN2 (LEDGER SR121) — a reply's fenced code blocks highlighted through syntax_core, UnaOS's own syntax
// engine (R105: tree-sitter is OUT): the fence's info word picks the grammar (`rust`/`rs`, `c`/`cpp`/`h`,
// `toml`, `markdown`/`md`, `sh`/`bash`/`shell`), each code line is lexed from the LexState the line before it
// ended in, and every token's kind maps to one of the theme's inks (theme.rs is read, never extended: the kinds
// share the window's existing roles). The window draws rows top to bottom, so ONE cache walks a block forward:
// the block's opener, the state at the next unlexed line, and the last line's tokens with the state it started
// in (a line still streaming re-lexes from that state when it grows). Any change to the transcript's offsets
// (a dropped turn, a new or reloaded conversation) resets it.

use alloc::boxed::Box;
use alloc::vec::Vec;
use syntax_core::{HighlightKind, Language, LexState, LineOut, Token};

/// The grammar for a fence's info word (ASCII case-insensitive), or `None` (the block is drawn in code ink).
pub fn language(info: &[u8]) -> Option<Box<dyn Language>> {
    let mut low = [0u8; 16];
    if info.len() > low.len() {
        return None;
    }
    for (d, s) in low.iter_mut().zip(info) {
        *d = s.to_ascii_lowercase();
    }
    let id = match &low[..info.len()] {
        b"rust" | b"rs" => "rust",
        b"c" | b"h" | b"cpp" | b"c++" | b"cc" | b"hpp" | b"cxx" => "c",
        b"toml" => "toml",
        b"markdown" | b"md" => "markdown",
        b"sh" | b"bash" | b"shell" | b"zsh" | b"console" => "sh",
        _ => return None,
    };
    syntax_core::language_for_id(id)
}

struct Cache {
    /// Log offset of the block's opening fence line.
    open: usize,
    lang: Option<Box<dyn Language>>,
    /// The next line to lex and the state it starts in.
    next: usize,
    state: LexState,
    /// The last lexed line: where it starts, its length, the state it started in, its tokens.
    line: usize,
    line_len: usize,
    line_state: LexState,
    toks: Vec<Token>,
}

static mut CACHE: Option<Cache> = None;

fn cache() -> &'static mut Option<Cache> {
    unsafe { &mut *core::ptr::addr_of_mut!(CACHE) }
}

/// Forget everything (the transcript's offsets moved).
pub fn reset() {
    *cache() = None;
}

fn lex(lang: &dyn Language, text: &[u8], state: &mut LexState, out: &mut LineOut) {
    out.clear();
    if let Ok(t) = core::str::from_utf8(text) {
        lang.lex_line(t, state, out);
    }
}

/// The tokens of the code line at `log[p..line_end(p)]`, inside the block opened by the fence line at `open`
/// (whose own line ends at `line_end(open)`). Token offsets are relative to `p`. Empty when the fence names no
/// grammar this core has.
pub fn tokens(log: &[u8], open: usize, p: usize, line_end: &dyn Fn(usize) -> usize) -> &'static [Token] {
    let c = cache();
    let fresh = match c {
        Some(k) => k.open != open || (k.line != usize::MAX && p < k.line),
        None => true,
    };
    if fresh {
        let oe = line_end(open);
        let info = md::fence_info(&log[open..oe]).unwrap_or(b"");
        let next = if oe < log.len() && log[oe] == b'\n' { oe + 1 } else { oe };
        *c = Some(Cache { open, lang: language(info), next, state: LexState::default(), line: usize::MAX, line_len: 0, line_state: LexState::default(), toks: Vec::new() });
    }
    let Some(k) = c.as_mut() else { return &[] };
    let Some(lang) = k.lang.as_deref() else { return &[] };
    let mut out = LineOut::default();
    let term = |e: usize| e < log.len() && log[e] == b'\n';
    // The last lexed line again: as it was, or re-lexed from its own start state if it grew (still streaming).
    // A later line asked for while the last one had no terminator: finish that line first (it may have grown).
    if k.line != usize::MAX && (p == k.line || (p > k.line && k.next <= k.line + k.line_len)) {
        let q = k.line;
        let e = line_end(q);
        if e - q != k.line_len || k.next <= q + k.line_len {
            let mut st = k.line_state.clone();
            lex(lang, &log[q..e], &mut st, &mut out);
            k.toks = core::mem::take(&mut out.tokens);
            k.line_len = e - q;
            k.state = st;
            k.next = if term(e) { e + 1 } else { e };
        }
        if p == q {
            return keep(&k.toks);
        }
    }
    // Walk forward to `p` (rows are drawn top to bottom, so this is usually one line).
    let mut guard = 0;
    while k.next <= p && guard < 4096 {
        guard += 1;
        let q = k.next;
        let e = line_end(q);
        let before = k.state.clone();
        lex(lang, &log[q..e], &mut k.state, &mut out);
        let ended = term(e);
        k.next = if ended { e + 1 } else { e };
        if q == p {
            k.line = p;
            k.line_len = e - p;
            k.line_state = before;
            k.toks = core::mem::take(&mut out.tokens);
            break;
        }
        if !ended {
            break;
        }
    }
    if k.line == p { keep(&k.toks) } else { &[] }
}

/// The cache is a static the single-threaded window owns; the slice lives until the next call (the caller draws
/// one row with it and asks again for the next).
fn keep(v: &Vec<Token>) -> &'static [Token] {
    unsafe { &*(v.as_slice() as *const [Token]) }
}

use vein_core::md;

/// The ink a token kind is drawn in: the theme's roles (theme.rs is read, not extended).
pub fn ink(kind: HighlightKind) -> u32 {
    let th = crate::theme::cur();
    match kind {
        HighlightKind::Keyword | HighlightKind::Builtin | HighlightKind::Heading => th.head_ink,
        HighlightKind::Type | HighlightKind::Link => th.link_ink,
        HighlightKind::Function | HighlightKind::Macro => th.user_ink,
        HighlightKind::String | HighlightKind::Char | HighlightKind::Escape => th.ok_ink,
        HighlightKind::Number | HighlightKind::Lifetime | HighlightKind::Attribute | HighlightKind::Emphasis => th.quote_ink,
        HighlightKind::Comment | HighlightKind::DocComment => th.note_ink,
        HighlightKind::Error => th.caret,
        _ => th.code_ink,
    }
}

/// The ink of byte `i` of a line with `toks` (code ink between tokens).
pub fn ink_at(toks: &[Token], i: usize) -> u32 {
    for t in toks {
        if (t.start as usize) <= i && i < t.end as usize {
            return ink(t.kind);
        }
        if t.start as usize > i {
            break;
        }
    }
    crate::theme::cur().code_ink
}
