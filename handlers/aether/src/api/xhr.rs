//! `XMLHttpRequest` (the XHR Standard) — AETHERNET, LEDGER SR70.
//!
//! Asynchronous requests run on `api::fetch`'s streaming worker (`__net_http`): the readyState machine
//! advances as the head and each body chunk arrive (OPENED → HEADERS_RECEIVED → LOADING → DONE, a
//! `readystatechange` at each step and a `progress` per chunk at most every 50 ms), with `load` / `error` /
//! `abort` / `timeout` and `loadend` at the end. Synchronous requests (`open(…, false)`) join the
//! exchange on `api::fetch::http_sync`.
//!
//! AETHERWPT4 (SR142): the rest of the XHR Standard's send / response steps —
//! - §3.5.6 upload progress: the upload listener flag (listeners on `upload` when `send()` runs), the upload
//!   complete flag (set at once for a null body), `loadstart` on the upload object after the request's, and the
//!   "process request end-of-body" events (`progress`, `load`, `loadend`) before the response head is handled
//!   (the transport sends the body whole, so its end is known when the head arrives); the request error steps fire
//!   the error event and `loadend` at the upload object first while the upload is incomplete.
//! - §3.6 the MIME type and encoding of the response: MIME Sniffing §4.4 "parse a MIME type" and its
//!   serialization, Fetch's "extract a MIME type" over the `Content-Type` values, "get a response MIME type"
//!   (`text/xml` when none parses), `overrideMimeType()` (a MIME type that does not parse becomes
//!   `application/octet-stream`), "get a final MIME type" and "get a final encoding" (the override's `charset`
//!   over the response's, a label that names no encoding giving none).
//! - §3.6.9 text response: Encoding §6 "decode" (a BOM wins) with the final encoding, the XML declaration's
//!   encoding for an XML response when `responseType` is "", else UTF-8.
//! - §3.6.10 document response: `responseType = "document"` (ignored outside a Window) and `responseXML` — an HTML
//!   document (only for "document"; the final encoding, else the §13.2.3.2 prescan, else UTF-8) through html_core,
//!   or an XML document through api/xml.rs (null when not namespace-well-formed), its content type, URL and
//!   encoding set; the result kept as the response object.
//! - §3.5.6 step 4: a Document body is serialized (`text/html` or `application/xml`, UTF-8), and an author
//!   `Content-Type` whose `charset` is not UTF-8 is rewritten to UTF-8 for a Document or string body.

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
        s.error = true; s.cache = undefined; s.cacheKind = undefined; s.text = null; s.doc = null; s.docDone = false;
    };
    // ---- AETHERWPT4 (SR142): MIME Sniffing §4.4 "parse a MIME type" / serialize, Fetch "extract a MIME type" ----
    var HWS_EDGE = /^[\t\n\r ]+|[\t\n\r ]+$/g;
    var QTOKEN = /^[\t\u0020-\u007E\u0080-\u00FF]*$/;
    var parseMime = function (input) {
        var str = String(input).replace(HWS_EDGE, ''), n = str.length;
        var slash = str.indexOf('/');
        if (slash < 0) { return null; }
        var type = str.slice(0, slash);
        if (!type || !TOKEN.test(type)) { return null; }
        var pos = slash + 1;
        var semi = str.indexOf(';', pos);
        var subtype = str.slice(pos, semi < 0 ? n : semi).replace(/[\t\n\r ]+$/, '');
        if (!subtype || !TOKEN.test(subtype)) { return null; }
        pos = semi < 0 ? n : semi;
        var m = { type: type.toLowerCase(), subtype: subtype.toLowerCase(), params: [] };
        while (pos < n) {
            pos++;
            while (pos < n && /[\t\n\r ]/.test(str[pos])) { pos++; }
            var start = pos;
            while (pos < n && str[pos] !== ';' && str[pos] !== '=') { pos++; }
            var name = str.slice(start, pos).toLowerCase();
            if (pos < n) {
                if (str[pos] === ';') { continue; }
                pos++;
            }
            if (pos >= n) { break; }
            var value = '';
            if (str[pos] === '"') {
                pos++;
                for (;;) {
                    while (pos < n && str[pos] !== '"' && str[pos] !== '\\') { value += str[pos++]; }
                    if (pos >= n) { break; }
                    var qb = str[pos++];
                    if (qb === '\\') {
                        if (pos >= n) { value += '\\'; break; }
                        value += str[pos++];
                    } else { break; }
                }
                while (pos < n && str[pos] !== ';') { pos++; }
            } else {
                start = pos;
                while (pos < n && str[pos] !== ';') { pos++; }
                value = str.slice(start, pos).replace(/[\t\n\r ]+$/, '');
                if (value === '') { continue; }
            }
            if (name && TOKEN.test(name) && QTOKEN.test(value) && !m.params.some(function (p) { return p[0] === name; })) {
                m.params.push([name, value]);
            }
        }
        return m;
    };
    var essence = function (m) { return m.type + '/' + m.subtype; };
    Object.defineProperty(globalThis, '__mime_parse', { value: parseMime, writable: true, configurable: true, enumerable: false });
    var param = function (m, k) {
        for (var i = 0; i < m.params.length; i++) { if (m.params[i][0] === k) { return m.params[i][1]; } }
        return null;
    };
    var serializeMime = function (m) {
        var out = essence(m);
        m.params.forEach(function (p) {
            var v = p[1];
            if (!TOKEN.test(v)) { v = '"' + v.replace(/(["\\])/g, '\\$1') + '"'; }
            out += ';' + p[0] + '=' + v;
        });
        return out;
    };
    // Fetch "get, decode, and split" a header value: commas outside quoted strings.
    var splitValues = function (v) {
        var out = [], cur = '', q = false;
        for (var i = 0; i < v.length; i++) {
            var c = v[i];
            if (q && c === '\\' && i + 1 < v.length) { cur += c + v[++i]; continue; }
            if (c === '"') { q = !q; }
            if (c === ',' && !q) { out.push(cur.replace(HWS_EDGE, '')); cur = ''; continue; }
            cur += c;
        }
        out.push(cur.replace(HWS_EDGE, ''));
        return out;
    };
    var extractMime = function (list) {
        var vals = [];
        for (var i = 0; i + 1 < list.length; i += 2) { if (String(list[i]).toLowerCase() === 'content-type') { vals.push(String(list[i + 1])); } }
        if (!vals.length) { return null; }
        var charset = null, ess = null, mime = null;
        splitValues(vals.join(', ')).forEach(function (v) {
            var t = parseMime(v);
            if (t === null || essence(t) === '*/*') { return; }
            mime = t;
            if (ess !== essence(mime)) {
                charset = param(t, 'charset');
                ess = essence(mime);
            } else if (param(mime, 'charset') === null && charset !== null) {
                mime.params.push(['charset', charset]);
            }
        });
        return mime;
    };
    var isXMLMime = function (m) { var e = essence(m); return e === 'text/xml' || e === 'application/xml' || /\+xml$/.test(m.subtype); };
    var isHTMLMime = function (m) { return essence(m) === 'text/html'; };
    var responseMime = function (s) { return extractMime(s.headers) || parseMime('text/xml'); };
    var finalMime = function (s) { return s.override !== null ? s.override : responseMime(s); };
    var getEncoding = function (label) {
        if (typeof __get_encoding === 'function') { return __get_encoding(label); }
        try { return new TextDecoder(label).encoding; } catch (e) { return null; }
    };
    var finalEncoding = function (s) {
        var label = param(responseMime(s), 'charset');
        if (s.override !== null && param(s.override, 'charset') !== null) { label = param(s.override, 'charset'); }
        return label === null ? null : getEncoding(label);
    };
    // Encoding §6 "decode": the BOM, else `label`, else UTF-8.
    var decodeBytes = function (bytes, label) {
        if (typeof __decode_bytes === 'function') { return __decode_bytes(bytes, label); }
        var d;
        try { d = new TextDecoder(label || 'utf-8'); } catch (e) { d = new TextDecoder(); }
        return [d.decode(bytes), d.encoding];
    };
    var hasListeners = function (t) { return typeof __net_has_listeners === 'function' ? __net_has_listeners(t) : true; };
    var isWindow = function () { return typeof document !== 'undefined' && typeof WorkerGlobalScope === 'undefined'; };
    // §3.6.10 document response.
    var documentResponse = function (s) {
        if (s.docDone) { return s.doc; }
        s.docDone = true;
        s.doc = null;
        var m = finalMime(s), html = isHTMLMime(m);
        if (!html && !isXMLMime(m)) { return null; }
        if (s.responseType === '' && html) { return null; }
        var bytes = all(s), cs = finalEncoding(s), doc = null, r;
        if (html) {
            if (cs === null && typeof __html_prescan === 'function') { cs = __html_prescan(bytes); }
            r = decodeBytes(bytes, cs === null ? 'UTF-8' : cs);
            if (typeof __parse_document !== 'function') { return null; }
            doc = __parse_document(r[0]);
        } else {
            if (cs === null && typeof __xml_sniff === 'function') { cs = __xml_sniff(bytes); }
            r = decodeBytes(bytes, cs === null ? 'UTF-8' : cs);
            if (typeof __xml_document !== 'function') { return null; }
            doc = __xml_document(r[0], essence(m), s.url, r[1]);
            if (doc === null) { return null; }
        }
        if (typeof __doc_meta === 'function') { __doc_meta(doc, essence(m), s.url, r[1]); }
        s.doc = doc;
        return doc;
    };
    class XMLHttpRequest extends XMLHttpRequestEventTarget {
        constructor() {
            creating = true;
            var up;
            try { super(); up = new XMLHttpRequestUpload(); } finally { creating = false; }
            var s = { readyState: 0, method: 'GET', reqURL: '', url: '', async: true, user: null, password: null, reqHeaders: [],
                      send: false, timeout: 0, withCredentials: false, responseType: '', mime: null, id: 0, timer: 0,
                      started: 0, lastProgress: 0, upload: up, gen: 0, override: null, upListener: false,
                      upComplete: true, upTotal: 0 };
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
            // §3.6.7 step 1: "document" is ignored outside a Window.
            if (v === 'document' && !isWindow()) { return; }
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
            if (s.readyState !== 4 || s.error) { return null; }
            return documentResponse(s);
        }
        get response() {
            var s = st(this);
            if (s.responseType === '' || s.responseType === 'text') {
                if (s.readyState !== 3 && s.readyState !== 4) { return ''; }
                return textOf(s);
            }
            if (s.readyState !== 4 || s.error) { return null; }
            if (s.responseType === 'document') { return documentResponse(s); }
            if (s.cacheKind === s.responseType) { return s.cache; }
            var bytes = all(s), v = null;
            if (s.responseType === 'arraybuffer') { v = bytes.buffer; }
            else if (s.responseType === 'blob') { v = new Blob([bytes], { type: serializeMime(finalMime(s)) }); }
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
            s.override = parseMime(String(mime)) || parseMime('application/octet-stream');
        }
        send(body) {
            var s = st(this), self = this;
            if (s.readyState !== 1) { throw dom("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError'); }
            if (s.send) { throw dom("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError'); }
            var bytes = null, ctype = null, isDoc = false;
            if (s.method !== 'GET' && s.method !== 'HEAD' && body !== undefined && body !== null) {
                if (typeof Document !== 'undefined' && body instanceof Document) {
                    // §3.5.6 step 4.1: the document serialized (fragment serializing algorithm, require well-formed false).
                    isDoc = true;
                    var htmlDoc = body.contentType === 'text/html';
                    var ser = '';
                    if (htmlDoc) { ser = body.documentElement ? body.documentElement.outerHTML : ''; }
                    else { try { ser = new XMLSerializer().serializeToString(body); } catch (e) { ser = ''; } }
                    bytes = new TextEncoder().encode(ser);
                    ctype = htmlDoc ? 'text/html;charset=UTF-8' : 'application/xml;charset=UTF-8';
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
                    // §3.5.6 step 4.4: for a Document or string body, a parsed author Content-Type whose charset is
                    // not UTF-8 gets charset=UTF-8 (re-serialized); anything else is sent as the author wrote it.
                    if (bytes !== null && (isDoc || typeof body === 'string')) {
                        var rec = parseMime(h[1]), cs = rec === null ? null : param(rec, 'charset');
                        if (cs !== null && cs.toLowerCase() !== 'utf-8') {
                            rec.params.forEach(function (p) { if (p[0] === 'charset') { p[1] = 'UTF-8'; } });
                            h = [h[0], serializeMime(rec)];
                        }
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
            // §3.5.6 steps 5, 7, 8: the upload listener flag, and the upload complete flag for a null body.
            s.upListener = hasListeners(s.upload);
            s.upComplete = bytes === null;
            s.upTotal = bytes === null ? 0 : bytes.byteLength;
            if (!s.async) { return sendSync(this, s, flat, bytes, data); }
            s.send = true;
            s.gen++;
            s.started = now();
            s.lastProgress = 0;
            fire(this, 'loadstart', 0, 0, false);
            if (!s.upComplete && s.upListener && s.readyState === 1 && s.send) {
                fire(this, 'loadstart', 0, s.upTotal, true);
            }
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
            s.rtStart = performance.now(); // AETHERWPT3 (SR138): Resource Timing
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
        // §3.5.6 "request error steps" step 6: an incomplete upload ends with the event at the upload object.
        if (!s.upComplete) {
            s.upComplete = true;
            if (s.upListener) {
                fire(xhr, event, 0, 0, true);
                fire(xhr, 'loadend', 0, 0, true);
            }
        }
        fire(xhr, event, 0, 0, false);
        fire(xhr, 'loadend', 0, 0, false);
    };
    var headerOf = function (list, name) {
        for (var i = 0; i + 1 < list.length; i += 2) { if (String(list[i]).toLowerCase() === name) { return list[i + 1]; } }
        return null;
    };
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
        // AETHERWPT4 (SR142): §3.6.9 "text response" — the final encoding; for responseType "" and an XML final MIME
        // type without one, the XML declaration's; else UTF-8; a BOM overriding any of them (Encoding §6 "decode").
        var bytes = all(s), cs = finalEncoding(s);
        if (cs === null && s.responseType === '' && isXMLMime(finalMime(s)) && typeof __xml_sniff === 'function') { cs = __xml_sniff(bytes); }
        s.text = decodeBytes(bytes, cs === null ? 'UTF-8' : cs)[0];
        s.textLen = s.received;
        return s.text;
    };
    var onNet = function (xhr, s, kind, a) {
        if (kind === 'head') {
            // §3.5.6 "process request end-of-body": the body was sent before the response began.
            if (!s.upComplete) {
                var g0 = s.gen;
                s.upComplete = true;
                if (s.upListener) {
                    fire(xhr, 'progress', s.upTotal, s.upTotal, true);
                    if (s.gen !== g0) { return; }
                    fire(xhr, 'load', s.upTotal, s.upTotal, true);
                    if (s.gen !== g0) { return; }
                    fire(xhr, 'loadend', s.upTotal, s.upTotal, true);
                    if (s.gen !== g0) { return; }
                }
            }
            s.error = false;
            s.rtHead = performance.now();
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
            if (typeof __rt_record === 'function' && s.rtStart !== undefined) {
                var same = false;
                try { same = new URL(s.reqURL).origin === String(location.origin); } catch (e) {}
                __rt_record(s.reqURL.replace(/#.*$/, ''), 'xmlhttprequest', s.rtStart, s.rtHead, s.received, s.headers, same ? 'basic' : 'cors', s.status);
            }
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
