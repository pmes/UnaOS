//! HTML §2.6.1 reflection (AETHERWPT, SR72): every reflected IDL attribute of the HTML element
//! interfaces, by the algorithm its type names — DOMString (with [LegacyNullToEmptyString] where the IDL
//! says so), USVString-reflecting-a-URL, boolean, enumerated limited to known values (keywords, missing
//! value default, invalid value default, aliases; nullable for `crossOrigin` and the ARIA mixin), long,
//! long limited to non-negative, unsigned long, unsigned long limited to positive (throwing or with
//! fallback), clamped unsigned long, double and double limited to positive.
//!
//! Installed after `html::install`, it supersedes that file's short reflection table for the names it
//! lists; anything with behaviour beyond reflection (input.value, option.label, …) stays where it is.

use super::dom::{self, attr_value, remove_attr, set_attr, this_element};
use super::idl::*;
use js_core::vm::*;

#[derive(Clone, Debug)]
enum K {
    Str,
    /// DOMString with [LegacyNullToEmptyString].
    StrNull,
    Url,
    /// `form.action` / `formAction`: a URL whose absent or empty value is the document's URL.
    Action,
    Bool,
    Long(i64),
    LimLong(i64),
    ULong(i64),
    LimULong(i64),
    LimULongFb(i64),
    Clamped(i64, i64, i64),
    Double(f64),
    LimDouble(f64),
    /// keywords, missing default (None = null), invalid default (None = null), nullable, aliases.
    Enum(Vec<String>, Option<String>, Option<String>, bool, Vec<(String, Option<String>)>),
    /// `hidden`: boolean, or the "until-found" state.
    Hidden,
    TabIndex,
}

struct R {
    iface: String,
    idl: String,
    attr: String,
    k: K,
}

thread_local! {
    static TABLE: std::cell::RefCell<Vec<R>> = const { std::cell::RefCell::new(Vec::new()) };
}

const MAX_INT: i64 = 2147483647;
const REFERRER: &str = ",no-referrer,no-referrer-when-downgrade,same-origin,origin,strict-origin,origin-when-cross-origin,strict-origin-when-cross-origin,unsafe-url";
const ENCTYPES: &str = "application/x-www-form-urlencoded,multipart/form-data,text/plain";

/// The table, one attribute per line: `Interface idlName[=content] kind [args]`.
/// Kinds: s, sn (null → ""), u, action, b, l D, ll, ul D, lu D, luf D, cu MIN MAX D, d D, ld D,
/// e KEYWORDS [m=MISSING] [i=INVALID] [a=FROM>TO …] (ne: nullable; `null` is the null default).
fn spec() -> String {
    let mut s = String::from(
        "\
HTMLElement title s
HTMLElement lang s
HTMLElement dir e ltr,rtl,auto
HTMLElement hidden hidden
HTMLElement inert b
HTMLElement accessKey=accesskey s
HTMLElement autofocus b
HTMLElement tabIndex=tabindex tabindex
HTMLElement enterKeyHint=enterkeyhint e enter,done,go,next,previous,search,send
HTMLElement inputMode=inputmode e none,text,tel,url,email,numeric,decimal,search
HTMLAnchorElement target s
HTMLAnchorElement download s
HTMLAnchorElement ping s
HTMLAnchorElement rel s
HTMLAnchorElement hreflang s
HTMLAnchorElement type s
HTMLAnchorElement coords s
HTMLAnchorElement charset s
HTMLAnchorElement name s
HTMLAnchorElement rev s
HTMLAnchorElement shape s
HTMLAreaElement alt s
HTMLAreaElement coords s
HTMLAreaElement shape s
HTMLAreaElement target s
HTMLAreaElement download s
HTMLAreaElement ping s
HTMLAreaElement rel s
HTMLAreaElement hreflang s
HTMLAreaElement type s
HTMLAreaElement noHref=nohref b
HTMLBaseElement target s
HTMLQuoteElement cite u
HTMLModElement cite u
HTMLModElement dateTime=datetime s
HTMLBodyElement text sn
HTMLBodyElement link sn
HTMLBodyElement vLink=vlink sn
HTMLBodyElement aLink=alink sn
HTMLBodyElement bgColor=bgcolor sn
HTMLBodyElement background s
HTMLBRElement clear s
HTMLButtonElement disabled b
HTMLButtonElement formAction=formaction action
HTMLButtonElement formNoValidate=formnovalidate b
HTMLButtonElement formTarget=formtarget s
HTMLButtonElement name s
HTMLButtonElement value s
HTMLButtonElement type e submit,reset,button m=submit
HTMLCanvasElement width ul 300
HTMLCanvasElement height ul 150
HTMLTableCaptionElement align s
HTMLTableColElement span cu 1 1000 1
HTMLTableColElement align s
HTMLTableColElement ch=char s
HTMLTableColElement chOff=charoff s
HTMLTableColElement vAlign=valign s
HTMLTableColElement width s
HTMLDataElement value s
HTMLDetailsElement open b
HTMLDialogElement open b
HTMLDirectoryElement compact b
HTMLDivElement align s
HTMLDListElement compact b
HTMLEmbedElement src u
HTMLEmbedElement type s
HTMLEmbedElement width s
HTMLEmbedElement height s
HTMLEmbedElement align s
HTMLEmbedElement name s
HTMLFieldSetElement disabled b
HTMLFieldSetElement name s
HTMLFontElement color sn
HTMLFontElement face s
HTMLFontElement size s
HTMLFormElement acceptCharset=accept-charset s
HTMLFormElement action action
HTMLFormElement name s
HTMLFormElement noValidate=novalidate b
HTMLFormElement target s
HTMLFrameElement name s
HTMLFrameElement scrolling s
HTMLFrameElement src u
HTMLFrameElement frameBorder=frameborder s
HTMLFrameElement longDesc=longdesc u
HTMLFrameElement noResize=noresize b
HTMLFrameElement marginHeight=marginheight sn
HTMLFrameElement marginWidth=marginwidth sn
HTMLFrameSetElement cols s
HTMLFrameSetElement rows s
HTMLHeadingElement align s
HTMLHRElement align s
HTMLHRElement color s
HTMLHRElement noShade=noshade b
HTMLHRElement size s
HTMLHRElement width s
HTMLHtmlElement version s
HTMLIFrameElement src u
HTMLIFrameElement srcdoc s
HTMLIFrameElement name s
HTMLIFrameElement allowFullscreen=allowfullscreen b
HTMLIFrameElement width s
HTMLIFrameElement height s
HTMLIFrameElement align s
HTMLIFrameElement scrolling s
HTMLIFrameElement frameBorder=frameborder s
HTMLIFrameElement longDesc=longdesc u
HTMLIFrameElement marginHeight=marginheight sn
HTMLIFrameElement marginWidth=marginwidth sn
HTMLImageElement alt s
HTMLImageElement src u
HTMLImageElement srcset s
HTMLImageElement sizes s
HTMLImageElement useMap=usemap s
HTMLImageElement isMap=ismap b
HTMLImageElement decoding e async,sync,auto m=auto i=auto
HTMLImageElement loading e lazy,eager m=eager i=eager
HTMLImageElement fetchPriority=fetchpriority e high,low,auto m=auto i=auto
HTMLImageElement name s
HTMLImageElement lowsrc u
HTMLImageElement align s
HTMLImageElement hspace ul 0
HTMLImageElement vspace ul 0
HTMLImageElement longDesc=longdesc u
HTMLImageElement border sn
HTMLInputElement accept s
HTMLInputElement alt s
HTMLInputElement defaultChecked=checked b
HTMLInputElement dirName=dirname s
HTMLInputElement disabled b
HTMLInputElement formAction=formaction action
HTMLInputElement formNoValidate=formnovalidate b
HTMLInputElement formTarget=formtarget s
HTMLInputElement max s
HTMLInputElement maxLength=maxlength ll
HTMLInputElement min s
HTMLInputElement minLength=minlength ll
HTMLInputElement multiple b
HTMLInputElement name s
HTMLInputElement pattern s
HTMLInputElement placeholder s
HTMLInputElement readOnly=readonly b
HTMLInputElement required b
HTMLInputElement size lu 20
HTMLInputElement src u
HTMLInputElement step s
HTMLInputElement defaultValue=value s
HTMLInputElement align s
HTMLInputElement useMap=usemap s
HTMLLabelElement htmlFor=for s
HTMLLegendElement align s
HTMLLIElement value l 0
HTMLLIElement type s
HTMLLinkElement href u
HTMLLinkElement rel s
HTMLLinkElement media s
HTMLLinkElement nonce s
HTMLLinkElement integrity s
HTMLLinkElement hreflang s
HTMLLinkElement type s
HTMLLinkElement charset s
HTMLLinkElement rev s
HTMLLinkElement target s
HTMLLinkElement as e fetch,audio,document,embed,font,image,manifest,object,report,script,sharedworker,style,track,video,worker,xslt
HTMLLinkElement fetchPriority=fetchpriority e high,low,auto m=auto i=auto
HTMLLinkElement disabled b
HTMLMapElement name s
HTMLMenuElement compact b
HTMLMetaElement name s
HTMLMetaElement httpEquiv=http-equiv s
HTMLMetaElement content s
HTMLMetaElement media s
HTMLMetaElement scheme s
HTMLMarqueeElement bgColor=bgcolor s
HTMLMarqueeElement height s
HTMLMarqueeElement hspace ul 0
HTMLMarqueeElement scrollAmount=scrollamount ul 6
HTMLMarqueeElement scrollDelay=scrolldelay ul 85
HTMLMarqueeElement trueSpeed=truespeed b
HTMLMarqueeElement vspace ul 0
HTMLMarqueeElement width s
HTMLObjectElement data u
HTMLObjectElement type s
HTMLObjectElement name s
HTMLObjectElement useMap=usemap s
HTMLObjectElement width s
HTMLObjectElement height s
HTMLObjectElement align s
HTMLObjectElement archive s
HTMLObjectElement code s
HTMLObjectElement declare b
HTMLObjectElement hspace ul 0
HTMLObjectElement standby s
HTMLObjectElement vspace ul 0
HTMLObjectElement codeBase=codebase u
HTMLObjectElement codeType=codetype s
HTMLObjectElement border sn
HTMLOListElement reversed b
HTMLOListElement start l 1
HTMLOListElement type s
HTMLOListElement compact b
HTMLOptGroupElement disabled b
HTMLOptGroupElement label s
HTMLOptionElement disabled b
HTMLOptionElement defaultSelected=selected b
HTMLOutputElement name s
HTMLParagraphElement align s
HTMLParamElement name s
HTMLParamElement value s
HTMLParamElement type s
HTMLParamElement valueType=valuetype s
HTMLPreElement width l 0
HTMLProgressElement max ld 1
HTMLScriptElement src u
HTMLScriptElement type s
HTMLScriptElement noModule=nomodule b
HTMLScriptElement charset s
HTMLScriptElement defer b
HTMLScriptElement integrity s
HTMLScriptElement event s
HTMLScriptElement htmlFor=for s
HTMLScriptElement fetchPriority=fetchpriority e high,low,auto m=auto i=auto
HTMLSelectElement disabled b
HTMLSelectElement multiple b
HTMLSelectElement name s
HTMLSelectElement required b
HTMLSelectElement size ul 0
HTMLSlotElement name s
HTMLSourceElement src u
HTMLSourceElement type s
HTMLSourceElement srcset s
HTMLSourceElement sizes s
HTMLSourceElement media s
HTMLSourceElement width ul 0
HTMLSourceElement height ul 0
HTMLStyleElement media s
HTMLStyleElement nonce s
HTMLStyleElement type s
HTMLTableElement align s
HTMLTableElement border s
HTMLTableElement frame s
HTMLTableElement rules s
HTMLTableElement summary s
HTMLTableElement width s
HTMLTableElement bgColor=bgcolor sn
HTMLTableElement cellPadding=cellpadding sn
HTMLTableElement cellSpacing=cellspacing sn
HTMLTableSectionElement align s
HTMLTableSectionElement ch=char s
HTMLTableSectionElement chOff=charoff s
HTMLTableSectionElement vAlign=valign s
HTMLTableRowElement align s
HTMLTableRowElement ch=char s
HTMLTableRowElement chOff=charoff s
HTMLTableRowElement vAlign=valign s
HTMLTableRowElement bgColor=bgcolor sn
HTMLTableCellElement colSpan=colspan cu 1 1000 1
HTMLTableCellElement rowSpan=rowspan cu 0 65534 1
HTMLTableCellElement headers s
HTMLTableCellElement scope e row,col,rowgroup,colgroup
HTMLTableCellElement abbr s
HTMLTableCellElement align s
HTMLTableCellElement axis s
HTMLTableCellElement height s
HTMLTableCellElement width s
HTMLTableCellElement ch=char s
HTMLTableCellElement chOff=charoff s
HTMLTableCellElement noWrap=nowrap b
HTMLTableCellElement vAlign=valign s
HTMLTableCellElement bgColor=bgcolor sn
HTMLTextAreaElement cols luf 20
HTMLTextAreaElement dirName=dirname s
HTMLTextAreaElement disabled b
HTMLTextAreaElement maxLength=maxlength ll
HTMLTextAreaElement minLength=minlength ll
HTMLTextAreaElement name s
HTMLTextAreaElement placeholder s
HTMLTextAreaElement readOnly=readonly b
HTMLTextAreaElement required b
HTMLTextAreaElement rows luf 2
HTMLTextAreaElement wrap s
HTMLTimeElement dateTime=datetime s
HTMLTrackElement kind e subtitles,captions,descriptions,chapters,metadata m=subtitles i=metadata
HTMLTrackElement src u
HTMLTrackElement srclang s
HTMLTrackElement label s
HTMLTrackElement default b
HTMLUListElement compact b
HTMLUListElement type s
HTMLMediaElement src u
HTMLMediaElement preload e none,metadata,auto m=metadata i=metadata a=>auto
HTMLMediaElement autoplay b
HTMLMediaElement loop b
HTMLMediaElement controls b
HTMLMediaElement defaultMuted=muted b
HTMLMediaElement loading e lazy,eager m=eager i=eager
HTMLVideoElement width ul 0
HTMLVideoElement height ul 0
HTMLVideoElement poster u
HTMLVideoElement playsInline=playsinline b
",
    );
    for i in ["HTMLImageElement", "HTMLLinkElement", "HTMLScriptElement", "HTMLMediaElement"] {
        s.push_str(&format!("{i} crossOrigin=crossorigin ne anonymous,use-credentials m=null i=anonymous a=>anonymous\n"));
    }
    for i in ["HTMLImageElement", "HTMLIFrameElement", "HTMLAnchorElement", "HTMLAreaElement", "HTMLLinkElement", "HTMLScriptElement"] {
        s.push_str(&format!("{i} referrerPolicy=referrerpolicy e {REFERRER}\n"));
    }
    s.push_str(&format!("HTMLFormElement enctype e {ENCTYPES} m=application/x-www-form-urlencoded\n"));
    s.push_str(&format!("HTMLFormElement encoding=enctype e {ENCTYPES} m=application/x-www-form-urlencoded\n"));
    s.push_str("HTMLFormElement method e get,post,dialog m=get\n");
    s.push_str("HTMLFormElement autocomplete e on,off m=on\n");
    s.push_str(&format!("HTMLInputElement formEnctype=formenctype e {ENCTYPES} m= i=application/x-www-form-urlencoded\n"));
    s.push_str("HTMLInputElement formMethod=formmethod e get,post,dialog m= i=get\n");
    s.push_str(&format!("HTMLButtonElement formEnctype=formenctype e {ENCTYPES} m= i=application/x-www-form-urlencoded\n"));
    s.push_str("HTMLButtonElement formMethod=formmethod e get,post,dialog m= i=get\n");
    // ARIA reflection (ARIAMixin) — nullable DOMString reflection on Element.
    for (idl, attr) in [
        ("ariaAtomic", "aria-atomic"),
        ("ariaAutoComplete", "aria-autocomplete"),
        ("ariaBusy", "aria-busy"),
        ("ariaChecked", "aria-checked"),
        ("ariaColCount", "aria-colcount"),
        ("ariaColIndex", "aria-colindex"),
        ("ariaColSpan", "aria-colspan"),
        ("ariaCurrent", "aria-current"),
        ("ariaDescription", "aria-description"),
        ("ariaDisabled", "aria-disabled"),
        ("ariaExpanded", "aria-expanded"),
        ("ariaHasPopup", "aria-haspopup"),
        ("ariaHidden", "aria-hidden"),
        ("ariaInvalid", "aria-invalid"),
        ("ariaKeyShortcuts", "aria-keyshortcuts"),
        ("ariaLabel", "aria-label"),
        ("ariaLevel", "aria-level"),
        ("ariaLive", "aria-live"),
        ("ariaModal", "aria-modal"),
        ("ariaMultiLine", "aria-multiline"),
        ("ariaMultiSelectable", "aria-multiselectable"),
        ("ariaOrientation", "aria-orientation"),
        ("ariaPlaceholder", "aria-placeholder"),
        ("ariaPosInSet", "aria-posinset"),
        ("ariaPressed", "aria-pressed"),
        ("ariaReadOnly", "aria-readonly"),
        ("ariaRelevant", "aria-relevant"),
        ("ariaRequired", "aria-required"),
        ("ariaRoleDescription", "aria-roledescription"),
        ("ariaRowCount", "aria-rowcount"),
        ("ariaRowIndex", "aria-rowindex"),
        ("ariaRowSpan", "aria-rowspan"),
        ("ariaSelected", "aria-selected"),
        ("ariaSetSize", "aria-setsize"),
        ("ariaSort", "aria-sort"),
        ("ariaValueMax", "aria-valuemax"),
        ("ariaValueMin", "aria-valuemin"),
        ("ariaValueNow", "aria-valuenow"),
        ("ariaValueText", "aria-valuetext"),
        ("ariaBrailleLabel", "aria-braillelabel"),
        ("ariaBrailleRoleDescription", "aria-brailleroledescription"),
        ("role", "role"),
    ] {
        let e = match idl {
            "ariaAtomic" => "true,false m=null i=false a=>false",
            "ariaAutoComplete" => "inline,list,both,none m=none i=none",
            "ariaBusy" | "ariaDisabled" | "ariaHidden" | "ariaModal" | "ariaMultiLine" | "ariaMultiSelectable" | "ariaReadOnly"
            | "ariaRequired" => "true,false m=false i=false a=>false",
            "ariaChecked" | "ariaPressed" => "true,false,mixed m=null i=null a=>null",
            "ariaCurrent" => "page,step,location,date,time,true,false m=false i=true a=>false",
            "ariaExpanded" | "ariaSelected" => "true,false m=null i=null a=>null",
            "ariaHasPopup" => "true,false,menu,dialog,listbox,tree,grid m=null i=false",
            "ariaInvalid" => "true,false,spelling,grammar m=false i=true a=>false",
            "ariaLive" => "polite,assertive,off m=off i=off",
            "ariaOrientation" => "horizontal,vertical m=null i=null a=>null",
            "ariaSort" => "ascending,descending,other,none m=none i=none",
            _ => "",
        };
        if e.is_empty() {
            s.push_str(&format!("Element {idl}={attr} sq\n"));
        } else {
            s.push_str(&format!("Element {idl}={attr} ne {e}\n"));
        }
    }
    s
}

fn parse_table() -> Vec<R> {
    let mut out = Vec::new();
    for line in spec().lines() {
        let mut w = line.split(' ');
        let (Some(iface), Some(name), Some(kind)) = (w.next(), w.next(), w.next()) else { continue };
        let (idl, attr) = match name.split_once('=') {
            Some((i, a)) => (i.to_string(), a.to_string()),
            None => (name.to_string(), name.to_ascii_lowercase()),
        };
        let rest: Vec<&str> = w.collect();
        let n = |i: usize, d: f64| rest.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(d);
        let k = match kind {
            "s" => K::Str,
            "sn" => K::StrNull,
            "sq" => K::Enum(Vec::new(), None, None, true, Vec::new()),
            "u" => K::Url,
            "action" => K::Action,
            "b" => K::Bool,
            "hidden" => K::Hidden,
            "tabindex" => K::TabIndex,
            "l" => K::Long(n(0, 0.0) as i64),
            "ll" => K::LimLong(-1),
            "ul" => K::ULong(n(0, 0.0) as i64),
            "lu" => K::LimULong(n(0, 1.0) as i64),
            "luf" => K::LimULongFb(n(0, 1.0) as i64),
            "cu" => K::Clamped(n(0, 0.0) as i64, n(1, 0.0) as i64, n(2, 0.0) as i64),
            "d" => K::Double(n(0, 0.0)),
            "ld" => K::LimDouble(n(0, 0.0)),
            "e" | "ne" => {
                let kws: Vec<String> = rest.first().map(|k| k.split(',').map(String::from).collect()).unwrap_or_default();
                let mut missing = Some(String::new());
                let mut invalid: Option<Option<String>> = None;
                let mut aliases = Vec::new();
                let val = |v: &str| if v == "null" { None } else { Some(v.to_string()) };
                for opt in rest.iter().skip(1) {
                    if let Some(v) = opt.strip_prefix("m=") {
                        missing = val(v);
                    } else if let Some(v) = opt.strip_prefix("i=") {
                        invalid = Some(val(v));
                    } else if let Some(v) = opt.strip_prefix("a=") {
                        let (from, to) = v.split_once('>').unwrap_or((v, ""));
                        aliases.push((from.to_string(), val(to)));
                    }
                }
                let invalid = invalid.unwrap_or_else(|| missing.clone());
                K::Enum(kws, missing, invalid, kind == "ne", aliases)
            }
            _ => continue,
        };
        out.push(R { iface: iface.to_string(), idl, attr, k });
    }
    out
}

/// "rules for parsing integers" (HTML §2.3.4.1); None on error.
pub fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start_matches([' ', '\t', '\n', '\x0C', '\r']);
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: &str = &rest[..rest.bytes().take_while(|c| c.is_ascii_digit()).count()];
    if digits.is_empty() {
        return None;
    }
    let mut v: i64 = 0;
    for c in digits.bytes() {
        v = v.saturating_mul(10).saturating_add((c - b'0') as i64);
        if v > 1 << 40 {
            v = 1 << 40;
        }
    }
    Some(if neg { -v } else { v })
}

fn parse_nonneg(s: &str) -> Option<i64> {
    parse_int(s).filter(|v| *v >= 0)
}

/// "rules for parsing floating-point number values" (HTML §2.3.4.3); None on error.
pub fn parse_float(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | 0x0C | b'\r') {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let mut sign = 1.0;
    if b[i] == b'-' {
        sign = -1.0;
        i += 1;
    } else if b[i] == b'+' {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let start = i;
    let mut txt = String::new();
    if b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
        txt.push('0');
    } else if !b[i].is_ascii_digit() {
        return None;
    } else {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        txt.push_str(&s[start..i]);
    }
    if i < b.len() && b[i] == b'.' {
        let f0 = i;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i > f0 + 1 {
            txt.push_str(&s[f0..i]);
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        let mut exp = String::from("e");
        if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
            exp.push(b[j] as char);
            j += 1;
        }
        let d0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > d0 {
            exp.push_str(&s[d0..j]);
            txt.push_str(&exp);
        }
    }
    let v: f64 = txt.parse().ok()?;
    let v = sign * v;
    if !v.is_finite() {
        return None;
    }
    Some(if v == 0.0 { 0.0 } else { v })
}

fn doc_url_of(d: usize) -> String {
    let u = super::page(|p| p.docs.get(&d).map(|m| m.url.clone())).unwrap_or_else(super::page_url);
    if u.is_empty() { "about:blank".into() } else { u }
}

fn entry(vm: &Vm, ctx: &CallCtx) -> usize {
    match vm.native_slot(ctx.callee, 0) {
        Value::Number(n) => n as usize,
        _ => 0,
    }
}

fn with_entry<T>(i: usize, f: impl FnOnce(&R) -> T) -> T {
    TABLE.with(|t| f(&t.borrow()[i]))
}

fn enum_state(kws: &[String], missing: &Option<String>, invalid: &Option<String>, aliases: &[(String, Option<String>)], v: Option<String>) -> Option<String> {
    let Some(v) = v else { return missing.clone() };
    if kws.is_empty() {
        return Some(v);
    }
    for (from, to) in aliases {
        if v.eq_ignore_ascii_case(from) {
            return to.clone();
        }
    }
    for k in kws {
        if v.eq_ignore_ascii_case(k) {
            return Some(k.clone());
        }
    }
    invalid.clone()
}

/// The default tabIndex: 0 for elements focusable by default, else -1.
fn default_tab_index(el: usize) -> i64 {
    let tag = dom::with_doc(|d| dom::local_name_in(d, el));
    let focusable = match tag.as_str() {
        "a" | "area" => attr_value(el, "href").is_some(),
        "button" | "input" | "select" | "textarea" | "iframe" | "frame" | "object" | "embed" => true,
        "summary" => true,
        _ => false,
    };
    if focusable || attr_value(el, "contenteditable").is_some() {
        0
    } else {
        -1
    }
}

fn reflect_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let i = entry(vm, ctx);
    let (attr, k) = with_entry(i, |r| (r.attr.clone(), r.k.clone()));
    let v = attr_value(el, &attr);
    Ok(match k {
        K::Str | K::StrNull => s(&v.unwrap_or_default()),
        K::Bool => Value::Bool(v.is_some()),
        K::Hidden => match v {
            Some(x) if x.eq_ignore_ascii_case("until-found") => s("until-found"),
            x => Value::Bool(x.is_some()),
        },
        K::Url => match v {
            Some(x) => s(&dom::resolve_url(dom::node_document(el), &x).unwrap_or(x)),
            None => s(""),
        },
        K::Action => match v.filter(|x| !x.is_empty()) {
            Some(x) => s(&dom::resolve_url(dom::node_document(el), &x).unwrap_or(x)),
            None => s(&doc_url_of(dom::node_document(el))),
        },
        K::Long(d) => num(v.and_then(|x| parse_int(&x)).filter(|n| *n >= i32::MIN as i64 && *n <= MAX_INT).unwrap_or(d) as f64),
        K::TabIndex => num(v.and_then(|x| parse_int(&x)).filter(|n| *n >= i32::MIN as i64 && *n <= MAX_INT).unwrap_or_else(|| default_tab_index(el)) as f64),
        K::LimLong(d) => num(v.and_then(|x| parse_nonneg(&x)).filter(|n| *n <= MAX_INT).unwrap_or(d) as f64),
        K::ULong(d) => num(v.and_then(|x| parse_nonneg(&x)).filter(|n| *n <= MAX_INT).unwrap_or(d) as f64),
        K::LimULong(d) | K::LimULongFb(d) => num(v.and_then(|x| parse_nonneg(&x)).filter(|n| *n >= 1 && *n <= MAX_INT).unwrap_or(d) as f64),
        K::Clamped(min, max, d) => num(match v.and_then(|x| parse_nonneg(&x)) {
            None => d,
            Some(n) => n.clamp(min, max),
        } as f64),
        K::Double(d) => num(v.and_then(|x| parse_float(&x)).unwrap_or(d)),
        K::LimDouble(d) => num(v.and_then(|x| parse_float(&x)).filter(|n| *n > 0.0).unwrap_or(d)),
        K::Enum(kws, missing, invalid, _, aliases) => match enum_state(&kws, &missing, &invalid, &aliases, v) {
            Some(x) => s(&x),
            None => Value::Null,
        },
    })
}

fn set_num(vm: &mut Vm, el: usize, attr: &str, n: i64) {
    set_attr(vm, el, attr, &n.to_string());
}

fn reflect_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let i = entry(vm, ctx);
    let (attr, k) = with_entry(i, |r| (r.attr.clone(), r.k.clone()));
    let v = arg(vm, ctx, 0);
    match k {
        K::Str | K::Url | K::Action => {
            let x = string(vm, &v)?;
            set_attr(vm, el, &attr, &x);
        }
        K::StrNull => {
            let x = string_null_empty(vm, &v)?;
            set_attr(vm, el, &attr, &x);
        }
        K::Bool => {
            if vm.to_boolean(&v) {
                set_attr(vm, el, &attr, "");
            } else {
                remove_attr(vm, el, &attr);
            }
        }
        K::Hidden => {
            if let Value::String(x) = &v {
                if x.to_rust().eq_ignore_ascii_case("until-found") {
                    set_attr(vm, el, &attr, "until-found");
                    return Ok(Value::Undefined);
                }
            }
            if vm.to_boolean(&v) {
                set_attr(vm, el, &attr, "");
            } else {
                remove_attr(vm, el, &attr);
            }
        }
        K::Long(_) | K::TabIndex => {
            let n = vm.to_int32(&v)?;
            set_num(vm, el, &attr, n as i64);
        }
        K::LimLong(_) => {
            let n = vm.to_int32(&v)?;
            if n < 0 {
                return throw_dom(vm, "IndexSizeError", "The value provided is negative.");
            }
            set_num(vm, el, &attr, n as i64);
        }
        K::ULong(d) | K::Clamped(_, _, d) => {
            let n = vm.to_uint32(&v)? as i64;
            set_num(vm, el, &attr, if n <= MAX_INT { n } else { d });
        }
        K::LimULong(d) => {
            let n = vm.to_uint32(&v)? as i64;
            if n == 0 {
                return throw_dom(vm, "IndexSizeError", "The value provided is 0, which is an invalid size.");
            }
            set_num(vm, el, &attr, if n <= MAX_INT { n } else { d });
        }
        K::LimULongFb(d) => {
            let n = vm.to_uint32(&v)? as i64;
            set_num(vm, el, &attr, if (1..=MAX_INT).contains(&n) { n } else { d });
        }
        K::Double(_) | K::LimDouble(_) => {
            let n = vm.to_number(&v)?;
            if !n.is_finite() {
                return vm.throw_type("The provided double value is non-finite.");
            }
            if matches!(k, K::LimDouble(_)) && n <= 0.0 {
                return Ok(Value::Undefined);
            }
            let t = vm.to_string(&Value::Number(n))?.to_rust();
            set_attr(vm, el, &attr, &t);
        }
        K::Enum(_, _, _, nullable, _) => {
            if nullable && v.is_nullish() {
                remove_attr(vm, el, &attr);
            } else {
                let x = string(vm, &v)?;
                set_attr(vm, el, &attr, &x);
            }
        }
    }
    Ok(Value::Undefined)
}

// ------------------------------------------------------------------------------------------------ Document color/dir reflection

fn doc_reflect_target(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Option<usize>> {
    let Some(d) = dom::node_of(vm, &ctx.this).filter(|n| dom::kind(*n) == dom::NK::Document) else {
        return vm.throw_type("Illegal invocation");
    };
    let which = callee_str(vm, ctx);
    Ok(if which == "dir" { dom::document_element_of(d) } else { dom::body_of(d).filter(|b| dom::with_doc(|doc| dom::is_html_tag(doc, *b, "body"))) })
}

fn doc_attr_name(which: &str) -> &'static str {
    match which {
        "dir" => "dir",
        "fgColor" => "text",
        "linkColor" => "link",
        "vlinkColor" => "vlink",
        "alinkColor" => "alink",
        _ => "bgcolor",
    }
}

fn doc_reflect_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let which = callee_str(vm, ctx);
    let Some(el) = doc_reflect_target(vm, ctx)? else { return Ok(s("")) };
    let v = attr_value(el, doc_attr_name(&which)).unwrap_or_default();
    if which == "dir" {
        let l = v.to_ascii_lowercase();
        return Ok(s(if matches!(l.as_str(), "ltr" | "rtl" | "auto") { &l } else { "" }));
    }
    Ok(s(&v))
}

fn doc_reflect_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let which = callee_str(vm, ctx);
    let v = arg(vm, ctx, 0);
    let x = if which == "dir" { string(vm, &v)? } else { string_null_empty(vm, &v)? };
    if let Some(el) = doc_reflect_target(vm, ctx)? {
        set_attr(vm, el, doc_attr_name(&which), &x);
    }
    Ok(Value::Undefined)
}

pub fn install(vm: &mut Vm) {
    let table = parse_table();
    for (i, r) in table.iter().enumerate() {
        let Some(iface) = dom::iface(&r.iface) else { continue };
        attr_with(vm, iface.proto, &r.idl, reflect_get, Some(reflect_set), num(i as f64));
    }
    TABLE.with(|t| *t.borrow_mut() = table);
    let dp = dom::proto("Document");
    for w in ["dir", "fgColor", "linkColor", "vlinkColor", "alinkColor", "bgColor"] {
        attr_with(vm, dp, w, doc_reflect_get, Some(doc_reflect_set), s(w));
    }
}
