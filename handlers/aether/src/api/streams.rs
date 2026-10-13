//! AETHERWPT3 (LEDGER SR138): the Streams Standard (WHATWG, §4 ReadableStream with the default and byte controllers,
//! the default and BYOB readers, tee, pipeTo / pipeThrough, async iteration and `ReadableStream.from`; §5
//! WritableStream, its writer and controller; §6 TransformStream; §7 the two queuing strategies), the Encoding
//! Standard's TextEncoderStream / TextDecoderStream (§5, §6), and the Compression Streams `DecompressionStream`
//! (§5) over pixel_core's inflater — installed by `js::net::install` ahead of `api::fetch::PRELUDE`, whose Body
//! mixin and `Blob.stream()` build on it (and read "disturbed" through `__rs_disturbed`).
//!
//! The JS is the specification's abstract operations, one function per algorithm and named after it
//! (`rsdcEnqueue` = ReadableStreamDefaultControllerEnqueue, `rbscRespondInReadableState` =
//! ReadableByteStreamControllerRespondInReadableState, …), over branded internal records (no slot is
//! reachable from the page). ArrayBuffers move by `ArrayBuffer.prototype.transfer` (TransferArrayBuffer).
//!
//! `CompressionStream` is NOT here: its compressor is git_core's DEFLATE encoder (`git_core::deflate`, the one
//! encoder in the tree — pixel_core and http_core only inflate), and handlers/aether/Cargo.toml has no git_core
//! dependency; that one line is outside this arc's files (see docs/dev/evidence/aetherwpt/WPT.md, AETHERWPT3 O1).

use crate::js::idl::arg;
use js_core::builtins::typedarray::new_array_buffer;
use js_core::vm::*;
use pixel_core::inflate::{self, ByteSource, InflateError, Sink};

struct Src<'a> {
    b: &'a [u8],
    at: usize,
}

impl ByteSource for Src<'_> {
    fn next(&mut self) -> Option<u8> {
        let v = self.b.get(self.at).copied();
        self.at += 1;
        v
    }
}

/// Collects the output past `skip` bytes (what an earlier call already enqueued).
struct Tail {
    skip: usize,
    seen: usize,
    out: Vec<u8>,
}

impl Sink for Tail {
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.seen >= self.skip {
            self.out.push(byte);
        }
        self.seen += 1;
        Ok(())
    }
}

/// How far a decompression of everything received so far got.
#[derive(Debug, PartialEq, Eq)]
pub enum Inflated {
    /// The input ends inside the stream: more is needed (the output so far is returned).
    More,
    /// The stream ended after `consumed` input bytes.
    Done(usize),
    /// The input is not a valid stream of the format.
    Error,
}

/// Decompresses `input` (the whole input so far) as `format` (`deflate` = RFC 1950 zlib, `deflate-raw` = RFC 1951,
/// `gzip` = RFC 1952 one member; Compression Streams §2), answering the output past the first `skip` bytes.
/// Re-running from the start each time keeps pixel_core's inflater (a pull decoder) unchanged; the output of
/// a prefix of a stream is a prefix of its output, so the tail is exactly what is new.
pub fn inflate_from(format: &str, input: &[u8], skip: usize) -> (Inflated, Vec<u8>) {
    let mut src = Src { b: input, at: 0 };
    let mut sink = Tail { skip, seen: 0, out: Vec::new() };
    let r = match format {
        "deflate" => inflate::zlib_inflate(&mut src, &mut sink).map(|r| r.compressed),
        "deflate-raw" => inflate::inflate_raw(&mut src, &mut sink).map(|r| r.compressed),
        "gzip" => inflate::gunzip(&mut src, &mut sink).map(|r| r.compressed),
        _ => Err(InflateError::BadBlock),
    };
    let state = match r {
        Ok(consumed) => Inflated::Done(consumed as usize),
        Err(InflateError::TruncatedInput) => Inflated::More,
        Err(_) => Inflated::Error,
    };
    (state, sink.out)
}

/// `__inflate(format, bytes, skip)` → `[state, output ArrayBuffer, consumed]`, state "more" | "done" | "error".
fn n_inflate(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let f = arg(vm, ctx, 0);
    let format = vm.to_string(&f)?.to_rust();
    let b = arg(vm, ctx, 1);
    let input = crate::js::net::buffer_bytes(vm, &b).unwrap_or_default();
    let s = arg(vm, ctx, 2);
    let skip = vm.to_number(&s)?.max(0.0) as usize;
    let (state, out) = inflate_from(&format, &input, skip);
    let (name, consumed) = match state {
        Inflated::More => ("more", 0),
        Inflated::Done(n) => ("done", n),
        Inflated::Error => ("error", 0),
    };
    let ab = new_array_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(ab).kind {
        d.data = out;
    }
    let items = vec![crate::js::idl::s(name), Value::Object(ab), Value::Number(consumed as f64)];
    Ok(Value::Object(vm.new_array(items)))
}

/// Installs `__inflate` (the prelude's DecompressionStream reads it).
pub fn init(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__inflate", 3, n_inflate, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__inflate"), Prop::data(Value::Object(fo), WC));
}

/// The Streams Standard and the stream classes over it.
pub const PRELUDE: &str = r#"
(function () {
    'use strict';
    var G = globalThis;
    var hide = function (o, name, v) { Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false }); };
    var methods = function (proto, table) {
        Object.keys(table).forEach(function (k) {
            Object.defineProperty(proto, k, { value: table[k], writable: true, enumerable: true, configurable: true });
        });
    };
    var getters = function (proto, table) {
        Object.keys(table).forEach(function (k) {
            Object.defineProperty(proto, k, { get: table[k], enumerable: true, configurable: true });
        });
    };
    var tag = function (C) { Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true }); };

    // ---- internal slots: one record per platform object, branded ---------------------------------------------
    var SL = new WeakMap();
    var rec = function (o, brand) {
        var r = (o !== null && (typeof o === 'object' || typeof o === 'function')) ? SL.get(o) : undefined;
        return (r !== undefined && r.brand === brand) ? r : undefined;
    };
    var need = function (o, brand, what) {
        var r = rec(o, brand);
        if (r === undefined) { throw new TypeError("Illegal invocation" + (what ? ": " + what : '')); }
        return r;
    };
    var make = function (proto, brand, r) {
        var o = Object.create(proto);
        r.brand = brand;
        r.obj = o;
        SL.set(o, r);
        return r;
    };

    // ---- promises (Streams §8.1 / WebIDL §3.6.8 "a new promise", "upon fulfillment", …) -----------------------
    var Prom = Promise, pThen = Promise.prototype.then;
    var defer = function () {
        var d = { state: 'pending' };
        d.promise = new Prom(function (res, rej) {
            d.resolve = function (v) { if (d.state === 'pending') { d.state = 'resolved'; res(v); } };
            d.reject = function (e) { if (d.state === 'pending') { d.state = 'rejected'; rej(e); } };
        });
        return d;
    };
    var resolvedWith = function (v) { return new Prom(function (res) { res(v); }); };
    var rejectedWith = function (e) { return new Prom(function (res, rej) { rej(e); }); };
    var handled = function (p) { pThen.call(p, undefined, function () {}); };
    var transformP = function (p, f, r) { return pThen.call(p, f, r); };
    var upon = function (p, f, r) { handled(pThen.call(p, f, r)); };
    var uponFulfill = function (p, f) { upon(p, f); };
    var uponReject = function (p, r) { upon(p, undefined, r); };
    var deferredRejected = function (e) { var d = defer(); d.reject(e); handled(d.promise); return d; };
    var deferredResolved = function (v) { var d = defer(); d.resolve(v); return d; };
    var promiseCall = function (f, thisArg, args) {
        try { return resolvedWith(f.apply(thisArg, args)); } catch (e) { return rejectedWith(e); }
    };
    var microtask = typeof queueMicrotask === 'function' ? queueMicrotask : function (f) { resolvedWith(undefined).then(f); };
    var iterResult = function (value, done) { return { value: value, done: done }; };

    // ---- WebIDL conversions used by the dictionaries -------------------------------------------------------
    var isObj = function (v) { return v !== null && (typeof v === 'object' || typeof v === 'function'); };
    var dictOf = function (v, what) {
        if (v === undefined || v === null) { return {}; }
        if (!isObj(v)) { throw new TypeError(what + ' is not an object.'); }
        return v;
    };
    var callback = function (v, what) {
        if (v === undefined) { return undefined; }
        if (typeof v !== 'function') { throw new TypeError(what + ' is not a function.'); }
        return v;
    };
    var unrestricted = function (v) {
        if (typeof v === 'symbol') { throw new TypeError('Cannot convert a Symbol value to a number'); }
        return +v;
    };
    // [EnforceRange] unsigned long long (WebIDL §3.2.4.9 / §3.2.4.10).
    var enforceULL = function (v, what) {
        var n = unrestricted(v);
        if (!isFinite(n)) { throw new TypeError(what + ' is not a finite number.'); }
        n = Math.trunc(n);
        if (n < 0 || n > Number.MAX_SAFE_INTEGER) { throw new TypeError(what + ' is outside the accepted range.'); }
        return n === 0 ? 0 : n;
    };
    var bool = function (v) { return !!v; };
    var isNonNegativeNumber = function (v) { return typeof v === 'number' && !isNaN(v) && v >= 0; };

    // ---- queuing strategies (Streams §7) -----------------------------------------------------------------------
    // QueuingStrategy {highWaterMark: unrestricted double, size: QueuingStrategySize}, converted in member order.
    var strategyDict = function (v, what) {
        var d = dictOf(v, what);
        var out = {};
        var h = d.highWaterMark;
        if (h !== undefined) { out.highWaterMark = unrestricted(h); }
        out.size = callback(d.size, "The 'size' member of " + what);
        return out;
    };
    var extractHWM = function (s, dflt) {
        if (s.highWaterMark === undefined) { return dflt; }
        var h = s.highWaterMark;
        if (isNaN(h) || h < 0) { throw new RangeError('Invalid highWaterMark'); }
        return h;
    };
    var extractSize = function (s) {
        if (s.size === undefined) { return function () { return 1; }; }
        var f = s.size;
        return function (chunk) { return unrestricted(f.call(undefined, chunk)); };
    };
    var countSize = ({ size() { return 1; } }).size;
    var byteSize = ({ size(chunk) { return chunk.byteLength; } }).size;
    var strategyInit = function (init, iface) {
        if (init === undefined || init === null) { init = {}; }
        if (!isObj(init)) { throw new TypeError("Failed to construct '" + iface + "': The provided value is not of type 'QueuingStrategyInit'."); }
        var h = init.highWaterMark;
        if (h === undefined) {
            throw new TypeError("Failed to construct '" + iface + "': Failed to read the 'highWaterMark' property from 'QueuingStrategyInit': Required member is undefined.");
        }
        return unrestricted(h);
    };
    class CountQueuingStrategy {
        constructor(init) { var r = { brand: 'CQS', hwm: strategyInit(init, 'CountQueuingStrategy') }; r.obj = this; SL.set(this, r); }
    }
    class ByteLengthQueuingStrategy {
        constructor(init) { var r = { brand: 'BQS', hwm: strategyInit(init, 'ByteLengthQueuingStrategy') }; r.obj = this; SL.set(this, r); }
    }
    getters(CountQueuingStrategy.prototype, {
        highWaterMark: function () { return need(this, 'CQS').hwm; },
        size: function () { need(this, 'CQS'); return countSize; },
    });
    getters(ByteLengthQueuingStrategy.prototype, {
        highWaterMark: function () { return need(this, 'BQS').hwm; },
        size: function () { need(this, 'BQS'); return byteSize; },
    });

    // ---- queue-with-sizes (Streams §8.1) ---------------------------------------------------------------------
    var dequeueValue = function (c) {
        var pair = c.queue.shift();
        c.queueTotalSize -= pair.size;
        if (c.queueTotalSize < 0) { c.queueTotalSize = 0; }
        return pair.value;
    };
    var enqueueValueWithSize = function (c, value, size) {
        if (!isNonNegativeNumber(size)) { throw new RangeError('The size of a chunk must be a non-negative number.'); }
        if (size === Infinity) { throw new RangeError('The size of a chunk must be finite.'); }
        c.queue.push({ value: value, size: size });
        c.queueTotalSize += size;
    };
    var peekQueueValue = function (c) { return c.queue[0].value; };
    var resetQueue = function (c) { c.queue = []; c.queueTotalSize = 0; };

    // ---- ArrayBuffer helpers (Streams §8.3) --------------------------------------------------------------------
    var abDetached = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'detached');
    var isDetached = function (b) {
        if (abDetached && abDetached.get) { return abDetached.get.call(b); }
        try { new Uint8Array(b); return false; } catch (e) { return true; }
    };
    var abTransfer = ArrayBuffer.prototype.transfer;
    var transferAB = function (b) {
        if (isDetached(b)) { throw new TypeError('The ArrayBuffer is detached.'); }
        return abTransfer.call(b);
    };
    var abSlice = ArrayBuffer.prototype.slice;
    var cloneAsU8 = function (view) { return new Uint8Array(abSlice.call(view.buffer, view.byteOffset, view.byteOffset + view.byteLength)); };
    var copyBytes = function (dest, destOff, src, srcOff, n) { new Uint8Array(dest, destOff, n).set(new Uint8Array(src, srcOff, n)); };
    var TA_TAG = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
    var TA_CTORS = {};
    ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array', 'Uint32Array', 'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array', 'Float16Array'].forEach(function (n) {
        if (typeof G[n] === 'function') { TA_CTORS[n] = G[n]; }
    });
    var typedName = function (v) { return isObj(v) ? TA_TAG.call(v) : undefined; };
    var isView = ArrayBuffer.isView;

    // =============================================================================================================
    // ReadableStream (Streams §4)
    // =============================================================================================================
    var RSProto, RSDRProto, RSBRProto, RSDCProto, RBSCProto, BYOBReqProto;

    var initRS = function (s) { s.state = 'readable'; s.reader = undefined; s.storedError = undefined; s.disturbed = false; s.controller = undefined; };
    var isLocked = function (s) { return s.reader !== undefined; };
    var newRS = function () { var s = make(RSProto, 'RS', {}); initRS(s); return s; };
    var createReadableStream = function (start, pull, cancel, hwm, size) {
        var s = newRS();
        var c = make(RSDCProto, 'RSDC', {});
        setUpRSDC(s, c, start, pull, cancel, hwm === undefined ? 1 : hwm, size === undefined ? function () { return 1; } : size);
        return s;
    };
    var createReadableByteStream = function (start, pull, cancel) {
        var s = newRS();
        var c = make(RBSCProto, 'RBSC', {});
        setUpRBSC(s, c, start, pull, cancel, 0, undefined);
        return s;
    };

    var rsCancel = function (s, reason) {
        s.disturbed = true;
        if (s.state === 'closed') { return resolvedWith(undefined); }
        if (s.state === 'errored') { return rejectedWith(s.storedError); }
        rsClose(s);
        var reader = s.reader;
        if (reader !== undefined && reader.brand === 'RSBR') {
            var reqs = reader.readIntoRequests;
            reader.readIntoRequests = [];
            reqs.forEach(function (r) { r.close(undefined); });
        }
        var sourceCancel = s.controller.cancelSteps(reason);
        return transformP(sourceCancel, function () { return undefined; });
    };
    var rsClose = function (s) {
        s.state = 'closed';
        var reader = s.reader;
        if (reader === undefined) { return; }
        reader.closed.resolve(undefined);
        if (reader.brand === 'RSDR') {
            var reqs = reader.readRequests;
            reader.readRequests = [];
            reqs.forEach(function (r) { r.close(); });
        }
    };
    var rsError = function (s, e) {
        s.state = 'errored';
        s.storedError = e;
        var reader = s.reader;
        if (reader === undefined) { return; }
        reader.closed.reject(e);
        handled(reader.closed.promise);
        if (reader.brand === 'RSDR') { defaultReaderErrorReadRequests(reader, e); }
        else { byobReaderErrorReadIntoRequests(reader, e); }
    };
    var numReadRequests = function (s) { return s.reader.readRequests.length; };
    var numReadIntoRequests = function (s) { return s.reader.readIntoRequests.length; };
    var hasDefaultReader = function (s) { return s.reader !== undefined && s.reader.brand === 'RSDR'; };
    var hasBYOBReader = function (s) { return s.reader !== undefined && s.reader.brand === 'RSBR'; };
    var fulfillReadRequest = function (s, chunk, done) {
        var r = s.reader.readRequests.shift();
        if (done) { r.close(); } else { r.chunk(chunk); }
    };
    var fulfillReadIntoRequest = function (s, chunk, done) {
        var r = s.reader.readIntoRequests.shift();
        if (done) { r.close(chunk); } else { r.chunk(chunk); }
    };

    // ---- readers (§4.3, §4.4, §4.5) ----
    var readerGenericInitialize = function (reader, s) {
        reader.stream = s;
        s.reader = reader;
        if (s.state === 'readable') { reader.closed = defer(); }
        else if (s.state === 'closed') { reader.closed = deferredResolved(undefined); }
        else { reader.closed = deferredRejected(s.storedError); }
    };
    var readerGenericCancel = function (reader, reason) { return rsCancel(reader.stream, reason); };
    var readerGenericRelease = function (reader) {
        var s = reader.stream;
        var e = new TypeError('The reader was released.');
        if (s.state === 'readable') { reader.closed.reject(e); }
        else { reader.closed = deferredRejected(e); }
        handled(reader.closed.promise);
        s.controller.releaseSteps();
        s.reader = undefined;
        reader.stream = undefined;
    };
    var defaultReaderErrorReadRequests = function (reader, e) {
        var reqs = reader.readRequests;
        reader.readRequests = [];
        reqs.forEach(function (r) { r.error(e); });
    };
    var byobReaderErrorReadIntoRequests = function (reader, e) {
        var reqs = reader.readIntoRequests;
        reader.readIntoRequests = [];
        reqs.forEach(function (r) { r.error(e); });
    };
    var defaultReaderRelease = function (reader) {
        readerGenericRelease(reader);
        defaultReaderErrorReadRequests(reader, new TypeError('The reader was released.'));
    };
    var byobReaderRelease = function (reader) {
        readerGenericRelease(reader);
        byobReaderErrorReadIntoRequests(reader, new TypeError('The reader was released.'));
    };
    var setUpDefaultReader = function (reader, s) {
        if (isLocked(s)) { throw new TypeError('The stream is already locked to a reader.'); }
        readerGenericInitialize(reader, s);
        reader.readRequests = [];
    };
    var setUpBYOBReader = function (reader, s) {
        if (isLocked(s)) { throw new TypeError('The stream is already locked to a reader.'); }
        if (s.controller.brand !== 'RBSC') { throw new TypeError('A BYOB reader needs a readable byte stream.'); }
        readerGenericInitialize(reader, s);
        reader.readIntoRequests = [];
    };
    var acquireDefaultReader = function (s) { var r = make(RSDRProto, 'RSDR', {}); setUpDefaultReader(r, s); return r; };
    var acquireBYOBReader = function (s) { var r = make(RSBRProto, 'RSBR', {}); setUpBYOBReader(r, s); return r; };
    var defaultReaderRead = function (reader, req) {
        var s = reader.stream;
        s.disturbed = true;
        if (s.state === 'closed') { req.close(); }
        else if (s.state === 'errored') { req.error(s.storedError); }
        else { s.controller.pullSteps(req); }
    };
    var byobReaderRead = function (reader, view, min, req) {
        var s = reader.stream;
        s.disturbed = true;
        if (s.state === 'errored') { req.error(s.storedError); }
        else { rbscPullInto(s.controller, view, min, req); }
    };

    // ---- ReadableStreamDefaultController (§4.6, §4.9.4) ----
    var rsdcCallPullIfNeeded = function (c) {
        if (!rsdcShouldCallPull(c)) { return; }
        if (c.pulling) { c.pullAgain = true; return; }
        c.pulling = true;
        upon(c.pullAlgorithm(), function () {
            c.pulling = false;
            if (c.pullAgain) { c.pullAgain = false; rsdcCallPullIfNeeded(c); }
        }, function (e) { rsdcError(c, e); });
    };
    var rsdcShouldCallPull = function (c) {
        var s = c.stream;
        if (!rsdcCanCloseOrEnqueue(c)) { return false; }
        if (!c.started) { return false; }
        if (isLocked(s) && numReadRequests(s) > 0) { return true; }
        return rsdcDesiredSize(c) > 0;
    };
    var rsdcClearAlgorithms = function (c) { c.pullAlgorithm = undefined; c.cancelAlgorithm = undefined; c.strategySizeAlgorithm = undefined; };
    var rsdcClose = function (c) {
        if (!rsdcCanCloseOrEnqueue(c)) { return; }
        c.closeRequested = true;
        if (c.queue.length === 0) { rsdcClearAlgorithms(c); rsClose(c.stream); }
    };
    var rsdcEnqueue = function (c, chunk) {
        if (!rsdcCanCloseOrEnqueue(c)) { return; }
        var s = c.stream;
        if (isLocked(s) && numReadRequests(s) > 0) {
            fulfillReadRequest(s, chunk, false);
        } else {
            var size;
            try { size = c.strategySizeAlgorithm(chunk); } catch (e) { rsdcError(c, e); throw e; }
            try { enqueueValueWithSize(c, chunk, size); } catch (e2) { rsdcError(c, e2); throw e2; }
        }
        rsdcCallPullIfNeeded(c);
    };
    var rsdcError = function (c, e) {
        var s = c.stream;
        if (s.state !== 'readable') { return; }
        resetQueue(c);
        rsdcClearAlgorithms(c);
        rsError(s, e);
    };
    var rsdcDesiredSize = function (c) {
        var st = c.stream.state;
        if (st === 'errored') { return null; }
        if (st === 'closed') { return 0; }
        return c.strategyHWM - c.queueTotalSize;
    };
    var rsdcHasBackpressure = function (c) { return !rsdcShouldCallPull(c); };
    var rsdcCanCloseOrEnqueue = function (c) { return !c.closeRequested && c.stream.state === 'readable'; };
    var setUpRSDC = function (s, c, start, pull, cancel, hwm, size) {
        c.stream = s;
        resetQueue(c);
        c.started = false; c.closeRequested = false; c.pullAgain = false; c.pulling = false;
        c.strategySizeAlgorithm = size;
        c.strategyHWM = hwm;
        c.pullAlgorithm = pull;
        c.cancelAlgorithm = cancel;
        c.cancelSteps = function (reason) {
            resetQueue(c);
            var r = c.cancelAlgorithm(reason);
            rsdcClearAlgorithms(c);
            return r;
        };
        c.pullSteps = function (req) {
            var st = c.stream;
            if (c.queue.length > 0) {
                var chunk = dequeueValue(c);
                if (c.closeRequested && c.queue.length === 0) { rsdcClearAlgorithms(c); rsClose(st); }
                else { rsdcCallPullIfNeeded(c); }
                req.chunk(chunk);
            } else {
                st.reader.readRequests.push(req);
                rsdcCallPullIfNeeded(c);
            }
        };
        c.releaseSteps = function () {};
        s.controller = c;
        var startResult = start();
        upon(resolvedWith(startResult), function () {
            c.started = true;
            rsdcCallPullIfNeeded(c);
        }, function (r) { rsdcError(c, r); });
    };

    // ---- ReadableByteStreamController (§4.7, §4.9.5) ----
    var rbscCallPullIfNeeded = function (c) {
        if (!rbscShouldCallPull(c)) { return; }
        if (c.pulling) { c.pullAgain = true; return; }
        c.pulling = true;
        upon(c.pullAlgorithm(), function () {
            c.pulling = false;
            if (c.pullAgain) { c.pullAgain = false; rbscCallPullIfNeeded(c); }
        }, function (e) { rbscError(c, e); });
    };
    var rbscShouldCallPull = function (c) {
        var s = c.stream;
        if (s.state !== 'readable' || c.closeRequested || !c.started) { return false; }
        if (hasDefaultReader(s) && numReadRequests(s) > 0) { return true; }
        if (hasBYOBReader(s) && numReadIntoRequests(s) > 0) { return true; }
        return rbscDesiredSize(c) > 0;
    };
    var rbscClearAlgorithms = function (c) { c.pullAlgorithm = undefined; c.cancelAlgorithm = undefined; };
    var rbscClearPendingPullIntos = function (c) { rbscInvalidateBYOBRequest(c); c.pendingPullIntos = []; };
    var rbscClose = function (c) {
        var s = c.stream;
        if (c.closeRequested || s.state !== 'readable') { return; }
        if (c.queueTotalSize > 0) { c.closeRequested = true; return; }
        if (c.pendingPullIntos.length > 0) {
            var first = c.pendingPullIntos[0];
            if (first.bytesFilled % first.elementSize !== 0) {
                var e = new TypeError('Insufficient bytes to fill elements in the given buffer');
                rbscError(c, e);
                throw e;
            }
        }
        rbscClearAlgorithms(c);
        rsClose(s);
    };
    var rbscCommitPullIntoDescriptor = function (s, pid) {
        var done = s.state === 'closed';
        var view = rbscConvertPullIntoDescriptor(pid);
        if (pid.readerType === 'default') { fulfillReadRequest(s, view, done); }
        else { fulfillReadIntoRequest(s, view, done); }
    };
    var rbscConvertPullIntoDescriptor = function (pid) {
        var buffer = transferAB(pid.buffer);
        return new pid.viewConstructor(buffer, pid.byteOffset, pid.bytesFilled / pid.elementSize);
    };
    var rbscEnqueue = function (c, chunk) {
        var s = c.stream;
        if (c.closeRequested || s.state !== 'readable') { return; }
        var buffer = chunk.buffer, byteOffset = chunk.byteOffset, byteLength = chunk.byteLength;
        if (isDetached(buffer)) { throw new TypeError("The chunk's buffer is detached."); }
        var transferred = transferAB(buffer);
        if (c.pendingPullIntos.length > 0) {
            var first = c.pendingPullIntos[0];
            if (isDetached(first.buffer)) { throw new TypeError("The BYOB request's buffer has been detached."); }
            rbscInvalidateBYOBRequest(c);
            first.buffer = transferAB(first.buffer);
            if (first.readerType === 'none') { rbscEnqueueDetachedPullIntoToQueue(c, first); }
        }
        if (hasDefaultReader(s)) {
            rbscProcessReadRequestsUsingQueue(c);
            if (numReadRequests(s) === 0) {
                rbscEnqueueChunkToQueue(c, transferred, byteOffset, byteLength);
            } else {
                if (c.pendingPullIntos.length > 0) { rbscShiftPendingPullInto(c); }
                fulfillReadRequest(s, new Uint8Array(transferred, byteOffset, byteLength), false);
            }
        } else if (hasBYOBReader(s)) {
            rbscEnqueueChunkToQueue(c, transferred, byteOffset, byteLength);
            rbscProcessPullIntoDescriptorsUsingQueue(c).forEach(function (pid) { rbscCommitPullIntoDescriptor(s, pid); });
        } else {
            rbscEnqueueChunkToQueue(c, transferred, byteOffset, byteLength);
        }
        rbscCallPullIfNeeded(c);
    };
    var rbscEnqueueChunkToQueue = function (c, buffer, byteOffset, byteLength) {
        c.queue.push({ buffer: buffer, byteOffset: byteOffset, byteLength: byteLength });
        c.queueTotalSize += byteLength;
    };
    var rbscEnqueueClonedChunkToQueue = function (c, buffer, byteOffset, byteLength) {
        var clone;
        try { clone = abSlice.call(buffer, byteOffset, byteOffset + byteLength); }
        catch (e) { rbscError(c, e); throw e; }
        rbscEnqueueChunkToQueue(c, clone, 0, byteLength);
    };
    var rbscEnqueueDetachedPullIntoToQueue = function (c, pid) {
        if (pid.bytesFilled > 0) { rbscEnqueueClonedChunkToQueue(c, pid.buffer, pid.byteOffset, pid.bytesFilled); }
        rbscShiftPendingPullInto(c);
    };
    var rbscError = function (c, e) {
        var s = c.stream;
        if (s.state !== 'readable') { return; }
        rbscClearPendingPullIntos(c);
        resetQueue(c);
        rbscClearAlgorithms(c);
        rsError(s, e);
    };
    var rbscFillPullIntoDescriptorFromQueue = function (c, pid) {
        var maxBytesToCopy = Math.min(c.queueTotalSize, pid.byteLength - pid.bytesFilled);
        var maxBytesFilled = pid.bytesFilled + maxBytesToCopy;
        var remaining = maxBytesToCopy;
        var ready = false;
        var maxAligned = maxBytesFilled - (maxBytesFilled % pid.elementSize);
        if (maxAligned >= pid.minimumFill) {
            remaining = maxAligned - pid.bytesFilled;
            ready = true;
        }
        while (remaining > 0) {
            var head = c.queue[0];
            var n = Math.min(remaining, head.byteLength);
            copyBytes(pid.buffer, pid.byteOffset + pid.bytesFilled, head.buffer, head.byteOffset, n);
            if (head.byteLength === n) { c.queue.shift(); }
            else { head.byteOffset += n; head.byteLength -= n; }
            c.queueTotalSize -= n;
            pid.bytesFilled += n;
            remaining -= n;
        }
        return ready;
    };
    var rbscGetBYOBRequest = function (c) {
        if (c.byobRequest === null && c.pendingPullIntos.length > 0) {
            var first = c.pendingPullIntos[0];
            var view = new Uint8Array(first.buffer, first.byteOffset + first.bytesFilled, first.byteLength - first.bytesFilled);
            var req = make(BYOBReqProto, 'BYOBReq', { controller: c, view: view });
            c.byobRequest = req;
        }
        return c.byobRequest === null ? null : c.byobRequest.obj;
    };
    var rbscDesiredSize = function (c) {
        var st = c.stream.state;
        if (st === 'errored') { return null; }
        if (st === 'closed') { return 0; }
        return c.strategyHWM - c.queueTotalSize;
    };
    var rbscHandleQueueDrain = function (c) {
        if (c.queueTotalSize === 0 && c.closeRequested) { rbscClearAlgorithms(c); rsClose(c.stream); }
        else { rbscCallPullIfNeeded(c); }
    };
    var rbscInvalidateBYOBRequest = function (c) {
        if (c.byobRequest === null) { return; }
        c.byobRequest.controller = undefined;
        c.byobRequest.view = null;
        c.byobRequest = null;
    };
    var rbscProcessPullIntoDescriptorsUsingQueue = function (c) {
        var filled = [];
        while (c.pendingPullIntos.length > 0) {
            if (c.queueTotalSize === 0) { break; }
            var pid = c.pendingPullIntos[0];
            if (rbscFillPullIntoDescriptorFromQueue(c, pid)) {
                rbscShiftPendingPullInto(c);
                filled.push(pid);
            } else { break; }
        }
        return filled;
    };
    var rbscProcessReadRequestsUsingQueue = function (c) {
        var reader = c.stream.reader;
        while (reader.readRequests.length > 0) {
            if (c.queueTotalSize === 0) { return; }
            rbscFillReadRequestFromQueue(c, reader.readRequests.shift());
        }
    };
    var rbscFillReadRequestFromQueue = function (c, req) {
        var entry = c.queue.shift();
        c.queueTotalSize -= entry.byteLength;
        rbscHandleQueueDrain(c);
        req.chunk(new Uint8Array(entry.buffer, entry.byteOffset, entry.byteLength));
    };
    var rbscPullInto = function (c, view, min, req) {
        var s = c.stream;
        var elementSize = 1, ctor = DataView;
        var tn = typedName(view);
        if (tn !== undefined) { ctor = TA_CTORS[tn]; elementSize = ctor.BYTES_PER_ELEMENT; }
        var minimumFill = min * elementSize;
        var byteOffset = view.byteOffset, byteLength = view.byteLength;
        var buffer;
        try { buffer = transferAB(view.buffer); } catch (e) { req.error(e); return; }
        var pid = { buffer: buffer, bufferByteLength: buffer.byteLength, byteOffset: byteOffset, byteLength: byteLength, bytesFilled: 0,
                    minimumFill: minimumFill, elementSize: elementSize, viewConstructor: ctor, readerType: 'byob' };
        if (c.pendingPullIntos.length > 0) {
            c.pendingPullIntos.push(pid);
            s.reader.readIntoRequests.push(req);
            return;
        }
        if (s.state === 'closed') {
            req.close(new ctor(pid.buffer, pid.byteOffset, 0));
            return;
        }
        if (c.queueTotalSize > 0) {
            if (rbscFillPullIntoDescriptorFromQueue(c, pid)) {
                var filled = rbscConvertPullIntoDescriptor(pid);
                rbscHandleQueueDrain(c);
                req.chunk(filled);
                return;
            }
            if (c.closeRequested) {
                var e = new TypeError('Insufficient bytes to fill elements in the given buffer');
                rbscError(c, e);
                req.error(e);
                return;
            }
        }
        c.pendingPullIntos.push(pid);
        s.reader.readIntoRequests.push(req);
        rbscCallPullIfNeeded(c);
    };
    var rbscRespond = function (c, bytesWritten) {
        var first = c.pendingPullIntos[0];
        if (c.stream.state === 'closed') {
            if (bytesWritten !== 0) { throw new TypeError('bytesWritten must be 0 when calling respond() on a closed stream'); }
        } else {
            if (bytesWritten === 0) { throw new TypeError('bytesWritten must be greater than 0 when calling respond() on a readable stream'); }
            if (first.bytesFilled + bytesWritten > first.byteLength) { throw new RangeError('bytesWritten out of range'); }
        }
        first.buffer = transferAB(first.buffer);
        rbscRespondInternal(c, bytesWritten);
    };
    var rbscRespondInClosedState = function (c, first) {
        if (first.readerType === 'none') { rbscShiftPendingPullInto(c); }
        var s = c.stream;
        if (hasBYOBReader(s)) {
            var filled = [];
            while (filled.length < numReadIntoRequests(s)) { filled.push(rbscShiftPendingPullInto(c)); }
            filled.forEach(function (pid) { rbscCommitPullIntoDescriptor(s, pid); });
        }
    };
    var rbscRespondInReadableState = function (c, bytesWritten, pid) {
        pid.bytesFilled += bytesWritten;
        var s = c.stream;
        if (pid.readerType === 'none') {
            rbscEnqueueDetachedPullIntoToQueue(c, pid);
            rbscProcessPullIntoDescriptorsUsingQueue(c).forEach(function (p) { rbscCommitPullIntoDescriptor(s, p); });
            return;
        }
        if (pid.bytesFilled < pid.minimumFill) { return; }
        rbscShiftPendingPullInto(c);
        var remainder = pid.bytesFilled % pid.elementSize;
        if (remainder > 0) {
            var end = pid.byteOffset + pid.bytesFilled;
            rbscEnqueueClonedChunkToQueue(c, pid.buffer, end - remainder, remainder);
        }
        pid.bytesFilled -= remainder;
        var filled = rbscProcessPullIntoDescriptorsUsingQueue(c);
        rbscCommitPullIntoDescriptor(s, pid);
        filled.forEach(function (p) { rbscCommitPullIntoDescriptor(s, p); });
    };
    var rbscRespondInternal = function (c, bytesWritten) {
        var first = c.pendingPullIntos[0];
        rbscInvalidateBYOBRequest(c);
        if (c.stream.state === 'closed') { rbscRespondInClosedState(c, first); }
        else { rbscRespondInReadableState(c, bytesWritten, first); }
        rbscCallPullIfNeeded(c);
    };
    var rbscRespondWithNewView = function (c, view) {
        var first = c.pendingPullIntos[0];
        if (c.stream.state === 'closed') {
            if (view.byteLength !== 0) { throw new TypeError("The view's length must be 0 when calling respondWithNewView() on a closed stream"); }
        } else if (view.byteLength === 0) {
            throw new TypeError("The view's length must be greater than 0 when calling respondWithNewView() on a readable stream");
        }
        if (first.byteOffset + first.bytesFilled !== view.byteOffset) { throw new RangeError('The region specified by view does not match byobRequest'); }
        if (first.bufferByteLength !== view.buffer.byteLength) { throw new RangeError('The buffer of view has different capacity than byobRequest'); }
        if (first.bytesFilled + view.byteLength > first.byteLength) { throw new RangeError('The region specified by view is larger than byobRequest'); }
        var n = view.byteLength;
        first.buffer = transferAB(view.buffer);
        rbscRespondInternal(c, n);
    };
    var rbscShiftPendingPullInto = function (c) { return c.pendingPullIntos.shift(); };
    var setUpRBSC = function (s, c, start, pull, cancel, hwm, autoAllocate) {
        c.stream = s;
        c.pullAgain = false; c.pulling = false;
        c.byobRequest = null;
        resetQueue(c);
        c.closeRequested = false; c.started = false;
        c.strategyHWM = hwm;
        c.pullAlgorithm = pull;
        c.cancelAlgorithm = cancel;
        c.autoAllocateChunkSize = autoAllocate;
        c.pendingPullIntos = [];
        c.cancelSteps = function (reason) {
            rbscClearPendingPullIntos(c);
            resetQueue(c);
            var r = c.cancelAlgorithm(reason);
            rbscClearAlgorithms(c);
            return r;
        };
        c.pullSteps = function (req) {
            var st = c.stream;
            if (c.queueTotalSize > 0) { rbscFillReadRequestFromQueue(c, req); return; }
            var size = c.autoAllocateChunkSize;
            if (size !== undefined) {
                var buffer;
                try { buffer = new ArrayBuffer(size); } catch (e) { req.error(e); return; }
                c.pendingPullIntos.push({ buffer: buffer, bufferByteLength: size, byteOffset: 0, byteLength: size, bytesFilled: 0,
                                          minimumFill: 1, elementSize: 1, viewConstructor: Uint8Array, readerType: 'default' });
            }
            st.reader.readRequests.push(req);
            rbscCallPullIfNeeded(c);
        };
        c.releaseSteps = function () {
            if (c.pendingPullIntos.length > 0) {
                var first = c.pendingPullIntos[0];
                first.readerType = 'none';
                c.pendingPullIntos = [first];
            }
        };
        s.controller = c;
        var startResult = start();
        upon(resolvedWith(startResult), function () {
            c.started = true;
            rbscCallPullIfNeeded(c);
        }, function (r) { rbscError(c, r); });
    };

    // ---- tee (§4.9.1) ----
    var defaultTee = function (s) {
        var reader = acquireDefaultReader(s);
        var reading = false, readAgain = false, canceled1 = false, canceled2 = false, reason1, reason2, branch1, branch2;
        var cancelP = defer();
        var pull = function () {
            if (reading) { readAgain = true; return resolvedWith(undefined); }
            reading = true;
            defaultReaderRead(reader, {
                chunk: function (chunk) {
                    microtask(function () {
                        readAgain = false;
                        if (!canceled1) { rsdcEnqueue(branch1.controller, chunk); }
                        if (!canceled2) { rsdcEnqueue(branch2.controller, chunk); }
                        reading = false;
                        if (readAgain) { pull(); }
                    });
                },
                close: function () {
                    reading = false;
                    if (!canceled1) { rsdcClose(branch1.controller); }
                    if (!canceled2) { rsdcClose(branch2.controller); }
                    if (!canceled1 || !canceled2) { cancelP.resolve(undefined); }
                },
                error: function () { reading = false; },
            });
            return resolvedWith(undefined);
        };
        var cancel1 = function (reason) {
            canceled1 = true; reason1 = reason;
            if (canceled2) { cancelP.resolve(rsCancel(s, [reason1, reason2])); }
            return cancelP.promise;
        };
        var cancel2 = function (reason) {
            canceled2 = true; reason2 = reason;
            if (canceled1) { cancelP.resolve(rsCancel(s, [reason1, reason2])); }
            return cancelP.promise;
        };
        var start = function () {};
        branch1 = createReadableStream(start, pull, cancel1);
        branch2 = createReadableStream(start, pull, cancel2);
        uponReject(reader.closed.promise, function (r) {
            rsdcError(branch1.controller, r);
            rsdcError(branch2.controller, r);
            if (!canceled1 || !canceled2) { cancelP.resolve(undefined); }
        });
        return [branch1.obj, branch2.obj];
    };
    var byteTee = function (s) {
        var reader = acquireDefaultReader(s);
        var reading = false, again1 = false, again2 = false, canceled1 = false, canceled2 = false, reason1, reason2, branch1, branch2;
        var cancelP = defer();
        var forwardReaderError = function (thisReader) {
            uponReject(thisReader.closed.promise, function (r) {
                if (thisReader !== reader) { return; }
                rbscError(branch1.controller, r);
                rbscError(branch2.controller, r);
                if (!canceled1 || !canceled2) { cancelP.resolve(undefined); }
            });
        };
        var cloneFail = function (e) {
            rbscError(branch1.controller, e);
            rbscError(branch2.controller, e);
            cancelP.resolve(rsCancel(s, e));
        };
        var pullWithDefaultReader = function () {
            if (reader.brand === 'RSBR') {
                byobReaderRelease(reader);
                reader = acquireDefaultReader(s);
                forwardReaderError(reader);
            }
            defaultReaderRead(reader, {
                chunk: function (chunk) {
                    microtask(function () {
                        again1 = false; again2 = false;
                        var chunk1 = chunk, chunk2 = chunk;
                        if (!canceled1 && !canceled2) {
                            try { chunk2 = cloneAsU8(chunk); } catch (e) { cloneFail(e); return; }
                        }
                        if (!canceled1) { rbscEnqueue(branch1.controller, chunk1); }
                        if (!canceled2) { rbscEnqueue(branch2.controller, chunk2); }
                        reading = false;
                        if (again1) { pull1(); } else if (again2) { pull2(); }
                    });
                },
                close: function () {
                    reading = false;
                    if (!canceled1) { rbscClose(branch1.controller); }
                    if (!canceled2) { rbscClose(branch2.controller); }
                    if (branch1.controller.pendingPullIntos.length > 0) { rbscRespond(branch1.controller, 0); }
                    if (branch2.controller.pendingPullIntos.length > 0) { rbscRespond(branch2.controller, 0); }
                    if (!canceled1 || !canceled2) { cancelP.resolve(undefined); }
                },
                error: function () { reading = false; },
            });
        };
        var pullWithBYOBReader = function (view, forBranch2) {
            if (reader.brand === 'RSDR') {
                defaultReaderRelease(reader);
                reader = acquireBYOBReader(s);
                forwardReaderError(reader);
            }
            var byobBranch = forBranch2 ? branch2 : branch1, otherBranch = forBranch2 ? branch1 : branch2;
            byobReaderRead(reader, view, 1, {
                chunk: function (chunk) {
                    microtask(function () {
                        again1 = false; again2 = false;
                        var byobCanceled = forBranch2 ? canceled2 : canceled1, otherCanceled = forBranch2 ? canceled1 : canceled2;
                        if (!otherCanceled) {
                            var cloned;
                            try { cloned = cloneAsU8(chunk); } catch (e) { cloneFail(e); return; }
                            if (!byobCanceled) { rbscRespondWithNewView(byobBranch.controller, chunk); }
                            rbscEnqueue(otherBranch.controller, cloned);
                        } else if (!byobCanceled) {
                            rbscRespondWithNewView(byobBranch.controller, chunk);
                        }
                        reading = false;
                        if (again1) { pull1(); } else if (again2) { pull2(); }
                    });
                },
                close: function (chunk) {
                    reading = false;
                    var byobCanceled = forBranch2 ? canceled2 : canceled1, otherCanceled = forBranch2 ? canceled1 : canceled2;
                    if (!byobCanceled) { rbscClose(byobBranch.controller); }
                    if (!otherCanceled) { rbscClose(otherBranch.controller); }
                    if (chunk !== undefined) {
                        if (!byobCanceled) { rbscRespondWithNewView(byobBranch.controller, chunk); }
                        if (!otherCanceled && otherBranch.controller.pendingPullIntos.length > 0) { rbscRespond(otherBranch.controller, 0); }
                    }
                    if (!byobCanceled || !otherCanceled) { cancelP.resolve(undefined); }
                },
                error: function () { reading = false; },
            });
        };
        var pull1 = function () {
            if (reading) { again1 = true; return resolvedWith(undefined); }
            reading = true;
            var req = rbscGetBYOBRequest(branch1.controller);
            if (req === null) { pullWithDefaultReader(); } else { pullWithBYOBReader(rec(req, 'BYOBReq').view, false); }
            return resolvedWith(undefined);
        };
        var pull2 = function () {
            if (reading) { again2 = true; return resolvedWith(undefined); }
            reading = true;
            var req = rbscGetBYOBRequest(branch2.controller);
            if (req === null) { pullWithDefaultReader(); } else { pullWithBYOBReader(rec(req, 'BYOBReq').view, true); }
            return resolvedWith(undefined);
        };
        var cancel1 = function (reason) {
            canceled1 = true; reason1 = reason;
            if (canceled2) { cancelP.resolve(rsCancel(s, [reason1, reason2])); }
            return cancelP.promise;
        };
        var cancel2 = function (reason) {
            canceled2 = true; reason2 = reason;
            if (canceled1) { cancelP.resolve(rsCancel(s, [reason1, reason2])); }
            return cancelP.promise;
        };
        var start = function () {};
        branch1 = createReadableByteStream(start, pull1, cancel1);
        branch2 = createReadableByteStream(start, pull2, cancel2);
        forwardReaderError(reader);
        return [branch1.obj, branch2.obj];
    };

    // ---- pipeTo (§4.9.1 ReadableStreamPipeTo) ----
    var rsPipeTo = function (source, dest, preventClose, preventAbort, preventCancel, signal) {
        var reader = acquireDefaultReader(source);
        var writer = acquireWriter(dest);
        source.disturbed = true;
        var shuttingDown = false;
        var currentWrite = resolvedWith(undefined);
        var result = defer();
        var abortAlgorithm;
        var finalize = function (isError, error) {
            writerRelease(writer);
            defaultReaderRelease(reader);
            if (signal !== undefined) { signal.removeEventListener('abort', abortAlgorithm); }
            if (isError) { result.reject(error); } else { result.resolve(undefined); }
            return null;
        };
        var waitForWritesToFinish = function () {
            var old = currentWrite;
            return transformP(currentWrite, function () { return old !== currentWrite ? waitForWritesToFinish() : undefined; });
        };
        var shutdownWithAnAction = function (action, originalIsError, originalError) {
            if (shuttingDown) { return; }
            shuttingDown = true;
            var doTheRest = function () {
                upon(action(), function () { return finalize(originalIsError, originalError); }, function (newError) { return finalize(true, newError); });
                return null;
            };
            if (dest.state === 'writable' && !wsCloseQueuedOrInFlight(dest)) { uponFulfill(waitForWritesToFinish(), doTheRest); }
            else { doTheRest(); }
        };
        var shutdown = function (isError, error) {
            if (shuttingDown) { return; }
            shuttingDown = true;
            if (dest.state === 'writable' && !wsCloseQueuedOrInFlight(dest)) {
                uponFulfill(waitForWritesToFinish(), function () { return finalize(isError, error); });
            } else { finalize(isError, error); }
        };
        if (signal !== undefined) {
            abortAlgorithm = function () {
                var error = signal.reason;
                var actions = [];
                if (!preventAbort) {
                    actions.push(function () { return dest.state === 'writable' ? wsAbort(dest, error) : resolvedWith(undefined); });
                }
                if (!preventCancel) {
                    actions.push(function () { return source.state === 'readable' ? rsCancel(source, error) : resolvedWith(undefined); });
                }
                shutdownWithAnAction(function () { return Prom.all(actions.map(function (a) { return a(); })); }, true, error);
            };
            if (signal.aborted) { abortAlgorithm(); return result.promise; }
            signal.addEventListener('abort', abortAlgorithm);
        }
        var pipeStep = function () {
            if (shuttingDown) { return resolvedWith(true); }
            return transformP(writer.ready.promise, function () {
                return new Prom(function (resolveRead, rejectRead) {
                    defaultReaderRead(reader, {
                        chunk: function (chunk) {
                            currentWrite = transformP(writerWrite(writer, chunk), undefined, function () {});
                            resolveRead(false);
                        },
                        close: function () { resolveRead(true); },
                        error: rejectRead,
                    });
                });
            });
        };
        var pipeLoop = function () {
            return new Prom(function (resolveLoop, rejectLoop) {
                var next = function (done) {
                    if (done) { resolveLoop(); } else { pThen.call(pipeStep(), next, rejectLoop); }
                };
                next(false);
            });
        };
        // errors forward
        if (source.state === 'errored') { errForward(source.storedError); } else { uponReject(reader.closed.promise, errForward); }
        function errForward(e) {
            if (!preventAbort) { shutdownWithAnAction(function () { return wsAbort(dest, e); }, true, e); }
            else { shutdown(true, e); }
            return null;
        }
        // errors backward
        if (dest.state === 'errored') { errBackward(dest.storedError); } else { uponReject(writer.closed.promise, errBackward); }
        function errBackward(e) {
            if (!preventCancel) { shutdownWithAnAction(function () { return rsCancel(source, e); }, true, e); }
            else { shutdown(true, e); }
            return null;
        }
        // closing forward
        if (source.state === 'closed') { closeForward(); } else { uponFulfill(reader.closed.promise, closeForward); }
        function closeForward() {
            if (!preventClose) { shutdownWithAnAction(function () { return writerCloseWithErrorPropagation(writer); }); }
            else { shutdown(); }
            return null;
        }
        // closing backward
        if (wsCloseQueuedOrInFlight(dest) || dest.state === 'closed') {
            var destClosed = new TypeError('the destination writable stream closed before all data could be piped to it');
            if (!preventCancel) { shutdownWithAnAction(function () { return rsCancel(source, destClosed); }, true, destClosed); }
            else { shutdown(true, destClosed); }
        }
        handled(pipeLoop());
        return result.promise;
    };

    // ---- ReadableStream.from (§4.9.1 ReadableStreamFromIterable) ----
    var getAsyncIterator = function (obj) {
        var m = obj[Symbol.asyncIterator];
        if (m === undefined || m === null) {
            var sm = obj[Symbol.iterator];
            if (sm === undefined || sm === null) { throw new TypeError('The value is not async iterable.'); }
            var syncIt = sm.call(obj);
            if (!isObj(syncIt)) { throw new TypeError('The iterator is not an object.'); }
            var syncNext = syncIt.next;
            var wrapResult = function (r) {
                if (!isObj(r)) { throw new TypeError('The iterator result is not an object.'); }
                var done = !!r.done;
                return transformP(resolvedWith(r.value), function (v) { return { value: v, done: done }; });
            };
            return {
                iterator: {
                    next: function () { return wrapResult(syncNext.call(syncIt)); },
                    return: function (v) {
                        var ret = syncIt.return;
                        if (ret === undefined || ret === null) { return resolvedWith({ value: v, done: true }); }
                        return wrapResult(ret.call(syncIt, v));
                    },
                },
                next: undefined,
            };
        }
        var it = m.call(obj);
        if (!isObj(it)) { throw new TypeError('The async iterator is not an object.'); }
        return { iterator: it, next: it.next };
    };
    var rsFromIterable = function (iterable) {
        var record = getAsyncIterator(iterable);
        var stream;
        var pull = function () {
            var nextResult;
            try {
                nextResult = record.next === undefined ? record.iterator.next() : record.next.call(record.iterator);
            } catch (e) { return rejectedWith(e); }
            return transformP(resolvedWith(nextResult), function (r) {
                if (!isObj(r)) { throw new TypeError('The promise returned by the iterator.next() method must fulfill with an object'); }
                if (r.done) { rsdcClose(stream.controller); }
                else { rsdcEnqueue(stream.controller, r.value); }
            });
        };
        var cancel = function (reason) {
            var it = record.iterator, ret;
            try {
                ret = it.return;
                if (ret === undefined || ret === null) { return resolvedWith(undefined); }
                ret = ret.call(it, reason);
            } catch (e) { return rejectedWith(e); }
            return transformP(resolvedWith(ret), function (r) {
                if (!isObj(r)) { throw new TypeError('The promise returned by the iterator.return() method must fulfill with an object'); }
                return undefined;
            });
        };
        stream = createReadableStream(function () {}, pull, cancel, 0);
        return stream.obj;
    };

    // ---- the async iterator (§4.2.5, WebIDL §3.7.10.2 default asynchronous iterator) ----
    var AsyncIteratorPrototype = (function () {
        try { return Object.getPrototypeOf(Object.getPrototypeOf((async function* () {}).prototype)); } catch (e) { return Object.prototype; }
    })();
    var RSAsyncIterProto = Object.create(AsyncIteratorPrototype);
    var makeAsyncIterator = function (s, preventCancel) {
        var reader = acquireDefaultReader(s);
        return make(RSAsyncIterProto, 'RSIter', { reader: reader, preventCancel: preventCancel, ongoing: undefined, finished: false }).obj;
    };
    var iterNextSteps = function (it) {
        if (it.finished) { return resolvedWith(iterResult(undefined, true)); }
        var reader = it.reader;
        var d = defer();
        defaultReaderRead(reader, {
            chunk: function (chunk) { d.resolve(chunk); },
            close: function () { defaultReaderRelease(reader); d.resolve(END); },
            error: function (e) { defaultReaderRelease(reader); d.reject(e); },
        });
        return transformP(d.promise, function (v) {
            it.ongoing = undefined;
            if (v === END) { it.finished = true; return iterResult(undefined, true); }
            return iterResult(v, false);
        }, function (e) {
            it.ongoing = undefined;
            it.finished = true;
            throw e;
        });
    };
    var END = {};
    methods(RSAsyncIterProto, {
        next: function () {
            var it = rec(this, 'RSIter');
            if (it === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            var steps = function () { return iterNextSteps(it); };
            it.ongoing = it.ongoing ? transformP(it.ongoing, steps, steps) : steps();
            return it.ongoing;
        },
        return: function (value) {
            var it = rec(this, 'RSIter');
            if (it === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            var steps = function () {
                if (it.finished) { return resolvedWith(iterResult(value, true)); }
                it.finished = true;
                var reader = it.reader;
                var r;
                if (!it.preventCancel) {
                    r = readerGenericCancel(reader, value);
                    defaultReaderRelease(reader);
                } else {
                    defaultReaderRelease(reader);
                    r = resolvedWith(undefined);
                }
                return transformP(r, function () { return iterResult(value, true); });
            };
            var p = it.ongoing ? transformP(it.ongoing, steps, steps) : steps();
            return p;
        },
    });
    Object.defineProperty(RSAsyncIterProto, Symbol.toStringTag, { value: 'ReadableStream AsyncIterator', configurable: true });

    // ---- the ReadableStream interface ----
    var sourceDict = function (v) {
        var d = dictOf(v, 'The underlying source');
        var out = {};
        if (d.autoAllocateChunkSize !== undefined) { out.autoAllocateChunkSize = enforceULL(d.autoAllocateChunkSize, "The 'autoAllocateChunkSize' member"); }
        out.cancel = callback(d.cancel, "The 'cancel' member of the underlying source");
        out.pull = callback(d.pull, "The 'pull' member of the underlying source");
        out.start = callback(d.start, "The 'start' member of the underlying source");
        if (d.type !== undefined) {
            var t = String(d.type);
            if (t !== 'bytes') { throw new TypeError("The provided value '" + t + "' is not a valid enum value of type ReadableStreamType."); }
            out.type = t;
        }
        return out;
    };
    var pipeOptions = function (v) {
        var d = dictOf(v, 'The pipe options');
        var out = { preventAbort: bool(d.preventAbort), preventCancel: bool(d.preventCancel), preventClose: bool(d.preventClose), signal: undefined };
        if (d.signal !== undefined) {
            if (typeof AbortSignal !== 'function' || !(d.signal instanceof AbortSignal)) { throw new TypeError("The 'signal' member is not an AbortSignal."); }
            out.signal = d.signal;
        }
        return out;
    };
    class ReadableStream {
        constructor() {
            var underlyingSource = arguments[0], strategyArg = arguments[1];
            if (underlyingSource !== undefined && !isObj(underlyingSource)) {
                throw new TypeError("Failed to construct 'ReadableStream': The provided value is not of type 'object'.");
            }
            var strategy = strategyDict(strategyArg, 'The queuing strategy');
            var src = underlyingSource === undefined ? null : underlyingSource;
            var dict = sourceDict(src);
            var s = { brand: 'RS', obj: this };
            SL.set(this, s);
            initRS(s);
            if (dict.type === 'bytes') {
                if (strategy.size !== undefined) { throw new RangeError('The strategy for a byte stream cannot have a size function'); }
                var hwm = extractHWM(strategy, 0);
                if (dict.autoAllocateChunkSize === 0) { throw new TypeError('autoAllocateChunkSize must be greater than 0'); }
                var bc = make(RBSCProto, 'RBSC', {});
                setUpRBSC(s, bc, function () { return dict.start ? dict.start.call(src, bc.obj) : undefined; },
                    function () { return dict.pull ? promiseCall(dict.pull, src, [bc.obj]) : resolvedWith(undefined); },
                    function (r) { return dict.cancel ? promiseCall(dict.cancel, src, [r]) : resolvedWith(undefined); },
                    hwm, dict.autoAllocateChunkSize);
            } else {
                var size = extractSize(strategy);
                var hwm2 = extractHWM(strategy, 1);
                var dc = make(RSDCProto, 'RSDC', {});
                setUpRSDC(s, dc, function () { return dict.start ? dict.start.call(src, dc.obj) : undefined; },
                    function () { return dict.pull ? promiseCall(dict.pull, src, [dc.obj]) : resolvedWith(undefined); },
                    function (r) { return dict.cancel ? promiseCall(dict.cancel, src, [r]) : resolvedWith(undefined); },
                    hwm2, size);
            }
        }
        static from(asyncIterable) { return rsFromIterable(asyncIterable); }
    }
    RSProto = ReadableStream.prototype;
    getters(RSProto, { locked: function () { return isLocked(need(this, 'RS')); } });
    var rsValues = function () {
        var s = need(this, 'RS');
        var opts = dictOf(arguments[0], 'The iterator options');
        return makeAsyncIterator(s, bool(opts.preventCancel));
    };
    methods(RSProto, {
        cancel: function () {
            var s = rec(this, 'RS');
            if (s === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (isLocked(s)) { return rejectedWith(new TypeError('Cannot cancel a stream that already has a reader')); }
            return rsCancel(s, arguments[0]);
        },
        getReader: function () {
            var s = need(this, 'RS');
            var o = dictOf(arguments[0], 'The reader options');
            if (o.mode === undefined) { return acquireDefaultReader(s).obj; }
            var m = String(o.mode);
            if (m !== 'byob') { throw new TypeError("The provided value '" + m + "' is not a valid enum value of type ReadableStreamReaderMode."); }
            return acquireBYOBReader(s).obj;
        },
        pipeThrough: function (transform) {
            var s = need(this, 'RS');
            var t = dictOf(transform, 'The transform pair');
            if (t.readable === undefined) { throw new TypeError("Failed to read the 'readable' property from 'ReadableWritablePair': Required member is undefined."); }
            var readable = rec(t.readable, 'RS');
            if (readable === undefined) { throw new TypeError("The 'readable' member is not a ReadableStream."); }
            if (t.writable === undefined) { throw new TypeError("Failed to read the 'writable' property from 'ReadableWritablePair': Required member is undefined."); }
            var writable = rec(t.writable, 'WS');
            if (writable === undefined) { throw new TypeError("The 'writable' member is not a WritableStream."); }
            var o = pipeOptions(arguments[1]);
            if (isLocked(s)) { throw new TypeError('Cannot pipe a locked stream'); }
            if (wsLocked(writable)) { throw new TypeError('Cannot pipe to a locked stream'); }
            var p = rsPipeTo(s, writable, o.preventClose, o.preventAbort, o.preventCancel, o.signal);
            handled(p);
            return readable.obj;
        },
        pipeTo: function (destination) {
            var s = rec(this, 'RS');
            if (s === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            var dest = rec(destination, 'WS');
            if (dest === undefined) { return rejectedWith(new TypeError("Failed to execute 'pipeTo' on 'ReadableStream': parameter 1 is not of type 'WritableStream'.")); }
            var o;
            try { o = pipeOptions(arguments[1]); } catch (e) { return rejectedWith(e); }
            if (isLocked(s)) { return rejectedWith(new TypeError('Cannot pipe a locked stream')); }
            if (wsLocked(dest)) { return rejectedWith(new TypeError('Cannot pipe to a locked stream')); }
            return rsPipeTo(s, dest, o.preventClose, o.preventAbort, o.preventCancel, o.signal);
        },
        tee: function () {
            var s = need(this, 'RS');
            return s.controller.brand === 'RBSC' ? byteTee(s) : defaultTee(s);
        },
        values: rsValues,
    });
    Object.defineProperty(RSProto, Symbol.asyncIterator, { value: rsValues, writable: true, enumerable: false, configurable: true });

    // ---- the reader interfaces ----
    class ReadableStreamDefaultReader {
        constructor(stream) {
            var s = rec(stream, 'RS');
            if (s === undefined) { throw new TypeError("Failed to construct 'ReadableStreamDefaultReader': parameter 1 is not of type 'ReadableStream'."); }
            var r = { brand: 'RSDR', obj: this };
            setUpDefaultReader(r, s);
            SL.set(this, r);
        }
    }
    RSDRProto = ReadableStreamDefaultReader.prototype;
    getters(RSDRProto, {
        closed: function () {
            var r = rec(this, 'RSDR');
            return r === undefined ? rejectedWith(new TypeError('Illegal invocation')) : r.closed.promise;
        },
    });
    methods(RSDRProto, {
        cancel: function () {
            var r = rec(this, 'RSDR');
            if (r === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (r.stream === undefined) { return rejectedWith(new TypeError('This readable stream reader has been released and cannot be used to cancel its previous owner stream')); }
            return readerGenericCancel(r, arguments[0]);
        },
        read: function () {
            var r = rec(this, 'RSDR');
            if (r === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (r.stream === undefined) { return rejectedWith(new TypeError('This readable stream reader has been released and cannot be used to read from its previous owner stream')); }
            var d = defer();
            defaultReaderRead(r, {
                chunk: function (chunk) { d.resolve(iterResult(chunk, false)); },
                close: function () { d.resolve(iterResult(undefined, true)); },
                error: function (e) { d.reject(e); },
            });
            return d.promise;
        },
        releaseLock: function () {
            var r = need(this, 'RSDR');
            if (r.stream === undefined) { return; }
            defaultReaderRelease(r);
        },
    });
    class ReadableStreamBYOBReader {
        constructor(stream) {
            var s = rec(stream, 'RS');
            if (s === undefined) { throw new TypeError("Failed to construct 'ReadableStreamBYOBReader': parameter 1 is not of type 'ReadableStream'."); }
            var r = { brand: 'RSBR', obj: this };
            setUpBYOBReader(r, s);
            SL.set(this, r);
        }
    }
    RSBRProto = ReadableStreamBYOBReader.prototype;
    getters(RSBRProto, {
        closed: function () {
            var r = rec(this, 'RSBR');
            return r === undefined ? rejectedWith(new TypeError('Illegal invocation')) : r.closed.promise;
        },
    });
    methods(RSBRProto, {
        cancel: function () {
            var r = rec(this, 'RSBR');
            if (r === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (r.stream === undefined) { return rejectedWith(new TypeError('This readable stream reader has been released and cannot be used to cancel its previous owner stream')); }
            return readerGenericCancel(r, arguments[0]);
        },
        read: function (view) {
            var r = rec(this, 'RSBR');
            if (r === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (!isView(view)) { return rejectedWith(new TypeError("Failed to execute 'read' on 'ReadableStreamBYOBReader': parameter 1 is not of type 'ArrayBufferView'.")); }
            var min;
            try {
                var o = dictOf(arguments[1], 'The read options');
                min = o.min === undefined ? 1 : enforceULL(o.min, "The 'min' member");
            } catch (e) { return rejectedWith(e); }
            if (view.byteLength === 0) { return rejectedWith(new TypeError('view must have non-zero byteLength')); }
            if (view.buffer.byteLength === 0) { return rejectedWith(new TypeError("view's buffer must have non-zero byteLength")); }
            if (isDetached(view.buffer)) { return rejectedWith(new TypeError("view's buffer has been detached")); }
            if (min === 0) { return rejectedWith(new TypeError('options.min must be greater than 0')); }
            if (typedName(view) !== undefined) {
                if (min > view.length) { return rejectedWith(new RangeError('options.min must be less than or equal to view\'s length')); }
            } else if (min > view.byteLength) { return rejectedWith(new RangeError('options.min must be less than or equal to view\'s byteLength')); }
            if (r.stream === undefined) { return rejectedWith(new TypeError('This readable stream reader has been released and cannot be used to read from its previous owner stream')); }
            var d = defer();
            byobReaderRead(r, view, min, {
                chunk: function (chunk) { d.resolve(iterResult(chunk, false)); },
                close: function (chunk) { d.resolve(iterResult(chunk, true)); },
                error: function (e) { d.reject(e); },
            });
            return d.promise;
        },
        releaseLock: function () {
            var r = need(this, 'RSBR');
            if (r.stream === undefined) { return; }
            byobReaderRelease(r);
        },
    });

    // ---- the controller interfaces ----
    var illegal = function () { throw new TypeError('Illegal constructor'); };
    class ReadableStreamDefaultController { constructor() { illegal(); } }
    RSDCProto = ReadableStreamDefaultController.prototype;
    getters(RSDCProto, { desiredSize: function () { return rsdcDesiredSize(need(this, 'RSDC')); } });
    methods(RSDCProto, {
        close: function () {
            var c = need(this, 'RSDC');
            if (!rsdcCanCloseOrEnqueue(c)) { throw new TypeError('The stream is not in a state that permits close'); }
            rsdcClose(c);
        },
        enqueue: function () {
            var c = need(this, 'RSDC');
            if (!rsdcCanCloseOrEnqueue(c)) { throw new TypeError('The stream is not in a state that permits enqueue'); }
            rsdcEnqueue(c, arguments[0]);
        },
        error: function () { rsdcError(need(this, 'RSDC'), arguments[0]); },
    });
    class ReadableByteStreamController { constructor() { illegal(); } }
    RBSCProto = ReadableByteStreamController.prototype;
    getters(RBSCProto, {
        byobRequest: function () { return rbscGetBYOBRequest(need(this, 'RBSC')); },
        desiredSize: function () { return rbscDesiredSize(need(this, 'RBSC')); },
    });
    methods(RBSCProto, {
        close: function () {
            var c = need(this, 'RBSC');
            if (c.closeRequested) { throw new TypeError('The stream has already been closed; do not close it again!'); }
            if (c.stream.state !== 'readable') { throw new TypeError('The stream is not in the readable state and cannot be closed'); }
            rbscClose(c);
        },
        enqueue: function (chunk) {
            var c = need(this, 'RBSC');
            if (!isView(chunk)) { throw new TypeError("Failed to execute 'enqueue' on 'ReadableByteStreamController': parameter 1 is not of type 'ArrayBufferView'."); }
            if (chunk.byteLength === 0) { throw new TypeError('chunk must have non-zero byteLength'); }
            if (chunk.buffer.byteLength === 0) { throw new TypeError("chunk's buffer must have non-zero byteLength"); }
            if (c.closeRequested) { throw new TypeError('stream is closed or draining'); }
            if (c.stream.state !== 'readable') { throw new TypeError('The stream is not in the readable state and cannot be enqueued to'); }
            rbscEnqueue(c, chunk);
        },
        error: function () { rbscError(need(this, 'RBSC'), arguments[0]); },
    });
    class ReadableStreamBYOBRequest { constructor() { illegal(); } }
    BYOBReqProto = ReadableStreamBYOBRequest.prototype;
    getters(BYOBReqProto, { view: function () { return need(this, 'BYOBReq').view; } });
    methods(BYOBReqProto, {
        respond: function (bytesWritten) {
            var r = need(this, 'BYOBReq');
            var n = enforceULL(bytesWritten, 'bytesWritten');
            if (r.controller === undefined) { throw new TypeError('This BYOB request has been invalidated'); }
            if (isDetached(r.view.buffer)) { throw new TypeError('The BYOB request\'s buffer has been detached and so cannot be used as a response'); }
            rbscRespond(r.controller, n);
        },
        respondWithNewView: function (view) {
            var r = need(this, 'BYOBReq');
            if (!isView(view)) { throw new TypeError("Failed to execute 'respondWithNewView' on 'ReadableStreamBYOBRequest': parameter 1 is not of type 'ArrayBufferView'."); }
            if (r.controller === undefined) { throw new TypeError('This BYOB request has been invalidated'); }
            if (isDetached(view.buffer)) { throw new TypeError('The given view\'s buffer has been detached and so cannot be used as a response'); }
            rbscRespondWithNewView(r.controller, view);
        },
    });

    // =============================================================================================================
    // WritableStream (Streams §5)
    // =============================================================================================================
    var WSProto, WSDWProto, WSDCProto;
    var CLOSE_SENTINEL = {};
    var initWS = function (s) {
        s.state = 'writable'; s.storedError = undefined; s.writer = undefined; s.controller = undefined;
        s.inFlightWriteRequest = undefined; s.closeRequest = undefined; s.inFlightCloseRequest = undefined;
        s.pendingAbortRequest = undefined; s.writeRequests = []; s.backpressure = false;
    };
    var wsLocked = function (s) { return s.writer !== undefined; };
    var createWritableStream = function (start, write, close, abort, hwm, size) {
        var s = make(WSProto, 'WS', {});
        initWS(s);
        var c = make(WSDCProto, 'WSDC', {});
        setUpWSDC(s, c, start, write, close, abort, hwm, size);
        return s;
    };
    var wsAbort = function (s, reason) {
        if (s.state === 'closed' || s.state === 'errored') { return resolvedWith(undefined); }
        s.controller.abortController.abort(reason);
        var state = s.state;
        if (state === 'closed' || state === 'errored') { return resolvedWith(undefined); }
        if (s.pendingAbortRequest !== undefined) { return s.pendingAbortRequest.promise.promise; }
        var wasAlreadyErroring = false;
        if (state === 'erroring') { wasAlreadyErroring = true; reason = undefined; }
        var d = defer();
        s.pendingAbortRequest = { promise: d, reason: reason, wasAlreadyErroring: wasAlreadyErroring };
        if (!wasAlreadyErroring) { wsStartErroring(s, reason); }
        return d.promise;
    };
    var wsClose = function (s) {
        var state = s.state;
        if (state === 'closed' || state === 'errored') {
            return rejectedWith(new TypeError('The stream (in ' + state + ' state) is not in the writable state and cannot be closed'));
        }
        var d = defer();
        s.closeRequest = d;
        var writer = s.writer;
        if (writer !== undefined && s.backpressure && state === 'writable') { writer.ready.resolve(undefined); }
        wsdcClose(s.controller);
        return d.promise;
    };
    var wsAddWriteRequest = function (s) { var d = defer(); s.writeRequests.push(d); return d.promise; };
    var wsDealWithRejection = function (s, error) {
        if (s.state === 'writable') { wsStartErroring(s, error); return; }
        wsFinishErroring(s);
    };
    var wsFinishErroring = function (s) {
        s.state = 'errored';
        s.controller.errorSteps();
        var storedError = s.storedError;
        s.writeRequests.forEach(function (r) { r.reject(storedError); });
        s.writeRequests = [];
        if (s.pendingAbortRequest === undefined) { wsRejectCloseAndClosedPromiseIfNeeded(s); return; }
        var abortRequest = s.pendingAbortRequest;
        s.pendingAbortRequest = undefined;
        if (abortRequest.wasAlreadyErroring) {
            abortRequest.promise.reject(storedError);
            wsRejectCloseAndClosedPromiseIfNeeded(s);
            return;
        }
        var p = s.controller.abortSteps(abortRequest.reason);
        upon(p, function () {
            abortRequest.promise.resolve(undefined);
            wsRejectCloseAndClosedPromiseIfNeeded(s);
        }, function (reason) {
            abortRequest.promise.reject(reason);
            wsRejectCloseAndClosedPromiseIfNeeded(s);
        });
    };
    var wsFinishInFlightClose = function (s) {
        s.inFlightCloseRequest.resolve(undefined);
        s.inFlightCloseRequest = undefined;
        if (s.state === 'erroring') {
            s.storedError = undefined;
            if (s.pendingAbortRequest !== undefined) {
                s.pendingAbortRequest.promise.resolve(undefined);
                s.pendingAbortRequest = undefined;
            }
        }
        s.state = 'closed';
        if (s.writer !== undefined) { s.writer.closed.resolve(undefined); }
    };
    var wsFinishInFlightCloseWithError = function (s, error) {
        s.inFlightCloseRequest.reject(error);
        s.inFlightCloseRequest = undefined;
        if (s.pendingAbortRequest !== undefined) {
            s.pendingAbortRequest.promise.reject(error);
            s.pendingAbortRequest = undefined;
        }
        wsDealWithRejection(s, error);
    };
    var wsFinishInFlightWrite = function (s) { s.inFlightWriteRequest.resolve(undefined); s.inFlightWriteRequest = undefined; };
    var wsFinishInFlightWriteWithError = function (s, error) {
        s.inFlightWriteRequest.reject(error);
        s.inFlightWriteRequest = undefined;
        wsDealWithRejection(s, error);
    };
    var wsHasOperationMarkedInFlight = function (s) { return s.inFlightWriteRequest !== undefined || s.inFlightCloseRequest !== undefined; };
    var wsCloseQueuedOrInFlight = function (s) { return s.closeRequest !== undefined || s.inFlightCloseRequest !== undefined; };
    var wsRejectCloseAndClosedPromiseIfNeeded = function (s) {
        if (s.closeRequest !== undefined) {
            s.closeRequest.reject(s.storedError);
            s.closeRequest = undefined;
        }
        var writer = s.writer;
        if (writer !== undefined) {
            writer.closed.reject(s.storedError);
            handled(writer.closed.promise);
        }
    };
    var wsStartErroring = function (s, reason) {
        var c = s.controller;
        s.state = 'erroring';
        s.storedError = reason;
        if (s.writer !== undefined) { writerEnsureReadyPromiseRejected(s.writer, reason); }
        if (!wsHasOperationMarkedInFlight(s) && c.started) { wsFinishErroring(s); }
    };
    var wsUpdateBackpressure = function (s, backpressure) {
        var writer = s.writer;
        if (writer !== undefined && backpressure !== s.backpressure) {
            if (backpressure) { writer.ready = defer(); }
            else { writer.ready.resolve(undefined); }
        }
        s.backpressure = backpressure;
    };

    // ---- the writer (§5.3, §5.5.4) ----
    var setUpWriter = function (w, s) {
        if (wsLocked(s)) { throw new TypeError('This stream has already been locked for exclusive writing by another writer'); }
        w.stream = s;
        s.writer = w;
        var state = s.state;
        if (state === 'writable') {
            w.ready = (!wsCloseQueuedOrInFlight(s) && s.backpressure) ? defer() : deferredResolved(undefined);
            w.closed = defer();
        } else if (state === 'erroring') {
            w.ready = deferredRejected(s.storedError);
            w.closed = defer();
        } else if (state === 'closed') {
            w.ready = deferredResolved(undefined);
            w.closed = deferredResolved(undefined);
        } else {
            w.ready = deferredRejected(s.storedError);
            w.closed = deferredRejected(s.storedError);
        }
    };
    var acquireWriter = function (s) { var w = make(WSDWProto, 'WSDW', {}); setUpWriter(w, s); return w; };
    var writerCloseWithErrorPropagation = function (w) {
        var s = w.stream;
        if (wsCloseQueuedOrInFlight(s) || s.state === 'closed') { return resolvedWith(undefined); }
        if (s.state === 'errored') { return rejectedWith(s.storedError); }
        return wsClose(s);
    };
    var writerEnsureClosedPromiseRejected = function (w, error) {
        if (w.closed.state === 'pending') { w.closed.reject(error); } else { w.closed = deferredRejected(error); }
        handled(w.closed.promise);
    };
    var writerEnsureReadyPromiseRejected = function (w, error) {
        if (w.ready.state === 'pending') { w.ready.reject(error); } else { w.ready = deferredRejected(error); }
        handled(w.ready.promise);
    };
    var writerDesiredSize = function (w) {
        var s = w.stream, state = s.state;
        if (state === 'errored' || state === 'erroring') { return null; }
        if (state === 'closed') { return 0; }
        return wsdcDesiredSize(s.controller);
    };
    var writerRelease = function (w) {
        var s = w.stream;
        var e = new TypeError('Writer was released and can no longer be used to monitor the stream\'s closedness');
        writerEnsureReadyPromiseRejected(w, e);
        writerEnsureClosedPromiseRejected(w, e);
        s.writer = undefined;
        w.stream = undefined;
    };
    var writerWrite = function (w, chunk) {
        var s = w.stream;
        var c = s.controller;
        var chunkSize = wsdcGetChunkSize(c, chunk);
        if (s !== w.stream) { return rejectedWith(new TypeError('The writer was released.')); }
        var state = s.state;
        if (state === 'errored') { return rejectedWith(s.storedError); }
        if (wsCloseQueuedOrInFlight(s) || state === 'closed') { return rejectedWith(new TypeError('The stream is closing or closed and cannot be written to')); }
        if (state === 'erroring') { return rejectedWith(s.storedError); }
        var p = wsAddWriteRequest(s);
        wsdcWrite(c, chunk, chunkSize);
        return p;
    };

    // ---- WritableStreamDefaultController (§5.4, §5.5.5) ----
    var setUpWSDC = function (s, c, start, write, close, abort, hwm, size) {
        c.stream = s;
        s.controller = c;
        resetQueue(c);
        c.abortController = new AbortController();
        c.started = false;
        c.strategySizeAlgorithm = size;
        c.strategyHWM = hwm;
        c.writeAlgorithm = write;
        c.closeAlgorithm = close;
        c.abortAlgorithm = abort;
        c.abortSteps = function (reason) {
            var r = c.abortAlgorithm(reason);
            wsdcClearAlgorithms(c);
            return r;
        };
        c.errorSteps = function () { resetQueue(c); };
        wsUpdateBackpressure(s, wsdcGetBackpressure(c));
        var startResult = start();
        upon(resolvedWith(startResult), function () {
            c.started = true;
            wsdcAdvanceQueueIfNeeded(c);
        }, function (r) {
            c.started = true;
            wsDealWithRejection(s, r);
        });
    };
    var wsdcAdvanceQueueIfNeeded = function (c) {
        var s = c.stream;
        if (!c.started) { return; }
        if (s.inFlightWriteRequest !== undefined) { return; }
        if (s.state === 'erroring') { wsFinishErroring(s); return; }
        if (c.queue.length === 0) { return; }
        var value = peekQueueValue(c);
        if (value === CLOSE_SENTINEL) { wsdcProcessClose(c); } else { wsdcProcessWrite(c, value); }
    };
    var wsdcClearAlgorithms = function (c) {
        c.writeAlgorithm = undefined; c.closeAlgorithm = undefined; c.abortAlgorithm = undefined; c.strategySizeAlgorithm = undefined;
    };
    var wsdcClose = function (c) {
        enqueueValueWithSize(c, CLOSE_SENTINEL, 0);
        wsdcAdvanceQueueIfNeeded(c);
    };
    var wsdcError = function (c, e) {
        wsdcClearAlgorithms(c);
        wsStartErroring(c.stream, e);
    };
    var wsdcErrorIfNeeded = function (c, e) { if (c.stream.state === 'writable') { wsdcError(c, e); } };
    var wsdcGetBackpressure = function (c) { return wsdcDesiredSize(c) <= 0; };
    var wsdcGetChunkSize = function (c, chunk) {
        if (c.strategySizeAlgorithm === undefined) { return 1; }
        try { return c.strategySizeAlgorithm(chunk); } catch (e) { wsdcErrorIfNeeded(c, e); return 1; }
    };
    var wsdcDesiredSize = function (c) { return c.strategyHWM - c.queueTotalSize; };
    var wsdcProcessClose = function (c) {
        var s = c.stream;
        s.inFlightCloseRequest = s.closeRequest;
        s.closeRequest = undefined;
        dequeueValue(c);
        var p = c.closeAlgorithm();
        wsdcClearAlgorithms(c);
        upon(p, function () { wsFinishInFlightClose(s); }, function (r) { wsFinishInFlightCloseWithError(s, r); });
    };
    var wsdcProcessWrite = function (c, chunk) {
        var s = c.stream;
        s.inFlightWriteRequest = s.writeRequests.shift();
        upon(c.writeAlgorithm(chunk), function () {
            wsFinishInFlightWrite(s);
            var state = s.state;
            dequeueValue(c);
            if (!wsCloseQueuedOrInFlight(s) && state === 'writable') { wsUpdateBackpressure(s, wsdcGetBackpressure(c)); }
            wsdcAdvanceQueueIfNeeded(c);
        }, function (reason) {
            if (s.state === 'writable') { wsdcClearAlgorithms(c); }
            wsFinishInFlightWriteWithError(s, reason);
        });
    };
    var wsdcWrite = function (c, chunk, chunkSize) {
        try { enqueueValueWithSize(c, chunk, chunkSize); } catch (e) { wsdcErrorIfNeeded(c, e); return; }
        var s = c.stream;
        if (!wsCloseQueuedOrInFlight(s) && s.state === 'writable') { wsUpdateBackpressure(s, wsdcGetBackpressure(c)); }
        wsdcAdvanceQueueIfNeeded(c);
    };

    // ---- the WritableStream interfaces ----
    var sinkDict = function (v) {
        var d = dictOf(v, 'The underlying sink');
        return {
            abort: callback(d.abort, "The 'abort' member of the underlying sink"),
            close: callback(d.close, "The 'close' member of the underlying sink"),
            start: callback(d.start, "The 'start' member of the underlying sink"),
            type: d.type,
            write: callback(d.write, "The 'write' member of the underlying sink"),
        };
    };
    class WritableStream {
        constructor() {
            var underlyingSink = arguments[0], strategyArg = arguments[1];
            if (underlyingSink !== undefined && !isObj(underlyingSink)) {
                throw new TypeError("Failed to construct 'WritableStream': The provided value is not of type 'object'.");
            }
            var strategy = strategyDict(strategyArg, 'The queuing strategy');
            var sink = underlyingSink === undefined ? null : underlyingSink;
            var dict = sinkDict(sink);
            if (dict.type !== undefined) { throw new RangeError('Invalid type is specified'); }
            var s = { brand: 'WS', obj: this };
            SL.set(this, s);
            initWS(s);
            var size = extractSize(strategy);
            var hwm = extractHWM(strategy, 1);
            var c = make(WSDCProto, 'WSDC', {});
            setUpWSDC(s, c, function () { return dict.start ? dict.start.call(sink, c.obj) : undefined; },
                function (chunk) { return dict.write ? promiseCall(dict.write, sink, [chunk, c.obj]) : resolvedWith(undefined); },
                function () { return dict.close ? promiseCall(dict.close, sink, []) : resolvedWith(undefined); },
                function (r) { return dict.abort ? promiseCall(dict.abort, sink, [r]) : resolvedWith(undefined); },
                hwm, size);
        }
    }
    WSProto = WritableStream.prototype;
    getters(WSProto, { locked: function () { return wsLocked(need(this, 'WS')); } });
    methods(WSProto, {
        abort: function () {
            var s = rec(this, 'WS');
            if (s === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (wsLocked(s)) { return rejectedWith(new TypeError('Cannot abort a stream that already has a writer')); }
            return wsAbort(s, arguments[0]);
        },
        close: function () {
            var s = rec(this, 'WS');
            if (s === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (wsLocked(s)) { return rejectedWith(new TypeError('Cannot close a stream that already has a writer')); }
            if (wsCloseQueuedOrInFlight(s)) { return rejectedWith(new TypeError('Cannot close an already-closing stream')); }
            return wsClose(s);
        },
        getWriter: function () { return acquireWriter(need(this, 'WS')).obj; },
    });
    class WritableStreamDefaultWriter {
        constructor(stream) {
            var s = rec(stream, 'WS');
            if (s === undefined) { throw new TypeError("Failed to construct 'WritableStreamDefaultWriter': parameter 1 is not of type 'WritableStream'."); }
            var w = { brand: 'WSDW', obj: this };
            setUpWriter(w, s);
            SL.set(this, w);
        }
    }
    WSDWProto = WritableStreamDefaultWriter.prototype;
    getters(WSDWProto, {
        closed: function () {
            var w = rec(this, 'WSDW');
            return w === undefined ? rejectedWith(new TypeError('Illegal invocation')) : w.closed.promise;
        },
        desiredSize: function () {
            var w = need(this, 'WSDW');
            if (w.stream === undefined) { throw new TypeError('The writer was released.'); }
            return writerDesiredSize(w);
        },
        ready: function () {
            var w = rec(this, 'WSDW');
            return w === undefined ? rejectedWith(new TypeError('Illegal invocation')) : w.ready.promise;
        },
    });
    methods(WSDWProto, {
        abort: function () {
            var w = rec(this, 'WSDW');
            if (w === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (w.stream === undefined) { return rejectedWith(new TypeError('The writer was released.')); }
            return wsAbort(w.stream, arguments[0]);
        },
        close: function () {
            var w = rec(this, 'WSDW');
            if (w === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            var s = w.stream;
            if (s === undefined) { return rejectedWith(new TypeError('The writer was released.')); }
            if (wsCloseQueuedOrInFlight(s)) { return rejectedWith(new TypeError('Cannot close an already-closing stream')); }
            return wsClose(s);
        },
        releaseLock: function () {
            var w = need(this, 'WSDW');
            if (w.stream === undefined) { return; }
            writerRelease(w);
        },
        write: function () {
            var w = rec(this, 'WSDW');
            if (w === undefined) { return rejectedWith(new TypeError('Illegal invocation')); }
            if (w.stream === undefined) { return rejectedWith(new TypeError('The writer was released.')); }
            return writerWrite(w, arguments[0]);
        },
    });
    class WritableStreamDefaultController { constructor() { illegal(); } }
    WSDCProto = WritableStreamDefaultController.prototype;
    getters(WSDCProto, { signal: function () { return need(this, 'WSDC').abortController.signal; } });
    methods(WSDCProto, {
        error: function () {
            var c = need(this, 'WSDC');
            if (c.stream.state !== 'writable') { return; }
            wsdcError(c, arguments[0]);
        },
    });

    // =============================================================================================================
    // TransformStream (Streams §6)
    // =============================================================================================================
    var TSProto, TSDCProto;
    var initTS = function (t, startP, wHWM, wSize, rHWM, rSize) {
        var start = function () { return startP.promise; };
        t.writable = createWritableStream(start, function (chunk) { return tsSinkWrite(t, chunk); },
            function () { return tsSinkClose(t); }, function (r) { return tsSinkAbort(t, r); }, wHWM, wSize);
        t.readable = createReadableStream(start, function () { return tsSourcePull(t); }, function (r) { return tsSourceCancel(t, r); }, rHWM, rSize);
        t.backpressure = undefined;
        t.backpressureChangePromise = undefined;
        tsSetBackpressure(t, true);
        t.controller = undefined;
    };
    var tsError = function (t, e) {
        rsdcError(t.readable.controller, e);
        tsErrorWritableAndUnblockWrite(t, e);
    };
    var tsErrorWritableAndUnblockWrite = function (t, e) {
        tsdcClearAlgorithms(t.controller);
        wsdcErrorIfNeeded(t.writable.controller, e);
        tsUnblockWrite(t);
    };
    var tsUnblockWrite = function (t) { if (t.backpressure) { tsSetBackpressure(t, false); } };
    var tsSetBackpressure = function (t, bp) {
        if (t.backpressureChangePromise !== undefined) { t.backpressureChangePromise.resolve(undefined); }
        t.backpressureChangePromise = defer();
        t.backpressure = bp;
    };
    var setUpTSDC = function (t, c, transform, flush, cancel) {
        c.stream = t;
        t.controller = c;
        c.transformAlgorithm = transform;
        c.flushAlgorithm = flush;
        c.cancelAlgorithm = cancel;
        c.finishPromise = undefined;
    };
    var tsdcClearAlgorithms = function (c) { c.transformAlgorithm = undefined; c.flushAlgorithm = undefined; c.cancelAlgorithm = undefined; };
    var tsdcEnqueue = function (c, chunk) {
        var t = c.stream;
        var rc = t.readable.controller;
        if (!rsdcCanCloseOrEnqueue(rc)) { throw new TypeError('Readable side is not in a state that permits enqueue'); }
        try { rsdcEnqueue(rc, chunk); }
        catch (e) { tsErrorWritableAndUnblockWrite(t, e); throw t.readable.storedError; }
        var bp = rsdcHasBackpressure(rc);
        if (bp !== t.backpressure) { tsSetBackpressure(t, true); }
    };
    var tsdcPerformTransform = function (c, chunk) {
        return transformP(c.transformAlgorithm(chunk), undefined, function (r) { tsError(c.stream, r); throw r; });
    };
    var tsdcTerminate = function (c) {
        var t = c.stream;
        rsdcClose(t.readable.controller);
        tsErrorWritableAndUnblockWrite(t, new TypeError('TransformStream terminated'));
    };
    var tsSinkWrite = function (t, chunk) {
        var c = t.controller;
        if (t.backpressure) {
            return transformP(t.backpressureChangePromise.promise, function () {
                var w = t.writable;
                if (w.state === 'erroring') { throw w.storedError; }
                return tsdcPerformTransform(c, chunk);
            });
        }
        return tsdcPerformTransform(c, chunk);
    };
    var tsSinkAbort = function (t, reason) {
        var c = t.controller;
        if (c.finishPromise !== undefined) { return c.finishPromise.promise; }
        var readable = t.readable;
        c.finishPromise = defer();
        var p = c.cancelAlgorithm(reason);
        tsdcClearAlgorithms(c);
        upon(p, function () {
            if (readable.state === 'errored') { c.finishPromise.reject(readable.storedError); }
            else { rsdcError(readable.controller, reason); c.finishPromise.resolve(undefined); }
        }, function (r) {
            rsdcError(readable.controller, r);
            c.finishPromise.reject(r);
        });
        return c.finishPromise.promise;
    };
    var tsSinkClose = function (t) {
        var c = t.controller;
        if (c.finishPromise !== undefined) { return c.finishPromise.promise; }
        var readable = t.readable;
        c.finishPromise = defer();
        var p = c.flushAlgorithm();
        tsdcClearAlgorithms(c);
        upon(p, function () {
            if (readable.state === 'errored') { c.finishPromise.reject(readable.storedError); }
            else { rsdcClose(readable.controller); c.finishPromise.resolve(undefined); }
        }, function (r) {
            rsdcError(readable.controller, r);
            c.finishPromise.reject(r);
        });
        return c.finishPromise.promise;
    };
    var tsSourcePull = function (t) {
        tsSetBackpressure(t, false);
        return t.backpressureChangePromise.promise;
    };
    var tsSourceCancel = function (t, reason) {
        var c = t.controller;
        if (c.finishPromise !== undefined) { return c.finishPromise.promise; }
        var writable = t.writable;
        c.finishPromise = defer();
        var p = c.cancelAlgorithm(reason);
        tsdcClearAlgorithms(c);
        upon(p, function () {
            if (writable.state === 'errored') { c.finishPromise.reject(writable.storedError); }
            else {
                wsdcErrorIfNeeded(writable.controller, reason);
                tsUnblockWrite(t);
                c.finishPromise.resolve(undefined);
            }
        }, function (r) {
            wsdcErrorIfNeeded(writable.controller, r);
            tsUnblockWrite(t);
            c.finishPromise.reject(r);
        });
        return c.finishPromise.promise;
    };
    var transformerDict = function (v) {
        var d = dictOf(v, 'The transformer');
        return {
            cancel: callback(d.cancel, "The 'cancel' member of the transformer"),
            flush: callback(d.flush, "The 'flush' member of the transformer"),
            readableType: d.readableType,
            start: callback(d.start, "The 'start' member of the transformer"),
            transform: callback(d.transform, "The 'transform' member of the transformer"),
            writableType: d.writableType,
        };
    };
    // A TransformStream built from Rust-free algorithms (TextEncoderStream, TextDecoderStream, DecompressionStream).
    var createTransformStream = function (transformFn, flushFn) {
        var t = make(TSProto, 'TS', {});
        var startP = defer();
        initTS(t, startP, 1, function () { return 1; }, 0, function () { return 1; });
        var c = make(TSDCProto, 'TSDC', {});
        setUpTSDC(t, c, function (chunk) { return promiseCall(transformFn, undefined, [chunk, c]); },
            function () { return flushFn ? promiseCall(flushFn, undefined, [c]) : resolvedWith(undefined); },
            function () { return resolvedWith(undefined); });
        startP.resolve(undefined);
        return t;
    };
    class TransformStream {
        constructor() {
            var transformer = arguments[0], wsArg = arguments[1], rsArg = arguments[2];
            if (transformer !== undefined && !isObj(transformer)) {
                throw new TypeError("Failed to construct 'TransformStream': The provided value is not of type 'object'.");
            }
            var writableStrategy = strategyDict(wsArg, 'The writable strategy');
            var readableStrategy = strategyDict(rsArg, 'The readable strategy');
            var tr = transformer === undefined ? null : transformer;
            var dict = transformerDict(tr);
            if (dict.readableType !== undefined) { throw new RangeError('Invalid readableType specified'); }
            if (dict.writableType !== undefined) { throw new RangeError('Invalid writableType specified'); }
            var rHWM = extractHWM(readableStrategy, 0), rSize = extractSize(readableStrategy);
            var wHWM = extractHWM(writableStrategy, 1), wSize = extractSize(writableStrategy);
            var t = { brand: 'TS', obj: this };
            SL.set(this, t);
            var startP = defer();
            initTS(t, startP, wHWM, wSize, rHWM, rSize);
            var c = make(TSDCProto, 'TSDC', {});
            var transformAlg = dict.transform
                ? function (chunk) { return promiseCall(dict.transform, tr, [chunk, c.obj]); }
                : function (chunk) { try { tsdcEnqueue(c, chunk); return resolvedWith(undefined); } catch (e) { return rejectedWith(e); } };
            var flushAlg = dict.flush ? function () { return promiseCall(dict.flush, tr, [c.obj]); } : function () { return resolvedWith(undefined); };
            var cancelAlg = dict.cancel ? function (r) { return promiseCall(dict.cancel, tr, [r]); } : function () { return resolvedWith(undefined); };
            setUpTSDC(t, c, transformAlg, flushAlg, cancelAlg);
            if (dict.start) { startP.resolve(dict.start.call(tr, c.obj)); }
            else { startP.resolve(undefined); }
        }
    }
    TSProto = TransformStream.prototype;
    getters(TSProto, {
        readable: function () { return need(this, 'TS').readable.obj; },
        writable: function () { return need(this, 'TS').writable.obj; },
    });
    class TransformStreamDefaultController { constructor() { illegal(); } }
    TSDCProto = TransformStreamDefaultController.prototype;
    getters(TSDCProto, { desiredSize: function () { return rsdcDesiredSize(need(this, 'TSDC').stream.readable.controller); } });
    methods(TSDCProto, {
        enqueue: function () { tsdcEnqueue(need(this, 'TSDC'), arguments[0]); },
        error: function () { tsError(need(this, 'TSDC').stream, arguments[0]); },
        terminate: function () { tsdcTerminate(need(this, 'TSDC')); },
    });

    // =============================================================================================================
    // Encoding §5 / §6: TextEncoderStream, TextDecoderStream
    // =============================================================================================================
    var toStr = function (v) {
        if (typeof v === 'symbol') { throw new TypeError('Cannot convert a Symbol value to a string'); }
        return String(v);
    };
    var bufferSourceBytes = function (v) {
        if (v instanceof ArrayBuffer) { return new Uint8Array(v); }
        if (typeof SharedArrayBuffer !== 'undefined' && v instanceof SharedArrayBuffer) { return new Uint8Array(v); }
        if (isView(v)) { return new Uint8Array(v.buffer, v.byteOffset, v.byteLength); }
        throw new TypeError("The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
    };
    class TextEncoderStream {
        constructor() {
            var enc = new TextEncoder();
            var pending = null;
            var t = createTransformStream(function (chunk, c) {
                var s = toStr(chunk);
                if (pending !== null) { s = pending + s; pending = null; }
                if (s.length > 0) {
                    var last = s.charCodeAt(s.length - 1);
                    if (last >= 0xD800 && last <= 0xDBFF) { pending = s.slice(-1); s = s.slice(0, -1); }
                }
                if (s.length > 0) { tsdcEnqueue(c, enc.encode(s)); }
            }, function (c) {
                if (pending !== null) { pending = null; tsdcEnqueue(c, new Uint8Array([0xEF, 0xBF, 0xBD])); }
            });
            SL.set(this, { brand: 'TES', obj: this, ts: t });
        }
    }
    getters(TextEncoderStream.prototype, {
        encoding: function () { need(this, 'TES'); return 'utf-8'; },
        readable: function () { return need(this, 'TES').ts.readable.obj; },
        writable: function () { return need(this, 'TES').ts.writable.obj; },
    });
    class TextDecoderStream {
        constructor() {
            var label = arguments[0] === undefined ? 'utf-8' : toStr(arguments[0]);
            var options = arguments[1];
            var dec = new TextDecoder(label, options);
            var t = createTransformStream(function (chunk, c) {
                var out = dec.decode(bufferSourceBytes(chunk), { stream: true });
                if (out !== '') { tsdcEnqueue(c, out); }
            }, function (c) {
                var out = dec.decode();
                if (out !== '') { tsdcEnqueue(c, out); }
            });
            SL.set(this, { brand: 'TDS', obj: this, ts: t, dec: dec });
        }
    }
    getters(TextDecoderStream.prototype, {
        encoding: function () { return need(this, 'TDS').dec.encoding; },
        fatal: function () { return need(this, 'TDS').dec.fatal; },
        ignoreBOM: function () { return need(this, 'TDS').dec.ignoreBOM; },
        readable: function () { return need(this, 'TDS').ts.readable.obj; },
        writable: function () { return need(this, 'TDS').ts.writable.obj; },
    });

    // =============================================================================================================
    // Compression Streams §5: DecompressionStream (pixel_core's inflater through __inflate)
    // =============================================================================================================
    var FORMATS = ['deflate', 'deflate-raw', 'gzip'];
    var formatOf = function (v, iface) {
        var f = toStr(v);
        if (FORMATS.indexOf(f) < 0) { throw new TypeError("Failed to construct '" + iface + "': The provided value '" + f + "' is not a valid enum value of type CompressionFormat."); }
        return f;
    };
    class DecompressionStream {
        constructor(format) {
            var f = formatOf(format, 'DecompressionStream');
            var input = [], total = 0, emitted = 0, finished = false;
            var concat = function () {
                var out = new Uint8Array(total), at = 0;
                input.forEach(function (b) { out.set(b, at); at += b.length; });
                input = [out];
                return out;
            };
            var step = function (c, last) {
                var all = concat();
                var r = __inflate(f, all, emitted);
                var out = new Uint8Array(r[1]);
                if (r[0] === 'error') { throw new TypeError('The compressed data is corrupt.'); }
                if (out.length > 0) { emitted += out.length; tsdcEnqueue(c, out); }
                if (r[0] === 'done') {
                    finished = true;
                    if (r[2] < all.length) { throw new TypeError('Junk found after the end of the compressed data.'); }
                } else if (last) {
                    throw new TypeError('The compressed data ended before the end of the stream.');
                }
            };
            var t = createTransformStream(function (chunk, c) {
                var b = bufferSourceBytes(chunk);
                if (finished) {
                    if (b.length > 0) { throw new TypeError('Junk found after the end of the compressed data.'); }
                    return;
                }
                if (b.length === 0) { return; }
                input.push(new Uint8Array(b));
                total += b.length;
                step(c, false);
            }, function (c) {
                if (!finished) { step(c, true); }
            });
            SL.set(this, { brand: 'DS', obj: this, ts: t });
        }
    }
    getters(DecompressionStream.prototype, {
        readable: function () { return need(this, 'DS').ts.readable.obj; },
        writable: function () { return need(this, 'DS').ts.writable.obj; },
    });

    [CountQueuingStrategy, ByteLengthQueuingStrategy, ReadableStream, ReadableStreamDefaultReader, ReadableStreamBYOBReader,
     ReadableStreamDefaultController, ReadableByteStreamController, ReadableStreamBYOBRequest, WritableStream,
     WritableStreamDefaultWriter, WritableStreamDefaultController, TransformStream, TransformStreamDefaultController,
     TextEncoderStream, TextDecoderStream, DecompressionStream].forEach(function (C) {
        tag(C);
        hide(G, C.name, C);
    });
    // fetch's Body mixin reads "disturbed" (Fetch §5.3) through this.
    hide(G, '__rs_disturbed', function (stream) { var s = rec(stream, 'RS'); return s !== undefined && s.disturbed; });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 1950 / 1951 / 1952 framings of "expected output" (the WPT compression fixture's bytes), fed whole, cut
    /// short, padded and corrupted.
    #[test]
    fn inflate_from_states() {
        // zlib: 78 9c + deflate("expected output") + adler32
        let zlib = [
            0x78, 0x9c, 0x4b, 0xad, 0x28, 0x48, 0x4d, 0x2e, 0x49, 0x4d, 0x51, 0xc8, 0x2f, 0x2d, 0x29, 0x28, 0x2d, 0x01, 0x00,
            0x30, 0xad, 0x06, 0x24,
        ];
        let (st, out) = inflate_from("deflate", &zlib, 0);
        assert_eq!(st, Inflated::Done(zlib.len()));
        assert_eq!(out, b"expected output");
        let (st, out) = inflate_from("deflate", &zlib[..10], 0);
        assert_eq!(st, Inflated::More);
        assert!(b"expected output".starts_with(&out));
        let (_, tail) = inflate_from("deflate", &zlib, 9);
        assert_eq!(tail, b"output");
        let mut padded = zlib.to_vec();
        padded.push(0);
        assert_eq!(inflate_from("deflate", &padded, 0).0, Inflated::Done(zlib.len()));
        let mut bad = zlib.to_vec();
        bad[0] = 0;
        assert_eq!(inflate_from("deflate", &bad, 0).0, Inflated::Error);
        let (st, out) = inflate_from("deflate-raw", &zlib[2..zlib.len() - 4], 0);
        assert_eq!((st, out.as_slice()), (Inflated::Done(zlib.len() - 6), &b"expected output"[..]));
    }
}
