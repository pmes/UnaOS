//! M2 — the arena DOM's navigation and the querySelector-shaped traversal hooks a CSS core will drive.

use html_core::{parse_document, parse_fragment, Document, Namespace, NodeData, ParseOpts, QuirksMode};

#[test]
fn navigation_and_query_hooks() {
    let doc = parse_document(
        "<!DOCTYPE html><title>t</title><div id=a class='x  y'><p>one<p class=y>two</div><svg><circle r=\"1\"/></svg>",
        ParseOpts::default(),
    );
    assert_eq!(doc.quirks_mode, QuirksMode::NoQuirks);
    let html = doc.document_element().unwrap();
    assert_eq!(doc.element(html).unwrap().local, "html");
    let body = doc.element_children(html).nth(1).unwrap();
    assert!(doc.element(body).unwrap().is_html("body"));

    let div = doc.query_first(Document::ROOT, |d, n| d.element_id(n) == Some("a")).unwrap();
    assert!(doc.has_class(div, "x") && doc.has_class(div, "y") && !doc.has_class(div, "z"));
    let ys = doc.query_all(Document::ROOT, |d, n| d.has_class(n, "y"));
    assert_eq!(ys.len(), 2);
    let p2 = ys[1];
    let p1 = doc.prev_sibling_element(p2).unwrap();
    assert_eq!(doc.next_sibling_element(p1), Some(p2));
    assert_eq!(doc.parent_element(p1), Some(div));
    assert_eq!(doc.text_content(div), "onetwo");

    let circle = doc.query_first(Document::ROOT, |d, n| d.element(n).unwrap().is(Namespace::Svg, "circle")).unwrap();
    assert_eq!(doc.element(circle).unwrap().attr("r"), Some("1"));
    assert!(doc.first_child(circle).is_none(), "self-closing foreign element has no children");

    // descendants() is tree order
    let names: Vec<String> = doc
        .descendants(body)
        .filter_map(|n| doc.element(n).map(|e| e.local.clone()))
        .collect();
    assert_eq!(names, ["div", "p", "p", "svg", "circle"]);
}

#[test]
fn template_contents_live_in_a_fragment() {
    let doc = parse_document("<template><td>x</td></template>", ParseOpts::default());
    let t = doc.query_first(Document::ROOT, |d, n| d.element(n).unwrap().is_html("template")).unwrap();
    assert!(doc.first_child(t).is_none());
    let contents = doc.element(t).unwrap().template_contents.unwrap();
    assert!(matches!(doc.data(contents), NodeData::DocumentFragment));
    let td = doc.first_child(contents).unwrap();
    assert!(doc.element(td).unwrap().is_html("td"));
}

#[test]
fn fragment_parsing_in_context() {
    let (doc, frag) = parse_fragment(Namespace::Html, "tr", Vec::new(), "<td>a<td>b", QuirksMode::NoQuirks, ParseOpts::default());
    let cells: Vec<_> = doc.children(frag).collect();
    assert_eq!(cells.len(), 2);
    assert_eq!(doc.text_content(cells[1]), "b");
}
