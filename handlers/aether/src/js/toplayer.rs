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
//! The DOM and event half is here. AETHERTOP (SR146) added the rendering half's seams: the prelude's top layer is
//! MIRRORED in Rust (`TOP`, fed by `__tl('track', …)` on every change) so the cascade answers `:popover-open` /
//! `:modal` (dom/select.rs `state`, css_core's `ElementState::{PopoverOpen, Modal}`) and the UA sheet
//! (`crate::css::UA_SHEET`) can hide a hidden popover; `render` paints [`top_layer`] above the document in order,
//! each element after its `::backdrop` (CSS Position 4 §top-layer, HTML §15.5.x "the top layer" rendering);
//! [`is_inert`] is HTML §4.11.4's "blocked by a modal dialog" for the engine's input path (js/events.rs); and the
//! events the prelude fires go through `__tl('fire', target, event)` → `events::dispatch_trusted`, so
//! `beforetoggle` / `toggle` / `cancel` / `close` carry `isTrusted` true as the user agent's events do.
//!
//! Hooks from the engine: [`invoker_activation`] (a button's or input's activation behavior, js/events.rs),
//! [`light_dismiss`] (a trusted click), [`node_removed`] (DOM removal, js/dom.rs), [`attribute_changed`] (the
//! `popover` and dialog `open` attributes, js/dom.rs); all are no-ops while nothing is showing (`TRACKED`).

use super::idl::arg;
use super::Task;
use js_core::vm::*;
use std::cell::{Cell, RefCell};

/// A top-layer entry's kind, as the prelude reports it: a showing popover, a modal dialog, else (an element the
/// prelude tracks but paints in flow — nothing today), and the fullscreen element appended by [`top_layer`].
pub const KIND_POPOVER: u8 = 1;
pub const KIND_MODAL: u8 = 2;
pub const KIND_FULLSCREEN: u8 = 3;

thread_local! {
    /// How many elements the prelude is tracking (showing popovers, open dialogs): the hooks skip the JS call at 0.
    static TRACKED: Cell<u32> = const { Cell::new(0) };
    /// AETHERTOP (SR146): the prelude's top layer, in order (node id, kind) — the Rust mirror the cascade, the
    /// painter and the input path read.
    static TOP: RefCell<Vec<(usize, u8)>> = const { RefCell::new(Vec::new()) };
    /// Bumped on every top-layer change (and every fullscreen change): the restyle's cue that a `:popover-open` /
    /// `:modal` / `:fullscreen` match may have flipped without a DOM change (css/mod.rs `restyle_plan`), and the
    /// painter's cue that the last frame's damage is not enough (render `relayout_damage`).
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

pub fn reset() {
    TRACKED.with(|t| t.set(0));
    TOP.with(|t| t.borrow_mut().clear());
    bump();
}

/// A top-layer change happened (the mirror, or the fullscreen element): the next restyle re-reads the three
/// states and the next frame repaints whole; the DOM counts as mutated so the engine lays out and paints again.
pub fn bump() {
    GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    super::touch();
}

/// The top-layer generation (see `GENERATION`).
pub fn generation() -> u64 {
    GENERATION.with(Cell::get)
}

/// HTML §4.16.3 `:popover-open`: `id` is a popover in the showing state.
pub fn is_popover_showing(id: usize) -> bool {
    TOP.with(|t| t.borrow().iter().any(|&(n, k)| n == id && k == KIND_POPOVER))
}

/// HTML §4.16.3 `:modal`: `id` is a dialog whose is-modal flag is set.
pub fn is_modal(id: usize) -> bool {
    TOP.with(|t| t.borrow().iter().any(|&(n, k)| n == id && k == KIND_MODAL))
}

/// The document's top layer in order (HTML "top layer": the prelude's list — popovers and modal dialogs as they
/// were added — then the fullscreen element when it is not already in it; Fullscreen §4 adds it on the request, so
/// its true place is where that request fell among the others — approximated as last), as (node id, kind).
pub fn top_layer() -> Vec<(usize, u8)> {
    let mut v = TOP.with(|t| t.borrow().clone());
    if let Some(fs) = super::fullscreen::element() {
        if !v.iter().any(|&(n, _)| n == fs) {
            v.push((fs, KIND_FULLSCREEN));
        }
    }
    v
}

/// Is `id` painted in the top layer (so the document's own walk skips it)?
pub fn in_top_layer(id: usize) -> bool {
    TOP.with(|t| t.borrow().iter().any(|&(n, _)| n == id)) || super::fullscreen::element() == Some(id)
}

/// The topmost modal dialog (the last one in the top layer), when one is shown.
pub fn modal_dialog() -> Option<usize> {
    TOP.with(|t| t.borrow().iter().rev().find(|&&(_, k)| k == KIND_MODAL).map(|&(n, _)| n))
}

/// HTML §4.11.4 "blocked by a modal dialog" / §6.3 inert: while a modal dialog is shown, every node outside the
/// topmost one's subtree is inert (a click does not reach it, a key is retargeted — js/events.rs), except the
/// top-layer elements added after it (a popover opened from the dialog) and their subtrees.
pub fn is_inert(id: usize) -> bool {
    let top = top_layer();
    let Some(pos) = top.iter().rposition(|&(_, k)| k == KIND_MODAL) else { return false };
    let roots: Vec<usize> = top[pos..].iter().map(|&(n, _)| n).collect();
    let mut cur = Some(id);
    let mut hops = 0;
    while let Some(n) = cur {
        if roots.contains(&n) {
            return false;
        }
        cur = super::dom::parent_of(n);
        hops += 1;
        if hops > 65_536 {
            break;
        }
    }
    true
}

/// AETHERFIT (SR189): HTML §6.3 / §4.11.4 — an inert node is not a target of user interaction events: the event goes to
/// the topmost modal dialog (the `::backdrop`'s originating element) instead. The ONE retarget rule: the click, keys,
/// pointer, mouse and wheel paths (js/events.rs) and the hover path (lib.rs `pointer_moved`) all take it.
pub fn retarget(id: usize) -> usize {
    if is_inert(id) { modal_dialog().unwrap_or(id) } else { id }
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

/// `__tl(op, …)`: `queue(fn)` queues `fn` as a task; `track(n, elements, kinds)` records how many elements are
/// showing and mirrors the top layer (`elements[i]` of kind `kinds[i]`, in order); `fire(target, event)` dispatches
/// a script-built event as a trusted one (`isTrusted` true) and answers whether it was not canceled.
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
            let els = arg(vm, ctx, 2);
            let kinds = arg(vm, ctx, 3);
            let mut top = Vec::new();
            if let (Some(eo), Some(ko)) = (els.as_object(), kinds.as_object()) {
                let len = vm.get(eo, &PropertyKey::from_str("length")).ok().and_then(|l| vm.to_number(&l).ok()).unwrap_or(0.0);
                for i in 0..len.max(0.0).min(65_536.0) as u32 {
                    let e = vm.get(eo, &PropertyKey::Index(i)).unwrap_or(Value::Undefined);
                    let k = vm.get(ko, &PropertyKey::Index(i)).unwrap_or(Value::Undefined);
                    if let Some(id) = super::dom::node_of(vm, &e) {
                        let k = vm.to_number(&k).unwrap_or(0.0);
                        top.push((id, k.clamp(0.0, 255.0) as u8));
                    }
                }
            }
            let changed = TOP.with(|t| {
                let mut t = t.borrow_mut();
                let changed = *t != top;
                *t = top;
                changed
            });
            if changed {
                bump();
            }
        }
        "fire" => {
            let t = arg(vm, ctx, 1);
            let e = arg(vm, ctx, 2);
            let ok = super::events::dispatch_trusted(vm, &t, &e).unwrap_or(true);
            return Ok(Value::Bool(ok));
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
    // AETHERTOP (SR146): the count the hooks gate on, and the top layer mirrored for the cascade and the painter
    // (1 a showing popover, 2 a modal dialog).
    var track = function () {
        var kinds = topLayer.map(function (el) { var s = st(el); return s.vis === 'showing' ? 1 : (s.modal ? 2 : 0); });
        __tl('track', topLayer.length + autoList.length + hintList.length + dialogs.length + (fsElement() ? 1 : 0), topLayer, kinds);
    };
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
    // The events the user agent fires are trusted: __tl('fire') dispatches with isTrusted true (AETHERTOP SR146).
    var fire = function (el, e) { return __tl('fire', el, e); };
    var fireToggle = function (el, type, oldState, newState, cancelable) {
        var e = new ToggleEvent(type, { cancelable: !!cancelable, oldState: oldState, newState: newState });
        fire(el, e);
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
        track(); // AETHERFIT2 (SR191): the Rust mirror out of the top layer before the focus restore, as closeDialog
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
                queueTask(function () { fire(el, new Event('close')); });
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
                fire(d, e);
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
        // AETHERFIT2 (SR191): HTML's close steps take the dialog out of the top layer BEFORE the focus restore —
        // the Rust mirror (`is_inert`, which `focus()` refuses on) is synced here, or the restore is refused.
        track();
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
        queueTask(function () { fire(d, new Event('close')); });
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
                cancel: function (canPreventClose) { var e = new Event('cancel', { cancelable: canPreventClose }); fire(d, e); return !e.defaultPrevented; },
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
