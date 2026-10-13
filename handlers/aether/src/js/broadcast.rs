//! AETHERSTORAGE (LEDGER SR94): the storage family's cross-document inbox, `BroadcastChannel` (HTML §9.5) and
//! the structured serialization (HTML §2.7) the family stores and posts values with.
//!
//! CHARTER: Aether — binding (the page-facing half; the engines are `crate::storage` and `crate::workers`).
//!
//! # The inbox
//! Every document (one `Engine` on its page thread) owns an inbox: a process-wide registry maps its serial to a
//! channel sender, and [`poll`] — called from `Engine::tick` and `Engine::run_network_tasks`, beside the
//! network queue's poll — drains it on the event loop, each message a task that calls the prelude's
//! `__inbox(kind, json)`. The engines post to a document by serial from any thread: IndexedDB's
//! `versionchange` (an open in another document needs this connection closed), Web Locks' grants and steals,
//! a service worker's `client.postMessage`, and the `BroadcastChannel` messages below. Unlike a network
//! operation an inbox is not "in flight", so an idle `BroadcastChannel` or an open database never keeps
//! `event_loop::settle` waiting on real time. A new `Engine` on the thread retires the previous document's
//! serial: its connections close, its locks release, its channels and service-worker client entry go
//! ([`retire`]); a thread that ends retires its document the same way.
//!
//! # BroadcastChannel
//! A channel is `(storage key, name)` (HTML §9.5: same storage key, same name). `postMessage` serializes the
//! message at once (a `DataCloneError` throws), then queues it to every OTHER open channel object of that key
//! and name — in this document or any other of the process — in creation order; a closed channel neither sends
//! nor receives. The receiver deserializes on its own event loop and fires `message` (or `messageerror` when
//! deserialization fails).
//!
//! # Structured serialization
//! `__sc_ser(value)` answers a JSON text of a tagged tree (StructuredSerializeInternal: primitives incl. -0, NaN,
//! ±Infinity and BigInt; Boolean/Number/String objects; Date; RegExp; ArrayBuffer and every view; Map; Set;
//! Error types; Blob/File; arrays and ordinary objects with their own enumerable string keys; object identity and
//! cycles by memory id) and throws "DataCloneError" for functions, symbols and platform objects; `__sc_de(text)`
//! rebuilds it in the current realm. `structuredClone(value)` is the pair — the platform prelude's JSON round
//! trip is gone.

use super::idl::{self, arg, s};
use js_core::vm::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// Most inbox messages one event-loop turn runs.
pub const MAX_PER_TURN: usize = 256;

type Msg = (String, String);

fn inboxes() -> std::sync::MutexGuard<'static, HashMap<u64, Sender<Msg>>> {
    static I: OnceLock<Mutex<HashMap<u64, Sender<Msg>>>> = OnceLock::new();
    I.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Inbox {
    doc: u64,
    rx: Receiver<Msg>,
}

impl Inbox {
    fn new() -> Inbox {
        let doc = SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = channel();
        inboxes().insert(doc, tx);
        Inbox { doc, rx }
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        retire(self.doc);
    }
}

thread_local! {
    static INBOX: RefCell<Option<Inbox>> = const { RefCell::new(None) };
}

/// The running document's serial (its inbox address; the `clientId` Web Locks and service workers report).
pub fn doc() -> u64 {
    INBOX.with(|i| {
        let mut i = i.borrow_mut();
        if i.is_none() {
            *i = Some(Inbox::new());
        }
        i.as_ref().map(|b| b.doc).unwrap_or(0)
    })
}

/// Posts a message to document `doc`'s inbox; false when it is gone.
pub fn post(doc: u64, kind: &str, payload: String) -> bool {
    let tx = inboxes().get(&doc).cloned();
    match tx {
        Some(tx) => tx.send((kind.to_string(), payload)).is_ok(),
        None => false,
    }
}

/// A document is gone: everything the storage family held for it is released.
pub fn retire(doc: u64) {
    inboxes().remove(&doc);
    channels().retain(|c| c.doc != doc);
    crate::storage::idb::doc_gone(doc);
    crate::storage::locks::doc_gone(doc);
    crate::workers::sw::doc_gone(doc);
    // AETHERWORKERS (SR123): the document's dedicated workers terminate and its message ports disentangle.
    crate::workers::dedicated::doc_gone(doc);
    super::ports::doc_gone(doc);
}

/// Runs the queued inbox messages (at most [`MAX_PER_TURN`]); answers how many ran.
pub fn poll(vm: &mut Vm) -> usize {
    if super::engine_poisoned() {
        return 0;
    }
    let mut ran = 0;
    while ran < MAX_PER_TURN {
        let Some((kind, payload)) = INBOX.with(|i| i.borrow().as_ref().and_then(|b| b.rx.try_recv().ok())) else { break };
        ran += 1;
        let g = vm.realm().global;
        let f = vm.get(g, &PropertyKey::from_str("__inbox")).unwrap_or(Value::Undefined);
        if !vm.is_callable(&f) {
            continue;
        }
        super::invoke(vm, "inbox-task", &f, &Value::Undefined, &[s(&kind), s(&payload)]);
        super::checkpoint(vm);
        if super::engine_poisoned() {
            break;
        }
    }
    ran
}

// ------------------------------------------------------------------------------------------------ channels

struct Chan {
    id: u64,
    key: String,
    name: String,
    doc: u64,
}

fn channels() -> std::sync::MutexGuard<'static, Vec<Chan>> {
    static C: OnceLock<Mutex<Vec<Chan>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap_or_else(|e| e.into_inner())
}

static CHAN_ID: AtomicU64 = AtomicU64::new(0);

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

/// `__bc(op, a, b)`: `open(name)` → channel id; `post(id, serialized)`; `close(id)`.
fn bc_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    match op.as_str() {
        "open" => {
            let name = str_arg(vm, ctx, 1)?;
            // An opaque origin's channels reach only themselves (a key no other document has).
            let key = crate::storage::page_storage_key().unwrap_or_else(|| format!("opaque:{}", doc()));
            let id = CHAN_ID.fetch_add(1, Ordering::Relaxed) + 1;
            channels().push(Chan { id, key, name, doc: doc() });
            Ok(idl::num(id as f64))
        }
        "post" => {
            let id = vm.to_number(&arg(vm, ctx, 1))? as u64;
            let data = str_arg(vm, ctx, 2)?;
            let origin = crate::js::net::page_origin();
            let targets: Vec<(u64, u64)> = {
                let ch = channels();
                let Some(me) = ch.iter().find(|c| c.id == id) else { return Ok(Value::Undefined) };
                ch.iter().filter(|c| c.id != id && c.key == me.key && c.name == me.name).map(|c| (c.doc, c.id)).collect()
            };
            for (d, cid) in targets {
                let payload = serde_json::json!({ "id": cid, "data": data, "origin": origin }).to_string();
                post(d, "bc-message", payload);
            }
            Ok(Value::Undefined)
        }
        "close" => {
            let id = vm.to_number(&arg(vm, ctx, 1))? as u64;
            channels().retain(|c| c.id != id);
            Ok(Value::Undefined)
        }
        _ => vm.throw_type("__bc: unknown operation"),
    }
}

/// Installs the inbox (retiring the thread's previous document), `__bc`, the serializer and
/// `BroadcastChannel`. Runs first of the storage family (`js::idb`, `js::cache_api`, `js::locks` and `js::sw`
/// use the serializer and the inbox dispatcher).
pub fn install(vm: &mut Vm) {
    INBOX.with(|i| {
        let old = i.borrow_mut().take();
        drop(old);
        *i.borrow_mut() = Some(Inbox::new());
    });
    let g = vm.realm().global;
    let fo = vm.make_native("__bc", 3, bc_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__bc"), Prop::data(Value::Object(fo), WC));
    for (label, src) in [("structured-serialize", SERIALIZE), ("broadcast", PRELUDE)] {
        if let Err(e) = vm.run_script_str(src) {
            let m = vm.error_string(&e);
            crate::ledger::record_js(&format!("{label}-prelude-failed:{}", super::clip(&m, 64)));
            eprintln!("[aether] {label} prelude failed: {m}");
        }
    }
}

/// HTML §2.7 StructuredSerialize / StructuredDeserialize over a JSON text (see the module docs). Shared by
/// every realm that runs it: the service-worker realm runs the same text (`workers::sw`), and so does a
/// dedicated worker's realm (`workers::dedicated`, AETHERWORKERS SR123), whose `postMessage` and MessagePort
/// transfer lists ride `__sc_ser(value, portIndexMap)` / `__sc_de(text, ports)` (`js::ports`).
pub const SERIALIZE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var err = function (what) {
        var m = what + ' could not be cloned.';
        return typeof DOMException === 'function' ? new DOMException(m, 'DataCloneError') : new Error('DataCloneError: ' + m);
    };
    var HEX = '0123456789abcdef';
    var hex = function (u8) {
        var out = [];
        for (var i = 0; i < u8.length; i++) { out.push(HEX[u8[i] >> 4] + HEX[u8[i] & 15]); }
        return out.join('');
    };
    var unhex = function (h) {
        var u8 = new Uint8Array(h.length >> 1);
        for (var i = 0; i < u8.length; i++) { u8[i] = parseInt(h.substr(i * 2, 2), 16); }
        return u8;
    };
    var tagOf = function (x) { return Object.prototype.toString.call(x).slice(8, -1); };
    var VIEWS = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array', 'Uint32Array',
        'Float16Array', 'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array', 'DataView'];
    var num = function (n) {
        if (n !== n) { return { t: 'N', v: 'NaN' }; }
        if (n === Infinity) { return { t: 'N', v: 'Infinity' }; }
        if (n === -Infinity) { return { t: 'N', v: '-Infinity' }; }
        if (n === 0 && 1 / n < 0) { return { t: 'N', v: '-0' }; }
        return n;
    };
    // AETHERWORKERS (SR123): `xp` (optional) maps each transferred MessagePort to its index in the message's
    // port list; such a port serializes as `{t:'P', x}` and `de(text, ports)` answers `ports[x]`. Without `xp`
    // a port is a platform object and throws DataCloneError as before.
    var ser = function (value, xp) {
        var mem = new Map(), next = 0;
        var walk = function (v) {
            var ty = typeof v;
            if (v === undefined) { return { t: 'u' }; }
            if (v === null || ty === 'boolean' || ty === 'string') { return v; }
            if (ty === 'number') { return num(v); }
            if (ty === 'bigint') { return { t: 'I', v: String(v) }; }
            if (ty === 'symbol') { throw err(String(v)); }
            if (ty === 'function') { throw err(v.name ? 'function ' + v.name : 'A function'); }
            if (xp && xp.has(v)) { return { t: 'P', x: xp.get(v) }; }
            if (mem.has(v)) { return { t: 'r', i: mem.get(v) }; }
            var id = next++;
            mem.set(v, id);
            var tag = tagOf(v);
            if (tag === 'Boolean') { return { t: 'Bo', v: Boolean.prototype.valueOf.call(v), i: id }; }
            if (tag === 'Number') { return { t: 'Nu', v: num(Number.prototype.valueOf.call(v)), i: id }; }
            if (tag === 'String') { return { t: 'St', v: String.prototype.valueOf.call(v), i: id }; }
            if (tag === 'BigInt') { return { t: 'BI', v: String(BigInt.prototype.valueOf.call(v)), i: id }; }
            if (tag === 'Date') { return { t: 'D', v: num(Date.prototype.getTime.call(v)), i: id }; }
            if (tag === 'RegExp') { return { t: 'R', s: v.source, f: v.flags, i: id }; }
            if (tag === 'ArrayBuffer') {
                if (typeof v.detached === 'boolean' && v.detached) { throw err('A detached ArrayBuffer'); }
                return { t: 'AB', b: hex(new Uint8Array(v)), i: id };
            }
            if (VIEWS.indexOf(tag) >= 0) {
                var len = tag === 'DataView' ? v.byteLength : v.length;
                return { t: 'V', k: tag, buf: walk(v.buffer), o: v.byteOffset, l: len, i: id };
            }
            if (tag === 'Map') {
                var me = [];
                Map.prototype.forEach.call(v, function (val, k) { me.push([k, val]); });
                return { t: 'M', e: me.map(function (kv) { return [walk(kv[0]), walk(kv[1])]; }), i: id };
            }
            if (tag === 'Set') {
                var se = [];
                Set.prototype.forEach.call(v, function (val) { se.push(val); });
                return { t: 'Se', e: se.map(walk), i: id };
            }
            if (tag === 'Error' || v instanceof Error) {
                var n = String(v.name);
                if (['Error', 'EvalError', 'RangeError', 'ReferenceError', 'SyntaxError', 'TypeError', 'URIError'].indexOf(n) < 0) { n = 'Error'; }
                var desc = Object.getOwnPropertyDescriptor(v, 'message');
                return { t: 'E', n: n, m: desc && 'value' in desc ? String(desc.value) : undefined, i: id };
            }
            if (typeof File === 'function' && v instanceof File && typeof __blob_bytes === 'function') {
                return { t: 'Fi', b: hex(new Uint8Array(__blob_bytes(v))), ty: v.type, n: v.name, lm: v.lastModified, i: id };
            }
            if (typeof Blob === 'function' && v instanceof Blob && typeof __blob_bytes === 'function') {
                return { t: 'Bl', b: hex(new Uint8Array(__blob_bytes(v))), ty: v.type, i: id };
            }
            if (Array.isArray(v)) {
                var ap = [];
                Object.keys(v).forEach(function (k) { ap.push([k, walk(v[k])]); });
                return { t: 'A', l: v.length, p: ap, i: id };
            }
            if (tag !== 'Object' && tag !== 'Arguments') { throw err(tag + ' object'); }
            var proto = Object.getPrototypeOf(v);
            if (proto !== null && proto !== Object.prototype && typeof Node === 'function' && v instanceof Node) { throw err(tag + ' object'); }
            var op = [];
            Object.keys(v).forEach(function (k) { op.push([k, walk(v[k])]); });
            return { t: 'O', p: op, i: id };
        };
        return JSON.stringify(walk(value));
    };
    var de = function (text, ports) {
        var mem = [];
        var unnum = function (n) {
            if (n !== null && typeof n === 'object') { return n.v === 'NaN' ? NaN : n.v === 'Infinity' ? Infinity : n.v === '-Infinity' ? -Infinity : -0; }
            return n;
        };
        var walk = function (n) {
            if (n === null || typeof n !== 'object') { return n; }
            var o;
            switch (n.t) {
                case 'u': return undefined;
                case 'N': return unnum(n);
                case 'I': return BigInt(n.v);
                case 'r': return mem[n.i];
                case 'P':
                    if (!ports || !(n.x in ports)) { throw err('A MessagePort'); }
                    return ports[n.x];
                case 'Bo': o = Object(n.v); break;
                case 'Nu': o = Object(unnum(n.v)); break;
                case 'St': o = Object(n.v); break;
                case 'BI': o = Object(BigInt(n.v)); break;
                case 'D': o = new Date(unnum(n.v)); break;
                case 'R': o = new RegExp(n.s, n.f); break;
                case 'AB': o = unhex(n.b).buffer; break;
                case 'V': {
                    var buf = walk(n.buf);
                    var C = globalThis[n.k];
                    o = typeof C === 'function' ? new C(buf, n.o, n.l) : new Uint8Array(buf, n.o, n.l);
                    break;
                }
                case 'M': o = new Map(); mem[n.i] = o; n.e.forEach(function (kv) { o.set(walk(kv[0]), walk(kv[1])); }); return o;
                case 'Se': o = new Set(); mem[n.i] = o; n.e.forEach(function (x) { o.add(walk(x)); }); return o;
                case 'E': {
                    var EC = globalThis[n.n] || Error;
                    o = n.m === undefined ? new EC() : new EC(n.m);
                    break;
                }
                case 'Fi': o = new File([unhex(n.b)], n.n, { type: n.ty, lastModified: n.lm }); break;
                case 'Bl': o = new Blob([unhex(n.b)], { type: n.ty }); break;
                case 'A': o = new Array(n.l); mem[n.i] = o; n.p.forEach(function (kv) { o[kv[0]] = walk(kv[1]); }); return o;
                case 'O': o = {}; mem[n.i] = o; n.p.forEach(function (kv) { o[kv[0]] = walk(kv[1]); }); return o;
                default: throw err('A serialized value');
            }
            mem[n.i] = o;
            return o;
        };
        return walk(JSON.parse(text));
    };
    hide('__sc_ser', ser);
    hide('__sc_de', de);
    Object.defineProperty(globalThis, 'structuredClone', {
        // HTML §2.7.11 structuredClone(value, { transfer }): ArrayBuffers in `transfer` move — the clone owns
        // their bytes and the originals are detached (AETHERJS2).
        value: function structuredClone(value, options) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'structuredClone': 1 argument required, but only 0 present."); }
            var transfer = [];
            if (options !== undefined && options !== null) {
                if (typeof options !== 'object' && typeof options !== 'function') {
                    throw new TypeError("Failed to execute 'structuredClone': The provided value is not of type 'StructuredSerializeOptions'.");
                }
                if (options.transfer !== undefined) { transfer = Array.from(options.transfer); }
            }
            var seen = new Set();
            transfer.forEach(function (b, i) {
                if (tagOf(b) !== 'ArrayBuffer') { throw err('Value at index ' + i + ' does not have a transferable type and'); }
                if (seen.has(b)) { throw err('ArrayBuffer at index ' + i + ' is a duplicate of an earlier ArrayBuffer and'); }
                if (typeof b.detached === 'boolean' && b.detached) { throw err('ArrayBuffer at index ' + i + ' is already detached and'); }
                seen.add(b);
            });
            var out = de(ser(value));
            transfer.forEach(function (b) { ArrayBuffer.prototype.transfer.call(b, 0); });
            return out;
        },
        writable: true, configurable: true, enumerable: false,
    });
})();
"#;

/// The inbox dispatcher (`__inbox`, with a registry the other bindings add their kinds to) and
/// `BroadcastChannel`.
const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var kinds = Object.create(null);
    hide('__inbox_on', function (kind, f) { kinds[kind] = f; });
    hide('__inbox', function (kind, payload) {
        var f = kinds[kind];
        if (typeof f === 'function') { f(JSON.parse(payload)); }
    });

    var ST = new WeakMap();
    var BY_ID = new Map();
    var st = function (o) {
        var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    class BroadcastChannel extends EventTarget {
        constructor(name) {
            if (arguments.length < 1) {
                throw new TypeError("Failed to construct 'BroadcastChannel': 1 argument required, but only 0 present.");
            }
            super();
            var n = String(name);
            var id = __bc('open', n);
            ST.set(this, { name: n, id: id, closed: false });
            BY_ID.set(id, this);
        }
        get name() { return st(this).name; }
        postMessage(message) {
            var s = st(this);
            if (arguments.length < 1) {
                throw new TypeError("Failed to execute 'postMessage' on 'BroadcastChannel': 1 argument required, but only 0 present.");
            }
            if (s.closed) {
                throw new DOMException("Failed to execute 'postMessage' on 'BroadcastChannel': Channel is closed", 'InvalidStateError');
            }
            __bc('post', s.id, __sc_ser(message));
        }
        close() {
            var s = st(this);
            if (s.closed) { return; }
            s.closed = true;
            __bc('close', s.id);
            BY_ID.delete(s.id);
        }
    }
    __net_handlers(BroadcastChannel.prototype, ['message', 'messageerror']);
    Object.defineProperty(BroadcastChannel.prototype, Symbol.toStringTag, { value: 'BroadcastChannel', configurable: true });
    hide('BroadcastChannel', BroadcastChannel);
    __inbox_on('bc-message', function (m) {
        var ch = BY_ID.get(m.id);
        if (!ch || st(ch).closed) { return; }
        var data;
        try { data = __sc_de(m.data); }
        catch (e) { __net_fire(ch, 'MessageEvent', 'messageerror', { data: null, origin: m.origin }); return; }
        __net_fire(ch, 'MessageEvent', 'message', { data: data, origin: m.origin });
    });
})();
"#;
