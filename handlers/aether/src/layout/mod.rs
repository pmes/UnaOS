//! Aether's layout. LAYOUTCORE (LEDGER SR76, AETHERMETAL M2): the engine — box generation, the taffy style
//! mapping, the inline formatting context, tables, margin collapsing, block widths — is `layout_core`
//! (unaos/libs/web/layout_core), the ONE engine the ring-3 APPS/AETHER.ELF lays out with too. This module is
//! the host's side of its seams:
//!
//! * `Dom` over [`NodeRef`] (html_core's arena behind the `Rc<RefCell<…>>` handle);
//! * `Fonts` over `fonts/` ([`HostFonts`]: the text_host face database, the shaper, `fonts::lines`);
//! * the `Host` hooks ([`AetherHost`]): the coverage ledger, `render::effects` gradients and shadows, the
//!   family-list registry, media / image intrinsic sizes, script-set `<option>` selectedness, `var()`;
//!
//! and the entry points the rest of the browser has always called (`compute_layout`, `build_tree`,
//! `remeasure`, …), with `LayoutTree` / `PaintStyle` the core's types for this host. `generated` (AETHERGEN,
//! SR71) stays here: it builds its boxes from the host's cascade output into the host document.

use crate::dom::NodeRef;
use layout_core::{Adv, FontSel, Line, NodeKind, TextMode};

pub(crate) mod generated;
pub mod inline;

pub use layout_core::{
    control_kind, default_font_size, default_italic, default_line_through, default_rtl, default_weight,
    default_white_space, is_inline, is_quirks, ua_font_size, Control, FontWeight, CONTROL_FONT_PX,
};

/// The box tree for this host's documents.
pub type LayoutTree = layout_core::LayoutTree<AetherHost>;
/// A box's paint style for this host (gradients and shadows are `render::effects`').
pub type PaintStyle = layout_core::PaintStyle<AetherHost>;

pub fn compute_layout(dom: &NodeRef) -> LayoutTree {
    compute_layout_sized(dom, 800.0, 600.0)
}

pub fn compute_layout_sized(dom: &NodeRef, vw: f32, vh: f32) -> LayoutTree {
    let mut tree = build_tree(dom, vw, vh);
    remeasure(&mut tree);
    tree
}

/// Builds the box tree WITHOUT the measuring layout pass (`layout_core::build_tree`). Records the viewport the
/// host's later parses (`css::parse_px`, the cascade) resolve `vw`/`vh` against, as the builder always did.
pub fn build_tree(dom: &NodeRef, vw: f32, vh: f32) -> LayoutTree {
    crate::css::set_viewport(vw, vh);
    layout_core::build_tree::<AetherHost>(dom, vw, vh)
}

/// Recomputes layout with real text measurement (`layout_core::remeasure` over [`HostFonts`]).
pub fn remeasure(tree: &mut LayoutTree) {
    layout_core::remeasure(tree, &HostFonts);
}

// ---- AETHERSTYLE3 (SR103): scroll containers' offsets across relayouts ---------------------------------

thread_local! {
    /// Each scroll container's offset by ELEMENT (a relayout rebuilds the boxes; the element keeps its scroll
    /// position, css-overflow-3 §2.3), and the thumb drag in progress: (element, axis, pointer start, offset start).
    static SCROLLED: std::cell::RefCell<std::collections::HashMap<html_core::NodeId, (f32, f32)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static SCROLL_DRAG: std::cell::Cell<Option<(html_core::NodeId, u8, f32, f32)>> = const { std::cell::Cell::new(None) };
    /// AETHERSTYLE3C (SR122): each snap container's snapped target per axis, by ELEMENT (css-scroll-snap-1 §5.5:
    /// after a relayout the container re-snaps to the same element, wherever it moved).
    static SNAPPED: std::cell::RefCell<std::collections::HashMap<html_core::NodeId, (Option<html_core::NodeId>, Option<html_core::NodeId>)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The snap containers of a tree (`scroll-snap-type` on a scroll container).
fn snap_containers(tree: &LayoutTree) -> Vec<taffy::NodeId> {
    tree.paint_map
        .iter()
        .filter(|(_, p)| p.snap_type.is_some_and(|(a, _)| a != 0))
        .map(|(id, _)| *id)
        .filter(|id| layout_core::scroll::snap_axes(tree, *id) != (false, false))
        .collect()
}

/// Records every box's current offset under its element.
pub(crate) fn remember_scroll(tree: &LayoutTree) {
    SCROLLED.with(|m| {
        let mut m = m.borrow_mut();
        m.clear();
        for (id, off) in &tree.scroll {
            if let Some(n) = tree.node_map.get(id) {
                m.insert(n.id(), *off);
            }
        }
    });
    SNAPPED.with(|m| {
        let mut m = m.borrow_mut();
        m.clear();
        let el = |t: Option<taffy::NodeId>| t.and_then(|t| tree.node_map.get(&t)).map(|n| n.id());
        for id in snap_containers(tree) {
            let (tx, ty) = layout_core::scroll::snapped_targets(tree, id);
            if let (Some(n), true) = (tree.node_map.get(&id), tx.is_some() || ty.is_some()) {
                m.insert(n.id(), (el(tx), el(ty)));
            }
        }
    });
}

/// Puts the recorded offsets back onto a freshly laid-out tree (clamped to the new scroll range).
pub(crate) fn restore_scroll(tree: &mut LayoutTree) {
    let saved: Vec<(html_core::NodeId, (f32, f32))> = SCROLLED.with(|m| m.borrow().iter().map(|(k, v)| (*k, *v)).collect());
    let boxes: Vec<(taffy::NodeId, html_core::NodeId)> = tree.node_map.iter().map(|(t, n)| (*t, n.id())).collect();
    if !saved.is_empty() {
        for &(tid, nid) in &boxes {
            if let Some((_, (x, y))) = saved.iter().find(|(k, _)| *k == nid) {
                layout_core::scroll::set_offset(tree, tid, *x, *y);
            }
        }
    }
    // AETHERSTYLE3C (SR122): every snap container re-snaps — to the element it was snapped to, else to the
    // nearest position from the restored offset — and its new offset is what is remembered for it.
    let snaps = snap_containers(tree);
    if snaps.is_empty() {
        return;
    }
    let targets = SNAPPED.with(|m| m.borrow().clone());
    let find = |n: Option<html_core::NodeId>| n.and_then(|n| boxes.iter().find(|(_, b)| *b == n).map(|(t, _)| *t));
    for id in snaps {
        let el = tree.node_map.get(&id).map(|n| n.id());
        let (tx, ty) = el.and_then(|e| targets.get(&e).copied()).unwrap_or((None, None));
        let off = layout_core::scroll::resnap(tree, id, (find(tx), find(ty)));
        if let Some(e) = el {
            SCROLLED.with(|m| {
                let mut m = m.borrow_mut();
                if off == (0.0, 0.0) {
                    m.remove(&e);
                } else {
                    m.insert(e, off);
                }
            });
        }
    }
}

/// A new document: no element is scrolled.
pub(crate) fn forget_scroll() {
    SCROLLED.with(|m| m.borrow_mut().clear());
    SCROLL_DRAG.with(|d| d.set(None));
    SNAPPED.with(|m| m.borrow_mut().clear());
    SCROLL_REQUESTS.with(|r| r.borrow_mut().clear());
    SMOOTH.with(|s| s.borrow_mut().clear());
}

/// A thumb drag starts on box `id` along `axis` at pointer coordinate `at` with the offset `start`.
pub(crate) fn begin_scroll_drag(tree: &LayoutTree, id: taffy::NodeId, axis: u8, at: f32, start: f32) {
    if let Some(n) = tree.node_map.get(&id) {
        SCROLL_DRAG.with(|d| d.set(Some((n.id(), axis, at, start))));
    }
}

/// The drag follows the pointer (document px); `false` when no drag is in progress.
pub(crate) fn continue_scroll_drag(tree: &mut LayoutTree, px: f32, py: f32) -> bool {
    let Some((nid, axis, at, start)) = SCROLL_DRAG.with(|d| d.get()) else { return false };
    let Some(tid) = tree.node_map.iter().find(|(_, n)| n.id() == nid).map(|(t, _)| *t) else { return false };
    let delta = if axis == 1 { py - at } else { px - at };
    let v = layout_core::scroll::drag_offset(tree, tid, axis, start, delta);
    let (cx, cy) = layout_core::scroll::offset(tree, tid);
    if axis == 1 {
        layout_core::scroll::set_offset(tree, tid, cx, v);
    } else {
        layout_core::scroll::set_offset(tree, tid, v, cy);
    }
    remember_scroll(tree);
    true
}

/// The button went up: the drag ends.
pub(crate) fn end_scroll_drag() {
    SCROLL_DRAG.with(|d| d.set(None));
}

// ---- AETHERSTYLE3B (SR115): programmatic scrolling of an element (CSSOM View §7) --------------------------------

/// How a programmatic scroll moves (CSSOM View §5.1 `ScrollBehavior`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollBehavior {
    /// The element's `scroll-behavior` decides.
    Auto,
    Instant,
    Smooth,
}

thread_local! {
    /// Scrolls asked for since the last frame: (element, x or keep, y or keep, behavior).
    static SCROLL_REQUESTS: std::cell::RefCell<Vec<(html_core::NodeId, Option<f32>, Option<f32>, ScrollBehavior)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Smooth scrolls in flight, by element.
    static SMOOTH: std::cell::RefCell<std::collections::HashMap<html_core::NodeId, layout_core::scroll::SmoothScroll>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// An element's scroll offset (`scrollLeft`, `scrollTop`) as the page's laid-out tree last recorded it —
/// mid-way through a smooth scroll, where it is now. (0, 0) for an element that is not scrolled.
pub fn element_scroll_offset(el: html_core::NodeId) -> (f32, f32) {
    SCROLLED.with(|m| m.borrow().get(&el).copied()).unwrap_or((0.0, 0.0))
}

/// A script scrolls `el` to `x` / `y` (None: that axis stays): `scrollTop =`, `scrollLeft =`, `scrollTo()`,
/// `scroll()`; `scrollBy()` passes the current offset plus its delta. Applied on the next frame
/// ([`scroll_frame`]) against the page's tree — clamped, snapped on a snap container, animated when smooth.
pub fn request_element_scroll(el: html_core::NodeId, x: Option<f32>, y: Option<f32>, behavior: ScrollBehavior) {
    SCROLL_REQUESTS.with(|r| r.borrow_mut().push((el, x, y, behavior)));
}

/// One frame of programmatic scrolling: the queued requests start (or jump), the smooth scrolls advance.
/// `true` when an offset moved (the frame repaints).
pub(crate) fn scroll_frame(tree: &mut LayoutTree) -> bool {
    let reqs = SCROLL_REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()));
    let busy = SMOOTH.with(|s| !s.borrow().is_empty());
    if reqs.is_empty() && !busy {
        return false;
    }
    let now = crate::css::anim::now_ms();
    let tid_of = |tree: &LayoutTree, el: html_core::NodeId| tree.node_map.iter().find(|(_, n)| n.id() == el && n.is_element()).map(|(t, _)| *t);
    let mut moved = false;
    for (el, x, y, behavior) in reqs {
        let Some(tid) = tid_of(tree, el) else { continue };
        let cur = layout_core::scroll::offset(tree, tid);
        let to = layout_core::scroll::scroll_target(tree, tid, x.unwrap_or(cur.0), y.unwrap_or(cur.1));
        let smooth = match behavior {
            ScrollBehavior::Smooth => true,
            ScrollBehavior::Instant => false,
            ScrollBehavior::Auto => layout_core::scroll::smooth_behavior(tree, tid),
        };
        if smooth && to != cur {
            SMOOTH.with(|s| s.borrow_mut().insert(el, layout_core::scroll::SmoothScroll::new(cur, to, now)));
        } else {
            SMOOTH.with(|s| s.borrow_mut().remove(&el));
            moved |= layout_core::scroll::set_offset(tree, tid, to.0, to.1) != cur;
        }
    }
    let flights: Vec<(html_core::NodeId, layout_core::scroll::SmoothScroll)> = SMOOTH.with(|s| s.borrow().iter().map(|(k, v)| (*k, *v)).collect());
    for (el, f) in flights {
        let ((x, y), done) = f.at(now);
        match tid_of(tree, el) {
            Some(tid) => {
                let cur = layout_core::scroll::offset(tree, tid);
                moved |= layout_core::scroll::set_offset(tree, tid, x, y) != cur;
            }
            None => {
                SMOOTH.with(|s| s.borrow_mut().remove(&el));
                continue;
            }
        }
        if done {
            SMOOTH.with(|s| s.borrow_mut().remove(&el));
        }
    }
    if moved {
        remember_scroll(tree);
    }
    moved || SMOOTH.with(|s| !s.borrow().is_empty())
}

/// The (submit value, display label) of a `<select>`'s selected option (`layout_core::select_selected_option`).
pub(crate) fn select_selected_option(select: &NodeRef) -> Option<(String, String)> {
    layout_core::select_selected_option::<AetherHost>(select)
}

/// UA default font FAMILY per element (`layout_core::default_family`).
pub fn default_family(tag: &str, inherited: u16) -> u16 {
    layout_core::default_family::<AetherHost>(tag, inherited)
}

/// The family-list id of `Arial` (the control font's computed family).
pub fn control_family() -> u16 {
    static ID: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *ID.get_or_init(|| crate::fonts::intern_family_list(vec![crate::fonts::Family::Named("Arial".into())]))
}

// ---- the Host hooks --------------------------------------------------------------------------------------

/// The host browser's side of `layout_core::Host`.
#[derive(Clone, Copy, Debug, Default)]
pub struct AetherHost;

impl layout_core::Host for AetherHost {
    type Node = NodeRef;
    type Gradient = crate::render::effects::Gradient;
    type Shadow = crate::render::effects::Shadow;

    fn record_css(what: &str) {
        crate::ledger::record_css(what);
    }
    fn parse_gradient(value: &str) -> Option<Self::Gradient> {
        crate::render::effects::parse_gradient(value)
    }
    fn parse_box_shadow(value: &str) -> Vec<Self::Shadow> {
        crate::render::effects::parse_box_shadow(value)
    }
    fn shadow_is_inset(s: &Self::Shadow) -> bool {
        s.inset
    }
    fn parse_color_alpha(value: &str) -> Option<((u8, u8, u8), f32)> {
        crate::render::effects::parse_color_alpha(value)
    }
    fn intern_family_list(value: &str) -> Option<u16> {
        crate::fonts::parse_family_list(value).map(crate::fonts::intern_family_list)
    }
    fn control_family() -> u16 {
        control_family()
    }
    fn parse_object_fit(value: &str) -> Option<u8> {
        crate::media::parse_object_fit(value)
    }
    fn custom_prop(name: &str) -> Option<String> {
        crate::css::custom_prop(name)
    }
    fn transform_text(text: &str, tt: u8) -> String {
        crate::render::transform_text(text, tt)
    }
    fn media_intrinsic(node: &NodeRef) -> Option<(f32, f32)> {
        if let Some(el) = node.as_element() {
            match el.name.local.as_ref() {
                // AETHERFRAMES (SR83): a nested browsing context's box (the iframe's own size rules).
                "iframe" => return crate::frames::replaced_size(node),
                // AETHERCANVAS (SR85): HTML §4.12.5 — the bitmap's width/height attributes, else 300×150.
                "canvas" => {
                    let attrs = el.attributes.borrow();
                    let px = |n: &str| attrs.get(n).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| *v >= 0.0);
                    return Some((px("width").unwrap_or(300.0), px("height").unwrap_or(150.0)));
                }
                _ => {}
            }
        }
        crate::media::intrinsic_size(node)
    }
    fn img_intrinsic(node: &NodeRef) -> Option<(f32, f32)> {
        crate::images::img_source(node, crate::images::SELECTION_VIEWPORT) // AETHERMEDIA SR107: <picture>
            .and_then(|s| crate::images::get(&s))
            .map(|i| (i.width() as f32, i.height() as f32))
    }
    fn option_selected(node: &NodeRef) -> Option<bool> {
        crate::js::option_selectedness(node)
    }
}

// ---- the Dom seam --------------------------------------------------------------------------------------------

impl layout_core::DomNode for NodeRef {
    fn kind(&self) -> NodeKind {
        self.with_doc(|d, id| match d.data(id) {
            html_core::NodeData::Element(_) => NodeKind::Element,
            html_core::NodeData::Text(_) => NodeKind::Text,
            html_core::NodeData::Comment(_) => NodeKind::Comment,
            html_core::NodeData::Doctype { .. } => NodeKind::Doctype,
            html_core::NodeData::Document => NodeKind::Document,
            _ => NodeKind::Other,
        })
    }
    fn tag(&self) -> Option<String> {
        self.local_name()
    }
    fn attr(&self, name: &str) -> Option<String> {
        let el = self.as_element()?;
        let attrs = el.attributes.borrow();
        attrs.get(name).map(str::to_string)
    }
    // AETHERSTYLE3B (SR115): a referenced SVG `<filter>` is serialized for svg_core with every attribute.
    fn attrs(&self) -> Vec<(String, String)> {
        let Some(el) = self.as_element() else { return Vec::new() };
        let attrs = el.attributes.borrow();
        attrs.map.iter().map(|(n, a)| (n.local.to_string(), a.value.to_string())).collect()
    }
    fn text(&self) -> String {
        self.text_contents()
    }
    fn children(&self) -> Vec<NodeRef> {
        NodeRef::children(self).collect()
    }
    fn parent(&self) -> Option<NodeRef> {
        NodeRef::parent(self)
    }
    fn same(&self, other: &NodeRef) -> bool {
        self == other
    }
    fn doctype(&self) -> Option<(String, String, String)> {
        self.as_doctype().map(|d| (d.name, d.public_id, d.system_id))
    }
    fn preceding_siblings(&self) -> Vec<NodeRef> {
        NodeRef::preceding_siblings(self).collect()
    }
    fn following_siblings(&self) -> Vec<NodeRef> {
        NodeRef::following_siblings(self).collect()
    }
    fn ancestors(&self) -> Vec<NodeRef> {
        NodeRef::ancestors(self).collect()
    }
    fn descendants(&self) -> Vec<NodeRef> {
        NodeRef::descendants(self).collect()
    }
    fn is_element(&self) -> bool {
        NodeRef::is_element(self)
    }
}

impl layout_core::Dom for NodeRef {
    type Node = NodeRef;
    fn root(&self) -> NodeRef {
        self.clone()
    }
}

// ---- the Fonts seam ------------------------------------------------------------------------------------------

/// `layout_core::Fonts` over `fonts/`: the same face lookups, shaper and line breaker the layout called before
/// the move, so the host's numbers do not move with it.
pub struct HostFonts;

fn sel_of(s: FontSel) -> crate::fonts::FontSel {
    crate::fonts::FontSel { family: s.family, weight: s.weight, italic: s.italic, stretch: s.stretch }
}

fn advancer_of(a: &Adv) -> crate::fonts::lines::Advancer {
    crate::fonts::lines::Advancer::new(sel_of(a.sel), a.size, a.letter_spacing)
        .with_word_spacing(a.word_spacing)
        .with_dir(a.rtl)
}

fn mode_of(m: &TextMode) -> crate::fonts::lines::TextMode {
    crate::fonts::lines::TextMode {
        white_space: m.white_space,
        word_break: m.word_break,
        overflow_wrap: m.overflow_wrap,
        letter_spacing: m.letter_spacing,
        word_spacing: m.word_spacing,
        rtl: m.rtl,
        lead: m.lead,
        trail: m.trail,
    }
}

impl layout_core::Fonts for HostFonts {
    fn has_face(&self, sel: FontSel) -> bool {
        crate::fonts::face(&sel_of(sel)).is_some()
    }
    fn measure(&self, run: &str, adv: &Adv) -> f32 {
        advancer_of(adv).str(run)
    }
    fn line_metrics(&self, sel: FontSel, size: f32) -> (f32, f32, f32) {
        match crate::fonts::face(&sel_of(sel)) {
            Some(f) => crate::fonts::line_metrics(f, size),
            None => (size * 0.8, size * 0.2, 0.0),
        }
    }
    fn x_height(&self, sel: FontSel, size: f32) -> f32 {
        match crate::fonts::face(&sel_of(sel)) {
            Some(f) => {
                let m = f.metrics();
                m.x_height * size / m.units_per_em as f32
            }
            None => size * 0.5,
        }
    }
    fn fallback_metrics(&self, run: &str, adv: &Adv) -> Vec<(f32, f32, f32)> {
        let Some(primary) = crate::fonts::face(&sel_of(adv.sel)) else { return Vec::new() };
        let shaped = advancer_of(adv).shape(run);
        let mut seen: Vec<u32> = vec![primary.id];
        let mut out = Vec::new();
        for g in &shaped.glyphs {
            if seen.contains(&g.face.id) {
                continue;
            }
            seen.push(g.face.id);
            out.push(crate::fonts::line_metrics(g.face, adv.size));
        }
        out
    }
    fn line_height(&self, sel: FontSel, size: f32, mult: f32) -> f32 {
        match crate::fonts::face(&sel_of(sel)) {
            Some(f) => crate::fonts::line_height(f, size, mult),
            None if mult < 0.0 => -mult,
            None if mult > 0.0 => size * mult,
            None => size,
        }
    }
    fn baseline_offset(&self, sel: FontSel, size: f32, mult: f32) -> f32 {
        match crate::fonts::face(&sel_of(sel)) {
            Some(f) => crate::fonts::baseline_offset(f, size, mult),
            None => size * 0.8,
        }
    }
    fn break_lines(&self, text: &str, mode: &TextMode, adv: &Adv, max_w: f32, start_x: f32, prior: bool) -> Vec<Line> {
        crate::fonts::lines::break_lines_from(&advancer_of(adv), text, &mode_of(mode), max_w, start_x, prior)
            .into_iter()
            .map(|l| Line { text: l.text, width: l.width })
            .collect()
    }
}
