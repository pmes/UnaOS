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
    if with_doc(|d| sh::shadow_root_of(d, NodeId(el))).is_some() {
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

/// The fragment parsing algorithm with the host as context, then "replace all" (`innerHTML`,
/// `setHTMLUnsafe` without declarative shadow roots).
fn sr_set_markup(vm: &mut Vm, i: sh::ShadowRootInfo, markup: &str) {
    let frag = parse_fragment(i.host.0, markup);
    replace_all(vm, Some(frag), i.root.0);
}

fn sr_inner_html_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    let markup = string_null_empty(vm, &arg(vm, ctx, 0))?;
    sr_set_markup(vm, i, &markup);
    Ok(Value::Undefined)
}

fn sr_set_html_unsafe(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let i = this_shadow(vm, ctx)?;
    need(vm, ctx, 1, "ShadowRoot", "setHTMLUnsafe")?;
    let markup = string(vm, &arg(vm, ctx, 0))?;
    sr_set_markup(vm, i, &markup);
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
