//! Platform APIs that are *real*, not shaped-like-real: `URL` /
//! `URLSearchParams` over the same `url` crate the network stack parses
//! with, UTF-8 `TextEncoder`/`TextDecoder`, the `AbortController` object
//! graph, `DOMParser` over the same `kuchiki::parse_html` the `innerHTML`
//! setter uses, and `crypto` seeded from the OS entropy pool.
//!
//! The split follows `api::fetch`: anything that needs engine truth (URL
//! resolution, HTML parsing, entropy) is a small native; the WHATWG object
//! shape around it is a JS prelude. Every place the implementation stops
//! short of the spec records itself through `__ledger` or
//! `crate::ledger::record_js`, so a page that leaned on the missing part
//! shows up as a gap rather than as silence.

use boa_engine::{
    Context, JsValue,
    native_function::NativeFunction,
    object::{ObjectInitializer, builtins::JsArray},
    property::Attribute,
};
use kuchiki::traits::*;

fn str_arg(args: &[JsValue], i: usize, ctx: &mut Context) -> String {
    args.get(i)
        .cloned()
        .unwrap_or_default()
        .to_string(ctx)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

/// Serialises a parsed `Url` into the component bag the JS `URL` class
/// reads. Every field is what the `url` crate actually computed — none of
/// them are reconstructed by string surgery in JS.
fn components(u: &url::Url, ctx: &mut Context) -> JsValue {
    let s = |x: &str| boa_engine::string::JsString::from(x);
    let port = u.port().map(|p| p.to_string()).unwrap_or_default();
    let origin = match u.origin() {
        url::Origin::Tuple(..) => u.origin().ascii_serialization(),
        url::Origin::Opaque(_) => "null".to_string(),
    };
    let host = match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        (None, _) => String::new(),
    };
    let o = ObjectInitializer::new(ctx)
        .property(s("href"), s(u.as_str()), Attribute::all())
        .property(s("protocol"), s(&format!("{}:", u.scheme())), Attribute::all())
        .property(s("username"), s(u.username()), Attribute::all())
        .property(s("password"), s(u.password().unwrap_or("")), Attribute::all())
        .property(s("host"), s(&host), Attribute::all())
        .property(s("hostname"), s(u.host_str().unwrap_or("")), Attribute::all())
        .property(s("port"), s(&port), Attribute::all())
        .property(s("pathname"), s(u.path()), Attribute::all())
        .property(
            s("search"),
            s(&u.query().map(|q| format!("?{q}")).unwrap_or_default()),
            Attribute::all(),
        )
        .property(
            s("hash"),
            s(&u.fragment().map(|f| format!("#{f}")).unwrap_or_default()),
            Attribute::all(),
        )
        .property(s("origin"), s(&origin), Attribute::all())
        .build();
    o.into()
}

/// Reads `n` bytes from the OS entropy pool. `/dev/urandom` is the real
/// source and is present on every platform this engine runs on (and inside
/// the UnaOS userspace); when it cannot be read the fallback mixes
/// `RandomState` — which is itself seeded from the OS — with the monotonic
/// clock, and the substitution is ledgered so a page that needed
/// cryptographic quality can see it did not get it. No fixed fill, ever.
fn entropy(n: usize) -> Vec<u8> {
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut buf = vec![0u8; n];
        if f.read_exact(&mut buf).is_ok() {
            return buf;
        }
    }
    crate::ledger::record_js("crypto-entropy-fallback-not-urandom");
    use std::hash::{BuildHasher, Hash, Hasher};
    let mut out = Vec::with_capacity(n);
    let state = std::collections::hash_map::RandomState::new();
    let mut counter: u64 = 0;
    while out.len() < n {
        let mut h = state.build_hasher();
        counter = counter.wrapping_add(1);
        counter.hash(&mut h);
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            .hash(&mut h);
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(n);
    out
}

pub fn init(context: &mut Context) {
    // __url_parse(input, base) -> components | null. Relative resolution is
    // the `url` crate's, so `new URL('../a', 'https://h/x/y/z')` lands
    // exactly where the network stack would fetch it from.
    let parse = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let input = str_arg(args, 0, ctx);
        let base = args.get(1).cloned().unwrap_or_default();
        let parsed = if base.is_undefined() || base.is_null() {
            url::Url::parse(&input)
        } else {
            let base = str_arg(args, 1, ctx);
            match url::Url::parse(&base) {
                Ok(b) => b.join(&input),
                Err(e) => Err(e),
            }
        };
        match parsed {
            Ok(u) => Ok(components(&u, ctx)),
            Err(_) => Ok(JsValue::null()),
        }
    });
    let _ = context.register_global_callable("__url_parse".into(), 2, parse);

    // __url_set(href, part, value) -> components | null. Component writes
    // go through the crate's own setters, which re-normalise the whole URL
    // (a `hostname` write re-serialises the authority, a `protocol` write
    // is refused when the scheme change is not permitted) instead of being
    // spliced into the href string.
    let set = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let href = str_arg(args, 0, ctx);
        let part = str_arg(args, 1, ctx);
        let value = str_arg(args, 2, ctx);
        let Ok(mut u) = url::Url::parse(&href) else { return Ok(JsValue::null()) };
        let ok = match part.as_str() {
            "protocol" => u.set_scheme(value.trim_end_matches(':')).is_ok(),
            "username" => u.set_username(&value).is_ok(),
            "password" => u
                .set_password(if value.is_empty() { None } else { Some(&value) })
                .is_ok(),
            "hostname" => u.set_host(if value.is_empty() { None } else { Some(&value) }).is_ok(),
            "host" => {
                let (h, p) = match value.rsplit_once(':') {
                    Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
                    None => (value.clone(), None),
                };
                u.set_host(if h.is_empty() { None } else { Some(&h) }).is_ok() && u.set_port(p).is_ok()
            }
            "port" => u.set_port(value.parse::<u16>().ok()).is_ok(),
            "pathname" => {
                u.set_path(&value);
                true
            }
            "search" => {
                let q = value.trim_start_matches('?');
                u.set_query(if q.is_empty() { None } else { Some(q) });
                true
            }
            "hash" => {
                let f = value.trim_start_matches('#');
                u.set_fragment(if f.is_empty() { None } else { Some(f) });
                true
            }
            _ => false,
        };
        if !ok {
            // A refused component write is exactly what a browser does
            // (silently), but it is worth seeing when a page depended on it.
            crate::ledger::record_js(&format!("URL-set-refused:{part}"));
        }
        Ok(components(&u, ctx))
    });
    let _ = context.register_global_callable("__url_set".into(), 3, set);

    // __random_bytes(n) -> array of n bytes from the OS entropy pool.
    let rand = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let n = args
            .first()
            .cloned()
            .unwrap_or_default()
            .to_number(ctx)
            .unwrap_or(0.0)
            .max(0.0)
            .min(65536.0) as usize;
        let bytes = entropy(n);
        let arr = JsArray::from_iter(bytes.into_iter().map(|b| JsValue::from(b as u32)), ctx);
        Ok(arr.into())
    });
    let _ = context.register_global_callable("__random_bytes".into(), 1, rand);

    // __parse_document(html) -> wrapped document node. Same parser, same
    // node wrapper, same query bindings as the live tree: a DOMParser
    // document supports querySelector/getElementById/textContent for real.
    let parse_doc = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let html = str_arg(args, 0, ctx);
        let doc = kuchiki::parse_html().one(html);
        let mut extras: Vec<(&str, kuchiki::NodeRef)> = Vec::new();
        for (prop, sel) in [("documentElement", "html"), ("body", "body"), ("head", "head")] {
            if let Ok(mut m) = doc.select(sel) {
                if let Some(el) = m.next() {
                    extras.push((prop, el.as_node().clone()));
                }
            }
        }
        let wrapped = crate::js::Engine::wrap_node(ctx, doc);
        if let Some(obj) = wrapped.as_object() {
            for (prop, node) in extras {
                let w = crate::js::Engine::wrap_node(ctx, node);
                let _ = obj.set(boa_engine::string::JsString::from(prop), w, false, ctx);
            }
        }
        Ok(wrapped)
    });
    let _ = context.register_global_callable("__parse_document".into(), 1, parse_doc);

    let _ = context.eval(boa_engine::Source::from_bytes(PRELUDE));
}

/// The WHATWG object shapes over the natives above.
const PRELUDE: &str = r#"

// ---- URLSearchParams -------------------------------------------------
// Real application/x-www-form-urlencoded semantics: '+' decodes to space,
// insertion order is preserved, and repeated names are kept (getAll).
(function () {
    var enc = function (s) {
        return encodeURIComponent(String(s))
            .replace(/%20/g, '+')
            .replace(/[!'()~]/g, function (c) {
                return '%' + c.charCodeAt(0).toString(16).toUpperCase();
            });
    };
    var dec = function (s) {
        try { return decodeURIComponent(String(s).replace(/\+/g, ' ')); }
        catch (e) { return String(s).replace(/\+/g, ' '); }
    };

    function URLSearchParams(init) {
        this._p = [];
        this._onchange = null;
        if (init === undefined || init === null) { return; }
        if (typeof init === 'string') {
            var q = init.charAt(0) === '?' ? init.slice(1) : init;
            if (q.length) {
                var parts = q.split('&');
                for (var i = 0; i < parts.length; i++) {
                    if (!parts[i].length) { continue; }
                    var j = parts[i].indexOf('=');
                    if (j < 0) { this._p.push([dec(parts[i]), '']); }
                    else { this._p.push([dec(parts[i].slice(0, j)), dec(parts[i].slice(j + 1))]); }
                }
            }
        } else if (Array.isArray(init)) {
            for (var k = 0; k < init.length; k++) {
                this._p.push([String(init[k][0]), String(init[k][1])]);
            }
        } else if (init instanceof URLSearchParams) {
            for (var m = 0; m < init._p.length; m++) {
                this._p.push([init._p[m][0], init._p[m][1]]);
            }
        } else if (typeof init === 'object') {
            for (var key in init) {
                if (Object.prototype.hasOwnProperty.call(init, key)) {
                    this._p.push([String(key), String(init[key])]);
                }
            }
        }
    }
    URLSearchParams.prototype._changed = function () {
        if (this._onchange) { this._onchange(this.toString()); }
    };
    URLSearchParams.prototype.get = function (n) {
        n = String(n);
        for (var i = 0; i < this._p.length; i++) { if (this._p[i][0] === n) { return this._p[i][1]; } }
        return null;
    };
    URLSearchParams.prototype.getAll = function (n) {
        n = String(n);
        var out = [];
        for (var i = 0; i < this._p.length; i++) { if (this._p[i][0] === n) { out.push(this._p[i][1]); } }
        return out;
    };
    URLSearchParams.prototype.has = function (n) { return this.get(n) !== null; };
    URLSearchParams.prototype.append = function (n, v) {
        this._p.push([String(n), String(v)]); this._changed();
    };
    URLSearchParams.prototype.set = function (n, v) {
        n = String(n); v = String(v);
        var seen = false, out = [];
        for (var i = 0; i < this._p.length; i++) {
            if (this._p[i][0] !== n) { out.push(this._p[i]); }
            else if (!seen) { seen = true; out.push([n, v]); }
        }
        if (!seen) { out.push([n, v]); }
        this._p = out; this._changed();
    };
    URLSearchParams.prototype['delete'] = function (n) {
        n = String(n);
        var out = [];
        for (var i = 0; i < this._p.length; i++) { if (this._p[i][0] !== n) { out.push(this._p[i]); } }
        this._p = out; this._changed();
    };
    URLSearchParams.prototype.sort = function () {
        this._p.sort(function (a, b) { return a[0] < b[0] ? -1 : (a[0] > b[0] ? 1 : 0); });
        this._changed();
    };
    URLSearchParams.prototype.forEach = function (cb, thisArg) {
        for (var i = 0; i < this._p.length; i++) { cb.call(thisArg, this._p[i][1], this._p[i][0], this); }
    };
    // entries/keys/values return arrays: boa has generators, but an array is
    // iterable with for..of and spread and is what every real consumer of
    // these does with them. Array-vs-iterator identity checks would differ;
    // no page has been seen to make one.
    URLSearchParams.prototype.entries = function () {
        var out = [];
        for (var i = 0; i < this._p.length; i++) { out.push([this._p[i][0], this._p[i][1]]); }
        return out;
    };
    URLSearchParams.prototype.keys = function () {
        return this._p.map(function (e) { return e[0]; });
    };
    URLSearchParams.prototype.values = function () {
        return this._p.map(function (e) { return e[1]; });
    };
    URLSearchParams.prototype.toString = function () {
        var out = [];
        for (var i = 0; i < this._p.length; i++) {
            out.push(enc(this._p[i][0]) + '=' + enc(this._p[i][1]));
        }
        return out.join('&');
    };
    Object.defineProperty(URLSearchParams.prototype, 'size', {
        configurable: true,
        get: function () { return this._p.length; },
    });
    try {
        URLSearchParams.prototype[Symbol.iterator] = function () {
            return this.entries()[Symbol.iterator]();
        };
    } catch (e) {}
    globalThis.URLSearchParams = URLSearchParams;

    // ---- URL ---------------------------------------------------------
    // Every component comes from the `url` crate; component writes go back
    // through it and re-normalise. searchParams is live-linked both ways,
    // as in the platform.
    function URL(input, base) {
        var c = __url_parse(String(input), base === undefined || base === null ? undefined : String(base));
        if (!c) { throw new TypeError('Failed to construct URL: Invalid URL: ' + input); }
        this._c = c;
        this._sp = new URLSearchParams(c.search);
        var self = this;
        this._sp._onchange = function (q) {
            self._c = __url_set(self._c.href, 'search', q);
        };
    }
    // Rebuilds the live searchParams link after the query string changed
    // out from under it (an href or search write).
    var relink = function (u) {
        u._sp = new URLSearchParams(u._c.search);
        u._sp._onchange = function (q) { u._c = __url_set(u._c.href, 'search', q); };
    };
    var PARTS = ['protocol', 'username', 'password', 'host', 'hostname',
                 'port', 'pathname', 'search', 'hash'];
    for (var pi = 0; pi < PARTS.length; pi++) {
        (function (part) {
            Object.defineProperty(URL.prototype, part, {
                configurable: true, enumerable: true,
                get: function () { return this._c[part]; },
                set: function (v) {
                    var next = __url_set(this._c.href, part, String(v));
                    if (next) { this._c = next; }
                    if (part === 'search') { relink(this); }
                },
            });
        })(PARTS[pi]);
    }
    Object.defineProperty(URL.prototype, 'href', {
        configurable: true, enumerable: true,
        get: function () { return this._c.href; },
        set: function (v) {
            var c = __url_parse(String(v), undefined);
            if (!c) { throw new TypeError('Invalid URL: ' + v); }
            this._c = c;
            relink(this);
        },
    });
    Object.defineProperty(URL.prototype, 'origin', {
        configurable: true, enumerable: true,
        get: function () { return this._c.origin; },
    });
    Object.defineProperty(URL.prototype, 'searchParams', {
        configurable: true, enumerable: true,
        get: function () { return this._sp; },
    });
    URL.prototype.toString = function () { return this._c.href; };
    URL.prototype.toJSON = function () { return this._c.href; };
    URL.canParse = function (input, base) {
        return !!__url_parse(String(input), base === undefined ? undefined : String(base));
    };
    URL.parse = function (input, base) {
        try { return new URL(input, base); } catch (e) { return null; }
    };
    // Object URLs need a blob store this engine does not have; the handle
    // is unique and revocable but dereferences to nothing.
    var __blobSeq = 0;
    URL.createObjectURL = function () {
        __ledger('URL.createObjectURL-no-blob-store');
        return 'blob:' + (globalThis.location ? location.origin : 'null') + '/' + (++__blobSeq);
    };
    URL.revokeObjectURL = function () {};
    globalThis.URL = URL;
})();

// ---- TextEncoder / TextDecoder ---------------------------------------
// Real UTF-8 in both directions, surrogate pairs included, lone surrogates
// replaced with U+FFFD exactly as the spec requires.
(function () {
    var HAS_U8 = (typeof Uint8Array !== 'undefined');
    if (!HAS_U8) { __ledger('TextEncoder-no-Uint8Array-plain-array'); }

    function encodeUtf8(str) {
        var s = String(str), bytes = [];
        for (var i = 0; i < s.length; i++) {
            var cp = s.charCodeAt(i);
            if (cp >= 0xD800 && cp <= 0xDBFF && i + 1 < s.length) {
                var lo = s.charCodeAt(i + 1);
                if (lo >= 0xDC00 && lo <= 0xDFFF) {
                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    i++;
                } else { cp = 0xFFFD; }
            } else if (cp >= 0xD800 && cp <= 0xDFFF) { cp = 0xFFFD; }
            if (cp < 0x80) { bytes.push(cp); }
            else if (cp < 0x800) { bytes.push(0xC0 | (cp >> 6), 0x80 | (cp & 63)); }
            else if (cp < 0x10000) {
                bytes.push(0xE0 | (cp >> 12), 0x80 | ((cp >> 6) & 63), 0x80 | (cp & 63));
            } else {
                bytes.push(0xF0 | (cp >> 18), 0x80 | ((cp >> 12) & 63),
                           0x80 | ((cp >> 6) & 63), 0x80 | (cp & 63));
            }
        }
        return bytes;
    }
    function decodeUtf8(bytes) {
        var out = '', i = 0, n = bytes.length;
        while (i < n) {
            var b = bytes[i++] & 255, cp, need;
            if (b < 0x80) { cp = b; need = 0; }
            else if ((b & 0xE0) === 0xC0) { cp = b & 31; need = 1; }
            else if ((b & 0xF0) === 0xE0) { cp = b & 15; need = 2; }
            else if ((b & 0xF8) === 0xF0) { cp = b & 7; need = 3; }
            else { out += '�'; continue; }
            var ok = true;
            for (var k = 0; k < need; k++) {
                if (i >= n) { ok = false; break; }
                var c = bytes[i] & 255;
                if ((c & 0xC0) !== 0x80) { ok = false; break; }
                cp = (cp << 6) | (c & 63); i++;
            }
            if (!ok) { out += '�'; continue; }
            if (cp > 0x10FFFF) { out += '�'; }
            else if (cp > 0xFFFF) {
                cp -= 0x10000;
                out += String.fromCharCode(0xD800 + (cp >> 10), 0xDC00 + (cp & 1023));
            } else { out += String.fromCharCode(cp); }
        }
        return out;
    }

    function TextEncoder() { this.encoding = 'utf-8'; }
    TextEncoder.prototype.encode = function (str) {
        var bytes = encodeUtf8(str === undefined ? '' : str);
        return HAS_U8 ? new Uint8Array(bytes) : bytes;
    };
    TextEncoder.prototype.encodeInto = function (str, dest) {
        var bytes = encodeUtf8(str === undefined ? '' : str);
        var n = Math.min(bytes.length, dest.length);
        for (var i = 0; i < n; i++) { dest[i] = bytes[i]; }
        return { read: n === bytes.length ? String(str).length : 0, written: n };
    };
    globalThis.TextEncoder = TextEncoder;

    function TextDecoder(label) {
        this.encoding = (label ? String(label) : 'utf-8').toLowerCase();
        this.fatal = false;
        this.ignoreBOM = false;
        if (this.encoding !== 'utf-8' && this.encoding !== 'utf8' && this.encoding !== 'unicode-1-1-utf-8') {
            __ledger('TextDecoder-non-utf8-decoded-as-utf8:' + this.encoding);
            this.encoding = 'utf-8';
        }
    }
    TextDecoder.prototype.decode = function (input) {
        if (input === undefined || input === null) { return ''; }
        var src = input;
        if (src.buffer !== undefined && src.BYTES_PER_ELEMENT === undefined) {
            // Raw ArrayBuffer-alike without a view: wrap it if we can.
            try { src = new Uint8Array(input); } catch (e) {}
        } else if (typeof ArrayBuffer !== 'undefined' && input instanceof ArrayBuffer) {
            try { src = new Uint8Array(input); } catch (e) {}
        }
        var bytes = [];
        for (var i = 0; i < src.length; i++) { bytes.push(src[i] & 255); }
        if (!this.ignoreBOM && bytes.length >= 3 &&
            bytes[0] === 0xEF && bytes[1] === 0xBB && bytes[2] === 0xBF) {
            bytes = bytes.slice(3);
        }
        return decodeUtf8(bytes);
    };
    globalThis.TextDecoder = TextDecoder;
})();

// ---- AbortController / AbortSignal -----------------------------------
// A real object graph: aborting flips `aborted`, records the reason, and
// dispatches an `abort` event to `onabort` and every registered listener,
// once and only once. Not wired into fetch — that integration is ledgered
// at the point a signal reaches fetch, not faked here.
(function () {
    function AbortSignal() {
        this.aborted = false;
        this.reason = undefined;
        this.onabort = null;
        this._l = [];
    }
    AbortSignal.prototype.addEventListener = function (type, cb) {
        if (type === 'abort' && cb) { this._l.push(cb); }
    };
    AbortSignal.prototype.removeEventListener = function (type, cb) {
        if (type !== 'abort') { return; }
        var i = this._l.indexOf(cb);
        if (i >= 0) { this._l.splice(i, 1); }
    };
    AbortSignal.prototype.dispatchEvent = function (ev) {
        if (this.onabort) { try { this.onabort.call(this, ev); } catch (e) {} }
        var l = this._l.slice();
        for (var i = 0; i < l.length; i++) { try { l[i].call(this, ev); } catch (e) {} }
        return true;
    };
    AbortSignal.prototype.throwIfAborted = function () {
        if (this.aborted) { throw this.reason; }
    };
    AbortSignal.abort = function (reason) {
        var s = new AbortSignal();
        s.aborted = true;
        s.reason = reason === undefined ? new Error('AbortError') : reason;
        return s;
    };
    AbortSignal.timeout = function (ms) {
        var s = new AbortSignal();
        setTimeout(function () { __abortSignal(s, new Error('TimeoutError')); }, ms);
        return s;
    };
    AbortSignal.any = function (signals) {
        var out = new AbortSignal();
        for (var i = 0; i < signals.length; i++) {
            if (signals[i].aborted) { __abortSignal(out, signals[i].reason); return out; }
            (function (s) {
                s.addEventListener('abort', function () { __abortSignal(out, s.reason); });
            })(signals[i]);
        }
        return out;
    };
    globalThis.AbortSignal = AbortSignal;

    globalThis.__abortSignal = function (signal, reason) {
        if (signal.aborted) { return; }
        signal.aborted = true;
        signal.reason = reason === undefined ? new Error('AbortError') : reason;
        var ev = { type: 'abort', target: signal, currentTarget: signal };
        signal.dispatchEvent(ev);
    };

    function AbortController() { this.signal = new AbortSignal(); }
    AbortController.prototype.abort = function (reason) { __abortSignal(this.signal, reason); };
    globalThis.AbortController = AbortController;

    // fetch does not observe signals: the network call is synchronous
    // engine-side and has already completed by the time script could abort
    // it. Recorded where the signal is handed over, so the gap is visible
    // on exactly the pages that rely on it.
    if (typeof globalThis.fetch === 'function') {
        var __rawFetch = globalThis.fetch;
        globalThis.fetch = function (input, opts) {
            if (opts && opts.signal) { __ledger('abortsignal-not-wired-to-fetch'); }
            return __rawFetch.apply(this, arguments);
        };
    }
})();

// ---- DOMParser / XMLSerializer ---------------------------------------
(function () {
    function DOMParser() {}
    DOMParser.prototype.parseFromString = function (str, type) {
        var t = type ? String(type).toLowerCase() : 'text/html';
        if (t !== 'text/html') {
            // The HTML parser is error-recovering and namespace-flat; an XML
            // document parsed through it differs in casing and well-formed
            // error reporting.
            __ledger('DOMParser-unsupported-parsed-as-html:' + t);
        }
        return __parse_document(String(str === undefined ? '' : str));
    };
    globalThis.DOMParser = DOMParser;

    function XMLSerializer() {}
    XMLSerializer.prototype.serializeToString = function (node) {
        if (!node) { return ''; }
        if (node.outerHTML !== undefined && node.outerHTML !== null) { return node.outerHTML; }
        if (node.documentElement) { return node.documentElement.outerHTML || ''; }
        return node.innerHTML || '';
    };
    globalThis.XMLSerializer = XMLSerializer;
})();

// ---- crypto ----------------------------------------------------------
// getRandomValues fills from the OS entropy pool; randomUUID is a real
// v4 UUID built on the same bytes.
(function () {
    var crypto = globalThis.crypto || {};
    crypto.getRandomValues = function (arr) {
        if (!arr || typeof arr.length !== 'number') {
            throw new TypeError('getRandomValues expects an integer-typed array');
        }
        if (typeof arr.BYTES_PER_ELEMENT === 'number' && arr.BYTES_PER_ELEMENT === 8) {
            // BigInt64/BigUint64 need BigInt writes; not filled rather than
            // filled wrongly.
            __ledger('crypto.getRandomValues-64bit-array-unfilled');
            return arr;
        }
        var width = arr.BYTES_PER_ELEMENT || 1;
        var bytes = __random_bytes(arr.length * width);
        for (var i = 0; i < arr.length; i++) {
            var v = 0;
            for (var b = 0; b < width; b++) { v = v * 256 + bytes[i * width + b]; }
            // Typed-array assignment truncates to the element width; a
            // plain Array keeps the full unsigned value, which is what a
            // caller of a non-typed array can expect at best.
            arr[i] = v;
        }
        return arr;
    };
    crypto.randomUUID = function () {
        var b = __random_bytes(16);
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        var hex = [];
        for (var i = 0; i < 16; i++) { hex.push((b[i] + 256).toString(16).slice(1)); }
        return hex.slice(0, 4).join('') + '-' + hex.slice(4, 6).join('') + '-' +
               hex.slice(6, 8).join('') + '-' + hex.slice(8, 10).join('') + '-' +
               hex.slice(10, 16).join('');
    };
    if (!crypto.subtle) {
        Object.defineProperty(crypto, 'subtle', {
            configurable: true,
            get: function () {
                __ledger('crypto.subtle-unimplemented');
                return undefined;
            },
        });
    }
    globalThis.crypto = crypto;
})();
"#;
