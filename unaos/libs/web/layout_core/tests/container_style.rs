//! AETHERSTYLE3B (SR115): what SR103's container queries refused. `@container style(...)` rules evaluate
//! against the nearest ancestor's computed custom properties (every element is a style container, so no size
//! container and no first layout are needed): `--theme: dark` on the ancestor applies the dark rule and the
//! `not style(--theme: light)` rule, skips the light one, `style(--theme)` tests presence, a named style query
//! finds its named ancestor, and a missing property is false. Container-relative units resolve against the
//! query containers the layout sized: 50cqw of a 300px inline-size container is 150px, 10cqh with no height
//! container falls back to the viewport (60px of 600), 10cqmin of a 200 x 100 size container is 10px.
mod common;
use common::*;
use css_core::container::ContainerRule;
use css_core::media::Tri;
use layout_core::DomNode;

const SHEET: &str = "
.s { --theme: dark }
.named { container-name: panel; --mode:  compact }
@container style(--theme: dark) { .t { width: 30px } }
@container style(--theme: light) { .t { height: 40px } }
@container style(--theme) { .t { min-height: 12px } }
@container not style(--theme: light) { .x { width: 50px } }
@container style(--missing) { .x { height: 13px } }
@container panel style(--mode: compact) { .y { width: 70px } }
";

#[test]
fn style_queries_and_container_units() {
    let doc = page(vec![
        el("div", &[("class", "s")], vec![el("div", &[("id", "t"), ("class", "t")], vec![]), el("div", &[("id", "x"), ("class", "x")], vec![])]),
        el("div", &[("class", "named")], vec![el("div", &[("class", "s")], vec![el("div", &[("id", "y"), ("class", "y")], vec![])])]),
        el("div", &[("style", "container-type: inline-size; width: 300px")], vec![el("div", &[("id", "u"), ("style", "width: 50cqw; height: 10cqh")], vec![])]),
        el("div", &[("style", "container-type: size; width: 200px; height: 100px")], vec![el("div", &[("id", "m"), ("style", "width: 10cqmin; height: 5px")], vec![])]),
    ]);
    let sheet = css_core::stylesheet::parse_stylesheet(SHEET);
    let env = css_core::media::Environment::viewport(800.0, 600.0);
    let supports = |_: &css_core::parser::Declaration| true;
    let import = |_: &str| -> Option<&css_core::stylesheet::Stylesheet> { None };
    let cond = css_core::cascade::Conditions { env: &env, supports: &supports, import: &import };
    let mut rules = css_core::cascade::RuleSet::new();
    rules.add_sheet(&sheet, css_core::cascade::Origin::Author, &cond);

    // an element's own custom property `name` (the fixture sets each on one element)
    let own_custom = |e: &N, name: &str| -> Option<String> {
        let applied = css_core::cascade::cascade(&rules, e, &[], None);
        applied
            .iter()
            .rev()
            .find(|a| a.declaration.name == name)
            .map(|a| css_core::serialize::to_css(css_core::parser::trim_ws(&a.declaration.value)))
    };
    let names_of = |e: &N| -> Vec<String> {
        let applied = css_core::cascade::cascade(&rules, e, &[], None);
        applied
            .iter()
            .filter(|a| a.declaration.name == "container-name")
            .map(|a| css_core::serialize::to_css(css_core::parser::trim_ws(&a.declaration.value)))
            .collect()
    };
    // the nearest ancestor whose name matches answers; custom properties inherit down from it
    let query = |e: &N, c: &ContainerRule| -> bool {
        assert!(!c.needs_size());
        let mut cur = DomNode::parent(e);
        while let Some(a) = cur {
            if a.kind() == layout_core::NodeKind::Element && c.names(&names_of(&a)) {
                let lookup = |n: &str| {
                    let mut k = Some(a.clone());
                    while let Some(x) = k {
                        if let Some(v) = own_custom(&x, n) {
                            return Some(v);
                        }
                        k = DomNode::parent(&x).filter(|p| p.kind() == layout_core::NodeKind::Element);
                    }
                    None
                };
                return c.evaluate_with(None, &lookup) == Tri::True;
            }
            cur = DomNode::parent(&a);
        }
        false
    };
    let styled = cascade_sheet(&doc, &rules, Some(&query), 800.0, 600.0);
    let tree = lay_styled(&doc, &styled, 800.0, 600.0);
    let (_, _, tw, th) = box_of(&tree, &doc.by_id("t"));
    assert_eq!(tw, 30.0, "style(--theme: dark) applies");
    assert_eq!(th, 12.0, "style(--theme: light) does not; style(--theme) does (min-height)");
    let (_, _, xw, xh) = box_of(&tree, &doc.by_id("x"));
    assert_eq!((xw, xh), (50.0, 0.0), "not style(--theme: light) applies, style(--missing) does not");
    assert_eq!(box_of(&tree, &doc.by_id("y")).2, 70.0, "the named query finds the named ancestor (whitespace squashed)");

    // container-relative units
    let (_, _, uw, uh) = box_of(&tree, &doc.by_id("u"));
    assert_eq!((uw, uh), (150.0, 60.0));
    assert_eq!(box_of(&tree, &doc.by_id("m")).2, 10.0);
    assert!(layout_core::css::is_container_relative("calc(50cqw + 2px)") && !layout_core::css::is_container_relative("50vw"));
}
