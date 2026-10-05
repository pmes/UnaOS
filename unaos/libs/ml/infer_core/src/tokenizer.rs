// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The BERT tokenizer, in the order the reference (`BertTokenizerFast` / HF `tokenizers`) runs it:
//!
//! 1. **Added tokens.** The `tokenizer.json` `added_tokens` whose `normalized` is false (BERT's
//!    `[PAD] [UNK] [CLS] [SEP] [MASK]`) are found in the RAW text, leftmost-longest, and become
//!    their ids directly; the text between them goes on. (`normalized: true` ones are matched the
//!    same way in the normalised text; `lstrip` / `rstrip` swallow adjacent whitespace;
//!    `single_word` requires non-word neighbours.)
//! 2. **BertNormalizer.** Clean (drop U+0000, U+FFFD and every Cc/Cf/Co except `\t \n \r`; map
//!    whitespace to a space), pad CJK ideographs with spaces, strip accents (NFD, then drop Mn) —
//!    on when `strip_accents` is true, or null with `lowercase` — then lowercase (per char).
//! 3. **BertPreTokenizer.** Split on whitespace (removed), then on punctuation (ASCII punctuation
//!    or Unicode P*), each punctuation char its own word.
//! 4. **WordPiece.** Per word: more than `max_input_chars_per_word` chars → `[UNK]`; else greedy
//!    longest-match-first from the left, continuations prefixed `##`; a word with no full cover is
//!    one `[UNK]`.
//! 5. **Post-processing.** `[CLS] … [SEP]`; truncation (when asked) drops tokens from the right so
//!    the total, specials included, fits.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::json::{self, Value};
use crate::unicode;
use crate::{Result, err};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Added {
    content: String,
    id: u32,
    normalized: bool,
    lstrip: bool,
    rstrip: bool,
    single_word: bool,
}

#[derive(Debug, Clone)]
pub struct Tokenizer {
    vocab: BTreeMap<String, u32>,
    added: Vec<Added>,
    unk: u32,
    cls: u32,
    sep: u32,
    pad: u32,
    prefix: String,
    max_chars: usize,
    clean_text: bool,
    chinese: bool,
    strip_accents: bool,
    lowercase: bool,
}

fn is_chinese(c: char) -> bool {
    // The reference's ranges, verbatim (note 0x2B920, not 0x2B820: kept, because ids must match).
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2B73F |
        0x2B740..=0x2B81F | 0x2B920..=0x2CEAF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F)
}

fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || c.is_whitespace()
}

fn is_control(c: char) -> bool {
    !matches!(c, '\t' | '\n' | '\r') && unicode::is_other(c)
}

fn is_bert_punct(c: char) -> bool {
    c.is_ascii_punctuation() || unicode::is_punctuation(c)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A piece of the input after the added-token split.
enum Piece<'a> {
    Text(&'a str),
    Id(u32),
}

impl Tokenizer {
    /// From a `vocab.txt` (one token per line, the line number is the id) with the BERT
    /// uncased defaults; `[PAD] [UNK] [CLS] [SEP] [MASK]` are the added tokens when present.
    pub fn from_vocab_txt(src: &str) -> Result<Self> {
        let mut vocab = BTreeMap::new();
        // As the reference's `WordPiece::read_file`: one entry per line, trailing whitespace trimmed.
        for (i, line) in src.lines().enumerate() {
            vocab.insert(line.trim_end().to_string(), u32::try_from(i).map_err(|_| err("vocab: too many lines"))?);
        }
        let added = ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]"]
            .iter()
            .filter_map(|t| {
                vocab.get(*t).map(|&id| Added {
                    content: t.to_string(),
                    id,
                    normalized: false,
                    lstrip: false,
                    rstrip: false,
                    single_word: false,
                })
            })
            .collect();
        Self::build(vocab, added, "[UNK]", None, "##".to_string(), 100, (true, true, None, true))
    }

    /// From a HF `tokenizer.json` with a WordPiece model, a BertNormalizer (or none) and a
    /// BertPreTokenizer.
    pub fn from_tokenizer_json(src: &str) -> Result<Self> {
        let doc = json::parse(src)?;
        let model = doc.get("model").ok_or_else(|| err("tokenizer.json: no model"))?;
        if model.get("type").and_then(Value::as_str) != Some("WordPiece") {
            return Err(err("tokenizer.json: the model is not WordPiece"));
        }
        let mut vocab = BTreeMap::new();
        for (k, v) in model.get("vocab").and_then(Value::as_object).ok_or_else(|| err("tokenizer.json: no vocab"))? {
            let id = v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| err("tokenizer.json: bad vocab id"))?;
            vocab.insert(k.clone(), id);
        }
        let unk = model.get("unk_token").and_then(Value::as_str).unwrap_or("[UNK]");
        let prefix = model.get("continuing_subword_prefix").and_then(Value::as_str).unwrap_or("##").to_string();
        let max_chars = model
            .get("max_input_chars_per_word")
            .and_then(Value::as_u64)
            .map(|n| n.min(usize::MAX as u64) as usize)
            .unwrap_or(100);
        let mut added = Vec::new();
        for a in doc.get("added_tokens").and_then(Value::as_array).unwrap_or(&[]) {
            let content = a.get("content").and_then(Value::as_str).ok_or_else(|| err("tokenizer.json: added token without content"))?;
            let id = a.get("id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| err("tokenizer.json: added token without id"))?;
            if content.is_empty() {
                return Err(err("tokenizer.json: empty added token"));
            }
            let flag = |k: &str| a.get(k).and_then(Value::as_bool).unwrap_or(false);
            added.push(Added {
                content: content.to_string(),
                id,
                normalized: flag("normalized"),
                lstrip: flag("lstrip"),
                rstrip: flag("rstrip"),
                single_word: flag("single_word"),
            });
        }
        let flags = match doc.get("normalizer") {
            None | Some(Value::Null) => (false, false, Some(false), false),
            Some(n) if n.get("type").and_then(Value::as_str) == Some("BertNormalizer") => {
                let b = |k: &str, d: bool| n.get(k).and_then(Value::as_bool).unwrap_or(d);
                (b("clean_text", true), b("handle_chinese_chars", true), n.get("strip_accents").and_then(Value::as_bool), b("lowercase", true))
            }
            Some(_) => return Err(err("tokenizer.json: only the BertNormalizer is supported")),
        };
        match doc.get("pre_tokenizer").and_then(|p| p.get("type")).and_then(Value::as_str) {
            Some("BertPreTokenizer") => {}
            _ => return Err(err("tokenizer.json: only the BertPreTokenizer is supported")),
        }
        // [CLS] / [SEP] from the post-processor when it names them.
        let pp = doc.get("post_processor");
        let special = |name: &str| -> Option<u32> {
            let p = pp?;
            match p.get("type").and_then(Value::as_str)? {
                "TemplateProcessing" => p.get("special_tokens")?.get(name)?.get("ids")?.as_array()?.first()?.as_u64().map(|n| n as u32),
                "BertProcessing" => {
                    let key = if name == "[CLS]" { "cls" } else { "sep" };
                    p.get(key)?.as_array()?.get(1)?.as_u64().map(|n| n as u32)
                }
                _ => None,
            }
        };
        let cls_sep = special("[CLS]").zip(special("[SEP]"));
        Self::build(vocab, added, unk, cls_sep, prefix, max_chars, flags)
    }

    fn build(
        vocab: BTreeMap<String, u32>,
        mut added: Vec<Added>,
        unk: &str,
        cls_sep: Option<(u32, u32)>,
        prefix: String,
        max_chars: usize,
        (clean_text, chinese, strip_accents, lowercase): (bool, bool, Option<bool>, bool),
    ) -> Result<Self> {
        let id = |t: &str| {
            vocab
                .get(t)
                .copied()
                .or_else(|| added.iter().find(|a| a.content == t).map(|a| a.id))
                .ok_or_else(|| err(alloc::format!("tokenizer: no {t} token")))
        };
        let unk = id(unk)?;
        let (cls, sep) = match cls_sep {
            Some(p) => p,
            None => (id("[CLS]")?, id("[SEP]")?),
        };
        let pad = id("[PAD]").unwrap_or(0);
        // Longest content first so a scan can take the first hit as the longest.
        added.sort_by(|a, b| b.content.len().cmp(&a.content.len()).then(a.content.cmp(&b.content)));
        added.dedup_by(|a, b| a.content == b.content);
        Ok(Tokenizer {
            vocab,
            added,
            unk,
            cls,
            sep,
            pad,
            prefix,
            max_chars,
            clean_text,
            chinese,
            strip_accents: strip_accents.unwrap_or(lowercase),
            lowercase,
        })
    }

    pub fn pad_id(&self) -> u32 {
        self.pad
    }
    pub fn cls_id(&self) -> u32 {
        self.cls
    }
    pub fn sep_id(&self) -> u32 {
        self.sep
    }
    pub fn unk_id(&self) -> u32 {
        self.unk
    }
    /// The largest id this tokenizer can emit, plus one.
    pub fn id_bound(&self) -> u32 {
        let v = self.vocab.values().copied().max().unwrap_or(0);
        let a = self.added.iter().map(|a| a.id).max().unwrap_or(0);
        v.max(a).max(self.cls).max(self.sep) + 1
    }

    /// The vocabulary entry for `token`.
    pub fn token_id(&self, token: &str) -> Option<u32> {
        self.vocab.get(token).copied()
    }

    /// Split `text` on the added tokens with the given `normalized` flag (leftmost-longest).
    fn split_added<'a>(&self, text: &'a str, normalized: bool, out: &mut Vec<Piece<'a>>) {
        let cands: Vec<&Added> = self.added.iter().filter(|a| a.normalized == normalized).collect();
        if cands.is_empty() {
            if !text.is_empty() {
                out.push(Piece::Text(text));
            }
            return;
        }
        let mut last = 0; // start of the pending text
        let mut i = 0;
        while i < text.len() {
            let hit = cands.iter().find(|a| {
                if !text[i..].starts_with(a.content.as_str()) {
                    return false;
                }
                if a.single_word {
                    let before = text[..i].chars().next_back();
                    let after = text[i + a.content.len()..].chars().next();
                    if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                        return false;
                    }
                }
                true
            });
            match hit {
                Some(a) => {
                    let mut s = i;
                    if a.lstrip {
                        while s > last && text[..s].ends_with(|c: char| c.is_whitespace()) {
                            s -= text[..s].chars().next_back().map_or(1, char::len_utf8);
                        }
                    }
                    let mut e = i + a.content.len();
                    if a.rstrip {
                        while let Some(c) = text[e..].chars().next().filter(|c| c.is_whitespace()) {
                            e += c.len_utf8();
                        }
                    }
                    if s > last {
                        out.push(Piece::Text(&text[last..s]));
                    }
                    out.push(Piece::Id(a.id));
                    last = e;
                    i = e;
                }
                None => i += text[i..].chars().next().map_or(1, char::len_utf8),
            }
        }
        if last < text.len() {
            out.push(Piece::Text(&text[last..]));
        }
    }

    /// The BertNormalizer.
    pub fn normalize(&self, text: &str) -> String {
        let mut chars: Vec<char> = Vec::with_capacity(text.len());
        for c in text.chars() {
            if self.clean_text {
                if c == '\0' || c == '\u{FFFD}' || is_control(c) {
                    continue;
                }
                if is_ws(c) {
                    chars.push(' ');
                    continue;
                }
            }
            if self.chinese && is_chinese(c) {
                chars.extend([' ', c, ' ']);
            } else {
                chars.push(c);
            }
        }
        if self.strip_accents {
            chars = unicode::nfd(chars);
            chars.retain(|&c| !unicode::is_mark_nonspacing(c));
        }
        let mut out = String::with_capacity(chars.len());
        for c in chars {
            if self.lowercase {
                out.extend(c.to_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }

    /// The BertPreTokenizer.
    pub fn pre_tokenize(text: &str) -> Vec<&str> {
        let mut words = Vec::new();
        for chunk in text.split(char::is_whitespace).filter(|s| !s.is_empty()) {
            let mut start = 0;
            for (i, c) in chunk.char_indices() {
                if is_bert_punct(c) {
                    if i > start {
                        words.push(&chunk[start..i]);
                    }
                    words.push(&chunk[i..i + c.len_utf8()]);
                    start = i + c.len_utf8();
                }
            }
            if start < chunk.len() {
                words.push(&chunk[start..]);
            }
        }
        words
    }

    /// WordPiece for one word.
    fn word_ids(&self, word: &str, out: &mut Vec<u32>) {
        if word.chars().count() > self.max_chars {
            out.push(self.unk);
            return;
        }
        let mark = out.len();
        let mut start = 0;
        let mut probe = String::new();
        while start < word.len() {
            let mut end = word.len();
            let mut found = None;
            while start < end {
                probe.clear();
                if start > 0 {
                    probe.push_str(&self.prefix);
                }
                probe.push_str(&word[start..end]);
                if let Some(&id) = self.vocab.get(probe.as_str()) {
                    found = Some(id);
                    break;
                }
                end -= word[start..end].chars().next_back().map_or(1, char::len_utf8);
            }
            match found {
                Some(id) => out.push(id),
                None => {
                    out.truncate(mark);
                    out.push(self.unk);
                    return;
                }
            }
            start = end;
        }
    }

    fn encode_text(&self, text: &str, out: &mut Vec<u32>) {
        let normalized = self.normalize(text);
        let mut pieces = Vec::new();
        self.split_added(&normalized, true, &mut pieces);
        for p in pieces {
            match p {
                Piece::Id(id) => out.push(id),
                Piece::Text(t) => {
                    for w in Self::pre_tokenize(t) {
                        self.word_ids(w, out);
                    }
                }
            }
        }
    }

    /// The ids of `text` without `[CLS]` / `[SEP]`.
    pub fn tokenize(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        let mut pieces = Vec::new();
        self.split_added(text, false, &mut pieces);
        for p in pieces {
            match p {
                Piece::Id(id) => out.push(id),
                Piece::Text(t) => self.encode_text(t, &mut out),
            }
        }
        out
    }

    /// `[CLS] ids… [SEP]`, untruncated.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        self.encode_truncated(text, usize::MAX)
    }

    /// `[CLS] ids… [SEP]` with at most `max_len` (≥ 2) ids in all; tokens past the limit are
    /// dropped from the right.
    pub fn encode_truncated(&self, text: &str, max_len: usize) -> Vec<u32> {
        let mut ids = alloc::vec![self.cls];
        let mut body = self.tokenize(text);
        body.truncate(max_len.max(2) - 2);
        ids.extend(body);
        ids.push(self.sep);
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const VOCAB: &str = "[PAD]\n[UNK]\n[CLS]\n[SEP]\n[MASK]\nun\n##believ\n##able\nhello\n,\ncafe\n!\n##s\n中\n";

    #[test]
    fn normalises_and_splits() {
        let t = Tokenizer::from_vocab_txt(VOCAB).unwrap();
        assert_eq!(t.normalize("Café\u{200B}\tNAÏVE\u{0}中"), "cafe naive 中 ");
        assert_eq!(Tokenizer::pre_tokenize("hello,world!  x"), vec!["hello", ",", "world", "!", "x"]);
    }

    #[test]
    fn wordpiece_and_specials() {
        let t = Tokenizer::from_vocab_txt(VOCAB).unwrap();
        assert_eq!(t.encode("Unbelievable, hello xyz"), vec![2, 5, 6, 7, 9, 8, 1, 3]);
        assert_eq!(t.encode("CAFÉS!中"), vec![2, 10, 12, 11, 13, 3]);
        // Added tokens are found in the raw text, case-sensitively.
        assert_eq!(t.encode("hello[MASK]hello [mask]"), vec![2, 8, 4, 8, 1, 1, 1, 3]);
        assert_eq!(t.encode_truncated("hello hello hello", 3), vec![2, 8, 3]);
        assert_eq!(t.encode(""), vec![2, 3]);
        let long: String = core::iter::repeat_n('a', 101).collect();
        assert_eq!(t.encode(&long), vec![2, 1, 3]);
    }
}
