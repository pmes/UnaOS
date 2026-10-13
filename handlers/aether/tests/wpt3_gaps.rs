//! AETHERWPT3 (LEDGER SR138): one test per gap of the AETHERWPT2 list this series closed (docs/dev/evidence/aetherwpt/
//! WPT.md, the AETHERWPT3 section, O1–O9), each on a headless page asserting what the WPT directory's tests read
//! back. The expected values are the specification's (section cited per test).

fn load_with(html: &str, budget_ms: u64) -> aether::AetherEngine {
    let mut engine = aether::AetherEngine::new();
    engine.load_html("http://127.0.0.1:8000/wpt3.html", html, true);
    if let Some(js) = engine.js_engine.as_mut() {
        aether::event_loop::settle(js, budget_ms);
    }
    engine
}

fn eval(engine: &mut aether::AetherEngine, src: &str) -> String {
    let js = engine.js_engine.as_mut().expect("page has a script engine");
    js.eval_string(src).unwrap_or_else(|e| format!("THROW {e}"))
}

/// A page whose one script stores its answers in `window.R` (joined with `|` by the test).
fn page(script: &str) -> String {
    format!("<!doctype html><html><body><script>var R = [];\n{script}\n</script></body></html>")
}

fn answers(script: &str) -> String {
    let mut e = load_with(&page(script), 5_000);
    eval(&mut e, "R.join('|')")
}

/// O7 — WebCrypto §20 RSASSA-PKCS1-v1_5, §21 RSA-PSS, §22 RSA-OAEP, §23 ECDSA, §24 ECDH, §26 AES-CBC, §28
/// AES-KW, §32 X25519, §33 Ed25519, §14.3.11/§14.3.12 wrapKey / unwrapKey, and the spki / pkcs8 / jwk formats:
/// RFC 8032 test 1 (Ed25519 from a pkcs8 seed), RFC 7748 §6.1 (X25519 shared secret), RFC 3394 §4.1 (AES-KW),
/// SP 800-38A F.2.1 (AES-CBC); a generated RSA key whose PKCS #1 v1.5 signature survives a pkcs8 round trip
/// byte for byte, the same key as RSA-PSS and RSA-OAEP, ECDSA / ECDH on P-256, and the format errors.
#[test]
fn o7_subtle_crypto_asymmetric_and_formats() {
    let mut e = load_with(
        &page(
            r#"
var hex = function (b) { return Array.prototype.map.call(new Uint8Array(b), function (x) { return (x + 256).toString(16).slice(1); }).join(''); };
var un = function (h) { var o = new Uint8Array(h.length / 2); for (var i = 0; i < o.length; i++) { o[i] = parseInt(h.substr(i * 2, 2), 16); } return o; };
var te = new TextEncoder(), S = crypto.subtle;
var fail = function (p) { return p.then(function () { return 'resolved'; }, function (e) { return e.name; }); };
var msg = te.encode('abc');
Promise.all([
    S.importKey('pkcs8', un('302e020100300506032b657004220420' + '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60'), 'Ed25519', true, ['sign']).then(function (k) {
        return Promise.all([S.sign('Ed25519', k, new Uint8Array(0)), S.exportKey('jwk', k)]).then(function (r) {
            return hex(r[0]) + ':' + r[1].kty + ':' + r[1].crv + ':' + r[1].alg + ':' + Object.keys(r[1]).length;
        });
    }),
    Promise.all([
        S.importKey('pkcs8', un('302e020100300506032b656e04220420' + '77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a'), 'X25519', false, ['deriveBits']),
        S.importKey('raw', un('de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f'), 'X25519', true, []),
    ]).then(function (k) { return S.deriveBits({ name: 'X25519', public: k[1] }, k[0], 256).then(hex); }),
    Promise.all([
        S.importKey('raw', un('000102030405060708090a0b0c0d0e0f'), 'AES-KW', false, ['wrapKey', 'unwrapKey']),
        S.importKey('raw', un('00112233445566778899aabbccddeeff'), 'AES-GCM', true, ['encrypt']),
    ]).then(function (k) {
        return S.wrapKey('raw', k[1], k[0], 'AES-KW').then(function (w) {
            return S.unwrapKey('raw', w, k[0], 'AES-KW', 'AES-GCM', true, ['decrypt']).then(function (u) {
                return S.exportKey('raw', u).then(function (r) { return hex(w) + ':' + hex(r) + ':' + u.usages.join(','); });
            });
        });
    }),
    S.importKey('raw', un('2b7e151628aed2a6abf7158809cf4f3c'), 'AES-CBC', false, ['encrypt', 'decrypt']).then(function (k) {
        var iv = un('000102030405060708090a0b0c0d0e0f');
        return S.encrypt({ name: 'AES-CBC', iv: iv }, k, un('6bc1bee22e409f96e93d7e117393172a')).then(function (ct) {
            var bad = new Uint8Array(ct.slice(0)); bad[15] ^= 1; // the padding byte 0x10 decrypts as 0x11
            return Promise.all([S.decrypt({ name: 'AES-CBC', iv: iv }, k, ct).then(hex), fail(S.decrypt({ name: 'AES-CBC', iv: iv }, k, bad))])
                .then(function (r) { return hex(ct).slice(0, 32) + ':' + ct.byteLength + ':' + r.join(':'); });
        });
    }),
    S.generateKey({ name: 'RSASSA-PKCS1-v1_5', modulusLength: 1024, publicExponent: new Uint8Array([1, 0, 1]), hash: 'SHA-256' }, true, ['sign', 'verify']).then(function (p) {
        var a = p.privateKey.algorithm;
        return Promise.all([S.sign('RSASSA-PKCS1-v1_5', p.privateKey, msg), S.exportKey('pkcs8', p.privateKey), S.exportKey('spki', p.publicKey), S.exportKey('jwk', p.publicKey), fail(S.exportKey('spki', p.privateKey))]).then(function (r) {
            var sig = r[0], p8 = r[1], spki = r[2];
            return Promise.all([
                S.verify('RSASSA-PKCS1-v1_5', p.publicKey, sig, msg),
                S.verify('RSASSA-PKCS1-v1_5', p.publicKey, sig, te.encode('abd')),
                S.importKey('pkcs8', p8, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, false, ['sign']).then(function (k2) { return S.sign('RSASSA-PKCS1-v1_5', k2, msg).then(function (s2) { return hex(s2) === hex(sig); }); }),
                S.importKey('jwk', r[3], { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, true, ['verify']).then(function (k3) { return S.verify('RSASSA-PKCS1-v1_5', k3, sig, msg); }),
                Promise.all([S.importKey('pkcs8', p8, { name: 'RSA-PSS', hash: 'SHA-256' }, false, ['sign']), S.importKey('spki', spki, { name: 'RSA-PSS', hash: 'SHA-256' }, false, ['verify'])]).then(function (k) {
                    return S.sign({ name: 'RSA-PSS', saltLength: 32 }, k[0], msg).then(function (s) { return S.verify({ name: 'RSA-PSS', saltLength: 32 }, k[1], s, msg); });
                }),
                Promise.all([S.importKey('pkcs8', p8, { name: 'RSA-OAEP', hash: 'SHA-256' }, false, ['decrypt']), S.importKey('spki', spki, { name: 'RSA-OAEP', hash: 'SHA-256' }, false, ['encrypt'])]).then(function (k) {
                    return S.encrypt({ name: 'RSA-OAEP', label: msg }, k[1], te.encode('hello')).then(function (c) {
                        return Promise.all([S.decrypt({ name: 'RSA-OAEP', label: msg }, k[0], c).then(function (d) { return new TextDecoder().decode(d); }), fail(S.decrypt({ name: 'RSA-OAEP' }, k[0], c))]);
                    }).then(function (x) { return x.join(':'); });
                }),
            ]).then(function (v) { return a.name + ':' + a.modulusLength + ':' + hex(a.publicExponent) + ':' + a.hash.name + ':' + r[3].alg + ':' + r[4] + ':' + v.join(':'); });
        });
    }),
    S.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']).then(function (p) {
        return S.sign({ name: 'ECDSA', hash: 'SHA-384' }, p.privateKey, msg).then(function (sig) {
            return Promise.all([S.exportKey('spki', p.publicKey), S.exportKey('jwk', p.privateKey), S.exportKey('raw', p.publicKey)]).then(function (x) {
                return Promise.all([
                    S.importKey('spki', x[0], { name: 'ECDSA', namedCurve: 'P-256' }, true, ['verify']).then(function (k) { return S.verify({ name: 'ECDSA', hash: 'SHA-384' }, k, sig, msg); }),
                    S.verify({ name: 'ECDSA', hash: 'SHA-256' }, p.publicKey, sig, msg),
                    S.importKey('jwk', x[1], { name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign']).then(function (k) { return k.type + ',' + k.algorithm.namedCurve; }),
                ]).then(function (v) { return sig.byteLength + ':' + x[2].byteLength + ':' + x[1].kty + ':' + x[1].crv + ':' + v.join(':'); });
            });
        });
    }),
    Promise.all([S.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, false, ['deriveBits']), S.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, false, ['deriveBits'])]).then(function (k) {
        return Promise.all([S.deriveBits({ name: 'ECDH', public: k[1].publicKey }, k[0].privateKey, 256), S.deriveBits({ name: 'ECDH', public: k[0].publicKey }, k[1].privateKey, 256), S.deriveBits({ name: 'ECDH', public: k[0].publicKey }, k[1].privateKey, 100)]).then(function (b) {
            return (hex(b[0]) === hex(b[1])) + ':' + b[0].byteLength + ':' + b[2].byteLength + ':' + k[0].publicKey.usages.length;
        });
    }),
    fail(S.importKey('spki', un('3000'), { name: 'ECDSA', namedCurve: 'P-256' }, true, ['verify'])),
    fail(S.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['verify'])),
    fail(S.generateKey({ name: 'ECDSA', namedCurve: 'P-521' }, true, ['sign'])),
    fail(S.importKey('jwk', { kty: 'OKP', crv: 'Ed25519', x: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA', alg: 'ed25519' }, 'Ed25519', true, ['verify'])),
]).then(function (r) { R = r; }, function (e) { R = ['rejected ' + e]; });
"#,
        ),
        60_000,
    );
    assert_eq!(
        eval(&mut e, "R.join('|')"),
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b:OKP:Ed25519:Ed25519:7\
         |4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742\
         |1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5:00112233445566778899aabbccddeeff:decrypt\
         |7649abac8119b246cee98e9b12e9197d:32:6bc1bee22e409f96e93d7e117393172a:OperationError\
         |RSASSA-PKCS1-v1_5:1024:010001:SHA-256:RS256:InvalidAccessError:true:false:true:true:true:hello:OperationError\
         |64:65:EC:P-256:true:false:private,P-256\
         |true:32:13:0\
         |DataError\
         |SyntaxError\
         |NotSupportedError\
         |DataError"
    );
}

/// O1 — Streams §5 (WritableStream backpressure: desiredSize over a CountQueuingStrategy), §6 (TransformStream
/// transform / flush), §4.9.1 ReadableStreamPipeTo (pipeThrough into pipeTo; an AbortSignal shuts the pipe down
/// with its reason), §4.7 / §4.5 (a byte stream answering a BYOB read through `byobRequest.respond`, the reader's
/// buffer transferred), §4.2.5 async iteration, Encoding §5 / §6 (TextEncoderStream joining a surrogate pair
/// split across chunks, TextDecoderStream a UTF-8 sequence split across chunks), Compression Streams §5
/// (DecompressionStream over zlib split mid-stream, gzip, and deflate-raw with junk after the end: TypeError).
#[test]
fn o1_streams_writable_transform_pipes_bytes_codecs() {
    let mut e = load_with(
        &page(
            r#"
var hex = function (b) { return Array.prototype.map.call(new Uint8Array(b), function (x) { return (x + 256).toString(16).slice(1); }).join(''); };
var un = function (h) { var o = new Uint8Array(h.length / 2); for (var i = 0; i < o.length; i++) { o[i] = parseInt(h.substr(i * 2, 2), 16); } return o; };
var collect = async function (readable) { var out = []; for await (const c of readable) { out.push(c); } return out; };
var bytes = function (chunks) { var n = 0; chunks.forEach(function (c) { n += c.length; }); var o = new Uint8Array(n), at = 0; chunks.forEach(function (c) { o.set(c, at); at += c.length; }); return o; };
var log = [];
var ws = new WritableStream({ write: function (c) { log.push('w:' + c); }, close: function () { log.push('closed'); } }, new CountQueuingStrategy({ highWaterMark: 2 }));
var w = ws.getWriter();
var d0 = w.desiredSize;
w.write('a'); w.write('b');
var d1 = w.desiredSize;
var pWritable = w.close().then(function () { return d0 + ':' + d1 + ':' + log.join(',') + ':' + ws.locked; });
var rs = new ReadableStream({ start: function (c) { c.enqueue('x'); c.enqueue('yz'); c.close(); } });
var upper = new TransformStream({ transform: function (chunk, c) { c.enqueue(chunk.toUpperCase()); }, flush: function (c) { c.enqueue('!'); } });
var piped = [];
var pPipe = rs.pipeThrough(upper).pipeTo(new WritableStream({ write: function (c) { piped.push(c); } })).then(function () { return piped.join(',') + ':' + rs.locked; });
var ac = new AbortController();
var pAbort = new ReadableStream({ pull: function (c) { c.enqueue(1); } }).pipeTo(new WritableStream(), { signal: ac.signal }).then(function () { return 'resolved'; }, function (e) { return e.name; });
ac.abort();
var bs = new ReadableStream({ type: 'bytes', pull: function (c) { var v = c.byobRequest.view; v[0] = 7; v[1] = 8; c.byobRequest.respond(2); } });
var view = new Uint8Array(4);
var pByob = bs.getReader({ mode: 'byob' }).read(view).then(function (r) { return r.value.constructor.name + ':' + r.value.length + ':' + r.value[0] + ',' + r.value[1] + ':' + r.done + ':' + view.byteLength; });
var tes = new TextEncoderStream(), tw = tes.writable.getWriter();
tw.write('a\uD83D'); tw.write('\uDCA9'); tw.close();
var pEnc = collect(tes.readable).then(function (c) { return tes.encoding + ':' + hex(bytes(c)); });
var tds = new TextDecoderStream(), dw = tds.writable.getWriter();
dw.write(new Uint8Array([0xE2, 0x82])); dw.write(new Uint8Array([0xAC])); dw.close();
var pDec = collect(tds.readable).then(function (c) { return c.length + ':' + c.join('').charCodeAt(0).toString(16); });
var inflate = function (format, parts) {
    var ds = new DecompressionStream(format), wr = ds.writable.getWriter();
    parts.forEach(function (p) { wr.write(p).catch(function () {}); });
    wr.close().catch(function () {});
    return collect(ds.readable).then(function (c) { return new TextDecoder().decode(bytes(c)); }, function (e) { return e.name; });
};
var z = un('789c4bad28484d2e494d51c82f2d29282d010030ad0624');
Promise.all([
    pWritable, pPipe, pAbort, pByob, pEnc, pDec,
    inflate('deflate', [z.slice(0, 5), z.slice(5)]),
    inflate('gzip', [un('1f8b08000000000002034bad28484d2e494d51c82f2d29282d0100b00139b30f000000')]),
    inflate('deflate-raw', [un('4bad28484d2e494d51c82f2d29282d010000')]),
    inflate('deflate', [z.slice(0, 7)]),
]).then(function (r) { R = r; }, function (e) { R = ['rejected ' + e]; });
"#,
        ),
        10_000,
    );
    assert_eq!(
        eval(&mut e, "R.join('|')"),
        "2:0:w:a,w:b,closed:true|X,YZ,!:false|AbortError|Uint8Array:2:7,8:false:0|utf-8:61f09f92a9|1:20ac|expected output|expected output|TypeError|TypeError"
    );
}

/// O2 — Encoding §9–§13 through TextDecoder: a single-byte index (KOI8-R, windows-1251, an unmapped ISO-8859-6
/// byte), Shift_JIS, EUC-JP (JIS X 0212 behind 0x8F, half-width katakana behind 0x8E), ISO-2022-JP escapes, EUC-KR,
/// Big5 (pointer 1133's two code points), GBK and gb18030 (four-byte ranges; the last ranges pointer, 39419 → U+FFFF),
/// `fatal`, and `{stream: true}` across a lead byte. The code points are the WHATWG indexes' (index JSON at
/// whatwg/encoding a985b62a).
#[test]
fn o2_legacy_decoders() {
    let got = answers(
        r#"
var hex = function (s) { var o = []; for (var i = 0; i < s.length; i++) { o.push(s.charCodeAt(i).toString(16)); } return o.join(' '); };
var d = function (l, b, o) { return hex(new TextDecoder(l, o).decode(new Uint8Array(b))); };
R.push(d('koi8-r', [0xC1, 0x41]), d('cp1251', [0xC0]), d('iso-8859-6', [0xA1]));
R.push(d('sjis', [0x82, 0xA0, 0x41, 0xB1]), d('euc-jp', [0xA4, 0xA2, 0x8E, 0xB1, 0x8F, 0xB0, 0xA1]));
R.push(d('iso-2022-jp', [0x1B, 0x24, 0x42, 0x24, 0x22, 0x1B, 0x28, 0x42, 0x41]), d('korean', [0xB0, 0xA1]));
R.push(d('big5', [0xA4, 0x40, 0x88, 0x62]), d('gb2312', [0xC4, 0xE3, 0x80]), d('gb18030', [0x81, 0x30, 0x81, 0x30, 0x84, 0x31, 0xA4, 0x39]));
try { new TextDecoder('shift_jis', { fatal: true }).decode(new Uint8Array([0x82])); R.push('no throw'); } catch (e) { R.push(e.name); }
var t = new TextDecoder('euc-kr'), s = t.decode(new Uint8Array([0x41, 0xB0]), { stream: true });
s += t.decode(new Uint8Array([0xA1]));
R.push(hex(s), new TextDecoder('x-sjis').encoding);
"#,
    );
    assert_eq!(got, "430 41|410|fffd|3042 41 ff71|3042 ff71 4e02|3042 41|ac00|4e00 ca 304|4f60 20ac|80 ffff|TypeError|41 ac00|shift_jis");
}

/// Drives the event loop until `cond` holds (real time, for a worker's thread), then answers `out`.
fn run_until(engine: &mut aether::AetherEngine, cond: &str, out: &str, secs: u64) -> String {
    let js = engine.js_engine.as_mut().expect("engine");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        js.tick();
        if js.eval_string(&format!("!!({cond})")).unwrap_or_default() == "true" {
            return js.eval_string(out).unwrap_or_else(|e| format!("<error {e}>"));
        }
        if std::time::Instant::now() > end {
            return format!("timed out: {}", js.eval_string(out).unwrap_or_default());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// O3 — File API §8.3 (the blob URL store: `blob:<origin>/<UUID v4>`, only an exact match revokes, a Request keeps
/// the entry it parsed, a non-Blob is a TypeError) and Fetch §4.3 (the blob scheme: Content-Type / Content-Length,
/// a fragment ignored, a `Range` → 206 with Content-Range, a method other than GET and a revoked URL network
/// errors) through fetch and XMLHttpRequest; File API §7 FileReaderSync in a worker whose script is itself a blob
/// URL.
#[test]
fn o3_blob_url_store_fetch_and_file_reader_sync() {
    let mut e = load_with(
        &page(
            r#"
var b = new Blob(['hello blob'], { type: 'text/plain' }), u = URL.createObjectURL(b);
R.push(/^blob:http:\/\/127\.0\.0\.1:8000\/[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(u), new URL(u).origin);
try { URL.createObjectURL({}); R.push('no throw'); } catch (e) { R.push(e.name); }
var req = new Request(u);
URL.revokeObjectURL(u + '#x');
var fail = function (p) { return p.then(function () { return 'resolved'; }, function (e) { return e.name; }); };
var x = new XMLHttpRequest();
x.open('GET', u, false);
x.send();
R.push(x.status + ':' + x.responseText);
var P = [
    fetch(u + '#frag').then(function (r) { return r.text().then(function (t) { return r.status + ':' + r.headers.get('content-type') + ':' + r.headers.get('content-length') + ':' + t + ':' + (r.url === u); }); }),
    fetch(u, { headers: { Range: 'bytes=6-' } }).then(function (r) { return r.text().then(function (t) { return r.status + ':' + r.headers.get('content-range') + ':' + t; }); }),
    fail(fetch(u, { method: 'POST' })),
];
URL.revokeObjectURL(u);
P.push(fail(fetch(u)), fetch(req).then(function (r) { return r.text(); }));
var W = null;
var wsrc = URL.createObjectURL(new Blob(["var s = new FileReaderSync(), bl = new Blob([new Uint8Array([0xEF, 0xBB, 0xBF, 0x61, 0x62])]); postMessage([s.readAsText(bl), s.readAsDataURL(new Blob()), s.readAsArrayBuffer(bl).byteLength, s.readAsBinaryString(new Blob(['\\u03c3'])).length].join(','));"], { type: 'text/javascript' }));
new Worker(wsrc).onmessage = function (e) { W = e.data; };
Promise.all(P).then(function (r) { R = R.concat(r); }, function (e) { R.push('rejected ' + e); });
"#,
        ),
        5_000,
    );
    assert_eq!(
        eval(&mut e, "R.join('|')"),
        "true|http://127.0.0.1:8000|TypeError|200:hello blob|200:text/plain:10:hello blob:true|206:bytes 6-9/10:blob|TypeError|TypeError|hello blob"
    );
    assert_eq!(run_until(&mut e, "W !== null", "W", 20), "ab,data:application/octet-stream;base64,,5,2");
}

/// O4 — HTML §4.13.7 ElementInternals and form-associated custom elements: attachInternals (once; never on a
/// non-custom element), the form owner and `form.elements`, `labels`, formAssociatedCallback /
/// formDisabledCallback / formResetCallback as reactions, setFormValue (a string under `name`, a FormData cloned
/// when set) in the form's entry list, setValidity (flags need a message; CRLF normalized) driving `validity`,
/// `validationMessage`, `checkValidity` and the form's, `states` (CustomStateSet), the ARIA mixin, and the
/// NotSupportedError of a definition that is not form-associated.
#[test]
fn o4_element_internals_form_associated() {
    let mut e = load_with(
        &page(
            r#"
var log = [];
class MyControl extends HTMLElement {
    static get formAssociated() { return true; }
    constructor() { super(); this.i = this.attachInternals(); }
    formAssociatedCallback(f) { log.push('assoc:' + (f ? f.id : null)); }
    formDisabledCallback(d) { log.push('disabled:' + d); }
    formResetCallback() { log.push('reset'); }
}
customElements.define('my-control', MyControl);
var form = document.createElement('form');
form.id = 'f';
document.body.appendChild(form);
var c = document.createElement('my-control');
c.setAttribute('name', 'n');
c.id = 'mc';
var lab = document.createElement('label');
lab.htmlFor = 'mc';
form.appendChild(lab);
form.appendChild(c);
var entries = function () { return Array.from(new FormData(form)).map(function (e) { return e.join('='); }).join('&'); };
R.push(log.join(','), c.i.form === form, form.elements.length, form.elements[0] === c, c.i.labels.length, c.i.willValidate);
c.i.setFormValue('v1');
R.push(entries());
var fd = new FormData();
fd.append('a', '1');
fd.append('b', '2');
c.i.setFormValue(fd);
fd.append('c', '3');
R.push(entries());
try { c.i.setValidity({ valueMissing: true }); R.push('no throw'); } catch (e) { R.push(e.name); }
c.i.setValidity({ valueMissing: true, tooShort: true }, 'need\r\nmore');
R.push(c.i.validity.valueMissing, c.i.validity.tooShort, c.i.validity.valid, JSON.stringify(c.i.validationMessage), c.i.checkValidity(), form.checkValidity());
c.setAttribute('disabled', '');
R.push(log.join(','), c.i.willValidate);
c.removeAttribute('disabled');
form.reset();
R.push(log.join(','));
c.i.states.add('checked');
R.push(c.i.states.has('checked'), c.i.states.size, c.i.states instanceof CustomStateSet);
c.i.ariaLabel = 'Lab';
R.push(c.i.ariaLabel, c.i.role);
try { c.attachInternals(); R.push('no throw'); } catch (e) { R.push(e.name); }
try { document.createElement('div').attachInternals(); R.push('no throw'); } catch (e) { R.push(e.name); }
customElements.define('plain-el', class extends HTMLElement { constructor() { super(); this.i = this.attachInternals(); } });
var p = document.createElement('plain-el');
try { p.i.form; R.push('no throw'); } catch (e) { R.push(e.name); }
R.push(p.i.shadowRoot);
form.removeChild(c);
R.push(log.join(','));
"#,
        ),
        5_000,
    );
    assert_eq!(
        eval(&mut e, "R.join('|')"),
        r#"assoc:f|true|1|true|1|true|n=v1|a=1&b=2|TypeError|true|true|false|"need\nmore"|false|false|assoc:f,disabled:true|false|assoc:f,disabled:true,disabled:false,reset|true|1|true|Lab||NotSupportedError|NotSupportedError|NotSupportedError||assoc:f,disabled:true,disabled:false,reset,assoc:null"#
    );
}

/// O5 — Pointer Events §10 / §11: setPointerCapture's NotFoundError (no such pointer) and InvalidStateError
/// (disconnected), no capture outside the active buttons state, then a real press (the engine's mouse, pointerId 1):
/// pointerdown before the compatibility mousedown, capture taken in the pointerdown listener, `gotpointercapture`
/// before the next pointer event, the move over another element retargeted to the capturing one (pointermove and
/// mousemove), and the implicit release after pointerup (`lostpointercapture`).
#[test]
fn o5_pointer_capture() {
    let mut e = aether::AetherEngine::new();
    e.load_html(
        "http://127.0.0.1:8000/wpt3.html",
        &page(
            r#"
var log = [];
var a = document.getElementById('a'), b = document.getElementById('b');
['pointerdown', 'pointermove', 'pointerup', 'gotpointercapture', 'lostpointercapture', 'mousedown', 'mouseup', 'mousemove'].forEach(function (t) {
    a.addEventListener(t, function (e) { log.push('a:' + e.type + (e.pointerId !== undefined ? ':' + e.pointerId + ':' + e.pointerType : '')); });
    b.addEventListener(t, function (e) { log.push('b:' + e.type); });
});
a.addEventListener('pointerdown', function (e) { a.setPointerCapture(e.pointerId); log.push('has:' + a.hasPointerCapture(1)); });
try { a.setPointerCapture(7); R.push('no throw'); } catch (e) { R.push(e.name); }
try { document.createElement('div').setPointerCapture(1); R.push('no throw'); } catch (e) { R.push(e.name); }
a.setPointerCapture(1);
R.push(a.hasPointerCapture(1));
"#,
        )
        .replace(
            "<body>",
            "<body style=\"margin:0\"><div id=\"a\" style=\"position:absolute;left:0;top:0;width:100px;height:100px\"></div><div id=\"b\" style=\"position:absolute;left:0;top:200px;width:100px;height:100px\"></div>",
        ),
        true,
    );
    use aether::api::events::Event;
    e.handle_event(Event::Resize(400, 400));
    e.handle_event(Event::MouseDown(10.0, 10.0));
    e.handle_event(Event::MouseMove(10.0, 210.0));
    e.handle_event(Event::MouseUp(10.0, 210.0));
    assert_eq!(
        eval(&mut e, "R.join('|') + ' / ' + log.join(',')"),
        "NotFoundError|InvalidStateError|false / a:pointerdown:1:mouse,has:true,a:mousedown,a:gotpointercapture:1:mouse,a:pointermove:1:mouse,a:mousemove,a:pointerup:1:mouse,a:mouseup,a:lostpointercapture:1:mouse"
    );
}

/// O6 — Resource Timing L2 §4 (entries from the fetch path's record — `__rt_record`, what fetch() and XHR call at the
/// end of a response: times, sizes with the 300-byte header allowance, Fetch's TAO check zeroing a cross-origin
/// entry's detailed timings and transferSize, a Timing-Allow-Origin match exposing them; the buffer limit, the
/// secondary buffer and `resourcetimingbufferfull`, observers of `resource`) and Navigation Timing L2 (the
/// document's entry, its lifecycle in order, queued to a buffered observer once, after load).
#[test]
fn o6_resource_and_navigation_timing() {
    let mut e = load_with(
        &page(
            r#"
var full = 0, seen = [];
new PerformanceObserver(function (l) { l.getEntries().forEach(function (x) { seen.push(x.entryType + ':' + x.name); }); }).observe({ type: 'resource' });
new PerformanceObserver(function (l) { seen.push('nav:' + l.getEntries().map(function (x) { return x.type; }).join('+')); }).observe({ type: 'navigation', buffered: true });
performance.onresourcetimingbufferfull = function () { full++; performance.setResourceTimingBufferSize(10); };
performance.setResourceTimingBufferSize(2);
__rt_record('http://127.0.0.1:8000/a', 'fetch', 1, 2, 10, ['content-type', 'text/plain; charset=utf-8'], 'basic', 200);
__rt_record('http://other.test/b', 'fetch', 3, 4, 20, ['content-type', 'text/css'], 'cors', 200);
__rt_record('http://other.test/c', 'xmlhttprequest', 5, 6, 30, ['timing-allow-origin', 'http://127.0.0.1:8000', 'content-encoding', 'gzip', 'content-length', '12'], 'cors', 404);
var a = performance.getEntriesByName('http://127.0.0.1:8000/a')[0], b = performance.getEntriesByName('http://other.test/b')[0];
R.push(a instanceof PerformanceResourceTiming, a.initiatorType, a.decodedBodySize, a.encodedBodySize, a.transferSize, a.contentType, a.responseStatus, a.nextHopProtocol, a.fetchStart, a.responseStart);
R.push(b.transferSize, b.responseStart, b.decodedBodySize, b.nextHopProtocol, performance.getEntriesByType('resource').length);
var nav = performance.getEntriesByType('navigation')[0];
R.push(nav instanceof PerformanceNavigationTiming, nav.type, nav.name, nav.initiatorType, PerformanceObserver.supportedEntryTypes.join(','));
"#,
        ),
        5_000,
    );
    assert_eq!(
        eval(&mut e, "R.join('|')"),
        "true|fetch|10|10|310|text/plain|200|http/1.1|1|2|0|0|20||2|true|navigate|http://127.0.0.1:8000/wpt3.html|navigation|mark,measure,navigation,resource"
    );
    assert_eq!(
        eval(
            &mut e,
            "var c = performance.getEntriesByName('http://other.test/c')[0], n = performance.getEntriesByType('navigation')[0];
             [full, performance.getEntriesByType('resource').length, c.transferSize, c.encodedBodySize, c.decodedBodySize, c.responseStatus, c.contentEncoding, c.responseStart,
              n.domInteractive <= n.domContentLoadedEventStart && n.domContentLoadedEventStart <= n.domContentLoadedEventEnd && n.domContentLoadedEventEnd <= n.domComplete
                && n.domComplete <= n.loadEventStart && n.loadEventStart <= n.loadEventEnd && n.duration === n.loadEventEnd,
              seen.join(',')].join('|')"
        ),
        "1|3|312|12|30|404|gzip|6|true|resource:http://127.0.0.1:8000/a,resource:http://other.test/b,resource:http://other.test/c,nav:navigate"
    );
}

/// O8 — HTML §7.2.2.1 the window open steps (the URL parsed against the source document — SyntaxError; "tokenize the
/// features argument" and "parse a boolean feature" for `noopener` / `noreferrer`; null under noopener), §7.3.1.7 the
/// rules for choosing a navigable (`_self` / `_top`, a case-sensitive name match), §7.3.1.4 auxiliary browsing
/// contexts (`opener`, `opener = null` disowning, `top` / `parent` themselves, `close()` → `closed` at once and the
/// context gone after the queued close, an iframe inside it), a navigated auxiliary context running its script; and
/// §7.2.6 the Navigation API (entries / currentEntry / canGoBack, navigate() with state and info, the navigate event's
/// members, `currententrychange` and its `from`, a replaceState keeping the entry's navigation API state, an intercepted
/// fragment navigation firing no `hashchange` and leaving a null classic state, a canceled navigate event rejecting
/// `committed` with AbortError after `navigateerror`).
#[test]
fn o8_auxiliary_contexts_and_navigation_api() {
    let mut e = load_with(
        &page(
            r#"
var M = [], H = 0, N = [], K = 0;
addEventListener('message', function (e) { M.push(e.data); });
addEventListener('hashchange', function () { H++; });
var w = window.open();
R.push(w !== null && w !== window, w.opener === window, w.top === w, w.parent === w, w.name === '', w.closed, w.document.URL, w.frameElement);
var w2 = window.open('', 'named'), w3 = window.open('about:blank', 'NAMED'), w4 = window.open('about:blank', 'named');
R.push(w2 === w4, w2 !== w3, w2.name);
R.push(window.open('', '', ' noopener') === null, window.open('', '', 'noopener=0') !== null, window.open('', '', 'NOREFERRER=-1') === null,
       window.open('', '', 'noopenerRRR') !== null, window.open('', '', 'foo=\u000Cnoopener') !== null, window.open('', '', 'noopener=1.5') === null);
try { window.open('http://[bad'); R.push('no-throw'); } catch (x) { R.push(x.name); }
R.push(window.open('', '_self') === window, window.open('', '_top') === window);
w.document.body.appendChild(w.document.createElement('iframe'));
R.push(w.length);
w.close();
R.push(w.closed, w[0].closed);
w2.opener = null;
R.push(w2.opener, w3.opener === window);
var kid = window.open('data:text/html,<script>opener.postMessage("hi:" + (opener !== null) + ":" + name + ":" + (parent === window) + ":" + (top === window), "*")<\/script>', 'kid');
R.push(kid !== null);

navigation.onnavigate = function (e) {
    N.push('nav:' + e.navigationType + ':' + e.destination.url.split('/').pop() + ':' + e.hashChange + ':' + e.canIntercept + ':' + e.isTrusted + ':' + e.cancelable);
    if (e.destination.url.indexOf('#i') >= 0) { e.intercept({ handler: function () { return Promise.resolve(); } }); }
    if (e.destination.url.indexOf('#no') >= 0) { e.preventDefault(); }
};
navigation.oncurrententrychange = function (e) { N.push('cec:' + e.navigationType + ':' + e.from.index); };
navigation.onnavigatesuccess = function () { K++; };
navigation.onnavigateerror = function (e) { N.push('err:' + e.error.name); };
var e0 = navigation.currentEntry;
R.push(navigation.entries().length, e0.index, e0.url.split('/').pop(), navigation.canGoBack);
navigation.navigate('#a', { state: { s: 1 }, info: 'x' });
R.push(location.hash, navigation.currentEntry.getState().s, navigation.currentEntry.index, navigation.canGoBack);
history.replaceState('cs', '', '#b');
R.push(location.hash, history.state, navigation.currentEntry.getState().s, navigation.entries().length);
location.hash = 'i';
R.push(location.hash, String(history.state), navigation.entries().length);
try { new NavigateEvent('navigate'); } catch (x) { R.push(x.name); }
R.push(navigation.activation.navigationType);
"#,
        ),
        5_000,
    );
    assert_eq!(
        eval(&mut e, "R.join('|')"),
        "true|true|true|true|true|false|about:blank||true|true|named|true|true|true|true|true|true|SyntaxError|true|true|1|true|false||true|true\
         |1|0|wpt3.html|false|#a|1|1|true|#b|cs|1|2|#i|null|3|TypeError|push"
    );
    assert_eq!(run_until(&mut e, "M.length > 0", "M.join()", 10), "hi:true:kid:true:true");
    assert_eq!(
        eval(&mut e, "[w.closed, typeof w[0], N.join(), K, H].join('|')"),
        "true|undefined|nav:push:wpt3.html#a:true:true:true:true,cec:push:0,nav:replace:wpt3.html#b:true:true:true:true,cec:replace:-1,\
         nav:push:wpt3.html#i:true:true:true:true,cec:push:1|3|1"
    );
    eval(&mut e, "N = []; navigation.navigate('#no').committed.catch(function (x) { N.push('rej:' + x.name); }); 0");
    assert_eq!(eval(&mut e, "N.join() + '|' + location.hash"), "nav:push:wpt3.html#no:true:true:true:true,err:AbortError,rej:AbortError|#i");
}
