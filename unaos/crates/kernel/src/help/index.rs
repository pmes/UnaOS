//! CHARTER: Kernel — wm (HELPSEARCH B729: the in-OS manual's reader; no CODEX handler owns help, the glass shows it)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HELPSEARCH (rmbp-ledger B729; BP-bar-2/3, research C §8: help is in the OS, searchable, offline, written for the
//! person at the glass) — the ONE help index's reader, `no_std`, a child of `help` (no `lib.rs` line).
//!
//! * THE CORPUS is `docs/help/*.md` in the tree; the builder (`builder/src/main.rs` `pack_help`) packs it at build
//!   time into ONE file, `HELP.IDX`, staged as `<volume>/system/help/HELP.IDX` beside the fonts. [`parse`] is the
//!   reader of that exact format (the packer's doc comment is the spec): a header line, one `P` row per page, one
//!   `W` row per word (sorted, `*` = the word is in the page's title), `B`, then the bodies back to back.
//! * [`ensure_loaded`] reads it ONCE through the mount table (`/system/help`, else `/boot/system/help`, the fonts'
//!   two homes) — called from the desktop service pass (the Launcher's snapshot) and the fixture, never from an input
//!   door (R88). `[help] index pages=<n> words=<n> bytes=<n> src=<path>` once; a missing file is said once.
//! * [`search_pages`] ranks pages for a query: every query word must be found (an index word it equals, or starts
//!   with when it is 3 bytes or longer); a title word scores over a body word; the whole query inside the title scores
//!   most. [`search_verbs`] ranks `help::DOCS` (every shell verb has a row): the verb's name, then its summary.
//!   Settings rows are the third source, ranked over `settings::MAP` by the Help window (`help::window`), which is
//!   where the desktop seam lives.

use alloc::string::String;
use alloc::vec::Vec;

/// One page of the corpus.
pub struct Page {
    pub slug: String,
    pub title: String,
    /// The program whose F1 opens this page (`desktop`, `quarry`, `settings`, `shell`), or empty.
    pub app: String,
    /// The Principia keys (`system.` stripped) the page's "Open the setting" links name.
    pub keys: Vec<String>,
    pub body: String,
}

/// The parsed index.
pub struct Index {
    pub pages: Vec<Page>,
    /// `(word, [(page, in_title)])`, sorted by word (the packer's order).
    pub words: Vec<(String, Vec<(u16, bool)>)>,
    pub bytes: usize,
}

/// The two homes of the staged file, in order.
pub const PATHS: [&str; 2] = ["/system/help/HELP.IDX", "/boot/system/help/HELP.IDX"];
/// The largest index read (the corpus is at most 40 short pages).
const MAX_BYTES: usize = 512 * 1024;

/// Parse `HELP.IDX` bytes. `Err` names the first thing that is not the format. Pure.
pub fn parse(b: &[u8]) -> Result<Index, &'static str> {
    let text = core::str::from_utf8(b).map_err(|_| "not utf-8")?;
    let mut lines = text.split('\n');
    let head = lines.next().ok_or("empty")?;
    let mut hw = head.split(' ');
    if hw.next() != Some("UNAHELP") || hw.next() != Some("1") {
        return Err("bad header");
    }
    let num = |p: &str, k: &str| p.strip_prefix(k).and_then(|v| v.parse::<usize>().ok());
    let n_pages = hw.next().and_then(|p| num(p, "pages=")).ok_or("no pages=")?;
    let n_words = hw.next().and_then(|p| num(p, "words=")).ok_or("no words=")?;
    let mut rows: Vec<(Page, usize, usize)> = Vec::new();
    let mut words: Vec<(String, Vec<(u16, bool)>)> = Vec::new();
    let mut consumed = head.len() + 1;
    let mut body_at = None;
    for l in lines.by_ref() {
        consumed += l.len() + 1;
        if l == "B" {
            body_at = Some(consumed);
            break;
        }
        let mut f = l.split('\t');
        match f.next() {
            Some("P") => {
                let (slug, title, app, keys) = (f.next().ok_or("P slug")?, f.next().ok_or("P title")?, f.next().ok_or("P app")?, f.next().ok_or("P keys")?);
                let off = f.next().and_then(|v| v.parse::<usize>().ok()).ok_or("P off")?;
                let len = f.next().and_then(|v| v.parse::<usize>().ok()).ok_or("P len")?;
                let keys = keys.split(',').filter(|k| !k.is_empty()).map(String::from).collect();
                rows.push((Page { slug: String::from(slug), title: String::from(title), app: String::from(app), keys, body: String::new() }, off, len));
            }
            Some("W") => {
                let w = f.next().ok_or("W word")?;
                let mut list = Vec::new();
                for e in f.next().ok_or("W list")?.split(' ').filter(|e| !e.is_empty()) {
                    let (n, t) = match e.strip_suffix('*') { Some(n) => (n, true), None => (e, false) };
                    let p = n.parse::<u16>().map_err(|_| "W page")?;
                    if p as usize >= n_pages {
                        return Err("W page out of range");
                    }
                    list.push((p, t));
                }
                words.push((String::from(w), list));
            }
            _ => return Err("bad row"),
        }
    }
    let base = body_at.ok_or("no B")?;
    if rows.len() != n_pages || words.len() != n_words {
        return Err("counts differ from the header");
    }
    let bodies = text.get(base.min(text.len())..).unwrap_or("");
    let mut pages = Vec::with_capacity(rows.len());
    for (mut p, off, len) in rows {
        p.body = String::from(bodies.get(off..off.saturating_add(len)).ok_or("body out of range")?);
        pages.push(p);
    }
    Ok(Index { pages, words, bytes: b.len() })
}

/// The words the packer leaves out (`builder/src/main.rs` `HELP_STOP`, the same list).
const STOP: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "has", "have", "how", "i", "if", "in", "is", "it",
    "its", "my", "no", "not", "of", "on", "or", "so", "that", "the", "then", "this", "to", "up", "was", "what", "when", "where", "which",
    "with", "you", "your",
];

/// The query's words, as the packer cut the corpus: lowercase ASCII letter/digit runs of 2 bytes or more, stop words out.
pub fn words_of(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_ascii_alphanumeric())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| w.len() >= 2 && !STOP.contains(&w.as_str()))
        .collect()
}

/// ASCII case-insensitive `needle` inside `hay`.
pub fn holds_ci(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    !n.is_empty() && h.len() >= n.len() && (0..=h.len() - n.len()).any(|i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

impl Index {
    /// Pages for `q`, best first: `(page, score)`. Every query word must be found. Pure.
    pub fn search_pages(&self, q: &str) -> Vec<(usize, i32)> {
        let qw = words_of(q);
        if qw.is_empty() {
            return Vec::new();
        }
        let mut score: Vec<i32> = alloc::vec![0; self.pages.len()];
        let mut found: Vec<usize> = alloc::vec![0; self.pages.len()];
        for t in &qw {
            // The index words equal to `t`, or (t of 3 bytes or more) starting with it: one sorted range.
            let lo = self.words.partition_point(|(w, _)| w.as_str() < t.as_str());
            let mut best: Vec<i32> = alloc::vec![0; self.pages.len()];
            for (w, list) in self.words[lo..].iter() {
                if !w.starts_with(t.as_str()) {
                    break;
                }
                let exact = w == t;
                if !exact && t.len() < 3 {
                    continue;
                }
                for &(p, title) in list {
                    let s = if title { 10 } else { 3 } + exact as i32;
                    let b = &mut best[p as usize];
                    *b = (*b).max(s);
                }
            }
            for (p, b) in best.iter().enumerate() {
                if *b > 0 {
                    score[p] += b;
                    found[p] += 1;
                }
            }
        }
        let mut out: Vec<(usize, i32)> = (0..self.pages.len())
            .filter(|&p| found[p] == qw.len())
            .map(|p| (p, score[p] + if holds_ci(&self.pages[p].title, q.trim()) { 20 } else { 0 }))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    /// The page named `slug`.
    pub fn page(&self, slug: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.slug == slug)
    }
}

/// Shell verbs for `q`, best first: `(row of help::DOCS, score)` — the name equal (100), the name starting with a
/// query of 2 bytes or more (60), every query word (3 bytes or more) in the summary (20). One row per verb. Pure.
pub fn search_verbs(q: &str) -> Vec<(usize, i32)> {
    let q = q.trim();
    let qw: Vec<String> = words_of(q).into_iter().filter(|w| w.len() >= 3).collect();
    let mut out: Vec<(usize, i32)> = Vec::new();
    for (i, d) in super::DOCS.iter().enumerate() {
        let s = if d.name.eq_ignore_ascii_case(q) {
            100
        } else if q.len() >= 2 && !q.contains(' ') && d.name.len() >= q.len() && d.name.as_bytes()[..q.len()].eq_ignore_ascii_case(q.as_bytes()) {
            60
        } else if !qw.is_empty() && qw.iter().all(|w| holds_ci(d.summary, w)) {
            20
        } else {
            continue;
        };
        if !out.iter().any(|(j, _)| super::DOCS[*j].name == d.name) {
            out.push((i, s));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The loaded index (`None` until [`ensure_loaded`] found it).
static INDEX: crate::sync::Mutex<Option<Index>> = crate::sync::Mutex::new(None);
/// Tried once per boot (a missing file is said once, not per keystroke).
static TRIED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Read and parse `HELP.IDX` once (the service pass, never an input door). `true` when an index is loaded.
pub fn ensure_loaded() -> bool {
    if INDEX.lock().is_some() {
        return true;
    }
    if TRIED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return false;
    }
    let mt = crate::shell::vfs_mount_table();
    for p in PATHS {
        let Ok(st) = mt.stat(p) else { continue };
        let n = st.size as usize;
        if n == 0 || n > MAX_BYTES {
            serial_println!("[help] index REFUSED src={} bytes={} reason=size", p, n);
            continue;
        }
        let mut v: Vec<u8> = Vec::new();
        if v.try_reserve_exact(n).is_err() {
            serial_println!("[help] index REFUSED src={} reason=heap", p);
            return false;
        }
        while v.len() < n {
            match mt.read(p, v.len() as u64, (n - v.len()).min(64 * 1024)) {
                Ok(b) if !b.is_empty() => v.extend_from_slice(&b[..b.len().min(n - v.len())]),
                _ => break,
            }
        }
        if v.len() != n {
            serial_println!("[help] index REFUSED src={} read={}/{} reason=short", p, v.len(), n);
            continue;
        }
        match parse(&v) {
            Ok(ix) => {
                serial_println!("[help] index pages={} words={} bytes={} src={}", ix.pages.len(), ix.words.len(), ix.bytes, p);
                *INDEX.lock() = Some(ix);
                return true;
            }
            Err(e) => serial_println!("[help] index REFUSED src={} reason={}", p, e),
        }
    }
    serial_println!("[help] index absent paths={},{} (the builder stages docs/help as HELP.IDX)", PATHS[0], PATHS[1]);
    false
}

/// Run `f` over the loaded index (`None` = not loaded). The lock is held for `f` only.
pub fn with<R>(f: impl FnOnce(&Index) -> R) -> Option<R> {
    INDEX.lock().as_ref().map(f)
}

/// Forget a failed try so the next [`ensure_loaded`] reads again (the fixture's retry after a volume bound late).
pub fn retry() {
    TRIED.store(false, core::sync::atomic::Ordering::Release);
}
