//! The accessible name and description: Accessible Name and Description Computation 1.2 (accname 1.2)
//! §4.3 "Text Alternative Computation", steps 2A–2I, with HTML-AAM 1.0 §4.1 for the host-language step
//! 2D (labels, `alt`, `<legend>`, `<caption>`, `<figcaption>`, button values, `title`, `placeholder`) and
//! §4.2 for descriptions.
//!
//! Every step is a branch of [`alternative`], which answers the text and the step that produced it
//! ([`NameFrom`], what the devtools panel shows as "name from"). The recursion carries [`Flags`]: whether
//! it is inside another node's name (step 2E and 2F), inside an `aria-labelledby` / `aria-describedby`
//! traversal (step 2B runs once), and whether that traversal began at a hidden node (step 2A then admits
//! hidden content). A node is visited at most once per computation (the cycle rule of step 2B).

use std::collections::HashSet;

use super::role;
use super::tree::{role_of, Rendering};
use super::{attr, flat, id_refs, tag};
use crate::dom::NodeRef;
use html_core::NodeId;

/// Which step of the computation produced a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NameFrom {
    /// No name.
    #[default]
    None,
    /// Step 2B: `aria-labelledby`.
    LabelledBy,
    /// Step 2C: `aria-label`.
    AriaLabel,
    /// Step 2D: `<label>` elements (`for=` and the wrapping label).
    Label,
    /// Step 2D: `alt` (`<img>`, `<area>`, `<input type=image>`).
    Alt,
    /// Step 2D: the first `<legend>` of a `<fieldset>`.
    Legend,
    /// Step 2D: a `<table>`'s `<caption>`.
    Caption,
    /// Step 2D: a `<figure>`'s `<figcaption>`.
    Figcaption,
    /// Step 2D: a button input's `value` (or its default label), an `<option>`/`<optgroup>`'s `label`.
    Value,
    /// Step 2D: an `<svg>`'s `<title>` child, a document's `<title>`.
    TitleElement,
    /// Step 2F: the subtree's text.
    Contents,
    /// Step 2I: the `title` attribute (the tooltip).
    Title,
    /// HTML-AAM: a text field's `placeholder` / `aria-placeholder`, after `title`.
    Placeholder,
}

impl NameFrom {
    pub fn label(self) -> &'static str {
        match self {
            NameFrom::None => "",
            NameFrom::LabelledBy => "aria-labelledby",
            NameFrom::AriaLabel => "aria-label",
            NameFrom::Label => "label",
            NameFrom::Alt => "alt",
            NameFrom::Legend => "legend",
            NameFrom::Caption => "caption",
            NameFrom::Figcaption => "figcaption",
            NameFrom::Value => "value",
            NameFrom::TitleElement => "title element",
            NameFrom::Contents => "contents",
            NameFrom::Title => "title",
            NameFrom::Placeholder => "placeholder",
        }
    }
}

/// The recursion state of one computation.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    /// Inside another node's name (accname "recursion"): steps 2E and 2F apply to every role.
    pub recursing: bool,
    /// Inside an `aria-labelledby` / `aria-describedby` traversal: step 2B does not run again.
    pub in_ref: bool,
    /// The traversal's referenced node was hidden: hidden nodes contribute (step 2A's exception).
    pub ref_hidden: bool,
}

/// One computation's context.
pub struct Cx<'a> {
    pub r: &'a Rendering,
    visited: HashSet<NodeId>,
}

impl<'a> Cx<'a> {
    pub fn new(r: &'a Rendering) -> Self {
        Cx { r, visited: HashSet::new() }
    }
}

/// The accessible name of `n` (whose computed role is `role`), flattened, and the step it came from.
/// A role whose naming is prohibited has none.
pub fn accessible_name(n: &NodeRef, role: &str, r: &Rendering) -> (String, NameFrom) {
    if role::name_prohibited(role) {
        return (String::new(), NameFrom::None);
    }
    let mut cx = Cx::new(r);
    cx.visited.insert(n.id());
    let (s, from) = alternative(n, &mut cx, Flags::default(), Some(role));
    let s = flat(&s);
    if s.is_empty() { (s, NameFrom::None) } else { (s, from) }
}

/// The accessible description (accname 1.2 §4.2, HTML-AAM §4.2): `aria-describedby`, else
/// `aria-description`, else the `title` attribute when the name did not already use it.
pub fn accessible_description(n: &NodeRef, name_from: NameFrom, r: &Rendering) -> String {
    let refs = id_refs(n, "aria-describedby");
    if !refs.is_empty() {
        let mut cx = Cx::new(r);
        cx.visited.insert(n.id());
        let parts: Vec<String> = refs
            .iter()
            .map(|t| {
                let f = Flags { recursing: true, in_ref: true, ref_hidden: r.hidden(t) };
                flat(&alternative(t, &mut cx, f, None).0)
            })
            .filter(|s| !s.is_empty())
            .collect();
        let s = parts.join(" ");
        if !s.is_empty() {
            return s;
        }
    }
    if let Some(d) = attr(n, "aria-description").map(|d| flat(&d)).filter(|d| !d.is_empty()) {
        return d;
    }
    if name_from != NameFrom::Title {
        if let Some(t) = attr(n, "title").map(|t| flat(&t)).filter(|t| !t.is_empty()) {
            return t;
        }
    }
    String::new()
}

/// Steps 2A–2I for one node. `role_hint` is the node's computed role when the caller has it.
fn alternative(n: &NodeRef, cx: &mut Cx<'_>, f: Flags, role_hint: Option<&str>) -> (String, NameFrom) {
    // 2G: a text node contributes its text (whitespace is collapsed at the end).
    if let Some(t) = n.as_text() {
        let parent_hidden = n.parent().is_some_and(|p| cx.r.hidden(&p));
        if parent_hidden && !f.ref_hidden {
            return (String::new(), NameFrom::None);
        }
        return (t.borrow().clone(), NameFrom::Contents);
    }
    if !n.is_element() {
        return (String::new(), NameFrom::None);
    }
    // 2A: hidden and not part of a traversal that began at a hidden node.
    if !f.ref_hidden && cx.r.hidden(n) {
        return (String::new(), NameFrom::None);
    }
    // 2B: aria-labelledby, once per traversal.
    if !f.in_ref {
        let refs = id_refs(n, "aria-labelledby");
        if !refs.is_empty() {
            let mut parts = Vec::new();
            for t in &refs {
                let ff = Flags { recursing: true, in_ref: true, ref_hidden: cx.r.hidden(t) };
                // A self-reference names the node by its own other steps; any other node once.
                if t != n && !cx.visited.insert(t.id()) {
                    continue;
                }
                let s = flat(&alternative(t, cx, ff, None).0);
                if !s.is_empty() {
                    parts.push(s);
                }
            }
            let s = parts.join(" ");
            if !s.is_empty() {
                return (s, NameFrom::LabelledBy);
            }
        }
    }
    let role = role_hint.unwrap_or_else(|| role_of(n, cx.r));
    let embedded = f.recursing && role::is_embedded_control(role);
    // 2C: aria-label (an embedded control inside another name gives its value instead, step 2E).
    if !embedded {
        if let Some(l) = attr(n, "aria-label").filter(|l| !l.trim().is_empty()) {
            return (l, NameFrom::AriaLabel);
        }
    }
    // 2D: the host language's text alternative, unless the role is presentational.
    if !role::is_presentational(role) {
        if let Some(r) = native(n, cx, f) {
            return r;
        }
    }
    // 2E: an embedded control inside another node's name: its value.
    if embedded {
        return (embedded_value(n, role), NameFrom::Value);
    }
    // 2F: name from content.
    if role::allows_name_from_content(role) || f.recursing {
        let s = contents(n, cx, f);
        if !flat(&s).is_empty() {
            return (s, NameFrom::Contents);
        }
    }
    // 2I: the tooltip attribute, then (HTML-AAM, text fields) the placeholder.
    if let Some(t) = attr(n, "title").filter(|t| !t.trim().is_empty()) {
        return (t, NameFrom::Title);
    }
    if matches!(role, "textbox" | "searchbox" | "combobox") {
        if let Some(p) = attr(n, "placeholder").or_else(|| attr(n, "aria-placeholder")).filter(|p| !p.trim().is_empty()) {
            return (p, NameFrom::Placeholder);
        }
    }
    (String::new(), NameFrom::None)
}

/// Labelable elements (HTML §4.10.2: button, input but hidden, meter, output, progress, select, textarea).
pub(crate) fn labelable(n: &NodeRef) -> bool {
    match tag(n).as_str() {
        "button" | "meter" | "output" | "progress" | "select" | "textarea" => true,
        "input" => crate::forms::input_type(n) != "hidden",
        _ => false,
    }
}

/// The `<label>` elements of a labelable element (HTML §4.10.4 `labels`): every label whose `for` names
/// its id, and the nearest ancestor label that has no `for` (or whose `for` names it), in tree order.
pub(crate) fn labels(n: &NodeRef) -> Vec<NodeRef> {
    if !labelable(n) {
        return Vec::new();
    }
    let id = attr(n, "id").filter(|i| !i.is_empty());
    let mut out: Vec<NodeRef> = n
        .document_node()
        .descendants()
        .filter(|l| tag(l) == "label" && l.as_element().is_some())
        .filter(|l| match attr(l, "for") {
            Some(f) => id.as_deref() == Some(f.as_str()),
            None => false,
        })
        .collect();
    if let Some(wrap) = n.ancestors().find(|a| tag(a) == "label") {
        let ok = match attr(&wrap, "for") {
            Some(f) => id.as_deref() == Some(f.as_str()),
            None => true,
        };
        if ok && !out.contains(&wrap) {
            out.push(wrap);
        }
    }
    out
}

/// Step 2D for HTML. None when the host language gives nothing (the computation goes on to 2E).
fn native(n: &NodeRef, cx: &mut Cx<'_>, f: Flags) -> Option<(String, NameFrom)> {
    let t = tag(n);
    let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    if t == "input" {
        let ty = crate::forms::input_type(n);
        match ty.as_str() {
            "button" | "submit" | "reset" => {
                // HTML-AAM: the value attribute, else the default label of submit and reset.
                return match attr(n, "value") {
                    Some(v) if !v.trim().is_empty() => Some((v, NameFrom::Value)),
                    _ => match ty.as_str() {
                        "submit" => Some(("Submit".into(), NameFrom::Value)),
                        "reset" => Some(("Reset".into(), NameFrom::Value)),
                        _ => None,
                    },
                };
            }
            "image" => {
                if let Some(a) = nonempty(attr(n, "alt")) {
                    return Some((a, NameFrom::Alt));
                }
                if let Some(v) = nonempty(attr(n, "value")) {
                    return Some((v, NameFrom::Value));
                }
                if attr(n, "title").is_none() {
                    return Some(("Submit".into(), NameFrom::Value));
                }
                return None;
            }
            _ => {}
        }
    }
    if labelable(n) {
        let ls = labels(n);
        if !ls.is_empty() {
            let mut parts = Vec::new();
            for l in &ls {
                if !cx.visited.insert(l.id()) {
                    continue;
                }
                let ff = Flags { recursing: true, ..f };
                let s = flat(&alternative(l, cx, ff, Some(role::GENERIC)).0);
                if !s.is_empty() {
                    parts.push(s);
                }
            }
            let s = parts.join(" ");
            if !s.is_empty() {
                return Some((s, NameFrom::Label));
            }
        }
    }
    let child = |name: &str| n.children().find(|c| tag(c) == name);
    let sub = |c: NodeRef, cx: &mut Cx<'_>, from: NameFrom| -> Option<(String, NameFrom)> {
        if !cx.visited.insert(c.id()) {
            return None;
        }
        let s = flat(&alternative(&c, cx, Flags { recursing: true, ..f }, Some(role::GENERIC)).0);
        (!s.is_empty()).then_some((s, from))
    };
    match t.as_str() {
        "img" | "area" => nonempty(attr(n, "alt")).map(|a| (a, NameFrom::Alt)),
        "fieldset" => child("legend").and_then(|l| sub(l, cx, NameFrom::Legend)),
        "figure" => child("figcaption").and_then(|c| sub(c, cx, NameFrom::Figcaption)),
        "table" => child("caption").and_then(|c| sub(c, cx, NameFrom::Caption)),
        "svg" => child("title").map(|c| flat(&c.text_contents())).filter(|s| !s.is_empty()).map(|s| (s, NameFrom::TitleElement)),
        "optgroup" | "option" | "track" => nonempty(attr(n, "label")).map(|l| (l, NameFrom::Value)),
        _ => None,
    }
}

/// Step 2E: an embedded control's value.
fn embedded_value(n: &NodeRef, role: &str) -> String {
    match role {
        "textbox" | "searchbox" => match tag(n).as_str() {
            "input" | "textarea" => crate::forms::control_value(n),
            _ => n.text_contents(),
        },
        "combobox" | "listbox" => {
            if tag(n) == "select" {
                selected_options(n).iter().map(|o| flat(&o.text_contents())).collect::<Vec<_>>().join(" ")
            } else if tag(n) == "input" {
                crate::forms::control_value(n)
            } else {
                n.descendants()
                    .filter(|d| super::role::explicit_role(d) == Some("option") && super::aria_bool(d, "aria-selected") == Some(true))
                    .map(|o| flat(&o.text_contents()))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        }
        _ => attr(n, "aria-valuetext")
            .or_else(|| attr(n, "aria-valuenow"))
            .or_else(|| attr(n, "value"))
            .unwrap_or_default(),
    }
}

/// A `<select>`'s selected options: each option's selectedness (the dirty state script and the user set,
/// else the `selected` attribute); a single-select with none selected shows its first option.
pub(crate) fn selected_options(select: &NodeRef) -> Vec<NodeRef> {
    let opts: Vec<NodeRef> = select.descendants().filter(|d| tag(d) == "option").collect();
    let sel: Vec<NodeRef> = opts
        .iter()
        .filter(|o| crate::js::option_selectedness(o).unwrap_or_else(|| super::has_attr(o, "selected")))
        .cloned()
        .collect();
    if sel.is_empty() && attr(select, "multiple").is_none() {
        return opts.into_iter().take(1).collect();
    }
    sel
}

/// Step 2F: the text of the children, each child's own text alternative, block-level children set off
/// by spaces (accname 1.2 2F.iii: "if the current node is a block element, add white space").
fn contents(n: &NodeRef, cx: &mut Cx<'_>, f: Flags) -> String {
    let mut out = String::new();
    for c in n.children() {
        if c.is_element() {
            if tag(&c) == "br" {
                out.push(' ');
                continue;
            }
            if !cx.visited.insert(c.id()) {
                continue;
            }
        }
        let (s, _) = alternative(&c, cx, Flags { recursing: true, ..f }, None);
        if c.is_element() && cx.r.is_block(&c) {
            out.push(' ');
            out.push_str(&s);
            out.push(' ');
        } else {
            out.push_str(&s);
        }
    }
    out
}
