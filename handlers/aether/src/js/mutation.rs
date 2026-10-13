//! MutationObserver and MutationRecord (DOM §4.3) over `dom::mutation` (AETHERSHADOW, LEDGER SR82),
//! and [`settle`]: the binding's [CEReactions] / observer-microtask boundary that every mutation
//! algorithm ends with.
//!
//! Delivery is HTML's microtask: when a record (or a slot change) is first queued, `dom::mutation`
//! marks the microtask owed and `settle` pushes a `Job::Call` of [`notify`] onto the VM's job queue,
//! which the page's microtask checkpoint runs ("notify mutation observers": each observer in creation
//! order with its records, transient registrations dropped, the callback invoked with the observer as
//! `this`, then `slotchange` at each signalled slot).

use super::dom::{self, node_arg, register_iface, static_node_list, wrap, wrap_opt};
use super::idl::*;
use super::TargetKey;
use crate::dom::mutation::{self as mo, Options, Record};
use html_core::NodeId;
use js_core::vm::*;

/// Platform-object tags of this binding (beyond idl.rs's families).
pub const T_MUTOBS: u8 = 40;
pub const T_MUTREC: u8 = 41;

const R_TYPE: usize = 0;
const R_TARGET: usize = 1;
const R_ADDED: usize = 2;
const R_REMOVED: usize = 3;
const R_PREV: usize = 4;
const R_NEXT: usize = 5;
const R_NAME: usize = 6;
const R_NS: usize = 7;
const R_OLD: usize = 8;

/// The boundary each mutation algorithm ends with: push the observer microtask when one is owed,
/// run the queued custom element reactions, and push again if a reaction queued records.
pub fn settle(vm: &mut Vm) {
    push_job(vm);
    super::custom::flush(vm);
    push_job(vm);
}

fn push_job(vm: &mut Vm) {
    if mo::take_job_request() {
        let fp = vm.intr().function_proto;
        let f = vm.make_native_with("", 0, notify, false, Some(fp), Vec::new());
        vm.jobs.push_back(Job::Call { func: Value::Object(f), this: Value::Undefined, args: Vec::new() });
    }
}

/// DOM "notify mutation observers".
fn notify(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    let (observers, slots) = mo::begin_notify();
    for id in observers {
        let records = mo::take_for_delivery(id);
        if records.is_empty() {
            continue;
        }
        let Some((cb, obj)) = mo::observer_roots(id) else { continue };
        let list = records_array(vm, &records);
        let r = root(vm, list.clone());
        let f = rooted(vm, cb);
        let this = rooted(vm, obj);
        super::invoke(vm, "mutation-observer", &f, &this, &[list, this.clone()]);
        unroot(vm, r);
    }
    for s in slots {
        super::events::fire_simple(vm, TargetKey::Node(s.0), "slotchange", true, false);
    }
    Ok(Value::Undefined)
}

fn record_object(vm: &mut Vm, r: &Record) -> Value {
    let proto = dom::iface("MutationRecord").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let target = wrap(vm, r.target.0);
    let added = static_node_list(vm, r.added.iter().map(|n| n.0).collect());
    let removed = static_node_list(vm, r.removed.iter().map(|n| n.0).collect());
    let prev = wrap_opt(vm, r.prev.map(|n| n.0));
    let next = wrap_opt(vm, r.next.map(|n| n.0));
    let opt = |v: &Option<String>| v.as_deref().map(s).unwrap_or(Value::Null);
    let slots = vec![s(r.kind.as_str()), target, added, removed, prev, next, opt(&r.attr_name), opt(&r.attr_ns), opt(&r.old_value)];
    Value::Object(host_obj(vm, proto, T_MUTREC, slots))
}

fn records_array(vm: &mut Vm, records: &[Record]) -> Value {
    let mut vals = Vec::with_capacity(records.len());
    for r in records {
        let o = record_object(vm, r);
        vals.push(o);
    }
    Value::Object(vm.new_array(vals))
}

// ---------------------------------------------------------------------------------------------------
// MutationObserver
// ---------------------------------------------------------------------------------------------------

fn mo_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'MutationObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    }
    need(vm, ctx, 1, "MutationObserver", "constructor")?;
    let cb = arg(vm, ctx, 0);
    if !vm.is_callable(&cb) {
        return vm.throw_type("Failed to construct 'MutationObserver': parameter 1 is not of type 'MutationCallback'.");
    }
    let default = dom::iface("MutationObserver").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    let o = host_obj(vm, proto, T_MUTOBS, vec![num(0.0)]);
    let cb_root = root(vm, cb);
    let obj_root = root(vm, Value::Object(o));
    let id = mo::new_observer(cb_root, obj_root);
    set_slot(vm, o, 0, num(id as f64));
    Ok(Value::Object(o))
}

fn this_observer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<u32> {
    match this_tagged(vm, &ctx.this, T_MUTOBS) {
        Some(o) => Ok(slot_num(vm, o, 0) as u32),
        None => vm.throw_type("Illegal invocation"),
    }
}

/// An optional dictionary boolean: None when absent (undefined).
fn opt_bool(vm: &mut Vm, dict: &Value, name: &str) -> JsResult<Option<bool>> {
    let v = dict_member(vm, dict, name)?;
    Ok(if v.is_undefined() { None } else { Some(boolean(vm, &v)) })
}

fn mo_observe(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_observer(vm, ctx)?;
    need(vm, ctx, 1, "MutationObserver", "observe")?;
    let target = node_arg(vm, ctx, 0, "MutationObserver", "observe")?;
    let dict = arg(vm, ctx, 1);
    // WebIDL dictionary members in lexicographic order.
    let filter_v = dict_member(vm, &dict, "attributeFilter")?;
    let attribute_filter = if filter_v.is_undefined() {
        None
    } else {
        if !matches!(filter_v, Value::Object(_)) {
            return vm.throw_type("Failed to execute 'observe' on 'MutationObserver': The provided value cannot be converted to a sequence.");
        }
        let items = vm.iterable_to_list(&filter_v)?;
        let mut out = Vec::with_capacity(items.len());
        for v in items {
            out.push(string(vm, &v)?);
        }
        Some(out)
    };
    let attribute_old_value = opt_bool(vm, &dict, "attributeOldValue")?;
    let attributes = opt_bool(vm, &dict, "attributes")?;
    let character_data = opt_bool(vm, &dict, "characterData")?;
    let character_data_old_value = opt_bool(vm, &dict, "characterDataOldValue")?;
    let child_list = opt_bool(vm, &dict, "childList")?.unwrap_or(false);
    let subtree = opt_bool(vm, &dict, "subtree")?.unwrap_or(false);
    // §4.3.1 observe() steps 1–6.
    let attributes = match attributes {
        None if attribute_old_value.is_some() || attribute_filter.is_some() => Some(true),
        a => a,
    };
    let character_data = match character_data {
        None if character_data_old_value.is_some() => Some(true),
        c => c,
    };
    let fail = "Failed to execute 'observe' on 'MutationObserver': ";
    if !child_list && attributes != Some(true) && character_data != Some(true) {
        return vm.throw_type(&format!("{fail}The options object must set at least one of 'attributes', 'characterData', or 'childList' to true."));
    }
    if attribute_old_value == Some(true) && attributes == Some(false) {
        return vm.throw_type(&format!("{fail}The options object may only set 'attributeOldValue' to true when 'attributes' is true or not present."));
    }
    if attribute_filter.is_some() && attributes == Some(false) {
        return vm.throw_type(&format!("{fail}The options object may only set 'attributeFilter' when 'attributes' is true or not present."));
    }
    if character_data_old_value == Some(true) && character_data == Some(false) {
        return vm.throw_type(&format!("{fail}The options object may only set 'characterDataOldValue' to true when 'characterData' is true or not present."));
    }
    let options = Options {
        child_list,
        attributes: attributes == Some(true),
        character_data: character_data == Some(true),
        subtree,
        attribute_old_value: attribute_old_value == Some(true),
        character_data_old_value: character_data_old_value == Some(true),
        attribute_filter,
    };
    mo::observe(id, NodeId(target), options);
    Ok(Value::Undefined)
}

fn mo_disconnect(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_observer(vm, ctx)?;
    mo::disconnect(id);
    Ok(Value::Undefined)
}

fn mo_take_records(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_observer(vm, ctx)?;
    let records = mo::take_records(id);
    Ok(records_array(vm, &records))
}

// ---------------------------------------------------------------------------------------------------
// MutationRecord
// ---------------------------------------------------------------------------------------------------

fn rec_field(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<Value> {
    match this_tagged(vm, &ctx.this, T_MUTREC) {
        Some(o) => Ok(slot(vm, o, i)),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn rec_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_TYPE)
}
fn rec_target(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_TARGET)
}
fn rec_added(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_ADDED)
}
fn rec_removed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_REMOVED)
}
fn rec_prev(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_PREV)
}
fn rec_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_NEXT)
}
fn rec_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_NAME)
}
fn rec_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_NS)
}
fn rec_old(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rec_field(vm, ctx, R_OLD)
}

/// Installs MutationObserver and MutationRecord (after window::install: replaces the platform.rs stub).
pub fn install(vm: &mut Vm) {
    let i = interface(vm, "MutationObserver", None, Some((mo_ctor, 1)));
    register_iface("MutationObserver", i);
    op(vm, i.proto, "observe", 1, mo_observe);
    op(vm, i.proto, "disconnect", 0, mo_disconnect);
    op(vm, i.proto, "takeRecords", 0, mo_take_records);
    let r = interface(vm, "MutationRecord", None, None);
    register_iface("MutationRecord", r);
    let p = r.proto;
    attr(vm, p, "type", rec_type, None);
    attr(vm, p, "target", rec_target, None);
    attr(vm, p, "addedNodes", rec_added, None);
    attr(vm, p, "removedNodes", rec_removed, None);
    attr(vm, p, "previousSibling", rec_prev, None);
    attr(vm, p, "nextSibling", rec_next, None);
    attr(vm, p, "attributeName", rec_name, None);
    attr(vm, p, "attributeNamespace", rec_ns, None);
    attr(vm, p, "oldValue", rec_old, None);
}
