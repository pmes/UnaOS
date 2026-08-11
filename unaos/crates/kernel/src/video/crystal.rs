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

//! CRYSTAL MENU — the SHARD menu, UnaOS's first live menu, hung off the brand crystal.
//!
//! # What this is, and how it differs from the menu PROTOCOL
//!
//! The design ledger at the foot of [`super::menubar`] specifies a renderer-agnostic menu PROTOCOL:
//! apps publish a menu tree as bandy messages, picks come back principal-stamped, and no renderer is
//! privileged. That is a later, larger arc. **This module is not that.** It is the SYSTEM menu — the
//! one macOS hangs off its apple and UnaOS hangs off its crystal — and it is KERNEL-OWNED end to end:
//! no app publishes it, no principal but the kernel authors it, and the items act on the machine, not
//! on a focused program. Because the publisher and the actor are both the kernel, this menu needs
//! none of the protocol's registry, wire encoding, or cross-principal pick delivery. It follows the
//! protocol's *shape* only where it fits: a fixed TREE of items addressed by identity, a PICK that
//! fires a named action, and a witness that makes every item falsifiable.
//!
//! # The items — Peter's naming, LOCKED (2026-08-11)
//!
//! The machine is a **SHARD**; the bus carries **Shard Messages**. The menu reads:
//!
//! | # | item              | action                                                          |
//! |---|-------------------|-----------------------------------------------------------------|
//! | 0 | About This Shard  | prints the Shard identity + version to the log (REAL)           |
//! | 1 | — (separator)     | —                                                               |
//! | 2 | Sleep             | honest STUB — prints `unimplemented: Sleep` (no ACPI S3 path)    |
//! | 3 | Restart           | honest STUB — prints `unimplemented: Restart` (no reboot path)   |
//! | 4 | Shut Down         | REAL — [`crate::arch::acpi_power::poweroff`] (ACPI S5 soft-off)  |
//!
//! **A stub is not a no-op and not a fake success.** Sleep and Restart RENDER as pickable items — a
//! menu item that could not be shown pickable would be unfalsifiable — and their pick prints one
//! honest line naming the verb and that it is unimplemented. Peter sees on glass/serial that the item
//! is a stub, not that the menu is broken.
//!
//! # Destructive-action discipline
//!
//! Shut Down and Restart end or interrupt the session. Per the safety model a destructive action must
//! be either confirmed or clearly deliberate; **a menu pick IS deliberate** — the operator opened the
//! menu, moved to the item, and pressed it — so no second "really?" affordance is added this arc. What
//! is guaranteed instead is that the destructive path fires on a PICK and on nothing else: opening the
//! menu, hovering (there is no hover this arc), or dismissing never reaches [`fire`]. And the one
//! action that halts the machine — Shut Down — is fired by exactly one caller, the live press in
//! [`press_at`]; the fixture proves Shut Down's ROUTING (that a press on its row resolves to
//! [`Verb::ShutDown`]) through the pure resolver [`item_at`] and NEVER drives a press at its
//! coordinates, so no gate can power the machine off. See [`selftest`].
//!
//! # How it composites — a transient SURFACE, not a strip tenant
//!
//! The dropdown is painted through [`super::strip::paint`] and erased through [`super::strip::erase_rect`]
//! — the same front-buffer row-run discipline every non-compositor painter uses — from [`compose`],
//! which [`super::strip::compose_all`] calls at the composite tail beside the dock and the bar. It is
//! deliberately **not** a `strip::TENANTS` entry: a registered strip consumes an occlusion slot
//! (`wm::OCC_MAX`), and this surface is modal and momentary, not standing furniture. The consequence,
//! disclosed rather than designed away: the dropdown is not in `wm::erase_clip`, so a window moved or
//! closed UNDER an open menu could clip it. In practice a click outside the menu dismisses it before
//! any such gesture can begin, so the overlap does not arise; a future arc that wants menus to survive
//! background repaints would promote this to a clip citizen.
//!
//! # It is only reachable when the bar is enabled
//!
//! The crystal is part of the menu bar, which is DEFAULT OFF ([`super::menubar`]). With the bar off
//! there is no crystal to click and [`press_at`] returns `false` for every point. Disabling the bar
//! while the menu is open tears the menu down — [`super::menubar::set_enabled`] calls [`dismiss`] — so
//! the surface never outlives the mark it hangs from.

use super::{menubar, strip, theme, wm};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

// ---------------------------------------------------------------------------
// The menu model — the SHARD tree
// ---------------------------------------------------------------------------

/// A menu verb: what a pick DOES. Kernel-authored, so this is a closed set, not a wire tag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// About This Shard — prints the Shard identity + version. REAL.
    About,
    /// Sleep — ACPI S3 suspend. STUB: no S3 path exists; the pick prints `unimplemented`.
    Sleep,
    /// Restart — a warm reboot. STUB: no reboot path exists; the pick prints `unimplemented`.
    Restart,
    /// Shut Down — ACPI S5 soft-off. REAL, and the one action that halts the machine.
    ShutDown,
}

impl Verb {
    /// The verb's name for the witness line.
    const fn name(self) -> &'static str {
        match self {
            Verb::About => "About",
            Verb::Sleep => "Sleep",
            Verb::Restart => "Restart",
            Verb::ShutDown => "ShutDown",
        }
    }

    /// `true` when the verb is BACKED by a real action, `false` when it is an honest stub. On the
    /// witness so a capture reads `action=real` or `action=stub` beside the pick.
    const fn real(self) -> bool {
        matches!(self, Verb::About | Verb::ShutDown)
    }

    /// The verb's stable ordinal for the witness (`u8`), independent of its row index.
    const fn ord(self) -> u8 {
        match self {
            Verb::About => 0,
            Verb::Sleep => 1,
            Verb::Restart => 2,
            Verb::ShutDown => 3,
        }
    }
}

/// One row of the menu: an item with a label and verb, or a separator (`verb == None`).
struct Row {
    label: &'static str,
    verb: Option<Verb>,
}

/// **The SHARD tree.** Order is Peter's, LOCKED: About first, a separator, then the power verbs.
const ROWS: [Row; 5] = [
    Row { label: "About This Shard", verb: Some(Verb::About) },
    Row { label: "", verb: None },
    Row { label: "Sleep", verb: Some(Verb::Sleep) },
    Row { label: "Restart", verb: Some(Verb::Restart) },
    Row { label: "Shut Down", verb: Some(Verb::ShutDown) },
];

// ---------------------------------------------------------------------------
// Metrics — all derived, none guessed
// ---------------------------------------------------------------------------

/// The glyph cell the menu draws text in — [`wm::TITLE_CELL`], the same 16 px the bar caption and the
/// dock tiles resolve to, so a kit text-size change moves all of them together.
const CELL: usize = wm::TITLE_CELL;

/// The integer scale the 8-px font is replicated at — [`wm::TITLE_SCALE`].
const SCALE: usize = wm::TITLE_SCALE;

/// An item row's height, px: the glyph cell plus 8 px of clearance split top and bottom, so a 16 px
/// glyph sits with 4 px of air above and below in a 24 px row. Pinned by the const-assert below.
const ITEM_H: usize = CELL + 8;

/// A separator row's height, px — a thin band carrying one keyline, centred.
const SEP_H: usize = 7;

/// The menu's 1 px keyline border, all four sides — the same [`theme::FRAME_LINE`] the window frame
/// and the bar keyline use.
const BORDER: usize = 1;

/// The horizontal inset of item text from the menu's inner edge — one [`strip::PAD`], the kit's gap.
const PADX: usize = strip::PAD;

/// The longest item label in glyphs, walked at compile time so the menu width is a function of the
/// tree rather than a magic number that a relabelled item could silently overflow.
const fn max_label_glyphs() -> usize {
    let mut m = 0;
    let mut i = 0;
    while i < ROWS.len() {
        let l = ROWS[i].label.len();
        if ROWS[i].verb.is_some() && l > m {
            m = l;
        }
        i += 1;
    }
    m
}

/// The menu's total height, walked at compile time from the row heights plus the two borders.
const fn menu_height() -> usize {
    let mut h = 2 * BORDER;
    let mut i = 0;
    while i < ROWS.len() {
        h += if ROWS[i].verb.is_some() { ITEM_H } else { SEP_H };
        i += 1;
    }
    h
}

/// The menu's width, px: both borders, both insets, and the widest label.
const MENU_W: usize = 2 * BORDER + 2 * PADX + max_label_glyphs() * CELL;

/// The menu's height, px.
const MENU_H: usize = menu_height();

/// How many pickable ITEMS the tree carries (separators excluded) — for the witness `items=` term.
const fn item_count() -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < ROWS.len() {
        if ROWS[i].verb.is_some() {
            n += 1;
        }
        i += 1;
    }
    n
}
const ITEM_COUNT: usize = item_count();

const _: () = {
    // The row height must clear the glyph it centres, or the label is cut.
    assert!(ITEM_H >= CELL);
    assert!(ITEM_H == 24);
    // The separator band must hold its keyline with air either side.
    assert!(SEP_H >= 3);
    // A menu with no pickable item would be a surface with nothing to pick.
    assert!(ITEM_COUNT >= 1);
    // The menu must fit the strip painter's scratch, exactly as any strip must.
    assert!(MENU_W <= strip::MAX_STRIP_W);
    // The widest label must be representable — a sanity floor, not a real bound.
    assert!(max_label_glyphs() >= 1);
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Is the menu open? The whole of the modal state. `false` at boot and after every dismissal.
static OPEN: AtomicBool = AtomicBool::new(false);

/// What the dropdown last put on the panel — one signature, one packed rect. The strip primitive's
/// damage slot, so the menu repaints only when it opens, moves, or closes.
static SLOT: strip::Slot = strip::Slot::new();

/// The cost ledger, on the dock's and bar's precedent: NOT `witness`-gated, because the metal image
/// is built without `witness` and a cost claim absent from it is not a claim.
static LEDGER: strip::Ledger = strip::Ledger::new();

/// Falsifiable counters. `opens`/`dismisses` bracket the modal life; `picks` counts fired actions and
/// `last_verb` (a [`Verb::ord`], or `0xFF` for none) names the last one — so a capture reads the flow
/// without a debugger.
static OPENS: AtomicU64 = AtomicU64::new(0);
static DISMISSES: AtomicU64 = AtomicU64::new(0);
static PICKS: AtomicU64 = AtomicU64::new(0);
static LAST_VERB: AtomicU8 = AtomicU8::new(0xFF);

// ---------------------------------------------------------------------------
// Geometry — the dropdown rect, and the row layout inside it
// ---------------------------------------------------------------------------

/// **The dropdown's rect on a `pw` x `ph` panel, or `None`.** Anchored under the crystal: its left
/// edge aligns with the crystal's left (as macOS drops its apple menu from the logo's left), its top
/// is the bar's bottom edge, and it is clamped so it never runs off the right or bottom of the panel.
///
/// `None` when the bar is absent (so there is no crystal to hang from) or the panel is too small to
/// hold the menu below the bar at all — the same decline-rather-than-squeeze rule the strip
/// constructors follow.
fn menu_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    let (cx, _cy, _cw, _ch) = menubar::crystal_box_abs(pw, ph)?;
    let (_bx, by, _bw, bh) = menubar::strip_rect(pw, ph)?;
    let my = by + bh; // flush under the bar
    if MENU_W > pw || my + MENU_H > ph {
        return None; // panel cannot host the menu below the bar
    }
    // Clamp x so the right edge stays on the panel; the left never goes past 0 because the crystal is
    // one PAD in from the left already.
    let mx = if cx + MENU_W > pw { pw - MENU_W } else { cx };
    Some((mx, my, MENU_W, MENU_H))
}

/// MENUFIT — **the dropdown's LIVE extent: its rect while the menu is open, `None` while it is not.**
///
/// The one accessor every other writer asks "where is the SHARD menu". The dropdown is a TRANSIENT
/// surface and deliberately not a [`strip::TENANTS`] member (it takes no occlusion slot), so
/// `strip::rects` does not report it and the desktop layer's whole-panel writes — `console::draw`
/// opens every repaint with a `clear_screen` — flushed the shell's background straight over an open
/// menu. `Screen::present_background` now subtracts this rect alongside the strips, which closes it.
///
/// Published from [`menu_rect`], the same function [`compose`] paints from and [`press_at`] hit-tests
/// against, for the reason the SHELLDESK review gave when it recorded this defect: re-deriving the
/// menu's geometry in `screen.rs` is precisely the drift the registry exists to prevent, so the fix
/// is an accessor here, not a second copy there.
///
/// `None` while closed, so a boot that never opens the menu pays one relaxed load per desktop
/// present and computes no geometry at all. It reports the rect from the instant [`open`] flips
/// `OPEN`, which can be one composite before [`compose`] has PAINTED those pixels — the same bounded
/// residual the strips carry, and bounded the same way: the desktop withholds rows the menu is about
/// to own rather than rows it has stopped owning, and `compose` runs on the next composite.
pub fn open_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    if !OPEN.load(Ordering::Relaxed) {
        return None;
    }
    menu_rect(pw, ph)
}

/// The top of row `idx` as an offset from the menu's top edge, in px — border plus every earlier row.
fn row_top(idx: usize) -> usize {
    let mut y = BORDER;
    let mut i = 0;
    while i < idx {
        y += if ROWS[i].verb.is_some() { ITEM_H } else { SEP_H };
        i += 1;
    }
    y
}

/// Which row, if any, the menu-local vertical offset `ly` falls inside. Borders and out-of-range
/// answer `None`.
fn row_at(ly: usize) -> Option<usize> {
    if ly < BORDER || ly >= MENU_H - BORDER {
        return None;
    }
    for i in 0..ROWS.len() {
        let top = row_top(i);
        let h = if ROWS[i].verb.is_some() { ITEM_H } else { SEP_H };
        if ly >= top && ly < top + h {
            return Some(i);
        }
    }
    None
}

/// **The pure hit resolver: which VERB does panel point `(px, py)` land on, if any.**
///
/// The whole of the routing decision, factored out with NO side effect precisely so the fixture can
/// prove that a press on the Shut Down row resolves to [`Verb::ShutDown`] WITHOUT firing it. A press
/// inside the menu but on a separator, a border, or the inner padding answers `None`; a press outside
/// the menu answers `None`.
fn item_at(r: strip::Rect, px: usize, py: usize) -> Option<Verb> {
    let (mx, my, mw, mh) = r;
    if px < mx || px >= mx + mw || py < my || py >= my + mh {
        return None;
    }
    let row = row_at(py - my)?;
    ROWS[row].verb
}

/// Does panel point `(px, py)` fall anywhere inside the menu rect (border included)?
fn menu_contains(r: strip::Rect, px: usize, py: usize) -> bool {
    let (mx, my, mw, mh) = r;
    px >= mx && px < mx + mw && py >= my && py < my + mh
}

// ---------------------------------------------------------------------------
// Open / dismiss / pick
// ---------------------------------------------------------------------------

/// Open the menu. Idempotent: opening an open menu is a no-op that neither re-counts nor repaints.
fn open(pw: usize, ph: usize) {
    if OPEN.swap(true, Ordering::AcqRel) {
        return;
    }
    OPENS.fetch_add(1, Ordering::Relaxed);
    let (mw, mh, mx, my) = menu_rect(pw, ph).map(|(x, y, w, h)| (w, h, x, y)).unwrap_or((0, 0, 0, 0));
    serial_println!(
        ":: SHARD-MENU: crystal_press=open menu={}x{}+{}+{} items={} ::",
        mw, mh, mx, my, ITEM_COUNT
    );
}

/// Dismiss the menu. Idempotent. The next [`compose`] erases whatever rect the dropdown owned — the
/// strip primitive's vacate rule — so dismissal leaves the panel exactly as it was before the open.
/// `reason` names WHY on the witness (`outside` / `pick` / `escape` / `bar-off`) so a capture can tell
/// a cancel from a selection.
fn dismiss(reason: &str) {
    if !OPEN.swap(false, Ordering::AcqRel) {
        return;
    }
    DISMISSES.fetch_add(1, Ordering::Relaxed);
    serial_println!(":: SHARD-MENU: crystal_press=dismiss reason={} ::", reason);
}

/// **Public dismissal**, for [`super::menubar::set_enabled`]: turning the bar off must tear the menu
/// down, or the dropdown would outlive the crystal it hangs from with nothing left to dismiss it.
pub fn dismiss_for_bar_off() {
    dismiss("bar-off");
}

/// **Fire a verb's action.** THE ONE PLACE an action happens, and the only caller of the halting
/// Shut Down path.
///
/// - `About`    — prints the Shard identity + version (REAL).
/// - `Sleep`    — honest STUB: prints `unimplemented: Sleep`, no side effect.
/// - `Restart`  — honest STUB: prints `unimplemented: Restart`, no side effect.
/// - `ShutDown` — REAL: [`crate::arch::acpi_power::poweroff`], which does not return.
///
/// ⛔ The fixture must NEVER call this with [`Verb::ShutDown`], and does not: it drives picks only for
/// the safe verbs and proves Shut Down's routing through [`item_at`].
fn fire(verb: Verb) {
    match verb {
        Verb::About => {
            // A minimal, honest About: the Shard identity and the kernel crate version, to the log.
            // A crispy About *panel* (a small centred surface with the crystal mark and these facts)
            // is the natural next step and is DESIGNED at the foot of this file; this arc prints it.
            serial_println!(
                ":: SHARD: about name=UnaOS shard=this version={} ::",
                env!("CARGO_PKG_VERSION")
            );
        }
        Verb::Sleep => {
            serial_println!(":: SHARD: unimplemented: Sleep (no ACPI S3 suspend path) ::");
        }
        Verb::Restart => {
            serial_println!(":: SHARD: unimplemented: Restart (no reboot path) ::");
        }
        Verb::ShutDown => {
            // Deliberate and destructive: the operator opened the menu and pressed Shut Down. Say so
            // once, then hand off to the real ACPI S5 soft-off, which either powers the machine off
            // mid-instruction or parks in `hlt` with its own witness. Never returns.
            serial_println!(":: SHARD: shut down — entering ACPI S5 soft-off ::");
            crate::arch::acpi_power::poweroff();
        }
    }
}

// ---------------------------------------------------------------------------
// The click arm — reached from `wc_click_route_at`, ahead of the dock and window arms
// ---------------------------------------------------------------------------

/// **Route a PRESS at panel `(x, y)` to the crystal menu.** Returns `true` iff the press was CONSUMED
/// by the menu (opened it, dismissed it, or picked an item), in which case the caller drops the
/// matching release, exactly as the dock and close arms do.
///
/// The rule:
///  * **menu OPEN, press on an item** — pick it: fire the action and dismiss. Consumed.
///  * **menu OPEN, press inside on a separator/border/padding** — consumed, menu stays open.
///  * **menu OPEN, press outside** — dismiss (a first click outside a menu closes it). Consumed.
///  * **menu CLOSED, press on the crystal box** — open. Consumed.
///  * **menu CLOSED, press elsewhere** — not ours; `false`, so the dock and window arms get their say.
///
/// Judged before the dock and window arms because an open dropdown is a modal surface composited on
/// top of everything; when closed, the only point it claims is the crystal box in the bar, which the
/// bar owns anyway (the bar composites above the windows).
pub fn press_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            return false;
        }
        (fb.width(), fb.height())
    };

    if OPEN.load(Ordering::Acquire) {
        if let Some(r) = menu_rect(pw, ph) {
            if menu_contains(r, px, py) {
                return match item_at(r, px, py) {
                    Some(verb) => {
                        PICKS.fetch_add(1, Ordering::Relaxed);
                        LAST_VERB.store(verb.ord(), Ordering::Relaxed);
                        serial_println!(
                            ":: SHARD-MENU: crystal_pick verb={} action={} ::",
                            verb.name(),
                            if verb.real() { "real" } else { "stub" }
                        );
                        dismiss("pick");
                        fire(verb); // ShutDown does not return; every other verb does
                        true
                    }
                    // Inside the menu but not on an item (separator / border / padding): swallow the
                    // press and keep the menu open, as a real menu does.
                    None => true,
                };
            }
        }
        // A press anywhere outside the open menu dismisses it, and the click is spent doing so.
        dismiss("outside");
        return true;
    }

    // Closed: the only press we own is one on the crystal box itself.
    if let Some((cx, cy, cw, ch)) = menubar::crystal_box_abs(pw, ph) {
        if px >= cx && px < cx + cw && py >= cy && py < cy + ch {
            open(pw, ph);
            return true;
        }
    }
    false
}

/// **The Escape arm**, for `wc_route_event`: a bare `Esc` (0x1b) while the menu is open dismisses it
/// and is consumed. Returns `false` for every other event and for `Esc` when the menu is closed, so
/// Escape reaches an app normally when no menu is up.
///
/// Only the press edge is taken. The matching `KeyUp(0x1b)` — arriving with the menu already closed —
/// flows on untouched: no drain acts on a lone key release, so consuming it would buy nothing and
/// would need a scrap of state to remember the press. The Tab seam swallows its release because it
/// pairs with a focus move; a dismissal has no such pairing.
pub fn key_escape(ev: crate::pal::Event) -> bool {
    const K_ESC: u8 = 0x1b;
    match ev {
        crate::pal::Event::Key(K_ESC) if OPEN.load(Ordering::Acquire) => {
            dismiss("escape");
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The composite seam — a transient surface through the strip painter
// ---------------------------------------------------------------------------

/// **Paint or erase the dropdown.** Called from [`super::strip::compose_all`] at the composite tail.
///
/// Returns `true` iff it painted or erased (the caller then owes the sprite a `Repaint`, as every
/// strip does). Damage-driven: an open menu whose rect is unchanged repaints nothing; a dismissal
/// erases the owned rect exactly once and clears the slot.
///
/// The disabled/closed path is the first line — one relaxed load and one packed check — so a boot
/// that never opens the menu pays two atomics per composite for its existence and touches no lock, no
/// framebuffer, and no allocation.
pub fn compose() -> bool {
    if !OPEN.load(Ordering::Relaxed) {
        // Closed. Owe the pixels of a menu just dismissed, once.
        if SLOT.packed() != 0 {
            let r = SLOT.rect();
            SLOT.clear();
            return strip::erase_rect(r);
        }
        return false;
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
    let rect = menu_rect(pw, ph);
    LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
    LEDGER.tick(
        "crystal",
        format_args!(
            "state=open opens={} dismisses={} picks={}",
            OPENS.load(Ordering::Relaxed),
            DISMISSES.load(Ordering::Relaxed),
            PICKS.load(Ordering::Relaxed)
        ),
    );

    // The menu cannot be placed on this panel (too small) — erase anything we owed and stand down.
    let Some(r) = rect else {
        if SLOT.packed() != 0 {
            let old = SLOT.rect();
            SLOT.clear();
            return strip::erase_rect(old);
        }
        return false;
    };

    let sig = strip::seal(strip::fnv1a_u64(strip::FNV_BASIS, strip::pack_rect(Some(r))));
    if sig == SLOT.sig() && SLOT.packed() == strip::pack_rect(Some(r)) {
        return false;
    }

    // Vacate the old rect first if the menu moved (a panel resize under an open menu), so the two
    // never race to own an overlapping pixel — the strip primitive's rule.
    let old = SLOT.packed();
    let new = strip::pack_rect(Some(r));
    if old != 0 && old != new {
        strip::erase_rect(strip::unpack_rect(old));
    }

    let t1 = crate::arch::now_cycles();
    if !strip::paint("crystal", r, |out, j| compose_row(out, r, j)) {
        return false;
    }
    LEDGER.paint(crate::arch::now_cycles().saturating_sub(t1), (r.2 * r.3) as u64);
    SLOT.store(sig, Some(r));
    true
}

/// Compose panel row `j` (0..MENU_H) of the dropdown into `out[0..MENU_W]` as logical colours.
///
/// A field pass — the menu face, the keyline border, and per-row content — then the item labels
/// overlaid by index. The dock's and bar's shape: an overlay keeps the inner loop a handful of
/// integer compares rather than a scan over every label at every pixel.
fn compose_row(out: &mut [u32], r: strip::Rect, j: usize) {
    let (_mx, _my, w, h) = r;

    // The whole-row base: keyline on the top and bottom border rows, menu face elsewhere.
    let base = if j < BORDER || j + 1 > h - BORDER {
        theme::FRAME_LINE
    } else {
        theme::CHROME_FACE
    };
    for i in 0..w {
        out[i] = base;
    }
    // The two side borders, on every row.
    for i in 0..w {
        if i < BORDER || i + BORDER >= w {
            out[i] = theme::FRAME_LINE;
        }
    }
    if j < BORDER || j + 1 > h - BORDER {
        return; // a pure border row — nothing else on it
    }

    let Some(row) = row_at(j) else {
        return;
    };
    let top = row_top(row);

    match ROWS[row].verb {
        // A separator: one keyline centred in the band, inset from the side borders by PADX.
        None => {
            if j == top + SEP_H / 2 {
                for i in (BORDER + PADX)..(w - BORDER - PADX) {
                    out[i] = theme::FRAME_LINE;
                }
            }
        }
        // An item: its label, vertically centred in the row, at PADX from the inner edge.
        Some(_) => {
            let vpad = (ITEM_H - 8 * SCALE) / 2;
            let gtop = top + vpad;
            if j < gtop || j >= gtop + 8 * SCALE {
                return;
            }
            let sy = (j - gtop) / SCALE;
            if sy >= 8 {
                return;
            }
            let label = ROWS[row].label.as_bytes();
            draw_text(out, w, label, BORDER + PADX, sy, SCALE, theme::TITLE_TEXT_ACTIVE);
        }
    }
}

/// Overlay one scaled row of an ASCII byte string at `x0`, in `ink` — the bar's glyph loop, kept
/// local because it is six lines and the strip primitive has no opinion about text.
fn draw_text(out: &mut [u32], w: usize, s: &[u8], x0: usize, sy: usize, scale: usize, ink: u32) {
    for (c, &b) in s.iter().enumerate() {
        let ch = if (0x20..0x7f).contains(&b) { b } else { b' ' };
        let bits = font8x8::legacy::BASIC_LEGACY[ch as usize][sy];
        if bits == 0 {
            continue;
        }
        let gx = x0 + c * CELL;
        for rx in 0..8usize {
            if bits & (1 << rx) == 0 {
                continue;
            }
            for sx in 0..scale {
                let i = gx + rx * scale + sx;
                if i < w {
                    out[i] = ink;
                }
            }
        }
    }
}

/// The crystal menu's ledger line, on the dock's/bar's terms plus this surface's own tail.
pub fn rollup(scope: &str) {
    LEDGER.rollup(
        "crystal",
        scope,
        format_args!(
            "opens={} dismisses={} picks={} last_verb={}",
            OPENS.load(Ordering::Relaxed),
            DISMISSES.load(Ordering::Relaxed),
            PICKS.load(Ordering::Relaxed),
            LAST_VERB.load(Ordering::Relaxed)
        ),
    );
}

// ---------------------------------------------------------------------------
// Witness
// ---------------------------------------------------------------------------

/// CRYSTAL-MENU fixture — **the SHARD menu opens on the crystal, resolves every item, fires a pick,
/// dismisses three ways, and NEVER powers the machine off.**
///
/// Each leg can FAIL on its own, and the fixture restores the bar and the menu to their prior state
/// before it returns.
///
/// 1. **closed by default** — `OPEN` is observed `false` before the fixture touches it.
/// 2. **a crystal press OPENS** — with the bar enabled, a press at the crystal box centre opens the
///    menu and [`menu_rect`] is placeable.
/// 3. **every item resolves to its verb** — [`item_at`] at each item row's centre answers About,
///    Sleep, Restart, Shut Down in tree order. This is the leg that proves Shut Down is REACHABLE as
///    a pick, and it does so WITHOUT firing it — the whole guard.
/// 4. **a SAFE pick fires and dismisses** — a press on the About row increments `picks`, prints the
///    identity, and leaves the menu closed. Sleep and Restart are driven too, so the wire carries
///    their honest `unimplemented` lines. Shut Down is NEVER driven.
/// 5. **click-outside dismisses** — reopened, a press well outside the menu closes it and is consumed.
/// 6. **Escape dismisses** — reopened, [`key_escape`] on `Esc` closes it and is consumed.
///
/// ⛔ **The Shut Down guard is structural, not a comment.** This function calls [`item_at`] at the
/// Shut Down row (a pure resolver) but never [`press_at`] there, and [`fire`] reaches
/// `acpi_power::poweroff` only from the live press. A gate that ran this fixture and lost the serial
/// tail would be the signature of that guard having failed — so the PASS line printing AFTER all legs
/// is itself the proof the machine stayed up.
#[cfg(feature = "witness")]
pub fn selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        (fb.width(), fb.height())
    };
    let saved_bar = menubar::enabled();

    // Leg 1 — closed by default, observed before any mutation.
    let start_closed = !OPEN.load(Ordering::Acquire);

    // The menu only exists when the bar is present; enable it for the battery.
    menubar::set_enabled(true);
    let cbox = menubar::crystal_box_abs(pw, ph);

    // Leg 2 — a crystal press opens.
    let opened = match cbox {
        Some((cx, cy, cw, ch)) => {
            let hit = press_at((cx + cw / 2) as i32, (cy + ch / 2) as i32);
            hit && OPEN.load(Ordering::Acquire)
        }
        None => false,
    };
    let mr = menu_rect(pw, ph);
    let placed = mr.is_some();

    // Leg 3 — every item resolves to its verb (Shut Down included, via the PURE resolver only).
    let mut resolve_ok = placed;
    if let Some(r) = mr {
        for row in 0..ROWS.len() {
            if let Some(want) = ROWS[row].verb {
                let cy = r.1 + row_top(row) + ITEM_H / 2;
                let cx = r.0 + r.2 / 2;
                if item_at(r, cx, cy) != Some(want) {
                    resolve_ok = false;
                }
            }
        }
    }

    // Leg 4 — a SAFE pick fires and dismisses. About first (real), then Sleep and Restart (stubs) so
    // the wire carries their honest lines. Shut Down is NEVER driven.
    let picks_before = PICKS.load(Ordering::Relaxed);
    let pick_safe = |verb: Verb| -> bool {
        // Reopen and press the row that carries `verb`.
        open(pw, ph);
        let Some(r) = menu_rect(pw, ph) else { return false };
        let mut ok = false;
        for row in 0..ROWS.len() {
            if ROWS[row].verb == Some(verb) {
                let cy = r.1 + row_top(row) + ITEM_H / 2;
                let cx = r.0 + r.2 / 2;
                let consumed = press_at(cx as i32, cy as i32);
                ok = consumed && !OPEN.load(Ordering::Acquire);
            }
        }
        ok
    };
    let about_fired = pick_safe(Verb::About);
    let sleep_fired = pick_safe(Verb::Sleep);
    let restart_fired = pick_safe(Verb::Restart);
    let picks_counted = PICKS.load(Ordering::Relaxed) == picks_before + 3;
    let pick_ok = about_fired && sleep_fired && restart_fired && picks_counted;

    // Leg 5 — click-outside dismisses.
    open(pw, ph);
    // A point guaranteed outside the menu: the panel's far bottom-right corner.
    let outside_consumed = press_at((pw - 1) as i32, (ph - 1) as i32);
    let outside_ok = outside_consumed && !OPEN.load(Ordering::Acquire);

    // Leg 6 — Escape dismisses.
    open(pw, ph);
    let esc_consumed = key_escape(crate::pal::Event::Key(0x1b));
    let esc_ok = esc_consumed && !OPEN.load(Ordering::Acquire);

    // Restore: ensure closed, and put the bar back the way we found it.
    dismiss("selftest");
    menubar::set_enabled(saved_bar);

    let (rw, rh, rx, ry) = mr.map(|(x, y, w, h)| (w, h, x, y)).unwrap_or((0, 0, 0, 0));
    let ok = start_closed
        && opened
        && placed
        && resolve_ok
        && pick_ok
        && outside_ok
        && esc_ok;
    serial_println!(
        ":: CRYSTAL-MENU: menu={}x{}+{}+{} panel={}x{} items={} start_closed={} opened={} \
         placed={} resolve={} pick={} outside={} escape={} :: {} ::",
        rw, rh, rx, ry, pw, ph, ITEM_COUNT,
        start_closed, opened, placed, resolve_ok, pick_ok, outside_ok, esc_ok,
        if ok { "PASS" } else { "FAIL" }
    );
    rollup("selftest");
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// DESIGNED, NOT BUILT — the rest of the SHARD menu, recorded beside the first cut.
//
// ── A HOVER HIGHLIGHT ────────────────────────────────────────────────────────────────────────────
// This arc's menu fires on the PRESS edge and draws no hovered row: reaching a hover highlight means
// tracking pointer MOTION over the open menu, which is the drag-motion path (`wc_route_tail` ->
// `wc_drag_motion`) — a lane this arc does not touch. The next arc adds a `HOVER: AtomicU8` (the
// highlighted row index, or NONE), folds it into the damage signature so only the two changed rows
// repaint, and paints the hovered ITEM row in `theme::ACCENT` with `theme::BEVEL_LIGHT` ink. The
// press then fires the hovered row rather than the row under the cursor, matching press-drag-release
// menu behaviour.
//
// ── THE ABOUT PANEL ──────────────────────────────────────────────────────────────────────────────
// About prints the identity this arc. The crispy About PANEL is a second transient surface, minted the
// same way this dropdown is (a `strip::paint` rect, its own `Slot`, its own dismiss-on-click), centred
// on the panel: the brand crystal drawn large (reuse `menubar`'s `crystal_facet`), the name "UnaOS",
// the Shard version, and the build. It is a SECOND modal, so it wants the same press-outside/Escape
// dismissal this menu has — which is the argument for factoring open/dismiss/compose into a tiny modal
// surface primitive that both the menu and the About panel are tenants of, rather than copying them.
//
// ── SLEEP AND RESTART, MADE REAL ─────────────────────────────────────────────────────────────────
// Both are honest stubs today because neither op exists. RESTART is the nearer one: a warm reboot via
// the 8042 pulse (`0xFE` -> port `0x64`) or the PCI reset control register (`0xCF9`), each a real,
// small, discoverable action in the `acpi_power` idiom — refuse rather than guess, one witness line,
// fall back to a triple-fault only as a last resort. SLEEP is ACPI S3 (suspend-to-RAM), which needs
// the DSDT `\_S3_` package read the exact way `scan_s5` reads `\_S5_`, plus a resume vector and a
// register save/restore the kernel has no machinery for yet — a genuine subsystem, correctly a stub
// until that machinery exists. When each lands, its `Verb::_::real()` flips to `true` and its `fire`
// arm calls the op; the menu, the routing, and this fixture are unchanged.
// ═════════════════════════════════════════════════════════════════════════════════════════════════
