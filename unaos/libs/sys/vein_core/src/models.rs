// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! LUMEN2 (LEDGER SR121) — the MODEL TABLE: which dialect answers a model id and how large its context window
//! is, so the window can show `ctx used/limit` and pick the stream decoder from the conversation's model. ONE
//! table, longest-prefix match; a family prefix (`claude-`, `gemini-`) is the fallback row for an id the table
//! does not name. The figures are the providers' published context sizes as this table was written
//! (unverified against a live endpoint): this is the one place to correct them.

use crate::claude::Dialect;
use crate::Out;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    /// The id prefix the row matches.
    pub prefix: &'static str,
    pub dialect: Dialect,
    /// Context window, tokens (input + output). 0 = no limit known (the offline Echo).
    pub context: u32,
}

/// The rows, most specific first within a family (lookup takes the LONGEST matching prefix regardless).
pub const TABLE: &[ModelInfo] = &[
    ModelInfo { prefix: "claude-opus-5", dialect: Dialect::Claude, context: 200_000 },
    ModelInfo { prefix: "claude-fable-5", dialect: Dialect::Claude, context: 200_000 },
    ModelInfo { prefix: "claude-sonnet-5", dialect: Dialect::Claude, context: 200_000 },
    ModelInfo { prefix: "claude-haiku-4-5", dialect: Dialect::Claude, context: 200_000 },
    ModelInfo { prefix: "claude-", dialect: Dialect::Claude, context: 200_000 },
    ModelInfo { prefix: "gemini-2.5-pro", dialect: Dialect::Gemini, context: 1_048_576 },
    ModelInfo { prefix: "gemini-2.5-flash", dialect: Dialect::Gemini, context: 1_048_576 },
    ModelInfo { prefix: "gemini-", dialect: Dialect::Gemini, context: 1_048_576 },
    ModelInfo { prefix: "gemma-", dialect: Dialect::Gemini, context: 128_000 },
];

/// The row for `model` (longest prefix); an unknown id is a Claude id with no known limit.
pub fn lookup(model: &str) -> ModelInfo {
    let mut best: Option<&ModelInfo> = None;
    for r in TABLE {
        if model.starts_with(r.prefix) && best.is_none_or(|b| r.prefix.len() > b.prefix.len()) {
            best = Some(r);
        }
    }
    best.copied().unwrap_or(ModelInfo { prefix: "", dialect: Dialect::Claude, context: 0 })
}

/// A token estimate for `bytes` of text when the provider has not reported usage (about four bytes a token
/// for English prose and code; rounded up so a non-empty text is never zero).
pub fn estimate_tokens(bytes: usize) -> u32 {
    (bytes.div_ceil(4)).min(u32::MAX as usize) as u32
}

/// `n` compactly: `950`, `12.3k`, `200k`, `1.0M`.
pub fn compact(n: u32, o: &mut Out<'_>) {
    if n < 1000 {
        o.dec(n as u64);
    } else if n < 1_000_000 {
        let tenths = (n as u64 + 50) / 100; // k in tenths
        o.dec(tenths / 10);
        if tenths < 1000 && tenths % 10 != 0 {
            o.put(b".");
            o.dec(tenths % 10);
        }
        o.put(b"k");
    } else {
        let tenths = (n as u64 + 50_000) / 100_000;
        o.dec(tenths / 10);
        o.put(b".");
        o.dec(tenths % 10);
        o.put(b"M");
    }
}

/// The indicator: `ctx 12.3k/200k (6%)`, or `ctx 12.3k` when the limit is unknown.
pub fn indicator(used: u32, limit: u32, o: &mut Out<'_>) {
    o.put(b"ctx ");
    compact(used, o);
    if limit > 0 {
        o.put(b"/");
        compact(limit, o);
        o.put(b" (");
        o.dec((used as u64 * 100 / limit as u64).min(999));
        o.put(b"%)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    fn ind(u: u32, l: u32) -> std::string::String {
        let mut b = [0u8; 64];
        let mut o = Out::new(&mut b);
        indicator(u, l, &mut o);
        let n = o.done().unwrap();
        std::string::String::from_utf8(b[..n].to_vec()).unwrap()
    }

    #[test]
    fn table_and_indicator() {
        assert_eq!(lookup("claude-opus-5-5").context, 200_000);
        assert_eq!(lookup("gemini-2.5-pro-preview").dialect, Dialect::Gemini);
        assert_eq!(lookup("gemini-3-ultra").context, 1_048_576);
        assert_eq!(lookup("reverse").context, 0);
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(5), 2);
        assert_eq!(ind(950, 200_000), "ctx 950/200k (0%)");
        assert_eq!(ind(12_345, 200_000), "ctx 12.3k/200k (6%)");
        assert_eq!(ind(1_048_576, 0), "ctx 1.0M");
        assert_eq!(ind(150_000, 200_000), "ctx 150k/200k (75%)");
    }
}
