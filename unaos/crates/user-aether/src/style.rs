// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL (SR66): the cascade. css_core's matcher runs over html_core's arena through the `Element` impl
// below (the "follow-on" both cores' docs name), the sheets in cascade order are the UA sheet ([`UA`], the html.css
// subset this formatter draws), every `<style>` element of the page (author) and each element's `style=""`
// (author, inline), and `css_core::cascade::cascade` sorts what applies. This file turns the winning declarations
// into the COMPUTED values the formatter reads ([`Style`]): inherited properties start from the parent's, the
// rest from their initial values; `em` resolves against the element's own font size (the font properties are
// applied in a first pass for that reason), `rem` against the root's, `%` lengths stay percentages for layout.

use alloc::string::String;
use alloc::vec::Vec;
use css_core::cascade::{cascade, Conditions, Origin, RuleSet};
use css_core::color::{parse_color, Color};
use css_core::matching::Element;
use css_core::media::Environment;
use css_core::stylesheet::{parse_style_attribute, parse_stylesheet, Stylesheet};
use css_core::{Declaration, Token, CV};
use html_core::{Document, NodeData, NodeId};

/// The UA sheet: the subset of the HTML Standard's rendering section (§15.3) this formatter draws — block
/// elements, the heading scale, paragraph and list margins, the link colour, the phrase elements, pre and
/// blockquote. The initial font size is [`ROOT_PX`] (the window is 288 px wide).
pub const UA: &str = "
html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, li, pre, blockquote, address, article, aside, footer,
header, main, nav, section, figure, figcaption, dl, dt, dd, hr, form, fieldset, table, tr, center, details,
summary, menu, dir, hgroup, search { display: block; }
li { display: list-item; }
head, style, script, title, meta, link, template, noscript, base { display: none; }
[hidden] { display: none; }
body { margin: 8px; }
p, blockquote, figure, dl, pre, address { margin-top: 1em; margin-bottom: 1em; }
h1 { font-size: 2em; margin-top: 0.67em; margin-bottom: 0.67em; font-weight: bold; }
h2 { font-size: 1.5em; margin-top: 0.83em; margin-bottom: 0.83em; font-weight: bold; }
h3 { font-size: 1.17em; margin-top: 1em; margin-bottom: 1em; font-weight: bold; }
h4 { margin-top: 1.33em; margin-bottom: 1.33em; font-weight: bold; }
h5 { font-size: 0.83em; margin-top: 1.67em; margin-bottom: 1.67em; font-weight: bold; }
h6 { font-size: 0.67em; margin-top: 2.33em; margin-bottom: 2.33em; font-weight: bold; }
ul, ol, menu, dir { margin-top: 1em; margin-bottom: 1em; padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
ul ul, ol ul { list-style-type: circle; }
ul ul ul, ol ul ul, ul ol ul { list-style-type: square; }
ul ul, ul ol, ol ul, ol ol { margin-top: 0; margin-bottom: 0; }
dd { margin-left: 40px; }
blockquote { margin-left: 40px; margin-right: 40px; }
figure { margin-left: 40px; margin-right: 40px; }
hr { border: 1px inset #888888; margin-top: 0.5em; margin-bottom: 0.5em; }
pre, code, kbd, samp, tt, var { font-family: monospace; }
pre { white-space: pre; }
b, strong, th, dt { font-weight: bold; }
i, em, cite, dfn, var, address { font-style: italic; }
u, ins { text-decoration: underline; }
small { font-size: smaller; }
big { font-size: larger; }
center { text-align: center; }
a:link { color: #0645ad; text-decoration: underline; }
mark { background-color: #ffff00; color: #000000; }
";

/// The initial font size, px (the UA default the window is laid out at).
pub const ROOT_PX: f32 = 13.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    None,
    Block,
    Inline,
    ListItem,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListType {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Normal,
    Pre,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Len {
    Px(f32),
    Pct(f32),
    Auto,
}

impl Len {
    /// Resolve against `basis` (the containing block's width); `auto` = 0.
    pub fn px(self, basis: f32) -> f32 {
        match self {
            Len::Px(v) => v,
            Len::Pct(p) => basis * p / 100.0,
            Len::Auto => 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum LineH {
    Normal,
    Mul(f32),
    Px(f32),
}

/// One element's computed style (what the formatter and the painter read).
#[derive(Clone)]
pub struct Style {
    pub display: Display,
    pub color: u32,
    pub bg: Option<u32>,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub underline: bool,
    pub align: Align,
    pub space: Space,
    pub list: ListType,
    pub line_h: LineH,
    /// top, right, bottom, left.
    pub margin: [Len; 4],
    pub padding: [Len; 4],
    pub border_w: [f32; 4],
    pub border_on: [bool; 4],
    pub border_c: [Option<u32>; 4],
    pub width: Len,
    pub max_width: Len,
}

impl Style {
    /// The initial values (the style of the document node, the root's parent).
    pub fn initial() -> Style {
        Style {
            display: Display::Inline,
            color: 0x00_1a1a1a,
            bg: None,
            font_size: ROOT_PX,
            bold: false,
            italic: false,
            mono: false,
            underline: false,
            align: Align::Left,
            space: Space::Normal,
            list: ListType::Disc,
            line_h: LineH::Normal,
            margin: [Len::Px(0.0); 4],
            padding: [Len::Px(0.0); 4],
            border_w: [3.0; 4],
            border_on: [false; 4],
            border_c: [None; 4],
            width: Len::Auto,
            max_width: Len::Auto,
        }
    }

    /// A child's starting point: the inherited properties copied, the rest initial.
    fn inherit(&self) -> Style {
        Style {
            color: self.color,
            font_size: self.font_size,
            bold: self.bold,
            italic: self.italic,
            mono: self.mono,
            underline: self.underline, // text-decoration propagates to the descendants' text
            align: self.align,
            space: self.space,
            list: self.list,
            line_h: self.line_h,
            ..Style::initial()
        }
    }

    /// The used border width of side `i` (0 when that side's style is none).
    pub fn bw(&self, i: usize) -> f32 {
        if self.border_on[i] { self.border_w[i] } else { 0.0 }
    }

    /// The colour of border side `i` (currentColor when unset).
    pub fn bc(&self, i: usize) -> u32 {
        self.border_c[i].unwrap_or(self.color)
    }
}

// ---- css_core's Element over html_core's arena ------------------------------------------------------------

#[derive(Clone)]
pub struct El<'d> {
    pub doc: &'d Document,
    pub id: NodeId,
}

impl PartialEq for El<'_> {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}

impl<'d> El<'d> {
    fn at(&self, id: NodeId) -> El<'d> {
        El { doc: self.doc, id }
    }
}

impl Element for El<'_> {
    fn local_name(&self) -> &str {
        self.doc.element(self.id).map_or("", |e| e.local.as_str())
    }
    fn namespace_url(&self) -> &str {
        self.doc.element(self.id).map_or("", |e| e.ns.url())
    }
    fn each_attr(&self, f: &mut dyn FnMut(&str, &str, &str) -> bool) -> bool {
        if let Some(e) = self.doc.element(self.id) {
            for a in &e.attrs {
                if f(a.ns.url(), &a.local, &a.value) {
                    return true;
                }
            }
        }
        false
    }
    fn parent_element(&self) -> Option<Self> {
        self.doc.parent_element(self.id).map(|p| self.at(p))
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.doc.prev_sibling_element(self.id).map(|p| self.at(p))
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.doc.next_sibling_element(self.id).map(|p| self.at(p))
    }
    fn first_child_element(&self) -> Option<Self> {
        self.doc.element_children(self.id).next().map(|p| self.at(p))
    }
    fn is_empty(&self) -> bool {
        self.doc.children(self.id).all(|c| match self.doc.data(c) {
            NodeData::Element(_) => false,
            NodeData::Text(t) => t.is_empty(),
            _ => true,
        })
    }
}

// ---- the cascade over the document ------------------------------------------------------------------------

/// The computed styles: `of[node index]` is an index into `styles` (text nodes carry their parent's).
pub struct Styled {
    pub styles: Vec<Style>,
    pub of: Vec<usize>,
    pub rules: usize,
    pub elements: usize,
}

/// Cascade every element of `doc` (viewport `vw` x `vh` for the media environment).
pub fn compute(doc: &Document, vw: f32, vh: f32) -> Styled {
    // Every sheet first (the RuleSet borrows them): UA, then each <style> in document order.
    let mut sheets: Vec<(Stylesheet, Origin)> = Vec::new();
    sheets.push((parse_stylesheet(UA), Origin::UserAgent));
    for n in doc.descendants(NodeId(0)) {
        if let Some(e) = doc.element(n) {
            if e.is_html("style") {
                sheets.push((parse_stylesheet(&doc.text_content(n)), Origin::Author));
            }
        }
    }
    // Each element's style attribute, parsed once (index = node index; empty for the rest).
    let inline: Vec<Vec<Declaration>> = doc
        .nodes
        .iter()
        .map(|nd| match &nd.data {
            NodeData::Element(e) => e.attr("style").map(parse_style_attribute).unwrap_or_default(),
            _ => Vec::new(),
        })
        .collect();
    let env = Environment::viewport(vw as f64, vh as f64);
    let supports = |_: &Declaration| true;
    let import = |_: &str| -> Option<&Stylesheet> { None };
    let cond = Conditions { env: &env, supports: &supports, import: &import };
    let mut rules = RuleSet::new();
    for (s, o) in sheets.iter() {
        rules.add_sheet(s, *o, &cond);
    }
    rules.build_index();
    let nrules = rules.entries.len();

    let mut out = Styled { styles: Vec::new(), of: alloc::vec![0; doc.nodes.len()], rules: nrules, elements: 0 };
    out.styles.push(Style::initial()); // index 0: the document's (initial values)
    let mut root_px = ROOT_PX;
    // Tree order with an explicit stack of (node, parent style index).
    let mut stack: Vec<(NodeId, usize)> = Vec::new();
    let mut kids: Vec<NodeId> = doc.children(NodeId(0)).collect();
    kids.reverse();
    for k in kids {
        stack.push((k, 0));
    }
    while let Some((n, ps)) = stack.pop() {
        let si = match doc.data(n) {
            NodeData::Element(_) => {
                let el = El { doc, id: n };
                let applied = cascade(&rules, &el, &inline[n.0], None);
                let parent = out.styles[ps].clone();
                let mut st = parent.inherit();
                let is_root = doc.parent_element(n).is_none();
                // Pass 1: the font properties (em lengths resolve against the element's own font size).
                for a in applied.iter() {
                    let d = a.declaration;
                    if is_font_prop(&d.name) {
                        apply(&mut st, &parent, &d.name, &d.value, root_px);
                    }
                }
                if is_root {
                    root_px = st.font_size;
                }
                for a in applied.iter() {
                    let d = a.declaration;
                    if !is_font_prop(&d.name) {
                        apply(&mut st, &parent, &d.name, &d.value, root_px);
                    }
                }
                out.styles.push(st);
                out.elements += 1;
                out.styles.len() - 1
            }
            _ => ps,
        };
        out.of[n.0] = si;
        let mut kids: Vec<NodeId> = doc.children(n).collect();
        kids.reverse();
        for k in kids {
            stack.push((k, si));
        }
    }
    out
}

fn is_font_prop(n: &str) -> bool {
    matches!(n, "font" | "font-size" | "font-family" | "font-weight" | "font-style")
}

/// The value's component values without whitespace.
fn words(v: &[CV]) -> Vec<&CV> {
    v.iter().filter(|c| !c.is_whitespace()).collect()
}

fn ident(c: &CV) -> Option<String> {
    c.ident().map(|s| s.to_ascii_lowercase())
}

/// A length (`px em rem pt ex ch vw vh %`, a bare 0, `auto`).
fn len(c: &CV, fs: f32, root: f32) -> Option<Len> {
    match c {
        CV::Token(Token::Dimension(n, u)) => {
            let v = n.value as f32;
            let px = match u.to_ascii_lowercase().as_str() {
                "px" => v,
                "em" => v * fs,
                "rem" => v * root,
                "pt" => v * 4.0 / 3.0,
                "pc" => v * 16.0,
                "in" => v * 96.0,
                "cm" => v * 96.0 / 2.54,
                "mm" => v * 96.0 / 25.4,
                "ex" | "ch" => v * fs * 0.5,
                "vw" => v * crate::MAX_W as f32 / 100.0,
                "vh" => v * crate::MAX_H as f32 / 100.0,
                _ => return None,
            };
            Some(Len::Px(px))
        }
        CV::Token(Token::Percentage(n)) => Some(Len::Pct(n.value as f32)),
        CV::Token(Token::Number(n)) if n.value == 0.0 => Some(Len::Px(0.0)),
        CV::Token(Token::Ident(s)) if s.eq_ignore_ascii_case("auto") => Some(Len::Auto),
        _ => None,
    }
}

/// An opaque sRGB colour as 0x00RRGGBB; `transparent` / mostly-transparent = `Some(None)`.
fn color(v: &[CV], current: u32) -> Option<Option<u32>> {
    match parse_color(v)? {
        Color::CurrentColor => Some(Some(current)),
        Color::Rgba(c) => {
            if c.a < 0.5 {
                return Some(None);
            }
            let ch = |x: f64| -> u32 {
                let x = if x < 0.0 { 0.0 } else if x > 255.0 { 255.0 } else { x };
                (x + 0.5) as u32
            };
            Some(Some((ch(c.r) << 16) | (ch(c.g) << 8) | ch(c.b)))
        }
    }
}

/// 1–4 values to (top, right, bottom, left).
fn four<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    Some(match v.len() {
        1 => [v[0], v[0], v[0], v[0]],
        2 => [v[0], v[1], v[0], v[1]],
        3 => [v[0], v[1], v[2], v[1]],
        4 => [v[0], v[1], v[2], v[3]],
        _ => return None,
    })
}

fn border_width(c: &CV, fs: f32, root: f32) -> Option<f32> {
    match ident(c).as_deref() {
        Some("thin") => Some(1.0),
        Some("medium") => Some(3.0),
        Some("thick") => Some(5.0),
        _ => match len(c, fs, root)? {
            Len::Px(v) => Some(if v < 0.0 { 0.0 } else { v }),
            _ => None,
        },
    }
}

/// A border-style keyword: `Some(true)` draws (every style draws as solid), `Some(false)` is none/hidden.
fn border_style(c: &CV) -> Option<bool> {
    match ident(c)?.as_str() {
        "none" | "hidden" => Some(false),
        "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => Some(true),
        _ => None,
    }
}

const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];

fn apply_border_side(st: &mut Style, i: usize, w: &[&CV], fs: f32, root: f32) {
    for c in w {
        if let Some(b) = border_style(c) {
            st.border_on[i] = b;
        } else if let Some(x) = border_width(c, fs, root) {
            st.border_w[i] = x;
        } else if let Some(col) = color(core::slice::from_ref(*c), st.color) {
            st.border_c[i] = col;
        }
    }
}

/// Apply one declaration to `st` (the element's computed style so far; `p` its parent's).
fn apply(st: &mut Style, p: &Style, name: &str, value: &[CV], root: f32) {
    let w = words(value);
    if w.is_empty() {
        return;
    }
    let kw = if w.len() == 1 { ident(w[0]) } else { None };
    if kw.as_deref() == Some("inherit") {
        inherit_one(st, p, name);
        return;
    }
    let fs = st.font_size;
    match name {
        "display" => {
            if let Some(k) = kw {
                st.display = match k.as_str() {
                    "none" => Display::None,
                    "inline" | "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "contents" => Display::Inline,
                    "list-item" => Display::ListItem,
                    _ => Display::Block, // block, flex, grid, table, flow-root …: a block box here
                };
            }
        }
        "color" => {
            if let Some(Some(c)) = color(value, p.color) {
                st.color = c;
            }
        }
        "background-color" => {
            if let Some(c) = color(value, st.color) {
                st.bg = c;
            }
        }
        "background" => {
            for c in &w {
                if ident(c).as_deref() == Some("none") {
                    continue;
                }
                if let Some(col) = color(core::slice::from_ref(*c), st.color) {
                    st.bg = col;
                }
            }
        }
        "font-size" => {
            let base = p.font_size;
            let v = match kw.as_deref() {
                Some("xx-small") => Some(ROOT_PX * 0.6),
                Some("x-small") => Some(ROOT_PX * 0.75),
                Some("small") => Some(ROOT_PX * 0.89),
                Some("medium") => Some(ROOT_PX),
                Some("large") => Some(ROOT_PX * 1.2),
                Some("x-large") => Some(ROOT_PX * 1.5),
                Some("xx-large") => Some(ROOT_PX * 2.0),
                Some("smaller") => Some(base / 1.2),
                Some("larger") => Some(base * 1.2),
                _ => match len(w[0], base, root) {
                    Some(Len::Px(v)) => Some(v),
                    Some(Len::Pct(pc)) => Some(base * pc / 100.0),
                    _ => None,
                },
            };
            if let Some(v) = v {
                st.font_size = v.clamp(4.0, 96.0);
            }
        }
        "font-weight" => match w[0] {
            CV::Token(Token::Number(n)) => st.bold = n.value >= 600.0,
            c => match ident(c).as_deref() {
                Some("bold") | Some("bolder") => st.bold = true,
                Some("normal") | Some("lighter") => st.bold = false,
                _ => {}
            },
        },
        "font-style" => match kw.as_deref() {
            Some("italic") | Some("oblique") => st.italic = true,
            Some("normal") => st.italic = false,
            _ => {}
        },
        "font-family" => {
            if let Some(m) = family_is_mono(value) {
                st.mono = m;
            }
        }
        "font" => font_shorthand(st, p, &w, value, root),
        "text-align" => {
            if let Some(k) = kw {
                st.align = match k.as_str() {
                    "center" | "-webkit-center" => Align::Center,
                    "right" | "end" => Align::Right,
                    _ => Align::Left,
                };
            }
        }
        "white-space" => {
            if let Some(k) = kw {
                st.space = if k.starts_with("pre") { Space::Pre } else { Space::Normal };
            }
        }
        "list-style-type" | "list-style" => {
            for c in &w {
                if let Some(k) = ident(c) {
                    let t = match k.as_str() {
                        "none" => Some(ListType::None),
                        "disc" => Some(ListType::Disc),
                        "circle" => Some(ListType::Circle),
                        "square" => Some(ListType::Square),
                        "decimal" | "decimal-leading-zero" | "lower-alpha" | "upper-alpha" | "lower-latin" | "upper-latin" | "lower-roman"
                        | "upper-roman" => Some(ListType::Decimal),
                        _ => None,
                    };
                    if let Some(t) = t {
                        st.list = t;
                    }
                }
            }
        }
        "line-height" => {
            st.line_h = match w[0] {
                CV::Token(Token::Number(n)) => LineH::Mul(n.value as f32),
                c => match (ident(c).as_deref(), len(c, fs, root)) {
                    (Some("normal"), _) => LineH::Normal,
                    (_, Some(Len::Px(v))) => LineH::Px(v),
                    (_, Some(Len::Pct(pc))) => LineH::Px(fs * pc / 100.0),
                    _ => st.line_h,
                },
            }
        }
        "text-decoration" | "text-decoration-line" => {
            for c in &w {
                match ident(c).as_deref() {
                    Some("underline") => st.underline = true,
                    Some("none") => st.underline = false,
                    _ => {}
                }
            }
        }
        "margin" | "padding" => {
            let ls: Vec<Len> = w.iter().filter_map(|c| len(c, fs, root)).collect();
            if ls.len() == w.len() {
                if let Some(f) = four(&ls) {
                    if name == "margin" {
                        st.margin = f;
                    } else {
                        st.padding = f;
                    }
                }
            }
        }
        "width" => {
            if let Some(l) = len(w[0], fs, root) {
                st.width = l;
            }
        }
        "max-width" => {
            if kw.as_deref() == Some("none") {
                st.max_width = Len::Auto;
            } else if let Some(l) = len(w[0], fs, root) {
                st.max_width = l;
            }
        }
        "border" => {
            for i in 0..4 {
                st.border_w[i] = 3.0;
                st.border_on[i] = false;
                st.border_c[i] = None;
                apply_border_side(st, i, &w, fs, root);
            }
        }
        "border-width" => {
            let v: Vec<f32> = w.iter().filter_map(|c| border_width(c, fs, root)).collect();
            if let Some(f) = four(&v) {
                st.border_w = f;
            }
        }
        "border-style" => {
            let v: Vec<bool> = w.iter().filter_map(|c| border_style(c)).collect();
            if let Some(f) = four(&v) {
                st.border_on = f;
            }
        }
        "border-color" => {
            let v: Vec<Option<u32>> = w.iter().filter_map(|c| color(core::slice::from_ref(*c), st.color)).collect();
            if let Some(f) = four(&v) {
                st.border_c = f;
            }
        }
        _ => {
            // margin-top / padding-left / border-bottom / border-left-color …
            for (i, side) in SIDES.iter().enumerate() {
                if let Some(rest) = name.strip_prefix("margin-") {
                    if rest == *side {
                        if let Some(l) = len(w[0], fs, root) {
                            st.margin[i] = l;
                        }
                    }
                } else if let Some(rest) = name.strip_prefix("padding-") {
                    if rest == *side {
                        if let Some(l) = len(w[0], fs, root) {
                            st.padding[i] = l;
                        }
                    }
                } else if let Some(rest) = name.strip_prefix("border-") {
                    if rest == *side {
                        st.border_w[i] = 3.0;
                        st.border_on[i] = false;
                        st.border_c[i] = None;
                        apply_border_side(st, i, &w, fs, root);
                    } else if let Some(prop) = rest.strip_prefix(side).and_then(|r| r.strip_prefix('-')) {
                        match prop {
                            "width" => {
                                if let Some(x) = border_width(w[0], fs, root) {
                                    st.border_w[i] = x;
                                }
                            }
                            "style" => {
                                if let Some(b) = border_style(w[0]) {
                                    st.border_on[i] = b;
                                }
                            }
                            "color" => {
                                if let Some(c) = color(value, st.color) {
                                    st.border_c[i] = c;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// `font-family`: the first family this window can tell apart decides — a monospace generic or a family
/// named like one (mono, courier, consolas, menlo) → the mono face; any other → the sans face.
fn family_is_mono(value: &[CV]) -> Option<bool> {
    let mut name = String::new();
    let mut fams: Vec<String> = Vec::new();
    for c in value {
        match c {
            CV::Token(Token::Comma) => {
                fams.push(core::mem::take(&mut name));
            }
            CV::Token(Token::Ident(s)) | CV::Token(Token::String(s)) => {
                if !name.is_empty() {
                    name.push(' ');
                }
                name.push_str(&s.to_ascii_lowercase());
            }
            _ => {}
        }
    }
    fams.push(name);
    let f = fams.into_iter().find(|f| !f.is_empty())?;
    Some(f == "monospace" || f.contains("mono") || f.contains("courier") || f.contains("consolas") || f.contains("menlo"))
}

/// `font: [style] [weight] size[/line-height] family`.
fn font_shorthand(st: &mut Style, p: &Style, w: &[&CV], value: &[CV], root: f32) {
    st.italic = false;
    st.bold = false;
    let mut i = 0;
    while i < w.len() {
        match w[i] {
            CV::Token(Token::Number(n)) => {
                st.bold = n.value >= 600.0;
                i += 1;
            }
            c => match ident(c).as_deref() {
                Some("italic") | Some("oblique") => {
                    st.italic = true;
                    i += 1;
                }
                Some("bold") | Some("bolder") => {
                    st.bold = true;
                    i += 1;
                }
                Some("normal") | Some("lighter") | Some("small-caps") => i += 1,
                _ => break,
            },
        }
    }
    if i < w.len() {
        apply(st, p, "font-size", core::slice::from_ref(w[i]), root);
        i += 1;
        if i + 1 < w.len() && w[i].is_delim('/') {
            let fs = st.font_size;
            st.line_h = match w[i + 1] {
                CV::Token(Token::Number(n)) => LineH::Mul(n.value as f32),
                c => match len(c, fs, root) {
                    Some(Len::Px(v)) => LineH::Px(v),
                    _ => st.line_h,
                },
            };
            i += 2;
        }
    }
    // the family is everything after the size: find where it starts in the raw value
    if i < w.len() {
        let start = w[i] as *const CV;
        if let Some(k) = value.iter().position(|c| core::ptr::eq(c, start)) {
            if let Some(m) = family_is_mono(&value[k..]) {
                st.mono = m;
            }
        }
    }
}

fn inherit_one(st: &mut Style, p: &Style, name: &str) {
    match name {
        "color" => st.color = p.color,
        "background-color" | "background" => st.bg = p.bg,
        "font-size" => st.font_size = p.font_size,
        "font-weight" => st.bold = p.bold,
        "font-style" => st.italic = p.italic,
        "font-family" => st.mono = p.mono,
        "text-align" => st.align = p.align,
        "white-space" => st.space = p.space,
        "line-height" => st.line_h = p.line_h,
        "margin" => st.margin = p.margin,
        "padding" => st.padding = p.padding,
        "display" => st.display = p.display,
        "width" => st.width = p.width,
        _ => {}
    }
}
