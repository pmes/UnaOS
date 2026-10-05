//! HTML's element interfaces (HTML §3.2.8, §4): HTMLElement and the per-element interfaces with their
//! reflected content attributes (§2.6.1), `dataset` (DOMStringMap over `data-*`), form-control values
//! (the dirty value / dirty checkedness flags), anchors' URL decomposition (HTMLHyperlinkElementUtils),
//! `template.content`, `script.text`, and HTMLMediaElement over Aether's `media` registry (play, pause,
//! seeking, muting through Stria on the bus).

use super::dom::{self, attr_value, iface, register_iface, remove_attr, set_attr, this_element, with_doc, wrap};
use super::idl::*;
use super::page;
use html_core::{Namespace, NodeId};
use js_core::vm::*;

/// tag → HTML interface (HTML §4 element index). Hyphenated names are custom elements (HTMLElement).
pub fn interface_for_tag(tag: &str) -> &'static str {
    match tag {
        "a" => "HTMLAnchorElement",
        "area" => "HTMLAreaElement",
        "audio" => "HTMLAudioElement",
        "base" => "HTMLBaseElement",
        "blockquote" | "q" => "HTMLQuoteElement",
        "body" => "HTMLBodyElement",
        "br" => "HTMLBRElement",
        "button" => "HTMLButtonElement",
        "canvas" => "HTMLCanvasElement",
        "caption" => "HTMLTableCaptionElement",
        "col" | "colgroup" => "HTMLTableColElement",
        "data" => "HTMLDataElement",
        "datalist" => "HTMLDataListElement",
        "del" | "ins" => "HTMLModElement",
        "details" => "HTMLDetailsElement",
        "dialog" => "HTMLDialogElement",
        "dir" => "HTMLDirectoryElement",
        "div" => "HTMLDivElement",
        "dl" => "HTMLDListElement",
        "embed" => "HTMLEmbedElement",
        "fieldset" => "HTMLFieldSetElement",
        "font" => "HTMLFontElement",
        "form" => "HTMLFormElement",
        "frame" => "HTMLFrameElement",
        "frameset" => "HTMLFrameSetElement",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "HTMLHeadingElement",
        "head" => "HTMLHeadElement",
        "hr" => "HTMLHRElement",
        "html" => "HTMLHtmlElement",
        "iframe" => "HTMLIFrameElement",
        "img" => "HTMLImageElement",
        "input" => "HTMLInputElement",
        "label" => "HTMLLabelElement",
        "legend" => "HTMLLegendElement",
        "li" => "HTMLLIElement",
        "link" => "HTMLLinkElement",
        "map" => "HTMLMapElement",
        "marquee" => "HTMLMarqueeElement",
        "menu" => "HTMLMenuElement",
        "meta" => "HTMLMetaElement",
        "meter" => "HTMLMeterElement",
        "object" => "HTMLObjectElement",
        "ol" => "HTMLOListElement",
        "optgroup" => "HTMLOptGroupElement",
        "option" => "HTMLOptionElement",
        "output" => "HTMLOutputElement",
        "p" => "HTMLParagraphElement",
        "param" => "HTMLParamElement",
        "picture" => "HTMLPictureElement",
        "pre" | "listing" | "xmp" => "HTMLPreElement",
        "progress" => "HTMLProgressElement",
        "script" => "HTMLScriptElement",
        "search" => "HTMLElement",
        "select" => "HTMLSelectElement",
        "slot" => "HTMLSlotElement",
        "source" => "HTMLSourceElement",
        "span" => "HTMLSpanElement",
        "style" => "HTMLStyleElement",
        "table" => "HTMLTableElement",
        "tbody" | "thead" | "tfoot" => "HTMLTableSectionElement",
        "td" | "th" => "HTMLTableCellElement",
        "template" => "HTMLTemplateElement",
        "textarea" => "HTMLTextAreaElement",
        "time" => "HTMLTimeElement",
        "title" => "HTMLTitleElement",
        "tr" => "HTMLTableRowElement",
        "track" => "HTMLTrackElement",
        "ul" => "HTMLUListElement",
        "video" => "HTMLVideoElement",
        "abbr" | "address" | "article" | "aside" | "b" | "basefont" | "bdi" | "bdo" | "center" | "cite" | "code"
        | "dd" | "dfn" | "dt" | "em" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "i" | "kbd"
        | "main" | "mark" | "nav" | "noscript" | "noembed" | "noframes" | "plaintext" | "rb" | "rp" | "rt" | "rtc"
        | "ruby" | "s" | "samp" | "section" | "small" | "strike" | "strong" | "sub" | "summary" | "sup" | "tt"
        | "u" | "var" | "wbr" | "acronym" | "big" | "nobr" => "HTMLElement",
        other if other.contains('-') => "HTMLElement",
        _ => "HTMLUnknownElement",
    }
}

/// Every HTML element interface, its parent (HTMLElement unless noted).
const ELEMENT_IFACES: &[(&str, &str)] = &[
    ("HTMLAnchorElement", "HTMLElement"),
    ("HTMLAreaElement", "HTMLElement"),
    ("HTMLBaseElement", "HTMLElement"),
    ("HTMLQuoteElement", "HTMLElement"),
    ("HTMLBodyElement", "HTMLElement"),
    ("HTMLBRElement", "HTMLElement"),
    ("HTMLButtonElement", "HTMLElement"),
    ("HTMLCanvasElement", "HTMLElement"),
    ("HTMLTableCaptionElement", "HTMLElement"),
    ("HTMLTableColElement", "HTMLElement"),
    ("HTMLDataElement", "HTMLElement"),
    ("HTMLDataListElement", "HTMLElement"),
    ("HTMLModElement", "HTMLElement"),
    ("HTMLDetailsElement", "HTMLElement"),
    ("HTMLDialogElement", "HTMLElement"),
    ("HTMLDirectoryElement", "HTMLElement"),
    ("HTMLDivElement", "HTMLElement"),
    ("HTMLDListElement", "HTMLElement"),
    ("HTMLEmbedElement", "HTMLElement"),
    ("HTMLFieldSetElement", "HTMLElement"),
    ("HTMLFontElement", "HTMLElement"),
    ("HTMLFormElement", "HTMLElement"),
    ("HTMLFrameElement", "HTMLElement"),
    ("HTMLFrameSetElement", "HTMLElement"),
    ("HTMLHeadingElement", "HTMLElement"),
    ("HTMLHeadElement", "HTMLElement"),
    ("HTMLHRElement", "HTMLElement"),
    ("HTMLHtmlElement", "HTMLElement"),
    ("HTMLIFrameElement", "HTMLElement"),
    ("HTMLImageElement", "HTMLElement"),
    ("HTMLInputElement", "HTMLElement"),
    ("HTMLLabelElement", "HTMLElement"),
    ("HTMLLegendElement", "HTMLElement"),
    ("HTMLLIElement", "HTMLElement"),
    ("HTMLLinkElement", "HTMLElement"),
    ("HTMLMapElement", "HTMLElement"),
    ("HTMLMarqueeElement", "HTMLElement"),
    ("HTMLMenuElement", "HTMLElement"),
    ("HTMLMetaElement", "HTMLElement"),
    ("HTMLMeterElement", "HTMLElement"),
    ("HTMLObjectElement", "HTMLElement"),
    ("HTMLOListElement", "HTMLElement"),
    ("HTMLOptGroupElement", "HTMLElement"),
    ("HTMLOptionElement", "HTMLElement"),
    ("HTMLOutputElement", "HTMLElement"),
    ("HTMLParagraphElement", "HTMLElement"),
    ("HTMLParamElement", "HTMLElement"),
    ("HTMLPictureElement", "HTMLElement"),
    ("HTMLPreElement", "HTMLElement"),
    ("HTMLProgressElement", "HTMLElement"),
    ("HTMLScriptElement", "HTMLElement"),
    ("HTMLSelectElement", "HTMLElement"),
    ("HTMLSlotElement", "HTMLElement"),
    ("HTMLSourceElement", "HTMLElement"),
    ("HTMLSpanElement", "HTMLElement"),
    ("HTMLStyleElement", "HTMLElement"),
    ("HTMLTableElement", "HTMLElement"),
    ("HTMLTableSectionElement", "HTMLElement"),
    ("HTMLTableCellElement", "HTMLElement"),
    ("HTMLTemplateElement", "HTMLElement"),
    ("HTMLTextAreaElement", "HTMLElement"),
    ("HTMLTimeElement", "HTMLElement"),
    ("HTMLTitleElement", "HTMLElement"),
    ("HTMLTableRowElement", "HTMLElement"),
    ("HTMLTrackElement", "HTMLElement"),
    ("HTMLUListElement", "HTMLElement"),
    ("HTMLUnknownElement", "HTMLElement"),
    ("HTMLMediaElement", "HTMLElement"),
    ("HTMLAudioElement", "HTMLMediaElement"),
    ("HTMLVideoElement", "HTMLMediaElement"),
];

/// Reflected attributes (HTML §2.6.1): (interface, IDL name, content attribute, kind). Kinds: `s`
/// DOMString, `b` boolean, `u` URL (USVString reflecting a URL), `l` long (default 0), `n` non-negative
/// long (default -1 → absent), `p` unsigned long (default 0), `1` unsigned long default 1 (colSpan,
/// rowSpan, size), `e` enumerated lowercase (limited) with default after `:` in the attribute field.
const REFLECT: &[(&str, &str, &str, char)] = &[
    ("HTMLElement", "title", "title", 's'),
    ("HTMLElement", "lang", "lang", 's'),
    ("HTMLElement", "dir", "dir", 'e'),
    ("HTMLElement", "hidden", "hidden", 'b'),
    ("HTMLElement", "inert", "inert", 'b'),
    ("HTMLElement", "accessKey", "accesskey", 's'),
    ("HTMLElement", "autofocus", "autofocus", 'b'),
    ("HTMLElement", "nonce", "nonce", 's'),
    ("HTMLAnchorElement", "target", "target", 's'),
    ("HTMLAnchorElement", "download", "download", 's'),
    ("HTMLAnchorElement", "ping", "ping", 's'),
    ("HTMLAnchorElement", "rel", "rel", 's'),
    ("HTMLAnchorElement", "hreflang", "hreflang", 's'),
    ("HTMLAnchorElement", "type", "type", 's'),
    ("HTMLAnchorElement", "name", "name", 's'),
    ("HTMLAreaElement", "alt", "alt", 's'),
    ("HTMLAreaElement", "coords", "coords", 's'),
    ("HTMLAreaElement", "shape", "shape", 's'),
    ("HTMLAreaElement", "target", "target", 's'),
    ("HTMLAreaElement", "rel", "rel", 's'),
    ("HTMLBaseElement", "target", "target", 's'),
    ("HTMLQuoteElement", "cite", "cite", 'u'),
    ("HTMLModElement", "cite", "cite", 'u'),
    ("HTMLModElement", "dateTime", "datetime", 's'),
    ("HTMLButtonElement", "disabled", "disabled", 'b'),
    ("HTMLButtonElement", "name", "name", 's'),
    ("HTMLButtonElement", "value", "value", 's'),
    ("HTMLButtonElement", "formAction", "formaction", 'u'),
    ("HTMLButtonElement", "formNoValidate", "formnovalidate", 'b'),
    ("HTMLButtonElement", "formTarget", "formtarget", 's'),
    ("HTMLCanvasElement", "width", "width", 'p'),
    ("HTMLCanvasElement", "height", "height", 'p'),
    ("HTMLTableColElement", "span", "span", 'C'),
    ("HTMLDataElement", "value", "value", 's'),
    ("HTMLDetailsElement", "open", "open", 'b'),
    ("HTMLDetailsElement", "name", "name", 's'),
    ("HTMLDialogElement", "open", "open", 'b'),
    ("HTMLEmbedElement", "src", "src", 'u'),
    ("HTMLEmbedElement", "type", "type", 's'),
    ("HTMLEmbedElement", "width", "width", 's'),
    ("HTMLEmbedElement", "height", "height", 's'),
    ("HTMLFieldSetElement", "disabled", "disabled", 'b'),
    ("HTMLFieldSetElement", "name", "name", 's'),
    ("HTMLFormElement", "acceptCharset", "accept-charset", 's'),
    ("HTMLFormElement", "action", "action", 'u'),
    ("HTMLFormElement", "autocomplete", "autocomplete", 's'),
    ("HTMLFormElement", "name", "name", 's'),
    ("HTMLFormElement", "noValidate", "novalidate", 'b'),
    ("HTMLFormElement", "target", "target", 's'),
    ("HTMLFormElement", "rel", "rel", 's'),
    ("HTMLHtmlElement", "version", "version", 's'),
    ("HTMLIFrameElement", "src", "src", 'u'),
    ("HTMLIFrameElement", "srcdoc", "srcdoc", 's'),
    ("HTMLIFrameElement", "name", "name", 's'),
    ("HTMLIFrameElement", "allow", "allow", 's'),
    ("HTMLIFrameElement", "allowFullscreen", "allowfullscreen", 'b'),
    ("HTMLIFrameElement", "width", "width", 's'),
    ("HTMLIFrameElement", "height", "height", 's'),
    ("HTMLImageElement", "alt", "alt", 's'),
    ("HTMLImageElement", "src", "src", 'u'),
    ("HTMLImageElement", "srcset", "srcset", 's'),
    ("HTMLImageElement", "sizes", "sizes", 's'),
    ("HTMLImageElement", "useMap", "usemap", 's'),
    ("HTMLImageElement", "isMap", "ismap", 'b'),
    ("HTMLImageElement", "width", "width", 'p'),
    ("HTMLImageElement", "height", "height", 'p'),
    ("HTMLImageElement", "name", "name", 's'),
    ("HTMLImageElement", "align", "align", 's'),
    ("HTMLImageElement", "border", "border", 's'),
    ("HTMLImageElement", "longDesc", "longdesc", 'u'),
    ("HTMLImageElement", "loading", "loading", 's'),
    ("HTMLImageElement", "decoding", "decoding", 's'),
    ("HTMLInputElement", "accept", "accept", 's'),
    ("HTMLInputElement", "alt", "alt", 's'),
    ("HTMLInputElement", "autocomplete", "autocomplete", 's'),
    ("HTMLInputElement", "defaultChecked", "checked", 'b'),
    ("HTMLInputElement", "dirName", "dirname", 's'),
    ("HTMLInputElement", "disabled", "disabled", 'b'),
    ("HTMLInputElement", "formAction", "formaction", 'u'),
    ("HTMLInputElement", "formNoValidate", "formnovalidate", 'b'),
    ("HTMLInputElement", "formTarget", "formtarget", 's'),
    ("HTMLInputElement", "max", "max", 's'),
    ("HTMLInputElement", "maxLength", "maxlength", 'n'),
    ("HTMLInputElement", "min", "min", 's'),
    ("HTMLInputElement", "minLength", "minlength", 'n'),
    ("HTMLInputElement", "multiple", "multiple", 'b'),
    ("HTMLInputElement", "name", "name", 's'),
    ("HTMLInputElement", "pattern", "pattern", 's'),
    ("HTMLInputElement", "placeholder", "placeholder", 's'),
    ("HTMLInputElement", "readOnly", "readonly", 'b'),
    ("HTMLInputElement", "required", "required", 'b'),
    ("HTMLInputElement", "size", "size", '1'),
    ("HTMLInputElement", "src", "src", 'u'),
    ("HTMLInputElement", "step", "step", 's'),
    ("HTMLInputElement", "defaultValue", "value", 's'),
    ("HTMLLabelElement", "htmlFor", "for", 's'),
    ("HTMLLIElement", "value", "value", 'l'),
    ("HTMLLinkElement", "href", "href", 'u'),
    ("HTMLLinkElement", "crossOrigin", "crossorigin", 's'),
    ("HTMLLinkElement", "rel", "rel", 's'),
    ("HTMLLinkElement", "media", "media", 's'),
    ("HTMLLinkElement", "hreflang", "hreflang", 's'),
    ("HTMLLinkElement", "type", "type", 's'),
    ("HTMLLinkElement", "as", "as", 's'),
    ("HTMLLinkElement", "integrity", "integrity", 's'),
    ("HTMLLinkElement", "disabled", "disabled", 'b'),
    ("HTMLMapElement", "name", "name", 's'),
    ("HTMLMetaElement", "name", "name", 's'),
    ("HTMLMetaElement", "httpEquiv", "http-equiv", 's'),
    ("HTMLMetaElement", "content", "content", 's'),
    ("HTMLMetaElement", "media", "media", 's'),
    ("HTMLMeterElement", "value", "value", 's'),
    ("HTMLObjectElement", "data", "data", 'u'),
    ("HTMLObjectElement", "type", "type", 's'),
    ("HTMLObjectElement", "name", "name", 's'),
    ("HTMLObjectElement", "width", "width", 's'),
    ("HTMLObjectElement", "height", "height", 's'),
    ("HTMLOListElement", "reversed", "reversed", 'b'),
    ("HTMLOListElement", "start", "start", 'l'),
    ("HTMLOListElement", "type", "type", 's'),
    ("HTMLOptGroupElement", "disabled", "disabled", 'b'),
    ("HTMLOptGroupElement", "label", "label", 's'),
    ("HTMLOptionElement", "disabled", "disabled", 'b'),
    ("HTMLOptionElement", "defaultSelected", "selected", 'b'),
    ("HTMLOutputElement", "name", "name", 's'),
    ("HTMLScriptElement", "src", "src", 'u'),
    ("HTMLScriptElement", "type", "type", 's'),
    ("HTMLScriptElement", "noModule", "nomodule", 'b'),
    ("HTMLScriptElement", "defer", "defer", 'b'),
    ("HTMLScriptElement", "crossOrigin", "crossorigin", 's'),
    ("HTMLScriptElement", "integrity", "integrity", 's'),
    ("HTMLScriptElement", "charset", "charset", 's'),
    ("HTMLScriptElement", "event", "event", 's'),
    ("HTMLScriptElement", "htmlFor", "for", 's'),
    ("HTMLSelectElement", "autocomplete", "autocomplete", 's'),
    ("HTMLSelectElement", "disabled", "disabled", 'b'),
    ("HTMLSelectElement", "multiple", "multiple", 'b'),
    ("HTMLSelectElement", "name", "name", 's'),
    ("HTMLSelectElement", "required", "required", 'b'),
    ("HTMLSlotElement", "name", "name", 's'),
    ("HTMLSourceElement", "src", "src", 'u'),
    ("HTMLSourceElement", "type", "type", 's'),
    ("HTMLSourceElement", "srcset", "srcset", 's'),
    ("HTMLSourceElement", "sizes", "sizes", 's'),
    ("HTMLSourceElement", "media", "media", 's'),
    ("HTMLStyleElement", "media", "media", 's'),
    ("HTMLStyleElement", "disabled", "disabled", 'b'),
    ("HTMLTableCellElement", "colSpan", "colspan", 'C'),
    ("HTMLTableCellElement", "rowSpan", "rowspan", 'R'),
    ("HTMLTableCellElement", "headers", "headers", 's'),
    ("HTMLTableCellElement", "abbr", "abbr", 's'),
    ("HTMLTableCellElement", "scope", "scope", 's'),
    ("HTMLTextAreaElement", "autocomplete", "autocomplete", 's'),
    ("HTMLTextAreaElement", "cols", "cols", '1'),
    ("HTMLTextAreaElement", "dirName", "dirname", 's'),
    ("HTMLTextAreaElement", "disabled", "disabled", 'b'),
    ("HTMLTextAreaElement", "maxLength", "maxlength", 'n'),
    ("HTMLTextAreaElement", "minLength", "minlength", 'n'),
    ("HTMLTextAreaElement", "name", "name", 's'),
    ("HTMLTextAreaElement", "placeholder", "placeholder", 's'),
    ("HTMLTextAreaElement", "readOnly", "readonly", 'b'),
    ("HTMLTextAreaElement", "required", "required", 'b'),
    ("HTMLTextAreaElement", "rows", "rows", '1'),
    ("HTMLTextAreaElement", "wrap", "wrap", 's'),
    ("HTMLTimeElement", "dateTime", "datetime", 's'),
    ("HTMLTrackElement", "kind", "kind", 's'),
    ("HTMLTrackElement", "src", "src", 'u'),
    ("HTMLTrackElement", "srclang", "srclang", 's'),
    ("HTMLTrackElement", "label", "label", 's'),
    ("HTMLTrackElement", "default", "default", 'b'),
    ("HTMLMediaElement", "src", "src", 'u'),
    ("HTMLMediaElement", "crossOrigin", "crossorigin", 's'),
    ("HTMLMediaElement", "preload", "preload", 's'),
    ("HTMLMediaElement", "autoplay", "autoplay", 'b'),
    ("HTMLMediaElement", "loop", "loop", 'b'),
    ("HTMLMediaElement", "controls", "controls", 'b'),
    ("HTMLMediaElement", "defaultMuted", "muted", 'b'),
    ("HTMLVideoElement", "width", "width", 'p'),
    ("HTMLVideoElement", "height", "height", 'p'),
    ("HTMLVideoElement", "poster", "poster", 'u'),
    ("HTMLVideoElement", "playsInline", "playsinline", 'b'),
    ("HTMLProgressElement", "value", "value", 's'),
    ("HTMLHeadingElement", "align", "align", 's'),
    ("HTMLParagraphElement", "align", "align", 's'),
    ("HTMLDivElement", "align", "align", 's'),
    ("HTMLTableElement", "border", "border", 's'),
    ("HTMLTableElement", "width", "width", 's'),
    ("HTMLTableElement", "cellPadding", "cellpadding", 's'),
    ("HTMLTableElement", "cellSpacing", "cellspacing", 's'),
    ("HTMLBodyElement", "bgColor", "bgcolor", 's'),
    ("HTMLBodyElement", "text", "text", 's'),
    ("HTMLBodyElement", "link", "link", 's'),
    ("HTMLFontElement", "color", "color", 's'),
    ("HTMLFontElement", "face", "face", 's'),
    ("HTMLFontElement", "size", "size", 's'),
    ("HTMLHRElement", "align", "align", 's'),
    ("HTMLHRElement", "color", "color", 's'),
    ("HTMLHRElement", "noShade", "noshade", 'b'),
    ("HTMLHRElement", "size", "size", 's'),
    ("HTMLHRElement", "width", "width", 's'),
    ("HTMLPreElement", "width", "width", 'l'),
    ("HTMLBRElement", "clear", "clear", 's'),
    ("HTMLUListElement", "type", "type", 's'),
    ("HTMLUListElement", "compact", "compact", 'b'),
    ("HTMLMarqueeElement", "behavior", "behavior", 's'),
    ("HTMLMarqueeElement", "direction", "direction", 's'),
    ("HTMLParamElement", "name", "name", 's'),
    ("HTMLParamElement", "value", "value", 's'),
];

fn reflect_spec(vm: &Vm, ctx: &CallCtx) -> (String, char) {
    let d = callee_str(vm, ctx);
    let (a, k) = d.split_once('|').unwrap_or((&d, "s"));
    (a.to_string(), k.chars().next().unwrap_or('s'))
}

/// "rules for parsing integers" (HTML §2.3.4.1).
pub fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start_matches([' ', '\t', '\n', '\x0C', '\r']);
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let v: i64 = digits.parse().unwrap_or(i64::MAX);
    Some(if neg { -v } else { v })
}

fn reflect_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let (a, k) = reflect_spec(vm, ctx);
    let v = attr_value(el, &a);
    Ok(match k {
        'b' => Value::Bool(v.is_some()),
        'u' => match v {
            Some(x) => s(&dom::resolve_url(dom::node_document(el), &x).unwrap_or(x)),
            None => s(""),
        },
        'l' => num(v.and_then(|x| parse_int(&x)).filter(|n| *n >= i32::MIN as i64 && *n <= i32::MAX as i64).unwrap_or(0) as f64),
        'n' => num(v.and_then(|x| parse_int(&x)).filter(|n| *n >= 0 && *n <= i32::MAX as i64).unwrap_or(-1) as f64),
        'p' => num(v.and_then(|x| parse_int(&x)).filter(|n| *n >= 0 && *n <= i32::MAX as i64).unwrap_or(0) as f64),
        '1' => num(v.and_then(|x| parse_int(&x)).filter(|n| *n > 0 && *n <= i32::MAX as i64).unwrap_or(1) as f64),
        // clamped to [1, 1000] (colSpan, col span) / [0, 65534] (rowSpan), HTML §4.9.11
        'C' => num(v.and_then(|x| parse_int(&x)).map(|n| n.clamp(1, 1000)).unwrap_or(1) as f64),
        'R' => num(v.and_then(|x| parse_int(&x)).map(|n| n.clamp(0, 65534)).unwrap_or(1) as f64),
        'e' => {
            // `dir`: limited to ltr/rtl/auto.
            let x = v.unwrap_or_default().to_ascii_lowercase();
            s(if matches!(x.as_str(), "ltr" | "rtl" | "auto") { &x } else { "" })
        }
        _ => s(&v.unwrap_or_default()),
    })
}

fn reflect_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let (a, k) = reflect_spec(vm, ctx);
    let v = arg(vm, ctx, 0);
    match k {
        'b' => {
            if vm.to_boolean(&v) {
                set_attr(vm, el, &a, "");
            } else {
                remove_attr(vm, el, &a);
            }
        }
        'l' => {
            let n = vm.to_int32(&v)?;
            set_attr(vm, el, &a, &n.to_string());
        }
        'n' => {
            let n = vm.to_int32(&v)?;
            if n < 0 {
                return throw_dom(vm, "IndexSizeError", "The value provided is negative.");
            }
            set_attr(vm, el, &a, &n.to_string());
        }
        'p' | 'C' | 'R' => {
            let n = vm.to_uint32(&v)?;
            let n = if n > i32::MAX as u32 { 0 } else { n };
            set_attr(vm, el, &a, &n.to_string());
        }
        '1' => {
            // "limited to only positive numbers": 0 throws (HTML §2.6.1).
            let n = vm.to_uint32(&v)?;
            if n == 0 {
                return throw_dom(vm, "IndexSizeError", "The value provided is 0, which is an invalid size.");
            }
            let n = if n > i32::MAX as u32 { 1 } else { n };
            set_attr(vm, el, &a, &n.to_string());
        }
        _ => {
            let x = string(vm, &v)?;
            set_attr(vm, el, &a, &x);
        }
    }
    Ok(Value::Undefined)
}

// =================================================================================================
// HTMLElement
// =================================================================================================

fn this_html(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    this_element(vm, ctx)
}

fn tag_of_el(el: usize) -> String {
    with_doc(|d| dom::local_name_in(d, el))
}

fn inner_text_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    crate::ledger::record_dom("innerText-approximated-from-markup");
    Ok(s(&inner_text_of(el)))
}

/// innerText (HTML §3.2.7) approximated from markup rather than from the rendering: `<br>` is a line
/// break, block-level elements are line boundaries (`<p>` two), `script`/`style`/`head`/hidden content
/// contributes nothing, whitespace collapses like `white-space: normal`.
pub fn inner_text_of(el: usize) -> String {
    const BLOCK: &[&str] = &[
        "address", "article", "aside", "blockquote", "details", "dialog", "dd", "div", "dl", "dt", "fieldset",
        "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hgroup", "hr", "li",
        "main", "nav", "ol", "pre", "section", "table", "ul", "tr", "caption", "summary",
    ];
    fn walk(d: &html_core::Document, n: NodeId, out: &mut Vec<(String, u8)>, pre: bool) {
        for c in d.children(n) {
            match d.data(c) {
                html_core::NodeData::Text(t) => {
                    if pre {
                        out.push((t.clone(), 0));
                    } else {
                        let mut buf = String::new();
                        let mut ws = false;
                        for ch in t.chars() {
                            if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0C') {
                                ws = true;
                            } else {
                                if ws {
                                    buf.push(' ');
                                }
                                ws = false;
                                buf.push(ch);
                            }
                        }
                        if ws {
                            buf.push(' ');
                        }
                        out.push((buf, 0));
                    }
                }
                html_core::NodeData::Element(e) => {
                    let tag = e.local.as_str();
                    if matches!(tag, "script" | "style" | "head" | "template" | "noscript" | "title") || e.attr("hidden").is_some() {
                        continue;
                    }
                    if tag == "br" {
                        out.push(("\n".into(), 0));
                        continue;
                    }
                    let req = if tag == "p" { 2 } else if BLOCK.contains(&tag) { 1 } else { 0 };
                    if req > 0 {
                        out.push((String::new(), req));
                    }
                    if tag == "td" || tag == "th" {
                        if d.prev_sibling_element(c).is_some() {
                            out.push(("\t".into(), 0));
                        }
                    }
                    walk(d, c, out, pre || tag == "pre" || tag == "textarea");
                    if req > 0 {
                        out.push((String::new(), req));
                    }
                }
                _ => {}
            }
        }
    }
    with_doc(|d| {
        let mut items: Vec<(String, u8)> = Vec::new();
        walk(d, NodeId(el), &mut items, false);
        // Collapse: spaces at line starts/ends removed, required line breaks merged (max wins).
        let mut out = String::new();
        let mut pending: u8 = 0;
        for (t, req) in items {
            if req > 0 {
                pending = pending.max(req);
                continue;
            }
            if t.is_empty() {
                continue;
            }
            let mut t = t;
            if pending > 0 && !out.is_empty() {
                while out.ends_with(' ') {
                    out.pop();
                }
                for _ in 0..pending {
                    out.push('\n');
                }
            }
            if pending > 0 || out.is_empty() || out.ends_with('\n') {
                t = t.trim_start_matches(' ').to_string();
            }
            if out.ends_with(' ') && t.starts_with(' ') {
                t = t[1..].to_string();
            }
            pending = 0;
            out.push_str(&t);
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out
    })
}

fn inner_text_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let t = string_null_empty(vm, &arg(vm, ctx, 0))?;
    // HTML "rendered text fragment": line breaks become <br>.
    let doc = dom::node_document(el);
    let frag = dom::with_doc_mut(|d| d.create(html_core::NodeData::DocumentFragment).0);
    let mut first = true;
    for line in t.split('\n') {
        if !first {
            let br = dom::create_element(doc, Namespace::Html, "br");
            dom::with_doc_mut(|d| d.append(NodeId(frag), NodeId(br)));
        }
        first = false;
        if !line.is_empty() {
            let tx = dom::create_text(doc, line.trim_end_matches('\r').to_string());
            dom::with_doc_mut(|d| d.append(NodeId(frag), NodeId(tx)));
        }
    }
    dom::replace_all(vm, Some(frag), el);
    Ok(Value::Undefined)
}

fn click_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let disabled = with_doc(|d| {
        d.element(NodeId(el)).is_some_and(|e| matches!(e.local.as_str(), "button" | "input" | "select" | "textarea") && e.attr("disabled").is_some())
    });
    if !disabled {
        super::events::synthetic_click(vm, el);
    }
    Ok(Value::Undefined)
}

fn focus_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    if !dom::is_connected(el) {
        return Ok(Value::Undefined);
    }
    let old = page(|p| p.focused);
    if old == Some(el) {
        return Ok(Value::Undefined);
    }
    if let Some(o) = old {
        page(|p| p.focused = None);
        super::events::fire_simple(vm, super::TargetKey::Node(o), "blur", false, false);
        super::events::fire_simple(vm, super::TargetKey::Node(o), "focusout", true, false);
    }
    page(|p| p.focused = Some(el));
    super::events::fire_simple(vm, super::TargetKey::Node(el), "focus", false, false);
    super::events::fire_simple(vm, super::TargetKey::Node(el), "focusin", true, false);
    Ok(Value::Undefined)
}

fn blur_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    if page(|p| p.focused) == Some(el) {
        page(|p| p.focused = None);
        super::events::fire_simple(vm, super::TargetKey::Node(el), "blur", false, false);
        super::events::fire_simple(vm, super::TargetKey::Node(el), "focusout", true, false);
    }
    Ok(Value::Undefined)
}

fn tab_index_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let v = attr_value(el, "tabindex").and_then(|x| parse_int(&x));
    let focusable = matches!(tag_of_el(el).as_str(), "a" | "button" | "input" | "select" | "textarea" | "iframe" | "summary");
    Ok(num(v.unwrap_or(if focusable { 0 } else { -1 }) as f64))
}

fn tab_index_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let n = vm.to_int32(&arg(vm, ctx, 0))?;
    set_attr(vm, el, "tabindex", &n.to_string());
    Ok(Value::Undefined)
}

fn content_editable_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    Ok(s(match attr_value(el, "contenteditable").map(|v| v.to_ascii_lowercase()) {
        Some(v) if v.is_empty() || v == "true" => "true",
        Some(v) if v == "false" => "false",
        Some(v) if v == "plaintext-only" => "plaintext-only",
        _ => "inherit",
    }))
}

fn content_editable_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?.to_ascii_lowercase();
    match v.as_str() {
        "inherit" => remove_attr(vm, el, "contenteditable"),
        "true" | "false" | "plaintext-only" => set_attr(vm, el, "contenteditable", &v),
        _ => return throw_dom(vm, "SyntaxError", "The value provided is not one of 'true', 'false', 'plaintext-only', or 'inherit'."),
    }
    Ok(Value::Undefined)
}

fn is_content_editable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let mut cur = Some(el);
    while let Some(c) = cur {
        match attr_value(c, "contenteditable").map(|v| v.to_ascii_lowercase()) {
            Some(v) if v.is_empty() || v == "true" || v == "plaintext-only" => return Ok(Value::Bool(true)),
            Some(v) if v == "false" => return Ok(Value::Bool(false)),
            _ => {}
        }
        cur = with_doc(|d| d.parent_element(NodeId(c)).map(|p| p.0));
    }
    Ok(Value::Bool(false))
}

fn draggable_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    Ok(Value::Bool(match attr_value(el, "draggable").map(|v| v.to_ascii_lowercase()).as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => tag_of_el(el) == "img" || (tag_of_el(el) == "a" && attr_value(el, "href").is_some()),
    }))
}

fn draggable_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    set_attr(vm, el, "draggable", if b { "true" } else { "false" });
    Ok(Value::Undefined)
}

fn spellcheck_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    Ok(Value::Bool(attr_value(el, "spellcheck").map(|v| v.to_ascii_lowercase()) != Some("false".into())))
}

fn spellcheck_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    set_attr(vm, el, "spellcheck", if b { "true" } else { "false" });
    Ok(Value::Undefined)
}

fn translate_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let mut cur = Some(el);
    while let Some(c) = cur {
        match attr_value(c, "translate").map(|v| v.to_ascii_lowercase()).as_deref() {
            Some("") | Some("yes") => return Ok(Value::Bool(true)),
            Some("no") => return Ok(Value::Bool(false)),
            _ => {}
        }
        cur = with_doc(|d| d.parent_element(NodeId(c)).map(|p| p.0));
    }
    Ok(Value::Bool(true))
}

fn translate_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    set_attr(vm, el, "translate", if b { "yes" } else { "no" });
    Ok(Value::Undefined)
}

// ---- dataset (DOMStringMap, HTML §3.2.6.6) as a proxy over data-* attributes

thread_local! {
    static DATASET_HANDLER: std::cell::Cell<Option<Obj>> = const { std::cell::Cell::new(None) };
}

/// `data-*` attribute name for a dataset property name; None for the shape the spec rejects.
pub fn dataset_attr_name(prop: &str) -> Option<String> {
    let b = prop.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_lowercase) {
            return None;
        }
    }
    let mut out = String::from("data-");
    for c in prop.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// The dataset property name of a `data-*` attribute.
pub fn dataset_prop_name(attr: &str) -> Option<String> {
    let rest = attr.strip_prefix("data-")?;
    let mut out = String::new();
    let mut it = rest.chars().peekable();
    while let Some(c) = it.next() {
        if c == '-' {
            if let Some(n) = it.peek().copied() {
                if n.is_ascii_lowercase() {
                    it.next();
                    out.push(n.to_ascii_uppercase());
                    continue;
                }
            }
        }
        out.push(c);
    }
    Some(out)
}

fn dataset_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_html(vm, ctx)?;
    Ok(dom::cached(vm, el, dom::SO_DATASET, |vm| {
        let p = iface("DOMStringMap").unwrap().proto;
        let t = host_obj(vm, p, T_STRINGMAP, vec![num(el as f64)]);
        let handler = DATASET_HANDLER.with(|h| h.get()).unwrap();
        vm.alloc(ObjectData::new(None, Kind::Proxy(Some(Box::new(ProxyData { target: t, handler, callable: false, ctor: false, revoked: false })))))
    }))
}

fn ds_target(vm: &Vm, ctx: &CallCtx) -> Option<(Obj, usize)> {
    let t = vm.arg(ctx, 0).as_object()?;
    (tag_of(vm, t) == Some(T_STRINGMAP)).then(|| (t, slot_num(vm, t, 0) as usize))
}

fn ds_key(vm: &mut Vm, ctx: &CallCtx) -> JsResult<PropertyKey> {
    let k = vm.arg(ctx, 1);
    vm.to_property_key(&k)
}

fn ds_name(k: &PropertyKey) -> Option<String> {
    match k {
        PropertyKey::Str(s) => Some(s.to_rust()),
        PropertyKey::Index(i) => Some(i.to_string()),
        _ => None,
    }
}

fn ds_value(el: usize, name: &str) -> Option<String> {
    let a = dataset_attr_name(name)?;
    attr_value(el, &a)
}

fn ds_trap_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = ds_target(vm, ctx) else { return Ok(Value::Undefined) };
    let k = ds_key(vm, ctx)?;
    if let Some(n) = ds_name(&k) {
        if let Some(v) = ds_value(el, &n) {
            return Ok(s(&v));
        }
    }
    let recv = vm.arg(ctx, 2);
    vm.get_with_receiver(t, &k, &recv)
}

fn ds_trap_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = ds_target(vm, ctx) else { return Ok(Value::Bool(false)) };
    let k = ds_key(vm, ctx)?;
    let v = vm.arg(ctx, 2);
    match ds_name(&k) {
        Some(n) => {
            let Some(a) = dataset_attr_name(&n) else {
                return throw_dom(vm, "SyntaxError", &format!("'{n}' is not a valid property name."));
            };
            let val = string(vm, &v)?;
            set_attr(vm, el, &a, &val);
            Ok(Value::Bool(true))
        }
        None => {
            let r = vm.set(t, k, v, &Value::Object(t))?;
            Ok(Value::Bool(r))
        }
    }
}

fn ds_trap_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = ds_target(vm, ctx) else { return Ok(Value::Bool(false)) };
    let k = ds_key(vm, ctx)?;
    if let Some(n) = ds_name(&k) {
        if ds_value(el, &n).is_some() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(vm.has_property(t, &k)?))
}

fn ds_trap_delete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = ds_target(vm, ctx) else { return Ok(Value::Bool(true)) };
    let k = ds_key(vm, ctx)?;
    if let Some(n) = ds_name(&k) {
        if let Some(a) = dataset_attr_name(&n) {
            if attr_value(el, &a).is_some() {
                remove_attr(vm, el, &a);
                return Ok(Value::Bool(true));
            }
        }
    }
    Ok(Value::Bool(vm.delete(t, &k)?))
}

fn ds_trap_own_keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = ds_target(vm, ctx) else { return Ok(Value::Object(vm.new_array(Vec::new()))) };
    let mut keys: Vec<Value> = with_doc(|d| {
        d.element(NodeId(el))
            .map(|e| {
                e.attrs
                    .iter()
                    .filter(|a| a.ns == Namespace::None)
                    .filter_map(|a| dataset_prop_name(&a.local))
                    .map(|n| s(&n))
                    .collect()
            })
            .unwrap_or_default()
    });
    for k in vm.ordinary_own_keys(t) {
        keys.push(k.to_value());
    }
    Ok(Value::Object(vm.new_array(keys)))
}

fn ds_trap_gopd(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = ds_target(vm, ctx) else { return Ok(Value::Undefined) };
    let k = ds_key(vm, ctx)?;
    if let Some(n) = ds_name(&k) {
        if let Some(v) = ds_value(el, &n) {
            let o = vm.new_plain_object();
            vm.create_data_property(o, PropertyKey::from_str("value"), s(&v))?;
            vm.create_data_property(o, PropertyKey::from_str("writable"), Value::Bool(true))?;
            vm.create_data_property(o, PropertyKey::from_str("enumerable"), Value::Bool(true))?;
            vm.create_data_property(o, PropertyKey::from_str("configurable"), Value::Bool(true))?;
            return Ok(Value::Object(o));
        }
    }
    match vm.get_own_property(t, &k)? {
        Some(d) => Ok(vm.from_property_descriptor(&d)),
        None => Ok(Value::Undefined),
    }
}

fn ds_trap_define(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some((t, el)) = ds_target(vm, ctx) else { return Ok(Value::Bool(false)) };
    let k = ds_key(vm, ctx)?;
    let dv = vm.arg(ctx, 2);
    let d = vm.to_property_descriptor(&dv)?;
    if let Some(n) = ds_name(&k) {
        if let Some(v) = d.value.clone() {
            let Some(a) = dataset_attr_name(&n) else {
                return throw_dom(vm, "SyntaxError", &format!("'{n}' is not a valid property name."));
            };
            let val = string(vm, &v)?;
            set_attr(vm, el, &a, &val);
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(vm.define_own_property(t, k, d)?))
}

// ---- form controls

/// The checkable kind of an `<input>` ("checkbox" / "radio").
pub fn checkable_type(n: usize) -> Option<&'static str> {
    with_doc(|d| {
        let e = d.element(NodeId(n))?;
        if e.ns != Namespace::Html || e.local != "input" {
            return None;
        }
        match e.attr("type").map(|t| t.to_ascii_lowercase()).as_deref() {
            Some("checkbox") => Some("checkbox"),
            Some("radio") => Some("radio"),
            _ => None,
        }
    })
}

pub fn checkedness(n: usize) -> bool {
    page(|p| p.checked.get(&n).copied()).unwrap_or_else(|| attr_value(n, "checked").is_some())
}

/// Sets checkedness (dirty) — and for a radio, unchecks the others of its group (HTML §4.10.5.1.16).
pub fn set_checkedness(_vm: &mut Vm, n: usize, v: bool) {
    page(|p| p.checked.insert(n, v));
    if v && checkable_type(n) == Some("radio") {
        if let Some(name) = attr_value(n, "name").filter(|x| !x.is_empty()) {
            let root = dom::root_of(n);
            let others: Vec<usize> = with_doc(|d| {
                d.descendants(NodeId(root))
                    .filter(|i| i.0 != n && d.element(*i).is_some_and(|e| e.is_html("input") && e.attr("name") == Some(name.as_str())))
                    .map(|i| i.0)
                    .collect()
            });
            for o in others {
                if checkable_type(o) == Some("radio") {
                    page(|p| p.checked.insert(o, false));
                }
            }
        }
    }
    super::touch();
}

fn input_type(el: usize) -> String {
    const KNOWN: &[&str] = &[
        "hidden", "text", "search", "tel", "url", "email", "password", "date", "month", "week", "time",
        "datetime-local", "number", "range", "color", "checkbox", "radio", "file", "submit", "image", "reset", "button",
    ];
    let t = attr_value(el, "type").unwrap_or_default().to_ascii_lowercase();
    if KNOWN.contains(&t.as_str()) { t } else { "text".into() }
}

fn input_type_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&input_type(el)))
}

fn input_type_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "type", &v);
    Ok(Value::Undefined)
}

/// HTML §4.10.5.4 value sanitization for the text-like types: strip newlines.
fn sanitize(ty: &str, v: &str) -> String {
    match ty {
        "text" | "search" | "tel" | "password" => v.chars().filter(|c| *c != '\n' && *c != '\r').collect(),
        "url" | "email" => v.chars().filter(|c| *c != '\n' && *c != '\r').collect::<String>().trim().to_string(),
        "color" => {
            let l = v.to_ascii_lowercase();
            if l.len() == 7 && l.starts_with('#') && l[1..].chars().all(|c| c.is_ascii_hexdigit()) { l } else { "#000000".into() }
        }
        _ => v.to_string(),
    }
}

fn input_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let ty = input_type(el);
    if matches!(ty.as_str(), "checkbox" | "radio") {
        return Ok(s(&attr_value(el, "value").unwrap_or_else(|| "on".into())));
    }
    if matches!(ty.as_str(), "hidden" | "submit" | "image" | "reset" | "button") {
        return Ok(s(&attr_value(el, "value").unwrap_or_default()));
    }
    let v = page(|p| p.values.get(&el).cloned()).unwrap_or_else(|| attr_value(el, "value").unwrap_or_default());
    Ok(s(&sanitize(&ty, &v)))
}

fn input_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    let ty = input_type(el);
    if matches!(ty.as_str(), "checkbox" | "radio" | "hidden" | "submit" | "image" | "reset" | "button") {
        set_attr(vm, el, "value", &v);
        return Ok(Value::Undefined);
    }
    let v = sanitize(&ty, &v);
    page(|p| p.values.insert(el, v));
    super::touch();
    Ok(Value::Undefined)
}

fn checked_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(Value::Bool(checkedness(el)))
}

fn checked_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    set_checkedness(vm, el, b);
    Ok(Value::Undefined)
}

fn textarea_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = page(|p| p.values.get(&el).cloned()).unwrap_or_else(|| with_doc(|d| dom::text_content_of(d, el)));
    Ok(s(&v.replace("\r\n", "\n").replace('\r', "\n")))
}

fn textarea_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    page(|p| p.values.insert(el, v));
    super::touch();
    Ok(Value::Undefined)
}

fn textarea_default_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&with_doc(|d| dom::text_content_of(d, el))))
}

fn textarea_default_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    dom::set_text_content(vm, el, v)?;
    Ok(Value::Undefined)
}

fn options_of(select: usize) -> Vec<usize> {
    with_doc(|d| {
        d.descendants(NodeId(select)).filter(|i| d.element(*i).is_some_and(|e| e.is_html("option"))).map(|i| i.0).collect()
    })
}

fn option_selected(o: usize) -> bool {
    page(|p| p.checked.get(&o).copied()).unwrap_or_else(|| attr_value(o, "selected").is_some())
}

fn option_value(o: usize) -> String {
    attr_value(o, "value").unwrap_or_else(|| {
        let t = with_doc(|d| dom::text_content_of(d, o));
        t.split(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
    })
}

/// The selected option of a single select (HTML §4.10.7 selectedness: the last selected, else the first
/// non-disabled option).
fn selected_option(select: usize) -> Option<usize> {
    let opts = options_of(select);
    opts.iter().rev().find(|&&o| option_selected(o)).copied().or_else(|| opts.into_iter().find(|&o| attr_value(o, "disabled").is_none()))
}

fn select_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&selected_option(el).map(option_value).unwrap_or_default()))
}

fn select_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    let mut hit = false;
    for o in options_of(el) {
        let sel = !hit && option_value(o) == v;
        hit |= sel;
        page(|p| p.checked.insert(o, sel));
    }
    super::touch();
    Ok(Value::Undefined)
}

fn select_selected_index_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let opts = options_of(el);
    let sel = selected_option(el);
    Ok(num(sel.and_then(|s| opts.iter().position(|o| *o == s)).map(|i| i as f64).unwrap_or(-1.0)))
}

fn select_selected_index_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let i = vm.to_int32(&arg(vm, ctx, 0))?;
    for (k, o) in options_of(el).into_iter().enumerate() {
        page(|p| p.checked.insert(o, k as i32 == i));
    }
    super::touch();
    Ok(Value::Undefined)
}

fn select_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, 35, |vm| dom::make_collection(vm, dom::CK_DOCFILTER, el, "options", "")))
}

fn select_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(options_of(el).len() as f64))
}

fn select_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(if attr_value(el, "multiple").is_some() { "select-multiple" } else { "select-one" }))
}

fn option_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&option_value(el)))
}

fn option_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "value", &v);
    Ok(Value::Undefined)
}

fn option_text_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let t = with_doc(|d| dom::text_content_of(d, el));
    Ok(s(&t.split(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")))
}

fn option_text_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    dom::set_text_content(vm, el, v)?;
    Ok(Value::Undefined)
}

fn option_selected_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let sel = with_doc(|d| d.ancestors_select(el));
    Ok(Value::Bool(match sel {
        Some(s) if attr_value(s, "multiple").is_none() => selected_option(s) == Some(el),
        _ => option_selected(el),
    }))
}

fn option_selected_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    if b {
        if let Some(sel) = with_doc(|d| d.ancestors_select(el)) {
            if attr_value(sel, "multiple").is_none() {
                for o in options_of(sel) {
                    page(|p| p.checked.insert(o, false));
                }
            }
        }
    }
    page(|p| p.checked.insert(el, b));
    super::touch();
    Ok(Value::Undefined)
}

trait SelectAncestor {
    fn ancestors_select(&self, el: usize) -> Option<usize>;
}

impl SelectAncestor for html_core::Document {
    fn ancestors_select(&self, el: usize) -> Option<usize> {
        let mut cur = self.parent_element(NodeId(el));
        while let Some(c) = cur {
            if self.element(c).is_some_and(|e| e.is_html("select")) {
                return Some(c.0);
            }
            cur = self.parent_element(c);
        }
        None
    }
}

fn form_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let f = with_doc(|d| {
        let mut cur = d.parent_element(NodeId(el));
        while let Some(c) = cur {
            if d.element(c).is_some_and(|e| e.is_html("form")) {
                return Some(c.0);
            }
            cur = d.parent_element(c);
        }
        None
    });
    Ok(dom::wrap_opt(vm, f))
}

fn form_method_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let m = attr_value(el, "method").unwrap_or_default().to_ascii_lowercase();
    Ok(s(if matches!(m.as_str(), "post" | "dialog") { &m } else { "get" }))
}

fn form_method_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "method", &v);
    Ok(Value::Undefined)
}

fn form_elements(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let ids: Vec<usize> = with_doc(|d| {
        d.descendants(NodeId(el))
            .filter(|i| d.element(*i).is_some_and(|e| e.ns == Namespace::Html && matches!(e.local.as_str(), "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea")))
            .map(|i| i.0)
            .collect()
    });
    Ok(dom::static_node_list(vm, ids))
}

fn form_submit(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    crate::ledger::record_dom("HTMLFormElement.submit-not-navigating");
    Ok(Value::Undefined)
}

fn form_reset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let ids: Vec<usize> = with_doc(|d| d.descendants(NodeId(el)).map(|i| i.0).collect());
    page(|p| {
        for i in ids {
            p.values.remove(&i);
            p.checked.remove(&i);
        }
    });
    super::touch();
    Ok(Value::Undefined)
}

fn button_type_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let t = attr_value(el, "type").unwrap_or_default().to_ascii_lowercase();
    Ok(s(if matches!(t.as_str(), "reset" | "button") { &t } else { "submit" }))
}

fn button_type_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "type", &v);
    Ok(Value::Undefined)
}

// ---- anchors and areas: HTMLHyperlinkElementUtils (HTML §4.6.2)

fn href_url(el: usize) -> Option<url::Url> {
    let h = attr_value(el, "href")?;
    let abs = dom::resolve_url(dom::node_document(el), &h)?;
    url::Url::parse(&abs).ok()
}

fn hyper_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let part = callee_str(vm, ctx);
    let Some(u) = href_url(el) else {
        let raw = if part == "href" { attr_value(el, "href").unwrap_or_default() } else { String::new() };
        return Ok(s(&raw));
    };
    Ok(s(&url_part(&u, &part)))
}

pub fn url_part(u: &url::Url, part: &str) -> String {
    match part {
        "href" => u.as_str().to_string(),
        "origin" => match u.origin() {
            url::Origin::Tuple(..) => u.origin().ascii_serialization(),
            url::Origin::Opaque(_) => "null".into(),
        },
        "protocol" => format!("{}:", u.scheme()),
        "username" => u.username().to_string(),
        "password" => u.password().unwrap_or("").to_string(),
        "host" => match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => String::new(),
        },
        "hostname" => u.host_str().unwrap_or("").to_string(),
        "port" => u.port().map(|p| p.to_string()).unwrap_or_default(),
        "pathname" => u.path().to_string(),
        "search" => u.query().filter(|q| !q.is_empty()).map(|q| format!("?{q}")).unwrap_or_default(),
        "hash" => u.fragment().filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Applies a URL component write the way the URL Standard's setters do.
pub fn set_url_part(u: &mut url::Url, part: &str, v: &str) {
    let _ = match part {
        "protocol" => u.set_scheme(v.trim_end_matches(':')).map_err(|_| ()),
        "username" => u.set_username(v),
        "password" => u.set_password(if v.is_empty() { None } else { Some(v) }),
        "hostname" => u.set_host(if v.is_empty() { None } else { Some(v) }).map_err(|_| ()),
        "host" => {
            let (h, p) = match v.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
                None => (v.to_string(), None),
            };
            u.set_host(if h.is_empty() { None } else { Some(&h) }).map_err(|_| ()).and_then(|_| u.set_port(p))
        }
        "port" => u.set_port(if v.is_empty() { None } else { v.parse::<u16>().ok() }),
        "pathname" => {
            u.set_path(v);
            Ok(())
        }
        "search" => {
            let q = v.trim_start_matches('?');
            u.set_query(if q.is_empty() { None } else { Some(q) });
            Ok(())
        }
        "hash" => {
            let f = v.trim_start_matches('#');
            u.set_fragment(if f.is_empty() { None } else { Some(f) });
            Ok(())
        }
        _ => Ok(()),
    };
}

fn hyper_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let part = callee_str(vm, ctx);
    let v = string(vm, &arg(vm, ctx, 0))?;
    if part == "href" {
        set_attr(vm, el, "href", &v);
        return Ok(Value::Undefined);
    }
    if let Some(mut u) = href_url(el) {
        set_url_part(&mut u, &part, &v);
        set_attr(vm, el, "href", u.as_str());
    }
    Ok(Value::Undefined)
}

fn anchor_text_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&with_doc(|d| dom::text_content_of(d, el))))
}

fn anchor_text_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    dom::set_text_content(vm, el, v)?;
    Ok(Value::Undefined)
}

fn rel_list_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, dom::SO_RELLIST, |vm| dom::make_collection(vm, dom::CK_TOKENS, el, "rel", "")))
}

// ---- script, template, image, canvas, label

fn script_text_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let t = with_doc(|d| d.children(NodeId(el)).filter_map(|c| match d.data(c) {
        html_core::NodeData::Text(t) => Some(t.clone()),
        _ => None,
    }).collect::<String>());
    Ok(s(&t))
}

fn script_text_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    dom::set_text_content(vm, el, v)?;
    Ok(Value::Undefined)
}

fn script_async_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    // "force async" applies to script-created (non-parser-inserted) elements without the attribute.
    let forced = page(|p| !p.parser_inserted.contains(&el) && !p.async_cleared.contains(&el));
    Ok(Value::Bool(attr_value(el, "async").is_some() || forced))
}

fn script_async_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    page(|p| p.async_cleared.insert(el));
    if b {
        set_attr(vm, el, "async", "");
    } else {
        remove_attr(vm, el, "async");
    }
    Ok(Value::Undefined)
}

// ---- tables (HTML §4.9)

fn table_coll(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let so = match which.as_str() {
        "rows" => 30,
        "tbodies" => 31,
        "srows" => 32,
        "cells" => 33,
        _ => 34,
    };
    Ok(dom::cached(vm, el, so, |vm| dom::make_collection(vm, dom::CK_DOCFILTER, el, &which, "")))
}

fn first_child_tag(el: usize, tag: &str) -> Option<usize> {
    with_doc(|d| d.children(NodeId(el)).find(|c| d.element(*c).is_some_and(|e| e.is_html(tag))).map(|c| c.0))
}

fn table_part(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let hit = first_child_tag(el, &which);
    Ok(dom::wrap_opt(vm, hit))
}

fn index_in(parent_list: Vec<usize>, el: usize) -> f64 {
    parent_list.iter().position(|x| *x == el).map(|i| i as f64).unwrap_or(-1.0)
}

fn row_index(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let parent = dom::parent_of(el);
    let Some(parent) = parent else { return Ok(num(-1.0)) };
    if which == "section" {
        let rows: Vec<usize> = with_doc(|d| d.children(NodeId(parent)).filter(|c| d.element(*c).is_some_and(|e| e.is_html("tr"))).map(|c| c.0).collect());
        return Ok(num(index_in(rows, el)));
    }
    // rowIndex: the index in the nearest table's rows collection.
    let table = with_doc(|d| {
        let mut cur = Some(NodeId(parent));
        while let Some(c) = cur {
            if d.element(c).is_some_and(|e| e.is_html("table")) {
                return Some(c.0);
            }
            cur = d.parent(c);
        }
        None
    });
    let Some(t) = table else { return Ok(num(-1.0)) };
    let coll = dom::make_collection(vm, dom::CK_DOCFILTER, t, "rows", "");
    let len = vm.get(coll, &PropertyKey::from_str("length"))?;
    let n = vm.to_number(&len)? as usize;
    for i in 0..n {
        let v = vm.get(coll, &PropertyKey::Index(i as u32))?;
        if dom::node_of(vm, &v) == Some(el) {
            return Ok(num(i as f64));
        }
    }
    Ok(num(-1.0))
}

fn cell_index(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let Some(parent) = dom::parent_of(el) else { return Ok(num(-1.0)) };
    let cells: Vec<usize> = with_doc(|d| d.children(NodeId(parent)).filter(|c| d.element(*c).is_some_and(|e| e.is_html("td") || e.is_html("th"))).map(|c| c.0).collect());
    Ok(num(index_in(cells, el)))
}

fn insert_child_tag(vm: &mut Vm, parent: usize, tag: &str, kids: Vec<usize>, index: i32) -> JsResult<Value> {
    if index < -1 || index > kids.len() as i32 {
        return throw_dom(vm, "IndexSizeError", &format!("The index provided ({index}) is outside the range [-1, {}].", kids.len()));
    }
    let new = dom::create_element(dom::node_document(parent), Namespace::Html, tag);
    let reference = if index == -1 || index as usize == kids.len() { None } else { Some(kids[index as usize]) };
    let p = match reference {
        Some(r) => dom::parent_of(r).unwrap_or(parent),
        None => parent,
    };
    dom::insert(vm, new, p, reference);
    Ok(wrap(vm, new))
}

fn insert_row(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let i = arg(vm, ctx, 0);
    let index = if i.is_undefined() { -1 } else { vm.to_int32(&i)? };
    let is_table = with_doc(|d| dom::is_html_tag(d, el, "table"));
    if is_table {
        let coll = dom::make_collection(vm, dom::CK_DOCFILTER, el, "rows", "");
        let lv = vm.get(coll, &PropertyKey::from_str("length"))?;
        let len = vm.to_number(&lv)? as usize;
        let mut rows = Vec::new();
        for k in 0..len {
            let v = vm.get(coll, &PropertyKey::Index(k as u32))?;
            if let Some(n) = dom::node_of(vm, &v) {
                rows.push(n);
            }
        }
        if rows.is_empty() && (index == -1 || index == 0) {
            // A new row goes into the last tbody, or a new tbody.
            let tbody = with_doc(|d| d.children(NodeId(el)).filter(|c| d.element(*c).is_some_and(|e| e.is_html("tbody"))).last().map(|c| c.0));
            let tb = match tbody {
                Some(t) => t,
                None => {
                    let t = dom::create_element(dom::node_document(el), Namespace::Html, "tbody");
                    dom::insert(vm, t, el, None);
                    t
                }
            };
            return insert_child_tag(vm, tb, "tr", Vec::new(), -1);
        }
        return insert_child_tag(vm, el, "tr", rows, index);
    }
    let rows: Vec<usize> = with_doc(|d| d.children(NodeId(el)).filter(|c| d.element(*c).is_some_and(|e| e.is_html("tr"))).map(|c| c.0).collect());
    insert_child_tag(vm, el, "tr", rows, index)
}

fn insert_cell(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let i = arg(vm, ctx, 0);
    let index = if i.is_undefined() { -1 } else { vm.to_int32(&i)? };
    let cells: Vec<usize> = with_doc(|d| d.children(NodeId(el)).filter(|c| d.element(*c).is_some_and(|e| e.is_html("td") || e.is_html("th"))).map(|c| c.0).collect());
    insert_child_tag(vm, el, "td", cells, index)
}

fn delete_child_at(vm: &mut Vm, ctx: &CallCtx, tags: &[&str]) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let index = vm.to_int32(&arg(vm, ctx, 0))?;
    let kids: Vec<usize> = with_doc(|d| d.children(NodeId(el)).filter(|c| d.element(*c).is_some_and(|e| tags.iter().any(|t| e.is_html(t)))).map(|c| c.0).collect());
    let i = if index == -1 { kids.len() as i32 - 1 } else { index };
    if i < 0 || i as usize >= kids.len() {
        if index == -1 {
            return Ok(Value::Undefined);
        }
        return throw_dom(vm, "IndexSizeError", &format!("The index provided ({index}) is outside the range."));
    }
    dom::remove(vm, kids[i as usize]);
    Ok(Value::Undefined)
}

fn delete_row(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    delete_child_at(vm, ctx, &["tr"])
}

fn delete_cell(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    delete_child_at(vm, ctx, &["td", "th"])
}

fn template_content(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let t = with_doc(|d| d.element(NodeId(el)).and_then(|e| e.template_contents).map(|t| t.0));
    Ok(dom::wrap_opt(vm, t))
}

fn img_complete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    Ok(Value::Bool(true))
}

fn img_natural(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let src = attr_value(el, "src").and_then(|x| dom::resolve_url(dom::node_document(el), &x));
    let dims = src.and_then(|u| crate::images::get(&u)).map(|i| (i.width(), i.height()));
    Ok(num(dims.map(|(w, h)| if which == "w" { w } else { h }).unwrap_or(0) as f64))
}

fn img_current_src(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let src = attr_value(el, "src").and_then(|x| dom::resolve_url(dom::node_document(el), &x));
    Ok(s(&src.unwrap_or_default()))
}

fn image_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Image': Please use the 'new' operator");
    }
    let doc = dom::main_doc();
    let el = dom::create_element(doc, Namespace::Html, "img");
    let w = arg(vm, ctx, 0);
    let h = arg(vm, ctx, 1);
    if !w.is_undefined() {
        let n = vm.to_uint32(&w)?;
        set_attr(vm, el, "width", &n.to_string());
    }
    if !h.is_undefined() {
        let n = vm.to_uint32(&h)?;
        set_attr(vm, el, "height", &n.to_string());
    }
    Ok(wrap(vm, el))
}

fn audio_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Audio': Please use the 'new' operator");
    }
    let el = dom::create_element(dom::main_doc(), Namespace::Html, "audio");
    set_attr(vm, el, "preload", "auto");
    let src = arg(vm, ctx, 0);
    if !src.is_undefined() {
        let v = string(vm, &src)?;
        set_attr(vm, el, "src", &v);
    }
    Ok(wrap(vm, el))
}

fn option_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Option': Please use the 'new' operator");
    }
    let doc = dom::main_doc();
    let el = dom::create_element(doc, Namespace::Html, "option");
    let text = arg(vm, ctx, 0);
    if !text.is_undefined() {
        let t = string(vm, &text)?;
        if !t.is_empty() {
            let tx = dom::create_text(doc, t);
            dom::with_doc_mut(|d| d.append(NodeId(el), NodeId(tx)));
        }
    }
    let value = arg(vm, ctx, 1);
    if !value.is_undefined() {
        let v = string(vm, &value)?;
        set_attr(vm, el, "value", &v);
    }
    if vm.to_boolean(&arg(vm, ctx, 2)) {
        set_attr(vm, el, "selected", "");
    }
    if vm.to_boolean(&arg(vm, ctx, 3)) {
        page(|p| p.checked.insert(el, true));
    }
    Ok(wrap(vm, el))
}

fn canvas_get_context(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    crate::ledger::record_dom("HTMLCanvasElement.getContext-unsupported");
    Ok(Value::Null)
}

fn canvas_to_data_url(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    Ok(s("data:,"))
}

fn label_control(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let hit = match attr_value(el, "for") {
        Some(id) => with_doc(|d| d.query_first(NodeId(dom::root_of(el)), |d, i| d.element_id(i) == Some(id.as_str())).map(|i| i.0)),
        None => with_doc(|d| {
            d.query_first(NodeId(el), |d, i| d.element(i).is_some_and(|e| e.ns == Namespace::Html && matches!(e.local.as_str(), "input" | "select" | "textarea" | "button" | "meter" | "output" | "progress")))
                .map(|i| i.0)
        }),
    };
    Ok(dom::wrap_opt(vm, hit))
}

fn dialog_show(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    set_attr(vm, el, "open", "");
    Ok(Value::Undefined)
}

fn dialog_close(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if attr_value(el, "open").is_some() {
        remove_attr(vm, el, "open");
        super::events::fire_simple(vm, super::TargetKey::Node(el), "close", false, false);
    }
    Ok(Value::Undefined)
}

// =================================================================================================
// HTMLMediaElement (HTML §4.8.11) over Aether's media registry
// =================================================================================================

fn media_entry(el: usize) -> Option<crate::media::Element> {
    let node = dom::node_ref(el);
    let find = || crate::media::snapshot().into_iter().find(|e| e.node == node);
    if let Some(e) = find() {
        return Some(e);
    }
    // A script-created element registers when it is in the document (as layout would register it).
    if dom::is_connected(el) {
        let doc = dom::node_ref(dom::main_doc());
        crate::media::scan(&doc, &super::page_url());
        return find();
    }
    None
}

fn media_play(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let pc = vm.intr().promise_ctor;
    let (p, resolve, reject) = js_core::builtins::promise::new_capability(vm, &Value::Object(pc))?;
    // No src and no <source>: the resource selection algorithm waits (networkState NETWORK_EMPTY), and
    // the play promise stays pending until a source appears (HTML §4.8.11.5).
    let has_source = attr_value(el, "src").is_some()
        || with_doc(|d| d.children(NodeId(el)).any(|c| d.element(c).is_some_and(|e| e.is_html("source"))));
    if !has_source {
        return Ok(Value::Object(p));
    }
    match media_entry(el) {
        Some(e) if !matches!(e.state, crate::media::State::Error(_)) => {
            let was_paused = !matches!(e.state, crate::media::State::Playing);
            crate::media::play(&dom::node_ref(el));
            if was_paused {
                page(|p| p.tasks.push_back(super::Task::Event(super::TargetKey::Node(el), "play".into(), false)));
            }
            vm.call(&resolve, &Value::Undefined, &[Value::Undefined])?;
        }
        _ => {
            let e = dom_exception(vm, "NotSupportedError", "The element has no supported sources.");
            vm.call(&reject, &Value::Undefined, &[e])?;
        }
    }
    Ok(Value::Object(p))
}

fn media_pause(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    if crate::media::pause(&dom::node_ref(el)) {
        page(|p| p.tasks.push_back(super::Task::Event(super::TargetKey::Node(el), "pause".into(), false)));
    }
    Ok(Value::Undefined)
}

fn media_load(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let _ = media_entry(el);
    Ok(Value::Undefined)
}

fn media_can_play_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    need(vm, ctx, 1, "HTMLMediaElement", "canPlayType")?;
    let t = string(vm, &arg(vm, ctx, 0))?;
    Ok(s(if crate::media::can_play_type(&t) { if t.contains("codecs") { "probably" } else { "maybe" } } else { "" }))
}

fn media_paused(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(Value::Bool(!matches!(media_entry(el).map(|e| e.state), Some(crate::media::State::Playing))))
}

fn media_ended(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(Value::Bool(matches!(media_entry(el).map(|e| e.state), Some(crate::media::State::Ended))))
}

fn media_current_time_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(media_entry(el).map(|e| e.pts_ns.max(0) as f64 / 1e9).unwrap_or(0.0)))
}

fn media_current_time_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let t = vm.to_number(&arg(vm, ctx, 0))?;
    if !t.is_finite() {
        return vm.throw_type("The provided double value is non-finite.");
    }
    crate::media::seek(&dom::node_ref(el), (t.max(0.0) * 1e9) as u64);
    Ok(Value::Undefined)
}

fn media_duration(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(media_entry(el).filter(|e| e.duration_ns > 0).map(|e| e.duration_ns as f64 / 1e9).unwrap_or(f64::NAN)))
}

fn media_ready_state(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(match media_entry(el) {
        Some(e) if e.duration_ns > 0 || e.frame.is_some() => 4.0,
        Some(e) if matches!(e.state, crate::media::State::Playing | crate::media::State::Paused) => 1.0,
        _ => 0.0,
    }))
}

fn media_network_state(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(match media_entry(el) {
        Some(e) if e.fetching => 2.0,
        Some(e) if !e.url.is_empty() => 1.0,
        Some(_) => 3.0,
        None => 0.0,
    }))
}

fn media_current_src(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let src = crate::media::source_for(&dom::node_ref(el), &super::page_url()).map(|(u, _)| u).unwrap_or_default();
    Ok(s(&src))
}

fn media_volume_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(num(page(|p| p.media_volume.get(&el).copied()).unwrap_or(1.0)))
}

fn media_volume_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = vm.to_number(&arg(vm, ctx, 0))?;
    if !(0.0..=1.0).contains(&v) {
        return throw_dom(vm, "IndexSizeError", &format!("The volume provided ({v}) is outside the range [0, 1]."));
    }
    page(|p| p.media_volume.insert(el, v));
    crate::ledger::record_dom("HTMLMediaElement.volume-not-forwarded-to-stria");
    page(|p| p.tasks.push_back(super::Task::Event(super::TargetKey::Node(el), "volumechange".into(), false)));
    Ok(Value::Undefined)
}

fn media_muted_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(Value::Bool(page(|p| p.media_muted.get(&el).copied()).unwrap_or_else(|| attr_value(el, "muted").is_some())))
}

fn media_muted_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let b = vm.to_boolean(&arg(vm, ctx, 0));
    page(|p| p.media_muted.insert(el, b));
    crate::media::set_muted(&dom::node_ref(el), b);
    page(|p| p.tasks.push_back(super::Task::Event(super::TargetKey::Node(el), "volumechange".into(), false)));
    Ok(Value::Undefined)
}

fn media_const_false(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    Ok(Value::Bool(false))
}

fn media_null(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    Ok(Value::Null)
}

fn media_rate(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    Ok(num(1.0))
}

fn video_size(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let which = callee_str(vm, ctx);
    Ok(num(media_entry(el).and_then(|e| e.natural).map(|(w, h)| if which == "w" { w } else { h }).unwrap_or(0) as f64))
}

// =================================================================================================
// Installation
// =================================================================================================

pub fn install(vm: &mut Vm) {
    let element = iface("Element").unwrap();
    let he = interface(vm, "HTMLElement", Some(element), None);
    register_iface("HTMLElement", he);
    let p = he.proto;
    attr(vm, p, "innerText", inner_text_get, Some(inner_text_set));
    attr(vm, p, "outerText", inner_text_get, None);
    op(vm, p, "click", 0, click_op);
    op(vm, p, "focus", 0, focus_op);
    op(vm, p, "blur", 0, blur_op);
    attr(vm, p, "tabIndex", tab_index_get, Some(tab_index_set));
    attr(vm, p, "contentEditable", content_editable_get, Some(content_editable_set));
    attr(vm, p, "isContentEditable", is_content_editable, None);
    attr(vm, p, "draggable", draggable_get, Some(draggable_set));
    attr(vm, p, "spellcheck", spellcheck_get, Some(spellcheck_set));
    attr(vm, p, "translate", translate_get, Some(translate_set));
    attr(vm, p, "dataset", dataset_get, None);
    super::events::install_handlers(vm, p, super::events::GLOBAL_HANDLERS);
    // SVGElement/MathMLElement share focus, dataset and the global handlers.
    let svg = interface(vm, "SVGElement", Some(element), None);
    register_iface("SVGElement", svg);
    attr(vm, svg.proto, "dataset", dataset_get, None);
    super::events::install_handlers(vm, svg.proto, super::events::GLOBAL_HANDLERS);
    let svg_graphics = interface(vm, "SVGGraphicsElement", Some(svg), None);
    register_iface("SVGGraphicsElement", svg_graphics);
    let svgsvg = interface(vm, "SVGSVGElement", Some(svg_graphics), None);
    register_iface("SVGSVGElement", svgsvg);
    let mathml = interface(vm, "MathMLElement", Some(element), None);
    register_iface("MathMLElement", mathml);

    for (name, parent) in ELEMENT_IFACES {
        let pi = iface(parent).unwrap();
        let i = interface(vm, name, Some(pi), None);
        register_iface(name, i);
    }
    for (iname, idl_name, content, k) in REFLECT {
        let i = iface(iname).unwrap();
        attr_with(vm, i.proto, idl_name, reflect_get, Some(reflect_set), s(&format!("{content}|{k}")));
    }

    let ip = iface("HTMLInputElement").unwrap().proto;
    attr(vm, ip, "type", input_type_get, Some(input_type_set));
    attr(vm, ip, "value", input_value_get, Some(input_value_set));
    attr(vm, ip, "checked", checked_get, Some(checked_set));
    attr(vm, ip, "form", form_get, None);
    let tp = iface("HTMLTextAreaElement").unwrap().proto;
    attr(vm, tp, "value", textarea_value_get, Some(textarea_value_set));
    attr(vm, tp, "defaultValue", textarea_default_get, Some(textarea_default_set));
    attr(vm, tp, "form", form_get, None);
    let sp = iface("HTMLSelectElement").unwrap().proto;
    attr(vm, sp, "value", select_value_get, Some(select_value_set));
    attr(vm, sp, "selectedIndex", select_selected_index_get, Some(select_selected_index_set));
    attr(vm, sp, "options", select_options, None);
    attr(vm, sp, "length", select_length, None);
    attr(vm, sp, "type", select_type, None);
    attr(vm, sp, "form", form_get, None);
    let op_ = iface("HTMLOptionElement").unwrap().proto;
    attr(vm, op_, "value", option_value_get, Some(option_value_set));
    attr(vm, op_, "text", option_text_get, Some(option_text_set));
    attr(vm, op_, "label", option_text_get, None);
    attr(vm, op_, "selected", option_selected_get, Some(option_selected_set));
    attr(vm, op_, "form", form_get, None);
    let bp = iface("HTMLButtonElement").unwrap().proto;
    attr(vm, bp, "type", button_type_get, Some(button_type_set));
    attr(vm, bp, "form", form_get, None);
    let fp = iface("HTMLFormElement").unwrap().proto;
    attr(vm, fp, "method", form_method_get, Some(form_method_set));
    attr(vm, fp, "elements", form_elements, None);
    op(vm, fp, "submit", 0, form_submit);
    op(vm, fp, "requestSubmit", 0, form_submit);
    op(vm, fp, "reset", 0, form_reset);
    for name in ["HTMLAnchorElement", "HTMLAreaElement"] {
        let p = iface(name).unwrap().proto;
        for part in ["href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash"] {
            let setter = if part == "origin" { None } else { Some(hyper_set as NativeFn) };
            attr_with(vm, p, part, hyper_get, setter, s(part));
        }
        attr(vm, p, "relList", rel_list_get, None);
        let ts = vm.make_native_with("toString", 0, hyper_get, false, Some(vm.intr().function_proto), vec![s("href")]);
        vm.heap.get_mut(p).props.insert(PropertyKey::from_str("toString"), Prop::data(Value::Object(ts), WEC));
    }
    attr(vm, iface("HTMLAnchorElement").unwrap().proto, "text", anchor_text_get, Some(anchor_text_set));
    attr(vm, iface("HTMLLinkElement").unwrap().proto, "relList", rel_list_get, None);
    let scr = iface("HTMLScriptElement").unwrap().proto;
    attr(vm, scr, "text", script_text_get, Some(script_text_set));
    attr(vm, scr, "async", script_async_get, Some(script_async_set));
    attr(vm, iface("HTMLTemplateElement").unwrap().proto, "content", template_content, None);
    let tbl = iface("HTMLTableElement").unwrap().proto;
    attr_with(vm, tbl, "rows", table_coll, None, s("rows"));
    attr_with(vm, tbl, "tBodies", table_coll, None, s("tbodies"));
    attr_with(vm, tbl, "caption", table_part, None, s("caption"));
    attr_with(vm, tbl, "tHead", table_part, None, s("thead"));
    attr_with(vm, tbl, "tFoot", table_part, None, s("tfoot"));
    op(vm, tbl, "insertRow", 0, insert_row);
    op(vm, tbl, "deleteRow", 1, delete_row);
    let sec = iface("HTMLTableSectionElement").unwrap().proto;
    attr_with(vm, sec, "rows", table_coll, None, s("srows"));
    op(vm, sec, "insertRow", 0, insert_row);
    op(vm, sec, "deleteRow", 1, delete_row);
    let tr = iface("HTMLTableRowElement").unwrap().proto;
    attr_with(vm, tr, "cells", table_coll, None, s("cells"));
    attr_with(vm, tr, "rowIndex", row_index, None, s("table"));
    attr_with(vm, tr, "sectionRowIndex", row_index, None, s("section"));
    op(vm, tr, "insertCell", 0, insert_cell);
    op(vm, tr, "deleteCell", 1, delete_cell);
    attr(vm, iface("HTMLTableCellElement").unwrap().proto, "cellIndex", cell_index, None);
    let img = iface("HTMLImageElement").unwrap().proto;
    attr(vm, img, "complete", img_complete, None);
    attr(vm, img, "currentSrc", img_current_src, None);
    attr_with(vm, img, "naturalWidth", img_natural, None, s("w"));
    attr_with(vm, img, "naturalHeight", img_natural, None, s("h"));
    let canvas = iface("HTMLCanvasElement").unwrap().proto;
    op(vm, canvas, "getContext", 1, canvas_get_context);
    op(vm, canvas, "toDataURL", 0, canvas_to_data_url);
    attr(vm, iface("HTMLLabelElement").unwrap().proto, "control", label_control, None);
    let dlg = iface("HTMLDialogElement").unwrap().proto;
    op(vm, dlg, "show", 0, dialog_show);
    op(vm, dlg, "showModal", 0, dialog_show);
    op(vm, dlg, "close", 0, dialog_close);
    // Named constructors (HTML §4.8.3 Image, §4.10.10 Option, §4.8.11 Audio).
    let fpr = vm.intr().function_proto;
    for (name, f, proto) in [
        ("Image", image_ctor as NativeFn, "HTMLImageElement"),
        ("Option", option_ctor, "HTMLOptionElement"),
        ("Audio", audio_ctor, "HTMLAudioElement"),
    ] {
        let c = vm.make_native_with(name, 0, f, true, Some(fpr), Vec::new());
        let pr = iface(proto).unwrap().proto;
        vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(pr), 0));
        let g = vm.realm().global;
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(c), WC));
        root(vm, Value::Object(c));
    }

    // HTMLMediaElement
    let mi = iface("HTMLMediaElement").unwrap();
    for (n, v) in [
        ("NETWORK_EMPTY", 0.0),
        ("NETWORK_IDLE", 1.0),
        ("NETWORK_LOADING", 2.0),
        ("NETWORK_NO_SOURCE", 3.0),
        ("HAVE_NOTHING", 0.0),
        ("HAVE_METADATA", 1.0),
        ("HAVE_CURRENT_DATA", 2.0),
        ("HAVE_FUTURE_DATA", 3.0),
        ("HAVE_ENOUGH_DATA", 4.0),
    ] {
        konst(vm, mi, n, v);
    }
    let mp = mi.proto;
    op(vm, mp, "play", 0, media_play);
    op(vm, mp, "pause", 0, media_pause);
    op(vm, mp, "load", 0, media_load);
    op(vm, mp, "canPlayType", 1, media_can_play_type);
    attr(vm, mp, "paused", media_paused, None);
    attr(vm, mp, "ended", media_ended, None);
    attr(vm, mp, "currentTime", media_current_time_get, Some(media_current_time_set));
    attr(vm, mp, "duration", media_duration, None);
    attr(vm, mp, "readyState", media_ready_state, None);
    attr(vm, mp, "networkState", media_network_state, None);
    attr(vm, mp, "currentSrc", media_current_src, None);
    attr(vm, mp, "volume", media_volume_get, Some(media_volume_set));
    attr(vm, mp, "muted", media_muted_get, Some(media_muted_set));
    attr(vm, mp, "seeking", media_const_false, None);
    attr(vm, mp, "error", media_null, None);
    attr(vm, mp, "playbackRate", media_rate, None);
    attr(vm, mp, "defaultPlaybackRate", media_rate, None);
    let vp = iface("HTMLVideoElement").unwrap().proto;
    attr_with(vm, vp, "videoWidth", video_size, None, s("w"));
    attr_with(vm, vp, "videoHeight", video_size, None, s("h"));

    // DOMStringMap
    let dsm = interface(vm, "DOMStringMap", None, None);
    register_iface("DOMStringMap", dsm);
    let handler = vm.new_plain_object();
    for (name, f, len) in [
        ("get", ds_trap_get as NativeFn, 3),
        ("set", ds_trap_set, 4),
        ("has", ds_trap_has, 2),
        ("deleteProperty", ds_trap_delete, 2),
        ("ownKeys", ds_trap_own_keys, 1),
        ("getOwnPropertyDescriptor", ds_trap_gopd, 2),
        ("defineProperty", ds_trap_define, 3),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(handler).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
    }
    root(vm, Value::Object(handler));
    DATASET_HANDLER.with(|h| h.set(Some(handler)));
}
