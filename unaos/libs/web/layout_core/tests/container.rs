//! AETHERSTYLE3 (SR103): container queries. The fixture is cascaded through css_core with its `@container`
//! rules, laid out once to size the query containers, then cascaded again with each element's nearest
//! matching ancestor container answering the rules (the host's two rounds): the 300px `inline-size`
//! container's child takes the `width < 400px` rule and not the `min-width: 400px` one, a named rule finds
//! its named container past an unnamed one, a height query against an `inline-size` container is unknown
//! (false), and `container-type: size` lays an auto-height box out as if empty.
mod common;
use common::*;
use css_core::container::{ContainerRule, ContainerSize};
use layout_core::DomNode;

const SHEET: &str = "
.c { container-type: inline-size; width: 300px }
.card { container: card / size; width: 150px; height: 50px }
@container (min-width: 400px) { .t { width: 10px } }
@container (width < 400px) { .t { width: 20px } }
@container card (min-width: 100px) and (height <= 50px) { .u { height: 33px } }
@container (height > 0px) { .h { height: 44px } }
";

#[test]
fn container_rules_against_laid_out_containers() {
    let doc = page(vec![
        el("div", &[("id", "c"), ("class", "c")], vec![el("div", &[("id", "t"), ("class", "t h")], vec![])]),
        el("div", &[("class", "card")], vec![el("div", &[("class", "c")], vec![el("div", &[("id", "u"), ("class", "u")], vec![])])]),
        el("div", &[("id", "z"), ("style", "container-type: size")], vec![el("div", &[("style", "height: 40px")], vec![])]),
    ]);
    let sheet = css_core::stylesheet::parse_stylesheet(SHEET);
    let env = css_core::media::Environment::viewport(800.0, 600.0);
    let supports = |_: &css_core::parser::Declaration| true;
    let import = |_: &str| -> Option<&css_core::stylesheet::Stylesheet> { None };
    let cond = css_core::cascade::Conditions { env: &env, supports: &supports, import: &import };
    let mut rules = css_core::cascade::RuleSet::new();
    rules.add_sheet(&sheet, css_core::cascade::Origin::Author, &cond);
    assert!(rules.has_container_rules());

    // round 1: no container sized, no container rule applies
    let first = cascade_sheet(&doc, &rules, None, 800.0, 600.0);
    let tree = lay_styled(&doc, &first, 800.0, 600.0);
    assert_eq!(box_of(&tree, &doc.by_id("t")).2, 300.0);
    assert_eq!(box_of(&tree, &doc.by_id("z")).3, 0.0, "size containment: laid out as if empty");

    // the query containers as round 1 laid them out: (element, names, size)
    let mut sizes: Vec<(usize, Vec<String>, ContainerSize)> = Vec::new();
    for (tid, n) in &tree.node_map {
        let p = &tree.paint_map[tid];
        let ty = p.container_type.unwrap_or(0);
        if ty == 0 {
            continue;
        }
        let l = tree.taffy.layout(*tid).unwrap();
        let (w, h) = (l.size.width as f64, l.size.height as f64);
        let size = ContainerSize { width: Some(w), height: (ty == 1).then_some(h), vertical: false, font_size: 16.0 };
        sizes.push((n.id, p.container_name.clone().unwrap_or_default(), size));
    }
    let query = |e: &N, c: &ContainerRule| -> bool {
        let mut cur = DomNode::parent(e);
        while let Some(a) = cur {
            if let Some((_, names, size)) = sizes.iter().find(|(i, _, _)| *i == a.id) {
                if c.names(names) {
                    return c.evaluate(size) == css_core::media::Tri::True;
                }
            }
            cur = DomNode::parent(&a);
        }
        false
    };
    let second = cascade_sheet(&doc, &rules, Some(&query), 800.0, 600.0);
    let tree = lay_styled(&doc, &second, 800.0, 600.0);
    assert_eq!(box_of(&tree, &doc.by_id("t")).2, 20.0, "300px container: width < 400px");
    assert_ne!(box_of(&tree, &doc.by_id("t")).3, 44.0, "an inline-size container answers no height query");
    assert_eq!(box_of(&tree, &doc.by_id("u")).3, 33.0, "the named query skips the unnamed container");
}
