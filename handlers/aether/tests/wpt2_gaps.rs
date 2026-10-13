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
