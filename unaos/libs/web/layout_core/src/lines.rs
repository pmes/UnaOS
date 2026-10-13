//! Line breaking for one text run (css-text-3) over the [`Fonts`] seam — the host's `fonts/lines.rs`
//! algorithm (white-space, word-break, overflow-wrap, letter-spacing, the inter-element `lead`/`trail` spaces,
//! UAX #14 soft wraps through font_core) with the measurer taken from [`Fonts::measure`]. It is the default
//! [`Fonts::break_lines`]; the host keeps calling its own copy (same algorithm, same numbers) until fonts/ moves.

use crate::prelude::*;
use crate::{Adv, Fonts, Line, TextMode};

fn collapses(m: &TextMode) -> bool {
    matches!(m.white_space, 0 | 1 | 4)
}
fn wraps(m: &TextMode) -> bool {
    !matches!(m.white_space, 1 | 2)
}
fn forced_breaks(m: &TextMode) -> bool {
    matches!(m.white_space, 2 | 3 | 4 | 5)
}

fn ch<F: Fonts + ?Sized>(f: &F, adv: &Adv, c: char) -> f32 {
    let mut b = [0u8; 4];
    f.measure(c.encode_utf8(&mut b), adv)
}

/// Breaks `text` into lines no wider than `max_width` (when the mode wraps).
pub fn break_lines<F: Fonts + ?Sized>(f: &F, adv: &Adv, text: &str, mode: &TextMode, max_width: f32) -> Vec<Line> {
    break_lines_from(f, adv, text, mode, max_width, 0.0, false)
}

/// The line being filled: its text so far, plus what earlier inline items
/// already put on it (`offset` px of advance, `prior` = real content).
struct Cur {
    line: Line,
    offset: f32,
    prior: bool,
    /// The next word continues the previous item's word (no space between
    /// the items): there is no break opportunity before it (css-text-3 §5.1).
    glued: bool,
}

impl Cur {
    fn push(&mut self, out: &mut Vec<Line>) {
        out.push(core::mem::replace(&mut self.line, Line { text: String::new(), width: 0.0 }));
        self.offset = 0.0;
        self.prior = false;
        self.glued = false;
    }
}

/// Breaks one text run of an inline formatting context whose current line
/// already holds `start_x` px of earlier items (`prior` = some of it is
/// content, so a break may come before this run's first word). The first
/// returned line is what joins the current line (its `text` may be empty
/// when even the first word must move down); each further line is a new
/// line box. `break_lines` is the `start_x = 0` case.
pub fn break_lines_from<F: Fonts + ?Sized>(f: &F, adv: &Adv, text: &str, mode: &TextMode, max_width: f32, start_x: f32, prior: bool) -> Vec<Line> {
    let max = if wraps(mode) { max_width.max(0.0) } else { f32::MAX };
    let mut out: Vec<Line> = Vec::new();
    // Forced breaks split the run into paragraphs first.
    let paragraphs: Vec<&str> = if forced_breaks(mode) { text.split('\n').collect() } else { vec![text] };
    let n_par = paragraphs.len();
    let starts_space = text.starts_with(|c: char| c.is_ascii_whitespace());
    let mut cur = Cur {
        line: Line { text: String::new(), width: 0.0 },
        offset: start_x,
        prior,
        glued: prior && !mode.lead && !(starts_space && !collapses(mode)),
    };
    for (pi, para) in paragraphs.into_iter().enumerate() {
        if collapses(mode) {
            let words: Vec<&str> = para.split_whitespace().collect();
            let lead = pi == 0 && mode.lead;
            if lead {
                cur.line.text.push(' ');
                cur.line.width += ch(f, adv, ' ');
            }
            for word in &words {
                place_word(f, adv, word, mode, max, &mut cur, &mut out, true);
            }
            if pi == n_par - 1 && mode.trail && !words.is_empty() {
                cur.line.text.push(' ');
                cur.line.width += ch(f, adv, ' ');
            }
        } else {
            // Preserved: spaces are content; tabs advance to 8-space stops.
            let mut expanded = String::new();
            for c in para.chars() {
                if c == '\t' {
                    let col = expanded.chars().count();
                    for _ in 0..(8 - col % 8) {
                        expanded.push(' ');
                    }
                } else if c != '\r' {
                    expanded.push(c);
                }
            }
            if !wraps(mode) {
                cur.line.width = f.measure(&expanded, adv);
                cur.line.text = expanded;
            } else {
                // pre-wrap: break after space runs.
                let mut word = String::new();
                for c in expanded.chars() {
                    word.push(c);
                    if c == ' ' {
                        place_word(f, adv, &word, mode, max, &mut cur, &mut out, false);
                        word.clear();
                    }
                }
                if !word.is_empty() {
                    place_word(f, adv, &word, mode, max, &mut cur, &mut out, false);
                }
            }
        }
        if pi + 1 < n_par {
            cur.push(&mut out);
        }
    }
    out.push(cur.line);
    // A line's width is its text shaped as one run, as the painter draws it (kerning across the
    // word-separating spaces included).
    for l in out.iter_mut() {
        if !l.text.is_empty() {
            l.width = f.measure(&l.text, adv);
        }
    }
    out
}

/// The UAX #14 soft-wrap pieces of one space-free word (a break opportunity before each piece but the
/// first): "well-known" → ["well-", "known"], "世界" → ["世", "界"]. `keep_all` (css-text-3 §5.2) drops the
/// opportunities between two letters, keeping punctuation ones.
fn soft_pieces(word: &str, keep_all: bool) -> Vec<&str> {
    if word.is_ascii() && !word.contains(['-', '/', '?', '!', '}', ')', ']', '%']) {
        return vec![word]; // no opportunity can occur inside a plain ASCII letter/digit run
    }
    let b = font_core::linebreak::breaks(word);
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0;
    for ci in 1..chars.len() {
        let bo = chars[ci].0;
        if keep_all && chars[ci - 1].1.is_alphanumeric() && chars[ci].1.is_alphanumeric() {
            continue;
        }
        if b[ci] != font_core::linebreak::Break::None {
            out.push(&word[start..bo]);
            start = bo;
        }
    }
    out.push(&word[start..]);
    out
}

/// Appends one word to the current line, its UAX #14 pieces as separate
/// wrap units (no space between them).
fn place_word<F: Fonts + ?Sized>(f: &F, adv: &Adv, word: &str, mode: &TextMode, max: f32, cur: &mut Cur, out: &mut Vec<Line>, spaced: bool) {
    if mode.word_break == 1 || !wraps(mode) {
        return place_unit(f, adv, word, mode, max, cur, out, spaced);
    }
    for (k, piece) in soft_pieces(word, mode.word_break == 2).into_iter().enumerate() {
        place_unit(f, adv, piece, mode, max, cur, out, spaced && k == 0);
    }
}

/// Appends one wrap unit to the current line, wrapping or breaking it as the
/// mode allows. `spaced` = collapsible mode: a space separates words.
fn place_unit<F: Fonts + ?Sized>(f: &F, adv: &Adv, word: &str, mode: &TextMode, max: f32, cur: &mut Cur, out: &mut Vec<Line>, spaced: bool) {
    let own = !cur.line.text.trim().is_empty() || (!spaced && !cur.line.text.is_empty());
    let glued = core::mem::take(&mut cur.glued) && cur.line.text.is_empty();
    let has_content = (own || cur.prior) && !glued;
    let gap = if spaced && own { ch(f, adv, ' ') } else { 0.0 };
    let w = f.measure(word, adv);
    let used = cur.offset + cur.line.width;
    if mode.word_break == 1 {
        // break-all: fill the line character by character.
        if gap > 0.0 {
            if used + gap < max {
                cur.line.text.push(' ');
                cur.line.width += gap;
            } else {
                cur.push(out);
            }
        }
        for c in word.chars() {
            let cw = ch(f, adv, c);
            let filled = !cur.line.text.trim().is_empty() || cur.prior;
            if cur.offset + cur.line.width + cw > max && filled {
                cur.push(out);
            }
            cur.line.text.push(c);
            cur.line.width += cw;
        }
        return;
    }
    if has_content && used + gap + w > max {
        cur.push(out);
        return place_unit(f, adv, word, mode, max, cur, out, spaced);
    }
    if gap > 0.0 {
        cur.line.text.push(' ');
        cur.line.width += gap;
    }
    if cur.offset + cur.line.width + w > max && mode.overflow_wrap == 1 && max < f32::MAX && !glued {
        // overflow-wrap: the word cannot fit even on its own line.
        for c in word.chars() {
            let cw = ch(f, adv, c);
            if cur.offset + cur.line.width + cw > max && (!cur.line.text.trim().is_empty() || cur.prior) {
                cur.push(out);
            }
            cur.line.text.push(c);
            cur.line.width += cw;
        }
        return;
    }
    cur.line.text.push_str(word);
    cur.line.width += w;
}

