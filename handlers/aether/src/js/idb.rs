//! AETHERSTORAGE (LEDGER SR94): the Indexed Database API 3.0's page-facing half — `indexedDB`, IDBFactory,
//! IDBOpenDBRequest/IDBRequest, IDBDatabase, IDBTransaction, IDBObjectStore, IDBIndex, IDBCursor(WithValue),
//! IDBKeyRange, IDBVersionChangeEvent, DOMStringList — and `navigator.storage` (StorageManager).
//!
//! CHARTER: Aether — binding over `crate::storage::idb` (engine) and `kvstore_core` (shared core).
//!
//! Split (the `api::fetch` shape): the engine — records, indexes, key order, ranges, cursor positioning, the
//! key generator, the cross-document transaction scheduler, commit and abort — is Rust behind one native,
//! `__idb(op, json)`; the IDL objects, §7.1 key conversion, §2.5 key-path evaluation and injection (they read
//! the live JS value), the request queue and the event model are the prelude below.
//!
//! The event loop (§2.7.1 transaction lifecycle): a transaction is ACTIVE in the task that created it and
//! inactive from the next task on; requests queue in order and a single pump task (`setTimeout(…, 0)`, so
//! everything stays on the page's event loop and in FIFO order) runs ONE request per task — the engine call,
//! then `success` or `error` at the request with the transaction active again during dispatch. A transaction
//! that is inactive with no request left auto-commits (`complete`); an `error` event not canceled aborts it
//! (`abort`; pending requests fail with "AbortError"); a transaction the scheduler has not started yet waits
//! in creation order behind the older overlapping ones (the engine answers `start`).
//!
//! Honest gaps (ledgered where they occur): a request's `error` event is re-dispatched at the transaction and
//! then the database instead of travelling one propagation path (its target changes on the way up); the
//! events are script-created, so `isTrusted` is false; an exception thrown by a `success` listener does not
//! abort the transaction; nested documents use the top document's storage key.

use super::idl::{self, arg, s};
use js_core::vm::*;

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

/// `__idb(op, json)` → JSON text (`{"ok": …}` or `{"e": name, "m": message}`).
fn idb_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    let a = str_arg(vm, ctx, 1)?;
    let Some(key) = crate::storage::page_storage_key() else {
        return idl::throw_dom(vm, "SecurityError", "Access to the IndexedDB API is denied in this context.");
    };
    let a: serde_json::Value = serde_json::from_str(&a).unwrap_or(serde_json::Value::Null);
    let doc = super::broadcast::doc();
    Ok(s(&crate::storage::idb::dispatch(&key, doc, &op, &a).to_string()))
}

/// `__storage_manager(op)` → JSON text (`estimate`, `persist`, `persisted`).
fn storage_manager_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    let Some(key) = crate::storage::page_storage_key() else {
        return idl::throw_dom(vm, "SecurityError", "Access to the storage manager is denied in this context.");
    };
    let origin = crate::js::net::page_origin();
    Ok(s(&crate::storage::quota::dispatch(&key, &origin, &op).to_string()))
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    for (name, len, f) in [("__idb", 2, idb_native as NativeFn), ("__storage_manager", 1, storage_manager_native)] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("idb-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] idb prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var TOKEN = {};
    var illegal = function () { return new TypeError('Illegal constructor'); };
    var mkErr = function (name, msg) { return name === 'TypeError' ? new TypeError(msg) : new DOMException(msg, name); };
    var call = function (op, args) {
        var r = JSON.parse(__idb(op, JSON.stringify(args || {})));
        if (r && r.e !== undefined) { throw mkErr(r.e, r.m); }
        return r.ok;
    };
    var hasOwn = function (o, k) { return Object.prototype.hasOwnProperty.call(o, k); };
    var need = function (n, have, iface, m) {
        if (have < n) {
            throw new TypeError("Failed to execute '" + m + "' on '" + iface + "': " + n + ' argument' + (n > 1 ? 's' : '') + ' required, but only ' + have + ' present.');
        }
    };

    // ---- §7.1 keys ---------------------------------------------------------------------------------
    var wellFormed = function (s) {
        for (var i = 0; i < s.length; i++) {
            var c = s.charCodeAt(i);
            if (c >= 0xD800 && c <= 0xDBFF) {
                var d = s.charCodeAt(i + 1);
                if (!(d >= 0xDC00 && d <= 0xDFFF)) { return false; }
                i++;
            } else if (c >= 0xDC00 && c <= 0xDFFF) { return false; }
        }
        return true;
    };
    var units = function (s) { var u = []; for (var i = 0; i < s.length; i++) { u.push(s.charCodeAt(i)); } return u; };
    var bytesOf = function (v) {
        var u8 = v instanceof ArrayBuffer ? new Uint8Array(v) : new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
        return Array.prototype.slice.call(u8);
    };
    var isDate = function (v) { return Object.prototype.toString.call(v) === '[object Date]'; };
    var toKey = function (v, seen) {
        if (typeof v === 'number') {
            if (v !== v) { return null; }
            return ['n', v === Infinity ? 'Infinity' : v === -Infinity ? '-Infinity' : v];
        }
        if (typeof v === 'string') { return wellFormed(v) ? ['s', v] : ['u', units(v)]; }
        if (v === null || typeof v !== 'object') { return null; }
        if (isDate(v)) {
            var t = v.getTime();
            return t !== t ? null : ['d', t];
        }
        if (v instanceof ArrayBuffer || ArrayBuffer.isView(v)) {
            if (v instanceof ArrayBuffer && v.detached) { return null; }
            return ['b', bytesOf(v)];
        }
        if (Array.isArray(v)) {
            seen = seen || [];
            if (seen.indexOf(v) >= 0) { return null; }
            seen.push(v);
            var out = [];
            for (var i = 0; i < v.length; i++) {
                if (!hasOwn(v, i)) { seen.pop(); return null; }
                var k = toKey(v[i], seen);
                if (k === null) { seen.pop(); return null; }
                out.push(k);
            }
            seen.pop();
            return ['a', out];
        }
        return null;
    };
    var keyOrThrow = function (v) {
        var k = toKey(v);
        if (k === null) { throw new DOMException('The parameter is not a valid key.', 'DataError'); }
        return k;
    };
    var unnum = function (n) { return n === 'Infinity' ? Infinity : n === '-Infinity' ? -Infinity : n; };
    var fromKey = function (k) {
        if (k === null || k === undefined) { return undefined; }
        switch (k[0]) {
            case 'n': return unnum(k[1]);
            case 'd': return new Date(unnum(k[1]));
            case 's': return k[1];
            case 'u': return String.fromCharCode.apply(null, k[1]);
            case 'b': return new Uint8Array(k[1]).buffer;
            case 'a': return k[1].map(fromKey);
        }
        return undefined;
    };
    var cmpWire = function (a, b) { return call('cmp', { a: a, b: b }); };

    // ---- §2.5 key paths ----------------------------------------------------------------------------
    var IDENT = /^[$A-Z_a-zª-￿][$0-9A-Z_a-zª-￿]*$/;
    var validPathString = function (p) {
        if (p === '') { return true; }
        return p.split('.').every(function (x) { return IDENT.test(x); });
    };
    var normPath = function (p) {
        // A sequence<DOMString> or a DOMString; anything else is stringified (WebIDL).
        if (p === null || p === undefined) { return null; }
        if (typeof p !== 'string' && typeof p === 'object' && typeof p[Symbol.iterator] === 'function') {
            var a = Array.from(p, String);
            if (a.length === 0 || !a.every(validPathString)) { throw new DOMException('The keyPath argument contains an invalid key path.', 'SyntaxError'); }
            return a;
        }
        var s = String(p);
        if (!validPathString(s)) { throw new DOMException('The keyPath argument contains an invalid key path.', 'SyntaxError'); }
        return s;
    };
    var FAIL = {};
    var evalPath = function (value, path) {
        if (Array.isArray(path)) {
            var out = [];
            for (var i = 0; i < path.length; i++) {
                var r = evalPath(value, path[i]);
                if (r === FAIL) { return FAIL; }
                out.push(r);
            }
            return out;
        }
        if (path === '') { return value; }
        var parts = path.split('.'), cur = value;
        for (var j = 0; j < parts.length; j++) {
            var id = parts[j];
            if (typeof cur === 'string' && id === 'length') { cur = cur.length; continue; }
            if (Array.isArray(cur) && id === 'length') { cur = cur.length; continue; }
            if (typeof Blob === 'function' && cur instanceof Blob && (id === 'size' || id === 'type')) { cur = cur[id]; continue; }
            if (typeof File === 'function' && cur instanceof File && (id === 'name' || id === 'lastModified')) { cur = cur[id]; continue; }
            if (cur === null || typeof cur !== 'object' || !hasOwn(cur, id)) { return FAIL; }
            cur = cur[id];
            if (cur === undefined) { return FAIL; }
        }
        return cur;
    };
    var canInject = function (value, path) {
        var parts = path.split('.'), cur = value;
        for (var i = 0; i < parts.length - 1; i++) {
            if (cur === null || typeof cur !== 'object') { return false; }
            if (!hasOwn(cur, parts[i])) { return true; }
            cur = cur[parts[i]];
        }
        return cur !== null && typeof cur === 'object';
    };
    var inject = function (value, path, key) {
        var parts = path.split('.'), cur = value;
        for (var i = 0; i < parts.length - 1; i++) {
            if (!hasOwn(cur, parts[i])) {
                Object.defineProperty(cur, parts[i], { value: {}, writable: true, enumerable: true, configurable: true });
            }
            cur = cur[parts[i]];
        }
        Object.defineProperty(cur, parts[parts.length - 1], { value: key, writable: true, enumerable: true, configurable: true });
    };
    var indexKeys = function (meta, value) {
        var out = {};
        for (var n in meta.indexes) {
            var im = meta.indexes[n];
            var r = evalPath(value, im.keyPath);
            if (r === FAIL) { continue; }
            if (im.multiEntry && Array.isArray(r)) {
                var ks = [];
                for (var i = 0; i < r.length; i++) { var k = toKey(r[i]); if (k !== null) { ks.push(k); } }
                out[n] = ks;
            } else {
                var k1 = toKey(r);
                if (k1 !== null) { out[n] = [k1]; }
            }
        }
        return out;
    };

    // ---- DOMStringList -----------------------------------------------------------------------------
    var DSL = typeof DOMStringList === 'function' ? null : (function () {
        class DOMStringList {
            constructor(tok, list) {
                if (tok !== TOKEN) { throw illegal(); }
                Object.defineProperty(this, '__l', { value: list.slice() });
                for (var i = 0; i < list.length; i++) { Object.defineProperty(this, i, { value: list[i], enumerable: true }); }
            }
            get length() { return this.__l.length; }
            item(i) { i = Number(i) >>> 0; return i < this.__l.length ? this.__l[i] : null; }
            contains(s) { return this.__l.indexOf(String(s)) >= 0; }
            [Symbol.iterator]() { return this.__l[Symbol.iterator](); }
        }
        Object.defineProperty(DOMStringList.prototype, Symbol.toStringTag, { value: 'DOMStringList', configurable: true });
        hide('DOMStringList', DOMStringList);
        return DOMStringList;
    })();
    var strings = function (list) {
        var sorted = list.slice().sort(function (a, b) { return a < b ? -1 : a > b ? 1 : 0; });
        if (DSL) { return new DSL(TOKEN, sorted); }
        var o = sorted.slice();
        o.item = function (i) { return i < sorted.length ? sorted[i] : null; };
        o.contains = function (s) { return sorted.indexOf(String(s)) >= 0; };
        return o;
    };

    // ---- IDBKeyRange -------------------------------------------------------------------------------
    var KR = new WeakMap();
    class IDBKeyRange {
        constructor(tok, lower, upper, lo, uo) {
            if (tok !== TOKEN) { throw illegal(); }
            KR.set(this, { lower: lower, upper: upper, lowerOpen: !!lo, upperOpen: !!uo });
        }
        get lower() { return fromKey(KR.get(this).lower); }
        get upper() { return fromKey(KR.get(this).upper); }
        get lowerOpen() { return KR.get(this).lowerOpen; }
        get upperOpen() { return KR.get(this).upperOpen; }
        includes(key) {
            need(1, arguments.length, 'IDBKeyRange', 'includes');
            var r = KR.get(this), k = keyOrThrow(key);
            if (r.lower !== null) { var a = cmpWire(r.lower, k); if (a > 0 || (a === 0 && r.lowerOpen)) { return false; } }
            if (r.upper !== null) { var b = cmpWire(r.upper, k); if (b < 0 || (b === 0 && r.upperOpen)) { return false; } }
            return true;
        }
        static only(v) { need(1, arguments.length, 'IDBKeyRange', 'only'); var k = keyOrThrow(v); return new IDBKeyRange(TOKEN, k, k, false, false); }
        static lowerBound(v, open) { need(1, arguments.length, 'IDBKeyRange', 'lowerBound'); return new IDBKeyRange(TOKEN, keyOrThrow(v), null, open, true); }
        static upperBound(v, open) { need(1, arguments.length, 'IDBKeyRange', 'upperBound'); return new IDBKeyRange(TOKEN, null, keyOrThrow(v), true, open); }
        static bound(lower, upper, lo, uo) {
            need(2, arguments.length, 'IDBKeyRange', 'bound');
            var a = keyOrThrow(lower), b = keyOrThrow(upper);
            var c = cmpWire(a, b);
            if (c > 0 || (c === 0 && (lo || uo))) { throw new DOMException('The lower key is greater than the upper key.', 'DataError'); }
            return new IDBKeyRange(TOKEN, a, b, lo, uo);
        }
    }
    Object.defineProperty(IDBKeyRange.prototype, Symbol.toStringTag, { value: 'IDBKeyRange', configurable: true });
    var rangeWire = function (r) {
        return { lower: r.lower, upper: r.upper, lowerOpen: r.lower === null ? false : r.lowerOpen, upperOpen: r.upper === null ? false : r.upperOpen };
    };
    // §7.3 "convert a value to a key range".
    var toRange = function (q, nullOk) {
        if (q instanceof IDBKeyRange) { return rangeWire(KR.get(q)); }
        if (q === undefined || q === null) {
            if (nullOk) { return null; }
            throw new DOMException('No key or key range specified.', 'DataError');
        }
        var k = keyOrThrow(q);
        return { lower: k, upper: k, lowerOpen: false, upperOpen: false };
    };

    // ---- events ------------------------------------------------------------------------------------
    class IDBVersionChangeEvent extends Event {
        constructor(type, init) {
            need(1, arguments.length, 'IDBVersionChangeEvent', 'constructor');
            super(type, init);
            var i = init || {};
            Object.defineProperty(this, '__ov', { value: i.oldVersion === undefined ? 0 : Number(i.oldVersion) });
            Object.defineProperty(this, '__nv', { value: i.newVersion === undefined || i.newVersion === null ? null : Number(i.newVersion) });
        }
        get oldVersion() { return this.__ov; }
        get newVersion() { return this.__nv; }
    }
    var fireSimple = function (target, type) { __net_fire(target, 'Event', type, undefined); };
    var fireVersion = function (target, type, ov, nv) {
        target.dispatchEvent(new IDBVersionChangeEvent(type, { oldVersion: ov, newVersion: nv }));
    };

    // ---- requests ----------------------------------------------------------------------------------
    var REQ = new WeakMap();
    var rq = function (o) { var s = REQ.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    class IDBRequest extends EventTarget {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } super(); REQ.set(this, { readyState: 'pending', result: undefined, error: null, source: null, transaction: null }); }
        get result() {
            var s = rq(this);
            if (s.readyState !== 'done') { throw new DOMException("Failed to read the 'result' property from 'IDBRequest': The request has not finished.", 'InvalidStateError'); }
            return s.result;
        }
        get error() {
            var s = rq(this);
            if (s.readyState !== 'done') { throw new DOMException("Failed to read the 'error' property from 'IDBRequest': The request has not finished.", 'InvalidStateError'); }
            return s.error;
        }
        get source() { return rq(this).source; }
        get transaction() { return rq(this).transaction; }
        get readyState() { return rq(this).readyState; }
    }
    __net_handlers(IDBRequest.prototype, ['success', 'error']);
    class IDBOpenDBRequest extends IDBRequest {
        constructor(tok) { super(tok); }
    }
    __net_handlers(IDBOpenDBRequest.prototype, ['blocked', 'upgradeneeded']);
    var newRequest = function (C, source, txn) {
        var r = new C(TOKEN);
        var s = REQ.get(r);
        s.source = source; s.transaction = txn;
        return r;
    };

    // ---- transactions ------------------------------------------------------------------------------
    var TX = new WeakMap();
    var tx = function (o) { var s = TX.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    var LIVE = [];
    var pumping = false;
    var schedule = function (ms) {
        if (pumping) { return; }
        pumping = true;
        setTimeout(pump, ms || 0);
    };
    var anyWaiting = function () { return LIVE.some(function (t) { return !t.started && t.state !== 'finished'; }); };
    var pump = function () {
        pumping = false;
        var progressed = false;
        for (var i = 0; i < LIVE.length && !progressed; i++) {
            var t = LIVE[i];
            if (t.state === 'finished') { continue; }
            if (!t.started) {
                var ok = false;
                try { ok = call('start', { txn: t.id }); } catch (e) { abortTxn(t, e); progressed = true; continue; }
                if (!ok) { continue; }
                t.started = true;
            }
            if (t.requests.length) {
                runRequest(t, t.requests.shift());
                progressed = true;
            } else if (t.state === 'inactive') {
                commitTxn(t);
                progressed = true;
            }
        }
        LIVE = LIVE.filter(function (t) { return t.state !== 'finished'; });
        if (progressed) { schedule(0); }
        else if (anyWaiting()) { schedule(10); }
    };
    var deactivateLater = function (t) {
        setTimeout(function () { if (t.state === 'active') { t.state = 'inactive'; } schedule(0); }, 0);
    };
    var runRequest = function (t, r) {
        var result, error = null;
        try { result = r.op(); } catch (e) { error = e; }
        var s = REQ.get(r.req);
        s.readyState = 'done';
        if (t.state === 'finished') { return; }
        if (error !== null) {
            if (!(error instanceof DOMException) && !(error instanceof Error)) { error = new DOMException(String(error), 'UnknownError'); }
            s.result = undefined; s.error = error;
            t.state = 'active';
            var ev = new Event('error', { bubbles: true, cancelable: true });
            r.req.dispatchEvent(ev);
            // The error's path continues at the transaction, then the database (see the module docs).
            if (!ev.cancelBubble && t.state !== 'finished') { t.txn.dispatchEvent(ev); }
            if (!ev.cancelBubble && t.state !== 'finished') { t.db.dispatchEvent(ev); }
            if (t.state !== 'finished') { t.state = 'inactive'; }
            if (!ev.defaultPrevented && t.state !== 'finished') { abortTxn(t, error); }
        } else {
            s.result = result; s.error = null;
            t.state = 'active';
            fireSimple(r.req, 'success');
            if (t.state !== 'finished') { t.state = 'inactive'; }
        }
        if (t.state === 'inactive' && t.commitRequested && !t.requests.length) { commitTxn(t); }
    };
    var commitTxn = function (t) {
        if (t.state === 'finished') { return; }
        t.state = 'committing';
        try { call('commit', { txn: t.id }); }
        catch (e) { t.state = 'inactive'; abortTxn(t, e); return; }
        t.state = 'finished';
        fireSimple(t.txn, 'complete');
        if (t.upgrade) {
            var dbs = DB.get(t.db);
            dbs.upgradeTxn = null;
            var os = REQ.get(t.openRequest);
            os.transaction = null;
            if (dbs.closePending) {
                os.result = undefined;
                os.error = new DOMException('The connection was closed.', 'AbortError');
                t.openRequest.dispatchEvent(new Event('error', { bubbles: true, cancelable: true }));
            } else {
                fireSimple(t.openRequest, 'success');
            }
        }
    };
    var abortTxn = function (t, error) {
        if (t.state === 'finished') { return; }
        var old = 0;
        try { old = call('abort', { txn: t.id }).old; } catch (e) {}
        t.state = 'finished';
        t.error = error === undefined ? null : error;
        var pend = t.requests;
        t.requests = [];
        var dbs = DB.get(t.db);
        if (t.upgrade) {
            dbs.version = old;
            try { dbs.schema = call('schema', { conn: dbs.conn }).schema; } catch (e) { dbs.schema = {}; }
            dbs.upgradeTxn = null;
        }
        setTimeout(function () {
            pend.forEach(function (r) {
                var s = REQ.get(r.req);
                s.readyState = 'done'; s.result = undefined;
                s.error = new DOMException('The transaction was aborted, so the request cannot be fulfilled.', 'AbortError');
                r.req.dispatchEvent(new Event('error', { bubbles: true, cancelable: true }));
            });
            var ev = new Event('abort', { bubbles: true });
            t.txn.dispatchEvent(ev);
            if (!ev.cancelBubble) { t.db.dispatchEvent(ev); }
            if (t.upgrade) {
                var os = REQ.get(t.openRequest);
                os.transaction = null; os.result = undefined; os.readyState = 'done';
                os.error = new DOMException('Version change transaction was aborted in upgradeneeded event handler.', 'AbortError');
                dbs.closePending = true;
                try { call('close', { conn: dbs.conn }); } catch (e) {}
                t.openRequest.dispatchEvent(new Event('error', { bubbles: true, cancelable: true }));
            }
        }, 0);
        schedule(0);
    };
    var activeOrThrow = function (t, iface, m) {
        if (t.state !== 'active') {
            throw new DOMException("Failed to execute '" + m + "' on '" + iface + "': The transaction is " + (t.state === 'finished' ? 'finished.' : 'not active.'), 'TransactionInactiveError');
        }
    };
    var queue = function (t, source, op, req) {
        var r = req || newRequest(IDBRequest, source, t.txn);
        if (req) { REQ.get(req).readyState = 'pending'; }
        t.requests.push({ req: r, op: op });
        schedule(0);
        return r;
    };
    var newTxn = function (db, id, names, mode, upgrade) {
        var o = new IDBTransaction(TOKEN);
        var t = { id: id, txn: o, db: db, names: names, mode: mode, state: 'active', started: !!upgrade, requests: [], error: null,
            upgrade: !!upgrade, openRequest: null, stores: Object.create(null), commitRequested: false, durability: 'default' };
        TX.set(o, t);
        LIVE.push(t);
        deactivateLater(t);
        return t;
    };
    class IDBTransaction extends EventTarget {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } super(); }
        get objectStoreNames() {
            var t = tx(this);
            return strings(t.upgrade ? Object.keys(DB.get(t.db).schema) : t.names);
        }
        get mode() { return tx(this).mode; }
        get durability() { return tx(this).durability; }
        get db() { return tx(this).db; }
        get error() { return tx(this).error; }
        objectStore(name) {
            need(1, arguments.length, 'IDBTransaction', 'objectStore');
            var t = tx(this);
            name = String(name);
            if (t.state === 'finished') { throw new DOMException("Failed to execute 'objectStore' on 'IDBTransaction': The transaction has finished.", 'InvalidStateError'); }
            var schema = DB.get(t.db).schema;
            if ((t.upgrade ? !hasOwn(schema, name) : t.names.indexOf(name) < 0) || !hasOwn(schema, name)) {
                throw new DOMException("Failed to execute 'objectStore' on 'IDBTransaction': The specified object store was not found.", 'NotFoundError');
            }
            return storeObj(t, name);
        }
        commit() {
            var t = tx(this);
            if (t.state !== 'active') { throw new DOMException("Failed to execute 'commit' on 'IDBTransaction': The transaction is not active.", 'InvalidStateError'); }
            t.commitRequested = true;
            t.state = 'inactive';
            schedule(0);
        }
        abort() {
            var t = tx(this);
            if (t.state === 'finished' || t.state === 'committing') {
                throw new DOMException("Failed to execute 'abort' on 'IDBTransaction': The transaction has already been committed or aborted.", 'InvalidStateError');
            }
            abortTxn(t, null);
        }
    }
    __net_handlers(IDBTransaction.prototype, ['abort', 'complete', 'error']);

    // ---- object stores -----------------------------------------------------------------------------
    var OS = new WeakMap();
    var os = function (o) { var s = OS.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    var metaOf = function (s) {
        var m = DB.get(s.t.db).schema[s.name];
        if (!m || s.deleted) { throw new DOMException('The object store has been deleted.', 'InvalidStateError'); }
        return m;
    };
    var storeObj = function (t, name) {
        if (t.stores[name]) { return t.stores[name]; }
        var o = new IDBObjectStore(TOKEN);
        OS.set(o, { t: t, name: name, indexes: Object.create(null), deleted: false });
        t.stores[name] = o;
        return o;
    };
    var readOp = function (s, what, range, count, index) {
        return function () {
            var r = call('read', { txn: s.t.id, store: s.name, index: index, what: what, range: range, count: count });
            switch (what) {
                case 'get': return r === null ? undefined : __sc_de(r.value);
                case 'getKey': return r === null ? undefined : (index ? fromKey(r.pk) : fromKey(r.key));
                case 'getAll': return r.map(function (e) { return __sc_de(e.value); });
                case 'getAllKeys': return r.map(function (e) { return fromKey(index ? e.pk : e.key); });
                case 'count': return r;
            }
        };
    };
    var countArg = function (c) {
        if (c === undefined) { return undefined; }
        var n = Number(c);
        if (!isFinite(n) || n < 0 || n > 4294967295) { throw new TypeError("Value is outside the 'unsigned long' value range."); }
        return Math.floor(n);
    };
    var DIRS = ['next', 'nextunique', 'prev', 'prevunique'];
    var dirArg = function (d) {
        if (d === undefined) { return 'next'; }
        d = String(d);
        if (DIRS.indexOf(d) < 0) { throw new TypeError("The provided value '" + d + "' is not a valid enum value of type IDBCursorDirection."); }
        return d;
    };
    var storeWrite = function (o, value, key, noOverwrite, method, haveKey) {
        var s = os(o), t = s.t;
        var meta = metaOf(s);
        activeOrThrow(t, 'IDBObjectStore', method);
        if (t.mode === 'readonly') { throw new DOMException("Failed to execute '" + method + "' on 'IDBObjectStore': The transaction is read-only.", 'ReadOnlyError'); }
        var inline = meta.keyPath !== null;
        if (inline && haveKey) { throw new DOMException("Failed to execute '" + method + "' on 'IDBObjectStore': The object store uses in-line keys and the key parameter was provided.", 'DataError'); }
        if (!inline && !meta.autoIncrement && !haveKey) { throw new DOMException("Failed to execute '" + method + "' on 'IDBObjectStore': The object store uses out-of-line keys and has no key generator and the key parameter was not provided.", 'DataError'); }
        var k = haveKey ? keyOrThrow(key) : null;
        var ser = __sc_ser(value);
        var clone = __sc_de(ser);
        if (inline) {
            var r = evalPath(clone, meta.keyPath);
            if (r !== FAIL) {
                k = toKey(r);
                if (k === null) { throw new DOMException("Failed to execute '" + method + "' on 'IDBObjectStore': Evaluating the object store's key path yielded a value that is not a valid key.", 'DataError'); }
            } else if (!meta.autoIncrement) {
                throw new DOMException("Failed to execute '" + method + "' on 'IDBObjectStore': Evaluating the object store's key path did not yield a value.", 'DataError');
            } else if (!canInject(clone, meta.keyPath)) {
                throw new DOMException("Failed to execute '" + method + "' on 'IDBObjectStore': A generated key could not be inserted into the value.", 'DataError');
            }
        }
        var ik = indexKeys(meta, clone);
        return function () {
            var key2 = k;
            if (key2 === null) {
                key2 = call('genkey', { txn: t.id, store: s.name });
                if (inline) {
                    inject(clone, meta.keyPath, fromKey(key2));
                    ser = __sc_ser(clone);
                    ik = indexKeys(meta, clone);
                }
            }
            return fromKey(call('put', { txn: t.id, store: s.name, key: key2, value: ser, index: ik, noOverwrite: noOverwrite }));
        };
    };
    class IDBObjectStore {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } }
        get name() { return os(this).name; }
        set name(v) {
            var s = os(this), t = s.t;
            v = String(v);
            metaOf(s);
            if (!t.upgrade) { throw new DOMException("Failed to set the 'name' property on 'IDBObjectStore': The database is not running a version change transaction.", 'InvalidStateError'); }
            activeOrThrow(t, 'IDBObjectStore', 'name');
            if (v === s.name) { return; }
            call('renameStore', { txn: t.id, from: s.name, to: v });
            var schema = DB.get(t.db).schema;
            schema[v] = schema[s.name]; delete schema[s.name];
            delete t.stores[s.name]; t.stores[v] = this;
            s.name = v;
        }
        get keyPath() { var p = metaOf(os(this)).keyPath; return Array.isArray(p) ? p.slice() : p; }
        get indexNames() { return strings(Object.keys(metaOf(os(this)).indexes)); }
        get transaction() { return os(this).t.txn; }
        get autoIncrement() { return metaOf(os(this)).autoIncrement; }
        put(value, key) { need(1, arguments.length, 'IDBObjectStore', 'put'); return queue(os(this).t, this, storeWrite(this, value, key, false, 'put', arguments.length > 1 && key !== undefined)); }
        add(value, key) { need(1, arguments.length, 'IDBObjectStore', 'add'); return queue(os(this).t, this, storeWrite(this, value, key, true, 'add', arguments.length > 1 && key !== undefined)); }
        delete(query) {
            need(1, arguments.length, 'IDBObjectStore', 'delete');
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'delete');
            if (s.t.mode === 'readonly') { throw new DOMException("Failed to execute 'delete' on 'IDBObjectStore': The transaction is read-only.", 'ReadOnlyError'); }
            var range = toRange(query, false);
            return queue(s.t, this, function () { call('delete', { txn: s.t.id, store: s.name, range: range }); return undefined; });
        }
        clear() {
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'clear');
            if (s.t.mode === 'readonly') { throw new DOMException("Failed to execute 'clear' on 'IDBObjectStore': The transaction is read-only.", 'ReadOnlyError'); }
            return queue(s.t, this, function () { call('clear', { txn: s.t.id, store: s.name }); return undefined; });
        }
        get(query) {
            need(1, arguments.length, 'IDBObjectStore', 'get');
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'get');
            return queue(s.t, this, readOp(s, 'get', toRange(query, false)));
        }
        getKey(query) {
            need(1, arguments.length, 'IDBObjectStore', 'getKey');
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'getKey');
            return queue(s.t, this, readOp(s, 'getKey', toRange(query, false)));
        }
        getAll(query, count) {
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'getAll');
            return queue(s.t, this, readOp(s, 'getAll', toRange(query, true), countArg(count)));
        }
        getAllKeys(query, count) {
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'getAllKeys');
            return queue(s.t, this, readOp(s, 'getAllKeys', toRange(query, true), countArg(count)));
        }
        count(query) {
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'count');
            return queue(s.t, this, readOp(s, 'count', toRange(query, true)));
        }
        openCursor(query, direction) {
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'openCursor');
            return openCursor(s, this, null, toRange(query, true), dirArg(direction), false);
        }
        openKeyCursor(query, direction) {
            var s = os(this); metaOf(s); activeOrThrow(s.t, 'IDBObjectStore', 'openKeyCursor');
            return openCursor(s, this, null, toRange(query, true), dirArg(direction), true);
        }
        index(name) {
            need(1, arguments.length, 'IDBObjectStore', 'index');
            var s = os(this), meta = metaOf(s);
            name = String(name);
            if (s.t.state === 'finished') { throw new DOMException("Failed to execute 'index' on 'IDBObjectStore': The transaction has finished.", 'InvalidStateError'); }
            if (!hasOwn(meta.indexes, name)) { throw new DOMException("Failed to execute 'index' on 'IDBObjectStore': The specified index was not found.", 'NotFoundError'); }
            if (s.indexes[name]) { return s.indexes[name]; }
            var ix = new IDBIndex(TOKEN);
            IX.set(ix, { s: s, store: this, name: name, deleted: false });
            s.indexes[name] = ix;
            return ix;
        }
        createIndex(name, keyPath, options) {
            need(2, arguments.length, 'IDBObjectStore', 'createIndex');
            var s = os(this), t = s.t, meta = metaOf(s);
            name = String(name);
            if (!t.upgrade) { throw new DOMException("Failed to execute 'createIndex' on 'IDBObjectStore': The database is not running a version change transaction.", 'InvalidStateError'); }
            activeOrThrow(t, 'IDBObjectStore', 'createIndex');
            if (hasOwn(meta.indexes, name)) { throw new DOMException("Failed to execute 'createIndex' on 'IDBObjectStore': An index with the specified name already exists.", 'ConstraintError'); }
            var path = normPath(keyPath);
            var o = options || {};
            var unique = !!o.unique, multi = !!o.multiEntry;
            if (multi && Array.isArray(path)) { throw new DOMException("Failed to execute 'createIndex' on 'IDBObjectStore': The keyPath argument was an array and the multiEntry option is true.", 'InvalidAccessError'); }
            var im = { keyPath: path, unique: unique, multiEntry: multi };
            var probe = { indexes: {} };
            probe.indexes[name] = im;
            var all = call('read', { txn: t.id, store: s.name, what: 'getAll', range: null });
            var entries = all.map(function (e) { var ks = indexKeys(probe, __sc_de(e.value)); return [e.key, ks[name] || []]; });
            try {
                call('createIndex', { txn: t.id, store: s.name, name: name, keyPath: path, unique: unique, multiEntry: multi, entries: entries });
            } catch (e) {
                if (e.name !== 'ConstraintError') { throw e; }
                // §4.5 step 13: a uniqueness clash aborts the upgrade transaction, it does not throw.
                meta.indexes[name] = im;
                var ix0 = this.index(name);
                setTimeout(function () { abortTxn(t, e); }, 0);
                return ix0;
            }
            meta.indexes[name] = im;
            return this.index(name);
        }
        deleteIndex(name) {
            need(1, arguments.length, 'IDBObjectStore', 'deleteIndex');
            var s = os(this), t = s.t, meta = metaOf(s);
            name = String(name);
            if (!t.upgrade) { throw new DOMException("Failed to execute 'deleteIndex' on 'IDBObjectStore': The database is not running a version change transaction.", 'InvalidStateError'); }
            activeOrThrow(t, 'IDBObjectStore', 'deleteIndex');
            if (!hasOwn(meta.indexes, name)) { throw new DOMException("Failed to execute 'deleteIndex' on 'IDBObjectStore': The specified index was not found.", 'NotFoundError'); }
            call('deleteIndex', { txn: t.id, store: s.name, name: name });
            delete meta.indexes[name];
            if (s.indexes[name]) { IX.get(s.indexes[name]).deleted = true; delete s.indexes[name]; }
        }
    }
    Object.defineProperty(IDBObjectStore.prototype, Symbol.toStringTag, { value: 'IDBObjectStore', configurable: true });

    // ---- indexes -----------------------------------------------------------------------------------
    var IX = new WeakMap();
    var ixs = function (o) { var s = IX.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    var imetaOf = function (x) {
        var m = metaOf(x.s);
        if (x.deleted || !hasOwn(m.indexes, x.name)) { throw new DOMException('The index has been deleted.', 'InvalidStateError'); }
        return m.indexes[x.name];
    };
    class IDBIndex {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } }
        get name() { return ixs(this).name; }
        set name(v) {
            var x = ixs(this), t = x.s.t;
            v = String(v);
            imetaOf(x);
            if (!t.upgrade) { throw new DOMException("Failed to set the 'name' property on 'IDBIndex': The database is not running a version change transaction.", 'InvalidStateError'); }
            activeOrThrow(t, 'IDBIndex', 'name');
            if (v === x.name) { return; }
            call('renameIndex', { txn: t.id, store: x.s.name, from: x.name, to: v });
            var m = metaOf(x.s);
            m.indexes[v] = m.indexes[x.name]; delete m.indexes[x.name];
            delete x.s.indexes[x.name]; x.s.indexes[v] = this;
            x.name = v;
        }
        get objectStore() { return ixs(this).store; }
        get keyPath() { var p = imetaOf(ixs(this)).keyPath; return Array.isArray(p) ? p.slice() : p; }
        get multiEntry() { return imetaOf(ixs(this)).multiEntry; }
        get unique() { return imetaOf(ixs(this)).unique; }
        get(query) {
            need(1, arguments.length, 'IDBIndex', 'get');
            var x = ixs(this); imetaOf(x); activeOrThrow(x.s.t, 'IDBIndex', 'get');
            return queue(x.s.t, this, readOp(x.s, 'get', toRange(query, false), undefined, x.name));
        }
        getKey(query) {
            need(1, arguments.length, 'IDBIndex', 'getKey');
            var x = ixs(this); imetaOf(x); activeOrThrow(x.s.t, 'IDBIndex', 'getKey');
            return queue(x.s.t, this, readOp(x.s, 'getKey', toRange(query, false), undefined, x.name));
        }
        getAll(query, count) {
            var x = ixs(this); imetaOf(x); activeOrThrow(x.s.t, 'IDBIndex', 'getAll');
            return queue(x.s.t, this, readOp(x.s, 'getAll', toRange(query, true), countArg(count), x.name));
        }
        getAllKeys(query, count) {
            var x = ixs(this); imetaOf(x); activeOrThrow(x.s.t, 'IDBIndex', 'getAllKeys');
            return queue(x.s.t, this, readOp(x.s, 'getAllKeys', toRange(query, true), countArg(count), x.name));
        }
        count(query) {
            var x = ixs(this); imetaOf(x); activeOrThrow(x.s.t, 'IDBIndex', 'count');
            return queue(x.s.t, this, readOp(x.s, 'count', toRange(query, true), undefined, x.name));
        }
        openCursor(query, direction) {
            var x = ixs(this); imetaOf(x); activeOrThrow(x.s.t, 'IDBIndex', 'openCursor');
            return openCursor(x.s, this, x.name, toRange(query, true), dirArg(direction), false);
        }
        openKeyCursor(query, direction) {
            var x = ixs(this); imetaOf(x); activeOrThrow(x.s.t, 'IDBIndex', 'openKeyCursor');
            return openCursor(x.s, this, x.name, toRange(query, true), dirArg(direction), true);
        }
    }
    Object.defineProperty(IDBIndex.prototype, Symbol.toStringTag, { value: 'IDBIndex', configurable: true });

    // ---- cursors -----------------------------------------------------------------------------------
    var CU = new WeakMap();
    var cu = function (o) { var s = CU.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    var step = function (c, target, targetPk, count) {
        var r = null, n = count || 1;
        var posKey = c.keyWire, posPk = c.pkWire;
        while (n-- > 0) {
            r = call('cursor', { txn: c.s.t.id, store: c.s.name, index: c.index, range: c.range, dir: c.dir,
                posKey: posKey === undefined ? null : posKey, posPk: posPk === undefined ? null : posPk,
                target: target || null, targetPk: targetPk || null, value: !c.keyOnly });
            if (r === null) { break; }
            posKey = r.key; posPk = r.pk; target = null; targetPk = null;
        }
        if (r === null) {
            c.keyWire = undefined; c.pkWire = undefined; c.key = undefined; c.pk = undefined; c.value = undefined; c.gotValue = false;
            return null;
        }
        c.keyWire = r.key; c.pkWire = r.pk; c.key = fromKey(r.key); c.pk = fromKey(r.pk);
        if (!c.keyOnly) { c.value = __sc_de(r.value); }
        c.gotValue = true;
        return c.obj;
    };
    var openCursor = function (s, source, index, range, dir, keyOnly) {
        var o = keyOnly ? new IDBCursor(TOKEN) : new IDBCursorWithValue(TOKEN);
        var c = { s: s, source: source, index: index, range: range, dir: dir, keyOnly: keyOnly, obj: o, req: null,
            keyWire: undefined, pkWire: undefined, key: undefined, pk: undefined, value: undefined, gotValue: false };
        CU.set(o, c);
        c.req = queue(s.t, source, function () { return step(c); });
        return c.req;
    };
    var cursorChecks = function (c, m) {
        activeOrThrow(c.s.t, 'IDBCursor', m);
        if (c.s.deleted || (c.index !== null && IX.get(c.source).deleted)) { throw new DOMException("Failed to execute '" + m + "' on 'IDBCursor': The cursor's source or effective object store has been deleted.", 'InvalidStateError'); }
        if (!c.gotValue) { throw new DOMException("Failed to execute '" + m + "' on 'IDBCursor': The cursor is being iterated or has iterated past its end.", 'InvalidStateError'); }
    };
    class IDBCursor {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } }
        get source() { return cu(this).source; }
        get direction() { return cu(this).dir; }
        get key() { return cu(this).key; }
        get primaryKey() { return cu(this).pk; }
        get request() { return cu(this).req; }
        advance(count) {
            need(1, arguments.length, 'IDBCursor', 'advance');
            var c = cu(this), n = Number(count);
            if (!(n >= 1 && n <= 4294967295)) { throw new TypeError("Failed to execute 'advance' on 'IDBCursor': A count argument with value 0 (zero) was supplied, must be greater than 0."); }
            cursorChecks(c, 'advance');
            c.gotValue = false;
            queue(c.s.t, c.source, function () { return step(c, null, null, Math.floor(n)); }, c.req);
        }
        continue(key) {
            var c = cu(this);
            cursorChecks(c, 'continue');
            var target = null;
            if (key !== undefined) {
                target = keyOrThrow(key);
                var d = cmpWire(target, c.keyWire);
                if ((c.dir.indexOf('next') === 0 && d <= 0) || (c.dir.indexOf('prev') === 0 && d >= 0)) {
                    throw new DOMException("Failed to execute 'continue' on 'IDBCursor': The parameter is less than or equal to this cursor's position.", 'DataError');
                }
            }
            c.gotValue = false;
            queue(c.s.t, c.source, function () { return step(c, target); }, c.req);
        }
        continuePrimaryKey(key, primaryKey) {
            need(2, arguments.length, 'IDBCursor', 'continuePrimaryKey');
            var c = cu(this);
            activeOrThrow(c.s.t, 'IDBCursor', 'continuePrimaryKey');
            if (c.index === null) { throw new DOMException("Failed to execute 'continuePrimaryKey' on 'IDBCursor': The cursor's source is not an index.", 'InvalidAccessError'); }
            if (c.dir !== 'next' && c.dir !== 'prev') { throw new DOMException("Failed to execute 'continuePrimaryKey' on 'IDBCursor': The cursor's direction is not 'next' or 'prev'.", 'InvalidAccessError'); }
            cursorChecks(c, 'continuePrimaryKey');
            var k = keyOrThrow(key), p = keyOrThrow(primaryKey);
            var d = cmpWire(k, c.keyWire);
            var fwd = c.dir === 'next';
            if ((fwd && d < 0) || (!fwd && d > 0)) { throw new DOMException("Failed to execute 'continuePrimaryKey' on 'IDBCursor': The key is behind the cursor's position.", 'DataError'); }
            if (d === 0) {
                var e = cmpWire(p, c.pkWire);
                if ((fwd && e <= 0) || (!fwd && e >= 0)) { throw new DOMException("Failed to execute 'continuePrimaryKey' on 'IDBCursor': The primary key is behind the cursor's position.", 'DataError'); }
            }
            c.gotValue = false;
            queue(c.s.t, c.source, function () { return step(c, k, p); }, c.req);
        }
        update(value) {
            need(1, arguments.length, 'IDBCursor', 'update');
            var c = cu(this), t = c.s.t;
            activeOrThrow(t, 'IDBCursor', 'update');
            if (t.mode === 'readonly') { throw new DOMException("Failed to execute 'update' on 'IDBCursor': The record may not be updated inside a read-only transaction.", 'ReadOnlyError'); }
            cursorChecks(c, 'update');
            if (c.keyOnly) { throw new DOMException("Failed to execute 'update' on 'IDBCursor': The cursor is a key cursor.", 'InvalidStateError'); }
            var meta = metaOf(c.s);
            var ser = __sc_ser(value), clone = __sc_de(ser);
            if (meta.keyPath !== null) {
                var r = evalPath(clone, meta.keyPath), k = r === FAIL ? null : toKey(r);
                if (k === null || cmpWire(k, c.pkWire) !== 0) {
                    throw new DOMException("Failed to execute 'update' on 'IDBCursor': The effective object store of this cursor uses in-line keys and evaluating the key path of the value parameter results in a different value than the cursor's effective key.", 'DataError');
                }
            }
            var ik = indexKeys(meta, clone), pk = c.pkWire, name = c.s.name;
            return queue(t, this, function () { return fromKey(call('put', { txn: t.id, store: name, key: pk, value: ser, index: ik, noOverwrite: false })); });
        }
        delete() {
            var c = cu(this), t = c.s.t;
            activeOrThrow(t, 'IDBCursor', 'delete');
            if (t.mode === 'readonly') { throw new DOMException("Failed to execute 'delete' on 'IDBCursor': The record may not be deleted inside a read-only transaction.", 'ReadOnlyError'); }
            cursorChecks(c, 'delete');
            if (c.keyOnly) { throw new DOMException("Failed to execute 'delete' on 'IDBCursor': The cursor is a key cursor.", 'InvalidStateError'); }
            var pk = c.pkWire, name = c.s.name;
            return queue(t, this, function () { call('delete', { txn: t.id, store: name, range: { lower: pk, upper: pk } }); return undefined; });
        }
    }
    class IDBCursorWithValue extends IDBCursor {
        constructor(tok) { super(tok); }
        get value() { return cu(this).value; }
    }

    // ---- databases ---------------------------------------------------------------------------------
    var DB = new WeakMap();
    var CONNS = new Map();
    var dbs = function (o) { var s = DB.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    class IDBDatabase extends EventTarget {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } super(); }
        get name() { return dbs(this).name; }
        get version() { return dbs(this).version; }
        get objectStoreNames() { return strings(Object.keys(dbs(this).schema)); }
        createObjectStore(name, options) {
            need(1, arguments.length, 'IDBDatabase', 'createObjectStore');
            var d = dbs(this), t = d.upgradeTxn;
            name = String(name);
            if (!t) { throw new DOMException("Failed to execute 'createObjectStore' on 'IDBDatabase': The database is not running a version change transaction.", 'InvalidStateError'); }
            activeOrThrow(t, 'IDBDatabase', 'createObjectStore');
            var o = options || {};
            var path = o.keyPath === undefined ? null : normPath(o.keyPath);
            var auto = !!o.autoIncrement;
            if (hasOwn(d.schema, name)) { throw new DOMException("Failed to execute 'createObjectStore' on 'IDBDatabase': An object store with the specified name already exists.", 'ConstraintError'); }
            if (auto && (path === '' || Array.isArray(path))) { throw new DOMException("Failed to execute 'createObjectStore' on 'IDBDatabase': The autoIncrement option was set but the keyPath option was empty or an array.", 'InvalidAccessError'); }
            call('createStore', { txn: t.id, name: name, keyPath: path, autoIncrement: auto });
            d.schema[name] = { keyPath: path, autoIncrement: auto, indexes: {} };
            return storeObj(t, name);
        }
        deleteObjectStore(name) {
            need(1, arguments.length, 'IDBDatabase', 'deleteObjectStore');
            var d = dbs(this), t = d.upgradeTxn;
            name = String(name);
            if (!t) { throw new DOMException("Failed to execute 'deleteObjectStore' on 'IDBDatabase': The database is not running a version change transaction.", 'InvalidStateError'); }
            activeOrThrow(t, 'IDBDatabase', 'deleteObjectStore');
            if (!hasOwn(d.schema, name)) { throw new DOMException("Failed to execute 'deleteObjectStore' on 'IDBDatabase': The specified object store was not found.", 'NotFoundError'); }
            call('deleteStore', { txn: t.id, name: name });
            delete d.schema[name];
            if (t.stores[name]) { OS.get(t.stores[name]).deleted = true; delete t.stores[name]; }
        }
        transaction(storeNames, mode, options) {
            need(1, arguments.length, 'IDBDatabase', 'transaction');
            var d = dbs(this);
            if (d.closePending) { throw new DOMException("Failed to execute 'transaction' on 'IDBDatabase': The database connection is closing.", 'InvalidStateError'); }
            if (d.upgradeTxn && d.upgradeTxn.state !== 'finished') { throw new DOMException("Failed to execute 'transaction' on 'IDBDatabase': A version change transaction is running.", 'InvalidStateError'); }
            var names = typeof storeNames === 'string' ? [storeNames] : Array.from(storeNames, String);
            var m = mode === undefined ? 'readonly' : String(mode);
            if (m !== 'readonly' && m !== 'readwrite') {
                throw new TypeError("Failed to execute 'transaction' on 'IDBDatabase': The provided value '" + m + "' is not a valid enum value of type IDBTransactionMode.");
            }
            var id = call('begin', { conn: d.conn, stores: names, mode: m });
            var uniq = names.filter(function (n, i) { return names.indexOf(n) === i; }).sort();
            var t = newTxn(this, id, uniq, m, false);
            if (options && options.durability !== undefined) {
                var du = String(options.durability);
                if (['default', 'strict', 'relaxed'].indexOf(du) < 0) { throw new TypeError("The provided value '" + du + "' is not a valid enum value of type IDBTransactionDurability."); }
                t.durability = du;
            }
            schedule(0);
            return t.txn;
        }
        close() {
            var d = dbs(this);
            if (d.closePending) { return; }
            d.closePending = true;
            CONNS.delete(d.conn);
            call('close', { conn: d.conn });
        }
    }
    __net_handlers(IDBDatabase.prototype, ['abort', 'close', 'error', 'versionchange']);
    __inbox_on('idb-versionchange', function (m) {
        var db = CONNS.get(m.conn);
        if (!db || dbs(db).closePending) { return; }
        fireVersion(db, 'versionchange', m.old, m['new']);
    });

    // ---- the factory -------------------------------------------------------------------------------
    var MAX_VERSION = 9007199254740991;
    class IDBFactory {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } }
        open(name, version) {
            need(1, arguments.length, 'IDBFactory', 'open');
            name = String(name);
            var v;
            if (version !== undefined) {
                v = Number(version);
                if (!(v >= 1 && v <= MAX_VERSION) || Math.floor(v) !== v) {
                    throw new TypeError("Failed to execute 'open' on 'IDBFactory': The version provided must not be 0.");
                }
            }
            __idb('databases', '{}'); // the SecurityError of an opaque origin throws here, synchronously
            var req = newRequest(IDBOpenDBRequest, null, null);
            var blockedSeen = 0;
            var fail = function (e) {
                var s = REQ.get(req);
                s.readyState = 'done'; s.result = undefined; s.error = e;
                req.dispatchEvent(new Event('error', { bubbles: true, cancelable: true }));
            };
            var attempt = function (notify) {
                var r;
                try { r = call('open', { name: name, version: v, notify: notify }); } catch (e) { fail(e); return; }
                if (r.wait) { setTimeout(function () { attempt(true); }, 10); return; }
                if (r.blocked) {
                    blockedSeen++;
                    // The `versionchange` events reach the other connections first (the inbox runs before the
                    // timers); `blocked` fires once if they are still open on the next look.
                    if (blockedSeen === 2) { fireVersion(req, 'blocked', r.old, r['new']); }
                    setTimeout(function () { attempt(false); }, blockedSeen === 1 ? 0 : 10);
                    return;
                }
                var db = new IDBDatabase(TOKEN);
                var d = { conn: r.conn, name: name, version: r['new'], schema: r.schema || {}, closePending: false, upgradeTxn: null };
                DB.set(db, d);
                CONNS.set(r.conn, db);
                var s = REQ.get(req);
                s.result = db;
                if (r.upgrade) {
                    var t = newTxn(db, r.upgrade, Object.keys(d.schema), 'versionchange', true);
                    t.openRequest = req;
                    d.upgradeTxn = t;
                    s.transaction = t.txn;
                    s.readyState = 'done';
                    t.state = 'active';
                    fireVersion(req, 'upgradeneeded', r.old, r['new']);
                    if (t.state === 'active') { t.state = 'inactive'; }
                    schedule(0);
                } else {
                    s.readyState = 'done';
                    fireSimple(req, 'success');
                }
            };
            setTimeout(function () { attempt(true); }, 0);
            return req;
        }
        deleteDatabase(name) {
            need(1, arguments.length, 'IDBFactory', 'deleteDatabase');
            name = String(name);
            __idb('databases', '{}');
            var req = newRequest(IDBOpenDBRequest, null, null);
            var blockedSeen = 0;
            var attempt = function (notify) {
                var r, s = REQ.get(req);
                try { r = call('deleteDatabase', { name: name, notify: notify }); }
                catch (e) {
                    s.readyState = 'done'; s.error = e;
                    req.dispatchEvent(new Event('error', { bubbles: true, cancelable: true }));
                    return;
                }
                if (r.wait) { setTimeout(function () { attempt(true); }, 10); return; }
                if (r.blocked) {
                    blockedSeen++;
                    if (blockedSeen === 2) { fireVersion(req, 'blocked', r.old, null); }
                    setTimeout(function () { attempt(false); }, blockedSeen === 1 ? 0 : 10);
                    return;
                }
                s.readyState = 'done'; s.result = undefined;
                fireVersion(req, 'success', r.deleted, null);
            };
            setTimeout(function () { attempt(true); }, 0);
            return req;
        }
        databases() {
            return new Promise(function (resolve, reject) {
                try { resolve(call('databases', {})); } catch (e) { reject(e); }
            });
        }
        cmp(a, b) {
            need(2, arguments.length, 'IDBFactory', 'cmp');
            return cmpWire(keyOrThrow(a), keyOrThrow(b));
        }
    }
    var factory = new IDBFactory(TOKEN);
    Object.defineProperty(globalThis, 'indexedDB', { get: function () { return factory; }, configurable: true, enumerable: true });
    [IDBFactory, IDBDatabase, IDBTransaction, IDBObjectStore, IDBIndex, IDBCursor, IDBCursorWithValue, IDBKeyRange,
     IDBRequest, IDBOpenDBRequest, IDBVersionChangeEvent].forEach(function (C) {
        try { Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true }); } catch (e) {}
        hide(C.name, C);
    });

    // ---- navigator.storage (Storage §5) -----------------------------------------------------------
    var sm = function (op) { return JSON.parse(__storage_manager(op)); };
    class StorageManager {
        constructor(tok) { if (tok !== TOKEN) { throw illegal(); } }
        estimate() { return new Promise(function (res, rej) { try { res(sm('estimate')); } catch (e) { rej(e); } }); }
        persist() { return new Promise(function (res, rej) { try { res(sm('persist')); } catch (e) { rej(e); } }); }
        persisted() { return new Promise(function (res, rej) { try { res(sm('persisted')); } catch (e) { rej(e); } }); }
        getDirectory() {
            __ledger('navigator.storage.getDirectory-no-opfs');
            return Promise.reject(new DOMException('The origin private file system is not available.', 'NotSupportedError'));
        }
    }
    Object.defineProperty(StorageManager.prototype, Symbol.toStringTag, { value: 'StorageManager', configurable: true });
    hide('StorageManager', StorageManager);
    var manager = new StorageManager(TOKEN);
    var NP = (typeof Navigator === 'function' && Navigator.prototype) || (typeof navigator === 'object' && Object.getPrototypeOf(navigator));
    if (NP) {
        Object.defineProperty(NP, 'storage', { get: function () { return manager; }, configurable: true, enumerable: true });
    }
})();
"#;

#[cfg(test)]
mod tests {
    /// The page half of the IndexedDB ordering KAT (Indexed DB 3.0 §2.7.2): requests of one transaction
    /// complete in request order; a read-only transaction created after an overlapping read/write one starts
    /// only after it completed (and sees its writes); an `error` not canceled aborts its transaction and its
    /// writes are undone; the upgrade transaction runs before the open request's `success`.
    #[test]
    fn transaction_ordering_kat() {
        let dir = std::env::temp_dir().join(format!("aether_idb_page_kat_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::storage::set_root_dir(Some(dir.clone()));
        let doc = crate::dom::parse_html("<html><body></body></html>");
        let mut e = crate::js::Engine::new(doc);
        e.set_location("https://idb.kat.example/");
        e.execute(
            r#"
            var log = [];
            var r = indexedDB.open('kat', 1);
            r.onupgradeneeded = function () {
                log.push('upgrade');
                var db = r.result;
                var s = db.createObjectStore('s', { keyPath: 'id' });
                s.createIndex('n', 'n', { unique: true });
            };
            r.onsuccess = function () {
                log.push('open');
                var db = r.result;
                var rw = db.transaction('s', 'readwrite');
                var s = rw.objectStore('s');
                s.put({ id: 1, n: 'a' }).onsuccess = function () { log.push('put1'); };
                s.put({ id: 2, n: 'b' }).onsuccess = function () { log.push('put2'); };
                rw.oncomplete = function () { log.push('rw-complete'); };
                var ro = db.transaction('s', 'readonly');
                ro.objectStore('s').count().onsuccess = function (ev) { log.push('count=' + ev.target.result); };
                ro.oncomplete = function () {
                    log.push('ro-complete');
                    var bad = db.transaction('s', 'readwrite');
                    var bs = bad.objectStore('s');
                    bs.put({ id: 3, n: 'c' });
                    bs.add({ id: 4, n: 'a' }).onerror = function () { log.push('constraint'); };
                    bad.onabort = function () {
                        log.push('abort');
                        var check = db.transaction('s').objectStore('s').getAllKeys();
                        check.onsuccess = function () { log.push('keys=' + JSON.stringify(check.result)); window.__done = true; };
                    };
                };
            };
            "#,
        )
        .unwrap();
        for _ in 0..400 {
            e.tick();
            if e.eval_string("String(!!window.__done)").unwrap() == "true" {
                break;
            }
        }
        let log = e.eval_string("log.join(',')").unwrap();
        assert_eq!(log, "upgrade,open,put1,put2,rw-complete,count=2,ro-complete,constraint,abort,keys=[1,2]");
        crate::storage::set_root_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
