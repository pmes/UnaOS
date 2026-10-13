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
//!
//! A11YFACE (SR124): the author's outlines (css-ui-4 §4, `layout_core::paint::outline_of`) paint here too,
//! before the ring, as the final step of the frame (CSS 2.2 Appendix E step 10 allows every outline to be
//! drawn last). The ring follows the focused element's own outline:
//!
//! | focused element's `outline-style`        | ring                                                        |
//! |------------------------------------------|-------------------------------------------------------------|
//! | unspecified, or `auto`                   | painted (the UA's `:focus-visible { outline: auto }`)        |
//! | a line style with a width (`2px solid`)  | not painted: the author's outline is the indicator           |
//! | `none` (or a zero width)                 | not painted only when the author styles `:focus-visible` (a rule naming it matches the element, [`author_styles_focus_visible`]); a bare `:focus { outline: none }` keeps the ring — the WCAG 2.4.7 UA convention |

use std::cell::{Cell, RefCell};

use crate::dom::NodeRef;
use crate::layout::LayoutTree;
use layout_core::paint::{self as lpaint, OUTLINE_AUTO, OUTLINE_DASHED, OUTLINE_DOTTED, OUTLINE_DOUBLE, OUTLINE_NONE};

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
    /// A11YFACE (SR124): the armed target matches an author `:focus-visible` rule.
    static AUTHOR_FV: Cell<bool> = const { Cell::new(false) };
    /// The `:focus-visible` style rules of the page's sheets, keyed by a fingerprint of the sheet text.
    static FV_RULES: RefCell<(u64, Vec<css_core::selectors::SelectorList>)> = const { RefCell::new((0, Vec::new())) };
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

/// A11YFACE (SR124): records whether the armed target is styled by an author `:focus-visible` rule (decided by
/// `a11y::focus::focus` when it arms the ring, from [`author_styles_focus_visible`]).
pub fn set_author_focus_visible(styled: bool) {
    AUTHOR_FV.with(|c| c.set(styled));
}

pub fn author_focus_visible() -> bool {
    AUTHOR_FV.with(|c| c.get())
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

/// FNV-1a over the sheets (and their boundaries): the [`FV_RULES`] cache key.
fn fingerprint(sheets: &[String]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for s in sheets {
        for b in s.bytes().chain(std::iter::once(0xff)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h ^ sheets.len() as u64
}

/// The style rules (with declarations) whose selectors name `:focus-visible`, nested and conditional rules
/// included (a rule under a `@media` that does not match is counted too: the author styled focus).
fn focus_visible_rules(rules: &[css_core::stylesheet::CssRule], out: &mut Vec<css_core::selectors::SelectorList>) {
    use css_core::selectors::ElementState;
    use css_core::stylesheet::CssRule;
    for r in rules {
        match r {
            CssRule::Style(s) => {
                if !s.declarations.is_empty() && s.selectors.mentions_state(ElementState::FocusVisible) {
                    out.push(s.selectors.clone());
                }
                focus_visible_rules(&s.children, out);
            }
            CssRule::Media(_, rs) | CssRule::Supports(_, rs) | CssRule::LayerBlock(_, rs) | CssRule::Container(_, rs) => {
                focus_visible_rules(rs, out)
            }
            _ => {}
        }
    }
}

/// A11YFACE (SR124): whether the author styles `:focus-visible` for `n` — a style rule of `sheets` (the page's,
/// `AetherEngine::stylesheets`) whose selector names `:focus-visible` matches `n` now. Read when the element
/// holds keyboard focus, so `:focus-visible` itself matches and `:not(:focus-visible)` does not.
pub fn author_styles_focus_visible(sheets: &[String], n: &NodeRef) -> bool {
    let key = fingerprint(sheets);
    FV_RULES.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != key {
            let mut out = Vec::new();
            for s in sheets {
                focus_visible_rules(&css_core::stylesheet::parse_stylesheet(s).rules, &mut out);
            }
            *c = (key, out);
        }
        c.1.iter().any(|l| crate::dom::Selectors(l.clone()).matches_node(n))
    })
}

/// The taffy box of `n`.
fn id_of(layout: &LayoutTree, n: &NodeRef) -> Option<taffy::prelude::NodeId> {
    layout.node_map.iter().find(|(_, m)| *m == n).map(|(id, _)| *id)
}

/// A11YFACE (SR124): whether `n`'s own outline replaces the ring (the module doc's table).
pub fn ring_suppressed(layout: &LayoutTree, n: &NodeRef) -> bool {
    let Some(p) = id_of(layout, n).and_then(|id| layout.paint_map.get(&id)) else { return false };
    match p.outline_style {
        None | Some(OUTLINE_AUTO) => false,
        Some(OUTLINE_NONE) => author_focus_visible(),
        Some(_) => match lpaint::outline_of(p) {
            Some(_) => true,
            None => author_focus_visible(),
        },
    }
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

/// Fills the frame between `inner` and `outer` (viewport rects `(x, y, w, h)`), inside the damage. `keep` picks
/// the pixels of a patterned band by their position along the edge (dashes, dots); `alpha` blends.
#[allow(clippy::too_many_arguments)]
fn band(
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    inner: (f32, f32, f32, f32),
    outer: (f32, f32, f32, f32),
    color: (u8, u8, u8),
    alpha: u8,
    keep: &dyn Fn(u32, u32) -> bool,
) {
    let (ax0, ay0, ax1, ay1) = (outer.0, outer.1, outer.0 + outer.2, outer.1 + outer.3);
    let (bx0, by0, bx1, by1) = (inner.0, inner.1, inner.0 + inner.2, inner.1 + inner.3);
    let (py0, py1) = (ay0.round().max(0.0) as u32, (ay1.round().max(0.0) as u32).min(height));
    let (px0, px1) = (ax0.round().max(0.0) as u32, (ax1.round().max(0.0) as u32).min(width));
    for py in py0..py1 {
        for px in px0..px1 {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let in_inner = fx >= bx0 && fx < bx1 && fy >= by0 && fy < by1;
            if in_inner || !super::in_damage(px, py, damage_rects) || !keep(px, py) {
                continue;
            }
            if alpha == 255 {
                super::put_px(surface, width, px, py, color);
            } else {
                super::blend_px(surface, width, px, py, color, alpha);
            }
        }
    }
}

/// The nearest specified `color` on `id` or an ancestor box (an outline's `currentColor`), else black.
fn current_color(layout: &LayoutTree, id: taffy::prelude::NodeId) -> (u8, u8, u8) {
    let mut cur = Some(id);
    while let Some(c) = cur {
        if let Some(col) = layout.paint_map.get(&c).and_then(|p| p.color) {
            return col;
        }
        cur = layout.taffy.parent(c);
    }
    (0, 0, 0)
}

/// Not painted: inside a `display: none` box, or `visibility: hidden` by the nearest box that says.
fn unpainted(layout: &LayoutTree, id: taffy::prelude::NodeId) -> bool {
    let mut cur = Some(id);
    let mut hidden: Option<bool> = None;
    while let Some(c) = cur {
        if layout.taffy.style(c).is_ok_and(|s| s.display == taffy::style::Display::None) {
            return true;
        }
        if hidden.is_none() {
            hidden = layout.paint_map.get(&c).and_then(|p| p.hidden);
        }
        cur = layout.taffy.parent(c);
    }
    hidden == Some(true)
}

/// A11YFACE (SR124): every author outline (`outline-style` other than none and auto, with a width) around its
/// border box grown by `outline-offset`. A fully transparent outline paints nothing, except under forced colors
/// (`forced-colors: active`), where it takes the system's text colour as UAs do. Transforms and multicol
/// fragments are not followed (the box's untransformed border box is outlined).
fn paint_outlines(layout: &LayoutTree, surface: &mut [u8], width: u32, height: u32, sx: i32, sy: i32, damage_rects: &[(u32, u32, u32, u32)]) {
    let forced = css_core::media::user_preferences().forced_colors;
    let mut list: Vec<(taffy::prelude::NodeId, lpaint::Outline)> = layout
        .paint_map
        .iter()
        .filter_map(|(id, p)| lpaint::outline_of(p).filter(|o| o.style != OUTLINE_AUTO).map(|o| (*id, o)))
        .collect();
    if list.is_empty() {
        return;
    }
    // Document order of boxes is not the map's: paint in a stable order (taffy ids grow in build order).
    list.sort_by_key(|(id, _)| u64::from(*id));
    for (id, o) in list {
        if unpainted(layout, id) {
            continue;
        }
        let Some((x, y)) = layout_core::scroll::box_origin(layout, id, (sx as f32, sy as f32)) else { continue };
        let Ok(l) = layout.taffy.layout(id) else { continue };
        let bb = (x - sx as f32, y - sy as f32, l.size.width, l.size.height);
        let (rgb, mut alpha) = match o.color {
            Some((r, g, b, a)) => ((r, g, b), a),
            None => (current_color(layout, id), 255),
        };
        let rgb = if forced && alpha == 0 { (0, 0, 0) } else { rgb };
        if forced {
            alpha = 255;
        }
        if alpha == 0 {
            continue;
        }
        let (inner, outer) = lpaint::outline_band(bb, o.width, o.offset);
        let w = o.width.max(1.0);
        match o.style {
            OUTLINE_DOUBLE if o.width >= 3.0 => {
                // two lines of a third each, a third apart (css-backgrounds-3 `double`)
                let t = o.width / 3.0;
                let mid_in = (inner.0 - 2.0 * t, inner.1 - 2.0 * t, inner.2 + 4.0 * t, inner.3 + 4.0 * t);
                let mid_out = (inner.0 - t, inner.1 - t, inner.2 + 2.0 * t, inner.3 + 2.0 * t);
                band(surface, width, height, damage_rects, inner, mid_out, rgb, alpha, &|_, _| true);
                band(surface, width, height, damage_rects, mid_in, outer, rgb, alpha, &|_, _| true);
            }
            OUTLINE_DASHED | OUTLINE_DOTTED => {
                // dashes 3 widths long with 3-width gaps (`dotted`: one width on, one off), counted along each edge
                let (on, period) = if o.style == OUTLINE_DOTTED { (w, 2.0 * w) } else { (3.0 * w, 6.0 * w) };
                let (ox, oy) = (outer.0, outer.1);
                let keep = move |px: u32, py: u32| {
                    let (fx, fy) = (px as f32 + 0.5 - ox, py as f32 + 0.5 - oy);
                    let along = if fy < w || fy >= outer.3 - w { fx } else { fy };
                    along.rem_euclid(period) < on
                };
                band(surface, width, height, damage_rects, inner, outer, rgb, alpha, &keep);
            }
            _ => band(surface, width, height, damage_rects, inner, outer, rgb, alpha, &|_, _| true),
        }
    }
}

/// Paints the author outlines, then the ring (the last things a frame paints), inside the damage.
pub(super) fn paint(layout: &LayoutTree, surface: &mut [u8], width: u32, height: u32, sx: i32, sy: i32, damage_rects: &[(u32, u32, u32, u32)]) {
    paint_outlines(layout, surface, width, height, sx, sy, damage_rects);
    let Some(n) = target() else { return };
    if super::focus_of(&n).is_some() || ring_suppressed(layout, &n) {
        return;
    }
    let Some((x, y, w, h)) = box_of(layout, &n) else { return };
    let t = theme();
    let bb = (x - sx as f32, y - sy as f32, w, h);
    let ring = |inset: f32, thick: f32| {
        // the frame between the rect grown by `inset` and grown by `inset + thick`
        let inner = (bb.0 - inset, bb.1 - inset, bb.2 + 2.0 * inset, bb.3 + 2.0 * inset);
        let outer = (inner.0 - thick, inner.1 - thick, inner.2 + 2.0 * thick, inner.3 + 2.0 * thick);
        (inner, outer)
    };
    if let Some(halo) = t.halo {
        let (inner, outer) = ring(t.offset + t.width, 1.0);
        band(surface, width, height, damage_rects, inner, outer, halo, 255, &|_, _| true);
    }
    let (inner, outer) = ring(t.offset, t.width);
    band(surface, width, height, damage_rects, inner, outer, t.color, 255, &|_, _| true);
}
