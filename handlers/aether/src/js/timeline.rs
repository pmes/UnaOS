//! AETHERWPT2 (LEDGER SR135): User Timing L3 and Performance Timeline L2 on the page's `performance` object —
//! `PerformanceEntry` / `PerformanceMark` / `PerformanceMeasure`, `performance.mark` / `measure` /
//! `clearMarks` / `clearMeasures` / `getEntries` / `getEntriesByType` / `getEntriesByName`, and a
//! `PerformanceObserver` that delivers (`observe` with `type` + `buffered` or `entryTypes`, `takeRecords`,
//! `disconnect`, `supportedEntryTypes`, `droppedEntriesCount`) with `PerformanceObserverEntryList`. It replaces the
//! register-and-never-deliver observer stub of `api::platform`'s COMPAT prelude.
//!
//! The supported entry types are `mark` and `measure`: the buffer holding them is unbounded, so no entry is ever
//! dropped. Resource, navigation and paint entries are not produced (AETHERWPT2 gap O6), and there is no
//! PerformanceTiming (Navigation Timing L1): `measure` converts `navigationStart` to 0 (the time origin) and the
//! other PerformanceTiming names to an InvalidAccessError, as for an attribute whose value is still 0.

use js_core::vm::Vm;

/// Installs the timeline prelude into a page (Window) realm. Runs after `api::platform::init`.
pub fn install(vm: &mut Vm) {
    if let Err(e) = vm.run_script_str(TIMELINE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("timeline-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] timeline prelude failed: {m}");
    }
}

const TIMELINE: &str = r#"
(function () {
    if (typeof performance !== 'object' || performance === null) { return; }
    var PP = Object.getPrototypeOf(performance);
    var op = function (o, n, f) { Object.defineProperty(o, n, { value: f, writable: true, enumerable: true, configurable: true }); };
    var getter = function (o, n, f) { Object.defineProperty(o, n, { get: f, enumerable: true, configurable: true }); };
    var hide = function (n, v) { Object.defineProperty(globalThis, n, { value: v, writable: true, enumerable: false, configurable: true }); };
    var need = function (n, min, iface, opname) {
        if (n < min) { throw new TypeError("Failed to execute '" + opname + "' on '" + iface + "': " + min + ' argument' + (min > 1 ? 's' : '') + ' required, but only ' + n + ' present.'); }
    };
    var str = function (v) {
        if (typeof v === 'symbol') { throw new TypeError('Cannot convert a Symbol value to a string'); }
        return String(v);
    };
    var dbl = function (v, what) {
        var n = +v;
        if (!isFinite(n)) { throw new TypeError(what + ' is not a finite floating-point value.'); }
        return n;
    };
    var dict = function (v, what) {
        if (v === undefined || v === null) { return {}; }
        if (typeof v !== 'object' && typeof v !== 'function') { throw new TypeError(what + ": The provided value is not of type 'dictionary'."); }
        return v;
    };
    var clone = function (v) { return v === undefined || v === null ? null : structuredClone(v); };
    // The PerformanceTiming attribute names (Navigation Timing L1), which a mark may not take (User Timing §3.1.1).
    var TIMING = ['navigationStart', 'unloadEventStart', 'unloadEventEnd', 'redirectStart', 'redirectEnd', 'fetchStart',
        'domainLookupStart', 'domainLookupEnd', 'connectStart', 'connectEnd', 'secureConnectionStart', 'requestStart',
        'responseStart', 'responseEnd', 'domLoading', 'domInteractive', 'domContentLoadedEventStart',
        'domContentLoadedEventEnd', 'domComplete', 'loadEventStart', 'loadEventEnd'];
    // Installed by js::window::install only, so the current global object is always a Window.
    var isWindow = true;

    // ---- PerformanceEntry, PerformanceMark, PerformanceMeasure ---------------------------------------
    var ES = new WeakMap();
    var es = function (o) {
        var s = (o !== null && typeof o === 'object') ? ES.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var creating = false;
    class PerformanceEntry {
        constructor() { if (!creating) { throw new TypeError('Illegal constructor'); } }
    }
    getter(PerformanceEntry.prototype, 'name', function () { return es(this).name; });
    getter(PerformanceEntry.prototype, 'entryType', function () { return es(this).entryType; });
    getter(PerformanceEntry.prototype, 'startTime', function () { return es(this).startTime; });
    getter(PerformanceEntry.prototype, 'duration', function () { return es(this).duration; });
    op(PerformanceEntry.prototype, 'toJSON', {
        toJSON() { var s = es(this); return { name: s.name, entryType: s.entryType, startTime: s.startTime, duration: s.duration }; },
    }.toJSON);
    var make = function (C, state) {
        creating = true;
        var e;
        try { e = Reflect.construct(PerformanceEntry, [], C); } finally { creating = false; }
        ES.set(e, state);
        return e;
    };
    // User Timing §4.1 / §3.1.1: the PerformanceMark constructor.
    class PerformanceMark extends PerformanceEntry {
        constructor(markName) {
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'PerformanceMark': 1 argument required, but only 0 present."); }
            var name = str(markName);
            var o = dict(arguments[1], "Failed to construct 'PerformanceMark'");
            var detail = o.detail, start = o.startTime;
            if (start !== undefined) { start = dbl(start, "Failed to construct 'PerformanceMark': The provided double value"); }
            if (isWindow && TIMING.indexOf(name) >= 0) {
                throw new DOMException("Failed to construct 'PerformanceMark': '" + name + "' is part of the PerformanceTiming interface, and cannot be used as a mark name.", 'SyntaxError');
            }
            if (start !== undefined && start < 0) { throw new TypeError("Failed to construct 'PerformanceMark': '" + name + "' cannot have a negative start time."); }
            creating = true;
            try { super(); } finally { creating = false; }
            ES.set(this, { name: name, entryType: 'mark', startTime: start === undefined ? performance.now() : start, duration: 0, detail: clone(detail) });
        }
    }
    getter(PerformanceMark.prototype, 'detail', function () { return es(this).detail; });
    class PerformanceMeasure extends PerformanceEntry {
        constructor() { throw new TypeError('Illegal constructor'); }
    }
    getter(PerformanceMeasure.prototype, 'detail', function () { return es(this).detail; });

    // ---- the performance entry buffer (marks and measures: unbounded, so nothing is ever dropped) -----
    var buffer = [];
    // "filter buffer by name and type", chronological by startTime (stable).
    var filtered = function (list, name, type) {
        var out = [];
        for (var i = 0; i < list.length; i++) {
            var s = es(list[i]);
            if ((name === undefined || s.name === name) && (type === undefined || s.entryType === type)) { out.push([list[i], s.startTime, i]); }
        }
        out.sort(function (a, b) { return a[1] - b[1] || a[2] - b[2]; });
        return out.map(function (x) { return x[0]; });
    };

    // ---- PerformanceObserver (Performance Timeline §3) -----------------------------------------------
    var SUPPORTED = Object.freeze(['mark', 'measure']);
    var registered = [];
    var OS = new WeakMap();
    var os = function (o) {
        var s = (o !== null && typeof o === 'object') ? OS.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var pending = false;
    // "queue the PerformanceObserver task" (§4.5.2): one task delivers every registered observer's buffer.
    var queueTask = function () {
        if (pending) { return; }
        pending = true;
        setTimeout(function () {
            pending = false;
            var list = registered.slice();
            for (var i = 0; i < list.length; i++) {
                var obs = list[i], s = OS.get(obs);
                if (!s || !s.buffer.length) { continue; }
                var entries = s.buffer;
                s.buffer = [];
                var el = mkList(entries);
                var options = {};
                if (s.requiresDropped) { options.droppedEntriesCount = 0; s.requiresDropped = false; }
                try { s.callback.call(obs, el, obs, options); } catch (e) { reportError(e); }
            }
        }, 0);
    };
    // "queue a PerformanceEntry" (§4.5.1).
    var queueEntry = function (entry) {
        var type = es(entry).entryType;
        for (var i = 0; i < registered.length; i++) {
            var s = OS.get(registered[i]);
            if (s && s.types.indexOf(type) >= 0) { s.buffer.push(entry); }
        }
        buffer.push(entry);
        queueTask();
    };
    class PerformanceObserver {
        constructor(callback) {
            if (typeof callback !== 'function') { throw new TypeError("Failed to construct 'PerformanceObserver': The callback provided as parameter 1 is not a function."); }
            OS.set(this, { callback: callback, buffer: [], type: undefined, types: [], requiresDropped: false });
        }
        static get supportedEntryTypes() { return SUPPORTED; }
    }
    op(PerformanceObserver.prototype, 'observe', {
        observe() {
            var s = os(this);
            var o = dict(arguments[0], "Failed to execute 'observe' on 'PerformanceObserver'");
            // PerformanceObserverInit members in order: buffered, durationThreshold, entryTypes, type.
            var buffered = o.buffered, dt = o.durationThreshold, entryTypes = o.entryTypes, type = o.type;
            if (buffered !== undefined) { buffered = !!buffered; }
            if (dt !== undefined) { dt = +dt; }
            if (entryTypes !== undefined) {
                if (entryTypes === null || typeof entryTypes !== 'object' || typeof entryTypes[Symbol.iterator] !== 'function') {
                    throw new TypeError("Failed to execute 'observe' on 'PerformanceObserver': The provided value cannot be converted to a sequence.");
                }
                var et = [];
                for (var x of entryTypes) { et.push(str(x)); }
                entryTypes = et;
            }
            if (type !== undefined) { type = str(type); }
            if (entryTypes === undefined && type === undefined) {
                throw new TypeError("Failed to execute 'observe' on 'PerformanceObserver': An observe() call must include either entryTypes or type arguments.");
            }
            if (entryTypes !== undefined && (type !== undefined || buffered !== undefined || dt !== undefined)) {
                throw new TypeError("Failed to execute 'observe' on 'PerformanceObserver': An observe() call must not include both entryTypes and type arguments.");
            }
            if (s.type === undefined) { s.type = entryTypes !== undefined ? 'multiple' : 'single'; }
            if (s.type === 'single' && entryTypes !== undefined) {
                throw new DOMException("Failed to execute 'observe' on 'PerformanceObserver': This observer has performed observe({type:...}, therefore it cannot perform observe({entryTypes:...})", 'InvalidModificationError');
            }
            if (s.type === 'multiple' && type !== undefined) {
                throw new DOMException("Failed to execute 'observe' on 'PerformanceObserver': This observer has performed observe({entryTypes:...}, therefore it cannot perform observe({type:...})", 'InvalidModificationError');
            }
            s.requiresDropped = true;
            if (s.type === 'multiple') {
                var types = entryTypes.filter(function (t) { return SUPPORTED.indexOf(t) >= 0; });
                if (!types.length) { return; }
                s.types = types;
                if (registered.indexOf(this) < 0) { registered.push(this); }
                return;
            }
            if (SUPPORTED.indexOf(type) < 0) { return; }
            if (s.types.indexOf(type) < 0) { s.types.push(type); }
            if (registered.indexOf(this) < 0) { registered.push(this); }
            if (buffered) {
                var old = filtered(buffer, undefined, type);
                for (var i = 0; i < old.length; i++) { s.buffer.push(old[i]); }
                queueTask();
            }
        },
    }.observe);
    op(PerformanceObserver.prototype, 'disconnect', {
        disconnect() {
            var s = os(this);
            var at = registered.indexOf(this);
            if (at >= 0) { registered.splice(at, 1); }
            s.buffer = [];
            s.types = [];
        },
    }.disconnect);
    op(PerformanceObserver.prototype, 'takeRecords', {
        takeRecords() { var s = os(this), b = s.buffer; s.buffer = []; return b; },
    }.takeRecords);

    var EL = new WeakMap();
    var el = function (o) {
        var v = (o !== null && typeof o === 'object') ? EL.get(o) : undefined;
        if (v === undefined) { throw new TypeError('Illegal invocation'); }
        return v;
    };
    class PerformanceObserverEntryList {
        constructor() { throw new TypeError('Illegal constructor'); }
    }
    var mkList = function (entries) {
        var o = Object.create(PerformanceObserverEntryList.prototype);
        EL.set(o, entries);
        return o;
    };
    op(PerformanceObserverEntryList.prototype, 'getEntries', { getEntries() { return filtered(el(this)); } }.getEntries);
    op(PerformanceObserverEntryList.prototype, 'getEntriesByType', {
        getEntriesByType(type) { var l = el(this); need(arguments.length, 1, 'PerformanceObserverEntryList', 'getEntriesByType'); return filtered(l, undefined, str(type)); },
    }.getEntriesByType);
    op(PerformanceObserverEntryList.prototype, 'getEntriesByName', {
        getEntriesByName(name) {
            var l = el(this);
            need(arguments.length, 1, 'PerformanceObserverEntryList', 'getEntriesByName');
            var t = arguments[1];
            return filtered(l, str(name), t === undefined ? undefined : str(t));
        },
    }.getEntriesByName);

    // ---- Performance's User Timing and Performance Timeline members ----------------------------------
    // "convert a mark to a timestamp" (User Timing §3.1.3). There is no PerformanceTiming here (Navigation
    // Timing L1 is not implemented): navigationStart converts to 0 (it is the time origin), the other
    // PerformanceTiming names are an InvalidAccessError, as for an attribute whose value is still 0.
    var toTimestamp = function (mark) {
        if (typeof mark === 'number') {
            if (mark < 0) { throw new TypeError("Failed to execute 'measure' on 'Performance': '" + mark + "' is a negative time stamp."); }
            return mark;
        }
        var name = str(mark);
        if (isWindow && TIMING.indexOf(name) >= 0) {
            if (name === 'navigationStart') { return 0; }
            throw new DOMException("Failed to execute 'measure' on 'Performance': '" + name + "' is empty: either the event hasn't happened yet, or it would provide cross-origin timing information.", 'InvalidAccessError');
        }
        for (var i = buffer.length - 1; i >= 0; i--) {
            var s = es(buffer[i]);
            if (s.entryType === 'mark' && s.name === name) { return s.startTime; }
        }
        throw new DOMException("Failed to execute 'measure' on 'Performance': The mark '" + name + "' does not exist.", 'SyntaxError');
    };
    op(PP, 'mark', {
        mark(markName) {
            need(arguments.length, 1, 'Performance', 'mark');
            var m = new PerformanceMark(markName, arguments[1]);
            queueEntry(m);
            return m;
        },
    }.mark);
    var clearer = function (type) {
        return function (name) {
            var n = name === undefined ? undefined : str(name);
            buffer = buffer.filter(function (e) { var s = es(e); return !(s.entryType === type && (n === undefined || s.name === n)); });
        };
    };
    op(PP, 'clearMarks', { clearMarks(markName) { clearer('mark')(markName); } }.clearMarks);
    op(PP, 'clearMeasures', { clearMeasures(measureName) { clearer('measure')(measureName); } }.clearMeasures);
    // User Timing §3.1.2 measure().
    op(PP, 'measure', {
        measure(measureName) {
            need(arguments.length, 1, 'Performance', 'measure');
            var name = str(measureName);
            var som = arguments[1], endMark = arguments[2];
            var opts = null, startStr;
            if (som === undefined || som === null || typeof som === 'object' || typeof som === 'function') {
                var d = dict(som, "Failed to execute 'measure' on 'Performance'");
                // PerformanceMeasureOptions members in order: detail, duration, end, start.
                opts = { detail: d.detail, duration: d.duration, end: d.end, start: d.start };
                if (opts.duration !== undefined) { opts.duration = dbl(opts.duration, "Failed to execute 'measure' on 'Performance': The provided double value"); }
                if (opts.end !== undefined && typeof opts.end !== 'number') { opts.end = str(opts.end); }
                if (opts.start !== undefined && typeof opts.start !== 'number') { opts.start = str(opts.start); }
            } else {
                startStr = str(som);
            }
            if (endMark !== undefined) { endMark = str(endMark); }
            var has = function (k) { return opts !== null && opts[k] !== undefined; };
            if (has('start') || has('end') || has('duration') || has('detail')) {
                if (endMark !== undefined) { throw new TypeError("Failed to execute 'measure' on 'Performance': If a non-empty PerformanceMeasureOptions object was passed, |end_mark| must not be passed."); }
                if (!has('start') && !has('end')) { throw new TypeError("Failed to execute 'measure' on 'Performance': If a non-empty PerformanceMeasureOptions object was passed, at least one of its 'start' or 'end' properties must be present."); }
                if (has('start') && has('duration') && has('end')) { throw new TypeError("Failed to execute 'measure' on 'Performance': If a non-empty PerformanceMeasureOptions object was passed, it must not have all of its 'start', 'duration', and 'end' properties defined"); }
            }
            var end;
            if (endMark !== undefined) { end = toTimestamp(endMark); }
            else if (has('end')) { end = toTimestamp(opts.end); }
            else if (has('start') && has('duration')) { end = toTimestamp(opts.start) + opts.duration; }
            else { end = performance.now(); }
            var start;
            if (has('start')) { start = toTimestamp(opts.start); }
            else if (has('duration') && has('end')) { start = toTimestamp(opts.end) - opts.duration; }
            else if (startStr !== undefined) { start = toTimestamp(startStr); }
            else { start = 0; }
            var m = make(PerformanceMeasure, { name: name, entryType: 'measure', startTime: start, duration: end - start, detail: has('detail') ? clone(opts.detail) : null });
            queueEntry(m);
            return m;
        },
    }.measure);
    op(PP, 'getEntries', { getEntries() { return filtered(buffer); } }.getEntries);
    op(PP, 'getEntriesByType', {
        getEntriesByType(type) { need(arguments.length, 1, 'Performance', 'getEntriesByType'); return filtered(buffer, undefined, str(type)); },
    }.getEntriesByType);
    op(PP, 'getEntriesByName', {
        getEntriesByName(name) {
            need(arguments.length, 1, 'Performance', 'getEntriesByName');
            var t = arguments[1];
            return filtered(buffer, str(name), t === undefined ? undefined : str(t));
        },
    }.getEntriesByName);

    [PerformanceEntry, PerformanceMark, PerformanceMeasure, PerformanceObserver, PerformanceObserverEntryList].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(C.name, C);
    });
})();
"#;
