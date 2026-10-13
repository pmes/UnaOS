//! Platform APIs that are *real*, not shaped-like-real: `URL` /
//! `URLSearchParams` over UnaOS's own WHATWG URL parser (`html_core::url`,
//! URLCORE SR87 — the one the network stack parses with, via http_core), `TextEncoder`/`TextDecoder` (the WHATWG
//! label table, UTF-8 / UTF-16 / windows-1252 / x-user-defined decoders, fatal / ignoreBOM / stream — AETHERWPT2,
//! SR135), the `AbortController` object
//! graph, `DOMParser` over the same `html_core` parser (`dom::parse_html`) the
//! `innerHTML` setter uses, and `crypto` seeded from the OS entropy pool.
//!
//! The split follows `api::fetch`: anything that needs engine truth (URL
//! resolution, HTML parsing, entropy) is a small native; the WHATWG object
//! shape around it is a JS prelude. Every place the implementation stops
//! short of the spec records itself through `__ledger` or
//! `crate::ledger::record_js`, so a page that leaned on the missing part
//! shows up as a gap rather than as silence.

use crate::js::idl;
use html_core::url;
use js_core::vm::*;

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = vm.arg(ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

/// Serialises a parsed `Url` into the component bag the JS `URL` class reads. Every field is what
/// `html_core::url` actually computed — none of them are reconstructed by string surgery in JS.
fn components(u: &url::Url, vm: &mut Vm) -> JsResult<Value> {
    let o = vm.new_plain_object();
    for part in ["href", "protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash", "origin"] {
        let v = crate::js::html::url_part(u, part);
        vm.create_data_property(o, PropertyKey::from_str(part), idl::s(&v))?;
    }
    Ok(Value::Object(o))
}

/// Reads `n` bytes from the OS entropy pool. `/dev/urandom` is the real
/// source and is present on every platform this engine runs on (and inside
/// the UnaOS userspace); when it cannot be read the fallback mixes
/// `RandomState` — which is itself seeded from the OS — with the monotonic
/// clock, and the substitution is ledgered so a page that needed
/// cryptographic quality can see it did not get it. No fixed fill, ever.
pub(crate) fn entropy(n: usize) -> Vec<u8> {
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

/// `__url_parse(input, base)` → components | null. Relative resolution is html_core::url's, so
/// `new URL('../a', 'https://h/x/y/z')` lands exactly where the network stack would fetch it from.
fn url_parse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let input = str_arg(vm, ctx, 0)?;
    let base = vm.arg(ctx, 1);
    let parsed = if base.is_nullish() {
        url::Url::parse(&input)
    } else {
        let base = str_arg(vm, ctx, 1)?;
        url::Url::parse(&base).and_then(|b| b.join(&input))
    };
    match parsed {
        Ok(u) => components(&u, vm),
        Err(_) => Ok(Value::Null),
    }
}

/// `__url_set(href, part, value)` → components | null. Component writes go through the URL Standard's
/// setters (a basic URL parse with that component's state override).
fn url_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let href = str_arg(vm, ctx, 0)?;
    let part = str_arg(vm, ctx, 1)?;
    let value = str_arg(vm, ctx, 2)?;
    let Ok(mut u) = url::Url::parse(&href) else { return Ok(Value::Null) };
    let before = u.to_string();
    crate::js::html::set_url_part(&mut u, &part, &value);
    if u.href() == before && !value.is_empty() && part != "search" && part != "hash" {
        crate::ledger::record_js(&format!("URL-set-refused:{part}"));
    }
    components(&u, vm)
}

/// `__form_parse(s)` → `[[name, value], ...]`: the application/x-www-form-urlencoded parser (URL §5.1).
fn form_parse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let input = str_arg(vm, ctx, 0)?;
    let mut rows = Vec::new();
    for (n, v) in url::form::parse_str(&input) {
        rows.push(Value::Object(vm.new_array(vec![idl::s(&n), idl::s(&v)])));
    }
    Ok(Value::Object(vm.new_array(rows)))
}

/// `__form_byte(s)` → the urlencoded byte serializer of `s` (URL §5.2: form set, space as `+`).
fn form_byte(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let input = str_arg(vm, ctx, 0)?;
    Ok(idl::s(&url::form::byte_serialize(input.as_bytes())))
}

/// AETHERWPT3 (SR138): `__codec_decode(name, s0, s1, s2, s3, bytes, flush)` → `[text, s0, s1, s2, s3, errors]` — one
/// run of a legacy decoder (api/encoding: Encoding §9–§13) from the given state; TextDecoder holds the state
/// between `{stream: true}` calls.
fn codec_decode(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let name = str_arg(vm, ctx, 0)?;
    let mut st = [0u32; 4];
    for (k, slot) in st.iter_mut().enumerate() {
        let v = vm.arg(ctx, 1 + k);
        *slot = vm.to_number(&v)? as u32;
    }
    let b = vm.arg(ctx, 5);
    let bytes = crate::js::net::buffer_bytes(vm, &b).unwrap_or_default();
    let f = vm.arg(ctx, 6);
    let flush = vm.to_boolean(&f);
    let (out, next) = crate::api::encoding::decode_chunk(&name, crate::api::encoding::DecoderState(st), &bytes, flush);
    let mut vals = vec![idl::s(&String::from_utf16_lossy(&out.units))];
    vals.extend(next.0.iter().map(|&x| Value::Number(x as f64)));
    vals.push(Value::Number(out.errors as f64));
    Ok(Value::Object(vm.new_array(vals)))
}

/// AETHERWPT3 (SR138): `__blob_url_register(origin, bytes, type)` → a new blob URL (File API §8.3.1, api/blobstore.rs).
fn blob_url_register(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let origin = str_arg(vm, ctx, 0)?;
    let b = vm.arg(ctx, 1);
    let bytes = crate::js::net::buffer_bytes(vm, &b).unwrap_or_default();
    let mime = str_arg(vm, ctx, 2)?;
    Ok(idl::s(&crate::api::blobstore::register(&origin, bytes, &mime)))
}

/// `__blob_url_revoke(url, origin)` (File API §8.3.3).
fn blob_url_revoke(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let url = str_arg(vm, ctx, 0)?;
    let origin = str_arg(vm, ctx, 1)?;
    crate::api::blobstore::revoke(&url, &origin);
    Ok(Value::Undefined)
}

/// `__blob_url_resolve(url, origin)` → `[ArrayBuffer, type]` or null (File API §8.3.4, partitioned by origin).
fn blob_url_resolve(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let url = str_arg(vm, ctx, 0)?;
    let origin = str_arg(vm, ctx, 1)?;
    let Some(e) = crate::api::blobstore::resolve(&url, Some(&origin)) else { return Ok(Value::Null) };
    let ab = js_core::builtins::typedarray::new_array_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(ab).kind {
        d.data = e.bytes.to_vec();
    }
    Ok(Value::Object(vm.new_array(vec![Value::Object(ab), idl::s(&e.mime)])))
}

/// AETHERWPT4 (SR142): `__decode_bytes(bytes, label)` → `[text, encoding]` — Encoding §6 "decode": a UTF-8 / UTF-16 BOM
/// wins and is dropped, else `label`'s encoding (Encoding §4.2), else UTF-8 (XHR §3.6.9 "text response").
fn decode_bytes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = vm.arg(ctx, 0);
    let bytes = crate::js::net::buffer_bytes(vm, &b).unwrap_or_default();
    let l = vm.arg(ctx, 1);
    let enc = if l.is_null() || l.is_undefined() { None } else { crate::api::encoding::get_encoding(&vm.to_string(&l)?.to_rust()) };
    let (text, used) = crate::api::encoding::decode(&bytes, enc.unwrap_or("UTF-8"));
    Ok(Value::Object(vm.new_array(vec![idl::s(&text), idl::s(used)])))
}

/// AETHERWPT4 (SR142): `__get_encoding(label)` → the canonical encoding name, or null (Encoding §4.2 "get an encoding").
fn get_encoding_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let l = str_arg(vm, ctx, 0)?;
    Ok(crate::api::encoding::get_encoding(&l).map(idl::s).unwrap_or(Value::Null))
}

/// AETHERWPT4 (SR142): `__html_prescan(bytes)` → the encoding HTML §13.2.3.2's prescan of the first 1024 bytes finds, or
/// null (XHR §3.6.10 "document response" for an HTML document without a final encoding).
fn html_prescan(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = vm.arg(ctx, 0);
    let bytes = crate::js::net::buffer_bytes(vm, &b).unwrap_or_default();
    Ok(crate::dom::charset::prescan(&bytes).map(idl::s).unwrap_or(Value::Null))
}

/// AETHERWPT4 (SR142): `__xml_sniff(bytes)` → the encoding a leading XML declaration names (canonical), or null.
fn xml_sniff(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = vm.arg(ctx, 0);
    let bytes = crate::js::net::buffer_bytes(vm, &b).unwrap_or_default();
    let e = crate::api::xml::sniff_declared_encoding(&bytes).and_then(|l| crate::api::encoding::get_encoding(&l));
    Ok(e.map(idl::s).unwrap_or(Value::Null))
}

/// AETHERWPT4 (SR142): `__parse_xml(text)` → `[encoding|null, items…]` (api/xml.rs), or `[null]` plus the error as a
/// string when the text is not namespace-well-formed: `typeof r[1] === 'string'`. Items are arrays: `['d', name,
/// publicId, systemId]`, `['s', ns, qname, [ns, qname, value, …]]`, `['e']`, `['t', text]`, `['c', text]` (CDATA),
/// `['m', data]` (comment), `['p', target, data]`.
fn parse_xml(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    use crate::api::xml::Tok;
    let text = str_arg(vm, ctx, 0)?;
    let parsed = match crate::api::xml::parse(&text) {
        Ok(p) => p,
        Err(e) => return Ok(Value::Object(vm.new_array(vec![Value::Null, idl::s(&e)]))),
    };
    let opt = |v: &Option<String>| v.as_deref().map(idl::s).unwrap_or(Value::Null);
    let mut out = vec![opt(&parsed.encoding)];
    for t in &parsed.toks {
        let row = match t {
            Tok::Doctype { name, public_id, system_id } => vec![idl::s("d"), idl::s(name), idl::s(public_id), idl::s(system_id)],
            Tok::Start { ns, qname, attrs } => {
                let mut a = Vec::with_capacity(attrs.len() * 3);
                for (ans, q, v) in attrs {
                    a.push(opt(ans));
                    a.push(idl::s(q));
                    a.push(idl::s(v));
                }
                let arr = Value::Object(vm.new_array(a));
                vec![idl::s("s"), opt(ns), idl::s(qname), arr]
            }
            Tok::End => vec![idl::s("e")],
            Tok::Text(x) => vec![idl::s("t"), idl::s(x)],
            Tok::CData(x) => vec![idl::s("c"), idl::s(x)],
            Tok::Comment(x) => vec![idl::s("m"), idl::s(x)],
            Tok::Pi { target, data } => vec![idl::s("p"), idl::s(target), idl::s(data)],
        };
        out.push(Value::Object(vm.new_array(row)));
    }
    Ok(Value::Object(vm.new_array(out)))
}

/// AETHERWPT4 (SR142): `__doc_meta(doc, contentType, url, charset)` — a created document's content type, URL and
/// encoding (XHR §3.6.10 step 9; DOMParser's document takes the type it was asked for).
fn doc_meta(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = vm.arg(ctx, 0);
    let Some(id) = crate::js::dom::node_of(vm, &d) else { return vm.throw_type("__doc_meta: not a node") };
    let ct = str_arg(vm, ctx, 1)?;
    let url = str_arg(vm, ctx, 2)?;
    let cs = str_arg(vm, ctx, 3)?;
    crate::js::page(|p| {
        if let Some(m) = p.docs.get_mut(&id) {
            m.content_type = ct;
            m.url = url;
            m.charset = cs;
        }
    });
    Ok(Value::Undefined)
}

/// `__random_bytes(n)` → array of n bytes from the OS entropy pool.
fn random_bytes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    let n = vm.to_number(&v)?.clamp(0.0, 65536.0) as usize;
    let vals: Vec<Value> = entropy(n).into_iter().map(|b| Value::Number(b as f64)).collect();
    Ok(Value::Object(vm.new_array(vals)))
}

/// `__parse_document(html)` → a new Document (HTML §8.5.1 DOMParser): parsed by html_core, imported
/// whole into the page arena as its own document (scripts marked already started — they never run).
fn parse_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let html = str_arg(vm, ctx, 0)?;
    let parsed = html_core::parse_document(&html, crate::dom::parse_opts());
    let url = crate::js::page_url();
    let doc = crate::js::dom::new_document(true, "text/html", &url);
    let top: Vec<html_core::NodeId> = parsed.children(html_core::Document::ROOT).collect();
    for t in top {
        let id = crate::js::dom::with_doc_mut(|d| crate::dom::import_tree(d, &parsed, t, false).0);
        crate::js::dom::with_doc_mut(|d| d.append(html_core::NodeId(doc), html_core::NodeId(id)));
    }
    let ids: Vec<usize> = crate::js::dom::with_doc(|d| d.descendants(html_core::NodeId(doc)).map(|i| i.0).collect());
    for i in ids {
        crate::js::dom::set_node_doc(i, doc);
        crate::js::dom::mark_script_started_if_script(i);
    }
    Ok(crate::js::dom::wrap(vm, doc))
}

pub fn init(vm: &mut Vm) {
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__url_parse", 2, url_parse as NativeFn),
        ("__url_set", 3, url_set),
        ("__form_parse", 1, form_parse),
        ("__form_byte", 1, form_byte),
        ("__random_bytes", 1, random_bytes),
        ("__parse_document", 1, parse_document),
        ("__codec_decode", 7, codec_decode),
        ("__decode_bytes", 2, decode_bytes),
        ("__get_encoding", 1, get_encoding_native),
        ("__html_prescan", 1, html_prescan),
        ("__xml_sniff", 1, xml_sniff),
        ("__parse_xml", 1, parse_xml),
        ("__doc_meta", 4, doc_meta),
        ("__blob_url_register", 3, blob_url_register),
        ("__blob_url_revoke", 2, blob_url_revoke),
        ("__blob_url_resolve", 2, blob_url_resolve),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    for (label, src) in [("platform", PRELUDE), ("compat", COMPAT)] {
        if let Err(e) = vm.run_script_str(src) {
            let m = vm.error_string(&e);
            crate::ledger::record_js(&format!("{label}-prelude-failed:{m}"));
            eprintln!("[aether] {label} prelude failed: {m}");
        }
    }
    // AETHERWPT2 (SR135): `crypto.subtle` over crypto_core, on the `crypto` the prelude made.
    crate::api::subtle::init(vm);
}

/// AETHERWORKERS (SR123): the DOM-less half of [`init`] for a dedicated worker's realm (`workers::dedicated`):
/// `URL` / `URLSearchParams`, `TextEncoder` / `TextDecoder`, `AbortController`, `crypto` — the natives and the
/// prelude without `__parse_document` (a worker has no Document) and without the page-only compat shapes.
pub(crate) fn init_worker(vm: &mut Vm) {
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__url_parse", 2, url_parse as NativeFn),
        ("__url_set", 3, url_set),
        ("__form_parse", 1, form_parse),
        ("__form_byte", 1, form_byte),
        ("__random_bytes", 1, random_bytes),
        ("__codec_decode", 7, codec_decode),
        ("__decode_bytes", 2, decode_bytes),
        ("__get_encoding", 1, get_encoding_native),
        ("__xml_sniff", 1, xml_sniff),
        ("__blob_url_register", 3, blob_url_register),
        ("__blob_url_revoke", 2, blob_url_revoke),
        ("__blob_url_resolve", 2, blob_url_resolve),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        eprintln!("[aether-worker] platform prelude failed: {m}");
    }
    crate::api::subtle::init(vm);
}

/// AETHERWPT3 (SR138): `FileReaderSync` (File API §7) for a dedicated worker's realm, over its Blob's bytes
/// (`__blob_bytes`, the worker scope prelude's); installed by `workers::dedicated` after that prelude.
pub(crate) fn install_file_reader_sync(vm: &mut Vm) {
    if let Err(e) = vm.run_script_str(FILE_READER_SYNC) {
        let m = vm.error_string(&e);
        eprintln!("[aether-worker] FileReaderSync prelude failed: {m}");
    }
}

/// File API §7 FileReaderSync and the read operations' "package data": an ArrayBuffer, a binary string, text (the label's encoding,
/// else the Blob type's charset, else UTF-8 — a BOM overriding either, Encoding §6 "decode"), a data: URL.
const FILE_READER_SYNC: &str = r#"
(function () {
    if (typeof Blob !== 'function' || typeof __blob_bytes !== 'function') { return; }
    var bytesOf = function (blob, what) {
        if (!(blob instanceof Blob)) { throw new TypeError("Failed to execute '" + what + "' on 'FileReaderSync': parameter 1 is not of type 'Blob'."); }
        return new Uint8Array(__blob_bytes(blob));
    };
    var binary = function (b) {
        var out = '';
        for (var i = 0; i < b.length; i += 8192) { out += String.fromCharCode.apply(null, b.subarray(i, i + 8192)); }
        return out;
    };
    var usable = function (label) {
        try { new TextDecoder(label); return true; } catch (e) { return false; }
    };
    class FileReaderSync {
        constructor() {}
        readAsArrayBuffer(blob) { return bytesOf(blob, 'readAsArrayBuffer').slice().buffer; }
        readAsBinaryString(blob) { return binary(bytesOf(blob, 'readAsBinaryString')); }
        readAsText(blob) {
            var b = bytesOf(blob, 'readAsText');
            var enc = null, label = arguments[1];
            if (label !== undefined && usable(String(label))) { enc = String(label); }
            if (enc === null) {
                var m = /;\s*charset\s*=\s*"?([^;"\s]+)/i.exec(blob.type);
                if (m && usable(m[1])) { enc = m[1]; }
            }
            if (b[0] === 0xEF && b[1] === 0xBB && b[2] === 0xBF) { enc = 'utf-8'; }
            else if (b[0] === 0xFE && b[1] === 0xFF) { enc = 'utf-16be'; }
            else if (b[0] === 0xFF && b[1] === 0xFE) { enc = 'utf-16le'; }
            return new TextDecoder(enc || 'utf-8').decode(b);
        }
        readAsDataURL(blob) {
            var b = bytesOf(blob, 'readAsDataURL');
            return 'data:' + (blob.type || 'application/octet-stream') + ';base64,' + btoa(binary(b));
        }
    }
    Object.keys({ readAsArrayBuffer: 0, readAsBinaryString: 0, readAsText: 0, readAsDataURL: 0 }).forEach(function (k) {
        var d = Object.getOwnPropertyDescriptor(FileReaderSync.prototype, k);
        d.enumerable = true;
        Object.defineProperty(FileReaderSync.prototype, k, d);
    });
    Object.defineProperty(FileReaderSync.prototype, Symbol.toStringTag, { value: 'FileReaderSync', configurable: true });
    Object.defineProperty(globalThis, 'FileReaderSync', { value: FileReaderSync, writable: true, configurable: true, enumerable: false });
})();
"#;

/// The WHATWG object shapes over the natives above.
const PRELUDE: &str = r#"

// ---- URLSearchParams / URL (URL Standard §6) -------------------------
// The parser, the setters and the application/x-www-form-urlencoded parser and
// serializer are html_core::url's (__url_parse / __url_set / __form_parse /
// __form_byte). AETHERWPT2 (SR135): URL §6.2 exactly — the init union
// (sequence of pairs, record, USVString), `has` / `delete` with a value, live
// `URLSearchParams Iterator`s, and one query object per URL that the `search`
// and `href` setters update in place ("update" steps both ways).
(function () {
    var op = function (o, n, f) { Object.defineProperty(o, n, { value: f, writable: true, enumerable: true, configurable: true }); };
    var acc = function (o, n, get, set) { Object.defineProperty(o, n, { get: get, set: set, enumerable: true, configurable: true }); };
    // WebIDL USVString: ToString, then every lone surrogate becomes U+FFFD.
    var usv = function (v) {
        var s = String(v), n = s.length, out = '', start = 0;
        for (var i = 0; i < n; i++) {
            var c = s.charCodeAt(i);
            if (c < 0xD800 || c > 0xDFFF) { continue; }
            if (c <= 0xDBFF && i + 1 < n) {
                var d = s.charCodeAt(i + 1);
                if (d >= 0xDC00 && d <= 0xDFFF) { i++; continue; }
            }
            out += s.slice(start, i) + '�';
            start = i + 1;
        }
        return start === 0 ? s : out + s.slice(start);
    };
    globalThis.__usv = usv;
    var enc = function (s) { return __form_byte(s); };
    var parseQ = function (q) {
        var rows = __form_parse(q), out = [];
        for (var i = 0; i < rows.length; i++) { out.push([rows[i][0], rows[i][1]]); }
        return out;
    };
    var serialize = function (list) {
        var out = [];
        for (var i = 0; i < list.length; i++) { out.push(enc(list[i][0]) + '=' + enc(list[i][1])); }
        return out.join('&');
    };
    var need = function (n, min, iface, opname) {
        if (n < min) {
            throw new TypeError("Failed to execute '" + opname + "' on '" + iface + "': " + min + ' argument' + (min > 1 ? 's' : '') + ' required, but only ' + n + ' present.');
        }
    };
    var SP = new WeakMap();
    var sp = function (o) {
        var s = (o !== null && typeof o === 'object') ? SP.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var US = new WeakMap();
    var us = function (o) {
        var s = (o !== null && typeof o === 'object') ? US.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    // URL §6.2 "update": the URL object's query becomes the serialization (null when empty).
    var update = function (s) {
        if (s.url === null) { return; }
        var u = US.get(s.url);
        var next = __url_set(u.c.href, 'search', serialize(s.list));
        if (next) { u.c = next; }
    };

    class URLSearchParams {
        constructor() {
            var init = arguments[0];
            var list = [];
            if (init !== undefined) {
                if (init !== null && (typeof init === 'object' || typeof init === 'function')) {
                    var m = init[Symbol.iterator];
                    if (m !== undefined && m !== null) {
                        if (typeof m !== 'function') { throw new TypeError("Failed to construct 'URLSearchParams': The object's @@iterator is not callable."); }
                        for (var pair of init) {
                            if (pair === null || (typeof pair !== 'object' && typeof pair !== 'function')) {
                                throw new TypeError("Failed to construct 'URLSearchParams': The provided value cannot be converted to a sequence.");
                            }
                            var p = [];
                            for (var x of pair) { p.push(usv(x)); }
                            if (p.length !== 2) { throw new TypeError("Failed to construct 'URLSearchParams': Sequence initializer must only contain pair elements"); }
                            list.push(p);
                        }
                    } else {
                        // record<USVString, USVString>: own enumerable keys in [[OwnPropertyKeys]] order.
                        var keys = Reflect.ownKeys(init);
                        for (var i = 0; i < keys.length; i++) {
                            var desc = Reflect.getOwnPropertyDescriptor(init, keys[i]);
                            if (desc === undefined || !desc.enumerable) { continue; }
                            if (typeof keys[i] === 'symbol') { throw new TypeError("Failed to construct 'URLSearchParams': Cannot convert a Symbol value to a string"); }
                            var k = usv(keys[i]);
                            var v = usv(init[keys[i]]);
                            var at = -1;
                            for (var j = 0; j < list.length; j++) { if (list[j][0] === k) { at = j; } }
                            if (at >= 0) { list[at][1] = v; } else { list.push([k, v]); }
                        }
                    }
                } else {
                    var q = usv(init);
                    if (q.charAt(0) === '?') { q = q.slice(1); }
                    list = parseQ(q);
                }
            }
            SP.set(this, { list: list, url: null });
        }
    }
    var SPM = {
        append(name, value) {
            var s = sp(this);
            need(arguments.length, 2, 'URLSearchParams', 'append');
            s.list.push([usv(name), usv(value)]);
            update(s);
        },
        delete(name) {
            var s = sp(this);
            need(arguments.length, 1, 'URLSearchParams', 'delete');
            var n = usv(name), value = arguments[1], v = value === undefined ? undefined : usv(value);
            s.list = s.list.filter(function (p) { return !(p[0] === n && (v === undefined || p[1] === v)); });
            update(s);
        },
        get(name) {
            var s = sp(this);
            need(arguments.length, 1, 'URLSearchParams', 'get');
            var n = usv(name);
            for (var i = 0; i < s.list.length; i++) { if (s.list[i][0] === n) { return s.list[i][1]; } }
            return null;
        },
        getAll(name) {
            var s = sp(this);
            need(arguments.length, 1, 'URLSearchParams', 'getAll');
            var n = usv(name), out = [];
            for (var i = 0; i < s.list.length; i++) { if (s.list[i][0] === n) { out.push(s.list[i][1]); } }
            return out;
        },
        has(name) {
            var s = sp(this);
            need(arguments.length, 1, 'URLSearchParams', 'has');
            var n = usv(name), value = arguments[1], v = value === undefined ? undefined : usv(value);
            for (var i = 0; i < s.list.length; i++) {
                if (s.list[i][0] === n && (v === undefined || s.list[i][1] === v)) { return true; }
            }
            return false;
        },
        set(name, value) {
            var s = sp(this);
            need(arguments.length, 2, 'URLSearchParams', 'set');
            var n = usv(name), v = usv(value), seen = false, out = [];
            for (var i = 0; i < s.list.length; i++) {
                if (s.list[i][0] !== n) { out.push(s.list[i]); }
                else if (!seen) { seen = true; out.push([n, v]); }
            }
            if (!seen) { out.push([n, v]); }
            s.list = out;
            update(s);
        },
        sort() {
            var s = sp(this);
            // Stable, by code units of the names.
            var d = s.list.map(function (p, i) { return [p, i]; });
            d.sort(function (a, b) { return a[0][0] < b[0][0] ? -1 : (a[0][0] > b[0][0] ? 1 : a[1] - b[1]); });
            s.list = d.map(function (x) { return x[0]; });
            update(s);
        },
        forEach(callback) {
            var s = sp(this);
            need(arguments.length, 1, 'URLSearchParams', 'forEach');
            if (typeof callback !== 'function') { throw new TypeError("Failed to execute 'forEach' on 'URLSearchParams': The callback provided as parameter 1 is not a function."); }
            var thisArg = arguments[1];
            for (var i = 0; i < s.list.length; i++) {
                var p = s.list[i];
                callback.call(thisArg, p[1], p[0], this);
            }
        },
        entries() { sp(this); return mkIter(this, 2); },
        keys() { sp(this); return mkIter(this, 0); },
        values() { sp(this); return mkIter(this, 1); },
        toString() { return serialize(sp(this).list); },
    };
    Object.keys(SPM).forEach(function (k) { op(URLSearchParams.prototype, k, SPM[k]); });
    Object.defineProperty(URLSearchParams.prototype, Symbol.iterator, { value: SPM.entries, writable: true, enumerable: false, configurable: true });
    acc(URLSearchParams.prototype, 'size', function () { return sp(this).list.length; }, undefined);
    Object.defineProperty(URLSearchParams.prototype, Symbol.toStringTag, { value: 'URLSearchParams', configurable: true });

    // The default iterator object (WebIDL §3.7.10.1): live over the list, by index.
    var IterProto = Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()));
    var SPIter = Object.create(IterProto);
    var IT = new WeakMap();
    op(SPIter, 'next', {
        next() {
            var st = (this !== null && typeof this === 'object') ? IT.get(this) : undefined;
            if (st === undefined) { throw new TypeError('Illegal invocation'); }
            var list = sp(st.target).list;
            if (st.index >= list.length) { return { value: undefined, done: true }; }
            var p = list[st.index++];
            return { value: st.kind === 0 ? p[0] : (st.kind === 1 ? p[1] : [p[0], p[1]]), done: false };
        },
    }.next);
    Object.defineProperty(SPIter, Symbol.toStringTag, { value: 'URLSearchParams Iterator', configurable: true });
    var mkIter = function (target, kind) {
        var o = Object.create(SPIter);
        IT.set(o, { target: target, kind: kind, index: 0 });
        return o;
    };
    globalThis.URLSearchParams = URLSearchParams;
    Object.defineProperty(globalThis, 'URLSearchParams', { enumerable: false });

    // ---- URL ---------------------------------------------------------
    class URL {
        constructor(url) {
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'URL': 1 argument required, but only 0 present."); }
            var base = arguments[1];
            var c = __url_parse(usv(url), base === undefined ? undefined : usv(base));
            if (!c) { throw new TypeError("Failed to construct 'URL': Invalid URL"); }
            var q = new URLSearchParams(c.search);
            US.set(this, { c: c, q: q });
            sp(q).url = this;
        }
        static canParse(url) {
            need(arguments.length, 1, 'URL', 'canParse');
            var base = arguments[1];
            return !!__url_parse(usv(url), base === undefined ? undefined : usv(base));
        }
        static parse(url) {
            need(arguments.length, 1, 'URL', 'parse');
            try { return new URL(url, arguments[1]); } catch (e) { return null; }
        }
    }
    var PARTS = ['protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'];
    PARTS.forEach(function (part) {
        acc(URL.prototype, part, function () { return us(this).c[part]; }, function (v) {
            var u = us(this);
            var input = usv(v);
            var next = __url_set(u.c.href, part, input);
            if (next) { u.c = next; }
            if (part === 'search') {
                // URL §6.1 search setter: the query object's list is the parse of the input (empty when "").
                if (input.charAt(0) === '?') { input = input.slice(1); }
                sp(u.q).list = input === '' ? [] : parseQ(input);
            }
        });
    });
    acc(URL.prototype, 'href', function () { return us(this).c.href; }, function (v) {
        var u = us(this);
        var c = __url_parse(usv(v), undefined);
        if (!c) { throw new TypeError("Failed to set the 'href' property on 'URL': Invalid URL"); }
        u.c = c;
        sp(u.q).list = c.search === '' ? [] : parseQ(c.search.slice(1));
    });
    acc(URL.prototype, 'origin', function () { return us(this).c.origin; }, undefined);
    acc(URL.prototype, 'searchParams', function () { return us(this).q; }, undefined);
    op(URL.prototype, 'toString', { toString() { return us(this).c.href; } }.toString);
    op(URL.prototype, 'toJSON', { toJSON() { return us(this).c.href; } }.toJSON);
    Object.defineProperty(URL.prototype, Symbol.toStringTag, { value: 'URL', configurable: true });
    // AETHERWPT3 (SR138): File API §8.3 — the blob URL store (api/blobstore.rs). createObjectURL takes a Blob (a
    // MediaSource is js/mse.rs's, which wraps these); revokeObjectURL removes exactly the URL given, when this
    // environment's origin created it.
    var hereOrigin = function () { try { return String(globalThis.location.origin); } catch (e) { return 'null'; } };
    op(URL, 'createObjectURL', {
        createObjectURL(obj) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'createObjectURL' on 'URL': 1 argument required, but only 0 present."); }
            if (typeof Blob !== 'function' || !(obj instanceof Blob)) { throw new TypeError("Failed to execute 'createObjectURL' on 'URL': Overload resolution failed."); }
            var bytes = typeof __blob_bytes === 'function' ? __blob_bytes(obj) : new Uint8Array(0);
            return __blob_url_register(hereOrigin(), bytes, obj.type);
        },
    }.createObjectURL);
    op(URL, 'revokeObjectURL', {
        revokeObjectURL(url) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'revokeObjectURL' on 'URL': 1 argument required, but only 0 present."); }
            var s = String(url);
            if (!/^blob:/i.test(s)) { return; }
            __blob_url_revoke(s, hereOrigin());
        },
    }.revokeObjectURL);
    globalThis.URL = URL;
    Object.defineProperty(globalThis, 'URL', { enumerable: false });
})();

// ---- TextEncoder / TextDecoder (Encoding Standard §7) ----------------
// AETHERWPT2 (SR135): the WHATWG label table (encodings.json), `fatal`,
// `ignoreBOM`, `{stream: true}`, the UTF-8 decoder of §8.1.1 (bounds-checked
// continuation bytes, one U+FFFD per maximal subpart), UTF-16LE/BE (§14.2),
// windows-1252 and x-user-defined; `replacement` refused with a RangeError.
// AETHERWPT3 (SR138): every other legacy encoding (§9–§13) decodes through
// `__codec_decode` (api/encoding, the WHATWG indexes), its state carried here.
(function () {
    var op = function (o, n, f) { Object.defineProperty(o, n, { value: f, writable: true, enumerable: true, configurable: true }); };
    var getter = function (o, n, f) { Object.defineProperty(o, n, { get: f, enumerable: true, configurable: true }); };
    var ledger = function (m) { if (typeof __ledger === 'function') { __ledger(m); } };
    var LABELS = Object.create(null);
    'utf-8:unicode-1-1-utf-8 unicode11utf8 unicode20utf8 utf-8 utf8 x-unicode20utf8|ibm866:866 cp866 csibm866 ibm866|iso-8859-2:csisolatin2 iso-8859-2 iso-ir-101 iso8859-2 iso88592 iso_8859-2 iso_8859-2:1987 l2 latin2|iso-8859-3:csisolatin3 iso-8859-3 iso-ir-109 iso8859-3 iso88593 iso_8859-3 iso_8859-3:1988 l3 latin3|iso-8859-4:csisolatin4 iso-8859-4 iso-ir-110 iso8859-4 iso88594 iso_8859-4 iso_8859-4:1988 l4 latin4|iso-8859-5:csisolatincyrillic cyrillic iso-8859-5 iso-ir-144 iso8859-5 iso88595 iso_8859-5 iso_8859-5:1988|iso-8859-6:arabic asmo-708 csiso88596e csiso88596i csisolatinarabic ecma-114 iso-8859-6 iso-8859-6-e iso-8859-6-i iso-ir-127 iso8859-6 iso88596 iso_8859-6 iso_8859-6:1987|iso-8859-7:csisolatingreek ecma-118 elot_928 greek greek8 iso-8859-7 iso-ir-126 iso8859-7 iso88597 iso_8859-7 iso_8859-7:1987 sun_eu_greek|iso-8859-8:csiso88598e csisolatinhebrew hebrew iso-8859-8 iso-8859-8-e iso-ir-138 iso8859-8 iso88598 iso_8859-8 iso_8859-8:1988 visual|iso-8859-8-i:csiso88598i iso-8859-8-i logical|iso-8859-10:csisolatin6 iso-8859-10 iso-ir-157 iso8859-10 iso885910 l6 latin6|iso-8859-13:iso-8859-13 iso8859-13 iso885913|iso-8859-14:iso-8859-14 iso8859-14 iso885914|iso-8859-15:csisolatin9 iso-8859-15 iso8859-15 iso885915 iso_8859-15 l9|iso-8859-16:iso-8859-16|koi8-r:cskoi8r koi koi8 koi8-r koi8_r|koi8-u:koi8-ru koi8-u|macintosh:csmacintosh mac macintosh x-mac-roman|windows-874:dos-874 iso-8859-11 iso8859-11 iso885911 tis-620 windows-874|windows-1250:cp1250 windows-1250 x-cp1250|windows-1251:cp1251 windows-1251 x-cp1251|windows-1252:ansi_x3.4-1968 ascii cp1252 cp819 csisolatin1 ibm819 iso-8859-1 iso-ir-100 iso8859-1 iso88591 iso_8859-1 iso_8859-1:1987 l1 latin1 us-ascii windows-1252 x-cp1252|windows-1253:cp1253 windows-1253 x-cp1253|windows-1254:cp1254 csisolatin5 iso-8859-9 iso-ir-148 iso8859-9 iso88599 iso_8859-9 iso_8859-9:1989 l5 latin5 windows-1254 x-cp1254|windows-1255:cp1255 windows-1255 x-cp1255|windows-1256:cp1256 windows-1256 x-cp1256|windows-1257:cp1257 windows-1257 x-cp1257|windows-1258:cp1258 windows-1258 x-cp1258|x-mac-cyrillic:x-mac-cyrillic x-mac-ukrainian|gbk:chinese csgb2312 csiso58gb231280 gb2312 gb_2312 gb_2312-80 gbk iso-ir-58 x-gbk|gb18030:gb18030|big5:big5 big5-hkscs cn-big5 csbig5 x-x-big5|euc-jp:cseucpkdfmtjapanese euc-jp x-euc-jp|iso-2022-jp:csiso2022jp iso-2022-jp|shift_jis:csshiftjis ms932 ms_kanji shift-jis shift_jis sjis windows-31j x-sjis|euc-kr:cseuckr csksc56011987 euc-kr iso-ir-149 korean ks_c_5601-1987 ks_c_5601-1989 ksc5601 ksc_5601 windows-949|replacement:csiso2022kr hz-gb-2312 iso-2022-cn iso-2022-cn-ext iso-2022-kr replacement|utf-16be:unicodefffe utf-16be|utf-16le:csunicode iso-10646-ucs-2 ucs-2 unicode unicodefeff utf-16 utf-16le|x-user-defined:x-user-defined'.split('|').forEach(function (row) {
        var at = row.indexOf(':'), name = row.slice(0, at);
        row.slice(at + 1).split(' ').forEach(function (l) { LABELS[l] = name; });
    });
    // Encoding §4.2 "get an encoding": strip ASCII whitespace, ASCII-lowercase, look up.
    var getEncoding = function (label) {
        var l = String(label).replace(/^[\t\n\f\r ]+|[\t\n\f\r ]+$/g, '').replace(/[A-Z]/g, function (c) { return String.fromCharCode(c.charCodeAt(0) + 32); });
        return LABELS[l] || null;
    };
    globalThis.__get_encoding = getEncoding;
    var dict = function (v, what) {
        if (v === undefined || v === null) { return {}; }
        if (typeof v !== 'object' && typeof v !== 'function') { throw new TypeError("Failed to " + what + ": The provided value is not of type 'dictionary'."); }
        return v;
    };
    // BufferSource (AllowShared) → a copy of its bytes.
    var bytesOf = function (input, what) {
        if (typeof ArrayBuffer !== 'undefined' && input instanceof ArrayBuffer) { return new Uint8Array(input.slice(0)); }
        if (typeof SharedArrayBuffer !== 'undefined' && input instanceof SharedArrayBuffer) { return new Uint8Array(input.slice(0)); }
        if (ArrayBuffer.isView(input)) { return new Uint8Array(input.buffer, input.byteOffset, input.byteLength).slice(); }
        throw new TypeError("Failed to execute '" + what + "' on 'TextDecoder': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
    };
    var W1252 = [0x20AC, 0x81, 0x201A, 0x192, 0x201E, 0x2026, 0x2020, 0x2021, 0x2C6, 0x2030, 0x160, 0x2039, 0x152, 0x8D, 0x17D, 0x8F,
                 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x2DC, 0x2122, 0x161, 0x203A, 0x153, 0x9D, 0x17E, 0x178];

    // A decoder instance: run(bytes, emit, error) consumes bytes; end(emit, error) handles end-of-queue.
    // error() returns true when the decoder must stop (fatal mode).
    var makeDecoder = function (name, fatal) {
        if (name === 'utf-8') {
            var need = 0, seen = 0, cp = 0, lo = 0x80, hi = 0xBF;
            return {
                run: function (b, emit, error) {
                    var i = 0, n = b.length;
                    while (i < n) {
                        var x = b[i];
                        if (need === 0) {
                            i++;
                            if (x <= 0x7F) { emit(x); }
                            else if (x >= 0xC2 && x <= 0xDF) { need = 1; cp = x & 0x1F; }
                            else if (x >= 0xE0 && x <= 0xEF) { if (x === 0xE0) { lo = 0xA0; } if (x === 0xED) { hi = 0x9F; } need = 2; cp = x & 0xF; }
                            else if (x >= 0xF0 && x <= 0xF4) { if (x === 0xF0) { lo = 0x90; } if (x === 0xF4) { hi = 0x8F; } need = 3; cp = x & 0x7; }
                            else if (error()) { return; }
                            continue;
                        }
                        if (x < lo || x > hi) {
                            // The byte is not consumed: it is prepended to the queue and read again.
                            cp = 0; need = 0; seen = 0; lo = 0x80; hi = 0xBF;
                            if (error()) { return; }
                            continue;
                        }
                        i++;
                        lo = 0x80; hi = 0xBF;
                        cp = (cp << 6) | (x & 0x3F);
                        if (++seen === need) { var c = cp; cp = 0; need = 0; seen = 0; emit(c); }
                    }
                },
                end: function (emit, error) {
                    if (need !== 0) { cp = 0; need = 0; seen = 0; lo = 0x80; hi = 0xBF; error(); }
                },
            };
        }
        if (name === 'utf-16le' || name === 'utf-16be') {
            var be = name === 'utf-16be', lead = -1, sur = -1;
            return {
                run: function (b, emit, error) {
                    for (var i = 0; i < b.length; i++) {
                        if (lead === -1) { lead = b[i]; continue; }
                        var cu = be ? (lead << 8) | b[i] : (b[i] << 8) | lead;
                        lead = -1;
                        if (sur !== -1) {
                            var ls = sur;
                            sur = -1;
                            if (cu >= 0xDC00 && cu <= 0xDFFF) { emit(0x10000 + ((ls - 0xD800) << 10) + (cu - 0xDC00)); continue; }
                            // §14.2.1 step 5.5: the code unit's bytes are restored to the queue; reading them again
                            // yields this same code unit, so it is processed below after the error.
                            if (error()) { return; }
                        }
                        if (cu >= 0xD800 && cu <= 0xDBFF) { sur = cu; continue; }
                        if (cu >= 0xDC00 && cu <= 0xDFFF) { if (error()) { return; } continue; }
                        emit(cu);
                    }
                },
                end: function (emit, error) {
                    if (lead !== -1 || sur !== -1) { lead = -1; sur = -1; error(); }
                },
            };
        }
        if (name === 'windows-1252' || name === 'x-user-defined') {
            var udef = name === 'x-user-defined';
            return {
                run: function (b, emit) {
                    for (var i = 0; i < b.length; i++) {
                        var x = b[i];
                        emit(x < 0x80 ? x : (udef ? 0xF780 + x - 0x80 : (x < 0xA0 ? W1252[x - 0x80] : x)));
                    }
                },
                end: function () {},
            };
        }
        if (typeof __codec_decode === 'function') {
            var cst = [0, 0, 0, 0];
            var runNative = function (b, flush, emit, error) {
                var r = __codec_decode(name, cst[0], cst[1], cst[2], cst[3], b, flush);
                cst = [r[1], r[2], r[3], r[4]];
                if (r[5] > 0 && fatal) { error(); return; }
                var t = r[0];
                for (var i = 0; i < t.length; i++) { emit(t.charCodeAt(i)); }
            };
            return {
                run: function (b, emit, error) { runNative(b, false, emit, error); },
                end: function (emit, error) { runNative(new Uint8Array(0), true, emit, error); },
            };
        }
        ledger('TextDecoder-no-decoder-decoded-as-utf8:' + name);
        return makeDecoder('utf-8');
    };

    var TD = new WeakMap();
    var td = function (o) {
        var s = (o !== null && typeof o === 'object') ? TD.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    class TextDecoder {
        constructor() {
            var label = arguments[0] === undefined ? 'utf-8' : String(arguments[0]);
            var o = dict(arguments[1], "construct 'TextDecoder'");
            var fatal = !!o.fatal, ignoreBOM = !!o.ignoreBOM;
            var name = getEncoding(label);
            if (name === null || name === 'replacement') {
                throw new RangeError("Failed to construct 'TextDecoder': The encoding label provided ('" + label + "') is invalid.");
            }
            TD.set(this, { name: name, fatal: fatal, ignoreBOM: ignoreBOM, doNotFlush: false, decoder: null, bomSeen: false });
        }
    }
    getter(TextDecoder.prototype, 'encoding', function () { return td(this).name; });
    getter(TextDecoder.prototype, 'fatal', function () { return td(this).fatal; });
    getter(TextDecoder.prototype, 'ignoreBOM', function () { return td(this).ignoreBOM; });
    op(TextDecoder.prototype, 'decode', {
        decode() {
            var s = td(this);
            var input = arguments[0];
            var bytes = input === undefined ? new Uint8Array(0) : bytesOf(input, 'decode');
            var stream = !!dict(arguments[1], "execute 'decode' on 'TextDecoder'").stream;
            if (!s.doNotFlush) { s.decoder = makeDecoder(s.name, s.fatal); s.bomSeen = false; }
            s.doNotFlush = stream;
            var units = [], out = '', failed = false;
            var bomCheck = !s.ignoreBOM && (s.name === 'utf-8' || s.name === 'utf-16le' || s.name === 'utf-16be');
            var emit = function (c) {
                if (bomCheck && !s.bomSeen) { s.bomSeen = true; if (c === 0xFEFF) { return; } }
                if (c > 0xFFFF) { c -= 0x10000; units.push(0xD800 + (c >> 10), 0xDC00 + (c & 0x3FF)); } else { units.push(c); }
                if (units.length >= 8192) { out += String.fromCharCode.apply(null, units); units = []; }
            };
            var error = function () {
                if (s.fatal) { failed = true; return true; }
                emit(0xFFFD);
                return false;
            };
            s.decoder.run(bytes, emit, error);
            if (!failed && !stream) { s.decoder.end(emit, error); }
            if (failed) {
                s.doNotFlush = false;
                throw new TypeError("Failed to execute 'decode' on 'TextDecoder': The encoded data was not valid.");
            }
            return out + String.fromCharCode.apply(null, units);
        },
    }.decode);
    Object.defineProperty(TextDecoder.prototype, Symbol.toStringTag, { value: 'TextDecoder', configurable: true });

    var TE = new WeakSet();
    var te = function (o) { if (o === null || typeof o !== 'object' || !TE.has(o)) { throw new TypeError('Illegal invocation'); } };
    // UTF-8 of a USVString, as code points (lone surrogates are U+FFFD).
    var encodeUtf8 = function (str) {
        var s = String(str), bytes = [];
        for (var i = 0; i < s.length; i++) {
            var cp = s.charCodeAt(i);
            if (cp >= 0xD800 && cp <= 0xDBFF && i + 1 < s.length) {
                var lo = s.charCodeAt(i + 1);
                if (lo >= 0xDC00 && lo <= 0xDFFF) { cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00); i++; }
                else { cp = 0xFFFD; }
            } else if (cp >= 0xD800 && cp <= 0xDFFF) { cp = 0xFFFD; }
            if (cp < 0x80) { bytes.push(cp); }
            else if (cp < 0x800) { bytes.push(0xC0 | (cp >> 6), 0x80 | (cp & 63)); }
            else if (cp < 0x10000) { bytes.push(0xE0 | (cp >> 12), 0x80 | ((cp >> 6) & 63), 0x80 | (cp & 63)); }
            else { bytes.push(0xF0 | (cp >> 18), 0x80 | ((cp >> 12) & 63), 0x80 | ((cp >> 6) & 63), 0x80 | (cp & 63)); }
        }
        return bytes;
    };
    class TextEncoder {
        constructor() { TE.add(this); }
    }
    getter(TextEncoder.prototype, 'encoding', function () { te(this); return 'utf-8'; });
    op(TextEncoder.prototype, 'encode', {
        encode() {
            te(this);
            var input = arguments[0];
            return new Uint8Array(encodeUtf8(input === undefined ? '' : input));
        },
    }.encode);
    op(TextEncoder.prototype, 'encodeInto', {
        encodeInto(source, destination) {
            te(this);
            if (arguments.length < 2) { throw new TypeError("Failed to execute 'encodeInto' on 'TextEncoder': 2 arguments required, but only " + arguments.length + ' present.'); }
            var s = String(source);
            if (!(destination instanceof Uint8Array)) { throw new TypeError("Failed to execute 'encodeInto' on 'TextEncoder': parameter 2 is not of type 'Uint8Array'."); }
            // Encoding §7.4.3: whole scalar values while they fit; read counts UTF-16 code units consumed.
            var read = 0, written = 0, cap = destination.length;
            for (var i = 0; i < s.length; i++) {
                var cp = s.charCodeAt(i), units = 1;
                if (cp >= 0xD800 && cp <= 0xDBFF && i + 1 < s.length) {
                    var lo = s.charCodeAt(i + 1);
                    if (lo >= 0xDC00 && lo <= 0xDFFF) { cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00); units = 2; }
                    else { cp = 0xFFFD; }
                } else if (cp >= 0xD800 && cp <= 0xDFFF) { cp = 0xFFFD; }
                var len = cp < 0x80 ? 1 : (cp < 0x800 ? 2 : (cp < 0x10000 ? 3 : 4));
                if (written + len > cap) { break; }
                if (len === 1) { destination[written] = cp; }
                else if (len === 2) { destination[written] = 0xC0 | (cp >> 6); destination[written + 1] = 0x80 | (cp & 63); }
                else if (len === 3) { destination[written] = 0xE0 | (cp >> 12); destination[written + 1] = 0x80 | ((cp >> 6) & 63); destination[written + 2] = 0x80 | (cp & 63); }
                else { destination[written] = 0xF0 | (cp >> 18); destination[written + 1] = 0x80 | ((cp >> 12) & 63); destination[written + 2] = 0x80 | ((cp >> 6) & 63); destination[written + 3] = 0x80 | (cp & 63); }
                written += len;
                read += units;
                i += units - 1;
            }
            return { read: read, written: written };
        },
    }.encodeInto);
    Object.defineProperty(TextEncoder.prototype, Symbol.toStringTag, { value: 'TextEncoder', configurable: true });
    globalThis.TextEncoder = TextEncoder;
    globalThis.TextDecoder = TextDecoder;
    Object.defineProperty(globalThis, 'TextEncoder', { enumerable: false });
    Object.defineProperty(globalThis, 'TextDecoder', { enumerable: false });
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
    // AETHERWPT4 (SR142): the XML document builder shared by DOMParser and XMLHttpRequest's document response — the
    // tokens of api/xml.rs (`__parse_xml`) appended with the DOM's own methods, so namespaces, prefixes and
    // attribute namespaces live where the DOM keeps them. Answers the document, or null when the text is not
    // namespace-well-formed (the caller decides what failure means).
    var xmlDocument = function (text, contentType, url, charset) {
        if (typeof __parse_xml !== 'function') { return null; }
        var r = __parse_xml(String(text));
        if (typeof r[1] === 'string') { return null; }
        var doc = document.implementation.createDocument(null, null, null);
        var cur = doc;
        for (var i = 1; i < r.length; i++) {
            var t = r[i];
            switch (t[0]) {
                case 'd': cur.appendChild(doc.implementation.createDocumentType(t[1], t[2], t[3])); break;
                case 's':
                    var el = doc.createElementNS(t[1], t[2]);
                    for (var k = 0; k + 2 < t[3].length; k += 3) { el.setAttributeNS(t[3][k], t[3][k + 1], t[3][k + 2]); }
                    cur.appendChild(el);
                    cur = el;
                    break;
                case 'e': cur = cur.parentNode; break;
                case 't': cur.appendChild(doc.createTextNode(t[1])); break;
                case 'c': cur.appendChild(doc.createCDATASection(t[1])); break;
                case 'm': cur.appendChild(doc.createComment(t[1])); break;
                case 'p': cur.appendChild(doc.createProcessingInstruction(t[1], t[2])); break;
            }
        }
        if (typeof __doc_meta === 'function') { __doc_meta(doc, contentType, url, charset || 'UTF-8'); }
        return doc;
    };
    Object.defineProperty(globalThis, '__xml_document', { value: xmlDocument, writable: true, configurable: true, enumerable: false });
    var pageURL = function () { try { return document.URL; } catch (e) { return 'about:blank'; } };
    // HTML §8.5.1 DOMParser: `parseFromString(string, type)` with type a DOMParserSupportedType.
    function DOMParser() {
        if (!(this instanceof DOMParser)) { throw new TypeError("Failed to construct 'DOMParser': Please use the 'new' operator, this DOM object constructor cannot be called as a function."); }
    }
    DOMParser.prototype.parseFromString = function (str, type) {
        if (arguments.length < 2) { throw new TypeError("Failed to execute 'parseFromString' on 'DOMParser': 2 arguments required, but only " + arguments.length + ' present.'); }
        var t = String(type);
        if (['text/html', 'text/xml', 'application/xml', 'application/xhtml+xml', 'image/svg+xml'].indexOf(t) < 0) {
            throw new TypeError("Failed to execute 'parseFromString' on 'DOMParser': The provided value '" + t + "' is not a valid enum value of type DOMParserSupportedType.");
        }
        if (t === 'text/html') {
            var hd = __parse_document(String(str));
            if (typeof __doc_meta === 'function') { __doc_meta(hd, 'text/html', pageURL(), 'UTF-8'); }
            return hd;
        }
        var d = xmlDocument(String(str), t, pageURL(), 'UTF-8');
        if (d === null) {
            // "If that fails": a document whose element is a parsererror in Gecko's namespace.
            d = document.implementation.createDocument('http://www.mozilla.org/newlayout/xml/parsererror.xml', 'parsererror', null);
            if (typeof __doc_meta === 'function') { __doc_meta(d, t, pageURL(), 'UTF-8'); }
        }
        return d;
    };
    globalThis.DOMParser = DOMParser;

    function XMLSerializer() {}
    XMLSerializer.prototype.serializeToString = function (node) {
        return __xml_serialize(node);
    };
    globalThis.XMLSerializer = XMLSerializer;
})();

// ---- QuotaExceededError (WebIDL §3.14.2) ------------------------------
// AETHERWPT2 (SR135): the DOMException subclass with `quota` and `requested`
// (null when not given); negative or non-finite options are refused, and a
// `requested` below `quota` is a RangeError.
(function () {
    if (typeof DOMException !== 'function') { return; }
    var QE = new WeakMap();
    var opt = function (v, what) {
        if (v === undefined) { return null; }
        var n = +v;
        if (!isFinite(n)) { throw new TypeError("Failed to construct 'QuotaExceededError': Failed to read the '" + what + "' property from 'QuotaExceededErrorOptions': The provided double value is non-finite."); }
        if (n < 0) { throw new RangeError("Failed to construct 'QuotaExceededError': '" + what + "' must not be negative."); }
        return n;
    };
    class QuotaExceededError extends DOMException {
        constructor() {
            var message = arguments[0] === undefined ? '' : String(arguments[0]);
            var o = arguments[1];
            if (o !== undefined && o !== null && typeof o !== 'object' && typeof o !== 'function') { throw new TypeError("Failed to construct 'QuotaExceededError': The provided value is not of type 'QuotaExceededErrorOptions'."); }
            var quota = opt(o ? o.quota : undefined, 'quota');
            var requested = opt(o ? o.requested : undefined, 'requested');
            if (quota !== null && requested !== null && requested < quota) { throw new RangeError("Failed to construct 'QuotaExceededError': 'requested' must not be less than 'quota'."); }
            super(message, 'QuotaExceededError');
            QE.set(this, { quota: quota, requested: requested });
        }
    }
    var slot = function (o) {
        var s = (o !== null && typeof o === 'object') ? QE.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    Object.defineProperty(QuotaExceededError.prototype, 'quota', { get: function () { return slot(this).quota; }, enumerable: true, configurable: true });
    Object.defineProperty(QuotaExceededError.prototype, 'requested', { get: function () { return slot(this).requested; }, enumerable: true, configurable: true });
    Object.defineProperty(globalThis, 'QuotaExceededError', { value: QuotaExceededError, writable: true, enumerable: false, configurable: true });
})();

// ---- crypto ----------------------------------------------------------
// getRandomValues fills from the OS entropy pool; randomUUID is a real
// v4 UUID built on the same bytes.
(function () {
    var crypto = globalThis.crypto || {};
    // AETHERWPT2 (SR135): WebCrypto §10.1.1 — an integer-typed ArrayBufferView only (TypeMismatchError for the
    // float arrays and DataView), at most 65,536 bytes (QuotaExceededError), filled through a byte view so the
    // BigInt64 arrays and subclasses fill too; the same object comes back.
    var taDesc = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag);
    var taName = taDesc && typeof taDesc.get === 'function'
        ? function (a) { return taDesc.get.call(a); }
        : function (a) { return ArrayBuffer.isView(a) && !(a instanceof DataView) ? a.constructor.name : undefined; };
    var INTEGER = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array', 'Uint32Array', 'BigInt64Array', 'BigUint64Array'];
    crypto.getRandomValues = function getRandomValues(array) {
        if (arguments.length < 1) { throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': 1 argument required, but only 0 present."); }
        if (!ArrayBuffer.isView(array)) { throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'."); }
        var kind = array instanceof DataView ? 'DataView' : taName(array);
        if (INTEGER.indexOf(kind) < 0) {
            throw new DOMException("Failed to execute 'getRandomValues' on 'Crypto': The provided ArrayBufferView is of type '" + kind + "', which is not an integer array type.", 'TypeMismatchError');
        }
        if (array.byteLength > 65536) {
            var msg = "Failed to execute 'getRandomValues' on 'Crypto': The ArrayBufferView's byte length (" + array.byteLength + ") exceeds the number of bytes of entropy available via this API (65536).";
            throw typeof QuotaExceededError === 'function' ? new QuotaExceededError(msg) : new DOMException(msg, 'QuotaExceededError');
        }
        if (array.byteLength) {
            new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(__random_bytes(array.byteLength));
        }
        return array;
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


/// Platform pieces with honest, self-reporting approximations: the observer families (register, never
/// deliver). (AETHERWORKERS, SR123: the MessageChannel-over-the-timer-queue and `requestIdleCallback` shims are
/// gone — `js::ports` installs HTML §9.4.5's entangled ports and `event_loop` the idle-callback task source with
/// a real IdleDeadline.) (AETHERSTORAGE, SR94: the in-page
/// `localStorage`/`sessionStorage` shim is gone — `js::net` installs the real Storage over `crate::storage` —
/// and so is the JSON-round-trip `structuredClone`: `js::broadcast` installs HTML §2.7's.)
const COMPAT: &str = r#"
(function () {
    var mkObserver = function (kind) {
        var C = function (cb) {
            this._cb = cb;
            this.root = null; this.rootMargin = '0px'; this.thresholds = [0];
        };
        C.prototype.observe = function () { __ledger('observer-never-delivers:' + kind); };
        C.prototype.unobserve = function () {};
        C.prototype.disconnect = function () {};
        C.prototype.takeRecords = function () { return []; };
        try { Object.defineProperty(C, 'name', { value: kind }); } catch (e) {}
        return C;
    };
    globalThis.MutationObserver = mkObserver('MutationObserver');
    globalThis.IntersectionObserver = mkObserver('IntersectionObserver');
    globalThis.ResizeObserver = mkObserver('ResizeObserver');
    globalThis.PerformanceObserver = mkObserver('PerformanceObserver');

    globalThis.customElements = {
        define: function (name) { __ledger('customElements.define-not-upgrading:' + name); },
        get: function () { return undefined; },
        whenDefined: function () { return new Promise(function () {}); },
        upgrade: function () {},
    };
})();
"#;
