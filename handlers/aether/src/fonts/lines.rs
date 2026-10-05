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
use font_kit::font::Font;
use std::cell::RefCell;
use std::collections::HashMap;

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

/// Per-character advance in px at `size` for the face `key`, cached in font
/// units (taffy measures every leaf several times per layout).
pub struct Advancer<'a> {
    font: &'a Font,
    key: u8,
    scale: f32,
    space: f32,
    ls: f32,
}

thread_local! {
    static ADVANCES: RefCell<HashMap<(u8, char), f32>> = RefCell::new(HashMap::new());
}

impl<'a> Advancer<'a> {
    pub fn new(font: &'a Font, key: u8, size: f32, letter_spacing: f32) -> Self {
        let scale = size / font.metrics().units_per_em as f32;
        Advancer { font, key, scale, space: super::space_advance(font, size), ls: letter_spacing }
    }
    pub fn char(&self, c: char) -> f32 {
        if c == ' ' {
            return self.space + self.ls;
        }
        // Cached in em (advance / units-per-em), so a fallback face with
        // its own units per em measures right.
        let em = ADVANCES.with(|m| {
            if let Some(a) = m.borrow().get(&(self.key, c)) {
                return *a;
            }
            let upem = self.font.metrics().units_per_em as f32;
            let a = match self.font.glyph_for_char(c).filter(|&g| g != 0) {
                Some(g) => self.font.advance(g).map(|a| a.x() / upem).unwrap_or(0.0),
                // The glyph comes from the fallback face (fonts::fallback_for),
                // drawn with ITS advance.
                None => super::fallback_for(c, super::key_is_bold(self.key))
                    .and_then(|(f, _)| {
                        let g = f.glyph_for_char(c)?;
                        let u = f.metrics().units_per_em as f32;
                        f.advance(g).ok().map(|a| a.x() / u)
                    })
                    .unwrap_or(0.0),
            };
            m.borrow_mut().insert((self.key, c), a);
            a
        });
        em * self.scale * self.font.metrics().units_per_em as f32 + self.ls
    }
    pub fn str(&self, s: &str) -> f32 {
        s.chars().map(|c| self.char(c)).sum()
    }
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
    out
}

/// Appends one word to the current line, wrapping or breaking it as the
/// mode allows. `spaced` = collapsible mode: a space separates words.
fn place_word(adv: &Advancer, word: &str, mode: &TextMode, max: f32, cur: &mut Cur, out: &mut Vec<Line>, spaced: bool) {
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
        return place_word(adv, word, mode, max, cur, out, spaced);
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
        let font = crate::fonts::face(2, false, false)?; // monospace: every char the same advance
        let a = Advancer::new(&font, 200, 10.0, 0.0);
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
    fn letter_spacing_kat() {
        let Some(font) = crate::fonts::face(2, false, false) else { return };
        let a0 = Advancer::new(&font, 201, 10.0, 0.0);
        let a2 = Advancer::new(&font, 201, 10.0, 2.0);
        assert!((a2.str("abc") - a0.str("abc") - 6.0).abs() < 1e-3);
    }
}
