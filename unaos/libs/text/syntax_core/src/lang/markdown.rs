// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Markdown, from the CommonMark Spec 0.31.2, by its own two-phase strategy (Appendix: "A parsing
//! strategy"): phase 1 decides each line's BLOCK structure, phase 2 parses the INLINES of paragraph text.
//! Good enough for highlighting, not a renderer.
//!
//! Phase 1 (§4, §5): block-quote markers (`>`, up to three spaces of indent) and list markers (`-` `+`
//! `*`, `1.` `1)`) are container punctuation; then the leaf: ATX headings (§4.2, 1–6 `#` then a space or
//! the end), setext underlines (§4.3, only directly under paragraph text), thematic breaks (§4.1),
//! fenced code (§4.5: three or more backticks or tildes, a backtick fence's info string has no backtick;
//! it closes on a fence of the same character at least as long, indented at most three spaces), indented
//! code (§4.4: four columns, never interrupting a paragraph), HTML blocks (§4.6, start conditions 1–6
//! with their end conditions; condition 7 cannot interrupt a paragraph and is left to inline raw HTML),
//! link reference definitions (§4.7, never interrupting a paragraph), and paragraphs (§4.8).
//!
//! Phase 2 (§6), within one line: backslash escapes (§2.4), entity and numeric character references
//! (§2.5), code spans (§6.1: a backtick run closed by a run of the same length; they bind tighter than
//! emphasis), autolinks (§6.5, URI and email), raw HTML (§6.6), links (§6.3: `[text](destination)` and
//! full/collapsed references `[text][label]`; links do not contain links), and emphasis by the spec's
//! delimiter-run algorithm (§6.2: left- and right-flanking runs, the `_` intraword rule, the "rule of 3",
//! then "process emphasis" matching closers to openers). Paragraph continuation lines are inlined on
//! their own (emphasis does not span lines here).
//!
//! State: `mode` 1 = fenced code (`n` = fence length, `stack` = [fence byte]), 2 = HTML block to a blank
//! line (condition 6), 3 = HTML comment (2), 4 = raw-text element (1), 5 = processing instruction (3),
//! 6 = declaration (4), 7 = CDATA (5); `flags` bit 0 = the previous line was paragraph text.

use alloc::vec;
use alloc::vec::Vec;

use crate::scan::{at, blank, skip, starts};
use crate::{HighlightKind as K, Language, LexState, LineOut};

pub struct Markdown;

const M_FENCE: u8 = 1;
const M_HTML6: u8 = 2;
const M_COMMENT: u8 = 3;
const M_RAWTEXT: u8 = 4;
const M_PI: u8 = 5;
const M_DECL: u8 = 6;
const M_CDATA: u8 = 7;
const PARA: u32 = 1;

const RAW_TAGS: [&[u8]; 4] = [b"script", b"pre", b"style", b"textarea"];
const BLOCK_TAGS: &[&str] = &[
    "address", "article", "aside", "base", "basefont", "blockquote", "body", "caption", "center", "col",
    "colgroup", "dd", "details", "dialog", "dir", "div", "dl", "dt", "fieldset", "figcaption", "figure", "footer",
    "form", "frame", "frameset", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hr", "html", "iframe",
    "legend", "li", "link", "main", "menu", "menuitem", "nav", "noframes", "ol", "optgroup", "option", "p",
    "param", "search", "section", "summary", "table", "tbody", "td", "tfoot", "th", "thead", "title", "tr",
    "track", "ul",
];

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// Up to three columns of indent from `i`; `None` when there are four or more.
fn indent3(s: &[u8], i: usize) -> Option<usize> {
    let j = skip(s, i, |b| b == b' ');
    if j - i <= 3 { Some(j) } else { None }
}

/// The column width of the leading blanks from `i` (a tab advances to the next multiple of four).
fn columns(s: &[u8], i: usize) -> (usize, usize) {
    let mut col = 0;
    let mut k = i;
    while k < s.len() && blank(s[k]) {
        col = if s[k] == b'\t' { (col / 4 + 1) * 4 } else { col + 1 };
        k += 1;
    }
    (k, col)
}

/// Does the raw-text end condition (`</script>` …) occur in `s`?
fn rawtext_end(s: &[u8]) -> bool {
    RAW_TAGS.iter().any(|t| {
        let mut n = Vec::with_capacity(t.len() + 3);
        n.extend_from_slice(b"</");
        n.extend_from_slice(t);
        n.push(b'>');
        contains_ci(s, &n)
    })
}

fn ends_block(mode: u8, s: &[u8]) -> bool {
    match mode {
        M_COMMENT => contains(s, b"-->"),
        M_RAWTEXT => rawtext_end(s),
        M_PI => contains(s, b"?>"),
        M_DECL => contains(s, b">"),
        M_CDATA => contains(s, b"]]>"),
        _ => false,
    }
}

/// HTML block start conditions 1–6 at `j` (`s[j] == b'<'`).
fn html_start(s: &[u8], j: usize) -> Option<u8> {
    if starts(s, j, b"<!--") {
        return Some(M_COMMENT);
    }
    if starts(s, j, b"<?") {
        return Some(M_PI);
    }
    if starts(s, j, b"<![CDATA[") {
        return Some(M_CDATA);
    }
    if at(s, j + 1) == b'!' && at(s, j + 2).is_ascii_alphabetic() {
        return Some(M_DECL);
    }
    let name_end = |from: usize| skip(s, from, |b| b.is_ascii_alphanumeric());
    for t in RAW_TAGS {
        let e = j + 1 + t.len();
        if s.len() >= e && s[j + 1..e].eq_ignore_ascii_case(t) && (e == s.len() || matches!(s[e], b' ' | b'\t' | b'>')) {
            return Some(M_RAWTEXT);
        }
    }
    let from = if at(s, j + 1) == b'/' { j + 2 } else { j + 1 };
    let e = name_end(from);
    if e > from {
        let name = &s[from..e];
        let known = BLOCK_TAGS.iter().any(|t| t.as_bytes().eq_ignore_ascii_case(name));
        let after_ok = e == s.len() || matches!(s[e], b' ' | b'\t' | b'>') || starts(s, e, b"/>");
        if known && after_ok {
            return Some(M_HTML6);
        }
    }
    None
}

/// `[label]:` at `j` — a link reference definition's opening.
fn link_ref_def(s: &[u8], j: usize) -> bool {
    if at(s, j) != b'[' {
        return false;
    }
    let mut k = j + 1;
    while k < s.len() {
        match s[k] {
            b'\\' => k += 1,
            b'[' => return false,
            b']' => return k > j + 1 && at(s, k + 1) == b':',
            _ => {}
        }
        k += 1;
    }
    false
}

// ---- phase 2: inlines ---------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Delim {
    start: usize,
    len: usize,
    orig: usize,
    ch: u8,
    open: bool,
    close: bool,
}

#[derive(Clone, Copy)]
struct Bracket {
    pos: usize,
    image: bool,
    active: bool,
}

fn ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn punct(b: u8) -> bool {
    b.is_ascii_punctuation()
}

/// `&name;`, `&#123;`, `&#x1F;` at `i`.
fn entity(s: &[u8], i: usize) -> Option<usize> {
    let (from, e) = if at(s, i + 1) == b'#' {
        if matches!(at(s, i + 2), b'x' | b'X') {
            (i + 3, skip(s, i + 3, |b| b.is_ascii_hexdigit()))
        } else {
            (i + 2, skip(s, i + 2, |b| b.is_ascii_digit()))
        }
    } else {
        (i + 1, skip(s, i + 1, |b| b.is_ascii_alphanumeric()))
    };
    if e > from && e - from <= 32 && at(s, e) == b';' { Some(e + 1) } else { None }
}

/// A URI or email autolink at `i` (`s[i] == b'<'`).
fn autolink(s: &[u8], i: usize) -> Option<usize> {
    // URI: scheme (2–32 of [A-Za-z][A-Za-z0-9+.-]*) ':' then no space, '<' or '>' until '>'.
    let se = skip(s, i + 1, |b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'));
    if at(s, i + 1).is_ascii_alphabetic() && (2..=32).contains(&(se - i - 1)) && at(s, se) == b':' {
        let e = skip(s, se + 1, |b| b > b' ' && b != b'<' && b != b'>' && b != 0x7f);
        if at(s, e) == b'>' {
            return Some(e + 1);
        }
    }
    // Email: local '@' domain '>'.
    let le = skip(s, i + 1, |b| b.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&b));
    if le > i + 1 && at(s, le) == b'@' {
        let de = skip(s, le + 1, |b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if de > le + 1 && at(s, de) == b'>' {
            return Some(de + 1);
        }
    }
    None
}

/// Inline raw HTML at `i`: an open/closing tag, a comment, or a processing instruction on this line.
fn raw_html(s: &[u8], i: usize) -> Option<usize> {
    if starts(s, i, b"<!--") {
        return s[i + 4..].windows(3).position(|w| w == b"-->").map(|p| i + 4 + p + 3);
    }
    if starts(s, i, b"<?") {
        return s[i + 2..].windows(2).position(|w| w == b"?>").map(|p| i + 2 + p + 2);
    }
    let from = if at(s, i + 1) == b'/' { i + 2 } else { i + 1 };
    if !at(s, from).is_ascii_alphabetic() {
        return None;
    }
    let mut k = skip(s, from, |b| b.is_ascii_alphanumeric() || b == b'-');
    while k < s.len() {
        match s[k] {
            b'>' => return Some(k + 1),
            b'<' => return None,
            b'"' | b'\'' => {
                let q = s[k];
                k += 1;
                while k < s.len() && s[k] != q {
                    k += 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// `(destination "title")` at `p` (`s[p] == b'('`): past the balancing `)`.
fn inline_dest_end(s: &[u8], p: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut k = p;
    while k < s.len() {
        match s[k] {
            b'\\' => k += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k + 1);
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// `[label]` at `p` (`s[p] == b'['`): past the `]`.
fn ref_end(s: &[u8], p: usize) -> Option<usize> {
    let mut k = p + 1;
    while k < s.len() {
        match s[k] {
            b'\\' => k += 1,
            b'[' => return None,
            b']' => return Some(k + 1),
            _ => {}
        }
        k += 1;
    }
    None
}

fn mark(kinds: &mut [Option<K>], a: usize, b: usize, k: K, overwrite: bool) {
    let e = b.min(kinds.len());
    for slot in &mut kinds[a.min(e)..e] {
        if overwrite || slot.is_none() {
            *slot = Some(k);
        }
    }
}

/// §6.2 delimiter run at `i..r` of byte `c`.
fn delim(s: &[u8], i: usize, r: usize, c: u8) -> Delim {
    let before = if i == 0 { b' ' } else { s[i - 1] };
    let after = if r >= s.len() { b' ' } else { s[r] };
    let left = !ws(after) && (!punct(after) || ws(before) || punct(before));
    let right = !ws(before) && (!punct(before) || ws(after) || punct(after));
    let (open, close) = if c == b'*' {
        (left, right)
    } else {
        (left && (!right || punct(before)), right && (!left || punct(after)))
    };
    Delim { start: i, len: r - i, orig: r - i, ch: c, open, close }
}

/// §6.3 "process emphasis" over the whole line's delimiter list.
fn process_emphasis(d: &mut [Delim], kinds: &mut [Option<K>]) {
    let mut ci = 0;
    while ci < d.len() {
        if !d[ci].close || d[ci].len == 0 {
            ci += 1;
            continue;
        }
        let mut found = None;
        let mut oi = ci;
        while oi > 0 {
            oi -= 1;
            let (o, c) = (d[oi], d[ci]);
            if o.ch == c.ch && o.open && o.len > 0 {
                let odd = (o.close || c.open) && (o.orig + c.orig) % 3 == 0 && !(o.orig % 3 == 0 && c.orig % 3 == 0);
                if !odd {
                    found = Some(oi);
                    break;
                }
            }
        }
        match found {
            Some(oi) => {
                let n = if d[oi].len >= 2 && d[ci].len >= 2 { 2 } else { 1 };
                let a = d[oi].start + d[oi].len - n;
                let b = d[ci].start + n;
                mark(kinds, a, b, K::Emphasis, false);
                d[oi].len -= n;
                d[ci].start += n;
                d[ci].len -= n;
                for k in d[oi + 1..ci].iter_mut() {
                    k.len = 0;
                }
                if d[ci].len == 0 {
                    ci += 1;
                }
            }
            None => ci += 1,
        }
    }
}

/// Phase 2 over `s[from..]`, appending tokens.
fn inline(s: &[u8], from: usize, out: &mut LineOut) {
    let len = s.len();
    let mut kinds: Vec<Option<K>> = vec![None; len];
    let mut delims: Vec<Delim> = Vec::new();
    let mut brackets: Vec<Bracket> = Vec::new();
    let mut i = from;
    while i < len {
        let c = s[i];
        match c {
            b'\\' => {
                if i + 1 < len && punct(s[i + 1]) {
                    mark(&mut kinds, i, i + 2, K::Escape, true);
                    i += 2;
                } else {
                    if i + 1 == len {
                        mark(&mut kinds, i, i + 1, K::Escape, true);
                    }
                    i += 1;
                }
            }
            b'&' => match entity(s, i) {
                Some(e) => {
                    mark(&mut kinds, i, e, K::Escape, true);
                    i = e;
                }
                None => i += 1,
            },
            b'`' => {
                let r = skip(s, i, |b| b == b'`');
                let n = r - i;
                let mut k = r;
                let mut found = None;
                while k < len {
                    if s[k] == b'`' {
                        let r2 = skip(s, k, |b| b == b'`');
                        if r2 - k == n {
                            found = Some(r2);
                            break;
                        }
                        k = r2;
                    } else {
                        k += 1;
                    }
                }
                match found {
                    Some(e) => {
                        mark(&mut kinds, i, e, K::Code, true);
                        i = e;
                    }
                    None => i = r,
                }
            }
            b'<' => {
                if let Some(e) = autolink(s, i) {
                    mark(&mut kinds, i, e, K::Link, true);
                    i = e;
                } else if let Some(e) = raw_html(s, i) {
                    mark(&mut kinds, i, e, K::Attribute, true);
                    i = e;
                } else {
                    i += 1;
                }
            }
            b'*' | b'_' => {
                let r = skip(s, i, |b| b == c);
                delims.push(delim(s, i, r, c));
                i = r;
            }
            b'!' if at(s, i + 1) == b'[' => {
                brackets.push(Bracket { pos: i, image: true, active: true });
                i += 2;
            }
            b'[' => {
                brackets.push(Bracket { pos: i, image: false, active: true });
                i += 1;
            }
            b']' => {
                if let Some(bi) = brackets.len().checked_sub(1) {
                    let br = brackets[bi];
                    brackets.truncate(bi);
                    if br.active {
                        let after = i + 1;
                        let end = match at(s, after) {
                            b'(' => inline_dest_end(s, after),
                            b'[' => ref_end(s, after),
                            _ => None,
                        };
                        if let Some(e) = end {
                            mark(&mut kinds, br.pos, e, K::Link, false);
                            if !br.image {
                                for o in brackets.iter_mut().filter(|o| !o.image) {
                                    o.active = false;
                                }
                            }
                            i = e;
                            continue;
                        }
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    process_emphasis(&mut delims, &mut kinds);
    let mut k = from;
    while k < len {
        match kinds[k] {
            Some(kind) => {
                let st = k;
                while k < len && kinds[k] == Some(kind) {
                    k += 1;
                }
                out.push(kind, st, k);
            }
            None => k += 1,
        }
    }
}

impl Language for Markdown {
    fn id(&self) -> &'static str {
        "markdown"
    }

    fn lex_line(&self, line: &str, st: &mut LexState, out: &mut LineOut) {
        let s = line.as_bytes();
        let len = s.len();
        // Containers: block-quote markers.
        let mut i = 0;
        while let Some(j) = indent3(s, i) {
            if at(s, j) != b'>' {
                break;
            }
            out.push(K::Punctuation, j, j + 1);
            i = j + 1;
            if at(s, i) == b' ' {
                i += 1;
            }
        }
        let para = st.flags & PARA != 0;
        match st.mode {
            M_FENCE => {
                let fc = st.stack.first().copied().unwrap_or(b'`');
                if let Some(j) = indent3(s, i) {
                    let r = skip(s, j, |b| b == fc);
                    if r - j >= st.n as usize && r - j >= 3 && skip(s, r, blank) == len {
                        out.push(K::Code, j, len);
                        st.mode = 0;
                        st.n = 0;
                        st.stack.clear();
                        return;
                    }
                }
                out.push(K::Code, i, len);
                return;
            }
            M_HTML6 => {
                if skip(s, i, blank) == len {
                    st.mode = 0;
                } else {
                    out.push(K::Attribute, i, len);
                }
                return;
            }
            M_COMMENT | M_RAWTEXT | M_PI | M_DECL | M_CDATA => {
                out.push(if st.mode == M_COMMENT { K::Comment } else { K::Attribute }, i, len);
                if ends_block(st.mode, &s[i..]) {
                    st.mode = 0;
                }
                return;
            }
            _ => {}
        }
        let (j, col) = columns(s, i);
        if j == len {
            st.flags &= !PARA;
            return;
        }
        if col >= 4 && !para {
            out.push(K::Code, i, len);
            return;
        }
        let c = s[j];
        // Fenced code opening.
        if c == b'`' || c == b'~' {
            let r = skip(s, j, |b| b == c);
            if r - j >= 3 && (c == b'~' || !s[r..].contains(&b'`')) {
                out.push(K::Code, j, len);
                st.mode = M_FENCE;
                st.n = (r - j) as u32;
                st.stack.clear();
                st.stack.push(c);
                st.flags &= !PARA;
                return;
            }
        }
        // ATX heading.
        if c == b'#' {
            let r = skip(s, j, |b| b == b'#');
            if r - j <= 6 && (r == len || blank(s[r])) {
                out.push(K::Heading, j, len);
                out.heading = (r - j) as u8;
                st.flags &= !PARA;
                return;
            }
        }
        // Setext underline (only under paragraph text).
        if para && (c == b'=' || c == b'-') {
            let r = skip(s, j, |b| b == c);
            if skip(s, r, blank) == len {
                out.push(K::Heading, j, len);
                out.heading = if c == b'=' { 1 } else { 2 };
                st.flags &= !PARA;
                return;
            }
        }
        // Thematic break.
        if matches!(c, b'*' | b'-' | b'_') {
            let n = s[j..].iter().filter(|&&b| b == c).count();
            if n >= 3 && s[j..].iter().all(|&b| b == c || blank(b)) {
                out.push(K::Punctuation, j, len);
                st.flags &= !PARA;
                return;
            }
        }
        // HTML block (conditions 1-6 may interrupt a paragraph).
        if c == b'<' {
            if let Some(m) = html_start(s, j) {
                out.push(if m == M_COMMENT { K::Comment } else { K::Attribute }, j, len);
                let rest = &s[j..];
                let done = match m {
                    M_COMMENT => contains(&rest[4.min(rest.len())..], b"-->"),
                    M_PI => contains(&rest[2.min(rest.len())..], b"?>"),
                    M_CDATA => contains(&rest[9.min(rest.len())..], b"]]>"),
                    M_DECL => contains(&rest[2.min(rest.len())..], b">"),
                    M_RAWTEXT => rawtext_end(rest),
                    _ => false,
                };
                if !done {
                    st.mode = m;
                }
                st.flags &= !PARA;
                return;
            }
        }
        // List item marker.
        let mut k = j;
        if matches!(c, b'-' | b'+' | b'*') && (j + 1 == len || blank(s[j + 1])) {
            out.push(K::Punctuation, j, j + 1);
            k = j + 1;
        } else if c.is_ascii_digit() {
            let r = skip(s, j, |b| b.is_ascii_digit());
            if r - j <= 9 && matches!(at(s, r), b'.' | b')') && (r + 1 == len || blank(s[r + 1])) {
                out.push(K::Punctuation, j, r + 1);
                k = r + 1;
            }
        }
        // Link reference definition (never interrupts a paragraph).
        if !para && k == j && link_ref_def(s, j) {
            out.push(K::Link, j, len);
            return;
        }
        inline(s, k, out);
        st.flags |= PARA;
    }
}
