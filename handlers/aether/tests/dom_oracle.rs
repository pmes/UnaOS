//! AETHERDOM (SR49) proof: the DOM Aether builds for each of the 22 EYES pages serializes to exactly
//! what `html_core` produces stand-alone, and therefore to what Chromium produces.
//!
//! - subject: `aether::dom::parse_html(page)` (Aether's own document model, scripting flag on), the
//!   document element's `outerHTML` through Aether's `NodeRef` API;
//! - stand-alone: `html_core::parse_document(page, ParseOpts::default())` + `serialize::outer_html`;
//! - oracle: `unaos/libs/web/html_core/tests/data/eyes/<page>.chromium.html`, Chromium's
//!   `document.documentElement.outerHTML` (produced by HTMLCORE's `tests/oracle/outer_html.cjs`).
//!
//! None of the 22 pages has a `<noscript>`, the only construct where the scripting flag changes a parse.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn eyes_corpus_dom_equals_html_core_and_chromium() {
    let corpus = repo().join("tools/eyes/suites/aether/corpus");
    let goldens = repo().join("unaos/libs/web/html_core/tests/data/eyes");
    let mut pages: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("EYES corpus")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    pages.sort();
    assert_eq!(pages.len(), 22, "the 22 EYES pages (17-21: AETHERINLINE's cases, 22: AETHERFONT's)");
    let mut fails = Vec::new();
    for p in &pages {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        assert!(!src.contains("<noscript"), "{name}: scripting flag would matter");

        let doc = aether::dom::parse_html(&src);
        let html = doc.select_first("html").expect("document element");
        let ours = html.as_node().to_string();
        assert_eq!(ours, html.as_node().outer_html());

        let alone = html_core::parse_document(&src, html_core::ParseOpts::default());
        let root = alone.document_element().unwrap();
        let standalone = html_core::serialize::outer_html(&alone, root, Default::default());

        let chromium = std::fs::read_to_string(goldens.join(format!("{name}.chromium.html"))).unwrap();
        let (a, c) = (ours == standalone, ours == chromium);
        println!(
            "aetherdom {name:22} {:5} bytes  vs html_core: {}  vs chromium: {}",
            ours.len(),
            if a { "EQUAL" } else { "DIFF" },
            if c { "EQUAL" } else { "DIFF" }
        );
        if !(a && c) {
            fails.push(name);
        }
    }
    assert!(fails.is_empty(), "serialized DOM differs on {fails:?}");
}
