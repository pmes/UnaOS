//! Line breaking for one text run (css-text-3), shared by the measurer
//! (layout::remeasure) and the painter (render::draw_text) so a run is
//! painted on exactly the lines it was measured with.
//!
//! Covered:
//! - §3 `white-space`: normal / nowrap / pre-line collapse runs of
//!   whitespace to one space (§4.1.1); pre / pre-wrap / break-spaces
//!   preserve spaces and tabs (tab stops of 8 spaces, §4.2); pre, pre-wrap
//!   and pre-line honour forced line breaks; nowrap and pre never wrap.
//! - §5.2 `word-break: break-all` (a break opportunity between any two
//!   typographic letter units) and `keep-all` (treated as normal for the
//!   scripts in the corpus).
//! - §5.5 `overflow-wrap: break-word | anywhere` (and the legacy
//!   `word-break: break-word`): a word that cannot fit on an empty line
//!   breaks at an arbitrary point instead of overflowing.
//! - §8.2 `letter-spacing`: added after every typographic character unit.
//! - Inter-element spaces: a run's collapsible leading/trailing whitespace
//!   is carried as `lead`/`trail` flags (set by the inline whitespace
//!   collapsing pass across element boundaries, §4.1.1 phase I) so
//!   "text <b>bold</b>, more" keeps its spaces and loses none to trimming.
//! - §5.1 / UAX #14 (AETHERFONT): inside a space-delimited word, the line
//!   breaking algorithm's opportunities (after hyphens, between ideographs,
//!   …; `font_core::linebreak`) are soft wrap points too; `keep-all` keeps
//!   letters together.
//! - Widths are SHAPED widths ([`Advancer`]: kerning, ligatures, fallback),
//!   and a finished line's width is its whole text shaped, as painted.
pub use super::shape::Advancer;

/// The text properties line breaking depends on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMode {
    /// 0 normal, 1 nowrap, 2 pre, 3 pre-wrap, 4 pre-line, 5 break-spaces.
    pub white_space: u8,
    /// 0 normal, 1 break-all, 2 keep-all.
    pub word_break: u8,
    /// 0 normal, 1 break-word / anywhere.
    pub overflow_wrap: u8,
    /// Extra px after every character.
    pub letter_spacing: f32,
    /// Extra px on every word separator (css-text-3 §7.1).
    pub word_spacing: f32,
    /// The paragraph direction is right-to-left (bidi base level 1).
    pub rtl: bool,
    /// A collapsible space precedes the run's first word.
    pub lead: bool,
    /// A collapsible space follows the run's last word.
    pub trail: bool,
}

impl TextMode {
    fn collapses(&self) -> bool {
        matches!(self.white_space, 0 | 1 | 4)
    }
    fn wraps(&self) -> bool {
        !matches!(self.white_space, 1 | 2)
    }
    fn forced_breaks(&self) -> bool {
        matches!(self.white_space, 2 | 3 | 4 | 5)
    }
}

/// One laid-out line: the characters to draw (spaces included), and its
/// advance width in px.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub width: f32,
}

/// Breaks `text` into lines no wider than `max_width` (when the mode wraps).
pub fn break_lines(adv: &Advancer, text: &str, mode: &TextMode, max_width: f32) -> Vec<Line> {
    break_lines_from(adv, text, mode, max_width, 0.0, false)
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
        out.push(std::mem::replace(&mut self.line, Line { text: String::new(), width: 0.0 }));
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
pub fn break_lines_from(adv: &Advancer, text: &str, mode: &TextMode, max_width: f32, start_x: f32, prior: bool) -> Vec<Line> {
    let max = if mode.wraps() { max_width.max(0.0) } else { f32::MAX };
    let mut out: Vec<Line> = Vec::new();
    // Forced breaks split the run into paragraphs first.
    let paragraphs: Vec<&str> = if mode.forced_breaks() { text.split('\n').collect() } else { vec![text] };
    let n_par = paragraphs.len();
    let starts_space = text.starts_with(|c: char| c.is_ascii_whitespace());
    let mut cur = Cur {
        line: Line { text: String::new(), width: 0.0 },
        offset: start_x,
        prior,
        glued: prior && !mode.lead && !(starts_space && !mode.collapses()),
    };
    for (pi, para) in paragraphs.into_iter().enumerate() {
        if mode.collapses() {
            let words: Vec<&str> = para.split_whitespace().collect();
            let lead = pi == 0 && mode.lead;
            if lead {
                cur.line.text.push(' ');
                cur.line.width += adv.char(' ');
            }
            for word in &words {
                place_word(adv, word, mode, max, &mut cur, &mut out, true);
            }
            if pi == n_par - 1 && mode.trail && !words.is_empty() {
                cur.line.text.push(' ');
                cur.line.width += adv.char(' ');
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
            if !mode.wraps() {
                cur.line.width = adv.str(&expanded);
                cur.line.text = expanded;
            } else {
                // pre-wrap: break after space runs.
                let mut word = String::new();
                for c in expanded.chars() {
                    word.push(c);
                    if c == ' ' {
                        place_word(adv, &word, mode, max, &mut cur, &mut out, false);
                        word.clear();
                    }
                }
                if !word.is_empty() {
                    place_word(adv, &word, mode, max, &mut cur, &mut out, false);
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
            l.width = adv.str(&l.text);
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
fn place_word(adv: &Advancer, word: &str, mode: &TextMode, max: f32, cur: &mut Cur, out: &mut Vec<Line>, spaced: bool) {
    if mode.word_break == 1 || !mode.wraps() {
        return place_unit(adv, word, mode, max, cur, out, spaced);
    }
    for (k, piece) in soft_pieces(word, mode.word_break == 2).into_iter().enumerate() {
        place_unit(adv, piece, mode, max, cur, out, spaced && k == 0);
    }
}

/// Appends one wrap unit to the current line, wrapping or breaking it as the
/// mode allows. `spaced` = collapsible mode: a space separates words.
fn place_unit(adv: &Advancer, word: &str, mode: &TextMode, max: f32, cur: &mut Cur, out: &mut Vec<Line>, spaced: bool) {
    let own = !cur.line.text.trim().is_empty() || (!spaced && !cur.line.text.is_empty());
    let glued = std::mem::take(&mut cur.glued) && cur.line.text.is_empty();
    let has_content = (own || cur.prior) && !glued;
    let gap = if spaced && own { adv.char(' ') } else { 0.0 };
    let w = adv.str(word);
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
            let cw = adv.char(c);
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
        return place_unit(adv, word, mode, max, cur, out, spaced);
    }
    if gap > 0.0 {
        cur.line.text.push(' ');
        cur.line.width += gap;
    }
    if cur.offset + cur.line.width + w > max && mode.overflow_wrap == 1 && max < f32::MAX && !glued {
        // overflow-wrap: the word cannot fit even on its own line.
        for c in word.chars() {
            let cw = adv.char(c);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn adv_test<R>(f: impl FnOnce(&Advancer) -> R) -> Option<R> {
        // monospace: every char the same advance
        let sel = crate::fonts::FontSel::new(crate::fonts::MONO, 400, false);
        crate::fonts::face(&sel)?;
        let a = Advancer::new(sel, 10.0, 0.0);
        Some(f(&a))
    }

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn white_space_kat() {
        adv_test(|a| {
            let cw = a.char('x');
            let m = TextMode::default();
            // normal: collapse, wrap at 7 chars wide.
            let l = break_lines(a, "  aa   bb\n cc ", &m, cw * 7.0);
            assert_eq!(texts(&l), vec!["aa bb", "cc"]);
            // nowrap: one line.
            let l = break_lines(a, "aa bb cc", &TextMode { white_space: 1, ..m }, cw * 3.0);
            assert_eq!(texts(&l), vec!["aa bb cc"]);
            // pre: spaces and newlines kept, tab to the 8-column stop.
            let l = break_lines(a, "a  b\n\tc", &TextMode { white_space: 2, ..m }, cw * 2.0);
            assert_eq!(texts(&l), vec!["a  b", "        c"]);
            // pre-line: newlines kept, spaces collapsed.
            let l = break_lines(a, "a   b\nc", &TextMode { white_space: 4, ..m }, 1000.0);
            assert_eq!(texts(&l), vec!["a b", "c"]);
            // lead/trail spaces carry across element boundaries.
            let l = break_lines(a, " x ", &TextMode { lead: true, trail: true, ..m }, 1000.0);
            assert_eq!(texts(&l), vec![" x "]);
        });
    }

    #[test]
    fn word_breaking_kat() {
        adv_test(|a| {
            let cw = a.char('x');
            let m = TextMode::default();
            // A long word overflows by default...
            let l = break_lines(a, "ab abcdefgh", &m, cw * 4.0);
            assert_eq!(texts(&l), vec!["ab", "abcdefgh"]);
            // ...breaks anywhere under overflow-wrap once alone on a line...
            let l = break_lines(a, "ab abcdefgh", &TextMode { overflow_wrap: 1, ..m }, cw * 4.0);
            assert_eq!(texts(&l), vec!["ab", "abcd", "efgh"]);
            // ...and fills every line under word-break: break-all.
            let l = break_lines(a, "ab abcdefgh", &TextMode { word_break: 1, ..m }, cw * 4.0);
            assert_eq!(texts(&l), vec!["ab a", "bcde", "fgh"]);
        });
    }

    #[test]
    fn continuation_kat() {
        adv_test(|a| {
            let cw = a.char('x');
            let m = TextMode::default();
            // 6 chars already on a 10-char line: "bb" joins it, "cccc" wraps.
            let l = break_lines_from(a, "bb cccc dd", &TextMode { lead: true, ..m }, cw * 10.0, cw * 6.0, true);
            assert_eq!(texts(&l), vec![" bb", "cccc dd"]);
            // Even the first word does not fit: the joining line is empty.
            let l = break_lines_from(a, "bbbbb", &TextMode { lead: true, ..m }, cw * 10.0, cw * 8.0, true);
            assert_eq!(texts(&l), vec![" ", "bbbbb"]);
            // No space between the items: no break opportunity before "bbbbb".
            let l = break_lines_from(a, "bbbbb cc", &m, cw * 10.0, cw * 8.0, true);
            assert_eq!(texts(&l), vec!["bbbbb", "cc"]);
        });
    }

    #[test]
    fn uax14_soft_wraps_kat() {
        adv_test(|a| {
            let cw = a.char('x');
            let m = TextMode::default();
            // after the hyphen of a compound
            let l = break_lines(a, "aa well-known", &m, cw * 8.0);
            assert_eq!(texts(&l), vec!["aa well-", "known"]);
            assert_eq!(soft_pieces("世界、こんにちは", false), vec!["世", "界、", "こ", "ん", "に", "ち", "は"]);
            // keep-all keeps letters together, not the hyphen opportunity
            assert_eq!(soft_pieces("世界、こんにちは", true), vec!["世界、", "こんにちは"]);
            let l = break_lines(a, "aa well-known", &TextMode { word_break: 2, ..m }, cw * 8.0);
            assert_eq!(texts(&l), vec!["aa well-", "known"]);
        });
    }

    #[test]
    fn letter_spacing_kat() {
        let sel = crate::fonts::FontSel::new(crate::fonts::MONO, 400, false);
        if crate::fonts::face(&sel).is_none() {
            return;
        }
        let a0 = Advancer::new(sel, 10.0, 0.0);
        let a2 = Advancer::new(sel, 10.0, 2.0);
        assert!((a2.str("abc") - a0.str("abc") - 6.0).abs() < 1e-3);
    }
}
