//! Selectors Level 4: the grammar (§3–§16) parsed over component values, and specificity (§17).
//!
//! Covered: type / universal selectors with namespace prefixes (`ns|E`, `*|E`, `|E`), `#id`, `.class`,
//! attribute selectors with every operator and the `i` / `s` flags, the logical combinations `:not()`
//! `:is()` `:where()` `:has()`, the child-indexed `:nth-*()` (An+B, `of S`), `:first-child` … `:only-of-type`,
//! `:root` `:empty` `:scope`, the user-action / location / input pseudo-classes as state hooks,
//! `:lang()` `:dir()`, pseudo-elements (`::before` `::after` `::first-line` `::first-letter` `::marker`
//! `::placeholder` `::selection` `::backdrop` `::file-selector-button`, plus the legacy one-colon forms),
//! the combinators ` ` `>` `+` `~`, relative selectors, and CSS Nesting's `&`.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::anb::parse_anb;
use crate::parser::{parse_component_values, trim_ws, BlockKind, CV};
use crate::tokenizer::Token;

pub const HTML_NS: &str = "http://www.w3.org/1999/xhtml";
pub const SVG_NS: &str = "http://www.w3.org/2000/svg";
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// `@namespace` declarations in force for a stylesheet (or none for the Selectors API).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Namespaces {
    pub default: Option<String>,
    pub prefixes: Vec<(String, String)>,
}

impl Namespaces {
    pub fn resolve(&self, prefix: &str) -> Option<&str> {
        self.prefixes.iter().rev().find(|(p, _)| p == prefix).map(|(_, u)| u.as_str())
    }
}

/// A namespace constraint on an element or attribute.
#[derive(Clone, Debug, PartialEq)]
pub enum Ns {
    /// `*|` (or no prefix with no default namespace, for elements).
    Any,
    /// `|`: the null namespace.
    Null,
    Url(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrOp {
    Exists,
    /// `=`
    Equals,
    /// `~=`
    Includes,
    /// `|=`
    DashMatch,
    /// `^=`
    Prefix,
    /// `$=`
    Suffix,
    /// `*=`
    Substring,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseFlag {
    /// No flag: case-sensitive, except HTML's legacy case-insensitive attributes on HTML elements.
    Default,
    /// `i`
    Insensitive,
    /// `s`
    Sensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NthKind {
    Child,
    LastChild,
    OfType,
    LastOfType,
}

/// The element states a host reports through [`crate::matching::Element::state`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementState {
    Hover,
    Active,
    Focus,
    FocusVisible,
    FocusWithin,
    Link,
    Visited,
    Target,
    Checked,
    Indeterminate,
    Default,
    Enabled,
    Disabled,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    Valid,
    Invalid,
    InRange,
    OutOfRange,
    Defined,
    Open,
    Autofill,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PseudoClass {
    Not(SelectorList),
    Is(SelectorList),
    Where(SelectorList),
    Has(Vec<RelativeSelector>),
    Nth { kind: NthKind, a: i32, b: i32, of: Option<SelectorList> },
    OnlyChild,
    OnlyOfType,
    Root,
    Empty,
    Scope,
    AnyLink,
    Lang(Vec<String>),
    /// `:dir(rtl)` when true.
    Dir(bool),
    State(ElementState),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PseudoElement {
    Before,
    After,
    FirstLine,
    FirstLetter,
    Marker,
    Placeholder,
    Selection,
    Backdrop,
    FileSelectorButton,
    /// `::slotted(<compound-selector>)`
    Slotted(Vec<Simple>),
    /// `::part(<ident>+)`
    Part(Vec<String>),
    /// `::highlight(<custom-ident>)`
    Highlight(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Simple {
    Type { ns: Ns, name: String, lower: String },
    Universal { ns: Ns },
    Id(String),
    Class(String),
    Attr { ns: Ns, name: String, lower: String, op: AttrOp, value: String, case: CaseFlag },
    Pseudo(PseudoClass),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Specificity {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

impl Specificity {
    pub const ZERO: Specificity = Specificity { a: 0, b: 0, c: 0 };
    fn add(self, o: Specificity) -> Specificity {
        Specificity { a: self.a + o.a, b: self.b + o.b, c: self.c + o.c }
    }
}
impl PartialOrd for Specificity {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Specificity {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.a, self.b, self.c).cmp(&(o.a, o.b, o.c))
    }
}

/// A complex selector: `compounds[k]` and `compounds[k+1]` are joined by `combinators[k]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub compounds: Vec<Vec<Simple>>,
    pub combinators: Vec<Combinator>,
    pub pseudo_element: Option<PseudoElement>,
    pub specificity: Specificity,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectorList(pub Vec<Selector>);

impl SelectorList {
    pub fn max_specificity(&self) -> Specificity {
        self.0.iter().map(|s| s.specificity).max().unwrap_or(Specificity::ZERO)
    }
}

/// A relative selector (`:has()` arguments, nested style rules): the leading combinator binds the
/// selector's leftmost compound to the anchor element.
#[derive(Clone, Debug, PartialEq)]
pub struct RelativeSelector {
    pub combinator: Combinator,
    pub selector: Selector,
}

/// What the selector being parsed may contain.
#[derive(Clone, Copy)]
pub struct ParseContext<'a> {
    pub namespaces: &'a Namespaces,
    /// The parent rule's selector list when parsing a nested style rule (`&` resolves to `:is(parent)`).
    pub nesting_parent: Option<&'a SelectorList>,
    /// Inside a `:has()` argument (`:has()` may not nest) or another logical pseudo (no pseudo-elements).
    pub in_has: bool,
    pub in_logical: bool,
}

impl<'a> ParseContext<'a> {
    pub fn new(namespaces: &'a Namespaces) -> Self {
        ParseContext { namespaces, nesting_parent: None, in_has: false, in_logical: false }
    }
}

type R<T> = Result<T, ()>;

fn ascii_lower(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

fn split_commas(v: &[CV]) -> Vec<&[CV]> {
    v.split(|c| matches!(c, CV::Token(Token::Comma))).collect()
}

/// `<selector-list>` / `<complex-selector-list>`: every item must be valid.
pub fn parse_selector_list(v: &[CV], cx: &ParseContext) -> R<SelectorList> {
    let mut out = Vec::new();
    for part in split_commas(v) {
        out.push(parse_complex(part, cx)?);
    }
    Ok(SelectorList(out))
}

/// Convenience: parse a selector list from text (the Selectors API, no namespaces declared).
pub fn parse_selector_str(s: &str, ns: &Namespaces) -> R<SelectorList> {
    parse_selector_list(&parse_component_values(s), &ParseContext::new(ns))
}

/// `<forgiving-selector-list>` (`:is()`, `:where()`): invalid items are dropped.
fn parse_forgiving(v: &[CV], cx: &ParseContext) -> SelectorList {
    if trim_ws(v).is_empty() {
        return SelectorList(Vec::new());
    }
    SelectorList(split_commas(v).into_iter().filter_map(|p| parse_complex(p, cx).ok()).collect())
}

/// `<relative-selector-list>` (`:has()`).
pub fn parse_relative_list(v: &[CV], cx: &ParseContext) -> R<Vec<RelativeSelector>> {
    let mut out = Vec::new();
    for part in split_commas(v) {
        out.push(parse_relative(part, cx)?);
    }
    Ok(out)
}

/// One relative selector: an optional leading combinator (default: descendant).
pub fn parse_relative(part: &[CV], cx: &ParseContext) -> R<RelativeSelector> {
    let v = trim_ws(part);
    let (combinator, rest) = match v.first() {
        Some(c) if c.is_delim('>') => (Combinator::Child, &v[1..]),
        Some(c) if c.is_delim('+') => (Combinator::NextSibling, &v[1..]),
        Some(c) if c.is_delim('~') => (Combinator::SubsequentSibling, &v[1..]),
        _ => (Combinator::Descendant, v),
    };
    Ok(RelativeSelector { combinator, selector: parse_complex(rest, cx)? })
}

/// A nested style rule's selector list (CSS Nesting §2): each item is relative to the parent; an item
/// without `&` gets an implicit `& ` (or `& >` etc. when it starts with a combinator).
pub fn parse_nested_selector_list(v: &[CV], cx: &ParseContext) -> R<SelectorList> {
    let parent = cx.nesting_parent.ok_or(())?;
    let mut out = Vec::new();
    for part in split_commas(v) {
        let p = trim_ws(part);
        let has_amp = contains_amp(p);
        if has_amp && !p.first().map(|c| c.is_delim('>') || c.is_delim('+') || c.is_delim('~')).unwrap_or(false) {
            out.push(parse_complex(p, cx)?);
        } else {
            let rel = parse_relative(p, cx)?;
            let mut sel = rel.selector;
            let amp = Simple::Pseudo(PseudoClass::Is(parent.clone()));
            sel.compounds.insert(0, alloc::vec![amp]);
            sel.combinators.insert(0, rel.combinator);
            sel.specificity = specificity_of(&sel);
            out.push(sel);
        }
    }
    Ok(SelectorList(out))
}

fn contains_amp(v: &[CV]) -> bool {
    v.iter().any(|c| match c {
        CV::Token(Token::Delim('&')) => true,
        CV::Function(_, a) | CV::Block(_, a) => contains_amp(a),
        _ => false,
    })
}

struct P<'v, 'c> {
    v: &'v [CV],
    i: usize,
    cx: &'c ParseContext<'c>,
}

impl<'v, 'c> P<'v, 'c> {
    fn at(&self, k: usize) -> Option<&'v CV> {
        self.v.get(self.i + k)
    }
    fn skip_ws(&mut self) -> bool {
        let s = self.i;
        while matches!(self.at(0), Some(c) if c.is_whitespace()) {
            self.i += 1;
        }
        self.i > s
    }
}

/// `<complex-selector>`.
pub fn parse_complex(part: &[CV], cx: &ParseContext) -> R<Selector> {
    let v = trim_ws(part);
    if v.is_empty() {
        return Err(());
    }
    let mut p = P { v, i: 0, cx };
    let mut compounds = Vec::new();
    let mut combinators = Vec::new();
    let mut pseudo_element = None;
    loop {
        let (compound, pe) = parse_compound(&mut p)?;
        compounds.push(compound);
        if pe.is_some() {
            pseudo_element = pe;
        }
        let had_ws = p.skip_ws();
        let Some(c) = p.at(0) else { break };
        if pseudo_element.is_some() {
            return Err(());
        }
        let comb = if c.is_delim('>') {
            Combinator::Child
        } else if c.is_delim('+') {
            Combinator::NextSibling
        } else if c.is_delim('~') {
            Combinator::SubsequentSibling
        } else if had_ws {
            Combinator::Descendant
        } else {
            return Err(());
        };
        if comb != Combinator::Descendant {
            p.i += 1;
            p.skip_ws();
            if p.at(0).is_none() {
                return Err(());
            }
        }
        combinators.push(comb);
    }
    let mut sel = Selector { compounds, combinators, pseudo_element, specificity: Specificity::ZERO };
    sel.specificity = specificity_of(&sel);
    Ok(sel)
}

fn resolve_prefix(cx: &ParseContext, prefix: &str) -> R<Ns> {
    cx.namespaces.resolve(prefix).map(|u| Ns::Url(String::from(u))).ok_or(())
}

/// The optional `ns|` prefix and name of a type selector. `Ok(None)` when none starts here.
fn parse_type(p: &mut P) -> R<Option<Simple>> {
    // name_or_star: Some(Some(name)) for ident, Some(None) for '*'
    fn nm(c: Option<&CV>) -> Option<Option<String>> {
        match c {
            Some(CV::Token(Token::Ident(s))) => Some(Some(s.clone())),
            Some(CV::Token(Token::Delim('*'))) => Some(None),
            _ => None,
        }
    }
    let first = nm(p.at(0));
    let bar_at = |k: usize, p: &P| p.at(k).map(|c| c.is_delim('|')).unwrap_or(false);
    let (ns, name) = if let Some(f) = first.clone() {
        if bar_at(1, p) {
            let Some(n) = nm(p.at(2)) else { return Err(()) };
            let ns = match f {
                None => Ns::Any,
                Some(prefix) => resolve_prefix(p.cx, &prefix)?,
            };
            p.i += 3;
            (ns, n)
        } else {
            p.i += 1;
            let ns = match &p.cx.namespaces.default {
                Some(u) => Ns::Url(u.clone()),
                None => Ns::Any,
            };
            (ns, f)
        }
    } else if bar_at(0, p) {
        let Some(n) = nm(p.at(1)) else { return Err(()) };
        p.i += 2;
        (Ns::Null, n)
    } else {
        return Ok(None);
    };
    Ok(Some(match name {
        Some(n) => Simple::Type { ns, lower: ascii_lower(&n), name: n },
        None => Simple::Universal { ns },
    }))
}

fn parse_compound(p: &mut P) -> R<(Vec<Simple>, Option<PseudoElement>)> {
    let mut simples = Vec::new();
    let mut pe = None;
    if let Some(t) = parse_type(p)? {
        simples.push(t);
    }
    loop {
        let Some(c) = p.at(0) else { break };
        match c {
            CV::Token(Token::Hash { value, is_id }) => {
                if !is_id || pe.is_some() {
                    return Err(());
                }
                simples.push(Simple::Id(value.clone()));
                p.i += 1;
            }
            CV::Token(Token::Delim('.')) => {
                if pe.is_some() {
                    return Err(());
                }
                match p.at(1) {
                    Some(CV::Token(Token::Ident(n))) => {
                        simples.push(Simple::Class(n.clone()));
                        p.i += 2;
                    }
                    _ => return Err(()),
                }
            }
            CV::Block(BlockKind::Bracket, inner) => {
                if pe.is_some() {
                    return Err(());
                }
                simples.push(parse_attr(inner, p.cx)?);
                p.i += 1;
            }
            CV::Token(Token::Delim('&')) => {
                let parent = p.cx.nesting_parent.ok_or(())?;
                simples.push(Simple::Pseudo(PseudoClass::Is(parent.clone())));
                p.i += 1;
            }
            CV::Token(Token::Colon) => match p.at(1) {
                Some(CV::Token(Token::Colon)) => {
                    if pe.is_some() || p.cx.in_logical {
                        return Err(());
                    }
                    match p.at(2) {
                        Some(CV::Token(Token::Ident(n))) => {
                            pe = Some(pseudo_element(&ascii_lower(n)).ok_or(())?);
                            p.i += 3;
                        }
                        Some(CV::Function(n, args)) => {
                            pe = Some(functional_pseudo_element(&ascii_lower(n), args, p.cx)?);
                            p.i += 3;
                        }
                        _ => return Err(()),
                    }
                }
                Some(CV::Token(Token::Ident(n))) => {
                    let l = ascii_lower(n);
                    if let Some(legacy) = match l.as_str() {
                        "before" => Some(PseudoElement::Before),
                        "after" => Some(PseudoElement::After),
                        "first-line" => Some(PseudoElement::FirstLine),
                        "first-letter" => Some(PseudoElement::FirstLetter),
                        _ => None,
                    } {
                        if pe.is_some() || p.cx.in_logical {
                            return Err(());
                        }
                        pe = Some(legacy);
                    } else {
                        let pc = pseudo_class(&l).ok_or(())?;
                        // After a pseudo-element only the user-action pseudo-classes may follow.
                        if pe.is_some()
                            && !matches!(pc, PseudoClass::State(ElementState::Hover | ElementState::Active | ElementState::Focus))
                        {
                            return Err(());
                        }
                        simples.push(Simple::Pseudo(pc));
                    }
                    p.i += 2;
                }
                Some(CV::Function(n, args)) => {
                    if pe.is_some() {
                        return Err(());
                    }
                    simples.push(Simple::Pseudo(functional(&ascii_lower(n), args, p.cx)?));
                    p.i += 2;
                }
                _ => return Err(()),
            },
            _ => break,
        }
    }
    if simples.is_empty() && pe.is_none() {
        return Err(());
    }
    Ok((simples, pe))
}

fn pseudo_element(n: &str) -> Option<PseudoElement> {
    Some(match n {
        "before" => PseudoElement::Before,
        "after" => PseudoElement::After,
        "first-line" => PseudoElement::FirstLine,
        "first-letter" => PseudoElement::FirstLetter,
        "marker" => PseudoElement::Marker,
        "placeholder" => PseudoElement::Placeholder,
        "selection" => PseudoElement::Selection,
        "backdrop" => PseudoElement::Backdrop,
        "file-selector-button" => PseudoElement::FileSelectorButton,
        _ => return None,
    })
}

fn functional_pseudo_element(n: &str, args: &[CV], cx: &ParseContext) -> R<PseudoElement> {
    let v = trim_ws(args);
    match n {
        "slotted" => {
            let inner = ParseContext { in_logical: true, ..*cx };
            let mut p = P { v, i: 0, cx: &inner };
            let (compound, pe) = parse_compound(&mut p)?;
            if pe.is_some() || p.i != v.len() {
                return Err(());
            }
            Ok(PseudoElement::Slotted(compound))
        }
        "part" => {
            let mut names = Vec::new();
            for c in v {
                match c {
                    CV::Token(Token::Ident(s)) => names.push(s.clone()),
                    c if c.is_whitespace() => {}
                    _ => return Err(()),
                }
            }
            if names.is_empty() {
                return Err(());
            }
            Ok(PseudoElement::Part(names))
        }
        "highlight" => match v {
            [CV::Token(Token::Ident(s))] => Ok(PseudoElement::Highlight(s.clone())),
            _ => Err(()),
        },
        _ => Err(()),
    }
}

fn pseudo_class(n: &str) -> Option<PseudoClass> {
    use ElementState as S;
    use PseudoClass as P;
    let nth = |kind, a, b| P::Nth { kind, a, b, of: None };
    Some(match n {
        "root" => P::Root,
        "empty" => P::Empty,
        "scope" => P::Scope,
        "first-child" => nth(NthKind::Child, 0, 1),
        "last-child" => nth(NthKind::LastChild, 0, 1),
        "only-child" => P::OnlyChild,
        "first-of-type" => nth(NthKind::OfType, 0, 1),
        "last-of-type" => nth(NthKind::LastOfType, 0, 1),
        "only-of-type" => P::OnlyOfType,
        "any-link" => P::AnyLink,
        "link" => P::State(S::Link),
        "visited" => P::State(S::Visited),
        "hover" => P::State(S::Hover),
        "active" => P::State(S::Active),
        "focus" => P::State(S::Focus),
        "focus-visible" => P::State(S::FocusVisible),
        "focus-within" => P::State(S::FocusWithin),
        "target" => P::State(S::Target),
        "checked" => P::State(S::Checked),
        "indeterminate" => P::State(S::Indeterminate),
        "default" => P::State(S::Default),
        "enabled" => P::State(S::Enabled),
        "disabled" => P::State(S::Disabled),
        "required" => P::State(S::Required),
        "optional" => P::State(S::Optional),
        "read-only" => P::State(S::ReadOnly),
        "read-write" => P::State(S::ReadWrite),
        "placeholder-shown" => P::State(S::PlaceholderShown),
        "valid" => P::State(S::Valid),
        "invalid" => P::State(S::Invalid),
        "in-range" => P::State(S::InRange),
        "out-of-range" => P::State(S::OutOfRange),
        "defined" => P::State(S::Defined),
        "open" => P::State(S::Open),
        "autofill" => P::State(S::Autofill),
        _ => return None,
    })
}

fn functional(n: &str, args: &[CV], cx: &ParseContext) -> R<PseudoClass> {
    let inner = ParseContext { in_logical: true, ..*cx };
    Ok(match n {
        "not" => PseudoClass::Not(parse_selector_list(args, &inner)?),
        "is" => PseudoClass::Is(parse_forgiving(args, &inner)),
        "where" => PseudoClass::Where(parse_forgiving(args, &inner)),
        "has" => {
            if cx.in_has {
                return Err(());
            }
            let hx = ParseContext { in_has: true, ..inner };
            PseudoClass::Has(parse_relative_list(args, &hx)?)
        }
        "nth-child" | "nth-last-child" => {
            let kind = if n == "nth-child" { NthKind::Child } else { NthKind::LastChild };
            // `An+B [of S]?`
            let mut split = None;
            for (k, c) in args.iter().enumerate() {
                if matches!(c.ident(), Some(s) if s.eq_ignore_ascii_case("of")) {
                    split = Some(k);
                    break;
                }
            }
            match split {
                Some(k) => {
                    if k == 0 || !args[k - 1].is_whitespace() {
                        return Err(());
                    }
                    let (a, b) = parse_anb(&args[..k]).ok_or(())?;
                    let of = parse_selector_list(&args[k + 1..], &inner)?;
                    PseudoClass::Nth { kind, a, b, of: Some(of) }
                }
                None => {
                    let (a, b) = parse_anb(args).ok_or(())?;
                    PseudoClass::Nth { kind, a, b, of: None }
                }
            }
        }
        "nth-of-type" | "nth-last-of-type" => {
            let kind = if n == "nth-of-type" { NthKind::OfType } else { NthKind::LastOfType };
            let (a, b) = parse_anb(args).ok_or(())?;
            PseudoClass::Nth { kind, a, b, of: None }
        }
        "lang" => {
            let mut ranges = Vec::new();
            for part in split_commas(args) {
                match trim_ws(part) {
                    [CV::Token(Token::Ident(s))] | [CV::Token(Token::String(s))] => ranges.push(s.clone()),
                    _ => return Err(()),
                }
            }
            PseudoClass::Lang(ranges)
        }
        "dir" => match trim_ws(args) {
            [CV::Token(Token::Ident(s))] if s.eq_ignore_ascii_case("ltr") => PseudoClass::Dir(false),
            [CV::Token(Token::Ident(s))] if s.eq_ignore_ascii_case("rtl") => PseudoClass::Dir(true),
            _ => return Err(()),
        },
        _ => return Err(()),
    })
}

/// `[ ns|name op value flag ]`
fn parse_attr(inner: &[CV], cx: &ParseContext) -> R<Simple> {
    let v = trim_ws(inner);
    let mut i;
    let ident = |c: Option<&CV>| match c {
        Some(CV::Token(Token::Ident(s))) => Some(s.clone()),
        _ => None,
    };
    let bar = |c: Option<&CV>| c.map(|c| c.is_delim('|')).unwrap_or(false);
    let (ns, name) = if let Some(a) = ident(v.first()) {
        if bar(v.get(1)) {
            let n = ident(v.get(2)).ok_or(())?;
            i = 3;
            (resolve_prefix(cx, &a)?, n)
        } else {
            i = 1;
            (Ns::Null, a)
        }
    } else if v.first().map(|c| c.is_delim('*')).unwrap_or(false) && bar(v.get(1)) {
        let n = ident(v.get(2)).ok_or(())?;
        i = 3;
        (Ns::Any, n)
    } else if bar(v.first()) {
        let n = ident(v.get(1)).ok_or(())?;
        i = 2;
        (Ns::Null, n)
    } else {
        return Err(());
    };
    let lower = ascii_lower(&name);
    while i < v.len() && v[i].is_whitespace() {
        i += 1;
    }
    if i == v.len() {
        return Ok(Simple::Attr { ns, name, lower, op: AttrOp::Exists, value: String::new(), case: CaseFlag::Default });
    }
    let op = match &v[i] {
        CV::Token(Token::Delim('=')) => {
            i += 1;
            AttrOp::Equals
        }
        CV::Token(Token::IncludeMatch) => {
            i += 1;
            AttrOp::Includes
        }
        CV::Token(Token::DashMatch) => {
            i += 1;
            AttrOp::DashMatch
        }
        CV::Token(Token::PrefixMatch) => {
            i += 1;
            AttrOp::Prefix
        }
        CV::Token(Token::SuffixMatch) => {
            i += 1;
            AttrOp::Suffix
        }
        CV::Token(Token::SubstringMatch) => {
            i += 1;
            AttrOp::Substring
        }
        // The newer tokenizer form: a delim followed immediately by `=`.
        CV::Token(Token::Delim(c @ ('~' | '|' | '^' | '$' | '*'))) if v.get(i + 1).map(|d| d.is_delim('=')).unwrap_or(false) => {
            i += 2;
            match c {
                '~' => AttrOp::Includes,
                '|' => AttrOp::DashMatch,
                '^' => AttrOp::Prefix,
                '$' => AttrOp::Suffix,
                _ => AttrOp::Substring,
            }
        }
        _ => return Err(()),
    };
    while i < v.len() && v[i].is_whitespace() {
        i += 1;
    }
    let value = match v.get(i) {
        Some(CV::Token(Token::Ident(s))) | Some(CV::Token(Token::String(s))) => s.clone(),
        _ => return Err(()),
    };
    i += 1;
    while i < v.len() && v[i].is_whitespace() {
        i += 1;
    }
    let mut case = CaseFlag::Default;
    if let Some(CV::Token(Token::Ident(f))) = v.get(i) {
        case = if f.eq_ignore_ascii_case("i") {
            CaseFlag::Insensitive
        } else if f.eq_ignore_ascii_case("s") {
            CaseFlag::Sensitive
        } else {
            return Err(());
        };
        i += 1;
        while i < v.len() && v[i].is_whitespace() {
            i += 1;
        }
    }
    if i != v.len() {
        return Err(());
    }
    Ok(Simple::Attr { ns, name, lower, op, value, case })
}

// ------------------------------------------------------------------------------------------------
// §17 specificity
// ------------------------------------------------------------------------------------------------

fn simple_specificity(s: &Simple) -> Specificity {
    let a = Specificity { a: 1, b: 0, c: 0 };
    let b = Specificity { a: 0, b: 1, c: 0 };
    let c = Specificity { a: 0, b: 0, c: 1 };
    match s {
        Simple::Id(_) => a,
        Simple::Class(_) | Simple::Attr { .. } => b,
        Simple::Type { .. } => c,
        Simple::Universal { .. } => Specificity::ZERO,
        Simple::Pseudo(p) => match p {
            PseudoClass::Is(l) | PseudoClass::Not(l) => l.max_specificity(),
            PseudoClass::Has(rel) => rel.iter().map(|r| r.selector.specificity).max().unwrap_or(Specificity::ZERO),
            PseudoClass::Where(_) => Specificity::ZERO,
            PseudoClass::Nth { of: Some(l), .. } => b.add(l.max_specificity()),
            _ => b,
        },
    }
}

/// The specificity of a complex selector (§17), pseudo-element included.
pub fn specificity_of(sel: &Selector) -> Specificity {
    let mut s = Specificity::ZERO;
    for comp in &sel.compounds {
        for simple in comp {
            s = s.add(simple_specificity(simple));
        }
    }
    if sel.pseudo_element.is_some() {
        s.c += 1;
    }
    s
}
