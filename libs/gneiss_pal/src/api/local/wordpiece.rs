// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! BERT uncased WordPiece tokenization (EMBED, B317), hand-written.
//!
//! The same steps as the reference `BertNormalizer` + `BertPreTokenizer` +
//! `WordPiece` (lowercase, accents stripped): clean (drop NUL, U+FFFD and
//! control characters; whitespace → space), space-pad CJK ideographs,
//! lowercase, NFD + drop combining marks, split on whitespace and
//! punctuation, then greedy longest-match-first wordpieces with `##`
//! continuations (a word over 100 chars, or one with no full cover, is
//! `[UNK]`). Punctuation is ASCII punctuation plus the Unicode punctuation
//! blocks listed in [`is_punct`] — an approximation of category `P*` (the
//! reference splits there too; a symbol such as an emoji stays attached).
//! `core` + `alloc` shaped except for the NFD table.

use std::collections::BTreeMap;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

pub struct WordPiece {
    vocab: BTreeMap<String, u32>,
    unk: u32,
    cls: u32,
    sep: u32,
}

const MAX_WORD_CHARS: usize = 100;

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2B73F |
        0x2B740..=0x2B81F | 0x2B820..=0x2CEAF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F)
}

/// ASCII punctuation (as the reference treats `$`, `+`, `^`, ... ) plus the
/// Unicode punctuation blocks.
pub fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(c as u32,
            0x00A1 | 0x00A7 | 0x00AB | 0x00B6 | 0x00B7 | 0x00BB | 0x00BF |
            0x2010..=0x2027 | 0x2030..=0x205E | 0x3001..=0x3003 | 0x3008..=0x3011 | 0x3014..=0x301F |
            0xFE10..=0xFE19 | 0xFE30..=0xFE52 | 0xFE54..=0xFE61 | 0xFF01..=0xFF03 | 0xFF05..=0xFF0A |
            0xFF0C..=0xFF0F | 0xFF1A | 0xFF1B | 0xFF1F | 0xFF20 | 0xFF3B..=0xFF3D | 0xFF3F | 0xFF5B | 0xFF5D)
}

impl WordPiece {
    /// From `vocab.txt` (one token per line; the line number is the id).
    pub fn from_vocab(text: &str) -> Result<Self, String> {
        let vocab: BTreeMap<String, u32> =
            text.lines().enumerate().map(|(i, t)| (t.trim_end_matches('\r').to_string(), i as u32)).collect();
        let id = |t: &str| vocab.get(t).copied().ok_or_else(|| format!("vocab.txt has no {t}"));
        Ok(WordPiece { unk: id("[UNK]")?, cls: id("[CLS]")?, sep: id("[SEP]")?, vocab })
    }

    /// Normalise and split into words.
    pub fn words(text: &str) -> Vec<String> {
        let mut cleaned = String::with_capacity(text.len());
        for c in text.chars() {
            if c == '\0' || c == '\u{FFFD}' || (c.is_control() && !matches!(c, '\t' | '\n' | '\r')) {
                continue;
            }
            if c.is_whitespace() {
                cleaned.push(' ');
            } else if is_cjk(c) {
                cleaned.push(' ');
                cleaned.push(c);
                cleaned.push(' ');
            } else {
                cleaned.push(c);
            }
        }
        let lowered = cleaned.to_lowercase();
        let stripped: String = lowered.nfd().filter(|c| !is_combining_mark(*c)).collect();
        let mut words = Vec::new();
        for chunk in stripped.split_whitespace() {
            let mut cur = String::new();
            for c in chunk.chars() {
                if is_punct(c) {
                    if !cur.is_empty() {
                        words.push(core::mem::take(&mut cur));
                    }
                    words.push(c.to_string());
                } else {
                    cur.push(c);
                }
            }
            if !cur.is_empty() {
                words.push(cur);
            }
        }
        words
    }

    fn word_ids(&self, word: &str, out: &mut Vec<u32>) {
        let chars: Vec<char> = word.chars().collect();
        if chars.len() > MAX_WORD_CHARS {
            out.push(self.unk);
            return;
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let mut end = chars.len();
            let mut found = None;
            while start < end {
                let mut sub: String = chars[start..end].iter().collect();
                if start > 0 {
                    sub.insert_str(0, "##");
                }
                if let Some(&id) = self.vocab.get(&sub) {
                    found = Some(id);
                    break;
                }
                end -= 1;
            }
            match found {
                Some(id) => pieces.push(id),
                None => {
                    out.push(self.unk);
                    return;
                }
            }
            start = end;
        }
        out.extend(pieces);
    }

    /// `[CLS] tokens… [SEP]`, at most `max_len` ids.
    pub fn encode(&self, text: &str, max_len: usize) -> Vec<u32> {
        let mut ids = vec![self.cls];
        for w in Self::words(text) {
            self.word_ids(&w, &mut ids);
        }
        ids.truncate(max_len.max(2) - 1);
        ids.push(self.sep);
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_normalises_and_wordpieces() {
        assert_eq!(WordPiece::words("Café, naïve!  Hello\tWORLD"), vec!["cafe", ",", "naive", "!", "hello", "world"]);
        let wp = WordPiece::from_vocab("[PAD]\n[UNK]\n[CLS]\n[SEP]\nun\n##believ\n##able\nhello\n,").unwrap();
        assert_eq!(wp.encode("Unbelievable, hello xyz", 64), vec![2, 4, 5, 6, 8, 7, 1, 3]);
        assert_eq!(wp.encode("hello hello hello", 3), vec![2, 7, 3]);
    }
}
