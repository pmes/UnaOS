//! AETHERA11Y (SR119): the keyboard focus ring, painted over the page from the accessibility theme.
//!
//! `:focus-visible` (Selectors 4 §9.4) is what a keyboard user must see. The engine paints it as Chromium
//! paints `outline: auto` (`-webkit-focus-ring-color`): a [`FocusRingTheme::width`] band of
//! [`FocusRingTheme::color`] just outside the border box, with a 1px [`FocusRingTheme::halo`] outside that
//! so the ring reads on dark and light backgrounds alike. The theme is set by `a11y::prefs` (a wider ring
//! under `prefers-contrast: more` / `forced-colors: active`).
//!
//! What it rings: the element keyboard navigation focused (`a11y::focus::focus` arms [`set_target`]),
//! while it is still the script's `document.activeElement` and the last focus move was not a pointer's
//! (`css::pointer_focus`). A focused text field already carries its own 2px ring (render/mod.rs) and is
//! not ringed twice.

use std::cell::RefCell;

use crate::dom::NodeRef;
use crate::layout::LayoutTree;

/// How the ring looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusRingTheme {
    pub color: (u8, u8, u8),
    /// The 1px contrast band outside the ring (None: no halo).
    pub halo: Option<(u8, u8, u8)>,
    /// Ring thickness in CSS px.
    pub width: f32,
    /// Gap between the border box and the ring.
    pub offset: f32,
}

impl Default for FocusRingTheme {
    /// Chromium's focus ring: 2px #101010 with a 1px white halo, flush with the border box.
    fn default() -> Self {
        FocusRingTheme { color: (16, 16, 16), halo: Some((255, 255, 255)), width: 2.0, offset: 0.0 }
    }
}

thread_local! {
    static THEME: RefCell<FocusRingTheme> = RefCell::new(FocusRingTheme::default());
    static TARGET: RefCell<Option<NodeRef>> = const { RefCell::new(None) };
}

/// Sets the ring's look.
pub fn set_theme(t: FocusRingTheme) {
    THEME.with(|c| *c.borrow_mut() = t);
}

pub fn theme() -> FocusRingTheme {
    THEME.with(|c| *c.borrow())
}

/// Arms (or clears) the element the ring goes around.
pub fn set_target(n: Option<NodeRef>) {
    TARGET.with(|c| *c.borrow_mut() = n);
}

/// The element the next frame rings, when the ring shows: the armed target, still the page's focused
/// element, focus last moved by the keyboard.
pub fn target() -> Option<NodeRef> {
    let n = TARGET.with(|c| c.borrow().clone())?;
    if crate::css::pointer_focus() {
        return None;
    }
    let still = crate::js::PAGE.with(|p| {
        let p = p.try_borrow().ok()?;
        let doc = p.doc.as_ref()?;
        Some(n.same_document(doc) && p.focused == Some(n.id().0))
    });
    // A page with no script state at all (no document registered) still shows the ring.
    if still == Some(false) {
        return None;
    }
    Some(n)
}

/// `n`'s border box in document coordinates.
fn box_of(layout: &LayoutTree, n: &NodeRef) -> Option<(f32, f32, f32, f32)> {
    fn walk(id: taffy::prelude::NodeId, cx: f32, cy: f32, layout: &LayoutTree, want: &NodeRef) -> Option<(f32, f32, f32, f32)> {
        let l = layout.taffy.layout(id).ok()?;
        let (x, y) = (cx + l.location.x, cy + l.location.y);
        if layout.node_map.get(&id) == Some(want) {
            return Some((x, y, l.size.width, l.size.height));
        }
        for c in layout.taffy.children(id).ok()? {
            if let Some(r) = walk(c, x, y, layout, want) {
                return Some(r);
            }
        }
        None
    }
    walk(layout.root_node, 0.0, 0.0, layout, n)
}

/// Paints the ring (the last thing a frame paints), inside the damage.
pub(super) fn paint(layout: &LayoutTree, surface: &mut [u8], width: u32, height: u32, sx: i32, sy: i32, damage_rects: &[(u32, u32, u32, u32)]) {
    let Some(n) = target() else { return };
    if super::focus_of(&n).is_some() {
        return;
    }
    let Some((x, y, w, h)) = box_of(layout, &n) else { return };
    let t = theme();
    let (x0, y0) = (x - sx as f32, y - sy as f32);
    let (x1, y1) = (x0 + w, y0 + h);
    let mut band = |inset: f32, thick: f32, color: (u8, u8, u8)| {
        // the frame between the rect grown by `inset` and grown by `inset + thick`
        let (ax0, ay0, ax1, ay1) = (x0 - inset - thick, y0 - inset - thick, x1 + inset + thick, y1 + inset + thick);
        let (bx0, by0, bx1, by1) = (x0 - inset, y0 - inset, x1 + inset, y1 + inset);
        let (py0, py1) = (ay0.round().max(0.0) as u32, (ay1.round().max(0.0) as u32).min(height));
        let (px0, px1) = (ax0.round().max(0.0) as u32, (ax1.round().max(0.0) as u32).min(width));
        for py in py0..py1 {
            for px in px0..px1 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let inner = fx >= bx0 && fx < bx1 && fy >= by0 && fy < by1;
                if !inner && super::in_damage(px, py, damage_rects) {
                    super::put_px(surface, width, px, py, color);
                }
            }
        }
    };
    if let Some(halo) = t.halo {
        band(t.offset + t.width, 1.0, halo);
    }
    band(t.offset, t.width, t.color);
}
