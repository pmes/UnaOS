//! CSS selector matching over the `html_core` arena through UnaOS's own `css_core` (CSSCORE, SR47):
//! Selectors Level 4 in full (`:is/:where/:not/:has`, `:nth-*(An+B of S)`, attribute operators and
//! flags, namespaces, HTML's attribute-derived states via `css_core::matching::html_state`).
//!
//! AETHERSTYLE (SR54) M1: [`El`] is html_core's arena implementing [`css_core::matching::Element`].
//! It lives HERE, in Aether's `dom` module, and not inside html_core, on purpose: the two cores are
//! independent `no_std` libraries (the parser has consumers that never style anything, the matcher
//! has its own test DOM), the impl needs nothing but html_core's public traversal hooks
//! (`parent_element`, `prev/next_sibling_element`, `element_children`, the attribute list), and the
//! states a matcher asks about beyond the attributes (`:hover`, `:focus`, `:target`) are a property of
//! Aether's event loop, not of either core. A newtype in the one crate that links both keeps both
//! cores unaware of each other.
//!
//! `El` borrows the arena (`&Document`) instead of holding the `Rc<RefCell<…>>`, so a whole cascade
//! or query walks the tree under ONE shared borrow and the matcher allocates nothing per hop.

use super::NodeRef;
use css_core::matching::{matches_list, Element, MatchContext};
use css_core::selectors::ElementState;
use css_core::selectors::{parse_selector_str, Namespaces, SelectorList};
use html_core::{Document, NodeData, NodeId};

/// An element of an html_core arena, as the css_core matcher sees it.
#[derive(Clone, Copy)]
pub struct El<'a> {
    pub doc: &'a Document,
    pub id: NodeId,
}

impl<'a> El<'a> {
    pub fn new(doc: &'a Document, id: NodeId) -> Self {
        El { doc, id }
    }
    fn hop(&self, id: Option<NodeId>) -> Option<Self> {
        id.map(|id| El { doc: self.doc, id })
    }
    fn element(&self) -> Option<&'a html_core::Element> {
        self.doc.element(self.id)
    }
}

impl PartialEq for El<'_> {
    fn eq(&self, o: &Self) -> bool {
        std::ptr::eq(self.doc, o.doc) && self.id == o.id
    }
}

impl std::fmt::Debug for El<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{}#{}>", self.local_name(), self.id.0)
    }
}

fn is_html_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

impl Element for El<'_> {
    fn local_name(&self) -> &str {
        self.element().map(|e| e.local.as_str()).unwrap_or("")
    }
    fn namespace_url(&self) -> &str {
        self.element().map(|e| e.ns.url()).unwrap_or("")
    }
    fn each_attr(&self, f: &mut dyn FnMut(&str, &str, &str) -> bool) -> bool {
        match self.element() {
            Some(e) => e.attrs.iter().any(|a| f(a.ns.url(), &a.local, &a.value)),
            None => false,
        }
    }
    fn parent_element(&self) -> Option<Self> {
        self.hop(self.doc.parent_element(self.id))
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.hop(self.doc.prev_sibling_element(self.id))
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.hop(self.doc.next_sibling_element(self.id))
    }
    fn first_child_element(&self) -> Option<Self> {
        self.hop(self.doc.element_children(self.id).next())
    }
    fn is_empty(&self) -> bool {
        self.doc.children(self.id).all(|c| match self.doc.data(c) {
            NodeData::Element(_) => false,
            NodeData::Text(t) => t.is_empty(),
            _ => true,
        })
    }
    /// The document element: the parent is the Document node (a detached element or a fragment's
    /// top element has no element parent either, but is not `:root`).
    fn is_root(&self) -> bool {
        self.doc.parent(self.id).is_some_and(|p| matches!(self.doc.data(p), NodeData::Document))
    }
    // The hot simple selectors read the attribute list directly.
    fn attr(&self, name: &str) -> Option<String> {
        self.element()?.attrs.iter().find(|a| a.ns.url().is_empty() && a.local == name).map(|a| a.value.clone())
    }
    fn has_attr(&self, name: &str) -> bool {
        self.element().is_some_and(|e| e.attrs.iter().any(|a| a.ns.url().is_empty() && a.local == name))
    }
    fn id_is(&self, id: &str) -> bool {
        self.element().and_then(|e| e.attr("id")).is_some_and(|v| v == id)
    }
    fn has_class(&self, class: &str) -> bool {
        self.element().and_then(|e| e.attr("class")).is_some_and(|v| v.split(is_html_ws).any(|c| c == class))
    }
    // `state` is css_core's `html_state` (the attribute-derived states: `:link`, `:checked`,
    // `:disabled`/`:enabled`, `:required`, `:read-only`, `:placeholder-shown`, …) plus the focus
    // states, read from the engine's focus (AETHERGEN, SR71). A static render has no pointer or
    // fragment target, so `:hover`/`:target` never match, and no focus until something takes it.
    fn state(&self, s: ElementState) -> bool {
        match s {
            ElementState::Focus | ElementState::FocusVisible | ElementState::FocusWithin => {
                let Some(f) = focused_in(self.doc) else { return false };
                match s {
                    ElementState::Focus => f == self.id,
                    ElementState::FocusVisible => f == self.id && focus_visible(self),
                    _ => self.doc.is_inclusive_ancestor(self.id, f),
                }
            }
            _ => css_core::matching::html_state(self, s),
        }
    }
}

/// The engine's focused element (`document.activeElement` when it is an element — the page state
/// `focus()`/`blur()` maintain) when `doc` is that page's document.
fn focused_in(doc: &Document) -> Option<NodeId> {
    crate::js::PAGE.with(|p| {
        let p = p.try_borrow().ok()?;
        let page_doc = p.doc.as_ref()?;
        if !std::ptr::eq(page_doc.doc.as_ptr() as *const Document, doc) {
            return None;
        }
        let f = NodeId(p.focused?);
        doc.element(f).is_some().then_some(f)
    })
}

/// The `:focus-visible` heuristic (Selectors 4 §9.4, as browsers implement it): an element that takes
/// keyboard text input always shows its focus; otherwise focus is visible unless it came from a
/// pointer interaction.
fn focus_visible(el: &El<'_>) -> bool {
    let text_entry = match el.local_name() {
        "textarea" => true,
        "input" => !matches!(
            el.attr("type").map(|t| t.to_ascii_lowercase()).as_deref(),
            Some("checkbox" | "radio" | "button" | "submit" | "reset" | "image" | "file" | "range" | "color" | "hidden")
        ),
        _ => el.attr("contenteditable").is_some_and(|v| !v.eq_ignore_ascii_case("false")),
    };
    text_entry || !crate::css::pointer_focus()
}

/// A compiled selector list (css_core's Selectors 4 grammar, HTML namespace defaults).
pub struct Selectors(pub SelectorList);

impl Selectors {
    /// Compiles a selector list; `Err` on a syntax error (the Selectors API's SyntaxError).
    pub fn compile(s: &str) -> Result<Selectors, ()> {
        parse_selector_str(s, &Namespaces::default()).map(Selectors).map_err(|_| ())
    }

    /// Does any selector of the list match element `id`? `scope` is `:scope` (the queried element).
    pub fn matches_in(&self, doc: &Document, id: NodeId, scope: Option<NodeId>) -> bool {
        if doc.element(id).is_none() {
            return false;
        }
        let scope_el = scope.filter(|s| doc.element(*s).is_some()).map(|s| El::new(doc, s));
        let cx = MatchContext { scope: scope_el.as_ref() };
        matches_list(&self.0, &El::new(doc, id), &cx)
    }

    pub fn matches_node(&self, node: &NodeRef) -> bool {
        self.matches_in(&node.doc.borrow(), node.id, None)
    }
}
