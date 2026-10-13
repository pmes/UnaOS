//! AETHERWPT2 (LEDGER SR135): one test per gap the series closed (docs/dev/evidence/aetherwpt/WPT.md, the
//! AETHERWPT2 gap list), each on a headless page asserting what the WPT directory's tests read back. The
//! expected values are the specification's (section cited per test), the same ones Chromium answers.

fn load(html: &str) -> aether::AetherEngine {
    let mut engine = aether::AetherEngine::new();
    engine.load_html("http://127.0.0.1:8000/wpt2.html", html, true);
    if let Some(js) = engine.js_engine.as_mut() {
        aether::event_loop::settle(js, 2_000);
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
    let mut e = load(&page(script));
    eval(&mut e, "R.join('|')")
}

/// G1 — URL §6.2: live `URLSearchParams Iterator`s over one query object per URL (the `search` setter
/// updates that object in place), `has` / `delete` with a value, the sequence-of-pairs check, records.
#[test]
fn g1_urlsearchparams_per_url_standard() {
    let got = answers(
        r#"
var a = new URL('http://a.b/c?a=1&b=2&c=3&d=4'), b = a.searchParams, seen = [];
for (const i of b) { a.search = 'x=1&y=2&z=3'; seen.push(i.join('=')); }
R.push(seen.join(','), a.searchParams === b);
var p = new URLSearchParams('a=1&a=2&b=3');
p.delete('a', '2');
R.push(p + '', p.has('a', '1'), p.has('a', '2'));
try { new URLSearchParams([['a']]); R.push('no throw'); } catch (e) { R.push(e.name); }
R.push(new URLSearchParams({ a: '1', b: 'x y' }) + '');
R.push(Object.prototype.toString.call(p.entries()), p.size, URLSearchParams.prototype[Symbol.iterator] === URLSearchParams.prototype.entries);
var u = new URL('http://h/?q=1'), sp = u.searchParams;
sp.append('z', '9');
R.push(u.href);
u.href = 'http://h/?k=v';
R.push(sp.get('k'), u.searchParams === sp);
"#,
    );
    assert_eq!(
        got,
        "a=1,y=2,z=3|true|a=1&b=3|true|false|TypeError|a=1&b=x+y|[object URLSearchParams Iterator]|2|true|http://h/?q=1&z=9|v|true"
    );
}

/// G2 — Encoding §4.2 / §7.2 / §8.1.1 / §14.2: labels, `replacement` refused, UTF-8 errors one U+FFFD per
/// maximal subpart, the BOM, `fatal`, `{stream: true}`, UTF-16LE surrogates, windows-1252, `encodeInto`.
#[test]
fn g2_textdecoder_per_encoding_standard() {
    let got = answers(
        r#"
var hex = function (s) { var o = []; for (var i = 0; i < s.length; i++) { o.push(s.charCodeAt(i).toString(16)); } return o.join(' '); };
var d = function (b, l, o) { return hex(new TextDecoder(l, o).decode(new Uint8Array(b))); };
R.push(new TextDecoder(' Latin1 ').encoding, new TextDecoder('unicode').encoding, new TextDecoder('SJIS').encoding);
try { new TextDecoder('iso-2022-kr'); R.push('no throw'); } catch (e) { R.push(e.name); }
R.push(d([0xED, 0xA0, 0x80]), d([0xC0, 0x80]), d([0xE2, 0x82]), d([0xEF, 0xBB, 0xBF, 0x41]), d([0xEF, 0xBB, 0xBF, 0x41], 'utf-8', { ignoreBOM: true }));
try { new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xFF])); R.push('no throw'); } catch (e) { R.push(e.name); }
var t = new TextDecoder(), s = t.decode(new Uint8Array([0xF0, 0x9F]), { stream: true });
s += t.decode(new Uint8Array([0x92, 0xA9]));
R.push(hex(s), d([0x3D, 0xD8, 0x00, 0xDC], 'utf-16le'), d([0x00, 0xD8, 0x41, 0x00], 'utf-16le'), d([0x80, 0x9F, 0xE9], 'ascii'));
var dst = new Uint8Array(4), r = new TextEncoder().encodeInto('aé💩', dst);
R.push(r.read + ',' + r.written);
"#,
    );
    assert_eq!(got, "windows-1252|utf-16le|shift_jis|RangeError|fffd fffd fffd|fffd fffd|fffd|41|feff 41|TypeError|d83d dca9|d83d dc00|fffd 41|20ac 178 e9|2,3");
}

/// G3 — Streams §7.2/§7.3 (the two queuing strategies: one size function per global, named `size`, not a
/// constructor, the required `highWaterMark`) and §4.5 (a ReadableStream's desiredSize counts the strategy's
/// sizes; a size that is not a finite non-negative number errors the stream with a RangeError).
#[test]
fn g3_queuing_strategies_and_size() {
    let got = answers(
        r#"
var c1 = new CountQueuingStrategy({ highWaterMark: 5 }), c2 = new CountQueuingStrategy({ highWaterMark: 1 });
var b = new ByteLengthQueuingStrategy({ highWaterMark: 16 });
R.push(c1.size === c2.size, c1.size.name, 'prototype' in c1.size, c1.size.length, b.size.length, b.size({ byteLength: 7 }), c1.highWaterMark);
try { new CountQueuingStrategy({}); R.push('no throw'); } catch (e) { R.push(e.name); }
try { new c1.size(); R.push('no throw'); } catch (e) { R.push(e.name); }
R.push(new ByteLengthQueuingStrategy({ highWaterMark: '0' }).highWaterMark, String(new CountQueuingStrategy({ highWaterMark: 'foo' }).highWaterMark));
var ctl;
var rs = new ReadableStream({ start: function (c) { ctl = c; } }, new ByteLengthQueuingStrategy({ highWaterMark: 10 }));
ctl.enqueue(new Uint8Array(4));
R.push(ctl.desiredSize);
var ctl2;
new ReadableStream({ start: function (c) { ctl2 = c; } }, { size: function () { return -1; } });
try { ctl2.enqueue('x'); R.push('no throw'); } catch (e) { R.push(e.name); }
R.push(ctl2.desiredSize);
"#,
    );
    assert_eq!(got, "true|size|false|0|1|7|5|TypeError|TypeError|0|NaN|6|RangeError|");
}

/// G4 — File API §3.1 / §4.1: blobParts is an iterable (an array-like is refused), `endings: "native"`
/// converts string line endings, an invalid `endings` is a TypeError, `File.lastModified` is a `long long`.
#[test]
fn g4_blob_and_file_constructors() {
    let mut e = load(&page(
        r#"
try { new Blob({ length: 1, 0: 'x' }); R.push('no throw'); } catch (e) { R.push(e.name); }
try { new Blob([], { endings: 'bogus' }); R.push('no throw'); } catch (e) { R.push(e.name); }
var set = new Set(['a', 'b']);
R.push(new Blob(set).size, new Blob(['a\r\nb\rc'], { endings: 'native' }).size, new Blob(['a\r\nb'], { endings: 'transparent' }).size);
R.push(new File([], 'f', { lastModified: -1 }).lastModified, new File([], 'f', { lastModified: NaN }).lastModified, new File([], 'f', { lastModified: 1.9 }).lastModified);
new Blob(['a\r\nb\rc'], { endings: 'native' }).text().then(function (t) { R.push(JSON.stringify(t)); });
"#,
    ));
    assert_eq!(eval(&mut e, "R.join('|')"), "TypeError|TypeError|2|5|4|-1|0|1|\"a\\nb\\nc\"");
}

/// G5 — HTML §4.9.1: `caption` / `tHead` / `tFoot` setters (type check, HierarchyRequestError, placement), the
/// create / delete methods and `createTBody` placement.
#[test]
fn g5_table_caption_thead_tfoot_tbody() {
    let got = answers(
        r#"
var t = document.createElement('table');
t.appendChild(document.createElement('colgroup'));
t.appendChild(document.createElement('tbody'));
var tags = function () { return Array.prototype.map.call(t.childNodes, function (n) { return n.localName; }).join(','); };
var h = t.createTHead(), c = t.createCaption(), f = t.createTFoot();
R.push(tags(), t.createTHead() === h, t.tHead === h);
var b = t.createTBody();
R.push(tags(), b.previousSibling.localName);
try { t.caption = document.createElement('div'); R.push('no throw'); } catch (e) { R.push(e.name); }
try { t.tHead = document.createElement('tbody'); R.push('no throw'); } catch (e) { R.push(e.name); }
var c2 = document.createElement('caption');
t.caption = c2;
R.push(t.caption === c2, t.firstChild === c2, c.parentNode === null);
t.deleteTHead(); t.deleteTFoot(); t.deleteCaption();
R.push(tags());
t.tFoot = f;
R.push(tags());
t.caption = null;
R.push(String(t.caption));
"#,
    );
    assert_eq!(
        got,
        "caption,colgroup,thead,tbody,tfoot|true|true|caption,colgroup,thead,tbody,tbody,tfoot|tbody|TypeError|HierarchyRequestError|true|true|true|colgroup,tbody,tbody|colgroup,tbody,tbody,tfoot|null"
    );
}

/// G9 — Console §1.2/§1.4 and WebIDL §3.13: a throwing label conversion propagates; the namespace object's
/// [[Prototype]] is an empty object whose [[Prototype]] is %Object.prototype%.
#[test]
fn g9_console_namespace_and_labels() {
    let got = answers(
        r#"
var proto = Object.getPrototypeOf(console);
R.push(Object.getOwnPropertyNames(proto).length, Object.getPrototypeOf(proto) === Object.prototype, Object.prototype.toString.call(console));
['count', 'countReset', 'time', 'timeLog', 'timeEnd'].forEach(function (m) {
    try { console[m]({ toString: function () { throw new Error('conversion error'); } }); R.push(m + ':no throw'); }
    catch (e) { R.push(m + ':' + e.message); }
});
"#,
    );
    assert_eq!(got, "0|true|[object console]|count:conversion error|countReset:conversion error|time:conversion error|timeLog:conversion error|timeEnd:conversion error");
}
