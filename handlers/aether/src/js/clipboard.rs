//! AETHERAPIS (LEDGER SR145): the Clipboard API (Clipboard API and events §4 `ClipboardEvent` — its `clipboardData`
//! member is js/events.rs's; §5 `Clipboard` on `navigator.clipboard`: `read()`, `readText()`, `write()`,
//! `writeText()`; §5.2 `ClipboardItem` with `types`, `getType()`, `presentationStyle`, `supports()`) over the shell's
//! clipboard seam: the engine owns a text the shell last handed it ([`set_platform_text`], the `BrowserPaste` text
//! lib.rs's `paste` receives) and a text the page last wrote ([`take_written`], for the shell to send as
//! `BrowserClipboardSet`). A write needs transient activation (the permission the user grants by acting); a read
//! needs it too, standing in for the `clipboard-read` permission prompt. Only `text/plain` crosses the seam;
//! `text/html` and `image/png` items are kept in the page and read back as written.
//!
//! CHARTER: Aether — binding.

use super::idl::{arg, s};
use js_core::vm::*;
use std::cell::RefCell;

thread_local! {
    static PLATFORM: RefCell<String> = const { RefCell::new(String::new()) };
    static WRITTEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The shell's clipboard text, for the page's `navigator.clipboard.readText()`.
pub fn set_platform_text(text: &str) {
    PLATFORM.with(|p| *p.borrow_mut() = text.to_string());
}

/// The text the page last wrote with `navigator.clipboard.writeText()` (taken once), for the shell's clipboard.
pub fn take_written() -> Option<String> {
    WRITTEN.with(|w| w.borrow_mut().take())
}

pub fn reset() {
    WRITTEN.with(|w| *w.borrow_mut() = None);
}

fn native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let op = vm.to_string(&v)?.to_rust();
    match op.as_str() {
        "read" => Ok(s(&PLATFORM.with(|p| p.borrow().clone()))),
        "write" => {
            let t = arg(vm, ctx, 1);
            let text = vm.to_string(&t)?.to_rust();
            PLATFORM.with(|p| *p.borrow_mut() = text.clone());
            WRITTEN.with(|w| *w.borrow_mut() = Some(text));
            Ok(Value::Undefined)
        }
        _ => Ok(Value::Undefined),
    }
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__clipboard", 2, native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__clipboard"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("clipboard-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] clipboard prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var TOKEN = {};
    var ITEMS = new WeakMap();
    var MANDATORY = ['text/plain', 'text/html', 'image/png'];
    var blobOf = function (v, type) {
        if (typeof Blob === 'function' && v instanceof Blob) { return v; }
        return new Blob([String(v)], { type: type });
    };
    class ClipboardItem {
        constructor(items, options) {
            if (items === null || typeof items !== 'object') { throw new TypeError("Failed to construct 'ClipboardItem': parameter 1 is not of type 'record'."); }
            var keys = Object.keys(items);
            if (keys.length === 0) { throw new TypeError("Failed to construct 'ClipboardItem': Empty dictionary argument"); }
            var map = new Map();
            keys.forEach(function (k) {
                var v = items[k];
                map.set(k, Promise.resolve(v).then(function (x) {
                    if (typeof x === 'string' || (typeof Blob === 'function' && x instanceof Blob)) { return blobOf(x, k); }
                    throw new TypeError("Failed to construct 'ClipboardItem': item is neither a string nor a Blob");
                }));
            });
            var style = options && options.presentationStyle !== undefined ? String(options.presentationStyle) : 'unspecified';
            if (['unspecified', 'inline', 'attachment'].indexOf(style) < 0) { throw new TypeError("Failed to construct 'ClipboardItem': presentationStyle is not a valid enum value"); }
            ITEMS.set(this, { map: map, style: style });
        }
        get presentationStyle() { return ITEMS.get(this).style; }
        get types() { return Object.freeze(Array.from(ITEMS.get(this).map.keys())); }
        getType(type) {
            var it = ITEMS.get(this);
            type = String(type);
            if (!it.map.has(type)) { return Promise.reject(new DOMException('The type was not found', 'NotFoundError')); }
            return it.map.get(type);
        }
        static supports(type) { return MANDATORY.indexOf(String(type)) >= 0; }
    }
    Object.defineProperty(ClipboardItem.prototype, Symbol.toStringTag, { value: 'ClipboardItem', configurable: true });
    hide(globalThis, 'ClipboardItem', ClipboardItem);
    var notAllowed = function (what) { return Promise.reject(new DOMException(what, 'NotAllowedError')); };
    var items = [];   // the page's own rich items (text/html, image/png) beside the platform text
    class Clipboard extends EventTarget {
        constructor(tok) { super(); if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        readText() {
            if (!__activation('transient')) { return notAllowed('Read permission denied.'); }
            return Promise.resolve(__clipboard('read'));
        }
        read() {
            if (!__activation('transient')) { return notAllowed('Read permission denied.'); }
            var text = __clipboard('read');
            var out = items.slice();
            if (out.length === 0 && text !== '') { out = [new ClipboardItem({ 'text/plain': text })]; }
            return Promise.resolve(out);
        }
        writeText(text) {
            if (!__activation('transient')) { return notAllowed('Write permission denied.'); }
            items = [];
            __clipboard('write', String(text));
            return Promise.resolve();
        }
        write(data) {
            if (!__activation('transient')) { return notAllowed('Write permission denied.'); }
            var list;
            try { list = Array.from(data); } catch (e) { return Promise.reject(new TypeError("Failed to execute 'write' on 'Clipboard': The provided value cannot be converted to a sequence.")); }
            for (var i = 0; i < list.length; i++) {
                if (!(list[i] instanceof ClipboardItem)) { return Promise.reject(new TypeError("Failed to execute 'write' on 'Clipboard': parameter 1 is not of type 'ClipboardItem'.")); }
            }
            return Promise.all(list.map(function (it) {
                var types = it.types;
                for (var j = 0; j < types.length; j++) { if (!ClipboardItem.supports(types[j]) && types[j].indexOf('web ') !== 0) { throw new DOMException('Type ' + types[j] + ' not supported on write.', 'NotAllowedError'); } }
                return it.types.indexOf('text/plain') >= 0 ? it.getType('text/plain').then(function (b) { return b.text(); }) : Promise.resolve(null);
            })).then(function (texts) {
                items = list.slice();
                var text = texts.filter(function (t) { return t !== null; }).join('');
                __clipboard('write', text);
            });
        }
    }
    Object.defineProperty(Clipboard.prototype, Symbol.toStringTag, { value: 'Clipboard', configurable: true });
    hide(globalThis, 'Clipboard', Clipboard);
    var cb = null;
    if (typeof Navigator === 'function') {
        Object.defineProperty(Navigator.prototype, 'clipboard', {
            get: function () { if (!cb) { cb = new Clipboard(TOKEN); } return cb; },
            configurable: true, enumerable: true
        });
    }
})();
"#;
