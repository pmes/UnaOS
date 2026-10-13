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
//! Aether's event loop, not of either core (`:hover` reads the engine's hover chain, [`El::set_hover_chain`]). A newtype in the one crate that links both keeps both
//! cores unaware of each other.
//!
//! `El` borrows the arena (`&Document`) instead of holding the `Rc<RefCell<…>>`, so a whole cascade
//! or query walks the tree under ONE shared borrow and the matcher allocates nothing per hop.
//!
//! AETHERSHADOW (SR82): an `El` may carry the shadow host of the tree it is matched in. Then a
//! top-level element of the shadow tree has the host as its parent, the host's own ancestors and
//! siblings are out of reach, and the host alone reports the reserved attribute [`HOST_MARK`] that
//! `:host` / `:host(<compound>)` are rewritten to before css_core parses (css_core has no `:host`;
//! `[attr]` has the pseudo-class's specificity). Outside a shadow tree nothing reports the mark, so
//! `:host` matches nothing, as the spec says. [`Selectors::matches_slotted`] matches `::slotted()`.

use super::NodeRef;
use css_core::matching::{html_state, matches_list, Element, MatchContext};
use css_core::selectors::{parse_selector_str, ElementState, Namespaces, SelectorList};
use html_core::{Document, NodeData, NodeId};

/// An element of an html_core arena, as the css_core matcher sees it.
#[derive(Clone, Copy)]
pub struct El<'a> {
    pub doc: &'a Document,
    pub id: NodeId,
    /// The shadow host whose shadow tree this match runs in (None: a light-tree match).
    pub host: Option<NodeId>,
}

/// The attribute only the in-scope shadow host reports (`:host` is rewritten to it).
pub const HOST_MARK: &str = "__aether_host__";

impl<'a> El<'a> {
    pub fn new(doc: &'a Document, id: NodeId) -> Self {
        El { doc, id, host: None }
    }
    /// An element matched inside the shadow tree of `host` (None: as [`El::new`]).
    pub fn in_tree(doc: &'a Document, id: NodeId, host: Option<NodeId>) -> Self {
        El { doc, id, host }
    }
    fn hop(&self, id: Option<NodeId>) -> Option<Self> {
        id.map(|id| El { doc: self.doc, id, host: self.host })
    }
    fn is_scoped_host(&self) -> bool {
        self.host == Some(self.id)
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
        if self.is_scoped_host() && f("", HOST_MARK, "") {
            return true;
        }
        match self.element() {
            Some(e) => e.attrs.iter().any(|a| f(a.ns.url(), &a.local, &a.value)),
            None => false,
        }
    }
    fn parent_element(&self) -> Option<Self> {
        if let Some(h) = self.host {
            if self.id == h {
                return None;
            }
            if let Some(p) = self.doc.parent(self.id) {
                if super::shadow::host_of(self.doc, p) == Some(h) {
                    return self.hop(Some(h));
                }
            }
        }
        self.hop(self.doc.parent_element(self.id))
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        if self.is_scoped_host() {
            return None;
        }
        self.hop(self.doc.prev_sibling_element(self.id))
    }
    fn next_sibling_element(&self) -> Option<Self> {
        if self.is_scoped_host() {
            return None;
        }
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
        if name == HOST_MARK {
            return self.is_scoped_host().then(String::new);
        }
        self.element()?.attrs.iter().find(|a| a.ns.url().is_empty() && a.local == name).map(|a| a.value.clone())
    }
    fn has_attr(&self, name: &str) -> bool {
        if name == HOST_MARK {
            return self.is_scoped_host();
        }
        self.element().is_some_and(|e| e.attrs.iter().any(|a| a.ns.url().is_empty() && a.local == name))
    }
    fn id_is(&self, id: &str) -> bool {
        self.element().and_then(|e| e.attr("id")).is_some_and(|v| v == id)
    }
    fn has_class(&self, class: &str) -> bool {
        self.element().and_then(|e| e.attr("class")).is_some_and(|v| v.split(is_html_ws).any(|c| c == class))
    }
    // `state` is css_core's `html_state` (the attribute-derived states: `:link`, `:checked`,
    // `:disabled`/`:enabled`, `:required`, `:read-only`, `:placeholder-shown`, …) plus the user-action
    // states the engine drives: `:hover` (AETHERHOVER, SR67 — the element under the pointer and its
    // ancestors, as [`El::set_hover_chain`] last set it) and the focus states read from the engine's
    // focus (AETHERGEN, SR71). A static render has no pointer and no focus, so neither matches.
    fn state(&self, s: ElementState) -> bool {
        match s {
            ElementState::Hover => is_hovered(self.doc, self.id),
            ElementState::Focus | ElementState::FocusVisible | ElementState::FocusWithin => {
                let Some(f) = focused_in(self.doc) else { return false };
                match s {
                    ElementState::Focus => f == self.id,
                    ElementState::FocusVisible => f == self.id && focus_visible(self),
                    _ => self.doc.is_inclusive_ancestor(self.id, f),
                }
            }
            // AETHERSHADOW: `:defined` is the element's custom element state (HTML §4.13.1).
            ElementState::Defined => super::custom::is_defined(self.doc, self.id),
            _ => html_state(self, s),
        }
    }
}

thread_local! {
    /// The hover chain (AETHERHOVER, SR67): the arena it belongs to (its address — a chain never matches
    /// another document's elements) and the hit element's id followed by its ancestor elements' ids.
    static HOVER_CHAIN: std::cell::RefCell<(usize, Vec<NodeId>)> = const { std::cell::RefCell::new((0, Vec::new())) };
}

// The chain's setters are associated with `El` (re-exported by `dom`) so the engine reaches them
// through the one name the module already publishes.
impl El<'_> {
    /// Sets the elements `:hover` matches: `ids` (the hit element and its ancestors) of arena `doc`. An
    /// empty `ids` clears it. The caller re-styles (the cascade reads this through `Element::state`).
    pub fn set_hover_chain(doc: &Document, ids: &[NodeId]) {
        let key = if ids.is_empty() { 0 } else { doc as *const Document as usize };
        HOVER_CHAIN.with(|h| *h.borrow_mut() = (key, ids.to_vec()));
    }

    /// Clears the hover chain (a new document, a pointer that left the viewport).
    pub fn clear_hover_chain() {
        HOVER_CHAIN.with(|h| *h.borrow_mut() = (0, Vec::new()));
    }
}

fn is_hovered(doc: &Document, id: NodeId) -> bool {
    HOVER_CHAIN.with(|h| {
        let h = h.borrow();
        h.0 != 0 && h.0 == doc as *const Document as usize && h.1.contains(&id)
    })
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
        let rewritten = rewrite_host(s);
        let text = rewritten.as_deref().unwrap_or(s);
        parse_selector_str(text, &Namespaces::default()).map(Selectors).map_err(|_| ())
    }

    /// Does any selector of the list match element `id`? `scope` is `:scope` (the queried element).
    /// An element in a shadow tree is matched in its host's scope (`:host`, the host as the top-level
    /// parent).
    pub fn matches_in(&self, doc: &Document, id: NodeId, scope: Option<NodeId>) -> bool {
        if doc.element(id).is_none() {
            return false;
        }
        let host = if super::shadow::any(doc) { super::shadow::containing_host(doc, id) } else { None };
        let scope_el = scope.filter(|s| doc.element(*s).is_some()).map(|s| El::in_tree(doc, s, host));
        let cx = MatchContext { scope: scope_el.as_ref() };
        matches_list(&self.0, &El::in_tree(doc, id, host), &cx)
    }

    /// Does a `::slotted(<compound>)` selector of the list match slottable `id`? The compound must
    /// match `id` and the rest of the selector its assigned slot (in the slot's shadow tree), as CSS
    /// Scoping §3.2.2 says. For the cascade, which owns where shadow-tree rules apply.
    pub fn matches_slotted(&self, doc: &Document, id: NodeId) -> bool {
        use css_core::matching::{matches_for_pseudo, matches_selector};
        use css_core::selectors::{PseudoElement, Selector, Specificity};
        let Some(slot) = super::shadow::find_slot(doc, id, false) else { return false };
        let host = super::shadow::containing_host(doc, slot);
        let slot_el = El::in_tree(doc, slot, host);
        let me = El::new(doc, id);
        self.0.0.iter().any(|sel| {
            let Some(pe) = &sel.pseudo_element else { return false };
            let PseudoElement::Slotted(compound) = pe else { return false };
            let inner = Selector {
                compounds: vec![compound.clone()],
                combinators: Vec::new(),
                pseudo_element: None,
                specificity: Specificity::ZERO,
            };
            matches_selector(&inner, &me, &MatchContext { scope: None })
                && matches_for_pseudo(sel, &slot_el, Some(pe), &MatchContext { scope: None })
        })
    }

    pub fn matches_node(&self, node: &NodeRef) -> bool {
        self.matches_in(&node.doc.borrow(), node.id, None)
    }
}

/// Rewrites `:host` and `:host(<compound>)` to the reserved attribute the in-scope host reports
/// (`[__aether_host__]`, `[__aether_host__]:is(<compound>)`). Token-aware: strings and escapes are
/// skipped; `::` pseudo-elements and `:host-context()` are left alone. None when there is no `:host`.
fn rewrite_host(s: &str) -> Option<String> {
    if !s.to_ascii_lowercase().contains(":host") {
        return None;
    }
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0;
    let mut quote: Option<char> = None;
    let mut changed = false;
    let ident = |c: char| c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii();
    while i < b.len() {
        let c = b[i];
        if c == '\\' {
            out.push(c);
            if i + 1 < b.len() {
                out.push(b[i + 1]);
            }
            i += 2;
            continue;
        }
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            out.push(c);
            i += 1;
            continue;
        }
        let word: String = b.get(i + 1..i + 5).map(|w| w.iter().collect::<String>().to_ascii_lowercase()).unwrap_or_default();
        let after = b.get(i + 5).copied();
        if c == ':' && (i == 0 || b[i - 1] != ':') && word == "host" && !after.is_some_and(|a| ident(a) && a != '(') {
            changed = true;
            if after == Some('(') {
                // the balanced argument
                let mut depth = 0i32;
                let mut j = i + 5;
                let mut q2: Option<char> = None;
                let mut end = None;
                while j < b.len() {
                    let ch = b[j];
                    if ch == '\\' {
                        j += 2;
                        continue;
                    }
                    if let Some(q) = q2 {
                        if ch == q {
                            q2 = None;
                        }
                    } else if ch == '"' || ch == '\'' {
                        q2 = Some(ch);
                    } else if ch == '(' {
                        depth += 1;
                    } else if ch == ')' {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    j += 1;
                }
                let Some(end) = end else { return None };
                let inner: String = b[i + 6..end].iter().collect();
                out.push('[');
                out.push_str(HOST_MARK);
                out.push_str("]:is(");
                out.push_str(&inner);
                out.push(')');
                i = end + 1;
            } else {
                out.push('[');
                out.push_str(HOST_MARK);
                out.push(']');
                i += 5;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    changed.then_some(out)
}
