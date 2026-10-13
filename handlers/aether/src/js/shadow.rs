//! Shadow DOM's binding (AETHERSHADOW, LEDGER SR82) over `dom::shadow`: `Element.attachShadow` /
//! `shadowRoot` / `assignedSlot`, `Text.assignedSlot`, `Node.getRootNode({composed})`, the ShadowRoot
//! interface's members, and HTMLSlotElement's `assignedNodes` / `assignedElements` / `assign`.
//!
//! A shadow root's wrapper is the DocumentFragment wrapper of its node with ShadowRoot.prototype (the
//! node is new when `attachShadow` wraps it, so identity is the usual one-wrapper-per-node).

use super::dom::{self, arena, iface, node_document, node_of, parse_fragment, replace_all, set_node_doc, this_element, this_node, with_doc, wrap, wrap_opt, NK};
use super::idl::*;
use super::page;
use crate::dom::shadow::{self as sh, ShadowInit, ShadowMode, SlotAssignment};
use html_core::NodeId;
use js_core::vm::*;

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// "attach a shadow root" (DOM §4.9) behind `Element.attachShadow(init)`.
fn attach_shadow(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "attachShadow")?;
    let init = arg(vm, ctx, 0);
    let fail = "Failed to execute 'attachShadow' on 'Element': ";
    if !matches!(init, Value::Object(_) | Value::Undefined | Value::Null) {
        return vm.throw_type(&format!("{fail}The provided value is not of type 'ShadowRootInit'."));
    }
    // ShadowRootInit members in lexicographic order (WebIDL dictionary conversion).
    let v = dict_member(vm, &init, "clonable")?;
    let clonable = boolean(vm, &v);
    let v = dict_member(vm, &init, "delegatesFocus")?;
    let delegates_focus = boolean(vm, &v);
    let v = dict_member(vm, &init, "mode")?;
    if v.is_undefined() {
        return vm.throw_type(&format!("{fail}Failed to read the 'mode' property from 'ShadowRootInit': Required member is undefined."));
    }
    let mode = match string(vm, &v)?.as_str() {
        "open" => ShadowMode::Open,
        "closed" => ShadowMode::Closed,
        other => {
            return vm.throw_type(&format!("{fail}The provided value '{other}' is not a valid enum value of type ShadowRootMode."));
        }
    };
    let v = dict_member(vm, &init, "serializable")?;
    let serializable = boolean(vm, &v);
    let v = dict_member(vm, &init, "slotAssignment")?;
    let slot_assignment = if v.is_undefined() {
        SlotAssignment::Named
    } else {
        match string(vm, &v)?.as_str() {
            "named" => SlotAssignment::Named,
            "manual" => SlotAssignment::Manual,
            other => {
                return vm.throw_type(&format!("{fail}The provided value '{other}' is not a valid enum value of type SlotAssignmentMode."));
            }
        }
    };
    // Steps 1–4.
    let (ns, local) = with_doc(|d| (dom::element_ns(d, el).0, dom::local_name_in(d, el)));
    if ns != HTML_NS {
        return throw_dom(vm, "NotSupportedError", &format!("{fail}This element does not support attachShadow"));
    }
    if !sh::is_valid_shadow_host_name(&local) {
        return throw_dom(vm, "NotSupportedError", &format!("{fail}This element does not support attachShadow"));
    }
    let is = with_doc(|d| crate::dom::custom::is_value(d, NodeId(el)));
    if crate::dom::custom::valid_custom_element_name(&local) || is.is_some() {
        if let Some(def) = crate::dom::custom::look_up(html_core::Namespace::Html, &local, is.as_deref()) {
            if crate::dom::custom::definition(def).is_some_and(|d| d.disable_shadow) {
                return throw_dom(vm, "NotSupportedError", &format!("{fail}attachShadow() is disabled by disabledFeatures static field."));
            }
        }
    }
    if let Some(existing) = with_doc(|d| sh::shadow_root_of(d, NodeId(el))) {
        // DOM §4.9 "attach a shadow root" step 5: a declarative root of the same mode is reused —
        // its children removed, the declarative flag cleared — instead of throwing.
        if existing.declarative && existing.mode == mode {
            let kids = dom::children_of(existing.root.0);
            for k in kids {
                dom::remove(vm, k);
            }
            with_doc(|d| sh::set_declarative(d, existing.root, false));
            return Ok(wrap(vm, existing.root.0));
        }
        return throw_dom(vm, "NotSupportedError", &format!("{fail}Shadow root cannot be created on a host which already hosts a shadow tree."));
    }
    let a = arena();
    let root = sh::attach(&a, NodeId(el), ShadowInit { mode, delegates_focus, slot_assignment, clonable, serializable }).0;
    drop(a);
    set_node_doc(root, node_document(el));
    let w = wrap(vm, root);
    if let (Value::Object(o), Some(i)) = (&w, iface("ShadowRoot")) {
        vm.heap.get_mut(*o).proto = Some(i.proto);
    }
    Ok(w)
}

fn shadow_root_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let r = with_doc(|d| sh::shadow_root_of(d, NodeId(el))).filter(|i| i.mode == ShadowMode::Open).map(|i| i.root.0);
    Ok(wrap_opt(vm, r))
}

/// `assignedSlot` (Element and Text): "find a slot" with the open flag.
fn assigned_slot(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let s = with_doc(|d| sh::find_slot(d, NodeId(n), true)).map(|s| s.0);
    Ok(wrap_opt(vm, s))
}

/// `getRootNode(options)`: the shadow-including root when `composed`, else the root.
fn get_root_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let opts = arg(vm, ctx, 0);
    let v = dict_member(vm, &opts, "composed")?;
    let composed = boolean(vm, &v);
    let r = with_doc(|d| if composed { sh::shadow_including_root(d, NodeId(n)) } else { sh::root(d, NodeId(n)) });
    Ok(wrap(vm, r.0))
}

// ---------------------------------------------------------------------------------------------------
// ShadowRoot
// ---------------------------------------------------------------------------------------------------

fn this_shadow(vm: &mut Vm, ctx: &CallCtx) -> JsResult<sh::ShadowRootInfo> {
    let info = node_of(vm, &ctx.this).and_then(|n| with_doc(|d| sh::info_of_root(d, NodeId(n))));
    match info {
        Some(i) => Ok(i),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn sr_mode(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(s(this_shadow(vm, ctx)?.mode.as_str()))
}
fn sr_delegates_focus(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(this_shadow(vm, ctx)?.delegates_focus))
}
fn sr_slot_assignment(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(s(this_shadow(vm, ctx)?.slot_assignment.as_str()))
}
fn sr_clonable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(this_shadow(vm, ctx)?.clonable))
}
fn sr_serializable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(this_shadow(vm, ctx)?.serializable))
}
fn sr_host(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    Ok(wrap(vm, i.host.0))
}

fn sr_inner_html_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    Ok(s(&dom::serialize_inner(i.root.0)))
}

/// The fragment parsing algorithm with the host as context, then "replace all" (`innerHTML`; with
/// `declarative`, `setHTMLUnsafe`: the fragment's `<template shadowrootmode>` elements attach first).
fn sr_set_markup(vm: &mut Vm, i: sh::ShadowRootInfo, markup: &str, declarative: bool) {
    let frag = parse_fragment(i.host.0, markup);
    if declarative {
        attach_declarative(frag);
    }
    replace_all(vm, Some(frag), i.root.0);
}

/// Is `t` a `<template>` whose `shadowrootmode` is a valid ShadowRootMode (HTML §13.2.6.4.4, the
/// "template" start tag in "in head")?
fn declarative_mode(d: &html_core::Document, t: NodeId) -> Option<ShadowMode> {
    let e = d.element(t)?;
    if !e.is_html("template") {
        return None;
    }
    match e.attr("shadowrootmode")?.to_ascii_lowercase().as_str() {
        "open" => Some(ShadowMode::Open),
        "closed" => Some(ShadowMode::Closed),
        _ => None,
    }
}

/// Declarative shadow roots (HTML §13.2.6.4.4 + §13.2.6.4.7, AETHERDEBT SR93) over the parsed tree
/// under `root`: each `<template shadowrootmode=open|closed>` whose parent is an HTML element that
/// may host a shadow root and hosts none yet attaches a shadow root to that parent (mode,
/// `shadowrootdelegatesfocus`, `shadowrootclonable`, `shadowrootserializable`; declarative), its
/// contents become the shadow root's children and the template leaves the tree, as if it had never
/// been inserted. A template that cannot attach stays an ordinary template. Templates nested in a
/// declarative template's contents are processed once those contents are in the shadow root, so a
/// nested declarative tree attaches inside its parent's. Runs over a tree no script has seen yet
/// (the page parser's newly parsed nodes, `setHTMLUnsafe`'s fragment), so it records nothing.
pub fn attach_declarative(root: usize) {
    attach_declarative_where(root, &|_| true);
}

/// [`attach_declarative`] restricted to the templates `ok` accepts (the page parser passes "this
/// template is closed", i.e. not an ancestor of the insertion point).
pub fn attach_declarative_where(root: usize, ok: &dyn Fn(usize) -> bool) {
    let mut work: Vec<NodeId> = with_doc(|d| d.descendants(NodeId(root)).filter(|&n| declarative_mode(d, n).is_some()).collect());
    let mut i = 0;
    while i < work.len() {
        let t = work[i];
        i += 1;
        if !ok(t.0) {
            continue;
        }
        let plan = with_doc(|d| {
            let mode = declarative_mode(d, t)?;
            let e = d.element(t)?;
            let contents = e.template_contents?;
            let host = d.parent(t)?;
            let he = d.element(host)?;
            if he.ns != html_core::Namespace::Html || !sh::is_valid_shadow_host_name(&he.local) || sh::shadow_root_of(d, host).is_some() {
                return None;
            }
            let init = ShadowInit {
                mode,
                delegates_focus: e.attr("shadowrootdelegatesfocus").is_some(),
                slot_assignment: SlotAssignment::Named,
                clonable: e.attr("shadowrootclonable").is_some(),
                serializable: e.attr("shadowrootserializable").is_some(),
            };
            Some((host, contents, init))
        });
        let Some((host, contents, init)) = plan else { continue };
        let a = arena();
        let sroot = sh::attach(&a, host, init);
        drop(a);
        let doc = node_document(host.0);
        let moved: Vec<usize> = super::dom::with_doc_mut(|d| {
            d.reparent_children(contents, sroot);
            d.detach(t);
            std::iter::once(sroot).chain(d.descendants(sroot)).map(|n| n.0).collect()
        });
        with_doc(|d| sh::set_declarative(d, sroot, true));
        for m in moved {
            set_node_doc(m, doc);
            if with_doc(|d| d.element(NodeId(m)).is_some_and(|e| e.template_contents.is_some())) {
                super::dom::set_template_contents_doc(m, doc);
            }
        }
        let nested: Vec<NodeId> = with_doc(|d| d.descendants(sroot).filter(|&n| declarative_mode(d, n).is_some()).collect());
        work.extend(nested);
    }
}

/// `Element.setHTMLUnsafe(html)` (HTML §8.5.1): the fragment parsing algorithm with declarative
/// shadow roots allowed, then "replace all" over the element (a template's contents for `<template>`).
fn el_set_html_unsafe(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "setHTMLUnsafe")?;
    let markup = string(vm, &arg(vm, ctx, 0))?;
    let frag = parse_fragment(el, &markup);
    attach_declarative(frag);
    let target = with_doc(|d| d.element(NodeId(el)).and_then(|e| e.template_contents).map(|t| t.0)).unwrap_or(el);
    replace_all(vm, Some(frag), target);
    Ok(Value::Undefined)
}

fn sr_inner_html_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    let markup = string_null_empty(vm, &arg(vm, ctx, 0))?;
    sr_set_markup(vm, i, &markup, false);
    Ok(Value::Undefined)
}

fn sr_set_html_unsafe(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    need(vm, ctx, 1, "ShadowRoot", "setHTMLUnsafe")?;
    let markup = string(vm, &arg(vm, ctx, 0))?;
    sr_set_markup(vm, i, &markup, true);
    Ok(Value::Undefined)
}

fn sr_get_html(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    Ok(s(&dom::serialize_inner(i.root.0)))
}

/// DocumentOrShadowRoot `activeElement`: the focused element retargeted against this root, when its
/// root is this root.
fn sr_active_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    let Some(f) = page(|p| p.focused) else { return Ok(Value::Null) };
    let r = with_doc(|d| {
        let t = sh::retarget(d, NodeId(f), Some(i.root));
        (sh::root(d, t) == i.root).then_some(t.0)
    });
    Ok(wrap_opt(vm, r))
}

// ---------------------------------------------------------------------------------------------------
// HTMLSlotElement
// ---------------------------------------------------------------------------------------------------

fn this_slot(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    let el = this_element(vm, ctx)?;
    if !with_doc(|d| sh::is_slot(d, NodeId(el))) {
        return vm.throw_type("Illegal invocation");
    }
    Ok(el)
}

fn assigned(vm: &mut Vm, ctx: &CallCtx, elements_only: bool) -> JsResult<Value> {
    let slot = this_slot(vm, ctx)?;
    let opts = arg(vm, ctx, 0);
    let v = dict_member(vm, &opts, "flatten")?;
    let flatten = boolean(vm, &v);
    let ids: Vec<usize> = with_doc(|d| {
        let list = if flatten { sh::flattened_slottables(d, NodeId(slot)) } else { sh::find_slottables(d, NodeId(slot)) };
        list.into_iter().filter(|&n| !elements_only || d.element(n).is_some()).map(|n| n.0).collect()
    });
    let vals: Vec<Value> = ids.into_iter().map(|i| wrap(vm, i)).collect();
    Ok(Value::Object(vm.new_array(vals)))
}

fn assigned_nodes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    assigned(vm, ctx, false)
}

fn assigned_elements(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    assigned(vm, ctx, true)
}

/// `assign(...nodes)` (HTML §4.12.4): the slot's manually assigned nodes, then "assign slottables
/// for a tree" over its root.
fn slot_assign(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let slot = this_slot(vm, ctx)?;
    let mut nodes = Vec::with_capacity(ctx.argc);
    for i in 0..ctx.argc {
        let v = arg(vm, ctx, i);
        let ok = node_of(vm, &v).filter(|&n| matches!(dom::kind(n), NK::Element | NK::Text));
        match ok {
            Some(n) => nodes.push(NodeId(n)),
            None => {
                return vm.throw_type(&format!(
                    "Failed to execute 'assign' on 'HTMLSlotElement': Failed to convert value to '(Element or Text)'. (argument {})",
                    i + 1
                ));
            }
        }
    }
    with_doc(|d| {
        sh::assign_manual(d, NodeId(slot), &nodes);
        let r = sh::root(d, NodeId(slot));
        sh::refresh(d, r);
    });
    super::mutation::settle(vm);
    Ok(Value::Undefined)
}

/// Installs the shadow DOM members (after dom/html: ShadowRoot and HTMLSlotElement exist).
pub fn install(vm: &mut Vm) {
    let el = iface("Element").expect("Element").proto;
    op(vm, el, "attachShadow", 1, attach_shadow);
    op(vm, el, "setHTMLUnsafe", 1, el_set_html_unsafe);
    attr(vm, el, "shadowRoot", shadow_root_get, None);
    attr(vm, el, "assignedSlot", assigned_slot, None);
    if let Some(t) = iface("Text") {
        attr(vm, t.proto, "assignedSlot", assigned_slot, None);
    }
    let np = iface("Node").expect("Node").proto;
    op(vm, np, "getRootNode", 0, get_root_node);
    if let Some(sr) = iface("ShadowRoot") {
        let p = sr.proto;
        attr(vm, p, "mode", sr_mode, None);
        attr(vm, p, "delegatesFocus", sr_delegates_focus, None);
        attr(vm, p, "slotAssignment", sr_slot_assignment, None);
        attr(vm, p, "clonable", sr_clonable, None);
        attr(vm, p, "serializable", sr_serializable, None);
        attr(vm, p, "host", sr_host, None);
        attr(vm, p, "innerHTML", sr_inner_html_get, Some(sr_inner_html_set));
        attr(vm, p, "activeElement", sr_active_element, None);
        op(vm, p, "getHTML", 0, sr_get_html);
        op(vm, p, "setHTMLUnsafe", 1, sr_set_html_unsafe);
        super::events::install_handlers(vm, p, &["slotchange"]);
    }
    if let Some(sl) = iface("HTMLSlotElement") {
        let p = sl.proto;
        op(vm, p, "assignedNodes", 0, assigned_nodes);
        op(vm, p, "assignedElements", 0, assigned_elements);
        op(vm, p, "assign", 0, slot_assign);
    }
}
