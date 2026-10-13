//! Forms: the control state the engine reads (checkedness, values), the entry list a submission is built
//! from (HTML §4.10.21.4), and its three encodings — `application/x-www-form-urlencoded`,
//! `multipart/form-data` and `text/plain` (§4.10.21.7–9) — byte-for-byte as Chromium sends them
//! (`handlers/aether/tests/forms_oracle.rs` holds the Chromium-captured vectors).

pub mod edit;

use crate::dom::NodeRef;

#[derive(Debug, Clone, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenDocument {
    pub url: String,
    pub method: HttpMethod,
    pub body: Option<String>,
    /// The request body's `Content-Type` (a POST submission's enctype; multipart carries its boundary).
    pub content_type: Option<String>,
}

impl OpenDocument {
    /// A plain GET navigation (a link, a script navigation).
    pub fn get(url: String) -> Self {
        OpenDocument { url, method: HttpMethod::Get, body: None, content_type: None }
    }
}

#[derive(Debug, Clone)]
pub struct FormInput {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub action: String,
    pub method: HttpMethod,
    pub inputs: Vec<FormInput>,
}

impl Form {
    pub fn new(action: String, method: HttpMethod) -> Self {
        Self {
            action,
            method,
            inputs: Vec::new(),
        }
    }

    pub fn add_input(&mut self, name: String, value: String) {
        self.inputs.push(FormInput { name, value });
    }

    pub fn submit(&self) -> OpenDocument {
        let entries: Vec<Entry> =
            self.inputs.iter().map(|i| Entry { name: i.name.clone(), value: EntryValue::Text(i.value.clone()) }).collect();
        let query_string = urlencoded(&entries);
        match self.method {
            HttpMethod::Get => {
                let url = if self.action.contains('?') {
                    format!("{}&{}", self.action, query_string)
                } else if query_string.is_empty() {
                    self.action.clone()
                } else {
                    format!("{}?{}", self.action, query_string)
                };
                OpenDocument::get(url)
            }
            HttpMethod::Post => OpenDocument {
                url: self.action.clone(),
                method: HttpMethod::Post,
                body: if query_string.is_empty() { None } else { Some(query_string) },
                content_type: Some("application/x-www-form-urlencoded".into()),
            },
        }
    }
}

// =================================================================================================
// Control state
// =================================================================================================

fn tag(node: &NodeRef) -> String {
    node.as_element().map(|e| e.name.local.to_string()).unwrap_or_default()
}

fn attr(node: &NodeRef, name: &str) -> Option<String> {
    node.as_element().and_then(|e| e.attributes.borrow().get(name).map(str::to_string))
}

/// An `<input>`'s type keyword, lowercased; unknown and missing types are `text` (HTML §4.10.5).
pub fn input_type(node: &NodeRef) -> String {
    const KNOWN: &[&str] = &[
        "hidden", "text", "search", "tel", "url", "email", "password", "date", "month", "week", "time",
        "datetime-local", "number", "range", "color", "checkbox", "radio", "file", "submit", "image", "reset",
        "button",
    ];
    let t = attr(node, "type").unwrap_or_default().trim().to_ascii_lowercase();
    if KNOWN.contains(&t.as_str()) { t } else { "text".into() }
}

/// A checkbox's or radio's checkedness: the dirty checkedness a click or script set (kept by the page's
/// script state, which toggles it on activation), else the `checked` content attribute.
pub fn checkedness(node: &NodeRef) -> bool {
    crate::js::option_selectedness(node).unwrap_or_else(|| attr(node, "checked").is_some())
}

/// A text control the user can type into: a `<textarea>`, or an `<input>` in a text-entry state —
/// neither disabled nor readonly.
pub fn is_text_editable(node: &NodeRef) -> bool {
    let t = tag(node);
    let typed = match t.as_str() {
        "textarea" => true,
        "input" => matches!(
            input_type(node).as_str(),
            "text" | "search" | "tel" | "url" | "email" | "password" | "number"
        ),
        _ => false,
    };
    typed && attr(node, "readonly").is_none() && !is_disabled(node)
}

/// The control's default value: a textarea's child text (its parser-dropped leading line feed and CRs
/// normalized away), an input's `value` attribute with line breaks stripped (§4.10.5.1.2 sanitization).
pub fn default_value(node: &NodeRef) -> String {
    if tag(node) == "textarea" {
        let t = node.text_contents().replace("\r\n", "\n").replace('\r', "\n");
        return t.strip_prefix('\n').map(str::to_string).unwrap_or(t);
    }
    let v = attr(node, "value").unwrap_or_default();
    if matches!(input_type(node).as_str(), "text" | "search" | "tel" | "url" | "email" | "password") {
        v.chars().filter(|&c| c != '\n' && c != '\r').collect()
    } else {
        v
    }
}

/// The control's current value: the dirty value the user or a script set, else its default value.
pub fn control_value(node: &NodeRef) -> String {
    crate::js::control_value(node).unwrap_or_else(|| default_value(node))
}

/// Disabled: the `disabled` attribute, or a disabled `<fieldset>` ancestor whose first `<legend>` child
/// does not contain the element (HTML §4.10.18.2).
pub fn is_disabled(node: &NodeRef) -> bool {
    let own = matches!(tag(node).as_str(), "button" | "input" | "select" | "textarea" | "option" | "optgroup" | "fieldset");
    if own && attr(node, "disabled").is_some() {
        return true;
    }
    if tag(node) == "option" {
        return node.parent().is_some_and(|p| tag(&p) == "optgroup" && attr(&p, "disabled").is_some());
    }
    let mut child = node.clone();
    let mut cur = node.parent();
    while let Some(p) = cur {
        if tag(&p) == "fieldset" && attr(&p, "disabled").is_some() {
            let first_legend = p.children().find(|c| tag(c) == "legend");
            if first_legend.as_ref() != Some(&child) {
                return true;
            }
        }
        child = p.clone();
        cur = p.parent();
    }
    false
}

/// A submit button: `<button>` (type submit, the default) or `<input type=submit|image>`.
pub fn is_submit_button(node: &NodeRef) -> bool {
    match tag(node).as_str() {
        "button" => {
            let t = attr(node, "type").unwrap_or_default().trim().to_ascii_lowercase();
            !matches!(t.as_str(), "button" | "reset")
        }
        "input" => matches!(input_type(node).as_str(), "submit" | "image"),
        _ => false,
    }
}

/// The control's form owner: the `<form>` its `form` attribute names, else its nearest form ancestor.
pub fn form_owner(node: &NodeRef) -> Option<NodeRef> {
    if let Some(id) = attr(node, "form") {
        let doc = node.document_node();
        return doc.descendants().find(|n| tag(n) == "form" && attr(n, "id").as_deref() == Some(id.as_str()));
    }
    node.ancestors().find(|n| tag(n) == "form")
}

/// The form's submittable elements (`button`, `input`, `select`, `textarea`) whose form owner it is, in
/// tree order.
pub fn form_controls(form: &NodeRef) -> Vec<NodeRef> {
    let doc = form.document_node();
    doc.descendants()
        .filter(|n| matches!(tag(n).as_str(), "button" | "input" | "select" | "textarea"))
        .filter(|n| form_owner(n).as_ref() == Some(form))
        .collect()
}

/// A `<select>`'s list of options (`option` children and `optgroup > option`), with each one's
/// selectedness (§4.10.7): script-set first, else the `selected` attribute; a single-select with none
/// selected selects its first non-disabled option ("ask for a reset").
fn select_options(select: &NodeRef) -> Vec<(NodeRef, bool)> {
    let mut opts: Vec<NodeRef> = Vec::new();
    for c in select.children() {
        match tag(&c).as_str() {
            "option" => opts.push(c),
            "optgroup" => opts.extend(c.children().filter(|g| tag(g) == "option")),
            _ => {}
        }
    }
    let mut sel: Vec<(NodeRef, bool)> = opts
        .into_iter()
        .map(|o| {
            let s = crate::js::option_selectedness(&o).unwrap_or_else(|| attr(&o, "selected").is_some());
            (o, s)
        })
        .collect();
    let multiple = attr(select, "multiple").is_some();
    let size = attr(select, "size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
    if !multiple {
        // At most one selected: the last one set wins.
        if let Some(last) = sel.iter().rposition(|(_, s)| *s) {
            for (i, e) in sel.iter_mut().enumerate() {
                e.1 = i == last;
            }
        } else if size <= 1 {
            if let Some(e) = sel.iter_mut().find(|(o, _)| !is_disabled(o)) {
                e.1 = true;
            }
        }
    }
    sel
}

/// An option's value: its `value` attribute, else its text with ASCII whitespace stripped and collapsed.
fn option_value(o: &NodeRef) -> String {
    attr(o, "value").unwrap_or_else(|| {
        o.text_contents().split([' ', '\t', '\n', '\x0c', '\r']).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
    })
}

// =================================================================================================
// The entry list (HTML §4.10.21.4) and its encodings
// =================================================================================================

/// An entry's value: a string, or a file (Aether has no file picker: a file control submits the empty
/// file, `filename=""`).
#[derive(Debug, Clone, PartialEq)]
pub enum EntryValue {
    Text(String),
    File { filename: String, mime: String, bytes: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub value: EntryValue,
}

fn text_entry(name: impl Into<String>, value: impl Into<String>) -> Entry {
    Entry { name: name.into(), value: EntryValue::Text(value.into()) }
}

/// Constructs the entry list of `form` for a submission by `submitter` (HTML §4.10.21.4). `coords` is
/// where an image button was clicked (its `name.x`/`name.y`).
pub fn construct_entry_list(form: &NodeRef, submitter: Option<&NodeRef>, coords: (i32, i32)) -> Vec<Entry> {
    let mut list = Vec::new();
    for field in form_controls(form) {
        let t = tag(&field);
        if field.ancestors().any(|a| tag(&a) == "datalist") || is_disabled(&field) {
            continue;
        }
        let is_submitter = submitter == Some(&field);
        let ty = if t == "input" { input_type(&field) } else { String::new() };
        if (t == "button" || matches!(ty.as_str(), "submit" | "image" | "reset" | "button"))
            && (!is_submitter || !is_submit_button(&field))
        {
            continue;
        }
        if matches!(ty.as_str(), "checkbox" | "radio") && !checkedness(&field) {
            continue;
        }
        if ty == "image" {
            let name = attr(&field, "name").filter(|n| !n.is_empty());
            let (nx, ny) = match &name {
                Some(n) => (format!("{n}.x"), format!("{n}.y")),
                None => ("x".to_string(), "y".to_string()),
            };
            list.push(text_entry(nx, coords.0.to_string()));
            list.push(text_entry(ny, coords.1.to_string()));
            continue;
        }
        let Some(name) = attr(&field, "name").filter(|n| !n.is_empty()) else { continue };
        if t == "select" {
            for (o, selected) in select_options(&field) {
                if selected && !is_disabled(&o) {
                    list.push(text_entry(name.clone(), option_value(&o)));
                }
            }
        } else if matches!(ty.as_str(), "checkbox" | "radio") {
            list.push(text_entry(name.clone(), attr(&field, "value").unwrap_or_else(|| "on".into())));
        } else if ty == "file" {
            list.push(Entry {
                name: name.clone(),
                value: EntryValue::File { filename: String::new(), mime: "application/octet-stream".into(), bytes: Vec::new() },
            });
        } else if ty == "hidden" && name.eq_ignore_ascii_case("_charset_") {
            list.push(text_entry(name.clone(), "UTF-8"));
        } else if t == "button" {
            list.push(text_entry(name.clone(), attr(&field, "value").unwrap_or_default()));
        } else if ty == "submit" {
            // An input submit button without a value submits its default label (Chromium: "Submit").
            list.push(text_entry(name.clone(), attr(&field, "value").unwrap_or_else(|| "Submit".into())));
        } else {
            list.push(text_entry(name.clone(), control_value(&field)));
        }
        // dirname: the directionality of a text field or textarea (Aether lays text out LTR).
        if t == "textarea" || matches!(ty.as_str(), "text" | "search") {
            if let Some(d) = attr(&field, "dirname").filter(|d| !d.is_empty()) {
                list.push(text_entry(d, "ltr"));
            }
        }
    }
    list
}

/// Every CR not followed by LF, and every LF not preceded by CR, becomes CRLF.
pub fn normalize_crlf(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                out.push_str("\r\n");
                if it.peek() == Some(&'\n') {
                    it.next();
                }
            }
            '\n' => out.push_str("\r\n"),
            c => out.push(c),
        }
    }
    out
}

fn entry_text(e: &Entry) -> String {
    match &e.value {
        EntryValue::Text(v) => normalize_crlf(v),
        EntryValue::File { filename, .. } => filename.clone(),
    }
}

/// `application/x-www-form-urlencoded` (§4.10.21.7 + URL §5.2): names and values CRLF-normalized, then
/// percent-encoded as UTF-8 with space as `+`.
pub fn urlencoded(entries: &[Entry]) -> String {
    let mut out = String::new();
    for e in entries {
        if !out.is_empty() {
            out.push('&');
        }
        out.extend(url::form_urlencoded::byte_serialize(normalize_crlf(&e.name).as_bytes()));
        out.push('=');
        out.extend(url::form_urlencoded::byte_serialize(entry_text(e).as_bytes()));
    }
    out
}

/// `text/plain` (§4.10.21.9): `name=value` lines, CRLF-terminated.
pub fn text_plain(entries: &[Entry]) -> String {
    let mut out = String::new();
    for e in entries {
        out.push_str(&normalize_crlf(&e.name));
        out.push('=');
        out.push_str(&entry_text(e));
        out.push_str("\r\n");
    }
    out
}

/// Escapes a multipart header parameter (`name`, `filename`): LF, CR and `"` percent-encoded.
fn escape_param(s: &str) -> String {
    s.replace('\n', "%0A").replace('\r', "%0D").replace('"', "%22")
}

/// `multipart/form-data` (§4.10.21.8, RFC 7578) with `boundary`, as Chromium's FormDataEncoder writes it.
pub fn multipart(entries: &[Entry], boundary: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for e in entries {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let name = escape_param(&normalize_crlf(&e.name));
        match &e.value {
            EntryValue::Text(v) => {
                out.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes());
                out.extend_from_slice(normalize_crlf(v).as_bytes());
            }
            EntryValue::File { filename, mime, bytes } => {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{}\"\r\nContent-Type: {mime}\r\n\r\n",
                        escape_param(filename)
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// A fresh multipart boundary in Chromium's shape: `----WebKitFormBoundary` + 16 characters of its
/// alphabet.
pub fn new_boundary() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789AB";
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = (nanos ^ SEQ.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed) ^ ((std::process::id() as u64) << 32)) | 1;
    let mut s = String::from("----WebKitFormBoundary");
    for _ in 0..16 {
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        s.push(ALPHA[(r >> 58) as usize] as char);
    }
    s
}

/// The document's base URL: the first `<base href>` resolved against the document URL, else the URL.
fn document_base(doc_url: &str, node: &NodeRef) -> String {
    let doc = node.document_node();
    let base = doc.descendants().find(|n| tag(n) == "base" && attr(n, "href").is_some());
    match base.and_then(|b| attr(&b, "href")) {
        Some(h) => url::Url::parse(doc_url)
            .and_then(|u| u.join(h.trim()))
            .map(|u| u.to_string())
            .unwrap_or_else(|_| doc_url.to_string()),
        None => doc_url.to_string(),
    }
}

/// Builds the navigation a form submission performs (HTML §4.10.21.3 "submit"), for `control` (the
/// field Return was pressed in, or the button clicked) and `submitter` (the submit button, when one
/// submits): the entry list, then [`plan_navigation`]. None when the control has no form owner, the
/// method is `dialog`, or the action's scheme is not submittable here.
pub fn build_form_submission(
    doc_url: &str,
    control: &NodeRef,
    submitter: Option<&NodeRef>,
    coords: (i32, i32),
) -> Option<OpenDocument> {
    let form = form_owner(submitter.unwrap_or(control))?;
    let entries = construct_entry_list(&form, submitter, coords);
    plan_navigation(doc_url, &form, submitter, entries)
}

/// The navigation steps of "submit" (HTML §4.10.21.3, from the action URL on) over an entry list
/// already constructed — by [`construct_entry_list`] for the shell's own submissions, or by the
/// script binding (`js::forms`, which fires `formdata` first) for `submit()` / `requestSubmit()`.
/// Honors the submitter's `formaction`/`formmethod`/`formenctype`; GET replaces the action's query with
/// the urlencoded entry list; POST sends the list as the body in the enctype. None when the method is
/// `dialog` or the action's scheme is not submittable here.
pub fn plan_navigation(doc_url: &str, form: &NodeRef, submitter: Option<&NodeRef>, entries: Vec<Entry>) -> Option<OpenDocument> {
    let form = form.clone();
    let pick = |form_attr: &str, sub_attr: &str| -> Option<String> {
        submitter.and_then(|s| attr(s, sub_attr)).or_else(|| attr(&form, form_attr))
    };
    let action = pick("action", "formaction").unwrap_or_default();
    let method = pick("method", "formmethod").unwrap_or_default().trim().to_ascii_lowercase();
    let enctype = pick("enctype", "formenctype").unwrap_or_default().trim().to_ascii_lowercase();
    if method == "dialog" {
        return None;
    }
    let base = document_base(doc_url, &form);
    let resolved = if action.trim().is_empty() {
        url::Url::parse(doc_url).ok()?
    } else {
        url::Url::parse(&base).and_then(|b| b.join(action.trim())).ok()?
    };
    let post = method == "post";
    let form_method_post = attr(&form, "method").is_some_and(|m| m.trim().eq_ignore_ascii_case("post"));
    match (resolved.scheme(), post) {
        ("http" | "https" | "file" | "data", false) => {
            let mut u = resolved;
            u.set_query(Some(&urlencoded(&entries)));
            Some(OpenDocument::get(u.to_string()))
        }
        ("http" | "https", true) => {
            let (body, ct) = match enctype.as_str() {
                "multipart/form-data" => {
                    let b = new_boundary();
                    (
                        String::from_utf8_lossy(&multipart(&entries, &b)).into_owned(),
                        format!("multipart/form-data; boundary={b}"),
                    )
                }
                // Chromium encodes a non-multipart body by the FORM's own method: a `method=get`
                // form posted by `formmethod=post` sends a text/plain body urlencoded (the header
                // still says text/plain) — reproduced so the bytes match (forms_oracle `formattrs`).
                "text/plain" if !form_method_post => (urlencoded(&entries), "text/plain".to_string()),
                "text/plain" => (text_plain(&entries), "text/plain".to_string()),
                _ => (urlencoded(&entries), "application/x-www-form-urlencoded".to_string()),
            };
            Some(OpenDocument { url: resolved.to_string(), method: HttpMethod::Post, body: Some(body), content_type: Some(ct) })
        }
        _ => None,
    }
}
