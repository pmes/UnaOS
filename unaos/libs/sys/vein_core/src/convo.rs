// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! LUMEN2 (LEDGER SR121) — the CONVERSATIONS SIDEBAR's model, no allocator: the index of the conversation
//! files [`crate::history`] defines (`<home>/.config/unaos/lumen/NNNN.md`), each with its title, the one that
//! is open, and the operations the sidebar runs — list (from a `PATH_R_LIST` directory listing or a sequence
//! probe), new (the next free number), switch, rename (the title), delete-to-trash (the path the file is
//! renamed to under the home's Trash, RING3RENAME's `PATH_W_RENAME`) and search (titles, case-insensitive).
//! The I/O is the caller's; everything here is pure and host-tested, and the kernel's `tests lumen` drives it.

use crate::history;
use crate::Out;

/// Conversations the index holds (the newest are kept when a home has more).
pub const MAX: usize = 256;
/// Title bytes kept per entry.
pub const TITLE_MAX: usize = 40;
/// The home's Trash directory name (una-abi `TRASH_DIR_NAME`; spelled here so this crate stays dependency-free).
pub const TRASH_DIR: &str = ".Trash";
/// The prefix a trashed conversation file carries in the Trash (so `0003.md` reads as Lumen's there).
pub const TRASH_PREFIX: &str = "lumen-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub seq: u16,
    pub title: [u8; TITLE_MAX],
    pub title_n: u8,
}

impl Entry {
    pub const EMPTY: Entry = Entry { seq: 0, title: [0; TITLE_MAX], title_n: 0 };
    pub fn title(&self) -> &[u8] {
        &self.title[..self.title_n as usize]
    }
}

/// The index: entries sorted by sequence, ascending (the sidebar draws newest first).
pub struct Index {
    e: [Entry; MAX],
    n: usize,
    /// The open conversation's sequence (0 = none yet).
    pub cur: u16,
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Index {
    pub const fn new() -> Self {
        Index { e: [Entry::EMPTY; MAX], n: 0, cur: 0 }
    }
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    pub fn clear(&mut self) {
        self.n = 0;
        self.cur = 0;
    }
    pub fn entries(&self) -> &[Entry] {
        &self.e[..self.n]
    }
    /// Entry `i` counted from the NEWEST (the sidebar's row order).
    pub fn newest(&self, i: usize) -> Option<&Entry> {
        if i < self.n { Some(&self.e[self.n - 1 - i]) } else { None }
    }
    pub fn position(&self, seq: u16) -> Option<usize> {
        self.e[..self.n].iter().position(|x| x.seq == seq)
    }
    pub fn contains(&self, seq: u16) -> bool {
        self.position(seq).is_some()
    }
    /// The highest sequence held (0 = none).
    pub fn max_seq(&self) -> u16 {
        if self.n == 0 { 0 } else { self.e[self.n - 1].seq }
    }
    /// The number a NEW conversation takes: one past the highest held (capped at [`history::SEQ_MAX`]).
    pub fn next_seq(&self) -> u16 {
        (self.max_seq() as u32 + 1).min(history::SEQ_MAX) as u16
    }
    /// Add `seq` (kept sorted; a duplicate is a no-op). A full index drops its OLDEST.
    pub fn insert(&mut self, seq: u16) {
        if seq == 0 || self.contains(seq) {
            return;
        }
        if self.n == MAX {
            if seq < self.e[0].seq {
                return;
            }
            self.e.copy_within(1..MAX, 0);
            self.n -= 1;
        }
        let at = self.e[..self.n].iter().position(|x| x.seq > seq).unwrap_or(self.n);
        self.e.copy_within(at..self.n, at + 1);
        self.e[at] = Entry { seq, ..Entry::EMPTY };
        self.n += 1;
    }
    /// Remove `seq`; true when it was held. Removing the open one leaves `cur` for the caller to move
    /// ([`Index::neighbour`]).
    pub fn remove(&mut self, seq: u16) -> bool {
        let Some(at) = self.position(seq) else { return false };
        self.e.copy_within(at + 1..self.n, at);
        self.n -= 1;
        true
    }
    /// The conversation to open after `seq` goes: the next newer, else the next older, else none.
    pub fn neighbour(&self, seq: u16) -> Option<u16> {
        self.e[..self.n].iter().find(|x| x.seq > seq).or_else(|| self.e[..self.n].iter().rev().find(|x| x.seq < seq)).map(|x| x.seq)
    }
    /// Set an entry's title (trimmed, control bytes dropped, cut at [`TITLE_MAX`]).
    pub fn set_title(&mut self, seq: u16, t: &[u8]) {
        let Some(at) = self.position(seq) else { return };
        let e = &mut self.e[at];
        let mut k = 0;
        for &c in t.trim_ascii() {
            if k == TITLE_MAX {
                break;
            }
            if c >= 0x20 && c != 0x7f {
                e.title[k] = c;
                k += 1;
            }
        }
        e.title_n = k as u8;
    }
    pub fn title(&self, seq: u16) -> &[u8] {
        self.position(seq).map_or(b"", |i| self.e[i].title())
    }
    /// Search: the sequences whose title (or number) contains `q` (ASCII case-insensitive), newest first, into
    /// `out`; returns how many. An empty query matches every entry.
    pub fn search(&self, q: &[u8], out: &mut [u16]) -> usize {
        let mut k = 0;
        for i in 0..self.n {
            if k == out.len() {
                break;
            }
            let e = &self.e[self.n - 1 - i];
            let mut num = [0u8; 5];
            let nn = dec(e.seq as u32, &mut num);
            if q.is_empty() || contains_ci(e.title(), q) || contains_ci(&num[..nn], q) {
                out[k] = e.seq;
                k += 1;
            }
        }
        k
    }
}

fn dec(mut v: u32, out: &mut [u8; 5]) -> usize {
    let mut d = [0u8; 5];
    let mut i = d.len();
    loop {
        i -= 1;
        d[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 || i == 0 {
            break;
        }
    }
    let n = d.len() - i;
    out[..n].copy_from_slice(&d[i..]);
    n
}

/// ASCII case-insensitive substring test (an empty needle is contained everywhere).
pub fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// The sequence a conversation file name carries: `0003.md` → 3 (exactly four digits, `.md`, not 0).
pub fn seq_of_name(name: &[u8]) -> Option<u16> {
    let d = name.strip_suffix(b".md")?;
    if d.len() != 4 || !d.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let v = d.iter().fold(0u16, |a, &c| a * 10 + (c - b'0') as u16);
    if v == 0 { None } else { Some(v) }
}

/// Walk a `PATH_R_LIST` listing (`\n`-joined names, a directory with a trailing `/`) and hand every
/// conversation file's sequence to `on`.
pub fn parse_listing(list: &[u8], on: &mut dyn FnMut(u16)) {
    for name in list.split(|&c| c == b'\n') {
        if name.ends_with(b"/") {
            continue;
        }
        if let Some(s) = seq_of_name(name.trim_ascii()) {
            on(s);
        }
    }
}

/// The absolute path a trashed conversation is renamed to: `<home>/.Trash/lumen-0003.md`, or with ` <n>`
/// before `.md` for the n-th collision (n ≥ 2), the Finder's collision shape. `None` when `out` is too small.
pub fn trash_path(home: &[u8], seq: u16, collision: u32, out: &mut [u8]) -> Option<usize> {
    let mut o = Out::new(out);
    o.put(home);
    if !home.ends_with(b"/") {
        o.put(b"/");
    }
    o.put(TRASH_DIR.as_bytes());
    o.put(b"/");
    o.put(TRASH_PREFIX.as_bytes());
    let mut rel = [0u8; history::PATH_LEN];
    let rn = history::path(seq as u32, &mut rel);
    let leaf = &rel[history::DIR.len() + 1..rn];
    let stem = &leaf[..leaf.len() - 3];
    o.put(stem);
    if collision >= 2 {
        o.put(b" ");
        o.dec(collision as u64);
    }
    o.put(b".md");
    o.done()
}

/// The conversation directory as an absolute path under `home` (for `PATH_R_LIST`).
pub fn dir_path(home: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut o = Out::new(out);
    o.put(home);
    if !home.ends_with(b"/") {
        o.put(b"/");
    }
    o.put(history::DIR.as_bytes());
    o.done()
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn new_switch_rename_trash_search() {
        let mut ix = Index::new();
        let mut seen = [0u16; 8];
        let mut k = 0;
        parse_listing(b"0002.md\n0001.md\nsub/\nnotes.txt\n0010.md\n0000.md\n00011.md\n", &mut |s| {
            seen[k] = s;
            k += 1;
        });
        assert_eq!(&seen[..k], &[2, 1, 10]);
        for &s in &seen[..k] {
            ix.insert(s);
        }
        ix.insert(2);
        assert_eq!(ix.entries().iter().map(|e| e.seq).collect::<std::vec::Vec<_>>(), [1, 2, 10]);
        assert_eq!(ix.next_seq(), 11);
        assert_eq!(ix.newest(0).unwrap().seq, 10);
        ix.set_title(2, b"  Rust \x01lifetimes question  ");
        ix.set_title(10, b"Shopping list");
        assert_eq!(ix.title(2), b"Rust lifetimes question");
        let mut hits = [0u16; 8];
        let n = ix.search(b"RUST", &mut hits);
        assert_eq!(&hits[..n], &[2]);
        let n = ix.search(b"", &mut hits);
        assert_eq!(&hits[..n], &[10, 2, 1]);
        let n = ix.search(b"10", &mut hits);
        assert_eq!(&hits[..n], &[10]);
        ix.cur = 2;
        assert!(ix.remove(2));
        assert_eq!(ix.neighbour(2), Some(10));
        assert_eq!(ix.neighbour(10), Some(1));
        let mut p = [0u8; 80];
        let n = trash_path(b"/home/peter", 3, 0, &mut p).unwrap();
        assert_eq!(&p[..n], b"/home/peter/.Trash/lumen-0003.md");
        let n = trash_path(b"/home/peter/", 3, 2, &mut p).unwrap();
        assert_eq!(&p[..n], b"/home/peter/.Trash/lumen-0003 2.md");
        let n = dir_path(b"/home/peter", &mut p).unwrap();
        assert_eq!(&p[..n], b"/home/peter/.config/unaos/lumen");
        assert_eq!(seq_of_name(b"9999.md"), Some(9999));
        assert_eq!(seq_of_name(b"12.md"), None);
    }

    #[test]
    fn a_full_index_keeps_the_newest() {
        let mut ix = Index::new();
        for s in 1..=(MAX as u16 + 5) {
            ix.insert(s);
        }
        assert_eq!(ix.len(), MAX);
        assert_eq!(ix.entries()[0].seq, 6);
        ix.insert(1);
        assert!(!ix.contains(1));
    }
}
