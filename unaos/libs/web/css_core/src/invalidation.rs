//! AETHERPAINTPERF (LEDGER SR130): invalidation sets — what a DOM change can re-match, read off the selectors.
//!
//! The rule hash ([`crate::cascade::RuleIndex`]) answers "which rules can match this element"; a restyle that
//! re-cascades only part of the document needs the converse: "which elements can a change to this attribute,
//! this class, this id, these children re-match". This is the Servo / Blink invalidation-set shape, kept small:
//! every simple selector that reads something a mutation can change (a class, an id, an attribute, an
//! attribute-derived state, `:empty`, a child index) records the REACH of the compound it sits in —
//!
//! * [`Reach::SelfOnly`] — in the subject (rightmost) compound: only the changed element can re-match;
//! * [`Reach::Subtree`] — left of descendant / child combinators only: the changed element and its descendants;
//! * [`Reach::Siblings`] — left of a `+` / `~` combinator somewhere on the way to the subject (or inside
//!   `:nth-*(… of S)`): the changed element's later siblings and their descendants, i.e. its parent's subtree.
//!
//! A selector nested in `:is()` / `:where()` / `:not()` / `:nth-*(of S)` takes the reach of its own compound
//! combined with the reach of the compound that holds it (the larger of the two). What the shape cannot bound
//! sets [`InvalidationSet::whole`]: `:has()` (an element re-matches on a change BELOW it), `:lang()` / `:dir()`
//! (inherited from any ancestor and, for `dir=auto`, from descendant text), `::slotted()` / `::part()` (another
//! tree's elements). Host-driven element states (`:hover`, `:focus`, …) are listed in
//! [`InvalidationSet::states`] with their reach; the host diffs its own state across restyles. The states
//! [`crate::matching::html_state`] derives from attributes are folded into the attribute table instead
//! ([`state_attributes`]).
//!
//! The answer is a superset by construction: every simple selector is visited, the reach only grows.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::selectors::{Combinator, ElementState, PseudoClass, PseudoElement, Selector, SelectorList, Simple};

/// How far a change to one element can move other elements' matches (ordered: a larger reach covers a smaller).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// No selector reads it.
    #[default]
    None,
    /// The element itself.
    SelfOnly,
    /// The element and its descendants.
    Subtree,
    /// The element's parent's subtree (its later siblings and their descendants).
    Siblings,
}

/// What the selectors of a rule set read, by reach.
#[derive(Clone, Debug, Default)]
pub struct InvalidationSet {
    /// Some selector reads what the reach cannot bound: restyle the whole document on any change.
    pub whole: bool,
    /// Some selector reads sibling structure (`+`, `~`, `:nth-*`, `:first-child` … `:only-of-type`): a change
    /// to an element's children re-matches every child (and their descendants).
    pub siblings: bool,
    /// The reach of `:empty` (a change to an element's children can flip it).
    pub empty: Reach,
    /// Class names (ASCII-lowercased: quirks-mode class matching is case-insensitive) by reach.
    pub classes: BTreeMap<String, Reach>,
    /// Ids (ASCII-lowercased, for the same reason) by reach.
    pub ids: BTreeMap<String, Reach>,
    /// Attribute names (ASCII-lowercased) by reach: attribute selectors and the attribute-derived states.
    pub attrs: BTreeMap<String, Reach>,
    /// Host-driven element states by reach (the host diffs the state it drives).
    pub states: Vec<(ElementState, Reach)>,
}

/// The attributes [`crate::matching::html_state`] reads for an attribute-derived state, and whether the state
/// also reads ANCESTORS' attributes (HTML "actually disabled": a disabled `fieldset` / `optgroup` above), which
/// makes a change reach the subtree whatever compound the state sits in. `None` for a host-driven state.
pub fn state_attributes(s: ElementState) -> Option<(&'static [&'static str], bool)> {
    use ElementState as S;
    Some(match s {
        S::Link => (&["href"], false),
        S::Checked | S::Default => (&["checked", "selected", "type"], false),
        S::Disabled | S::Enabled => (&["disabled"], true),
        S::Required | S::Optional => (&["required"], false),
        S::ReadWrite | S::ReadOnly => (&["readonly", "disabled", "contenteditable", "type"], true),
        S::PlaceholderShown => (&["placeholder", "value"], false),
        S::Open => (&["open"], false),
        _ => return None,
    })
}

fn bump(map: &mut BTreeMap<String, Reach>, key: &str, r: Reach) {
    let e = map.entry(key.to_ascii_lowercase()).or_default();
    if r > *e {
        *e = r;
    }
}

/// The reach of compound `k` of `sel` on its own: the subject is [`Reach::SelfOnly`]; a compound left of
/// descendant / child combinators only is an ancestor of the subject; one left of any sibling combinator is a
/// sibling (or a sibling's ancestor) of it.
fn compound_reach(sel: &Selector, k: usize) -> Reach {
    if k + 1 >= sel.compounds.len() {
        return Reach::SelfOnly;
    }
    if sel.combinators[k..].iter().any(|c| matches!(c, Combinator::NextSibling | Combinator::SubsequentSibling)) {
        Reach::Siblings
    } else {
        Reach::Subtree
    }
}

impl InvalidationSet {
    /// The invalidation set of `selectors` (every selector of every rule in force).
    pub fn of<'s>(selectors: impl IntoIterator<Item = &'s Selector>) -> Self {
        let mut set = InvalidationSet::default();
        for sel in selectors {
            set.add_selector(sel, Reach::SelfOnly);
        }
        set
    }

    /// Records `sel`, whose subject compound sits where `outer` says (the reach of the compound holding it; the
    /// top-level selector's subject is [`Reach::SelfOnly`]).
    fn add_selector(&mut self, sel: &Selector, outer: Reach) {
        match &sel.pseudo_element {
            Some(PseudoElement::Slotted(_)) | Some(PseudoElement::Part(_)) => self.whole = true,
            _ => {}
        }
        if sel.combinators.iter().any(|c| matches!(c, Combinator::NextSibling | Combinator::SubsequentSibling)) {
            self.siblings = true;
        }
        for (k, compound) in sel.compounds.iter().enumerate() {
            let r = compound_reach(sel, k).max(outer);
            for simple in compound {
                self.add_simple(simple, r);
            }
        }
    }

    fn add_list(&mut self, list: &SelectorList, r: Reach) {
        for s in &list.0 {
            self.add_selector(s, r);
        }
    }

    fn add_simple(&mut self, simple: &Simple, r: Reach) {
        match simple {
            Simple::Type { .. } | Simple::Universal { .. } => {}
            Simple::Id(id) => bump(&mut self.ids, id, r),
            Simple::Class(c) => bump(&mut self.classes, c, r),
            Simple::Attr { lower, .. } => bump(&mut self.attrs, lower, r),
            Simple::Pseudo(p) => match p {
                PseudoClass::Not(l) | PseudoClass::Is(l) | PseudoClass::Where(l) => self.add_list(l, r),
                PseudoClass::Has(_) | PseudoClass::Lang(_) | PseudoClass::Dir(_) => self.whole = true,
                PseudoClass::Nth { of, .. } => {
                    self.siblings = true;
                    if let Some(l) = of {
                        // `of S` is evaluated on the siblings: a change to one re-indexes the others.
                        self.add_list(l, Reach::Siblings);
                    }
                }
                PseudoClass::OnlyChild | PseudoClass::OnlyOfType => self.siblings = true,
                PseudoClass::Empty => {
                    if r > self.empty {
                        self.empty = r;
                    }
                }
                PseudoClass::Root | PseudoClass::Scope => {}
                PseudoClass::AnyLink => bump(&mut self.attrs, "href", r),
                PseudoClass::State(s) => match state_attributes(*s) {
                    Some((names, ancestors)) => {
                        let r = if ancestors { r.max(Reach::Subtree) } else { r };
                        for n in names {
                            bump(&mut self.attrs, n, r);
                        }
                    }
                    None => match self.states.iter_mut().find(|(x, _)| x == s) {
                        Some((_, e)) => {
                            if r > *e {
                                *e = r;
                            }
                        }
                        None => self.states.push((*s, r)),
                    },
                },
            },
        }
    }

    /// The reach of a change to attribute `name` (any case) of an element from `old` to `new` (None: absent).
    /// `class` reaches as far as the changed class tokens (and `[class…]` selectors) do, `id` as far as the old
    /// and the new id do; `style` (the element's own declarations) reaches the element at least.
    pub fn attribute(&self, name: &str, old: Option<&str>, new: Option<&str>) -> Reach {
        let lower = name.to_ascii_lowercase();
        let mut r = self.attrs.get(&lower).copied().unwrap_or_default();
        let ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r');
        match lower.as_str() {
            "class" => {
                let toks = |v: Option<&str>| -> Vec<String> {
                    v.unwrap_or("").split(ws).filter(|t| !t.is_empty()).map(|t| t.to_ascii_lowercase()).collect()
                };
                let (a, b) = (toks(old), toks(new));
                for t in a.iter().filter(|t| !b.contains(*t)).chain(b.iter().filter(|t| !a.contains(*t))) {
                    if let Some(x) = self.classes.get(t) {
                        r = r.max(*x);
                    }
                }
            }
            "id" => {
                for v in [old, new].into_iter().flatten() {
                    if let Some(x) = self.ids.get(&v.to_ascii_lowercase()) {
                        r = r.max(*x);
                    }
                }
            }
            "style" => r = r.max(Reach::SelfOnly),
            _ => {}
        }
        r
    }

    /// The reach of host-driven state `s` (None: no selector reads it).
    pub fn state(&self, s: ElementState) -> Reach {
        self.states.iter().find(|(x, _)| *x == s).map(|(_, r)| *r).unwrap_or_default()
    }

    /// A one-line summary (for the engine's profile line).
    pub fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str(if self.whole { "whole" } else { "scoped" });
        if self.siblings {
            out.push_str(" siblings");
        }
        out.push_str(" classes=");
        out.push_str(&self.classes.len().to_string());
        out.push_str(" ids=");
        out.push_str(&self.ids.len().to_string());
        out.push_str(" attrs=");
        out.push_str(&self.attrs.len().to_string());
        out
    }
}
