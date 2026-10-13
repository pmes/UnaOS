//! AETHERWPT3 (LEDGER SR138): the Navigation API (HTML §7.2.6, "The Navigation API" — `window.navigation`).
//!
//! `Navigation` (an EventTarget: `entries()`, `currentEntry`, `updateCurrentEntry()`, `transition`, `activation`,
//! `canGoBack` / `canGoForward`, `navigate()`, `reload()`, `traverseTo()`, `back()`, `forward()`, and the
//! `onnavigate` / `onnavigatesuccess` / `onnavigateerror` / `oncurrententrychange` handlers),
//! `NavigationHistoryEntry`, `NavigationDestination`, `NavigateEvent` (with `intercept()` / `scroll()`),
//! `NavigationCurrentEntryChangeEvent`, `NavigationTransition` and `NavigationActivation` are a prelude over the
//! page's own navigations. The Rust side hooks the navigations the page already performs — a Location
//! navigation (js/window.rs `navigate`, a fragment one in the document, any other staged for the shell),
//! `location.reload()`, `history.pushState` / `replaceState`, and a same-document traversal landing
//! (js/history.rs `traverse_same_document`) — through [`begin`] (§7.2.6.10 "fire a navigate event": the event's
//! answer, cancel / proceed / intercept) and [`commit`] (§7.2.6.8 "update the navigation API entries for a
//! same-document navigation": the entry list, `currententrychange`, the API method tracker's `committed`, the
//! intercept handlers, then `navigatesuccess` / `navigateerror` and `finished`).
//!
//! The entry list is this document's same-document entries from its load on (the session's earlier entries
//! are the shell's, and other documents' are not exposed). Events the user agent fires are trusted
//! ([`super::events::dispatch_trusted`]).

use super::idl::*;
use js_core::vm::*;

/// The page's answer to a navigate event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Begin {
    /// A listener canceled it: the navigation does not happen.
    Cancel,
    /// The navigation goes ahead as it would have.
    Proceed,
    /// `intercept()` was called: the navigation becomes a same-document one (the URL and history update
    /// steps, no fetch, no `hashchange`).
    Intercept,
}

fn hidden(vm: &mut Vm, name: &str) -> Option<Value> {
    let g = vm.realms[0].global;
    match vm.get(g, &PropertyKey::from_str(name)) {
        Ok(f @ Value::Object(_)) => Some(f),
        _ => None,
    }
}

/// §7.2.6.10 "fire a push/replace/reload navigate event" (and the traverse one) for a navigation of the top
/// document to `url`; `ty` is the NavigationType, `same_doc` whether it is same-document without
/// interception (a fragment navigation, `pushState` / `replaceState`, a same-document traversal).
pub fn begin(vm: &mut Vm, url: &str, ty: &str, same_doc: bool) -> Begin {
    let Some(f) = hidden(vm, "__nav_begin") else { return Begin::Proceed };
    let g = Value::Object(vm.realms[0].global);
    match vm.call(&f, &g, &[s(url), s(ty), Value::Bool(same_doc)]) {
        Ok(Value::Number(n)) if n == 0.0 => Begin::Cancel,
        Ok(Value::Number(n)) if n == 2.0 => Begin::Intercept,
        _ => Begin::Proceed,
    }
}

/// The navigation [`begin`] let through has updated the document's URL and session history: the entries,
/// `currententrychange`, and the rest of the navigation's promises.
pub fn commit(vm: &mut Vm) {
    let Some(f) = hidden(vm, "__nav_commit") else { return };
    let g = Value::Object(vm.realms[0].global);
    let _ = vm.call(&f, &g, &[]);
}

/// `__nav_dispatch(target, event)`: the event dispatched as the user agent's.
fn nav_dispatch(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = arg(vm, ctx, 0);
    let e = arg(vm, ctx, 1);
    match super::events::dispatch_trusted(vm, &t, &e) {
        Some(ok) => Ok(Value::Bool(ok)),
        None => vm.throw_type("__nav_dispatch: not an EventTarget and an Event"),
    }
}

/// Installs `window.navigation` and the interfaces (after the fetch prelude: AbortController, and the ports
/// prelude: structuredClone).
pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__nav_dispatch", 2, nav_dispatch, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__nav_dispatch"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("navigation-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] navigation prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    'use strict';
    if (typeof EventTarget !== 'function' || typeof Event !== 'function') { return; }
    var G = globalThis;
    var dispatchTrusted = G.__nav_dispatch;
    delete G.__nav_dispatch;
    var TOKEN = {};
    var hide = function (n, v) { Object.defineProperty(G, n, { value: v, writable: true, enumerable: false, configurable: true }); };
    var iface = function (n, c) { Object.defineProperty(G, n, { value: c, writable: true, enumerable: false, configurable: true }); };
    var op = function (o, n, f) { Object.defineProperty(o, n, { value: f, writable: true, enumerable: true, configurable: true }); };
    var str = function (v) {
        if (typeof v === 'symbol') { throw new TypeError('Cannot convert a Symbol value to a string'); }
        return String(v);
    };
    var dom = function (name, msg) { return new DOMException(msg, name); };
    var handled = function (p) { p.catch(function () {}); return p; };
    var serialize = function (v) { return v === undefined ? undefined : { v: structuredClone(v) }; };
    var deserialize = function (sv) { return sv === undefined ? undefined : structuredClone(sv.v); };
    var counter = 0;
    var uuid = function () {
        try { if (G.crypto && typeof G.crypto.randomUUID === 'function') { return G.crypto.randomUUID(); } } catch (e) {}
        counter++;
        return 'navigation-' + counter;
    };
    var NAV_TYPES = ['push', 'replace', 'reload', 'traverse'];
    var navType = function (v, nullable) {
        if (nullable && v === null) { return null; }
        var s = str(v);
        if (NAV_TYPES.indexOf(s) < 0) { throw new TypeError("The provided value '" + s + "' is not a valid enum value of type NavigationType."); }
        return s;
    };
    var noFrag = function (u) { var i = u.indexOf('#'); return i < 0 ? u : u.slice(0, i); };
    var originOf = function (u) { try { return new URL(u).origin; } catch (e) { return 'null'; } };
    var here = function () { return String(G.location.href); };

    // ---- event handler IDL attributes (HTML §8.1.8.1) -------------------------------------------------
    var HANDLERS = new WeakMap();
    var handlerAttr = function (proto, name) {
        var type = name.slice(2);
        Object.defineProperty(proto, name, {
            get: function () { var h = HANDLERS.get(this); return (h && h[name]) || null; },
            set: function (f) {
                var h = HANDLERS.get(this);
                if (!h) { h = {}; HANDLERS.set(this, h); }
                var v = (typeof f === 'function' || (f !== null && typeof f === 'object')) ? f : null;
                var self = this;
                if (v === null) {
                    if (h[name + ' listener']) { self.removeEventListener(type, h[name + ' listener']); h[name + ' listener'] = null; }
                } else if (!h[name + ' listener']) {
                    var l = function (e) { var fn = h[name]; if (typeof fn === 'function') { return fn.call(self, e); } };
                    h[name + ' listener'] = l;
                    self.addEventListener(type, l);
                }
                h[name] = v;
            },
            enumerable: true, configurable: true,
        });
    };

    // ---- NavigationHistoryEntry (§7.2.6.3) ------------------------------------------------------------
    var ES = new WeakMap();
    var es = function (o) { var s = ES.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    var entries = [];
    var current = -1;
    class NavigationHistoryEntry extends EventTarget {
        constructor(tok, rec) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            ES.set(this, rec);
        }
        get url() { var s = es(this); return s.live ? s.url : null; }
        get key() { var s = es(this); return s.live ? s.key : ''; }
        get id() { var s = es(this); return s.live ? s.id : ''; }
        get index() { es(this); return entries.indexOf(this); }
        get sameDocument() { return es(this).live; }
        getState() { var s = es(this); return s.live ? deserialize(s.state) : undefined; }
    }
    handlerAttr(NavigationHistoryEntry.prototype, 'ondispose');
    var newEntry = function (url, key, state) {
        return new NavigationHistoryEntry(TOKEN, { url: url, key: key || uuid(), id: uuid(), state: state, live: true });
    };
    var dispose = function (e) {
        // §7.2.6.8: an entry that left the list fires `dispose`; it keeps answering as a same-document entry.
        try { dispatchTrusted(e, new Event('dispose')); } catch (x) {}
    };

    // ---- NavigationDestination (§7.2.6.11) ------------------------------------------------------------
    var DS = new WeakMap();
    var ds = function (o) { var s = DS.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    class NavigationDestination {
        constructor(tok, rec) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } DS.set(this, rec); }
        get url() { return ds(this).url; }
        get key() { var s = ds(this); return s.entry ? es(s.entry).key : ''; }
        get id() { var s = ds(this); return s.entry ? es(s.entry).id : ''; }
        get index() { var s = ds(this); return s.entry ? entries.indexOf(s.entry) : -1; }
        get sameDocument() { return ds(this).sameDocument; }
        getState() { return deserialize(ds(this).state); }
    }

    // ---- NavigateEvent (§7.2.6.10) --------------------------------------------------------------------
    var NE = new WeakMap();
    var ne = function (o) { var s = NE.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    var requireObject = function (init, what) {
        if (init === undefined || init === null) { throw new TypeError("Failed to construct '" + what + "': 2 arguments required, but only 1 present."); }
        if (typeof init !== 'object' && typeof init !== 'function') { throw new TypeError("Failed to construct '" + what + "': The provided value is not of type '" + what + "Init'."); }
    };
    class NavigateEvent extends Event {
        constructor(type, init) {
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'NavigateEvent': 2 arguments required, but only 0 present."); }
            type = str(type);
            requireObject(init, 'NavigateEvent');
            // NavigateEventInit, members in order (WebIDL §3.2.17).
            var canIntercept = !!init.canIntercept;
            var destination = init.destination;
            if (destination === undefined) { throw new TypeError("Failed to construct 'NavigateEvent': required member destination is undefined."); }
            if (!DS.has(destination)) { throw new TypeError("Failed to construct 'NavigateEvent': member destination is not of type 'NavigationDestination'."); }
            var downloadRequest = init.downloadRequest === undefined || init.downloadRequest === null ? null : str(init.downloadRequest);
            var formData = init.formData === undefined ? null : init.formData;
            if (formData !== null && !(typeof FormData === 'function' && formData instanceof FormData)) { throw new TypeError("Failed to construct 'NavigateEvent': member formData is not of type 'FormData'."); }
            var hasUAVisualTransition = !!init.hasUAVisualTransition;
            var hashChange = !!init.hashChange;
            var info = init.info;
            var navigationType = init.navigationType === undefined ? 'push' : navType(init.navigationType, false);
            var signal = init.signal;
            if (signal === undefined) { throw new TypeError("Failed to construct 'NavigateEvent': required member signal is undefined."); }
            if (!(signal instanceof AbortSignal)) { throw new TypeError("Failed to construct 'NavigateEvent': member signal is not of type 'AbortSignal'."); }
            var sourceElement = init.sourceElement === undefined ? null : init.sourceElement;
            if (sourceElement !== null && !(sourceElement instanceof Element)) { throw new TypeError("Failed to construct 'NavigateEvent': member sourceElement is not of type 'Element'."); }
            var userInitiated = !!init.userInitiated;
            super(type, init);
            NE.set(this, {
                navigationType: navigationType, destination: destination, canIntercept: canIntercept, userInitiated: userInitiated,
                hashChange: hashChange, signal: signal, formData: formData, downloadRequest: downloadRequest, info: info,
                hasUAVisualTransition: hasUAVisualTransition, sourceElement: sourceElement,
                handlers: [], interception: 'none', dispatching: false, focusReset: undefined, scroll: undefined,
            });
        }
        get navigationType() { return ne(this).navigationType; }
        get destination() { return ne(this).destination; }
        get canIntercept() { return ne(this).canIntercept; }
        get userInitiated() { return ne(this).userInitiated; }
        get hashChange() { return ne(this).hashChange; }
        get signal() { return ne(this).signal; }
        get formData() { return ne(this).formData; }
        get downloadRequest() { return ne(this).downloadRequest; }
        get info() { return ne(this).info; }
        get hasUAVisualTransition() { return ne(this).hasUAVisualTransition; }
        get sourceElement() { return ne(this).sourceElement; }
        intercept(options) {
            var s = ne(this);
            var o = options === undefined || options === null ? {} : options;
            if (typeof o !== 'object' && typeof o !== 'function') { throw new TypeError("Failed to execute 'intercept' on 'NavigateEvent': The provided value is not of type 'NavigationInterceptOptions'."); }
            var focusReset = o.focusReset, handler = o.handler, scroll = o.scroll;
            if (focusReset !== undefined) { focusReset = str(focusReset); if (focusReset !== 'after-transition' && focusReset !== 'manual') { throw new TypeError("The provided value '" + focusReset + "' is not a valid enum value of type NavigationFocusReset."); } }
            if (handler !== undefined && typeof handler !== 'function') { throw new TypeError("Failed to execute 'intercept' on 'NavigateEvent': The provided value is not a function."); }
            if (scroll !== undefined) { scroll = str(scroll); if (scroll !== 'after-transition' && scroll !== 'manual') { throw new TypeError("The provided value '" + scroll + "' is not a valid enum value of type NavigationScrollBehavior."); } }
            // "perform shared checks", then intercept()'s own (§7.2.6.10).
            if (!this.isTrusted) { throw dom('SecurityError', "Failed to execute 'intercept' on 'NavigateEvent': intercept() may only be called on a trusted event."); }
            if (this.defaultPrevented) { throw dom('InvalidStateError', "Failed to execute 'intercept' on 'NavigateEvent': Cannot intercept a canceled event."); }
            if (!s.canIntercept) { throw dom('SecurityError', "Failed to execute 'intercept' on 'NavigateEvent': A navigation with URL '" + es_url(s.destination) + "' cannot be intercepted."); }
            if (!s.dispatching) { throw dom('InvalidStateError', "Failed to execute 'intercept' on 'NavigateEvent': intercept() may only be called while the navigate event is being dispatched."); }
            s.interception = 'intercepted';
            if (handler !== undefined) { s.handlers.push(handler); }
            if (focusReset !== undefined) { s.focusReset = focusReset; }
            if (scroll !== undefined) { s.scroll = scroll; }
        }
        scroll() {
            var s = ne(this);
            if (!this.isTrusted) { throw dom('SecurityError', "Failed to execute 'scroll' on 'NavigateEvent': scroll() may only be called on a trusted event."); }
            if (s.interception !== 'committed') { throw dom('InvalidStateError', "Failed to execute 'scroll' on 'NavigateEvent': scroll() may only be called after the navigation commits."); }
            s.interception = 'scrolled';
        }
    }
    var es_url = function (d) { return ds(d).url; };

    // ---- NavigationCurrentEntryChangeEvent (§7.2.6.12) ------------------------------------------------
    var CE = new WeakMap();
    class NavigationCurrentEntryChangeEvent extends Event {
        constructor(type, init) {
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'NavigationCurrentEntryChangeEvent': 2 arguments required, but only 0 present."); }
            type = str(type);
            requireObject(init, 'NavigationCurrentEntryChangeEvent');
            var from = init.from;
            if (from === undefined) { throw new TypeError("Failed to construct 'NavigationCurrentEntryChangeEvent': required member from is undefined."); }
            if (!ES.has(from)) { throw new TypeError("Failed to construct 'NavigationCurrentEntryChangeEvent': member from is not of type 'NavigationHistoryEntry'."); }
            var t = init.navigationType === undefined ? null : navType(init.navigationType, true);
            super(type, init);
            CE.set(this, { from: from, navigationType: t });
        }
        get navigationType() { var s = CE.get(this); if (!s) { throw new TypeError('Illegal invocation'); } return s.navigationType; }
        get from() { var s = CE.get(this); if (!s) { throw new TypeError('Illegal invocation'); } return s.from; }
    }

    // ---- NavigationTransition, NavigationActivation (§7.2.6.4, §7.2.6.5) ---------------------------------
    var TS = new WeakMap();
    class NavigationTransition {
        constructor(tok, rec) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } TS.set(this, rec); }
        get navigationType() { return TS.get(this).navigationType; }
        get from() { return TS.get(this).from; }
        get committed() { return TS.get(this).committed; }
        get finished() { return TS.get(this).finished; }
    }
    var AS = new WeakMap();
    class NavigationActivation {
        constructor(tok, rec) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } AS.set(this, rec); }
        get from() { return AS.get(this).from; }
        get entry() { return AS.get(this).entry; }
        get navigationType() { return AS.get(this).navigationType; }
    }

    // ---- Navigation (§7.2.6.1) ------------------------------------------------------------------------
    var nav = null;
    var self = function (o) { if (o !== nav || nav === null) { throw new TypeError('Illegal invocation'); } };
    var S = { ongoing: null, pending: null, transition: null, activation: null, inner: 0, traversals: [] };
    // The document's first entry, made when the API is first used (its URL is the document's then).
    var init = function () {
        if (current >= 0) { return; }
        entries = [newEntry(here(), null, undefined)];
        current = 0;
        S.activation = new NavigationActivation(TOKEN, { from: null, entry: entries[0], navigationType: 'push' });
    };
    var deferred = function () {
        var d = {};
        d.promise = new Promise(function (res, rej) { d.resolve = res; d.reject = rej; });
        handled(d.promise);
        return d;
    };
    // An API method tracker (§7.2.6.7): the result's committed / finished.
    var tracker = function (info, state) { return { info: info, state: state, committed: deferred(), finished: deferred(), done: false }; };
    var result = function (t) { return { committed: t.committed.promise, finished: t.finished.promise }; };
    var early = function (err) {
        var c = handled(Promise.reject(err)), f = handled(Promise.reject(err));
        return { committed: c, finished: f };
    };
    var resolved = function (entry) { return { committed: Promise.resolve(entry), finished: Promise.resolve(entry) }; };
    var fire = function (target, ev) { return dispatchTrusted(target, ev); };
    var errorEvent = function (reason) {
        var msg = '';
        try { msg = reason && reason.message !== undefined ? String(reason.message) : String(reason); } catch (e) {}
        if (typeof ErrorEvent === 'function') { return new ErrorEvent('navigateerror', { error: reason, message: msg }); }
        return new Event('navigateerror');
    };
    // "abort the ongoing navigation" (§7.2.6.10): the signal aborts, `navigateerror`, the promises reject.
    var abortOngoing = function (err) {
        var rec = S.ongoing;
        if (!rec) { return; }
        S.ongoing = null;
        if (rec.pending) { S.pending = null; }
        var st = NE.get(rec.event);
        if (st) { st.dispatching = false; }
        try { rec.ac.abort(err); } catch (e) {}
        fire(nav, errorEvent(err));
        settle(rec, false, err);
    };
    var settle = function (rec, ok, value) {
        if (rec.settled) { return; }
        rec.settled = true;
        if (rec.transition && S.transition === rec.transition) { S.transition = null; }
        if (rec.transitionFinished) { if (ok) { rec.transitionFinished.resolve(); } else { rec.transitionFinished.reject(value); } }
        if (rec.transitionCommitted && !ok) { rec.transitionCommitted.reject(value); }
        var t = rec.tracker;
        if (!t || t.done) { return; }
        t.done = true;
        if (ok) { t.finished.resolve(value); } else { if (!rec.committed) { t.committed.reject(value); } t.finished.reject(value); }
    };

    // "fire a navigate event" (§7.2.6.10). o: { url, type, sameDoc, hashChange, entry (traverse), state, info,
    // tracker }. Returns the record, or null when canceled.
    var fireNavigate = function (o) {
        init();
        abortOngoing(dom('AbortError', 'Navigation was aborted'));
        var dest = new NavigationDestination(TOKEN, { url: o.url, entry: o.entry || null, sameDocument: !!o.sameDoc, state: o.state });
        var ac = new AbortController();
        var rewritable = originOf(o.url) !== 'null' && originOf(o.url) === originOf(here());
        var canIntercept = rewritable && (o.type !== 'traverse' || !!o.sameDoc);
        var cancelable = o.type !== 'traverse' || !!o.sameDoc;
        var ev = new NavigateEvent('navigate', {
            cancelable: cancelable, navigationType: o.type, destination: dest, canIntercept: canIntercept,
            userInitiated: false, hashChange: !!o.hashChange, signal: ac.signal, formData: null, downloadRequest: null,
            info: o.info, sourceElement: null,
        });
        var st = NE.get(ev);
        var rec = { event: ev, ac: ac, o: o, tracker: o.tracker || null, committed: false, settled: false };
        S.ongoing = rec;
        st.dispatching = true;
        var ok;
        try { ok = fire(nav, ev); } finally { st.dispatching = false; }
        if (S.ongoing !== rec) { return null; }
        if (!ok) {
            abortOngoing(dom('AbortError', 'Navigation was aborted'));
            return null;
        }
        rec.intercepted = st.interception === 'intercepted';
        if (rec.intercepted) {
            var from = entries[current];
            rec.transitionCommitted = deferred();
            rec.transitionFinished = deferred();
            rec.transition = new NavigationTransition(TOKEN, { navigationType: o.type, from: from, committed: rec.transitionCommitted.promise, finished: rec.transitionFinished.promise });
            S.transition = rec.transition;
        }
        return rec;
    };

    // "update the navigation API entries for a same-document navigation" (§7.2.6.8), then the rest of the
    // navigation: committed, the intercept handlers, navigatesuccess / navigateerror, finished.
    var commitRec = function (rec) {
        var o = rec.o;
        var from = entries[current];
        var disposed = [];
        if (o.type === 'push') {
            disposed = entries.splice(current + 1);
            entries.push(newEntry(here(), null, o.state));
            current = entries.length - 1;
        } else if (o.type === 'replace') {
            disposed = [from];
            entries[current] = newEntry(here(), es(from).key, o.keepState ? es(from).state : o.state);
        } else if (o.type === 'reload') {
            if (o.state !== undefined || o.stateGiven) { es(from).state = o.state; }
        } else if (o.type === 'traverse') {
            var i = entries.indexOf(o.entry);
            if (i >= 0) { current = i; }
        }
        rec.committed = true;
        // A same-document navigation nobody intercepted has nothing left to abort.
        if (!rec.intercepted && S.ongoing === rec) { S.ongoing = null; }
        var st = NE.get(rec.event);
        if (st && rec.intercepted) { st.interception = 'committed'; }
        fire(nav, new NavigationCurrentEntryChangeEvent('currententrychange', { navigationType: o.type, from: from }));
        disposed.forEach(dispose);
        if (rec.transitionCommitted) { rec.transitionCommitted.resolve(); }
        if (rec.tracker && !rec.tracker.done) { rec.tracker.committed.resolve(entries[current]); }
        var promises = [];
        if (rec.intercepted && st) {
            st.handlers.forEach(function (h) {
                try { promises.push(Promise.resolve(h())); } catch (e) { promises.push(Promise.reject(e)); }
            });
        }
        var entry = entries[current];
        Promise.all(promises).then(function () {
            if (rec.settled || rec.ac.signal.aborted) { return; }
            if (S.ongoing === rec) { S.ongoing = null; }
            fire(nav, new Event('navigatesuccess'));
            settle(rec, true, entry);
        }, function (reason) {
            if (rec.settled || rec.ac.signal.aborted) { return; }
            if (S.ongoing === rec) { S.ongoing = null; }
            try { rec.ac.abort(reason); } catch (e) {}
            fire(nav, errorEvent(reason));
            settle(rec, false, reason);
        });
    };

    // The Rust hooks: a navigation the page performed (js/window.rs, js/history.rs). 0 cancel, 1 proceed,
    // 2 intercept.
    hide('__nav_begin', function (url, type, sameDoc) {
        url = String(url); type = String(type); sameDoc = !!sameDoc;
        if (S.inner > 0) { return 1; }
        init();
        var cur = here();
        var o = {
            url: url, type: type, sameDoc: sameDoc, state: undefined, info: undefined,
            hashChange: type !== 'traverse' && type !== 'reload' && url.indexOf('#') >= 0 && noFrag(url) === noFrag(cur) && url !== cur,
            keepState: type === 'replace',
        };
        if (type === 'traverse') {
            // The entry the traversal lands on: the one a traverseTo() asked for, else the nearest with the URL.
            var t = S.traversals.shift();
            var target = t ? t.entry : null;
            if (!target) {
                for (var d = 1; d < entries.length && !target; d++) {
                    if (entries[current - d] && es(entries[current - d]).url === url) { target = entries[current - d]; }
                    else if (entries[current + d] && es(entries[current + d]).url === url) { target = entries[current + d]; }
                }
            }
            if (!target) { return 1; }
            o.entry = target;
            o.state = es(target).state;
            o.tracker = t ? t.tracker : null;
            if (o.tracker) { o.info = o.tracker.info; }
        }
        var rec = fireNavigate(o);
        if (!rec) { return 0; }
        if (sameDoc || rec.intercepted) {
            rec.pending = true;
            S.pending = rec;
            return rec.intercepted ? 2 : 1;
        }
        return 1;
    });
    hide('__nav_commit', function () {
        if (S.inner > 0) { return; }
        var rec = S.pending;
        S.pending = null;
        if (rec) { rec.pending = false; commitRec(rec); }
    });

    // Our own navigations through Location / History, without re-entering the hooks.
    var inner = function (f) { S.inner++; try { return f(); } finally { S.inner--; } };

    class Navigation extends EventTarget {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } super(); }
        entries() { self(this); init(); return entries.slice(); }
        get currentEntry() { self(this); init(); return entries[current]; }
        updateCurrentEntry(options) {
            self(this);
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'updateCurrentEntry' on 'Navigation': 1 argument required, but only 0 present."); }
            requireObject(options, 'NavigationUpdateCurrentEntryOptions');
            if (!('state' in Object(options))) { throw new TypeError("Failed to execute 'updateCurrentEntry' on 'Navigation': required member state is undefined."); }
            init();
            var state = serialize(options.state);
            var from = entries[current];
            es(from).state = state;
            fire(nav, new NavigationCurrentEntryChangeEvent('currententrychange', { navigationType: null, from: from }));
        }
        get transition() { self(this); return S.transition; }
        get activation() { self(this); init(); return S.activation; }
        get canGoBack() { self(this); init(); return current > 0; }
        get canGoForward() { self(this); init(); return current < entries.length - 1; }
        navigate(url, options) {
            self(this);
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'navigate' on 'Navigation': 1 argument required, but only 0 present."); }
            url = str(url);
            var opts = options === undefined || options === null ? {} : options;
            var history = opts.history === undefined ? 'auto' : str(opts.history);
            if (['auto', 'push', 'replace'].indexOf(history) < 0) { throw new TypeError("The provided value '" + history + "' is not a valid enum value of type NavigationHistoryBehavior."); }
            init();
            var abs;
            try { abs = new URL(url, document.baseURI).href; } catch (e) { return early(dom('SyntaxError', "Failed to execute 'navigate' on 'Navigation': Invalid URL '" + url + "'.")); }
            if (/^javascript:/i.test(abs) && history === 'push') { return early(dom('NotSupportedError', "Failed to execute 'navigate' on 'Navigation': A javascript: URL cannot be navigated to with history 'push'.")); }
            var state;
            try { state = serialize(opts.state); } catch (e) { return early(e); }
            var cur = here();
            var type = history === 'auto' ? (abs === cur ? 'replace' : 'push') : history;
            var fragment = abs.indexOf('#') >= 0 && noFrag(abs) === noFrag(cur);
            var t = tracker(opts.info, state);
            var o = { url: abs, type: type, sameDoc: fragment, hashChange: fragment && abs !== cur, state: state, info: opts.info, tracker: t };
            var rec = fireNavigate(o);
            if (!rec) { return result(t); }
            if (rec.intercepted) {
                inner(function () {
                    if (type === 'replace') { G.history.replaceState(null, '', abs); } else { G.history.pushState(null, '', abs); }
                });
                commitRec(rec);
            } else if (fragment) {
                inner(function () { if (type === 'replace') { G.location.replace(abs); } else { G.location.assign(abs); } });
                commitRec(rec);
            } else {
                // A cross-document navigation: staged for the shell; this document's promises stay pending.
                inner(function () { if (type === 'replace') { G.location.replace(abs); } else { G.location.assign(abs); } });
            }
            return result(t);
        }
        reload(options) {
            self(this);
            var opts = options === undefined || options === null ? {} : options;
            init();
            var stateGiven = 'state' in Object(opts);
            var state;
            try { state = stateGiven ? serialize(opts.state) : es(entries[current]).state; } catch (e) { return early(e); }
            var t = tracker(opts.info, state);
            var o = { url: here(), type: 'reload', sameDoc: false, state: state, stateGiven: stateGiven, info: opts.info, tracker: t };
            var rec = fireNavigate(o);
            if (!rec) { return result(t); }
            if (rec.intercepted) { commitRec(rec); } else { inner(function () { G.location.reload(); }); }
            return result(t);
        }
        traverseTo(key, options) {
            self(this);
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'traverseTo' on 'Navigation': 1 argument required, but only 0 present."); }
            key = str(key);
            init();
            var opts = options === undefined || options === null ? {} : options;
            if (es(entries[current]).key === key) { return resolved(entries[current]); }
            var idx = -1;
            for (var i = 0; i < entries.length; i++) { if (es(entries[i]).key === key) { idx = i; break; } }
            if (idx < 0) { return early(dom('InvalidStateError', "Failed to execute 'traverseTo' on 'Navigation': Invalid key")); }
            var t = tracker(opts.info, undefined);
            S.traversals.push({ entry: entries[idx], tracker: t });
            inner(function () { G.history.go(idx - current); });
            return result(t);
        }
        back(options) {
            self(this);
            init();
            if (current <= 0) { return early(dom('InvalidStateError', "Failed to execute 'back' on 'Navigation': Cannot go back")); }
            return this.traverseTo(es(entries[current - 1]).key, options);
        }
        forward(options) {
            self(this);
            init();
            if (current >= entries.length - 1) { return early(dom('InvalidStateError', "Failed to execute 'forward' on 'Navigation': Cannot go forward")); }
            return this.traverseTo(es(entries[current + 1]).key, options);
        }
    }
    ['onnavigate', 'onnavigatesuccess', 'onnavigateerror', 'oncurrententrychange'].forEach(function (n) { handlerAttr(Navigation.prototype, n); });
    [Navigation, NavigationHistoryEntry, NavigationDestination, NavigateEvent, NavigationCurrentEntryChangeEvent, NavigationTransition, NavigationActivation].forEach(function (c) {
        Object.defineProperty(c.prototype, Symbol.toStringTag, { value: c.name, configurable: true });
        iface(c.name, c);
    });
    nav = new Navigation(TOKEN);
    // [Replaceable] readonly attribute Navigation navigation (HTML §7.2.6.1).
    Object.defineProperty(G, 'navigation', {
        get: function () { return nav; },
        set: function (v) { Object.defineProperty(G, 'navigation', { value: v, writable: true, enumerable: true, configurable: true }); },
        enumerable: true, configurable: true,
    });
})();
"#;
