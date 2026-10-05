//! AETHERSTYLE (SR54) M5: css_core's 20-property Chromium oracle (CSSCORE SR47), re-run through
//! AETHER's own computed values instead of the css_core test harness's.
//!
//! For each of EYES's 18 cases, css_core's fixture (`unaos/libs/web/css_core/tests/oracle/*.json`, made
//! by its `tools/oracle.mjs` in Chromium 141) holds Chromium's own DOM after scripts ran, the page's
//! `<style>` texts, the viewport, and `getComputedStyle` of 20 properties for every element. This test
//! rebuilds that DOM in Aether (the element tree serialized back to HTML, a placeholder character where
//! Chromium had text, parsed by `aether::dom::parse_html`), lays it out (`layout::build_tree`), runs
//! Aether's cascade over the page's sheets (`css::apply_stylesheets`), and reads Aether's values with
//! `css::computed_report` — what its layout and paint consume. A cell agrees when the strings are equal
//! (px within 0.01). It prints per-page, per-property agreement and gates on the floor measured when
//! the arc landed; css_core's own harness agrees on 6980/6980, so every gap below is Aether's
//! computed-value model, not the cascade.
//!
//! `STYLE_ORACLE_VERBOSE=1` prints every disagreeing cell.

use serde_json::Value;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr",
];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// Chromium's element tree back to HTML (pre-order kept by the parser for these pages).
fn to_html(j: &Value, out: &mut String) {
    let name = j["n"].as_str().unwrap();
    out.push('<');
    out.push_str(name);
    for a in j["a"].as_array().into_iter().flatten() {
        let (ns, n, v) = (a[0].as_str().unwrap(), a[1].as_str().unwrap(), a[2].as_str().unwrap());
        out.push(' ');
        if ns == "http://www.w3.org/1999/xlink" {
            out.push_str("xlink:");
        } else if ns == "http://www.w3.org/XML/1998/namespace" {
            out.push_str("xml:");
        }
        out.push_str(n);
        out.push_str("=\"");
        out.push_str(&esc(v));
        out.push('"');
    }
    out.push('>');
    if VOID.contains(&name) {
        return;
    }
    if j["t"].as_bool() == Some(true)
        && !matches!(name, "style" | "script" | "html" | "head" | "table" | "thead" | "tbody" | "tfoot" | "tr" | "colgroup" | "select" | "svg")
    {
        out.push('x');
    }
    for c in j["c"].as_array().into_iter().flatten() {
        to_html(c, out);
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

fn preorder<'a>(j: &'a Value, out: &mut Vec<&'a Value>) {
    out.push(j);
    for c in j["c"].as_array().into_iter().flatten() {
        preorder(c, out);
    }
}

fn agree(chromium: &str, ours: &str) -> bool {
    if chromium == ours {
        return true;
    }
    match (chromium.strip_suffix("px"), ours.strip_suffix("px")) {
        (Some(a), Some(b)) => match (a.parse::<f64>(), b.parse::<f64>()) {
            (Ok(x), Ok(y)) => (x - y).abs() < 0.01,
            _ => false,
        },
        _ => false,
    }
}

#[test]
fn aether_computed_values_vs_chromium() {
    let dir = repo().join("unaos/libs/web/css_core/tests/oracle");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 18, "EYES's 18 cases");
    let verbose = std::env::var_os("STYLE_ORACLE_VERBOSE").is_some();
    let props = aether::css::REPORT_PROPS;
    let mut per_prop = [0usize; 20];
    let (mut ok_all, mut cells_all, mut els_all, mut els_ok_all) = (0, 0, 0, 0);
    println!("style-oracle page                       cells agreeing        elements fully agreeing");
    for f in &files {
        let fx: Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let case = fx["case"].as_str().unwrap().to_string();
        let fx_props: Vec<&str> = fx["props"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        assert_eq!(fx_props, props.to_vec(), "{case}: property list");
        let styles: Vec<String> = fx["styles"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        let (w, h) = (fx["width"].as_f64().unwrap() as f32, fx["height"].as_f64().unwrap() as f32);
        let mut html = String::from("<!DOCTYPE html>");
        to_html(&fx["dom"], &mut html);
        let mut want = Vec::new();
        preorder(&fx["dom"], &mut want);

        let doc = aether::dom::parse_html(&html);
        let els: Vec<aether::dom::NodeRef> = doc.descendants().filter(|n| n.is_element()).collect();
        assert_eq!(els.len(), want.len(), "{case}: rebuilt DOM element count");
        for (e, j) in els.iter().zip(&want) {
            assert_eq!(e.local_name().as_deref(), j["n"].as_str(), "{case}: rebuilt DOM order");
        }
        let mut tree = aether::layout::build_tree(&doc, w, h);
        aether::css::apply_stylesheets(&mut tree, &styles);
        let report = aether::css::computed_report(&tree);

        let (mut ok, mut cells, mut els_ok) = (0, 0, 0);
        for (e, j) in els.iter().zip(&want) {
            let ours = report.get(&e.id()).expect("every element reported");
            let theirs: Vec<&str> = j["s"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            let mut all = true;
            for p in 0..20 {
                cells += 1;
                if agree(theirs[p], &ours[p]) {
                    ok += 1;
                    per_prop[p] += 1;
                } else {
                    all = false;
                    if verbose {
                        println!("    {case}: <{}> {}: chromium {:?} aether {:?}", j["n"].as_str().unwrap(), props[p], theirs[p], ours[p]);
                    }
                }
            }
            els_ok += all as usize;
        }
        println!(
            "  {case:36} {ok:4}/{cells:4} ({:5.1}%)   {els_ok:3}/{:3}",
            100.0 * ok as f64 / cells as f64,
            els.len()
        );
        ok_all += ok;
        cells_all += cells;
        els_all += els.len();
        els_ok_all += els_ok;
    }
    println!(
        "  TOTAL                                {ok_all:4}/{cells_all:4} ({:5.1}%)   {els_ok_all:3}/{els_all:3}",
        100.0 * ok_all as f64 / cells_all as f64
    );
    println!("style-oracle per property (of {els_all} elements):");
    for p in 0..20 {
        println!("  {:22} {:4}/{els_all} ({:5.1}%)", props[p], per_prop[p], 100.0 * per_prop[p] as f64 / els_all as f64);
    }
    // Floor: the agreement measured when AETHERSTYLE landed (a regression gate; raise it as Aether's
    // computed-value model grows).
    let floor = std::env::var("STYLE_ORACLE_FLOOR").ok().and_then(|s| s.parse().ok()).unwrap_or(6627usize);
    assert!(ok_all >= floor, "Aether's computed values regressed: {ok_all}/{cells_all} < floor {floor}");
}
