//! The accessibility tree: the DOM seen through computed style and the role and name rules.
//!
//! What is left out (WAI-ARIA 1.2 §7.1 "Excluding Elements from the Accessibility Tree", HTML-AAM §5):
//! a node that is not rendered (`display: none`, the `hidden` attribute through the UA sheet: no box in
//! `css::computed_report`), `aria-hidden="true"` and its subtree, an `inert` subtree, the metadata elements
//! (`<head>`, `<script>`, `<style>`, `<template>`). A `visibility: hidden` / `collapse` element is left out
//! but its `visibility: visible` descendants are kept (visibility inherits and may be overridden). Generic
//! and presentational elements that are neither focusable nor named are flattened (their children join the
//! parent), and a role whose children are presentational folds its subtree into itself.

use std::collections::HashMap;

use super::name::{self, NameFrom};
use super::{aria_bool, attr, flat, has_attr, role, tag};
use crate::dom::NodeRef;
use crate::AetherEngine;
use html_core::NodeId;

/// How the page was rendered, as the a11y rules need it: per element, its computed `display` and
/// `visibility` (`css::computed_report`, the values layout and paint consumed). Without a layout tree the
/// DOM alone decides (the `hidden` attribute and the never-rendered metadata elements).
pub struct Rendering {
    report: Option<HashMap<NodeId, [String; 20]>>,
}

/// `css::REPORT_PROPS` indexes.
const DISPLAY: usize = 0;
const VISIBILITY: usize = 12;

/// Elements that never generate a box (HTML §15.3.1 `display: none` UA rules).
const NEVER_RENDERED: [&str; 12] = ["head", "script", "style", "template", "title", "meta", "link", "base", "noscript", "datalist", "param", "source"];

impl Rendering {
    /// The engine's current rendering.
    pub fn of(engine: &AetherEngine) -> Self {
        Rendering { report: engine.layout_tree.as_ref().map(crate::css::computed_report) }
    }

    /// No layout: the DOM-only rules.
    pub fn dom_only() -> Self {
        Rendering { report: None }
    }

    fn computed(&self, n: &NodeRef) -> Option<&[String; 20]> {
        self.report.as_ref()?.get(&n.id())
    }

    /// The computed `display` of an element with a box.
    pub fn display(&self, n: &NodeRef) -> Option<&str> {
        self.computed(n).map(|c| c[DISPLAY].as_str())
    }

    /// HTML §15.1 "being rendered": false for an element with no box. An element without a box whose
    /// descendants have boxes is `display: contents` and counts as rendered.
    pub fn not_rendered(&self, n: &NodeRef) -> bool {
        if !n.is_element() {
            return false;
        }
        if n.inclusive_ancestors_any(|a| NEVER_RENDERED.contains(&tag(a).as_str())) {
            return true;
        }
        match &self.report {
            Some(r) => {
                if r.contains_key(&n.id()) {
                    return false;
                }
                !n.descendants().any(|d| d.is_element() && r.contains_key(&d.id()))
            }
            None => n.inclusive_ancestors_any(|a| has_attr(a, "hidden")),
        }
    }

    /// `visibility: hidden | collapse`.
    pub fn visibility_hidden(&self, n: &NodeRef) -> bool {
        self.computed(n).is_some_and(|c| matches!(c[VISIBILITY].as_str(), "hidden" | "collapse"))
    }

    /// A block-level box (anything not inline-level), for name-from-content spacing.
    pub fn is_block(&self, n: &NodeRef) -> bool {
        match self.display(n) {
            Some(d) => !d.starts_with("inline") && d != "contents" && d != "none",
            None => matches!(tag(n).as_str(), "div" | "p" | "li" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "section" | "article" | "table" | "tr" | "td" | "th"),
        }
    }

    /// accname 1.2 "hidden": not rendered, `visibility: hidden`, or `aria-hidden="true"` on the node or an
    /// ancestor; an `inert` subtree is hidden from assistive technology too (HTML §6.3).
    pub fn hidden(&self, n: &NodeRef) -> bool {
        if !n.is_element() {
            return n.parent().is_some_and(|p| self.hidden(&p));
        }
        self.not_rendered(n)
            || self.visibility_hidden(n)
            || n.inclusive_ancestors_any(|a| aria_bool(a, "aria-hidden") == Some(true) || has_attr(a, "inert"))
    }
}

/// `n` and its ancestors.
trait AncestorsAny {
    fn inclusive_ancestors_any(&self, f: impl Fn(&NodeRef) -> bool) -> bool;
}

impl AncestorsAny for NodeRef {
    fn inclusive_ancestors_any(&self, f: impl Fn(&NodeRef) -> bool) -> bool {
        f(self) || self.ancestors().any(|a| f(&a))
    }
}

/// The computed role of `n` under rendering `r`.
pub(crate) fn role_of(n: &NodeRef, r: &Rendering) -> &'static str {
    if !n.is_element() {
        return if n.as_text().is_some() { role::TEXT } else { role::GENERIC };
    }
    let focusable = super::focus::is_focusable(n, r);
    role::computed_role(n, focusable, &|| author_named(n))
}

/// The element names itself through an author attribute (what makes `<section>` a region and `<svg>` an
/// image — HTML-AAM: "if the element has an accessible name").
fn author_named(n: &NodeRef) -> bool {
    attr(n, "aria-label").is_some_and(|v| !v.trim().is_empty())
        || super::id_refs(n, "aria-labelledby").iter().any(|t| !flat(&t.text_contents()).is_empty())
        || attr(n, "title").is_some_and(|v| !v.trim().is_empty())
        || (tag(n) == "svg" && n.children().any(|c| tag(&c) == "title"))
}

/// A three-valued ARIA state (`aria-checked`, `aria-pressed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tristate {
    False,
    True,
    Mixed,
}

impl Tristate {
    fn parse(v: &str) -> Option<Tristate> {
        match v.trim().to_ascii_lowercase().as_str() {
            "true" => Some(Tristate::True),
            "false" => Some(Tristate::False),
            "mixed" => Some(Tristate::Mixed),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Tristate::False => "false",
            Tristate::True => "true",
            Tristate::Mixed => "mixed",
        }
    }
}

/// The states and properties of a node (WAI-ARIA 1.2 §6.6/§6.7, from `aria-*` and the native semantics
/// HTML-AAM §3.5 maps onto them).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct States {
    pub focusable: bool,
    pub focused: bool,
    pub disabled: bool,
    pub required: bool,
    pub readonly: bool,
    pub multiline: bool,
    pub multiselectable: bool,
    pub invalid: bool,
    pub modal: bool,
    pub busy: bool,
    pub checked: Option<Tristate>,
    pub pressed: Option<Tristate>,
    pub expanded: Option<bool>,
    pub selected: Option<bool>,
    pub haspopup: Option<String>,
    pub current: Option<String>,
    pub live: Option<String>,
    pub autocomplete: Option<String>,
    pub orientation: Option<String>,
    pub value_min: Option<f64>,
    pub value_max: Option<f64>,
    pub value_now: Option<f64>,
}

impl States {
    /// The set states as `name` / `name=value` tokens, in a fixed order.
    pub fn tokens(&self) -> Vec<String> {
        let mut o = Vec::new();
        for (on, n) in [
            (self.focusable, "focusable"),
            (self.focused, "focused"),
            (self.disabled, "disabled"),
            (self.required, "required"),
            (self.readonly, "readonly"),
            (self.multiline, "multiline"),
            (self.multiselectable, "multiselectable"),
            (self.invalid, "invalid"),
            (self.modal, "modal"),
            (self.busy, "busy"),
        ] {
            if on {
                o.push(n.to_string());
            }
        }
        if let Some(c) = self.checked {
            o.push(format!("checked={}", c.label()));
        }
        if let Some(p) = self.pressed {
            o.push(format!("pressed={}", p.label()));
        }
        if let Some(e) = self.expanded {
            o.push(format!("expanded={e}"));
        }
        if let Some(s) = self.selected {
            o.push(format!("selected={s}"));
        }
        for (k, v) in [("haspopup", &self.haspopup), ("current", &self.current), ("live", &self.live), ("autocomplete", &self.autocomplete), ("orientation", &self.orientation)] {
            if let Some(v) = v {
                o.push(format!("{k}={v}"));
            }
        }
        for (k, v) in [("valuemin", self.value_min), ("valuemax", self.value_max), ("valuenow", self.value_now)] {
            if let Some(v) = v {
                o.push(format!("{k}={v}"));
            }
        }
        o
    }
}

/// One node of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct AxNode {
    /// The DOM node's arena id (the document's for the root; a text run's own id).
    pub node: usize,
    pub role: &'static str,
    pub name: String,
    pub name_from: NameFrom,
    pub description: String,
    /// The value: a text field's text, a combobox's selected option, a range's `aria-valuetext` / value.
    pub value: Option<String>,
    /// A heading's level (and `aria-level` on rows, tree items and list items).
    pub level: Option<u32>,
    pub states: States,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// The tree: `nodes[0]` is the root (role [`role::ROOT`], named by the document's `<title>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxTree {
    pub nodes: Vec<AxNode>,
}

impl AxTree {
    pub fn root(&self) -> Option<&AxNode> {
        self.nodes.first()
    }

    /// The tree node for DOM arena id `node`.
    pub fn find_dom(&self, node: usize) -> Option<&AxNode> {
        self.nodes.iter().find(|n| n.node == node)
    }

    /// The first node with `role` and `name`.
    pub fn find(&self, role: &str, name: &str) -> Option<&AxNode> {
        self.nodes.iter().find(|n| n.role == role && n.name == name)
    }

    /// Depth-first (document order) indices with their depth.
    pub fn walk(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if self.nodes.is_empty() {
            return out;
        }
        let mut stack = vec![(0usize, 0usize)];
        while let Some((i, d)) = stack.pop() {
            out.push((i, d));
            for &c in self.nodes[i].children.iter().rev() {
                stack.push((c, d + 1));
            }
        }
        out
    }

    /// The landmarks in document order (`form` and `region` only when named, WAI-ARIA 1.2 §5.3.4).
    pub fn landmarks(&self) -> Vec<&AxNode> {
        self.walk()
            .into_iter()
            .map(|(i, _)| &self.nodes[i])
            .filter(|n| role::is_landmark(n.role) && (!matches!(n.role, "form" | "region") || !n.name.is_empty()))
            .collect()
    }

    /// The headings in document order.
    pub fn headings(&self) -> Vec<&AxNode> {
        self.walk().into_iter().map(|(i, _)| &self.nodes[i]).filter(|n| n.role == "heading").collect()
    }

    /// One line per node, indented: `role "name" state…` — the plain-text form a face or test reads.
    pub fn lines(&self) -> Vec<String> {
        self.walk()
            .into_iter()
            .map(|(i, d)| {
                let n = &self.nodes[i];
                let mut s = format!("{}{}", "  ".repeat(d), n.role);
                if !n.name.is_empty() {
                    s.push_str(&format!(" \"{}\"", n.name));
                }
                if let Some(l) = n.level {
                    s.push_str(&format!(" level={l}"));
                }
                if let Some(v) = n.value.as_ref().filter(|v| !v.is_empty()) {
                    s.push_str(&format!(" value=\"{v}\""));
                }
                for t in n.states.tokens() {
                    s.push(' ');
                    s.push_str(&t);
                }
                s
            })
            .collect()
    }
}

/// Builds the tree of the engine's current document.
pub fn build(engine: &AetherEngine) -> AxTree {
    let Some(doc) = engine.document.clone() else { return AxTree::default() };
    let r = Rendering::of(engine);
    let focused = engine.focused_node.clone();
    build_with(&doc.document_node(), &r, focused.as_ref())
}

/// Builds the tree of `doc` under rendering `r` (`focused`: the element holding focus).
pub fn build_with(doc: &NodeRef, r: &Rendering, focused: Option<&NodeRef>) -> AxTree {
    let title = doc
        .descendants()
        .find(|n| tag(n) == "title")
        .map(|t| flat(&t.text_contents()))
        .unwrap_or_default();
    let mut tree = AxTree {
        nodes: vec![AxNode {
            node: doc.id().0,
            role: role::ROOT,
            name: title.clone(),
            name_from: if title.is_empty() { NameFrom::None } else { NameFrom::TitleElement },
            description: String::new(),
            value: None,
            level: None,
            states: States { focusable: true, focused: focused.is_none(), ..States::default() },
            parent: None,
            children: Vec::new(),
        }],
    };
    let b = Builder { r, focused };
    for c in doc.children() {
        b.walk(&c, 0, &mut tree, false);
    }
    tree
}

struct Builder<'a> {
    r: &'a Rendering,
    focused: Option<&'a NodeRef>,
}

impl Builder<'_> {
    fn push(&self, tree: &mut AxTree, parent: usize, node: AxNode) -> usize {
        let i = tree.nodes.len();
        tree.nodes.push(AxNode { parent: Some(parent), ..node });
        tree.nodes[parent].children.push(i);
        i
    }

    /// Adds `n` (and its subtree) under tree node `parent`. `in_select`: inside a `<select>`, whose options
    /// have no boxes while the popup is closed but are its accessible children.
    fn walk(&self, n: &NodeRef, parent: usize, tree: &mut AxTree, in_select: bool) {
        if let Some(t) = n.as_text() {
            let text = flat(&t.borrow());
            if text.is_empty() {
                return;
            }
            if n.parent().is_some_and(|p| p.is_element() && self.r.visibility_hidden(&p)) {
                return;
            }
            let node = AxNode {
                node: n.id().0,
                role: role::TEXT,
                name: text,
                name_from: NameFrom::Contents,
                description: String::new(),
                value: None,
                level: None,
                states: States::default(),
                parent: None,
                children: Vec::new(),
            };
            self.push(tree, parent, node);
            return;
        }
        if !n.is_element() {
            return;
        }
        let t = tag(n);
        // Excluded subtrees.
        if NEVER_RENDERED.contains(&t.as_str()) || aria_bool(n, "aria-hidden") == Some(true) || has_attr(n, "inert") {
            return;
        }
        let option_like = in_select && matches!(t.as_str(), "option" | "optgroup");
        if !option_like && self.r.not_rendered(n) {
            return;
        }
        let kids: Vec<NodeRef> = n.children().collect();
        // visibility: hidden — the element is out, its visible descendants stay (joined to the parent).
        if !option_like && self.r.visibility_hidden(n) {
            for k in &kids {
                if k.is_element() {
                    self.walk(k, parent, tree, in_select);
                }
            }
            return;
        }
        let role = role_of(n, self.r);
        let focusable = super::focus::is_focusable(n, self.r);
        let (name, name_from) = name::accessible_name(n, role, self.r);
        let flatten = (role == role::GENERIC || role::is_presentational(role)) && !focusable && name.is_empty();
        let sel = in_select || t == "select";
        if flatten {
            for k in &kids {
                self.walk(k, parent, tree, sel);
            }
            return;
        }
        let description = name::accessible_description(n, name_from, self.r);
        let node = AxNode {
            node: n.id().0,
            role,
            name,
            name_from,
            description,
            value: value_of(n, role),
            level: level_of(n, role),
            states: states_of(n, role, focusable, self.focused),
            parent: None,
            children: Vec::new(),
        };
        let me = self.push(tree, parent, node);
        if role::children_presentational(role) {
            return;
        }
        for k in &kids {
            self.walk(k, me, tree, sel);
        }
    }
}

fn num_attr(n: &NodeRef, name: &str) -> Option<f64> {
    attr(n, name).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// The value a node exposes.
fn value_of(n: &NodeRef, role: &str) -> Option<String> {
    let t = tag(n);
    match role {
        "textbox" | "searchbox" | "combobox" if t == "input" || t == "textarea" => Some(crate::forms::control_value(n)),
        "combobox" | "listbox" if t == "select" => {
            Some(name::selected_options(n).iter().map(|o| flat(&o.text_contents())).collect::<Vec<_>>().join(", "))
        }
        "slider" | "spinbutton" | "progressbar" | "meter" | "scrollbar" => attr(n, "aria-valuetext")
            .or_else(|| attr(n, "aria-valuenow"))
            .or_else(|| if matches!(t.as_str(), "input" | "progress" | "meter") { Some(crate::forms::control_value(n)).filter(|v| !v.is_empty()).or_else(|| attr(n, "value")) } else { None }),
        _ => None,
    }
}

/// A node's level: a heading's (`aria-level`, `<hN>`), else `aria-level` where ARIA defines it.
fn level_of(n: &NodeRef, role: &str) -> Option<u32> {
    match role {
        "heading" => role::heading_level(n),
        "listitem" | "row" | "treeitem" | "grid" | "treegrid" => attr(n, "aria-level").and_then(|v| v.trim().parse::<u32>().ok()).filter(|l| *l > 0),
        _ => None,
    }
}

/// The native form control kinds that carry `disabled` (HTML §4.10.18.5).
fn native_control(t: &str) -> bool {
    matches!(t, "button" | "input" | "select" | "textarea" | "option" | "optgroup" | "fieldset")
}

fn states_of(n: &NodeRef, role: &str, focusable: bool, focused: Option<&NodeRef>) -> States {
    let t = tag(n);
    let mut s = States { focusable, focused: focused == Some(n), ..States::default() };
    s.disabled = aria_bool(n, "aria-disabled") == Some(true) || (native_control(&t) && crate::forms::is_disabled(n));
    s.required = aria_bool(n, "aria-required") == Some(true) || (matches!(t.as_str(), "input" | "select" | "textarea") && has_attr(n, "required"));
    s.readonly = aria_bool(n, "aria-readonly") == Some(true) || (matches!(t.as_str(), "input" | "textarea") && has_attr(n, "readonly"));
    s.multiline = t == "textarea" || aria_bool(n, "aria-multiline") == Some(true);
    s.multiselectable = (t == "select" && has_attr(n, "multiple")) || aria_bool(n, "aria-multiselectable") == Some(true);
    s.invalid = attr(n, "aria-invalid").is_some_and(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "false"));
    s.modal = (role == "dialog" || role == "alertdialog") && aria_bool(n, "aria-modal") == Some(true);
    s.busy = aria_bool(n, "aria-busy") == Some(true);
    // checked: the native checkedness of a checkbox / radio input, else aria-checked on the roles that
    // support it.
    if t == "input" && matches!(crate::forms::input_type(n).as_str(), "checkbox" | "radio") {
        s.checked = Some(if crate::forms::checkedness(n) { Tristate::True } else { Tristate::False });
    } else if matches!(role, "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio" | "option" | "treeitem") {
        s.checked = attr(n, "aria-checked").and_then(|v| Tristate::parse(&v));
        if s.checked.is_none() && matches!(role, "checkbox" | "radio" | "switch") {
            s.checked = Some(Tristate::False);
        }
    }
    if role == "button" {
        s.pressed = attr(n, "aria-pressed").and_then(|v| Tristate::parse(&v));
    }
    s.expanded = aria_bool(n, "aria-expanded");
    if s.expanded.is_none() && t == "summary" {
        s.expanded = n.parent().filter(|p| tag(p) == "details").map(|p| has_attr(&p, "open"));
    }
    if matches!(role, "option" | "tab" | "gridcell" | "row" | "treeitem" | "columnheader" | "rowheader") {
        s.selected = if t == "option" {
            Some(crate::js::option_selectedness(n).unwrap_or_else(|| has_attr(n, "selected")))
        } else {
            aria_bool(n, "aria-selected").or(if matches!(role, "option" | "tab") { Some(false) } else { None })
        };
    }
    s.haspopup = match attr(n, "aria-haspopup").map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "true" => Some("menu".into()),
        Some(v) if matches!(v.as_str(), "menu" | "listbox" | "tree" | "grid" | "dialog") => Some(v),
        _ if role == "combobox" => Some("listbox".into()),
        _ => None,
    };
    s.current = attr(n, "aria-current").map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty() && v != "false");
    s.live = attr(n, "aria-live").map(|v| v.trim().to_ascii_lowercase()).filter(|v| matches!(v.as_str(), "off" | "polite" | "assertive")).or_else(|| {
        match role {
            "alert" => Some("assertive".into()),
            "status" | "log" => Some("polite".into()),
            "timer" | "marquee" => Some("off".into()),
            _ => None,
        }
    });
    s.autocomplete = attr(n, "aria-autocomplete").map(|v| v.trim().to_ascii_lowercase()).filter(|v| matches!(v.as_str(), "inline" | "list" | "both"));
    s.orientation = attr(n, "aria-orientation").map(|v| v.trim().to_ascii_lowercase()).filter(|v| v == "horizontal" || v == "vertical");
    if matches!(role, "slider" | "spinbutton" | "progressbar" | "meter" | "scrollbar" | "separator") {
        let native = matches!(t.as_str(), "input" | "progress" | "meter");
        s.value_min = num_attr(n, "aria-valuemin").or_else(|| if native { num_attr(n, "min") } else { None });
        s.value_max = num_attr(n, "aria-valuemax").or_else(|| if native { num_attr(n, "max") } else { None });
        s.value_now = num_attr(n, "aria-valuenow").or_else(|| if native { num_attr(n, "value") } else { None });
        if t == "input" && role == "slider" {
            // HTML §4.10.5.1.13: a range's defaults are 0, 100 and the midpoint.
            let (lo, hi) = (s.value_min.unwrap_or(0.0), s.value_max.unwrap_or(100.0));
            s.value_min = Some(lo);
            s.value_max = Some(hi);
            s.value_now = crate::forms::control_value(n).trim().parse::<f64>().ok().or(s.value_now).or(Some(lo + (hi - lo) / 2.0));
        }
        if t == "progress" && s.value_max.is_none() {
            s.value_max = Some(1.0);
            s.value_min = Some(0.0);
        }
    }
    s
}
