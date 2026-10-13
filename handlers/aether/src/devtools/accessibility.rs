//! The Accessibility panel: the page's accessibility tree (roles, names, states) and, for the node the
//! Elements panel selected, its computed role, name and the step the name came from, description, value,
//! states and its place in the Tab order.
//!
//! Seam: `a11y::tree` — THE read API the GTK face (AT-SPI later) reads too — and `a11y::focus::order` for
//! the Tab sequence. Nothing is cached: each call reads the tree as it stands.

use crate::dom::a11y::{self, focus, name::NameFrom, tree::Rendering};
use crate::AetherEngine;

/// Rows [`rows`] lists before it stops.
pub const MAX_ROWS: usize = 5000;

/// One row of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct AxRow {
    /// The DOM node's arena id (what the Elements panel selects).
    pub node: usize,
    pub depth: usize,
    pub role: &'static str,
    pub name: String,
    /// The set states, space-separated (`focusable checked=true …`).
    pub states: String,
}

impl AxRow {
    /// The row as the pane's text line: `role "name" states`.
    pub fn line(&self) -> String {
        let mut s = format!("{}{}", "  ".repeat(self.depth), self.role);
        if !self.name.is_empty() {
            s.push_str(&format!(" \"{}\"", self.name));
        }
        if !self.states.is_empty() {
            s.push(' ');
            s.push_str(&self.states);
        }
        s
    }
}

/// The selected node, inspected.
#[derive(Debug, Clone, PartialEq)]
pub struct AxInspection {
    pub node: usize,
    pub role: &'static str,
    pub name: String,
    pub name_from: NameFrom,
    pub description: String,
    pub value: Option<String>,
    pub level: Option<u32>,
    pub states: Vec<String>,
    /// 1-based place in the sequential focus order; None when it is not a Tab stop.
    pub tab_stop: Option<usize>,
    /// Not in the accessibility tree (hidden or flattened): the panel says why.
    pub ignored: Option<&'static str>,
}

/// The tree as rows, depth-first, at most `max`.
pub fn rows(engine: &AetherEngine, max: usize) -> Vec<AxRow> {
    let tree = a11y::tree(engine);
    tree.walk()
        .into_iter()
        .take(max)
        .map(|(i, depth)| {
            let n = &tree.nodes[i];
            AxRow { node: n.node, depth, role: n.role, name: n.name.clone(), states: n.states.tokens().join(" ") }
        })
        .collect()
}

/// The accessibility view of DOM node `id` (None for an id not in the document).
pub fn inspect(engine: &AetherEngine, id: usize) -> Option<AxInspection> {
    let node = super::elements::node(engine, id)?;
    let tree = a11y::tree(engine);
    let r = Rendering::of(engine);
    let doc = engine.document.as_ref()?.document_node();
    let tab_stop = focus::order(&doc, &r).iter().position(|n| *n == node).map(|p| p + 1);
    if let Some(n) = tree.find_dom(id) {
        return Some(AxInspection {
            node: id,
            role: n.role,
            name: n.name.clone(),
            name_from: n.name_from,
            description: n.description.clone(),
            value: n.value.clone(),
            level: n.level,
            states: n.states.tokens(),
            tab_stop,
            ignored: None,
        });
    }
    let ignored = if r.hidden(&node) { "hidden (not rendered, visibility, aria-hidden or inert)" } else { "generic, unnamed: its children join its parent" };
    let role = a11y::tree::role_of(&node, &r);
    Some(AxInspection {
        node: id,
        role,
        name: String::new(),
        name_from: NameFrom::None,
        description: String::new(),
        value: None,
        level: None,
        states: Vec::new(),
        tab_stop,
        ignored: Some(ignored),
    })
}

/// The panel as text lines: the tree, then the selected node's properties.
pub fn lines(engine: &AetherEngine, selected: Option<usize>) -> Vec<String> {
    let mut out: Vec<String> = rows(engine, MAX_ROWS).iter().map(AxRow::line).collect();
    if let Some(ins) = selected.and_then(|id| inspect(engine, id)) {
        out.push(String::new());
        out.push(format!("role: {}", ins.role));
        if let Some(why) = ins.ignored {
            out.push(format!("ignored: {why}"));
        }
        out.push(format!("name: \"{}\"{}", ins.name, if ins.name_from == NameFrom::None { String::new() } else { format!(" (from {})", ins.name_from.label()) }));
        if !ins.description.is_empty() {
            out.push(format!("description: \"{}\"", ins.description));
        }
        if let Some(v) = &ins.value {
            out.push(format!("value: \"{v}\""));
        }
        if let Some(l) = ins.level {
            out.push(format!("level: {l}"));
        }
        if !ins.states.is_empty() {
            out.push(format!("states: {}", ins.states.join(" ")));
        }
        match ins.tab_stop {
            Some(p) => out.push(format!("tab order: {p}")),
            None => out.push("tab order: not a stop".to_string()),
        }
    }
    out
}
