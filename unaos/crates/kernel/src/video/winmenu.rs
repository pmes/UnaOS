// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! WINMENU — **a window's menus live in the MENU BAR.**
//!
//! # The ruling this file exists to carry out
//!
//! Peter, R21 (2026-09-06), on seeing the pulse window's `View` menu drawn as the first row of the
//! window's own content: *"WHO PUT THE GOD DAMN MENU IN THE WINDOW IT GOES IN THE ----- GOD DAMN MENU
//! BAR"*. A window's menus belong in the bar, never inside the window. It is a ONE-OS rule — the same
//! code on x86 under `wc`, on the Pi and on the Orin — so nothing in this file is arch-conditional.
//!
//! # What was there before, and why it was built that way
//!
//! [`super::pulsewin`]'s header (PULSEWIN `27922509`) argued the defect into existence honestly: *"this
//! kernel has exactly one menu framework — crystal's SHARD dropdown — and it is hard-wired to the menu
//! bar's brand mark"*, so the pulse window carried its own one-title strip as its first content row.
//! The premise was true; the conclusion was the wrong one. The fix is not a second menu framework, it
//! is to make the ONE framework take a second publisher — which is what this module is.
//!
//! # The three pieces
//!
//! 1. **A REGISTRY.** [`publish`] binds a window id to a `&'static` tree and a pick handler;
//!    [`clear`] releases it. Kernel windows only this arc — the trees are `const`, so there is no
//!    allocation anywhere on this path and no lifetime to get wrong. The caps are the ones
//!    `menubar`'s protocol design ledger already fixed for the eventual ring-3 wire form
//!    ([`MENU_LABEL_MAX`] 24, [`MENU_DEPTH_MAX`] 2, [`MENU_ITEMS_MAX`] 64), so a tree that is legal
//!    here is legal on the wire later and the bus arc adds a decoder rather than a second model.
//! 2. **A BAR COMPOSE.** [`bar_boxes`] reports the title boxes of the frontmost publishing window,
//!    right of the caption slot; [`super::menubar::compose_row`] overlays them into the bar's own
//!    face. No focused window, or a focused window with no tree, means no boxes and a bar that looks
//!    exactly as it did before this arc.
//! 3. **A DROPDOWN.** [`compose`] paints it through [`super::strip::paint`] and erases it through
//!    [`super::strip::erase_rect`] — the SHARD dropdown's own discipline, with the SHARD dropdown's own
//!    row metrics, imported from [`super::crystal`] rather than re-derived here.
//!
//! # ONE dropdown at a time, and why that is a structural property rather than a policy
//!
//! [`super::wm::occ_clip`] reserves exactly `MENU_OCC_MAX == 1` transient occluder slot for "the open
//! dropdown", and [`super::screen::present_background`] subtracts exactly one. A second modal surface
//! that could be open at the same time as the SHARD menu would need that budget widened in `wm.rs`.
//! It cannot happen here:
//!
//!  * [`press_at`] runs FIRST in [`super::strip::press_route`], ahead of the crystal. While a window
//!    menu is open it consumes every press on the panel (pick / switch title / dismiss), so the
//!    crystal's closed-corner arm is never reached and cannot mint a second dropdown.
//!  * While the SHARD menu is open, [`press_at`]'s closed arm declines every point
//!    ([`super::crystal::is_open`]), so the press falls through to the crystal's own
//!    dismiss-outside arm.
//!
//! So the two surfaces are mutually exclusive by construction, `MENU_OCC_MAX` stays 1, and
//! [`super::menubar::open_dropdown_rect`] — the single accessor the three occlusion sites now ask —
//! is a total function over that fact rather than a list that has to be kept in step.
//!
//! # Locks, and what the input path is allowed to touch
//!
//! The registry's slot table is a [`spin::Mutex`], and **every** acquisition on the compose and input
//! paths is a [`spin::Mutex::try_lock`] whose failure prints a named refusal and declines the pass —
//! LOCKFIX `7847ceea`'s rule (no blocking lock on the input path), and `strip::paint`'s
//! decline-and-retry shape for the composite one. The *fast* path is cheaper than that: [`LIVE`] is a
//! relaxed count of published trees, so a boot in which nothing ever publishes touches no lock at all
//! and answers every question from one atomic.
//!
//! # What this is NOT, and what the ring-3 arc adds
//!
//! This is the KERNEL half. `menubar`'s design ledger (§THE MENU PROTOCOL) specifies the ring-3 half —
//! `BUS_VERB_MENU_PUBLISH`/`_CLEAR`/`_GET` over the already-principal-stamped `SYS_MSEND` frame, an
//! `INPUT_EV_MENU_PICK` event type, and picks addressed to the TREE'S OWNER rather than to whoever
//! holds focus. None of it is built here, and this module is deliberately shaped so that arc is
//! additive: an app's tree arrives as bytes, is decoded into the same caps, and lands in the same
//! registry keyed by `owner_asid` instead of by window id. The renderer — this file — does not change.

use super::{crystal, menubar, strip, theme, wm};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// The model, and the caps it shares with the eventual wire form
// ---------------------------------------------------------------------------

/// A label's maximum length in bytes — `menubar`'s `MENU_LABEL_MAX`, so a tree legal in this registry
/// is legal in the wire encoding the protocol arc will decode into it.
pub const MENU_LABEL_MAX: usize = 24;

/// The deepest a tree may go: a bar title and its items. `menubar`'s `MENU_DEPTH_MAX`. Enforced
/// STRUCTURALLY here — [`MenuTitle`] holds items and an item holds nothing — rather than by a walk,
/// which is the whole reason a kernel-authored tree needs no parser.
pub const MENU_DEPTH_MAX: usize = 2;

/// The most items one tree may carry, across every title. `menubar`'s `MENU_ITEMS_MAX`.
pub const MENU_ITEMS_MAX: usize = 64;

/// The most TITLES the bar will lay out for one window. Not a wire cap — the wire cap is
/// [`MENU_ITEMS_MAX`] — but the bar's own: a strip 34 px tall with a caption and a clock on it has
/// room for a handful of titles and nothing like sixty, and the snapshot below is sized by this.
pub const MENU_TITLES_MAX: usize = 4;

/// The item is not pickable; it renders dimmed and a press on it keeps the menu open.
pub const FLAG_DISABLED: u32 = 1 << 0;
/// The row is a separator: a keyline, no label, never pickable.
pub const FLAG_SEPARATOR: u32 = 1 << 1;
/// The item is the live one — a mark is drawn in the check column.
pub const FLAG_CHECKED: u32 = 1 << 2;

/// One row of a dropdown.
#[derive(Clone, Copy)]
pub struct MenuItem {
    /// The PUBLISHER's own id, handed back verbatim to its pick handler. The kernel assigns nothing:
    /// the protocol's rule ("a pick is delivered carrying the item id the publisher itself chose"),
    /// kept here so the ring-3 arc changes the transport and not the contract.
    pub id: u32,
    /// ASCII, at most [`MENU_LABEL_MAX`] bytes. Over-long labels are REFUSED at [`publish`], never
    /// truncated — a menu whose items the publisher did not author is worse than no menu.
    pub label: &'static str,
    /// [`FLAG_DISABLED`] / [`FLAG_SEPARATOR`] / [`FLAG_CHECKED`].
    pub flags: u32,
}

/// One title in the bar, and the items that drop from it.
#[derive(Clone, Copy)]
pub struct MenuTitle {
    /// ASCII, at most [`MENU_LABEL_MAX`] bytes.
    pub label: &'static str,
    /// The rows of this title's dropdown.
    pub items: &'static [MenuItem],
}

/// A published tree: the titles, and where a pick goes.
#[derive(Clone, Copy)]
struct Tree {
    titles: &'static [MenuTitle],
    /// **The pick sink.** A bare `fn` pointer, so delivery is a static call with no allocation, no
    /// trait object and no table of handlers to keep in step with the registry. The ring-3 arc
    /// replaces this one field with an input-ring enqueue addressed by principal; nothing else about
    /// the registry moves.
    on_pick: fn(u32),
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// How many windows may publish at once. Four, because [`MENU_TITLES_MAX`] windows' worth of kernel
/// furniture is already more than this desktop has; a fifth publisher is REFUSED on the wire rather
/// than silently evicting a live one.
const WINMENU_MAX: usize = 4;

/// The owner of each slot, or [`wm::WIN_NONE`]. **Lock-free on purpose**: [`has_tree`] is asked once
/// per window per bar compose, from inside `menubar::Model::read`'s existing table scan, and a lock
/// there would be a second lock nested under `wm`'s table.
static OWNERS: [AtomicU32; WINMENU_MAX] = [
    AtomicU32::new(wm::WIN_NONE),
    AtomicU32::new(wm::WIN_NONE),
    AtomicU32::new(wm::WIN_NONE),
    AtomicU32::new(wm::WIN_NONE),
];

/// The trees themselves. Guarded, and **never acquired blocking** — see the module header.
static TREES: spin::Mutex<[Option<Tree>; WINMENU_MAX]> = spin::Mutex::new([None; WINMENU_MAX]);

/// How many slots are live. The whole of the fast path: a boot with no publisher answers every
/// question from this one relaxed load and never reaches the table.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// The window whose menus the BAR is showing, or [`wm::WIN_NONE`]. Published by
/// [`super::menubar::compose`] from the table scan it already runs for the caption, and read by the
/// input path — so a press never takes the window table's lock to find out whose menu it hit.
static BAR_OWNER: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

/// Which title is dropped, 1-based; `0` is closed. One value, so "is a menu open" and "which one" can
/// never disagree.
static OPEN_TITLE: AtomicU32 = AtomicU32::new(0);
/// The window the OPEN dropdown belongs to. Checked on every compose: a publisher that closed or
/// cleared while its menu was down takes the menu with it.
static OPEN_OWNER: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
/// PANEL V-2 — **the open dropdown's rect, PUBLISHED as one atomic** ([`strip::pack_rect`]'s packing,
/// `0` for closed, which is that function's own "none" sentinel).
///
/// [`open_rect`] is asked ONCE PER WINDOW inside a single occlusion walk — `wm::occ_clip`,
/// `wm::erase_clip`, `wm::composite_inner`'s sprite arm and `screen::present_background` — so it has
/// to be a TOTAL function of one fact. The layout it replaced took `TREES.try_lock()` TWICE per call
/// (once in [`bar_boxes`], once in [`tree_of`]) and could therefore answer `Some` for window 3 and
/// `None` for window 5 in the SAME walk: window 5 blits straight over the open dropdown while window
/// 3 correctly withheld those rows. The accessor `crystal::open_rect` (which this replaced at all
/// four sites) was one relaxed load and structurally could not do that; this restores the property.
///
/// Written from TASK context at open time ([`open_title`]) and refreshed ONCE per compose
/// ([`republish_open_rect`]) — never from the per-window walk, which only ever reads it.
static OPEN_RECT: AtomicU64 = AtomicU64::new(0);

/// Falsifiable counters for the ledger line.
static PUBLISHES: AtomicU64 = AtomicU64::new(0);
static CLEARS: AtomicU64 = AtomicU64::new(0);
static OPENS: AtomicU64 = AtomicU64::new(0);
static DISMISSES: AtomicU64 = AtomicU64::new(0);
static PICKS: AtomicU64 = AtomicU64::new(0);
/// Refusals: a `try_lock` that did not get the table. On the ledger because a silent decline and a
/// working menu look identical on a capture.
static LOCK_REFUSALS: AtomicU64 = AtomicU64::new(0);

/// What the dropdown last put on the panel — the strip primitive's damage slot.
static SLOT: strip::Slot = strip::Slot::new();
/// The cost ledger. NOT `witness`-gated, on the furniture family's precedent: the metal image is
/// built without `witness` and a cost claim absent from it is not a claim.
static LEDGER: strip::Ledger = strip::Ledger::new();

/// One printed refusal per boot per site, so a contended table names itself once instead of filling
/// the wire at composite rate.
static REFUSED_ONCE: AtomicBool = AtomicBool::new(false);

fn note_refusal(site: &str) {
    LOCK_REFUSALS.fetch_add(1, Ordering::Relaxed);
    if !REFUSED_ONCE.swap(true, Ordering::Relaxed) {
        serial_println!("[winmenu] REFUSE site={} reason=registry-contended (declined, retried next pass)", site);
    }
}

/// Is `id` a live publisher? Lock-free, [`WINMENU_MAX`] relaxed loads, and short-circuited to nothing
/// by [`LIVE`] on a boot where no window ever published.
pub fn has_tree(id: wm::WinId) -> bool {
    if id == wm::WIN_NONE || LIVE.load(Ordering::Relaxed) == 0 {
        return false;
    }
    OWNERS.iter().any(|o| o.load(Ordering::Relaxed) == id)
}

/// **The registry's answer, and it is THREE-VALUED on purpose** (PANEL V-3).
///
/// A `try_lock` refusal and *"this window has no menus"* are DIFFERENT FACTS, and the module header's
/// rule — *"prints a named refusal and DECLINES THE PASS"* — is only implementable if the caller can
/// tell them apart. Folding both into one `None` made a busy lock indistinguishable from a departed
/// publisher, and the readers act on emptiness: [`bar_boxes`] built the empty snapshot, [`compose`]
/// read `s.open == 0` off it and tore the operator's open menu down because a lock was held for one
/// pass. Reachable, and worst on the single-core Orin, where [`publish`]/[`clear`] hold this very
/// guard across a `serial_println!` that re-enters the video stack on the same core.
enum Look {
    /// The tree published for the asked window.
    Found(Tree),
    /// The window genuinely has no menus. Act on it.
    Absent,
    /// The registry was CONTENDED: we could not look. Decline the pass; conclude nothing.
    Busy,
}

/// The tree published for `id` — see [`Look`] for why contention is its own answer.
fn tree_of(id: wm::WinId, site: &str) -> Look {
    if !has_tree(id) {
        return Look::Absent;
    }
    let g = match TREES.try_lock() {
        Some(g) => g,
        None => {
            note_refusal(site);
            return Look::Busy;
        }
    };
    for (k, slot) in g.iter().enumerate() {
        if OWNERS[k].load(Ordering::Relaxed) == id {
            return match *slot {
                Some(t) => Look::Found(t),
                None => Look::Absent,
            };
        }
    }
    Look::Absent
}

/// **Publish `owner`'s menu tree.** `true` when the registry took it.
///
/// Every refusal is NAMED on the wire and none is fatal to the caller: a window without menus is a
/// perfectly good window. The caps are REFUSALS, not truncations — the protocol ledger's own leg 3.
///
/// Re-publishing for the same owner REPLACES the tree in place, which is how a publisher moves a
/// [`FLAG_CHECKED`] mark: it hands over the other `const` tree. That keeps every tree `&'static` and
/// keeps allocation off this path entirely.
pub fn publish(owner: wm::WinId, titles: &'static [MenuTitle], on_pick: fn(u32)) -> bool {
    if owner == wm::WIN_NONE {
        serial_println!("[winmenu] publish REFUSE reason=no-window");
        return false;
    }
    if titles.is_empty() || titles.len() > MENU_TITLES_MAX {
        serial_println!(
            "[winmenu] publish REFUSE owner={} reason=titles titles={} max={}",
            owner, titles.len(), MENU_TITLES_MAX
        );
        return false;
    }
    let mut items = 0usize;
    for t in titles.iter() {
        if t.label.len() > MENU_LABEL_MAX || t.label.is_empty() {
            serial_println!(
                "[winmenu] publish REFUSE owner={} reason=title-label len={} max={}",
                owner, t.label.len(), MENU_LABEL_MAX
            );
            return false;
        }
        for it in t.items.iter() {
            if it.label.len() > MENU_LABEL_MAX {
                serial_println!(
                    "[winmenu] publish REFUSE owner={} reason=item-label len={} max={}",
                    owner, it.label.len(), MENU_LABEL_MAX
                );
                return false;
            }
        }
        items += t.items.len();
    }
    if items > MENU_ITEMS_MAX {
        serial_println!(
            "[winmenu] publish REFUSE owner={} reason=items items={} max={}",
            owner, items, MENU_ITEMS_MAX
        );
        return false;
    }
    let mut g = match TREES.try_lock() {
        Some(g) => g,
        None => {
            note_refusal("publish");
            return false;
        }
    };
    // Replace in place if this owner already holds a slot, else take a free one.
    let mut idx = None;
    for k in 0..WINMENU_MAX {
        if OWNERS[k].load(Ordering::Relaxed) == owner {
            idx = Some(k);
            break;
        }
    }
    let replaced = idx.is_some();
    if idx.is_none() {
        for k in 0..WINMENU_MAX {
            if OWNERS[k].load(Ordering::Relaxed) == wm::WIN_NONE {
                idx = Some(k);
                break;
            }
        }
    }
    let Some(k) = idx else {
        serial_println!("[winmenu] publish REFUSE owner={} reason=registry-full slots={}", owner, WINMENU_MAX);
        return false;
    };
    g[k] = Some(Tree { titles, on_pick });
    OWNERS[k].store(owner, Ordering::Release);
    if !replaced {
        LIVE.fetch_add(1, Ordering::Relaxed);
    }
    PUBLISHES.fetch_add(1, Ordering::Relaxed);
    serial_println!(
        "[winmenu] publish owner={} titles={} items={} slot={} replaced={}",
        owner, titles.len(), items, k, replaced
    );
    true
}

/// **Drop `owner`'s tree.** `true` when a tree went. Idempotent.
///
/// A dropdown belonging to the departing owner is dismissed first: a menu that outlived the window it
/// hangs from would be a modal surface with nothing left able to dismiss it — the same rule
/// `menubar::set_enabled(false)` applies to the SHARD menu.
pub fn clear(owner: wm::WinId) -> bool {
    if !has_tree(owner) {
        return false;
    }
    if OPEN_OWNER.load(Ordering::Relaxed) == owner {
        dismiss("clear");
    }
    let mut g = match TREES.try_lock() {
        Some(g) => g,
        None => {
            note_refusal("clear");
            return false;
        }
    };
    let mut gone = false;
    for k in 0..WINMENU_MAX {
        if OWNERS[k].load(Ordering::Relaxed) == owner {
            OWNERS[k].store(wm::WIN_NONE, Ordering::Release);
            g[k] = None;
            gone = true;
        }
    }
    if gone {
        LIVE.fetch_sub(1, Ordering::Relaxed);
        CLEARS.fetch_add(1, Ordering::Relaxed);
        serial_println!("[winmenu] clear owner={}", owner);
    }
    gone
}

/// **The window whose menus the bar is showing**, published by [`super::menubar::compose`]. Returns
/// the previous value.
///
/// A change closes an open dropdown: the titles under it have moved, so leaving it down would anchor
/// another window's menu under this window's title.
///
/// PANEL V-1 — the STATE half only. This function's one caller is [`super::menubar::compose`], i.e.
/// it runs INSIDE `strip::compose_all`, inside the composite pass; a [`dismiss`] here would re-enter
/// `wm::composite()` from within the pass. [`super::winmenu::compose`] runs later in the same
/// `compose_all`, so the erase this clear owes is discharged in THIS pass, not a future one.
pub fn set_bar_owner(id: wm::WinId) -> wm::WinId {
    let was = BAR_OWNER.swap(id, Ordering::Release);
    if was != id && OPEN_TITLE.load(Ordering::Relaxed) != 0 && OPEN_OWNER.load(Ordering::Relaxed) == was {
        dismiss_state("owner-change");
    }
    was
}

/// The window whose menus the bar is showing, or [`wm::WIN_NONE`].
#[inline]
pub fn bar_owner() -> wm::WinId {
    BAR_OWNER.load(Ordering::Acquire)
}

/// Is a window menu dropped?
#[inline]
pub fn is_open() -> bool {
    OPEN_TITLE.load(Ordering::Relaxed) != 0
}

// ---------------------------------------------------------------------------
// Bar geometry — the title boxes, from ONE accessor
// ---------------------------------------------------------------------------

/// The gap either side of a title's label inside its press/paint box. Half the kit's
/// [`strip::PAD`]: a bar title is a dense target sitting shoulder to shoulder with its neighbours,
/// where the kit's full gap is the spacing between unrelated things.
const TPAD: usize = strip::PAD / 2;

/// The glyph metrics the bar draws at — the crystal dropdown's, which are the bar's, which are
/// `wm`'s title cell. Named once here so this file cannot disagree with the strip it paints into.
const CELL_W: usize = crystal::DROP_CELL_W;
const CELL_H: usize = crystal::DROP_CELL_H;
const FACE: super::font::Face = crystal::DROP_FACE;

/// **The bar's title boxes, and which one is open.** A `Copy` snapshot, taken once per compose and
/// handed to the row painter, so a 34-row paint costs ONE registry read rather than thirty-four.
///
/// It is also what [`press_at`] hit-tests and what [`open_rect`] anchors to — one function, three
/// readers, which is the rule `menubar::crystal_box_abs` records for the brand mark.
#[derive(Clone, Copy)]
pub struct BarSnapshot {
    /// How many boxes are laid out. `0` when there is no publisher, no bar, or no room.
    pub n: usize,
    /// PANEL V-3 — **the registry was CONTENDED while this snapshot was taken**, so `n == 0` here
    /// means *"we could not look"*, never *"there is nothing"*. Every reader that would ACT on
    /// emptiness — the bar's repaint, the dropdown's teardown, the press router's dismiss-outside —
    /// must test this first and decline the pass instead. Deliberately NOT in [`signature`]: it is a
    /// transient property of one attempt, not content the painter reads.
    pub busy: bool,
    /// The open title, 1-based; `0` is closed.
    pub open: usize,
    /// The window these boxes belong to.
    pub owner: wm::WinId,
    /// Box origin and width, panel-absolute. Height is the bar's, and `y` is the bar's.
    pub x: [usize; MENU_TITLES_MAX],
    pub w: [usize; MENU_TITLES_MAX],
    pub label: [[u8; MENU_LABEL_MAX]; MENU_TITLES_MAX],
    pub label_len: [usize; MENU_TITLES_MAX],
    /// The bar rect these boxes were laid out in — so a reader never has to re-ask for it.
    pub bar: strip::Rect,
}

impl BarSnapshot {
    /// The empty snapshot: no publisher, or no bar. Draws nothing and hit-tests to nothing.
    pub const fn empty() -> BarSnapshot {
        BarSnapshot {
            n: 0,
            busy: false,
            open: 0,
            owner: wm::WIN_NONE,
            x: [0; MENU_TITLES_MAX],
            w: [0; MENU_TITLES_MAX],
            label: [[0; MENU_LABEL_MAX]; MENU_TITLES_MAX],
            label_len: [0; MENU_TITLES_MAX],
            bar: (0, 0, 0, 0),
        }
    }

    /// Which title box, if any, panel point `(px, py)` lands in.
    fn hit(&self, px: usize, py: usize) -> Option<usize> {
        let (_, by, _, bh) = self.bar;
        if py < by || py >= by + bh {
            return None;
        }
        (0..self.n).find(|&k| px >= self.x[k] && px < self.x[k] + self.w[k])
    }

    /// The label of box `k`, as bytes.
    #[inline]
    pub fn label_of(&self, k: usize) -> &[u8] {
        &self.label[k][..self.label_len[k]]
    }

    /// The snapshot reduced to one integer, for the bar's damage test. The bar repaints when a title
    /// appears, moves, is relabelled, or opens — and on nothing else.
    pub fn signature(&self) -> u64 {
        let mut h = strip::FNV_BASIS;
        h = strip::fnv1a_u64(h, self.owner as u64);
        h = strip::fnv1a_u64(h, self.n as u64);
        h = strip::fnv1a_u64(h, self.open as u64);
        for k in 0..self.n {
            h = strip::fnv1a_u64(h, self.x[k] as u64);
            h = strip::fnv1a_u64(h, self.w[k] as u64);
            for &b in self.label_of(k) {
                h = strip::fnv1a(h, b);
            }
        }
        strip::seal(h)
    }
}

/// **THE accessor**: the title boxes on a `pw` x `ph` panel.
///
/// Laid out left to right from [`super::menubar::menus_x0`] — a FIXED offset past the caption slot,
/// not past the caption's rendered width. A caption changing length must not make the menus dance
/// under the operator's hand, and a fixed slot is the only layout in which the box a press lands in
/// is the box the previous frame drew.
///
/// A title that would collide with the clock is DROPPED, not squeezed: the strip constructors'
/// decline rule. So a narrow panel shows the titles that fit and says so through `n`.
pub fn bar_boxes(pw: usize, ph: usize) -> BarSnapshot {
    let mut s = BarSnapshot::empty();
    if LIVE.load(Ordering::Relaxed) == 0 {
        return s;
    }
    let Some(bar) = menubar::strip_rect(pw, ph) else {
        return s;
    };
    let owner = bar_owner();
    // PANEL V-3 — DECLINED and EMPTY are different snapshots. `busy` carries the difference to the
    // readers; nothing here concludes "no publisher" from a lock it could not take.
    let tree = match tree_of(owner, "bar_boxes") {
        Look::Found(t) => t,
        Look::Absent => return s,
        Look::Busy => {
            s.busy = true;
            return s;
        }
    };
    s.bar = bar;
    s.owner = owner;
    let (bx, _by, bw, _bh) = bar;
    let limit = menubar::menus_right_limit(bar);
    let mut x = bx + menubar::menus_x0();
    for t in tree.titles.iter().take(MENU_TITLES_MAX) {
        let l = t.label.as_bytes();
        let w = l.len() * CELL_W + 2 * TPAD;
        if x + w > limit || x + w > bx + bw {
            break; // no room before the clock: decline this title and every one after it
        }
        let k = s.n;
        s.x[k] = x;
        s.w[k] = w;
        s.label_len[k] = l.len().min(MENU_LABEL_MAX);
        s.label[k][..s.label_len[k]].copy_from_slice(&l[..s.label_len[k]]);
        s.n += 1;
        x += w;
    }
    let open = OPEN_TITLE.load(Ordering::Relaxed) as usize;
    s.open = if open != 0 && open <= s.n && OPEN_OWNER.load(Ordering::Relaxed) == owner { open } else { 0 };
    s
}

// ---------------------------------------------------------------------------
// The dropdown — the SHARD primitive's metrics, a second surface
// ---------------------------------------------------------------------------

/// The dropdown's row metrics, imported from [`super::crystal`] rather than restated. A second copy
/// of "how tall is a menu row" is two things that can drift, and they would drift SILENTLY — the only
/// symptom is a pick landing one row off.
const BORDER: usize = crystal::DROP_BORDER;
const ITEM_H: usize = crystal::DROP_ITEM_H;
const SEP_H: usize = crystal::DROP_SEP_H;
const PADX: usize = crystal::DROP_PADX;

/// The check column, in glyphs: a mark and a space. Reserved on every row so the labels of a menu
/// with one marked item still line up with each other.
const CHECK_GLYPHS: usize = 2;

/// The mark drawn against a [`FLAG_CHECKED`] item. A glyph, not a sprite: the chrome face is the only
/// type this surface has, and a second glyph source for one checkmark would be a font for a checkmark
/// — [`super::pulsewin`]'s own argument, kept.
const CHECK_MARK: &[u8] = b">";

/// The dropdown's extent for `tree`'s title `k`, in px.
fn drop_extent(titles: &[MenuTitle], k: usize) -> (usize, usize) {
    let items = titles[k].items;
    let mut widest = 0usize;
    let mut h = 2 * BORDER;
    for it in items.iter() {
        if it.flags & FLAG_SEPARATOR != 0 {
            h += SEP_H;
            continue;
        }
        h += ITEM_H;
        if it.label.len() > widest {
            widest = it.label.len();
        }
    }
    let w = 2 * BORDER + 2 * PADX + (widest + CHECK_GLYPHS) * CELL_W;
    (w, h)
}

/// The top of item `i` as an offset from the dropdown's top edge.
fn item_top(items: &[MenuItem], i: usize) -> usize {
    let mut y = BORDER;
    for it in items.iter().take(i) {
        y += if it.flags & FLAG_SEPARATOR != 0 { SEP_H } else { ITEM_H };
    }
    y
}

/// Which item, if any, the menu-local vertical offset `ly` falls inside.
fn item_at_row(items: &[MenuItem], ly: usize, mh: usize) -> Option<usize> {
    if ly < BORDER || ly + BORDER >= mh {
        return None;
    }
    for i in 0..items.len() {
        let top = item_top(items, i);
        let h = if items[i].flags & FLAG_SEPARATOR != 0 { SEP_H } else { ITEM_H };
        if ly >= top && ly < top + h {
            return Some(i);
        }
    }
    None
}

/// **The dropdown's rect on a `pw` x `ph` panel, or `None` while nothing is dropped.**
///
/// PANEL V-2 — ONE acquire load of [`OPEN_RECT`], no lock, TOTAL. The panel extent is taken only to
/// keep the call shape the four occlusion consumers already use: the rect they get is the rect the
/// layout published for the panel it was laid out on, identical for every window in one walk.
///
/// This is what [`super::menubar::open_dropdown_rect`] folds together with the SHARD menu's own
/// accessor for the three occlusion sites; see the module header for why exactly one of the two can
/// ever answer `Some`.
pub fn open_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    let _ = (pw, ph);
    match OPEN_RECT.load(Ordering::Acquire) {
        0 => None,
        p => Some(strip::unpack_rect(p)),
    }
}

/// What the LAYOUT concluded this pass — three-valued for [`Look`]'s reason, one level up.
enum Layout {
    /// The dropdown belongs at this rect.
    At(strip::Rect),
    /// Nothing is dropped, or the panel cannot seat the menu: the surface owes an ERASE.
    Gone,
    /// The registry was contended. The PUBLISHED rect stands; nothing is concluded.
    Busy,
}

/// **Lay the dropdown out**, from the registry — the SHARD dropdown's anchoring rule applied to a
/// title box instead of the brand mark: under the OPEN title's left edge, the same clamp so the right
/// edge stays on the panel, and the same decline when the panel cannot seat the menu below the bar.
///
/// This is the half that TAKES THE LOCK, so its callers are counted: [`open_title`] (task context)
/// and [`compose`] (once per pass). The per-window occlusion walk reads [`OPEN_RECT`] instead.
fn layout_open_rect(pw: usize, ph: usize, s: &BarSnapshot) -> Layout {
    if s.busy {
        return Layout::Busy;
    }
    if !is_open() || s.open == 0 {
        return Layout::Gone;
    }
    let tree = match tree_of(s.owner, "open_rect") {
        Look::Found(t) => t,
        Look::Absent => return Layout::Gone,
        Look::Busy => return Layout::Busy,
    };
    let k = s.open - 1;
    if k >= tree.titles.len() {
        return Layout::Gone;
    }
    let (mw, mh) = drop_extent(tree.titles, k);
    let (_bx, by, _bw, bh) = s.bar;
    let my = by + bh;
    if mw > pw || my + mh > ph {
        return Layout::Gone;
    }
    let mx = if s.x[k] + mw > pw { pw - mw } else { s.x[k] };
    Layout::At((mx, my, mw, mh))
}

/// Lay the dropdown out and PUBLISH the answer into [`OPEN_RECT`], returning the rect now on the
/// panel. A contended pass republishes nothing and answers with the standing fact — the one behaviour
/// that keeps the accessor total across a pass in which the registry was briefly unavailable.
fn republish_open_rect(pw: usize, ph: usize, s: &BarSnapshot) -> Option<strip::Rect> {
    match layout_open_rect(pw, ph, s) {
        Layout::At(r) => {
            OPEN_RECT.store(strip::pack_rect(Some(r)), Ordering::Release);
            Some(r)
        }
        Layout::Gone => {
            OPEN_RECT.store(0, Ordering::Release);
            None
        }
        Layout::Busy => open_rect(pw, ph),
    }
}

// ---------------------------------------------------------------------------
// Open / dismiss / pick
// ---------------------------------------------------------------------------

/// Drop title `k` (0-based) of the bar owner's tree.
fn open_title(k: usize, s: &BarSnapshot) {
    OPEN_OWNER.store(s.owner, Ordering::Release);
    OPEN_TITLE.store((k + 1) as u32, Ordering::Release);
    OPENS.fetch_add(1, Ordering::Relaxed);
    let (pw, ph) = panel();
    // PANEL V-2 — publish the rect HERE, in task context, off a snapshot taken AFTER the open is
    // visible (`s` was read before `OPEN_TITLE` was stored, so its `open` is still 0). Every reader
    // downstream takes it as one atomic.
    let fresh = bar_boxes(pw, ph);
    let (mx, my) = republish_open_rect(pw, ph, &fresh).map(|r| (r.0, r.1)).unwrap_or((0, 0));
    let items = match tree_of(s.owner, "open") {
        Look::Found(t) => t.titles.get(k).map(|t| t.items.len()).unwrap_or(0),
        _ => 0,
    };
    serial_println!(
        "[winmenu] open title={} items={} at ({},{}) owner={}",
        core::str::from_utf8(s.label_of(k)).unwrap_or("?"),
        items, mx, my, s.owner
    );
    drive();
}

/// Tear the dropdown down AND drive the pass that erases it. Idempotent. `reason` is `outside` /
/// `esc` / `pick` / `title` / `clear` / `owner-change` — a capture must be able to tell a cancel from
/// a selection.
///
/// **TASK CONTEXT ONLY**, because [`drive`] is: the click router, [`key_escape`] and the publisher's
/// own [`clear`]. From a COMPOSE path call [`dismiss_state`] instead.
fn dismiss(reason: &str) {
    if dismiss_state(reason) {
        drive();
    }
}

/// PANEL V-1 — **the STATE half of a dismiss, with NO composite.** `true` when a menu actually went.
///
/// `wm::composite()` is re-entrancy-safe only on x86, where `COMP_GATE` declines a nested call. The
/// `#[cfg(not(target_arch = "x86_64"))]` arm (`video/wm.rs`) is a bare `composite_once()` with no
/// gate and no decline path of its own, so on the Orin and on the Pi desktop a [`drive`] reached from
/// inside `strip::compose_all` RE-ENTERS the pass that is running: a second `cursor::undraw`
/// save-under bracket opens inside the outer one (the inner restore writes back the outer pass's
/// half-drawn pixels), and the `OWED_ARM`/`OWED_TAIL` stash machinery runs from inside the present's
/// IRQ-masked half. [`drive`]'s own doc already states the precondition — *"task context only"* — and
/// the two compose-path callers violated it.
///
/// Clearing the state without driving costs nothing here, because a pass IS running: [`compose`]
/// takes the erase itself in the same call, and [`set_bar_owner`]'s caller (`menubar::compose`) is
/// followed by `winmenu::compose` inside the same `strip::compose_all`. No gesture waits on a pass
/// that may never come — which is the whole reason MENU-DRIVE exists.
fn dismiss_state(reason: &str) -> bool {
    if OPEN_TITLE.swap(0, Ordering::AcqRel) == 0 {
        return false;
    }
    // PANEL V-2 — the published rect goes with the state, so the occlusion walk stops reserving rows
    // for a menu that is down.
    OPEN_RECT.store(0, Ordering::Release);
    let owner = OPEN_OWNER.swap(wm::WIN_NONE, Ordering::AcqRel);
    DISMISSES.fetch_add(1, Ordering::Relaxed);
    serial_println!("[winmenu] dismiss reason={} owner={}", reason, owner);
    true
}

/// **An open or a dismissed menu must DRIVE the pass that paints or erases it** — [`super::crystal`]'s
/// MENU-DRIVE rule, verbatim and for its reason: [`compose`] runs only from `strip::compose_all` at
/// the tail of a composite, and on a static desktop no other pass is coming, so the gesture would
/// change the state and never the glass. Task context only (the click router, [`key_escape`], and the
/// publisher's own open/close), so the composite is taken directly, with the one verified retry that
/// covers a `strip::paint` declined on a contended scratch.
fn drive() {
    wm::composite();
    if paint_owed() {
        wm::composite();
    }
}

/// Does the dropdown owe a paint or an erase that only a composite can discharge? [`crystal::paint_owed`]'s
/// condition on this surface — open with an empty slot owes a PAINT, closed with a full slot owes an
/// ERASE.
pub fn paint_owed() -> bool {
    is_open() != (SLOT.packed() != 0)
}

/// The panel extent, or `(0, 0)`.
fn panel() -> (usize, usize) {
    let fb = *super::WRITER.lock();
    if fb.is_ready() { (fb.width(), fb.height()) } else { (0, 0) }
}

// ---------------------------------------------------------------------------
// The press arm — FIRST in `strip::press_route`
// ---------------------------------------------------------------------------

/// **Route a press at panel `(x, y)` to a window menu.** `true` iff it was CONSUMED.
///
/// The rule, and it is the SHARD menu's with one addition (switching titles):
///  * **open, press on an item** — pick it: dismiss, then deliver to the tree's owner. Consumed.
///  * **open, press on a separator / disabled row / border** — consumed, menu stays open.
///  * **open, press on ANOTHER title** — switch to it. Consumed.
///  * **open, press on the SAME title** — dismiss. Consumed.
///  * **open, press anywhere else** — dismiss (`outside`). Consumed.
///  * **closed, press on a title** — open it. Consumed. Unless the SHARD menu is open, in which case
///    this arm declines so that press reaches the crystal's own dismiss-outside arm — the whole of
///    the one-dropdown-at-a-time invariant, stated where it is enforced.
///  * **closed, anywhere else** — `false`; the crystal, the dock and the window arms get their say.
pub fn press_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 || LIVE.load(Ordering::Relaxed) == 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let (pw, ph) = panel();
    if pw == 0 {
        return false;
    }
    let s = bar_boxes(pw, ph);

    // PANEL V-3 — a CONTENDED registry declines the press; it never tears the operator's menu down.
    // A busy snapshot lays out no title boxes, so every hit-test below would miss and the miss arm
    // is `dismiss("outside")` — a lock held for one pass would close the menu under the operator's
    // hand. The menu is modal while it is down, so declining means CONSUMING and changing nothing.
    if s.busy && is_open() {
        return true;
    }

    if is_open() {
        if let Some(r) = open_rect(pw, ph) {
            let (mx, my, mw, mh) = r;
            if px >= mx && px < mx + mw && py >= my && py < my + mh {
                let tree = match tree_of(s.owner, "press") {
                    Look::Found(t) => t,
                    // Contended between the snapshot and here: decline, keep the menu down.
                    Look::Busy => return true,
                    Look::Absent => {
                        dismiss("outside");
                        return true;
                    }
                };
                let k = s.open.saturating_sub(1);
                let Some(t) = tree.titles.get(k) else {
                    dismiss("outside");
                    return true;
                };
                return match item_at_row(t.items, py - my, mh) {
                    Some(i)
                        if t.items[i].flags & (FLAG_SEPARATOR | FLAG_DISABLED) == 0 =>
                    {
                        let id = t.items[i].id;
                        let owner = s.owner;
                        PICKS.fetch_add(1, Ordering::Relaxed);
                        dismiss("pick");
                        serial_println!("[winmenu] pick owner={} id={} label={}", owner, id, t.items[i].label);
                        (tree.on_pick)(id);
                        true
                    }
                    // A separator, a disabled row, a border or the inner padding: swallow the press
                    // and keep the menu open, as a real menu does.
                    _ => true,
                };
            }
        }
        return match s.hit(px, py) {
            Some(k) if k + 1 == s.open => {
                dismiss("title");
                true
            }
            Some(k) => {
                dismiss("title");
                open_title(k, &s);
                true
            }
            None => {
                dismiss("outside");
                true
            }
        };
    }

    // CLOSED. The SHARD menu is the modal surface while it is up; declining here is what keeps the
    // two mutually exclusive and `wm::MENU_OCC_MAX` at one.
    if crystal::is_open() {
        return false;
    }
    match s.hit(px, py) {
        Some(k) => {
            open_title(k, &s);
            true
        }
        None => false,
    }
}

/// **The Escape arm**, asked from the same seam [`super::crystal::key_escape`] is: a bare `Esc`
/// (0x1b) while a window menu is open dismisses it and is consumed. Every other event, and `Esc` with
/// no menu down, falls straight through.
pub fn key_escape(ev: crate::pal::Event) -> bool {
    const K_ESC: u8 = 0x1b;
    match ev {
        crate::pal::Event::Key(K_ESC) if is_open() => {
            dismiss("esc");
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The bar's row overlay
// ---------------------------------------------------------------------------

/// **Overlay the title boxes into one composed bar row.** Called from
/// [`super::menubar::compose_row`] with that function's own scratch row, so the titles are part of
/// the bar's single paint rather than a second surface stacked on it.
///
/// `j` is the row's offset inside the bar box and `ty0`/`sy` are the caption's own text band, handed
/// in rather than recomputed, so a title's baseline can never drift from the caption's.
pub fn draw_bar_row(out: &mut [u32], w: usize, s: &BarSnapshot, j: usize, ty0: usize) {
    let (bx, _by, _bw, bh) = s.bar;
    for k in 0..s.n {
        let x0 = s.x[k] - bx; // box-relative: `out` is the bar's row, not the panel's
        let bw_k = s.w[k];
        let open = s.open == k + 1;
        // The open title's box is filled for the FULL height of the bar bar its keyline, so the
        // dropdown reads as hanging from a lit title rather than floating under a flat strip.
        if open && j + 1 < bh {
            for i in x0..(x0 + bw_k).min(w) {
                out[i] = theme::ACCENT;
            }
        }
        if j < ty0 || j >= ty0 + CELL_H {
            continue;
        }
        let ink = if open { theme::BEVEL_LIGHT } else { theme::TITLE_TEXT_ACTIVE };
        super::font::draw_row(out, w, s.label_of(k), x0 + TPAD, j - ty0, ink, false, FACE);
    }
}

// ---------------------------------------------------------------------------
// The composite seam
// ---------------------------------------------------------------------------

/// **The erase this surface owes once the menu is down.** `true` when pixels went back.
///
/// CRYSTAL-DISMISS's ordering, kept: the erase lands BEFORE the slot is cleared, so a declined erase
/// stays owed and [`paint_owed`] keeps reporting the debt instead of stranding the dismissed menu's
/// pixels on the glass. Lifted out of [`compose`]'s head because PANEL V-1's fix needs it from two
/// more arms — a compose-path teardown discharges its own pixels rather than driving a nested pass.
fn erase_owed() -> bool {
    if SLOT.packed() == 0 {
        return false;
    }
    let r = SLOT.rect();
    if !strip::erase_rect(r) {
        return false;
    }
    SLOT.clear();
    repaint_vacated(r);
    true
}

/// **Paint or erase the dropdown.** Called from [`super::strip::compose_all`] at the composite tail,
/// AFTER the SHARD menu, so a window menu is the topmost surface on the panel while it is down.
///
/// The closed path is two relaxed atomics and a return, so a boot that never opens a window menu pays
/// exactly that for this module's existence — the tenant law, on a transient.
pub fn compose() -> bool {
    if !is_open() {
        return erase_owed();
    }
    let t0 = crate::arch::now_cycles();
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
            return false;
        }
        (fb.width(), fb.height())
    };
    let s = bar_boxes(pw, ph);
    // PANEL V-3 — the registry was busy for this pass. DECLINE it: `s.open == 0` below is the
    // teardown test, and reaching it off a lock we could not take is exactly how a transient refusal
    // destroyed operator state. The menu keeps its pixels; the next pass re-asks.
    if s.busy {
        return false;
    }
    // The publisher went away, or the bar moved to another window, while the menu was down. Say so
    // and tear it down rather than painting another window's menu under this window's title.
    //
    // PANEL V-1 — STATE ONLY, then the erase, HERE. This function runs from `strip::compose_all`,
    // inside the composite pass; `dismiss` would call `drive()` -> `wm::composite()` and re-enter it
    // (ungated on aarch64). `erase_owed` is the pixel half of what `drive` would have driven, taken
    // in the pass that is already running. See [`dismiss_state`].
    if s.open == 0 {
        dismiss_state("clear");
        return erase_owed();
    }
    let rect = republish_open_rect(pw, ph, &s);
    LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
    LEDGER.tick(
        "winmenu",
        format_args!(
            "state=open owner={} publishes={} clears={} opens={} dismisses={} picks={} refusals={}",
            s.owner,
            PUBLISHES.load(Ordering::Relaxed),
            CLEARS.load(Ordering::Relaxed),
            OPENS.load(Ordering::Relaxed),
            DISMISSES.load(Ordering::Relaxed),
            PICKS.load(Ordering::Relaxed),
            LOCK_REFUSALS.load(Ordering::Relaxed)
        ),
    );

    let Some(r) = rect else {
        return erase_owed();
    };

    // CLOBBER-REPAIR — the dropdown's signature is a function of its rect and its content, so a
    // window that painted over the open menu changes nothing this test can see. The bar's and the
    // SHARD menu's condition, asked the same way: one bounded table scan, only while open.
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (_, clobbered) = wm::dock_scan(&mut rows, SLOT.rect());
    let sig = drop_sig(&s, r);
    if sig == SLOT.sig() && SLOT.packed() == strip::pack_rect(Some(r)) && !clobbered {
        return false;
    }

    let old = SLOT.packed();
    let new = strip::pack_rect(Some(r));
    if old != 0 && old != new {
        strip::erase_rect(strip::unpack_rect(old));
    }

    let Look::Found(tree) = tree_of(s.owner, "compose") else {
        return false;
    };
    let k = s.open - 1;
    let Some(t) = tree.titles.get(k) else { return false };
    let items = t.items;
    let t1 = crate::arch::now_cycles();
    if !strip::paint("winmenu", r, |out, j| compose_row(out, r, items, j)) {
        return false;
    }
    LEDGER.paint(crate::arch::now_cycles().saturating_sub(t1), (r.2 * r.3) as u64);
    SLOT.store(sig, Some(r));
    true
}

/// The dropdown's damage signature: the rect, the owner, the open title, and every item's id, flags
/// and label. Flags are IN because a check mark moving is the one content change a face switch makes.
fn drop_sig(s: &BarSnapshot, r: strip::Rect) -> u64 {
    let mut h = strip::fnv1a_u64(strip::FNV_BASIS, strip::pack_rect(Some(r)));
    h = strip::fnv1a_u64(h, s.owner as u64);
    h = strip::fnv1a_u64(h, s.open as u64);
    if let Look::Found(tree) = tree_of(s.owner, "sig") {
        if let Some(t) = tree.titles.get(s.open.saturating_sub(1)) {
            for it in t.items.iter() {
                h = strip::fnv1a_u64(h, it.id as u64);
                h = strip::fnv1a_u64(h, it.flags as u64);
                for &b in it.label.as_bytes() {
                    h = strip::fnv1a(h, b);
                }
            }
        }
    }
    strip::seal(h)
}

/// Hand a just-vacated dropdown rect back to its owners — [`super::crystal`]'s `repaint_vacated`,
/// same pair, same reason: the windows under an open menu WITHHELD its rows, so `erase_rect`'s
/// `DESKTOP_BG` would stamp a hole over them rather than restore them.
fn repaint_vacated(r: strip::Rect) {
    let (x, y, w, h) = r;
    if w == 0 || h == 0 {
        return;
    }
    wm::damage_intersecting(x, y, w, h);
    super::screen::request_full_present();
}

/// Compose panel row `j` of the dropdown. The SHARD dropdown's own row shape — a field pass for the
/// face and the four keylines, then the item's label overlaid — with a check column in front of the
/// label and a dim ink for a disabled row.
fn compose_row(out: &mut [u32], r: strip::Rect, items: &[MenuItem], j: usize) {
    let (_mx, _my, w, h) = r;
    let base = if j < BORDER || j + BORDER >= h { theme::FRAME_LINE } else { theme::CHROME_FACE };
    for i in 0..w {
        out[i] = base;
    }
    for i in 0..w {
        if i < BORDER || i + BORDER >= w {
            out[i] = theme::FRAME_LINE;
        }
    }
    if j < BORDER || j + BORDER >= h {
        return;
    }
    let Some(idx) = item_at_row(items, j, h) else {
        return;
    };
    let it = items[idx];
    let top = item_top(items, idx);
    if it.flags & FLAG_SEPARATOR != 0 {
        if j == top + SEP_H / 2 {
            for i in (BORDER + PADX)..(w - BORDER - PADX) {
                out[i] = theme::FRAME_LINE;
            }
        }
        return;
    }
    let vpad = (ITEM_H - CELL_H) / 2;
    let gtop = top + vpad;
    if j < gtop || j >= gtop + CELL_H {
        return;
    }
    let sy = j - gtop;
    let ink = if it.flags & FLAG_DISABLED != 0 {
        theme::TITLE_TEXT_INACTIVE
    } else {
        theme::TITLE_TEXT_ACTIVE
    };
    if it.flags & FLAG_CHECKED != 0 {
        super::font::draw_row(out, w, CHECK_MARK, BORDER + PADX, sy, ink, false, FACE);
    }
    super::font::draw_row(
        out,
        w,
        it.label.as_bytes(),
        BORDER + PADX + CHECK_GLYPHS * CELL_W,
        sy,
        ink,
        false,
        FACE,
    );
}

/// The ledger line, on the furniture family's terms.
pub fn rollup(scope: &str) {
    LEDGER.rollup(
        "winmenu",
        scope,
        format_args!(
            "live={} bar_owner={} open={} publishes={} clears={} opens={} dismisses={} picks={} refusals={}",
            LIVE.load(Ordering::Relaxed),
            bar_owner(),
            OPEN_TITLE.load(Ordering::Relaxed),
            PUBLISHES.load(Ordering::Relaxed),
            CLEARS.load(Ordering::Relaxed),
            OPENS.load(Ordering::Relaxed),
            DISMISSES.load(Ordering::Relaxed),
            PICKS.load(Ordering::Relaxed),
            LOCK_REFUSALS.load(Ordering::Relaxed)
        ),
    );
}

// ---------------------------------------------------------------------------
// Compile-time sanity
// ---------------------------------------------------------------------------

const _: () = {
    // A registry with no slots would be a registry that refuses every publisher.
    assert!(WINMENU_MAX >= 1);
    // The bar cannot lay out more titles than the snapshot can carry.
    assert!(MENU_TITLES_MAX >= 1);
    // The wire caps this registry shares with the protocol design must hold what a tree can be.
    assert!(MENU_LABEL_MAX >= 1 && MENU_ITEMS_MAX >= 1 && MENU_DEPTH_MAX == 2);
    // The row must clear the glyph it centres, or a label is cut — the crystal's own assert, on the
    // metrics this file imported from it, so an import that ever stops agreeing fails the BUILD.
    assert!(ITEM_H >= CELL_H && (ITEM_H - CELL_H) % 2 == 0);
    assert!(SEP_H >= 3);
    // A title box must be wider than its own padding.
    assert!(TPAD * 2 < CELL_W * MENU_LABEL_MAX);
};
