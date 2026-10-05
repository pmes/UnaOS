//! Selector matching (Selectors Level 4 §3–§16) over an abstract [`Element`] trait.
//!
//! Any DOM implements [`Element`] (html_core's in the follow-on; the tests use a small arena DOM). The
//! matcher is a right-to-left walk with backtracking over descendant / sibling combinators; `:has()`
//! anchors its relative selectors at the subject; `:nth-*()` count element siblings (optionally filtered
//! by `of S`).

use alloc::string::String;
use alloc::vec::Vec;

use crate::anb::anb_matches;
use crate::selectors::*;

/// What a selector needs to know about an element. Element names of HTML elements in HTML documents
/// are lowercase; attribute names are as stored (HTML lowercases them at parse time).
pub trait Element: Clone + PartialEq {
    fn local_name(&self) -> &str;
    /// The namespace URL, `""` for the null namespace.
    fn namespace_url(&self) -> &str;
    /// HTML element in an HTML document (selects ASCII-case-insensitive type/attribute-name matching).
    fn is_html_element_in_html_document(&self) -> bool {
        self.namespace_url() == HTML_NS
    }
    /// Visit every attribute as `(namespace, local name, value)`; stop early when `f` returns `true`.
    /// Returns whether `f` returned `true`.
    fn each_attr(&self, f: &mut dyn FnMut(&str, &str, &str) -> bool) -> bool;
    fn parent_element(&self) -> Option<Self>;
    fn prev_sibling_element(&self) -> Option<Self>;
    fn next_sibling_element(&self) -> Option<Self>;
    fn first_child_element(&self) -> Option<Self>;
    /// `:empty`: no element children and no non-empty text children.
    fn is_empty(&self) -> bool;
    /// The document element.
    fn is_root(&self) -> bool {
        self.parent_element().is_none()
    }
    /// User-action, location and input states. Default: [`html_state`] from attributes, no interaction.
    fn state(&self, s: ElementState) -> bool {
        html_state(self, s)
    }

    /// The value of a null-namespace attribute.
    fn attr(&self, name: &str) -> Option<String> {
        let mut out = None;
        self.each_attr(&mut |ns, n, v| {
            if ns.is_empty() && n == name {
                out = Some(String::from(v));
                true
            } else {
                false
            }
        });
        out
    }
    fn has_attr(&self, name: &str) -> bool {
        self.each_attr(&mut |ns, n, _| ns.is_empty() && n == name)
    }
    fn id_is(&self, id: &str) -> bool {
        self.each_attr(&mut |ns, n, v| ns.is_empty() && n == "id" && v == id)
    }
    fn has_class(&self, class: &str) -> bool {
        self.each_attr(&mut |ns, n, v| ns.is_empty() && n == "class" && v.split(is_html_ws).any(|c| c == class))
    }
    /// The language (`xml:lang`, then `lang` on HTML elements), inherited from ancestors.
    fn lang(&self) -> Option<String> {
        let mut cur = Some(self.clone());
        while let Some(e) = cur {
            let mut found = None;
            e.each_attr(&mut |ns, n, v| {
                if ns == XML_NS && n == "lang" {
                    found = Some(String::from(v));
                    true
                } else {
                    false
                }
            });
            if found.is_none() && e.namespace_url() == HTML_NS {
                found = e.attr("lang");
            }
            if found.is_some() {
                return found;
            }
            cur = e.parent_element();
        }
        None
    }
}

fn is_html_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

/// HTML's states derived from attributes alone (HTML §4.16.3 "Pseudo-classes"): `:link` (`a` / `area`
/// with `href`), `:checked`, `:disabled` / `:enabled`, `:required` / `:optional`, `:read-only` /
/// `:read-write`, `:placeholder-shown`, `:default`, `:defined`. No interaction (`:hover` etc. false).
pub fn html_state<E: Element>(e: &E, s: ElementState) -> bool {
    use ElementState as S;
    let html = e.namespace_url() == HTML_NS;
    let n = e.local_name();
    let ty = || e.attr("type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
    let form_control = html && matches!(n, "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset");
    match s {
        S::Link => html && matches!(n, "a" | "area") && e.has_attr("href"),
        S::Checked => {
            html && ((n == "input" && matches!(ty().as_str(), "checkbox" | "radio") && e.has_attr("checked"))
                || (n == "option" && e.has_attr("selected")))
        }
        S::Default => {
            html && ((n == "input" && matches!(ty().as_str(), "checkbox" | "radio") && e.has_attr("checked"))
                || (n == "option" && e.has_attr("selected")))
        }
        S::Disabled => form_control && is_disabled(e),
        S::Enabled => form_control && !is_disabled(e),
        S::Required => html && matches!(n, "input" | "select" | "textarea") && e.has_attr("required"),
        S::Optional => html && matches!(n, "input" | "select" | "textarea") && !e.has_attr("required"),
        S::ReadWrite => html && ((n == "textarea" && !e.has_attr("readonly") && !is_disabled(e))
            || (n == "input" && !e.has_attr("readonly") && !is_disabled(e)
                && !matches!(ty().as_str(), "hidden" | "range" | "color" | "checkbox" | "radio" | "file" | "submit" | "image" | "reset" | "button"))
            || e.has_attr("contenteditable")),
        S::ReadOnly => !html_state(e, S::ReadWrite),
        S::PlaceholderShown => html && matches!(n, "input" | "textarea") && e.has_attr("placeholder")
            && e.attr("value").map(|v| v.is_empty()).unwrap_or(true),
        S::Defined => true,
        S::Open => html && matches!(n, "details" | "dialog") && e.has_attr("open"),
        _ => false,
    }
}

/// HTML "actually disabled": the attribute, an `option` in a disabled `optgroup`, or a descendant of a
/// disabled `fieldset` (outside its first `legend`).
fn is_disabled<E: Element>(e: &E) -> bool {
    if e.has_attr("disabled") {
        return true;
    }
    if e.local_name() == "option" {
        if let Some(p) = e.parent_element() {
            if p.local_name() == "optgroup" && p.has_attr("disabled") {
                return true;
            }
        }
        return false;
    }
    if !matches!(e.local_name(), "button" | "input" | "select" | "textarea" | "fieldset") {
        return false;
    }
    let mut child = e.clone();
    let mut cur = e.parent_element();
    while let Some(p) = cur {
        if p.namespace_url() == HTML_NS && p.local_name() == "fieldset" && p.has_attr("disabled") {
            // the first legend child of the fieldset exempts its descendants
            let mut first_legend = None;
            let mut c = p.first_child_element();
            while let Some(x) = c {
                if x.local_name() == "legend" {
                    first_legend = Some(x);
                    break;
                }
                c = x.next_sibling_element();
            }
            if first_legend.as_ref() != Some(&child) {
                return true;
            }
        }
        child = p.clone();
        cur = p.parent_element();
    }
    false
}

/// HTML's attributes whose values match ASCII-case-insensitively in selectors (HTML §4.16.2).
const CASE_INSENSITIVE_ATTRS: &[&str] = &[
    "accept", "accept-charset", "align", "alink", "axis", "bgcolor", "charset", "checked", "clear", "codetype", "color",
    "compact", "declare", "defer", "dir", "direction", "disabled", "enctype", "face", "frame", "hreflang", "http-equiv",
    "lang", "language", "link", "media", "method", "multiple", "nohref", "noresize", "noshade", "nowrap", "readonly",
    "rel", "rev", "rules", "scope", "scrolling", "selected", "shape", "target", "text", "type", "valign", "valuetype",
    "vlink",
];

/// Matching context: the `:scope` element (the Selectors API's subject; `None` → `:root`).
pub struct MatchContext<'a, E> {
    pub scope: Option<&'a E>,
}

impl<'a, E> MatchContext<'a, E> {
    pub fn new() -> Self {
        MatchContext { scope: None }
    }
}

impl<'a, E> Default for MatchContext<'a, E> {
    fn default() -> Self {
        Self::new()
    }
}

/// Does `sel` (with no pseudo-element) match `el`?
pub fn matches_selector<E: Element>(sel: &Selector, el: &E, cx: &MatchContext<E>) -> bool {
    sel.pseudo_element.is_none() && match_from(sel, sel.compounds.len() - 1, el, cx, None)
}

/// Does `sel` match `el` as the originating element of a pseudo-element (or of none)?
pub fn matches_for_pseudo<E: Element>(sel: &Selector, el: &E, pe: Option<&PseudoElement>, cx: &MatchContext<E>) -> bool {
    sel.pseudo_element.as_ref() == pe && match_from(sel, sel.compounds.len() - 1, el, cx, None)
}

/// Does any selector of the list match `el`?
pub fn matches_list<E: Element>(list: &SelectorList, el: &E, cx: &MatchContext<E>) -> bool {
    list.0.iter().any(|s| matches_selector(s, el, cx))
}

/// `querySelectorAll`: the matching descendants of `root` in tree order (`root` is `:scope`).
pub fn query_all<E: Element>(root: &E, list: &SelectorList, include_root: bool) -> Vec<E> {
    let cx = MatchContext { scope: Some(root) };
    let mut out = Vec::new();
    if include_root && matches_list(list, root, &cx) {
        out.push(root.clone());
    }
    let mut stack = Vec::new();
    if let Some(c) = root.first_child_element() {
        stack.push(c);
    }
    // pre-order walk without recursion
    while let Some(e) = stack.pop() {
        if matches_list(list, &e, &cx) {
            out.push(e.clone());
        }
        if let Some(n) = e.next_sibling_element() {
            stack.push(n);
        }
        if let Some(c) = e.first_child_element() {
            stack.push(c);
        }
    }
    out
}

/// Right-to-left: does `compounds[..=idx]` match with `el` as the subject of `compounds[idx]`?
/// `anchor`: for a relative selector, the anchor element and the leading combinator binding it.
fn match_from<E: Element>(sel: &Selector, idx: usize, el: &E, cx: &MatchContext<E>, anchor: Option<(&E, Combinator)>) -> bool {
    if !match_compound(&sel.compounds[idx], el, cx) {
        return false;
    }
    if idx == 0 {
        return match anchor {
            None => true,
            Some((a, comb)) => related(a, el, comb),
        };
    }
    let next = |e: &E| match_from(sel, idx - 1, e, cx, anchor);
    match sel.combinators[idx - 1] {
        Combinator::Child => el.parent_element().map(|p| next(&p)).unwrap_or(false),
        Combinator::Descendant => {
            let mut p = el.parent_element();
            while let Some(x) = p {
                if next(&x) {
                    return true;
                }
                p = x.parent_element();
            }
            false
        }
        Combinator::NextSibling => el.prev_sibling_element().map(|p| next(&p)).unwrap_or(false),
        Combinator::SubsequentSibling => {
            let mut p = el.prev_sibling_element();
            while let Some(x) = p {
                if next(&x) {
                    return true;
                }
                p = x.prev_sibling_element();
            }
            false
        }
    }
}

/// Is `el` related to `anchor` by `comb` (anchor on the left)?
fn related<E: Element>(anchor: &E, el: &E, comb: Combinator) -> bool {
    match comb {
        Combinator::Child => el.parent_element().as_ref() == Some(anchor),
        Combinator::Descendant => {
            let mut p = el.parent_element();
            while let Some(x) = p {
                if &x == anchor {
                    return true;
                }
                p = x.parent_element();
            }
            false
        }
        Combinator::NextSibling => el.prev_sibling_element().as_ref() == Some(anchor),
        Combinator::SubsequentSibling => {
            let mut p = el.prev_sibling_element();
            while let Some(x) = p {
                if &x == anchor {
                    return true;
                }
                p = x.prev_sibling_element();
            }
            false
        }
    }
}

fn match_compound<E: Element>(c: &[Simple], el: &E, cx: &MatchContext<E>) -> bool {
    c.iter().all(|s| match_simple(s, el, cx))
}

fn ns_ok(ns: &Ns, url: &str) -> bool {
    match ns {
        Ns::Any => true,
        Ns::Null => url.is_empty(),
        Ns::Url(u) => u == url,
    }
}

fn match_simple<E: Element>(s: &Simple, el: &E, cx: &MatchContext<E>) -> bool {
    match s {
        Simple::Type { ns, name, lower } => {
            let want = if el.is_html_element_in_html_document() { lower } else { name };
            el.local_name() == want && ns_ok(ns, el.namespace_url())
        }
        Simple::Universal { ns } => ns_ok(ns, el.namespace_url()),
        Simple::Id(id) => el.id_is(id),
        Simple::Class(c) => el.has_class(c),
        Simple::Attr { ns, name, lower, op, value, case } => {
            let html = el.is_html_element_in_html_document();
            let want = if html { lower.as_str() } else { name.as_str() };
            el.each_attr(&mut |ans, an, av| {
                if an != want || !ns_ok(ns, ans) {
                    return false;
                }
                let ci = match case {
                    CaseFlag::Insensitive => true,
                    CaseFlag::Sensitive => false,
                    CaseFlag::Default => html && ans.is_empty() && CASE_INSENSITIVE_ATTRS.contains(&lower.as_str()),
                };
                attr_value_matches(*op, av, value, ci)
            })
        }
        Simple::Pseudo(p) => match_pseudo(p, el, cx),
    }
}

fn eq(a: &str, b: &str, ci: bool) -> bool {
    if ci { a.eq_ignore_ascii_case(b) } else { a == b }
}

fn attr_value_matches(op: AttrOp, attr: &str, val: &str, ci: bool) -> bool {
    match op {
        AttrOp::Exists => true,
        AttrOp::Equals => eq(attr, val, ci),
        AttrOp::Includes => !val.is_empty() && !val.contains(is_html_ws) && attr.split(is_html_ws).any(|w| eq(w, val, ci)),
        AttrOp::DashMatch => {
            eq(attr, val, ci)
                || (attr.len() > val.len() && attr.as_bytes()[val.len()] == b'-' && attr.is_char_boundary(val.len()) && eq(&attr[..val.len()], val, ci))
        }
        AttrOp::Prefix => !val.is_empty() && attr.len() >= val.len() && attr.is_char_boundary(val.len()) && eq(&attr[..val.len()], val, ci),
        AttrOp::Suffix => {
            let k = attr.len().wrapping_sub(val.len());
            !val.is_empty() && attr.len() >= val.len() && attr.is_char_boundary(k) && eq(&attr[k..], val, ci)
        }
        AttrOp::Substring => {
            if val.is_empty() {
                return false;
            }
            if ci {
                let a = attr.to_ascii_lowercase();
                a.contains(&val.to_ascii_lowercase())
            } else {
                attr.contains(val)
            }
        }
    }
}

fn same_type<E: Element>(a: &E, b: &E) -> bool {
    a.local_name() == b.local_name() && a.namespace_url() == b.namespace_url()
}

/// 1-based index of `el` among its siblings (from the end when `last`), counting those passing `keep`.
fn nth_index<E: Element>(el: &E, last: bool, keep: &dyn Fn(&E) -> bool) -> i32 {
    let mut n = 1;
    let mut cur = if last { el.next_sibling_element() } else { el.prev_sibling_element() };
    while let Some(x) = cur {
        if keep(&x) {
            n += 1;
        }
        cur = if last { x.next_sibling_element() } else { x.prev_sibling_element() };
    }
    n
}

fn match_pseudo<E: Element>(p: &PseudoClass, el: &E, cx: &MatchContext<E>) -> bool {
    match p {
        PseudoClass::Not(l) => !matches_list(l, el, cx),
        PseudoClass::Is(l) | PseudoClass::Where(l) => matches_list(l, el, cx),
        PseudoClass::Has(rels) => rels.iter().any(|r| has_match(r, el, cx)),
        PseudoClass::Nth { kind, a, b, of } => {
            if let Some(l) = of {
                if !matches_list(l, el, cx) {
                    return false;
                }
            }
            let idx = match kind {
                NthKind::Child | NthKind::LastChild => {
                    let last = *kind == NthKind::LastChild;
                    match of {
                        Some(l) => nth_index(el, last, &|x| matches_list(l, x, cx)),
                        None => nth_index(el, last, &|_| true),
                    }
                }
                NthKind::OfType => nth_index(el, false, &|x| same_type(x, el)),
                NthKind::LastOfType => nth_index(el, true, &|x| same_type(x, el)),
            };
            anb_matches(*a, *b, idx)
        }
        PseudoClass::OnlyChild => el.prev_sibling_element().is_none() && el.next_sibling_element().is_none(),
        PseudoClass::OnlyOfType => nth_index(el, false, &|x| same_type(x, el)) == 1 && nth_index(el, true, &|x| same_type(x, el)) == 1,
        PseudoClass::Root => el.is_root(),
        PseudoClass::Empty => el.is_empty(),
        PseudoClass::Scope => match cx.scope {
            Some(s) => s == el,
            None => el.is_root(),
        },
        PseudoClass::AnyLink => el.state(ElementState::Link) || el.state(ElementState::Visited),
        PseudoClass::Lang(ranges) => match el.lang() {
            Some(tag) => ranges.iter().any(|r| lang_matches(r, &tag)),
            None => false,
        },
        PseudoClass::Dir(rtl) => {
            let mut cur = Some(el.clone());
            while let Some(e) = cur {
                if let Some(d) = e.attr("dir") {
                    if d.eq_ignore_ascii_case("rtl") {
                        return *rtl;
                    }
                    if d.eq_ignore_ascii_case("ltr") {
                        return !*rtl;
                    }
                }
                cur = e.parent_element();
            }
            !*rtl
        }
        PseudoClass::State(s) => el.state(*s),
    }
}

/// `:has(<relative selector>)` with `anchor` as the anchor element.
fn has_match<E: Element>(r: &RelativeSelector, anchor: &E, cx: &MatchContext<E>) -> bool {
    let last = r.selector.compounds.len() - 1;
    let test = |c: &E| match_from(&r.selector, last, c, cx, Some((anchor, r.combinator)));
    // Candidates: the anchor's descendants (` `, `>`), or its following siblings and their descendants.
    let mut roots = Vec::new();
    match r.combinator {
        Combinator::Descendant | Combinator::Child => {
            if let Some(c) = anchor.first_child_element() {
                roots.push(c);
            }
        }
        Combinator::NextSibling | Combinator::SubsequentSibling => {
            if let Some(n) = anchor.next_sibling_element() {
                roots.push(n);
            }
        }
    }
    let mut stack = roots;
    while let Some(e) = stack.pop() {
        if test(&e) {
            return true;
        }
        if let Some(n) = e.next_sibling_element() {
            stack.push(n);
        }
        if let Some(c) = e.first_child_element() {
            stack.push(c);
        }
    }
    false
}

/// RFC 4647 §3.3.2 extended filtering (Selectors 4 `:lang()`), ASCII case-insensitive.
pub fn lang_matches(range: &str, tag: &str) -> bool {
    let r: Vec<String> = range.split('-').map(|s| s.to_ascii_lowercase()).collect();
    let t: Vec<String> = tag.split('-').map(|s| s.to_ascii_lowercase()).collect();
    if range.is_empty() {
        return tag.is_empty();
    }
    if tag.is_empty() {
        return false;
    }
    if r[0] != "*" && r[0] != t[0] {
        return false;
    }
    let (mut i, mut j) = (1, 1);
    while i < r.len() {
        if r[i] == "*" {
            i += 1;
            continue;
        }
        if j >= t.len() {
            return false;
        }
        if r[i] == t[j] {
            i += 1;
            j += 1;
            continue;
        }
        if t[j].len() == 1 {
            return false;
        }
        j += 1;
    }
    true
}
