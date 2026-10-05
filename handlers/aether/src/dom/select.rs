//! CSS selector matching over the `html_core` arena: servo's `selectors` 0.41 (the one generation
//! left in the tree) driven through its `Element` trait. This replaces the 2020 `selectors` 0.22 the
//! old tree carried, and stays until CSSCORE (SR47) brings UnaOS's own matcher.
//!
//! Parity with what Aether's cascade was tuned against: the same ten non-tree-structural
//! pseudo-classes parse (`:any-link :link :visited :active :focus :hover :enabled :disabled :checked
//! :indeterminate`), only the link pair matches (no UI state is modelled yet), no pseudo-elements,
//! no `:is()`/`:where()`/`:has()` (the cascade's own lowering rewrites those), so the set of rules
//! that compile — and hence what paints — is unchanged by the swap.

use super::{ElementData, NodeDataRef, NodeRef};
use cssparser::{CowRcStr, ParseError, ToCss};
use html_core::{Document, Namespace, NodeData, NodeId, QuirksMode as DocQuirks};
use precomputed_hash::PrecomputedHash;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::matching::ElementSelectorFlags;
use selectors::parser::{ParseRelative, SelectorParseErrorKind};
use selectors::{OpaqueElement, SelectorImpl, SelectorList};
use std::fmt;

/// A CSS string atom (identifiers, attribute values, local names).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct CssStr(pub String);

impl<'a> From<&'a str> for CssStr {
    fn from(s: &'a str) -> Self {
        CssStr(s.to_string())
    }
}
impl AsRef<str> for CssStr {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl ToCss for CssStr {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        cssparser::serialize_identifier(&self.0, dest)
    }
}
fn fnv(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}
impl PrecomputedHash for CssStr {
    fn precomputed_hash(&self) -> u32 {
        fnv(&self.0)
    }
}

/// A namespace URL as the selector parser carries it ("" = no namespace).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct CssNs(pub String);

impl PrecomputedHash for CssNs {
    fn precomputed_hash(&self) -> u32 {
        fnv(&self.0)
    }
}

#[derive(Debug, Clone)]
pub struct AetherSelectors;

impl SelectorImpl for AetherSelectors {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssStr;
    type Identifier = CssStr;
    type LocalName = CssStr;
    type NamespaceUrl = CssNs;
    type NamespacePrefix = CssStr;
    type BorrowedNamespaceUrl = CssNs;
    type BorrowedLocalName = CssStr;
    type NonTSPseudoClass = PseudoClass;
    type PseudoElement = PseudoElement;
}

struct AetherParser;

impl<'i> selectors::Parser<'i> for AetherParser {
    type Impl = AetherSelectors;
    type Error = SelectorParseErrorKind;

    fn parse_non_ts_pseudo_class(
        &self,
        name: CowRcStr<'i>,
    ) -> Result<PseudoClass, ParseError<SelectorParseErrorKind>> {
        match_ignore_ascii_case_pc(&name)
            .ok_or_else(|| ParseError::custom(SelectorParseErrorKind::UnsupportedPseudoClassOrElement))
    }
}

fn match_ignore_ascii_case_pc(name: &str) -> Option<PseudoClass> {
    use PseudoClass::*;
    let table = [
        ("any-link", AnyLink),
        ("link", Link),
        ("visited", Visited),
        ("active", Active),
        ("focus", Focus),
        ("hover", Hover),
        ("enabled", Enabled),
        ("disabled", Disabled),
        ("checked", Checked),
        ("indeterminate", Indeterminate),
    ];
    table.into_iter().find(|(n, _)| name.eq_ignore_ascii_case(n)).map(|(_, p)| p)
}

#[derive(PartialEq, Eq, Clone, Debug, Hash)]
pub enum PseudoClass {
    AnyLink,
    Link,
    Visited,
    Active,
    Focus,
    Hover,
    Enabled,
    Disabled,
    Checked,
    Indeterminate,
}

impl selectors::parser::NonTSPseudoClass for PseudoClass {
    fn is_active_or_hover(&self) -> bool {
        matches!(self, PseudoClass::Active | PseudoClass::Hover)
    }
    fn is_user_action_state(&self) -> bool {
        matches!(self, PseudoClass::Active | PseudoClass::Hover | PseudoClass::Focus)
    }
}

impl ToCss for PseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(match self {
            PseudoClass::AnyLink => ":any-link",
            PseudoClass::Link => ":link",
            PseudoClass::Visited => ":visited",
            PseudoClass::Active => ":active",
            PseudoClass::Focus => ":focus",
            PseudoClass::Hover => ":hover",
            PseudoClass::Enabled => ":enabled",
            PseudoClass::Disabled => ":disabled",
            PseudoClass::Checked => ":checked",
            PseudoClass::Indeterminate => ":indeterminate",
        })
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Hash)]
pub enum PseudoElement {}

impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, _dest: &mut W) -> fmt::Result {
        match *self {}
    }
}

impl selectors::parser::PseudoElement for PseudoElement {}

/// The element handle the matcher walks: a node, no snapshot (matching allocates nothing).
#[derive(Clone)]
pub struct El(NodeRef);

impl fmt::Debug for El {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl El {
    fn with<R>(&self, f: impl FnOnce(&Document, NodeId) -> R) -> R {
        f(&self.0.doc.borrow(), self.0.id)
    }
    fn hop(&self, f: impl FnOnce(&Document, NodeId) -> Option<NodeId>) -> Option<El> {
        let id = self.with(f)?;
        Some(El(NodeRef { doc: self.0.doc.clone(), id }))
    }
    fn elem<R>(&self, f: impl FnOnce(&html_core::Element) -> R) -> Option<R> {
        self.with(|d, id| d.element(id).map(f))
    }
}

impl selectors::Element for El {
    type Impl = AetherSelectors;

    fn opaque(&self) -> OpaqueElement {
        self.with(|d, id| OpaqueElement::new(&d.nodes[id.0]))
    }
    fn parent_element(&self) -> Option<Self> {
        self.hop(|d, id| d.parent_element(id))
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }
    fn is_pseudo_element(&self) -> bool {
        false
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.hop(|d, id| d.prev_sibling_element(id))
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.hop(|d, id| d.next_sibling_element(id))
    }
    fn first_element_child(&self) -> Option<Self> {
        self.hop(|d, id| d.element_children(id).next())
    }
    fn is_html_element_in_html_document(&self) -> bool {
        self.elem(|e| e.ns == Namespace::Html).unwrap_or(false)
    }
    fn has_local_name(&self, local_name: &CssStr) -> bool {
        self.elem(|e| e.local == local_name.0).unwrap_or(false)
    }
    fn has_namespace(&self, ns: &CssNs) -> bool {
        self.elem(|e| e.ns.url() == ns.0).unwrap_or(false)
    }
    fn is_same_type(&self, other: &Self) -> bool {
        let a = self.elem(|e| (e.ns, e.local.clone()));
        let b = other.elem(|e| (e.ns, e.local.clone()));
        a.is_some() && a == b
    }
    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&CssNs>,
        local_name: &CssStr,
        operation: &AttrSelectorOperation<&CssStr>,
    ) -> bool {
        self.elem(|e| {
            e.attrs.iter().any(|a| {
                a.local == local_name.0
                    && match ns {
                        NamespaceConstraint::Any => true,
                        NamespaceConstraint::Specific(u) => a.ns.url() == u.0,
                    }
                    && operation.eval_str(&a.value)
            })
        })
        .unwrap_or(false)
    }
    fn match_non_ts_pseudo_class(&self, pc: &PseudoClass, _context: &mut MatchingContext<AetherSelectors>) -> bool {
        match pc {
            PseudoClass::AnyLink | PseudoClass::Link => self.is_link(),
            _ => false,
        }
    }
    fn match_pseudo_element(&self, pe: &PseudoElement, _context: &mut MatchingContext<AetherSelectors>) -> bool {
        match *pe {}
    }
    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}
    fn is_link(&self) -> bool {
        self.elem(|e| e.ns == Namespace::Html && matches!(e.local.as_str(), "a" | "area" | "link") && e.attr("href").is_some())
            .unwrap_or(false)
    }
    fn is_html_slot_element(&self) -> bool {
        false
    }
    fn has_id(&self, id: &CssStr, case_sensitivity: CaseSensitivity) -> bool {
        self.elem(|e| e.attr("id").is_some_and(|v| case_sensitivity.eq(id.0.as_bytes(), v.as_bytes())))
            .unwrap_or(false)
    }
    fn has_class(&self, name: &CssStr, case_sensitivity: CaseSensitivity) -> bool {
        !name.0.is_empty()
            && self
                .elem(|e| {
                    e.attr("class").is_some_and(|v| {
                        v.split([' ', '\t', '\n', '\r', '\x0C'])
                            .any(|c| case_sensitivity.eq(c.as_bytes(), name.0.as_bytes()))
                    })
                })
                .unwrap_or(false)
    }
    fn has_custom_state(&self, _name: &CssStr) -> bool {
        false
    }
    fn imported_part(&self, _name: &CssStr) -> Option<CssStr> {
        None
    }
    fn is_part(&self, _name: &CssStr) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        self.with(|d, id| {
            d.children(id).all(|c| match d.data(c) {
                NodeData::Element(_) => false,
                NodeData::Text(t) => t.is_empty(),
                _ => true,
            })
        })
    }
    fn is_root(&self) -> bool {
        self.with(|d, id| d.parent(id).is_some_and(|p| matches!(d.data(p), NodeData::Document)))
    }
    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}

/// A pre-compiled selector list.
pub struct Selectors(pub Vec<Selector>);

/// One compiled selector.
pub struct Selector(selectors::parser::Selector<AetherSelectors>);

/// A selector's specificity (ordered; ties go to source order).
#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub struct Specificity(u32);

impl Selectors {
    /// Compiles a selector list; `Err` on a syntax error or anything unsupported.
    pub fn compile(s: &str) -> Result<Selectors, ()> {
        let mut parser = cssparser::Parser::new(s);
        match SelectorList::parse(&AetherParser, &mut parser, ParseRelative::No) {
            Ok(list) => Ok(Selectors(list.slice().iter().cloned().map(Selector).collect())),
            Err(_) => Err(()),
        }
    }

    pub fn matches(&self, element: &NodeDataRef<ElementData>) -> bool {
        self.matches_node(element.as_node())
    }

    pub fn matches_node(&self, node: &NodeRef) -> bool {
        self.0.iter().any(|s| s.matches_node(node))
    }
}

fn quirks_of(node: &NodeRef) -> QuirksMode {
    match node.doc.borrow().quirks_mode {
        DocQuirks::Quirks => QuirksMode::Quirks,
        DocQuirks::LimitedQuirks => QuirksMode::LimitedQuirks,
        DocQuirks::NoQuirks => QuirksMode::NoQuirks,
    }
}

impl Selector {
    pub fn matches(&self, element: &NodeDataRef<ElementData>) -> bool {
        self.matches_node(element.as_node())
    }

    pub fn matches_node(&self, node: &NodeRef) -> bool {
        if !node.is_element() {
            return false;
        }
        let mut caches = SelectorCaches::default();
        let mut context = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            quirks_of(node),
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        selectors::matching::matches_selector(&self.0, 0, None, &El(node.clone()), &mut context)
    }

    pub fn specificity(&self) -> Specificity {
        Specificity(self.0.specificity())
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.to_css(f)
    }
}

impl fmt::Debug for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
