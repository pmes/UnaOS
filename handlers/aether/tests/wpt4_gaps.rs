//! AETHERWPT4 (LEDGER SR142): one test per exclusion group this series lifted from the WPT runner's predicates
//! (docs/dev/evidence/aetherwpt/WPT.md, the AETHERWPT4 section, groups W1–W10), each on a headless page asserting what
//! the lifted directory's tests read back. The expected values are the specifications' (section cited per test).

fn load_with(html: &str, budget_ms: u64) -> aether::AetherEngine {
    let mut engine = aether::AetherEngine::new();
    engine.load_html("http://127.0.0.1:8000/wpt4.html", html, true);
    if let Some(js) = engine.js_engine.as_mut() {
        aether::event_loop::settle(js, budget_ms);
    }
    engine
}

fn eval(engine: &mut aether::AetherEngine, src: &str) -> String {
    let js = engine.js_engine.as_mut().expect("page has a script engine");
    js.eval_string(src).unwrap_or_else(|e| format!("THROW {e}"))
}

/// A page with `body` markup and one script that stores its answers in `window.R` (joined with `|` by the test).
fn page_with(body: &str, script: &str) -> String {
    format!("<!doctype html><html><head></head><body>{body}<script>var R = [];\n{script}\n</script></body></html>")
}

fn answers(body: &str, script: &str) -> String {
    let mut e = load_with(&page_with(body, script), 5_000);
    eval(&mut e, "R.join('|')")
}

/// W1 — HTML §7.2.2.1 the window open steps and §7.3.1.4 auxiliary browsing contexts (AETHERWPT3 O8), the
/// implementation behind lifting `window.open(` from all three predicates: a named auxiliary context, its `opener`,
/// `close()` making `closed` true at once.
#[test]
fn w1_window_open_auxiliary_context() {
    let got = answers(
        "",
        r#"
var w = window.open('about:blank', 'aux4');
R.push(w !== null, w.opener === window, w.name, w.closed);
w.close();
R.push(w.closed);
"#,
    );
    assert_eq!(got, "true|true|aux4|false|true");
}

/// W2 — CSSOM §9 getComputedStyle over the author cascade (Cascade 5 §6 specificity and the style attribute
/// winning, §7.2 inheritance of an undeclared inherited property, §7.3 `inherit`) for properties the layout report
/// does not compute.
#[test]
fn w2_get_computed_style_cascade() {
    let got = answers(
        r#"<style>.p { word-spacing: 4px; } #t { letter-spacing: 2px; padding-left: 3px; opacity: 0.5 } #t.k { letter-spacing: 5px } #u { cursor: inherit }</style>
<div class="p" style="cursor: pointer"><span id="t" class="k" style="opacity: 0.25">x</span><span id="u">y</span></div>"#,
        r#"
var t = getComputedStyle(document.getElementById('t')), u = getComputedStyle(document.getElementById('u'));
R.push(t.getPropertyValue('letter-spacing'), t.getPropertyValue('word-spacing'), t.getPropertyValue('padding-left'), t.getPropertyValue('opacity'), u.getPropertyValue('cursor'));
"#,
    );
    assert_eq!(got, "5px|4px|3px|0.25|pointer");
}

/// W3 — HTML §8.10 animation frames: callbacks run in registration order with one timestamp per frame, a canceled one
/// does not run (the `requestAnimationFrame` predicate token lifted).
#[test]
fn w3_request_animation_frame() {
    let mut e = load_with(
        &page_with(
            "",
            r#"
var ts = [], order = [];
var h = requestAnimationFrame(function () { order.push('c'); });
requestAnimationFrame(function (t) { ts.push(t); order.push('a'); });
requestAnimationFrame(function (t) { ts.push(t); order.push('b'); });
cancelAnimationFrame(h);
"#,
        ),
        2_000,
    );
    assert_eq!(eval(&mut e, "order.join(',') + '|' + (ts.length === 2 && ts[0] === ts[1] && typeof ts[0] === 'number')"), "a,b|true");
}

/// W4 — XHR §3.6.10 document response and DOMParser (HTML §8.5.1) over the XML 1.0 + Namespaces parser: namespaces,
/// prefixes and attribute namespaces, entity and CDATA text, the document's content type; a non-well-formed body
/// gives null; an HTML document for `responseType = "document"`; DOMParser's XML types, its parsererror document and
/// the enum TypeError.
#[test]
fn w4_xhr_document_response_and_xml_parser() {
    let mut e = load_with(
        &page_with(
            "",
            r#"
var R2 = null;
var x = new XMLHttpRequest();
x.open('GET', 'data:application/xml,' + encodeURIComponent('<?xml version="1.0"?><r xmlns="urn:x" xmlns:p="urn:p" p:a="1"><p:c>t&amp;</p:c><![CDATA[<z>]]></r>'), false);
x.send();
var d = x.responseXML, de = d.documentElement;
R.push(de.namespaceURI, de.getAttributeNS('urn:p', 'a'), de.firstChild.prefix, de.textContent, d.contentType, x.responseXML === d);
var bad = new XMLHttpRequest();
bad.open('GET', 'data:text/xml,%3Cx%3E', false);
bad.send();
R.push(String(bad.responseXML));
var h = new XMLHttpRequest();
h.open('GET', 'data:text/html,%3Cp%3Ehi', false);
h.send();
R.push(String(h.responseXML));
var y = new XMLHttpRequest();
y.responseType = 'document';
y.open('GET', 'data:text/html,' + encodeURIComponent('<!doctype html><title>T</title><p id=q>hi'));
y.onload = function () { R2 = [y.response.getElementById('q').textContent, y.response.title, y.responseXML === y.response, y.response.contentType].join(','); };
y.send();
var p = new DOMParser(), s = p.parseFromString('<a:b xmlns:a="urn:a"/>', 'image/svg+xml');
R.push(s.documentElement.namespaceURI, s.documentElement.localName, s.contentType, p.parseFromString('<a>', 'text/xml').documentElement.localName);
try { p.parseFromString('<a/>', 'text/plain'); R.push('no throw'); } catch (err) { R.push(err.name); }
"#,
        ),
        3_000,
    );
    assert_eq!(eval(&mut e, "R.join('|')"), "urn:x|1|p|t&<z>|application/xml|true|null|null|urn:a|b|image/svg+xml|parsererror|TypeError");
    assert_eq!(eval(&mut e, "String(R2)"), "hi,T,true,text/html");
}

/// W5 — Fetch §5 Body `formData()`: the multipart/form-data parser (a part with a filename becomes a File with its
/// type) round-tripping a FormData body, the urlencoded parser, and a TypeError for any other MIME type.
#[test]
fn w5_form_data_body_parsers() {
    let mut e = load_with(
        &page_with(
            "",
            r#"
var fd = new FormData();
fd.append('a', '1');
fd.append('f', new Blob(['xyz'], { type: 'text/csv' }), 'n.csv');
Promise.all([
    new Response(fd).formData().then(function (g) { var f = g.get('f'); return f.text().then(function (t) { return [g.get('a'), f.name, f.type, f.size, t].join(','); }); }),
    new Response('a=1&b=%20x', { headers: { 'content-type': 'application/x-www-form-urlencoded' } }).formData().then(function (g) { return g.get('b'); }),
    new Response('x', { headers: { 'content-type': 'text/plain' } }).formData().then(function () { return 'resolved'; }, function (err) { return err.name; }),
]).then(function (r) { R = r; }, function (err) { R = ['rejected ' + err]; });
"#,
        ),
        3_000,
    );
    assert_eq!(eval(&mut e, "R.join('|')"), "1,n.csv,text/csv,3,xyz| x|TypeError");
}

/// W6 — the network APIs the `dom/` and `html/` directories' tests call (`XMLHttpRequest`, `fetch(`, no longer an
/// exclusion), and the Fetch data: URL processor they read through: the body percent-decoded to bytes.
#[test]
fn w6_network_apis_in_dom_tests() {
    let mut e = load_with(
        &page_with(
            "",
            r#"
var x = new XMLHttpRequest();
x.open('GET', 'data:,hi', false);
x.send();
R.push(x.responseText);
Promise.all([
    fetch('data:,hi').then(function (r) { return r.text(); }),
    fetch('data:text/plain;charset=windows-1252,%E9').then(function (r) { return r.arrayBuffer(); }).then(function (b) { var u = new Uint8Array(b); return u.length + ':' + u[0]; }),
    fetch('data:;base64,aGk=').then(function (r) { return r.text().then(function (t) { return r.headers.get('content-type') + ':' + t; }); }),
]).then(function (r) { R = R.concat(r); });
"#,
        ),
        3_000,
    );
    assert_eq!(eval(&mut e, "R.join('|')"), "hi|hi|1:233|text/plain;charset=US-ASCII:hi");
}

/// W7 — CSSOM View §7 `scrollIntoView`: block `start` (the default), `end` (`false`), `center`, the viewport scrolled
/// at once; an invalid ScrollLogicalPosition is a TypeError.
#[test]
fn w7_scroll_into_view() {
    let got = answers(
        r#"<div style="height:3000px"></div><div id="t" style="height:50px">t</div><div style="height:3000px"></div>"#,
        r#"
var t = document.getElementById('t');
t.scrollIntoView();
var r1 = t.getBoundingClientRect();
R.push(Math.round(r1.top), scrollY > 0);
t.scrollIntoView(false);
var r2 = t.getBoundingClientRect();
R.push(Math.abs(r2.bottom - innerHeight) < 1);
t.scrollIntoView({ block: 'center' });
var r3 = t.getBoundingClientRect();
R.push(Math.abs(r3.top + r3.height / 2 - innerHeight / 2) < 1);
try { t.scrollIntoView({ block: 'middle' }); R.push('no throw'); } catch (err) { R.push(err.name); }
"#,
    );
    assert_eq!(got, "0|true|true|true|TypeError");
}

/// W8 — XHR §3.5.6 upload progress: `loadstart` at the upload object after the request's, "process request
/// end-of-body" (`progress`, `load`, `loadend` with the body length) before the response head; the request error
/// steps at an incomplete upload (`abort` and `loadend` at the upload object first, the state reset after).
#[test]
fn w8_xhr_upload_progress_events() {
    let mut e = load_with(
        &page_with(
            "",
            r#"
var log = [], log2 = [];
var x = new XMLHttpRequest();
var rec = function (L, xhr) { return function (e) { L.push((e.target === xhr.upload ? 'upload.' : '') + e.type + '(' + e.loaded + ',' + e.total + ',' + e.lengthComputable + ')'); }; };
['loadstart', 'progress', 'load', 'loadend', 'abort'].forEach(function (k) { x.addEventListener(k, rec(log, x)); x.upload.addEventListener(k, rec(log, x)); });
x.onreadystatechange = function () { log.push(x.readyState); };
x.open('POST', 'data:,ok');
x.send('body');
var y = new XMLHttpRequest();
['abort', 'loadend'].forEach(function (k) { y.addEventListener(k, rec(log2, y)); y.upload.addEventListener(k, rec(log2, y)); });
y.upload.onloadstart = function () { y.abort(); };
y.open('POST', 'data:,ok');
y.send('body');
log2.push(y.readyState);
"#,
        ),
        3_000,
    );
    assert_eq!(
        eval(&mut e, "log.join(',')"),
        "1,loadstart(0,0,false),upload.loadstart(0,4,true),upload.progress(4,4,true),upload.load(4,4,true),upload.loadend(4,4,true),2,3,progress(2,0,false),4,progress(2,0,false),load(2,0,false),loadend(2,0,false)"
    );
    assert_eq!(eval(&mut e, "log2.join(',')"), "upload.abort(0,0,false),upload.loadend(0,0,false),abort(0,0,false),loadend(0,0,false),0");
}

/// W9 — CSSOM View §6.1 `elementFromPoint` / `elementsFromPoint`: the topmost box at a viewport point (later
/// positioned boxes above earlier ones), every box there topmost first with the root element last, null outside the
/// viewport, the root element where no box is hit.
#[test]
fn w9_element_from_point() {
    let got = answers(
        r#"<div id="a" style="position:absolute;left:0;top:0;width:100px;height:100px"></div><div id="b" style="position:absolute;left:50px;top:50px;width:100px;height:100px"></div>"#,
        r#"
var all = document.elementsFromPoint(75, 75);
R.push(document.elementFromPoint(75, 75).id, document.elementFromPoint(10, 10).id,
       all.filter(function (e) { return e.id; }).map(function (e) { return e.id; }).join(','), all[all.length - 1] === document.documentElement,
       String(document.elementFromPoint(-1, 5)), document.elementFromPoint(300, 200) === document.documentElement || document.elementFromPoint(300, 200) === document.body);
"#,
    );
    assert_eq!(got, "b|a|b,a|true|null|true");
}

/// W10 — XHR §3.6 the final MIME type and encoding (`overrideMimeType`'s charset over the response's, a charset that
/// names no encoding giving UTF-8, an override without one keeping the response's), Encoding §6 "decode" (a BOM wins),
/// the XML declaration's encoding for an XML response, and a blob: URL's XML body as responseXML.
#[test]
fn w10_xhr_final_mime_type_and_encoding() {
    let got = answers(
        "",
        r#"
var t = function (url, ov) { var x = new XMLHttpRequest(); x.open('GET', url, false); if (ov) { x.overrideMimeType(ov); } x.send(); return x; };
var u = 'data:text/plain;charset=windows-1252,%C2%F0';
R.push(encodeURIComponent(t(u).responseText), encodeURIComponent(t(u, 'text/plain;charset=UTF-8').responseText), encodeURIComponent(t(u, 'text/xml').responseText), encodeURIComponent(t(u, 'text/plain;charset=342').responseText));
R.push(t('data:text/plain,%FE%FF%00a').responseText);
R.push(t('data:application/xml,' + encodeURIComponent("<?xml version='1.0' encoding='windows-1252'?><x>") + '%E6' + encodeURIComponent('</x>')).responseXML.documentElement.textContent === 'æ');
var b = URL.createObjectURL(new Blob(['<r/>'], { type: 'text/xml' }));
R.push(t(b).responseXML.documentElement.localName);
"#,
    );
    assert_eq!(got, "%C3%82%C3%B0|%EF%BF%BD%EF%BF%BD|%C3%82%C3%B0|%EF%BF%BD%EF%BF%BD|a|true|r");
}
