//! The shell's reading verbs: skip to content, next / previous heading, next / previous landmark.
//!
//! Each reads [`super::tree`] (headings and landmarks as the accessibility tree has them, hidden ones
//! already pruned) and moves the reading position: a focusable target takes keyboard focus; any other
//! becomes the sequential focus navigation starting point (HTML §6.6.3 — the next Tab continues from it)
//! with focus cleared, as a fragment navigation to a non-focusable element does. Either way the target
//! is scrolled into view.

use std::collections::HashMap;

use super::focus;
use super::tree::{AxNode, AxTree, Rendering};
use crate::dom::NodeRef;
use crate::AetherEngine;

/// A heading row: its DOM id, level and name.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub node: usize,
    pub level: u32,
    pub name: String,
}

/// A landmark row: its DOM id, role and name.
#[derive(Debug, Clone, PartialEq)]
pub struct Landmark {
    pub node: usize,
    pub role: &'static str,
    pub name: String,
}

pub fn headings(engine: &AetherEngine) -> Vec<Heading> {
    super::tree(engine).headings().into_iter().map(|h| Heading { node: h.node, level: h.level.unwrap_or(2), name: h.name.clone() }).collect()
}

pub fn landmarks(engine: &AetherEngine) -> Vec<Landmark> {
    super::tree(engine).landmarks().into_iter().map(|l| Landmark { node: l.node, role: l.role, name: l.name.clone() }).collect()
}

/// The DOM node with arena id `id` in the engine's document.
fn dom_node(engine: &AetherEngine, id: usize) -> Option<NodeRef> {
    let doc = engine.document.as_ref()?.document_node();
    doc.inclusive_descendants().find(|n| n.id().0 == id)
}

/// Moves the reading position to `target` (see the module doc). Returns its arena id.
pub fn go_to(engine: &mut AetherEngine, target: &NodeRef) -> usize {
    let r = Rendering::of(engine);
    if focus::is_focusable(target, &r) {
        focus::focus(engine, Some(target.clone()), true);
    } else {
        focus::focus(engine, None, true);
        focus::set_starting_point(Some(target.clone()));
        focus::scroll_into_view(engine, target);
    }
    target.id().0
}

/// Document-order positions of every node (to compare the reading position with tree nodes).
fn positions(engine: &AetherEngine) -> HashMap<usize, usize> {
    match engine.document.as_ref() {
        Some(d) => d.document_node().inclusive_descendants().enumerate().map(|(i, n)| (n.id().0, i)).collect(),
        None => HashMap::new(),
    }
}

/// The first of `nodes` after (or, `backward`, the last before) the reading position; with no position,
/// the first (last). Wraps around.
fn pick(engine: &AetherEngine, nodes: &[&AxNode], backward: bool) -> Option<usize> {
    if nodes.is_empty() {
        return None;
    }
    let pos = positions(engine);
    let here = focus::position(engine).and_then(|n| pos.get(&n.id().0).copied());
    let at = |n: &&AxNode| pos.get(&n.node).copied().unwrap_or(0);
    let found = match (here, backward) {
        (None, false) => nodes.first(),
        (None, true) => nodes.last(),
        (Some(h), false) => nodes.iter().find(|n| at(n) > h).or(nodes.first()),
        (Some(h), true) => nodes.iter().rev().find(|n| at(n) < h).or(nodes.last()),
    };
    found.map(|n| n.node)
}

/// Next (or previous) heading, of `level` when given. Returns the heading's arena id.
pub fn next_heading(engine: &mut AetherEngine, level: Option<u32>, backward: bool) -> Option<usize> {
    let tree = super::tree(engine);
    let hs: Vec<&AxNode> = tree.headings().into_iter().filter(|h| level.is_none_or(|l| h.level == Some(l))).collect();
    let id = pick(engine, &hs, backward)?;
    let n = dom_node(engine, id)?;
    Some(go_to(engine, &n))
}

/// Next (or previous) landmark.
pub fn next_landmark(engine: &mut AetherEngine, backward: bool) -> Option<usize> {
    let tree = super::tree(engine);
    let ls = tree.landmarks();
    let id = pick(engine, &ls, backward)?;
    let n = dom_node(engine, id)?;
    Some(go_to(engine, &n))
}

/// The page's own skip link: an in-page link (`href="#id"`) among the first sequential stops whose name
/// says it skips (`skip`, `jump to`, `main content`), resolved to its target.
fn skip_link_target(engine: &AetherEngine, tree: &AxTree) -> Option<NodeRef> {
    let doc = engine.document.as_ref()?.document_node();
    let r = Rendering::of(engine);
    for n in focus::order(&doc, &r).into_iter().take(3) {
        if super::tag(&n) != "a" {
            continue;
        }
        let Some(frag) = super::attr(&n, "href").and_then(|h| h.strip_prefix('#').map(str::to_string)) else { continue };
        let name = tree.find_dom(n.id().0).map(|a| a.name.to_ascii_lowercase()).unwrap_or_default();
        if name.contains("skip") || name.contains("jump to") || name.contains("main content") {
            if let Some(t) = super::by_id(&n, &frag) {
                return Some(t);
            }
        }
    }
    None
}

/// Skip to content: the `main` landmark, else the target of the page's own skip link, else the first
/// level-1 heading, else the first heading. Returns the target's arena id.
pub fn skip_to_content(engine: &mut AetherEngine) -> Option<usize> {
    let tree = super::tree(engine);
    let id = tree
        .landmarks()
        .into_iter()
        .find(|l| l.role == "main")
        .map(|m| m.node)
        .or_else(|| skip_link_target(engine, &tree).map(|t| t.id().0))
        .or_else(|| tree.headings().into_iter().find(|h| h.level == Some(1)).map(|h| h.node))
        .or_else(|| tree.headings().first().map(|h| h.node))?;
    let n = dom_node(engine, id)?;
    Some(go_to(engine, &n))
}

/// A11YFACE (SR124): runs a bandy accessibility verb ([`bandy::A11yVerb`], the shell's `BrowserA11y`) on the
/// engine's page and answers its lines — `a11y tree`: the tree's; a move: the one line of the node the reading
/// position moved to ([`AxTree::line`]), empty when there was nothing to move to.
pub fn run(engine: &mut AetherEngine, verb: &bandy::A11yVerb) -> Vec<String> {
    use bandy::A11yVerb as V;
    let moved = match *verb {
        V::Tree => return super::tree(engine).lines(),
        V::Skip => skip_to_content(engine),
        V::NextHeading { backward, level } => next_heading(engine, level, backward),
        V::NextLandmark { backward } => next_landmark(engine, backward),
    };
    let Some(id) = moved else { return Vec::new() };
    super::tree(engine).find_dom(id).map(|n| vec![AxTree::line(n)]).unwrap_or_default()
}
