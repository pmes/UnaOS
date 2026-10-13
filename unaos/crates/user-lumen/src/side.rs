// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Vein — the Lumen window (ring 3); the index is vein_core::convo (shared-core).
//
// LUMEN2 (LEDGER SR121) — the CONVERSATIONS SIDEBAR: a panel over the transcript's left edge (Ctrl-B, or the
// footer's [chats]) listing the conversation files newest first — the open one marked, its row carrying [del]
// (delete-to-trash, a second click within three seconds confirms) — a [+ new] button, and the FIND row Ctrl-K
// opens: what is typed filters the titles live (vein_core::convo::Index::search), Up/Down pick, Enter opens,
// Ctrl-K again searches the files' text, Esc closes. The geometry here is the ONE both the painter and the
// pointer's hit test read, so a row is clicked where it is drawn.

use vein_core::convo;

/// The panel's width, px (the window is at most 288 wide).
pub const SIDE_W: i32 = 132;
/// Hits a search keeps.
pub const HITS: usize = 64;
/// A [del] click is confirmed by a second one within this long.
pub const DEL_CONFIRM_MS: u64 = 3000;

pub struct Search {
    pub q: [u8; 40],
    pub n: usize,
    pub hits: [u16; HITS],
    pub hn: usize,
    pub sel: usize,
    /// The hits came from the files' TEXT (Ctrl-K pressed again), not the titles.
    pub deep: bool,
}

impl Search {
    pub const fn new() -> Search {
        Search { q: [0; 40], n: 0, hits: [0; HITS], hn: 0, sel: 0, deep: false }
    }
    pub fn query(&self) -> &[u8] {
        &self.q[..self.n]
    }
    /// Re-filter by title.
    pub fn refilter(&mut self) {
        self.deep = false;
        self.hn = index().search(&self.q[..self.n], &mut self.hits);
        self.sel = self.sel.min(self.hn.saturating_sub(1));
    }
}

pub struct Side {
    pub open: bool,
    /// The panel was opened by Ctrl-K (closing the search closes it too).
    pub by_search: bool,
    /// Rows scrolled down the list.
    pub scroll: usize,
    pub search: Option<Search>,
    /// A [del] armed for this sequence until this time.
    pub del_armed: Option<(u16, u64)>,
}

impl Side {
    pub const fn new() -> Side {
        Side { open: false, by_search: false, scroll: 0, search: None, del_armed: None }
    }
    /// How many entries the list shows (the hits while searching).
    pub fn count(&self) -> usize {
        match &self.search {
            Some(s) => s.hn,
            None => index().len(),
        }
    }
    /// The sequence on list row `i` (0 = the top row, after the scroll).
    pub fn seq_at(&self, i: usize) -> Option<u16> {
        let k = i + self.scroll;
        match &self.search {
            Some(s) => s.hits[..s.hn].get(k).copied(),
            None => index().newest(k).map(|e| e.seq),
        }
    }
    pub fn scroll_by(&mut self, d: i32, rows: usize) {
        let max = self.count().saturating_sub(rows.max(1)) as i32;
        self.scroll = (self.scroll as i32 - d).clamp(0, max.max(0)) as usize;
    }
}

static mut INDEX: convo::Index = convo::Index::new();

/// The conversations the home holds (single-threaded, as the window).
pub fn index() -> &'static mut convo::Index {
    unsafe { &mut *core::ptr::addr_of_mut!(INDEX) }
}

/// The panel's geometry for a line box `lh` and a list area ending at `bottom`.
#[derive(Clone, Copy)]
pub struct Geom {
    pub lh: i32,
    /// The header row's top.
    pub y0: i32,
    /// The first list row's top.
    pub ey: i32,
    pub rows: usize,
}

pub fn geom(lh: i32, bottom: i32) -> Geom {
    let y0 = 3;
    let ey = y0 + lh + 4;
    let rows = ((bottom - ey) / lh).max(0) as usize;
    Geom { lh, y0, ey, rows }
}

/// What a point on the panel is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    New,
    Find,
    Row(u16),
    Del(u16),
    Panel,
}

/// The [+ new] button's left edge and the [del] marks' left edge (right-aligned in the panel).
pub fn new_x() -> i32 {
    SIDE_W - 6 - crate::width(b"+ new", crate::txt::Kind::Sans) as i32
}
pub fn del_x() -> i32 {
    SIDE_W - 6 - crate::width(b"del?", crate::txt::Kind::Sans) as i32
}

pub fn spot(side: &Side, g: &Geom, x: i32, y: i32) -> Spot {
    if y < g.ey - 2 {
        return if side.search.is_some() { Spot::Find } else if x >= new_x() - 2 { Spot::New } else { Spot::Panel };
    }
    let i = ((y - g.ey) / g.lh) as usize;
    if y < g.ey || i >= g.rows {
        return Spot::Panel;
    }
    match side.seq_at(i) {
        Some(seq) if seq == index().cur && side.search.is_none() && x >= del_x() - 2 => Spot::Del(seq),
        Some(seq) => Spot::Row(seq),
        None => Spot::Panel,
    }
}
