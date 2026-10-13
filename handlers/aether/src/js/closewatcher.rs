//! AETHERAPIS (LEDGER SR145): close watchers (HTML §6.6.5 "close requests and close watchers"): the window's close
//! watcher manager (groups, the allowed number of groups that a user activation raises, "next user interaction allows
//! a new group"), "establish a close watcher", "request to close", "close", "destroy", "process close watchers"
//! (what a close request — the Escape key, `js/events.rs` — runs), and the `CloseWatcher` interface (§6.6.5.3:
//! the constructor with its `signal`, `requestClose()`, `close()`, `destroy()`, the `cancel` / `close` events).
//!
//! CHARTER: Aether — binding.
//!
//! The `cancel` event is cancelable only when the page has history-action activation (an unconsumed user gesture)
//! or the request came from script (`requestClose()`, which does not require it); that activation is consumed
//! when a cancel action prevents the close. A modal `<dialog>` and an auto popover establish their watchers through
//! the hidden `__cw_make` (js/toplayer.rs), so one Escape closes the topmost of them in the manager's order.

use super::idl::arg;
use js_core::vm::*;

/// A close request (Escape): "process close watchers" in the page. True when a watcher was processed.
pub fn close_request(vm: &mut Vm) -> bool {
    if super::engine_poisoned() {
        return false;
    }
    let g = vm.realm().global;
    let f = vm.get(g, &PropertyKey::from_str("__cw_request")).unwrap_or(Value::Undefined);
    if !vm.is_callable(&f) {
        return false;
    }
    let r = super::invoke(vm, "close-request", &f, &Value::Undefined, &[]);
    super::checkpoint(vm);
    matches!(r, Some(Value::Bool(true)))
}

/// "Notify the close watcher manager about user activation" (HTML §6.6.5.1), from the activation notification.
pub fn note_user_activation(vm: &mut Vm) {
    if super::engine_poisoned() {
        return;
    }
    let g = vm.realm().global;
    let f = vm.get(g, &PropertyKey::from_str("__cw_activation")).unwrap_or(Value::Undefined);
    if vm.is_callable(&f) {
        super::invoke(vm, "close-watcher-activation", &f, &Value::Undefined, &[]);
    }
}

fn native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let op = vm.to_string(&v)?.to_rust();
    Ok(match op.as_str() {
        "history" => Value::Bool(super::activation::has_history_action()),
        "consume-history" => {
            super::activation::consume_history_action();
            Value::Bool(true)
        }
        _ => Value::Undefined,
    })
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__cw", 1, native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__cw"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("closewatcher-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] closewatcher prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    // HTML §6.6.5.1: the close watcher manager of this Window.
    var mgr = { groups: [], allowed: 1, nextAllowsGroup: true };
    // A close watcher: { cancel(canPreventClose) -> continue?, close(), enabled() -> bool, active, running }
    var establish = function (w) {
        w.active = true; w.running = false;
        if (mgr.groups.length < mgr.allowed) { mgr.groups.push([w]); } else { mgr.groups[mgr.groups.length - 1].push(w); }
        mgr.nextAllowsGroup = true;
        return w;
    };
    var destroy = function (w) {
        if (!w.active) { return; }
        w.active = false;
        for (var i = 0; i < mgr.groups.length; i++) {
            var k = mgr.groups[i].indexOf(w);
            if (k >= 0) {
                mgr.groups[i].splice(k, 1);
                if (mgr.groups[i].length === 0) { mgr.groups.splice(i, 1); }
                break;
            }
        }
    };
    // "request to close" given requireHistoryActionActivation: true when the close may proceed.
    var requestClose = function (w, requireActivation) {
        if (!w.active) { return true; }
        if (!w.enabled()) { return true; }
        if (w.running) { return true; }
        var canPreventClose = (mgr.groups.length < mgr.allowed) || !requireActivation || __cw('history');
        w.running = true;
        var shouldContinue;
        try { shouldContinue = w.cancel(canPreventClose); } finally { w.running = false; }
        if (shouldContinue === false) {
            __cw('consume-history');
            return false;
        }
        close(w);
        return true;
    };
    var close = function (w) {
        if (!w.active || !w.enabled()) { return; }
        destroy(w);
        w.close();
    };
    // "Process close watchers" (a close request).
    hide(globalThis, '__cw_request', function () {
        var processed = false;
        if (mgr.groups.length > 0) {
            var group = mgr.groups[mgr.groups.length - 1].slice();
            for (var i = group.length - 1; i >= 0; i--) {
                processed = true;
                if (!requestClose(group[i], true)) { break; }
            }
        }
        if (mgr.allowed > 1) { mgr.allowed -= 1; }
        return processed;
    });
    hide(globalThis, '__cw_activation', function () {
        if (mgr.nextAllowsGroup) { mgr.allowed += 1; mgr.nextAllowsGroup = false; }
    });
    // For the engine's own watchers (dialog, popover): returns the handle.
    hide(globalThis, '__cw_make', function (w) {
        establish(w);
        return {
            requestClose: function (requireActivation) { return requestClose(w, !!requireActivation); },
            close: function () { close(w); },
            destroy: function () { destroy(w); },
            get active() { return w.active; }
        };
    });
    var W = new WeakMap();
    var defineHandler = function (proto, name) {
        var key = '__on' + name;
        Object.defineProperty(proto, 'on' + name, {
            get: function () { return this[key] || null; },
            set: function (f) {
                var old = this[key];
                if (typeof old === 'function') { this.removeEventListener(name, old); }
                if (typeof f === 'function') { Object.defineProperty(this, key, { value: f, writable: true, configurable: true, enumerable: false }); this.addEventListener(name, f); }
                else { Object.defineProperty(this, key, { value: null, writable: true, configurable: true, enumerable: false }); }
            },
            configurable: true, enumerable: true
        });
    };
    class CloseWatcher extends EventTarget {
        constructor(options) {
            super();
            var self = this;
            var w = {
                cancel: function (canPreventClose) {
                    var e = new Event('cancel', { cancelable: canPreventClose });
                    self.dispatchEvent(e);
                    return !e.defaultPrevented;
                },
                close: function () { self.dispatchEvent(new Event('close')); },
                enabled: function () { return true; }
            };
            W.set(this, w);
            var signal = options && options.signal;
            if (signal !== undefined && signal !== null) {
                if (!(typeof AbortSignal === 'function' && signal instanceof AbortSignal)) {
                    throw new TypeError("Failed to construct 'CloseWatcher': member signal is not of type AbortSignal.");
                }
                if (signal.aborted) { w.active = false; return; }
                signal.addEventListener('abort', function () { destroy(w); });
            }
            establish(w);
        }
        requestClose() { var w = W.get(this); if (!w) { throw new TypeError('Illegal invocation'); } requestClose(w, false); }
        close() { var w = W.get(this); if (!w) { throw new TypeError('Illegal invocation'); } close(w); }
        destroy() { var w = W.get(this); if (!w) { throw new TypeError('Illegal invocation'); } destroy(w); }
    }
    defineHandler(CloseWatcher.prototype, 'cancel');
    defineHandler(CloseWatcher.prototype, 'close');
    Object.defineProperty(CloseWatcher.prototype, Symbol.toStringTag, { value: 'CloseWatcher', configurable: true });
    hide(globalThis, 'CloseWatcher', CloseWatcher);
    hide(globalThis, '__defineEventHandler', defineHandler);
})();
"#;
