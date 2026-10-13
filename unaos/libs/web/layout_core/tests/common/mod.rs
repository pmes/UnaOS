//! AETHERSTYLE3 (SR103): the fixture harness layout_core's feature tests render through — a small arena DOM
//! that is both a `layout_core::DomNode` and a `css_core::matching::Element`, a fixed-advance `Fonts` (every
//! character is half its font size wide; ascent 0.8 em, descent 0.2 em, no line gap), and a `Host` with the
//! core's defaults. A fixture is built in code (`el` / `text`), styled by `style=""` (the build-time pass) or by
//! a stylesheet through css_core's cascade (`cascade_sheet`), and laid out by `layout_core::layout`.
#![allow(dead_code)]

use std::rc::Rc;

use css_core::matching::Element;
use layout_core::css::SpecifiedStyle;
use layout_core::{Adv, ComputedStyles, Dom, DomNode, FontSel, Fonts, Host, LayoutTree, NodeKind};

/// One node of the fixture document.
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// The fixture document: node 0 is the Document.
#[derive(Debug, Default)]
pub struct Arena {
    pub nodes: Vec<Node>,
}

/// A fixture element or text node before it is placed in the arena.
pub enum Spec {
    El(&'static str, Vec<(&'static str, String)>, Vec<Spec>),
    Text(String),
}

pub fn el(tag: &'static str, attrs: &[(&'static str, &str)], kids: Vec<Spec>) -> Spec {
    Spec::El(tag, attrs.iter().map(|(k, v)| (*k, v.to_string())).collect(), kids)
}

pub fn text(t: &str) -> Spec {
    Spec::Text(t.to_string())
}

/// A handle: the shared arena and a node index.
#[derive(Clone, Debug)]
pub struct N {
    pub doc: Rc<Arena>,
    pub id: usize,
}

impl PartialEq for N {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.doc, &o.doc) && self.id == o.id
    }
}

fn add(a: &mut Arena, s: Spec, parent: usize) -> usize {
    let i = a.nodes.len();
    match s {
        Spec::Text(t) => {
            a.nodes.push(Node { kind: NodeKind::Text, tag: String::new(), attrs: Vec::new(), text: t, parent: Some(parent), children: Vec::new() });
        }
        Spec::El(tag, attrs, kids) => {
            a.nodes.push(Node {
                kind: NodeKind::Element,
                tag: tag.to_string(),
                attrs: attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
                text: String::new(),
                parent: Some(parent),
                children: Vec::new(),
            });
            for k in kids {
                let c = add(a, k, i);
                a.nodes[i].children.push(c);
            }
        }
    }
    i
}

/// `<html><body style="margin:0">{body}</body></html>` as a fixture document.
pub fn page(body: Vec<Spec>) -> N {
    let mut a = Arena::default();
    a.nodes.push(Node { kind: NodeKind::Document, tag: String::new(), attrs: Vec::new(), text: String::new(), parent: None, children: Vec::new() });
    let html = add(&mut a, el("html", &[], vec![el("body", &[("style", "margin: 0")], body)]), 0);
    a.nodes[0].children.push(html);
    N { doc: Rc::new(a), id: 0 }
}

impl N {
    fn node(&self) -> &Node {
        &self.doc.nodes[self.id]
    }
    fn at(&self, id: usize) -> N {
        N { doc: self.doc.clone(), id }
    }
    /// The element whose `id` attribute is `id`.
    pub fn by_id(&self, id: &str) -> N {
        let i = self.doc.nodes.iter().position(|n| n.attrs.iter().any(|(k, v)| k == "id" && v == id)).expect("fixture id");
        self.at(i)
    }
    fn element_kids(&self) -> Vec<N> {
        self.node().children.iter().filter(|&&c| self.doc.nodes[c].kind == NodeKind::Element).map(|&c| self.at(c)).collect()
    }
}

impl DomNode for N {
    fn kind(&self) -> NodeKind {
        self.node().kind
    }
    fn tag(&self) -> Option<String> {
        (self.node().kind == NodeKind::Element).then(|| self.node().tag.clone())
    }
    fn attr(&self, name: &str) -> Option<String> {
        self.node().attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }
    fn text(&self) -> String {
        if self.node().kind == NodeKind::Text {
            return self.node().text.clone();
        }
        self.node().children.iter().map(|&c| DomNode::text(&self.at(c))).collect()
    }
    fn children(&self) -> Vec<Self> {
        self.node().children.iter().map(|&c| self.at(c)).collect()
    }
    fn parent(&self) -> Option<Self> {
        self.node().parent.map(|p| self.at(p))
    }
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn doctype(&self) -> Option<(String, String, String)> {
        None
    }
}

impl Element for N {
    fn local_name(&self) -> &str {
        &self.node().tag
    }
    fn namespace_url(&self) -> &str {
        css_core::selectors::HTML_NS
    }
    fn each_attr(&self, f: &mut dyn FnMut(&str, &str, &str) -> bool) -> bool {
        self.node().attrs.iter().any(|(k, v)| f("", k, v))
    }
    fn parent_element(&self) -> Option<Self> {
        let p = self.node().parent?;
        (self.doc.nodes[p].kind == NodeKind::Element).then(|| self.at(p))
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        let p = self.parent()?;
        let kids = p.element_kids();
        let i = kids.iter().position(|k| k == self)?;
        i.checked_sub(1).map(|j| kids[j].clone())
    }
    fn next_sibling_element(&self) -> Option<Self> {
        let p = self.parent()?;
        let kids = p.element_kids();
        let i = kids.iter().position(|k| k == self)?;
        kids.get(i + 1).cloned()
    }
    fn first_child_element(&self) -> Option<Self> {
        self.element_kids().into_iter().next()
    }
    fn is_empty(&self) -> bool {
        self.node().children.is_empty()
    }
}

/// The fixture document as `layout_core::Dom`.
pub struct D(pub N);

impl Dom for D {
    type Node = N;
    fn root(&self) -> N {
        self.0.clone()
    }
}

/// The test ring's hooks: the core's defaults.
#[derive(Clone, Debug, Default)]
pub struct TH;

impl Host for TH {
    type Node = N;
    type Gradient = ();
    type Shadow = ();
}

pub type Tree = LayoutTree<TH>;

/// Fixed-advance fonts: a character is half the font size wide.
pub struct F;

impl Fonts for F {
    fn has_face(&self, _sel: FontSel) -> bool {
        true
    }
    fn measure(&self, run: &str, adv: &Adv) -> f32 {
        let n = run.chars().count() as f32;
        n * (adv.size * 0.5 + adv.letter_spacing) + run.chars().filter(|c| *c == ' ').count() as f32 * adv.word_spacing
    }
    fn line_metrics(&self, _sel: FontSel, size: f32) -> (f32, f32, f32) {
        ((size * 0.8).round(), (size * 0.2).round(), 0.0)
    }
    fn x_height(&self, _sel: FontSel, size: f32) -> f32 {
        size * 0.5
    }
}

/// No stylesheet: only `style=""` applies (folded at build time).
pub struct NoSheet;

impl ComputedStyles<TH> for NoSheet {
    fn style_of(&self, _node: &N) -> Option<Rc<SpecifiedStyle<TH>>> {
        None
    }
}

/// A computed-style table: each element's winning declarations folded (the host's cascade step).
pub struct Sheet(pub Vec<(usize, Rc<SpecifiedStyle<TH>>)>);

impl ComputedStyles<TH> for Sheet {
    fn style_of(&self, node: &N) -> Option<Rc<SpecifiedStyle<TH>>> {
        self.0.iter().find(|(i, _)| *i == node.id).map(|(_, s)| s.clone())
    }
}

/// Lays the fixture out at `vw` x `vh` with `style=""` only.
pub fn lay(doc: &N, vw: f32, vh: f32) -> Tree {
    layout_core::layout(&D(doc.clone()), &NoSheet, &mut F, (vw, vh))
}

/// Lays the fixture out with a computed-style table.
pub fn lay_styled(doc: &N, sheet: &Sheet, vw: f32, vh: f32) -> Tree {
    layout_core::layout(&D(doc.clone()), sheet, &mut F, (vw, vh))
}

/// The laid-out border box of `node` in page coordinates: (x, y, w, h).
pub fn box_of(tree: &Tree, node: &N) -> (f32, f32, f32, f32) {
    let (id, _) = tree.node_map.iter().find(|(_, n)| n.id == node.id).expect("node has a box");
    let l = tree.taffy.layout(*id).unwrap();
    let (mut x, mut y) = (l.location.x, l.location.y);
    let mut cur = *id;
    while let Some(p) = tree.taffy.parent(cur) {
        let pl = tree.taffy.layout(p).unwrap();
        x += pl.location.x;
        y += pl.location.y;
        cur = p;
    }
    (x, y, l.size.width, l.size.height)
}

/// The taffy id of `node`'s box.
pub fn id_of(tree: &Tree, node: &N) -> layout_core::taffy::NodeId {
    *tree.node_map.iter().find(|(_, n)| n.id == node.id).expect("node has a box").0
}
