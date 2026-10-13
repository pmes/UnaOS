//! AETHERMEDIA2 (LEDGER SR134): Media Source Extensions™ (W3C, 2024) — `MediaSource`, `SourceBuffer`,
//! `SourceBufferList`, `TimeRanges`, `URL.createObjectURL(mediaSource)` and `HTMLMediaElement.buffered`
//! over `crate::media::mse` (the page's model and journal; Stria plays what is appended).
//!
//! CHARTER: Aether — binding.
//!
//! The split follows `api::platform`: the WebIDL shapes, argument checks and exceptions are a JS
//! prelude; the state, the segment parser and the coded frame processing are Rust (`__mse(op, …)`,
//! one native). Every interface is an `EventTarget` subclass, so the events Rust queues
//! (`TargetKey::Other(serial)`) dispatch to the page's listeners and `on…` handlers. `appendBuffer` /
//! `remove` set `updating` and queue `updatestart`; the SourceBuffer's own first `updatestart`
//! listener runs the operation (`__mse('run')`), which queues `update` + `updateend` (or `error` +
//! `updateend` and endOfStream("decode")); its own first `update` / `error` listener clears
//! `updating` (`__mse('done')`) before the page's listeners run, as MSE orders it; `abort()` clears it
//! at once.

use super::idl::{self, arg, num, s};
use js_core::vm::*;

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

fn num_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<f64> {
    let v = arg(vm, ctx, i);
    vm.to_number(&v)
}

fn u32_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<u32> {
    Ok(num_arg(vm, ctx, i)? as u32)
}

/// The EventTarget serial of a script-made target.
fn serial(vm: &Vm, v: &Value) -> Option<u32> {
    match super::events::target_of(vm, v) {
        Some(super::TargetKey::Other(n)) => Some(n),
        _ => None,
    }
}

fn numbers(vm: &mut Vm, v: Vec<f64>) -> Value {
    let elems = v.into_iter().map(num).collect();
    Value::Object(vm.new_array(elems))
}

/// `__mse(op, …)`: the MediaSource family's one native.
fn mse_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    use crate::media::mse;
    let op = str_arg(vm, ctx, 0)?;
    Ok(match op.as_str() {
        "new" => {
            let t = serial(vm, &arg(vm, ctx, 1));
            num(t.map(mse::create).unwrap_or(0) as f64)
        }
        "list" => {
            let id = u32_arg(vm, ctx, 1)?;
            if let Some(t) = serial(vm, &arg(vm, ctx, 2)) {
                mse::set_list_target(id, t);
            }
            Value::Undefined
        }
        "url" => {
            let id = u32_arg(vm, ctx, 1)?;
            s(&mse::object_url(id, &super::net::page_origin()))
        }
        "revoke" => {
            let u = str_arg(vm, ctx, 1)?;
            mse::revoke(&u);
            Value::Undefined
        }
        "state" => s(mse::ready_state(u32_arg(vm, ctx, 1)?)),
        "duration" => num(mse::duration(u32_arg(vm, ctx, 1)?)),
        "setDuration" => {
            let id = u32_arg(vm, ctx, 1)?;
            let v = num_arg(vm, ctx, 2)?;
            mse::set_duration(id, v);
            Value::Undefined
        }
        "anyUpdating" => Value::Bool(mse::any_updating(u32_arg(vm, ctx, 1)?)),
        "add" => {
            let id = u32_arg(vm, ctx, 1)?;
            let mime = str_arg(vm, ctx, 2)?;
            let t = serial(vm, &arg(vm, ctx, 3)).unwrap_or(0);
            num(mse::add_source_buffer(id, &mime, t).map(|i| i as f64).unwrap_or(-1.0))
        }
        "removeSb" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            mse::remove_source_buffer(id, sb);
            Value::Undefined
        }
        "append" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            let bytes = super::net::buffer_bytes(vm, &arg(vm, ctx, 3)).unwrap_or_default();
            mse::append(id, sb, bytes);
            Value::Undefined
        }
        "remove" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            let (a, b) = (num_arg(vm, ctx, 3)?, num_arg(vm, ctx, 4)?);
            mse::remove(id, sb, a, b);
            Value::Undefined
        }
        "run" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            mse::run(id, sb);
            Value::Undefined
        }
        "done" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            mse::done(id, sb);
            Value::Undefined
        }
        "abort" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            mse::abort(id, sb);
            Value::Undefined
        }
        "updating" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            Value::Bool(mse::updating(id, sb))
        }
        "settings" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            let (tso, seq, a, b) = mse::sb_settings(id, sb);
            numbers(vm, vec![tso, if seq { 1.0 } else { 0.0 }, a, b])
        }
        "setTso" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            let v = num_arg(vm, ctx, 3)?;
            mse::set_timestamp_offset(id, sb, v);
            Value::Undefined
        }
        "setMode" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            let seq = idl::boolean(vm, &arg(vm, ctx, 3));
            mse::set_mode(id, sb, seq);
            Value::Undefined
        }
        "setWindow" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            let (a, b) = (num_arg(vm, ctx, 3)?, num_arg(vm, ctx, 4)?);
            mse::set_append_window(id, sb, a, b);
            Value::Undefined
        }
        "sbBuffered" => {
            let id = u32_arg(vm, ctx, 1)?;
            let sb = u32_arg(vm, ctx, 2)? as u16;
            let v = mse::sb_buffered(id, sb);
            numbers(vm, v)
        }
        "eos" => {
            let id = u32_arg(vm, ctx, 1)?;
            let e = u32_arg(vm, ctx, 2)? as u8;
            mse::end_of_stream(id, e);
            Value::Undefined
        }
        "supported" => {
            let m = str_arg(vm, ctx, 1)?;
            Value::Bool(mse::is_type_supported(&m))
        }
        "mediaBuffered" => {
            let Some(el) = super::dom::node_of(vm, &arg(vm, ctx, 1)) else { return Ok(numbers(vm, Vec::new())) };
            let r = crate::media::buffered(&super::dom::node_ref(el));
            let v = r.iter().flat_map(|&(a, b)| [a as f64 / 1e9, if b == i64::MAX { f64::INFINITY } else { b as f64 / 1e9 }]).collect();
            numbers(vm, v)
        }
        "queue" => {
            // a task that fires the simple event `type` at an EventTarget (MSE's "queue a task to fire")
            if let Some(t) = super::events::target_of(vm, &arg(vm, ctx, 1)) {
                let ty = str_arg(vm, ctx, 2)?;
                crate::media::queue_target_event(t, &ty);
            }
            Value::Undefined
        }
        _ => return vm.throw_type("__mse: unknown operation"),
    })
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__mse", 4, mse_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__mse"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("mse-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] mse prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var TOKEN = {};
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var M = __mse;
    var fail = function (iface, op, msg, name) {
        return new DOMException("Failed to execute '" + op + "' on '" + iface + "': " + msg, name);
    };
    var setFail = function (iface, prop, msg, name) {
        return new DOMException("Failed to set the '" + prop + "' property on '" + iface + "': " + msg, name);
    };
    // HTML §8.1.8.1 event handler IDL attributes on a script-made EventTarget.
    var handlers = function (proto, names) {
        names.forEach(function (n) {
            Object.defineProperty(proto, 'on' + n, {
                configurable: true, enumerable: true,
                get: function () { var h = H.get(this); return (h && h[n]) || null; },
                set: function (f) {
                    var h = H.get(this);
                    if (!h) { h = {}; H.set(this, h); }
                    var first = !(n in h);
                    h[n] = (typeof f === 'function' || (typeof f === 'object' && f !== null)) ? f : null;
                    if (first) {
                        var self = this;
                        this.addEventListener(n, function (e) {
                            var f = h[n];
                            if (typeof f === 'function') { return f.call(self, e); }
                        });
                    }
                },
            });
        });
    };
    var H = new WeakMap();
    var isBufferSource = function (d) {
        return d instanceof ArrayBuffer || ArrayBuffer.isView(d);
    };

    class TimeRanges {
        constructor(tok, flat) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            Object.defineProperty(this, '_r', { value: flat.slice(), enumerable: false });
        }
        get length() { return this._r.length / 2; }
        start(i) {
            i = i >>> 0;
            if (i >= this.length) { throw fail('TimeRanges', 'start', 'The index provided (' + i + ') is greater than or equal to the maximum bound (' + this.length + ').', 'IndexSizeError'); }
            return this._r[2 * i];
        }
        end(i) {
            i = i >>> 0;
            if (i >= this.length) { throw fail('TimeRanges', 'end', 'The index provided (' + i + ') is greater than or equal to the maximum bound (' + this.length + ').', 'IndexSizeError'); }
            return this._r[2 * i + 1];
        }
    }
    var ranges = function (flat) { return new TimeRanges(TOKEN, flat); };

    var MS = new WeakMap();   // MediaSource -> { id, list, active }
    var SB = new WeakMap();   // SourceBuffer -> { ms, sb, removed }

    class SourceBufferList extends EventTarget {
        constructor(tok) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            Object.defineProperty(this, '_l', { value: [], enumerable: false });
        }
        get length() { return this._l.length; }
        _sync() {
            for (var k = 0; k < this._l.length + 1; k++) { delete this[k]; }
            for (var i = 0; i < this._l.length; i++) {
                Object.defineProperty(this, i, { value: this._l[i], configurable: true, enumerable: true });
            }
        }
        [Symbol.iterator]() { return this._l[Symbol.iterator](); }
    }
    handlers(SourceBufferList.prototype, ['addsourcebuffer', 'removesourcebuffer']);

    var msOf = function (sb, op) {
        var s = SB.get(sb);
        if (!s) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var checkWritable = function (s, iface, op, setter) {
        var f = setter ? setFail : fail;
        if (s.removed) { return f(iface, op, 'This SourceBuffer has been removed from the parent media source.', 'InvalidStateError'); }
        if (M('updating', s.ms.id, s.sb)) { return f(iface, op, "This SourceBuffer is still processing an 'appendBuffer' or 'remove' operation.", 'InvalidStateError'); }
        return null;
    };

    class SourceBuffer extends EventTarget {
        constructor(tok, ms, mime) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            var self = this;
            var st = { ms: ms, sb: -1, removed: false, aborted: false };
            SB.set(this, st);
            // MSE's ordering: the operation runs in the updatestart task, `updating` is false before
            // `update` / `error` / `abort` reach the page — these listeners are registered first.
            this.addEventListener('updatestart', function () {
                if (st.aborted) { st.aborted = false; return; }
                M('run', st.ms.id, st.sb);
            });
            var done = function () { M('done', st.ms.id, st.sb); };
            this.addEventListener('update', done);
            this.addEventListener('error', done);
            st.sb = M('add', ms.id, mime, this);
        }
        get updating() { var s = msOf(this); return !s.removed && M('updating', s.ms.id, s.sb); }
        get buffered() {
            var s = msOf(this);
            if (s.removed) { throw new DOMException("Failed to read the 'buffered' property from 'SourceBuffer': This SourceBuffer has been removed from the parent media source.", 'InvalidStateError'); }
            return ranges(M('sbBuffered', s.ms.id, s.sb));
        }
        get mode() { var s = msOf(this); return M('settings', s.ms.id, s.sb)[1] ? 'sequence' : 'segments'; }
        set mode(v) {
            var s = msOf(this);
            v = String(v);
            if (v !== 'segments' && v !== 'sequence') { throw new TypeError("Failed to set the 'mode' property on 'SourceBuffer': The provided value '" + v + "' is not a valid enum value of type AppendMode."); }
            var e = checkWritable(s, 'SourceBuffer', 'mode', true); if (e) { throw e; }
            M('setMode', s.ms.id, s.sb, v === 'sequence');
        }
        get timestampOffset() { var s = msOf(this); return M('settings', s.ms.id, s.sb)[0]; }
        set timestampOffset(v) {
            var s = msOf(this);
            v = Number(v);
            if (!isFinite(v)) { throw new TypeError("Failed to set the 'timestampOffset' property on 'SourceBuffer': The provided double value is non-finite."); }
            var e = checkWritable(s, 'SourceBuffer', 'timestampOffset', true); if (e) { throw e; }
            M('setTso', s.ms.id, s.sb, v);
        }
        get appendWindowStart() { var s = msOf(this); return M('settings', s.ms.id, s.sb)[2]; }
        set appendWindowStart(v) {
            var s = msOf(this);
            v = Number(v);
            var cur = M('settings', s.ms.id, s.sb);
            if (!isFinite(v)) { throw new TypeError("Failed to set the 'appendWindowStart' property on 'SourceBuffer': The provided double value is non-finite."); }
            var e = checkWritable(s, 'SourceBuffer', 'appendWindowStart', true); if (e) { throw e; }
            if (v < 0 || v >= cur[3]) { throw new TypeError("Failed to set the 'appendWindowStart' property on 'SourceBuffer': The value provided (" + v + ") is outside the range [0, " + cur[3] + ")."); }
            M('setWindow', s.ms.id, s.sb, v, cur[3]);
        }
        get appendWindowEnd() { var s = msOf(this); return M('settings', s.ms.id, s.sb)[3]; }
        set appendWindowEnd(v) {
            var s = msOf(this);
            v = Number(v);
            var cur = M('settings', s.ms.id, s.sb);
            if (v !== v) { throw new TypeError("Failed to set the 'appendWindowEnd' property on 'SourceBuffer': The value provided is NaN."); }
            var e = checkWritable(s, 'SourceBuffer', 'appendWindowEnd', true); if (e) { throw e; }
            if (v <= cur[2]) { throw new TypeError("Failed to set the 'appendWindowEnd' property on 'SourceBuffer': The value provided (" + v + ") is less than or equal to the start value."); }
            M('setWindow', s.ms.id, s.sb, cur[2], v);
        }
        appendBuffer(data) {
            var s = msOf(this);
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'appendBuffer' on 'SourceBuffer': 1 argument required, but only 0 present."); }
            if (!isBufferSource(data)) { throw new TypeError("Failed to execute 'appendBuffer' on 'SourceBuffer': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'."); }
            var e = checkWritable(s, 'SourceBuffer', 'appendBuffer', false); if (e) { throw e; }
            s.aborted = false;
            M('append', s.ms.id, s.sb, data);
        }
        remove(start, end) {
            var s = msOf(this);
            start = Number(start); end = Number(end);
            var e = checkWritable(s, 'SourceBuffer', 'remove', false); if (e) { throw e; }
            var d = M('duration', s.ms.id);
            if (d !== d || start < 0 || start > d) { throw new TypeError("Failed to execute 'remove' on 'SourceBuffer': The start provided (" + start + ") is outside the range (0, " + d + ")."); }
            if (!(end > start) || end !== end) { throw new TypeError("Failed to execute 'remove' on 'SourceBuffer': The end value provided (" + end + ") must be greater than the start value provided (" + start + ")."); }
            s.aborted = false;
            M('remove', s.ms.id, s.sb, start, end);
        }
        abort() {
            var s = msOf(this);
            if (s.removed) { throw fail('SourceBuffer', 'abort', 'This SourceBuffer has been removed from the parent media source.', 'InvalidStateError'); }
            if (M('state', s.ms.id) !== 'open') { throw fail('SourceBuffer', 'abort', "The parent media source's readyState is not 'open'.", 'InvalidStateError'); }
            if (M('updating', s.ms.id, s.sb)) { s.aborted = true; }
            M('abort', s.ms.id, s.sb);
        }
        changeType(type) {
            var s = msOf(this);
            type = String(type);
            if (type === '') { throw new TypeError("Failed to execute 'changeType' on 'SourceBuffer': The type provided is empty."); }
            var e = checkWritable(s, 'SourceBuffer', 'changeType', false); if (e) { throw e; }
            if (!M('supported', type)) { throw fail('SourceBuffer', 'changeType', "Changing to the type provided ('" + type + "') is not supported.", 'NotSupportedError'); }
        }
    }
    handlers(SourceBuffer.prototype, ['updatestart', 'update', 'updateend', 'error', 'abort']);

    class MediaSource extends EventTarget {
        constructor() {
            super();
            var st = { id: 0, list: null, active: null };
            MS.set(this, st);
            st.id = M('new', this);
            st.list = new SourceBufferList(TOKEN);
            st.active = new SourceBufferList(TOKEN);
            M('list', st.id, st.list);
        }
        get readyState() { return M('state', MS.get(this).id); }
        get duration() { return M('duration', MS.get(this).id); }
        set duration(v) {
            var st = MS.get(this);
            v = Number(v);
            if (v !== v || v < 0) { throw new TypeError("Failed to set the 'duration' property on 'MediaSource': The value provided (" + v + ") is " + (v !== v ? 'NaN.' : 'negative.')); }
            if (M('state', st.id) !== 'open') { throw setFail('MediaSource', 'duration', "The MediaSource's readyState is not 'open'.", 'InvalidStateError'); }
            if (M('anyUpdating', st.id)) { throw setFail('MediaSource', 'duration', "The 'updating' attribute is true on one or more of this MediaSource's SourceBuffers.", 'InvalidStateError'); }
            M('setDuration', st.id, v);
        }
        get sourceBuffers() { return MS.get(this).list; }
        get activeSourceBuffers() { return MS.get(this).active; }
        addSourceBuffer(type) {
            var st = MS.get(this);
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'addSourceBuffer' on 'MediaSource': 1 argument required, but only 0 present."); }
            type = String(type);
            if (type === '') { throw new TypeError("Failed to execute 'addSourceBuffer' on 'MediaSource': The type provided is empty."); }
            if (!M('supported', type)) { throw fail('MediaSource', 'addSourceBuffer', "The type provided ('" + type + "') is unsupported.", 'NotSupportedError'); }
            if (M('state', st.id) !== 'open') { throw fail('MediaSource', 'addSourceBuffer', "The MediaSource's readyState is not 'open'.", 'InvalidStateError'); }
            var sb = new SourceBuffer(TOKEN, st, type);
            st.list._l.push(sb); st.list._sync();
            st.active._l.push(sb); st.active._sync();
            return sb;
        }
        removeSourceBuffer(sb) {
            var st = MS.get(this);
            var i = st.list._l.indexOf(sb);
            if (i < 0) { throw fail('MediaSource', 'removeSourceBuffer', 'The SourceBuffer provided is not contained in this MediaSource.', 'NotFoundError'); }
            var s = SB.get(sb);
            if (M('updating', s.ms.id, s.sb)) { s.aborted = true; }
            M('removeSb', st.id, s.sb);
            s.removed = true;
            st.list._l.splice(i, 1); st.list._sync();
            var j = st.active._l.indexOf(sb);
            if (j >= 0) { st.active._l.splice(j, 1); st.active._sync(); }
        }
        endOfStream(error) {
            var st = MS.get(this);
            if (error !== undefined && error !== 'network' && error !== 'decode') {
                throw new TypeError("Failed to execute 'endOfStream' on 'MediaSource': The provided value '" + error + "' is not a valid enum value of type EndOfStreamError.");
            }
            if (M('state', st.id) !== 'open') { throw fail('MediaSource', 'endOfStream', "The MediaSource's readyState is not 'open'.", 'InvalidStateError'); }
            if (M('anyUpdating', st.id)) { throw fail('MediaSource', 'endOfStream', "The 'updating' attribute is true on one or more of this MediaSource's SourceBuffers.", 'InvalidStateError'); }
            M('eos', st.id, error === 'network' ? 1 : error === 'decode' ? 2 : 0);
        }
        setLiveSeekableRange(start, end) {
            var st = MS.get(this);
            if (M('state', st.id) !== 'open') { throw fail('MediaSource', 'setLiveSeekableRange', "The MediaSource's readyState is not 'open'.", 'InvalidStateError'); }
            start = Number(start); end = Number(end);
            if (start < 0 || start > end) { throw new TypeError("Failed to execute 'setLiveSeekableRange' on 'MediaSource': The start provided (" + start + ") is outside the range (0, " + end + ")."); }
            st.live = [start, end];
        }
        clearLiveSeekableRange() {
            var st = MS.get(this);
            if (M('state', st.id) !== 'open') { throw fail('MediaSource', 'clearLiveSeekableRange', "The MediaSource's readyState is not 'open'.", 'InvalidStateError'); }
            st.live = null;
        }
        static isTypeSupported(type) { return M('supported', String(type)); }
    }
    Object.defineProperty(MediaSource, 'canConstructInDedicatedWorker', { value: false, enumerable: true, configurable: true });
    handlers(MediaSource.prototype, ['sourceopen', 'sourceended', 'sourceclose']);

    hide('MediaSource', MediaSource);
    hide('SourceBuffer', SourceBuffer);
    hide('SourceBufferList', SourceBufferList);
    hide('TimeRanges', TimeRanges);

    // URL.createObjectURL(mediaSource): the MediaSource object URL (MSE §2.5); other objects keep the
    // platform's handle.
    if (typeof URL === 'function') {
        var create = URL.createObjectURL, revoke = URL.revokeObjectURL;
        URL.createObjectURL = function (obj) {
            if (obj instanceof MediaSource) { return M('url', MS.get(obj).id); }
            return create.apply(this, arguments);
        };
        URL.revokeObjectURL = function (url) {
            M('revoke', String(url));
            return revoke.apply(this, arguments);
        };
    }

    // HTMLMediaElement.buffered (HTML §4.8.11.10): the MediaSource's for an attached element.
    if (typeof HTMLMediaElement === 'function') {
        Object.defineProperty(HTMLMediaElement.prototype, 'buffered', {
            configurable: true, enumerable: true,
            get: function () { return ranges(M('mediaBuffered', this)); },
        });
        Object.defineProperty(HTMLMediaElement.prototype, 'seekable', {
            configurable: true, enumerable: true,
            get: function () {
                var d = this.duration;
                if (d !== d) { return ranges([]); }
                if (d === Infinity) { var b = M('mediaBuffered', this); return ranges(b.length ? [b[0], b[b.length - 1]] : []); }
                return ranges([0, d]);
            },
        });
    }
})();
"#;
