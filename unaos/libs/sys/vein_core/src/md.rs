// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! LUMENUX M2 (rmbp-ledger B348) — the markdown a reply is drawn with, ONE SOURCE LINE AT A TIME, into the
//! caller's buffers (no allocator: LUMEN.ELF has none). A reply is rendered while it streams, so the only
//! block state is [`State`] — whether a fenced code block is open — carried from line to line by the caller;
//! a fence still arriving stays code-tinted because its opening line already set the state.
//!
//! The SHAPE is QUARRY2's `video/richtext.rs` (display bytes + styled byte ranges, the same [`Tint`] names),
//! so the kernel viewer can be expressed over [`line`] once Tabula's core exists; that file is alloc-only
//! and whole-document, which is why it is not this one (LUMENUX.md §The AST question).
//!
//! The subset: ATX headings (`#`..`######`), `**bold**` / `__bold__`, `*italic*` / `_italic_` (an `_`
//! inside a word is a letter: `snake_case` stays), `` `code` ``, `[text](url)` shown as `text (url)`,
//! bullet (`-` `*` `+`) and numbered (`1.` `1)`) list items with a hanging indent, `>` quotes, thematic
//! breaks, and ``` / ~~~ fences. An unclosed mark is shown as written (a `**` still streaming is two stars
//! until its partner arrives). [`wrap`] breaks a rendered line into rows.

/// What a styled range IS; the window picks the colour (richtext's names, plus `Link`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    Plain,
    Heading,
    Dim,
    Code,
    Quote,
    Link,
}

/// One styled range of the rendered line's display bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u16,
    pub end: u16,
    pub tint: Tint,
    pub bold: bool,
    pub italic: bool,
}

impl Span {
    pub const EMPTY: Span = Span { start: 0, end: 0, tint: Tint::Plain, bold: false, italic: false };
}

/// What kind of line it is (the window draws a rule, a code ground, a heading size from this).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Block {
    Para = 0,
    Heading = 1,
    Bullet = 2,
    Number = 3,
    Quote = 4,
    /// A line inside an open fence.
    Code = 5,
    /// The ``` / ~~~ line itself.
    Fence = 6,
    Rule = 7,
    Blank = 8,
}

impl Block {
    pub fn from_u8(b: u8) -> Block {
        match b {
            1 => Block::Heading,
            2 => Block::Bullet,
            3 => Block::Number,
            4 => Block::Quote,
            5 => Block::Code,
            6 => Block::Fence,
            7 => Block::Rule,
            8 => Block::Blank,
            _ => Block::Para,
        }
    }
}

/// The block state carried between lines: is a fence open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub fence: bool,
}

/// One rendered line: `text[..len]` and `spans[..spans]` were written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub block: Block,
    /// Heading level 1..=6 (0 otherwise).
    pub level: u8,
    pub len: usize,
    pub spans: usize,
    /// Columns a wrapped continuation row is indented by (list items and quotes).
    pub hang: usize,
    /// The caller's buffers were too small; what fit was written.
    pub over: bool,
}

struct W<'a> {
    text: &'a mut [u8],
    n: usize,
    spans: &'a mut [Span],
    ns: usize,
    over: bool,
}

impl W<'_> {
    fn push(&mut self, s: &[u8], tint: Tint, bold: bool, italic: bool) {
        let room = self.text.len().min(u16::MAX as usize) - self.n;
        let k = s.len().min(room);
        if k < s.len() {
            self.over = true;
        }
        if k == 0 {
            return;
        }
        let a = self.n;
        self.text[a..a + k].copy_from_slice(&s[..k]);
        self.n += k;
        if tint == Tint::Plain && !bold && !italic {
            return;
        }
        if self.ns > 0 {
            let last = &mut self.spans[self.ns - 1];
            if last.end as usize == a && last.tint == tint && last.bold == bold && last.italic == italic {
                last.end = self.n as u16;
                return;
            }
        }
        if self.ns == self.spans.len() {
            self.over = true;
            return;
        }
        self.spans[self.ns] = Span { start: a as u16, end: self.n as u16, tint, bold, italic };
        self.ns += 1;
    }
}

fn lead_ws(s: &[u8]) -> usize {
    s.iter().take_while(|&&c| c == b' ').count()
}

fn is_fence(s: &[u8]) -> bool {
    let i = lead_ws(s);
    i <= 3 && (s[i..].starts_with(b"```") || s[i..].starts_with(b"~~~"))
}

fn is_rule(s: &[u8]) -> bool {
    let t: &[u8] = trim(s);
    let Some(&c) = t.first() else { return false };
    if !matches!(c, b'-' | b'*' | b'_') {
        return false;
    }
    let mut k = 0;
    for &x in t {
        if x == c {
            k += 1;
        } else if x != b' ' {
            return false;
        }
    }
    k >= 3
}

fn trim(s: &[u8]) -> &[u8] {
    let a = s.iter().position(|&c| c != b' ').unwrap_or(s.len());
    let b = s.iter().rposition(|&c| c != b' ').map_or(a, |p| p + 1);
    &s[a..b]
}

/// Render one source line (no `\n`). `st` is the state BEFORE the line and is advanced past it.
pub fn line(st: &mut State, src: &[u8], text: &mut [u8], spans: &mut [Span]) -> Line {
    let mut w = W { text, n: 0, spans, ns: 0, over: false };
    let mut out = Line { block: Block::Para, level: 0, len: 0, spans: 0, hang: 0, over: false };
    let src = src.strip_suffix(b"\r").unwrap_or(src);
    if is_fence(src) {
        st.fence = !st.fence;
        out.block = Block::Fence;
        w.push(src, Tint::Dim, false, false);
    } else if st.fence {
        out.block = Block::Code;
        w.push(src, Tint::Code, false, false);
    } else if trim(src).is_empty() {
        out.block = Block::Blank;
    } else if is_rule(src) {
        out.block = Block::Rule;
    } else {
        let ind = lead_ws(src);
        let s = &src[ind..];
        let hashes = s.iter().take_while(|&&c| c == b'#').count();
        if ind <= 3 && (1..=6).contains(&hashes) && (s.len() == hashes || s[hashes] == b' ') {
            out.block = Block::Heading;
            out.level = hashes as u8;
            let body = trim(&s[hashes..]);
            let body = body.strip_suffix(b"#").map_or(body, |b| trim(b.strip_suffix(b"#").unwrap_or(b)));
            inline(&mut w, body, Tint::Heading, true, false, 0);
        } else if let Some(rest) = s.strip_prefix(b">") {
            out.block = Block::Quote;
            out.hang = 2;
            w.push(b"| ", Tint::Dim, false, false);
            inline(&mut w, rest.strip_prefix(b" ").unwrap_or(rest), Tint::Quote, false, false, 0);
        } else if s.len() >= 2 && matches!(s[0], b'-' | b'*' | b'+') && s[1] == b' ' {
            out.block = Block::Bullet;
            let pad = (ind / 2) * 2;
            spaces(&mut w, pad);
            w.push(b"- ", Tint::Dim, false, false);
            out.hang = pad + 2;
            inline(&mut w, &s[2..], Tint::Plain, false, false, 0);
        } else if let Some(m) = number_marker(s) {
            out.block = Block::Number;
            let pad = (ind / 2) * 2;
            spaces(&mut w, pad);
            w.push(&s[..m], Tint::Dim, false, false);
            out.hang = pad + m;
            inline(&mut w, &s[m..], Tint::Plain, false, false, 0);
        } else {
            inline(&mut w, s, Tint::Plain, false, false, 0);
        }
    }
    out.len = w.n;
    out.spans = w.ns;
    out.over = w.over;
    out
}

fn spaces(w: &mut W<'_>, n: usize) {
    for _ in 0..n {
        w.push(b" ", Tint::Plain, false, false);
    }
}

/// `1. ` / `12) ` → the marker's length including the space.
fn number_marker(s: &[u8]) -> Option<usize> {
    let d = s.iter().take_while(|c| c.is_ascii_digit()).count();
    if d == 0 || d > 9 || s.len() < d + 2 || !matches!(s[d], b'.' | b')') || s[d + 1] != b' ' {
        return None;
    }
    Some(d + 2)
}

fn word(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// Find the closing `mark` for an emphasis opened at `from` (the first byte after the opener).
fn close_emph(s: &[u8], from: usize, mark: &[u8]) -> Option<usize> {
    let single = mark.len() == 1;
    let mut j = from;
    while j + mark.len() <= s.len() {
        if s[j] == b'`' {
            // a code span inside emphasis is opaque
            match s[j + 1..].iter().position(|&c| c == b'`') {
                Some(p) => {
                    j += p + 2;
                    continue;
                }
                None => return None,
            }
        }
        if s[j..].starts_with(mark) && j > from && s[j - 1] != b' ' {
            if !single {
                // `***x***`: the closer is the LAST two of the run, so the inner `*x*` keeps its own pair
                while s.get(j + 2) == Some(&mark[0]) {
                    j += 1;
                }
            }
            let after = s.get(j + mark.len()).copied();
            let doubled = single && after == Some(mark[0]);
            let intraword = mark[0] == b'_' && after.is_some_and(word);
            if !doubled && !intraword {
                return Some(j);
            }
            if doubled {
                j += 2;
                continue;
            }
        }
        j += 1;
    }
    None
}

/// The inline marks of one line's body, in `tint`/`bold`/`italic`.
fn inline(w: &mut W<'_>, s: &[u8], tint: Tint, bold: bool, italic: bool, depth: u8) {
    let mut i = 0usize;
    let mut run = 0usize;
    while i < s.len() {
        let c = s[i];
        let prev = if i > 0 { s[i - 1] } else { b' ' };
        if c == b'\\' && i + 1 < s.len() && s[i + 1].is_ascii_punctuation() {
            w.push(&s[run..i], tint, bold, italic);
            run = i + 1;
            i += 2;
            continue;
        }
        if c == b'`' {
            if let Some(p) = s[i + 1..].iter().position(|&x| x == b'`') {
                w.push(&s[run..i], tint, bold, italic);
                w.push(&s[i + 1..i + 1 + p], Tint::Code, bold, italic);
                i += p + 2;
                run = i;
                continue;
            }
        } else if depth < 3 && (c == b'*' || c == b'_') {
            let dbl = s.get(i + 1) == Some(&c);
            let mark: &[u8] = if dbl { &s[i..i + 2] } else { &s[i..i + 1] };
            let open_ok = s.get(i + mark.len()).is_some_and(|&n| n != b' ') && !(c == b'_' && word(prev));
            if open_ok {
                if let Some(j) = close_emph(s, i + mark.len(), mark) {
                    w.push(&s[run..i], tint, bold, italic);
                    let (b, it) = if dbl { (true, italic) } else { (bold, true) };
                    inline(w, &s[i + mark.len()..j], tint, b, it, depth + 1);
                    i = j + mark.len();
                    run = i;
                    continue;
                }
            }
        } else if c == b'[' && depth < 3 {
            if let Some(m) = s[i + 1..].windows(2).position(|x| x == b"](") {
                let te = i + 1 + m;
                if let Some(q) = s[te + 2..].iter().position(|&x| x == b')') {
                    let url = &s[te + 2..te + 2 + q];
                    if !url.contains(&b' ') {
                        w.push(&s[run..i], tint, bold, italic);
                        inline(w, &s[i + 1..te], Tint::Link, bold, italic, depth + 1);
                        if !url.is_empty() && url != &s[i + 1..te] {
                            w.push(b" (", Tint::Dim, false, false);
                            w.push(url, Tint::Dim, false, false);
                            w.push(b")", Tint::Dim, false, false);
                        }
                        i = te + 3 + q;
                        run = i;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    w.push(&s[run..], tint, bold, italic);
}

/// Break a rendered line of `len` display bytes into rows of at most `width` columns, continuation rows
/// indented by `hang` (clamped so a row is never narrower than 4). Breaks at the last space that fits,
/// else hard. `emit(start, end, first_row)`; an empty line emits one empty row.
pub fn wrap(text: &[u8], width: usize, hang: usize, emit: &mut dyn FnMut(usize, usize, bool)) {
    let width = width.max(4);
    let hang = hang.min(width.saturating_sub(4));
    let mut s = 0usize;
    let mut first = true;
    loop {
        let wd = if first { width } else { width - hang };
        // continuation rows drop the space they broke at
        if !first {
            while s < text.len() && text[s] == b' ' {
                s += 1;
            }
        }
        let rem = text.len() - s;
        if rem <= wd {
            if first || rem > 0 {
                emit(s, text.len(), first);
            }
            return;
        }
        let mut b = s + wd;
        while b > s && text[b] != b' ' {
            b -= 1;
        }
        let e = if b == s { s + wd } else { b };
        emit(s, e, first);
        s = e;
        first = false;
    }
}

/// KERNELFONT2 (rmbp-ledger B363): [`wrap`] for a PROPORTIONAL face — rows of at most `width` px, continuation rows
/// indented by `hang` px (clamped so a row keeps at least a quarter of `width`), `adv(i)` the advance of display
/// byte `i` in the style it is drawn in. Same rules as [`wrap`]: break at the last space that fits, else hard (at
/// least one byte per row); continuation rows drop the spaces they broke at; an empty line emits one empty row.
pub fn wrap_px(text: &[u8], width: f32, hang: f32, adv: &dyn Fn(usize) -> f32, emit: &mut dyn FnMut(usize, usize, bool)) {
    let width = if width < 16.0 { 16.0 } else { width };
    let hang = if hang > width * 0.75 { width * 0.75 } else if hang < 0.0 { 0.0 } else { hang };
    let mut s = 0usize;
    let mut first = true;
    loop {
        let wd = if first { width } else { width - hang };
        if !first {
            while s < text.len() && text[s] == b' ' {
                s += 1;
            }
        }
        // the first byte that does not fit
        let mut e = s;
        let mut x = 0f32;
        while e < text.len() {
            let a = adv(e);
            if x + a > wd + 0.01 {
                break;
            }
            x += a;
            e += 1;
        }
        if e >= text.len() {
            if first || s < text.len() {
                emit(s, text.len(), first);
            }
            return;
        }
        let mut b = e;
        while b > s && text[b] != b' ' {
            b -= 1;
        }
        let cut = if b > s { b } else { e.max(s + 1) };
        emit(s, cut, first);
        s = cut;
        first = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    fn r(st: &mut State, src: &str) -> (Line, String, Vec<Span>) {
        let mut t = [0u8; 512];
        let mut sp = [Span::EMPTY; 32];
        let l = line(st, src.as_bytes(), &mut t, &mut sp);
        (l, String::from_utf8(t[..l.len].to_vec()).unwrap(), sp[..l.spans].to_vec())
    }
    fn styled(text: &str, spans: &[Span]) -> Vec<(String, Tint, bool, bool)> {
        spans.iter().map(|s| (String::from(&text[s.start as usize..s.end as usize]), s.tint, s.bold, s.italic)).collect()
    }

    #[test]
    fn headings_lists_quotes_rules() {
        let mut st = State::default();
        let (l, t, sp) = r(&mut st, "## Two **words** ##");
        assert_eq!((l.block, l.level, t.as_str()), (Block::Heading, 2, "Two words"));
        assert!(sp.iter().all(|s| s.bold && s.tint == Tint::Heading));
        let (l, t, _) = r(&mut st, "  - item *one*");
        assert_eq!((l.block, t.as_str(), l.hang), (Block::Bullet, "  - item one", 4));
        let (l, t, _) = r(&mut st, "12. twelfth");
        assert_eq!((l.block, t.as_str(), l.hang), (Block::Number, "12. twelfth", 4));
        let (l, t, _) = r(&mut st, "> quoted");
        assert_eq!((l.block, t.as_str()), (Block::Quote, "| quoted"));
        assert_eq!(r(&mut st, "---").0.block, Block::Rule);
        assert_eq!(r(&mut st, "   ").0.block, Block::Blank);
        assert_eq!(r(&mut st, "#hashtag").0.block, Block::Para);
        assert_eq!(r(&mut st, "-not a list").0.block, Block::Para);
    }

    #[test]
    fn inline_marks() {
        let mut st = State::default();
        let (_, t, sp) = r(&mut st, "a **b** _c_ `d*e*` snake_case_name 2 * 3 * 4");
        assert_eq!(t, "a b c d*e* snake_case_name 2 * 3 * 4");
        assert_eq!(
            styled(&t, &sp),
            std::vec![("b".into(), Tint::Plain, true, false), ("c".into(), Tint::Plain, false, true), ("d*e*".into(), Tint::Code, false, false)]
        );
        let (_, t, sp) = r(&mut st, "see [the docs](https://x.dev/a) now");
        assert_eq!(t, "see the docs (https://x.dev/a) now");
        assert_eq!(sp[0].tint, Tint::Link);
        let (_, t, _) = r(&mut st, "***both*** and \\*literal\\*");
        assert_eq!(t, "both and *literal*");
        // unclosed marks are shown as written (a stream mid-mark)
        let (_, t, sp) = r(&mut st, "an **open mark and `open code");
        assert_eq!((t.as_str(), sp.len()), ("an **open mark and `open code", 0));
    }

    #[test]
    fn fences_carry_state_and_a_partial_fence_stays_code() {
        let mut st = State::default();
        assert_eq!(r(&mut st, "```rust").0.block, Block::Fence);
        assert!(st.fence);
        let (l, t, sp) = r(&mut st, "let **x** = 1; // # not a heading");
        assert_eq!((l.block, t.as_str()), (Block::Code, "let **x** = 1; // # not a heading"));
        assert_eq!(sp.len(), 1);
        assert_eq!(sp[0].tint, Tint::Code);
        // the stream has not closed it yet: the next line is still code
        assert_eq!(r(&mut st, "- not a list in code").0.block, Block::Code);
        assert_eq!(r(&mut st, "```").0.block, Block::Fence);
        assert!(!st.fence);
        assert_eq!(r(&mut st, "- a list again").0.block, Block::Bullet);
    }

    #[test]
    fn every_prefix_renders_without_panic_and_converges() {
        let doc = "# T\n\nSome **bold** and *it* and `c`.\n\n```py\nx = [1](2)\n```\n- [a](b) *c\n1) _d_\n> q\n***\n";
        for cut in 0..=doc.len() {
            let mut st = State::default();
            for l in doc[..cut].split('\n') {
                let _ = r(&mut st, l);
            }
        }
        let mut st = State::default();
        let all: Vec<String> = doc.split('\n').map(|l| r(&mut st, l).1).collect();
        assert_eq!(all[2], "Some bold and it and c.");
        assert_eq!(all[5], "x = [1](2)");
        assert_eq!(all[7], "- a (b) *c");
    }

    #[test]
    fn small_buffers_mark_over_and_never_panic() {
        let mut st = State::default();
        let mut t = [0u8; 8];
        let mut sp = [Span::EMPTY; 1];
        let l = line(&mut st, b"**a** *b* `c` [d](e) and more text", &mut t, &mut sp);
        assert!(l.over);
        assert!(l.len <= 8 && l.spans <= 1);
    }

    #[test]
    fn wrap_breaks_at_spaces_with_a_hanging_indent() {
        let t = b"- one two three four five";
        let mut rows = Vec::new();
        wrap(t, 10, 2, &mut |a, b, f| rows.push((String::from_utf8(t[a..b].to_vec()).unwrap(), f)));
        assert_eq!(rows, std::vec![("- one two".into(), true), ("three".into(), false), ("four".into(), false), ("five".into(), false)]);
        let mut n = 0;
        wrap(b"", 10, 0, &mut |_, _, _| n += 1);
        assert_eq!(n, 1);
        let mut rows = Vec::new();
        wrap(b"abcdefghijkl", 5, 0, &mut |a, b, _| rows.push((a, b)));
        assert_eq!(rows, std::vec![(0, 5), (5, 10), (10, 12)]);
    }

    #[test]
    fn wrap_px_rows() {
        // KERNELFONT2: proportional rows — 'i' is 3 px, everything else 7 px.
        let t = b"iii wide words here";
        let adv = |i: usize| if t[i] == b'i' { 3.0 } else { 7.0 };
        let mut rows: Vec<(usize, usize, bool)> = Vec::new();
        wrap_px(t, 60.0, 14.0, &adv, &mut |a, b, f| rows.push((a, b, f)));
        let w = |a: usize, b: usize| (a..b).map(|i| adv(i)).sum::<f32>();
        assert_eq!(rows[0], (0, 8, true)); // "iii wide" = 9+7+28 = 44; "+ words" would be 93
        for &(a, b, f) in &rows {
            assert!(w(a, b) <= if f { 60.0 } else { 46.0 } + 0.01, "{:?}", (a, b));
            assert!(b > a);
        }
        assert_eq!(rows.last().unwrap().1, t.len());
        let mut one = Vec::new();
        wrap_px(b"", 60.0, 0.0, &|_| 7.0, &mut |a, b, f| one.push((a, b, f)));
        assert_eq!(one, [(0, 0, true)]);
        let mut hard = Vec::new();
        let long = b"abcdefghijklmnop";
        wrap_px(long, 20.0, 0.0, &|_| 7.0, &mut |a, b, f| hard.push((a, b, f)));
        assert_eq!(hard[0], (0, 2, true));
        assert_eq!(hard.last().unwrap().1, long.len());
    }
}
