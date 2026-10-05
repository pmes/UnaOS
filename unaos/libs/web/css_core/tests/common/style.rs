//! The oracle's style resolver: css_core's cascade + a computed-value step for the 20 compared properties.
//!
//! What is css_core's and judged: parsing, selector matching, `@media`, the cascade sort, `var()`,
//! `calc()`/`clamp()`/`min()`/`max()`, colors. What lives only here (Aether owns the real style system): the
//! UA stylesheet (HTML §15 Rendering, with the values Chromium's html.css uses), shorthand expansion for the
//! shorthands the pages use, inheritance, and the computed / resolved value of each compared property.
#![allow(dead_code)]
use std::collections::BTreeMap;

use super::dom::{Dom, El};
use css_core::cascade::{cascade, Conditions, Origin, RuleSet};
use css_core::color::{parse_color, Color, Rgba};
use css_core::matching::Element;
use css_core::media::Environment;
use css_core::parser::{trim_ws, Declaration, CV};
use css_core::stylesheet::{parse_style_attribute, parse_stylesheet, Stylesheet};
use css_core::values::{compute_custom_properties, contains_var, parse_math, substitute_vars, LengthContext, Resolved};
use css_core::Token;

pub const PROPS: [&str; 20] = [
    "display", "position", "color", "background-color", "font-size", "font-weight", "font-style", "font-family", "line-height",
    "text-align", "text-decoration-line", "white-space", "visibility", "list-style-type", "border-top-width", "border-top-style",
    "border-left-color", "padding-top", "margin-top", "flex-direction",
];

/// HTML §15 (Rendering) as Chromium's html.css writes it, for the elements and properties compared.
pub const UA_CSS: &str = r#"
@namespace "http://www.w3.org/1999/xhtml";
html, body, address, blockquote, center, dialog, div, figure, figcaption, footer, form, header, hr, legend, listing, main, p,
plaintext, pre, search, xmp, article, aside, h1, h2, h3, h4, h5, h6, hgroup, nav, section, dl, dt, dd, ul, ol, menu, dir,
fieldset, details, optgroup { display: block }
[hidden]:not([hidden="until-found" i]):not(embed) { display: none }
area, base, basefont, datalist, head, link, meta, noembed, noframes, param, rp, script, style, template, title { display: none }
body { margin: 8px }
p { margin-block: 1em }
dl { margin-block: 1em }
dd { margin-inline-start: 40px }
blockquote, figure { margin-block: 1em; margin-inline: 40px }
h1 { font-size: 2em; margin-block: 0.67em; font-weight: bold }
:is(article, aside, nav, section) h1 { font-size: 1.5em; margin-block: 0.83em }
:is(article, aside, nav, section) :is(article, aside, nav, section) h1 { font-size: 1.17em; margin-block: 1em }
h2 { font-size: 1.5em; margin-block: 0.83em; font-weight: bold }
h3 { font-size: 1.17em; margin-block: 1em; font-weight: bold }
h4 { margin-block: 1.33em; font-weight: bold }
h5 { font-size: 0.83em; margin-block: 1.67em; font-weight: bold }
h6 { font-size: 0.67em; margin-block: 2.33em; font-weight: bold }
table { display: table; border-collapse: separate; border-spacing: 2px; border-color: gray; box-sizing: border-box }
caption { display: table-caption; text-align: -webkit-center }
colgroup { display: table-column-group }
col { display: table-column }
thead { display: table-header-group; vertical-align: middle; border-color: inherit }
tbody { display: table-row-group; vertical-align: middle; border-color: inherit }
tfoot { display: table-footer-group; vertical-align: middle; border-color: inherit }
tr { display: table-row; vertical-align: inherit; border-color: inherit }
td, th { display: table-cell; vertical-align: inherit; padding: 1px }
th { font-weight: bold; text-align: -internal-center }
ul, menu, dir { list-style-type: disc; margin-block: 1em; padding-inline-start: 40px }
ol { list-style-type: decimal; margin-block: 1em; padding-inline-start: 40px }
li { display: list-item }
:is(ul, ol, menu, dir) :is(ul, ol, menu, dir) { margin-block: 0 }
:is(ul, ol, menu, dir) :is(ul, menu, dir) { list-style-type: circle }
:is(ul, ol, menu, dir) :is(ul, ol, menu, dir) :is(ul, menu, dir) { list-style-type: square }
address, cite, dfn, em, i, var { font-style: italic }
b, strong { font-weight: bolder }
pre, xmp, plaintext, listing { font-family: monospace; white-space: pre; margin-block: 1em }
code, kbd, samp, tt { font-family: monospace }
mark { background-color: Mark; color: MarkText }
big { font-size: larger }
small, sub, sup { font-size: smaller }
s, strike, del { text-decoration: line-through }
u, ins { text-decoration: underline }
center { text-align: -webkit-center }
nobr { white-space: nowrap }
hr { color: gray; margin-block: 0.5em; margin-inline: auto; border-style: inset; border-width: 1px }
a:any-link { color: LinkText; text-decoration: underline }
a:any-link:active { color: ActiveText }
input, textarea, select, button { margin: 0em; font-size: 13.333333px; font-family: Arial; font-weight: normal;
  font-style: normal; line-height: normal; text-align: start; color: FieldText; display: inline-block }
input { padding-block: 1px; padding-inline: 2px; border: 2px inset rgb(118, 118, 118); background-color: Field }
input:disabled, textarea:disabled { color: rgb(84, 84, 84); background-color: rgba(239, 239, 239, 0.3); border-color: rgba(118, 118, 118, 0.3) }
input[type="checkbox" i] { margin: 3px 3px 3px 4px; padding: initial; border: initial; background-color: initial }
input[type="radio" i] { margin: 3px 3px 0px 5px; padding: initial; border: initial; background-color: initial }
input[type="hidden" i] { display: none }
input[type="button" i], input[type="submit" i], input[type="reset" i], button { color: ButtonText; padding-block: 1px;
  padding-inline: 6px; border: 2px outset ButtonBorder; background-color: ButtonFace; text-align: center }
button:disabled, input[type="button" i]:disabled, input[type="submit" i]:disabled, input[type="reset" i]:disabled {
  color: rgba(16, 16, 16, 0.3); background-color: rgba(239, 239, 239, 0.3); border-color: rgba(118, 118, 118, 0.3) }
textarea { font-family: monospace; padding: 2px; border: 1px solid rgb(118, 118, 118); white-space: pre-wrap; background-color: Field }
select { border: 1px solid rgb(118, 118, 118); background-color: ButtonFace; white-space: pre }
select:disabled { color: GrayText; border-color: rgba(118, 118, 118, 0.3) }
option { display: block; white-space: nowrap }
fieldset { margin-inline: 2px; border: 2px groove ThreeDFace; padding-block: 0.35em 0.625em; padding-inline: 0.75em }
legend { padding-inline: 2px }
iframe { border: 2px inset }
"#;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f64),
    Px(f64),
}

/// The computed values of the compared properties (plus what children inherit).
#[derive(Clone, Debug)]
pub struct Computed {
    pub display: String,
    pub position: String,
    pub color: Rgba,
    pub background_color: Rgba,
    pub font_size: f64,
    /// a font size derived from a keyword (factor of `medium`), Blink-style, for the monospace size rule
    pub font_kw: Option<f64>,
    pub font_weight: f64,
    pub font_style: String,
    pub font_family: Vec<String>,
    pub line_height: LineHeight,
    pub text_align: String,
    pub text_decoration_line: String,
    pub white_space: String,
    pub visibility: String,
    pub list_style_type: String,
    pub border_top_width: f64,
    pub border_top_style: String,
    pub border_left_color: Rgba,
    /// px, or the unresolved percentage text
    pub padding_top: String,
    pub margin_top: String,
    pub flex_direction: String,
    pub customs: BTreeMap<String, Vec<CV>>,
}

impl Computed {
    pub fn initial() -> Computed {
        Computed {
            display: "inline".into(),
            position: "static".into(),
            color: Rgba::rgb(0, 0, 0),
            background_color: Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
            font_size: 16.0,
            font_kw: Some(1.0),
            font_weight: 400.0,
            font_style: "normal".into(),
            font_family: vec!["\"Times New Roman\"".into()],
            line_height: LineHeight::Normal,
            text_align: "start".into(),
            text_decoration_line: "none".into(),
            white_space: "normal".into(),
            visibility: "visible".into(),
            list_style_type: "disc".into(),
            border_top_width: 0.0,
            border_top_style: "none".into(),
            border_left_color: Rgba::rgb(0, 0, 0),
            padding_top: "0px".into(),
            margin_top: "0px".into(),
            flex_direction: "row".into(),
            customs: BTreeMap::new(),
        }
    }

    /// The values as Chromium's getComputedStyle serializes them, in PROPS order.
    pub fn serialize(&self) -> Vec<String> {
        let lh = match self.line_height {
            LineHeight::Normal => "normal".to_string(),
            LineHeight::Number(n) => px(n * self.font_size),
            LineHeight::Px(p) => px(p),
        };
        vec![
            self.display.clone(),
            self.position.clone(),
            self.color.serialize(),
            self.background_color.serialize(),
            px(self.font_size),
            num(self.font_weight),
            self.font_style.clone(),
            self.font_family.join(", "),
            lh,
            self.text_align.clone(),
            self.text_decoration_line.clone(),
            self.white_space.clone(),
            self.visibility.clone(),
            self.list_style_type.clone(),
            px(self.border_top_width),
            self.border_top_style.clone(),
            self.border_left_color.serialize(),
            self.padding_top.clone(),
            self.margin_top.clone(),
            self.flex_direction.clone(),
        ]
    }
}

fn num(v: f64) -> String {
    let r = (v * 10000.0).round() / 10000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}
fn px(v: f64) -> String {
    format!("{}px", num(v))
}

const INHERITED: &[&str] = &[
    "color", "font-size", "font-weight", "font-style", "font-family", "line-height", "text-align", "white-space", "visibility",
    "list-style-type",
];

/// A longhand's cascaded value: component values, or a CSS-wide keyword.
#[derive(Clone, Debug)]
enum Cascaded {
    Value(Vec<CV>),
    Inherit,
    Initial,
}

fn ident_lower(v: &[CV]) -> Option<String> {
    match trim_ws(v) {
        [CV::Token(Token::Ident(s))] => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

fn wide(v: &[CV]) -> Option<Cascaded> {
    match ident_lower(v)?.as_str() {
        "inherit" => Some(Cascaded::Inherit),
        "initial" => Some(Cascaded::Initial),
        // unset / revert (approximated as unset): resolved per property below
        "unset" | "revert" | "revert-layer" => Some(Cascaded::Initial).map(|_| Cascaded::Value(vec![CV::Token(Token::Ident("unset".into()))])),
        _ => None,
    }
}

/// Space-separated top-level groups (whitespace dropped).
fn words(v: &[CV]) -> Vec<CV> {
    v.iter().filter(|c| !c.is_whitespace()).cloned().collect()
}

fn one(c: &CV) -> Vec<CV> {
    vec![c.clone()]
}

/// 1–4 value box shorthands: (top, right, bottom, left).
fn box4(v: &[CV]) -> Option<[Vec<CV>; 4]> {
    let w = words(v);
    let g = |i: usize| one(&w[i]);
    Some(match w.len() {
        1 => [g(0), g(0), g(0), g(0)],
        2 => [g(0), g(1), g(0), g(1)],
        3 => [g(0), g(1), g(2), g(1)],
        4 => [g(0), g(1), g(2), g(3)],
        _ => return None,
    })
}

fn is_border_style(c: &CV) -> bool {
    matches!(c.ident().map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("none" | "hidden" | "dotted" | "dashed" | "solid" | "double" | "groove" | "ridge" | "inset" | "outset"))
}
fn is_border_width(c: &CV) -> bool {
    matches!(c.ident().map(|s| s.to_ascii_lowercase()).as_deref(), Some("thin" | "medium" | "thick"))
        || matches!(c, CV::Token(Token::Dimension(..)))
        || matches!(c, CV::Token(Token::Number(n)) if n.value == 0.0)
        || matches!(c, CV::Function(..)) && parse_math(c).is_some()
}

/// `border` / `border-top` …: (width, style, color), missing parts reset to their initial values.
fn border_parts(v: &[CV]) -> Option<(Vec<CV>, Vec<CV>, Vec<CV>)> {
    let (mut w, mut s, mut c) = (None, None, None);
    for x in words(v) {
        if s.is_none() && is_border_style(&x) {
            s = Some(one(&x));
        } else if w.is_none() && is_border_width(&x) {
            w = Some(one(&x));
        } else if c.is_none() && parse_color(&one(&x)).is_some() {
            c = Some(one(&x));
        } else {
            return None;
        }
    }
    let id = |s: &str| vec![CV::Token(Token::Ident(s.into()))];
    Some((w.unwrap_or(id("medium")), s.unwrap_or(id("none")), c.unwrap_or(id("currentcolor"))))
}

/// Expand a declaration into the compared longhands it sets. `None`: invalid (dropped at parse time).
fn expand(name: &str, v: &[CV]) -> Option<Vec<(&'static str, Vec<CV>)>> {
    let v = trim_ws(v);
    if v.is_empty() {
        return None;
    }
    let id = |s: &str| vec![CV::Token(Token::Ident(s.into()))];
    if wide(v).is_some() {
        // a CSS-wide keyword on a shorthand applies to every longhand
        let all: &[&'static str] = match name {
            "margin" | "margin-block" | "margin-block-start" | "margin-top" => &["margin-top"],
            "padding" | "padding-block" | "padding-block-start" | "padding-top" => &["padding-top"],
            "border" => &["border-top-width", "border-top-style", "border-left-color"],
            "border-top" => &["border-top-width", "border-top-style"],
            "border-left" => &["border-left-color"],
            "border-width" => &["border-top-width"],
            "border-style" => &["border-top-style"],
            "border-color" => &["border-left-color"],
            "background" => &["background-color"],
            "font" => &["font-size", "font-weight", "font-style", "font-family", "line-height"],
            "text-decoration" => &["text-decoration-line"],
            "list-style" => &["list-style-type"],
            "flex-flow" => &["flex-direction"],
            n => return PROPS.iter().find(|p| **p == n).map(|p| vec![(*p, v.to_vec())]),
        };
        return Some(all.iter().map(|p| (*p, v.to_vec())).collect());
    }
    Some(match name {
        "margin" => vec![("margin-top", box4(v)?[0].clone())],
        "padding" => vec![("padding-top", box4(v)?[0].clone())],
        "margin-block" | "padding-block" => {
            let w = words(v);
            if w.is_empty() || w.len() > 2 {
                return None;
            }
            vec![(if name == "margin-block" { "margin-top" } else { "padding-top" }, one(&w[0]))]
        }
        "margin-block-start" => vec![("margin-top", v.to_vec())],
        "padding-block-start" => vec![("padding-top", v.to_vec())],
        "border" => {
            let (w, s, c) = border_parts(v)?;
            vec![("border-top-width", w), ("border-top-style", s), ("border-left-color", c)]
        }
        "border-top" => {
            let (w, s, _) = border_parts(v)?;
            vec![("border-top-width", w), ("border-top-style", s)]
        }
        "border-left" => vec![("border-left-color", border_parts(v)?.2)],
        "border-right" | "border-bottom" => {
            border_parts(v)?;
            vec![]
        }
        "border-width" => vec![("border-top-width", box4(v)?[0].clone())],
        "border-style" => vec![("border-top-style", box4(v)?[0].clone())],
        "border-color" => vec![("border-left-color", box4(v)?[3].clone())],
        "background" => {
            // the color belongs to the final layer
            let last = v.split(|c| matches!(c, CV::Token(Token::Comma))).last()?;
            let color = words(last).into_iter().find(|c| parse_color(&one(c)).is_some());
            vec![("background-color", color.map(|c| one(&c)).unwrap_or(id("transparent")))]
        }
        "text-decoration" => {
            let lines: Vec<CV> = words(v)
                .into_iter()
                .filter(|c| matches!(c.ident().map(|s| s.to_ascii_lowercase()).as_deref(), Some("none" | "underline" | "overline" | "line-through" | "blink")))
                .collect();
            vec![("text-decoration-line", if lines.is_empty() { id("none") } else { lines.iter().flat_map(|c| [c.clone(), CV::Token(Token::Whitespace)]).collect() })]
        }
        "list-style" => {
            let t = words(v).into_iter().find(|c| {
                !matches!(c.ident().map(|s| s.to_ascii_lowercase()).as_deref(), Some("inside" | "outside")) && !matches!(c, CV::Function(..) | CV::Token(Token::Url(_)))
            });
            vec![("list-style-type", t.map(|c| one(&c)).unwrap_or(id("disc")))]
        }
        "flex-flow" => {
            let d = words(v).into_iter().find(|c| matches!(c.ident().map(|s| s.to_ascii_lowercase()).as_deref(), Some("row" | "row-reverse" | "column" | "column-reverse")));
            vec![("flex-direction", d.map(|c| one(&c)).unwrap_or(id("row")))]
        }
        "font" => font_shorthand(v)?,
        n => match PROPS.iter().find(|p| **p == n) {
            Some(p) => vec![(*p, v.to_vec())],
            None => vec![],
        },
    })
}

/// `font: [style || weight]? size[/line-height]? family` (variant / stretch keywords accepted, unused).
fn font_shorthand(v: &[CV]) -> Option<Vec<(&'static str, Vec<CV>)>> {
    let id = |s: &str| vec![CV::Token(Token::Ident(s.into()))];
    let w: Vec<CV> = v.iter().cloned().collect();
    let mut i = 0;
    let (mut style, mut weight) = (id("normal"), id("normal"));
    let skip = |i: &mut usize| {
        while *i < w.len() && w[*i].is_whitespace() {
            *i += 1;
        }
    };
    loop {
        skip(&mut i);
        let Some(c) = w.get(i) else { return None };
        match c.ident().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("italic" | "oblique") => style = one(c),
            Some("bold" | "bolder" | "lighter") => weight = one(c),
            Some("normal" | "small-caps" | "condensed" | "expanded" | "semi-condensed" | "semi-expanded") => {}
            _ => match c {
                CV::Token(Token::Number(n)) if n.value >= 1.0 && n.value <= 1000.0 => weight = one(c),
                _ => break,
            },
        }
        i += 1;
    }
    let size = one(w.get(i)?);
    i += 1;
    let mut lh = id("normal");
    skip(&mut i);
    if w.get(i).map(|c| c.is_delim('/')).unwrap_or(false) {
        i += 1;
        skip(&mut i);
        lh = one(w.get(i)?);
        i += 1;
    }
    let family = w[i..].to_vec();
    if trim_ws(&family).is_empty() {
        return None;
    }
    Some(vec![("font-style", style), ("font-weight", weight), ("font-size", size), ("line-height", lh), ("font-family", family)])
}

pub struct Styler<'a> {
    pub rules: RuleSet<'a>,
    pub env: Environment,
}

/// Parse the UA sheet and the page's sheets.
pub fn sheets(styles: &[String]) -> (Stylesheet, Vec<Stylesheet>) {
    (parse_stylesheet(UA_CSS), styles.iter().map(|s| parse_stylesheet(s)).collect())
}

impl<'a> Styler<'a> {
    pub fn new(ua: &'a Stylesheet, author: &'a [Stylesheet], env: Environment) -> Self {
        let mut rules = RuleSet::new();
        let supports = |_: &Declaration| true;
        let import = |_: &str| -> Option<&'a Stylesheet> { None };
        let cond = Conditions { env: &env, supports: &supports, import: &import };
        rules.add_sheet(ua, Origin::UserAgent, &cond);
        for s in author {
            rules.add_sheet(s, Origin::Author, &cond);
        }
        Styler { rules, env }
    }

    /// Compute every element of `dom` in tree order (index-aligned with `dom.nodes`).
    pub fn compute_all(&self, dom: &Dom) -> Vec<Computed> {
        let mut out: Vec<Option<Computed>> = vec![None; dom.nodes.len()];
        for i in dom.preorder(0) {
            let parent = dom.nodes[i].parent.map(|p| out[p].clone().unwrap());
            let root_fs = if i == 0 { None } else { out[0].as_ref().map(|c| c.font_size) };
            out[i] = Some(self.compute(dom.el(i), parent.as_ref(), root_fs));
        }
        out.into_iter().map(|c| c.unwrap()).collect()
    }

    pub fn compute(&self, el: El, parent: Option<&Computed>, root_font_size: Option<f64>) -> Computed {
        let init = Computed::initial();
        let p = parent.unwrap_or(&init);
        let inline = el.attr("style").map(|s| parse_style_attribute(&s)).unwrap_or_default();
        let applied = cascade(&self.rules, &el, &inline, None);
        // custom properties first (their own substitution and cycle rules)
        let customs_spec: Vec<(String, Vec<CV>)> =
            applied.iter().filter(|a| a.declaration.name.starts_with("--")).map(|a| (a.declaration.name.clone(), a.declaration.value.clone())).collect();
        let customs = compute_custom_properties(&customs_spec, &p.customs);
        // longhand winners
        let mut won: BTreeMap<&'static str, Cascaded> = BTreeMap::new();
        for a in &applied {
            let d = a.declaration;
            if d.name.starts_with("--") {
                continue;
            }
            let name = d.name.to_ascii_lowercase();
            if contains_var(&d.value) {
                // pending substitution: an invalid result makes every longhand unset
                match substitute_vars(&d.value, &|n| customs.get(n).cloned()) {
                    Ok(v) => match expand(&name, &v) {
                        Some(ls) => {
                            for (l, val) in ls {
                                if valid(l, &val) {
                                    won.insert(l, wide(&val).unwrap_or(Cascaded::Value(val)));
                                } else {
                                    won.insert(l, Cascaded::Value(vec![CV::Token(Token::Ident("unset".into()))]));
                                }
                            }
                        }
                        None => {
                            if let Some(ls) = expand(&name, &[CV::Token(Token::Ident("unset".into()))]) {
                                for (l, val) in ls {
                                    won.insert(l, Cascaded::Value(val));
                                }
                            }
                        }
                    },
                    Err(()) => {
                        if let Some(ls) = expand(&name, &[CV::Token(Token::Ident("unset".into()))]) {
                            for (l, val) in ls {
                                won.insert(l, Cascaded::Value(val));
                            }
                        }
                    }
                }
                continue;
            }
            let Some(ls) = expand(&name, &d.value) else { continue };
            // a shorthand is all-or-nothing at parse time
            if !ls.iter().all(|(l, val)| wide(val).is_some() || valid(l, val)) {
                continue;
            }
            for (l, val) in ls {
                won.insert(l, wide(&val).unwrap_or(Cascaded::Value(val)));
            }
        }
        // resolve CSS-wide keywords per property
        let get = |n: &str| -> Option<Vec<CV>> {
            match won.get(n) {
                None => {
                    if INHERITED.contains(&n) { None } else { Some(vec![CV::Token(Token::Ident("initial".into()))]) }
                }
                Some(Cascaded::Inherit) => None,
                Some(Cascaded::Initial) => Some(vec![CV::Token(Token::Ident("initial".into()))]),
                Some(Cascaded::Value(v)) => {
                    if ident_lower(v).as_deref() == Some("unset") {
                        if INHERITED.contains(&n) { None } else { Some(vec![CV::Token(Token::Ident("initial".into()))]) }
                    } else {
                        Some(v.clone())
                    }
                }
            }
        };
        let is_initial = |v: &Option<Vec<CV>>| matches!(v, Some(x) if ident_lower(x).as_deref() == Some("initial"));
        let mut c = Computed::initial();
        c.customs = customs;
        let vw = self.env.width;
        let vh = self.env.height;

        // font-family, then font-size (the monospace size rule needs the family)
        let ff = get("font-family");
        c.font_family = match &ff {
            None => p.font_family.clone(),
            v if is_initial(v) => init.font_family.clone(),
            Some(v) => family(v).unwrap_or(p.font_family.clone()),
        };
        let mono = c.font_family.len() == 1 && c.font_family[0] == "monospace";
        let base = |m: bool| if m { 13.0 } else { 16.0 };
        let fs = get("font-size");
        let (size, kw) = match &fs {
            None => match p.font_kw {
                Some(k) => (base(mono) * k, Some(k)),
                None => (p.font_size, None),
            },
            v if is_initial(v) => (base(mono), Some(1.0)),
            Some(v) => font_size(v, p, mono, root_font_size, vw, vh).unwrap_or((p.font_size, p.font_kw)),
        };
        c.font_size = size;
        c.font_kw = kw;
        let rem = root_font_size.unwrap_or(c.font_size);
        let lcx = LengthContext { font_size: c.font_size, root_font_size: rem, viewport_width: vw, viewport_height: vh, percent_basis: None };

        // color (inherited); currentcolor = the parent's color
        c.color = match get("color") {
            None => p.color,
            v if is_initial(&v) => init.color,
            Some(v) => match parse_color(&v) {
                Some(Color::Rgba(x)) => x,
                _ => p.color,
            },
        };
        let colorish = |n: &str, initial: Rgba, cur: Rgba, inherited: Rgba| -> Rgba {
            match get(n) {
                None => inherited,
                v if is_initial(&v) => initial,
                Some(v) => match parse_color(&v) {
                    Some(Color::Rgba(x)) => x,
                    Some(Color::CurrentColor) => cur,
                    None => initial,
                },
            }
        };
        c.background_color = colorish("background-color", init.background_color, c.color, p.background_color);
        c.border_left_color = colorish("border-left-color", c.color, c.color, p.border_left_color);

        let kw = |n: &str, parent_v: &str, initial: &str| -> String {
            match get(n) {
                None => parent_v.to_string(),
                v if is_initial(&v) => initial.to_string(),
                Some(v) => keyword_value(n, &v).unwrap_or_else(|| initial.to_string()),
            }
        };
        c.position = kw("position", &p.position, "static");
        c.font_style = kw("font-style", &p.font_style, "normal");
        c.text_align = kw("text-align", &p.text_align, "start");
        c.text_decoration_line = kw("text-decoration-line", &p.text_decoration_line, "none");
        c.white_space = kw("white-space", &p.white_space, "normal");
        c.visibility = kw("visibility", &p.visibility, "visible");
        c.list_style_type = kw("list-style-type", &p.list_style_type, "disc");
        c.border_top_style = kw("border-top-style", &p.border_top_style, "none");
        c.flex_direction = kw("flex-direction", &p.flex_direction, "row");
        let display = kw("display", &p.display, "inline");
        // CSS Display §2.7 blockification: the root, absolutely positioned boxes, flex / grid items
        let parent_display = parent.map(|x| x.display.as_str()).unwrap_or("block");
        let blockify = parent.is_none()
            || matches!(c.position.as_str(), "absolute" | "fixed")
            || matches!(parent_display, "flex" | "inline-flex" | "grid" | "inline-grid");
        c.display = if blockify { blockified(&display) } else { display };

        c.font_weight = match get("font-weight") {
            None => p.font_weight,
            v if is_initial(&v) => 400.0,
            Some(v) => font_weight(&v, p.font_weight).unwrap_or(p.font_weight),
        };
        c.line_height = match get("line-height") {
            None => p.line_height,
            v if is_initial(&v) => LineHeight::Normal,
            Some(v) => line_height(&v, &lcx).unwrap_or(p.line_height),
        };
        c.border_top_width = if matches!(c.border_top_style.as_str(), "none" | "hidden") {
            0.0
        } else {
            let w = match get("border-top-width") {
                None => p.border_top_width,
                v if is_initial(&v) => 3.0,
                Some(v) => border_width(&v, &lcx).unwrap_or(3.0),
            };
            // Chromium snaps border widths to whole device pixels (at least 1px when non-zero)
            if w > 0.0 && w < 1.0 { 1.0 } else { w.floor() }
        };
        c.padding_top = match get("padding-top") {
            v if is_initial(&v) => "0px".into(),
            Some(v) => length_pct(&v, &lcx, false).unwrap_or("0px".into()),
            None => p.padding_top.clone(),
        };
        // Chromium's LayoutTheme drops the padding of natively drawn checkboxes and radio buttons
        if el.local_name() == "input" && matches!(el.attr("type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("checkbox" | "radio")) {
            c.padding_top = "0px".into();
        }
        c.margin_top = match get("margin-top") {
            v if is_initial(&v) => "0px".into(),
            Some(v) => length_pct(&v, &lcx, true).unwrap_or("0px".into()),
            None => p.margin_top.clone(),
        };
        c
    }
}

fn blockified(d: &str) -> String {
    match d {
        "inline" | "inline-block" | "table-row-group" | "table-column" | "table-column-group" | "table-header-group"
        | "table-footer-group" | "table-row" | "table-cell" | "table-caption" | "run-in" => "block".into(),
        "inline-flex" => "flex".into(),
        "inline-grid" => "grid".into(),
        "inline-table" => "table".into(),
        "inline-list-item" => "list-item".into(),
        d => d.into(),
    }
}

const KEYWORDS: &[(&str, &[&str])] = &[
    ("display", &["inline", "block", "inline-block", "flex", "inline-flex", "grid", "inline-grid", "table", "inline-table",
        "table-row", "table-cell", "table-row-group", "table-header-group", "table-footer-group", "table-column",
        "table-column-group", "table-caption", "list-item", "none", "contents", "flow-root", "ruby", "ruby-text"]),
    ("position", &["static", "relative", "absolute", "fixed", "sticky"]),
    ("font-style", &["normal", "italic", "oblique"]),
    ("text-align", &["start", "end", "left", "right", "center", "justify", "match-parent", "-webkit-center", "-webkit-left", "-webkit-right", "-internal-center"]),
    ("white-space", &["normal", "pre", "nowrap", "pre-wrap", "pre-line", "break-spaces"]),
    ("visibility", &["visible", "hidden", "collapse"]),
    ("border-top-style", &["none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset", "outset"]),
    ("flex-direction", &["row", "row-reverse", "column", "column-reverse"]),
    ("list-style-type", &["disc", "circle", "square", "decimal", "decimal-leading-zero", "lower-roman", "upper-roman",
        "lower-alpha", "upper-alpha", "lower-latin", "upper-latin", "lower-greek", "none", "disclosure-open", "disclosure-closed"]),
];

fn keyword_value(n: &str, v: &[CV]) -> Option<String> {
    if n == "text-decoration-line" {
        let ws: Vec<String> = words(v).iter().map(|c| c.ident().map(|s| s.to_ascii_lowercase())).collect::<Option<Vec<_>>>()?;
        if ws == ["none"] {
            return Some("none".into());
        }
        let order = ["underline", "overline", "line-through", "blink"];
        if ws.iter().any(|w| !order.contains(&w.as_str())) || ws.is_empty() {
            return None;
        }
        return Some(order.iter().filter(|o| ws.iter().any(|w| w == *o)).cloned().collect::<Vec<_>>().join(" "));
    }
    if n == "display" {
        // two-value syntax for the common pairs
        let ws: Vec<String> = words(v).iter().map(|c| c.ident().map(|s| s.to_ascii_lowercase())).collect::<Option<Vec<_>>>()?;
        if ws.len() == 2 {
            return match (ws[0].as_str(), ws[1].as_str()) {
                ("block", "flow") | ("flow", "block") => Some("block".into()),
                ("inline", "flow") | ("flow", "inline") => Some("inline".into()),
                ("inline", "flow-root") => Some("inline-block".into()),
                ("block", "flex") | ("flex", "block") => Some("flex".into()),
                ("inline", "flex") | ("flex", "inline") => Some("inline-flex".into()),
                ("block", "grid") | ("grid", "block") => Some("grid".into()),
                ("inline", "grid") | ("grid", "inline") => Some("inline-grid".into()),
                _ => None,
            };
        }
    }
    let k = ident_lower(v)?;
    let allowed = KEYWORDS.iter().find(|(p, _)| *p == n)?.1;
    if !allowed.contains(&k.as_str()) {
        return None;
    }
    Some(match (n, k.as_str()) {
        ("text-align", "-internal-center") => "center".into(),
        ("text-align", "match-parent") => "start".into(),
        _ => k,
    })
}

/// Does this value parse for the longhand? (Parse-time validity.)
fn valid(n: &str, v: &[CV]) -> bool {
    if wide(v).is_some() {
        return true;
    }
    let lcx = LengthContext { font_size: 16.0, root_font_size: 16.0, viewport_width: 800.0, viewport_height: 600.0, percent_basis: Some(100.0) };
    match n {
        "color" | "background-color" | "border-left-color" => parse_color(v).is_some(),
        "font-family" => family(v).is_some(),
        "font-size" => font_size(v, &Computed::initial(), false, Some(16.0), 800.0, 600.0).is_some(),
        "font-weight" => font_weight(v, 400.0).is_some(),
        "line-height" => line_height(v, &lcx).is_some(),
        "border-top-width" => border_width(v, &lcx).is_some(),
        "padding-top" => length_pct(v, &lcx, false).is_some(),
        "margin-top" => length_pct(v, &lcx, true).is_some(),
        n => keyword_value(n, v).is_some(),
    }
}

const GENERIC: &[&str] = &["serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui", "math", "emoji", "fangsong", "ui-serif", "ui-sans-serif", "ui-monospace", "ui-rounded"];

/// `font-family` serialized as Chromium does: generics bare, a name bare when it is one identifier,
/// otherwise double-quoted.
fn family(v: &[CV]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for part in v.split(|c| matches!(c, CV::Token(Token::Comma))) {
        match trim_ws(part) {
            [CV::Token(Token::String(s))] => out.push(format!("\"{s}\"")),
            p if !p.is_empty() => {
                let mut names = Vec::new();
                for c in p {
                    match c {
                        CV::Token(Token::Ident(s)) => names.push(s.clone()),
                        c if c.is_whitespace() => {}
                        _ => return None,
                    }
                }
                if names.len() == 1 && GENERIC.contains(&names[0].to_ascii_lowercase().as_str()) {
                    out.push(names[0].to_ascii_lowercase());
                } else if names.len() == 1 {
                    out.push(names[0].clone());
                } else {
                    out.push(format!("\"{}\"", names.join(" ")));
                }
            }
            _ => return None,
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn resolve_len(v: &[CV], cx: &LengthContext) -> Option<Resolved> {
    match trim_ws(v) {
        [CV::Token(Token::Dimension(n, u))] => Some(Resolved::LengthPercentage { px: cx.length(n.value, u)?, pct: 0.0, has_pct: false, has_len: true }),
        [CV::Token(Token::Percentage(n))] => match cx.percent_basis {
            Some(b) => Some(Resolved::LengthPercentage { px: b * n.value / 100.0, pct: 0.0, has_pct: false, has_len: true }),
            None => Some(Resolved::LengthPercentage { px: 0.0, pct: n.value, has_pct: true, has_len: false }),
        },
        [CV::Token(Token::Number(n))] if n.value == 0.0 => Some(Resolved::LengthPercentage { px: 0.0, pct: 0.0, has_pct: false, has_len: true }),
        [f @ CV::Function(..)] => parse_math(f)?.resolve(cx),
        _ => None,
    }
}

/// Returns (px, keyword factor).
fn font_size(v: &[CV], p: &Computed, mono: bool, root: Option<f64>, vw: f64, vh: f64) -> Option<(f64, Option<f64>)> {
    let base = if mono { 13.0 } else { 16.0 };
    if let Some(k) = ident_lower(v) {
        let f = match k.as_str() {
            "xx-small" => 9.0 / 16.0,
            "x-small" => 10.0 / 16.0,
            "small" => 13.0 / 16.0,
            "medium" => 1.0,
            "large" => 18.0 / 16.0,
            "x-large" => 1.5,
            "xx-large" => 2.0,
            "xxx-large" => 3.0,
            "larger" => return Some((p.font_size * 1.2, p.font_kw.map(|k| k * 1.2))),
            "smaller" => return Some((p.font_size / 1.2, p.font_kw.map(|k| k / 1.2))),
            _ => return None,
        };
        return Some((base * f, Some(f)));
    }
    // em and % keep a keyword-derived size keyword-derived (Blink), scaled
    let parent_base = match p.font_kw {
        Some(k) => base * k,
        None => p.font_size,
    };
    let cx = LengthContext { font_size: parent_base, root_font_size: root.unwrap_or(parent_base), viewport_width: vw, viewport_height: vh, percent_basis: Some(parent_base) };
    let w = trim_ws(v);
    let relative = match w {
        [CV::Token(Token::Dimension(_, u))] => u.eq_ignore_ascii_case("em"),
        [CV::Token(Token::Percentage(_))] => true,
        _ => false,
    };
    let r = resolve_len(v, &cx)?;
    let px = r.px()?;
    if px < 0.0 {
        return None;
    }
    if relative {
        Some((px, p.font_kw.map(|k| k * px / parent_base)))
    } else {
        Some((px, None))
    }
}

fn font_weight(v: &[CV], parent: f64) -> Option<f64> {
    match trim_ws(v) {
        [CV::Token(Token::Number(n))] if n.value >= 1.0 && n.value <= 1000.0 => Some(n.value),
        _ => Some(match ident_lower(v)?.as_str() {
            "normal" => 400.0,
            "bold" => 700.0,
            "bolder" => {
                if parent < 350.0 { 400.0 } else if parent < 550.0 { 700.0 } else { 900.0 }
            }
            "lighter" => {
                if parent < 100.0 { parent } else if parent < 550.0 { 100.0 } else if parent < 750.0 { 400.0 } else { 700.0 }
            }
            _ => return None,
        }),
    }
}

fn line_height(v: &[CV], cx: &LengthContext) -> Option<LineHeight> {
    if ident_lower(v).as_deref() == Some("normal") {
        return Some(LineHeight::Normal);
    }
    if let [CV::Token(Token::Number(n))] = trim_ws(v) {
        return if n.value >= 0.0 { Some(LineHeight::Number(n.value)) } else { None };
    }
    let cx = LengthContext { percent_basis: Some(cx.font_size), ..*cx };
    match resolve_len(v, &cx)? {
        Resolved::Number(n) => Some(LineHeight::Number(n)),
        r => Some(LineHeight::Px(r.px()?)),
    }
}

fn border_width(v: &[CV], cx: &LengthContext) -> Option<f64> {
    match ident_lower(v).as_deref() {
        Some("thin") => Some(1.0),
        Some("medium") => Some(3.0),
        Some("thick") => Some(5.0),
        _ => {
            let px = resolve_len(v, &LengthContext { percent_basis: None, ..*cx })?.px()?;
            if px < 0.0 { None } else { Some(px) }
        }
    }
}

/// A `<length-percentage>` (or `auto` for margins) as Chromium reports it: px; a percentage that needs
/// layout is reported as `%` (the oracle then disagrees, honestly).
fn length_pct(v: &[CV], cx: &LengthContext, auto_ok: bool) -> Option<String> {
    if ident_lower(v).as_deref() == Some("auto") {
        return if auto_ok { Some("0px".into()) } else { None };
    }
    let cx = LengthContext { percent_basis: None, ..*cx };
    match resolve_len(v, &cx)? {
        Resolved::LengthPercentage { px: p, pct, has_pct, .. } => {
            if !auto_ok && (p < 0.0 || pct < 0.0) {
                return None;
            }
            if has_pct && pct != 0.0 {
                Some(format!("{}%+{}px", num(pct), num(p)))
            } else {
                Some(px(p))
            }
        }
        _ => None,
    }
}

/// Chromium's string vs ours: px values within 0.01px, everything else exact.
pub fn agree(chromium: &str, ours: &str) -> bool {
    if chromium == ours {
        return true;
    }
    match (chromium.strip_suffix("px"), ours.strip_suffix("px")) {
        (Some(a), Some(b)) => match (a.parse::<f64>(), b.parse::<f64>()) {
            (Ok(x), Ok(y)) => (x - y).abs() < 0.01,
            _ => false,
        },
        _ => false,
    }
}
