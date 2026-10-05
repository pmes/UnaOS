//! M2 — tree construction (§13.2.6) + the arena DOM against html5lib-tests `tree-construction/*.dat` (pin
//! 9329e64, fetched at test time, see vectors.txt). The parser runs with scripting DISABLED: tests marked
//! `#script-on` are skipped (counted), unmarked and `#script-off` tests run. Comparison is the html5lib dump
//! format, exact. Parse errors are not compared.

mod common;

use common::{fetch, file_name, vectors};
use html_core::serialize::html5lib_dump;
use html_core::{parse_document, parse_fragment, Document, Namespace, ParseOpts, QuirksMode};

pub struct DatTest {
    pub data: String,
    pub fragment: Option<String>,
    pub script: Option<bool>,
    pub document: String,
}

/// Split a .dat file into tests.
pub fn parse_dat(text: &str) -> Vec<DatTest> {
    let mut out = Vec::new();
    let body = text.strip_prefix("#data\n").unwrap_or(text);
    for block in body.split("\n#data\n") {
        let mut data = String::new();
        let mut fragment = None;
        let mut script = None;
        let mut document = String::new();
        let mut section = "data";
        for line in block.split_inclusive('\n') {
            let header = line.trim_end_matches('\n');
            let is_header = section != "document"
                && matches!(header, "#errors" | "#new-errors" | "#document-fragment" | "#script-off" | "#script-on" | "#document");
            if is_header {
                section = header.trim_start_matches('#');
                match section {
                    "script-off" => script = Some(false),
                    "script-on" => script = Some(true),
                    _ => {}
                }
                continue;
            }
            match section {
                "data" => data.push_str(line),
                "document-fragment" => fragment = Some(header.to_string()),
                "document" => document.push_str(line),
                _ => {}
            }
        }
        if data.ends_with('\n') {
            data.pop();
        }
        out.push(DatTest { data, fragment, script, document });
    }
    out
}

pub fn run_one(t: &DatTest) -> String {
    let opts = ParseOpts::default();
    match &t.fragment {
        None => {
            let doc = parse_document(&t.data, opts);
            html5lib_dump(&doc, Document::ROOT)
        }
        Some(ctx) => {
            let (ns, local) = match ctx.split_once(' ') {
                Some(("svg", l)) => (Namespace::Svg, l),
                Some(("math", l)) => (Namespace::MathMl, l),
                _ => (Namespace::Html, ctx.as_str()),
            };
            let (doc, frag) = parse_fragment(ns, local, Vec::new(), &t.data, QuirksMode::NoQuirks, opts);
            html5lib_dump(&doc, frag)
        }
    }
}

#[test]
fn html5lib_tree_construction() {
    let files = vectors("tree");
    let (mut pass, mut total, mut skipped) = (0, 0, 0);
    let mut all_fails = Vec::new();
    for (url, sha) in &files {
        let Some(text) = fetch(url, sha) else { return };
        let fname = file_name(url);
        let tests = parse_dat(&text);
        let (mut fp, mut ft, mut fs) = (0, 0, 0);
        for (i, t) in tests.iter().enumerate() {
            if t.script == Some(true) {
                fs += 1;
                continue;
            }
            ft += 1;
            let got = std::panic::catch_unwind(|| run_one(t)).unwrap_or_else(|_| String::from("<PANIC>"));
            let want = t.document.trim_end_matches('\n');
            if got.trim_end_matches('\n') == want {
                fp += 1;
            } else {
                all_fails.push(format!(
                    "{fname}#{i} data={:?} ctx={:?}\n--- want\n{want}\n--- got\n{}",
                    t.data,
                    t.fragment,
                    got.trim_end_matches('\n')
                ));
            }
        }
        println!("tree {fname:45} {fp:4}/{ft:<4}{}", if fs > 0 { format!(" ({fs} script-on skipped)") } else { String::new() });
        pass += fp;
        total += ft;
        skipped += fs;
    }
    println!("tree TOTAL {pass}/{total} ({skipped} script-on skipped)");
    for f in &all_fails {
        println!("FAIL {f}\n");
    }
    assert!(all_fails.is_empty(), "{} tree-construction failures", all_fails.len());
}
