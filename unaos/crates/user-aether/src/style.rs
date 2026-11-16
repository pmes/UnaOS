// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL (SR66) M2, LAYOUTCORE (SR76): the document and the cascade, in the host engine's shape. Layout is
// `layout_core` — the SAME engine handlers/aether lays out with — so this file gives it what it reads:
//
// * the DOM seam: [`N`] (html_core's arena + a NodeId; the document is leaked to `'static`, one page per run)
//   implements `layout_core::DomNode`, [`Doc`] implements `layout_core::Dom`;
// * the ring hooks: [`ImgHost`] implements `layout_core::Host` with the core's defaults (no ledger, no
//   gradient/shadow painter, family lists classified into sans/serif/mono, no images or media yet);
// * the cascade: css_core's matcher over the arena through the `Element` impl below, the page's `<style>`
//   sheets (author origin) and each element's `style=""` sorted by `css_core::cascade::cascade`, custom
//   properties computed per element and `var()` substituted, and every winning declaration handed as CSS text to
//   `layout_core::css::apply_declaration` — the host's computed-value step, verbatim. There is NO UA sheet: the
//   UA defaults (block margins, heading sizes, list padding, bold/italic/mono tags) are layout_core's box
//   builder's tag defaults, as on the host.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use css_core::cascade::{cascade, Conditions, Origin, RuleSet};
use css_core::matching::Element;
use css_core::media::Environment;
use css_core::stylesheet::{parse_style_attribute, parse_stylesheet, Stylesheet};
use css_core::values::{compute_custom_properties, contains_var, substitute_vars};
use css_core::{Declaration, CV};
use html_core::{Document, NodeData, NodeId};
use layout_core::css::{apply_declaration, ParseCx, SpecifiedStyle};
use layout_core::{ComputedStyles, DomNode, NodeKind};

// ---- the DOM seam -----------------------------------------------------------------------------------------

/// One node of the page: the (leaked, `'static`) arena and its id.
#[derive(Clone, Copy)]
pub struct N {
    pub doc: &'static Document,
    pub id: NodeId,
}

impl DomNode for N {
    fn kind(&self) -> NodeKind {
        match self.doc.data(self.id) {
            NodeData::Element(_) => NodeKind::Element,
            NodeData::Text(_) => NodeKind::Text,
            NodeData::Comment(_) => NodeKind::Comment,
            NodeData::Doctype { .. } => NodeKind::Doctype,
            NodeData::Document => NodeKind::Document,
            _ => NodeKind::Other,
        }
    }
    fn tag(&self) -> Option<String> {
        self.doc.element(self.id).map(|e| e.local.clone())
    }
    fn attr(&self, name: &str) -> Option<String> {
        self.doc.element(self.id).and_then(|e| e.attr(name)).map(String::from)
    }
    // AETHERSTYLE3C (SR122): the element's attributes in source order (a foreign attribute under its prefix,
    // `xlink:href`), so `layout_core::effects::resolve_filter` can serialize a referenced SVG `<filter>` for
    // svg_core — `filter: url(#id)` resolves on this ring as on the host.
    fn attrs(&self) -> Vec<(String, String)> {
        let Some(e) = self.doc.element(self.id) else { return Vec::new() };
        e.attrs
            .iter()
            .map(|a| {
                let name = match a.prefix {
                    Some(p) => alloc::format!("{}:{}", p, a.local),
                    None => a.local.clone(),
                };
                (name, a.value.clone())
            })
            .collect()
    }
    fn text(&self) -> String {
        match self.doc.data(self.id) {
            NodeData::Text(t) => t.clone(),
            _ => self.doc.text_content(self.id),
        }
    }
    fn children(&self) -> Vec<N> {
        self.doc.children(self.id).map(|c| N { doc: self.doc, id: c }).collect()
    }
    fn parent(&self) -> Option<N> {
        self.doc.parent(self.id).map(|p| N { doc: self.doc, id: p })
    }
    fn same(&self, other: &N) -> bool {
        self.id == other.id
    }
    fn doctype(&self) -> Option<(String, String, String)> {
        match self.doc.data(self.id) {
            NodeData::Doctype { name, public_id, system_id } => Some((name.clone(), public_id.clone(), system_id.clone())),
            _ => None,
        }
    }
}

/// The page as `layout_core::Dom`: layout starts at the Document node.
pub struct Doc(pub &'static Document);

impl layout_core::Dom for Doc {
    type Node = N;
    fn root(&self) -> N {
        N { doc: self.0, id: Document::ROOT }
    }
}

/// This ring's `layout_core::Host`: the core's defaults (no gradient or shadow painter here yet).
#[derive(Clone, Copy, Debug, Default)]
pub struct ImgHost;

impl layout_core::Host for ImgHost {
    type Node = N;
    type Gradient = ();
    type Shadow = ();
    fn img_intrinsic(node: &N) -> Option<(f32, f32)> { with_image(node, |i| (i.width as f32, i.height as f32)) } // AETHERIMAGEINPUTELF (SR683): an image input's decoded size, layout_core's img rule
}

/// The box tree of the page.
pub type Tree = layout_core::LayoutTree<ImgHost>;
pub type Spec = SpecifiedStyle<ImgHost>;

// ---- css_core's Element over html_core's arena ------------------------------------------------------------

#[derive(Clone)]
pub struct El<'d> {
    pub doc: &'d Document,
    pub id: NodeId,
}

impl PartialEq for El<'_> {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}

impl<'d> El<'d> {
    fn at(&self, id: NodeId) -> El<'d> {
        El { doc: self.doc, id }
    }
}

impl Element for El<'_> {
    fn local_name(&self) -> &str {
        self.doc.element(self.id).map_or("", |e| e.local.as_str())
    }
    fn namespace_url(&self) -> &str {
        self.doc.element(self.id).map_or("", |e| e.ns.url())
    }
    fn each_attr(&self, f: &mut dyn FnMut(&str, &str, &str) -> bool) -> bool {
        if let Some(e) = self.doc.element(self.id) {
            for a in &e.attrs {
                if f(a.ns.url(), &a.local, &a.value) {
                    return true;
                }
            }
        }
        false
    }
    fn parent_element(&self) -> Option<Self> {
        self.doc.parent_element(self.id).map(|p| self.at(p))
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.doc.prev_sibling_element(self.id).map(|p| self.at(p))
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.doc.next_sibling_element(self.id).map(|p| self.at(p))
    }
    fn first_child_element(&self) -> Option<Self> {
        self.doc.element_children(self.id).next().map(|p| self.at(p))
    }
    fn is_empty(&self) -> bool {
        self.doc.children(self.id).all(|c| match self.doc.data(c) {
            NodeData::Element(_) => false,
            NodeData::Text(t) => t.is_empty(),
            _ => true,
        })
    }
}

// ---- the cascade over the document ------------------------------------------------------------------------

/// The cascaded style of each element (by node index) — `layout_core`'s `ComputedStyles`.
pub struct Styled {
    pub by_node: BTreeMap<usize, Rc<Spec>>,
    pub rules: usize,
    pub elements: usize,
}

impl ComputedStyles<ImgHost> for Styled {
    fn style_of(&self, node: &N) -> Option<Rc<Spec>> {
        self.by_node.get(&node.id.0).cloned()
    }
}

/// A declaration's value as CSS text for the computed-value step (the host's `value_text`).
fn value_text(v: &[CV]) -> String {
    css_core::serialize::to_css(css_core::parser::trim_ws(v))
}

/// Cascade every element of `doc` against the viewport `vw` x `vh` (media queries, `vw`/`vh` lengths).
pub fn compute(doc: &Document, vw: f32, vh: f32) -> Styled {
    // Every sheet first (the RuleSet borrows them): each <style> in document order, author origin.
    let mut sheets: Vec<Stylesheet> = Vec::new();
    for n in doc.descendants(Document::ROOT) {
        if let Some(e) = doc.element(n) {
            if e.is_html("style") {
                sheets.push(parse_stylesheet(&doc.text_content(n)));
            }
        }
    }
    let env = Environment::viewport(vw as f64, vh as f64);
    let supports = |_: &Declaration| true;
    let import = |_: &str| -> Option<&Stylesheet> { None };
    let cond = Conditions { env: &env, supports: &supports, import: &import };
    let mut rules = RuleSet::new();
    for s in sheets.iter() {
        rules.add_sheet(s, Origin::Author, &cond);
    }
    rules.build_index();
    let cx = ParseCx::new(vw, vh);

    type Customs = Rc<BTreeMap<String, Vec<CV>>>;
    let empty: Customs = Rc::new(BTreeMap::new());
    let mut customs: BTreeMap<usize, Customs> = BTreeMap::new();
    let mut out = Styled { by_node: BTreeMap::new(), rules: rules.entries.len(), elements: 0 };
    for id in doc.descendants(Document::ROOT) {
        let Some(e) = doc.element(id) else { continue };
        out.elements += 1;
        let inline: Vec<Declaration> = e.attr("style").map(parse_style_attribute).unwrap_or_default();
        let el = El { doc, id };
        let applied = cascade(&rules, &el, &inline, None);
        // custom properties: inherited from the parent element, overridden by this element's own
        let parent = doc.parent_element(id).and_then(|p| customs.get(&p.0)).cloned().unwrap_or_else(|| empty.clone());
        let own: Vec<(String, Vec<CV>)> = applied
            .iter()
            .filter(|a| a.declaration.name.starts_with("--"))
            .map(|a| (a.declaration.name.clone(), a.declaration.value.clone()))
            .collect();
        let mine = if own.is_empty() { parent } else { Rc::new(compute_custom_properties(&own, &parent)) };
        let mut s = Spec::default();
        let mut any = false;
        for a in applied.iter().filter(|a| !a.declaration.name.starts_with("--")) {
            let d = a.declaration;
            let name = d.name.to_ascii_lowercase();
            any = true;
            if contains_var(&d.value) {
                if let Ok(v) = substitute_vars(&d.value, &|n| mine.get(n).cloned()) {
                    apply_declaration(&name, &value_text(&v), &mut s, &cx);
                }
            } else {
                apply_declaration(&name, &value_text(&d.value), &mut s, &cx);
            }
        }
        if any {
            out.by_node.insert(id.0, Rc::new(s));
        }
        customs.insert(id.0, mine);
    }
    out
}

// ---- AETHERIMAGEINPUTELF (LEDGER SR683): the page's decoded image-input bitmaps (named tail block) ----

/// AETHERIMAGEINPUTELF (SR683): the open page's decoded image-input bitmaps by html_core node index, and the address of
/// the document they belong to (a tab's other page never reads them). ONE writer, main.rs `image_inputs` (once per page
/// open, before `lay`, replacing the map so the old page's bitmaps drop); readers `ImgHost::img_intrinsic` (layout_core's
/// img sizing rule) and paint.rs `image_blit`, through [`with_image`] only. Single-threaded program: net.rs `NET`'s idiom.
static mut IMAGES: (usize, BTreeMap<usize, pixel_core::Image>) = (0, BTreeMap::new());

/// AETHERIMAGEINPUTELF (SR683): the bitmaps of `doc`'s image inputs replace the store (an empty map on a page with none).
pub fn set_images(doc: &Document, m: BTreeMap<usize, pixel_core::Image>) {
    // SAFETY: one thread; no reference into the store outlives a `with_image` call.
    unsafe { *core::ptr::addr_of_mut!(IMAGES) = (doc as *const Document as usize, m) };
}

/// AETHERIMAGEINPUTELF (SR683): `f` over `node`'s decoded bitmap, when the store holds one for its document.
pub fn with_image<R>(node: &N, f: impl FnOnce(&pixel_core::Image) -> R) -> Option<R> {
    // SAFETY: one thread; the shared borrow ends with this call and no writer runs inside `f`.
    let store = unsafe { &*core::ptr::addr_of!(IMAGES) };
    if store.0 != node.doc as *const Document as usize {
        return None;
    }
    store.1.get(&node.id.0).map(f)
}
