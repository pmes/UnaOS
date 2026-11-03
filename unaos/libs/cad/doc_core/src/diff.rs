// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The semantic diff (VUGDOCDIFF, LEDGER SR501; BP-vug-6): what changed between two documents in design terms — a
//! statement added, removed, a field of it changed, an ordered statement moved — never a line diff that shows a
//! reordered key or a last-digit write difference as a change.
//!
//! * Statements are matched by their text id ([`crate::Stmt::id`]); the id-less ones by a pseudo-id: `axis:<word>`,
//!   `tool:<number>`, `stock`, `fixture:<n>` (1-based among the fixtures); a key seen again in one document (a Doc
//!   built in code: [`crate::read`] refuses a duplicate id) is `<key>#2`, `<key>#3`... A renamed id is a Removed plus
//!   an Added (VUG-DOC-DIFF-RENAME owed).
//! * The fields of a statement come by ONE generic road: the writer's text of the statement (a fixed key order per
//!   verb) split into `key=value` items, so no variant has code of its own. Two values are the same when their text is,
//!   or token by token: brackets and strings equal, integer words equal as integers, other numbers within the
//!   documents' Tol (BP-vug-2: per component the coarser of the two; `ang` for the radian keys — arc `start` / `end`,
//!   revolve `angle`, the `angle` constraint's `angle`, tool `sig` — `lin` for every other).
//! * The header's Tol is compared exactly, as a Changed on the pseudo-id [`HEADER_ID`].
//! * Order is a change only where replay reads it: extrude, revolve and the Ops. The ids both documents hold, in the
//!   new order and mapped to their old position, keep their longest increasing run (patience sort); each other one is
//!   a [`Entry::Moved`] with its 1-based positions among the ordered statements.
//! * The entries: the header change, the Removed in old order, then per statement of the new document its Moved,
//!   its Changed, or its Added. [`Diff::text`] writes one line per change: `+ <statement>`, `- <statement>`,
//!   `~ <verb> <id> <key>: <old> -> <new>` (an absent side `(none)`), `> <verb> <id> moved <from> -> <to>`.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::write::stmt_text;
use crate::{AXES, Doc, FORMAT, Stmt, f64_text, word_of};

/// The pseudo-id of the header (its `lin` / `ang`).
pub const HEADER_ID: &str = FORMAT;

/// One field of a changed statement: its key and the value text on each side (`None`: the key is absent there).
/// The key `verb` names a changed statement verb.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// One change, keyed by the statement's id (or pseudo-id).
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A statement only the new document holds, as the writer writes it.
    Added { id: String, text: String },
    /// A statement only the old document holds, as the writer writes it.
    Removed { id: String, text: String },
    /// A statement both hold whose fields differ (beyond the Tol); `verb` is the new one's.
    Changed { id: String, verb: String, fields: Vec<Field> },
    /// An extrude / revolve / Op at another place in the replay order (1-based among those statements).
    Moved { id: String, verb: String, from: usize, to: usize },
}

/// The semantic diff of two documents.
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub entries: Vec<Entry>,
}

impl Diff {
    /// No change in design terms.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The diff as text, one LF-ended line per change, the same bytes for the same two documents.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            match e {
                Entry::Added { text, .. } => {
                    out.push_str("+ ");
                    out.push_str(text);
                    out.push('\n');
                }
                Entry::Removed { text, .. } => {
                    out.push_str("- ");
                    out.push_str(text);
                    out.push('\n');
                }
                Entry::Changed { id, verb, fields } => {
                    for f in fields {
                        out.push_str(&format!(
                            "~ {} {} {}: {} -> {}\n",
                            verb,
                            id,
                            f.key,
                            f.old.as_deref().unwrap_or("(none)"),
                            f.new.as_deref().unwrap_or("(none)")
                        ));
                    }
                }
                Entry::Moved { id, verb, from, to } => {
                    out.push_str(&format!("> {} {} moved {} -> {}\n", verb, id, from, to));
                }
            }
        }
        out
    }
}

/// The semantic diff from `old` to `new`.
pub fn diff(old: &Doc, new: &Doc) -> Diff {
    let mut entries = Vec::new();

    // The header: the Tol itself, compared exactly.
    let mut hf = Vec::new();
    if old.tol.lin != new.tol.lin {
        hf.push(Field { key: "lin".to_string(), old: Some(f64_text(old.tol.lin)), new: Some(f64_text(new.tol.lin)) });
    }
    if old.tol.ang != new.tol.ang {
        hf.push(Field { key: "ang".to_string(), old: Some(f64_text(old.tol.ang)), new: Some(f64_text(new.tol.ang)) });
    }
    if !hf.is_empty() {
        entries.push(Entry::Changed { id: HEADER_ID.to_string(), verb: FORMAT.to_string(), fields: hf });
    }

    // The coarser Tol per component: symmetric, so diff(a, b) and diff(b, a) agree on what is unchanged.
    let lin = if old.tol.lin > new.tol.lin { old.tol.lin } else { new.tol.lin };
    let ang = if old.tol.ang > new.tol.ang { old.tol.ang } else { new.tol.ang };

    let old_keys = keys(old);
    let new_keys = keys(new);
    let old_ix: BTreeMap<&str, usize> = old_keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let new_ix: BTreeMap<&str, usize> = new_keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let moved = moves(old, &old_keys, new, &new_keys);

    for (i, s) in old.stmts.iter().enumerate() {
        if !new_ix.contains_key(old_keys[i].as_str()) {
            entries.push(Entry::Removed { id: old_keys[i].clone(), text: stmt_text(s) });
        }
    }
    for (j, s) in new.stmts.iter().enumerate() {
        let key = &new_keys[j];
        let ntext = stmt_text(s);
        let Some(&i) = old_ix.get(key.as_str()) else {
            entries.push(Entry::Added { id: key.clone(), text: ntext });
            continue;
        };
        let (nverb, nitems) = split(&ntext);
        if let Some(&(from, to)) = moved.get(key.as_str()) {
            entries.push(Entry::Moved { id: key.clone(), verb: nverb.to_string(), from, to });
        }
        let otext = stmt_text(&old.stmts[i]);
        let (overb, oitems) = split(&otext);
        let fields = fields(overb, &oitems, nverb, &nitems, lin, ang);
        if !fields.is_empty() {
            entries.push(Entry::Changed { id: key.clone(), verb: nverb.to_string(), fields });
        }
    }
    Diff { entries }
}

/// Every statement's match key, in document order: its id or pseudo-id, `#n` from the second occurrence on.
fn keys(doc: &Doc) -> Vec<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut fixtures = 0usize;
    let mut out = Vec::with_capacity(doc.stmts.len());
    for s in &doc.stmts {
        let base = match s.id() {
            Some(id) => id.to_string(),
            None => match s {
                Stmt::Axis(a) => format!("axis:{}", word_of(&AXES, a.id)),
                Stmt::Tool(t) => format!("tool:{}", t.number),
                Stmt::Fixture(_) => {
                    fixtures += 1;
                    format!("fixture:{}", fixtures)
                }
                _ => "stock".to_string(),
            },
        };
        let n = seen.entry(base.clone()).or_insert(0);
        *n += 1;
        out.push(if *n == 1 { base } else { format!("{}#{}", base, n) });
    }
    out
}

/// The statements whose order replay reads: the features and the Ops.
fn ordered(s: &Stmt) -> bool {
    matches!(s, Stmt::Extrude { .. } | Stmt::Revolve { .. } | Stmt::Op { .. })
}

/// The ordered statements both documents hold that left the longest increasing run: key -> (old, new) 1-based
/// positions among the ordered statements.
fn moves<'k>(old: &Doc, old_keys: &[String], new: &Doc, new_keys: &'k [String]) -> BTreeMap<&'k str, (usize, usize)> {
    let mut old_pos: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, s) in old.stmts.iter().enumerate() {
        if ordered(s) {
            let n = old_pos.len() + 1;
            old_pos.insert(old_keys[i].as_str(), n);
        }
    }
    // (key, old position, new position) for the common ordered ids, in new order.
    let mut seq: Vec<(&'k str, usize, usize)> = Vec::new();
    let mut new_n = 0usize;
    for (j, s) in new.stmts.iter().enumerate() {
        if ordered(s) {
            new_n += 1;
            if let Some(&from) = old_pos.get(new_keys[j].as_str()) {
                seq.push((new_keys[j].as_str(), from, new_n));
            }
        }
    }
    // Patience sort on the old positions (distinct): tails[k] is the index in `seq` of the smallest tail of a run of
    // length k + 1; prev links rebuild the run that ends at the last tail.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = Vec::with_capacity(seq.len());
    for i in 0..seq.len() {
        let v = seq[i].1;
        let k = tails.partition_point(|&t| seq[t].1 < v);
        prev.push(if k > 0 { Some(tails[k - 1]) } else { None });
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut keep = alloc::vec![false; seq.len()];
    let mut at = tails.last().copied();
    while let Some(i) = at {
        keep[i] = true;
        at = prev[i];
    }
    let mut out = BTreeMap::new();
    for (i, &(key, from, to)) in seq.iter().enumerate() {
        if !keep[i] {
            out.insert(key, (from, to));
        }
    }
    out
}

/// The writer's text of one statement split at the top level (outside strings and brackets): the verb, then the
/// `key=value` items in the writer's order (the id token, which has no `=`, is skipped: the key carries it).
fn split(text: &str) -> (&str, Vec<(&str, &str)>) {
    let mut toks: Vec<&str> = Vec::new();
    let b = text.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut st = 0usize;
    for (i, &c) in b.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'[' => depth += 1,
            b']' => depth -= 1,
            b' ' if depth == 0 => {
                if i > st {
                    toks.push(&text[st..i]);
                }
                st = i + 1;
            }
            _ => {}
        }
    }
    if text.len() > st {
        toks.push(&text[st..]);
    }
    let verb = toks.first().copied().unwrap_or("");
    let mut items = Vec::new();
    for &t in toks.iter().skip(1) {
        if let Some(eq) = t.find('=') {
            items.push((&t[..eq], &t[eq + 1..]));
        }
    }
    (verb, items)
}

/// The field changes of one statement: the verb, then the new keys in the new order, then the old-only keys.
fn fields(overb: &str, oitems: &[(&str, &str)], nverb: &str, nitems: &[(&str, &str)], lin: f64, ang: f64) -> Vec<Field> {
    let mut out = Vec::new();
    if overb != nverb {
        out.push(Field { key: "verb".to_string(), old: Some(overb.to_string()), new: Some(nverb.to_string()) });
    }
    for &(k, nv) in nitems {
        match oitems.iter().find(|(ok, _)| *ok == k) {
            Some(&(_, ov)) => {
                let tol = if angle_key(nverb, k) { ang } else { lin };
                if !same_value(ov, nv, tol) {
                    out.push(Field { key: k.to_string(), old: Some(ov.to_string()), new: Some(nv.to_string()) });
                }
            }
            None => out.push(Field { key: k.to_string(), old: None, new: Some(nv.to_string()) }),
        }
    }
    for &(k, ov) in oitems {
        if !nitems.iter().any(|(nk, _)| *nk == k) {
            out.push(Field { key: k.to_string(), old: Some(ov.to_string()), new: None });
        }
    }
    out
}

/// The keys whose numbers are radians (measured from the writer and make_core's `Tool::sig`).
fn angle_key(verb: &str, key: &str) -> bool {
    matches!((verb, key), ("arc", "start") | ("arc", "end") | ("revolve", "angle") | ("angle", "angle") | ("tool", "sig"))
}

/// A value's tokens: `[`, `]`, a whole string with its quotes, or a word.
fn value_tokens(v: &str) -> Vec<&str> {
    let b = v.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b' ' => i += 1,
            b'[' | b']' => {
                out.push(&v[i..i + 1]);
                i += 1;
            }
            b'"' => {
                let st = i;
                i += 1;
                let mut esc = false;
                while i < b.len() {
                    let c = b[i];
                    i += 1;
                    if esc {
                        esc = false;
                    } else if c == b'\\' {
                        esc = true;
                    } else if c == b'"' {
                        break;
                    }
                }
                out.push(&v[st..i]);
            }
            _ => {
                let st = i;
                while i < b.len() && !matches!(b[i], b' ' | b'[' | b']' | b'"') {
                    i += 1;
                }
                out.push(&v[st..i]);
            }
        }
    }
    out
}

/// Two value texts the same in design terms (see the module doc).
fn same_value(a: &str, b: &str, tol: f64) -> bool {
    if a == b {
        return true;
    }
    let ta = value_tokens(a);
    let tb = value_tokens(b);
    if ta.len() != tb.len() {
        return false;
    }
    ta.iter().zip(tb.iter()).all(|(x, y)| same_token(x, y, tol))
}

fn same_token(x: &str, y: &str, tol: f64) -> bool {
    if x == y {
        return true;
    }
    if x.starts_with('"') || y.starts_with('"') || x == "[" || x == "]" || y == "[" || y == "]" {
        return false;
    }
    if let (Ok(p), Ok(q)) = (x.parse::<i64>(), y.parse::<i64>()) {
        return p == q;
    }
    match (x.parse::<f64>(), y.parse::<f64>()) {
        (Ok(p), Ok(q)) => {
            let d = p - q;
            let d = if d < 0.0 { -d } else { d };
            d <= tol
        }
        _ => false,
    }
}
