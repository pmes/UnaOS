//! AETHERAPIS (LEDGER SR145): the top layer and what shows in it — the popover API (HTML §6.12: the `popover`
//! attribute, `showPopover()` / `hidePopover()` / `togglePopover()`, "check popover validity", "show popover",
//! "hide popover", "hide all popovers until", "topmost popover ancestor", the auto / hint stacks, light dismiss,
//! the `popovertarget` / `popovertargetaction` invokers, the `beforetoggle` / `toggle` ToggleEvents with their
//! toggle-task tracker, the removing and attribute-change steps) and the dialog element (HTML §4.11.4: `show()`,
//! `showModal()`, `close()`, `requestClose()`, `returnValue`, `closedBy`, the dialog focusing steps, the `cancel` /
//! `close` events, the toggle events, the close watcher a modal dialog establishes, the `open` attribute change
//! and removing steps).
//!
//! CHARTER: Aether — binding.
//!
//! The DOM and event half is here; the rendering half is owed (docs/dev/evidence/aetherwpt/WPT.md §AETHERAPIS):
//! css_core parses no `:popover-open` / `:modal`, so a hidden popover has no UA `display: none` rule and `matches()`
//! on those pseudo-classes throws; a closed dialog does get its UA rule (`crate::css` UA sheet). Nothing paints the
//! top layer above the document or a `::backdrop`.
//!
//! Hooks from the engine: [`invoker_activation`] (a button's or input's activation behavior, js/events.rs),
//! [`light_dismiss`] (a trusted click), [`node_removed`] (DOM removal, js/dom.rs), [`attribute_changed`] (the
//! `popover` and dialog `open` attributes, js/dom.rs); all are no-ops while nothing is showing (`TRACKED`).

use super::idl::arg;
use super::Task;
use js_core::vm::*;
use std::cell::Cell;

thread_local! {
    /// How many elements the prelude is tracking (showing popovers, open dialogs): the hooks skip the JS call at 0.
    static TRACKED: Cell<u32> = const { Cell::new(0) };
}

pub fn reset() {
    TRACKED.with(|t| t.set(0));
}

fn call(vm: &mut Vm, name: &str, args: &[Value]) -> Option<Value> {
    if super::engine_poisoned() {
        return None;
    }
    let g = vm.realm().global;
    let f = vm.get(g, &PropertyKey::from_str(name)).unwrap_or(Value::Undefined);
    if !vm.is_callable(&f) {
        return None;
    }
    let r = super::invoke(vm, name, &f, &Value::Undefined, args);
    super::checkpoint(vm);
    r
}

/// HTML §6.12.3 the popover target attribute activation behavior of a button or input `el`.
pub fn invoker_activation(vm: &mut Vm, el: usize) {
    let v = super::dom::wrap(vm, el);
    call(vm, "__tl_invoke", &[v]);
}

/// HTML §6.12.2 "light dismiss open popovers" for a trusted click whose target is `target`.
pub fn light_dismiss(vm: &mut Vm, target: usize) {
    if TRACKED.with(Cell::get) == 0 {
        return;
    }
    let v = super::dom::wrap(vm, target);
    call(vm, "__tl_light_dismiss", &[v]);
}

/// The removing steps of popovers, dialogs and the fullscreen element under `node` (already detached).
pub fn node_removed(vm: &mut Vm, node: usize) {
    if TRACKED.with(Cell::get) == 0 && super::fullscreen::element().is_none() {
        return;
    }
    let v = super::dom::wrap(vm, node);
    call(vm, "__tl_removed", &[v]);
}

/// The `popover` attribute change steps and the dialog `open` attribute change steps.
pub fn attribute_changed(vm: &mut Vm, el: usize, local: &str, old: &Option<String>, new: &Option<String>) {
    if !matches!(local, "popover" | "open") || TRACKED.with(Cell::get) == 0 {
        return;
    }
    let v = super::dom::wrap(vm, el);
    let o = old.as_deref().map(super::idl::s).unwrap_or(Value::Null);
    let n = new.as_deref().map(super::idl::s).unwrap_or(Value::Null);
    call(vm, "__tl_attr", &[v, super::idl::s(local), o, n]);
}

/// `__tl(op, arg)`: `queue(fn)` queues `fn` as a task; `track(n)` records how many elements are showing.
fn native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let op = vm.to_string(&v)?.to_rust();
    match op.as_str() {
        "queue" => {
            let f = arg(vm, ctx, 1);
            if vm.is_callable(&f) {
                let r = super::idl::root(vm, f);
                super::page(|p| p.tasks.push_back(Task::Callback(r)));
            }
        }
        "track" => {
            let v = arg(vm, ctx, 1);
            let n = vm.to_number(&v)?;
            TRACKED.with(|t| t.set(n.max(0.0) as u32));
        }
        _ => {}
    }
    Ok(Value::Undefined)
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__tl", 2, native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__tl"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("toplayer-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] toplayer prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var def = function (proto, name, f) {
        Object.defineProperty(proto, name, { value: f, writable: true, configurable: true, enumerable: true });
    };
    var accessor = function (proto, name, get, set) {
        Object.defineProperty(proto, name, { get: get, set: set, configurable: true, enumerable: true });
    };
    var domex = function (name, msg) { return new DOMException(msg, name); };
    var EP = Element.prototype, HP = HTMLElement.prototype, DP = Document.prototype;
    var isDialog = typeof HTMLDialogElement === 'function' ? function (el) { return el instanceof HTMLDialogElement; } : function () { return false; };
    // ----- per-element state -----
    var S = new WeakMap();
    var st = function (el) {
        var s = S.get(el);
        if (!s) {
            s = { vis: 'hidden', invoker: null, toggle: null, prevFocus: null, modal: false, cw: null, returnValue: '', closing: false, closeSource: null, fs: false };
            S.set(el, s);
        }
        return s;
    };
    var topLayer = [];       // the document's top layer, in order
    var autoList = [];       // showing auto popovers
    var hintList = [];       // showing hint popovers
    var dialogs = [];        // open dialogs (for light dismiss with closedby=any)
    var track = function () { __tl('track', topLayer.length + autoList.length + hintList.length + dialogs.length + (fsElement() ? 1 : 0)); };
    var fsElement = function () { return typeof __fs === 'function' ? __fs('get') : null; };
    var remove = function (list, el) { var i = list.indexOf(el); if (i >= 0) { list.splice(i, 1); } };
    var queueTask = function (f) { __tl('queue', f); };
    var flatParent = function (n) {
        if (!n) { return null; }
        if (n.assignedSlot) { return n.assignedSlot; }
        var p = n.parentNode;
        if (p && p.nodeType === 11 && p.host) { return p.host; }
        return p;
    };
    var fireToggle = function (el, type, oldState, newState, cancelable) {
        var e = new ToggleEvent(type, { cancelable: !!cancelable, oldState: oldState, newState: newState });
        el.dispatchEvent(e);
        return !e.defaultPrevented;
    };
    // "queue a popover toggle event task" / "queue a dialog toggle event task" (HTML §6.12.1 / §4.11.4)
    var queueToggle = function (el, oldState, newState) {
        var s = st(el);
        if (s.toggle) { oldState = s.toggle.oldState; s.toggle.cancelled = true; s.toggle = null; }
        var tracker = { oldState: oldState, newState: newState, cancelled: false };
        s.toggle = tracker;
        queueTask(function () {
            if (tracker.cancelled) { return; }
            s.toggle = null;
            fireToggle(el, 'toggle', tracker.oldState, tracker.newState, false);
        });
    };
    // ----- focus helpers -----
    var focusable = function (el) {
        if (el.nodeType !== 1) { return false; }
        if (el.hasAttribute('hidden') || el.hasAttribute('inert')) { return false; }
        if (el.hasAttribute('tabindex')) { return true; }
        var t = el.localName;
        if (t === 'a' || t === 'area') { return el.hasAttribute('href'); }
        if (t === 'input') { return el.type !== 'hidden' && !el.disabled; }
        if (t === 'button' || t === 'select' || t === 'textarea') { return !el.disabled; }
        if (t === 'iframe' || t === 'summary') { return true; }
        if (el.isContentEditable) { return true; }
        return false;
    };
    var firstDescendant = function (root, pred) {
        var walk = function (n) {
            for (var c = n.firstChild; c; c = c.nextSibling) {
                if (c.nodeType !== 1) { continue; }
                if (pred(c)) { return c; }
                if (c.shadowRoot) { var h = walk(c.shadowRoot); if (h) { return h; } }
                var r = walk(c);
                if (r) { return r; }
            }
            return null;
        };
        return walk(root);
    };
    // HTML "autofocus delegate": the element itself when it has autofocus, else its first autofocus descendant.
    var autofocusDelegate = function (subject) {
        return firstDescendant(subject, function (c) { return c.hasAttribute('autofocus') && focusable(c); });
    };
    var runFocus = function (el) { try { el.focus(); } catch (e) {} };
    // HTML §4.11.4 "dialog focusing steps"
    var dialogFocusingSteps = function (subject) {
        var control = null;
        if (subject.hasAttribute('autofocus')) { control = subject; }
        if (!control) { control = autofocusDelegate(subject); }
        if (!control) { control = firstDescendant(subject, focusable); }
        if (!control) { control = subject; }
        runFocus(control);
    };
    // HTML §6.12.1 "popover focusing steps"
    var popoverFocusingSteps = function (subject) {
        var control = subject.hasAttribute('autofocus') ? subject : autofocusDelegate(subject);
        if (!control) { return; }
        runFocus(control);
    };
    // ----- popovers (HTML §6.12) -----
    var popoverOf = function (el) { return el.popover; };
    var showing = function (el) { return st(el).vis === 'showing'; };
    hide(globalThis, '__tl_is_popover_showing', showing);
    hide(globalThis, '__tl_is_modal', function (el) { return st(el).modal; });
    // "check popover validity"
    var checkValidity = function (el, expectedToBeShowing, throwExceptions, expectedDocument, ignoreDom) {
        var fail = function (name, msg) { if (throwExceptions) { throw domex(name, msg); } return false; };
        if (popoverOf(el) === null) { return fail('NotSupportedError', 'Not supported on elements that do not have a valid value for the popover attribute.'); }
        var s = st(el);
        if (expectedToBeShowing && s.vis !== 'showing') { return false; }
        if (!expectedToBeShowing && s.vis !== 'hidden') { return false; }
        if (ignoreDom) { return true; }
        if (!el.isConnected) { return fail('InvalidStateError', 'Invalid on popover elements which are not connected.'); }
        if (expectedDocument && el.ownerDocument !== expectedDocument) { return fail('InvalidStateError', 'Invalid when the document changes while showing or hiding a popover element.'); }
        if (isDialog(el) && el.hasAttribute('open')) { return fail('InvalidStateError', 'The dialog is already open as a dialog, and therefore cannot be opened as a popover.'); }
        if (fsElement() === el) { return fail('InvalidStateError', 'This element is already in fullscreen, and therefore cannot be opened as a popover.'); }
        return true;
    };
    // "nearest inclusive open popover" of node in list
    var nearestOpen = function (node, list) {
        var n = node;
        while (n) {
            if (n.nodeType === 1 && list.indexOf(n) >= 0) { return n; }
            n = flatParent(n);
        }
        return null;
    };
    // HTML §6.12.1 "topmost popover ancestor" for newPopover (or for an invoker-only check) in list.
    var topmostAncestor = function (newPopover, list, invoker, isPopover) {
        var best = null, bestIdx = -1;
        var consider = function (node) {
            var p = nearestOpen(node, list);
            while (p) {
                var i = list.indexOf(p);
                if (i > bestIdx) { best = p; bestIdx = i; }
                var inv = st(p).invoker;
                p = inv ? nearestOpen(inv, list) : null;
                if (p && list.indexOf(p) <= bestIdx) { break; }
            }
        };
        consider(isPopover ? flatParent(newPopover) : newPopover);
        if (invoker) { consider(invoker); }
        return best;
    };
    var hidePopover;
    // "hide all popovers until" (endpoint: a popover, or null for the document)
    var hideAllUntil = function (endpoint, focusPrev, fireEvents) {
        var hideStack = function (list, end) {
            var repeating = true;
            var guard = 0;
            while (repeating && guard++ < 1000) {
                repeating = false;
                var idx = end ? list.indexOf(end) : -1;
                if (end && idx < 0) { return; }
                var top = list.length - 1;
                if (top > idx) {
                    var n0 = list.length;
                    hidePopover(list[top], focusPrev, fireEvents, false);
                    if (list.length === n0 && list.indexOf(list[top]) === top) { return; }
                    repeating = true;
                }
            }
        };
        if (endpoint === null) { hideStack(hintList, null); hideStack(autoList, null); return; }
        if (hintList.indexOf(endpoint) >= 0) { hideStack(hintList, endpoint); return; }
        hideStack(hintList, null);
        hideStack(autoList, endpoint);
    };
    // "show popover"
    var showPopover = function (el, throwExceptions, invoker) {
        if (!checkValidity(el, false, throwExceptions, null)) { return; }
        var doc = el.ownerDocument;
        if (!fireToggle(el, 'beforetoggle', 'closed', 'open', true)) { return; }
        if (!checkValidity(el, false, throwExceptions, doc)) { return; }
        var shouldRestoreFocus = false;
        var originalType = popoverOf(el);
        var stack = null;
        if (originalType === 'auto' || originalType === 'hint') {
            var list = originalType === 'hint' ? hintList : autoList;
            var ancestor = topmostAncestor(el, originalType === 'hint' ? hintList.concat(autoList) : autoList, invoker, true);
            if (originalType === 'hint') {
                if (ancestor && hintList.indexOf(ancestor) < 0) { hideStack2(hintList); } else if (!ancestor) { hideStack2(hintList); }
                if (ancestor && autoList.indexOf(ancestor) >= 0) { hideAllUntil(ancestor, false, true); }
            } else {
                hideAllUntil(ancestor, false, true);
            }
            if (popoverOf(el) !== originalType) {
                if (throwExceptions) { throw domex('InvalidStateError', 'The value of the popover attribute was changed while hiding the popover.'); }
                return;
            }
            if (!checkValidity(el, false, throwExceptions, doc)) { return; }
            if (list.length === 0) { shouldRestoreFocus = true; }
            stack = list;
        }
        var s = st(el);
        s.invoker = invoker || null;
        topLayer.push(el);
        if (stack) { stack.push(el); }
        s.vis = 'showing';
        if (shouldRestoreFocus) {
            var focused = doc.activeElement;
            s.prevFocus = focused && focused !== doc.body ? focused : null;
        }
        if (originalType === 'auto' || originalType === 'hint') {
            s.cw = __cw_make({
                cancel: function () { return true; },
                close: function () { hidePopover(el, true, true, false); },
                enabled: function () { return showing(el) && (popoverOf(el) === 'auto' || popoverOf(el) === 'hint'); }
            });
        }
        popoverFocusingSteps(el);
        queueToggle(el, 'closed', 'open');
        track();
    };
    var hideStack2 = function (list) { while (list.length) { var n0 = list.length; hidePopover(list[list.length - 1], false, true, false); if (list.length === n0) { break; } } };
    // "hide popover"
    hidePopover = function (el, focusPrev, fireEvents, throwExceptions, ignoreDom) {
        if (!checkValidity(el, true, throwExceptions, null, ignoreDom)) { return; }
        var doc = el.ownerDocument;
        var type = popoverOf(el);
        var s = st(el);
        if (type === 'auto' || type === 'hint') {
            hideAllUntil(el, focusPrev, fireEvents);
            if (!checkValidity(el, true, throwExceptions, doc, ignoreDom)) { return; }
        }
        if (s.cw) { s.cw.destroy(); s.cw = null; }
        s.invoker = null;
        if (fireEvents) {
            fireToggle(el, 'beforetoggle', 'open', 'closed', false);
            if (!checkValidity(el, true, throwExceptions, doc, ignoreDom)) { return; }
        }
        remove(topLayer, el);
        remove(autoList, el);
        remove(hintList, el);
        s.vis = 'hidden';
        if (fireEvents) { queueToggle(el, 'open', 'closed'); }
        var prev = s.prevFocus;
        s.prevFocus = null;
        if (prev && focusPrev) {
            var active = doc.activeElement;
            if (!active || active === doc.body || el.contains(active)) { runFocus(prev); }
        }
        track();
    };
    def(HP, 'showPopover', function showPopover_(options) {
        var source = options && typeof options === 'object' ? options.source : undefined;
        if (source !== undefined && source !== null && !(source instanceof HTMLElement)) { throw new TypeError("Failed to execute 'showPopover' on 'HTMLElement': The provided value is not of type 'HTMLElement'."); }
        showPopover(this, true, source || null);
    });
    def(HP, 'hidePopover', function hidePopover_(options) {
        hidePopover(this, true, true, true);
    });
    def(HP, 'togglePopover', function togglePopover(options) {
        var force, source = null;
        if (options !== null && typeof options === 'object') {
            force = options.force;
            source = options.source === undefined ? null : options.source;
            if (source !== null && !(source instanceof HTMLElement)) { throw new TypeError("Failed to execute 'togglePopover' on 'HTMLElement': The provided value is not of type 'HTMLElement'."); }
        } else if (options !== undefined) {
            force = !!options;
        }
        if (showing(this) && force !== true) {
            hidePopover(this, true, true, true);
        } else if (force !== false) {
            showPopover(this, true, source);
        } else {
            checkValidity(this, false, true, null);
        }
        return showing(this);
    });
    // popoverTargetElement (HTML §2.6.10 reflecting an Element)
    var explicitTarget = new WeakMap();
    var targetOf = function (el) {
        var ex = explicitTarget.get(el);
        if (ex !== undefined) {
            if (ex === null) { return null; }
            if (ex.isConnected && ex.getRootNode() === el.getRootNode()) { return ex; }
            return null;
        }
        var id = el.getAttribute('popovertarget');
        if (id === null || !el.isConnected) { return null; }
        var root = el.getRootNode();
        if (!root || typeof root.getElementById !== 'function') { return null; }
        return root.getElementById(id);
    };
    ['HTMLButtonElement', 'HTMLInputElement'].forEach(function (name) {
        var C = globalThis[name];
        if (typeof C !== 'function') { return; }
        accessor(C.prototype, 'popoverTargetElement', function () { return targetOf(this); }, function (v) {
            if (v === null) { explicitTarget.set(this, null); this.removeAttribute('popovertarget'); return; }
            if (!(v instanceof Element)) { throw new TypeError("Failed to set the 'popoverTargetElement' property on '" + name + "': Failed to convert value to 'Element'."); }
            explicitTarget.set(this, v);
            this.setAttribute('popovertarget', '');
        });
    });
    // HTML §6.12.3 popover target attribute activation behavior
    hide(globalThis, '__tl_invoke', function (node) {
        if (node.disabled) { return; }
        if (node.form && (node.type === 'submit' || node.type === 'image')) { return; }
        var target = targetOf(node);
        if (!target) { return; }
        var action = node.popoverTargetAction;
        var isShowing = showing(target);
        if (action === 'show' && isShowing) { return; }
        if (action === 'hide' && !isShowing) { return; }
        if (!isShowing) { showPopover(target, false, node); } else { hidePopover(target, true, true, false); }
    });
    // HTML §6.12.2 light dismiss open popovers, for a click whose target is `target`.
    hide(globalThis, '__tl_light_dismiss', function (target) {
        var all = hintList.concat(autoList);
        if (all.length > 0) {
            var clicked = topmostAncestor(target, all, null, false);
            var n = target;
            while (n && !clicked) {
                if (n.nodeType === 1 && (n.localName === 'button' || n.localName === 'input')) {
                    var t = targetOf(n);
                    if (t && showing(t)) { clicked = t; }
                }
                n = flatParent(n);
            }
            hideAllUntil(clicked, false, true);
        }
        dialogs.slice().forEach(function (d) {
            if (!d.isConnected || d.closedBy !== 'any') { return; }
            var inside = false;
            for (var m = target; m; m = flatParent(m)) { if (m === d) { inside = true; break; } }
            if (!inside) { requestCloseDialog(d, null); }
        });
    });
    // The removing steps (the subtree under `node` left the document).
    hide(globalThis, '__tl_removed', function (node) {
        var under = function (el) { for (var n = el; n; n = flatParent(n)) { if (n === node) { return true; } } return false; };
        topLayer.slice().forEach(function (el) {
            if (!under(el)) { return; }
            if (popoverOf(el) !== null && showing(el)) { hidePopover(el, false, false, false, true); }
            if (isDialog(el)) { var s = st(el); if (s.cw) { s.cw.destroy(); s.cw = null; } remove(topLayer, el); s.modal = false; }
        });
        dialogs.slice().forEach(function (d) { if (under(d)) { var s = st(d); if (s.cw) { s.cw.destroy(); s.cw = null; } remove(dialogs, d); s.modal = false; } });
        var fs = fsElement();
        if (fs && under(fs) && typeof __fs_removed === 'function') { __fs_removed(); }
        track();
    });
    // The popover attribute change steps and the dialog open attribute change steps.
    hide(globalThis, '__tl_attr', function (el, name, oldValue, value) {
        if (name === 'popover') {
            if (!showing(el)) { return; }
            var norm = function (v) { if (v === null) { return null; } var l = String(v).toLowerCase(); return l === '' || l === 'auto' ? 'auto' : (l === 'hint' ? 'hint' : 'manual'); };
            if (norm(oldValue) !== norm(value)) {
                var s = st(el);
                var was = s.vis;
                // the element is still a popover of the old kind for the hide: force the hidden transition
                if (value === null) {
                    s.cw && s.cw.destroy(); s.cw = null;
                    fireToggle(el, 'beforetoggle', 'open', 'closed', false);
                    remove(topLayer, el); remove(autoList, el); remove(hintList, el);
                    s.vis = 'hidden'; s.invoker = null;
                    queueToggle(el, 'open', 'closed');
                    track();
                } else {
                    hidePopover(el, true, true, false);
                }
                void was;
            }
            return;
        }
        if (name === 'open' && isDialog(el)) {
            var ds = st(el);
            if (value === null && oldValue !== null && !ds.closing) {
                ds.modal = false;
                remove(topLayer, el);
                remove(dialogs, el);
                if (ds.cw) { ds.cw.destroy(); ds.cw = null; }
                queueTask(function () { el.dispatchEvent(new Event('close')); });
                track();
            }
        }
    });
    // ----- the dialog element (HTML §4.11.4) -----
    var computedClosedBy = function (d) {
        var v = d.getAttribute('closedby');
        var l = v === null ? null : v.toLowerCase();
        if (l === 'any' || l === 'closerequest' || l === 'none') { return l; }
        return st(d).modal ? 'closerequest' : 'none';
    };
    var makeDialogWatcher = function (d) {
        return __cw_make({
            cancel: function (canPreventClose) {
                var e = new Event('cancel', { cancelable: canPreventClose });
                d.dispatchEvent(e);
                return !e.defaultPrevented;
            },
            close: function () { closeDialog(d, null); },
            enabled: function () { return d.hasAttribute('open') && computedClosedBy(d) !== 'none'; }
        });
    };
    var setupDialog = function (d, modal) {
        var s = st(d);
        queueToggle(d, 'closed', 'open');
        s.closing = true;
        try { d.setAttribute('open', ''); } finally { s.closing = false; }
        s.modal = modal;
        s.returnValue = s.returnValue || '';
        if (modal) { topLayer.push(d); }
        if (dialogs.indexOf(d) < 0) { dialogs.push(d); }
        if (s.cw) { s.cw.destroy(); s.cw = null; }
        if (modal || computedClosedBy(d) !== 'none') { s.cw = makeDialogWatcher(d); }
        var doc = d.ownerDocument;
        var focused = doc.activeElement;
        s.prevFocus = focused && focused !== doc.body ? focused : null;
        track();
        dialogFocusingSteps(d);
    };
    var closeDialog = function (d, result) {
        if (!d.hasAttribute('open')) { return; }
        fireToggle(d, 'beforetoggle', 'open', 'closed', false);
        if (!d.hasAttribute('open')) { return; }
        queueToggle(d, 'open', 'closed');
        var s = st(d);
        var wasModal = s.modal;
        s.closing = true;
        try { d.removeAttribute('open'); } finally { s.closing = false; }
        s.modal = false;
        remove(topLayer, d);
        remove(dialogs, d);
        if (s.cw) { s.cw.destroy(); s.cw = null; }
        if (result !== null && result !== undefined) { s.returnValue = String(result); }
        s.closeSource = null;
        var prev = s.prevFocus;
        if (prev) {
            s.prevFocus = null;
            var active = d.ownerDocument.activeElement;
            var inside = false;
            for (var n = active; n; n = flatParent(n)) { if (n === d) { inside = true; break; } }
            if (inside || wasModal || !active || active === d.ownerDocument.body) { runFocus(prev); }
        }
        queueTask(function () { d.dispatchEvent(new Event('close')); });
        track();
    };
    var requestCloseDialog = function (d, result) {
        if (!d.hasAttribute('open')) { return; }
        var s = st(d);
        if (!s.cw) { closeDialog(d, result); return; }
        s.pendingReturn = result;
        var cw = s.cw;
        var w = cw;
        // the close action closes with the requested return value
        var ok = w.requestClose(false);
        void ok;
    };
    if (typeof HTMLDialogElement === 'function') {
        var DLP = HTMLDialogElement.prototype;
        def(DLP, 'show', function show() {
            var d = this, s = st(d);
            if (d.hasAttribute('open')) {
                if (s.modal) { throw domex('InvalidStateError', "The element already has an 'open' attribute, and therefore cannot be opened modelessly."); }
                return;
            }
            if (!d.isConnected) { throw domex('InvalidStateError', 'The element is not in a Document.'); }
            if (showing(d)) { throw domex('InvalidStateError', 'The dialog is already open as a popover, and therefore cannot be opened modelessly.'); }
            if (!fireToggle(d, 'beforetoggle', 'closed', 'open', true)) { return; }
            if (d.hasAttribute('open')) { return; }
            setupDialog(d, false);
        });
        def(DLP, 'showModal', function showModal() {
            var d = this, s = st(d);
            if (d.hasAttribute('open')) {
                if (s.modal) { return; }
                throw domex('InvalidStateError', "The element already has an 'open' attribute, and therefore cannot be opened modally.");
            }
            if (!d.isConnected) { throw domex('InvalidStateError', 'The element is not in a Document.'); }
            if (showing(d)) { throw domex('InvalidStateError', 'The dialog is already open as a popover, and therefore cannot be opened modally.'); }
            if (!fireToggle(d, 'beforetoggle', 'closed', 'open', true)) { return; }
            if (d.hasAttribute('open')) { return; }
            if (!d.isConnected) { throw domex('InvalidStateError', 'The element is not in a Document.'); }
            if (showing(d)) { throw domex('InvalidStateError', 'The dialog is already open as a popover, and therefore cannot be opened modally.'); }
            setupDialog(d, true);
        });
        def(DLP, 'close', function close(returnValue) {
            closeDialog(this, returnValue === undefined ? null : String(returnValue));
        });
        def(DLP, 'requestClose', function requestClose(returnValue) {
            var d = this;
            if (!d.hasAttribute('open')) { return; }
            var s = st(d);
            if (!s.cw) { closeDialog(d, returnValue === undefined ? null : String(returnValue)); return; }
            var rv = returnValue === undefined ? null : String(returnValue);
            var cw = s.cw;
            s.cw = __cw_make({
                cancel: function (canPreventClose) { var e = new Event('cancel', { cancelable: canPreventClose }); d.dispatchEvent(e); return !e.defaultPrevented; },
                close: function () { closeDialog(d, rv); },
                enabled: function () { return d.hasAttribute('open') && computedClosedBy(d) !== 'none'; }
            });
            cw.destroy();
            var w = s.cw;
            w.requestClose(false);
            if (d.hasAttribute('open') && s.cw === w && !w.active) { s.cw = makeDialogWatcher(d); }
        });
        accessor(DLP, 'returnValue', function () { return st(this).returnValue; }, function (v) { st(this).returnValue = String(v); });
        accessor(DLP, 'closedBy', function () {
            var v = this.getAttribute('closedby');
            var l = v === null ? null : v.toLowerCase();
            if (l === 'any' || l === 'closerequest' || l === 'none') { return l; }
            return st(this).modal ? 'closerequest' : 'none';
        }, function (v) { this.setAttribute('closedby', String(v)); });
    }
    hide(globalThis, '__tl_dialog_light_dismiss', function (d) { requestCloseDialog(d, null); });
    // The elements in the top layer, for the engine and the tests of its order.
    hide(globalThis, '__tl_top_layer', function () { return topLayer.slice(); });
})();
"#;
