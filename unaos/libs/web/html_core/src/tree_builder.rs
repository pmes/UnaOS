//! Tree construction — §13.2.6, written from the WHATWG specification (the relaxed-`<select>` parser: there is
//! no "in select" insertion mode, `select` is in the default scope list).
//!
//! Every insertion mode, the stack of open elements with its scope algorithms, the list of active formatting
//! elements (Noah's Ark, reconstruction, the adoption agency algorithm), foster parenting, foreign content
//! (SVG/MathML with the attribute/tag-name adjustments and integration points), template contents, and the
//! fragment case (§13.4 "Parsing HTML fragments"). The scripting flag is a parameter; UnaOS parses with scripting
//! *disabled* (no script engine in this core), which is also how the html5lib `#script-off` vectors and the
//! Chromium oracle run.
//!
//! Not implemented (no tree effect without them): form-owner association, custom elements, declarative shadow
//! roots (the parser's "allow declarative shadow roots" is false, so `<template shadowrootmode>` stays a
//! template), the 2026 `<template for>` content patching, speculative parsing, encoding sniffing (input is a
//! decoded `&str`).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::dom::{Attribute, Document, Namespace, NodeData, NodeId, QuirksMode};
use crate::tokenizer::{Attr, Doctype, State, Tag, Token, Tokenizer, TokenizerOpts};

/// Parser options.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseOpts {
    /// The scripting flag (§13.2.4.5). Off by default: `<noscript>` content is parsed as markup.
    pub scripting: bool,
    /// Tokenize processing instructions (2026 spec). Off by default.
    pub processing_instructions: bool,
    /// The document is an iframe `srcdoc` document (never quirks).
    pub iframe_srcdoc: bool,
}

/// The insertion modes (§13.2.4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

#[derive(Clone, Debug)]
enum Afe {
    Marker,
    Element(NodeId, Tag),
}

enum Res {
    Done,
    Reprocess(Token),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Default,
    ListItem,
    Button,
    Table,
}

fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

fn is_one_of(name: &str, list: &[&str]) -> bool {
    list.contains(&name)
}

const IMPLIED_END: &[&str] = &["dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc"];
const IMPLIED_END_THOROUGH: &[&str] = &[
    "caption", "colgroup", "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc", "tbody", "td",
    "tfoot", "th", "thead", "tr",
];
const SPECIAL_HTML: &[&str] = &[
    "address", "applet", "area", "article", "aside", "base", "basefont", "bgsound", "blockquote", "body", "br",
    "button", "caption", "center", "col", "colgroup", "dd", "details", "dir", "div", "dl", "dt", "embed",
    "fieldset", "figcaption", "figure", "footer", "form", "frame", "frameset", "h1", "h2", "h3", "h4", "h5", "h6",
    "head", "header", "hgroup", "hr", "html", "iframe", "img", "input", "keygen", "li", "link", "listing", "main",
    "marquee", "menu", "meta", "nav", "noembed", "noframes", "noscript", "object", "ol", "p", "param", "plaintext",
    "pre", "script", "search", "section", "select", "source", "style", "summary", "table", "tbody", "td",
    "template", "textarea", "tfoot", "th", "thead", "title", "tr", "track", "ul", "wbr", "xmp",
];
const HEADINGS: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6"];

const SVG_ATTRS: &[(&str, &str)] = &[
    ("attributename", "attributeName"),
    ("attributetype", "attributeType"),
    ("basefrequency", "baseFrequency"),
    ("baseprofile", "baseProfile"),
    ("calcmode", "calcMode"),
    ("clippathunits", "clipPathUnits"),
    ("diffuseconstant", "diffuseConstant"),
    ("edgemode", "edgeMode"),
    ("filterunits", "filterUnits"),
    ("glyphref", "glyphRef"),
    ("gradienttransform", "gradientTransform"),
    ("gradientunits", "gradientUnits"),
    ("kernelmatrix", "kernelMatrix"),
    ("kernelunitlength", "kernelUnitLength"),
    ("keypoints", "keyPoints"),
    ("keysplines", "keySplines"),
    ("keytimes", "keyTimes"),
    ("lengthadjust", "lengthAdjust"),
    ("limitingconeangle", "limitingConeAngle"),
    ("markerheight", "markerHeight"),
    ("markerunits", "markerUnits"),
    ("markerwidth", "markerWidth"),
    ("maskcontentunits", "maskContentUnits"),
    ("maskunits", "maskUnits"),
    ("numoctaves", "numOctaves"),
    ("pathlength", "pathLength"),
    ("patterncontentunits", "patternContentUnits"),
    ("patterntransform", "patternTransform"),
    ("patternunits", "patternUnits"),
    ("pointsatx", "pointsAtX"),
    ("pointsaty", "pointsAtY"),
    ("pointsatz", "pointsAtZ"),
    ("preservealpha", "preserveAlpha"),
    ("preserveaspectratio", "preserveAspectRatio"),
    ("primitiveunits", "primitiveUnits"),
    ("refx", "refX"),
    ("refy", "refY"),
    ("repeatcount", "repeatCount"),
    ("repeatdur", "repeatDur"),
    ("requiredextensions", "requiredExtensions"),
    ("requiredfeatures", "requiredFeatures"),
    ("specularconstant", "specularConstant"),
    ("specularexponent", "specularExponent"),
    ("spreadmethod", "spreadMethod"),
    ("startoffset", "startOffset"),
    ("stddeviation", "stdDeviation"),
    ("stitchtiles", "stitchTiles"),
    ("surfacescale", "surfaceScale"),
    ("systemlanguage", "systemLanguage"),
    ("tablevalues", "tableValues"),
    ("targetx", "targetX"),
    ("targety", "targetY"),
    ("textlength", "textLength"),
    ("viewbox", "viewBox"),
    ("viewtarget", "viewTarget"),
    ("xchannelselector", "xChannelSelector"),
    ("ychannelselector", "yChannelSelector"),
    ("zoomandpan", "zoomAndPan"),
];

const SVG_TAGS: &[(&str, &str)] = &[
    ("altglyph", "altGlyph"),
    ("altglyphdef", "altGlyphDef"),
    ("altglyphitem", "altGlyphItem"),
    ("animatecolor", "animateColor"),
    ("animatemotion", "animateMotion"),
    ("animatetransform", "animateTransform"),
    ("clippath", "clipPath"),
    ("feblend", "feBlend"),
    ("fecolormatrix", "feColorMatrix"),
    ("fecomponenttransfer", "feComponentTransfer"),
    ("fecomposite", "feComposite"),
    ("feconvolvematrix", "feConvolveMatrix"),
    ("fediffuselighting", "feDiffuseLighting"),
    ("fedisplacementmap", "feDisplacementMap"),
    ("fedistantlight", "feDistantLight"),
    ("fedropshadow", "feDropShadow"),
    ("feflood", "feFlood"),
    ("fefunca", "feFuncA"),
    ("fefuncb", "feFuncB"),
    ("fefuncg", "feFuncG"),
    ("fefuncr", "feFuncR"),
    ("fegaussianblur", "feGaussianBlur"),
    ("feimage", "feImage"),
    ("femerge", "feMerge"),
    ("femergenode", "feMergeNode"),
    ("femorphology", "feMorphology"),
    ("feoffset", "feOffset"),
    ("fepointlight", "fePointLight"),
    ("fespecularlighting", "feSpecularLighting"),
    ("fespotlight", "feSpotLight"),
    ("fetile", "feTile"),
    ("feturbulence", "feTurbulence"),
    ("foreignobject", "foreignObject"),
    ("glyphref", "glyphRef"),
    ("lineargradient", "linearGradient"),
    ("radialgradient", "radialGradient"),
    ("textpath", "textPath"),
];

/// "Adjust foreign attributes": (token name, prefix, local name, namespace).
const FOREIGN_ATTRS: &[(&str, Option<&str>, &str, Namespace)] = &[
    ("xlink:actuate", Some("xlink"), "actuate", Namespace::XLink),
    ("xlink:arcrole", Some("xlink"), "arcrole", Namespace::XLink),
    ("xlink:href", Some("xlink"), "href", Namespace::XLink),
    ("xlink:role", Some("xlink"), "role", Namespace::XLink),
    ("xlink:show", Some("xlink"), "show", Namespace::XLink),
    ("xlink:title", Some("xlink"), "title", Namespace::XLink),
    ("xlink:type", Some("xlink"), "type", Namespace::XLink),
    ("xml:lang", Some("xml"), "lang", Namespace::Xml),
    ("xml:space", Some("xml"), "space", Namespace::Xml),
    ("xmlns", None, "xmlns", Namespace::Xmlns),
    ("xmlns:xlink", Some("xmlns"), "xlink", Namespace::Xmlns),
];

const BREAKOUT: &[&str] = &[
    "b", "big", "blockquote", "body", "br", "center", "code", "dd", "div", "dl", "dt", "em", "embed", "h1", "h2",
    "h3", "h4", "h5", "h6", "head", "hr", "i", "img", "li", "listing", "menu", "meta", "nobr", "ol", "p", "pre",
    "ruby", "s", "small", "span", "strong", "strike", "sub", "sup", "table", "tt", "u", "ul", "var",
];

const QUIRKY_PUBLIC_PREFIXES: &[&str] = &[
    "+//silmaril//dtd html pro v0r11 19970101//",
    "-//as//dtd html 3.0 aswedit + extensions//",
    "-//advasoft ltd//dtd html 3.0 aswedit + extensions//",
    "-//ietf//dtd html 2.0 level 1//",
    "-//ietf//dtd html 2.0 level 2//",
    "-//ietf//dtd html 2.0 strict level 1//",
    "-//ietf//dtd html 2.0 strict level 2//",
    "-//ietf//dtd html 2.0 strict//",
    "-//ietf//dtd html 2.0//",
    "-//ietf//dtd html 2.1e//",
    "-//ietf//dtd html 3.0//",
    "-//ietf//dtd html 3.2 final//",
    "-//ietf//dtd html 3.2//",
    "-//ietf//dtd html 3//",
    "-//ietf//dtd html level 0//",
    "-//ietf//dtd html level 1//",
    "-//ietf//dtd html level 2//",
    "-//ietf//dtd html level 3//",
    "-//ietf//dtd html strict level 0//",
    "-//ietf//dtd html strict level 1//",
    "-//ietf//dtd html strict level 2//",
    "-//ietf//dtd html strict level 3//",
    "-//ietf//dtd html strict//",
    "-//ietf//dtd html//",
    "-//metrius//dtd metrius presentational//",
    "-//microsoft//dtd internet explorer 2.0 html strict//",
    "-//microsoft//dtd internet explorer 2.0 html//",
    "-//microsoft//dtd internet explorer 2.0 tables//",
    "-//microsoft//dtd internet explorer 3.0 html strict//",
    "-//microsoft//dtd internet explorer 3.0 html//",
    "-//microsoft//dtd internet explorer 3.0 tables//",
    "-//netscape comm. corp.//dtd html//",
    "-//netscape comm. corp.//dtd strict html//",
    "-//o'reilly and associates//dtd html 2.0//",
    "-//o'reilly and associates//dtd html extended 1.0//",
    "-//o'reilly and associates//dtd html extended relaxed 1.0//",
    "-//sq//dtd html 2.0 hotmetal + extensions//",
    "-//softquad software//dtd hotmetal pro 6.0::19990601::extensions to html 4.0//",
    "-//softquad//dtd hotmetal pro 4.0::19971010::extensions to html 4.0//",
    "-//spyglass//dtd html 2.0 extended//",
    "-//sun microsystems corp.//dtd hotjava html//",
    "-//sun microsystems corp.//dtd hotjava strict html//",
    "-//w3c//dtd html 3 1995-03-24//",
    "-//w3c//dtd html 3.2 draft//",
    "-//w3c//dtd html 3.2 final//",
    "-//w3c//dtd html 3.2//",
    "-//w3c//dtd html 3.2s draft//",
    "-//w3c//dtd html 4.0 frameset//",
    "-//w3c//dtd html 4.0 transitional//",
    "-//w3c//dtd html experimental 19960712//",
    "-//w3c//dtd html experimental 970421//",
    "-//w3c//dtd w3 html//",
    "-//w3o//dtd w3 html 3.0//",
    "-//webtechs//dtd mozilla html 2.0//",
    "-//webtechs//dtd mozilla html//",
];

/// The tree builder: owns the document being built.
pub struct TreeBuilder {
    pub doc: Document,
    open: Vec<NodeId>,
    afe: Vec<Afe>,
    head: Option<NodeId>,
    form: Option<NodeId>,
    mode: Mode,
    orig_mode: Mode,
    template_modes: Vec<Mode>,
    frameset_ok: bool,
    foster: bool,
    pending_table_text: Vec<char>,
    ignore_lf: bool,
    scripting: bool,
    iframe_srcdoc: bool,
    context: Option<NodeId>,
    root_target: Option<NodeId>,
    /// A tokenizer state switch requested by the last token (applied by the driver).
    pub tokenizer_state: Option<State>,
    /// `option` selectedness (§4.10.10) — needed because the parser's pops drive `selectedcontent` cloning.
    selectedness: BTreeSet<NodeId>,
    /// `option` → cached nearest ancestor `select`.
    option_select: BTreeMap<NodeId, NodeId>,
}

impl TreeBuilder {
    pub fn new(opts: ParseOpts) -> TreeBuilder {
        TreeBuilder {
            doc: Document::new(),
            open: Vec::new(),
            afe: Vec::new(),
            head: None,
            form: None,
            mode: Mode::Initial,
            orig_mode: Mode::Initial,
            template_modes: Vec::new(),
            frameset_ok: true,
            foster: false,
            pending_table_text: Vec::new(),
            ignore_lf: false,
            scripting: opts.scripting,
            iframe_srcdoc: opts.iframe_srcdoc,
            context: None,
            root_target: None,
            tokenizer_state: None,
            selectedness: BTreeSet::new(),
            option_select: BTreeMap::new(),
        }
    }

    /// Set up the fragment case (§13.4) for a context element already created in `self.doc`.
    /// Returns the DocumentFragment that receives the parsed nodes and the initial tokenizer state.
    pub fn init_fragment(&mut self, context: NodeId) -> (NodeId, State) {
        self.context = Some(context);
        let (ns, local) = {
            let e = self.doc.element(context).expect("context is an element");
            (e.ns, e.local.clone())
        };
        let state = if ns == Namespace::Html {
            match local.as_str() {
                "title" | "textarea" => State::Rcdata,
                "style" | "xmp" | "iframe" | "noembed" | "noframes" => State::Rawtext,
                "script" => State::ScriptData,
                "noscript" if self.scripting => State::Rawtext,
                "plaintext" => State::Plaintext,
                _ => State::Data,
            }
        } else {
            State::Data
        };
        let root = self.doc.create_element(Namespace::Html, "html", Vec::new());
        self.doc.append(Document::ROOT, root);
        self.open.push(root);
        let frag = self.doc.create(NodeData::DocumentFragment);
        self.root_target = Some(frag);
        if ns == Namespace::Html && local == "template" {
            self.template_modes.push(Mode::InTemplate);
        }
        self.reset_insertion_mode();
        // Form pointer: nearest form inclusive ancestor of the context.
        let mut cur = Some(context);
        while let Some(c) = cur {
            if self.is_html(c, "form") {
                self.form = Some(c);
                break;
            }
            cur = self.doc.parent(c);
        }
        (frag, state)
    }

    /// Feed every token of `tz` through the tree builder until EOF.
    pub fn run(&mut self, tz: &mut Tokenizer) {
        loop {
            tz.allow_cdata = self.adjusted_current().is_some_and(|n| self.ns(n) != Namespace::Html);
            let tok = tz.next_token();
            let eof = tok == Token::Eof;
            self.process_token(tok);
            if let Some(s) = self.tokenizer_state.take() {
                tz.state = s;
            }
            if eof {
                break;
            }
        }
        while self.pop().is_some() {}
    }

    // ---- small helpers -------------------------------------------------------------------------------------

    fn ns(&self, n: NodeId) -> Namespace {
        self.doc.element(n).map(|e| e.ns).unwrap_or(Namespace::None)
    }
    fn local(&self, n: NodeId) -> &str {
        self.doc.element(n).map(|e| e.local.as_str()).unwrap_or("")
    }
    fn is_html(&self, n: NodeId, name: &str) -> bool {
        self.doc.element(n).is_some_and(|e| e.ns == Namespace::Html && e.local == name)
    }
    fn is_html_one_of(&self, n: NodeId, names: &[&str]) -> bool {
        self.doc.element(n).is_some_and(|e| e.ns == Namespace::Html && is_one_of(&e.local, names))
    }
    fn current(&self) -> NodeId {
        *self.open.last().expect("stack of open elements is not empty")
    }
    fn adjusted_current(&self) -> Option<NodeId> {
        if self.context.is_some() && self.open.len() == 1 {
            self.context
        } else {
            self.open.last().copied()
        }
    }
    fn is_special(&self, n: NodeId) -> bool {
        let Some(e) = self.doc.element(n) else { return false };
        match e.ns {
            Namespace::Html => is_one_of(&e.local, SPECIAL_HTML),
            Namespace::MathMl => is_one_of(&e.local, &["mi", "mo", "mn", "ms", "mtext", "annotation-xml"]),
            Namespace::Svg => is_one_of(&e.local, &["foreignObject", "desc", "title"]),
            _ => false,
        }
    }
    fn is_mathml_text_ip(&self, n: NodeId) -> bool {
        self.doc
            .element(n)
            .is_some_and(|e| e.ns == Namespace::MathMl && is_one_of(&e.local, &["mi", "mo", "mn", "ms", "mtext"]))
    }
    fn is_html_ip(&self, n: NodeId) -> bool {
        self.doc.element(n).is_some_and(|e| match e.ns {
            Namespace::MathMl => e.local == "annotation-xml" && e.html_integration_point,
            Namespace::Svg => is_one_of(&e.local, &["foreignObject", "desc", "title"]),
            _ => false,
        })
    }
    fn template_in_stack(&self) -> bool {
        self.open.iter().any(|&n| self.is_html(n, "template"))
    }

    fn scope_boundary(&self, n: NodeId, scope: Scope) -> bool {
        let Some(e) = self.doc.element(n) else { return false };
        if scope == Scope::Table {
            return e.ns == Namespace::Html && is_one_of(&e.local, &["html", "table", "template"]);
        }
        let base = match e.ns {
            Namespace::Html => is_one_of(
                &e.local,
                &["applet", "caption", "html", "table", "td", "th", "marquee", "object", "select", "template"],
            ),
            Namespace::MathMl => is_one_of(&e.local, &["mi", "mo", "mn", "ms", "mtext", "annotation-xml"]),
            Namespace::Svg => is_one_of(&e.local, &["foreignObject", "desc", "title"]),
            _ => false,
        };
        base || match scope {
            Scope::ListItem => e.ns == Namespace::Html && (e.local == "ol" || e.local == "ul"),
            Scope::Button => e.ns == Namespace::Html && e.local == "button",
            _ => false,
        }
    }

    /// "has an element in [scope]" for an HTML element named one of `names`.
    fn in_scope_any(&self, names: &[&str], scope: Scope) -> bool {
        for &n in self.open.iter().rev() {
            if self.is_html_one_of(n, names) {
                return true;
            }
            if self.scope_boundary(n, scope) {
                return false;
            }
        }
        false
    }
    fn in_scope(&self, name: &str, scope: Scope) -> bool {
        self.in_scope_any(&[name], scope)
    }
    fn node_in_scope(&self, target: NodeId) -> bool {
        for &n in self.open.iter().rev() {
            if n == target {
                return true;
            }
            if self.scope_boundary(n, Scope::Default) {
                return false;
            }
        }
        false
    }

    fn pop(&mut self) -> Option<NodeId> {
        let n = self.open.pop()?;
        self.popped(n);
        Some(n)
    }
    fn truncate(&mut self, len: usize) {
        while self.open.len() > len {
            self.pop();
        }
    }
    fn pop_until_html(&mut self, names: &[&str]) {
        while let Some(n) = self.pop() {
            if self.is_html_one_of(n, names) {
                break;
            }
        }
    }
    fn remove_from_stack(&mut self, n: NodeId) {
        if let Some(i) = self.open.iter().rposition(|&x| x == n) {
            self.open.remove(i);
            self.popped(n);
        }
    }

    fn generate_implied_end_tags(&mut self, except: Option<&str>) {
        while let Some(&n) = self.open.last() {
            let e = match self.doc.element(n) {
                Some(e) if e.ns == Namespace::Html => e,
                _ => break,
            };
            if is_one_of(&e.local, IMPLIED_END) && Some(e.local.as_str()) != except {
                self.pop();
            } else {
                break;
            }
        }
    }
    fn generate_all_implied_end_tags_thoroughly(&mut self) {
        while let Some(&n) = self.open.last() {
            if self.is_html_one_of(n, IMPLIED_END_THOROUGH) {
                self.pop();
            } else {
                break;
            }
        }
    }
    fn close_p(&mut self) {
        self.generate_implied_end_tags(Some("p"));
        self.pop_until_html(&["p"]);
    }
    fn close_p_if_in_button_scope(&mut self) {
        if self.in_scope("p", Scope::Button) {
            self.close_p();
        }
    }

    // ---- node creation and insertion (§13.2.6.1) ------------------------------------------------------------

    fn appropriate_place(&self, override_target: Option<NodeId>) -> (NodeId, Option<NodeId>) {
        let mut target = override_target.unwrap_or_else(|| self.current());
        let mut reference = None;
        if self.foster && self.is_html_one_of(target, &["table", "tbody", "tfoot", "thead", "tr"]) {
            let last_template = self.open.iter().rposition(|&n| self.is_html(n, "template"));
            let last_table = self.open.iter().rposition(|&n| self.is_html(n, "table"));
            match (last_template, last_table) {
                (Some(tp), tb) if tb.is_none_or(|tb| tp > tb) => target = self.open[tp],
                (_, None) => return (self.open[0], None),
                (_, Some(tb)) => {
                    let table = self.open[tb];
                    match self.doc.parent(table) {
                        Some(p) => {
                            target = p;
                            reference = Some(table);
                        }
                        None => target = self.open[tb - 1],
                    }
                }
            }
        }
        if let Some(e) = self.doc.element(target)
            && let Some(contents) = e.template_contents {
                return (contents, None);
            }
        (target, reference)
    }

    fn adjusted_location(&self, override_target: Option<NodeId>) -> (NodeId, Option<NodeId>) {
        let loc = self.appropriate_place(override_target);
        if let Some(rt) = self.root_target
            && self.open.first() == Some(&loc.0) {
                return (rt, None);
            }
        loc
    }

    fn convert_attrs(&self, attrs: &[Attr], ns: Namespace) -> Vec<Attribute> {
        attrs
            .iter()
            .map(|a| {
                let mut local = a.name.clone();
                if ns == Namespace::MathMl && local == "definitionurl" {
                    local = String::from("definitionURL");
                }
                if ns == Namespace::Svg
                    && let Some(&(_, fixed)) = SVG_ATTRS.iter().find(|(k, _)| *k == local) {
                        local = String::from(fixed);
                    }
                if (ns == Namespace::MathMl || ns == Namespace::Svg)
                    && let Some(&(_, prefix, l, ans)) = FOREIGN_ATTRS.iter().find(|(k, ..)| *k == a.name) {
                        return Attribute { prefix, ns: ans, local: String::from(l), value: a.value.clone() };
                    }
                Attribute { prefix: None, ns: Namespace::None, local, value: a.value.clone() }
            })
            .collect()
    }

    fn create_element_for(&mut self, tag: &Tag, ns: Namespace) -> NodeId {
        let attrs = self.convert_attrs(&tag.attrs, ns);
        let id = self.doc.create_element(ns, &tag.name, attrs);
        if ns == Namespace::Html && tag.name == "option" && tag.attr("selected").is_some() {
            self.selectedness.insert(id);
        }
        if ns == Namespace::MathMl && tag.name == "annotation-xml"
            && let Some(enc) = tag.attr("encoding") {
                let enc = enc.to_ascii_lowercase();
                if (enc == "text/html" || enc == "application/xhtml+xml")
                    && let Some(e) = self.doc.element_mut(id) {
                        e.html_integration_point = true;
                    }
            }
        id
    }

    fn insert_element_at(&mut self, el: NodeId, loc: (NodeId, Option<NodeId>)) {
        let (target, reference) = loc;
        if self.doc.parent(el).is_some() || self.doc.is_inclusive_ancestor(el, target) {
            return;
        }
        if matches!(self.doc.data(target), NodeData::Document) && self.doc.document_element().is_some() {
            return;
        }
        self.doc.insert_before(target, el, reference);
        self.inserted(el);
    }

    fn insert_foreign(&mut self, tag: &Tag, ns: Namespace, only_stack: bool) -> NodeId {
        let el = self.create_element_for(tag, ns);
        if !only_stack {
            let loc = self.adjusted_location(None);
            self.insert_element_at(el, loc);
        }
        self.open.push(el);
        el
    }

    fn insert_html(&mut self, tag: &Tag) -> NodeId {
        self.insert_foreign(tag, Namespace::Html, false)
    }

    fn insert_html_named(&mut self, name: &str) -> NodeId {
        self.insert_html(&Tag::new(name))
    }

    fn insert_char(&mut self, c: char) {
        let (target, reference) = self.adjusted_location(None);
        if matches!(self.doc.data(target), NodeData::Document) {
            return;
        }
        let prev = match reference {
            Some(r) => self.doc.prev_sibling(r),
            None => self.doc.last_child(target),
        };
        if let Some(p) = prev
            && let NodeData::Text(t) = &mut self.doc.node_mut(p).data {
                t.push(c);
                return;
            }
        let mut s = String::new();
        s.push(c);
        let t = self.doc.create(NodeData::Text(s));
        self.doc.insert_before(target, t, reference);
    }

    fn insert_comment(&mut self, data: String, override_target: Option<NodeId>) {
        let (target, reference) = self.adjusted_location(override_target);
        let c = self.doc.create(NodeData::Comment(data));
        self.doc.insert_before(target, c, reference);
    }

    fn insert_pi(&mut self, target_name: String, data: String, override_target: Option<NodeId>) {
        let (target, reference) = self.adjusted_location(override_target);
        let c = self.doc.create(NodeData::ProcessingInstruction { target: target_name, data });
        self.doc.insert_before(target, c, reference);
    }

    fn generic_text_element(&mut self, tag: &Tag, state: State) {
        self.insert_html(tag);
        self.tokenizer_state = Some(state);
        self.orig_mode = self.mode;
        self.mode = Mode::Text;
    }

    // ---- option / select / selectedcontent side effects (§4.10.7, §4.10.10, the selectedcontent element) ----
    //
    // Not tree construction proper, but the parser drives them: "When an option element is popped off the stack
    // of open elements of an HTML parser … update descendant selectedcontent elements for an option", plus the
    // option/selectedcontent insertion and post-connection steps. Moving/removing steps (microtask-queued) are
    // not modelled.

    fn nearest_ancestor_html(&self, n: NodeId, name: &str) -> Option<NodeId> {
        let mut cur = self.doc.parent(n);
        while let Some(c) = cur {
            if self.is_html(c, name) {
                return Some(c);
            }
            cur = self.doc.parent(c);
        }
        None
    }

    /// The list of options of a select (§4.10.7).
    fn list_of_options(&self, select: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut node = self.doc.first_child(select);
        while let Some(n) = node {
            if self.is_html(n, "option") {
                out.push(n);
            }
            let skip = self.is_html_one_of(n, &["select", "hr", "option", "datalist"])
                || (self.is_html(n, "optgroup") && {
                    let mut p = self.doc.parent(n);
                    let mut nested = false;
                    while let Some(x) = p {
                        if x == select {
                            break;
                        }
                        if self.is_html(x, "optgroup") {
                            nested = true;
                            break;
                        }
                        p = self.doc.parent(x);
                    }
                    nested
                });
            node = if !skip && self.doc.first_child(n).is_some() {
                self.doc.first_child(n)
            } else {
                // next in tree order excluding descendants, bounded by select
                let mut m = n;
                loop {
                    if m == select {
                        break None;
                    }
                    if let Some(sib) = self.doc.next_sibling(m) {
                        break Some(sib);
                    }
                    match self.doc.parent(m) {
                        Some(p) if p != select => m = p,
                        _ => break None,
                    }
                }
            };
        }
        out
    }

    fn option_disabled(&self, o: NodeId) -> bool {
        if self.doc.element(o).is_some_and(|e| e.attr("disabled").is_some()) {
            return true;
        }
        self.doc
            .parent(o)
            .is_some_and(|p| self.is_html(p, "optgroup") && self.doc.element(p).is_some_and(|e| e.attr("disabled").is_some()))
    }

    fn select_display_size_is_1(&self, select: NodeId) -> bool {
        let e = self.doc.element(select).expect("select");
        let multiple = e.attr("multiple").is_some();
        let parsed = e.attr("size").and_then(|v| {
            let v = v.trim_start_matches([' ', '\t', '\n', '\x0C', '\r']);
            let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() { None } else { Some(digits.parse::<u64>().unwrap_or(u64::MAX)) }
        });
        match parsed {
            Some(n) => n == 1,
            None => !multiple,
        }
    }

    /// The selectedness setting algorithm; returns hasChanged.
    fn selectedness_setting(&mut self, select: NodeId) -> bool {
        if self.doc.element(select).is_some_and(|e| e.attr("multiple").is_some()) {
            return false;
        }
        let mut changed = false;
        let mut first_enabled = None;
        let mut last_selected: Option<NodeId> = None;
        for o in self.list_of_options(select) {
            if self.selectedness.contains(&o) {
                if let Some(l) = last_selected {
                    self.selectedness.remove(&l);
                    changed = true;
                }
                last_selected = Some(o);
            }
            if first_enabled.is_none() && !self.option_disabled(o) {
                first_enabled = Some(o);
            }
        }
        if last_selected.is_none() && self.select_display_size_is_1(select)
            && let Some(f) = first_enabled {
                self.selectedness.insert(f);
                changed = true;
            }
        changed
    }

    /// Returns the nearest select if the selectedcontent is not disabled.
    fn selectedcontent_select(&self, sc: NodeId) -> Option<NodeId> {
        let mut nearest = None;
        let mut cur = self.doc.parent(sc);
        while let Some(a) = cur {
            if self.is_html(a, "select") {
                if nearest.is_none() {
                    nearest = Some(a);
                } else {
                    return None;
                }
            } else if self.is_html_one_of(a, &["option", "selectedcontent"]) {
                return None;
            }
            cur = self.doc.parent(a);
        }
        nearest
    }

    fn update_selectedcontent(&mut self, select: NodeId, sc: NodeId) {
        let option = self.list_of_options(select).into_iter().find(|o| self.selectedness.contains(o));
        while let Some(c) = self.doc.first_child(sc) {
            self.doc.detach(c);
        }
        if let Some(o) = option {
            let kids: Vec<NodeId> = self.doc.children(o).collect();
            for k in kids {
                let c = self.doc.clone_subtree(k);
                self.doc.append(sc, c);
            }
        }
    }

    fn update_select_selectedcontents(&mut self, select: NodeId) {
        if self.doc.element(select).is_some_and(|e| e.attr("multiple").is_some()) {
            return;
        }
        let scs: Vec<NodeId> = self
            .doc
            .descendants(select)
            .filter(|&n| self.is_html(n, "selectedcontent") && self.selectedcontent_select(n) == Some(select))
            .collect();
        for sc in scs {
            self.update_selectedcontent(select, sc);
        }
    }

    fn update_option_selectedcontents(&mut self, o: NodeId) {
        let Some(&select) = self.option_select.get(&o) else { return };
        if !self.selectedness.contains(&o) {
            return;
        }
        self.update_select_selectedcontents(select);
    }

    /// Insertion + post-connection steps for elements the parser inserts.
    fn inserted(&mut self, el: NodeId) {
        if self.is_html(el, "option") {
            let new = self.nearest_ancestor_html(el, "select");
            let old = self.option_select.get(&el).copied();
            match new {
                Some(s) => {
                    self.option_select.insert(el, s);
                }
                None => {
                    self.option_select.remove(&el);
                }
            }
            if old != new {
                for s in [old, new].into_iter().flatten() {
                    self.selectedness_setting(s);
                }
            }
            self.update_option_selectedcontents(el);
        } else if self.is_html(el, "selectedcontent")
            && let Some(select) = self.selectedcontent_select(el)
                && self.doc.element(select).is_some_and(|e| e.attr("multiple").is_none()) {
                    self.update_selectedcontent(select, el);
                }
    }

    /// An element was popped off (or removed from) the stack of open elements.
    fn popped(&mut self, n: NodeId) {
        if self.is_html(n, "option") {
            self.update_option_selectedcontents(n);
        }
    }

    // ---- active formatting elements ------------------------------------------------------------------------

    fn afe_index_of(&self, n: NodeId) -> Option<usize> {
        self.afe.iter().position(|e| matches!(e, Afe::Element(x, _) if *x == n))
    }

    fn push_afe(&mut self, el: NodeId, tag: &Tag) {
        let mut same = Vec::new();
        for (i, e) in self.afe.iter().enumerate().rev() {
            match e {
                Afe::Marker => break,
                Afe::Element(_, t) => {
                    if t.name == tag.name
                        && t.attrs.len() == tag.attrs.len()
                        && t.attrs.iter().all(|a| tag.attrs.iter().any(|b| b == a))
                    {
                        same.push(i);
                    }
                }
            }
        }
        if same.len() >= 3 {
            let earliest = *same.last().expect("non-empty");
            self.afe.remove(earliest);
        }
        self.afe.push(Afe::Element(el, tag.clone()));
    }

    fn reconstruct_afe(&mut self) {
        let Some(last) = self.afe.last() else { return };
        match last {
            Afe::Marker => return,
            Afe::Element(n, _) if self.open.contains(n) => return,
            _ => {}
        }
        let mut i = self.afe.len() - 1;
        while i > 0 {
            match &self.afe[i - 1] {
                Afe::Marker => break,
                Afe::Element(n, _) if self.open.contains(n) => break,
                _ => i -= 1,
            }
        }
        for j in i..self.afe.len() {
            let tag = match &self.afe[j] {
                Afe::Element(_, t) => t.clone(),
                Afe::Marker => continue,
            };
            let el = self.insert_html(&tag);
            self.afe[j] = Afe::Element(el, tag);
        }
    }

    fn clear_afe_to_marker(&mut self) {
        while let Some(e) = self.afe.pop() {
            if matches!(e, Afe::Marker) {
                break;
            }
        }
    }

    /// The adoption agency algorithm (§13.2.6.4.7).
    fn adoption_agency(&mut self, subject: &str) {
        let cur = self.current();
        if self.is_html(cur, subject) && self.afe_index_of(cur).is_none() {
            self.pop();
            return;
        }
        for _ in 0..8 {
            // formattingElement: last element in the AFE after the last marker with this tag name.
            let mut fe_idx = None;
            for (i, e) in self.afe.iter().enumerate().rev() {
                match e {
                    Afe::Marker => break,
                    Afe::Element(n, _) if self.local(*n) == subject => {
                        fe_idx = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(fe_idx) = fe_idx else {
                self.any_other_end_tag(subject);
                return;
            };
            let (fe, fe_tag) = match &self.afe[fe_idx] {
                Afe::Element(n, t) => (*n, t.clone()),
                Afe::Marker => unreachable!(),
            };
            let Some(fe_stack) = self.open.iter().position(|&n| n == fe) else {
                self.afe.remove(fe_idx);
                return;
            };
            if !self.node_in_scope(fe) {
                return;
            }
            let fb_stack = (fe_stack + 1..self.open.len()).find(|&i| self.is_special(self.open[i]));
            let Some(fb_stack) = fb_stack else {
                self.truncate(fe_stack);
                self.afe.remove(fe_idx);
                return;
            };
            let fb = self.open[fb_stack];
            let common_ancestor = self.open[fe_stack - 1];
            let mut bookmark = fe_idx;
            let mut node_idx = fb_stack;
            let mut last_node = fb;
            let mut inner = 0;
            loop {
                inner += 1;
                node_idx -= 1;
                let node = self.open[node_idx];
                if node == fe {
                    break;
                }
                let mut node_afe = self.afe_index_of(node);
                if inner > 3
                    && let Some(i) = node_afe {
                        self.afe.remove(i);
                        if i < bookmark {
                            bookmark -= 1;
                        }
                        node_afe = None;
                    }
                let Some(ai) = node_afe else {
                    let removed = self.open.remove(node_idx);
                    self.popped(removed);
                    continue;
                };
                let tag = match &self.afe[ai] {
                    Afe::Element(_, t) => t.clone(),
                    Afe::Marker => unreachable!(),
                };
                let new_el = self.create_element_for(&tag, Namespace::Html);
                self.afe[ai] = Afe::Element(new_el, tag);
                self.open[node_idx] = new_el;
                if last_node == fb {
                    bookmark = ai + 1;
                }
                self.doc.append(new_el, last_node);
                last_node = new_el;
            }
            let (target, reference) = self.adjusted_location(Some(common_ancestor));
            self.doc.detach(last_node);
            let target_is_doc_with_el =
                matches!(self.doc.data(target), NodeData::Document) && self.doc.document_element().is_some();
            if !self.doc.is_inclusive_ancestor(last_node, target)
                && !target_is_doc_with_el
                && reference.is_none_or(|r| self.doc.parent(r) == Some(target))
            {
                self.doc.insert_before(target, last_node, reference);
            }
            let new_el = self.create_element_for(&fe_tag, Namespace::Html);
            self.doc.reparent_children(fb, new_el);
            self.doc.append(fb, new_el);
            if let Some(p) = self.afe_index_of(fe) {
                self.afe.remove(p);
                if p < bookmark {
                    bookmark -= 1;
                }
            }
            let bookmark = bookmark.min(self.afe.len());
            self.afe.insert(bookmark, Afe::Element(new_el, fe_tag));
            self.remove_from_stack(fe);
            if let Some(p) = self.open.iter().position(|&n| n == fb) {
                self.open.insert(p + 1, new_el);
            }
        }
    }

    fn any_other_end_tag(&mut self, name: &str) {
        for i in (0..self.open.len()).rev() {
            let node = self.open[i];
            if self.is_html(node, name) {
                self.generate_implied_end_tags(Some(name));
                self.truncate(i);
                return;
            }
            if self.is_special(node) {
                return;
            }
        }
    }

    // ---- reset the insertion mode appropriately (§13.2.4.1) -------------------------------------------------

    fn reset_insertion_mode(&mut self) {
        let mut i = self.open.len();
        while i > 0 {
            i -= 1;
            let last = i == 0;
            let node = if last { self.context.unwrap_or(self.open[0]) } else { self.open[i] };
            let e = match self.doc.element(node) {
                Some(e) if e.ns == Namespace::Html => e.local.as_str(),
                _ => {
                    if last {
                        self.mode = Mode::InBody;
                        return;
                    }
                    continue;
                }
            };
            match e {
                "td" | "th" if !last => {
                    self.mode = Mode::InCell;
                    return;
                }
                "tr" => {
                    self.mode = Mode::InRow;
                    return;
                }
                "tbody" | "thead" | "tfoot" => {
                    self.mode = Mode::InTableBody;
                    return;
                }
                "caption" => {
                    self.mode = Mode::InCaption;
                    return;
                }
                "colgroup" => {
                    self.mode = Mode::InColumnGroup;
                    return;
                }
                "table" => {
                    self.mode = Mode::InTable;
                    return;
                }
                "template" => {
                    self.mode = *self.template_modes.last().unwrap_or(&Mode::InTemplate);
                    return;
                }
                "head" if !last => {
                    self.mode = Mode::InHead;
                    return;
                }
                "body" => {
                    self.mode = Mode::InBody;
                    return;
                }
                "frameset" => {
                    self.mode = Mode::InFrameset;
                    return;
                }
                "html" => {
                    self.mode = if self.head.is_none() { Mode::BeforeHead } else { Mode::AfterHead };
                    return;
                }
                _ => {}
            }
            if last {
                self.mode = Mode::InBody;
                return;
            }
        }
    }

    // ---- the dispatcher (§13.2.6) --------------------------------------------------------------------------

    /// Process one token.
    pub fn process_token(&mut self, tok: Token) {
        if self.ignore_lf {
            self.ignore_lf = false;
            if tok == Token::Character('\n') {
                return;
            }
        }
        let mut tok = tok;
        loop {
            let res = if self.use_html_rules(&tok) { self.step(self.mode, tok) } else { self.foreign(tok) };
            match res {
                Res::Done => return,
                Res::Reprocess(t) => tok = t,
            }
        }
    }

    fn use_html_rules(&self, tok: &Token) -> bool {
        let Some(acn) = self.adjusted_current() else { return true };
        if self.ns(acn) == Namespace::Html {
            return true;
        }
        let start = |names: &[&str]| matches!(tok, Token::StartTag(t) if !is_one_of(&t.name, names));
        if self.is_mathml_text_ip(acn) && (start(&["mglyph", "malignmark"]) || matches!(tok, Token::Character(_))) {
            return true;
        }
        if self.doc.element(acn).is_some_and(|e| e.is(Namespace::MathMl, "annotation-xml"))
            && matches!(tok, Token::StartTag(t) if t.name == "svg")
        {
            return true;
        }
        if self.is_html_ip(acn) && matches!(tok, Token::StartTag(_) | Token::Character(_)) {
            return true;
        }
        matches!(tok, Token::Eof)
    }

    fn step(&mut self, mode: Mode, tok: Token) -> Res {
        match mode {
            Mode::Initial => self.initial(tok),
            Mode::BeforeHtml => self.before_html(tok),
            Mode::BeforeHead => self.before_head(tok),
            Mode::InHead => self.in_head(tok),
            Mode::InHeadNoscript => self.in_head_noscript(tok),
            Mode::AfterHead => self.after_head(tok),
            Mode::InBody => self.in_body(tok),
            Mode::Text => self.text(tok),
            Mode::InTable => self.in_table(tok),
            Mode::InTableText => self.in_table_text(tok),
            Mode::InCaption => self.in_caption(tok),
            Mode::InColumnGroup => self.in_column_group(tok),
            Mode::InTableBody => self.in_table_body(tok),
            Mode::InRow => self.in_row(tok),
            Mode::InCell => self.in_cell(tok),
            Mode::InTemplate => self.in_template(tok),
            Mode::AfterBody => self.after_body(tok),
            Mode::InFrameset => self.in_frameset(tok),
            Mode::AfterFrameset => self.after_frameset(tok),
            Mode::AfterAfterBody => self.after_after_body(tok),
            Mode::AfterAfterFrameset => self.after_after_frameset(tok),
        }
    }

    // ---- §13.2.6.4.1 initial -------------------------------------------------------------------------------

    fn quirks_from_doctype(&self, d: &Doctype) -> QuirksMode {
        let name_html = d.name.as_deref() == Some("html");
        if d.force_quirks || !name_html {
            return QuirksMode::Quirks;
        }
        let public = d.public_id.as_deref().map(|s| s.to_ascii_lowercase());
        let system = d.system_id.as_deref().map(|s| s.to_ascii_lowercase());
        let p = public.as_deref();
        let s = system.as_deref();
        if matches!(p, Some("-//w3o//dtd w3 html strict 3.0//en//") | Some("-/w3c/dtd html 4.0 transitional/en") | Some("html"))
            || s == Some("http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd")
        {
            return QuirksMode::Quirks;
        }
        if let Some(p) = p {
            if QUIRKY_PUBLIC_PREFIXES.iter().any(|pre| p.starts_with(pre)) {
                return QuirksMode::Quirks;
            }
            let html401 = p.starts_with("-//w3c//dtd html 4.01 frameset//")
                || p.starts_with("-//w3c//dtd html 4.01 transitional//");
            if s.is_none() && html401 {
                return QuirksMode::Quirks;
            }
            if p.starts_with("-//w3c//dtd xhtml 1.0 frameset//") || p.starts_with("-//w3c//dtd xhtml 1.0 transitional//")
            {
                return QuirksMode::LimitedQuirks;
            }
            if s.is_some() && html401 {
                return QuirksMode::LimitedQuirks;
            }
        }
        QuirksMode::NoQuirks
    }

    fn initial(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => Res::Done,
            Token::Comment(d) => {
                self.insert_comment(d, Some(Document::ROOT));
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, Some(Document::ROOT));
                Res::Done
            }
            Token::Doctype(d) => {
                let node = self.doc.create(NodeData::Doctype {
                    name: d.name.clone().unwrap_or_default(),
                    public_id: d.public_id.clone().unwrap_or_default(),
                    system_id: d.system_id.clone().unwrap_or_default(),
                });
                let has = self.doc.children(Document::ROOT).any(|c| {
                    matches!(self.doc.data(c), NodeData::Doctype { .. } | NodeData::Element(_))
                });
                if !has {
                    self.doc.append(Document::ROOT, node);
                }
                if !self.iframe_srcdoc {
                    self.doc.quirks_mode = self.quirks_from_doctype(&d);
                }
                self.mode = Mode::BeforeHtml;
                Res::Done
            }
            t => {
                if !self.iframe_srcdoc {
                    self.doc.quirks_mode = QuirksMode::Quirks;
                }
                self.mode = Mode::BeforeHtml;
                Res::Reprocess(t)
            }
        }
    }

    // ---- §13.2.6.4.2 before html ---------------------------------------------------------------------------

    fn before_html(&mut self, tok: Token) -> Res {
        match tok {
            Token::Doctype(_) => Res::Done,
            Token::Comment(d) => {
                self.insert_comment(d, Some(Document::ROOT));
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, Some(Document::ROOT));
                Res::Done
            }
            Token::Character(c) if is_ws(c) => Res::Done,
            Token::StartTag(t) if t.name == "html" => {
                let el = self.create_element_for(&t, Namespace::Html);
                self.insert_element_at(el, (Document::ROOT, None));
                self.open.push(el);
                self.mode = Mode::BeforeHead;
                Res::Done
            }
            Token::EndTag(ref t) if !is_one_of(&t.name, &["head", "body", "html", "br"]) => Res::Done,
            t => {
                let el = self.doc.create_element(Namespace::Html, "html", Vec::new());
                self.doc.append(Document::ROOT, el);
                self.open.push(el);
                self.mode = Mode::BeforeHead;
                Res::Reprocess(t)
            }
        }
    }

    // ---- §13.2.6.4.3 before head ---------------------------------------------------------------------------

    fn before_head(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => Res::Done,
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::StartTag(t) if t.name == "head" => {
                let h = self.insert_html(&t);
                self.head = Some(h);
                self.mode = Mode::InHead;
                Res::Done
            }
            Token::EndTag(ref t) if !is_one_of(&t.name, &["head", "body", "html", "br"]) => Res::Done,
            t => {
                let h = self.insert_html_named("head");
                self.head = Some(h);
                self.mode = Mode::InHead;
                Res::Reprocess(t)
            }
        }
    }

    // ---- §13.2.6.4.4 in head -------------------------------------------------------------------------------

    fn in_head(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => {
                self.insert_char(c);
                Res::Done
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(t) => match t.name.as_str() {
                "html" => self.in_body(Token::StartTag(t)),
                "base" | "basefont" | "bgsound" | "link" | "meta" => {
                    self.insert_html(&t);
                    self.pop();
                    Res::Done
                }
                "title" => {
                    self.generic_text_element(&t, State::Rcdata);
                    Res::Done
                }
                "noscript" if self.scripting => {
                    self.generic_text_element(&t, State::Rawtext);
                    Res::Done
                }
                "noframes" | "style" => {
                    self.generic_text_element(&t, State::Rawtext);
                    Res::Done
                }
                "noscript" => {
                    self.insert_html(&t);
                    self.mode = Mode::InHeadNoscript;
                    Res::Done
                }
                "script" => {
                    let loc = self.adjusted_location(None);
                    let el = self.create_element_for(&t, Namespace::Html);
                    self.insert_element_at(el, loc);
                    self.open.push(el);
                    self.tokenizer_state = Some(State::ScriptData);
                    self.orig_mode = self.mode;
                    self.mode = Mode::Text;
                    Res::Done
                }
                "template" => {
                    self.afe.push(Afe::Marker);
                    self.frameset_ok = false;
                    self.mode = Mode::InTemplate;
                    self.template_modes.push(Mode::InTemplate);
                    self.insert_html(&t);
                    Res::Done
                }
                "head" => Res::Done,
                _ => {
                    self.pop();
                    self.mode = Mode::AfterHead;
                    Res::Reprocess(Token::StartTag(t))
                }
            },
            Token::EndTag(t) => match t.name.as_str() {
                "head" => {
                    self.pop();
                    self.mode = Mode::AfterHead;
                    Res::Done
                }
                "body" | "html" | "br" => {
                    self.pop();
                    self.mode = Mode::AfterHead;
                    Res::Reprocess(Token::EndTag(t))
                }
                "template" => {
                    self.end_template();
                    Res::Done
                }
                _ => Res::Done,
            },
            t => {
                self.pop();
                self.mode = Mode::AfterHead;
                Res::Reprocess(t)
            }
        }
    }

    fn end_template(&mut self) {
        if !self.template_in_stack() {
            return;
        }
        self.generate_all_implied_end_tags_thoroughly();
        self.pop_until_html(&["template"]);
        self.clear_afe_to_marker();
        self.template_modes.pop();
        self.reset_insertion_mode();
    }

    // ---- §13.2.6.4.5 in head noscript ----------------------------------------------------------------------

    fn in_head_noscript(&mut self, tok: Token) -> Res {
        match tok {
            Token::Doctype(_) => Res::Done,
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::EndTag(ref t) if t.name == "noscript" => {
                self.pop();
                self.mode = Mode::InHead;
                Res::Done
            }
            Token::Character(c) if is_ws(c) => self.in_head(tok),
            Token::Comment(_) | Token::ProcessingInstruction { .. } => self.in_head(tok),
            Token::StartTag(ref t)
                if is_one_of(&t.name, &["basefont", "bgsound", "link", "meta", "noframes", "style"]) =>
            {
                self.in_head(tok)
            }
            Token::StartTag(ref t) if t.name == "head" || t.name == "noscript" => Res::Done,
            Token::EndTag(ref t) if t.name != "br" => Res::Done,
            t => {
                self.pop();
                self.mode = Mode::InHead;
                Res::Reprocess(t)
            }
        }
    }

    // ---- §13.2.6.4.6 after head ----------------------------------------------------------------------------

    fn after_head(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => {
                self.insert_char(c);
                Res::Done
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(t) => match t.name.as_str() {
                "html" => self.in_body(Token::StartTag(t)),
                "body" => {
                    self.insert_html(&t);
                    self.frameset_ok = false;
                    self.mode = Mode::InBody;
                    Res::Done
                }
                "frameset" => {
                    self.insert_html(&t);
                    self.mode = Mode::InFrameset;
                    Res::Done
                }
                "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style" | "template"
                | "title" => {
                    let head = self.head.expect("head element pointer set");
                    self.open.push(head);
                    let r = self.in_head(Token::StartTag(t));
                    self.remove_from_stack(head);
                    r
                }
                "head" => Res::Done,
                _ => {
                    self.insert_html_named("body");
                    self.frameset_ok = true;
                    self.mode = Mode::InBody;
                    Res::Reprocess(Token::StartTag(t))
                }
            },
            Token::EndTag(t) => match t.name.as_str() {
                "template" => self.in_head(Token::EndTag(t)),
                "body" | "html" | "br" => {
                    self.insert_html_named("body");
                    self.frameset_ok = true;
                    self.mode = Mode::InBody;
                    Res::Reprocess(Token::EndTag(t))
                }
                _ => Res::Done,
            },
            t => {
                self.insert_html_named("body");
                self.frameset_ok = true;
                self.mode = Mode::InBody;
                Res::Reprocess(t)
            }
        }
    }

    // ---- §13.2.6.4.7 in body -------------------------------------------------------------------------------

    fn in_body(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character('\0') => Res::Done,
            Token::Character(c) => {
                self.reconstruct_afe();
                self.insert_char(c);
                if !is_ws(c) {
                    self.frameset_ok = false;
                }
                Res::Done
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(t) => self.in_body_start(t),
            Token::EndTag(t) => self.in_body_end(t),
            Token::Eof => {
                if !self.template_modes.is_empty() {
                    return self.in_template(Token::Eof);
                }
                Res::Done
            }
        }
    }

    fn add_missing_attrs(&mut self, target: NodeId, t: &Tag) {
        let attrs = self.convert_attrs(&t.attrs, Namespace::Html);
        if let Some(e) = self.doc.element_mut(target) {
            for a in attrs {
                if !e.attrs.iter().any(|b| b.ns == a.ns && b.local == a.local) {
                    e.attrs.push(a);
                }
            }
        }
    }

    fn in_body_start(&mut self, mut t: Tag) -> Res {
        let name = t.name.clone();
        match name.as_str() {
            "html" => {
                if !self.template_in_stack() {
                    let root = self.open[0];
                    self.add_missing_attrs(root, &t);
                }
            }
            "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style" | "template"
            | "title" => return self.in_head(Token::StartTag(t)),
            "body" => {
                if self.open.len() > 1 && self.is_html(self.open[1], "body") && !self.template_in_stack() {
                    self.frameset_ok = false;
                    let body = self.open[1];
                    self.add_missing_attrs(body, &t);
                }
            }
            "frameset" => {
                if self.open.len() > 1 && self.is_html(self.open[1], "body") && self.frameset_ok {
                    let body = self.open[1];
                    self.doc.detach(body);
                    self.truncate(1);
                    self.insert_html(&t);
                    self.mode = Mode::InFrameset;
                }
            }
            "address" | "article" | "aside" | "blockquote" | "center" | "details" | "dialog" | "dir" | "div" | "dl"
            | "fieldset" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "main" | "menu" | "nav"
            | "ol" | "p" | "search" | "section" | "summary" | "ul" => {
                self.close_p_if_in_button_scope();
                self.insert_html(&t);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.close_p_if_in_button_scope();
                if self.is_html_one_of(self.current(), HEADINGS) {
                    self.pop();
                }
                self.insert_html(&t);
            }
            "pre" | "listing" => {
                self.close_p_if_in_button_scope();
                self.insert_html(&t);
                self.ignore_lf = true;
                self.frameset_ok = false;
            }
            "form" => {
                let in_template = self.template_in_stack();
                if self.form.is_some() && !in_template {
                    return Res::Done;
                }
                self.close_p_if_in_button_scope();
                let el = self.insert_html(&t);
                if !in_template {
                    self.form = Some(el);
                }
            }
            "li" | "dd" | "dt" => {
                self.frameset_ok = false;
                for i in (0..self.open.len()).rev() {
                    let node = self.open[i];
                    let hit = if name == "li" {
                        self.is_html(node, "li").then_some("li")
                    } else if self.is_html(node, "dd") {
                        Some("dd")
                    } else if self.is_html(node, "dt") {
                        Some("dt")
                    } else {
                        None
                    };
                    if let Some(h) = hit {
                        self.generate_implied_end_tags(Some(h));
                        self.pop_until_html(&[h]);
                        break;
                    }
                    if self.is_special(node) && !self.is_html_one_of(node, &["address", "div", "p"]) {
                        break;
                    }
                }
                self.close_p_if_in_button_scope();
                self.insert_html(&t);
            }
            "plaintext" => {
                self.close_p_if_in_button_scope();
                self.insert_html(&t);
                self.tokenizer_state = Some(State::Plaintext);
            }
            "button" => {
                if self.in_scope("button", Scope::Default) {
                    self.generate_implied_end_tags(None);
                    self.pop_until_html(&["button"]);
                }
                self.reconstruct_afe();
                self.insert_html(&t);
                self.frameset_ok = false;
            }
            "a" => {
                let mut existing = None;
                for e in self.afe.iter().rev() {
                    match e {
                        Afe::Marker => break,
                        Afe::Element(n, _) if self.local(*n) == "a" => {
                            existing = Some(*n);
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(a) = existing {
                    self.adoption_agency("a");
                    if let Some(i) = self.afe_index_of(a) {
                        self.afe.remove(i);
                    }
                    self.remove_from_stack(a);
                }
                self.reconstruct_afe();
                let el = self.insert_html(&t);
                self.push_afe(el, &t);
            }
            "b" | "big" | "code" | "em" | "font" | "i" | "s" | "small" | "strike" | "strong" | "tt" | "u" => {
                self.reconstruct_afe();
                let el = self.insert_html(&t);
                self.push_afe(el, &t);
            }
            "nobr" => {
                self.reconstruct_afe();
                if self.in_scope("nobr", Scope::Default) {
                    self.adoption_agency("nobr");
                    self.reconstruct_afe();
                }
                let el = self.insert_html(&t);
                self.push_afe(el, &t);
            }
            "applet" | "marquee" | "object" => {
                self.reconstruct_afe();
                self.insert_html(&t);
                self.afe.push(Afe::Marker);
                self.frameset_ok = false;
            }
            "table" => {
                if self.doc.quirks_mode != QuirksMode::Quirks {
                    self.close_p_if_in_button_scope();
                }
                self.insert_html(&t);
                self.frameset_ok = false;
                self.mode = Mode::InTable;
            }
            "area" | "br" | "embed" | "img" | "keygen" | "wbr" => {
                self.reconstruct_afe();
                self.insert_html(&t);
                self.pop();
                self.frameset_ok = false;
            }
            "input" => {
                if self.context.is_some_and(|c| self.is_html(c, "select")) {
                    return Res::Done;
                }
                if self.in_scope("select", Scope::Default) {
                    self.pop_until_html(&["select"]);
                }
                self.reconstruct_afe();
                self.insert_html(&t);
                self.pop();
                if !t.attr("type").is_some_and(|v| v.eq_ignore_ascii_case("hidden")) {
                    self.frameset_ok = false;
                }
            }
            "param" | "source" | "track" => {
                self.insert_html(&t);
                self.pop();
            }
            "hr" => {
                self.close_p_if_in_button_scope();
                if self.in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags(None);
                }
                self.insert_html(&t);
                self.pop();
                self.frameset_ok = false;
            }
            "image" => {
                t.name = String::from("img");
                return Res::Reprocess(Token::StartTag(t));
            }
            "textarea" => {
                self.insert_html(&t);
                self.ignore_lf = true;
                self.tokenizer_state = Some(State::Rcdata);
                self.orig_mode = self.mode;
                self.frameset_ok = false;
                self.mode = Mode::Text;
            }
            "xmp" => {
                self.close_p_if_in_button_scope();
                self.reconstruct_afe();
                self.frameset_ok = false;
                self.generic_text_element(&t, State::Rawtext);
            }
            "iframe" => {
                self.frameset_ok = false;
                self.generic_text_element(&t, State::Rawtext);
            }
            "noembed" => self.generic_text_element(&t, State::Rawtext),
            "noscript" if self.scripting => self.generic_text_element(&t, State::Rawtext),
            "select" => {
                if self.context.is_some_and(|c| self.is_html(c, "select")) {
                    return Res::Done;
                }
                if self.in_scope("select", Scope::Default) {
                    self.pop_until_html(&["select"]);
                } else {
                    self.reconstruct_afe();
                    self.insert_html(&t);
                    self.frameset_ok = false;
                }
            }
            "option" => {
                if self.in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags(Some("optgroup"));
                } else if self.is_html(self.current(), "option") {
                    self.pop();
                }
                self.reconstruct_afe();
                self.insert_html(&t);
            }
            "optgroup" => {
                if self.in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags(None);
                } else if self.is_html(self.current(), "option") {
                    self.pop();
                }
                self.reconstruct_afe();
                self.insert_html(&t);
            }
            "rb" | "rtc" => {
                if self.in_scope("ruby", Scope::Default) {
                    self.generate_implied_end_tags(None);
                }
                self.insert_html(&t);
            }
            "rp" | "rt" => {
                if self.in_scope("ruby", Scope::Default) {
                    self.generate_implied_end_tags(Some("rtc"));
                }
                self.insert_html(&t);
            }
            "math" | "svg" => {
                self.reconstruct_afe();
                let ns = if name == "math" { Namespace::MathMl } else { Namespace::Svg };
                self.insert_foreign(&t, ns, false);
                if t.self_closing {
                    self.pop();
                }
            }
            "caption" | "col" | "colgroup" | "frame" | "head" | "tbody" | "td" | "tfoot" | "th" | "thead"
            | "tr" => {}
            _ => {
                self.reconstruct_afe();
                self.insert_html(&t);
            }
        }
        Res::Done
    }

    fn in_body_end(&mut self, t: Tag) -> Res {
        let name = t.name.as_str();
        match name {
            "template" => return self.in_head(Token::EndTag(t)),
            "body" => {
                if self.in_scope("body", Scope::Default) {
                    self.mode = Mode::AfterBody;
                }
            }
            "html" => {
                if self.in_scope("body", Scope::Default) {
                    self.mode = Mode::AfterBody;
                    return Res::Reprocess(Token::EndTag(t));
                }
            }
            "address" | "article" | "aside" | "blockquote" | "button" | "center" | "details" | "dialog" | "dir"
            | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "listing"
            | "main" | "menu" | "nav" | "ol" | "pre" | "search" | "section" | "select" | "summary" | "ul" => {
                if self.in_scope(name, Scope::Default) {
                    self.generate_implied_end_tags(None);
                    self.pop_until_html(&[name]);
                }
            }
            "form" => {
                if !self.template_in_stack() {
                    let node = self.form.take();
                    match node {
                        Some(n) if self.node_in_scope(n) => {
                            self.generate_implied_end_tags(None);
                            self.remove_from_stack(n);
                        }
                        _ => {}
                    }
                } else if self.in_scope("form", Scope::Default) {
                    self.generate_implied_end_tags(None);
                    self.pop_until_html(&["form"]);
                }
            }
            "p" => {
                if !self.in_scope("p", Scope::Button) {
                    self.insert_html_named("p");
                }
                self.close_p();
            }
            "li" => {
                if self.in_scope("li", Scope::ListItem) {
                    self.generate_implied_end_tags(Some("li"));
                    self.pop_until_html(&["li"]);
                }
            }
            "dd" | "dt" => {
                if self.in_scope(name, Scope::Default) {
                    self.generate_implied_end_tags(Some(name));
                    self.pop_until_html(&[name]);
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if self.in_scope_any(HEADINGS, Scope::Default) {
                    self.generate_implied_end_tags(None);
                    self.pop_until_html(HEADINGS);
                }
            }
            "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small" | "strike" | "strong" | "tt"
            | "u" => self.adoption_agency(name),
            "applet" | "marquee" | "object" => {
                if self.in_scope(name, Scope::Default) {
                    self.generate_implied_end_tags(None);
                    self.pop_until_html(&[name]);
                    self.clear_afe_to_marker();
                }
            }
            "br" => {
                self.reconstruct_afe();
                self.insert_html_named("br");
                self.pop();
                self.frameset_ok = false;
            }
            _ => self.any_other_end_tag(name),
        }
        Res::Done
    }

    // ---- §13.2.6.4.8 text ----------------------------------------------------------------------------------

    fn text(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) => {
                self.insert_char(c);
                Res::Done
            }
            Token::Eof => {
                self.pop();
                self.mode = self.orig_mode;
                Res::Reprocess(Token::Eof)
            }
            Token::EndTag(_) => {
                self.pop();
                self.mode = self.orig_mode;
                Res::Done
            }
            _ => Res::Done,
        }
    }

    // ---- §13.2.6.4.9 in table ------------------------------------------------------------------------------

    fn clear_stack_to(&mut self, names: &[&str]) {
        while let Some(&n) = self.open.last() {
            if self.is_html_one_of(n, names) {
                break;
            }
            self.pop();
        }
    }
    fn clear_to_table_context(&mut self) {
        self.clear_stack_to(&["table", "template", "html"]);
    }
    fn clear_to_table_body_context(&mut self) {
        self.clear_stack_to(&["tbody", "tfoot", "thead", "template", "html"]);
    }
    fn clear_to_table_row_context(&mut self) {
        self.clear_stack_to(&["tr", "template", "html"]);
    }

    fn in_table(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(_)
                if self.is_html_one_of(self.current(), &["table", "tbody", "template", "tfoot", "thead", "tr"]) =>
            {
                self.pending_table_text.clear();
                self.orig_mode = self.mode;
                self.mode = Mode::InTableText;
                Res::Reprocess(tok)
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(t) => match t.name.as_str() {
                "caption" => {
                    self.clear_to_table_context();
                    self.afe.push(Afe::Marker);
                    self.insert_html(&t);
                    self.mode = Mode::InCaption;
                    Res::Done
                }
                "colgroup" => {
                    self.clear_to_table_context();
                    self.insert_html(&t);
                    self.mode = Mode::InColumnGroup;
                    Res::Done
                }
                "col" => {
                    self.clear_to_table_context();
                    self.insert_html_named("colgroup");
                    self.mode = Mode::InColumnGroup;
                    Res::Reprocess(Token::StartTag(t))
                }
                "tbody" | "tfoot" | "thead" => {
                    self.clear_to_table_context();
                    self.insert_html(&t);
                    self.mode = Mode::InTableBody;
                    Res::Done
                }
                "td" | "th" | "tr" => {
                    self.clear_to_table_context();
                    self.insert_html_named("tbody");
                    self.mode = Mode::InTableBody;
                    Res::Reprocess(Token::StartTag(t))
                }
                "table" => {
                    if !self.in_scope("table", Scope::Table) {
                        return Res::Done;
                    }
                    self.pop_until_html(&["table"]);
                    self.reset_insertion_mode();
                    Res::Reprocess(Token::StartTag(t))
                }
                "style" | "script" | "template" => self.in_head(Token::StartTag(t)),
                "input" if t.attr("type").is_some_and(|v| v.eq_ignore_ascii_case("hidden")) => {
                    self.insert_html(&t);
                    self.pop();
                    Res::Done
                }
                "form" => {
                    if self.form.is_some() || self.template_in_stack() {
                        return Res::Done;
                    }
                    let el = self.insert_html(&t);
                    self.form = Some(el);
                    self.pop();
                    Res::Done
                }
                _ => self.in_table_anything_else(Token::StartTag(t)),
            },
            Token::EndTag(t) => match t.name.as_str() {
                "table" => {
                    if self.in_scope("table", Scope::Table) {
                        self.pop_until_html(&["table"]);
                        self.reset_insertion_mode();
                    }
                    Res::Done
                }
                "body" | "caption" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th" | "thead"
                | "tr" => Res::Done,
                "template" => self.in_head(Token::EndTag(t)),
                _ => self.in_table_anything_else(Token::EndTag(t)),
            },
            Token::Eof => self.in_body(Token::Eof),
            t => self.in_table_anything_else(t),
        }
    }

    fn in_table_anything_else(&mut self, tok: Token) -> Res {
        self.foster = true;
        let r = self.in_body(tok);
        self.foster = false;
        r
    }

    // ---- §13.2.6.4.10 in table text ------------------------------------------------------------------------

    fn in_table_text(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character('\0') => Res::Done,
            Token::Character(c) => {
                self.pending_table_text.push(c);
                Res::Done
            }
            t => {
                let pending = core::mem::take(&mut self.pending_table_text);
                if pending.iter().any(|&c| !is_ws(c)) {
                    for c in pending {
                        self.in_table_anything_else(Token::Character(c));
                    }
                } else {
                    for c in pending {
                        self.insert_char(c);
                    }
                }
                self.mode = self.orig_mode;
                Res::Reprocess(t)
            }
        }
    }

    // ---- §13.2.6.4.11 in caption ---------------------------------------------------------------------------

    fn close_caption(&mut self) -> bool {
        if !self.in_scope("caption", Scope::Table) {
            return false;
        }
        self.generate_implied_end_tags(None);
        self.pop_until_html(&["caption"]);
        self.clear_afe_to_marker();
        self.mode = Mode::InTable;
        true
    }

    fn in_caption(&mut self, tok: Token) -> Res {
        match tok {
            Token::EndTag(ref t) if t.name == "caption" => {
                self.close_caption();
                Res::Done
            }
            Token::StartTag(ref t)
                if is_one_of(&t.name, &["caption", "col", "colgroup", "tbody", "td", "tfoot", "th", "thead", "tr"]) =>
            {
                if self.close_caption() { Res::Reprocess(tok) } else { Res::Done }
            }
            Token::EndTag(ref t) if t.name == "table" => {
                if self.close_caption() { Res::Reprocess(tok) } else { Res::Done }
            }
            Token::EndTag(ref t)
                if is_one_of(&t.name, &["body", "col", "colgroup", "html", "tbody", "td", "tfoot", "th", "thead", "tr"]) =>
            {
                Res::Done
            }
            t => self.in_body(t),
        }
    }

    // ---- §13.2.6.4.12 in column group ----------------------------------------------------------------------

    fn in_column_group(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => {
                self.insert_char(c);
                Res::Done
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::StartTag(ref t) if t.name == "col" => {
                let t = t.clone();
                self.insert_html(&t);
                self.pop();
                Res::Done
            }
            Token::EndTag(ref t) if t.name == "colgroup" => {
                if self.is_html(self.current(), "colgroup") {
                    self.pop();
                    self.mode = Mode::InTable;
                }
                Res::Done
            }
            Token::EndTag(ref t) if t.name == "col" => Res::Done,
            Token::StartTag(ref t) if t.name == "template" => self.in_head(tok),
            Token::EndTag(ref t) if t.name == "template" => self.in_head(tok),
            Token::Eof => self.in_body(tok),
            t => {
                if !self.is_html(self.current(), "colgroup") {
                    return Res::Done;
                }
                self.pop();
                self.mode = Mode::InTable;
                Res::Reprocess(t)
            }
        }
    }

    // ---- §13.2.6.4.13 in table body ------------------------------------------------------------------------

    fn in_table_body(&mut self, tok: Token) -> Res {
        match tok {
            Token::StartTag(ref t) if t.name == "tr" => {
                let t = t.clone();
                self.clear_to_table_body_context();
                self.insert_html(&t);
                self.mode = Mode::InRow;
                Res::Done
            }
            Token::StartTag(ref t) if t.name == "th" || t.name == "td" => {
                self.clear_to_table_body_context();
                self.insert_html_named("tr");
                self.mode = Mode::InRow;
                Res::Reprocess(tok)
            }
            Token::EndTag(ref t) if is_one_of(&t.name, &["tbody", "tfoot", "thead"]) => {
                if self.in_scope(&t.name, Scope::Table) {
                    self.clear_to_table_body_context();
                    self.pop();
                    self.mode = Mode::InTable;
                }
                Res::Done
            }
            Token::StartTag(ref t) if is_one_of(&t.name, &["caption", "col", "colgroup", "tbody", "tfoot", "thead"]) => {
                self.table_body_exit(tok)
            }
            Token::EndTag(ref t) if t.name == "table" => self.table_body_exit(tok),
            Token::EndTag(ref t)
                if is_one_of(&t.name, &["body", "caption", "col", "colgroup", "html", "td", "th", "tr"]) =>
            {
                Res::Done
            }
            t => self.in_table(t),
        }
    }

    fn table_body_exit(&mut self, tok: Token) -> Res {
        if !self.in_scope_any(&["tbody", "thead", "tfoot"], Scope::Table) {
            return Res::Done;
        }
        self.clear_to_table_body_context();
        self.pop();
        self.mode = Mode::InTable;
        Res::Reprocess(tok)
    }

    // ---- §13.2.6.4.14 in row -------------------------------------------------------------------------------

    fn close_row(&mut self) -> bool {
        if !self.in_scope("tr", Scope::Table) {
            return false;
        }
        self.clear_to_table_row_context();
        self.pop();
        self.mode = Mode::InTableBody;
        true
    }

    fn in_row(&mut self, tok: Token) -> Res {
        match tok {
            Token::StartTag(ref t) if t.name == "th" || t.name == "td" => {
                let t = t.clone();
                self.clear_to_table_row_context();
                self.insert_html(&t);
                self.mode = Mode::InCell;
                self.afe.push(Afe::Marker);
                Res::Done
            }
            Token::EndTag(ref t) if t.name == "tr" => {
                self.close_row();
                Res::Done
            }
            Token::StartTag(ref t)
                if is_one_of(&t.name, &["caption", "col", "colgroup", "tbody", "tfoot", "thead", "tr"]) =>
            {
                if self.close_row() { Res::Reprocess(tok) } else { Res::Done }
            }
            Token::EndTag(ref t) if t.name == "table" => {
                if self.close_row() { Res::Reprocess(tok) } else { Res::Done }
            }
            Token::EndTag(ref t) if is_one_of(&t.name, &["tbody", "tfoot", "thead"]) => {
                if !self.in_scope(&t.name, Scope::Table) {
                    return Res::Done;
                }
                if self.close_row() { Res::Reprocess(tok) } else { Res::Done }
            }
            Token::EndTag(ref t) if is_one_of(&t.name, &["body", "caption", "col", "colgroup", "html", "td", "th"]) => {
                Res::Done
            }
            t => self.in_table(t),
        }
    }

    // ---- §13.2.6.4.15 in cell ------------------------------------------------------------------------------

    fn close_cell(&mut self) {
        self.generate_implied_end_tags(None);
        self.pop_until_html(&["td", "th"]);
        self.clear_afe_to_marker();
        self.mode = Mode::InRow;
    }

    fn in_cell(&mut self, tok: Token) -> Res {
        match tok {
            Token::EndTag(ref t) if t.name == "td" || t.name == "th" => {
                if self.in_scope(&t.name, Scope::Table) {
                    let n = t.name.clone();
                    self.generate_implied_end_tags(None);
                    self.pop_until_html(&[n.as_str()]);
                    self.clear_afe_to_marker();
                    self.mode = Mode::InRow;
                }
                Res::Done
            }
            Token::StartTag(ref t)
                if is_one_of(&t.name, &["caption", "col", "colgroup", "tbody", "td", "tfoot", "th", "thead", "tr"]) =>
            {
                if !self.in_scope_any(&["td", "th"], Scope::Table) {
                    return Res::Done;
                }
                self.close_cell();
                Res::Reprocess(tok)
            }
            Token::EndTag(ref t) if is_one_of(&t.name, &["body", "caption", "col", "colgroup", "html"]) => Res::Done,
            Token::EndTag(ref t) if is_one_of(&t.name, &["table", "tbody", "tfoot", "thead", "tr"]) => {
                if !self.in_scope(&t.name, Scope::Table) {
                    return Res::Done;
                }
                self.close_cell();
                Res::Reprocess(tok)
            }
            t => self.in_body(t),
        }
    }

    // ---- §13.2.6.4.16 in template --------------------------------------------------------------------------

    fn switch_template_mode(&mut self, m: Mode, tok: Token) -> Res {
        self.template_modes.pop();
        self.template_modes.push(m);
        self.mode = m;
        Res::Reprocess(tok)
    }

    fn in_template(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(_) | Token::Comment(_) | Token::ProcessingInstruction { .. } | Token::Doctype(_) => {
                self.in_body(tok)
            }
            Token::StartTag(ref t)
                if is_one_of(
                    &t.name,
                    &["base", "basefont", "bgsound", "link", "meta", "noframes", "script", "style", "template", "title"],
                ) =>
            {
                self.in_head(tok)
            }
            Token::EndTag(ref t) if t.name == "template" => self.in_head(tok),
            Token::StartTag(ref t) if is_one_of(&t.name, &["caption", "colgroup", "tbody", "tfoot", "thead"]) => {
                self.switch_template_mode(Mode::InTable, tok)
            }
            Token::StartTag(ref t) if t.name == "col" => self.switch_template_mode(Mode::InColumnGroup, tok),
            Token::StartTag(ref t) if t.name == "tr" => self.switch_template_mode(Mode::InTableBody, tok),
            Token::StartTag(ref t) if t.name == "td" || t.name == "th" => self.switch_template_mode(Mode::InRow, tok),
            Token::StartTag(_) => self.switch_template_mode(Mode::InBody, tok),
            Token::EndTag(_) => Res::Done,
            Token::Eof => {
                if !self.template_in_stack() {
                    return Res::Done;
                }
                self.pop_until_html(&["template"]);
                self.clear_afe_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
                Res::Reprocess(Token::Eof)
            }
        }
    }

    // ---- §13.2.6.4.17–22 after body, framesets, after after ------------------------------------------------

    fn after_body(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => self.in_body(tok),
            Token::Comment(d) => {
                let root = self.open[0];
                self.insert_comment(d, Some(root));
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                let root = self.open[0];
                self.insert_pi(target, data, Some(root));
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::EndTag(ref t) if t.name == "html" => {
                if self.context.is_none() {
                    self.mode = Mode::AfterAfterBody;
                }
                Res::Done
            }
            Token::Eof => Res::Done,
            t => {
                self.mode = Mode::InBody;
                Res::Reprocess(t)
            }
        }
    }

    fn in_frameset(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => {
                self.insert_char(c);
                Res::Done
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::StartTag(ref t) if t.name == "frameset" => {
                let t = t.clone();
                self.insert_html(&t);
                Res::Done
            }
            Token::EndTag(ref t) if t.name == "frameset" => {
                if self.open.len() > 1 {
                    self.pop();
                    if self.context.is_none() && !self.is_html(self.current(), "frameset") {
                        self.mode = Mode::AfterFrameset;
                    }
                }
                Res::Done
            }
            Token::StartTag(ref t) if t.name == "frame" => {
                let t = t.clone();
                self.insert_html(&t);
                self.pop();
                Res::Done
            }
            Token::StartTag(ref t) if t.name == "noframes" => self.in_head(tok),
            _ => Res::Done,
        }
    }

    fn after_frameset(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character(c) if is_ws(c) => {
                self.insert_char(c);
                Res::Done
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::EndTag(ref t) if t.name == "html" => {
                self.mode = Mode::AfterAfterFrameset;
                Res::Done
            }
            Token::StartTag(ref t) if t.name == "noframes" => self.in_head(tok),
            _ => Res::Done,
        }
    }

    fn after_after_body(&mut self, tok: Token) -> Res {
        match tok {
            Token::Comment(d) => {
                self.insert_comment(d, Some(Document::ROOT));
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, Some(Document::ROOT));
                Res::Done
            }
            Token::Doctype(_) => self.in_body(tok),
            Token::Character(c) if is_ws(c) => self.in_body(tok),
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::Eof => Res::Done,
            t => {
                self.mode = Mode::InBody;
                Res::Reprocess(t)
            }
        }
    }

    fn after_after_frameset(&mut self, tok: Token) -> Res {
        match tok {
            Token::Comment(d) => {
                self.insert_comment(d, Some(Document::ROOT));
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, Some(Document::ROOT));
                Res::Done
            }
            Token::Doctype(_) => self.in_body(tok),
            Token::Character(c) if is_ws(c) => self.in_body(tok),
            Token::StartTag(ref t) if t.name == "html" => self.in_body(tok),
            Token::StartTag(ref t) if t.name == "noframes" => self.in_head(tok),
            _ => Res::Done,
        }
    }

    // ---- §13.2.6.5 foreign content -------------------------------------------------------------------------

    fn foreign(&mut self, tok: Token) -> Res {
        match tok {
            Token::Character('\0') => {
                self.insert_char('\u{FFFD}');
                Res::Done
            }
            Token::Character(c) => {
                self.insert_char(c);
                if !is_ws(c) {
                    self.frameset_ok = false;
                }
                Res::Done
            }
            Token::Comment(d) => {
                self.insert_comment(d, None);
                Res::Done
            }
            Token::ProcessingInstruction { target, data } => {
                self.insert_pi(target, data, None);
                Res::Done
            }
            Token::Doctype(_) => Res::Done,
            Token::StartTag(ref t)
                if is_one_of(&t.name, BREAKOUT)
                    || (t.name == "font" && t.attrs.iter().any(|a| matches!(a.name.as_str(), "color" | "face" | "size"))) =>
            {
                self.foreign_breakout(tok)
            }
            Token::EndTag(ref t) if t.name == "br" || t.name == "p" => self.foreign_breakout(tok),
            Token::StartTag(mut t) => {
                let acn = self.adjusted_current().expect("adjusted current node");
                let ns = self.ns(acn);
                if ns == Namespace::Svg
                    && let Some(&(_, fixed)) = SVG_TAGS.iter().find(|(k, _)| *k == t.name) {
                        t.name = String::from(fixed);
                    }
                self.insert_foreign(&t, ns, false);
                if t.self_closing {
                    self.pop();
                }
                Res::Done
            }
            Token::EndTag(t) => {
                let cur = self.current();
                if t.name == "script" && self.doc.element(cur).is_some_and(|e| e.is(Namespace::Svg, "script")) {
                    self.pop();
                    return Res::Done;
                }
                let mut i = self.open.len() - 1;
                loop {
                    if i == 0 {
                        return Res::Done;
                    }
                    let node = self.open[i];
                    if self.local(node).to_ascii_lowercase() == t.name {
                        self.truncate(i);
                        return Res::Done;
                    }
                    i -= 1;
                    if self.ns(self.open[i]) == Namespace::Html {
                        let mode = self.mode;
                        return self.step(mode, Token::EndTag(t));
                    }
                }
            }
            Token::Eof => {
                let mode = self.mode;
                self.step(mode, Token::Eof)
            }
        }
    }

    fn foreign_breakout(&mut self, tok: Token) -> Res {
        while let Some(&n) = self.open.last() {
            if self.is_mathml_text_ip(n) || self.is_html_ip(n) || self.ns(n) == Namespace::Html {
                break;
            }
            self.pop();
        }
        let mode = self.mode;
        self.step(mode, tok)
    }
}

/// Parse a complete document.
pub fn parse_document(input: &str, opts: ParseOpts) -> Document {
    let mut tz = Tokenizer::from_str(input, TokenizerOpts { processing_instructions: opts.processing_instructions });
    let mut tb = TreeBuilder::new(opts);
    tb.run(&mut tz);
    tb.doc
}

/// Parse a fragment (§13.4) in the context of an element `(ns, local name, attributes)`.
/// Returns the document arena and the DocumentFragment node holding the result.
pub fn parse_fragment(
    context_ns: Namespace,
    context_local: &str,
    context_attrs: Vec<Attribute>,
    input: &str,
    quirks: QuirksMode,
    opts: ParseOpts,
) -> (Document, NodeId) {
    let mut tb = TreeBuilder::new(opts);
    tb.doc.quirks_mode = quirks;
    let ctx = tb.doc.create_element(context_ns, context_local, context_attrs);
    if context_ns == Namespace::MathMl && context_local == "annotation-xml" {
        let ip = tb.doc.element(ctx).and_then(|e| e.attr("encoding")).is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v == "text/html" || v == "application/xhtml+xml"
        });
        if let Some(e) = tb.doc.element_mut(ctx) {
            e.html_integration_point = ip;
        }
    }
    let (frag, state) = tb.init_fragment(ctx);
    let mut tz = Tokenizer::from_str(input, TokenizerOpts { processing_instructions: opts.processing_instructions });
    tz.state = state;
    tb.run(&mut tz);
    (tb.doc, frag)
}
