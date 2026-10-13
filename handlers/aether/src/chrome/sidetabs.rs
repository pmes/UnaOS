//! AETHERSIDETABS (SR140, R118): the VERTICAL tab strip (Chrome's side tabs) — the tab layout preference, the side
//! column's geometry, and Ctrl+Shift+A's tab search. std-free (core + alloc), like the rest of the chrome model, so
//! the UnaOS face can link it by `#[path]` beside `tabs.rs` (it names only `super::tabs`).
//!
//! The layout is Principia's `aether.tabs.layout` (`top` | `side`, default `top`; prefs_core `schema.rs`), read at
//! face build and again on a `PrefChanged` for it. Switching it re-lays the chrome only: the [`TabStrip`] and every
//! tab's engine stay as they are.
//!
//! The side column is [`SIDE_W`] px wide, or a [`RAIL_W`] px icon rail when collapsed (one control, the top row's
//! toggle). Below the toggle it lists [`TabStrip::rows`] one per [`ROW_H`] px — a group's header, its tabs (the
//! favicon or the letter tile, the ellipsised title, the close box on hover), the `+` row last — and scrolls when
//! the list is taller than the window. The page sits right of the column ([`SideStrip::width`]).

use alloc::vec::Vec;

use super::tabs::{GroupId, Row, TabStrip};

/// Principia's namespace and key for the layout.
pub const PREF_NS: &str = "aether";
pub const PREF_KEY: &str = "tabs.layout";

/// Where the tabs are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabLayout {
    /// The horizontal strip across the top (the default).
    #[default]
    Top,
    /// The vertical strip on the left.
    Side,
}

impl TabLayout {
    /// The stored value (`top` | `side`); anything else is the default.
    pub fn parse(s: &str) -> TabLayout {
        if s.trim().eq_ignore_ascii_case("side") {
            TabLayout::Side
        } else {
            TabLayout::Top
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TabLayout::Top => "top",
            TabLayout::Side => "side",
        }
    }
}

/// The side column's width, open and collapsed to the icon rail, and one row's height (px at scale 1).
pub const SIDE_W: u32 = 240;
pub const RAIL_W: u32 = 48;
pub const ROW_H: u32 = 32;
/// The close box's width at a tab row's right edge.
pub const CLOSE_W: u32 = 24;
/// The group header's colour stripe at a row's left edge.
pub const STRIPE_W: u32 = 4;

/// Whether a key press is the tab search: Ctrl+Shift+A (`key` as the face names it, `A` or `a`).
pub fn is_tab_search_key(key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    matches!(key, "a" | "A") && ctrl && shift && !alt
}

/// A place in the side column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideHit {
    /// The collapse / expand toggle (the column's top row).
    Toggle,
    Tab(u64),
    Close(u64),
    /// A group's header: a click collapses or expands it.
    Group(GroupId),
    NewTab,
}

/// One laid-out row: what it is and its top edge in column px (the toggle row and the scroll already applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowRect {
    pub row: Row,
    pub y: i32,
}

/// The side column's state for one window (std-free; the face keeps one and paints what [`SideStrip::layout`]
/// says).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideStrip {
    /// Collapsed to the icon rail.
    pub collapsed: bool,
    /// How far the list is scrolled, px (0 = the first row under the toggle).
    pub scroll: i32,
}

impl SideStrip {
    /// The column's width: what the page gives up on its left.
    pub fn width(&self) -> u32 {
        if self.collapsed { RAIL_W } else { SIDE_W }
    }

    /// Collapse to the rail or open again; answers whether it is now collapsed.
    pub fn toggle(&mut self) -> bool {
        self.collapsed = !self.collapsed;
        self.collapsed
    }

    /// The list's full height and the height it has under the toggle row in a column `h` px tall.
    fn extents(strip: &TabStrip, h: u32) -> (i32, i32) {
        let total = (strip.rows().len() as u32 * ROW_H) as i32;
        (total, h.saturating_sub(ROW_H) as i32)
    }

    /// The furthest the list can scroll in a column `h` px tall.
    pub fn max_scroll(strip: &TabStrip, h: u32) -> i32 {
        let (total, room) = Self::extents(strip, h);
        (total - room).max(0)
    }

    /// Scroll by `dy` px (positive: further down the list), clamped.
    pub fn scroll_by(&mut self, strip: &TabStrip, h: u32, dy: i32) {
        self.scroll = (self.scroll + dy).clamp(0, Self::max_scroll(strip, h));
    }

    /// Scroll just enough that tab `id`'s row is on view (the active tab after Ctrl+Tab, a search pick).
    pub fn reveal(&mut self, strip: &TabStrip, h: u32, id: u64) {
        let Some(k) = strip.rows().iter().position(|r| *r == Row::Tab(id)) else { return };
        let (_, room) = Self::extents(strip, h);
        let top = (k as u32 * ROW_H) as i32;
        let bottom = top + ROW_H as i32;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + room {
            self.scroll = bottom - room;
        }
        self.scroll = self.scroll.clamp(0, Self::max_scroll(strip, h));
    }

    /// The rows on view in a column `h` px tall: each with its top edge (the toggle row is `0..ROW_H`).
    pub fn layout(&self, strip: &TabStrip, h: u32) -> Vec<RowRect> {
        let scroll = self.scroll.clamp(0, Self::max_scroll(strip, h));
        let top = ROW_H as i32;
        strip
            .rows()
            .into_iter()
            .enumerate()
            .map(|(k, row)| RowRect { row, y: top + (k as u32 * ROW_H) as i32 - scroll })
            .filter(|r| r.y + ROW_H as i32 > top && r.y < h as i32)
            .collect()
    }

    /// What column point `(x, y)` is over in a column `h` px tall (None: outside it, or empty space).
    pub fn hit(&self, strip: &TabStrip, h: u32, x: i32, y: i32) -> Option<SideHit> {
        let w = self.width() as i32;
        if x < 0 || y < 0 || x >= w || y >= h as i32 {
            return None;
        }
        if y < ROW_H as i32 {
            return Some(SideHit::Toggle);
        }
        let r = self.layout(strip, h).into_iter().find(|r| y >= r.y && y < r.y + ROW_H as i32)?;
        Some(match r.row {
            Row::Group(g) => SideHit::Group(g),
            Row::NewTab => SideHit::NewTab,
            // The rail has no room for a close box: the whole row selects.
            Row::Tab(id) if !self.collapsed && x >= w - CLOSE_W as i32 => SideHit::Close(id),
            Row::Tab(id) => SideHit::Tab(id),
        })
    }
}

/// Ctrl+Shift+A's state: the query and which result is highlighted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabSearch {
    pub query: alloc::string::String,
    pub sel: usize,
}

impl TabSearch {
    /// The matches for the query now ([`TabStrip::search`]).
    pub fn results(&self, strip: &TabStrip) -> Vec<u64> {
        strip.search(&self.query)
    }

    /// Replace the query (typing); the highlight goes back to the first match.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.into();
        self.sel = 0;
    }

    /// Move the highlight `n` places, wrapping over the matches.
    pub fn step(&mut self, strip: &TabStrip, n: i32) {
        let len = self.results(strip).len() as i64;
        if len > 0 {
            self.sel = (self.sel as i64 + n as i64).rem_euclid(len) as usize;
        }
    }

    /// Enter: select the highlighted match in `strip`; answers its id (None: no match).
    pub fn accept(&self, strip: &mut TabStrip) -> Option<u64> {
        let r = self.results(strip);
        let id = *r.get(self.sel.min(r.len().saturating_sub(1)))?;
        strip.select(id).then_some(id)
    }
}
