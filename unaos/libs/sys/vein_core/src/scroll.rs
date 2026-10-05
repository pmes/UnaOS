// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! LUMENUX M1 (rmbp-ledger B348) — the scrollback: a RING of rendered rows over a transcript the caller
//! owns. A row is a 12-byte [`Rec`] (where its source line starts in the transcript, which slice of that
//! line's rendered text it shows, and how to draw it), so the ring costs 12 bytes a row and the window
//! re-renders only the rows it shows. When the ring is full the OLDEST row drops (the transcript still holds
//! the text; it is just no longer scrollable to). The caller relays incrementally: [`Ring::truncate_from_src`]
//! drops the rows of the line still streaming, and layout resumes there.

/// The rows LUMEN.ELF's ring keeps (12 bytes each: 1.5 MiB of its 4 MiB ELF window — LUMENUX.md M1 measures
/// the fit). Here so `tests lumen` reports the same number the program allocates.
pub const LUMEN_ROWS: usize = 128 * 1024;

/// One rendered row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Rec {
    /// Offset of the row's SOURCE line in the transcript.
    pub src: u32,
    /// The row's slice of that line's rendered text.
    pub dofs: u16,
    pub len: u16,
    /// The turn's role tag (the caller's numbering; 0 = the blank row after a turn).
    pub role: u8,
    /// `md::Block` as u8.
    pub block: u8,
    /// Columns this row is indented by (a wrapped list item's hang).
    pub hang: u8,
    /// [`F_FENCE`] | [`F_TURN`] | [`F_LEAD`].
    pub flags: u8,
}

/// The fence was open BEFORE this row's source line (the state to render the line with).
pub const F_FENCE: u8 = 1;
/// The first row of a turn.
pub const F_TURN: u8 = 2;
/// The first row of a source line.
pub const F_LEAD: u8 = 4;

impl Rec {
    pub const ZERO: Rec = Rec { src: 0, dofs: 0, len: 0, role: 0, block: 0, hang: 0, flags: 0 };
}

const _: () = assert!(core::mem::size_of::<Rec>() == 12);

/// The ring over a caller-owned slice of [`Rec`].
pub struct Ring<'a> {
    recs: &'a mut [Rec],
    head: usize,
    len: usize,
    /// Rows dropped off the old end since the last [`Ring::clear`].
    pub dropped: u64,
}

impl<'a> Ring<'a> {
    pub const fn new(recs: &'a mut [Rec]) -> Self {
        Ring { recs, head: 0, len: 0, dropped: 0 }
    }
    pub fn cap(&self) -> usize {
        self.recs.len()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.dropped = 0;
    }
    /// Append a row; a full ring drops its oldest.
    pub fn push(&mut self, r: Rec) {
        let cap = self.recs.len();
        if cap == 0 {
            return;
        }
        if self.len == cap {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
            self.dropped += 1;
        }
        self.recs[(self.head + self.len) % cap] = r;
        self.len += 1;
    }
    /// Row `i`, 0 = the oldest kept.
    pub fn get(&self, i: usize) -> Option<Rec> {
        if i >= self.len {
            return None;
        }
        Some(self.recs[(self.head + i) % self.recs.len()])
    }
    /// The newest row.
    pub fn last(&self) -> Option<Rec> {
        if self.len == 0 { None } else { self.get(self.len - 1) }
    }
    /// Drop every row whose source line starts at or after `src` (newest first); returns how many rows
    /// remain. The relayout of a streaming line resumes from there.
    pub fn truncate_from_src(&mut self, src: u32) -> usize {
        while self.len > 0 && self.get(self.len - 1).is_some_and(|r| r.src >= src) {
            self.len -= 1;
        }
        self.len
    }
    /// The transcript dropped its first `cut` bytes: drop the rows that pointed into them and rebase the rest.
    pub fn rebase(&mut self, cut: u32) {
        while self.len > 0 && self.get(0).is_some_and(|r| r.src < cut) {
            self.head = (self.head + 1) % self.recs.len();
            self.len -= 1;
        }
        let cap = self.recs.len();
        for i in 0..self.len {
            self.recs[(self.head + i) % cap].src -= cut;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(src: u32) -> Rec {
        Rec { src, ..Rec::default() }
    }

    #[test]
    fn ring_drops_oldest_truncates_and_rebases() {
        let mut store = [Rec::default(); 4];
        let mut r = Ring::new(&mut store);
        for s in [0, 10, 10, 20, 30] {
            r.push(rec(s));
        }
        assert_eq!((r.len(), r.dropped, r.get(0).unwrap().src, r.last().unwrap().src), (4, 1, 10, 30));
        assert_eq!(r.truncate_from_src(20), 2);
        r.push(rec(25));
        assert_eq!(r.last().unwrap().src, 25);
        r.rebase(15);
        assert_eq!((r.len(), r.get(0).unwrap().src), (1, 10));
        r.clear();
        assert!(r.is_empty() && r.get(0).is_none());
    }
}
