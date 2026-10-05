//! CSS Cascading and Inheritance Level 5, §6: collecting the declarations that apply to an element and
//! sorting them by the cascade order — origin and importance, element-attached (style attribute),
//! cascade layers, specificity, order of appearance. Conditional rules are resolved once per
//! [`Environment`] when sheets are added to a [`RuleSet`]; `@import`s through a resolver callback.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::matching::{matches_for_pseudo, Element, MatchContext};
use crate::media::Environment;
use crate::parser::Declaration;
use crate::selectors::{Namespaces, PseudoElement, Selector, Specificity};
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
    let cx = MatchContext::new();
    let mut out: Vec<Applied<'a>> = Vec::new();
    // one rule may match through several of its selectors: keep the highest specificity per rule
    let mut best: Vec<(u32, usize)> = Vec::new(); // (order, index into `matched`)
    let mut matched: Vec<(&RuleEntry<'a>, Specificity)> = Vec::new();
    for e in &rules.entries {
        if !matches_for_pseudo(e.selector, el, pseudo, &cx) {
            continue;
        }
        let spec = e.selector.specificity;
        match best.iter().find(|(o, _)| *o == e.order) {
            Some(&(_, k)) => {
                if spec > matched[k].1 {
                    matched[k] = (e, spec);
                }
            }
            None => {
                best.push((e.order, matched.len()));
                matched.push((e, spec));
            }
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
