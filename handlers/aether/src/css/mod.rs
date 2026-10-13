//! Aether's CSS: the cascade (css_core end to end) and the computed-value report. LAYOUTCORE (SR76,
//! AETHERMETAL M2): the taffy-style half — `SpecifiedStyle`, `apply_declaration`, the value parsers, the
//! paint merge — moved into `layout_core::css` (unaos/libs/web/layout_core), the ONE copy both rings parse
//! declarations with; the names below are that core under the host's hooks (`layout::AetherHost`) and the
//! host's parse context (the cascade's viewport, the initial 16px font basis).

use crate::layout::{AetherHost, LayoutTree};

pub(crate) mod anim;
use taffy::prelude::*;
use taffy::style::{FlexDirection, LengthPercentage, LengthPercentageAuto};

pub(crate) use layout_core::css::{split_top_level, Content, ContentItem, ParseCx, Quotes};
#[allow(unused_imports)] // engine_tests
pub(crate) use layout_core::css::extract_css_url;
pub use layout_core::css::{is_neutral_keyword, parse_font_stretch, parse_font_weight};

/// The declarations a rule specified, folded for this host (`layout_core::css::SpecifiedStyle`).
pub(crate) type SpecifiedStyle = layout_core::css::SpecifiedStyle<AetherHost>;

/// The context the host parses declarations in: the viewport [`set_viewport`] recorded, the initial font
/// basis (em 16, rem 16, ex 8, ch 8) — `layout_core::remeasure` re-applies font-relative lengths itself.
pub(crate) fn parse_cx() -> ParseCx {
    ParseCx { viewport: VIEWPORT.with(|v| v.get()), ..ParseCx::default() }
}

/// One declaration, shared by the stylesheet cascade and inline styles (`layout_core::css::apply_declaration`).
pub(crate) fn apply_declaration(prop: &str, value: &str, style: &mut SpecifiedStyle) {
    layout_core::css::apply_declaration(prop, value, style, &parse_cx());
}

/// Truncates a value for a stable, bounded ledger key.
fn clip(v: &str) -> &str {
    layout_core::css::clip(v)
}

/// Parses a font-size string at the 16px UA base (`layout_core::css::parse_font_size`).
pub fn parse_font_size(value: &str) -> Option<f32> {
    layout_core::css::parse_font_size(value, &parse_cx())
}

/// Parses a length into pixels (`layout_core::css::parse_px`).
pub fn parse_px(value: &str) -> Option<f32> {
    layout_core::css::parse_px(value, &parse_cx())
}

/// Evaluates a CSS math expression into pixels (`layout_core::css::eval_length`).
pub fn eval_length(value: &str, reference: Option<f32>) -> Option<f32> {
    layout_core::css::eval_length(value, reference, &parse_cx())
}

/// The `flex` shorthand → (grow, shrink, basis) (`layout_core::css::parse_flex_shorthand`).
#[allow(dead_code)] // engine_tests
pub(crate) fn parse_flex_shorthand(value: &str) -> Option<(f32, f32, Dimension)> {
    layout_core::css::parse_flex_shorthand(value, &parse_cx())
}

/// Parses a CSS color from a string (`layout_core::css::parse_color_str`).
pub fn parse_color_str(value: &str) -> Option<(u8, u8, u8)> {
    layout_core::css::parse_color_str::<AetherHost>(value)
}

/// The document element's computed custom property `name`, for the layout builder's `style=""` pass
/// (`layout::AetherHost::custom_prop`).
pub(crate) fn custom_prop(name: &str) -> Option<String> {
    CUSTOM_PROPS.with(|m| m.borrow().get(name).cloned())
}

thread_local! {
    /// The document element's computed custom properties (css_core, set by
    /// `apply_stylesheets`), for the one path outside the cascade: the
    /// layout builder's build-time inline-style pass. The cascade itself
    /// substitutes `var()` per element against that element's own computed
    /// custom properties.
    static CUSTOM_PROPS: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}


// ---------------------------------------------------------------------
// The cascade (AETHERSTYLE, SR54): UnaOS's own css_core end to end.
//
// Parsing (CSS Syntax 3 + CSS Nesting), selector matching (Selectors 4 over html_core's arena through
// `dom::El`), `@media` (MQ4 against Aether's viewport), `@supports` (against `property_supported`),
// `@layer`, the cascade sort (origin, importance, style attribute, layers, specificity, order), the rule
// hash and per-element custom properties with `var()` substitution are css_core's. What stays Aether's is
// the computed-value step: every declaration that wins its place in the cascade order is handed, as CSS
// text, to `apply_declaration`, which folds it into the `SpecifiedStyle` that layout and paint consume.
// ---------------------------------------------------------------------

thread_local! {
    /// (style, re-layout) wall time of the last `apply_stylesheets`.
    static STYLE_TIMING: std::cell::Cell<(std::time::Duration, std::time::Duration)> =
        const { std::cell::Cell::new((std::time::Duration::ZERO, std::time::Duration::ZERO)) };
}

/// Wall time of the last `apply_stylesheets` call: (the cascade up to the folded box styles, the
/// re-layout that follows). Read by `tests/style_time.rs`.
pub fn last_style_timing() -> (std::time::Duration, std::time::Duration) {
    STYLE_TIMING.with(|t| t.get())
}

thread_local! {
    /// How focus last moved: `true` after a pointer interaction (a click that focused), `false` after
    /// the keyboard or script. The `:focus-visible` heuristic (`dom::select`) reads it (AETHERGEN, SR71).
    static POINTER_FOCUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Records the modality of the interaction that moved focus (Selectors 4 §9.4 `:focus-visible`):
/// `true` for a mouse/touch focus, `false` for keyboard navigation. The engine calls this where it
/// moves focus; script focus leaves the last modality in place, as the HTML heuristic does.
pub fn note_focus_modality(pointer: bool) {
    POINTER_FOCUS.with(|p| p.set(pointer));
}

/// The modality [`note_focus_modality`] last recorded.
pub(crate) fn pointer_focus() -> bool {
    POINTER_FOCUS.with(|p| p.get())
}

/// What `@media` is evaluated against: Aether's viewport, a light colour scheme, no reduced motion, a
/// fine pointer that can hover, `screen`, scripting enabled (css_core's desktop defaults).
pub(crate) fn media_environment(vw: f32, vh: f32) -> css_core::media::Environment {
    css_core::media::Environment::viewport(vw.max(1.0) as f64, vh.max(1.0) as f64)
}

/// Ledgers what a sheet asks for that Aether does not do with it: rules that style a pseudo-element
/// Aether generates no box for (`pseudo-element-rule:<name>`; `::before`/`::after`/`::marker` are
/// generated since AETHERGEN, SR71) and at-rules css_core parses into data Aether does not consume.
fn ledger_rules(rules: &[css_core::stylesheet::CssRule]) {
    use css_core::selectors::PseudoElement as PE;
    use css_core::stylesheet::CssRule;
    for r in rules {
        match r {
            CssRule::Style(s) => {
                let mut seen: Vec<&str> = Vec::new();
                for sel in &s.selectors.0 {
                    let name = match &sel.pseudo_element {
                        None | Some(PE::Before | PE::After | PE::Marker) => continue,
                        Some(PE::FirstLine) => "first-line",
                        Some(PE::FirstLetter) => "first-letter",
                        Some(PE::Placeholder) => "placeholder",
                        Some(PE::Selection) => "selection",
                        Some(PE::Backdrop) => "backdrop",
                        Some(PE::FileSelectorButton) => "file-selector-button",
                        Some(PE::Slotted(_)) => "slotted",
                        Some(PE::Part(_)) => "part",
                        Some(PE::Highlight(_)) => "highlight",
                    };
                    if !seen.contains(&name) {
                        seen.push(name);
                        crate::ledger::record_css(&format!("pseudo-element-rule:{name}"));
                    }
                }
                ledger_rules(&s.children);
            }
            CssRule::Media(_, inner) | CssRule::Supports(_, inner) | CssRule::LayerBlock(_, inner) => ledger_rules(inner),
            CssRule::LayerStatement(_) => {}
            CssRule::Import(_) => crate::ledger::record_css("at-rule:@import"),
            CssRule::FontFace(_) => {}
            CssRule::Keyframes(_) => {} // AETHERSTYLE3 (SR103): run by `anim`
            CssRule::Container(_, inner) => ledger_rules(inner),
            CssRule::Other(a) => crate::ledger::record_css(&format!("at-rule:@{}", a.name.to_ascii_lowercase())),
        }
    }
}

/// A declaration's value as CSS text for the computed-value step.
fn value_text(v: &[css_core::CV]) -> String {
    css_core::serialize::to_css(css_core::parser::trim_ws(v))
}

pub fn apply_css(layout_tree: &mut LayoutTree, css: &str) {
    apply_stylesheets(layout_tree, std::slice::from_ref(&css.to_string()));
}

/// The cascaded style of every element of the document: for each element with a box, the
/// `SpecifiedStyle` its winning declarations fold into (`None` when no declaration applies).
///
/// Per element: css_core's `cascade` (rule-hash candidates, matching, the full Cascade 5 sort, the style
/// attribute) → its custom properties computed against the parent's (`compute_custom_properties`:
/// inheritance, cycles, guaranteed-invalid) → every other declaration, in cascade order, `var()`
/// substituted (`substitute_vars`; a failed substitution is invalid at computed-value time and dropped)
/// and handed to `apply_declaration`. Elements whose winning declarations are the same list (and use no
/// `var()`) share one folded style — the common case of every `<p>` or every `.reference` on a page.
fn cascade_document(
    doc: &html_core::Document,
    rules: &css_core::cascade::RuleSet<'_>,
    pseudo_rules: &[css_core::cascade::RuleSet<'_>; 3],
    boxed: &std::collections::HashSet<html_core::NodeId>,
    containers: Option<&ContainerSizes>,
) -> (
    std::collections::HashMap<html_core::NodeId, std::rc::Rc<SpecifiedStyle>>,
    std::collections::BTreeMap<String, Vec<css_core::CV>>,
    std::collections::HashMap<html_core::NodeId, PseudoStyles>,
) {
    use css_core::values::{compute_custom_properties, contains_var, substitute_vars};
    use std::collections::{BTreeMap, HashMap};
    use std::rc::Rc;
    type Customs = Rc<BTreeMap<String, Vec<css_core::CV>>>;
    let empty: Customs = Rc::new(BTreeMap::new());
    let mut customs: HashMap<html_core::NodeId, Customs> = HashMap::new();
    // each element's children's ancestor Bloom filter (css_core's quick reject)
    let mut child_filter: HashMap<html_core::NodeId, css_core::cascade::AncestorFilter> = HashMap::new();
    let mut shared: HashMap<Vec<usize>, Rc<SpecifiedStyle>> = HashMap::new();
    let mut out = HashMap::new();
    let mut pseudo_out: HashMap<html_core::NodeId, PseudoStyles> = HashMap::new();
    let mut pshared: HashMap<(usize, Vec<usize>), Rc<SpecifiedStyle>> = HashMap::new();
    let pseudo_kinds = [
        css_core::selectors::PseudoElement::Marker,
        css_core::selectors::PseudoElement::Before,
        css_core::selectors::PseudoElement::After,
    ];
    let mut root_customs = BTreeMap::new();
    for id in doc.descendants(html_core::Document::ROOT) {
        let Some(el) = doc.element(id) else { continue };
        let inline = el.attr("style").filter(|t| crate::security::csp::allow_style_attr(t)).map(css_core::stylesheet::parse_style_attribute).unwrap_or_default();
        let me_el = crate::dom::El::new(doc, id);
        let filter = doc.parent_element(id).and_then(|p| child_filter.get(&p)).copied().unwrap_or_default();
        // AETHERSTYLE3 (SR103): `@container` rules ask the element's query container (the nearest ancestor
        // container whose name matches, css-contain-3 §3.1) as the first layout sized it.
        // AETHERSTYLE3B (SR115): every element is a style container — a rule with only `style()` queries asks the
        // nearest (named) ancestor's computed custom properties, in the first round already; a rule with size
        // features needs a size container.
        let query = |c: &css_core::container::ContainerRule| -> bool {
            let needs_size = c.needs_size();
            let mut cur = doc.parent_element(id);
            while let Some(a) = cur {
                let sized = containers.and_then(|s| s.get(&a));
                let names = match sized {
                    Some(q) => q.names.clone(),
                    None => out.get(&a).and_then(|s: &std::rc::Rc<SpecifiedStyle>| s.paint.container_name.clone()).unwrap_or_default(),
                };
                if (!needs_size || sized.is_some()) && c.names(&names) {
                    let props = customs.get(&a);
                    let lookup = |n: &str| props.and_then(|m| m.get(n)).map(|v| css_core::serialize::to_css(css_core::parser::trim_ws(v)));
                    return c.evaluate_with(sized.map(|q| &q.size), &lookup) == css_core::media::Tri::True;
                }
                cur = doc.parent_element(a);
            }
            false
        };
        let applied = css_core::cascade::cascade_contained(rules, &me_el, &inline, None, Some(&filter), Some(&query));
        child_filter.insert(id, filter.with_element(&me_el));
        // custom properties: inherited from the parent element, overridden by this element's own
        let parent = doc.parent_element(id).and_then(|p| customs.get(&p)).cloned().unwrap_or_else(|| empty.clone());
        let own: Vec<(String, Vec<css_core::CV>)> = applied
            .iter()
            .filter(|a| a.declaration.name.starts_with("--"))
            .map(|a| (a.declaration.name.clone(), a.declaration.value.clone()))
            .collect();
        let mine = if own.is_empty() { parent } else { Rc::new(compute_custom_properties(&own, &parent)) };
        if doc.parent(id) == Some(html_core::Document::ROOT) {
            root_customs = (*mine).clone();
        }
        if !boxed.contains(&id) {
            customs.insert(id, mine);
            continue;
        }
        // AETHERGEN (SR71): the ::marker / ::before / ::after of this element, cascaded on their own
        // (css_core matches a pseudo-element selector against its originating element). A
        // pseudo-element's custom properties inherit from the originating element's.
        let mut ps: PseudoStyles = Default::default();
        for (k, prs) in pseudo_rules.iter().enumerate() {
            if prs.entries.is_empty() {
                continue;
            }
            let applied = css_core::cascade::cascade_contained(prs, &me_el, &[], Some(&pseudo_kinds[k]), Some(&filter), Some(&query));
            if applied.is_empty() {
                continue;
            }
            // Shared like element styles: the same winning declarations, no var(), one folded style
            // (`*::before, *::after { box-sizing: border-box }` matches every element of a page).
            let pkey: Vec<usize> = applied.iter().map(|a| a.declaration as *const _ as usize).collect();
            let pshareable = !applied.iter().any(|a| a.declaration.name.starts_with("--") || contains_var(&a.declaration.value));
            if pshareable {
                if let Some(st) = pshared.get(&(k, pkey.clone())) {
                    ps[k] = Some(st.clone());
                    continue;
                }
            }
            let own: Vec<(String, Vec<css_core::CV>)> = applied
                .iter()
                .filter(|a| a.declaration.name.starts_with("--"))
                .map(|a| (a.declaration.name.clone(), a.declaration.value.clone()))
                .collect();
            let pc = if own.is_empty() { mine.clone() } else { Rc::new(compute_custom_properties(&own, &mine)) };
            let mut s = SpecifiedStyle::default();
            for a in applied.iter().filter(|a| !a.declaration.name.starts_with("--")) {
                let d = a.declaration;
                let name = d.name.to_ascii_lowercase();
                if contains_var(&d.value) {
                    match substitute_vars(&d.value, &|n| pc.get(n).cloned()) {
                        Ok(v) => apply_declaration(&name, &value_text(&v), &mut s),
                        Err(()) => crate::ledger::record_css(&format!("var-unresolved:{}", clip(&name))),
                    }
                } else {
                    apply_declaration(&name, &value_text(&d.value), &mut s);
                }
            }
            let s = Rc::new(s);
            if pshareable {
                pshared.insert((k, pkey), s.clone());
            }
            ps[k] = Some(s);
        }
        if ps.iter().any(Option::is_some) {
            pseudo_out.insert(id, ps);
        }
        let decls: Vec<&css_core::parser::Declaration> =
            applied.iter().map(|a| a.declaration).filter(|d| !d.name.starts_with("--")).collect();
        if decls.is_empty() {
            customs.insert(id, mine);
            continue;
        }
        let uses_var = decls.iter().any(|d| contains_var(&d.value));
        // The share key is the winning declarations' addresses in the rule set. A style attribute's
        // declarations live in `inline`, which is freed at the end of this iteration, so the next
        // element's attribute can reuse the same addresses: an element with a style attribute never
        // shares (the AETHERFONT join found two inline-styled siblings folded into one style).
        let shareable = !uses_var && inline.is_empty();
        let key: Vec<usize> = decls.iter().map(|d| *d as *const _ as usize).collect();
        let style = match shareable.then(|| shared.get(&key)).flatten() {
            Some(s) => s.clone(),
            None => {
                let mut s = SpecifiedStyle::default();
                for d in &decls {
                    let name = d.name.to_ascii_lowercase();
                    if contains_var(&d.value) {
                        match substitute_vars(&d.value, &|n| mine.get(n).cloned()) {
                            Ok(v) => apply_declaration(&name, &value_text(&v), &mut s),
                            Err(()) => crate::ledger::record_css(&format!("var-unresolved:{}", clip(&name))),
                        }
                    } else {
                        apply_declaration(&name, &value_text(&d.value), &mut s);
                    }
                }
                let s = Rc::new(s);
                if shareable {
                    shared.insert(key, s.clone());
                }
                s
            }
        };
        out.insert(id, style);
        customs.insert(id, mine);
    }
    (out, root_customs, pseudo_out)
}

/// The cascaded styles of an element's ::marker, ::before and ::after (in that order; `None` = no
/// rule styles it).
pub(crate) type PseudoStyles = [Option<std::rc::Rc<SpecifiedStyle>>; 3];

/// AETHERSTYLE3 (SR103): each query container's names and content-box size, by element, from a first layout.
pub(crate) struct QueryContainer {
    pub names: Vec<String>,
    pub size: css_core::container::ContainerSize,
}
pub(crate) type ContainerSizes = std::collections::HashMap<html_core::NodeId, QueryContainer>;

/// The query containers of a laid-out tree: every element whose `container-type` is `size` (both axes) or
/// `inline-size` (the inline axis), at its content-box size.
fn container_sizes(tree: &LayoutTree) -> ContainerSizes {
    let mut out = ContainerSizes::new();
    for (tid, n) in &tree.node_map {
        let Some(p) = tree.paint_map.get(tid) else { continue };
        let ty = p.container_type.unwrap_or(0);
        if ty == 0 || !n.is_element() {
            continue;
        }
        let Ok(l) = tree.taffy.layout(*tid) else { continue };
        let w = (l.size.width - l.padding.left - l.padding.right - l.border.left - l.border.right).max(0.0) as f64;
        let h = (l.size.height - l.padding.top - l.padding.bottom - l.border.top - l.border.bottom).max(0.0) as f64;
        let vertical = layout_core::writing::is_vertical(p.writing_mode.unwrap_or(0));
        let (iw, ih) = match (ty, vertical) {
            (1, _) => (Some(w), Some(h)),
            (_, false) => (Some(w), None),
            (_, true) => (None, Some(h)),
        };
        let fs = p.used_font_size.unwrap_or(16.0) as f64;
        out.insert(
            n.id(),
            QueryContainer {
                names: p.container_name.clone().unwrap_or_default(),
                size: css_core::container::ContainerSize { width: iw, height: ih, vertical, font_size: fs },
            },
        );
    }
    out
}

/// Applies a set of stylesheets (document order, author origin) as ONE cascade through css_core, with
/// each element's `style` attribute at its CSS priority (above normal rules, below `!important` ones;
/// an `!important` style attribute beats every author rule). AETHERSTYLE3 (SR103): when the sheets hold
/// `@container` rules, the first cascade + layout sizes the query containers and the tree is rebuilt and
/// cascaded again with those sizes (css-contain-3 §4: container rules apply against the containers' laid-out
/// size; one round, so a rule that resizes its own container does not loop).
pub fn apply_stylesheets(layout_tree: &mut LayoutTree, sheets: &[String]) {
    let mut round = apply_stylesheets_with(layout_tree, sheets, None);
    if let Some(sizes) = round.containers.take() {
        if let Some(root) = layout_tree.node_map.get(&layout_tree.root_node).cloned() {
            let (vw, vh) = layout_tree.viewport;
            *layout_tree = crate::layout::build_tree(&root, vw, vh);
            round = apply_stylesheets_with(layout_tree, sheets, Some(&sizes));
        }
    }
    // AETHERSTYLE3 (SR103): transitions start from the style change, animations follow the names, and the
    // values running now are applied over the fresh layout.
    anim::sync(layout_tree, &round.styles, round.keyframes);
}

/// What one cascade round hands back: the query-container sizes when a second round is due, each element's
/// computed style and the document's `@keyframes`.
#[derive(Default)]
struct Round {
    containers: Option<ContainerSizes>,
    styles: std::collections::HashMap<html_core::NodeId, std::rc::Rc<SpecifiedStyle>>,
    keyframes: Vec<(String, layout_core::anim::KeyframeSet)>,
}

/// One cascade + layout. With `containers` None, returns the query-container sizes when the sheets carry
/// `@container` rules and the page has query containers (the caller's cue for the second round).
fn apply_stylesheets_with(layout_tree: &mut LayoutTree, sheets: &[String], containers: Option<&ContainerSizes>) -> Round {
    let t0 = std::time::Instant::now();
    let (vw, vh) = layout_tree.viewport;
    set_viewport(vw, vh);
    let parsed: Vec<css_core::stylesheet::Stylesheet> =
        sheets.iter().map(|s| css_core::stylesheet::parse_stylesheet(s)).collect();
    if containers.is_none() {
        for s in &parsed {
            ledger_rules(&s.rules);
        }
        // css-fonts-4 §4: the document's @font-face faces (AETHERFONT); a changed set bumps the font caches.
        crate::fonts::webfont::apply_rules(&parsed, &crate::images::page_base());
    }
    let env = media_environment(vw, vh);
    let supports = |d: &css_core::parser::Declaration| property_supported(&d.name.to_ascii_lowercase());
    let import = |_: &str| -> Option<&css_core::stylesheet::Stylesheet> { None };
    let cond = css_core::cascade::Conditions { env: &env, supports: &supports, import: &import };
    let mut rules = css_core::cascade::RuleSet::new();
    for s in &parsed {
        rules.add_sheet(s, css_core::cascade::Origin::Author, &cond);
    }
    let t_parse = t0.elapsed();
    // AETHER_NO_RULE_INDEX: the full scan, for measuring the hash (tests/style_time.rs);
    // AETHER_STYLE_PROFILE: print the phase times.
    if std::env::var_os("AETHER_NO_RULE_INDEX").is_none() {
        rules.build_index();
    }

    // The boxes of each DOM element (a layout node maps back to one DOM node).
    let mut boxes: std::collections::HashMap<html_core::NodeId, Vec<taffy::NodeId>> = std::collections::HashMap::new();
    let mut any: Option<crate::dom::NodeRef> = None;
    for (tid, n) in &layout_tree.node_map {
        if n.is_element() {
            boxes.entry(n.id()).or_default().push(*tid);
            any.get_or_insert_with(|| n.clone());
        }
    }
    let Some(any) = any else {
        STYLE_TIMING.with(|t| t.set((t0.elapsed(), std::time::Duration::ZERO)));
        return Round::default();
    };
    let boxed: std::collections::HashSet<html_core::NodeId> = boxes.keys().copied().collect();
    // AETHERGEN (SR71): the rules that style ::marker, ::before and ::after, one hashed set each.
    let pseudo_rules: [css_core::cascade::RuleSet<'_>; 3] = std::array::from_fn(|k| {
        use css_core::selectors::PseudoElement as PE;
        let mut set = css_core::cascade::RuleSet::new();
        for e in &rules.entries {
            let hit = match (&e.selector.pseudo_element, k) {
                (Some(PE::Marker), 0) | (Some(PE::Before), 1) | (Some(PE::After), 2) => true,
                _ => false,
            };
            if hit {
                set.entries.push(css_core::cascade::RuleEntry {
                    selector: e.selector,
                    declarations: e.declarations,
                    origin: e.origin,
                    layer: e.layer.clone(),
                    order: e.order,
                    container: e.container.clone(),
                });
            }
        }
        if std::env::var_os("AETHER_NO_RULE_INDEX").is_none() {
            set.build_index();
        }
        set
    });
    let t_c = std::time::Instant::now();
    let (styles, root_customs, pseudo_styles) =
        any.with_doc(|doc, _| cascade_document(doc, &rules, &pseudo_rules, &boxed, containers));
    if std::env::var_os("AETHER_STYLE_PROFILE").is_some() {
        eprintln!(
            "[style] parse + rule set {:?}, cascade {:?}, {} selector entries, rule hash {}",
            t_parse,
            t_c.elapsed(),
            rules.entries.len(),
            if rules.index().is_some() { "on" } else { "off (AETHER_NO_RULE_INDEX)" }
        );
    }
    // The build-time inline-style pass (layout) resolves var() against the document's tokens.
    CUSTOM_PROPS.with(|m| {
        let mut map = m.borrow_mut();
        map.clear();
        for (k, v) in &root_customs {
            map.insert(k.clone(), value_text(v));
        }
    });

    LAST_CURSORS.with(|c| {
        let mut c = c.borrow_mut();
        c.clear();
        c.extend(styles.iter().filter_map(|(id, s)| s.cursor.map(|k| (*id, k))));
    });
    let round_styles = styles.clone();
    // the last `@keyframes` of each name wins (css-animations-1 §3)
    let mut keyframes: Vec<(String, layout_core::anim::KeyframeSet)> = Vec::new();
    for k in &rules.keyframes {
        keyframes.retain(|(n, _)| *n != k.name);
        keyframes.push((k.name.clone(), layout_core::anim::KeyframeSet::from_rule(k)));
    }
    // AETHERGEN (SR71): counters, quotes and the ::marker/::before/::after boxes (layout::generated),
    // the generated boxes folded with their pseudo-element's cascaded style below like any other box.
    let generated = crate::layout::generated::generate(layout_tree, &any, &styles, &pseudo_styles, &boxes);
    let mut acc: std::collections::HashMap<taffy::NodeId, std::rc::Rc<SpecifiedStyle>> = std::collections::HashMap::new();
    for (dom_id, style) in styles {
        for tid in boxes.get(&dom_id).into_iter().flatten() {
            acc.insert(*tid, style.clone());
        }
    }
    acc.extend(generated);
    // LAYOUTCORE (SR76): the fold onto the boxes, the flex-item auto widths and the static-position fallback
    // are layout_core's `apply_specs` (the same pass AETHER.ELF's cascade ends in).
    let specs: Vec<(taffy::NodeId, std::rc::Rc<SpecifiedStyle>)> = acc.iter().map(|(k, v)| (*k, v.clone())).collect();
    layout_core::apply_specs(layout_tree, &specs);
    LAST_CASCADE.with(|l| *l.borrow_mut() = acc.clone());
    let t1 = std::time::Instant::now();
    // set_style only marks nodes dirty; re-lay out with text measurement.
    crate::layout::remeasure(layout_tree);
    // AETHERSTYLE3 (SR103): scroll containers keep their offsets across a relayout.
    crate::layout::restore_scroll(layout_tree);
    STYLE_TIMING.with(|t| t.set((t1 - t0, t1.elapsed())));
    let mut round = Round { containers: None, styles: round_styles, keyframes };
    if containers.is_none() && rules.has_container_rules() {
        let sizes = container_sizes(layout_tree);
        if !sizes.is_empty() {
            round.containers = Some(sizes);
        }
    }
    round
}

/// AETHERSTYLE3 (SR103): one animation frame (`anim::frame`) — the engine repaints or relayouts on its answer.
///
/// AETHERSTYLE3B (SR115): the frame also advances programmatic scrolling (`layout::scroll_frame`: script
/// scrolls start, smooth scrolls step) — a moved offset repaints.
pub(crate) fn animation_frame(tree: &mut LayoutTree) -> anim::Frame {
    let scrolled = crate::layout::scroll_frame(tree);
    match anim::frame(tree) {
        anim::Frame::Idle if scrolled => anim::Frame::Repaint,
        f => f,
    }
}

thread_local! {
    /// The `cursor` keyword each element's own winning declarations chose, from the last
    /// `apply_stylesheets` (AETHERHOVER, SR67) — inheritance and `auto` are resolved by [`cursor_for`].
    static LAST_CURSORS: std::cell::RefCell<std::collections::HashMap<html_core::NodeId, &'static str>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The used cursor over `node` (AETHERHOVER, SR67): the nearest element, from `node` outward, whose own
/// declarations set `cursor` (it is inherited), where a link (`a`/`area` with `href`) carries the UA
/// sheet's `cursor: pointer` (Chromium's `:any-link`; an author declaration on the same element wins).
/// `auto` resolves as css-ui-4 §5.1 leaves it to the UA: the I-beam over text (`over_text`, or an
/// editable text control), the arrow otherwise. The result is a css-ui-4 keyword, which is also the
/// GTK/freedesktop cursor name the shell hands the platform.
pub fn cursor_for(node: &crate::dom::NodeRef, over_text: bool) -> String {
    let set = LAST_CURSORS.with(|c| {
        let c = c.borrow();
        node.with_doc(|doc, id| {
            let mut cur = if doc.element(id).is_some() { Some(id) } else { doc.parent_element(id) };
            while let Some(e) = cur {
                if let Some(k) = c.get(&e) {
                    return Some(*k);
                }
                if let Some(el) = doc.element(e) {
                    if matches!(el.local.as_str(), "a" | "area") && el.attr("href").is_some() {
                        return Some("pointer");
                    }
                }
                cur = doc.parent_element(e);
            }
            None
        })
    });
    match set {
        Some(k) if k != "auto" => k.to_string(),
        _ => {
            let editable = node.with_doc(|doc, id| {
                doc.element(id).is_some_and(|el| match el.local.as_str() {
                    "textarea" => true,
                    "input" => !matches!(
                        el.attr("type").map(|t| t.to_ascii_lowercase()).as_deref(),
                        Some("checkbox" | "radio" | "submit" | "button" | "reset" | "image" | "file" | "color" | "range" | "hidden")
                    ),
                    _ => false,
                })
            });
            if over_text || editable { "text" } else { "default" }.to_string()
        }
    }
}

thread_local! {
    /// The folded `SpecifiedStyle` of each box from the last `apply_stylesheets`, for
    /// [`computed_report`] (the facts the box tree no longer distinguishes: `display: flex` versus
    /// Aether's block-as-column-flex approximation, `position`, `flex-direction`).
    static LAST_CASCADE: std::cell::RefCell<std::collections::HashMap<taffy::NodeId, std::rc::Rc<SpecifiedStyle>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The properties [`computed_report`] serializes, in css_core's oracle order (CSSCORE SR47).
pub const REPORT_PROPS: [&str; 20] = [
    "display", "position", "color", "background-color", "font-size", "font-weight", "font-style", "font-family", "line-height",
    "text-align", "text-decoration-line", "white-space", "visibility", "list-style-type", "border-top-width", "border-top-style",
    "border-left-color", "padding-top", "margin-top", "flex-direction",
];

/// AETHERSTYLE (SR54) M5: AETHER's computed values — what its layout and paint actually consume after
/// `apply_stylesheets` — serialized the way `getComputedStyle` serializes them, for every element of the
/// tree's document (keyed by DOM node). Inherited properties follow the renderer's own inheritance
/// (`render::draw_node`: the per-tag UA defaults of `layout::default_*`, the link colour, underline
/// propagation); box properties come from the taffy style; the values Aether does not model are
/// reported as what it effectively uses (sans/serif/monospace classes, solid borders, no list markers,
/// static for every non-absolute position). An element without a box reports `display: none`.
pub fn computed_report(tree: &LayoutTree) -> std::collections::HashMap<html_core::NodeId, [String; 20]> {
    use std::collections::HashMap;
    #[derive(Clone)]
    struct Inh {
        color: (u8, u8, u8),
        font_size: f32,
        weight: u16,
        italic: bool,
        family: u16,
        line_height: f32,
        text_align: u8,
        underline: bool,
        nowrap: bool,
        hidden: bool,
    }
    fn num(v: f32) -> String {
        let r = (v as f64 * 10000.0).round() / 10000.0;
        if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
    }
    let px = |v: f32| format!("{}px", num(v));
    let rgb = |c: (u8, u8, u8)| format!("rgb({}, {}, {})", c.0, c.1, c.2);
    let lp = |v: LengthPercentage| {
        use taffy::style::ExpandedLengthPercentage as E;
        match v.expand() {
            E::Percent(p) => format!("{}%", num(p * 100.0)),
            E::Length(l) => px(l),
            #[allow(unreachable_patterns)]
            _ => "calc".to_string(),
        }
    };
    let lpa_s = |v: LengthPercentageAuto| {
        use taffy::style::ExpandedLengthPercentageAuto as E;
        match v.expand() {
            E::Auto => "auto".to_string(),
            E::Percent(p) => format!("{}%", num(p * 100.0)),
            E::Length(l) => px(l),
            #[allow(unreachable_patterns)]
            _ => "calc".to_string(),
        }
    };
    let mut out = HashMap::new();
    let Some(any) = tree.node_map.values().find(|n| n.is_element()) else { return out };
    let mut box_of: HashMap<html_core::NodeId, taffy::NodeId> = HashMap::new();
    for (tid, n) in &tree.node_map {
        if n.is_element() {
            box_of.insert(n.id(), *tid);
        }
    }
    let last = LAST_CASCADE.with(|l| l.borrow().clone());
    any.with_doc(|doc, _| {
        let root = Inh {
            color: (0, 0, 0), font_size: 16.0, weight: 400, italic: false, family: crate::fonts::STANDARD, line_height: 0.0,
            text_align: 0, underline: false, nowrap: false, hidden: false,
        };
        let mut inh: HashMap<html_core::NodeId, Inh> = HashMap::new();
        for id in doc.descendants(html_core::Document::ROOT) {
            let Some(el) = doc.element(id) else { continue };
            let tag = el.local.as_str();
            let parent = doc.parent_element(id).and_then(|p| inh.get(&p)).cloned().unwrap_or_else(|| root.clone());
            let tid = box_of.get(&id).copied();
            let paint = tid.and_then(|t| tree.paint_map.get(&t)).cloned().unwrap_or_default();
            let spec = tid.and_then(|t| last.get(&t)).cloned();
            let mut me = parent.clone();
            me.font_size = paint.font_size.unwrap_or_else(|| crate::layout::default_font_size(tag, parent.font_size));
            me.weight = paint
                .weight
                .map(|w| w.resolve(parent.weight))
                .unwrap_or_else(|| crate::layout::default_weight(tag, parent.weight));
            me.italic = paint.italic.unwrap_or_else(|| crate::layout::default_italic(tag, parent.italic));
            me.family = paint.family.unwrap_or_else(|| crate::layout::default_family(tag, parent.family));
            if let Some(lh) = paint.line_height {
                me.line_height = lh;
            }
            if let Some(a) = paint.text_align {
                me.text_align = a;
            }
            if tag == "a" || tag == "u" {
                me.underline = true;
            }
            if let Some(u) = paint.underline {
                me.underline = u;
            }
            if let Some(nw) = paint.nowrap {
                me.nowrap = nw;
            }
            if let Some(h) = paint.hidden {
                me.hidden = h;
            }
            me.color = paint.color.unwrap_or(if tag == "a" { (0, 0, 238) } else { parent.color });
            let st = tid.and_then(|t| tree.taffy.style(t).ok()).cloned();
            let display = match (&st, &spec) {
                (None, _) => "none",
                (Some(s), _) if s.display == Display::None => "none",
                (_, Some(sp)) if sp.flex_container == Some(true) => "flex",
                _ if crate::layout::is_inline(tag) => "inline",
                _ => "block",
            };
            // AETHERJS: the five position kinds Aether paints (relative/sticky offsets, fixed to the
            // viewport) — the report used to fold everything but absolute into static.
            let position = match paint.position_kind {
                Some(1) => "relative",
                Some(2) => "absolute",
                Some(3) => "fixed",
                Some(4) => "sticky",
                _ => match spec.as_ref().and_then(|s| s.position) {
                    Some(taffy::style::Position::Absolute) => "absolute",
                    _ => "static",
                },
            };
            let flex_direction = match spec.as_ref().and_then(|s| s.flex_direction) {
                Some(FlexDirection::Column) => "column",
                Some(FlexDirection::ColumnReverse) => "column-reverse",
                Some(FlexDirection::RowReverse) => "row-reverse",
                _ => "row",
            };
            let side = |i: usize| paint.border.and_then(|b| b[i]);
            let bw_top = paint.border_width.and_then(|w| w[0]).or(side(0).map(|s| s.0)).unwrap_or(0.0);
            let lh = if me.line_height <= 0.0 { "normal".to_string() } else { px(me.line_height * me.font_size) };
            let values = [
                display.to_string(),
                position.to_string(),
                rgb(me.color),
                paint.background.map(rgb).unwrap_or_else(|| "rgba(0, 0, 0, 0)".to_string()),
                px(me.font_size),
                me.weight.to_string(),
                if me.italic { "italic" } else { "normal" }.to_string(),
                crate::fonts::serialize_family_list(&crate::fonts::family_list(me.family)),
                lh,
                match me.text_align { 1 => "center", 2 => "right", 3 => "end", 4 => "left", _ => "start" }.to_string(),
                if me.underline { "underline" } else { "none" }.to_string(),
                if me.nowrap { "nowrap" } else { "normal" }.to_string(),
                if me.hidden { "hidden" } else { "visible" }.to_string(),
                "disc".to_string(),
                px(if side(0).is_some() || bw_top > 0.0 { bw_top } else { 0.0 }),
                if side(0).is_some() && bw_top > 0.0 { "solid" } else { "none" }.to_string(),
                rgb(side(3).map(|s| s.1).unwrap_or(me.color)),
                st.as_ref().map(|s| lp(s.padding.top)).unwrap_or_else(|| px(0.0)),
                st.as_ref().map(|s| lpa_s(s.margin.top)).unwrap_or_else(|| px(0.0)),
                flex_direction.to_string(),
            ];
            out.insert(id, values);
            inh.insert(id, me);
        }
    });
    out
}

/// Properties this engine genuinely implements (the honest support set
/// for @supports; extend when apply_declaration grows an arm).
fn property_supported(prop: &str) -> bool {
    matches!(
        prop,
        "display" | "flex-direction" | "width" | "height" | "padding" | "margin"
            | "padding-top" | "padding-right" | "padding-bottom" | "padding-left"
            | "margin-top" | "margin-right" | "margin-bottom" | "margin-left"
            | "position" | "top" | "left" | "right" | "bottom" | "text-align" | "justify-content"
            | "font" | "align-items" | "align-content"
            | "background-color" | "background" | "background-image" | "color"
            | "background-size" | "background-position" | "background-repeat"
            | "object-fit" | "object-position"
            // Alpha-stencil masks really are implemented (render::draw_node),
            // so the component idiom `@supports (mask-image: none)` must take
            // the mask branch, not the background-image fallback branch.
            | "mask" | "mask-image" | "mask-size" | "mask-position" | "mask-repeat"
            | "-webkit-mask" | "-webkit-mask-image" | "-webkit-mask-size"
            | "-webkit-mask-position" | "-webkit-mask-repeat"
            | "font-size" | "font-weight" | "line-height" | "visibility"
            | "max-width" | "max-height" | "min-width" | "min-height"
            | "overflow" | "overflow-x" | "overflow-y"
            | "border" | "outline" | "border-color" | "border-width" | "border-style"
            | "border-top" | "border-right" | "border-bottom" | "border-left"
            | "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width"
            | "text-decoration" | "text-decoration-line" | "text-transform" | "white-space" | "box-sizing"
            | "font-family" | "font-style" | "font-stretch" | "letter-spacing" | "word-spacing" | "direction"
            | "text-shadow"
            | "flex" | "flex-grow" | "flex-shrink" | "flex-basis" | "flex-wrap" | "flex-flow"
            | "gap" | "row-gap" | "column-gap"
            | "cursor"
            | "content" | "counter-reset" | "counter-increment" | "counter-set" | "quotes"
            // AETHERSTYLE2 (SR95)
            | "inset" | "aspect-ratio" | "transform" | "transform-origin" | "translate" | "rotate" | "scale"
            | "clip-path" | "inline-size" | "block-size" | "min-inline-size" | "min-block-size"
            | "max-inline-size" | "max-block-size" | "margin-inline" | "margin-block" | "margin-inline-start"
            | "margin-inline-end" | "margin-block-start" | "margin-block-end" | "padding-inline"
            | "padding-block" | "padding-inline-start" | "padding-inline-end" | "padding-block-start"
            | "padding-block-end" | "inset-inline" | "inset-block" | "inset-inline-start" | "inset-inline-end"
            | "inset-block-start" | "inset-block-end"
            // AETHERSTYLE3 (SR103)
            | "filter" | "backdrop-filter" | "-webkit-filter" | "-webkit-backdrop-filter"
            | "overflow-inline" | "overflow-block"
            | "columns" | "column-count" | "column-width" | "column-fill" | "column-rule"
            | "column-rule-width" | "column-rule-style" | "column-rule-color"
            | "writing-mode" | "text-orientation"
            | "container" | "container-type" | "container-name" | "contain"
            | "transition" | "transition-property" | "transition-duration" | "transition-timing-function"
            | "transition-delay" | "transition-behavior" | "animation" | "animation-name" | "animation-duration"
            | "animation-timing-function" | "animation-delay" | "animation-iteration-count"
            | "animation-direction" | "animation-fill-mode" | "animation-play-state"
            // AETHERSTYLE3C (SR122)
            | "column-span" | "box-decoration-break" | "-webkit-box-decoration-break"
            | "text-combine-upright" | "-webkit-text-combine"
    )
}


thread_local! {
    /// Viewport the cascade resolves `vh`/`vw` against. Set by the layout
    /// builder and the cascade entry point; both know the real size.
    static VIEWPORT: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((800.0, 600.0)) };
}

/// Records the viewport that viewport-relative units resolve against.
pub fn set_viewport(w: f32, h: f32) {
    VIEWPORT.with(|v| v.set((w.max(1.0), h.max(1.0))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_size_and_weight_parsing() {
        assert_eq!(parse_font_size("14px"), Some(14.0));
        assert_eq!(parse_font_size("1.5em"), Some(24.0));
        assert_eq!(parse_font_size("2rem"), Some(32.0));
        assert_eq!(parse_font_size("110%"), Some(17.6));
        assert_eq!(parse_font_size("medium"), Some(16.0));
        assert_eq!(parse_font_size("banana"), None);
        use crate::layout::FontWeight;
        assert_eq!(parse_font_weight("bold"), Some(FontWeight::Abs(700)));
        assert_eq!(parse_font_weight("400"), Some(FontWeight::Abs(400)));
        assert_eq!(parse_font_weight("650"), Some(FontWeight::Abs(650)));
        assert_eq!(parse_font_weight("bolder"), Some(FontWeight::Bolder));
        assert_eq!(FontWeight::Bolder.resolve(400), 700);
        assert_eq!(FontWeight::Bolder.resolve(700), 900);
        assert_eq!(FontWeight::Lighter.resolve(700), 400);
        assert_eq!(FontWeight::Lighter.resolve(400), 100);
        assert_eq!(parse_font_stretch("condensed"), Some(75));
        assert_eq!(parse_font_stretch("120%"), Some(120));
        assert!(is_neutral_keyword("transparent"));
        assert!(is_neutral_keyword("Inherit"));
        assert!(!is_neutral_keyword("red"));
    }

    #[test]
    fn test_color_functions() {
        assert_eq!(parse_color_str("rgb(10, 20, 30)"), Some((10, 20, 30)));
        assert_eq!(parse_color_str("rgba(10, 20, 30, 0.5)"), Some((10, 20, 30)));
        assert_eq!(parse_color_str("rgba(10, 20, 30, 0)"), None);
        assert_eq!(parse_color_str("#abc"), Some((170, 187, 204)));
        assert_eq!(parse_color_str("#336699"), Some((51, 102, 153)));
    }

    /// Colour of the element with `id` after applying `css` to `html`.
    fn color_of(html: &str, css: &str, id: &str) -> Option<(u8, u8, u8)> {
        let mut tree = crate::layout::compute_layout(&crate::dom::parse_html(html));
        apply_css(&mut tree, css);
        for (node_id, dom_node) in &tree.node_map {
            let Some(el) = dom_node.as_element() else { continue };
            if el.attributes.borrow().get("id") == Some(id) {
                return tree.paint_map.get(node_id).and_then(|p| p.color);
            }
        }
        None
    }

    const LOWER_HTML: &str = r#"<html><body>
        <div id="a" class="x"><span id="s">s</span></div>
        <p id="b" class="y">y</p>
        <input id="i" required>
    </body></html>"#;

    /// Escaped punctuation is part of an identifier (CSS Syntax 3 §4.3.7): the
    /// whole `md:`/`lg:` utility layer of a utility-CSS page keys and matches
    /// on the class string the DOM actually carries.
    #[test]
    fn test_escaped_utility_classes() {
        let html = r#"<html><body><div id="a" class="md:block w-1/2 h-[5.75rem]">x</div></body></html>"#;
        for sel in [r".md\:block", r".w-1\/2", r".h-\[5\.75rem\]", r"body .md\:block", r".md\3a block"] {
            let css = format!("{sel} {{ color: rgb(1, 2, 3); }}");
            assert_eq!(color_of(html, &css, "a"), Some((1, 2, 3)), "{sel}");
        }
    }

    /// Selectors 4 through css_core (AETHERSTYLE): `:is()`/`:where()`, `:not()`
    /// with a list, `:has()`, form states. (The legacy `:-webkit-any()` that
    /// the old lowering rewrote is not a css_core pseudo-class: owed.)
    #[test]
    fn test_selectors4_match() {
        for sel in ["body :is(.x, .y)", "body :where(.x, .y)"] {
            let css = format!("{sel} {{ color: rgb(1, 2, 3); }}");
            assert_eq!(color_of(LOWER_HTML, &css, "a"), Some((1, 2, 3)), "{sel}");
            assert_eq!(color_of(LOWER_HTML, &css, "b"), Some((1, 2, 3)), "{sel}");
        }
        assert_eq!(color_of(LOWER_HTML, "p:is(.y, .z) { color: rgb(4, 5, 6); }", "b"), Some((4, 5, 6)));
        assert_eq!(color_of(LOWER_HTML, "body :is(.x > span) { color: rgb(7, 8, 9); }", "s"), Some((7, 8, 9)));
        assert_eq!(color_of(LOWER_HTML, "div.x:is(body > div) { color: rgb(7, 8, 9); }", "a"), Some((7, 8, 9)));
        assert_eq!(color_of(LOWER_HTML, ".x:not(.q, .r) { color: rgb(1, 2, 3); }", "a"), Some((1, 2, 3)));
        assert_eq!(color_of(LOWER_HTML, ".x:not(.q, .x) { color: rgb(1, 2, 3); }", "a"), None);
        assert_eq!(color_of(LOWER_HTML, ".x:not(:focus-visible) { color: rgb(4, 5, 6); }", "a"), Some((4, 5, 6)));
        assert_eq!(color_of(LOWER_HTML, ".x:has(> span) { color: rgb(4, 5, 6); }", "a"), Some((4, 5, 6)));
        assert_eq!(color_of(LOWER_HTML, ".y:has(> span) { color: rgb(4, 5, 6); }", "b"), None);
        assert_eq!(color_of(LOWER_HTML, "input:required { color: rgb(1, 2, 3); }", "i"), Some((1, 2, 3)));
        assert_eq!(color_of(LOWER_HTML, "input:optional { color: rgb(1, 2, 3); }", "i"), None);
        assert_eq!(color_of(LOWER_HTML, "input:placeholder-shown { color: rgb(1, 2, 3); }", "i"), None);
        assert_eq!(color_of(LOWER_HTML, ":lang(en) { color: rgb(1, 2, 3); }", "a"), None);
        assert_eq!(color_of(LOWER_HTML, ".x:focus-within { color: rgb(1, 2, 3); }", "a"), None);
    }

    /// Specificity is the spec's (Selectors 4 §17): `:where()` weighs nothing,
    /// `:is()`/`:not()`/`:has()` weigh their most specific argument.
    #[test]
    fn test_selectors4_specificity() {
        let css = "#a:where(.x) { color: rgb(1, 1, 1); } .x.x { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "a"), Some((1, 1, 1)));
        let css = ".y:is(.y) { color: rgb(1, 1, 1); } .y.y { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((2, 2, 2)));
        // `:is(#b, p)` weighs (1,0,0) even when it matches through `p`
        let css = "p:is(#zz, p) { color: rgb(1, 1, 1); } p.y.y { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((1, 1, 1)));
        let css = ":where(#b) { color: rgb(1, 1, 1); } p { color: rgb(2, 2, 2); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((2, 2, 2)));
    }

    /// A selector list with an INVALID member is an invalid selector, and the
    /// whole style rule is dropped (Selectors 4 §4.1, what Chromium does); a
    /// member that is valid but does not match the element (`::before`,
    /// `:focus-visible`) leaves its siblings alone. AETHERGEN (SR71): `.x::before`
    /// styles a generated box — the first child of `#a`'s box, carrying the rule's
    /// colour and the `content` text — and is no longer ledgered; a pseudo-element
    /// Aether generates no box for is ledgered by name.
    #[test]
    fn test_selector_list_validity() {
        crate::ledger::reset();
        let css = ".x::before, .y:focus-visible, .y { color: rgb(1, 2, 3); content: \"\\2713\"; } p::selection { color: red; }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), Some((1, 2, 3)));
        assert_eq!(color_of(LOWER_HTML, css, "a"), None);
        let mut tree = crate::layout::compute_layout(&crate::dom::parse_html(LOWER_HTML));
        apply_css(&mut tree, css);
        let a = tree.node_map.iter().find(|(_, n)| n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some("a"))).map(|(t, _)| *t).unwrap();
        let first = tree.taffy.children(a).unwrap()[0];
        let gbox = tree.node_map.get(&first).expect("the ::before box is a node of the tree");
        assert!(gbox.parent().is_none(), "a generated box is not in the DOM");
        assert_eq!(gbox.text_contents(), "\u{2713}");
        assert_eq!(tree.paint_map.get(&first).and_then(|p| p.color), Some((1, 2, 3)));
        let dump = format!("{:?}", crate::ledger::snapshot());
        assert!(!dump.contains("pseudo-element-rule:before"), "{dump}");
        assert!(dump.contains("pseudo-element-rule:selection"), "{dump}");
        let css = ".x:-moz-ui-invalid, .y { color: rgb(1, 2, 3); }";
        assert_eq!(color_of(LOWER_HTML, css, "b"), None, "an unknown pseudo-class invalidates the rule");
        let css = ".x:is(:-moz-ui-invalid, .x), .y { color: rgb(1, 2, 3); }";
        assert_eq!(color_of(LOWER_HTML, css, "a"), Some((1, 2, 3)), ":is() is forgiving");
    }

    /// Custom properties cascade and inherit per element (css-variables 1):
    /// element-scoped overrides, fallbacks, and cycles (guaranteed-invalid).
    #[test]
    fn test_custom_properties_per_element() {
        let html = r#"<html><body><div id="o" class="t"><p id="i">x</p></div><p id="p">y</p>
            <p id="c" class="cyc">z</p></body></html>"#;
        let css = r#"
            :root { --fg: rgb(1, 2, 3); }
            .t { --fg: rgb(9, 8, 7); }
            p { color: var(--fg); }
            .cyc { --a: var(--b); --b: var(--a); color: var(--a, rgb(5, 5, 5)); }
            #o { color: var(--missing, rgb(4, 4, 4)); }
        "#;
        assert_eq!(color_of(html, css, "p"), Some((1, 2, 3)));
        assert_eq!(color_of(html, css, "i"), Some((9, 8, 7)), "inherited from the scoped override");
        assert_eq!(color_of(html, css, "c"), Some((5, 5, 5)), "a cycle falls back");
        assert_eq!(color_of(html, css, "o"), Some((4, 4, 4)));
    }

    /// `@media` is Media Queries 4 (css_core) against Aether's viewport.
    #[test]
    fn test_media_queries() {
        let html = r#"<html><body><p id="p">y</p></body></html>"#;
        let at = |q: &str, w: f32| {
            let mut tree = crate::layout::compute_layout_sized(&crate::dom::parse_html(html), w, 600.0);
            apply_css(&mut tree, &format!("@media {q} {{ p {{ color: rgb(1, 2, 3); }} }}"));
            tree.node_map.iter().any(|(id, n)| {
                n.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some("p"))
                    && tree.paint_map.get(id).and_then(|p| p.color) == Some((1, 2, 3))
            })
        };
        assert!(at("screen", 800.0));
        assert!(!at("print", 800.0));
        assert!(at("screen and (min-width: 600px)", 800.0));
        assert!(!at("screen and (min-width: 1200px)", 800.0));
        assert!(at("screen and (min-width: 1200px)", 1280.0));
        assert!(at("(max-width: 900px)", 800.0));
        assert!(at("print, screen", 800.0));
        assert!(!at("(prefers-reduced-motion: reduce)", 800.0));
        assert!(at("(prefers-color-scheme: light)", 800.0));
        assert!(!at("(prefers-color-scheme: dark)", 800.0));
        assert!(at("(hover: hover)", 800.0));
        assert!(at("(orientation: landscape)", 800.0));
        assert!(!at("not all", 800.0));
        assert!(at("not print", 800.0));
        assert!(at("(400px <= width < 900px)", 800.0));
        assert!(!at("(400px <= width < 900px)", 1000.0));
        assert!(at("(width >= 40em)", 800.0));
    }

    #[test]
    fn test_viewport_units() {
        set_viewport(1000.0, 500.0);
        assert_eq!(parse_px("100vh"), Some(500.0));
        assert_eq!(parse_px("50vw"), Some(500.0));
        assert_eq!(parse_px("100dvh"), Some(500.0));
        assert_eq!(parse_px("100svh"), Some(500.0));
        assert_eq!(parse_px("100vmin"), Some(500.0));
        assert_eq!(parse_px("100vmax"), Some(1000.0));
        assert_eq!(eval_length("calc(100vh - 64px)", None), Some(436.0));
        // Units that merely contain "vw"/"vh" letters must not be misread.
        assert_eq!(parse_px("2rem"), Some(32.0));
        assert_eq!(parse_px("10px"), Some(10.0));
        set_viewport(800.0, 600.0);
    }


}
