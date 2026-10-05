//! A stylesheet as data: style rules (with CSS Nesting), `@media`, `@supports`, `@layer`, `@import`,
//! `@namespace`, `@font-face` and `@keyframes`, built from the Syntax-level rules.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::media::{parse_media_query_list, MediaQueryList};
use crate::parser::{
    declarations_of, parse_block_contents, parse_component_values, parse_declaration_cvs, parse_rule_list_cvs, trim_ws, AtRule,
    BlockItem, BlockKind, Declaration, Rule, CV,
};
use crate::selectors::{parse_complex, parse_nested_selector_list, parse_selector_list, Namespaces, ParseContext, SelectorList};
use crate::tokenizer::Token;

#[derive(Clone, Debug, PartialEq)]
pub struct StyleRule {
    pub selectors: SelectorList,
    pub declarations: Vec<Declaration>,
    /// Nested rules in order: nested style rules, conditional rules, and "nested declarations" rules
    /// (declarations that follow a nested rule, carried by a rule with the parent's selectors).
    pub children: Vec<CssRule>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SupportsCondition {
    Not(Box<SupportsCondition>),
    And(Vec<SupportsCondition>),
    Or(Vec<SupportsCondition>),
    Declaration(Declaration),
    Selector(Vec<CV>),
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportRule {
    pub url: String,
    pub media: MediaQueryList,
    /// `None`: no layer; `Some(None)`: an anonymous layer; `Some(Some(name))`.
    pub layer: Option<Option<String>>,
    pub supports: Option<SupportsCondition>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FontSource {
    Url { url: String, format: Option<String> },
    Local(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub descriptors: Vec<Declaration>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    /// Offsets in 0..=1 (`from` = 0, `to` = 1).
    pub offsets: Vec<f64>,
    pub declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keyframes {
    pub name: String,
    pub frames: Vec<Keyframe>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CssRule {
    Style(StyleRule),
    Media(MediaQueryList, Vec<CssRule>),
    Supports(SupportsCondition, Vec<CssRule>),
    /// `@layer name { … }` (`None`: anonymous).
    LayerBlock(Option<String>, Vec<CssRule>),
    /// `@layer a, b;`
    LayerStatement(Vec<String>),
    Import(ImportRule),
    FontFace(FontFace),
    Keyframes(Keyframes),
    /// Any other at-rule (`@page`, `@property`, `@counter-style`, `@container`…), kept as syntax.
    Other(AtRule),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Stylesheet {
    pub rules: Vec<CssRule>,
    pub namespaces: Namespaces,
}

/// Parse a stylesheet.
pub fn parse_stylesheet(css: &str) -> Stylesheet {
    let cvs = parse_component_values(css);
    let mut sheet = Stylesheet::default();
    let rules: Vec<Rule> = parse_rule_list_cvs(&cvs, true).into_iter().filter_map(|r| r.ok()).collect();
    // @import is valid only before any rule but @charset / @layer statements; @namespace before others.
    let mut phase = 0; // 0: imports allowed, 1: namespaces allowed, 2: body
    for r in rules {
        if let Rule::At(a) = &r {
            let n = a.name.to_ascii_lowercase();
            match n.as_str() {
                "charset" => continue,
                "import" => {
                    if phase == 0
                        && let Some(i) = import_rule(a) {
                            sheet.rules.push(CssRule::Import(i));
                        }
                    continue;
                }
                "namespace" => {
                    if phase <= 1 {
                        phase = 1;
                        namespace_rule(a, &mut sheet.namespaces);
                    }
                    continue;
                }
                "layer" if a.block.is_none() => {
                    if let Some(names) = layer_names(&a.prelude) {
                        sheet.rules.push(CssRule::LayerStatement(names));
                    }
                    continue;
                }
                _ => {}
            }
        }
        phase = 2;
        let ns = sheet.namespaces.clone();
        if let Some(c) = convert(r, &ns, None) {
            sheet.rules.push(c);
        }
    }
    sheet
}

/// The declarations of a `style` attribute.
pub fn parse_style_attribute(s: &str) -> Vec<Declaration> {
    declarations_of(&parse_component_values(s))
}

fn convert(r: Rule, ns: &Namespaces, parent: Option<&SelectorList>) -> Option<CssRule> {
    match r {
        Rule::Qualified(q) => {
            let cx = ParseContext { nesting_parent: parent, ..ParseContext::new(ns) };
            let selectors = match parent {
                None => parse_selector_list(&q.prelude, &cx).ok()?,
                Some(_) => parse_nested_selector_list(&q.prelude, &cx).ok()?,
            };
            Some(CssRule::Style(style_body(selectors, &q.block, ns)))
        }
        Rule::At(a) => at_rule(a, ns, parent),
    }
}

/// A style rule's block: its declarations, then nested rules in order.
fn style_body(selectors: SelectorList, block: &[CV], ns: &Namespaces) -> StyleRule {
    let mut declarations = Vec::new();
    let mut children = Vec::new();
    let mut pending: Vec<Declaration> = Vec::new();
    for item in parse_block_contents(block).into_iter().filter_map(|r| r.ok()) {
        match item {
            BlockItem::Declaration(d) => {
                if children.is_empty() {
                    declarations.push(d);
                } else {
                    pending.push(d);
                }
            }
            BlockItem::Rule(r) => {
                if !pending.is_empty() {
                    children.push(nested_decls(&selectors, core::mem::take(&mut pending)));
                }
                if let Some(c) = convert(r, ns, Some(&selectors)) {
                    children.push(c);
                }
            }
        }
    }
    if !pending.is_empty() {
        children.push(nested_decls(&selectors, pending));
    }
    StyleRule { selectors, declarations, children }
}

fn nested_decls(parent: &SelectorList, declarations: Vec<Declaration>) -> CssRule {
    CssRule::Style(StyleRule { selectors: parent.clone(), declarations, children: Vec::new() })
}

/// The contents of a conditional group rule: rules at the top level, or (nested in a style rule)
/// block contents whose bare declarations apply to the parent.
fn group_contents(block: &[CV], ns: &Namespaces, parent: Option<&SelectorList>) -> Vec<CssRule> {
    match parent {
        None => parse_rule_list_cvs(block, false).into_iter().filter_map(|r| r.ok()).filter_map(|r| convert(r, ns, None)).collect(),
        Some(p) => {
            let body = style_body(p.clone(), block, ns);
            let mut out = Vec::new();
            if !body.declarations.is_empty() {
                out.push(nested_decls(p, body.declarations));
            }
            out.extend(body.children);
            out
        }
    }
}

fn at_rule(a: AtRule, ns: &Namespaces, parent: Option<&SelectorList>) -> Option<CssRule> {
    let n = a.name.to_ascii_lowercase();
    match n.as_str() {
        "media" => Some(CssRule::Media(parse_media_query_list(&a.prelude), group_contents(a.block.as_deref()?, ns, parent))),
        "supports" => Some(CssRule::Supports(supports_condition(&a.prelude)?, group_contents(a.block.as_deref()?, ns, parent))),
        "layer" => {
            let names = layer_names(&a.prelude)?;
            match &a.block {
                None => Some(CssRule::LayerStatement(names)),
                Some(b) => {
                    if names.len() > 1 {
                        return None;
                    }
                    Some(CssRule::LayerBlock(names.into_iter().next(), group_contents(b, ns, parent)))
                }
            }
        }
        "font-face" if parent.is_none() => Some(CssRule::FontFace(FontFace { descriptors: declarations_of(a.block.as_deref()?) })),
        "keyframes" | "-webkit-keyframes" if parent.is_none() => keyframes(&a),
        _ => Some(CssRule::Other(a)),
    }
}

fn import_rule(a: &AtRule) -> Option<ImportRule> {
    let p = trim_ws(&a.prelude);
    let (url, mut rest) = match p.first()? {
        CV::Token(Token::String(s)) | CV::Token(Token::Url(s)) => (s.clone(), &p[1..]),
        CV::Function(f, args) if f.eq_ignore_ascii_case("url") => match trim_ws(args) {
            [CV::Token(Token::String(s))] => (s.clone(), &p[1..]),
            _ => return None,
        },
        _ => return None,
    };
    rest = trim_ws(rest);
    let mut layer = None;
    match rest.first() {
        Some(CV::Token(Token::Ident(l))) if l.eq_ignore_ascii_case("layer") => {
            layer = Some(None);
            rest = trim_ws(&rest[1..]);
        }
        Some(CV::Function(l, args)) if l.eq_ignore_ascii_case("layer") => {
            let names = layer_names(args)?;
            if names.len() != 1 {
                return None;
            }
            layer = Some(names.into_iter().next());
            rest = trim_ws(&rest[1..]);
        }
        _ => {}
    }
    let mut supports = None;
    if let Some(CV::Function(s, args)) = rest.first()
        && s.eq_ignore_ascii_case("supports") {
            supports = Some(supports_condition(args).or_else(|| parse_declaration_cvs(args).ok().map(SupportsCondition::Declaration))?);
            rest = trim_ws(&rest[1..]);
        }
    Some(ImportRule { url, media: parse_media_query_list(rest), layer, supports })
}

fn namespace_rule(a: &AtRule, ns: &mut Namespaces) {
    let parts: Vec<&CV> = a.prelude.iter().filter(|c| !c.is_whitespace()).collect();
    let url = |c: &CV| match c {
        CV::Token(Token::String(s)) | CV::Token(Token::Url(s)) => Some(s.clone()),
        CV::Function(f, args) if f.eq_ignore_ascii_case("url") => match trim_ws(args) {
            [CV::Token(Token::String(s))] => Some(s.clone()),
            _ => None,
        },
        _ => None,
    };
    match parts.as_slice() {
        [u] => {
            if let Some(u) = url(u) {
                ns.default = Some(u);
            }
        }
        [CV::Token(Token::Ident(p)), u] => {
            if let Some(u) = url(u) {
                ns.prefixes.push((p.clone(), u));
            }
        }
        _ => {}
    }
}

/// `<layer-name>#`: dotted ident sequences.
fn layer_names(v: &[CV]) -> Option<Vec<String>> {
    if trim_ws(v).is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for part in v.split(|c| matches!(c, CV::Token(Token::Comma))) {
        let p = trim_ws(part);
        let mut name = String::new();
        let mut want_ident = true;
        for c in p {
            match c {
                CV::Token(Token::Ident(s)) if want_ident => {
                    name.push_str(s);
                    want_ident = false;
                }
                c if c.is_delim('.') && !want_ident => {
                    name.push('.');
                    want_ident = true;
                }
                // `.b` tokenizes as a number only for digits; an ident after a dot arrives as Delim + Ident
                _ => return None,
            }
        }
        if want_ident {
            return None;
        }
        out.push(name);
    }
    Some(out)
}

/// `<supports-condition>`
pub fn supports_condition(v: &[CV]) -> Option<SupportsCondition> {
    let w: Vec<&CV> = v.iter().filter(|c| !c.is_whitespace()).collect();
    if w.is_empty() {
        return None;
    }
    if matches!(w[0].ident(), Some(n) if n.eq_ignore_ascii_case("not")) {
        if w.len() != 2 {
            return None;
        }
        return Some(SupportsCondition::Not(Box::new(supports_in_parens(w[1])?)));
    }
    let first = supports_in_parens(w[0])?;
    if w.len() == 1 {
        return Some(first);
    }
    let op = w[1].ident()?.to_ascii_lowercase();
    if op != "and" && op != "or" {
        return None;
    }
    let mut items = alloc::vec![first];
    let mut i = 1;
    while i < w.len() {
        if !matches!(w[i].ident(), Some(o) if o.eq_ignore_ascii_case(&op)) {
            return None;
        }
        items.push(supports_in_parens(*w.get(i + 1)?)?);
        i += 2;
    }
    Some(if op == "and" { SupportsCondition::And(items) } else { SupportsCondition::Or(items) })
}

fn supports_in_parens(c: &CV) -> Option<SupportsCondition> {
    match c {
        CV::Block(BlockKind::Paren, inner) => {
            if let Some(cond) = supports_condition(inner) {
                return Some(cond);
            }
            match parse_declaration_cvs(inner) {
                Ok(d) => Some(SupportsCondition::Declaration(d)),
                Err(_) => Some(SupportsCondition::Unknown),
            }
        }
        CV::Function(f, args) if f.eq_ignore_ascii_case("selector") => Some(SupportsCondition::Selector(args.clone())),
        CV::Function(..) => Some(SupportsCondition::Unknown),
        _ => None,
    }
}

impl SupportsCondition {
    /// Evaluate; `decl` answers whether a property / value pair is supported.
    pub fn evaluate(&self, ns: &Namespaces, decl: &dyn Fn(&Declaration) -> bool) -> bool {
        match self {
            SupportsCondition::Not(c) => !c.evaluate(ns, decl),
            SupportsCondition::And(v) => v.iter().all(|c| c.evaluate(ns, decl)),
            SupportsCondition::Or(v) => v.iter().any(|c| c.evaluate(ns, decl)),
            SupportsCondition::Declaration(d) => decl(d),
            SupportsCondition::Selector(s) => parse_complex(s, &ParseContext::new(ns)).is_ok(),
            SupportsCondition::Unknown => false,
        }
    }
}

fn keyframes(a: &AtRule) -> Option<CssRule> {
    let name = match trim_ws(&a.prelude) {
        [CV::Token(Token::Ident(s))] if !matches!(s.to_ascii_lowercase().as_str(), "none" | "initial" | "inherit" | "unset" | "default") => s.clone(),
        [CV::Token(Token::String(s))] => s.clone(),
        _ => return None,
    };
    let mut frames = Vec::new();
    for r in parse_rule_list_cvs(a.block.as_deref()?, false).into_iter().filter_map(|r| r.ok()) {
        let Rule::Qualified(q) = r else { continue };
        let mut offsets = Vec::new();
        let mut ok = true;
        for part in q.prelude.split(|c| matches!(c, CV::Token(Token::Comma))) {
            match trim_ws(part) {
                [CV::Token(Token::Ident(s))] if s.eq_ignore_ascii_case("from") => offsets.push(0.0),
                [CV::Token(Token::Ident(s))] if s.eq_ignore_ascii_case("to") => offsets.push(1.0),
                [CV::Token(Token::Percentage(p))] if (0.0..=100.0).contains(&p.value) => offsets.push(p.value / 100.0),
                _ => ok = false,
            }
        }
        if ok {
            frames.push(Keyframe { offsets, declarations: declarations_of(&q.block).into_iter().filter(|d| !d.important).collect() });
        }
    }
    Some(CssRule::Keyframes(Keyframes { name, frames }))
}

impl FontFace {
    pub fn descriptor(&self, name: &str) -> Option<&[CV]> {
        self.descriptors.iter().rev().find(|d| d.name.eq_ignore_ascii_case(name)).map(|d| trim_ws(&d.value))
    }
    /// `font-family`: a string, or idents joined by single spaces.
    pub fn family(&self) -> Option<String> {
        match self.descriptor("font-family")? {
            [CV::Token(Token::String(s))] => Some(s.clone()),
            v => {
                let mut out = String::new();
                for c in v {
                    match c {
                        CV::Token(Token::Ident(s)) => {
                            if !out.is_empty() {
                                out.push(' ');
                            }
                            out.push_str(s);
                        }
                        c if c.is_whitespace() => {}
                        _ => return None,
                    }
                }
                if out.is_empty() { None } else { Some(out) }
            }
        }
    }
    /// `src`: `url(…) [format(…)]` and `local(…)` entries in order.
    pub fn sources(&self) -> Vec<FontSource> {
        let mut out = Vec::new();
        let Some(v) = self.descriptor("src") else { return out };
        for part in v.split(|c| matches!(c, CV::Token(Token::Comma))) {
            let p: Vec<&CV> = part.iter().filter(|c| !c.is_whitespace()).collect();
            let fmt = p.iter().find_map(|c| match c {
                CV::Function(f, args) if f.eq_ignore_ascii_case("format") => trim_ws(args).first().and_then(|a| match a {
                    CV::Token(Token::String(s)) | CV::Token(Token::Ident(s)) => Some(s.clone()),
                    _ => None,
                }),
                _ => None,
            });
            match p.first() {
                Some(CV::Token(Token::Url(u))) => out.push(FontSource::Url { url: u.clone(), format: fmt }),
                Some(CV::Function(f, args)) if f.eq_ignore_ascii_case("url") => {
                    if let [CV::Token(Token::String(u))] = trim_ws(args) {
                        out.push(FontSource::Url { url: u.clone(), format: fmt });
                    }
                }
                Some(CV::Function(f, args)) if f.eq_ignore_ascii_case("local") => match trim_ws(args) {
                    [CV::Token(Token::String(s))] => out.push(FontSource::Local(s.clone())),
                    a => {
                        let names: Vec<&str> = a.iter().filter_map(|c| c.ident()).collect();
                        if !names.is_empty() {
                            out.push(FontSource::Local(names.join(" ")));
                        }
                    }
                },
                _ => {}
            }
        }
        out
    }
}
