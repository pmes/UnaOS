//! AETHERAPIS (LEDGER SR145): the Fullscreen API (Fullscreen API Standard §4 `Element.requestFullscreen()` with
//! its error path — not connected, not an HTML / SVG / MathML element, a showing popover, fullscreen not enabled,
//! no transient activation — and the `fullscreenchange` / `fullscreenerror` events queued as tasks; §5
//! `Document.fullscreenEnabled`, `fullscreenElement`, `fullscreen`, `exitFullscreen()`; the event handler IDL
//! attributes on Document and Element). The fullscreen element is the page's: the shell reads [`element`] to size
//! the viewport to it (owed: a shell seam that does — docs/dev/evidence/aetherwpt/WPT.md §AETHERAPIS).
//!
//! CHARTER: Aether — binding.

use super::idl::arg;
use js_core::vm::*;
use std::cell::Cell;

thread_local! {
    static ELEMENT: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The document's fullscreen element (a node id), when one is set.
pub fn element() -> Option<usize> {
    ELEMENT.with(Cell::get)
}

pub fn reset() {
    ELEMENT.with(|e| e.set(None));
}

/// `__fs(op, el)`: `set` (an element or null), `get`.
fn native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let op = vm.to_string(&v)?.to_rust();
    match op.as_str() {
        "set" => {
            let e = arg(vm, ctx, 1);
            ELEMENT.with(|c| c.set(super::dom::node_of(vm, &e)));
            Ok(Value::Undefined)
        }
        "get" => Ok(super::dom::wrap_opt(vm, element())),
        _ => Ok(Value::Undefined),
    }
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__fs", 2, native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__fs"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("fullscreen-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] fullscreen prelude failed: {m}");
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
    var accessor = function (proto, name, get) {
        Object.defineProperty(proto, name, { get: get, configurable: true, enumerable: true });
    };
    var queue = function (f) { __tl('queue', f); };
    var current = function () { var e = __fs('get'); return e && e.isConnected ? e : null; };
    var fire = function (target, type) {
        queue(function () {
            var t = target;
            if (t.nodeType === 1 && !t.isConnected) { t = document; }
            t.dispatchEvent(new Event(type, { bubbles: true, composed: true }));
        });
    };
    // Fullscreen §5 "fullscreen enabled flag": a nested document needs its iframe's allowfullscreen (or the policy).
    var enabled = function () {
        try {
            if (window.top === window) { return true; }
            var fe = window.frameElement;
            if (!fe) { return false; }
            return fe.hasAttribute('allowfullscreen') || (fe.allow || '').indexOf('fullscreen') >= 0;
        } catch (e) { return false; }
    };
    var svgNS = 'http://www.w3.org/2000/svg', mathNS = 'http://www.w3.org/1998/Math/MathML';
    def(Element.prototype, 'requestFullscreen', function requestFullscreen_(options) {
        var el = this;
        return new Promise(function (resolve, reject) {
            var error = false;
            if (!(el instanceof HTMLElement || (el.namespaceURI === svgNS && el.localName === 'svg') || (el.namespaceURI === mathNS && el.localName === 'math'))) { error = true; }
            if (el.localName === 'dialog' && el.namespaceURI === 'http://www.w3.org/1999/xhtml') { error = true; }
            if (!el.isConnected) { error = true; }
            if (typeof __tl_is_popover_showing === 'function' && __tl_is_popover_showing(el)) { error = true; }
            if (!enabled()) { error = true; }
            if (!__activation('transient')) { error = true; }
            if (error) {
                fire(el, 'fullscreenerror');
                queue(function () { reject(new TypeError("Failed to execute 'requestFullscreen' on 'Element': Fullscreen request denied")); });
                return;
            }
            __activation('consume');
            var prev = current();
            __fs('set', el);
            if (prev && prev !== el) { fire(prev, 'fullscreenchange'); }
            fire(el, 'fullscreenchange');
            queue(function () { resolve(); });
        });
    });
    def(Document.prototype, 'exitFullscreen', function exitFullscreen_() {
        var doc = this;
        return new Promise(function (resolve, reject) {
            var el = current();
            if (!el || doc !== el.ownerDocument) {
                queue(function () { reject(new TypeError("Failed to execute 'exitFullscreen' on 'Document': Document not active")); });
                return;
            }
            __fs('set', null);
            fire(el, 'fullscreenchange');
            queue(function () { resolve(); });
        });
    });
    accessor(Document.prototype, 'fullscreenElement', function () { var e = current(); return e && e.ownerDocument === this ? e : null; });
    accessor(Document.prototype, 'fullscreenEnabled', function () { return enabled(); });
    accessor(Document.prototype, 'fullscreen', function () { return current() !== null; });
    hide(globalThis, '__fs_removed', function () {
        var e = __fs('get');
        __fs('set', null);
        if (e) { fire(document, 'fullscreenchange'); }
    });
    if (typeof __defineEventHandler === 'function') {
        ['fullscreenchange', 'fullscreenerror'].forEach(function (n) {
            __defineEventHandler(Document.prototype, n);
            __defineEventHandler(Element.prototype, n);
        });
    }
})();
"#;
