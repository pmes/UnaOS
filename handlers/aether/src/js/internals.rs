//! AETHERWPT3 (LEDGER SR138): `ElementInternals` and form-associated custom elements (HTML §4.13.7 and §4.13.? "form-
//! associated custom elements"): `attachInternals()`, the internals' shadow root, form owner, submission value and
//! state, validity (`setValidity` over the ten ValidityStateFlags, the validation anchor), `willValidate` /
//! `checkValidity` / `reportValidity`, `labels`, `states` (CustomStateSet) and the ARIA mixin's stored values;
//! form association itself — a form-associated custom element is a listed, labelable, submittable, resettable
//! element (js/controls.rs, js/forms.rs ask [`is_face`]) whose definition's `formAssociatedCallback`,
//! `formDisabledCallback` and `formResetCallback` run as custom element reactions (dom/custom.rs).
//!
//! Form owner and disabled-state changes are observed, not threaded through every insertion and attribute
//! path: [`sync`] runs at the head of every custom element reaction flush (js/custom.rs, which
//! `mutation::settle` runs after each mutation algorithm), compares each such element's form owner and
//! disabled state with what it last saw, and enqueues `formAssociatedCallback(form)` /
//! `formDisabledCallback(disabled)` for the ones that changed.

use super::dom::{self, node_of, with_doc, wrap};
use super::idl::*;
use crate::dom::custom::{self as ce, Arg, Callback, CeState};
use html_core::{Namespace, NodeId};
use js_core::vm::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

/// ValidityStateFlags members, as bit positions (HTML §4.10.20.3).
pub const FLAG_NAMES: [&str; 10] =
    ["valueMissing", "typeMismatch", "patternMismatch", "tooLong", "tooShort", "rangeUnderflow", "rangeOverflow", "stepMismatch", "badInput", "customError"];

#[derive(Default)]
struct Internals {
    attached: bool,
    /// Host roots of the submission value and state (None = null).
    value: Option<usize>,
    state: Option<usize>,
    flags: u16,
    message: String,
    anchor: Option<usize>,
}

thread_local! {
    static STATE: RefCell<HashMap<usize, Internals>> = RefCell::new(HashMap::new());
    /// Form-associated custom elements → (the form owner and the disabled state last reported).
    static FACES: RefCell<BTreeMap<usize, (Option<usize>, bool)>> = const { RefCell::new(BTreeMap::new()) };
}

/// A new page: forget every element's internals.
pub fn reset() {
    STATE.with(|s| s.borrow_mut().clear());
    FACES.with(|f| f.borrow_mut().clear());
}

/// Called when `el` becomes "custom" (js/custom.rs): a form-associated definition makes it a form-associated
/// custom element.
pub fn note_custom(el: NodeId) {
    if ce::element_definition(el).and_then(ce::definition).is_some_and(|d| d.form_associated) {
        FACES.with(|f| {
            f.borrow_mut().entry(el.0).or_insert((None, false));
        });
    }
}

/// Whether `el` is a form-associated custom element (custom, with a form-associated definition).
pub fn is_face(el: usize) -> bool {
    FACES.with(|f| f.borrow().contains_key(&el))
}

/// The custom-validity flags of a form-associated custom element, as ValidityState bits.
pub fn face_flags(el: usize) -> Option<u16> {
    if !is_face(el) {
        return None;
    }
    Some(STATE.with(|s| s.borrow().get(&el).map(|i| i.flags).unwrap_or(0)))
}

/// The validation message `setValidity` set.
pub fn face_message(el: usize) -> String {
    STATE.with(|s| s.borrow().get(&el).map(|i| i.message.clone()).unwrap_or_default())
}

/// HTML "reset the form owner" / "disabled" observation (see the module comment): enqueues the
/// form-association callbacks for every form-associated custom element whose form owner or disabled state
/// changed since the last flush.
pub fn sync() {
    let all: Vec<(usize, (Option<usize>, bool))> = FACES.with(|f| f.borrow().iter().map(|(k, v)| (*k, *v)).collect());
    if all.is_empty() {
        return;
    }
    for (el, (last_form, last_dis)) in all {
        if !with_doc(|d| d.element(NodeId(el)).is_some()) {
            continue;
        }
        let form = super::controls::form_owner(el);
        let dis = super::controls::disabled(el);
        if form != last_form {
            let a = form.map(|f| Arg::Node(NodeId(f))).unwrap_or(Arg::Null);
            ce::enqueue_callback(NodeId(el), Callback::FormAssociated, vec![a]);
        }
        if dis != last_dis {
            ce::enqueue_callback(NodeId(el), Callback::FormDisabled, vec![Arg::Bool(dis)]);
        }
        FACES.with(|f| {
            f.borrow_mut().insert(el, (form, dis));
        });
    }
}

/// HTML "construct the entry list", the form-associated custom element step: the submission value's entries
/// (a FormData's, cloned when it was set) or `[name, value]` under a non-empty `name`.
pub fn append_entries(vm: &mut Vm, el: usize, pairs: &mut Vec<Value>) -> JsResult<()> {
    let Some(r) = STATE.with(|s| s.borrow().get(&el).and_then(|i| i.value)) else { return Ok(()) };
    let v = rooted(vm, r);
    if super::forms::is_form_data(vm, &v)? {
        let g = vm.realm().global;
        let f = get(vm, g, "__ei_form_data_pairs")?;
        let list = vm.call(&f, &Value::Undefined, &[v])?;
        for p in vm.array_to_list(&list) {
            pairs.push(p);
        }
        return Ok(());
    }
    let name = dom::attr_value(el, "name").unwrap_or_default();
    if name.is_empty() {
        return Ok(());
    }
    pairs.push(Value::Object(vm.new_array(vec![s(&name), v])));
    Ok(())
}

fn set_root(vm: &mut Vm, slot: &mut Option<usize>, v: Value) {
    if let Some(old) = slot.take() {
        unroot(vm, old);
    }
    if !v.is_null() && !v.is_undefined() {
        *slot = Some(root(vm, v));
    }
}

/// `__ei(op, element, …)`: the element-side steps of the ElementInternals prelude below.
fn ei(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = string(vm, &arg(vm, ctx, 0))?;
    let ev = arg(vm, ctx, 1);
    let Some(el) = node_of(vm, &ev) else { return vm.throw_type("Illegal invocation") };
    match op.as_str() {
        // HTML §4.13.7 attachInternals() steps 1-7: an error message, or undefined once attached.
        "attach" => {
            let (html, local, is, st) = with_doc(|d| {
                let e = d.element(NodeId(el));
                (
                    e.is_some_and(|e| e.ns == Namespace::Html),
                    e.map(|e| e.local.clone()).unwrap_or_default(),
                    ce::is_value(d, NodeId(el)),
                    ce::state(d, NodeId(el)),
                )
            });
            if !html {
                return vm.throw_type("Illegal invocation");
            }
            if is.is_some() {
                return Ok(s("Unable to attach ElementInternals to a customized built-in element."));
            }
            let Some(def) = ce::look_up(Namespace::Html, &local, None).and_then(ce::definition) else {
                return Ok(s("Unable to attach ElementInternals to non-custom elements."));
            };
            if def.disable_internals {
                return Ok(s("ElementInternals is disabled by disabledFeature static field."));
            }
            if STATE.with(|st| st.borrow().get(&el).is_some_and(|i| i.attached)) {
                return Ok(s("ElementInternals for the specified element was already attached."));
            }
            if !matches!(st, CeState::Precustomized | CeState::Custom) {
                return Ok(s("The attachInternals() function cannot be called prior to the execution of the custom element constructor."));
            }
            STATE.with(|st| st.borrow_mut().entry(el).or_default().attached = true);
            Ok(Value::Undefined)
        }
        "face" => Ok(Value::Bool(is_face(el))),
        "form" => {
            let f = super::controls::form_owner(el);
            Ok(match f {
                Some(f) => wrap(vm, f),
                None => Value::Null,
            })
        }
        "shadow" => {
            let r = with_doc(|d| crate::dom::shadow::shadow_root_of(d, NodeId(el)).map(|i| i.root.0));
            Ok(match r {
                Some(r) => wrap(vm, r),
                None => Value::Null,
            })
        }
        "willValidate" => Ok(Value::Bool(!super::controls::barred(el))),
        "validity" => Ok(super::controls::validity_value(vm, el)),
        "message" => {
            let m = super::controls::validation_message_of(vm, el);
            Ok(s(&m))
        }
        "labels" => Ok(super::controls::labels_value(vm, el)),
        "check" => Ok(Value::Bool(super::controls::check_one(vm, el))),
        // setFormValue(value, state): the prelude converted and cloned both.
        "setValue" => {
            let (v, st) = (arg(vm, ctx, 2), arg(vm, ctx, 3));
            let mut cur = STATE.with(|s| s.borrow_mut().remove(&el)).unwrap_or_default();
            set_root(vm, &mut cur.value, v);
            set_root(vm, &mut cur.state, st);
            STATE.with(|s| s.borrow_mut().insert(el, cur));
            Ok(Value::Undefined)
        }
        // setValidity(bits, message, anchor): the prelude checked the flags / message / anchor rules.
        "setValidity" => {
            let bits = vm.to_number(&arg(vm, ctx, 2))? as u16;
            let message = string(vm, &arg(vm, ctx, 3))?;
            let anchor = arg(vm, ctx, 4);
            let mut cur = STATE.with(|s| s.borrow_mut().remove(&el)).unwrap_or_default();
            cur.flags = bits;
            cur.message = if bits == 0 { String::new() } else { message };
            set_root(vm, &mut cur.anchor, anchor);
            STATE.with(|s| s.borrow_mut().insert(el, cur));
            Ok(Value::Undefined)
        }
        // Whether `node` (argument 2) is a shadow-including descendant of the element (the anchor rule).
        "descendant" => {
            let n = arg(vm, ctx, 2);
            let Some(n) = node_of(vm, &n) else { return Ok(Value::Bool(false)) };
            let inside = with_doc(|d| crate::dom::shadow::shadow_including_descendants(d, NodeId(el), false).into_iter().any(|x| x.0 == n));
            Ok(Value::Bool(inside))
        }
        _ => Ok(Value::Undefined),
    }
}

/// Installs `__ei` and the prelude (after `custom::install`: HTMLElement exists, the registry is live).
pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__ei", 5, ei, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__ei"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("internals-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] ElementInternals prelude failed: {m}");
    }
}

/// ElementInternals, CustomStateSet and `attachInternals` (HTML §4.13.7, §4.13.7.? custom state pseudo-class
/// state set; the ARIAMixin of WAI-ARIA §10.1 / HTML's "ARIA reflection", stored on the internals).
const PRELUDE: &str = r#"
(function () {
    if (typeof HTMLElement !== 'function' || typeof __ei !== 'function') { return; }
    var EI = new WeakMap();
    var slot = function (o) {
        var s = (o !== null && typeof o === 'object') ? EI.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var face = function (s, what) {
        if (!__ei('face', s.el)) {
            throw new DOMException("Failed to " + what + " 'ElementInternals': The target element is not a form-associated custom element.", 'NotSupportedError');
        }
    };
    var acc = function (o, n, g, st) { Object.defineProperty(o, n, { get: g, set: st, enumerable: true, configurable: true }); };
    var op = function (o, n, f) { Object.defineProperty(o, n, { value: f, writable: true, enumerable: true, configurable: true }); };

    // ---- CustomStateSet (a setlike<DOMString>) ----
    var CS = new WeakMap();
    var cs = function (o) {
        var s = (o !== null && typeof o === 'object') ? CS.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    class CustomStateSet { constructor() { throw new TypeError('Illegal constructor'); } }
    var CSP = CustomStateSet.prototype;
    acc(CSP, 'size', function () { return cs(this).size; });
    op(CSP, 'add', function (v) { cs(this).add(String(v)); return this; });
    op(CSP, 'delete', function (v) { return cs(this).delete(String(v)); });
    op(CSP, 'has', function (v) { return cs(this).has(String(v)); });
    op(CSP, 'clear', function () { cs(this).clear(); });
    op(CSP, 'forEach', function (f) {
        var self = this, thisArg = arguments[1];
        if (typeof f !== 'function') { throw new TypeError("Failed to execute 'forEach' on 'CustomStateSet': The callback provided as parameter 1 is not a function."); }
        cs(this).forEach(function (v) { f.call(thisArg, v, v, self); });
    });
    op(CSP, 'entries', function () { return cs(this).entries(); });
    op(CSP, 'values', function () { return cs(this).values(); });
    Object.defineProperty(CSP, 'keys', { value: CSP.values, writable: true, enumerable: true, configurable: true });
    Object.defineProperty(CSP, Symbol.iterator, { value: CSP.values, writable: true, enumerable: false, configurable: true });
    Object.defineProperty(CSP, Symbol.toStringTag, { value: 'CustomStateSet', configurable: true });

    // ---- ElementInternals ----
    class ElementInternals { constructor() { throw new TypeError('Illegal constructor'); } }
    var P = ElementInternals.prototype;
    acc(P, 'shadowRoot', function () { return __ei('shadow', slot(this).el); });
    acc(P, 'form', function () { var s = slot(this); face(s, "read the 'form' property from"); return __ei('form', s.el); });
    acc(P, 'willValidate', function () { var s = slot(this); face(s, "read the 'willValidate' property from"); return __ei('willValidate', s.el); });
    acc(P, 'validity', function () { var s = slot(this); face(s, "read the 'validity' property from"); return __ei('validity', s.el); });
    acc(P, 'validationMessage', function () { var s = slot(this); face(s, "read the 'validationMessage' property from"); return __ei('message', s.el); });
    acc(P, 'labels', function () { var s = slot(this); face(s, "read the 'labels' property from"); return __ei('labels', s.el); });
    acc(P, 'states', function () { return slot(this).states; });
    // setFormValue((File or USVString or FormData)? value, optional (File or USVString or FormData)? state)
    var formValue = function (v) {
        if (v === null || v === undefined) { return null; }
        if (typeof FormData === 'function' && v instanceof FormData) {
            var copy = new FormData();
            for (var e of v) { copy.append(e[0], e[1]); }
            return copy;
        }
        if (typeof File === 'function' && v instanceof File) { return v; }
        return String(v);
    };
    op(P, 'setFormValue', function setFormValue(value) {
        var s = slot(this);
        if (arguments.length < 1) { throw new TypeError("Failed to execute 'setFormValue' on 'ElementInternals': 1 argument required, but only 0 present."); }
        face(s, "execute 'setFormValue' on");
        var v = formValue(value);
        var st = arguments.length < 2 ? v : formValue(arguments[1]);
        __ei('setValue', s.el, v, st);
    });
    var FLAGS = ['valueMissing', 'typeMismatch', 'patternMismatch', 'tooLong', 'tooShort', 'rangeUnderflow', 'rangeOverflow', 'stepMismatch', 'badInput', 'customError'];
    var DICT_ORDER = FLAGS.slice().sort();
    op(P, 'setValidity', function setValidity() {
        var s = slot(this);
        var flags = arguments[0], message = arguments[1], anchor = arguments[2];
        if (flags !== undefined && flags !== null && typeof flags !== 'object' && typeof flags !== 'function') {
            throw new TypeError("Failed to execute 'setValidity' on 'ElementInternals': The provided value is not of type 'ValidityStateFlags'.");
        }
        var got = {};
        DICT_ORDER.forEach(function (n) { got[n] = flags ? !!flags[n] : false; });
        if (message !== undefined) { message = String(message); }
        if (anchor !== undefined && !(anchor instanceof HTMLElement)) {
            throw new TypeError("Failed to execute 'setValidity' on 'ElementInternals': parameter 3 is not of type 'HTMLElement'.");
        }
        face(s, "execute 'setValidity' on");
        var bits = 0;
        FLAGS.forEach(function (n, i) { if (got[n]) { bits |= 1 << i; } });
        if (bits !== 0 && (message === undefined || message === '')) {
            throw new TypeError("Failed to execute 'setValidity' on 'ElementInternals': The second argument should not be empty if one or more flags in the first argument are true.");
        }
        if (anchor !== undefined && !__ei('descendant', s.el, anchor)) {
            throw new DOMException("Failed to execute 'setValidity' on 'ElementInternals': The anchor element is not a shadow-including descendant of the element.", 'NotFoundError');
        }
        var m = message === undefined ? '' : message.replace(/\r\n|\r/g, '\n');
        __ei('setValidity', s.el, bits, m, anchor === undefined ? null : anchor);
    });
    op(P, 'checkValidity', function checkValidity() { var s = slot(this); face(s, "execute 'checkValidity' on"); return __ei('check', s.el); });
    op(P, 'reportValidity', function reportValidity() { var s = slot(this); face(s, "execute 'reportValidity' on"); return __ei('check', s.el); });
    // ARIAMixin (stored values; WAI-ARIA reflection on the internals is not reflected to attributes).
    ['role', 'ariaAtomic', 'ariaAutoComplete', 'ariaBrailleLabel', 'ariaBrailleRoleDescription', 'ariaBusy', 'ariaChecked',
     'ariaColCount', 'ariaColIndex', 'ariaColIndexText', 'ariaColSpan', 'ariaCurrent', 'ariaDescription', 'ariaDisabled',
     'ariaExpanded', 'ariaHasPopup', 'ariaHidden', 'ariaInvalid', 'ariaKeyShortcuts', 'ariaLabel', 'ariaLevel', 'ariaLive',
     'ariaModal', 'ariaMultiLine', 'ariaMultiSelectable', 'ariaOrientation', 'ariaPlaceholder', 'ariaPosInSet', 'ariaPressed',
     'ariaReadOnly', 'ariaRelevant', 'ariaRequired', 'ariaRoleDescription', 'ariaRowCount', 'ariaRowIndex', 'ariaRowIndexText',
     'ariaRowSpan', 'ariaSelected', 'ariaSetSize', 'ariaSort', 'ariaValueMax', 'ariaValueMin', 'ariaValueNow', 'ariaValueText'].forEach(function (n) {
        acc(P, n, function () { var a = slot(this).aria; return n in a ? a[n] : null; },
            function (v) { slot(this).aria[n] = v === null || v === undefined ? null : String(v); });
    });
    ['ariaActiveDescendantElement'].forEach(function (n) {
        acc(P, n, function () { var a = slot(this).aria; return n in a ? a[n] : null; },
            function (v) {
                if (v !== null && v !== undefined && !(v instanceof Element)) { throw new TypeError("Failed to set the '" + n + "' property on 'ElementInternals': The provided value is not of type 'Element'."); }
                slot(this).aria[n] = v === undefined ? null : v;
            });
    });
    ['ariaControlsElements', 'ariaDescribedByElements', 'ariaDetailsElements', 'ariaErrorMessageElements', 'ariaFlowToElements',
     'ariaLabelledByElements', 'ariaOwnsElements'].forEach(function (n) {
        acc(P, n, function () { var a = slot(this).aria; return n in a && a[n] !== null ? Object.freeze(a[n].slice()) : null; },
            function (v) {
                if (v === null || v === undefined) { slot(this).aria[n] = null; return; }
                var list = [];
                for (var x of v) {
                    if (!(x instanceof Element)) { throw new TypeError("Failed to set the '" + n + "' property on 'ElementInternals': The provided value is not of type 'Element'."); }
                    list.push(x);
                }
                slot(this).aria[n] = list;
            });
    });
    Object.defineProperty(P, Symbol.toStringTag, { value: 'ElementInternals', configurable: true });

    Object.defineProperty(HTMLElement.prototype, 'attachInternals', {
        value: function attachInternals() {
            var r = __ei('attach', this);
            if (typeof r === 'string') { throw new DOMException("Failed to execute 'attachInternals' on 'HTMLElement': " + r, 'NotSupportedError'); }
            var states = Object.create(CSP);
            CS.set(states, new Set());
            var o = Object.create(P);
            EI.set(o, { el: this, states: states, aria: {} });
            return o;
        },
        writable: true, enumerable: true, configurable: true,
    });
    Object.defineProperty(globalThis, '__ei_form_data_pairs', {
        value: function (fd) { var out = []; for (var e of fd) { out.push([e[0], e[1]]); } return out; },
        writable: true, configurable: true, enumerable: false,
    });
    [ElementInternals, CustomStateSet].forEach(function (C) {
        Object.defineProperty(globalThis, C.name, { value: C, writable: true, configurable: true, enumerable: false });
    });
})();
"#;
