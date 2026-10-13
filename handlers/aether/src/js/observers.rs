//! AETHERAPIS (LEDGER SR145): IntersectionObserver (Intersection Observer §2 the interface — the constructor's
//! `root` / `rootMargin` / `scrollMargin` / `threshold` with their validation, `observe()` / `unobserve()` /
//! `disconnect()` / `takeRecords()`, `IntersectionObserverEntry`; §3.2.x "run the update intersection
//! observations steps" with the implicit root (the viewport) or an element root, the root margin, the threshold
//! index, "queue an intersection observer task", "notify intersection observers") and ResizeObserver (Resize
//! Observer §3 `ResizeObserver` with `box` options, `ResizeObserverEntry`, `ResizeObserverSize`; §4 the processing
//! model: "gather active observations at depth", "broadcast active resize observations" with the observation
//! depth, "has active observations", "deliver resize loop error notification" as the window `error` event
//! "ResizeObserver loop completed with undelivered notifications.").
//!
//! CHARTER: Aether — binding.
//!
//! Both observe the engine's own layout: the rects are `getBoundingClientRect()`'s (AETHERINLINE's boxes in
//! viewport coordinates) and the content box is the border box less the computed paddings and borders; a root's
//! clipping by its own `overflow` ancestors is not modelled (the gap list). [`update`] runs once per rendering
//! opportunity, after the animation-frame callbacks (HTML §8.1.7.3 "update the rendering" steps 14 and 16:
//! `Engine::drain_raf_once`, which `event_loop::settle` and the shell's frame call), and returns how many
//! notifications it queued or delivered, so the loop keeps turning while observations change.

use super::idl::arg;
use super::Task;
use js_core::vm::*;

/// "Update the rendering" for the observers: the update intersection observations steps (entries queued as a
/// task) and the resize observation loop (callbacks delivered now). Returns the notifications queued or delivered.
pub fn update(vm: &mut Vm) -> usize {
    if super::engine_poisoned() {
        return 0;
    }
    let g = vm.realm().global;
    let f = vm.get(g, &PropertyKey::from_str("__obs_update")).unwrap_or(Value::Undefined);
    if !vm.is_callable(&f) {
        return 0;
    }
    let r = super::invoke(vm, "observers-update", &f, &Value::Undefined, &[]);
    super::checkpoint(vm);
    match r {
        Some(Value::Number(n)) if n > 0.0 => n as usize,
        _ => 0,
    }
}

/// `__obs(op, arg)`: `queue(fn)` queues `fn` as a task (the intersection observer task source).
fn native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let op = vm.to_string(&v)?.to_rust();
    if op == "queue" {
        let f = arg(vm, ctx, 1);
        if vm.is_callable(&f) {
            let r = super::idl::root(vm, f);
            super::page(|p| p.tasks.push_back(Task::Callback(r)));
        }
    }
    Ok(Value::Undefined)
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__obs", 2, native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__obs"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("observers-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] observers prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var TOKEN = {};
    var now = function () { return (typeof performance === 'object' && performance && typeof performance.now === 'function') ? performance.now() : 0; };
    var rectOf = function (el) {
        var r = el.getBoundingClientRect();
        return { x: r.left, y: r.top, w: r.width, h: r.height };
    };
    var px = function (v) { var n = parseFloat(v); return isNaN(n) ? 0 : n; };
    var domRect = function (r) { return new DOMRectReadOnly(r.x, r.y, r.w, r.h); };
    var edges = function (el) {
        var cs = getComputedStyle(el);
        var g = function (p) { return px(cs.getPropertyValue(p)); };
        return { pt: g('padding-top'), pr: g('padding-right'), pb: g('padding-bottom'), pl: g('padding-left'),
                 bt: g('border-top-width'), br: g('border-right-width'), bb: g('border-bottom-width'), bl: g('border-left-width') };
    };
    var hasBox = function (el) { return el.isConnected && el.getClientRects().length > 0; };
    // ===== Intersection Observer =====
    var IO = new WeakMap();
    var observers = [];
    // "parse a margin": four <length-percentage>s in px or %
    var parseMargin = function (text, what) {
        var str = String(text).trim();
        var parts = str === '' ? ['0px'] : str.split(/\s+/);
        if (parts.length > 4) { throw new DOMException(what + ' must be specified in pixels or percent.', 'SyntaxError'); }
        var out = parts.map(function (p) {
            var m = /^(-?\d*\.?\d+(?:[eE][-+]?\d+)?)(px|%)$/.exec(p);
            if (!m) {
                if (/^-?\d*\.?\d+$/.test(p) && parseFloat(p) === 0) { return { v: 0, pct: false }; }
                throw new DOMException(what + ' must be specified in pixels or percent.', 'SyntaxError');
            }
            return { v: parseFloat(m[1]), pct: m[2] === '%' };
        });
        while (out.length < 4) { out.push(out.length === 1 ? out[0] : (out.length === 2 ? out[0] : out[1])); }
        return out;
    };
    var marginText = function (m) { return m.map(function (e) { return (Number.isInteger(e.v) ? e.v : e.v) + (e.pct ? '%' : 'px'); }).join(' '); };
    var expand = function (r, m) {
        var top = m[0].pct ? r.h * m[0].v / 100 : m[0].v, right = m[1].pct ? r.w * m[1].v / 100 : m[1].v;
        var bottom = m[2].pct ? r.h * m[2].v / 100 : m[2].v, left = m[3].pct ? r.w * m[3].v / 100 : m[3].v;
        return { x: r.x - left, y: r.y - top, w: r.w + left + right, h: r.h + top + bottom };
    };
    var viewportRect = function () { return { x: 0, y: 0, w: window.innerWidth, h: window.innerHeight }; };
    var intersect = function (a, b) {
        var x0 = Math.max(a.x, b.x), y0 = Math.max(a.y, b.y), x1 = Math.min(a.x + a.w, b.x + b.w), y1 = Math.min(a.y + a.h, b.y + b.h);
        if (x1 < x0 || y1 < y0) { return null; }
        return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    };
    class IntersectionObserverEntry {
        constructor(init) {
            if (init === null || typeof init !== 'object') { throw new TypeError("Failed to construct 'IntersectionObserverEntry': parameter 1 is not of type 'IntersectionObserverEntryInit'."); }
            var mk = function (r) { return r && !(r instanceof DOMRectReadOnly) ? new DOMRectReadOnly(r.x, r.y, r.width, r.height) : r; };
            this.__time = Number(init.time); this.__rootBounds = init.rootBounds === undefined ? null : mk(init.rootBounds);
            this.__boundingClientRect = mk(init.boundingClientRect); this.__intersectionRect = mk(init.intersectionRect);
            this.__isIntersecting = !!init.isIntersecting; this.__intersectionRatio = Number(init.intersectionRatio); this.__target = init.target;
            this.__isVisible = !!init.isVisible;
        }
        get time() { return this.__time; }
        get rootBounds() { return this.__rootBounds; }
        get boundingClientRect() { return this.__boundingClientRect; }
        get intersectionRect() { return this.__intersectionRect; }
        get isIntersecting() { return this.__isIntersecting; }
        get isVisible() { return this.__isVisible; }
        get intersectionRatio() { return this.__intersectionRatio; }
        get target() { return this.__target; }
    }
    var entry = function (f) {
        var e = Object.create(IntersectionObserverEntry.prototype);
        e.__time = f.time; e.__rootBounds = f.rootBounds; e.__boundingClientRect = f.boundingClientRect; e.__intersectionRect = f.intersectionRect;
        e.__isIntersecting = f.isIntersecting; e.__isVisible = false; e.__intersectionRatio = f.intersectionRatio; e.__target = f.target;
        return e;
    };
    class IntersectionObserver {
        constructor(callback, options) {
            if (typeof callback !== 'function') { throw new TypeError("Failed to construct 'IntersectionObserver': The callback provided as parameter 1 is not a function."); }
            var o = options === undefined || options === null ? {} : options;
            if (typeof o !== 'object') { throw new TypeError("Failed to construct 'IntersectionObserver': The provided value is not of type 'IntersectionObserverInit'."); }
            var root = o.root === undefined ? null : o.root;
            if (root !== null && !(root instanceof Element) && !(root instanceof Document)) { throw new TypeError("Failed to construct 'IntersectionObserver': The provided value is not of type '(Element or Document)'."); }
            var rootMargin = parseMargin(o.rootMargin === undefined ? '0px' : o.rootMargin, 'rootMargin');
            var scrollMargin = parseMargin(o.scrollMargin === undefined ? '0px' : o.scrollMargin, 'scrollMargin');
            var th = o.threshold === undefined ? [0] : o.threshold;
            var thresholds = typeof th === 'number' ? [th] : (Array.isArray(th) || (th && typeof th[Symbol.iterator] === 'function') ? Array.from(th) : [Number(th)]);
            if (thresholds.length === 0) { thresholds = [0]; }
            thresholds = thresholds.map(function (t) {
                var n = Number(t);
                if (!(n >= 0 && n <= 1)) { throw new RangeError("Failed to construct 'IntersectionObserver': Threshold values must be numbers between 0 and 1"); }
                return n;
            });
            thresholds.sort(function (a, b) { return a - b; });
            var delay = o.delay === undefined ? 0 : Number(o.delay);
            IO.set(this, { cb: callback, root: root, rootMargin: rootMargin, scrollMargin: scrollMargin, thresholds: Object.freeze(thresholds), delay: delay, trackVisibility: !!o.trackVisibility, targets: [], queued: [], state: new Map() });
        }
        get root() { return IO.get(this).root; }
        get rootMargin() { return marginText(IO.get(this).rootMargin); }
        get scrollMargin() { return marginText(IO.get(this).scrollMargin); }
        get thresholds() { return IO.get(this).thresholds; }
        get delay() { return IO.get(this).delay; }
        get trackVisibility() { return IO.get(this).trackVisibility; }
        observe(target) {
            var s = IO.get(this);
            if (!(target instanceof Element)) { throw new TypeError("Failed to execute 'observe' on 'IntersectionObserver': parameter 1 is not of type 'Element'."); }
            if (s.targets.indexOf(target) >= 0) { return; }
            s.targets.push(target);
            s.state.set(target, { thresholdIndex: -1, isIntersecting: false });
            if (observers.indexOf(this) < 0) { observers.push(this); }
        }
        unobserve(target) {
            var s = IO.get(this);
            if (!(target instanceof Element)) { throw new TypeError("Failed to execute 'unobserve' on 'IntersectionObserver': parameter 1 is not of type 'Element'."); }
            var i = s.targets.indexOf(target);
            if (i >= 0) { s.targets.splice(i, 1); s.state.delete(target); }
            if (s.targets.length === 0) { var k = observers.indexOf(this); if (k >= 0) { observers.splice(k, 1); } }
        }
        disconnect() {
            var s = IO.get(this);
            s.targets = []; s.state = new Map();
            var k = observers.indexOf(this);
            if (k >= 0) { observers.splice(k, 1); }
        }
        takeRecords() {
            var s = IO.get(this);
            var q = s.queued; s.queued = [];
            return q;
        }
    }
    var ioTaskQueued = false;
    // §3.2.x "run the update intersection observations steps"
    var updateIntersections = function () {
        var queuedAny = 0;
        var time = now();
        observers.slice().forEach(function (obs) {
            var s = IO.get(obs);
            var root = s.root;
            var rootRect, rootConnected = true;
            if (root === null) { rootRect = viewportRect(); }
            else if (root instanceof Document) { rootRect = viewportRect(); rootConnected = root === document; }
            else { rootConnected = root.isConnected; rootRect = hasBox(root) ? rectOf(root) : null; }
            var rootBounds = rootRect ? expand(rootRect, s.rootMargin) : null;
            s.targets.slice().forEach(function (target) {
                var prev = s.state.get(target);
                var targetRect = { x: 0, y: 0, w: 0, h: 0 }, intersectionRect = { x: 0, y: 0, w: 0, h: 0 }, isIntersecting = false, ratio = 0;
                var sameDoc = target.ownerDocument === document;
                var inRoot = root === null || root instanceof Document ? target.isConnected && sameDoc : (root.contains(target) && root !== target);
                if (sameDoc && target.isConnected && hasBox(target) && rootBounds && rootConnected && inRoot) {
                    targetRect = rectOf(target);
                    var ir = intersect(targetRect, rootBounds);
                    if (ir) {
                        isIntersecting = true;
                        intersectionRect = ir;
                        var area = targetRect.w * targetRect.h;
                        ratio = area > 0 ? (ir.w * ir.h) / area : 1;
                    }
                } else if (sameDoc && target.isConnected && hasBox(target)) {
                    targetRect = rectOf(target);
                }
                if (ratio > 1) { ratio = 1; }
                var thresholdIndex = 0;
                if (isIntersecting) {
                    thresholdIndex = s.thresholds.length;
                    for (var i = 0; i < s.thresholds.length; i++) { if (s.thresholds[i] > ratio) { thresholdIndex = i; break; } }
                }
                if (thresholdIndex !== prev.thresholdIndex || isIntersecting !== prev.isIntersecting) {
                    s.queued.push(entry({ time: time, rootBounds: rootBounds ? domRect(rootBounds) : null, boundingClientRect: domRect(targetRect), intersectionRect: domRect(intersectionRect), isIntersecting: isIntersecting, intersectionRatio: ratio, target: target }));
                    prev.thresholdIndex = thresholdIndex; prev.isIntersecting = isIntersecting;
                    queuedAny++;
                }
            });
        });
        if (queuedAny > 0 && !ioTaskQueued) {
            ioTaskQueued = true;
            __obs('queue', function () {
                ioTaskQueued = false;
                observers.slice().forEach(function (obs) {
                    var s = IO.get(obs);
                    if (s.queued.length === 0) { return; }
                    var q = s.queued; s.queued = [];
                    try { s.cb.call(obs, q, obs); } catch (e) { if (typeof reportError === 'function') { reportError(e); } }
                });
            });
        }
        return queuedAny;
    };
    [IntersectionObserver, IntersectionObserverEntry].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(globalThis, C.name, C);
    });
    // ===== Resize Observer =====
    var RO = new WeakMap();
    var ros = [];
    class ResizeObserverSize {
        constructor(tok, i, b) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } this.__i = i; this.__b = b; }
        get inlineSize() { return this.__i; }
        get blockSize() { return this.__b; }
    }
    class ResizeObserverEntry {
        constructor(tok, target, boxes) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            this.__target = target;
            this.__contentRect = boxes.contentRect;
            this.__border = Object.freeze([new ResizeObserverSize(TOKEN, boxes.border.i, boxes.border.b)]);
            this.__content = Object.freeze([new ResizeObserverSize(TOKEN, boxes.content.i, boxes.content.b)]);
            this.__device = Object.freeze([new ResizeObserverSize(TOKEN, boxes.device.i, boxes.device.b)]);
        }
        get target() { return this.__target; }
        get contentRect() { return this.__contentRect; }
        get borderBoxSize() { return this.__border; }
        get contentBoxSize() { return this.__content; }
        get devicePixelContentBoxSize() { return this.__device; }
    }
    var boxesOf = function (el) {
        if (!hasBox(el)) {
            return { border: { i: 0, b: 0 }, content: { i: 0, b: 0 }, device: { i: 0, b: 0 }, contentRect: new DOMRectReadOnly(0, 0, 0, 0) };
        }
        var r = rectOf(el), e = edges(el);
        var cw = Math.max(0, r.w - e.pl - e.pr - e.bl - e.br), ch = Math.max(0, r.h - e.pt - e.pb - e.bt - e.bb);
        var dpr = window.devicePixelRatio || 1;
        return { border: { i: r.w, b: r.h }, content: { i: cw, b: ch }, device: { i: Math.round(cw * dpr), b: Math.round(ch * dpr) }, contentRect: new DOMRectReadOnly(e.pl, e.pt, cw, ch) };
    };
    var sizeFor = function (boxes, box) { return box === 'border-box' ? boxes.border : (box === 'device-pixel-content-box' ? boxes.device : boxes.content); };
    var depthOf = function (el) { var d = 0; for (var n = el; n; n = n.parentNode || n.host) { d++; } return d; };
    class ResizeObserver {
        constructor(callback) {
            if (typeof callback !== 'function') { throw new TypeError("Failed to construct 'ResizeObserver': The callback provided as parameter 1 is not a function."); }
            RO.set(this, { cb: callback, obs: [], active: [], skipped: false });
        }
        observe(target, options) {
            var s = RO.get(this);
            if (!(target instanceof Element)) { throw new TypeError("Failed to execute 'observe' on 'ResizeObserver': parameter 1 is not of type 'Element'."); }
            var box = options && options.box !== undefined ? String(options.box) : 'content-box';
            if (['content-box', 'border-box', 'device-pixel-content-box'].indexOf(box) < 0) { throw new TypeError("Failed to execute 'observe' on 'ResizeObserver': The provided value '" + box + "' is not a valid enum value of type ResizeObserverBoxOptions."); }
            for (var i = 0; i < s.obs.length; i++) { if (s.obs[i].target === target) { s.obs.splice(i, 1); break; } }
            s.obs.push({ target: target, box: box, last: { i: 0, b: 0 }, depth: 0 });
            if (ros.indexOf(this) < 0) { ros.push(this); }
        }
        unobserve(target) {
            var s = RO.get(this);
            if (!(target instanceof Element)) { throw new TypeError("Failed to execute 'unobserve' on 'ResizeObserver': parameter 1 is not of type 'Element'."); }
            for (var i = 0; i < s.obs.length; i++) { if (s.obs[i].target === target) { s.obs.splice(i, 1); break; } }
            if (s.obs.length === 0) { var k = ros.indexOf(this); if (k >= 0) { ros.splice(k, 1); } }
        }
        disconnect() {
            var s = RO.get(this);
            s.obs = []; s.active = [];
            var k = ros.indexOf(this);
            if (k >= 0) { ros.splice(k, 1); }
        }
    }
    // §4.1 "gather active observations at depth"
    var gather = function (depth) {
        var any = false, shallowest = Infinity;
        ros.forEach(function (ro) {
            var s = RO.get(ro);
            s.active = []; s.skipped = false;
            s.obs.forEach(function (o) {
                if (!o.target.isConnected) { return; }
                var sz = sizeFor(boxesOf(o.target), o.box);
                if (sz.i === o.last.i && sz.b === o.last.b) { return; }
                var d = depthOf(o.target);
                if (d > depth) { s.active.push(o); any = true; if (d < shallowest) { shallowest = d; } }
                else { s.skipped = true; }
            });
        });
        return { any: any, shallowest: shallowest };
    };
    // §4.2 "broadcast active resize observations"
    var broadcast = function () {
        var shallowest = Infinity;
        ros.slice().forEach(function (ro) {
            var s = RO.get(ro);
            if (s.active.length === 0) { return; }
            var entries = [];
            s.active.forEach(function (o) {
                var boxes = boxesOf(o.target);
                entries.push(new ResizeObserverEntry(TOKEN, o.target, boxes));
                var sz = sizeFor(boxes, o.box);
                o.last = { i: sz.i, b: sz.b };
                var d = depthOf(o.target);
                if (d < shallowest) { shallowest = d; }
            });
            s.active = [];
            try { s.cb.call(ro, entries, ro); } catch (e) { if (typeof reportError === 'function') { reportError(e); } }
        });
        return shallowest;
    };
    var resizeLoop = function () {
        var delivered = 0;
        var depth = 0;
        var guard = 0;
        while (guard++ < 100) {
            var g = gather(depth);
            if (!g.any) { break; }
            depth = broadcast();
            delivered++;
        }
        var skipped = ros.some(function (ro) { return RO.get(ro).skipped; });
        if (skipped) {
            var e = new ErrorEvent('error', { message: 'ResizeObserver loop completed with undelivered notifications.' });
            window.dispatchEvent(e);
            if (typeof console === 'object' && console && typeof console.error === 'function') { console.error('ResizeObserver loop completed with undelivered notifications.'); }
        }
        return delivered;
    };
    [ResizeObserver, ResizeObserverEntry, ResizeObserverSize].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(globalThis, C.name, C);
    });
    hide(globalThis, '__obs_update', function () {
        var n = 0;
        if (observers.length > 0) { n += updateIntersections(); }
        if (ros.length > 0) { n += resizeLoop(); }
        return n;
    });
})();
"#;
