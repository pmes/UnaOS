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
        crate::media::intrinsic_size(node)
    }
    fn img_intrinsic(node: &NodeRef) -> Option<(f32, f32)> {
        let el = node.as_element()?;
        let attrs = el.attributes.borrow();
        crate::images::effective_img_src(&attrs)
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
