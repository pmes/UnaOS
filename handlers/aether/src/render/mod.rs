use crate::fonts::lines::Advancer;
use crate::fonts::{Face as Font, FontSel};
use crate::layout::LayoutTree;
use taffy::prelude::*;

/// Inherited paint state carried down the box tree.
#[derive(Clone, Copy)]
struct Inherited {
    color: (u8, u8, u8),
    font_size: f32,
    /// Computed font-weight.
    weight: u16,
    italic: bool,
    /// font-stretch, percent.
    stretch: u16,
    word_spacing: f32,
    /// direction: rtl
    rtl: bool,
    /// The box whose `text-shadow` is in effect (inherited; `PaintStyle::text_shadows` holds the layers).
    text_shadow: Option<NodeId>,
    line_height: f32, // multiplier of font size
    underline: bool,
    nowrap: bool,
    white_space: u8,
    word_break: u8,
    overflow_wrap: u8,
    letter_spacing: f32,
    line_through: bool,
    /// vertical-align super/sub: paint-time baseline shift in px.
    shift_y: f32,
    /// list-style-type in effect (layout::PaintStyle::list_style codes).
    list_style: u8,
    family: u16, // fonts::family_list id
    /// The image-replacement idiom: this subtree's text is off-box, but
    /// its boxes and backgrounds still paint.
    text_hidden: bool,
    text_transform: u8, // 0 = none, 1 = uppercase, 2 = lowercase, 3 = capitalize
    /// Content box (screen space: x, y, w, h) of an enclosing button-like
    /// control. A `<button>`'s label is an ordinary text child, so the box it
    /// must be centred in is only known from the ancestor.
    center_box: Option<(f32, f32, f32, f32)>,
}

use crate::layout::default_font_size;

impl Inherited {
    fn sel(&self) -> FontSel {
        FontSel { family: self.family, weight: self.weight, italic: self.italic, stretch: self.stretch }
    }
    /// The measurer/shaper of this text: face stack, size, spacing, direction.
    fn advancer(&self) -> Advancer {
        Advancer::new(self.sel(), self.font_size, self.letter_spacing).with_word_spacing(self.word_spacing).with_dir(self.rtl)
    }
    fn shadows<'a>(&self, layout: &'a LayoutTree) -> &'a [effects::Shadow] {
        self.text_shadow
            .and_then(|n| layout.paint_map.get(&n))
            .and_then(|p| p.text_shadows.as_deref())
            .unwrap_or(&[])
    }
}

/// The sans-serif selection UI chrome text (media controls) is drawn in.
fn ui_sel() -> FontSel {
    FontSel::new(crate::fonts::SANS, 400, false)
}

/// Screen-space clip rect (x0, y0, x1, y1), already scroll-adjusted.
type Clip = (f32, f32, f32, f32);

/// One text fragment the last frame painted (AETHERSELECT, SR69): the paint-order record a pointer is
/// hit-tested against. Geometry is in viewport px at the frame's scroll offset (`scroll`).
#[derive(Clone, Debug)]
pub struct TextBox {
    /// The inline formatting context root and the fragment's index in its `frags`.
    pub ifc: NodeId,
    pub frag: usize,
    /// The text node's box.
    pub node: NodeId,
    /// Pen origin x and advance width of the fragment.
    pub x: f32,
    pub w: f32,
    /// The line box the fragment sits on.
    pub top: f32,
    pub bottom: f32,
    pub adv: Advancer,
    pub scroll: (i32, i32),
}

thread_local! {
    static TEXT_BOXES: std::cell::RefCell<Vec<TextBox>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The text fragments the last full frame painted, in paint order (topmost last).
pub fn text_boxes() -> Vec<TextBox> {
    TEXT_BOXES.with(|t| t.borrow().clone())
}

// ---- AETHERPAINTPERF (SR130): the paint index ----------------------------------------------------------
//
// A frame used to walk and paint every box of the tree, clipping each pixel to the damage rects: a page of
// 2,000 rows shaped and clipped 2,000 rows for a one-box damage. Now a frame walks twice. The INDEX walk runs the
// paint's own control flow with no pixels — every box at the place the paint puts it (placed_origin, the
// multicol shift, scroll offsets, atomic inlines, stacking-context layers) — and records each `draw_node`
// call's SUBTREE INK: the screen rect every pixel its paint (its own and its nested calls') can touch, or
// unbounded when the shape is not bounded here (transforms, filters, clip-paths, backdrop filters, box shadows,
// text shadows, vertical writing, column fragments, multicol rules, scrollbars, form controls and media,
// author ::marker text, text outside an inline formatting context). It records the text boxes too. The PAINT
// walk then skips a `draw_node` call whose subtree ink meets no damage rect: such a box writes no pixel of the
// damage, every pixel it would write is outside it, and every painter clips to the damage — the skip moves no
// pixel. The ink is a flat map of (box -> rect) in paint order of recording, rebuilt every frame, so it is never
// stale; `subtree_ink` hands the last frame's to the engine (the damage of an animation frame).

/// Screen-space rect (x0, y0, x1, y1) a paint can touch.
pub(crate) type Ink = (f32, f32, f32, f32);
const INK_NONE: Ink = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
const INK_ALL: Ink = (f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY);

fn ink_union(a: Ink, b: Ink) -> Ink {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// Whether `r` meets any damage rect.
fn ink_meets(r: Ink, damage_rects: &[(u32, u32, u32, u32)]) -> bool {
    damage_rects.iter().any(|&(dx, dy, dw, dh)| {
        r.2 > dx as f32 && r.0 < (dx + dw) as f32 && r.3 > dy as f32 && r.1 < (dy + dh) as f32
    })
}

#[derive(Default)]
struct PaintIndex {
    /// The walk in progress: 0 the classic walk (paints every box it reaches and records the text boxes), 1 the
    /// index walk (no pixels: subtree ink and text boxes), 2 the indexed paint walk.
    mode: u8,
    /// The ink of the `draw_node` calls open on the index walk (innermost last).
    stack: Vec<Ink>,
    /// Each box's subtree ink, from the last index walk.
    ink: std::collections::HashMap<NodeId, Ink>,
    /// The scroll offset that walk painted at.
    scroll: (i32, i32),
    painted: u64,
    skipped: u64,
    /// AETHERPAINTPERF2 (SR132): the `draw_node` calls the index walk has opened (preorder count), the calls open now
    /// (innermost last) with whether each one's OWN ink is unbounded, and the on-screen boxes it placed.
    seq: u32,
    open: Vec<(NodeId, bool)>,
    snap: FrameSnap,
}

/// AETHERPAINTPERF2 (SR132): one box of an indexed frame whose subtree ink meets the viewport, as the walk placed it.
#[derive(Clone, Copy, Debug)]
struct BoxSnap {
    /// Its `draw_node` call's place in the walk (preorder), and the call that encloses it.
    seq: u32,
    enclosing: Option<NodeId>,
    /// Its subtree ink, and whether its own ink is unbounded (a transform, filter or clip-path group, a shadow, a
    /// control, media, scrollbars, …).
    ink: Ink,
    own_unbounded: bool,
    /// The origin the walk handed its call, and its layout box.
    abs: (f32, f32),
    layout: Option<taffy::Layout>,
}

/// AETHERPAINTPERF2 (SR132): what an indexed frame placed on screen — the engine keeps the last one, so the frame
/// after a relayout can damage only the boxes that moved or changed ([`relayout_damage`]).
#[derive(Default)]
pub(crate) struct FrameSnap {
    boxes: std::collections::HashMap<NodeId, BoxSnap>,
    scroll: (i32, i32),
    size: (u32, u32),
    /// AETHERTOP (SR146): the top-layer generation (js/toplayer.rs) the frame painted — a changed one paints whole.
    top_gen: u64,
}

thread_local! {
    /// AETHERTOP (SR146): the boxes of the top layer's elements this frame (the document's own walk skips them;
    /// [`paint_top_layer`] paints each after its `::backdrop`), and the one the top-layer pass is painting now.
    static TOP_IDS: std::cell::RefCell<Vec<NodeId>> = const { std::cell::RefCell::new(Vec::new()) };
    static TOP_PASS: std::cell::Cell<Option<NodeId>> = const { std::cell::Cell::new(None) };
}

/// AETHERTOP (SR146) / AETHERFIT (SR189): the box a top-layer element (DOM node `dom`) is painted — and hit-tested —
/// as: its first box.
pub(crate) fn top_layer_box(layout: &LayoutTree, dom: html_core::NodeId) -> Option<NodeId> {
    let mut ids: Vec<NodeId> = layout.node_map.iter().filter(|(_, n)| n.is_element() && n.id() == dom).map(|(t, _)| *t).collect();
    ids.sort_by_key(|t| u64::from(*t));
    ids.first().copied()
}

/// Is `id` a top-layer box the walk must leave to the top-layer pass (HTML "top layer": an element in it is
/// painted there, not where its parent would paint it)?
fn hoisted_to_top_layer(id: NodeId) -> bool {
    TOP_PASS.with(|t| t.get()) != Some(id) && TOP_IDS.with(|t| t.borrow().contains(&id))
}

/// AETHERPAINTPERF2 (SR132): the last frame's on-screen boxes with the paint inputs of each, read off the tree it
/// painted before a relayout replaces it ([`before_relayout`]).
pub(crate) struct Before {
    snap: FrameSnap,
    /// Each box's paint inputs as text (None: never taken as unchanged — images, canvases, media, frames, controls).
    inputs: std::collections::HashMap<NodeId, Option<String>>,
    canvas: Option<(u8, u8, u8)>,
    outlines: bool,
}

thread_local! {
    static INDEX: std::cell::RefCell<PaintIndex> = std::cell::RefCell::new(PaintIndex::default());
}

fn walk_mode() -> u8 {
    INDEX.with(|x| x.borrow().mode)
}

/// Adds `r` to the ink of the innermost open `draw_node` call (index walk only).
fn ink_add(r: Ink) {
    INDEX.with(|x| {
        let mut x = x.borrow_mut();
        if let Some(top) = x.stack.last_mut() {
            *top = ink_union(*top, r);
        }
        // AETHERPAINTPERF2 (SR132): the innermost open call's own ink is unbounded.
        if !ink_bounded(r) {
            if let Some(o) = x.open.last_mut() {
                o.1 = true;
            }
        }
    });
}

/// Whether `r` is a finite rect (an unbounded ink is infinite on a side).
fn ink_bounded(r: Ink) -> bool {
    r.0 != f32::NEG_INFINITY && r.1 != f32::NEG_INFINITY && r.2 != f32::INFINITY && r.3 != f32::INFINITY
}

/// AETHERPAINTPERF2 (SR132): a box's paint inputs besides its place and layout box, as text: its DOM node (and a
/// text node's data), its paint style (inherited values follow the ancestors', which are compared too), its line
/// boxes, its scroll offset and multicol shift. None for what is never taken as unchanged: replaced content whose
/// pixels arrive later (images, background images, canvases, SVG, frames, media) and form controls.
fn box_inputs(layout: &LayoutTree, id: NodeId) -> Option<String> {
    let n = layout.node_map.get(&id);
    let tag = n.and_then(|n| n.as_element().map(|e| e.name.local.as_ref().to_string()));
    if tag.as_deref().is_some_and(|t| {
        matches!(t, "img" | "picture" | "canvas" | "svg" | "iframe" | "object" | "embed" | "video" | "audio" | "input" | "textarea" | "select" | "button")
    }) {
        return None;
    }
    let p = layout.paint_map.get(&id);
    if p.is_some_and(|p| p.bg_image.is_some()) {
        return None;
    }
    let text = n.filter(|n| layout_core::DomNode::is_text(*n)).map(layout_core::DomNode::text);
    Some(format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        n.map(|n| n.id()),
        text,
        p,
        layout.inline.get(&id),
        layout.scroll.get(&id),
        layout.col_shift.get(&id)
    ))
}

/// Whether any box has an author outline (painted after the walk, in box-key order).
fn any_outline(layout: &LayoutTree) -> bool {
    layout.paint_map.values().any(|p| layout_core::paint::outline_of(p).is_some())
}

/// AETHERPAINTPERF2 (SR132): the engine took the last indexed frame's boxes (`take_snapshot`).
pub(crate) fn take_snapshot() -> FrameSnap {
    INDEX.with(|x| std::mem::take(&mut x.borrow_mut().snap))
}

/// AETHERPAINTPERF2 (SR132): reads the paint inputs of the boxes `snap` (the last frame of `layout`) placed on
/// screen, before a relayout replaces or rewrites `layout`.
pub(crate) fn before_relayout(layout: &LayoutTree, snap: FrameSnap) -> Before {
    let inputs = snap.boxes.keys().map(|id| (*id, box_inputs(layout, *id))).collect();
    Before { snap, inputs, canvas: canvas_background(layout), outlines: any_outline(layout) }
}

/// AETHERPAINTPERF2 (SR132): the damage of the first frame after a relayout — the union of the old and new subtree
/// ink of every box whose place in the walk, origin, layout box or paint inputs differ between the frame `before`
/// read and an index walk of `layout` now (a box that left the screen: its old ink; one that came onto it: its new
/// ink). Box keys are stable across a kept relayout (`layout_core::retarget`), so an unchanged box compares equal.
/// A box that changed paints only inside its subtree ink, and every painter clips to the damage, so the frame is the
/// frame a viewport damage paints. None (the viewport) when the viewport, the scroll offset or the canvas background
/// moved, an author outline exists, or a changed box sits in a call whose own ink is unbounded (a transform or filter
/// group draws it elsewhere). The index walk leaves this layout's index and text boxes behind.
pub(crate) fn relayout_damage(
    layout: &LayoutTree,
    surface: &mut [u8],
    width: u32,
    height: u32,
    scroll_x: f64,
    scroll_y: f64,
    before: &Before,
) -> Option<Vec<(u32, u32, u32, u32)>> {
    if before.snap.size != (width, height) || before.snap.scroll != (scroll_x as i32, scroll_y as i32) || before.snap.boxes.is_empty() {
        return None;
    }
    if before.outlines || any_outline(layout) || canvas_background(layout) != before.canvas {
        return None;
    }
    // AETHERTOP (SR146): a top-layer change (a modal dialog's backdrop covers the viewport) paints the frame whole.
    if before.snap.top_gen != crate::js::toplayer::generation() {
        return None;
    }
    frame_walk(layout, surface, width, height, scroll_x, scroll_y, &[(0, 0, width, height)], true);
    INDEX.with(|x| {
        let x = x.borrow();
        let now = &x.snap;
        if now.size != (width, height) {
            return None;
        }
        // A call whose own ink is unbounded, or that sits in one: its pixels are not where its ink says.
        let grouped = |snap: &FrameSnap, id: NodeId| -> bool {
            let mut cur = Some(id);
            let mut hops = 0;
            while let Some(c) = cur {
                let Some(b) = snap.boxes.get(&c) else { return false };
                if b.own_unbounded {
                    return true;
                }
                cur = b.enclosing;
                hops += 1;
                if hops > 4096 {
                    return true;
                }
            }
            false
        };
        let mut rects: Vec<Ink> = Vec::new();
        for (id, old) in &before.snap.boxes {
            match now.boxes.get(id) {
                None => {
                    if grouped(&before.snap, *id) {
                        return None;
                    }
                    rects.push(old.ink);
                }
                Some(new) => {
                    let placed = old.seq == new.seq && old.enclosing == new.enclosing && old.abs == new.abs && old.layout == new.layout;
                    let same = placed && before.inputs.get(id).and_then(|t| t.as_ref()).is_some_and(|t| box_inputs(layout, *id).as_ref() == Some(t));
                    if !same {
                        if grouped(&before.snap, *id) || grouped(now, *id) {
                            return None;
                        }
                        rects.push(old.ink);
                        rects.push(new.ink);
                    }
                }
            }
        }
        for (id, new) in &now.boxes {
            if !before.snap.boxes.contains_key(id) {
                if grouped(now, *id) {
                    return None;
                }
                rects.push(new.ink);
            }
        }
        // Screen pixels, clamped to the viewport; many rects become their union.
        let (w, h) = (width as f32, height as f32);
        let mut out: Vec<(u32, u32, u32, u32)> = Vec::new();
        let mut all = INK_NONE;
        for r in rects {
            let (x0, y0, x1, y1) = (r.0.max(0.0).floor(), r.1.max(0.0).floor(), r.2.min(w).ceil(), r.3.min(h).ceil());
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            all = ink_union(all, (x0, y0, x1, y1));
            out.push((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32));
        }
        if out.len() > 64 {
            out = vec![(all.0 as u32, all.1 as u32, (all.2 - all.0) as u32, (all.3 - all.1) as u32)];
        }
        Some(out)
    })
}

/// The union of the last frame's subtree ink of `nodes` (boxes it did not paint add nothing), and the scroll
/// offset that frame painted at; the rect is None when one of them is unbounded.
pub(crate) fn subtree_ink(nodes: &[NodeId]) -> (Option<Ink>, (i32, i32)) {
    INDEX.with(|x| {
        let x = x.borrow();
        let mut acc = INK_NONE;
        for n in nodes {
            if let Some(r) = x.ink.get(n) {
                if r.0 == f32::NEG_INFINITY || r.2 == f32::INFINITY || r.1 == f32::NEG_INFINITY || r.3 == f32::INFINITY {
                    return (None, x.scroll);
                }
                acc = ink_union(acc, *r);
            }
        }
        (Some(acc), x.scroll)
    })
}

/// An element box's own ink (index walk): its border box, an outside list marker's reach left of it, or
/// unbounded for the painters whose extent is not bounded here.
#[allow(clippy::too_many_arguments)]
fn ink_element(
    layout: &LayoutTree, node_id: NodeId, tag: &str, spec: &crate::layout::PaintStyle, lb: &taffy::Layout, box_sx: f32, box_sy: f32,
    inherited: &Inherited,
) {
    let bars = layout_core::scroll::scrollbars(layout, node_id);
    let unbounded = layout.col_frags.get(&node_id).is_some_and(|f| !f.is_empty())
        || spec.shadows.as_ref().is_some_and(|s| !s.is_empty())
        || layout.multicol.contains_key(&node_id)
        || bars.v.is_some()
        || bars.h.is_some()
        || matches!(tag, "input" | "textarea" | "select" | "button" | "video" | "audio")
        || spec.marker.as_ref().is_some_and(|m| m.text.is_some());
    if unbounded {
        ink_add(INK_ALL);
        return;
    }
    let (bw, bh) = (lb.size.width.max(0.0), lb.size.height.max(0.0));
    let mut r = (box_sx - 2.0, box_sy - 2.0, box_sx + bw + 2.0, box_sy + bh + 2.0);
    // An outside marker ends at the content edge on the first line: a bullet one em before it, a counter
    // its text's width before it (40 em covers any ordinal the counter styles print).
    if spec.list_item.unwrap_or(tag == "li") && !inherited.text_hidden {
        let fs = spec.marker.as_ref().map_or(inherited.font_size, |m| m.size(inherited.font_size).max(inherited.font_size));
        r.0 -= 40.0 * fs + 4.0;
        r.1 -= 4.0 * fs; // the marker's glyphs about the first baseline (an empty item's sits below its top)
        r.3 += 4.0 * fs;
    }
    ink_add(r);
}

mod boxpaint;
// AETHERPAINTPERF (SR130): rasterized glyph runs kept across frames.
mod glyphcache;
// AETHERA11Y (SR119): the keyboard focus ring, painted last from the accessibility theme.
pub mod focusring;
pub(crate) mod effects;
// AETHERMEDIA2 (SR134): the caption box and the blocked-autoplay play affordance over a <video>.
mod media_overlay;

fn in_clip(x: u32, y: u32, clip: Clip) -> bool {
    let (x0, y0, x1, y1) = clip;
    (x as f32) >= x0 && (x as f32) < x1 && (y as f32) >= y0 && (y as f32) < y1
}

fn in_damage(x: u32, y: u32, damage_rects: &[(u32, u32, u32, u32)]) -> bool {
    damage_rects
        .iter()
        .any(|&(dx, dy, dw, dh)| x >= dx && x < dx + dw && y >= dy && y < dy + dh)
}

fn put_px(surface: &mut [u8], width: u32, x: u32, y: u32, (r, g, b): (u8, u8, u8)) {
    let idx = ((y * width + x) * 4) as usize;
    if idx + 3 < surface.len() {
        surface[idx] = b;
        surface[idx + 1] = g;
        surface[idx + 2] = r;
        surface[idx + 3] = 255;
    }
}

fn blend_px(surface: &mut [u8], width: u32, x: u32, y: u32, (r, g, b): (u8, u8, u8), alpha: u8) {
    let idx = ((y * width + x) * 4) as usize;
    if idx + 3 < surface.len() {
        let a = alpha as u32;
        let inv = 255 - a;
        surface[idx] = ((b as u32 * a + surface[idx] as u32 * inv) / 255) as u8;
        surface[idx + 1] = ((g as u32 * a + surface[idx + 1] as u32 * inv) / 255) as u8;
        surface[idx + 2] = ((r as u32 * a + surface[idx + 2] as u32 * inv) / 255) as u8;
        surface[idx + 3] = 255;
    }
}

/// The inherited paint state at the root of the box tree (the UA's initial values).
fn root_inherited() -> Inherited {
    Inherited {
        color: (0, 0, 0),
        font_size: 16.0,
        weight: 400,
        italic: false,
        stretch: 100,
        word_spacing: 0.0,
        rtl: false,
        text_shadow: None,
        line_height: 0.0, // natural
        underline: false,
        nowrap: false,
        white_space: 0,
        word_break: 0,
        overflow_wrap: 0,
        letter_spacing: 0.0,
        line_through: false,
        shift_y: 0.0,
        list_style: 1,
        family: crate::fonts::STANDARD, // the default standard font (Times New Roman)
        text_hidden: false,
        text_transform: 0,
        center_box: None,
    }
}

/// The Advancer a form control's value is painted with: the inherited font state along the box
/// tree's path from the root to `id` (the engine hit-tests a click into a field's text on it).
pub fn control_advancer(layout: &LayoutTree, id: NodeId) -> Option<Advancer> {
    let mut path = vec![id];
    let mut n = layout.taffy.parent(id);
    while let Some(p) = n {
        path.push(p);
        n = layout.taffy.parent(p);
    }
    let mut inh = root_inherited();
    for p in path.into_iter().rev() {
        if let Some(dn) = layout.node_map.get(&p) {
            if let Some(el) = dn.as_element() {
                let spec = layout.paint_map.get(&p).cloned().unwrap_or_default();
                inherit_element(&mut inh, el.name.local.as_ref(), &spec, dn, p);
            }
        }
    }
    Some(inh.advancer())
}

/// The focused text control's editing state, as the engine hands it to paint (AETHERINPUT, SR68): the
/// control, its caret and selection anchor (byte offsets into its value, on grapheme boundaries), and
/// whether the blinking caret is in its visible phase.
#[derive(Clone, Debug)]
pub struct FocusPaint {
    pub node: crate::dom::NodeRef,
    pub caret: usize,
    pub anchor: usize,
    pub caret_visible: bool,
}

/// A focused field's selection highlight: Chromium's default active selection colour, translucent so the
/// glyphs over it keep their own colour.
const SELECTION_BG: (u8, u8, u8) = (0x33, 0x90, 0xFF);
const SELECTION_ALPHA: u8 = 102;

thread_local! {
    static FOCUS: std::cell::RefCell<Option<FocusPaint>> = const { std::cell::RefCell::new(None) };
}

/// Sets (or clears) the focused text control the next frames paint a caret and selection in.
pub fn set_focus(f: Option<FocusPaint>) {
    FOCUS.with(|c| *c.borrow_mut() = f);
}

fn focus_of(node: &crate::dom::NodeRef) -> Option<FocusPaint> {
    FOCUS.with(|c| c.borrow().as_ref().filter(|f| &f.node == node).cloned())
}

/// Fills the screen rect [x0, x1) x [y0, y1) with `color` at `alpha`, inside the damage and the clip.
#[allow(clippy::too_many_arguments)]
fn fill_rect(
    x0: f32, y0: f32, x1: f32, y1: f32, color: (u8, u8, u8), alpha: u8,
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    let (xa, ya) = (x0.max(0.0) as u32, y0.max(0.0) as u32);
    let (xb, yb) = ((x1.max(0.0) as u32).min(width), (y1.max(0.0) as u32).min(height));
    for y in ya..yb {
        for x in xa..xb {
            if in_damage(x, y, damage_rects) && in_clip(x, y, clip) {
                if alpha == 255 {
                    put_px(surface, width, x, y, color);
                } else {
                    blend_px(surface, width, x, y, color, alpha);
                }
            }
        }
    }
}

/// Where the caret before byte `idx` of a textarea's value sits: (line index, x from the content box's
/// left), on the same pre-wrap line breaking the value is painted with.
fn textarea_caret(text: &str, idx: usize, adv: &Advancer, max_w: f32, mode: &crate::fonts::lines::TextMode) -> (usize, f32) {
    let prefix = &text[..idx.min(text.len())];
    let breaks = |s: &str| if s.is_empty() { Vec::new() } else { crate::fonts::lines::break_lines(adv, s, mode, max_w) };
    let mut segs: Vec<&str> = prefix.split('\n').collect();
    let cur = segs.pop().unwrap_or("");
    let above: usize = segs.iter().map(|l| breaks(l).len().max(1)).sum();
    let ls = breaks(cur);
    match ls.last() {
        Some(last) => (above + ls.len() - 1, adv.str(&last.text)),
        None => (above, 0.0),
    }
}

/// Draws `text` shaped (font_core: bidi, fallback, kerning, ligatures) with its pen at
/// (`origin_x`, `baseline_y`), glyphs at quarter-pixel x phases through Skia's A8 pre-blend for `color`
/// (fonts::raster). Returns the advance drawn.
#[allow(clippy::too_many_arguments)]
fn draw_shaped(
    text: &str, adv: &Advancer, origin_x: f32, baseline_y: f32, color: (u8, u8, u8),
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) -> f32 {
    // AETHERPAINTPERF (SR130): the same run at the same pen (exact bits), size, spacing, direction, colour and font
    // set draws the same coverage: replay the pixels the rasterizer emitted, through the same clip, in order.
    let key = crate::perf::scoped().then(|| glyphcache::key(text, adv, origin_x, baseline_y, color));
    if let Some(run) = key.as_ref().and_then(glyphcache::get) {
        crate::perf::add_glyph(true);
        for &(px, py, a) in &run.px {
            plot_glyph_px(surface, width, height, damage_rects, clip, color, px, py, a);
        }
        return run.advance;
    }
    let record = key.is_some();
    let mut emitted: Vec<(i32, i32, u8)> = Vec::new();
    let lut = crate::fonts::raster::preblend(color);
    let placed = adv.place(text);
    let mut end = 0.0f32;
    for (g, x, dy) in &placed {
        end = end.max(*x - g.dx * adv.size + g.adv * adv.size);
        crate::fonts::raster::draw_glyph(g.face, g.gid, adv.size, origin_x + x, baseline_y + dy, &lut, &mut |px, py, a| {
            if record {
                emitted.push((px, py, a));
            }
            plot_glyph_px(surface, width, height, damage_rects, clip, color, px, py, a);
        });
    }
    let advance = adv.str(text).max(end);
    if let Some(k) = key {
        crate::perf::add_glyph(false);
        glyphcache::put(k, glyphcache::Run { px: emitted, advance });
    }
    advance
}

/// One rasterized glyph pixel of coverage `a` (already through the ink's pre-blend), clipped to the surface,
/// the damage and the clip.
#[allow(clippy::too_many_arguments)]
fn plot_glyph_px(
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip, color: (u8, u8, u8),
    px: i32, py: i32, a: u8,
) {
    if px < 0 || py < 0 {
        return;
    }
    let (px, py) = (px as u32, py as u32);
    if px < width && py < height && in_damage(px, py, damage_rects) && in_clip(px, py, clip) {
        blend_px(surface, width, px, py, color, a);
    }
}

/// css-text-decor-3 §4: each shadow layer is the text's glyph coverage offset by (dx, dy), blurred by a
/// Gaussian of σ = blur/2 (Blink's `BlurRadiusToStdDev`), painted in the shadow colour beneath the text,
/// the first layer topmost.
#[allow(clippy::too_many_arguments)]
fn draw_text_shadows(
    text: &str, adv: &Advancer, origin_x: f32, baseline_y: f32, shadows: &[effects::Shadow],
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    if shadows.is_empty() {
        return;
    }
    let placed = adv.place(text);
    for sh in shadows.iter().rev() {
        let sigma = (sh.blur / 2.0).max(0.0);
        let pad = (sigma * 3.0).ceil() as i32 + 1;
        // the coverage mask of the run, in a box around its ink
        let lw = adv.str(text);
        let x0 = (origin_x + sh.dx).floor() as i32 - pad - 2;
        let y0 = (baseline_y + sh.dy - adv.size * 1.5).floor() as i32 - pad;
        let mw = (lw + adv.size).ceil() as i32 + 2 * pad + 4;
        let mh = (adv.size * 2.5).ceil() as i32 + 2 * pad;
        if mw <= 0 || mh <= 0 || mw as i64 * mh as i64 > 1 << 24 {
            continue;
        }
        let mut mask = vec![0f32; (mw * mh) as usize];
        let ident: [u8; 256] = std::array::from_fn(|i| i as u8);
        for (g, x, dy) in &placed {
            crate::fonts::raster::draw_glyph(g.face, g.gid, adv.size, origin_x + sh.dx + x, baseline_y + sh.dy + dy, &ident, &mut |px, py, a| {
                let (mx, my) = (px - x0, py - y0);
                if mx >= 0 && my >= 0 && mx < mw && my < mh {
                    let m = &mut mask[(my * mw + mx) as usize];
                    *m = (*m + a as f32 / 255.0).min(1.0);
                }
            });
        }
        if sigma > 0.0 {
            effects::gaussian_blur(&mut mask, mw as usize, mh as usize, sigma);
        }
        let c = sh.color.0;
        for my in 0..mh {
            for mx in 0..mw {
                let a = mask[(my * mw + mx) as usize] * sh.color.1;
                if a <= 0.0 {
                    continue;
                }
                let (px, py) = (x0 + mx, y0 + my);
                if px < 0 || py < 0 {
                    continue;
                }
                let (px, py) = (px as u32, py as u32);
                if px < width && py < height && in_damage(px, py, damage_rects) && in_clip(px, py, clip) {
                    blend_px(surface, width, px, py, c, (a * 255.0).round().min(255.0) as u8);
                }
            }
        }
    }
}

/// Text decoration lines of one painted span from `x0` to `x1` at `baseline` (css-text-decor-3 §2, the
/// face's `post` metrics as Blink uses them: fonts::decoration_metrics).
#[allow(clippy::too_many_arguments)]
fn draw_decorations(
    face: &Font, size: f32, deco: Deco, x0: f32, x1: f32, baseline: f32, color: (u8, u8, u8),
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    if !(deco.underline || deco.line_through) || x1 <= x0 {
        return;
    }
    let m = crate::fonts::decoration_metrics(face, size);
    let mut hline = |top: f32, thick: f32| {
        let yi = top.round() as i32;
        for dy in 0..thick.round().max(1.0) as i32 {
            let yy = yi + dy;
            if yy < 0 || yy as u32 >= height {
                continue;
            }
            let xa = x0.max(0.0).round() as u32;
            let xb = (x1.max(0.0).round() as u32).min(width);
            for x in xa..xb {
                if in_damage(x, yy as u32, damage_rects) && in_clip(x, yy as u32, clip) {
                    put_px(surface, width, x, yy as u32, color);
                }
            }
        }
    };
    if deco.underline {
        hline(baseline + m.underline_offset, m.thickness);
    }
    if deco.line_through {
        hline(baseline - m.line_through_offset, m.thickness);
    }
}

/// How one background (or mask) layer maps onto a box: the painted image
/// rectangle in box-local coordinates plus the repeat mode.
#[derive(Clone, Copy)]
struct BgGeometry {
    off_x: f32,
    off_y: f32,
    w: f32,
    h: f32,
    repeat: u8, // 0 repeat, 1 no-repeat, 2 repeat-x, 3 repeat-y
}

impl BgGeometry {
    /// Maps a box-local pixel to image-space UV, honouring the repeat mode.
    /// None = this pixel is outside every tile of the layer.
    fn sample(&self, lx: f32, ly: f32, iw: u32, ih: u32) -> Option<(u32, u32)> {
        let (rx, ry) = (matches!(self.repeat, 0 | 2), matches!(self.repeat, 0 | 3));
        let mut u = lx - self.off_x;
        let mut v = ly - self.off_y;
        if rx {
            u = u.rem_euclid(self.w);
        } else if u < 0.0 || u >= self.w {
            return None;
        }
        if ry {
            v = v.rem_euclid(self.h);
        } else if v < 0.0 || v >= self.h {
            return None;
        }
        Some((
            ((u / self.w * iw as f32) as u32).min(iw.saturating_sub(1)),
            ((v / self.h * ih as f32) as u32).min(ih.saturating_sub(1)),
        ))
    }
}

/// Resolves one length/percentage component against a reference length.
/// Returns None for `auto` and unparsed values.
fn bg_length(token: &str, reference: f32) -> Option<f32> {
    let t = token.trim();
    // calc()/min()/max()/clamp() — component CSS states icon metrics this way.
    if t.contains('(') {
        return crate::css::eval_length(t, Some(reference));
    }
    if let Some(p) = t.strip_suffix('%') {
        return p.parse::<f32>().ok().map(|p| p / 100.0 * reference);
    }
    for unit in ["px", "pt", "rem", "em"] {
        if let Some(n) = t.strip_suffix(unit) {
            let n: f32 = n.trim().parse().ok()?;
            return Some(match unit {
                "px" => n,
                "pt" => n * 4.0 / 3.0,
                _ => n * default_font_size("", 16.0),
            });
        }
    }
    t.parse::<f32>().ok().filter(|n| *n == 0.0)
}

/// Splits a background-/mask- component list on whitespace that sits outside
/// any parentheses, so `max(calc(1rem + 4px), 10px)` stays one token.
fn split_components(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => {
                if i > start {
                    out.push(&value[start..i]);
                }
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    if start < value.len() {
        out.push(&value[start..]);
    }
    out
}

/// Resolves background-size / background-position / background-repeat into
/// the painted rectangle. `size`/`position` are the declared strings (None
/// = CSS initial: `auto` and `0% 0%`).
fn resolve_bg_geometry(
    size: Option<&str>,
    position: Option<&str>,
    repeat: Option<u8>,
    box_w: f32,
    box_h: f32,
    img_w: f32,
    img_h: f32,
) -> BgGeometry {
    // --- background-size ---
    let (mut w, mut h) = (img_w, img_h);
    match size.map(str::trim) {
        Some("cover") | Some("contain") => {
            let s = if size == Some("cover") {
                (box_w / img_w).max(box_h / img_h)
            } else {
                (box_w / img_w).min(box_h / img_h)
            };
            w = img_w * s;
            h = img_h * s;
        }
        Some(v) if !v.is_empty() && v != "auto" => {
            let mut it = split_components(v).into_iter();
            let a = it.next().unwrap_or("auto");
            let b = it.next();
            let rw = bg_length(a, box_w);
            let rh = b.and_then(|b| bg_length(b, box_h));
            match (rw, rh) {
                // One value (or an explicit `auto` partner): the other axis
                // keeps the intrinsic aspect ratio.
                (Some(rw), None) => {
                    w = rw;
                    h = img_h * (rw / img_w);
                }
                (None, Some(rh)) => {
                    h = rh;
                    w = img_w * (rh / img_h);
                }
                (Some(rw), Some(rh)) => {
                    w = rw;
                    h = rh;
                }
                (None, None) => {}
            }
        }
        _ => {}
    }
    let (w, h) = (w.max(0.01), h.max(0.01));

    // --- background-position ---
    let mut off_x = 0.0;
    let mut off_y = 0.0;
    if let Some(pos) = position {
        let tokens: Vec<&str> = split_components(pos);
        // Keyword tokens name their own axis; anything else fills
        // horizontal-then-vertical in source order.
        let mut horiz: Option<&str> = None;
        let mut vert: Option<&str> = None;
        for t in &tokens {
            match *t {
                "left" | "right" => horiz = Some(t),
                "top" | "bottom" => vert = Some(t),
                "center" => {
                    if horiz.is_none() && vert.is_some() {
                        horiz = Some("center");
                    } else if horiz.is_none() {
                        horiz = Some("center");
                    } else if vert.is_none() {
                        vert = Some("center");
                    }
                }
                other => {
                    if horiz.is_none() {
                        horiz = Some(other);
                    } else if vert.is_none() {
                        vert = Some(other);
                    }
                }
            }
        }
        // A lone horizontal keyword/value centres nothing vertically: the
        // vertical component defaults to `center` per the CSS grammar only
        // when a keyword was used; a bare length defaults to 0 for the
        // second component in the one-value form -> `center` per spec.
        let resolve = |tok: Option<&str>, free: f32, span: f32| -> f32 {
            match tok {
                None | Some("left") | Some("top") => 0.0,
                Some("right") | Some("bottom") => free,
                Some("center") => free / 2.0,
                Some(t) => {
                    if t.ends_with('%') {
                        bg_length(t, free).unwrap_or(0.0)
                    } else {
                        bg_length(t, span).unwrap_or(0.0)
                    }
                }
            }
        };
        off_x = resolve(horiz, box_w - w, box_w);
        off_y = resolve(
            if tokens.len() == 1 && vert.is_none() { Some("center") } else { vert },
            box_h - h,
            box_h,
        );
    }

    BgGeometry { off_x, off_y, w, h, repeat: repeat.unwrap_or(0) }
}

/// Test hook: the resolved background/mask rectangle as
/// `(off_x, off_y, w, h)`. Keeps `BgGeometry` private to the renderer.
#[cfg(test)]
pub(crate) fn test_bg_geometry(
    size: Option<&str>,
    position: Option<&str>,
    repeat: Option<u8>,
    box_w: f32,
    box_h: f32,
    img_w: f32,
    img_h: f32,
) -> (f32, f32, f32, f32) {
    let g = resolve_bg_geometry(size, position, repeat, box_w, box_h, img_w, img_h);
    (g.off_x, g.off_y, g.w, g.h)
}

/// Transforms text based on text-transform property: 0 = none, 1 = uppercase, 2 = lowercase, 3 = capitalize.
pub(crate) fn transform_text(text: &str, transform: u8) -> String {
    match transform {
        1 => text.to_uppercase(),
        2 => text.to_lowercase(),
        3 => {
            // capitalize: uppercase first letter of each word
            text.split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => text.to_string(),
    }
}

/// Draws `text` starting at (origin_x, origin_y), wrapping at `max_width`
/// (white-space: normal, no decoration beyond `underline`).
#[allow(clippy::too_many_arguments)]
fn draw_text(
    text: &str,
    origin_x: f32,
    origin_y: f32,
    max_width: f32,
    adv: &Advancer,
    line_height_mult: f32,
    color: (u8, u8, u8),
    underline: bool,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let deco = Deco { underline, line_through: false };
    draw_lines(
        text, origin_x, origin_y, max_width, adv, line_height_mult, color,
        &crate::fonts::lines::TextMode::default(), deco, &[], surface, width, height, damage_rects, clip,
    );
}

/// The inherited paint state of an element's subtree: font, colour,
/// decorations, white-space, list style, super/sub shift (CSS 2.2 §6.2,
/// with the html.css UA defaults by tag).
fn inherit_element(inherited: &mut Inherited, tag: &str, spec: &crate::layout::PaintStyle, dom_node: &crate::dom::NodeRef, node: NodeId) {
    if spec.text_shadows.is_some() {
        inherited.text_shadow = Some(node);
    }
    let parent_font_size = inherited.font_size;
    let own_family = spec
        .family
        .unwrap_or_else(|| crate::layout::default_family(tag, inherited.family));
    inherited.font_size = spec.used_font_size.or(spec.font_size).unwrap_or_else(|| {
        crate::layout::ua_font_size(tag, inherited.font_size, inherited.family, own_family)
    });
    inherited.weight = spec
        .weight
        .map(|w| w.resolve(inherited.weight))
        .unwrap_or_else(|| crate::layout::default_weight(tag, inherited.weight));
    inherited.italic =
        spec.italic.unwrap_or_else(|| crate::layout::default_italic(tag, inherited.italic));
    if let Some(st) = spec.stretch {
        inherited.stretch = st;
    }
    if let Some(ws) = spec.word_spacing {
        inherited.word_spacing = ws;
    }
    inherited.rtl = spec.rtl.unwrap_or_else(|| crate::layout::default_rtl(dom_node, inherited.rtl));
    inherited.family = spec
        .family
        .unwrap_or_else(|| crate::layout::default_family(tag, inherited.family));
    if let Some(lh) = spec.line_height {
        inherited.line_height = lh;
    }
    if let Some(tt) = spec.text_transform {
        inherited.text_transform = tt;
    }

    if tag == "a" || tag == "u" {
        inherited.underline = true;
    }
    if let Some(u) = spec.underline {
        inherited.underline = u;
    }
    if let Some(nw) = spec.nowrap {
        inherited.nowrap = nw;
    }
    inherited.white_space = spec
        .white_space
        .unwrap_or_else(|| crate::layout::default_white_space(tag, inherited.white_space));
    if let Some(v) = spec.word_break { inherited.word_break = v; }
    if let Some(v) = spec.overflow_wrap { inherited.overflow_wrap = v; }
    if let Some(v) = spec.letter_spacing { inherited.letter_spacing = v; }
    inherited.line_through = spec
        .line_through
        .unwrap_or_else(|| crate::layout::default_line_through(tag, inherited.line_through));
    // html.css list styles: ul disc (circle one level down,
    // square deeper), ol decimal; inherited into the items.
    match tag {
        "ul" | "menu" | "dir" => {
            let depth = dom_node
                .ancestors()
                .filter(|a| a.as_element().is_some_and(|e| matches!(e.name.local.as_ref(), "ul" | "ol" | "menu" | "dir")))
                .count();
            inherited.list_style = [1, 2, 3][depth.min(2)];
        }
        "ol" => inherited.list_style = 4,
        _ => {}
    }
    if let Some(ls) = spec.list_style {
        inherited.list_style = ls;
    }
    // Blink: super raises by parent-size/3 + 1, sub lowers by
    // parent-size/5 + 1.
    match tag {
        "sup" => inherited.shift_y -= parent_font_size / 3.0 + 1.0,
        "sub" => inherited.shift_y += parent_font_size / 5.0 + 1.0,
        _ => {}
    }
    if spec.text_hidden == Some(true) {
        inherited.text_hidden = true;
    }
    inherited.color = spec.color.unwrap_or(if tag == "a" {
        (0, 0, 238) // UA default link blue
    } else {
        inherited.color
    });
}

/// Draws one line's worth of text with its baseline at `baseline_y` — an
/// inline formatting context's text fragment (layout::inline): shadows,
/// then the shaped glyphs, then decorations spanning the fragment.
#[allow(clippy::too_many_arguments)]
fn draw_glyph_run(
    text: &str,
    origin_x: f32,
    baseline_y: f32,
    adv: &Advancer,
    color: (u8, u8, u8),
    deco: Deco,
    shadows: &[effects::Shadow],
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    draw_text_shadows(text, adv, origin_x, baseline_y, shadows, surface, width, height, damage_rects, clip);
    let w = draw_shaped(text, adv, origin_x, baseline_y, color, surface, width, height, damage_rects, clip);
    if let Some(face) = crate::fonts::face(&adv.sel) {
        // The fragment's spaces are inside its inline box (the line's hanging
        // spaces were already removed by layout), so the decoration spans them.
        draw_decorations(face, adv.size, deco, origin_x, origin_x + w, baseline_y, color, surface, width, height, damage_rects, clip);
    }
}

/// Positioned (relative/absolute/fixed/sticky) with an integer z-index,
/// fixed/sticky, a flex item with an integer z-index, or opacity < 1:
/// the box forms a stacking context (CSS 2.2 §9.9.1, css-position-3 §8,
/// css-flexbox-1 §4.3, css-color-4 §15).
pub(crate) fn is_stacking_context(layout: &LayoutTree, id: NodeId) -> bool {
    let Some(p) = layout.paint_map.get(&id) else { return false };
    let kind = p.position_kind.unwrap_or(0);
    let z = p.z_index.flatten();
    if matches!(kind, 3 | 4) || (kind != 0 && z.is_some()) {
        return true;
    }
    if p.opacity.is_some_and(|o| o < 1.0) {
        return true;
    }
    // css-transforms-1 §3 / css-masking-1 §5: a transform or a clip-path other than none.
    if p.transform.as_ref().is_some_and(|t| !t.is_empty())
        || p.tf_translate.is_some()
        || p.tf_rotate.is_some()
        || p.tf_scale.is_some()
        || p.clip_path.as_ref().is_some_and(|c| c.is_some())
        // Filter Effects 1 §5 / 2 §3: a filter or backdrop-filter other than none (AETHERSTYLE3 SR103).
        || p.filter.as_ref().is_some_and(|f| f.is_some())
        || p.backdrop_filter.as_ref().is_some_and(|f| f.is_some())
    {
        return true;
    }
    z.is_some()
        && layout
            .taffy
            .parent(id)
            .and_then(|par| layout.paint_map.get(&par))
            .and_then(|pp| pp.flex_container)
            == Some(true)
}

/// Painted as a layer of its stacking context rather than in normal flow.
fn is_layered(layout: &LayoutTree, id: NodeId) -> bool {
    layout.paint_map.get(&id).and_then(|p| p.position_kind).is_some_and(|k| k != 0)
        || is_stacking_context(layout, id)
}

/// The layer's z: its integer z-index, else 0 (auto paints at 0).
fn z_of(layout: &LayoutTree, id: NodeId) -> i32 {
    layout.paint_map.get(&id).and_then(|p| p.z_index).flatten().unwrap_or(0)
}

/// The vertical offset of a table cell's content (CSS 2.2 §17.5.4): middle
/// (the UA default) centres the line boxes in the content box, bottom puts
/// them at its end, top/baseline leave them at the top.
fn cell_valign_offset(layout: &LayoutTree, id: NodeId, content_h: f32) -> f32 {
    let is_cell = layout
        .node_map
        .get(&id)
        .and_then(|n| n.as_element().map(|e| matches!(e.name.local.as_ref(), "td" | "th")))
        .unwrap_or(false);
    if !is_cell {
        return 0.0;
    }
    let Ok(l) = layout.taffy.layout(id) else { return 0.0 };
    let inner = l.size.height - l.padding.top - l.padding.bottom - l.border.top - l.border.bottom;
    let free = (inner - content_h).max(0.0);
    match layout.paint_map.get(&id).and_then(|p| p.vertical_align).map(|v| v.0) {
        Some(0 | 6) => 0.0,
        Some(7) => free,
        _ => (free / 2.0).floor(),
    }
}

/// Text decoration lines of one run.
#[derive(Clone, Copy, Default)]
struct Deco {
    underline: bool,
    line_through: bool,
}

/// Draws a run on the lines fonts::lines breaks it into — the same breaker
/// the measurer used, so the run fills exactly its measured box.
#[allow(clippy::too_many_arguments)]
fn draw_lines(
    text: &str,
    origin_x: f32,
    origin_y: f32,
    max_width: f32,
    adv: &Advancer,
    line_height_mult: f32,
    color: (u8, u8, u8),
    mode: &crate::fonts::lines::TextMode,
    deco: Deco,
    shadows: &[effects::Shadow],
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let Some(font) = crate::fonts::face(&adv.sel) else { return };
    let size = adv.size;
    let ascent = crate::fonts::baseline_offset(font, size, line_height_mult);
    let line_height = crate::fonts::line_height(font, size, line_height_mult);
    let lines = crate::fonts::lines::break_lines(adv, text, mode, max_width);
    for (i, line) in lines.iter().enumerate() {
        let baseline_y = origin_y + i as f32 * line_height + ascent;
        draw_text_shadows(&line.text, adv, origin_x, baseline_y, shadows, surface, width, height, damage_rects, clip);
        draw_shaped(&line.text, adv, origin_x, baseline_y, color, surface, width, height, damage_rects, clip);
        // Decorations span the line's ink extent (leading/trailing
        // collapsible spaces excluded).
        let lead = &line.text[..line.text.len() - line.text.trim_start_matches(' ').len()];
        let trail = &line.text[line.text.trim_end_matches(' ').len()..];
        let (lead_w, trail_w) = (adv.str(lead), adv.str(trail));
        let (x0, x1) = (origin_x + lead_w, origin_x + (line.width - trail_w).max(lead_w));
        draw_decorations(font, size, deco, x0, x1, baseline_y, color, surface, width, height, damage_rects, clip);
    }
}

/// The ordinal of a list item: its `value`, else the list's `start` (1)
/// plus the number of preceding items (`reversed` counts down).
fn list_ordinal(li: &crate::dom::NodeRef) -> i64 {
    let attr_num = |n: &crate::dom::NodeRef, a: &str| {
        n.as_element().and_then(|e| e.attributes.borrow().get(a).and_then(|v| v.trim().parse::<i64>().ok()))
    };
    if let Some(v) = attr_num(li, "value") {
        return v;
    }
    let is_li = |n: &crate::dom::NodeRef| n.as_element().is_some_and(|e| e.name.local.as_ref() == "li");
    let before = li.preceding_siblings().filter(|n| is_li(n)).count() as i64;
    let parent = li.parent();
    let reversed = parent.as_ref().and_then(|p| p.as_element().map(|e| e.attributes.borrow().get("reversed").is_some())).unwrap_or(false);
    if reversed {
        let total = parent.as_ref().map(|p| p.children().filter(|n| is_li(n)).count() as i64).unwrap_or(1);
        parent.as_ref().and_then(|p| attr_num(p, "start")).unwrap_or(total) - before
    } else {
        parent.as_ref().and_then(|p| attr_num(p, "start")).unwrap_or(1) + before
    }
}

fn roman(mut n: i64) -> String {
    if n <= 0 || n >= 4000 {
        return n.to_string();
    }
    let table = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: i64) -> String {
    if n <= 0 {
        return n.to_string();
    }
    let mut n = n;
    let mut s = Vec::new();
    while n > 0 {
        n -= 1;
        s.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    s.iter().rev().collect()
}

/// The marker text of a counter style (css-counter-styles-3 §6-7 for the
/// predefined styles): "3. ", "c. ", "iv. ", "03. ".
pub(crate) fn marker_text(style: u8, n: i64) -> String {
    format!("{}. ", counter_text(style.max(4), n))
}

/// A counter's representation in a predefined counter style (css-counter-styles-3 §6-7), for
/// `counter()`/`counters()` (AETHERGEN, SR71): the `list_style` codes; disc/circle/square are the
/// UA's bullet characters, `none` is empty, a value outside an alphabetic/additive style's range
/// falls back to decimal.
pub(crate) fn counter_text(style: u8, n: i64) -> String {
    match style {
        0 => String::new(),
        1 => "\u{2022}".into(),
        2 => "\u{25E6}".into(),
        3 => "\u{25AA}".into(),
        5 => alpha(n),
        6 => alpha(n).to_uppercase(),
        7 => roman(n),
        8 => roman(n).to_uppercase(),
        9 if (0..10).contains(&n) => format!("0{n}"),
        9 if (-9..0).contains(&n) => format!("-0{}", -n),
        10 if n > 0 => {
            const GREEK: [char; 24] = ['α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π', 'ρ', 'σ', 'τ', 'υ', 'φ', 'χ', 'ψ', 'ω'];
            let mut n = n;
            let mut s = Vec::new();
            while n > 0 {
                n -= 1;
                s.push(GREEK[(n % 24) as usize]);
                n /= 24;
            }
            s.iter().rev().collect()
        }
        _ => n.to_string(),
    }
}

/// Paints an outside list marker. Bullets are Blink's shapes — a disc,
/// circle or square of about a third of the em, centred ~0.27em above the
/// baseline, its left edge one em before the content edge; counters are
/// the marker text in the item's font, ending at the content edge.
#[allow(clippy::too_many_arguments)]
fn paint_marker(
    li: &crate::dom::NodeRef, style: u8, content_x: f32, baseline: f32, adv: &Advancer,
    color: (u8, u8, u8), surface: &mut [u8], width: u32, height: u32,
    damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    let fs = adv.size;
    if (1..=3).contains(&style) {
        let d = (fs * 0.31).round().max(3.0);
        let x = (content_x - fs).round();
        let cy = baseline - (fs * 0.27).round();
        let y = (cy - d / 2.0).round();
        let mut blend_at = |px: u32, py: u32, c: (u8, u8, u8), a: f32| {
            if !in_damage(px, py, damage_rects) || !in_clip(px, py, clip) {
                return;
            }
            if a >= 0.999 {
                put_px(surface, width, px, py, c);
            } else if a > 0.0 {
                blend_px(surface, width, px, py, c, (a * 255.0).round() as u8);
            }
        };
        match style {
            1 => boxpaint::paint(x, y, d, d, [d / 2.0; 4], [None; 4], Some(color), width, height, &mut blend_at),
            2 => boxpaint::paint(x, y, d, d, [d / 2.0; 4], [Some((1.0, color, 0)); 4], None, width, height, &mut blend_at),
            _ => boxpaint::paint(x, y, d, d, [0.0; 4], [None; 4], Some(color), width, height, &mut blend_at),
        }
        return;
    }
    let text = marker_text(style, list_ordinal(li));
    let w = adv.str(&text);
    draw_shaped(&text, adv, content_x - w, baseline, color, surface, width, height, damage_rects, clip);
}

/// A checkbox or radio at Chromium's control-theme look: 13x13, a 1px
/// #767676 frame (radius 2, or a circle) on white; checked, a #0075ff fill
/// with a white tick, or a #0075ff ring around a #0075ff dot.
#[allow(clippy::too_many_arguments)]
fn paint_checkable(
    x: f32, y: f32, w: f32, h: f32, radio: bool, checked: bool,
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    let mut blend_at = |px: u32, py: u32, c: (u8, u8, u8), a: f32| {
        if !in_damage(px, py, damage_rects) || !in_clip(px, py, clip) {
            return;
        }
        if a >= 0.999 {
            put_px(surface, width, px, py, c);
        } else if a > 0.0 {
            blend_px(surface, width, px, py, c, (a * 255.0).round() as u8);
        }
    };
    let blue = (0, 117, 255);
    let grey = (118, 118, 118);
    let r = if radio { [w.min(h) / 2.0; 4] } else { [2.0; 4] };
    let frame = |c| [Some((1.0, c, 0u8)); 4];
    if radio {
        if checked {
            boxpaint::paint(x, y, w, h, r, frame(blue), Some((255, 255, 255)), width, height, &mut blend_at);
            let d = w.min(h) * 0.55;
            boxpaint::paint(x + (w - d) / 2.0, y + (h - d) / 2.0, d, d, [d / 2.0; 4], [None; 4], Some(blue), width, height, &mut blend_at);
        } else {
            boxpaint::paint(x, y, w, h, r, frame(grey), Some((255, 255, 255)), width, height, &mut blend_at);
        }
        return;
    }
    if !checked {
        boxpaint::paint(x, y, w, h, r, frame(grey), Some((255, 255, 255)), width, height, &mut blend_at);
        return;
    }
    boxpaint::paint(x, y, w, h, r, [None; 4], Some(blue), width, height, &mut blend_at);
    // The tick: (3, 6.7) -> (5.4, 9.4) -> (10.3, 3) in the 13px box (fitted to Chromium's 1x pixels).
    let seg = |p: (f32, f32), q: (f32, f32), blend: &mut dyn FnMut(u32, u32, (u8, u8, u8), f32)| {
        let (x0, y0, x1, y1) = (x + p.0 * w / 13.0, y + p.1 * h / 13.0, x + q.0 * w / 13.0, y + q.1 * h / 13.0);
        let (minx, maxx) = (x0.min(x1).floor() - 1.0, x0.max(x1).ceil() + 1.0);
        let (miny, maxy) = (y0.min(y1).floor() - 1.0, y0.max(y1).ceil() + 1.0);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = dx * dx + dy * dy;
        let mut yy = miny;
        while yy <= maxy {
            let mut xx = minx;
            while xx <= maxx {
                let (cx, cy) = (xx + 0.5, yy + 0.5);
                let t = (((cx - x0) * dx + (cy - y0) * dy) / len2).clamp(0.0, 1.0);
                let (ex, ey) = (cx - (x0 + t * dx), cy - (y0 + t * dy));
                let d = (ex * ex + ey * ey).sqrt();
                let a = (1.25 - d).clamp(0.0, 1.0);
                if a > 0.0 && xx >= 0.0 && yy >= 0.0 {
                    blend(xx as u32, yy as u32, (255, 255, 255), a);
                }
                xx += 1.0;
            }
            yy += 1.0;
        }
    };
    seg((3.0, 6.7), (5.4, 9.4), &mut blend_at);
    seg((5.4, 9.4), (10.3, 3.0), &mut blend_at);
}

/// Single-line advance width of `text`, measured EXACTLY the way `draw_text`
/// lays it out (whitespace collapsed, shaped). Centering a control label
/// with any other measurement drifts.
fn measure_text_width(text: &str, adv: &Advancer) -> f32 {
    adv.str(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// (ascent, descent) in pixels at `font_size` — descent is positive-down, so
/// the inked extent of one line is `ascent + descent`.
fn text_extents(font: &Font, font_size: f32) -> (f32, f32) {
    let metrics = font.metrics();
    let scale = font_size / metrics.units_per_em as f32;
    (metrics.ascent * scale, -metrics.descent * scale)
}

/// `draw_text` origin_y that puts the FIRST line's inked extent centred on
/// `center_y`. `draw_text` adds the ascent to reach the baseline, so the
/// origin sits half an inked line above the centre.
fn centered_line_origin_y(font: &Font, font_size: f32, center_y: f32) -> f32 {
    let (ascent, descent) = text_extents(font, font_size);
    center_y - (ascent + descent) / 2.0
}

/// UNAOS_LAYOUTDUMP=<max-depth>: writes the computed box tree (tag, id/class,
/// rect, display, paint gates) to stderr. Diagnostic only — off by default.
pub fn dump_layout(layout: &LayoutTree) {
    let Ok(max_depth) = std::env::var("UNAOS_LAYOUTDUMP") else { return };
    let max_depth: usize = max_depth.trim().parse().unwrap_or(6);
    fn walk(layout: &LayoutTree, id: NodeId, x: f32, y: f32, depth: usize, max_depth: usize) {
        let Ok(b) = layout.taffy.layout(id) else { return };
        let (cx, cy) = (x + b.location.x, y + b.location.y);
        if depth <= max_depth {
            let st = layout.taffy.style(id);
            let disp = st.map(|s| format!("{:?}", s.display)).unwrap_or_default();
            let p = layout.paint_map.get(&id);
            let mut desc = String::new();
            if let Some(n) = layout.node_map.get(&id) {
                if let Some(el) = n.as_element() {
                    desc.push_str(el.name.local.as_ref());
                    let a = el.attributes.borrow();
                    if let Some(i) = a.get("id") {
                        desc.push_str(&format!("#{}", i));
                    }
                    if let Some(c) = a.get("class") {
                        desc.push_str(&format!(".{}", &c[..c.len().min(60)]));
                    }
                } else if n.as_text().is_some() {
                    let t = n.text_contents();
                    let t = t.trim();
                    desc = format!("\"{}\"", &t[..t.len().min(40)]);
                }
            }
            eprintln!(
                "{:indent$}{} @({:.0},{:.0}) {:.0}x{:.0} {} hid={:?} clip={:?}",
                "", desc, cx, cy, b.size.width, b.size.height, disp,
                p.and_then(|p| p.hidden), p.and_then(|p| p.clip),
                indent = depth * 2,
            );
            if let Some(il) = layout.inline.get(&id) {
                for (t, h, b) in &il.lines {
                    eprintln!("{:indent$}| line y={t:.2} h={h:.2} baseline={b:.2}", "", indent = depth * 2 + 2);
                }
                for f in &il.frags {
                    eprintln!("{:indent$}| {f:?}", "", indent = depth * 2 + 2);
                }
            }
        }
        if depth >= max_depth {
            return;
        }
        if let Ok(kids) = layout.taffy.children(id) {
            for k in kids {
                walk(layout, k, cx, cy, depth + 1, max_depth);
            }
        }
    }
    walk(layout, layout.root_node, 0.0, 0.0, 0, max_depth);
}

/// The colour that fills the whole canvas: `html`'s background-color, or,
/// when the root has none, `body`'s. None = no author background (white).
pub fn canvas_background(layout: &LayoutTree) -> Option<(u8, u8, u8)> {
    let find = |tag: &str| -> Option<NodeId> {
        // html and body sit within the top three levels of the box tree.
        let mut level = vec![layout.root_node];
        for _ in 0..4 {
            let mut next = Vec::new();
            for id in level {
                let is_tag = layout
                    .node_map
                    .get(&id)
                    .and_then(|n| n.as_element().map(|e| e.name.local.as_ref() == tag))
                    .unwrap_or(false);
                if is_tag {
                    return Some(id);
                }
                next.extend(layout.taffy.children(id).unwrap_or_default());
            }
            level = next;
        }
        None
    };
    let bg = |id: Option<NodeId>| {
        let p = id.and_then(|i| layout.paint_map.get(&i))?;
        let c = p.background?;
        let a = p.bg_alpha.unwrap_or(1.0);
        if a <= 0.0 {
            return None;
        }
        // Over the white the UA canvas starts as.
        let mix = |v: u8| (v as f32 * a + 255.0 * (1.0 - a)).round() as u8;
        Some((mix(c.0), mix(c.1), mix(c.2)))
    };
    bg(find("html")).or_else(|| bg(find("body")))
}

/// Paint a `<video>`/`<audio>` box (AETHERVIDEO, LEDGER SR39): the picture (poster or the
/// frame on glass) placed by `object-fit`/`object-position` inside the content box and clipped
/// to it, bilinear-sampled at pixel centres (exact at 1:1); Stria's error text when the stream
/// failed; and, with `controls`, a play/pause + progress + time strip along the bottom.
#[allow(clippy::too_many_arguments)]
fn paint_media(
    media: &crate::media::Paint,
    spec: &crate::layout::PaintStyle,
    content: (f32, f32, f32, f32),
    font: &Option<&'static Font>,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let (cx, cy, cw, ch) = content;
    if cw <= 0.0 || ch <= 0.0 {
        return;
    }
    // The content box, in clamped screen pixels, intersected with the clip.
    let span = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let x0 = x0.max(cx).max(clip.0).max(0.0);
        let y0 = y0.max(cy).max(clip.1).max(0.0);
        let x1 = x1.min(cx + cw).min(clip.2).min(width as f32);
        let y1 = y1.min(cy + ch).min(clip.3).min(height as f32);
        (x0.round() as u32, y0.round() as u32, (x1.round().max(0.0)) as u32, (y1.round().max(0.0)) as u32)
    };

    if let Some(err) = &media.error {
        let (x0, y0, x1, y1) = span(cx, cy, cx + cw, cy + ch);
        for y in y0..y1 {
            for x in x0..x1 {
                if in_damage(x, y, damage_rects) {
                    put_px(surface, width, x, y, (32, 32, 32));
                }
            }
        }
        if let Some(font) = font {
            let fs = 13.0;
            draw_text(
                err, cx + 8.0, centered_line_origin_y(font, fs, cy + ch.min(40.0) / 2.0), (cw - 16.0).max(1.0),
                &Advancer::new(ui_sel(), fs, 0.0), 1.2, (255, 255, 255), false, surface, width, height, damage_rects,
                (cx.max(clip.0), cy.max(clip.1), (cx + cw).min(clip.2), (cy + ch).min(clip.3)),
            );
        }
    } else if let Some(pic) = &media.picture {
        let (sw, sh) = (pic.width() as f32, pic.height() as f32);
        let fit = spec.object_fit.unwrap_or(1);
        let pos = spec.object_position.as_deref().map(crate::media::parse_object_position).unwrap_or((0.5, 0.5));
        let (rx, ry, rw, rh) = crate::media::object_fit_rect(fit, pos, cw, ch, sw, sh);
        let (dx, dy) = (cx + rx, cy + ry);
        let (x0, y0, x1, y1) = span(dx, dy, dx + rw, dy + rh);
        let (sx_scale, sy_scale) = (sw / rw, sh / rh);
        let (iw, ih) = (pic.width() as i32, pic.height() as i32);
        let raw = pic.as_raw();
        let at = |u: i32, v: i32| {
            let o = ((v.clamp(0, ih - 1) * iw + u.clamp(0, iw - 1)) * 4) as usize;
            [raw[o] as f32, raw[o + 1] as f32, raw[o + 2] as f32, raw[o + 3] as f32]
        };
        for y in y0..y1 {
            let fv = (y as f32 + 0.5 - dy) * sy_scale - 0.5;
            let v0 = fv.floor();
            let ty = fv - v0;
            for x in x0..x1 {
                if !in_damage(x, y, damage_rects) {
                    continue;
                }
                let fu = (x as f32 + 0.5 - dx) * sx_scale - 0.5;
                let u0 = fu.floor();
                let tx = fu - u0;
                let (u0, v0i) = (u0 as i32, v0 as i32);
                let (p00, p10, p01, p11) = (at(u0, v0i), at(u0 + 1, v0i), at(u0, v0i + 1), at(u0 + 1, v0i + 1));
                let mut c = [0u8; 4];
                for k in 0..4 {
                    let top = p00[k] + (p10[k] - p00[k]) * tx;
                    let bot = p01[k] + (p11[k] - p01[k]) * tx;
                    c[k] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
                }
                if c[3] == 255 {
                    put_px(surface, width, x, y, (c[0], c[1], c[2]));
                } else if c[3] > 0 {
                    blend_px(surface, width, x, y, (c[0], c[1], c[2]), c[3]);
                }
            }
        }
    }

    media_overlay::paint(media, content, font, surface, width, height, damage_rects, clip); // AETHERMEDIA2 (SR134)
    if media.controls && media.kind == crate::media::Kind::Audio {
        paint_audio_controls(media, content, font, surface, width, height, damage_rects, clip);
    } else if media.controls {
        // The strip: 32px, translucent black.
        let sh = if media.kind == crate::media::Kind::Audio { ch } else { ch.min(32.0) };
        let top = cy + ch - sh;
        let (x0, y0, x1, y1) = span(cx, top, cx + cw, cy + ch);
        for y in y0..y1 {
            for x in x0..x1 {
                if in_damage(x, y, damage_rects) {
                    blend_px(surface, width, x, y, (0, 0, 0), 160);
                }
            }
        }
        let mid = top + sh / 2.0;
        let white = (255, 255, 255);
        let fill = |surface: &mut [u8], fx0: f32, fy0: f32, fx1: f32, fy1: f32, test: &dyn Fn(f32, f32) -> bool, color: (u8, u8, u8)| {
            let (x0, y0, x1, y1) = span(fx0, fy0, fx1, fy1);
            for y in y0..y1 {
                for x in x0..x1 {
                    if in_damage(x, y, damage_rects) && test(x as f32 + 0.5, y as f32 + 0.5) {
                        put_px(surface, width, x, y, color);
                    }
                }
            }
        };
        let bx = cx + 10.0;
        if media.playing {
            // Pause: two bars.
            fill(surface, bx, mid - 7.0, bx + 4.0, mid + 7.0, &|_, _| true, white);
            fill(surface, bx + 8.0, mid - 7.0, bx + 12.0, mid + 7.0, &|_, _| true, white);
        } else {
            // Play: a right-pointing triangle.
            fill(surface, bx, mid - 7.0, bx + 12.0, mid + 7.0, &|x, y| (y - mid).abs() <= 7.0 * (1.0 - (x - bx) / 12.0), white);
        }
        // Time text and progress track.
        let label = format!(
            "{} / {}",
            crate::media::clock_text(media.pts_ns),
            crate::media::clock_text(media.duration_ns as i64)
        );
        let mut track_x0 = bx + 22.0;
        if let Some(font) = font {
            let fs = 12.0;
            let tw = measure_text_width(&label, &Advancer::new(ui_sel(), fs, 0.0));
            if cw > tw + 80.0 {
                draw_text(
                    &label, track_x0, centered_line_origin_y(font, fs, mid), tw + 2.0, &Advancer::new(ui_sel(), fs, 0.0), 1.2,
                    white, false, surface, width, height, damage_rects, clip,
                );
                track_x0 += tw + 10.0;
            }
        }
        let track_x1 = cx + cw - 10.0;
        if track_x1 > track_x0 + 4.0 {
            fill(surface, track_x0, mid - 2.0, track_x1, mid + 2.0, &|_, _| true, (110, 110, 110));
            if media.duration_ns > 0 {
                let f = (media.pts_ns.max(0) as f32 / media.duration_ns as f32).clamp(0.0, 1.0);
                fill(surface, track_x0, mid - 2.0, track_x0 + (track_x1 - track_x0) * f, mid + 2.0, &|_, _| true, white);
            }
        }
    }
}

/// `<audio controls>` as Chromium draws its default audio control (AUDIOTRACK, LEDGER SR45),
/// geometry measured from Chromium's own rendering of a 300×54 control: a pill of
/// rgb(241,243,244) with fully rounded ends; a play triangle (pause bars while playing) — the
/// Material icons at 20 px, at x+16; the `m:ss / m:ss` time at x+47, 13 px; the timeline
/// x+129 … right−91, 4 px tall with round caps, played part rgb(11,11,11), the rest
/// rgb(88,89,89); the speaker (Material `volume_up`, 24 px) at right−69; the overflow dots
/// (`more_vert`, 20 px) at right−37.5. Shapes are coverage-antialiased (4×4 samples per
/// pixel). Stria's meter frames move the time while it plays.
#[allow(clippy::too_many_arguments)]
fn paint_audio_controls(
    media: &crate::media::Paint,
    content: (f32, f32, f32, f32),
    font: &Option<&'static Font>,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let (cx, cy, cw, ch) = content;
    let lim = (cx.max(clip.0).max(0.0), cy.max(clip.1).max(0.0), (cx + cw).min(clip.2).min(width as f32), (cy + ch).min(clip.3).min(height as f32));
    // Fill `inside` over the rectangle with 4×4-sample coverage.
    let cover = |surface: &mut [u8], r: (f32, f32, f32, f32), inside: &dyn Fn(f32, f32) -> bool, color: (u8, u8, u8)| {
        let x0 = r.0.max(lim.0).floor().max(0.0) as u32;
        let y0 = r.1.max(lim.1).floor().max(0.0) as u32;
        let x1 = r.2.min(lim.2).ceil().max(0.0) as u32;
        let y1 = r.3.min(lim.3).ceil().max(0.0) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                if !in_damage(x, y, damage_rects) {
                    continue;
                }
                let mut n = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                            n += 1;
                        }
                    }
                }
                match n {
                    0 => {}
                    16 => put_px(surface, width, x, y, color),
                    _ => blend_px(surface, width, x, y, color, (n * 255 / 16) as u8),
                }
            }
        }
    };
    let mid = cy + ch / 2.0;
    let right = cx + cw;
    // the pill
    let rad = (ch / 2.0).min(cw / 2.0);
    let pill = |x: f32, y: f32| {
        let qx = if x < cx + rad { cx + rad - x } else if x > right - rad { x - (right - rad) } else { 0.0 };
        let qy = ((y - mid).abs() - (ch / 2.0 - rad)).max(0.0);
        qx * qx + qy * qy <= rad * rad && y >= cy && y <= cy + ch
    };
    cover(surface, (cx, cy, right, cy + ch), &pill, (241, 243, 244));
    let black = (0, 0, 0);
    // play / pause: Material play_arrow / pause at 20 px (scale 5/6), origin (x+16⅓, mid−10)
    let (ox, oy, k) = (cx + 16.0 + 1.0 / 3.0, mid - 10.0, 20.0 / 24.0);
    if media.playing {
        let bars = move |x: f32, y: f32| {
            let (u, v) = ((x - ox) / k, (y - oy) / k);
            (5.0..=19.0).contains(&v) && ((6.0..=10.0).contains(&u) || (14.0..=18.0).contains(&u))
        };
        cover(surface, (ox, oy, ox + 20.0, oy + 20.0), &bars, black);
    } else {
        let tri = move |x: f32, y: f32| {
            let (u, v) = ((x - ox) / k, (y - oy) / k);
            u >= 8.0 && u <= 19.0 && (v - 12.0).abs() <= 7.0 * (19.0 - u) / 11.0
        };
        cover(surface, (ox, oy, ox + 20.0, oy + 20.0), &tri, black);
    }
    // the time
    let label = format!("{} / {}", crate::media::clock_text(media.pts_ns), crate::media::clock_text(media.duration_ns as i64));
    if let Some(font) = font {
        let fs = 13.0;
        let tw = measure_text_width(&label, &Advancer::new(ui_sel(), fs, 0.0));
        draw_text(&label, cx + 47.0, centered_line_origin_y(font, fs, mid), tw + 2.0, &Advancer::new(ui_sel(), fs, 0.0), 1.2, (31, 31, 31), false, surface, width, height, damage_rects, lim);
    }
    // the timeline
    let (t0, t1) = (cx + 129.0, right - 91.0);
    if t1 > t0 + 4.0 {
        let cap = move |x: f32, y: f32| {
            let dx = if x < t0 + 2.0 { t0 + 2.0 - x } else if x > t1 - 2.0 { x - (t1 - 2.0) } else { 0.0 };
            dx * dx + (y - mid) * (y - mid) <= 4.0 && x >= t0 && x <= t1
        };
        let f = if media.duration_ns > 0 { (media.pts_ns.max(0) as f32 / media.duration_ns as f32).clamp(0.0, 1.0) } else { 0.0 };
        let split = t0 + (t1 - t0) * f;
        cover(surface, (t0, mid - 2.0, t1, mid + 2.0), &move |x, y| cap(x, y) && x >= split, (88, 89, 89));
        if f > 0.0 {
            cover(surface, (t0, mid - 2.0, split, mid + 2.0), &move |x, y| cap(x, y) && x < split, (11, 11, 11));
        }
    }
    // the speaker: Material volume_up at 24 px, origin (right−69, mid−12)
    let (sx, sy) = (right - 69.0, mid - 12.0);
    if sx > t1 {
        let body = move |x: f32, y: f32| {
            let (u, v) = (x - sx, y - sy);
            let rect = (3.0..=7.0).contains(&u) && (9.0..=15.0).contains(&v);
            let cone = (7.0..=12.0).contains(&u) && (v - 12.0).abs() <= 3.0 + (u - 7.0);
            rect || cone
        };
        cover(surface, (sx, sy, sx + 24.0, sy + 24.0), &body, black);
        let waves = move |x: f32, y: f32| {
            let (u, v) = (x - sx, y - sy);
            let r = ((u - 12.0) * (u - 12.0) + (v - 12.0) * (v - 12.0)).sqrt();
            u >= 14.0 && (r <= 4.5 || (7.0..=9.0).contains(&r))
        };
        cover(surface, (sx, sy, sx + 24.0, sy + 24.0), &waves, black);
    }
    // the overflow menu: Material more_vert at 20 px, dots r 5/3 at (10, 5|10|15)
    let (mx, my) = (right - 37.5, mid - 10.5);
    if mx > t1 {
        let dots = move |x: f32, y: f32| {
            let (u, v) = (x - mx - 10.0, y - my);
            [5.0f32, 10.0, 15.0].iter().any(|c| u * u + (v - c) * (v - c) <= (5.0f32 / 3.0) * (5.0 / 3.0))
        };
        cover(surface, (mx, my, mx + 20.0, my + 20.0), &dots, black);
    }
}

/// AETHERSTYLE3 (SR103): the overlay scrollbar colours (Chromium's light-theme track and thumb).
const SCROLL_TRACK: (u8, u8, u8) = (241, 241, 241);
const SCROLL_THUMB: (u8, u8, u8) = (193, 193, 193);

/// The bounding box (x0, y0, x1, y1 inclusive) of the pixels `surface` changed against `snap`; None = none.
fn changed_bounds(surface: &[u8], snap: &[u8], width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            if surface[i..i + 3] != snap[i..i + 3] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 <= x1 && y0 <= y1).then_some((x0, y0, x1, y1))
}

/// AETHERSTYLE3 (SR103): a `filter` group. The pixels the subtree changed against `snap` are the group (a
/// straight-alpha layer, opaque where drawn), padded by the list's spread, run through svg_core's CSS filter
/// hook (`layout_core::effects::filter_rgba`) and composited source-over onto the snapshot, clipped to `clip`.
///
/// AETHERSTYLE3B (SR115): a `url(#id)` entry runs the page's SVG `<filter>` `resolve` serializes, with `rect`
/// (the element's border box, screen px) as its bounding box (`layout_core::effects::filter_rgba_refs`).
#[allow(clippy::too_many_arguments)]
fn filter_group(
    surface: &mut [u8],
    snap: &[u8],
    width: u32,
    height: u32,
    value: &str,
    cx: &layout_core::effects::FilterCx,
    clip: Clip,
    rect: Clip,
    resolve: &dyn Fn(&str) -> Option<String>,
) {
    let Some((x0, y0, x1, y1)) = changed_bounds(surface, snap, width, height) else { return };
    let m = layout_core::effects::filter_margin_box(value, cx, rect.2 - rect.0, rect.3 - rect.1) as i64;
    let rx0 = (x0 as i64 - m).max(0).max(clip.0.floor() as i64) as u32;
    let ry0 = (y0 as i64 - m).max(0).max(clip.1.floor() as i64) as u32;
    let rx1 = ((x1 as i64 + 1 + m).min(width as i64).min(clip.2.ceil() as i64)).max(rx0 as i64) as u32;
    let ry1 = ((y1 as i64 + 1 + m).min(height as i64).min(clip.3.ceil() as i64)).max(ry0 as i64) as u32;
    let (lw, lh) = ((rx1 - rx0) as usize, (ry1 - ry0) as usize);
    if lw == 0 || lh == 0 {
        return;
    }
    let mut layer = vec![0u8; lw * lh * 4];
    for y in 0..lh {
        for x in 0..lw {
            let i = (((ry0 as usize + y) * width as usize) + rx0 as usize + x) * 4;
            if surface[i..i + 3] != snap[i..i + 3] {
                let o = (y * lw + x) * 4;
                layer[o..o + 4].copy_from_slice(&[surface[i + 2], surface[i + 1], surface[i], 255]);
            }
        }
    }
    let bbox = (rect.0 - rx0 as f32, rect.1 - ry0 as f32, rect.2 - rect.0, rect.3 - rect.1);
    if !layout_core::effects::filter_rgba_refs(&mut layer, lw, lh, value, cx, bbox, resolve) {
        return;
    }
    // the whole surface returns to the backdrop, then the filtered layer composites over it
    for y in y0..=y1 {
        let a = ((y * width + x0) * 4) as usize;
        let b = ((y * width + x1) * 4 + 4) as usize;
        surface[a..b].copy_from_slice(&snap[a..b]);
    }
    for y in 0..lh {
        for x in 0..lw {
            let i = (((ry0 as usize + y) * width as usize) + rx0 as usize + x) * 4;
            let o = (y * lw + x) * 4;
            let src = [layer[o], layer[o + 1], layer[o + 2], layer[o + 3]];
            let (r, g, b) = layout_core::effects::over((snap[i + 2], snap[i + 1], snap[i]), src);
            surface[i] = b;
            surface[i + 1] = g;
            surface[i + 2] = r;
            surface[i + 3] = 255;
        }
    }
}

/// AETHERSTYLE3 (SR103): `backdrop-filter` (Filter Effects 2 §3) — the backdrop under the box's border box
/// (`rect`, screen px) is filtered before the box paints: the region padded by the list's spread is read
/// opaque, filtered, and written back inside the border box (and `clip`) only.
#[allow(clippy::too_many_arguments)]
fn backdrop_group(
    surface: &mut [u8],
    width: u32,
    height: u32,
    rect: Clip,
    value: &str,
    cx: &layout_core::effects::FilterCx,
    clip: Clip,
    resolve: &dyn Fn(&str) -> Option<String>,
) {
    let m = layout_core::effects::filter_margin_box(value, cx, rect.2 - rect.0, rect.3 - rect.1);
    let rx0 = (rect.0 - m).floor().max(0.0) as u32;
    let ry0 = (rect.1 - m).floor().max(0.0) as u32;
    let rx1 = ((rect.2 + m).ceil().max(0.0) as u32).min(width);
    let ry1 = ((rect.3 + m).ceil().max(0.0) as u32).min(height);
    if rx1 <= rx0 || ry1 <= ry0 {
        return;
    }
    let (lw, lh) = ((rx1 - rx0) as usize, (ry1 - ry0) as usize);
    let mut layer = vec![0u8; lw * lh * 4];
    for y in 0..lh {
        for x in 0..lw {
            let i = (((ry0 as usize + y) * width as usize) + rx0 as usize + x) * 4;
            let o = (y * lw + x) * 4;
            layer[o..o + 4].copy_from_slice(&[surface[i + 2], surface[i + 1], surface[i], 255]);
        }
    }
    let bbox = (rect.0 - rx0 as f32, rect.1 - ry0 as f32, rect.2 - rect.0, rect.3 - rect.1);
    if !layout_core::effects::filter_rgba_refs(&mut layer, lw, lh, value, cx, bbox, resolve) {
        return;
    }
    let bx0 = rect.0.max(clip.0).max(0.0).floor() as u32;
    let by0 = rect.1.max(clip.1).max(0.0).floor() as u32;
    let bx1 = (rect.2.min(clip.2).max(0.0).ceil() as u32).min(width);
    let by1 = (rect.3.min(clip.3).max(0.0).ceil() as u32).min(height);
    for y in by0.max(ry0)..by1.min(ry1) {
        for x in bx0.max(rx0)..bx1.min(rx1) {
            let i = ((y * width + x) * 4) as usize;
            let o = (((y - ry0) as usize) * lw + (x - rx0) as usize) * 4;
            let src = [layer[o], layer[o + 1], layer[o + 2], layer[o + 3]];
            let (r, g, b) = layout_core::effects::over((surface[i + 2], surface[i + 1], surface[i]), src);
            surface[i] = b;
            surface[i + 1] = g;
            surface[i + 2] = r;
        }
    }
}

/// Where a box's border box sits in page coordinates: its laid-out place (`abs` + its location), except
/// `position: fixed` — the containing block is the viewport (CSS 2.2 §10.1): insets resolve against it
/// and the box ignores scrolling — and `position: sticky` (css-position-3 §3.4, AETHERSTYLE2 SR95): the
/// box keeps its normal-flow place until scrolling would carry it past an inset edge of the scrollport
/// (the viewport; Aether has no inner scrollers), then holds at that edge, never leaving its containing
/// block (the parent's content box).
#[allow(clippy::too_many_arguments)]
fn placed_origin(
    layout: &LayoutTree,
    node_id: NodeId,
    abs_x: f32,
    abs_y: f32,
    layout_box: &taffy::Layout,
    width: u32,
    height: u32,
    sx: i32,
    sy: i32,
) -> (f32, f32) {
    let mut current_x = abs_x + layout_box.location.x;
    let mut current_y = abs_y + layout_box.location.y;
    let paint = layout.paint_map.get(&node_id);
    let (vw, vh) = (width as f32, height as f32);
    let (w, h) = (layout_box.size.width, layout_box.size.height);
    match paint.and_then(|p| p.position_kind) {
        Some(3) => {
            // AETHERFIT (SR189): layout_core laid the box out against the viewport (its margins, `fit-content`
            // and auto-margin centring included) and recorded its viewport origin per axis with an inset.
            if let Some(&(fx, fy)) = layout.fixed_at.get(&node_id) {
                if let Some(x) = fx {
                    current_x = sx as f32 + x;
                }
                if let Some(y) = fy {
                    current_y = sy as f32 + y;
                }
            }
        }
        Some(4) => {
            let Some(ins) = paint.and_then(|p| p.inset_px) else { return (current_x, current_y) };
            // the containing block: the parent's content box (abs is the parent's border-box origin)
            let (cb_l, cb_t, cb_r, cb_b) = match layout.taffy.parent(node_id).and_then(|p| layout.taffy.layout(p).ok()) {
                Some(pl) => (
                    abs_x + pl.border.left + pl.padding.left,
                    abs_y + pl.border.top + pl.padding.top,
                    abs_x + pl.size.width - pl.border.right - pl.padding.right,
                    abs_y + pl.size.height - pl.border.bottom - pl.padding.bottom,
                ),
                None => (f32::MIN, f32::MIN, f32::MAX, f32::MAX),
            };
            let (nx, ny) = (current_x, current_y);
            if let Some(t) = ins[0] {
                let target = sy as f32 + t;
                if ny < target {
                    current_y = target.min(cb_b - h).max(ny);
                }
            } else if let Some(b) = ins[2] {
                let target = sy as f32 + vh - b - h;
                if ny > target {
                    current_y = target.max(cb_t).min(ny);
                }
            }
            if let Some(l) = ins[3] {
                let target = sx as f32 + l;
                if nx < target {
                    current_x = target.min(cb_r - w).max(nx);
                }
            } else if let Some(r) = ins[1] {
                let target = sx as f32 + vw - r - w;
                if nx > target {
                    current_x = target.max(cb_l).min(nx);
                }
            }
        }
        _ => {}
    }
    (current_x, current_y)
}

/// AETHERRENDER3 (SR126): the `text-combine-upright` cell (`InlineLayout::combined`, layout_core inline.rs
/// `upright_cells`) of the text fragment at staging x `x` on the line of `baseline`, when its glyphs must shrink to
/// fit it (scale below 1).
fn combined_cell(il: &layout_core::inline::InlineLayout, x: f32, baseline: f32) -> Option<layout_core::writing::CombinedCell> {
    let (_, top, _) = layout_core::hit::line_of(il, baseline)?;
    il.combined.iter().copied().find(|c| (c.cell.x - x).abs() < 0.01 && (c.cell.top - top).abs() < 0.01 && c.scale < 1.0)
}

/// AETHERRENDER3 (SR126): compresses a combined run drawn at its natural width into its cell (css-writing-modes-4
/// §9.1: the glyphs are compressed to fit 1em). `pre` is the staging buffer before the run drew; on the rows
/// `y0` .. `y1` (the line box), the run's natural span `[cell.x, cell.x + cell.w / scale)` (plus 2px of glyph
/// overhang each side) goes back to `pre`, then each column of the cell takes the box-filtered average of the drawn
/// columns it covers under the horizontal scale about the cell's start — a column only where the run changed one
/// of them, so the backdrop outside the glyphs is left as it was.
fn compress_combined(surface: &mut [u8], pre: &[u8], width: u32, height: u32, c: layout_core::writing::CombinedCell, y0: f32, y1: f32) {
    if c.scale <= 0.0 || c.scale >= 1.0 {
        return;
    }
    let x0 = c.cell.x;
    let natural = c.cell.w / c.scale;
    let (sx0, sx1) = ((x0.floor() as i64 - 2).max(0), ((x0 + natural).ceil() as i64 + 2).min(width as i64));
    let (dx0, dx1) = (x0.floor() as i64, ((x0 + c.cell.w).ceil() as i64).min(width as i64));
    if sx1 <= sx0 || dx1 <= dx0 {
        return;
    }
    let (ys, ye) = (y0.max(0.0) as u32, (y1.max(0.0) as u32).min(height));
    for y in ys..ye {
        let at = |x: i64| ((y as i64 * width as i64 + x) * 4) as usize;
        let drawn: Vec<[u8; 4]> = (sx0..sx1).map(|x| {
            let i = at(x);
            [surface[i], surface[i + 1], surface[i + 2], surface[i + 3]]
        }).collect();
        for x in sx0..sx1 {
            let i = at(x);
            surface[i..i + 4].copy_from_slice(&pre[i..i + 4]);
        }
        for dx in dx0..dx1 {
            let a = x0 + (dx as f32 - x0) / c.scale;
            let b = x0 + (dx as f32 + 1.0 - x0) / c.scale;
            let (mut acc, mut wsum, mut ink) = ([0.0f32; 3], 0.0f32, false);
            for s in (a.floor() as i64).max(sx0)..(b.ceil() as i64).min(sx1) {
                let wgt = (b.min(s as f32 + 1.0) - a.max(s as f32)).max(0.0);
                let d = drawn[(s - sx0) as usize];
                let i = at(s);
                ink |= d[..3] != pre[i..i + 3];
                for k in 0..3 {
                    acc[k] += d[k] as f32 * wgt;
                }
                wsum += wgt;
            }
            if ink && wsum > 0.0 && dx >= 0 {
                let i = at(dx);
                for k in 0..3 {
                    surface[i + k] = (acc[k] / wsum).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// AETHERRENDER3 (SR126): the caret of a vertical editing host — when the selection is collapsed in a Text node of
/// `il` that sits inside an element whose `contenteditable` is not "false", the text fragment holding the caret and
/// the caret's byte offset in it (the DOM offset mapped through `layout_core::hit::align`, as the selection painter
/// maps a range).
fn editable_caret(layout: &LayoutTree, il: &layout_core::inline::InlineLayout) -> Option<(usize, usize)> {
    use crate::layout::inline::Frag;
    let sel = crate::selection::get().filter(|s| s.is_collapsed())?;
    let p = sel.focus;
    let mut tid = None;
    let mut idxs = Vec::new();
    for (i, f) in il.frags.iter().enumerate() {
        if let Frag::Text { node, .. } = f {
            if layout.node_map.get(node).is_some_and(|n| n.id().0 == p.node) {
                tid = Some(*node);
                idxs.push(i);
            }
        }
    }
    let tid = tid?;
    let dn = layout.node_map.get(&tid)?;
    let editable = dn.ancestors().any(|a| {
        a.as_element().is_some_and(|el| el.attributes.borrow().get("contenteditable").is_some_and(|v| !v.trim().eq_ignore_ascii_case("false")))
    });
    if !editable {
        return None;
    }
    let texts: Vec<&str> = idxs
        .iter()
        .map(|&i| match &il.frags[i] {
            Frag::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect();
    let maps = layout_core::hit::align(&dn.text_contents(), &texts, layout_core::hit::collapses_at(layout, tid));
    for (k, &fi) in idxs.iter().enumerate() {
        let m = maps.get(k)?;
        let (Some(&first), Some(&last)) = (m.first(), m.last()) else { continue };
        if p.offset < first || p.offset > last {
            continue;
        }
        let c = m.iter().position(|&o| o >= p.offset).unwrap_or(m.len() - 1);
        let b = texts[k].char_indices().nth(c).map(|(b, _)| b).unwrap_or(texts[k].len());
        return Some((fi, b));
    }
    None
}

/// AETHERRENDER3 (SR126): one column fragment's decoration pass (css-break-3 §5.4 `box-decoration-break`) for a box
/// whose border box sits at screen (`box_sx`, `box_sy`), `bh` tall, painting under `clip`: (origin x, origin y,
/// height, clip). `slice` paints the whole box moved by the fragment's shift and clipped to the piece's band
/// (`f.top` .. `f.top + f.height` of the box), so its block-start border, padding and shadow show on the first piece
/// only and the block-end ones on the last, the background positioned against the whole box; the band is open
/// above the first piece and below the last (an outer shadow past the box's own block edges still shows) and open
/// sideways (the box's inline edges are on every piece). `clone` paints a box of its own, `f.height` tall at the
/// piece's place, with every edge, its own shadow, its background positioned against the piece.
fn frag_pass(f: &layout_core::multicol::BoxFragment, box_sx: f32, box_sy: f32, bh: f32, clip: Clip) -> (f32, f32, f32, Clip) {
    let (px, py) = (box_sx + f.shift.0, box_sy + f.shift.1);
    if f.cloned {
        return (px, py + f.top, f.height, clip);
    }
    let top = if f.edges.0 { clip.1 } else { clip.1.max(py + f.top) };
    let bottom = if f.edges.1 { clip.3 } else { clip.3.min(py + f.top + f.height) };
    (px, py, bh, (clip.0, top, clip.2, bottom))
}

pub fn render_frame(
    layout: &LayoutTree,
    surface: &mut [u8],
    width: u32,
    height: u32,
    scroll_x: f64,
    scroll_y: f64,
    damage_rects: &[(u32, u32, u32, u32)],
) {
    frame_walk(layout, surface, width, height, scroll_x, scroll_y, damage_rects, false);
}

/// The frame's walks. `index_only` (AETHERPAINTPERF2, SR132): the index walk alone — no pixel written — for
/// [`relayout_damage`].
#[allow(clippy::too_many_arguments)]
fn frame_walk(
    layout: &LayoutTree,
    surface: &mut [u8],
    width: u32,
    height: u32,
    scroll_x: f64,
    scroll_y: f64,
    damage_rects: &[(u32, u32, u32, u32)],
    index_only: bool,
) {
    if damage_rects.is_empty() {
        return;
    }
    if !index_only {
        dump_layout(layout);
    }
    TEXT_BOXES.with(|t| t.borrow_mut().clear());

    // Clear damaged regions to the CANVAS background: the root element's
    // background, else body's (CSS Backgrounds §2.11.2 — the root/body
    // background propagates to the whole canvas, not just their boxes).
    let canvas = canvas_background(layout).unwrap_or((255, 255, 255));
    for &(dx, dy, dw, dh) in damage_rects.iter().filter(|_| !index_only) {
        let ex = (dx + dw).min(width);
        let ey = (dy + dh).min(height);
        for y in dy..ey {
            for x in dx..ex {
                put_px(surface, width, x, y, canvas);
            }
        }
    }

    let font = crate::fonts::face(&FontSel::new(crate::fonts::SANS, 400, false));
    let font_bold = crate::fonts::face(&FontSel::new(crate::fonts::SANS, 700, false));

    let sy = scroll_y as i32;
    let sx = scroll_x as i32;

    /// AETHERPAINTPERF (SR130): one box of the walk — on the index walk, its subtree ink is the union of what
    /// its body adds and what its nested calls record; on the indexed paint walk it is skipped when that ink meets
    /// no damage rect (its text boxes were recorded by the index walk).
    #[allow(clippy::too_many_arguments)]
    fn draw_node(
        node_id: NodeId,
        abs_x: f32,
        abs_y: f32,
        inherited: Inherited,
        layout: &LayoutTree,
        font: &Option<&'static Font>,
        font_bold: &Option<&'static Font>,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
        damage_rects: &[(u32, u32, u32, u32)],
        clip: Clip,
    ) {
        match walk_mode() {
            1 => {
                // AETHERPAINTPERF2 (SR132): the call's place in the walk and the call enclosing it.
                let (seq, enclosing) = INDEX.with(|x| {
                    let mut x = x.borrow_mut();
                    x.stack.push(INK_NONE);
                    x.seq += 1;
                    let enclosing = x.open.last().map(|o| o.0);
                    x.open.push((node_id, false));
                    (x.seq, enclosing)
                });
                draw_node_body(
                    node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                    damage_rects, clip,
                );
                INDEX.with(|x| {
                    let mut x = x.borrow_mut();
                    let r = x.stack.pop().unwrap_or(INK_NONE);
                    let own_unbounded = x.open.pop().is_some_and(|o| o.1);
                    x.ink.insert(node_id, r);
                    if let Some(top) = x.stack.last_mut() {
                        *top = ink_union(*top, r);
                    }
                    // AETHERPAINTPERF2 (SR132): an on-screen box goes into the frame's snapshot.
                    if ink_meets(r, &[(0, 0, width, height)]) {
                        let lb = layout.taffy.layout(node_id).ok().copied();
                        x.snap.boxes.insert(node_id, BoxSnap { seq, enclosing, ink: r, own_unbounded, abs: (abs_x, abs_y), layout: lb });
                    }
                });
            }
            2 => {
                let hit = INDEX.with(|x| x.borrow().ink.get(&node_id).is_none_or(|r| ink_meets(*r, damage_rects)));
                if !hit {
                    INDEX.with(|x| x.borrow_mut().skipped += 1);
                    return;
                }
                INDEX.with(|x| x.borrow_mut().painted += 1);
                draw_node_body(
                    node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                    damage_rects, clip,
                );
            }
            _ => {
                INDEX.with(|x| x.borrow_mut().painted += 1);
                draw_node_body(
                    node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                    damage_rects, clip,
                );
            }
        }
    }

    /// css-color-4 / css-masking: `opacity` < 1 paints the subtree as a
    /// GROUP and composites it once. Painting the group straight onto the
    /// backdrop and then mixing with a snapshot of the backdrop by the
    /// opacity is exact: per pixel, `b(1-a) + a(g·ga + b(1-ga))` =
    /// `b(1 - a·ga) + a·ga·g`, which is source-over of the group at a·ga.
    /// AETHERSTYLE2 (SR95): a transformed or clip-pathed box paints as a GROUP, the opacity group's
    /// shape: the subtree (its opacity included) is drawn onto the surface, the changed pixels are the
    /// group, the surface goes back to its snapshot, then the clip-path shape (box-local, so it moves
    /// with the box) drops the group pixels outside it and the transform maps the rest to their place —
    /// every destination pixel of the transformed bounds samples the group through the inverse matrix
    /// (nearest). The group is drawn where the untransformed box sits, so what the surface edge or an
    /// ancestor clip cut off there is not in the group.
    #[allow(clippy::too_many_arguments)]
    fn draw_node_body(
        node_id: NodeId,
        abs_x: f32,
        abs_y: f32,
        inherited: Inherited,
        layout: &LayoutTree,
        font: &Option<&'static Font>,
        font_bold: &Option<&'static Font>,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
        damage_rects: &[(u32, u32, u32, u32)],
        clip: Clip,
    ) {
        let paint = layout.paint_map.get(&node_id);
        let lb = layout.taffy.layout(node_id).ok().copied();
        let shape = paint.and_then(|p| p.clip_path.clone()).flatten();
        let tf = match (paint, lb) {
            (Some(p), Some(lb)) => p.transform_matrix(lb.size.width, lb.size.height),
            _ => None,
        };
        // AETHERSTYLE3 (SR103): filter / backdrop-filter, resolved against the element's font and colour.
        let hidden = paint.and_then(|p| p.hidden).unwrap_or(false);
        let filter = paint.and_then(|p| p.filter.clone()).flatten().filter(|_| !hidden);
        let backdrop = paint.and_then(|p| p.backdrop_filter.clone()).flatten().filter(|_| !hidden);
        let fcx = layout_core::effects::FilterCx {
            scale: 1.0,
            font_size: paint.and_then(|p| p.used_font_size).unwrap_or(inherited.font_size),
            viewport: (width as f32, height as f32),
            color: paint.and_then(|p| p.color).unwrap_or(inherited.color),
        };
        // AETHERSTYLE3B (SR115): `url(#id)` entries resolve against this element's document.
        let resolve = |id: &str| -> Option<String> {
            let n = layout.node_map.get(&node_id)?;
            layout_core::effects::resolve_filter(n, id)
        };
        if let (Some(bf), Some(lb)) = (&backdrop, lb) {
            if walk_mode() == 1 {
                ink_add(INK_ALL); // AETHERPAINTPERF: a backdrop filter reads and writes around the box
            } else {
            let (ox, oy) = placed_origin(layout, node_id, abs_x, abs_y, &lb, width, height, sx, sy);
            let (bx, by) = (ox - sx as f32, oy - sy as f32);
            backdrop_group(surface, width, height, (bx, by, bx + lb.size.width, by + lb.size.height), bf, &fcx, clip, &resolve);
            }
        }
        let (Some(lb), true) = (lb, tf.is_some() || shape.is_some() || filter.is_some()) else {
            draw_node_op(
                node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                damage_rects, clip,
            );
            return;
        };
        if walk_mode() == 1 {
            // AETHERPAINTPERF: a transformed, filtered or clip-pathed group lands where its matrix sends it.
            ink_add(INK_ALL);
            draw_node_op(
                node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                damage_rects, clip,
            );
            return;
        }
        let (w, h) = (lb.size.width, lb.size.height);
        let (ox, oy) = placed_origin(layout, node_id, abs_x, abs_y, &lb, width, height, sx, sy);
        let (bx, by) = (ox - sx as f32, oy - sy as f32);
        let snap = surface.to_vec();
        draw_node_op(
            node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
            damage_rects, clip,
        );
        // Filter Effects 1 §5: the filter applies first, then clipping, then the transform.
        if let Some(f) = &filter {
            filter_group(surface, &snap, width, height, f, &fcx, clip, (bx, by, bx + w, by + h), &resolve);
        }
        let Some((x0, y0, x1, y1)) = changed_bounds(surface, &snap, width, height) else {
            return; // the group drew nothing
        };
        if let Some(shape) = &shape {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if !shape.contains(x as f32 + 0.5 - bx, y as f32 + 0.5 - by, w, h) {
                        let i = ((y * width + x) * 4) as usize;
                        surface[i..i + 4].copy_from_slice(&snap[i..i + 4]);
                    }
                }
            }
        }
        let Some(m) = tf else { return };
        use layout_core::paint::{mat_apply, mat_invert, mat_mul};
        let group = surface.to_vec();
        surface.copy_from_slice(&snap);
        // the box-local matrix in screen space: T(b) · m · T(-b)
        let screen = mat_mul(mat_mul([1.0, 0.0, 0.0, 1.0, bx, by], m), [1.0, 0.0, 0.0, 1.0, -bx, -by]);
        let Some(inv) = mat_invert(screen) else { return }; // singular (scale(0)): nothing paints
        let (fx0, fy0, fx1, fy1) = (x0 as f32, y0 as f32, (x1 + 1) as f32, (y1 + 1) as f32);
        let mut dmin = (f32::MAX, f32::MAX);
        let mut dmax = (f32::MIN, f32::MIN);
        for (cx, cy) in [(fx0, fy0), (fx1, fy0), (fx0, fy1), (fx1, fy1)] {
            let (dx, dy) = mat_apply(screen, cx, cy);
            dmin = (dmin.0.min(dx), dmin.1.min(dy));
            dmax = (dmax.0.max(dx), dmax.1.max(dy));
        }
        let ax = dmin.0.max(clip.0).max(0.0).floor() as u32;
        let ay = dmin.1.max(clip.1).max(0.0).floor() as u32;
        let ex = (dmax.0.min(clip.2).max(0.0).ceil() as u32).min(width);
        let ey = (dmax.1.min(clip.3).max(0.0).ceil() as u32).min(height);
        for y in ay..ey {
            for x in ax..ex {
                let (sxp, syp) = mat_apply(inv, x as f32 + 0.5, y as f32 + 0.5);
                if sxp < fx0 || syp < fy0 || sxp >= fx1 || syp >= fy1 {
                    continue;
                }
                let si = ((syp as u32 * width + sxp as u32) * 4) as usize;
                if group[si..si + 3] != snap[si..si + 3] {
                    let di = ((y * width + x) * 4) as usize;
                    surface[di..di + 3].copy_from_slice(&group[si..si + 3]);
                }
            }
        }
    }

    /// `opacity` < 1 as a composited group (see `render_frame`'s note above), else the box as is.
    #[allow(clippy::too_many_arguments)]
    fn draw_node_op(
        node_id: NodeId,
        abs_x: f32,
        abs_y: f32,
        inherited: Inherited,
        layout: &LayoutTree,
        font: &Option<&'static Font>,
        font_bold: &Option<&'static Font>,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
        damage_rects: &[(u32, u32, u32, u32)],
        clip: Clip,
    ) {
        let op = layout.paint_map.get(&node_id).and_then(|p| p.opacity).unwrap_or(1.0);
        if op > 0.0 && op < 1.0 && walk_mode() != 1 {
            let snap = surface.to_vec();
            draw_node_inner(
                node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                damage_rects, clip,
            );
            let a = (op * 256.0).round() as u32;
            for (o, b) in surface.chunks_exact_mut(4).zip(snap.chunks_exact(4)) {
                if o[..3] != b[..3] {
                    for i in 0..3 {
                        o[i] = ((b[i] as u32 * (256 - a) + o[i] as u32 * a) >> 8) as u8;
                    }
                }
            }
        } else {
            draw_node_inner(
                node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                damage_rects, clip,
            );
        }
    }

    /// One layer of a stacking context (CSS 2.2 Appendix E): a positioned or
    /// stacking-context-forming descendant, hoisted out of normal-flow
    /// painting with what `draw_node` needs to paint it later.
    #[derive(Clone, Copy)]
    struct Layer {
        z: i32,
        order: usize,
        node: NodeId,
        abs_x: f32,
        abs_y: f32,
        inh: Inherited,
        clip: Clip,
    }

    /// Collects the layers of the stacking context rooted at `node` (whose
    /// border box sits at `cur`): every positioned / z-indexed descendant
    /// reached through normal-flow boxes and through positioned z-index:auto
    /// boxes (whose positioned descendants belong to THIS context, §9.9.1),
    /// but not inside a nested stacking context (it paints its own).
    #[allow(clippy::too_many_arguments)]
    fn collect_layers(
        layout: &LayoutTree,
        node: NodeId,
        cur: (f32, f32),
        inh: Inherited,
        clip: Clip,
        sx: i32,
        sy: i32,
        out: &mut Vec<Layer>,
    ) {
        let Ok(lb) = layout.taffy.layout(node) else { return };
        // The children with the parent origin draw_node expects for each.
        let mut kids: Vec<(NodeId, f32, f32, Inherited)> = Vec::new();
        // AETHERSTYLE3 (SR103): a scroll container's content moves by its offset.
        let (shx, shy) = layout_core::scroll::content_shift(layout, node);
        let cur = (cur.0 + shx, cur.1 + shy);
        if let Some(il) = layout.inline.get(&node) {
            let cx = cur.0 + lb.border.left + lb.padding.left;
            let cy = cur.1 + lb.border.top + lb.padding.top;
            for f in &il.frags {
                if let crate::layout::inline::Frag::Atomic { node: a, x, y } = f {
                    let loc = layout.taffy.layout(*a).map(|l| l.location).unwrap_or(taffy::Point { x: 0.0, y: 0.0 });
                    kids.push((*a, cx + x - loc.x, cy + y - loc.y, inline_inherited(layout, node, *a, inh)));
                }
            }
            for c in layout.taffy.children(node).unwrap_or_default() {
                if matches!(layout.paint_map.get(&c).and_then(|p| p.position_kind), Some(2 | 3)) {
                    kids.push((c, cur.0, cur.1, inh));
                }
            }
        } else {
            for c in layout.taffy.children(node).unwrap_or_default() {
                let (mx, my) = layout_core::col_shift(layout, c);
                kids.push((c, cur.0 + mx, cur.1 + my, inh));
            }
        }
        for (c, ax, ay, ci) in kids {
            let p = layout.paint_map.get(&c);
            if p.and_then(|p| p.hidden).unwrap_or(false)
                || layout.taffy.style(c).map(|s| s.display == taffy::style::Display::None).unwrap_or(false)
            {
                continue;
            }
            let Ok(cl) = layout.taffy.layout(c) else { continue };
            let ccur = (ax + cl.location.x, ay + cl.location.y);
            let mut cinh = ci;
            if let Some(n) = layout.node_map.get(&c) {
                if let Some(el) = n.as_element() {
                    let spec = p.cloned().unwrap_or_default();
                    inherit_element(&mut cinh, el.name.local.as_ref(), &spec, n, c);
                }
            }
            let cclip = if p.and_then(|p| p.clip).unwrap_or(false) {
                let (bx0, by0) = (ccur.0 - sx as f32, ccur.1 - sy as f32);
                (clip.0.max(bx0), clip.1.max(by0), clip.2.min(bx0 + cl.size.width), clip.3.min(by0 + cl.size.height))
            } else {
                clip
            };
            if is_layered(layout, c) {
                out.push(Layer { z: z_of(layout, c), order: out.len(), node: c, abs_x: ax, abs_y: ay, inh: ci, clip });
                if is_stacking_context(layout, c) {
                    continue;
                }
            }
            collect_layers(layout, c, ccur, cinh, cclip, sx, sy, out);
        }
    }

    /// The inherited state at `child` (an atomic inline of the IFC rooted at
    /// `root`), applying the inline boxes between them.
    fn inline_inherited(layout: &LayoutTree, root: NodeId, child: NodeId, root_inh: Inherited) -> Inherited {
        let mut path = Vec::new();
        let mut n = layout.taffy.parent(child);
        while let Some(p) = n {
            if p == root {
                break;
            }
            path.push(p);
            n = layout.taffy.parent(p);
        }
        let mut inh = root_inh;
        for p in path.into_iter().rev() {
            if let Some(dn) = layout.node_map.get(&p) {
                if let Some(el) = dn.as_element() {
                    let spec = layout.paint_map.get(&p).cloned().unwrap_or_default();
                    inherit_element(&mut inh, el.name.local.as_ref(), &spec, dn, p);
                }
            }
        }
        inh.shift_y = 0.0;
        inh
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_node_inner(
        node_id: NodeId,
        abs_x: f32,
        abs_y: f32,
        inherited: Inherited,
        layout: &LayoutTree,
        font: &Option<&'static Font>,
        font_bold: &Option<&'static Font>,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
        damage_rects: &[(u32, u32, u32, u32)],
        clip: Clip,
    ) {
        // display:none subtrees exist in the tree with zero-size boxes;
        // they must not paint (their text would smear at the parent origin).
        if layout
            .taffy
            .style(node_id)
            .map(|s| s.display == taffy::style::Display::None)
            .unwrap_or(false)
        {
            return;
        }
        // AETHERTOP (SR146): a top-layer element paints in the top-layer pass, not where its parent would.
        if hoisted_to_top_layer(node_id) {
            return;
        }
        // visibility:hidden / opacity:0 keep their space but paint nothing;
        // approximation: the whole subtree skips (no visibility:visible
        // re-reveal inside a hidden ancestor).
        if layout
            .paint_map
            .get(&node_id)
            .and_then(|p| p.hidden)
            .unwrap_or(false)
        {
            return;
        }
        let Ok(layout_box) = layout.taffy.layout(node_id) else { return };
        let (current_x, current_y) = placed_origin(layout, node_id, abs_x, abs_y, layout_box, width, height, sx, sy);

        let mut inherited = inherited;

        if let Some(dom_node) = layout.node_map.get(&node_id) {
            let spec = layout.paint_map.get(&node_id).cloned().unwrap_or_default();

            if let Some(el) = dom_node.as_element() {
                let tag = el.name.local.as_ref();
                inherit_element(&mut inherited, tag, &spec, dom_node, node_id);
                // AETHERPAINTPERF (SR130): the index walk records the box's own ink and paints nothing.
                'paint: {
                if walk_mode() == 1 {
                    ink_element(layout, node_id, tag, &spec, layout_box, current_x - sx as f32, current_y - sy as f32, &inherited);
                    break 'paint;
                }

                let bw = layout_box.size.width.max(0.0);
                let bh = layout_box.size.height.max(0.0);
                // Box-local origin in screen space (may be negative when
                // scrolled past; the painted span is clamped, the local
                // coordinate is not — that is what keeps a positioned
                // background aligned while scrolling).
                let box_sx = current_x - sx as f32;
                let box_sy = current_y - sy as f32;
                // AETHERRENDER3 (SR126): a box layout_core split across columns (`LayoutTree::col_frags`, css-break-3 §5.4)
                // paints its shadows, backgrounds and borders once per fragment ([`frag_pass`]); any other box is one pass at
                // its own place. The span each background loop fills is the CLAMPED screen span of the pass's box.
                let deco_passes: Vec<(f32, f32, f32, Clip)> = match layout.col_frags.get(&node_id).filter(|f| !f.is_empty()) {
                    Some(frags) => frags.iter().map(|f| frag_pass(f, box_sx, box_sy, bh, clip)).collect(),
                    None => vec![(box_sx, box_sy, bh, clip)],
                };

                // mask-image is an alpha stencil over this box's background
                // paint: the fill only lands where the mask is opaque. A
                // declared-but-unresolvable mask suppresses the fill —
                // painting the raw box would show a solid blob where the
                // page means an icon glyph.
                let mask = spec.mask_image.as_ref().map(|url| {
                    crate::images::get(url).map(|img| {
                        let g = resolve_bg_geometry(
                            spec.mask_size.as_deref(),
                            spec.mask_position.as_deref(),
                            spec.mask_repeat,
                            bw,
                            bh,
                            img.width() as f32,
                            img.height() as f32,
                        );
                        (img, g)
                    })
                });
                let mask_alpha = |x: u32, y: u32, ox: f32, oy: f32| -> u8 {
                    match &mask {
                        None => 255,
                        Some(None) => 0,
                        Some(Some((img, g))) => {
                            let (lx, ly) = (x as f32 - ox, y as f32 - oy);
                            match g.sample(lx, ly, img.width(), img.height()) {
                                Some((u, v)) => img.get_pixel(u, v).0[3],
                                None => 0,
                            }
                        }
                    }
                };
                if matches!(mask, Some(None)) {
                    crate::ledger::record_css("mask-image-unresolved");
                }

                // Rounded corners or a patterned border: the shape painter
                // (render/boxpaint.rs) owns the background and the border.
                let fancy = spec.radius.is_some_and(|r| r.iter().any(|&v| v != 0.0))
                    || spec.border_style.is_some_and(|s| s.iter().any(|&v| v != 0));
                // AETHERRENDER3 (SR126): shadows and backgrounds per column fragment (one pass for an unfragmented box).
                for &(box_sx, box_sy, bh, clip) in &deco_passes {
                    let x_start = (box_sx.max(0.0)) as u32;
                    let y_start = (box_sy.max(0.0)) as u32;
                    let end_x = ((box_sx + bw).max(0.0) as u32).min(width);
                    let end_y = ((box_sy + bh).max(0.0) as u32).min(height);
                    let mut blend_at = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                        if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                            return;
                        }
                        if a >= 0.999 {
                            put_px(surface, width, x, y, c);
                        } else if a > 0.0 {
                            blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                        }
                    };
                    // Outer box shadows paint under the box, outside it.
                    if let Some(sh) = spec.shadows.as_ref().filter(|s| !s.is_empty()) {
                        let radii = boxpaint::used_radii(spec.radius.unwrap_or([0.0; 4]), bw, bh);
                        effects::paint_shadows(sh, box_sx, box_sy, bw, bh, radii, width, height, &boxpaint::sdf, &mut blend_at);
                    }
                    // css-color-4: a translucent background composites source-over.
                    let bga = spec.bg_alpha.unwrap_or(1.0).clamp(0.0, 1.0);
                    let background = spec.background.filter(|_| bga > 0.0);
                    if fancy && mask.is_none() && background.is_some() {
                        let mut blend_a = |x: u32, y: u32, c: (u8, u8, u8), a: f32| blend_at(x, y, c, a * bga);
                        boxpaint::paint(
                            box_sx, box_sy, bw, bh, spec.radius.unwrap_or([0.0; 4]),
                            [None; 4], background, width, height, &mut blend_a,
                        );
                    } else if let Some(bg) = background {
                        let ba = (bga * 255.0).round() as u32;
                        for y in y_start..end_y {
                            for x in x_start..end_x {
                                if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                    continue;
                                }
                                match mask_alpha(x, y, box_sx, box_sy) as u32 * ba / 255 {
                                    0 => {}
                                    255 => put_px(surface, width, x, y, bg),
                                    a => blend_px(surface, width, x, y, bg, a as u8),
                                }
                            }
                        }
                    }

                    if let Some(Some(g)) = spec.bg_gradient.as_ref() {
                        let radii = boxpaint::used_radii(spec.radius.unwrap_or([0.0; 4]), bw, bh);
                        let (gx, gy) = (box_sx, box_sy);
                        let cover = move |fx: f32, fy: f32| (0.5 - boxpaint::sdf(fx, fy, gx, gy, gx + bw, gy + bh, radii)).clamp(0.0, 1.0);
                        let mut blend_g = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                return;
                            }
                            if a >= 0.999 {
                                put_px(surface, width, x, y, c);
                            } else if a > 0.0 {
                                blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                            }
                        };
                        effects::paint_gradient(g, box_sx, box_sy, bw, bh, width, height, &cover, &mut blend_g);
                    }
                    // background-image paints over the color, under content,
                    // honouring background-size / -position / -repeat (the
                    // sprite-sheet idiom is exactly a positioned no-repeat
                    // layer of an intrinsically-sized sheet).
                    if let Some(img) = spec.bg_image.as_deref().and_then(crate::images::get) {
                        let g = resolve_bg_geometry(
                            spec.bg_size.as_deref(),
                            spec.bg_position.as_deref(),
                            spec.bg_repeat,
                            bw,
                            bh,
                            img.width() as f32,
                            img.height() as f32,
                        );
                        for y in y_start..end_y {
                            for x in x_start..end_x {
                                if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                    continue;
                                }
                                let Some((u, v)) =
                                    g.sample(x as f32 - box_sx, y as f32 - box_sy, img.width(), img.height())
                                else {
                                    continue;
                                };
                                let [r, gr, b, a] = img.get_pixel(u, v).0;
                                let a = (a as u32 * mask_alpha(x, y, box_sx, box_sy) as u32 / 255) as u8;
                                if a > 0 {
                                    blend_px(surface, width, x, y, (r, gr, b), a);
                                }
                            }
                        }
                    }

                    // Inset box shadows (§7.1.3): over the background, under the
                    // border and content, inside the padding box.
                    if let Some(sh) = spec.shadows.as_ref().filter(|s| s.iter().any(|s| s.inset)) {
                        let (bl, bt) = (layout_box.border.left, layout_box.border.top);
                        let (br, bb) = (layout_box.border.right, layout_box.border.bottom);
                        let outer = boxpaint::used_radii(spec.radius.unwrap_or([0.0; 4]), bw, bh);
                        let inner = [
                            (outer[0] - bl.max(bt)).max(0.0),
                            (outer[1] - br.max(bt)).max(0.0),
                            (outer[2] - br.max(bb)).max(0.0),
                            (outer[3] - bl.max(bb)).max(0.0),
                        ];
                        let mut blend_i = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                return;
                            }
                            if a >= 0.999 {
                                put_px(surface, width, x, y, c);
                            } else if a > 0.0 {
                                blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                            }
                        };
                        effects::paint_inset_shadows(
                            sh, box_sx + bl, box_sy + bt, (bw - bl - br).max(0.0), (bh - bt - bb).max(0.0), inner,
                            width, height, &boxpaint::sdf, &mut blend_i,
                        );
                    }
                }

                if tag == "img" {
                    let src = crate::images::img_source(dom_node, crate::images::SELECTION_VIEWPORT); // AETHERMEDIA SR107: <picture>
                    if let Some(img) = src.as_deref().and_then(crate::images::get) {
                        // The painted SPAN clamps to the viewport; the box
                        // ORIGIN must not. Clamping the origin re-anchors a
                        // scrolled-off image to the viewport edge, so it
                        // stops moving with the page — and an incremental
                        // scroll (shift-and-repaint-the-strip) then leaves a
                        // train of copies down the viewport.
                        let bw = layout_box.size.width.max(1.0);
                        let bh = layout_box.size.height.max(1.0);
                        let x_start = box_sx.max(0.0) as u32;
                        let y_start = box_sy.max(0.0) as u32;
                        let end_x = ((box_sx + bw).max(0.0) as u32).min(width);
                        let end_y = ((box_sy + bh).max(0.0) as u32).min(height);
                        for y in y_start..end_y {
                            for x in x_start..end_x {
                                if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                    continue;
                                }
                                // Nearest-neighbor sample into the layout box.
                                let u = ((x as f32 - box_sx) / bw * img.width() as f32).max(0.0) as u32;
                                let v = ((y as f32 - box_sy) / bh * img.height() as f32).max(0.0) as u32;
                                let px = img.get_pixel(u.min(img.width() - 1), v.min(img.height() - 1));
                                let [r, g, b, a] = px.0;
                                if a > 0 {
                                    blend_px(surface, width, x, y, (r, g, b), a);
                                }
                            }
                        }
                    }
                }

                if tag == "canvas" {
                    // AETHERCANVAS (SR85): the 2D context's bitmap is the element's replaced content, scaled
                    // into the content box (nearest neighbour, like <img>), straight alpha over the backdrop.
                    if let Some((cw, ch, rgba)) = crate::canvas::element_bitmap(dom_node) {
                        let (pad, bord) = (layout_box.padding, layout_box.border);
                        let cx = box_sx + pad.left + bord.left;
                        let cy = box_sy + pad.top + bord.top;
                        let cbw = (bw - pad.left - bord.left - pad.right - bord.right).max(1.0);
                        let cbh = (bh - pad.top - bord.top - pad.bottom - bord.bottom).max(1.0);
                        let x_start = cx.max(0.0) as u32;
                        let y_start = cy.max(0.0) as u32;
                        let end_x = ((cx + cbw).max(0.0) as u32).min(width);
                        let end_y = ((cy + cbh).max(0.0) as u32).min(height);
                        for y in y_start..end_y {
                            for x in x_start..end_x {
                                if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                    continue;
                                }
                                let u = (((x as f32 + 0.5 - cx) / cbw * cw as f32).max(0.0) as usize).min(cw - 1);
                                let v = (((y as f32 + 0.5 - cy) / cbh * ch as f32).max(0.0) as usize).min(ch - 1);
                                let i = (v * cw + u) * 4;
                                let a = rgba[i + 3];
                                if a > 0 {
                                    blend_px(surface, width, x, y, (rgba[i], rgba[i + 1], rgba[i + 2]), a);
                                }
                            }
                        }
                    }
                }

                if tag == "video" || tag == "audio" {
                    if let Some(media) = crate::media::paint_for(dom_node) {
                        let (pad, bord) = (layout_box.padding, layout_box.border);
                        let content = (
                            box_sx + pad.left + bord.left,
                            box_sy + pad.top + bord.top,
                            (bw - pad.left - bord.left - pad.right - bord.right).max(0.0),
                            (bh - pad.top - bord.top - pad.bottom - bord.bottom).max(0.0),
                        );
                        paint_media(&media, &spec, content, font, surface, width, height, damage_rects, clip);
                    }
                }

                // AETHERFRAMES (SR83): an `<iframe>` is a replaced box whose content is its nested
                // document's painted surface (`frames::child_surface`, painted by the engine before this
                // frame), composed 1:1 into the content box from its top-left, clipped to it.
                if tag == "iframe" {
                    if let Some(cs) = crate::frames::child_surface(dom_node) {
                        let (pad, bord) = (layout_box.padding, layout_box.border);
                        let cx = box_sx + pad.left + bord.left;
                        let cy = box_sy + pad.top + bord.top;
                        let cw = (bw - pad.left - bord.left - pad.right - bord.right).max(0.0).min(cs.width as f32);
                        let ch = (bh - pad.top - bord.top - pad.bottom - bord.bottom).max(0.0).min(cs.height as f32);
                        let x0 = cx.max(0.0) as u32;
                        let y0 = cy.max(0.0) as u32;
                        let x1 = ((cx + cw).max(0.0) as u32).min(width);
                        let y1 = ((cy + ch).max(0.0) as u32).min(height);
                        for y in y0..y1 {
                            for x in x0..x1 {
                                if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                    continue;
                                }
                                let u = (x as f32 - cx).max(0.0) as u32;
                                let v = (y as f32 - cy).max(0.0) as u32;
                                if u >= cs.width || v >= cs.height {
                                    continue;
                                }
                                let i = ((v * cs.width + u) * 4) as usize;
                                if let Some(p) = cs.pixels.get(i..i + 4) {
                                    put_px(surface, width, x, y, (p[2], p[1], p[0]));
                                }
                            }
                        }
                    }
                }

                // Form controls get a UA border and their value text.
                let is_control = matches!(tag, "input" | "textarea" | "select" | "button");
                let kind = crate::layout::control_kind(dom_node);
                let checkable = matches!(kind, Some(crate::layout::Control::Checkbox | crate::layout::Control::Radio));
                // Chromium's control theme paints a 1px #767676 frame at the
                // outer edge of the (2px) UA border; checkboxes and radios
                // are drawn whole below.
                let mut border = spec.border.or(if is_control && !checkable {
                    Some([Some((1.0, (118, 118, 118))); 4])
                } else {
                    None
                });
                // Apply border-*-width overrides if present
                if let Some(sides) = &mut border {
                    if let Some(widths) = &spec.border_width {
                        for (i, width) in widths.iter().enumerate() {
                            if let Some(w) = width {
                                if let Some((_, c)) = sides[i] {
                                    sides[i] = Some((*w, c));
                                }
                            }
                        }
                    }
                }
                // Content box in screen space — the label/value area, inside
                // the UA border and any author padding.
                let (pad, bord) = (layout_box.padding, layout_box.border);
                let inset_l = pad.left + bord.left;
                let inset_r = pad.right + bord.right;
                let content_x = box_sx + inset_l;
                let content_y = box_sy + pad.top + bord.top;
                let content_w = (bw - inset_l - inset_r).max(0.0);
                let content_h = (bh - pad.top - bord.top - pad.bottom - bord.bottom).max(0.0);
                // A push button's label centres on both axes; a text field's
                // value stays left-aligned but rides the vertical middle,
                // which is what makes native fields look "in" their box.
                let is_push_button = tag == "button"
                    || (tag == "input"
                        && el
                            .attributes
                            .borrow()
                            .get("type")
                            .map(|t| {
                                let t = t.to_ascii_lowercase();
                                matches!(t.as_str(), "submit" | "button" | "reset")
                            })
                            .unwrap_or(false));
                if is_push_button {
                    inherited.center_box = Some((content_x, content_y, content_w, content_h));
                }

                if checkable {
                    // Checkedness, not the attribute: a click (or script) toggles the dirty
                    // checkedness and the `checked` attribute stays the default (HTML §4.10.5.1.15).
                    let checked = crate::forms::checkedness(dom_node);
                    let radio = kind == Some(crate::layout::Control::Radio);
                    paint_checkable(box_sx, box_sy, bw, bh, radio, checked, surface, width, height, damage_rects, clip);
                }
                if kind == Some(crate::layout::Control::Select) {
                    // The drop-down arrow: Chromium's menulist chevron, pixel for pixel (its ink
                    // coverage as rendered at 1x), its last column 4px inside the right border and
                    // its middle row on the box's middle row.
                    const ARROW: [[u8; 10]; 7] = [
                        [255, 141, 231, 255, 255, 255, 255, 231, 143, 255],
                        [150, 0, 36, 230, 255, 255, 230, 36, 0, 154],
                        [231, 32, 0, 36, 231, 231, 36, 0, 36, 231],
                        [255, 227, 35, 0, 33, 33, 0, 35, 230, 255],
                        [255, 255, 226, 33, 0, 0, 35, 227, 255, 255],
                        [255, 255, 255, 227, 28, 32, 227, 255, 255, 255],
                        [255, 255, 255, 255, 222, 226, 255, 255, 255, 255],
                    ];
                    let right = (box_sx + bw).round() - 1.0;
                    let x0 = right - 13.0;
                    let y0 = (box_sy.round() + ((bh.round() - 1.0) / 2.0).floor()) - 3.0;
                    let ink = spec.color.unwrap_or((0, 0, 0));
                    for (r, row) in ARROW.iter().enumerate() {
                        for (c, &v) in row.iter().enumerate() {
                            let (x, y) = (x0 + c as f32, y0 + r as f32);
                            if v == 255 || x < 0.0 || y < 0.0 || x as u32 >= width || y as u32 >= height {
                                continue;
                            }
                            let (xu, yu) = (x as u32, y as u32);
                            if in_damage(xu, yu, damage_rects) && in_clip(xu, yu, clip) {
                                blend_px(surface, width, xu, yu, ink, 255 - v);
                            }
                        }
                    }
                }
                if tag == "textarea" {
                    // The value (the dirty value the user typed, else the element's text), pre-wrap,
                    // from the content box's top-left, in the monospace control font.
                    let text = crate::forms::control_value(dom_node);
                    let mode = crate::fonts::lines::TextMode { white_space: 3, ..Default::default() };
                    let adv = inherited.advancer();
                    let color = spec.color.unwrap_or((0, 0, 0));
                    let focus = focus_of(dom_node);
                    let lh = crate::fonts::face(&adv.sel).map(|f| crate::fonts::line_height(f, adv.size, 0.0)).unwrap_or(adv.size * 1.2);
                    let max_w = content_w.max(1.0);
                    if let Some(f) = focus.as_ref().filter(|f| f.caret != f.anchor) {
                        // The selection: one band per visual line it covers.
                        let (a, b) = (f.caret.min(f.anchor), f.caret.max(f.anchor));
                        let (la, xa) = textarea_caret(&text, a, &adv, max_w, &mode);
                        let (lb, xb) = textarea_caret(&text, b, &adv, max_w, &mode);
                        for l in la..=lb {
                            let x0 = if l == la { xa } else { 0.0 };
                            let x1 = if l == lb { xb } else { content_w };
                            let y0 = content_y + l as f32 * lh;
                            fill_rect(content_x + x0, y0, content_x + x1, y0 + lh, SELECTION_BG, SELECTION_ALPHA, surface, width, height, damage_rects, clip);
                        }
                    }
                    draw_lines(
                        &text, content_x, content_y, max_w, &adv,
                        0.0, color, &mode, Deco::default(), &[],
                        surface, width, height, damage_rects, clip,
                    );
                    if let Some(f) = focus.filter(|f| f.caret_visible) {
                        let (l, x) = textarea_caret(&text, f.caret, &adv, max_w, &mode);
                        let cx = (content_x + x).floor().min(content_x + content_w - 1.0);
                        let cy = content_y + l as f32 * lh;
                        fill_rect(cx, cy, cx + 1.0, cy + lh, color, 255, surface, width, height, damage_rects, clip);
                    }
                }
                if is_control && tag != "button" && !checkable && tag != "textarea" {
                    let face = crate::fonts::face(&inherited.sel());
                    if let Some(font) = face.or(*font) {
                        let adv = inherited.advancer();
                        // A value script (or the user) set is the control's value (the dirty value
                        // flag, HTML §4.10.5.4); the attribute is only its default.
                        let dirty = crate::js::control_value(dom_node);
                        let select_label = (tag == "select")
                            .then(|| crate::layout::select_selected_option(dom_node).map(|(_, l)| l))
                            .flatten();
                        let attrs = el.attributes.borrow();
                        // A <select> paints the SELECTED OPTION'S TEXT, not
                        // its submit value: layout publishes the visible label
                        // as data-aether-label, so "English" shows where the
                        // value attribute would only say "en".
                        let shown = select_label
                            .as_deref()
                            .filter(|v| !v.is_empty())
                            .or_else(|| dirty.as_deref())
                            .or_else(|| attrs.get("value"))
                            .filter(|v| !v.is_empty());
                        let is_placeholder = shown.is_none();
                        let password = attrs.get("type").is_some_and(|t| t.trim().eq_ignore_ascii_case("password"));
                        let mut value = shown.or_else(|| attrs.get("placeholder")).unwrap_or("").to_string();
                        if password && !is_placeholder {
                            value = "\u{2022}".repeat(value.chars().count());
                        }
                        drop(attrs);
                        // Field text is black (FieldText), a placeholder
                        // #757575 (html.css ::placeholder darkGray).
                        let value_color = if is_placeholder { (117, 117, 117) } else { spec.color.unwrap_or((0, 0, 0)) };
                        let fs = inherited.font_size;
                        let is_field = kind == Some(crate::layout::Control::Field);
                        let focus = if is_field { focus_of(dom_node) } else { None };
                        if !value.is_empty() || focus.is_some() {
                            let text_w = measure_text_width(&value, &adv);
                            // <input>/<select> are single-line by definition —
                            // their value rides the vertical middle however
                            // tall the author makes the box, which is what
                            // native fields do. A <textarea> is a multi-line
                            // edit surface: its text starts at the top.
                            let single_line = tag != "textarea" && content_h > 0.0;
                            let lh = crate::fonts::line_height(font, fs, 0.0);
                            let (tx, ty, max_w) = if is_push_button && text_w <= content_w {
                                (
                                    content_x + (content_w - text_w) / 2.0,
                                    centered_line_origin_y(font, fs, content_y + content_h / 2.0),
                                    content_w.max(1.0),
                                )
                            } else {
                                // The value's line box is centred in the
                                // content box (a single-line field).
                                let ty = if single_line {
                                    content_y + ((content_h - lh) / 2.0).floor()
                                } else {
                                    content_y
                                };
                                let left = if tag == "select" { content_x + 4.0 } else { content_x };
                                (left, ty, f32::MAX)
                            };
                            if is_field {
                                // A text field's value keeps its spaces (white-space: pre, as the
                                // inner editor's): the caret measures on exactly this run. A password
                                // paints one bullet per character, so its caret counts characters.
                                let real = if is_placeholder { String::new() } else { crate::forms::control_value(dom_node) };
                                let x_at = |i: usize| -> f32 {
                                    let i = i.min(real.len());
                                    let prefix = if password { "\u{2022}".repeat(real[..i].chars().count()) } else { real[..i].to_string() };
                                    tx + adv.str(&prefix)
                                };
                                if let Some(f) = focus.as_ref().filter(|f| f.caret != f.anchor && !is_placeholder) {
                                    let (a, b) = (f.caret.min(f.anchor), f.caret.max(f.anchor));
                                    fill_rect(x_at(a), ty, x_at(b), ty + lh, SELECTION_BG, SELECTION_ALPHA, surface, width, height, damage_rects, clip);
                                }
                                if !value.is_empty() {
                                    let mode = crate::fonts::lines::TextMode { white_space: 2, ..Default::default() };
                                    draw_lines(
                                        &value, tx, ty, f32::MAX, &adv, 0.0, value_color, &mode, Deco::default(), &[],
                                        surface, width, height, damage_rects, clip,
                                    );
                                }
                                if let Some(f) = focus.filter(|f| f.caret_visible) {
                                    let caret = if is_placeholder { 0 } else { f.caret };
                                    let cx = x_at(caret).floor().min(content_x + content_w - 1.0).max(content_x);
                                    let color = spec.color.unwrap_or((0, 0, 0));
                                    fill_rect(cx, ty, cx + 1.0, ty + lh, color, 255, surface, width, height, damage_rects, clip);
                                }
                            } else {
                                draw_text(
                                    &value,
                                    tx,
                                    ty,
                                    max_w,
                                    &adv,
                                    0.0,
                                    value_color,
                                    false,
                                    surface,
                                    width,
                                    height,
                                    damage_rects,
                                    clip,
                                );
                            }
                        }
                    }
                }

                // A focused text field shows Chromium's focus ring (`:focus-visible` always matches a
                // text field): 2px #101010 over the border box's outer edge.
                let focused_field = is_control && focus_of(dom_node).is_some();
                if focused_field {
                    border = Some([Some((2.0, (16, 16, 16))); 4]);
                }
                // Chromium's control frame (fields, selects, textareas) is a rounded rect whose four
                // corner pixels stay unpainted: keep what is under them.
                let ua_frame = is_control && !checkable && !is_push_button && spec.border.is_none() && !focused_field;
                let corners: Vec<(u32, u32, [u8; 4])> = if ua_frame {
                    let (x0, y0) = (box_sx.round(), box_sy.round());
                    let (x1, y1) = ((box_sx + bw).round() - 1.0, (box_sy + bh).round() - 1.0);
                    [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
                        .into_iter()
                        .filter(|&(x, y)| x >= 0.0 && y >= 0.0 && (x as u32) < width && (y as u32) < height)
                        .map(|(x, y)| {
                            let i = ((y as u32 * width + x as u32) * 4) as usize;
                            (x as u32, y as u32, [surface[i], surface[i + 1], surface[i + 2], surface[i + 3]])
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                // AETHERRENDER3 (SR126): the borders per column fragment (one pass for an unfragmented box).
                for &(box_sx, box_sy, bh, clip) in &deco_passes {
                    if let (true, Some(sides)) = (fancy, border) {
                        let styles = spec.border_style.unwrap_or([0; 4]);
                        let mut bs: [boxpaint::Side; 4] = [None; 4];
                        for i in 0..4 {
                            bs[i] = sides[i].filter(|(w, _)| *w > 0.0).map(|(w, c)| (w, c, styles[i]));
                        }
                        let bda = spec.border_alpha.unwrap_or(1.0);
                        let mut blend_at = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                            let a = a * bda;
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                return;
                            }
                            if a >= 0.999 {
                                put_px(surface, width, x, y, c);
                            } else if a > 0.0 {
                                blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                            }
                        };
                        boxpaint::paint(
                            box_sx, box_sy, bw, bh, spec.radius.unwrap_or([0.0; 4]),
                            bs, None, width, height, &mut blend_at,
                        );
                    } else if let Some(sides) = border {
                        // Same rule as the background/image spans: x_start..end_x
                        // is the CLAMPED screen span of the unclamped box origin
                        // (box_sx/box_sy). Re-deriving the end from a clamped
                        // start pins a scrolled-off box's borders to the viewport
                        // edge and repeats them on every scroll strip.
                        // [top, right, bottom, left], each side its own stroke.
                        // Sides are expressed in unclamped screen floats and the
                        // stroke clips them into the viewport, so a box whose
                        // bottom edge lies below the fold does not stamp its
                        // bottom border across the last visible row.
                        let (bx0, by0) = (box_sx, box_sy);
                        let (bx1, by1) = (box_sx + bw, box_sy + bh);
                        let bda = (spec.border_alpha.unwrap_or(1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                        let mut stroke = |x0: f32, y0: f32, x1: f32, y1: f32, color: (u8, u8, u8)| {
                            let x0 = x0.max(0.0) as u32;
                            let y0 = y0.max(0.0) as u32;
                            let x1 = (x1.max(0.0) as u32).min(width);
                            let y1 = (y1.max(0.0) as u32).min(height);
                            for y in y0..y1 {
                                for x in x0..x1 {
                                    if in_damage(x, y, damage_rects) && in_clip(x, y, clip) {
                                        if bda == 255 {
                                            put_px(surface, width, x, y, color);
                                        } else if bda > 0 {
                                            blend_px(surface, width, x, y, color, bda);
                                        }
                                    }
                                }
                            }
                        };
                        // A zero-width side draws NOTHING. `border-width: 0`
                        // with a style and colour still set is the single most
                        // common declaration on the web (every Tailwind/reset
                        // preflight opens with it); rounding it up to a hairline
                        // outlines every box on the page.
                        let px = |w: f32| if w > 0.0 { w.max(1.0) } else { 0.0 };
                        if let Some((w, c)) = sides[0].filter(|(w, _)| *w > 0.0) {
                            stroke(bx0, by0, bx1, by0 + px(w), c);
                        }
                        if let Some((w, c)) = sides[2].filter(|(w, _)| *w > 0.0) {
                            stroke(bx0, by1 - px(w), bx1, by1, c);
                        }
                        if let Some((w, c)) = sides[3].filter(|(w, _)| *w > 0.0) {
                            stroke(bx0, by0, bx0 + px(w), by1, c);
                        }
                        if let Some((w, c)) = sides[1].filter(|(w, _)| *w > 0.0) {
                            stroke(bx1 - px(w), by0, bx1, by1, c);
                        }
                    }
                }
                for (x, y, p) in corners {
                    if in_damage(x, y, damage_rects) && in_clip(x, y, clip) {
                        let i = ((y * width + x) * 4) as usize;
                        surface[i..i + 4].copy_from_slice(&p);
                    }
                }
                if tag == "textarea" && ua_frame {
                    // The resize grip: two #666 diagonals in the bottom-right corner of the padding box.
                    let (ix, iy) = ((box_sx + bw).round() - 2.0, (box_sy + bh).round() - 2.0);
                    let mut dot = |x: f32, y: f32| {
                        if x >= 0.0 && y >= 0.0 && (x as u32) < width && (y as u32) < height {
                            let (xu, yu) = (x as u32, y as u32);
                            if in_damage(xu, yu, damage_rects) && in_clip(xu, yu, clip) {
                                put_px(surface, width, xu, yu, (102, 102, 102));
                            }
                        }
                    };
                    for k in 0..7 {
                        dot(ix - 1.0 - k as f32, iy - 7.0 + k as f32);
                    }
                    for k in 0..3 {
                        dot(ix - 1.0 - k as f32, iy - 3.0 + k as f32);
                    }
                }
                // css-lists-3 §3.1: an outside ::marker for a list item,
                // its first line's baseline, ending at the content edge.
                let is_item = spec.list_item.unwrap_or(tag == "li");
                // AETHERGEN (SR71): an author-styled ::marker — `content` text replaces the
                // list-style marker (even under `list-style-type: none`), `content: none` removes
                // it, and its colour and font apply to it.
                if let Some(mk) = spec.marker.as_deref().filter(|_| is_item && !inherited.text_hidden) {
                    let mut sel = inherited.sel();
                    if let Some(w) = mk.weight { sel.weight = w.resolve(inherited.weight); }
                    if let Some(i) = mk.italic { sel.italic = i; }
                    if let Some(f) = mk.family { sel.family = f; }
                    let fs = mk.size(inherited.font_size);
                    let color = mk.color.unwrap_or(inherited.color);
                    let draw = !mk.none && (mk.text.is_some() || inherited.list_style != 0);
                    if let (true, Some(item_face)) = (draw, crate::fonts::face(&inherited.sel())) {
                        let base = match layout.inline.get(&node_id).and_then(|l| l.first_baseline) {
                            Some(b) => content_y + b,
                            None => content_y + crate::fonts::baseline_offset(item_face, inherited.font_size, inherited.line_height),
                        };
                        let adv = Advancer::new(sel, fs, 0.0).with_dir(inherited.rtl);
                        match &mk.text {
                            Some(t) => {
                                let w = adv.str(t);
                                draw_shaped(t, &adv, content_x - w, base.round(), color, surface, width, height, damage_rects, clip);
                            }
                            None => paint_marker(
                                dom_node, inherited.list_style, content_x, base.round(), &adv,
                                color, surface, width, height, damage_rects, clip,
                            ),
                        }
                    }
                } else if is_item && inherited.list_style != 0 && !inherited.text_hidden {
                    if let Some(f) = crate::fonts::face(&inherited.sel()) {
                        let fs = inherited.font_size;
                        let base = match layout.inline.get(&node_id).and_then(|l| l.first_baseline) {
                            Some(b) => content_y + b,
                            None => content_y + crate::fonts::baseline_offset(f, fs, inherited.line_height),
                        };
                        // ::marker text is unaffected by the item's letter/word spacing
                        let adv = Advancer::new(inherited.sel(), fs, 0.0).with_dir(inherited.rtl);
                        paint_marker(
                            dom_node, inherited.list_style, content_x, base.round(), &adv,
                            inherited.color, surface, width, height, damage_rects, clip,
                        );
                    }
                }
                }
            } else if dom_node.as_text().is_some() && !inherited.text_hidden && walk_mode() == 1 {
                // AETHERPAINTPERF (SR130): a text run outside an inline formatting context wraps (or does not) at
                // paint time; its lines are not bounded here.
                ink_add(INK_ALL);
            } else if dom_node.as_text().is_some() && !inherited.text_hidden {
                // The same face the measurer wrapped this run with
                // (fonts::face): family x weight x style. The preloaded sans
                // pair is only the fallback when no face loads at all.
                let face = crate::fonts::face(&inherited.sel());
                let font = face.or(if inherited.weight >= 600 { *font_bold } else { *font });
                if let Some(font) = font {
                    let text = dom_node.text_contents();
                    if !text.trim().is_empty() || inherited.white_space >= 2 || layout.paint_map.get(&node_id).and_then(|p| p.ws_lead) == Some(true) {
                        let text = transform_text(&text, inherited.text_transform);
                        let spec_t = layout.paint_map.get(&node_id);
                        let mut mode = crate::fonts::lines::TextMode {
                            white_space: inherited.white_space,
                            word_break: inherited.word_break,
                            overflow_wrap: inherited.overflow_wrap,
                            letter_spacing: inherited.letter_spacing,
                            word_spacing: inherited.word_spacing,
                            rtl: inherited.rtl,
                            lead: spec_t.and_then(|p| p.ws_lead).unwrap_or(false),
                            trail: spec_t.and_then(|p| p.ws_trail).unwrap_or(false),
                        };
                        let adv = inherited.advancer();
                        if inherited.nowrap && mode.white_space == 0 {
                            mode.white_space = 1;
                        }
                        // A <button>'s label is an ordinary text child, so it
                        // is centred here against the ancestor control's
                        // content box — but only when the measured run fits on
                        // one line inside it, so a button wrapping rich or
                        // overflowing content keeps normal flow painting.
                        let centered = inherited.center_box.and_then(|(cx, cy, cw, ch)| {
                            let tw = crate::fonts::lines::break_lines(&adv, &text, &mode, f32::MAX)[0].width;
                            (tw <= cw && cw > 0.0).then(|| {
                                (
                                    cx + (cw - tw) / 2.0,
                                    centered_line_origin_y(font, inherited.font_size, cy + ch / 2.0),
                                    cw.max(1.0),
                                )
                            })
                        });
                        // Break at the UNROUNDED box width the measurer saw
                        // (the rounded one can be a fraction narrower).
                        let wrap_w = layout
                            .taffy
                            .unrounded_layout(node_id)
                            .size
                            .width
                            .max(layout_box.size.width)
                            + 0.01;
                        let (tx, ty, max_w) = centered.unwrap_or((
                            current_x - sx as f32,
                            current_y - sy as f32 + inherited.shift_y,
                            if inherited.nowrap { f32::MAX } else { wrap_w.max(1.0) },
                        ));
                        draw_lines(
                            &text,
                            tx,
                            ty,
                            max_w,
                            &adv,
                            inherited.line_height,
                            inherited.color,
                            &mode,
                            Deco { underline: inherited.underline, line_through: inherited.line_through },
                            inherited.shadows(layout),
                            surface,
                            width,
                            height,
                            damage_rects,
                            clip,
                        );
                    }
                }
            }
        }

        // overflow != visible: children clip to this box's screen rect.
        let ifc = layout.inline.get(&node_id);
        let child_clip = if layout
            .paint_map
            .get(&node_id)
            .and_then(|p| p.clip)
            .unwrap_or(false)
        {
            let bx0 = current_x - sx as f32;
            let by0 = current_y - sy as f32;
            (
                clip.0.max(bx0),
                clip.1.max(by0),
                clip.2.min(bx0 + layout_box.size.width.max(0.0)),
                clip.3.min(by0 + layout_box.size.height.max(0.0)),
            )
        } else {
            clip
        };
        // overflow clipping to a ROUNDED box (css-backgrounds-3 §5.3): the
        // children paint against the rectangular clip, then every pixel of
        // the box outside its rounded padding box returns to the backdrop
        // (mixed by coverage at the curve) — exact for the corners.
        let round_clip = layout
            .paint_map
            .get(&node_id)
            .filter(|p| walk_mode() != 1 && p.clip.unwrap_or(false) && p.radius.is_some_and(|r| r.iter().any(|&v| v != 0.0)))
            .map(|p| {
                let (bw, bh) = (layout_box.size.width, layout_box.size.height);
                let outer = boxpaint::used_radii(p.radius.unwrap_or([0.0; 4]), bw, bh);
                let b = layout_box.border;
                let inner = [
                    (outer[0] - b.left.max(b.top)).max(0.0),
                    (outer[1] - b.right.max(b.top)).max(0.0),
                    (outer[2] - b.right.max(b.bottom)).max(0.0),
                    (outer[3] - b.left.max(b.bottom)).max(0.0),
                ];
                let (x0, y0) = (current_x - sx as f32 + b.left, current_y - sy as f32 + b.top);
                ((x0, y0, x0 + bw - b.left - b.right, y0 + bh - b.top - b.bottom), inner, surface.to_vec())
            });
        let finish_round_clip = |surface: &mut [u8]| {
            if let Some(((x0, y0, x1, y1), r, snap)) = &round_clip {
                let (ax, ay) = (x0.floor().max(0.0) as u32, y0.floor().max(0.0) as u32);
                let (bx, by) = ((x1.ceil().max(0.0) as u32).min(width), (y1.ceil().max(0.0) as u32).min(height));
                for y in ay..by {
                    for x in ax..bx {
                        let cov = (0.5 - boxpaint::sdf(x as f32 + 0.5, y as f32 + 0.5, *x0, *y0, *x1, *y1, *r)).clamp(0.0, 1.0);
                        if cov >= 1.0 {
                            continue;
                        }
                        let i = ((y * width + x) * 4) as usize;
                        for k in 0..3 {
                            surface[i + k] = (snap[i + k] as f32 * (1.0 - cov) + surface[i + k] as f32 * cov).round() as u8;
                        }
                    }
                }
            }
        };
        // A stacking context (Appendix E): its layers, collected through the
        // boxes it paints in normal flow; negative z-index under that flow.
        let mut layers: Vec<Layer> = Vec::new();
        if node_id == layout.root_node || is_stacking_context(layout, node_id) {
            collect_layers(layout, node_id, (current_x, current_y), inherited, child_clip, sx, sy, &mut layers);
            layers.sort_by_key(|l| (l.z, l.order));
        }
        let paint_layers = |layers: &[Layer], surface: &mut [u8]| {
            for l in layers {
                draw_node(
                    l.node, l.abs_x, l.abs_y, l.inh, layout, font, font_bold, surface, width, height, sx, sy,
                    damage_rects, l.clip,
                );
            }
        };
        let split = layers.iter().position(|l| l.z >= 0).unwrap_or(layers.len());
        paint_layers(&layers[..split], surface);
        if child_clip.2 <= child_clip.0 || child_clip.3 <= child_clip.1 {
            paint_layers(&layers[split..], surface);
            finish_round_clip(surface);
            return; // fully clipped out — nothing below can paint
        }
        if let Some(ifc) = ifc {
            // An inline formatting context: its line-box fragments, in tree
            // order (CSS 2.2 Appendix E step 7: each inline box's background
            // and borders, then its text; atomic inlines as whole boxes).
            // AETHERSTYLE3 (SR103): a scroll container's lines move by its offset.
            let (shx, shy) = layout_core::scroll::content_shift(layout, node_id);
            let content_x = current_x + shx + layout_box.border.left + layout_box.padding.left;
            // A table cell's vertical-align places its lines in the cell
            // (§17.5.4; html.css: middle).
            let valign_off = cell_valign_offset(layout, node_id, ifc.height);
            let content_y = current_y + shy + layout_box.border.top + layout_box.padding.top + valign_off;
            // Text baselines snap to whole pixels from the EXACT (unrounded)
            // position, as Skia rounds a glyph run's fractional origin; the
            // box tree's rounded block positions can be up to half a pixel
            // off it. (Box decorations stay on the rounded geometry.)
            let exact_content_y = {
                // The rounding error along the box's ancestor chain.
                let mut err = 0.0f32;
                let mut n = node_id;
                loop {
                    err += layout.taffy.unrounded_layout(n).location.y
                        - layout.taffy.layout(n).map(|l| l.location.y).unwrap_or(0.0);
                    match layout.taffy.parent(n) {
                        Some(p) => n = p,
                        None => break,
                    }
                }
                let u = layout.taffy.unrounded_layout(node_id);
                content_y + err + (u.border.top + u.padding.top) - (layout_box.border.top + layout_box.padding.top)
            };
            // The inherited state of every inline box in the context.
            let mut inh: std::collections::HashMap<NodeId, Inherited> = std::collections::HashMap::new();
            let mut skip: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
            fn walk_inh(
                layout: &LayoutTree,
                id: NodeId,
                parent: Inherited,
                hidden: bool,
                inh: &mut std::collections::HashMap<NodeId, Inherited>,
                skip: &mut std::collections::HashSet<NodeId>,
            ) {
                for k in layout.taffy.children(id).unwrap_or_default() {
                    let mut i = parent;
                    let spec = layout.paint_map.get(&k).cloned().unwrap_or_default();
                    let hid = hidden || spec.hidden.unwrap_or(false);
                    if hid {
                        skip.insert(k);
                    }
                    let mut recurse = false;
                    if let Some(n) = layout.node_map.get(&k) {
                        if let Some(el) = n.as_element() {
                            let tag = el.name.local.as_ref();
                            // Atomic boxes compute their own state in draw_node.
                            inh.insert(k, parent);
                            if !crate::layout::inline::is_atomic_pub(layout, k) {
                                inherit_element(&mut i, tag, &spec, n, k);
                                i.shift_y = 0.0;
                                inh.insert(k, i);
                                recurse = true;
                            }
                        } else {
                            inh.insert(k, parent);
                        }
                    } else {
                        inh.insert(k, parent);
                    }
                    if recurse {
                        walk_inh(layout, k, i, hid, inh, skip);
                    }
                }
            }
            let mut root_inh = inherited;
            root_inh.shift_y = 0.0;
            walk_inh(layout, node_id, root_inh, false, &mut inh, &mut skip);
            // AETHERSELECT: the selected part of each text fragment (empty without a selection).
            let sel_plan = crate::selection::plan_ifc(layout, node_id);
            // AETHERDEBT SR93: every find-in-page match, highlighted behind its text.
            let mark_plan = crate::selection::plan_marks(layout, node_id);
            // AETHERSTYLE3 (SR103): a vertical writing mode's fragments are laid out in a horizontal staging
            // space (inline along x, lines along y). They paint into an offscreen buffer pre-filled with the
            // backdrop each staging pixel lands on, then every pixel the text changed goes to its place in the
            // content box through `writing::to_physical` (vertical-rl / sideways-rl turn the lines clockwise,
            // vertical-lr keeps that glyph turn with the lines left to right, sideways-lr turns them
            // counter-clockwise).
            let vmode = ifc.vertical;
            if vmode != 0 && walk_mode() == 1 {
                ink_add(INK_ALL); // AETHERPAINTPERF: vertical lines land through the staging map
            }
            let (vbw, vbh) = if vmode != 0 {
                ((ifc.inline_extent.ceil().max(1.0)) as u32, (ifc.height.ceil().max(1.0)) as u32)
            } else {
                (0, 0)
            };
            let vorigin = ((content_x - sx as f32).floor() as i32, (content_y - sy as f32).floor() as i32);
            let vline = |y: i32| {
                ifc.lines.iter().find(|(t, h, _)| y as f32 >= t.floor() && (y as f32) < (t + h).floor()).map(|(t, h, _)| {
                    (t.floor() as i32, ((t + h).floor() - t.floor()) as i32)
                })
            };
            // AETHERSTYLE3B (SR115): an upright glyph's cell (CJK under `mixed`, every glyph under `upright`) is
            // translated into its place in the column instead of turned with the line.
            let vmap = |x: i32, y: i32| {
                let (px, py) = layout_core::writing::to_physical_upright(vmode, x, y, vbh as i32, &ifc.upright)
                    .unwrap_or_else(|| layout_core::writing::to_physical(vmode, x, y, vbh as i32, vbw as i32, &vline));
                (vorigin.0 + px, vorigin.1 + py)
            };
            let mut vbuf: Vec<u8> = Vec::new();
            if vmode != 0 {
                vbuf = vec![255; (vbw * vbh * 4) as usize];
                for y in 0..vbh as i32 {
                    for x in 0..vbw as i32 {
                        let (dx, dy) = vmap(x, y);
                        if dx >= 0 && dy >= 0 && (dx as u32) < width && (dy as u32) < height {
                            let si = ((dy as u32 * width + dx as u32) * 4) as usize;
                            let di = ((y as u32 * vbw + x as u32) * 4) as usize;
                            vbuf[di..di + 4].copy_from_slice(&surface[si..si + 4]);
                        }
                    }
                }
            }
            let vinit = vbuf.clone();
            // AETHERRENDER3 (SR126): a collapsed selection in an editing host of this vertical IFC — the text box
            // holding it (fragment, byte) and, once that box is drawn, its physical caret rect and colour.
            let vcaret_at = if vmode != 0 { editable_caret(layout, ifc) } else { None };
            let mut vcaret: Option<((f32, f32, f32, f32), (u8, u8, u8))> = None;
            {
            let vdamage = [(0u32, 0u32, vbw, vbh)];
            #[allow(clippy::type_complexity)]
            let (surface, width, height, damage_rects, child_clip, content_x, content_y, exact_content_y): (
                &mut [u8],
                u32,
                u32,
                &[(u32, u32, u32, u32)],
                Clip,
                f32,
                f32,
                f32,
            ) = if vmode != 0 {
                (&mut vbuf[..], vbw, vbh, &vdamage[..], (0.0, 0.0, vbw as f32, vbh as f32), sx as f32, sy as f32, sy as f32)
            } else {
                (&mut *surface, width, height, damage_rects, child_clip, content_x, content_y, exact_content_y)
            };
            for (fi, frag) in ifc.frags.iter().enumerate() {
                match frag {
                    crate::layout::inline::Frag::Box { node, x, y, w, h, first, last } => {
                        if skip.contains(node) {
                            continue;
                        }
                        let Some(spec) = layout.paint_map.get(node) else { continue };
                        let fx = content_x + x - sx as f32;
                        let fy = content_y + y - sy as f32;
                        if walk_mode() == 1 {
                            ink_add((fx - 2.0, fy - 2.0, fx + *w + 2.0, fy + *h + 2.0));
                            continue;
                        }
                        let mut sides: [boxpaint::Side; 4] = [None; 4];
                        if let Some(b) = spec.border {
                            let styles = spec.border_style.unwrap_or([0; 4]);
                            for i in 0..4 {
                                let mut side = b[i];
                                if let (Some(ws), Some((_, c))) = (spec.border_width.as_ref(), side) {
                                    if let Some(w) = ws[i] {
                                        side = Some((w, c));
                                    }
                                }
                                sides[i] = side.filter(|(w, _)| *w > 0.0).map(|(w, c)| (w, c, styles[i]));
                            }
                        }
                        // box-decoration-break: slice — the inline-start edge
                        // only on the first fragment, the end on the last.
                        if !*first {
                            sides[3] = None;
                        }
                        if !*last {
                            sides[1] = None;
                        }
                        let mut radius = spec.radius.unwrap_or([0.0; 4]);
                        if !*first {
                            radius[0] = 0.0;
                            radius[3] = 0.0;
                        }
                        if !*last {
                            radius[1] = 0.0;
                            radius[2] = 0.0;
                        }
                        let rounded = radius.iter().any(|&r| r != 0.0)
                            || sides.iter().any(|s| s.is_some_and(|s| s.2 != 0));
                        let bga = spec.bg_alpha.unwrap_or(1.0);
                        let bda = spec.border_alpha.unwrap_or(1.0);
                        let bg_rgb = spec.background;
                        let mut blend_at = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                            // Background and border colours composite at their alpha.
                            let a = a * if Some(c) == bg_rgb { bga } else { bda };
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, child_clip) {
                                return;
                            }
                            if a >= 0.999 {
                                put_px(surface, width, x, y, c);
                            } else if a > 0.0 {
                                blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                            }
                        };
                        if rounded {
                            boxpaint::paint(fx, fy, *w, *h, radius, sides, spec.background, width, height, &mut blend_at);
                        } else {
                            // Pixel-snapped rectangles, as Blink snaps box decorations.
                            let (x0, y0) = (fx.round(), fy.round());
                            let (x1, y1) = ((fx + w).round(), (fy + h).round());
                            let mut fill = |ax: f32, ay: f32, bx: f32, by: f32, c: (u8, u8, u8)| {
                                let (ax, ay) = (ax.max(0.0) as u32, ay.max(0.0) as u32);
                                let (bx, by) = ((bx.max(0.0) as u32).min(width), (by.max(0.0) as u32).min(height));
                                for yy in ay..by {
                                    for xx in ax..bx {
                                        blend_at(xx, yy, c, 1.0);
                                    }
                                }
                            };
                            if let Some(bg) = spec.background {
                                fill(x0, y0, x1, y1, bg);
                            }
                            if let Some((bw, c, _)) = sides[0] {
                                fill(x0, y0, x1, y0 + bw.round().max(1.0), c);
                            }
                            if let Some((bw, c, _)) = sides[2] {
                                fill(x0, y1 - bw.round().max(1.0), x1, y1, c);
                            }
                            if let Some((bw, c, _)) = sides[3] {
                                fill(x0, y0, x0 + bw.round().max(1.0), y1, c);
                            }
                            if let Some((bw, c, _)) = sides[1] {
                                fill(x1 - bw.round().max(1.0), y0, x1, y1, c);
                            }
                        }
                    }
                    crate::layout::inline::Frag::Text { node, text, x, baseline, .. } => {
                        if skip.contains(node) {
                            continue;
                        }
                        let Some(i) = inh.get(node).copied() else { continue };
                        if i.text_hidden {
                            continue;
                        }
                        let adv = i.advancer();
                        let ox = content_x + x - sx as f32;
                        let by = (exact_content_y + baseline).round() - sy as f32;
                        // The line box the fragment sits on (selection highlights span it, as Blink's do).
                        let (ltop, lbot) = match crate::selection::line_of(ifc, *baseline) {
                            Some((_, t, h)) => ((exact_content_y + t).round() - sy as f32, (exact_content_y + t + h).round() - sy as f32),
                            None => (by - i.font_size, by + i.font_size * 0.25),
                        };
                        let fw = match frag {
                            crate::layout::inline::Frag::Text { width: w, .. } => *w,
                            _ => 0.0,
                        };
                        // AETHERSTYLE3B (SR115): a vertical IFC's box is recorded in its staging space anchored at
                        // the content box's physical origin — the space `layout_core::hit` maps a pointer into.
                        let (rx, ry) = if vmode != 0 { (vorigin.0 as f32, vorigin.1 as f32) } else { (0.0, 0.0) };
                        // AETHERPAINTPERF (SR130): the index walk records the text boxes (the indexed paint walk skips
                        // boxes, so it records none), and the fragment's ink: its advance and line box, glyph overhang
                        // (3 em up, 2 em down, 2 em across) and the spacing a selected line break adds.
                        if walk_mode() != 2 {
                            TEXT_BOXES.with(|t| {
                                t.borrow_mut().push(TextBox { ifc: node_id, frag: fi, node: *node, x: ox + rx, w: fw, top: ltop + ry, bottom: lbot + ry, adv, scroll: (sx, sy) })
                            });
                        }
                        if walk_mode() == 1 {
                            let fs = i.font_size;
                            let m = 2.0 * fs + 4.0 + i.letter_spacing.abs() + i.word_spacing.abs();
                            if i.shadows(layout).is_empty() {
                                // 3 em above the baseline, 2 em below it, 2 em and the spacing either side.
                                ink_add((ox - m, ltop.min(by - 3.0 * fs) - 4.0, ox + fw + m, lbot.max(by + 2.0 * fs) + 4.0));
                            } else {
                                ink_add(INK_ALL);
                            }
                            continue;
                        }
                        let deco = Deco { underline: i.underline, line_through: i.line_through };
                        // AETHERRENDER3 (SR126): the caret of an editable vertical field — `layout_core::hit::caret_rect` maps the
                        // caret offset of this text box to the physical bar (a horizontal bar across the column), painted on the
                        // page once the staging buffer is composited.
                        if let Some((_, cb)) = vcaret_at.filter(|&(cf, _)| cf == fi) {
                            let tb = layout_core::hit::TextBox { ifc: node_id, frag: fi, node: *node, x: ox + rx, w: fw, top: ltop + ry, bottom: lbot + ry };
                            vcaret = Some((layout_core::hit::caret_rect(ifc, &tb, crate::selection::caret_x(&adv, text, cb)), i.color));
                        }
                        // AETHERRENDER3 (SR126): a `text-combine-upright` run is drawn at its natural width, then compressed into its
                        // 1em cell by `cell.scale` about the cell's start ([`compress_combined`]).
                        let combined = if vmode != 0 { combined_cell(ifc, *x, *baseline) } else { None };
                        let pre_combine = combined.map(|_| surface.to_vec());
                        if let Some(marks) = mark_plan.get(&fi) {
                            for &(m0, m1) in marks {
                                let (hx0, hx1) = (
                                    (ox + crate::selection::caret_x(&adv, text, m0)).round(),
                                    (ox + crate::selection::caret_x(&adv, text, m1)).round(),
                                );
                                let (ax, ay) = (hx0.max(child_clip.0).max(0.0) as u32, ltop.max(child_clip.1).max(0.0) as u32);
                                let (bx, by1) = ((hx1.min(child_clip.2).max(0.0) as u32).min(width), (lbot.min(child_clip.3).max(0.0) as u32).min(height));
                                for yy in ay..by1 {
                                    for xx in ax..bx {
                                        if in_damage(xx, yy, damage_rects) {
                                            blend_px(surface, width, xx, yy, crate::selection::FIND_MARK, 255);
                                        }
                                    }
                                }
                            }
                        }
                        match sel_plan.get(&fi) {
                            None => {
                                draw_glyph_run(text, ox, by, &adv, i.color, deco, i.shadows(layout), surface, width, height, damage_rects, child_clip);
                            }
                            Some(fs) => {
                                // Inverse video: the highlight behind the glyphs (pixel-snapped, the line
                                // box tall, a space wider when the line break is selected), the selected
                                // glyphs in the highlight text colour, the rest as they were.
                                let mut x1 = crate::selection::caret_x(&adv, text, fs.end);
                                if fs.newline {
                                    x1 += adv.char(' ');
                                }
                                let (hx0, hx1) = ((ox + crate::selection::caret_x(&adv, text, fs.start)).round(), (ox + x1).round());
                                let (hy0, hy1) = (ltop, lbot);
                                let (ax, ay) = (hx0.max(child_clip.0).max(0.0) as u32, hy0.max(child_clip.1).max(0.0) as u32);
                                let (bx, by1) = ((hx1.min(child_clip.2).max(0.0) as u32).min(width), (hy1.min(child_clip.3).max(0.0) as u32).min(height));
                                for yy in ay..by1 {
                                    for xx in ax..bx {
                                        if in_damage(xx, yy, damage_rects) {
                                            blend_px(surface, width, xx, yy, crate::selection::HIGHLIGHT, crate::selection::HIGHLIGHT_ALPHA);
                                        }
                                    }
                                }
                                let left = (child_clip.0, child_clip.1, child_clip.2.min(hx0), child_clip.3);
                                let mid = (child_clip.0.max(hx0), child_clip.1, child_clip.2.min(hx1), child_clip.3);
                                let right = (child_clip.0.max(hx1), child_clip.1, child_clip.2, child_clip.3);
                                for (c, color, shadows) in [
                                    (left, i.color, i.shadows(layout)),
                                    (right, i.color, i.shadows(layout)),
                                    (mid, crate::selection::HIGHLIGHT_TEXT, &[][..]),
                                ] {
                                    if c.2 > c.0 {
                                        draw_glyph_run(text, ox, by, &adv, color, deco, shadows, surface, width, height, damage_rects, c);
                                    }
                                }
                            }
                        }
                        if let (Some(c), Some(pre)) = (combined, pre_combine.as_deref()) {
                            compress_combined(surface, pre, width, height, c, ltop, lbot);
                        }
                    }
                    crate::layout::inline::Frag::Atomic { node, x, y } => {
                        if skip.contains(node) || is_layered(layout, *node) {
                            continue;
                        }
                        let i = inh.get(node).copied().unwrap_or(inherited);
                        let loc = layout.taffy.layout(*node).map(|l| l.location).unwrap_or(taffy::Point { x: 0.0, y: 0.0 });
                        draw_node(
                            *node, content_x + x - loc.x, content_y + y - loc.y, i, layout, font, font_bold, surface,
                            width, height, sx, sy, damage_rects, child_clip,
                        );
                    }
                }
            }
            }
            if vmode != 0 {
                for y in 0..vbh as i32 {
                    for x in 0..vbw as i32 {
                        let di = ((y as u32 * vbw + x as u32) * 4) as usize;
                        if vbuf[di..di + 3] == vinit[di..di + 3] {
                            continue;
                        }
                        let (px, py) = vmap(x, y);
                        if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
                            continue;
                        }
                        let (pxu, pyu) = (px as u32, py as u32);
                        if !in_damage(pxu, pyu, damage_rects) || !in_clip(pxu, pyu, child_clip) {
                            continue;
                        }
                        let si = ((pyu * width + pxu) * 4) as usize;
                        surface[si..si + 4].copy_from_slice(&vbuf[di..di + 4]);
                    }
                }
                if let Some(((cx, cy, cw, ch), color)) = vcaret {
                    let (x0, y0) = (cx.floor(), cy.floor());
                    fill_rect(x0, y0, x0 + cw.round().max(1.0), y0 + ch.round().max(1.0), color, 255, surface, width, height, damage_rects, child_clip);
                }
            }
        }
        if ifc.is_none() {
            // Normal flow (Appendix E steps 3-5), tree order; positioned and
            // stacking-context boxes were hoisted into their context's layers.
            // AETHERSTYLE3 (SR103): a scroll container's children move by its offset, a multicol
            // container's by the column they were balanced into.
            let (shx, shy) = layout_core::scroll::content_shift(layout, node_id);
            for child in layout.taffy.children(node_id).unwrap_or_default() {
                if is_layered(layout, child) {
                    continue;
                }
                let (mx, my) = layout_core::col_shift(layout, child);
                draw_node(
                    child, current_x + shx + mx, current_y + shy + my, inherited, layout, font, font_bold, surface,
                    width, height, sx, sy, damage_rects, child_clip,
                );
            }
        }
        // Steps 6-7: z-index auto/0 positioned boxes in tree order, then
        // positive z-index ascending.
        paint_layers(&layers[split..], surface);
        finish_round_clip(surface);
        // AETHERSTYLE3 (SR103): css-multicol-1 §4.2 — a column rule midway in each gap between columns that
        // hold content, as tall as the column boxes (painted solid whatever its style).
        if let Some(cols) = layout.multicol.get(&node_id).filter(|_| walk_mode() != 1) {
            if let Some((rw, rc, _)) = cols.rule {
                let (shx, shy) = layout_core::scroll::content_shift(layout, node_id);
                let cx0 = current_x - sx as f32 + shx + layout_box.border.left + layout_box.padding.left;
                let cy0 = current_y - sy as f32 + shy + layout_box.border.top + layout_box.padding.top;
                // AETHERSTYLE3B (SR115): a container split by `column-span: all` rules each column set.
                let one = [layout_core::multicol::ColumnSet { top: 0.0, height: cols.height, filled: cols.filled }];
                let sets = layout.col_sets.get(&node_id).map(|v| v.as_slice()).unwrap_or(&one);
                for set in sets {
                    for k in 1..set.filled {
                        let mid = cx0 + k as f32 * (cols.width + cols.gap) - cols.gap / 2.0;
                        let x0 = (mid - rw / 2.0).round();
                        let y0 = cy0 + set.top;
                        fill_rect(x0, y0, x0 + rw.max(1.0).round(), y0 + set.height, rc, 255, surface, width, height, damage_rects, child_clip);
                    }
                }
            }
        }
        // AETHERSTYLE3 (SR103): a scroll container's overlay scrollbars, over its content.
        let bars = layout_core::scroll::scrollbars(layout, node_id);
        let (bx0, by0) = (current_x - sx as f32, current_y - sy as f32);
        for bar in [bars.v, bars.h].into_iter().flatten().filter(|_| walk_mode() != 1) {
            let (tx, ty, tw, th) = bar.track;
            fill_rect(bx0 + tx, by0 + ty, bx0 + tx + tw, by0 + ty + th, SCROLL_TRACK, 255, surface, width, height, damage_rects, clip);
            let (mx, my, mw, mh) = bar.thumb;
            fill_rect(bx0 + mx, by0 + my, bx0 + mx + mw, by0 + my + mh, SCROLL_THUMB, 255, surface, width, height, damage_rects, clip);
        }
    }

    let root_inherited = root_inherited();
    // AETHERPAINTPERF (SR130): the index walk, then the paint walk that skips what meets no damage rect (one
    // classic walk when the scopes are off).
    let indexed = crate::perf::scoped() || index_only;
    INDEX.with(|x| {
        let mut x = x.borrow_mut();
        x.ink.clear();
        x.stack.clear();
        x.scroll = (sx, sy);
        x.painted = 0;
        x.skipped = 0;
        x.mode = if indexed { 1 } else { 0 };
        // AETHERPAINTPERF2 (SR132): the index walk records the frame's on-screen boxes afresh.
        x.seq = 0;
        x.open.clear();
        x.snap = FrameSnap {
            boxes: std::collections::HashMap::new(),
            scroll: (sx, sy),
            size: if indexed { (width, height) } else { (0, 0) },
            top_gen: crate::js::toplayer::generation(),
        };
    });
    // AETHERTOP (SR146): the top layer's boxes (one per element, its first box), in top-layer order, with each
    // element's `::backdrop` colour from the cascade (`css::top_layer_paint`).
    let top: Vec<(NodeId, Option<((u8, u8, u8), f32)>)> = crate::css::top_layer_paint()
        .into_iter()
        .filter_map(|(dom, backdrop)| top_layer_box(layout, dom).map(|t| (t, backdrop)))
        .collect();
    TOP_IDS.with(|t| *t.borrow_mut() = top.iter().map(|(t, _)| *t).collect());

    /// AETHERTOP (SR146): the top layer painted after the document (HTML §15.5.x "the top layer", CSS Position 4
    /// §top-layer): each element in order, as a stacking context of its own, after its `::backdrop` — a box the
    /// size of the viewport, blended at the backdrop's colour and alpha over everything painted so far. The element
    /// is painted from its parent's border-box origin with the paint state inherited along its ancestor chain
    /// (its UA rule makes it a fixed box, so `placed_origin` places it against the viewport).
    #[allow(clippy::too_many_arguments)]
    fn paint_top_layer(
        top: &[(NodeId, Option<((u8, u8, u8), f32)>)],
        root_inherited: Inherited,
        layout: &LayoutTree,
        font: &Option<&'static Font>,
        font_bold: &Option<&'static Font>,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
        damage_rects: &[(u32, u32, u32, u32)],
    ) {
        for (tid, backdrop) in top {
            if let Some((rgb, a)) = backdrop {
                if walk_mode() != 1 {
                    let alpha = (a * 255.0).round().clamp(0.0, 255.0) as u8;
                    for &(dx, dy, dw, dh) in damage_rects {
                        for y in dy..(dy + dh).min(height) {
                            for x in dx..(dx + dw).min(width) {
                                blend_px(surface, width, x, y, *rgb, alpha);
                            }
                        }
                    }
                }
            }
            let mut chain = Vec::new();
            let mut cur = layout.taffy.parent(*tid);
            while let Some(p) = cur {
                chain.push(p);
                cur = layout.taffy.parent(p);
            }
            let mut inh = root_inherited;
            let (mut ax, mut ay) = (0.0f32, 0.0f32);
            for p in chain.iter().rev() {
                if let Ok(l) = layout.taffy.layout(*p) {
                    ax += l.location.x;
                    ay += l.location.y;
                }
                if let Some(dn) = layout.node_map.get(p) {
                    if let Some(el) = dn.as_element() {
                        let spec = layout.paint_map.get(p).cloned().unwrap_or_default();
                        inherit_element(&mut inh, el.name.local.as_ref(), &spec, dn, *p);
                    }
                }
            }
            TOP_PASS.with(|t| t.set(Some(*tid)));
            draw_node(
                *tid, ax, ay, inh, layout, font, font_bold, surface, width, height, sx, sy, damage_rects,
                (0.0, 0.0, width as f32, height as f32),
            );
            TOP_PASS.with(|t| t.set(None));
        }
    }

    if indexed {
        draw_node(
            layout.root_node, 0.0, 0.0, root_inherited, layout, &font, &font_bold, surface, width, height, sx, sy,
            damage_rects, (0.0, 0.0, width as f32, height as f32),
        );
        paint_top_layer(&top, root_inherited, layout, &font, &font_bold, surface, width, height, sx, sy, damage_rects);
        INDEX.with(|x| x.borrow_mut().mode = if index_only { 0 } else { 2 });
        if index_only {
            return;
        }
    }
    draw_node(
        layout.root_node,
        0.0,
        0.0,
        root_inherited,
        layout,
        &font,
        &font_bold,
        surface,
        width,
        height,
        sx,
        sy,
        damage_rects,
        (0.0, 0.0, width as f32, height as f32),
    );
    paint_top_layer(&top, root_inherited, layout, &font, &font_bold, surface, width, height, sx, sy, damage_rects);
    let (painted, skipped) = INDEX.with(|x| {
        let mut x = x.borrow_mut();
        x.mode = 0;
        (x.painted, x.skipped)
    });
    crate::perf::add_painted(painted, skipped);
    // AETHERA11Y (SR119): `:focus-visible` from the keyboard — the ring over everything.
    focusring::paint(layout, surface, width, height, sx, sy, damage_rects);
}
