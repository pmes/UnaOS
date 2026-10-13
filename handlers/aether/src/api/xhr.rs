//! `XMLHttpRequest` (the XHR Standard) — AETHERNET, LEDGER SR70.
//!
//! Asynchronous requests run on `api::fetch`'s streaming worker (`__net_http`): the readyState machine
//! advances as the head and each body chunk arrive (OPENED → HEADERS_RECEIVED → LOADING → DONE, a
//! `readystatechange` at each step and a `progress` per chunk at most every 50 ms), with `load` / `error` /
//! `abort` / `timeout` and `loadend` at the end. Synchronous requests (`open(…, false)`) join the
//! exchange on `api::fetch::http_sync`. Upload progress events are not produced (`upload` is an inert
//! `XMLHttpRequestUpload`).
//!
//! Not provided: `responseType = "document"` / `responseXML` (Aether parses HTML documents only through
//! the page loader; both answer null and ledger it), and charsets other than UTF-8 for `responseText`
//! (decoded as UTF-8, ledgered).

/// The `XMLHttpRequest`, `XMLHttpRequestEventTarget` and `XMLHttpRequestUpload` interfaces.
pub const PRELUDE: &str = r#"
(function () {
    var ST = new WeakMap();
    var st = function (o) {
        var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
    var baseURL = function () { try { return document.baseURI; } catch (e) { return location.href; } };
    var FORBIDDEN = ['accept-charset', 'accept-encoding', 'access-control-request-headers', 'access-control-request-method',
        'connection', 'content-length', 'cookie', 'cookie2', 'date', 'dnt', 'expect', 'host', 'keep-alive', 'origin',
        'referer', 'set-cookie', 'te', 'trailer', 'transfer-encoding', 'upgrade', 'via'];
    var NORMALIZE = ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'];
    var dom = function (m, name) { return new DOMException(m, name); };
    var now = function () { return Date.now(); };

    var creating = false;
    class XMLHttpRequestEventTarget extends EventTarget {
        constructor() {
            if (!creating) { throw new TypeError('Illegal constructor'); }
            super();
        }
    }
    __net_handlers(XMLHttpRequestEventTarget.prototype, ['loadstart', 'progress', 'abort', 'error', 'load', 'timeout', 'loadend']);
    class XMLHttpRequestUpload extends XMLHttpRequestEventTarget {
        constructor() {
            if (!creating) { throw new TypeError('Illegal constructor'); }
            super();
        }
    }

    var fresh = function (s) {
        s.status = 0; s.statusText = ''; s.url = ''; s.headers = []; s.chunks = []; s.received = 0; s.total = 0;
        s.error = true; s.cache = undefined; s.cacheKind = undefined; s.text = null;
    };
    class XMLHttpRequest extends XMLHttpRequestEventTarget {
        constructor() {
            creating = true;
            var up;
            try { super(); up = new XMLHttpRequestUpload(); } finally { creating = false; }
            var s = { readyState: 0, method: 'GET', reqURL: '', url: '', async: true, user: null, password: null, reqHeaders: [],
                      send: false, timeout: 0, withCredentials: false, responseType: '', mime: null, id: 0, timer: 0,
                      started: 0, lastProgress: 0, upload: up, gen: 0 };
            fresh(s);
            ST.set(this, s);
        }
        get readyState() { return st(this).readyState; }
        get timeout() { return st(this).timeout; }
        set timeout(v) {
            var s = st(this);
            if (!s.async && s.readyState === 1 && typeof document !== 'undefined') {
                throw dom("Failed to set the 'timeout' property on 'XMLHttpRequest': Timeouts cannot be set for synchronous requests made from a document.", 'InvalidAccessError');
            }
            s.timeout = Number(v) >>> 0;
            if (s.send && s.async) { armTimer(this); }
        }
        get withCredentials() { return st(this).withCredentials; }
        set withCredentials(v) {
            var s = st(this);
            if ((s.readyState !== 0 && s.readyState !== 1) || s.send) {
                throw dom("Failed to set the 'withCredentials' property on 'XMLHttpRequest': The value may only be set if the object's state is UNSENT or OPENED.", 'InvalidStateError');
            }
            s.withCredentials = !!v;
        }
        get upload() { return st(this).upload; }
        get responseURL() { return st(this).url.replace(/#.*$/, ''); }
        get status() { var s = st(this); return s.error ? 0 : s.status; }
        get statusText() { var s = st(this); return s.error ? '' : s.statusText; }
        get responseType() { return st(this).responseType; }
        set responseType(v) {
            var s = st(this);
            v = String(v);
            if (['', 'arraybuffer', 'blob', 'document', 'json', 'text'].indexOf(v) < 0) { return; }
            if (s.readyState === 3 || s.readyState === 4) {
                throw dom("Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be set if the object's state is LOADING or DONE.", 'InvalidStateError');
            }
            if (!s.async && s.readyState === 1 && typeof document !== 'undefined') {
                throw dom("Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be changed for synchronous requests made from a document.", 'InvalidAccessError');
            }
            if (v === 'document') { __ledger('xhr-responseType-document-null'); }
            s.responseType = v;
        }
        get responseText() {
            var s = st(this);
            if (s.responseType !== '' && s.responseType !== 'text') {
                throw dom("Failed to read the 'responseText' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'text' (was '" + s.responseType + "').", 'InvalidStateError');
            }
            if (s.readyState !== 3 && s.readyState !== 4) { return ''; }
            return textOf(s);
        }
        get responseXML() {
            var s = st(this);
            if (s.responseType !== '' && s.responseType !== 'document') {
                throw dom("Failed to read the 'responseXML' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'document' (was '" + s.responseType + "').", 'InvalidStateError');
            }
            if (s.readyState === 4 && !s.error) { __ledger('xhr-responseXML-null'); }
            return null;
        }
        get response() {
            var s = st(this);
            if (s.responseType === '' || s.responseType === 'text') {
                if (s.readyState !== 3 && s.readyState !== 4) { return ''; }
                return textOf(s);
            }
            if (s.readyState !== 4 || s.error) { return null; }
            if (s.cacheKind === s.responseType) { return s.cache; }
            var bytes = all(s), v = null;
            if (s.responseType === 'arraybuffer') { v = bytes.buffer; }
            else if (s.responseType === 'blob') { v = new Blob([bytes], { type: mimeOf(s) || '' }); }
            else if (s.responseType === 'json') {
                try {
                    var t = new TextDecoder().decode(bytes);
                    v = JSON.parse(t);
                } catch (e) { v = null; }
            }
            s.cache = v; s.cacheKind = s.responseType;
            return v;
        }
        open(method, url, async, user, password) {
            var s = st(this);
            if (arguments.length < 2) { throw new TypeError("Failed to execute 'open' on 'XMLHttpRequest': 2 arguments required, but only " + arguments.length + ' present.'); }
            method = String(method);
            if (!TOKEN.test(method)) { throw dom("Failed to execute 'open' on 'XMLHttpRequest': '" + method + "' is not a valid HTTP method.", 'SyntaxError'); }
            var up = method.toUpperCase();
            if (up === 'CONNECT' || up === 'TRACE' || up === 'TRACK') { throw dom("Failed to execute 'open' on 'XMLHttpRequest': '" + method + "' HTTP method is unsupported.", 'SecurityError'); }
            if (NORMALIZE.indexOf(up) >= 0) { method = up; }
            var u;
            try { u = new URL(String(url), baseURL()); }
            catch (e) { throw dom("Failed to execute 'open' on 'XMLHttpRequest': Invalid URL", 'SyntaxError'); }
            var isAsync = arguments.length < 3 ? true : !!async;
            if (arguments.length >= 4 && user !== undefined && user !== null) { u.username = String(user); }
            if (arguments.length >= 5 && password !== undefined && password !== null) { u.password = String(password); }
            if (!isAsync && typeof document !== 'undefined' && (s.timeout !== 0 || s.responseType !== '')) {
                throw dom("Failed to execute 'open' on 'XMLHttpRequest': Synchronous requests from a document must not set a response type or a timeout.", 'InvalidAccessError');
            }
            terminate(s);
            s.method = method; s.reqURL = u.href; s.async = isAsync; s.reqHeaders = []; s.send = false;
            // AETHERWPT3 (SR138): parsing a blob: URL attaches its entry (File API §8.3.4).
            s.blobEntry = /^blob:/i.test(u.href) && typeof __blob_url_resolve === 'function' ? __blob_url_resolve(u.href, __here_origin()) : null;
            fresh(s);
            if (s.readyState !== 1) {
                s.readyState = 1;
                __net_fire(this, 'Event', 'readystatechange', undefined);
            }
        }
        setRequestHeader(name, value) {
            var s = st(this);
            if (arguments.length < 2) { throw new TypeError("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': 2 arguments required, but only " + arguments.length + ' present.'); }
            if (s.readyState !== 1 || s.send) { throw dom("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError'); }
            name = String(name);
            value = String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
            if (!TOKEN.test(name)) { throw dom("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': '" + name + "' is not a valid HTTP header field name.", 'SyntaxError'); }
            if (/[\0\r\n]/.test(value) || /[^\u0000-ÿ]/.test(value)) { throw dom("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': '" + value + "' is not a valid HTTP header field value.", 'SyntaxError'); }
            var l = name.toLowerCase();
            if (FORBIDDEN.indexOf(l) >= 0 || l.indexOf('proxy-') === 0 || l.indexOf('sec-') === 0) { return; }
            for (var i = 0; i < s.reqHeaders.length; i++) {
                if (s.reqHeaders[i][0].toLowerCase() === l) { s.reqHeaders[i][1] += ', ' + value; return; }
            }
            s.reqHeaders.push([name, value]);
        }
        getResponseHeader(name) {
            var s = st(this);
            if (s.readyState < 2 || s.error) { return null; }
            var l = String(name).toLowerCase();
            if (l === 'set-cookie' || l === 'set-cookie2') { return null; }
            var vals = [];
            for (var i = 0; i + 1 < s.headers.length; i += 2) { if (String(s.headers[i]).toLowerCase() === l) { vals.push(s.headers[i + 1]); } }
            return vals.length ? vals.join(', ') : null;
        }
        getAllResponseHeaders() {
            var s = st(this);
            if (s.readyState < 2 || s.error) { return ''; }
            var names = [], map = {};
            for (var i = 0; i + 1 < s.headers.length; i += 2) {
                var l = String(s.headers[i]).toLowerCase();
                if (l === 'set-cookie' || l === 'set-cookie2') { continue; }
                if (!(l in map)) { names.push(l); map[l] = []; }
                map[l].push(s.headers[i + 1]);
            }
            names.sort();
            return names.map(function (n) { return n + ': ' + map[n].join(', ') + '\r\n'; }).join('');
        }
        overrideMimeType(mime) {
            var s = st(this);
            if (s.readyState === 3 || s.readyState === 4) { throw dom("Failed to execute 'overrideMimeType' on 'XMLHttpRequest': MimeType cannot be overridden when the state is LOADING or DONE.", 'InvalidStateError'); }
            mime = String(mime);
            s.mime = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+\/[!#$%&'*+\-.^_`|~0-9A-Za-z]+/.test(mime.trim()) ? mime.trim() : 'application/octet-stream';
        }
        send(body) {
            var s = st(this), self = this;
            if (s.readyState !== 1) { throw dom("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError'); }
            if (s.send) { throw dom("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError'); }
            var bytes = null, ctype = null;
            if (s.method !== 'GET' && s.method !== 'HEAD' && body !== undefined && body !== null) {
                if (typeof Document !== 'undefined' && body instanceof Document) {
                    bytes = new TextEncoder().encode(body.documentElement ? body.documentElement.outerHTML : '');
                    ctype = 'text/html;charset=UTF-8';
                } else {
                    var ex = __extract_body(body);
                    if (ex[0]) { __ledger('xhr-stream-body'); bytes = new Uint8Array(0); }
                    else { bytes = ex[1]; ctype = ex[2]; }
                }
            }
            var flat = [], hasType = false;
            s.reqHeaders.forEach(function (h) {
                if (h[0].toLowerCase() === 'content-type') {
                    hasType = true;
                    // A UTF-8 body declares its charset as UTF-8 (XHR §3.5.6 step 4.4).
                    if (ctype && /charset=/i.test(h[1]) && typeof body === 'string') {
                        h = [h[0], h[1].replace(/charset=[^;]*/i, 'charset=UTF-8')];
                    }
                }
                flat.push(h[0], h[1]);
            });
            if (!hasType && ctype) { flat.push('Content-Type', ctype); }
            // XHR §3.5.6 (AETHERDEBT SR93): mode cors, credentials 'include' when withCredentials is
            // set, else 'same-origin' — the policy pseudo-fields the worker lifts off.
            flat.push(':aether-mode', 'cors', ':aether-credentials', s.withCredentials ? 'include' : 'same-origin');
            var data = null;
            if (/^data:/i.test(s.reqURL)) {
                // A data: URL is fetched without a network (Fetch §4.2 "scheme fetch").
                try { data = __data_url(s.reqURL); } catch (e) { data = null; }
                data = data ? [200, 'OK', s.reqURL, ['content-type', data[0]], data[1]] : 'error';
            } else if (/^blob:/i.test(s.reqURL)) {
                // AETHERWPT3 (SR138): Fetch §4.3, the blob scheme (a Range request header selects a 206).
                var range = null;
                s.reqHeaders.forEach(function (h) { if (h[0].toLowerCase() === 'range') { range = h[1]; } });
                var br = typeof __blob_response === 'function' ? __blob_response(s.reqURL, s.method, s.blobEntry, range) : null;
                data = br ? [br[0], br[1], s.reqURL, br[2], br[3]] : 'error';
            }
            if (!s.async) { return sendSync(this, s, flat, bytes, data); }
            s.send = true;
            s.gen++;
            s.started = now();
            s.lastProgress = 0;
            fire(this, 'loadstart', 0, 0, false);
            if (s.readyState !== 1 || !s.send) { return; }
            var gen = s.gen;
            if (data !== null) {
                setTimeout(function () {
                    if (s.gen !== gen) { return; }
                    if (data === 'error') { onNet(self, s, 'error', []); return; }
                    onNet(self, s, 'head', data.slice(0, 4));
                    if (s.gen !== gen) { return; }
                    if (data[4].byteLength) { onNet(self, s, 'data', [data[4].buffer]); }
                    if (s.gen !== gen) { return; }
                    onNet(self, s, 'end', []);
                }, 0);
                armTimer(this);
                return;
            }
            s.id = /^https?:/i.test(s.reqURL) ? __net_http(s.method, s.reqURL.replace(/#.*$/, ''), flat, bytes, 'follow') : 0;
            if (!s.id) {
                setTimeout(function () { if (s.gen === gen && s.send) { requestError(self, s, 'error'); } }, 0);
                return;
            }
            __net_register(s.id, { __net: function (kind, a) { if (s.gen === gen) { onNet(self, s, kind, a); } } });
            armTimer(this);
        }
        abort() {
            var s = st(this);
            terminate(s);
            if ((s.readyState === 1 && s.send) || s.readyState === 2 || s.readyState === 3) {
                requestError(this, s, 'abort');
            }
            if (s.readyState === 4) {
                s.readyState = 0;
                fresh(s);
            }
        }
    }
    var terminate = function (s) {
        if (s.id) { __net_cancel(s.id); __net_forget(s.id); s.id = 0; }
        if (s.timer) { clearTimeout(s.timer); s.timer = 0; }
        s.gen++;
    };
    var armTimer = function (xhr) {
        var s = st(xhr);
        if (s.timer) { clearTimeout(s.timer); s.timer = 0; }
        if (!s.timeout || !s.send) { return; }
        var gen = s.gen;
        var left = Math.max(0, s.timeout - (now() - s.started));
        s.timer = setTimeout(function () {
            s.timer = 0;
            if (s.gen !== gen || !s.send) { return; }
            terminate(s);
            requestError(xhr, s, 'timeout');
        }, left);
    };
    var fire = function (xhr, type, loaded, total, upload) {
        __net_fire(upload ? st(xhr).upload : xhr, 'ProgressEvent', type, { lengthComputable: total > 0, loaded: loaded, total: total });
    };
    var requestError = function (xhr, s, event) {
        s.readyState = 4;
        s.send = false;
        fresh(s);
        if (s.timer) { clearTimeout(s.timer); s.timer = 0; }
        __net_fire(xhr, 'Event', 'readystatechange', undefined);
        fire(xhr, event, 0, 0, false);
        fire(xhr, 'loadend', 0, 0, false);
    };
    var headerOf = function (list, name) {
        for (var i = 0; i + 1 < list.length; i += 2) { if (String(list[i]).toLowerCase() === name) { return list[i + 1]; } }
        return null;
    };
    var mimeOf = function (s) { return s.mime !== null ? s.mime : headerOf(s.headers, 'content-type'); };
    var all = function (s) {
        if (s.chunks.length !== 1) {
            var n = 0, i;
            for (i = 0; i < s.chunks.length; i++) { n += s.chunks[i].byteLength; }
            var out = new Uint8Array(n), at = 0;
            for (i = 0; i < s.chunks.length; i++) { out.set(s.chunks[i], at); at += s.chunks[i].byteLength; }
            s.chunks = [out];
        }
        return s.chunks[0];
    };
    var textOf = function (s) {
        if (s.text !== null && s.textLen === s.received) { return s.text; }
        var m = mimeOf(s), cs = m && /charset\s*=\s*"?([^;"\s]+)/i.exec(m);
        // AETHERWPT3 (SR138): XHR §3.6.9 "text response" — the response charset's decoder (Encoding §9–§13 since O2),
        // UTF-8 when it names none or one TextDecoder refuses.
        var dec;
        try { dec = new TextDecoder(cs ? cs[1] : 'utf-8'); } catch (e) { dec = new TextDecoder(); }
        s.text = dec.decode(all(s));
        s.textLen = s.received;
        return s.text;
    };
    var onNet = function (xhr, s, kind, a) {
        if (kind === 'head') {
            s.error = false;
            s.status = a[0]; s.statusText = a[1]; s.url = a[2]; s.headers = a[3];
            var cl = headerOf(a[3], 'content-length');
            s.total = cl !== null && /^\d+$/.test(cl.trim()) ? Number(cl) : 0;
            s.readyState = 2;
            __net_fire(xhr, 'Event', 'readystatechange', undefined);
        } else if (kind === 'data') {
            var b = new Uint8Array(a[0]);
            s.chunks.push(b);
            s.received += b.byteLength;
            var t = now();
            if (s.lastProgress && t - s.lastProgress < 50) { return; }
            s.lastProgress = t;
            if (s.readyState === 2) { s.readyState = 3; }
            __net_fire(xhr, 'Event', 'readystatechange', undefined);
            if (!s.send) { return; }
            fire(xhr, 'progress', s.received, s.total, false);
        } else if (kind === 'end') {
            s.id = 0;
            if (s.timer) { clearTimeout(s.timer); s.timer = 0; }
            if (s.readyState === 2 && s.received > 0) {
                s.readyState = 3;
                __net_fire(xhr, 'Event', 'readystatechange', undefined);
                if (!s.send) { return; }
                fire(xhr, 'progress', s.received, s.total, false);
            }
            s.readyState = 4;
            s.send = false;
            __net_fire(xhr, 'Event', 'readystatechange', undefined);
            fire(xhr, 'progress', s.received, s.total, false);
            fire(xhr, 'load', s.received, s.total, false);
            fire(xhr, 'loadend', s.received, s.total, false);
        } else if (kind === 'error') {
            s.id = 0;
            requestError(xhr, s, 'error');
        }
    };
    var sendSync = function (xhr, s, flat, bytes, data) {
        var r;
        if (data === 'error') { r = { error: 'bad data: URL' }; }
        else if (data) { r = { status: data[0], statusText: data[1], url: s.reqURL, headers: data[3], body: data[4].buffer }; }
        else if (!/^https?:/i.test(s.reqURL)) { r = { error: 'unsupported scheme' }; }
        else { r = __net_http_sync(s.method, s.reqURL.replace(/#.*$/, ''), flat, bytes, 'follow', s.timeout); }
        if (r.error !== undefined) {
            s.readyState = 4;
            fresh(s);
            __net_fire(xhr, 'Event', 'readystatechange', undefined);
            fire(xhr, 'error', 0, 0, false);
            fire(xhr, 'loadend', 0, 0, false);
            throw dom("Failed to execute 'send' on 'XMLHttpRequest': Failed to load '" + s.reqURL + "'.", 'NetworkError');
        }
        s.error = false;
        s.status = r.status; s.statusText = r.statusText; s.url = r.url; s.headers = r.headers;
        s.chunks = [new Uint8Array(r.body)];
        s.received = s.chunks[0].byteLength;
        s.total = s.received;
        s.readyState = 4;
        __net_fire(xhr, 'Event', 'readystatechange', undefined);
        fire(xhr, 'load', s.received, s.total, false);
        fire(xhr, 'loadend', s.received, s.total, false);
    };
    ['UNSENT', 'OPENED', 'HEADERS_RECEIVED', 'LOADING', 'DONE'].forEach(function (n, i) {
        Object.defineProperty(XMLHttpRequest, n, { value: i, enumerable: true });
        Object.defineProperty(XMLHttpRequest.prototype, n, { value: i, enumerable: true });
    });
    Object.getOwnPropertyNames(XMLHttpRequest.prototype).forEach(function (n) {
        if (n === 'constructor' || /^[A-Z_]+$/.test(n)) { return; }
        var d = Object.getOwnPropertyDescriptor(XMLHttpRequest.prototype, n);
        d.enumerable = true;
        Object.defineProperty(XMLHttpRequest.prototype, n, d);
    });
    __net_handlers(XMLHttpRequest.prototype, ['readystatechange']);
    [XMLHttpRequestEventTarget, XMLHttpRequestUpload, XMLHttpRequest].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        Object.defineProperty(globalThis, C.name, { value: C, writable: true, configurable: true, enumerable: false });
    });
})();
"#;
