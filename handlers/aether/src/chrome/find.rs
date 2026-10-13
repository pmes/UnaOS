//! Find in page (std-free): the matcher and the active-match state behind Ctrl+F.
//!
//! [`find_all`] is the text search the engine runs over each rendered text run: case-insensitive by
//! simple case folding (a character whose lowercase is one character compares by it; the rare
//! multi-character lowercasings compare exactly), non-overlapping, left to right — what Chromium's
//! default find (no "match case") does for the common scripts. [`FindState`] is the bar's state: the
//! query, how many hits, and which one is active; a repeat of the same query steps instead of
//! searching again, wrapping at either end.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

fn fold(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// Every non-overlapping case-insensitive occurrence of `needle` in `hay`, as byte ranges into `hay`.
/// An empty needle finds nothing.
pub fn find_all(hay: &str, needle: &str) -> Vec<(usize, usize)> {
    let pat: Vec<char> = needle.chars().map(fold).collect();
    if pat.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = hay.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + pat.len() <= chars.len() {
        if chars[i..i + pat.len()].iter().zip(pat.iter()).all(|((_, a), b)| a == b) {
            let start = chars[i].0;
            let end = chars.get(i + pat.len()).map(|(b, _)| *b).unwrap_or(hay.len());
            out.push((start, end));
            i += pat.len();
        } else {
            i += 1;
        }
    }
    out
}

/// The find bar's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindState {
    pub query: String,
    pub count: usize,
    /// The active match (0-based), when there is one.
    pub current: Option<usize>,
}

impl FindState {
    /// Whether `query` is the one already searched (the next Enter steps rather than re-searches).
    pub fn is_repeat(&self, query: &str) -> bool {
        self.count > 0 && self.query == query
    }

    /// A fresh search found `count` hits; the first (or, searching backwards, the last) is active.
    pub fn begin(&mut self, query: &str, count: usize, forward: bool) -> Option<usize> {
        self.query = query.to_string();
        self.count = count;
        self.current = match count {
            0 => None,
            _ if forward => Some(0),
            n => Some(n - 1),
        };
        self.current
    }

    /// Step to the next (or previous) hit, wrapping.
    pub fn step(&mut self, forward: bool) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let c = self.current.unwrap_or(if forward { self.count - 1 } else { 0 });
        self.current = Some(if forward { (c + 1) % self.count } else { (c + self.count - 1) % self.count });
        self.current
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// `(matches, current)` for the wire: `current` is 1-based, 0 when there is no active match.
    pub fn result(&self) -> (u32, u32) {
        (self.count as u32, self.current.map(|c| c as u32 + 1).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_folds_case_and_does_not_overlap() {
        assert_eq!(find_all("Una una UNA", "una"), [(0, 3), (4, 7), (8, 11)]);
        assert_eq!(find_all("aaaa", "aa"), [(0, 2), (2, 4)]);
        assert_eq!(find_all("Ärger ärger", "ÄR"), [(0, 3), (7, 10)]);
        assert!(find_all("x", "").is_empty());
    }

    #[test]
    fn the_state_steps_and_wraps() {
        let mut f = FindState::default();
        assert_eq!(f.begin("q", 3, true), Some(0));
        assert!(f.is_repeat("q"));
        assert_eq!(f.step(true), Some(1));
        assert_eq!(f.step(true), Some(2));
        assert_eq!(f.step(true), Some(0));
        assert_eq!(f.step(false), Some(2));
        assert_eq!(f.result(), (3, 3));
        assert_eq!(f.begin("z", 0, true), None);
        assert_eq!(f.result(), (0, 0));
    }
}
