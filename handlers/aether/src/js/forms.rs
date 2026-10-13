//! HTML §4.10 forms, the script half of submission (AETHERFORMS, LEDGER SR79): constructing the entry
//! list over the script DOM (§4.10.21.4) with its `formdata` event, the `FormData` interface (XHR §4.3)
//! and `File` (File API §4) as a prelude over three natives, `FormDataEvent`'s FormData check, the
//! navigation a script-started submission stages (`submit()`, `requestSubmit()`, a script `click()` on
//! a submit button), and the two legacy platform objects of the forms chapter: `HTMLFormElement`'s
//! indexed and named properties ([LegacyOverrideBuiltIns], the past names map, RadioNodeList) and
//! `HTMLSelectElement`'s indexed getter/setter.
//!
//! The control state it reads is `controls`' (values, checkedness, selectedness, disabled); the
//! encodings and the action/method/enctype resolution are the engine's `crate::forms` (the bytes
//! Chromium sends). Installed after `net::install` (the prelude's `File` extends the fetch prelude's
//! `Blob`).

use super::controls;
use super::dom::{self, attr_value, this_element, with_doc, wrap};
use super::idl::*;
use super::{page, TargetKey};
use crate::forms::{self as core, Entry, EntryValue, HttpMethod, OpenDocument};
use html_core::{Document, Namespace, NodeId};
use js_core::vm::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

/// Host roots of the prelude's hidden helpers, captured at install (a page cannot swap them).
#[derive(Default, Clone, Copy)]
struct Helpers {
    /// `__formdata_from(pairs)` → a FormData over `[name, value]` pairs (null value: the empty file).
    from: Option<usize>,
    /// `__formdata_pairs(fd)` → a copy of a FormData's entry list as `[name, value]` pairs.
    pairs: Option<usize>,
    /// `__formdata_is(v)` → whether `v` is a FormData.
    is: Option<usize>,
    /// `__blob_byte_array(blob)` → the blob's bytes as an array of numbers.
    bytes: Option<usize>,
}

thread_local! {
    static HELPERS: Cell<Helpers> = Cell::new(Helpers { from: None, pairs: None, is: None, bytes: None });
    /// Forms whose "constructing entry list" flag is set (§4.10.21.4).
    static CONSTRUCTING: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    /// The user click's coordinates in an image submit button while [`user_submission`] builds its list.
    static CLICK_COORDS: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
    /// Each form's past names map (§4.10.3): name → element, in insertion order.
    static PAST_NAMES: RefCell<HashMap<usize, Vec<(String, usize)>>> = RefCell::new(HashMap::new());
    static FORM_HANDLER: Cell<Option<Obj>> = const { Cell::new(None) };
    static SELECT_HANDLER: Cell<Option<Obj>> = const { Cell::new(None) };
}

/// Per-page state off (a new realm is about to be built).
pub fn reset() {
    HELPERS.with(|h| h.set(Helpers::default()));
    CONSTRUCTING.with(|c| c.borrow_mut().clear());
    PAST_NAMES.with(|p| p.borrow_mut().clear());
    FORM_HANDLER.with(|h| h.set(None));
    SELECT_HANDLER.with(|h| h.set(None));
}

fn call_helper(vm: &mut Vm, which: fn(&Helpers) -> Option<usize>, args: &[Value]) -> JsResult<Value> {
    let f = HELPERS.with(|h| which(&h.get())).map(|r| rooted(vm, r)).unwrap_or(Value::Undefined);
    if !vm.is_callable(&f) {
        return Ok(Value::Undefined);
    }
    vm.call(&f, &Value::Undefined, args)
}

fn html_local_in(d: &Document, el: usize) -> Option<String> {
    d.element(NodeId(el)).filter(|e| e.ns == Namespace::Html).map(|e| e.local.to_string())
}

fn local(el: usize) -> String {
    with_doc(|d| html_local_in(d, el)).unwrap_or_default()
}

fn ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

// ================================================================================================
// The entry list (§4.10.21.4)
// ================================================================================================

/// An entry the DOM yields before script sees it: a string, or the empty file a file control with no
/// selection submits.
enum Raw {
    Text(String),
    EmptyFile,
    /// A file input with a selection (AETHERDEBT SR93, `input.files = dataTransfer.files`): one entry
    /// per selected File, read with the VM.
    Files(usize),
}

/// A submit button (§4.10.18.6): `<button>` in the Submit Button state, `<input type=submit|image>`.
pub fn is_submit_button(el: usize) -> bool {
    match local(el).as_str() {
        "button" => !matches!(attr_value(el, "type").unwrap_or_default().to_ascii_lowercase().as_str(), "reset" | "button"),
        "input" => matches!(controls::input_type(el), "submit" | "image"),
        _ => false,
    }
}

fn in_datalist(el: usize) -> bool {
    with_doc(|d| {
        let mut cur = d.parent(NodeId(el));
        while let Some(p) = cur {
            if html_local_in(d, p.0).as_deref() == Some("datalist") {
                return true;
            }
            cur = d.parent(p);
        }
        false
    })
}

/// The submittable elements (`button`, `input`, `select`, `textarea`) whose form owner is `form`, in
/// tree order.
fn submittable(form: usize) -> Vec<usize> {
    let root = dom::root_of(form);
    with_doc(|d| {
        d.descendants(NodeId(root))
            .map(|n| n.0)
            .filter(|&n| matches!(html_local_in(d, n).as_deref(), Some("button" | "input" | "select" | "textarea")))
            .filter(|&n| controls::form_owner_in(d, n) == Some(form))
            .collect()
    })
}

/// An option's value: the `value` attribute, else its text with ASCII whitespace stripped and collapsed.
fn option_value(o: usize) -> String {
    attr_value(o, "value").unwrap_or_else(|| {
        let t = with_doc(|d| dom::text_content_of(d, o));
        t.split(ws).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
    })
}

/// The textarea wrapping transformation (§4.10.11): with `wrap=hard`, LF line breaks so no line is longer
/// than `cols` characters — after the last space that fits, else at `cols`. Lines already short enough
/// are untouched, so the transformation is idempotent.
fn hard_wrap(el: usize, v: String) -> String {
    if !attr_value(el, "wrap").is_some_and(|w| w.eq_ignore_ascii_case("hard")) {
        return v;
    }
    let cols = attr_value(el, "cols").and_then(|c| crate::js::reflect::parse_int(&c)).filter(|n| *n > 0).map(|n| n as usize).unwrap_or(20);
    let mut out = String::with_capacity(v.len() + 8);
    for (i, line) in v.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut rest: Vec<char> = line.chars().collect();
        while rest.len() > cols {
            let brk = rest[..cols].iter().rposition(|c| *c == ' ').map(|p| p + 1).unwrap_or(cols);
            out.extend(&rest[..brk]);
            out.push('\n');
            rest.drain(..brk);
        }
        out.extend(rest.iter());
    }
    out
}

/// Steps 5.1–5.12 of "construct the entry list" over the script DOM: the entries before `formdata`.
/// A script-started submission has no click coordinates (an image button submits `x=0`, `y=0`).
fn raw_entries(form: usize, submitter: Option<usize>) -> Vec<(String, Raw)> {
    let mut out: Vec<(String, Raw)> = Vec::new();
    for field in submittable(form) {
        if in_datalist(field) || controls::disabled(field) {
            continue;
        }
        let tag = local(field);
        let ty = if tag == "input" { controls::input_type(field) } else { "" };
        let is_button = tag == "button" || matches!(ty, "submit" | "image" | "reset" | "button");
        if is_button && submitter != Some(field) {
            continue;
        }
        if matches!(ty, "checkbox" | "radio") && !super::html::checkedness(field) {
            continue;
        }
        if ty == "image" {
            let (nx, ny) = match attr_value(field, "name").filter(|n| !n.is_empty()) {
                Some(n) => (format!("{n}.x"), format!("{n}.y")),
                None => ("x".to_string(), "y".to_string()),
            };
            // A user click's coordinates in the image (AETHERDEBT SR93), else 0,0.
            let (cx, cy) = CLICK_COORDS.with(Cell::get).unwrap_or((0, 0));
            out.push((nx, Raw::Text(cx.to_string())));
            out.push((ny, Raw::Text(cy.to_string())));
            continue;
        }
        let Some(name) = attr_value(field, "name").filter(|n| !n.is_empty()) else { continue };
        match (tag.as_str(), ty) {
            ("select", _) => {
                for o in with_doc(|d| controls::selected_options_in(d, field)) {
                    if !controls::disabled(o) {
                        out.push((name.clone(), Raw::Text(option_value(o))));
                    }
                }
            }
            (_, "checkbox" | "radio") => out.push((name.clone(), Raw::Text(attr_value(field, "value").unwrap_or_else(|| "on".into())))),
            (_, "file") if controls::has_selected_files(field) => out.push((name.clone(), Raw::Files(field))),
            (_, "file") => out.push((name.clone(), Raw::EmptyFile)),
            (_, "hidden") if name.eq_ignore_ascii_case("_charset_") => out.push((name.clone(), Raw::Text("UTF-8".into()))),
            ("textarea", _) => out.push((name.clone(), Raw::Text(hard_wrap(field, controls::value_of(field))))),
            ("button", _) => out.push((name.clone(), Raw::Text(attr_value(field, "value").unwrap_or_default()))),
            // An input submit button without a value submits its default label (Chromium: "Submit";
            // `crate::forms::construct_entry_list` agrees, the forms oracle holds the bytes).
            (_, "submit") => out.push((name.clone(), Raw::Text(attr_value(field, "value").unwrap_or_else(|| "Submit".into())))),
            _ => out.push((name.clone(), Raw::Text(controls::value_of(field)))),
        }
        if tag == "textarea" || matches!(ty, "text" | "search") {
            if let Some(dn) = attr_value(field, "dirname").filter(|d| !d.is_empty()) {
                // The element's directionality: Aether lays text out LTR.
                out.push((dn, Raw::Text("ltr".into())));
            }
        }
    }
    out
}

/// "Construct the entry list" (§4.10.21.4): the entries, a FormData over them, a trusted bubbling
/// `formdata` FormDataEvent at the form (listeners may edit `event.formData`), and the resulting list
/// as a JS array of `[name, value]` pairs. None when the form's "constructing entry list" flag is set
/// (a `formdata` listener re-entering).
pub fn entry_list(vm: &mut Vm, form: usize, submitter: Option<usize>) -> JsResult<Option<Value>> {
    if CONSTRUCTING.with(|c| c.borrow().contains(&form)) {
        return Ok(None);
    }
    CONSTRUCTING.with(|c| c.borrow_mut().insert(form));
    let r = entry_list_inner(vm, form, submitter);
    CONSTRUCTING.with(|c| c.borrow_mut().remove(&form));
    r.map(Some)
}

fn entry_list_inner(vm: &mut Vm, form: usize, submitter: Option<usize>) -> JsResult<Value> {
    let raw = raw_entries(form, submitter);
    let mut pairs = Vec::with_capacity(raw.len());
    for (n, v) in raw {
        let pv = match v {
            Raw::Text(t) => s(&t),
            Raw::EmptyFile => Value::Null,
            Raw::Files(el) => {
                for f in controls::selected_files(vm, el) {
                    pairs.push(Value::Object(vm.new_array(vec![s(&n), f])));
                }
                continue;
            }
        };
        pairs.push(Value::Object(vm.new_array(vec![s(&n), pv])));
    }
    let arr = Value::Object(vm.new_array(pairs));
    let fd = call_helper(vm, |h| h.from, &[arr.clone()])?;
    if fd.is_undefined() {
        // No prelude (it failed to install): the raw pairs, files as the empty string.
        return Ok(arr);
    }
    let r = root(vm, fd.clone());
    let init = vm.new_plain_object();
    let _ = vm.create_data_property(init, PropertyKey::from_str("formData"), fd.clone());
    let e = super::events::new_event(vm, "FormDataEvent", "formdata", true, false, Some(init));
    let re = root(vm, Value::Object(e));
    super::events::dispatch(vm, e, TargetKey::Node(form));
    unroot(vm, re);
    let out = call_helper(vm, |h| h.pairs, &[fd]);
    unroot(vm, r);
    out
}

/// A FormData's (or a pairs array's) entries as the engine's entry list (files read through the
/// prelude: name, type, bytes).
fn entries_of(vm: &mut Vm, pairs: &Value) -> JsResult<Vec<Entry>> {
    let mut out = Vec::new();
    for p in vm.array_to_list(pairs) {
        let kv = vm.array_to_list(&p);
        let name = match kv.first() {
            Some(v) => vm.to_string(v)?.to_rust(),
            None => continue,
        };
        let value = match kv.get(1).cloned().unwrap_or(Value::Undefined) {
            Value::Object(o) => {
                let filename = get(vm, o, "name")?;
                let filename = if filename.is_undefined() { String::new() } else { vm.to_string(&filename)?.to_rust() };
                let mime = get(vm, o, "type")?;
                let mime = if mime.is_undefined() { String::new() } else { vm.to_string(&mime)?.to_rust() };
                let b = call_helper(vm, |h| h.bytes, &[Value::Object(o)])?;
                let bytes: Vec<u8> = vm
                    .array_to_list(&b)
                    .into_iter()
                    .map(|v| match v {
                        Value::Number(n) => n as u8,
                        _ => 0,
                    })
                    .collect();
                EntryValue::File {
                    filename,
                    mime: if mime.is_empty() { "application/octet-stream".into() } else { mime },
                    bytes,
                }
            }
            Value::Null | Value::Undefined => EntryValue::File { filename: String::new(), mime: "application/octet-stream".into(), bytes: Vec::new() },
            v => EntryValue::Text(vm.to_string(&v)?.to_rust()),
        };
        out.push(Entry { name, value });
    }
    Ok(out)
}

/// Whether `v` is a FormData (FormDataEvent's required `formData` member).
pub fn is_form_data(vm: &mut Vm, v: &Value) -> JsResult<bool> {
    if !v.is_object() {
        return Ok(false);
    }
    let r = call_helper(vm, |h| h.is, &[v.clone()])?;
    Ok(vm.to_boolean(&r))
}

// ================================================================================================
// Submission (§4.10.21.3, from the entry list on)
// ================================================================================================

/// Steps 6–23 of "submit" for a script-started submission (after the submit event, or straight from
/// `submit()`): the entry list (firing `formdata`), then the navigation `crate::forms::plan_navigation`
/// builds, staged for the shell — a GET as the script navigation (`location`'s path), a POST as the
/// staged form submission (`js::take_form_submission`). Nothing when the form is disconnected (it
/// cannot navigate) or already constructing its entry list.
pub fn navigate(vm: &mut Vm, form: usize, submitter: Option<usize>) {
    if !dom::is_connected(form) || CONSTRUCTING.with(|c| c.borrow().contains(&form)) {
        return;
    }
    let Ok(Some(pairs)) = entry_list(vm, form, submitter) else { return };
    if !dom::is_connected(form) {
        return;
    }
    let r = root(vm, pairs.clone());
    let entries = entries_of(vm, &pairs);
    unroot(vm, r);
    let Ok(entries) = entries else { return };
    let Some(doc) = page(|p| p.doc.clone()) else { return };
    let form_ref = doc.node_at(NodeId(form));
    let sub_ref = submitter.map(|s| doc.node_at(NodeId(s)));
    let url = super::page_url();
    let Some(nav) = core::plan_navigation(&url, &form_ref, sub_ref.as_ref(), entries) else { return };
    // AETHERDEBT SR93: CSP form-action decides whether the submission navigates.
    if !crate::security::form_action_allowed(Some(form), &nav.url) {
        return;
    }
    stage(nav);
}

/// The submission a USER starts (a click on a submit button, implicit submission — AETHERDEBT SR93,
/// the AETHERFORMS owed user-click path): after the shell's `submit` event, the entry list through the
/// script DOM so the trusted `formdata` event fires at the form and its listeners' edits count (and a
/// file input's script-set files submit), image button coordinates from the click, then the
/// navigation `crate::forms::plan_navigation` builds against `base` — returned for the shell to stage
/// (not staged here). None when the form is disconnected or already constructing its entry list.
pub fn user_submission(vm: &mut Vm, form: usize, submitter: Option<usize>, coords: (i32, i32), base: &str) -> Option<OpenDocument> {
    if !dom::is_connected(form) || CONSTRUCTING.with(|c| c.borrow().contains(&form)) {
        return None;
    }
    CLICK_COORDS.with(|c| c.set(Some(coords)));
    let list = entry_list(vm, form, submitter);
    CLICK_COORDS.with(|c| c.set(None));
    let Ok(Some(pairs)) = list else { return None };
    if !dom::is_connected(form) {
        return None;
    }
    let r = root(vm, pairs.clone());
    let entries = entries_of(vm, &pairs);
    unroot(vm, r);
    let entries = entries.ok()?;
    let doc = page(|p| p.doc.clone())?;
    let form_ref = doc.node_at(NodeId(form));
    let sub_ref = submitter.map(|s| doc.node_at(NodeId(s)));
    core::plan_navigation(base, &form_ref, sub_ref.as_ref(), entries)
}

fn stage(nav: OpenDocument) {
    match nav.method {
        HttpMethod::Get => page(|p| p.pending_nav = Some(nav.url)),
        HttpMethod::Post => page(|p| p.pending_submit = Some(nav)),
    }
}

/// `form.submit()`: submit from the form itself, "submitted from submit() method" — no validation, no
/// submit event.
fn form_submit(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    navigate(vm, el, None);
    Ok(Value::Undefined)
}

// ================================================================================================
// Natives behind the prelude
// ================================================================================================

/// `__form_entry_list(form, submitter)` → pairs, or null while the form constructs its list.
fn n_entry_list(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(form) = dom::node_of(vm, &arg(vm, ctx, 0)).filter(|f| local(*f) == "form") else {
        return vm.throw_type("Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.");
    };
    let sub = dom::node_of(vm, &arg(vm, ctx, 1));
    Ok(entry_list(vm, form, sub)?.unwrap_or(Value::Null))
}

/// `__form_submitter_check(form, submitter)` → 0 ok, 1 not a submit button, 2 not owned by the form.
fn n_submitter_check(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let form = dom::node_of(vm, &arg(vm, ctx, 0));
    let Some(b) = dom::node_of(vm, &arg(vm, ctx, 1)) else { return Ok(num(1.0)) };
    if !is_submit_button(b) {
        return Ok(num(1.0));
    }
    if form.is_none() || controls::form_owner(b) != form {
        return Ok(num(2.0));
    }
    Ok(num(0.0))
}

/// `__formdata_encode(fd)` → `[bytes, content type]`: the FormData as `multipart/form-data` (Fetch's
/// body extraction; the encoder is `crate::forms::multipart`).
fn n_formdata_encode(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let fd = arg(vm, ctx, 0);
    let pairs = call_helper(vm, |h| h.pairs, &[fd])?;
    let entries = entries_of(vm, &pairs)?;
    let boundary = core::new_boundary();
    let bytes = core::multipart(&entries, &boundary);
    let list: Vec<Value> = bytes.iter().map(|b| num(*b as f64)).collect();
    let b = Value::Object(vm.new_array(list));
    let ct = s(&format!("multipart/form-data; boundary={boundary}"));
    Ok(Value::Object(vm.new_array(vec![b, ct])))
}

/// FormData, File and the hidden helpers over the natives above.
const PRELUDE: &str = r#"
(function () {
    'use strict';
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var str = function (v) {
        if (typeof v === 'symbol') { throw new TypeError('Cannot convert a Symbol value to a string'); }
        return String(v);
    };
    // WebIDL USVString: lone surrogates become U+FFFD.
    var usv = function (v) {
        var s = str(v), out = '', i, c, d;
        for (i = 0; i < s.length; i++) {
            c = s.charCodeAt(i);
            if (c >= 0xD800 && c <= 0xDBFF) {
                d = i + 1 < s.length ? s.charCodeAt(i + 1) : 0;
                if (d >= 0xDC00 && d <= 0xDFFF) { out += s[i] + s[i + 1]; i++; continue; }
                out += '�';
                continue;
            }
            if (c >= 0xDC00 && c <= 0xDFFF) { out += '�'; continue; }
            out += s[i];
        }
        return out;
    };

    // ---- File (File API §4) ----------------------------------------------------------------------
    if (typeof File === 'undefined' && typeof Blob === 'function') {
        var FST = new WeakMap();
        var fst = function (o) {
            var s = o !== null && typeof o === 'object' ? FST.get(o) : undefined;
            if (!s) { throw new TypeError('Illegal invocation'); }
            return s;
        };
        class File extends Blob {
            constructor(bits, name, opts) {
                if (arguments.length < 2) {
                    throw new TypeError("Failed to construct 'File': 2 arguments required, but only " + arguments.length + ' present.');
                }
                super(bits, opts);
                // AETHERWPT2 (SR135): `lastModified` is a WebIDL `long long` (NaN / ±Infinity → 0, truncated, wrapped
                // modulo 2^64 into the signed range).
                var lm = opts !== undefined && opts !== null && opts.lastModified !== undefined ? +opts.lastModified : Date.now();
                if (!isFinite(lm)) { lm = 0; }
                lm = Math.trunc(lm);
                if (lm >= 9223372036854775808 || lm < -9223372036854775808) {
                    lm = lm % 18446744073709551616;
                    if (lm >= 9223372036854775808) { lm -= 18446744073709551616; }
                    else if (lm < -9223372036854775808) { lm += 18446744073709551616; }
                }
                FST.set(this, { name: usv(name), lastModified: lm === 0 ? 0 : lm });
            }
            get name() { return fst(this).name; }
            get lastModified() { return fst(this).lastModified; }
            get webkitRelativePath() { fst(this); return ''; }
        }
        Object.defineProperty(File.prototype, Symbol.toStringTag, { value: 'File', configurable: true });
        hide(globalThis, 'File', File);
    }
    var emptyFile = function () {
        return typeof File === 'function' ? new File([], '', { type: 'application/octet-stream' }) : '';
    };

    // ---- FormData (XHR §4.3) ---------------------------------------------------------------------
    var ST = new WeakMap();
    var st = function (o) {
        var s = o !== null && typeof o === 'object' ? ST.get(o) : undefined;
        if (!s) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var need = function (n, min, op) {
        if (n < min) {
            throw new TypeError("Failed to execute '" + op + "' on 'FormData': " + min + ' argument' + (min > 1 ? 's' : '') + ' required, but only ' + n + ' present.');
        }
    };
    var isBlob = function (v) { return typeof Blob === 'function' && v instanceof Blob; };
    // "create an entry": a Blob becomes a File ("blob" when unnamed); a filename renames.
    var entryValue = function (value, filename) {
        if (isBlob(value)) {
            var isFile = typeof File === 'function' && value instanceof File;
            if (!isFile || filename !== undefined) {
                var o = { type: value.type };
                if (isFile) { o.lastModified = value.lastModified; }
                return new File([value], filename !== undefined ? usv(filename) : (isFile ? value.name : 'blob'), o);
            }
            return value;
        }
        return usv(value);
    };
    var checkBlobArg = function (n, value, op) {
        if (n > 2 && !isBlob(value)) {
            throw new TypeError("Failed to execute '" + op + "' on 'FormData': parameter 2 is not of type 'Blob'.");
        }
    };
    class FormData {
        constructor(form, submitter) {
            var list = [];
            if (form !== undefined) {
                if (typeof HTMLFormElement !== 'function' || !(form instanceof HTMLFormElement)) {
                    throw new TypeError("Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.");
                }
                if (submitter !== undefined && submitter !== null) {
                    var r = __form_submitter_check(form, submitter);
                    if (r === 1) { throw new TypeError("Failed to construct 'FormData': The specified element is not a submit button."); }
                    if (r === 2) { throw new DOMException("Failed to construct 'FormData': The specified element is not owned by this form element.", 'NotFoundError'); }
                } else {
                    submitter = null;
                }
                var l = __form_entry_list(form, submitter);
                if (l === null) {
                    throw new DOMException("Failed to construct 'FormData': The form is constructing its entry list.", 'InvalidStateError');
                }
                list = l;
            }
            ST.set(this, { list: list });
        }
        append(name, value, filename) {
            var s = st(this);
            need(arguments.length, 2, 'append');
            checkBlobArg(arguments.length, value, 'append');
            s.list.push([usv(name), entryValue(value, arguments.length > 2 ? filename : undefined)]);
        }
        delete(name) {
            var s = st(this);
            need(arguments.length, 1, 'delete');
            name = usv(name);
            s.list = s.list.filter(function (e) { return e[0] !== name; });
        }
        get(name) {
            var s = st(this);
            need(arguments.length, 1, 'get');
            name = usv(name);
            for (var i = 0; i < s.list.length; i++) { if (s.list[i][0] === name) { return s.list[i][1]; } }
            return null;
        }
        getAll(name) {
            var s = st(this);
            need(arguments.length, 1, 'getAll');
            name = usv(name);
            return s.list.filter(function (e) { return e[0] === name; }).map(function (e) { return e[1]; });
        }
        has(name) {
            var s = st(this);
            need(arguments.length, 1, 'has');
            name = usv(name);
            return s.list.some(function (e) { return e[0] === name; });
        }
        set(name, value, filename) {
            var s = st(this);
            need(arguments.length, 2, 'set');
            checkBlobArg(arguments.length, value, 'set');
            name = usv(name);
            var e = [name, entryValue(value, arguments.length > 2 ? filename : undefined)];
            var at = -1, out = [];
            for (var i = 0; i < s.list.length; i++) {
                if (s.list[i][0] === name) {
                    if (at < 0) { at = out.length; out.push(e); }
                } else {
                    out.push(s.list[i]);
                }
            }
            if (at < 0) { out.push(e); }
            s.list = out;
        }
        forEach(cb, thisArg) {
            var s = st(this);
            need(arguments.length, 1, 'forEach');
            if (typeof cb !== 'function') { throw new TypeError("Failed to execute 'forEach' on 'FormData': The callback provided as parameter 1 is not a function."); }
            for (var i = 0; i < s.list.length; i++) { cb.call(thisArg, s.list[i][1], s.list[i][0], this); }
        }
        entries() { return iter(this, 'entries'); }
        keys() { return iter(this, 'keys'); }
        values() { return iter(this, 'values'); }
    }
    Object.defineProperty(FormData.prototype, Symbol.iterator, { value: FormData.prototype.entries, writable: true, configurable: true });
    Object.defineProperty(FormData.prototype, Symbol.toStringTag, { value: 'FormData', configurable: true });

    // The default iterator (WebIDL §3.7.10.1): live over the entry list.
    var IST = new WeakMap();
    var IterProto = Object.create(Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())));
    hide(IterProto, 'next', function next() {
        var it = IST.get(this);
        if (!it) { throw new TypeError('Illegal invocation'); }
        var l = st(it.fd).list;
        if (it.pos >= l.length) { return { value: undefined, done: true }; }
        var e = l[it.pos++];
        return { value: it.kind === 'keys' ? e[0] : it.kind === 'values' ? e[1] : [e[0], e[1]], done: false };
    });
    Object.defineProperty(IterProto, Symbol.toStringTag, { value: 'FormData Iterator', configurable: true });
    var iter = function (fd, kind) {
        st(fd);
        var it = Object.create(IterProto);
        IST.set(it, { fd: fd, kind: kind, pos: 0 });
        return it;
    };
    hide(globalThis, 'FormData', FormData);

    // ---- helpers the engine calls ----------------------------------------------------------------
    hide(globalThis, '__formdata_from', function (pairs) {
        var fd = new FormData();
        var l = st(fd).list;
        for (var i = 0; i < pairs.length; i++) {
            l.push([pairs[i][0], pairs[i][1] === null ? emptyFile() : pairs[i][1]]);
        }
        return fd;
    });
    hide(globalThis, '__formdata_pairs', function (fd) {
        return st(fd).list.map(function (e) { return [e[0], e[1]]; });
    });
    hide(globalThis, '__formdata_is', function (v) { return v !== null && typeof v === 'object' && ST.has(v); });
    hide(globalThis, '__blob_byte_array', function (b) {
        return typeof __blob_bytes === 'function' ? Array.from(__blob_bytes(b)) : [];
    });
})();
"#;

// ================================================================================================
// HTMLFormElement / HTMLSelectElement as legacy platform objects (WebIDL §3.9)
// ================================================================================================

/// The wrapper for node `id`: the platform object itself, or — for an HTML `form` or `select` — a
/// proxy over it whose traps add the interface's indexed (and, for form, named) properties.
pub fn legacy_wrapper(vm: &mut Vm, id: usize, target: Obj) -> Obj {
    let handler = match local(id).as_str() {
        "form" => FORM_HANDLER.with(Cell::get),
        "select" => SELECT_HANDLER.with(Cell::get),
        _ => None,
    };
    let Some(handler) = handler else { return target };
    vm.alloc(ObjectData::new(
        None,
        Kind::Proxy(Some(Box::new(ProxyData { target, handler, callable: false, ctor: false, revoked: false }))),
    ))
}

fn matches_name(d: &Document, n: usize, name: &str) -> bool {
    d.element(NodeId(n)).is_some_and(|e| e.attr("id") == Some(name) || (e.ns == Namespace::Html && e.attr("name") == Some(name)))
}

/// The form's named-getter candidates (§4.10.3): listed elements (image buttons aside) owned by the form
/// whose id or name is `name`; with none, `img` descendants so named.
fn form_candidates(form: usize, name: &str) -> Vec<usize> {
    if name.is_empty() {
        return Vec::new();
    }
    let listed: Vec<usize> = with_doc(|d| controls::form_elements_in(d, form).into_iter().filter(|&n| matches_name(d, n, name)).collect());
    if !listed.is_empty() {
        return listed;
    }
    with_doc(|d| {
        d.descendants(NodeId(form))
            .map(|n| n.0)
            .filter(|&n| html_local_in(d, n).as_deref() == Some("img") && matches_name(d, n, name))
            .collect()
    })
}

/// A past-names entry still stands while its element keeps this form as its owner (an `img`: stays
/// inside it).
fn past_valid(form: usize, el: usize) -> bool {
    with_doc(|d| match html_local_in(d, el).as_deref() {
        Some("img") => {
            let mut cur = d.parent(NodeId(el));
            while let Some(p) = cur {
                if p.0 == form {
                    return true;
                }
                cur = d.parent(p);
            }
            false
        }
        _ => controls::form_owner_in(d, el) == Some(form),
    })
}

fn past_lookup(form: usize, name: &str) -> Option<usize> {
    let hit = PAST_NAMES.with(|p| p.borrow().get(&form).and_then(|v| v.iter().find(|(n, _)| n == name).map(|(_, e)| *e)))?;
    if past_valid(form, hit) {
        return Some(hit);
    }
    // The element changed form owner: its entries leave the map.
    PAST_NAMES.with(|p| {
        if let Some(v) = p.borrow_mut().get_mut(&form) {
            v.retain(|(_, e)| *e != hit);
        }
    });
    None
}

fn past_record(form: usize, name: &str, el: usize) {
    PAST_NAMES.with(|p| {
        let mut p = p.borrow_mut();
        let v = p.entry(form).or_default();
        match v.iter_mut().find(|(n, _)| n == name) {
            Some(e) => e.1 = el,
            None => v.push((name.to_string(), el)),
        }
    });
}

/// The form's named getter: one candidate → that element (remembered in the past names map); several →
/// a live RadioNodeList; none → the past names map.
fn form_named(vm: &mut Vm, form: usize, name: &str) -> Option<Value> {
    let c = form_candidates(form, name);
    match c.len() {
        0 => past_lookup(form, name).map(|e| wrap(vm, e)),
        1 => {
            past_record(form, name, c[0]);
            Some(wrap(vm, c[0]))
        }
        _ => Some(Value::Object(dom::make_collection(vm, dom::CK_RADIO, form, name, ""))),
    }
}

/// The form's supported property names: ids then names of its listed elements, `img` descendants'
/// ids and names, then the past names map.
fn form_names(form: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |v: &mut Vec<String>, n: Option<&str>| {
        if let Some(n) = n.filter(|n| !n.is_empty()) {
            if !v.iter().any(|x| x == n) {
                v.push(n.to_string());
            }
        }
    };
    with_doc(|d| {
        for n in controls::form_elements_in(d, form) {
            if let Some(e) = d.element(NodeId(n)) {
                push(&mut out, e.attr("id"));
                if e.ns == Namespace::Html {
                    push(&mut out, e.attr("name"));
                }
            }
        }
        for n in d.descendants(NodeId(form)) {
            if html_local_in(d, n.0).as_deref() == Some("img") {
                if let Some(e) = d.element(n) {
                    push(&mut out, e.attr("id"));
                    push(&mut out, e.attr("name"));
                }
            }
        }
    });
    let past: Vec<(String, usize)> = PAST_NAMES.with(|p| p.borrow().get(&form).cloned().unwrap_or_default());
    for (n, e) in past {
        if past_valid(form, e) && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// `form.elements.namedItem(name)` / `form.elements[name]` (HTMLFormControlsCollection, §2.7.2.4): one
/// match → the element, several → a RadioNodeList.
pub fn controls_named_item(vm: &mut Vm, form: usize, name: &str) -> Option<Value> {
    if name.is_empty() {
        return None;
    }
    let hits: Vec<usize> = with_doc(|d| controls::form_elements_in(d, form).into_iter().filter(|&n| matches_name(d, n, name)).collect());
    match hits.len() {
        0 => None,
        1 => Some(wrap(vm, hits[0])),
        _ => Some(Value::Object(dom::make_collection(vm, dom::CK_RADIO, form, name, ""))),
    }
}

/// `form.elements` filtered by `name` (a RadioNodeList's members).
pub fn radio_list_in(d: &Document, form: usize, name: &str) -> Vec<usize> {
    controls::form_elements_in(d, form).into_iter().filter(|&n| matches_name(d, n, name)).collect()
}

fn select_options_of(sel: usize) -> Vec<usize> {
    with_doc(|d| controls::list_of_options_in(d, sel))
}

fn trap_node(vm: &Vm, ctx: &CallCtx) -> Option<(Obj, usize)> {
    let t = vm.arg(ctx, 0).as_object()?;
    if tag_of(vm, t) != Some(T_NODE) {
        return None;
    }
    let n = slot_num(vm, t, 0);
    n.is_finite().then_some((t, n as usize))
}

fn key_index(k: &PropertyKey) -> Option<usize> {
    match k {
        PropertyKey::Index(i) => Some(*i as usize),
        _ => None,
    }
}

fn key_name(k: &PropertyKey) -> Option<String> {
    match k {
        PropertyKey::Str(s) => Some(s.to_rust()),
        _ => None,
    }
}

fn own(vm: &Vm, t: Obj, key: &PropertyKey) -> bool {
    vm.heap.get(t).props.get(key).is_some()
}

fn desc(vm: &mut Vm, v: Value, writable: bool, enumerable: bool) -> Value {
    let o = vm.new_plain_object();
    let _ = vm.create_data_property(o, PropertyKey::from_str("value"), v);
    let _ = vm.create_data_property(o, PropertyKey::from_str("writable"), Value::Bool(writable));
    let _ = vm.create_data_property(o, PropertyKey::from_str("enumerable"), Value::Bool(enumerable));
    let _ = vm.create_data_property(o, PropertyKey::from_str("configurable"), Value::Bool(true));
    Value::Object(o)
}

/// Item `i` of the element's indexed properties: form → `elements[i]`, select → `options[i]`.
fn indexed(vm: &mut Vm, el: usize, i: usize) -> Option<Value> {
    let ids = if local(el) == "form" { with_doc(|d| controls::form_elements_in(d, el)) } else { select_options_of(el) };
    ids.get(i).copied().map(|n| wrap(vm, n))
}

fn indexed_len(el: usize) -> usize {
    if local(el) == "form" {
        with_doc(|d| controls::form_elements_in(d, el)).len()
    } else {
        select_options_of(el).len()
    }
}

/// Named lookup for the legacy object (only forms have named properties), honoring
/// [LegacyOverrideBuiltIns]: an own property hides the named one, the prototype chain does not.
fn named(vm: &mut Vm, t: Obj, el: usize, key: &PropertyKey) -> Option<Value> {
    let name = key_name(key)?;
    if local(el) != "form" || own(vm, t, key) {
        return None;
    }
    form_named(vm, el, &name)
}

fn trap_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = trap_node(vm, ctx) else { return Ok(Value::Undefined) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    let receiver = vm.arg(ctx, 2);
    if let Some(i) = key_index(&key) {
        if let Some(v) = indexed(vm, el, i) {
            return Ok(v);
        }
    } else if let Some(v) = named(vm, t, el, &key) {
        return Ok(v);
    }
    vm.get_with_receiver(t, &key, &receiver)
}

fn trap_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = trap_node(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if let Some(i) = key_index(&key) {
        if i < indexed_len(el) {
            return Ok(Value::Bool(true));
        }
    } else if named(vm, t, el, &key).is_some() {
        return Ok(Value::Bool(true));
    }
    Ok(Value::Bool(vm.has_property(t, &key)?))
}

fn trap_gopd(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = trap_node(vm, ctx) else { return Ok(Value::Undefined) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if let Some(i) = key_index(&key) {
        if let Some(v) = indexed(vm, el, i) {
            // A select's indexed properties have a setter: writable.
            let writable = local(el) == "select";
            return Ok(desc(vm, v, writable, true));
        }
    } else if let Some(v) = named(vm, t, el, &key) {
        // [LegacyUnenumerableNamedProperties].
        return Ok(desc(vm, v, false, false));
    }
    match vm.get_own_property(t, &key)? {
        Some(d) => Ok(vm.from_property_descriptor(&d)),
        None => Ok(Value::Undefined),
    }
}

fn trap_own_keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = trap_node(vm, ctx) else { return Ok(Value::Object(vm.new_array(Vec::new()))) };
    let n = indexed_len(el);
    let mut names: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    if local(el) == "form" {
        for nm in form_names(el) {
            if !names.contains(&nm) {
                names.push(nm);
            }
        }
    }
    let mut keys: Vec<Value> = Vec::new();
    for k in vm.ordinary_own_keys(t) {
        if let Some(nm) = match &k {
            PropertyKey::Index(i) => Some(i.to_string()),
            PropertyKey::Str(s) => Some(s.to_rust()),
            PropertyKey::Sym(_) => None,
        } {
            if names.contains(&nm) {
                continue;
            }
        }
        keys.push(k.to_value());
    }
    let mut out: Vec<Value> = names.iter().map(|x| s(x)).collect();
    out.extend(keys);
    Ok(Value::Object(vm.new_array(out)))
}

fn trap_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = trap_node(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    let v = vm.arg(ctx, 2);
    let receiver = vm.arg(ctx, 3);
    if let Some(i) = key_index(&key) {
        if local(el) == "select" {
            // HTMLSelectElement's indexed setter (the options collection's).
            controls::options_set_index(vm, el, i, &v)?;
            return Ok(Value::Bool(true));
        }
        return Ok(Value::Bool(false));
    }
    if named(vm, t, el, &key).is_some() {
        return Ok(Value::Bool(false));
    }
    let receiver = if receiver.is_object() { receiver } else { Value::Object(t) };
    Ok(Value::Bool(vm.set(t, key, v, &receiver)?))
}

fn trap_define(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = trap_node(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    let dv = vm.arg(ctx, 2);
    if let Some(i) = key_index(&key) {
        if local(el) == "select" {
            let d = vm.to_property_descriptor(&dv)?;
            if d.get.is_some() || d.set.is_some() {
                return Ok(Value::Bool(false));
            }
            let v = d.value.clone().unwrap_or(Value::Undefined);
            controls::options_set_index(vm, el, i, &v)?;
            return Ok(Value::Bool(true));
        }
        return Ok(Value::Bool(false));
    }
    if named(vm, t, el, &key).is_some() {
        return Ok(Value::Bool(false));
    }
    let d = vm.to_property_descriptor(&dv)?;
    Ok(Value::Bool(vm.define_own_property(t, key, d)?))
}

fn trap_delete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = trap_node(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if let Some(i) = key_index(&key) {
        if i < indexed_len(el) {
            return Ok(Value::Bool(false));
        }
        return Ok(Value::Bool(vm.delete(t, &key)?));
    }
    if named(vm, t, el, &key).is_some() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(vm.delete(t, &key)?))
}

fn make_handler(vm: &mut Vm) -> Obj {
    let handler = vm.new_plain_object();
    for (name, f, len) in [
        ("get", trap_get as NativeFn, 3),
        ("has", trap_has, 2),
        ("getOwnPropertyDescriptor", trap_gopd, 2),
        ("ownKeys", trap_own_keys, 1),
        ("set", trap_set, 4),
        ("defineProperty", trap_define, 3),
        ("deleteProperty", trap_delete, 2),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(handler).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
    }
    root(vm, Value::Object(handler));
    handler
}

// ================================================================================================
// RadioNodeList, HTMLSelectElement.item/namedItem
// ================================================================================================

/// The RadioNodeList behind `this`: (form, name).
fn this_radio(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(usize, String)> {
    let Some(t) = this_tagged(vm, &ctx.this, T_COLL).filter(|t| slot_num(vm, *t, 0) as u8 == dom::CK_RADIO) else {
        return vm.throw_type("Illegal invocation");
    };
    let form = slot_num(vm, t, 1) as usize;
    let name = match slot(vm, t, 2) {
        Value::String(s) => s.to_rust(),
        _ => String::new(),
    };
    Ok((form, name))
}

fn radio_value_of(n: usize) -> String {
    attr_value(n, "value").unwrap_or_else(|| "on".into())
}

fn is_radio(n: usize) -> bool {
    local(n) == "input" && controls::input_type(n) == "radio"
}

/// `RadioNodeList.value` (§2.7.2.5): the first checked radio's value, else "".
fn radio_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (form, name) = this_radio(vm, ctx)?;
    let items = with_doc(|d| radio_list_in(d, form, &name));
    let hit = items.into_iter().find(|&n| is_radio(n) && super::html::checkedness(n));
    Ok(s(&hit.map(radio_value_of).unwrap_or_default()))
}

/// Setting it checks the first radio whose value is the new value.
fn radio_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (form, name) = this_radio(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    let items = with_doc(|d| radio_list_in(d, form, &name));
    if let Some(n) = items.into_iter().find(|&n| is_radio(n) && radio_value_of(n) == v) {
        super::html::set_checkedness(vm, n, true);
        super::touch();
        crate::css::note_change(page(|p| p.version), crate::css::Change::Attribute { el: n, name: "checked".into(), old: None, new: None }); // AETHERPAINTPERF2 (SR132): form state, which no selector reads (css_core matches the attribute)
    }
    Ok(Value::Undefined)
}

fn this_select(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    let el = this_element(vm, ctx)?;
    if local(el) != "select" {
        return vm.throw_type("Illegal invocation");
    }
    Ok(el)
}

/// `select.item(index)`.
fn select_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_select(vm, ctx)?;
    need(vm, ctx, 1, "HTMLSelectElement", "item")?;
    let i = unsigned_long(vm, &arg(vm, ctx, 0))? as usize;
    Ok(select_options_of(el).get(i).copied().map(|n| wrap(vm, n)).unwrap_or(Value::Null))
}

/// `select.namedItem(name)`: the first option whose id or name is `name`.
fn select_named_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_select(vm, ctx)?;
    need(vm, ctx, 1, "HTMLSelectElement", "namedItem")?;
    let name = string(vm, &arg(vm, ctx, 0))?;
    if name.is_empty() {
        return Ok(Value::Null);
    }
    let opts = select_options_of(el);
    let hit = with_doc(|d| opts.into_iter().find(|&n| matches_name(d, n, &name)));
    Ok(hit.map(|n| wrap(vm, n)).unwrap_or(Value::Null))
}

// ================================================================================================
// Install
// ================================================================================================

pub fn install(vm: &mut Vm) {
    FORM_HANDLER.with(|h| h.set(None));
    SELECT_HANDLER.with(|h| h.set(None));
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__form_entry_list", 2, n_entry_list as NativeFn),
        ("__form_submitter_check", 2, n_submitter_check),
        ("__formdata_encode", 1, n_formdata_encode),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("forms-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] forms prelude failed: {m}");
    }
    let mut h = Helpers::default();
    for (name, slot) in [("__formdata_from", 0), ("__formdata_pairs", 1), ("__formdata_is", 2), ("__blob_byte_array", 3)] {
        let v = vm.get(g, &PropertyKey::from_str(name)).unwrap_or(Value::Undefined);
        if !vm.is_callable(&v) {
            continue;
        }
        let r = Some(root(vm, v));
        match slot {
            0 => h.from = r,
            1 => h.pairs = r,
            2 => h.is = r,
            _ => h.bytes = r,
        }
    }
    HELPERS.with(|c| c.set(h));

    if let Some(f) = dom::iface("HTMLFormElement") {
        op(vm, f.proto, "submit", 0, form_submit);
    }
    if let Some(sp) = dom::iface("HTMLSelectElement") {
        op(vm, sp.proto, "item", 1, select_item);
        op(vm, sp.proto, "namedItem", 1, select_named_item);
    }
    if let Some(hc) = dom::iface("HTMLCollection") {
        let fc = interface(vm, "HTMLFormControlsCollection", Some(hc), None);
        dom::register_iface("HTMLFormControlsCollection", fc);
    }
    if let Some(nl) = dom::iface("NodeList") {
        let rl = interface(vm, "RadioNodeList", Some(nl), None);
        dom::register_iface("RadioNodeList", rl);
        attr(vm, rl.proto, "value", radio_value_get, Some(radio_value_set));
    }
    let fh = make_handler(vm);
    FORM_HANDLER.with(|c| c.set(Some(fh)));
    let sh = make_handler(vm);
    SELECT_HANDLER.with(|c| c.set(Some(sh)));
}
