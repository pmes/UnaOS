//! AETHERWORKERS (LEDGER SR123): `MessageChannel` / `MessagePort` (HTML §9.4.5) as a real task source, and the
//! transfer half of `postMessage` every messaging binding shares.
//!
//! CHARTER: Aether — binding (the realm-facing half of the port registry; both the page realm and a dedicated
//! worker's realm install it — `workers::dedicated` runs the same prelude).
//!
//! # The registry
//! A port is a process-wide id. [`reg`] holds, per id, its entangled peer and its OWNER — the document (page
//! realm, by `js::broadcast` inbox serial), the dedicated worker (by worker id), or `Transit` while a message
//! carrying it is in flight. `port.postMessage` serializes on the sender's realm and hands the text to
//! `__port('post')`, which looks up the PEER's owner under the registry lock and queues one task there: a
//! document gets an inbox message (`port-message`, run by `broadcast::poll` from `Engine::tick` and the
//! event-loop drivers), a worker gets a `Cmd::Port` on its thread's channel. A peer in transit buffers the
//! message in its entry; the realm that deserializes the carrying message ADOPTS the port (`__port('adopt')`),
//! which re-owns it and flushes that buffer, in order, to the new owner's queue — so messages posted while the
//! port was moving arrive before the ones posted after. Shipping a port moves the messages its old realm had
//! received but not yet dispatched (a port not yet started) into the same buffer. `close()` disentangles both
//! ends; a document or worker that goes away disentangles every port it owned ([`owner_gone`]).
//!
//! # The realm half (`PRELUDE`)
//! `MessagePort` (an EventTarget; `postMessage(message, transfer)`, `start()`, `close()`, `onmessage` —
//! setting it implicitly starts the port — and `onmessageerror`) keeps a port message queue that holds
//! messages until `start()`; each queued message is dispatched as its own task (a zero-delay timer, so the
//! microtask checkpoint runs between them). `MessageChannel` mints an entangled pair. `__msg_pack(message,
//! transfer)` is the "StructuredSerializeWithTransfer" every messaging binding calls (MessagePort, Worker,
//! the worker global): it validates the transfer list (ArrayBuffers and MessagePorts; a duplicate, a detached
//! buffer or an already shipped port throws DataCloneError), serializes through `js::broadcast`'s serializer
//! with each transferred port as an index into the message's port list, then ships the ports and detaches the
//! buffers. `__msg_fire` dispatches a MessageEvent with its `ports`: in the page through `js::net`'s trusted
//! `__net_fire`, whose MessageEvent init carries a `ports` member (a frozen array — `events.rs`'s
//! MessageEventInit has it since AETHERWORKERS2, SR125, so `new MessageEvent(type, { ports })` from script
//! carries its ports too and the per-dispatch stash is gone), in a worker through that realm's own
//! MessageEvent.

use super::idl::{arg, num};
use js_core::vm::*;
use serde_json::json;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Where a port's messages go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// A document's realm (its `js::broadcast` inbox serial).
    Doc(u64),
    /// A dedicated worker's realm (its worker id).
    Worker(u64),
    /// Inside a message in flight: the port's messages wait in its entry.
    Transit,
}

struct Ent {
    peer: Option<u64>,
    owner: Owner,
    /// (serialized data, comma-separated port ids) received while in transit.
    pending: Vec<(String, String)>,
}

fn reg() -> MutexGuard<'static, HashMap<u64, Ent>> {
    static R: OnceLock<Mutex<HashMap<u64, Ent>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

static NEXT: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Set on a dedicated worker's thread: the realm that owns ports minted or adopted here.
    static WORKER: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Marks the current thread as dedicated worker `id`'s (its realm owns the ports it mints and adopts).
pub fn set_worker_owner(id: Option<u64>) {
    WORKER.with(|w| w.set(id));
}

/// The owner a port minted or adopted on this thread gets.
pub fn here() -> Owner {
    match WORKER.with(Cell::get) {
        Some(w) => Owner::Worker(w),
        None => Owner::Doc(super::broadcast::doc()),
    }
}

/// Queues one message task for `port` on `owner`'s event loop; false when the owner is gone or in transit.
fn deliver(owner: Owner, port: u64, data: &str, ports: &str) -> bool {
    match owner {
        Owner::Doc(d) => super::broadcast::post(d, "port-message", json!({ "port": port, "data": data, "ports": ports }).to_string()),
        Owner::Worker(w) => crate::workers::dedicated::deliver_port(w, port, data, ports),
        Owner::Transit => false,
    }
}

/// Disentangles every port `owner` held (a document retired, a worker ended).
pub fn owner_gone(owner: Owner) {
    let mut r = reg();
    let gone: Vec<u64> = r.iter().filter(|(_, e)| e.owner == owner).map(|(id, _)| *id).collect();
    for id in gone {
        if let Some(e) = r.remove(&id) {
            if let Some(p) = e.peer.and_then(|p| r.get_mut(&p)) {
                p.peer = None;
            }
        }
    }
}

/// `broadcast::retire`'s hook: the document's ports disentangle.
pub fn doc_gone(doc: u64) {
    owner_gone(Owner::Doc(doc));
}

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    if v.is_nullish() {
        return Ok(String::new());
    }
    Ok(vm.to_string(&v)?.to_rust())
}

fn id_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<u64> {
    let v = arg(vm, ctx, i);
    let n = vm.to_number(&v)?;
    Ok(if n.is_finite() && n >= 0.0 { n as u64 } else { 0 })
}

/// `__port(op, …)`: `pair()` → `[a, b]`; `post(id, data, ports)`; `ship(id, backlogJson)`; `adopt(id)`;
/// `close(id)`.
fn port_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    match op.as_str() {
        "pair" => {
            let a = NEXT.fetch_add(2, Ordering::Relaxed) + 1;
            let b = a + 1;
            let owner = here();
            {
                let mut r = reg();
                r.insert(a, Ent { peer: Some(b), owner, pending: Vec::new() });
                r.insert(b, Ent { peer: Some(a), owner, pending: Vec::new() });
            }
            let arr = vm.new_array(vec![num(a as f64), num(b as f64)]);
            Ok(Value::Object(arr))
        }
        "post" => {
            let id = id_arg(vm, ctx, 1)?;
            let data = str_arg(vm, ctx, 2)?;
            let ports = str_arg(vm, ctx, 3)?;
            let mut r = reg();
            let Some(peer) = r.get(&id).and_then(|e| e.peer) else { return Ok(Value::Undefined) };
            let Some(pe) = r.get_mut(&peer) else { return Ok(Value::Undefined) };
            // Delivered under the lock: an adopt that re-owns the peer cannot interleave and reorder.
            if pe.owner == Owner::Transit {
                pe.pending.push((data, ports));
            } else {
                deliver(pe.owner, peer, &data, &ports);
            }
            Ok(Value::Undefined)
        }
        "ship" => {
            let id = id_arg(vm, ctx, 1)?;
            let backlog = str_arg(vm, ctx, 2)?;
            let backlog: Vec<(String, String)> = serde_json::from_str::<Vec<(String, String)>>(&backlog).unwrap_or_default();
            if let Some(e) = reg().get_mut(&id) {
                e.owner = Owner::Transit;
                let later = std::mem::take(&mut e.pending);
                e.pending = backlog;
                e.pending.extend(later);
            }
            Ok(Value::Undefined)
        }
        "adopt" => {
            let id = id_arg(vm, ctx, 1)?;
            let owner = here();
            let mut r = reg();
            if let Some(e) = r.get_mut(&id) {
                e.owner = owner;
                for (data, ports) in std::mem::take(&mut e.pending) {
                    deliver(owner, id, &data, &ports);
                }
            }
            Ok(Value::Undefined)
        }
        "close" => {
            let id = id_arg(vm, ctx, 1)?;
            let mut r = reg();
            if let Some(e) = r.remove(&id) {
                if let Some(p) = e.peer.and_then(|p| r.get_mut(&p)) {
                    p.peer = None;
                }
            }
            Ok(Value::Undefined)
        }
        _ => vm.throw_type("__port: unknown operation"),
    }
}

/// Registers `__port` and runs the prelude in `vm`'s realm. The realm must already have `EventTarget`,
/// `DOMException`, `__net_handlers`, the serializer (`js::broadcast::SERIALIZE`) and timers; the page realm
/// also has `__net_fire` and `__inbox_on`, a worker realm its own `MessageEvent`.
pub fn install_into(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__port", 4, port_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__port"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("ports-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] ports prelude failed: {m}");
    }
}

/// The page's install (after `js::broadcast`, which provides the serializer and the inbox).
pub fn install(vm: &mut Vm) {
    install_into(vm);
}

/// MessagePort, MessageChannel and the shared transfer helpers.
pub const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var PAGE = typeof __net_fire === 'function';
    var TOKEN = {};
    var ST = new WeakMap(), BY_ID = new Map(), CH = new WeakMap();
    var EMPTY = Object.freeze([]);
    var dce = function (m) { return new DOMException(m, 'DataCloneError'); };
    var tagOf = function (x) { return Object.prototype.toString.call(x).slice(8, -1); };
    var st = function (o) {
        var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };

    // ---- the MessageEvent a message task fires (with its ports)
    var fireMsg = function (t, type, data, origin, ports) {
        if (PAGE) {
            return __net_fire(t, 'MessageEvent', type, { data: data, origin: origin, ports: ports.length ? Object.freeze(ports.slice()) : EMPTY });
        }
        var e = new MessageEvent(type, { data: data, origin: origin, ports: ports });
        if (typeof __w_trust === 'function') { __w_trust(e); }
        return t.dispatchEvent(e);
    };
    // Kept for callers that registered it before the stash went (a no-op listener).
    var stash = function () {};

    // ---- StructuredSerializeWithTransfer (HTML §2.7.6) for every messaging binding
    var xferList = function (t, iface) {
        if (t === undefined || t === null) { return []; }
        if (typeof t !== 'object' && typeof t !== 'function') {
            throw new TypeError("Failed to execute 'postMessage' on '" + iface + "': The provided value is not of type '(sequence<object> or StructuredSerializeOptions)'.");
        }
        if (typeof t[Symbol.iterator] === 'function') { return Array.from(t); }
        return t.transfer === undefined ? [] : Array.from(t.transfer);
    };
    var pack = function (message, list) {
        var xp = new Map(), bufs = [];
        list.forEach(function (x, i) {
            if (x instanceof MessagePort) {
                if (xp.has(x)) { throw dce('Port at index ' + i + ' is a duplicate of an earlier port.'); }
                if (ST.get(x).shipped) { throw dce('Port at index ' + i + ' is already neutered.'); }
                xp.set(x, xp.size);
            } else if (tagOf(x) === 'ArrayBuffer') {
                if (bufs.indexOf(x) >= 0) { throw dce('ArrayBuffer at index ' + i + ' is a duplicate of an earlier ArrayBuffer.'); }
                if (typeof x.detached === 'boolean' && x.detached) { throw dce('ArrayBuffer at index ' + i + ' is already detached.'); }
                bufs.push(x);
            } else {
                throw dce('Value at index ' + i + ' does not have a transferable type.');
            }
        });
        var text = __sc_ser(message, xp);
        var ids = [];
        xp.forEach(function (i, p) {
            var s = ST.get(p);
            s.shipped = true;
            s.closed = true;
            BY_ID.delete(s.id);
            ids[i] = s.id;
            // What this realm received but has not dispatched travels with the port.
            __port('ship', s.id, JSON.stringify(s.q));
            s.q = [];
        });
        bufs.forEach(function (b) { ArrayBuffer.prototype.transfer.call(b, 0); });
        return { data: text, ports: ids.join(',') };
    };
    var adoptAll = function (csv) {
        if (!csv) { return []; }
        return String(csv).split(',').map(function (x) {
            var id = Number(x);
            var p = new MessagePort(TOKEN, id);
            __port('adopt', id);
            return p;
        });
    };
    // One message task at its target: the ports adopted, the data deserialized (messageerror on failure).
    var dispatch = function (target, text, csv) {
        var ports = adoptAll(csv);
        var data;
        try { data = __sc_de(text, ports); }
        catch (e) { fireMsg(target, 'messageerror', null, '', []); return; }
        fireMsg(target, 'message', data, '', ports);
    };

    // ---- MessagePort / MessageChannel
    var drainOne = function (p) {
        var s = ST.get(p);
        s.backlog--;
        if (s.closed || s.q.length === 0) { return; }
        var m = s.q.shift();
        dispatch(p, m[0], m[1]);
    };
    var schedule = function (p, s) {
        while (s.backlog < s.q.length) {
            s.backlog++;
            setTimeout(function () { drainOne(p); }, 0);
        }
    };
    class MessagePort extends EventTarget {
        constructor(token, id) {
            if (token !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            ST.set(this, { id: id, started: false, closed: false, shipped: false, q: [], backlog: 0 });
            BY_ID.set(id, this);
        }
        postMessage(message, transfer) {
            var s = st(this);
            if (arguments.length < 1) {
                throw new TypeError("Failed to execute 'postMessage' on 'MessagePort': 1 argument required, but only 0 present.");
            }
            var list = xferList(transfer, 'MessagePort');
            var self = list.indexOf(this);
            if (self >= 0) { throw dce("Failed to execute 'postMessage' on 'MessagePort': Port at index " + self + ' is a source port.'); }
            var pkt = pack(message, list);
            if (s.closed || s.shipped) { return; }
            __port('post', s.id, pkt.data, pkt.ports);
        }
        start() {
            var s = st(this);
            if (s.started || s.shipped) { return; }
            s.started = true;
            schedule(this, s);
        }
        close() {
            var s = st(this);
            if (s.closed) { return; }
            s.closed = true;
            s.q = [];
            __port('close', s.id);
        }
    }
    __net_handlers(MessagePort.prototype, ['message', 'messageerror']);
    (function () {
        // HTML §9.4.5: the first set of `onmessage` enables the port message queue (an implicit start()).
        var d = Object.getOwnPropertyDescriptor(MessagePort.prototype, 'onmessage');
        Object.defineProperty(MessagePort.prototype, 'onmessage', {
            configurable: true, enumerable: true, get: d.get,
            set: function (f) { d.set.call(this, f); this.start(); },
        });
    })();
    Object.defineProperty(MessagePort.prototype, Symbol.toStringTag, { value: 'MessagePort', configurable: true });
    class MessageChannel {
        constructor() {
            var ids = __port('pair');
            CH.set(this, [new MessagePort(TOKEN, ids[0]), new MessagePort(TOKEN, ids[1])]);
        }
        get port1() { var c = CH.get(this); if (!c) { throw new TypeError('Illegal invocation'); } return c[0]; }
        get port2() { var c = CH.get(this); if (!c) { throw new TypeError('Illegal invocation'); } return c[1]; }
    }
    Object.defineProperty(MessageChannel.prototype, Symbol.toStringTag, { value: 'MessageChannel', configurable: true });
    hide('MessagePort', MessagePort);
    hide('MessageChannel', MessageChannel);

    // A message for port `id` arrived on this realm's event loop (inbox / worker command): queued until the port
    // is started, dispatched as its own task after any backlog.
    hide('__port_deliver', function (id, text, csv) {
        var p = BY_ID.get(Number(id));
        if (!p) { return; }
        var s = ST.get(p);
        if (s.closed) { return; }
        if (s.started && s.q.length === 0) { dispatch(p, text, csv); return; }
        s.q.push([text, csv]);
        if (s.started) { schedule(p, s); }
    });
    hide('__msg_xfer', xferList);
    hide('__msg_pack', pack);
    hide('__msg_fire', fireMsg);
    hide('__msg_stash', stash);
    hide('__msg_dispatch', dispatch);
    if (typeof __inbox_on === 'function') {
        __inbox_on('port-message', function (m) { __port_deliver(m.port, m.data, m.ports); });
    }
})();
"#;
