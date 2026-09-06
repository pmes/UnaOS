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

/// SO3 — the most BOXES the bar lays out: the APP MENU (the window's own name, box 0) plus the
/// tenant's [`MENU_TITLES_MAX`] published titles. The snapshot arrays are sized by this, not by
/// `MENU_TITLES_MAX`, because Peter's ruling gives every window a name-menu whether or not it ever
/// publishes one of its own — so the app box is not one of the tenant's four.
pub const BAR_BOXES_MAX: usize = MENU_TITLES_MAX + 1;

/// The item is not pickable; it renders dimmed and a press on it keeps the menu open.
pub const FLAG_DISABLED: u32 = 1 << 0;
/// The row is a separator: a keyline, no label, never pickable.
pub const FLAG_SEPARATOR: u32 = 1 << 1;
/// The item is the live one — a mark is drawn in the check column.
pub const FLAG_CHECKED: u32 = 1 << 2;
/// SO3 — the LIVE APP NAME is appended to this item's label, with one space between.
///
/// Peter asked for *"About &lt;app&gt;"*, and a `MenuItem`'s label is `&'static str` by design (the
/// registry allocates nothing and every tree is `const`). A flag costs the layout one addition and
/// the painter one extra `draw_row`; a dynamic label would cost the registry its whole no-allocation
/// property. So the composition happens at PAINT time, from the caption the bar already published.
pub const FLAG_APPNAME: u32 = 1 << 3;

// ---------------------------------------------------------------------------
// SO3 — the DEFAULT APP MENU
//
// Peter's ruling (2026-09-06): *"every app window's main menu — the bar's app title — must open a
// menu with at least Quit (closes the window / ends the tenant), and About <app> if cheap."*
//
// It is the WM's menu, not a tenant's: it exists for a window that has published nothing at all,
// which is every window on this desktop today bar the pulse one. A tenant that wants its own app
// menu hands one to [`publish_app`] and owns the id space; otherwise this tree is served and picks
// are delivered to [`app_pick`] rather than to any `on_pick`.
// ---------------------------------------------------------------------------

/// SO3 — the default app menu's `About` row. Reserved: a tenant that publishes its own app menu
/// through [`publish_app`] never sees these ids, so no id space is taken from anyone.
pub const APP_ITEM_ABOUT: u32 = 0xA0;
/// SO3 — the default app menu's `Quit` row: it closes the window, on the close box's own path.
pub const APP_ITEM_QUIT: u32 = 0xA1;

/// SO3 — **the menu every window gets.** `About <name>`, a keyline, `Quit`.
const APP_MENU_DEFAULT: &[MenuItem] = &[
    MenuItem { id: APP_ITEM_ABOUT, label: "About", flags: FLAG_APPNAME },
    MenuItem { id: 0, label: "", flags: FLAG_SEPARATOR },
    MenuItem { id: APP_ITEM_QUIT, label: "Quit", flags: 0 },
];

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
    /// SO3 — the tenant's OWN app menu, or `None` for the WM's [`APP_MENU_DEFAULT`]. Set by
    /// [`publish_app`] and preserved across a re-[`publish`], because a publisher moves a
    /// [`FLAG_CHECKED`] mark by re-publishing its tree and must not lose its app menu doing so.
    app: Option<&'static [MenuItem]>,
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

/// SO3 — **the window whose NAME the bar is showing**, or [`wm::WIN_NONE`]. Published by
/// [`super::menubar::compose`] from the same table scan the caption comes from, and read by the
/// input path — so a press on the app title never takes the window table's lock to find out which
/// row `Quit` must reap.
///
/// NOT the same window as [`BAR_OWNER`]: that is the frontmost PUBLISHER and this is the frontmost
/// FOCUSED row, and they differ whenever a window with menus sits behind a window without them.
static APP_OWNER: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
/// SO3 — the caption's bytes, as two words. [`wm::MAX_TITLE`] is 16 (asserted at the file's foot),
/// so the whole name fits in a pair of atomics and the app title box is laid out, hit-tested and
/// painted WITHOUT a lock — the property [`open_rect`] had to be rebuilt to get (PANEL V-2), given
/// to the caption by construction rather than recovered later.
///
/// A reader can see a half-updated NAME (the two words are not written atomically together): the
/// consequence is one composite's worth of wrong glyphs in a box whose width is taken from the same
/// snapshot, never a wrong OWNER — the id is its own atomic and `Quit` reads only that.
static APP_NAME_LO: AtomicU64 = AtomicU64::new(0);
/// SO3 — the caption's high eight bytes. See [`APP_NAME_LO`].
static APP_NAME_HI: AtomicU64 = AtomicU64::new(0);
/// SO3 — how many of [`APP_NAME_LO`]/[`APP_NAME_HI`]'s bytes are the name.
static APP_NAME_LEN: AtomicUsize = AtomicUsize::new(0);
/// SO3 — is the OPEN dropdown the app menu (box 0) rather than one of the tenant's titles? Stored
/// beside [`OPEN_TITLE`] and cleared with it, so "which menu is down" is never inferred from a
/// layout that may have changed under it.
static OPEN_APP: AtomicBool = AtomicBool::new(false);

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
    // SO3 — a re-publish must not silently drop the tenant's app menu. Re-publishing is how a
    // publisher moves a `FLAG_CHECKED` mark (see this function's doc), so it happens on every pick;
    // rebuilding the slot from scratch would make a custom app menu survive exactly until the first
    // selection in some unrelated title.
    let app = g[k].and_then(|t| t.app);
    g[k] = Some(Tree { titles, on_pick, app });
    OWNERS[k].store(owner, Ordering::Release);
    if !replaced {
        LIVE.fetch_add(1, Ordering::Relaxed);
    }
    PUBLISHES.fetch_add(1, Ordering::Relaxed);
    serial_println!(
        "[winmenu] publish owner={} titles={} items={} slot={} replaced={} app-menu={}",
        owner, titles.len(), items, k, replaced,
        if app.is_some() { "custom" } else { "default" }
    );
    true
}

/// SO3 — **give `owner` its OWN app menu**, replacing the WM's [`APP_MENU_DEFAULT`] under the bar's
/// app title. `true` when the registry took it.
///
/// The tenant must already hold a slot ([`publish`] first): an app menu is a property of a
/// registered publisher, not a second way to become one, and a slot minted here would be a tree
/// with no titles — which [`publish`] itself refuses. Picks go to that slot's `on_pick`, so the
/// tenant owns the whole id space of the menu it authored and [`APP_ITEM_QUIT`] means nothing in it;
/// a tenant that wants the close behaviour calls [`super::wm::close`] from its own handler.
///
/// Unused by any tenant today, and deliberately so — it is the ruling's *"unless the tenant
/// publishes its own"* clause, built as the seam it names rather than left to be invented later by
/// whoever needs it first.
pub fn publish_app(owner: wm::WinId, items: &'static [MenuItem]) -> bool {
    if !has_tree(owner) {
        serial_println!("[winmenu] publish_app REFUSE owner={} reason=no-tree", owner);
        return false;
    }
    if items.is_empty() || items.len() > MENU_ITEMS_MAX {
        serial_println!(
            "[winmenu] publish_app REFUSE owner={} reason=items items={} max={}",
            owner, items.len(), MENU_ITEMS_MAX
        );
        return false;
    }
    for it in items.iter() {
        if it.label.len() > MENU_LABEL_MAX {
            serial_println!(
                "[winmenu] publish_app REFUSE owner={} reason=item-label len={} max={}",
                owner, it.label.len(), MENU_LABEL_MAX
            );
            return false;
        }
    }
    let mut g = match TREES.try_lock() {
        Some(g) => g,
        None => {
            note_refusal("publish_app");
            return false;
        }
    };
    for k in 0..WINMENU_MAX {
        if OWNERS[k].load(Ordering::Relaxed) == owner {
            if let Some(t) = g[k].as_mut() {
                t.app = Some(items);
                serial_println!(
                    "[winmenu] publish owner={} titles={} items={} slot={} replaced=true app-menu=custom",
                    owner, t.titles.len(), items.len(), k
                );
                return true;
            }
        }
    }
    false
}

/// SO3 — **publish the window the bar's app title names, and the name itself.**
///
/// Called once per bar compose by [`super::menubar::compose`], off the `wm::dock_scan` it already
/// runs. Stores only ([`APP_OWNER`] and the two name words) — no lock, no composite: this runs
/// INSIDE `strip::compose_all`, so an owner change clears menu STATE through [`dismiss_state`] on
/// PANEL V-1's rule, exactly as [`set_bar_owner`] does, and [`compose`] discharges the erase later
/// in the same pass.
pub fn set_app_window(id: wm::WinId, name: &[u8]) {
    let n = name.len().min(wm::MAX_TITLE);
    let (mut lo, mut hi) = (0u64, 0u64);
    for (i, &b) in name[..n].iter().enumerate() {
        if i < 8 {
            lo |= (b as u64) << (8 * i);
        } else {
            hi |= (b as u64) << (8 * (i - 8));
        }
    }
    APP_NAME_LO.store(lo, Ordering::Relaxed);
    APP_NAME_HI.store(hi, Ordering::Relaxed);
    APP_NAME_LEN.store(n, Ordering::Release);
    let was = APP_OWNER.swap(id, Ordering::Release);
    if was == id {
        return;
    }
    // The app title has moved to another window: a dropdown still hanging from the old name would be
    // one window's menu under another window's title, and `Quit` in it would reap the wrong row.
    if OPEN_APP.load(Ordering::Relaxed) && OPEN_TITLE.load(Ordering::Relaxed) != 0 {
        dismiss_state("app-owner-change");
    }
    if id != wm::WIN_NONE {
        let (buf, len) = app_name();
        serial_println!(
            "[winmenu] app-menu owner={} name={} kind={}",
            id,
            core::str::from_utf8(&buf[..len]).unwrap_or("?"),
            if app_menu_is_custom(id) { "custom" } else { "default" }
        );
    }
}

/// SO3 — the caption, as bytes. Lock-free; see [`APP_NAME_LO`].
fn app_name() -> ([u8; wm::MAX_TITLE], usize) {
    let mut out = [0u8; wm::MAX_TITLE];
    let len = APP_NAME_LEN.load(Ordering::Acquire).min(wm::MAX_TITLE);
    let lo = APP_NAME_LO.load(Ordering::Relaxed);
    let hi = APP_NAME_HI.load(Ordering::Relaxed);
    for (i, slot) in out.iter_mut().enumerate().take(len) {
        *slot = if i < 8 { (lo >> (8 * i)) as u8 } else { (hi >> (8 * (i - 8))) as u8 };
    }
    (out, len)
}

/// SO3 — does `owner` serve its own app menu? Lock-free on the common answer: a window that
/// published nothing cannot have one, and [`has_tree`] short-circuits on [`LIVE`].
fn app_menu_is_custom(owner: wm::WinId) -> bool {
    matches!(tree_of(owner, "app-kind"), Look::Found(t) if t.app.is_some())
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

/// SO2 — **the type this file draws in, taken from the BAR.**
///
/// It used to be taken from [`crystal`]'s dropdown constants. Those resolve to the same atlas and
/// the same cell, so the bar titles and the drop-down looked nearly right — but "nearly" was the
/// defect. The bar's own text is BOLD ([`menubar::BAR_BOLD`], macOS's rule for the app name) and the
/// crystal export carries no weight at all, so the one attribute the third party could not pass on
/// was the one that differed. Peter, reading `render7`: *"misplaced and different font from main app
/// menu item font"*. Sourced from the bar now, weight included, so a client of the bar draws the
/// BAR'S text by construction; the file's foot asserts the cell still matches the dropdown's row
/// metrics, which are imported from `crystal` and are what [`ITEM_H`] is built on.
const CELL_W: usize = menubar::BAR_CELL_W;
const CELL_H: usize = menubar::BAR_CELL_H;
const FACE: super::font::Face = menubar::BAR_FACE;
/// SO2 — the bar's weight, for the titles AND the drop-down's rows. See [`CELL_W`].
const BOLD: bool = menubar::BAR_BOLD;

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
    /// The window the TENANT boxes belong to (the frontmost publisher).
    pub owner: wm::WinId,
    /// SO3 — box `0` is the APP MENU (the bar's own caption), so the tenant's published titles start
    /// at box 1. `false` when there is no focused window to name one for, in which case box 0 is the
    /// tenant's first title exactly as it was before this arc.
    pub app: bool,
    /// SO3 — the window the app menu belongs to, and the row its `Quit` reaps. [`wm::WIN_NONE`] when
    /// [`app`](Self::app) is `false`.
    pub app_owner: wm::WinId,
    /// Box origin and width, panel-absolute. Height is the bar's, and `y` is the bar's.
    pub x: [usize; BAR_BOXES_MAX],
    pub w: [usize; BAR_BOXES_MAX],
    pub label: [[u8; MENU_LABEL_MAX]; BAR_BOXES_MAX],
    pub label_len: [usize; BAR_BOXES_MAX],
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
            app: false,
            app_owner: wm::WIN_NONE,
            x: [0; BAR_BOXES_MAX],
            w: [0; BAR_BOXES_MAX],
            label: [[0; MENU_LABEL_MAX]; BAR_BOXES_MAX],
            label_len: [0; BAR_BOXES_MAX],
            bar: (0, 0, 0, 0),
        }
    }

    /// SO3 — is the APP MENU the one that is down? Read by [`super::menubar::compose_row`], which
    /// owns the caption's glyphs and inks them as a lit title while its menu hangs from them.
    #[inline]
    pub fn app_open(&self) -> bool {
        self.app && self.open == 1
    }

    /// SO3 — is box `k` the app menu?
    #[inline]
    fn is_app_box(&self, k: usize) -> bool {
        self.app && k == 0
    }

    /// Which title box, if any, panel point `(px, py)` lands in.
    fn hit(&self, px: usize, py: usize) -> Option<usize> {
        let (_, by, _, bh) = self.bar;
        if py < by || py >= by + bh {
            return None;
        }
        (0..self.n).find(|&k| px >= self.x[k] && px < self.x[k] + self.w[k])
    }

    /// SO2 — **the panel-absolute x of box `k`'s GLYPHS**, which is what the drop-down under it is
    /// anchored to and what the `[winmenu] open … title-x=` witness prints.
    ///
    /// The box's own left edge is [`TPAD`] further left; anchoring the menu there put its frame — and
    /// with it every row of text inside it — visibly off the title it hangs from. A menu belongs to
    /// the word the operator pressed, so the word's first pixel column is the anchor.
    #[inline]
    fn text_x(&self, k: usize) -> usize {
        self.x[k] + TPAD
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
    let app_owner = APP_OWNER.load(Ordering::Acquire);
    // The fast path, and the tenant-law cost claim this module is held to. SO3 widened what counts
    // as "nothing to lay out" — a window with no menus still has a NAME — but not what it costs: two
    // relaxed loads and a return on a desktop with no focused window and no publisher, and the bar's
    // own `ENABLED` load below on a boot with no bar at all. No lock on either.
    if LIVE.load(Ordering::Relaxed) == 0 && app_owner == wm::WIN_NONE {
        return s;
    }
    let Some(bar) = menubar::strip_rect(pw, ph) else {
        return s;
    };
    s.bar = bar;
    let (bx, _by, bw, _bh) = bar;
    let limit = menubar::menus_right_limit(bar);

    // SO3 — BOX 0 IS THE APP MENU: the caption's own glyphs, given a press box. The box is the name
    // plus one `TPAD` either side, NOT the whole fixed caption slot — the slot runs to `menus_x0()`
    // and is mostly empty, and a press target the operator cannot see is a press target that fires
    // when they meant to hit the bar. Laid out from the caption the bar published, so the box and
    // the glyphs it frames come from one fact.
    let (name, name_len) = app_name();
    if app_owner != wm::WIN_NONE && name_len > 0 {
        let x0 = (bx + menubar::caption_x0()).saturating_sub(TPAD);
        let w = name_len * CELL_W + 2 * TPAD;
        if x0 + w <= limit && x0 + w <= bx + bw {
            s.x[0] = x0;
            s.w[0] = w;
            s.label_len[0] = name_len.min(MENU_LABEL_MAX);
            s.label[0][..s.label_len[0]].copy_from_slice(&name[..s.label_len[0]]);
            s.app = true;
            s.app_owner = app_owner;
            s.n = 1;
        }
    }

    let owner = bar_owner();
    // PANEL V-3 — DECLINED and EMPTY are different snapshots. `busy` carries the difference to the
    // readers; nothing here concludes "no publisher" from a lock it could not take. SO3: a `Busy`
    // registry no longer voids the whole snapshot — the app box came from atomics and is still true
    // — but it still declines every reader that would ACT on the tenant half being empty.
    let tree = match tree_of(owner, "bar_boxes") {
        Look::Found(t) => Some(t),
        Look::Absent => None,
        Look::Busy => {
            s.busy = true;
            None
        }
    };
    if let Some(tree) = tree {
        s.owner = owner;
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
    }
    // Which box, if any, is DOWN. SO3 adds the second half of the question: the app menu and a
    // tenant title are different surfaces that share one index space, so "box 1 is open" is only
    // true if the box 1 this layout produced is the same KIND of box the open was taken on. An app
    // menu whose window stopped being the caption, or a tenant title whose publisher went, reports
    // closed here and `compose`'s teardown takes the pixels back.
    let open = OPEN_TITLE.load(Ordering::Relaxed) as usize;
    let open_owner = OPEN_OWNER.load(Ordering::Relaxed);
    let is_app = OPEN_APP.load(Ordering::Relaxed);
    let (wanted, kind_ok) = if is_app { (s.app_owner, open == 1) } else { (owner, !s.is_app_box(open.saturating_sub(1))) };
    s.open = if open != 0 && open <= s.n && kind_ok && wanted != wm::WIN_NONE && open_owner == wanted {
        open
    } else {
        0
    };
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

/// SO3 — an item's rendered width in GLYPHS, with [`FLAG_APPNAME`]'s live suffix folded in.
#[inline]
fn item_glyphs(it: &MenuItem, name_len: usize) -> usize {
    if it.flags & FLAG_APPNAME != 0 && name_len > 0 {
        it.label.len() + 1 + name_len
    } else {
        it.label.len()
    }
}

/// The dropdown's extent for one title's `items`, in px. `name_len` is the live app name's length,
/// which [`FLAG_APPNAME`] rows carry as a suffix (SO3).
fn drop_extent(items: &[MenuItem], name_len: usize) -> (usize, usize) {
    let mut widest = 0usize;
    let mut h = 2 * BORDER;
    for it in items.iter() {
        if it.flags & FLAG_SEPARATOR != 0 {
            h += SEP_H;
            continue;
        }
        h += ITEM_H;
        let g = item_glyphs(it, name_len);
        if g > widest {
            widest = g;
        }
    }
    let w = 2 * BORDER + 2 * PADX + (widest + CHECK_GLYPHS) * CELL_W;
    (w, h)
}

/// SO3 — **where a pick from box `k` goes.**
#[derive(Clone, Copy)]
enum Sink {
    /// The WM's own default app menu: [`app_pick`] on the named window.
    App(wm::WinId),
    /// The tenant's handler, called with the item id the tenant itself chose.
    Tenant(fn(u32)),
}

/// SO3 — what box `k` drops, and where its picks go. Three-valued for [`Look`]'s reason.
enum Menu {
    Found(&'static [MenuItem], Sink),
    Absent,
    Busy,
}

/// SO3 — **resolve box `k` to its rows and its sink.** THE one place the app menu and a tenant title
/// are told apart, so every consumer — the layout, the press router, the painter and the damage
/// signature — asks the same question and cannot answer it differently.
///
/// The default app menu costs NO lock on the common boot: `tree_of` short-circuits on [`LIVE`], so a
/// window that published nothing resolves straight to [`APP_MENU_DEFAULT`].
fn menu_of(s: &BarSnapshot, k: usize, site: &str) -> Menu {
    if s.is_app_box(k) {
        return match tree_of(s.app_owner, site) {
            Look::Found(t) => match t.app {
                Some(items) => Menu::Found(items, Sink::Tenant(t.on_pick)),
                None => Menu::Found(APP_MENU_DEFAULT, Sink::App(s.app_owner)),
            },
            Look::Absent => Menu::Found(APP_MENU_DEFAULT, Sink::App(s.app_owner)),
            Look::Busy => Menu::Busy,
        };
    }
    let ti = k - (s.app as usize);
    match tree_of(s.owner, site) {
        Look::Found(t) => match t.titles.get(ti) {
            Some(title) => Menu::Found(title.items, Sink::Tenant(t.on_pick)),
            None => Menu::Absent,
        },
        Look::Absent => Menu::Absent,
        Look::Busy => Menu::Busy,
    }
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
    if s.busy && !s.is_app_box(s.open.saturating_sub(1)) {
        return Layout::Busy;
    }
    if !is_open() || s.open == 0 {
        return Layout::Gone;
    }
    let k = s.open - 1;
    let items = match menu_of(s, k, "open_rect") {
        Menu::Found(items, _) => items,
        Menu::Absent => return Layout::Gone,
        Menu::Busy => return Layout::Busy,
    };
    let (_, name_len) = app_name();
    let (mw, mh) = drop_extent(items, name_len);
    let (_bx, by, _bw, bh) = s.bar;
    let my = by + bh;
    if mw > pw || my + mh > ph {
        return Layout::Gone;
    }
    // SO2 — anchored to the TITLE'S GLYPHS, not to its press box. See [`BarSnapshot::text_x`]: the
    // box carries `TPAD` of padding the operator never sees, and hanging the menu off the padding is
    // what put it visibly beside the word it belongs to. The right-edge clamp is unchanged.
    let tx = s.text_x(k);
    let mx = if tx + mw > pw { pw.saturating_sub(mw) } else { tx };
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
    let is_app = s.is_app_box(k);
    let owner = if is_app { s.app_owner } else { s.owner };
    OPEN_APP.store(is_app, Ordering::Release);
    OPEN_OWNER.store(owner, Ordering::Release);
    OPEN_TITLE.store((k + 1) as u32, Ordering::Release);
    OPENS.fetch_add(1, Ordering::Relaxed);
    let (pw, ph) = panel();
    // PANEL V-2 — publish the rect HERE, in task context, off a snapshot taken AFTER the open is
    // visible (`s` was read before `OPEN_TITLE` was stored, so its `open` is still 0). Every reader
    // downstream takes it as one atomic.
    let fresh = bar_boxes(pw, ph);
    let (mx, my) = republish_open_rect(pw, ph, &fresh).map(|r| (r.0, r.1)).unwrap_or((0, 0));
    let items = match menu_of(s, k, "open") {
        Menu::Found(items, _) => items.len(),
        _ => 0,
    };
    // SO2/SO3 — the witness carries the two numbers the geometry claim is made of (`title-x` is the
    // title's first GLYPH column; `x` must equal it unless the right-edge clamp fired) and the type
    // the rows are set in, so a capture states the fix instead of leaving it to a screenshot.
    serial_println!(
        "[winmenu] open title={} items={} at ({},{}) title-x={} font={} kind={} owner={}",
        core::str::from_utf8(s.label_of(k)).unwrap_or("?"),
        items, mx, my, s.text_x(k), menubar::BAR_FONT_NAME,
        if is_app { "app" } else { "title" }, owner
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
    let was_app = OPEN_APP.swap(false, Ordering::AcqRel);
    DISMISSES.fetch_add(1, Ordering::Relaxed);
    serial_println!(
        "[winmenu] dismiss reason={} kind={} owner={}",
        reason,
        if was_app { "app" } else { "title" },
        owner
    );
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
    // SO3 — the fast path is now TWO atomics, and it had to be. `LIVE == 0` alone said "no window in
    // this kernel has a menu", which stopped being true the moment every window got a name-menu: on
    // the desktop this ruling was written for NOTHING has ever published, so a `LIVE`-only guard
    // would have declined every press on the app title and shipped an unreachable feature that
    // type-checked. The boot that pays nothing is still the boot with no bar and no focused window,
    // which is what the second load answers.
    if x < 0 || y < 0 || (LIVE.load(Ordering::Relaxed) == 0 && APP_OWNER.load(Ordering::Relaxed) == wm::WIN_NONE) {
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
                // SO3 — the snapshot and the published rect DISAGREE: `is_open()` is true and a rect
                // is on the panel, but this pass's layout says no box is down (the owner moved, the
                // publisher went, the panel shrank). Before the app menu that mismatch could only
                // mis-deliver a pick to the wrong title; now box 0 can be `Quit`, and a destructive
                // pick off a snapshot that does not agree the menu is open is not a risk worth
                // carrying. Tear it down instead — which is what the next compose would do anyway.
                if s.open == 0 {
                    dismiss("outside");
                    return true;
                }
                let k = s.open - 1;
                let (items, sink) = match menu_of(&s, k, "press") {
                    Menu::Found(items, sink) => (items, sink),
                    // Contended between the snapshot and here: decline, keep the menu down.
                    Menu::Busy => return true,
                    Menu::Absent => {
                        dismiss("outside");
                        return true;
                    }
                };
                let owner = if s.is_app_box(k) { s.app_owner } else { s.owner };
                return match item_at_row(items, py - my, mh) {
                    Some(i)
                        if items[i].flags & (FLAG_SEPARATOR | FLAG_DISABLED) == 0 =>
                    {
                        let id = items[i].id;
                        PICKS.fetch_add(1, Ordering::Relaxed);
                        dismiss("pick");
                        match sink {
                            Sink::Tenant(f) => {
                                serial_println!("[winmenu] pick owner={} id={} label={}", owner, id, items[i].label);
                                f(id);
                            }
                            // SO3 — the WM's own app menu. The witness names the ROUTE, not just the
                            // row, because `Quit` is the one pick in this kernel that destroys the
                            // thing that was picked from: a capture must be able to pair it with the
                            // `[wc-a] close win=` the close box emits and see the SAME window id.
                            Sink::App(win) => {
                                serial_println!(
                                    "[winmenu] pick owner={} id={} label={} -> {} win={}",
                                    owner, id, items[i].label,
                                    if id == APP_ITEM_QUIT { "close" } else { "about" }, win
                                );
                                app_pick(win, id);
                            }
                        }
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
    // SO3 — **the BRAND MARK keeps its corner.** Before this arc the title boxes began at
    // `menus_x0()` (193 px in) and could not reach the crystal; the app box begins one `TPAD` before
    // the caption's glyphs (22 px — 28 − TPAD 6, since CRYSTALFIX 1046f81c; was 34 under bb513370), and `crystal_corner_abs` runs to `TITLE_X0` (40) — so six pixels
    // now belong to two surfaces, and `strip::press_route` asks THIS arm first. Declining them keeps
    // the SHARD menu reachable at every pixel it has always been reachable at; what is lost is six
    // pixels of the app box's left padding, which carries no glyph.
    if let Some((cx, cy, cw, ch)) = menubar::crystal_corner_abs(pw, ph) {
        if px >= cx && px < cx + cw && py >= cy && py < cy + ch {
            return false;
        }
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

/// SO3 — **deliver a pick from the WM's own default app menu.**
///
/// TASK CONTEXT ONLY, and for a harder reason than [`dismiss`]'s: [`wm::close`] runs a drain barrier
/// that spins on in-flight composites and then composites itself. Its one caller is [`press_at`]'s
/// pick arm, which is the click router — the same context the close box's own arm runs in
/// (`arch/x86_64/syscall.rs::wc_click_route_at`, `video/quarry/live.rs::press_route`) — and the
/// dropdown was already dismissed before this runs, so no menu state is live across the barrier.
///
/// `Quit` takes the CLOSE BOX'S path, so the ruling's *"closes the window / ends the tenant"* means exactly what the red disc means and emits the same `[wc-a] close win=` witness. The registry is
/// released first: a tree left behind would keep [`LIVE`] up and make `bar_boxes` lay out titles for a row that no longer exists. A30FIX — "the close box's path" is now read LITERALLY: for a window whose
/// OWNING MODULE has its own `close()` (the pulse instrument is the one such window today), that module's close IS the close box's path and a bare `wm::close` is not it. Bare, this arm left `pulsewin::ARMED` set while A29's WINID holder registry cleared `pulsewin::WIN` from inside `wm::close` — a window-less ARMED state published by the close itself — and the next render pass re-minted the window. Wire, render8: `[wm] close win=3 gen=5 … holders-cleared=1 names=pulsewin` (7296) -> `[wm] alloc win=3 gen=6` (7299) -> `[wc-a] create` (7300) -> `[winmenu] app-menu quit win=3 closed=true` (7303) -> `[pulsewin] open win=3` (7305); again at 7405->7418 and 7499->7511. That is Peter's *"menu quit does not quit pulse"*, and it leaked the surface every time as well: the
/// registry clears an id cell, it cannot free a `Vec`. ⚠ this block is line-NEUTRAL — the A30FIX prose was folded into the four lines that were already here, because `winmenu.rs` is a bare `pub mod` and IS compiled into the knob-off `kernel8.img`, where a doc line added above `app_pick` moves every `panic::Location` below it — PARITY.md §5.3.
fn app_pick(win: wm::WinId, id: u32) {
    match id {
        APP_ITEM_QUIT => {
            clear(win); #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] let closed = (crate::video::pulsewin::win() == win && crate::video::pulsewin::close()) || wm::close(win); // A30FIX — see this fn's header. The OWNING MODULE's close runs first when this is its window: `pulsewin::close()` disarms the latch BEFORE it swaps the id, then does the same `winmenu::clear` + `wm::close` + surface teardown the red disc does, so Quit and the disc are now ONE path emitting one witness pair. `||` short-circuits, so `wm::close` is not called twice; a non-pulse window (and every window on a desktop that never armed the instrument, where `win()` is `WIN_NONE` and no real id can equal it) takes the right-hand side exactly as before.
            #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] let closed = wm::close(win); // A30FIX — the KNOB-OFF twin, and the reason the arm above is a whole `let` rather than a folded sub-expression. The gate is `video/mod.rs`'s own predicate on `pub mod pulsewin`; `winmenu.rs` is a bare `pub mod` and IS compiled into the knob-off `kernel8.img`, where that module does not exist. This line is token-for-token the statement that stood here before the arc, so the knob-off image gets not one changed byte — not an inference about what LLVM folds. ⚠ Both arms are FOLDED onto lines that already existed: knob-off line numbers are load-bearing (panic `Location`) — PARITY.md §5.3.
            serial_println!("[winmenu] app-menu quit win={} closed={}", win, closed);
        }
        APP_ITEM_ABOUT => {
            let (name, len) = app_name();
            serial_println!(
                "[winmenu] app-menu about win={} name={}",
                win,
                core::str::from_utf8(&name[..len]).unwrap_or("?")
            );
        }
        other => serial_println!("[winmenu] app-menu REFUSE win={} id={} reason=unknown-item", win, other),
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
        // SO3 — box 0 is the APP MENU, and its glyphs are the BAR'S caption: `menubar::compose_row`
        // draws them a few lines after this call, from its own model, and inks them as a lit title
        // when `BarSnapshot::app_open`. Drawing them here too would blend the same anti-aliased
        // glyphs twice into one row and thicken them. The FILL above is still ours — it is title
        // chrome, not caption text, and it has to land before the band return.
        if s.is_app_box(k) {
            continue;
        }
        let ink = if open { theme::BEVEL_LIGHT } else { theme::TITLE_TEXT_ACTIVE };
        // SO2 — the BAR'S weight. See [`BOLD`].
        super::font::draw_row(out, w, s.label_of(k), x0 + TPAD, j - ty0, ink, BOLD, FACE);
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
            // SO3 — the OPEN menu's owner, not the snapshot's tenant field: an app menu on a window
            // that published nothing leaves `s.owner` at `WIN_NONE`, and a ledger line reading
            // `state=open owner=0` beside a menu plainly on the glass is a witness that lies.
            "state=open kind={} owner={} publishes={} clears={} opens={} dismisses={} picks={} refusals={}",
            if OPEN_APP.load(Ordering::Relaxed) { "app" } else { "title" },
            OPEN_OWNER.load(Ordering::Relaxed),
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

    let k = s.open - 1;
    let Menu::Found(items, _) = menu_of(&s, k, "compose") else {
        return false;
    };
    let (name, name_len) = app_name();
    let t1 = crate::arch::now_cycles();
    if !strip::paint("winmenu", r, |out, j| compose_row(out, r, items, j, &name[..name_len])) {
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
    h = strip::fnv1a_u64(h, s.app_owner as u64);
    h = strip::fnv1a_u64(h, s.open as u64);
    if let Menu::Found(items, _) = menu_of(s, s.open.saturating_sub(1), "sig") {
        for it in items.iter() {
            h = strip::fnv1a_u64(h, it.id as u64);
            h = strip::fnv1a_u64(h, it.flags as u64);
            for &b in it.label.as_bytes() {
                h = strip::fnv1a(h, b);
            }
            // SO3 — a `FLAG_APPNAME` row's rendered text includes the LIVE caption, so the caption is
            // part of this surface's content: a rename with the menu down must repaint it.
            if it.flags & FLAG_APPNAME != 0 {
                let (name, len) = app_name();
                for &b in name[..len].iter() {
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
fn compose_row(out: &mut [u32], r: strip::Rect, items: &[MenuItem], j: usize, name: &[u8]) {
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
    // SO2 — every glyph on this surface is now drawn at the BAR'S weight, the check mark included:
    // the drop-down is the bar's own text hanging below the bar, not a second typeface.
    if it.flags & FLAG_CHECKED != 0 {
        super::font::draw_row(out, w, CHECK_MARK, BORDER + PADX, sy, ink, BOLD, FACE);
    }
    let lx = BORDER + PADX + CHECK_GLYPHS * CELL_W;
    super::font::draw_row(out, w, it.label.as_bytes(), lx, sy, ink, BOLD, FACE);
    // SO3 — `About <app>`. The suffix is composed HERE, at paint time, from the caption the bar
    // published, because a `MenuItem`'s label is `&'static str` and the registry allocates nothing.
    if it.flags & FLAG_APPNAME != 0 && !name.is_empty() {
        let nx = lx + (it.label.len() + 1) * CELL_W;
        super::font::draw_row(out, w, name, nx, sy, ink, BOLD, FACE);
    }
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
// Witness
// ---------------------------------------------------------------------------

/// WINMENU fixture — **A10, SO2 and SO3, each driven through the seam a real gesture uses.**
///
/// Reached from [`super::crystal::selftest`]'s tail, which is the same arrangement `dock::selftest`
/// uses to reach `menubar::selftest`: the x86 battery calls one furniture fixture and the family
/// chains, so a new surface does not need a line in a file another lane owns.
///
/// Five legs, each able to red on its own. It mints its own window, so nothing the operator's boot
/// put on the panel is at risk, and it closes that window itself if `Quit` did not.
///
/// 1. **every window has an app menu** (SO3) — a window that has published NOTHING lays out a title
///    box carrying its own name. This is the ruling's whole claim, and it is the leg that reds if
///    the default tree, the caption publish or the box layout goes.
/// 2. **a ROUTED press opens it** — through [`super::strip::press_route`], the one shared furniture
///    router both arch click paths call, not through [`press_at`] directly. `crystal::selftest`'s
///    own header records why that distinction is load-bearing.
/// 3. **the drop-down hangs under the title's GLYPHS** (SO2) — `x == title-x` (or the right-edge
///    clamp fired) and `y` is the bar's bottom edge. The anchor was the title's press BOX before this
///    arc, which is `TPAD` further left, and that is the misplacement Peter read off `render7`.
/// 4. **`<Esc>` dismisses, through [`super::strip::key_escape`]** (A10) — the seam BOTH arch routers
///    ask. The Orin defect was never in this arm: it was that the board's key drain never reached the
///    seam. This leg pins the arm so the wiring fix has something to be wired TO.
/// 5. **`Quit` closes the window, on the close box's own path** (SO3) — the pick reaps the row, so
///    `wm::info` answers `None` and `[wc-a] close win=` is on the wire beside `[winmenu] pick …
///    label=Quit -> close win=`. A `Quit` that dismissed the menu and did nothing else would pass
///    legs 1-4 and red here.
#[cfg(feature = "witness")]
pub fn selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    // 8x8 px, 32 B a row: `create_inner`'s extent contract is `w * 4 <= stride` (8*4 == 32) and
    // `h * stride <= surf_len` (8*32 == 256 B == 64 `u32`) — `dock::selftest`'s own fixture surface,
    // same shape and the same size. A short buffer makes `wm::create` answer `WIN_NONE`, which is
    // indistinguishable at the call site from a full table, so the size is asserted at the file's
    // foot rather than trusted to this comment.
    static SURF: [u32; 64] = [0; 64];
    const FIX_W: usize = 8;
    const FIX_STRIDE: usize = 32;
    const _: () = assert!(FIX_W * 4 <= FIX_STRIDE && FIX_W * FIX_STRIDE <= 64 * 4);
    let (pw, ph) = panel();
    if pw == 0 || ph == 0 {
        serial_println!(":: WINMENU: no panel :: SKIP ::");
        return;
    }
    let saved_bar = menubar::enabled();
    menubar::set_enabled(true);
    // Any id in the reserved band is kernel furniture; `dock::selftest`'s reasoning for not reusing
    // `KERNEL_OWNER_CONSOLE` applies verbatim.
    const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x51;
    let win = wm::create(
        OWNER,
        SURF.as_ptr() as usize,
        core::mem::size_of_val(&SURF),
        FIX_W as u32,
        FIX_W as u32,
        FIX_STRIDE as u32,
        b"gate",
    );
    if win == wm::WIN_NONE {
        // `create` answers `WIN_NONE` for a full table AND for a surface that fails the extent
        // contract; the fixture's own buffer is asserted at the file's foot, so on this build the
        // only reachable cause is the table. Named that way, and a SKIP rather than a FAIL: a
        // battery that filled the table before this ran is not this surface's defect.
        serial_println!(":: WINMENU: wm::create declined (table full) :: SKIP ::");
        menubar::set_enabled(saved_bar);
        return;
    }
    // FOCUS, then a composite — and NOT a direct `set_app_window`, which was the first cut of this
    // fixture and was worse than useless. `menubar::compose` publishes the caption from its own
    // `wm::dock_scan` on every pass, so a forced value survives exactly until the next composite —
    // and `open_title` drives one. The first cut therefore opened the menu and had it torn straight
    // back down by `app-owner-change`, reading `routed_open=false`. Focusing the row instead makes
    // the bar name this window ITSELF, which is both the reason the leg now holds and a strictly
    // better proof: the PUBLISH half of SO3 is now on the live path too, not stubbed.
    let saved_focus = wm::focus_asid();
    wm::focus_changed(OWNER);
    wm::composite();

    // Leg 1 — the app box exists, is box 0, and carries the window's own name.
    let s = bar_boxes(pw, ph);
    let leg_box = s.app && s.app_owner == win && s.n >= 1 && s.label_of(0) == b"gate";

    // Leg 2 — a routed press opens it. The box CENTRE, so the press clears the brand-mark corner
    // this arm declines (see `press_at`'s closed arm).
    let (_bx, by, _bw, bh) = s.bar;
    let px = (s.x[0] + s.w[0] / 2) as i32;
    let py = (by + bh / 2) as i32;
    let open_consumed = strip::press_route(px, py);
    let leg_open = leg_box
        && open_consumed
        && is_open()
        && OPEN_APP.load(Ordering::Relaxed)
        && OPEN_OWNER.load(Ordering::Relaxed) == win;

    // Leg 3 — SO2 geometry. `title-x` is the caption's first glyph column; the clamp is the one
    // legitimate way `x` may differ from it, so it is named rather than tolerated silently.
    let r = open_rect(pw, ph);
    let leg_geom = match r {
        Some((mx, my, mw, _mh)) => (mx == s.text_x(0) || mx + mw == pw) && my == by + bh,
        None => false,
    };

    // Leg 4 — A10. Through the shared key seam, which is what both arch routers ask.
    let esc_consumed = strip::key_escape(crate::pal::Event::Key(0x1b));
    let leg_esc = leg_open && esc_consumed && !is_open();

    // Leg 5 — SO3. Reopen, then press the `Quit` row: `APP_MENU_DEFAULT`'s index 2, its vertical
    // middle, taken from the SAME `item_top`/`ITEM_H` the painter and the hit-test use.
    let _ = strip::press_route(px, py);
    let leg_quit = match (is_open(), open_rect(pw, ph)) {
        (true, Some((mx, my, mw, _mh))) => {
            let qy = my + item_top(APP_MENU_DEFAULT, 2) + ITEM_H / 2;
            let consumed = strip::press_route((mx + mw / 2) as i32, qy as i32);
            consumed && !is_open() && wm::info(win).is_none()
        }
        _ => false,
    };

    // Restore. `Quit` should have reaped the row; if a leg red left it standing, this fixture takes
    // it back rather than leaving an 8x8 square on the operator's desktop.
    if wm::info(win).is_some() {
        wm::close(win);
    }
    dismiss("selftest");
    wm::focus_changed(saved_focus);
    menubar::set_enabled(saved_bar);

    let (rx, ry, rw, rh) = r.map(|(x, y, w, h)| (x, y, w, h)).unwrap_or((0, 0, 0, 0));
    let ok = leg_box && leg_open && leg_geom && leg_esc && leg_quit;
    serial_println!(
        ":: WINMENU: win={} name=gate box={}x{}+{} title-x={} drop={}x{}+{}+{} font={} panel={}x{} \
         app_box={} routed_open={} geometry={} escape={} quit_closes={} :: {} ::",
        win, s.w[0], bh, s.x[0], s.text_x(0), rw, rh, rx, ry, menubar::BAR_FONT_NAME, pw, ph,
        leg_box, leg_open, leg_geom, leg_esc, leg_quit,
        if ok { "PASS" } else { "FAIL" }
    );
    rollup("selftest");
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
    // SO2 — the cell now comes from the BAR and the row metrics still come from the crystal
    // dropdown, so the two sources must agree or `ITEM_H`'s clearance assert above is testing one
    // face against another's rows. This is the assert that keeps the re-sourcing honest.
    assert!(CELL_W == crystal::DROP_CELL_W);
    assert!(CELL_H == crystal::DROP_CELL_H);
    // SO3 — the caption is carried in two `u64`s, so the window title must fit in sixteen bytes.
    assert!(wm::MAX_TITLE <= 16);
    // SO3 — the snapshot must hold the app box AND the tenant's full complement of titles.
    assert!(BAR_BOXES_MAX == MENU_TITLES_MAX + 1);
    // SO3 — the default app menu is legal in the registry it is served from.
    assert!(APP_MENU_DEFAULT.len() <= MENU_ITEMS_MAX);
    // SO3 — `Quit` and `About` must be distinguishable, or `app_pick` cannot route.
    assert!(APP_ITEM_QUIT != APP_ITEM_ABOUT);
};
