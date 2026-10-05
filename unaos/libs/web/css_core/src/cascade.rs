//! CSS Cascading and Inheritance Level 5, §6: collecting the declarations that apply to an element and
//! sorting them by the cascade order — origin and importance, element-attached (style attribute),
//! cascade layers, specificity, order of appearance. Conditional rules are resolved once per
//! [`Environment`] when sheets are added to a [`RuleSet`]; `@import`s through a resolver callback.
//!
//! Rule hashing (AETHERSTYLE, SR54): [`RuleSet::build_index`] buckets every selector by the most
//! selective simple selector of its rightmost (subject) compound — id, else class, else type, else the
//! universal bucket — and [`cascade`] then tests an element only against the rules of the buckets it can
//! satisfy (its id, each of its classes, its local name, plus the universal bucket). An element that
//! lacks the key cannot match the subject compound, so the candidate walk is exact, not a heuristic.
//! A subject compound keyed only by attributes buckets by attribute name. Each entry also carries an
//! ancestor mask — the ids, classes and type names its compounds left of a descendant / child
//! combinator require of some ancestor — checked against an [`AncestorFilter`] (a 256-bit Bloom filter
//! of the element's ancestors, built by the caller as it walks down the tree) before matching: a miss
//! is a proof the selector cannot match; a hit (possibly false) runs the real matcher.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::matching::{matches_for_pseudo, Element, MatchContext};
use crate::media::Environment;
use crate::parser::Declaration;
use crate::selectors::{Combinator, Namespaces, PseudoElement, Selector, Simple, Specificity};
use crate::stylesheet::{CssRule, FontFace, Keyframes, StyleRule, Stylesheet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    UserAgent,
    User,
    Author,
}

/// One selector of one style rule, flattened with everything the cascade needs.
#[derive(Clone, Debug)]
pub struct RuleEntry<'a> {
    pub selector: &'a Selector,
    pub declarations: &'a [Declaration],
    pub origin: Origin,
    /// Cascade-layer position: a path of layer indices, the layer's own rules at `u32::MAX`.
    pub layer: Vec<u32>,
    /// Order of appearance (rule index across all sheets).
    pub order: u32,
}

/// The rules in force for one environment.
#[derive(Default)]
pub struct RuleSet<'a> {
    pub entries: Vec<RuleEntry<'a>>,
    pub font_faces: Vec<&'a FontFace>,
    pub keyframes: Vec<&'a Keyframes>,
    layers: Vec<(String, u32)>,
    next_order: u32,
    index: Option<RuleIndex>,
}

/// Rule hashing: indices into [`RuleSet::entries`], bucketed by the subject compound's key.
#[derive(Default, Debug)]
pub struct RuleIndex {
    by_id: BTreeMap<String, Vec<u32>>,
    by_class: BTreeMap<String, Vec<u32>>,
    /// ASCII-lowercased type names (HTML type selectors match case-insensitively; a non-HTML element is
    /// looked up by its lowercased name too, so the bucket is a superset and matching decides).
    by_type: BTreeMap<String, Vec<u32>>,
    /// ASCII-lowercased attribute names, for subject compounds keyed only by attributes.
    by_attr: BTreeMap<String, Vec<u32>>,
    universal: Vec<u32>,
    /// Per entry: the ancestor Bloom bits its selector requires (all zero: none).
    masks: Vec<[u64; 4]>,
}

/// A 256-bit Bloom filter over the ids, classes and (ASCII-lowercased) type names of an element's
/// ancestors. Build it top-down: a child's filter is its parent's filter `with_element(parent)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AncestorFilter {
    bits: [u64; 4],
}

const K_ID: u32 = 0x9e37_79b9;
const K_CLASS: u32 = 0x85eb_ca6b;
const K_TYPE: u32 = 0xc2b2_ae35;

fn fnv(seed: u32, s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5 ^ seed;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn set_bits(bits: &mut [u64; 4], h: u32) {
    // two probes from one hash
    for p in [h & 0xff, (h >> 8) & 0xff] {
        bits[(p >> 6) as usize] |= 1u64 << (p & 63);
    }
}

fn is_html_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

impl AncestorFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// This filter plus `el`'s own id, classes and type name: the filter for `el`'s children.
    pub fn with_element<E: Element>(&self, el: &E) -> Self {
        let mut f = *self;
        let name = el.local_name();
        if name.bytes().any(|b| b.is_ascii_uppercase()) {
            set_bits(&mut f.bits, fnv(K_TYPE, &name.to_ascii_lowercase()));
        } else {
            set_bits(&mut f.bits, fnv(K_TYPE, name));
        }
        el.each_attr(&mut |ns, n, v| {
            if ns.is_empty() {
                if n == "id" {
                    set_bits(&mut f.bits, fnv(K_ID, v));
                } else if n == "class" {
                    for c in v.split(is_html_ws).filter(|c| !c.is_empty()) {
                        set_bits(&mut f.bits, fnv(K_CLASS, c));
                    }
                }
            }
            false
        });
        f
    }

    fn may_contain(&self, mask: &[u64; 4]) -> bool {
        (0..4).all(|i| self.bits[i] & mask[i] == mask[i])
    }
}

/// The Bloom bits of what `sel` requires of the subject's ancestors: every id, class and type name of
/// a compound joined to the compound on its right by a descendant or child combinator. Such a compound
/// is an ancestor of its right neighbour, which is the subject, a sibling of it, an ancestor of it or a
/// sibling of an ancestor — so in every case it is an ancestor of the subject. (A compound left of a
/// sibling combinator is NOT: `.b` in `.b + .c > .e` is a sibling of an ancestor.)
fn ancestor_mask(sel: &Selector) -> [u64; 4] {
    let mut bits = [0u64; 4];
    let n = sel.compounds.len();
    for k in 0..n.saturating_sub(1) {
        if !matches!(sel.combinators[k], Combinator::Descendant | Combinator::Child) {
            continue;
        }
        for s in &sel.compounds[k] {
            match s {
                Simple::Id(id) => set_bits(&mut bits, fnv(K_ID, id)),
                Simple::Class(c) => set_bits(&mut bits, fnv(K_CLASS, c)),
                Simple::Type { lower, .. } => set_bits(&mut bits, fnv(K_TYPE, lower)),
                _ => {}
            }
        }
    }
    bits
}

/// The bucket key of a selector: the most selective simple selector of its rightmost compound.
enum Key<'s> {
    Id(&'s str),
    Class(&'s str),
    Type(&'s str),
    Attr(&'s str),
    Universal,
}

fn key_of(sel: &Selector) -> Key<'_> {
    let Some(subject) = sel.compounds.last() else { return Key::Universal };
    let (mut class, mut ty, mut attr) = (None, None, None);
    for s in subject {
        match s {
            Simple::Id(id) => return Key::Id(id),
            Simple::Class(c) if class.is_none() => class = Some(c.as_str()),
            Simple::Type { lower, .. } if ty.is_none() => ty = Some(lower.as_str()),
            Simple::Attr { lower, .. } if attr.is_none() => attr = Some(lower.as_str()),
            _ => {}
        }
    }
    match (class, ty, attr) {
        (Some(c), _, _) => Key::Class(c),
        (None, Some(t), _) => Key::Type(t),
        (None, None, Some(a)) => Key::Attr(a),
        _ => Key::Universal,
    }
}

impl RuleIndex {
    fn build(entries: &[RuleEntry]) -> RuleIndex {
        let mut ix = RuleIndex::default();
        for (i, e) in entries.iter().enumerate() {
            let i = i as u32;
            match key_of(e.selector) {
                Key::Id(k) => ix.by_id.entry(String::from(k)).or_default().push(i),
                Key::Class(k) => ix.by_class.entry(String::from(k)).or_default().push(i),
                Key::Type(k) => ix.by_type.entry(String::from(k)).or_default().push(i),
                Key::Attr(k) => ix.by_attr.entry(String::from(k)).or_default().push(i),
                Key::Universal => ix.universal.push(i),
            }
            ix.masks.push(ancestor_mask(e.selector));
        }
        ix
    }

    /// The entries `el` could match, ascending (document order of the rules).
    fn candidates<E: Element>(&self, el: &E, out: &mut Vec<u32>) {
        out.clear();
        out.extend_from_slice(&self.universal);
        let name = el.local_name();
        let hit = if name.bytes().any(|b| b.is_ascii_uppercase()) {
            self.by_type.get(name.to_ascii_lowercase().as_str())
        } else {
            self.by_type.get(name)
        };
        if let Some(v) = hit {
            out.extend_from_slice(v);
        }
        let (by_id, by_class, by_attr) = (&self.by_id, &self.by_class, &self.by_attr);
        if !(by_id.is_empty() && by_class.is_empty() && by_attr.is_empty()) {
            el.each_attr(&mut |ns, n, v| {
                if ns.is_empty() && n == "id" {
                    if let Some(x) = by_id.get(v) {
                        out.extend_from_slice(x);
                    }
                } else if ns.is_empty() && n == "class" && !by_class.is_empty() {
                    for c in v.split(is_html_ws).filter(|c| !c.is_empty()) {
                        if let Some(x) = by_class.get(c) {
                            out.extend_from_slice(x);
                        }
                    }
                }
                if !by_attr.is_empty() {
                    let hit = if n.bytes().any(|b| b.is_ascii_uppercase()) {
                        by_attr.get(n.to_ascii_lowercase().as_str())
                    } else {
                        by_attr.get(n)
                    };
                    if let Some(x) = hit {
                        out.extend_from_slice(x);
                    }
                }
                false
            });
        }
        out.sort_unstable();
        out.dedup(); // a class listed twice on the element, an attribute bucket hit twice
    }

    /// (buckets, entries) per kind, for reports: id, class, type, attribute, universal.
    pub fn stats(&self) -> [(usize, usize); 5] {
        let n = |m: &BTreeMap<String, Vec<u32>>| (m.len(), m.values().map(|v| v.len()).sum());
        [n(&self.by_id), n(&self.by_class), n(&self.by_type), n(&self.by_attr), (1, self.universal.len())]
    }
}

/// Inputs for resolving conditional rules.
pub struct Conditions<'a, 'b> {
    pub env: &'b Environment,
    /// `@supports (prop: value)`: is the declaration supported?
    pub supports: &'b dyn Fn(&Declaration) -> bool,
    /// `@import url`: the imported sheet, when loaded.
    pub import: &'b dyn Fn(&str) -> Option<&'a Stylesheet>,
}

impl<'a> RuleSet<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a sheet (in document order) for `origin`.
    pub fn add_sheet(&mut self, sheet: &'a Stylesheet, origin: Origin, cond: &Conditions<'a, '_>) {
        self.index = None;
        let base: Vec<u32> = Vec::new();
        self.add_rules(&sheet.rules, origin, &base, "", &sheet.namespaces, cond);
    }

    /// The index path of a (possibly dotted) layer name under `prefix`, registering it on first sight.
    fn layer_path(&mut self, base: &[u32], prefix: &str, name: &str) -> (Vec<u32>, String) {
        let mut path = base.to_vec();
        let mut full = String::from(prefix);
        for seg in name.split('.') {
            if !full.is_empty() {
                full.push('.');
            }
            full.push_str(seg);
            let idx = match self.layers.iter().find(|(n, _)| *n == full) {
                Some((_, i)) => *i,
                None => {
                    // index among siblings: count registered layers with the same parent
                    let parent = String::from(layer_parent(&full));
                    let n = self.layers.iter().filter(|(l, _)| layer_parent(l) == parent).count() as u32;
                    self.layers.push((full.clone(), n));
                    n
                }
            };
            path.push(idx);
        }
        (path, full)
    }

    fn anonymous_layer(&mut self, base: &[u32], prefix: &str) -> (Vec<u32>, String) {
        let name = alloc::format!("\u{0}anon{}", self.layers.len());
        self.layer_path(base, prefix, &name)
    }

    fn add_rules(&mut self, rules: &'a [CssRule], origin: Origin, layer: &[u32], prefix: &str, ns: &Namespaces, cond: &Conditions<'a, '_>) {
        for r in rules {
            match r {
                CssRule::Style(s) => self.add_style(s, origin, layer, prefix, ns, cond),
                CssRule::Media(mq, inner) => {
                    if mq.matches(cond.env) {
                        self.add_rules(inner, origin, layer, prefix, ns, cond);
                    }
                }
                CssRule::Supports(c, inner) => {
                    if c.evaluate(ns, cond.supports) {
                        self.add_rules(inner, origin, layer, prefix, ns, cond);
                    }
                }
                CssRule::LayerStatement(names) => {
                    for n in names {
                        self.layer_path(layer_base(layer), prefix, n);
                    }
                }
                CssRule::LayerBlock(name, inner) => {
                    let (mut path, full) = match name {
                        Some(n) => self.layer_path(layer_base(layer), prefix, n),
                        None => self.anonymous_layer(layer_base(layer), prefix),
                    };
                    path.push(u32::MAX);
                    self.add_rules(inner, origin, &path, &full, ns, cond);
                }
                CssRule::Import(imp) => {
                    if !imp.media.matches(cond.env) {
                        continue;
                    }
                    if let Some(s) = &imp.supports
                        && !s.evaluate(ns, cond.supports) {
                            continue;
                        }
                    let Some(sheet) = (cond.import)(&imp.url) else { continue };
                    match &imp.layer {
                        None => self.add_rules(&sheet.rules, origin, layer, prefix, &sheet.namespaces, cond),
                        Some(l) => {
                            let (mut path, full) = match l {
                                Some(n) => self.layer_path(layer_base(layer), prefix, n),
                                None => self.anonymous_layer(layer_base(layer), prefix),
                            };
                            path.push(u32::MAX);
                            self.add_rules(&sheet.rules, origin, &path, &full, &sheet.namespaces, cond);
                        }
                    }
                }
                CssRule::FontFace(f) => self.font_faces.push(f),
                CssRule::Keyframes(k) => self.keyframes.push(k),
                CssRule::Other(_) => {}
            }
        }
    }

    fn add_style(&mut self, s: &'a StyleRule, origin: Origin, layer: &[u32], prefix: &str, ns: &Namespaces, cond: &Conditions<'a, '_>) {
        let order = self.next_order;
        self.next_order += 1;
        let own = if layer.is_empty() { alloc::vec![u32::MAX] } else { layer.to_vec() };
        if !s.declarations.is_empty() {
            for sel in &s.selectors.0 {
                self.entries.push(RuleEntry { selector: sel, declarations: &s.declarations, origin, layer: own.clone(), order });
            }
        }
        // nested style rules and nested conditional / layer rules, in order
        self.add_rules(&s.children, origin, layer, prefix, ns, cond);
    }

    /// Build the rule hash over the current entries (call after the last [`RuleSet::add_sheet`]);
    /// [`cascade`] then walks only each element's candidate buckets. Adding a sheet drops the index.
    pub fn build_index(&mut self) {
        self.index = Some(RuleIndex::build(&self.entries));
    }

    pub fn index(&self) -> Option<&RuleIndex> {
        self.index.as_ref()
    }

    /// The `@keyframes` rule in force for `name` (the last one wins).
    pub fn keyframes(&self, name: &str) -> Option<&'a Keyframes> {
        self.keyframes.iter().rev().find(|k| k.name == name).copied()
    }
}

fn layer_parent(full: &str) -> &str {
    match full.rfind('.') {
        Some(i) => &full[..i],
        None => "",
    }
}

/// Strip the trailing "own rules" marker of a layer path, giving the base for sub-layers.
fn layer_base(layer: &[u32]) -> &[u32] {
    match layer.last() {
        Some(&u32::MAX) => &layer[..layer.len() - 1],
        _ => layer,
    }
}

/// A declaration that applies to the element, with its cascade-sort inputs.
#[derive(Clone, Debug)]
pub struct Applied<'a> {
    pub declaration: &'a Declaration,
    pub origin: Origin,
    pub important: bool,
    /// From the element's `style` attribute.
    pub inline: bool,
    pub layer: Vec<u32>,
    pub specificity: Specificity,
    pub order: u32,
    /// Position inside its declaration block.
    pub index: u32,
}

/// origin + importance band (Cascade 5 §6.1), ascending precedence
fn band(origin: Origin, important: bool) -> u8 {
    match (important, origin) {
        (false, Origin::UserAgent) => 0,
        (false, Origin::User) => 1,
        (false, Origin::Author) => 2,
        (true, Origin::Author) => 3,
        (true, Origin::User) => 4,
        (true, Origin::UserAgent) => 5,
    }
}

/// The cascade order (ascending: later wins).
pub fn cascade_order(a: &Applied, b: &Applied) -> Ordering {
    band(a.origin, a.important)
        .cmp(&band(b.origin, b.important))
        .then(a.inline.cmp(&b.inline))
        .then_with(|| {
            let o = a.layer.cmp(&b.layer);
            if a.important { o.reverse() } else { o }
        })
        .then(a.specificity.cmp(&b.specificity))
        .then(a.order.cmp(&b.order))
        .then(a.index.cmp(&b.index))
}

/// Every declaration that applies to `el` (or to its `pseudo` element), sorted by cascade order
/// ascending — apply them in order and the last declaration of each property wins. `inline` is the
/// element's `style` attribute (author origin).
pub fn cascade<'a, E: Element>(rules: &RuleSet<'a>, el: &E, inline: &'a [Declaration], pseudo: Option<&PseudoElement>) -> Vec<Applied<'a>> {
    cascade_filtered(rules, el, inline, pseudo, None)
}

/// [`cascade`] with the element's [`AncestorFilter`] (its ancestors' ids, classes and type names):
/// with the rule hash built, selectors whose ancestor requirements the filter rules out are skipped
/// before matching. `None` is exactly [`cascade`].
pub fn cascade_filtered<'a, E: Element>(
    rules: &RuleSet<'a>,
    el: &E,
    inline: &'a [Declaration],
    pseudo: Option<&PseudoElement>,
    ancestors: Option<&AncestorFilter>,
) -> Vec<Applied<'a>> {
    let cx = MatchContext::new();
    let mut out: Vec<Applied<'a>> = Vec::new();
    // One rule may match through several of its selectors: keep the highest specificity per rule. A
    // rule's selectors are consecutive entries and candidates are walked in entry order, so the
    // selectors of one rule meet as neighbours in `matched`.
    let mut matched: Vec<(&RuleEntry<'a>, Specificity)> = Vec::new();
    let mut cand = Vec::new();
    let walk_all = match &rules.index {
        Some(ix) => {
            ix.candidates(el, &mut cand);
            false
        }
        None => true,
    };
    let n = if walk_all { rules.entries.len() } else { cand.len() };
    for k in 0..n {
        let i = if walk_all { k } else { cand[k] as usize };
        if let (Some(f), Some(ix)) = (ancestors, &rules.index)
            && !f.may_contain(&ix.masks[i])
        {
            continue;
        }
        let e = &rules.entries[i];
        if !matches_for_pseudo(e.selector, el, pseudo, &cx) {
            continue;
        }
        let spec = e.selector.specificity;
        match matched.last_mut() {
            Some(last) if last.0.order == e.order => {
                if spec > last.1 {
                    last.1 = spec;
                }
            }
            _ => matched.push((e, spec)),
        }
    }
    for (e, spec) in matched {
        for (i, d) in e.declarations.iter().enumerate() {
            out.push(Applied {
                declaration: d,
                origin: e.origin,
                important: d.important,
                inline: false,
                layer: e.layer.clone(),
                specificity: spec,
                order: e.order,
                index: i as u32,
            });
        }
    }
    if pseudo.is_none() {
        for (i, d) in inline.iter().enumerate() {
            out.push(Applied {
                declaration: d,
                origin: Origin::Author,
                important: d.important,
                inline: true,
                layer: alloc::vec![u32::MAX],
                specificity: Specificity::ZERO,
                order: u32::MAX,
                index: i as u32,
            });
        }
    }
    out.sort_by(cascade_order);
    out
}
