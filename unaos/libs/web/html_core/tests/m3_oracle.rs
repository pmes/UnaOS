//! M3 — the serializer (§13.3) and the Chromium oracle.
//!
//! 1. Serializer known answers straight from §13.3 (escaping, void elements, raw-text parents, template contents,
//!    namespaced attributes, deep trees).
//! 2. The EYES corpus (21 pages = the 23 EYES cases): our `outerHTML` of the document element vs Chromium's
//!    (tests/data/eyes/*.chromium.html, produced by tests/oracle/outer_html.cjs with JavaScript disabled) —
//!    byte-equal after whitespace normalization is the gate; raw byte equality is reported too.
//! 3. Optional, live: every html5lib tree-construction document input through Chromium's DOMParser (a
//!    scripting-disabled document) vs our parse+serialize. Runs only when node + Playwright + Chromium are
//!    present (skips otherwise); reports agreement and lists every disagreement.

mod common;

use common::{cache_dir, crate_dir, fetch, parse_json, vectors};
use html_core::serialize::{inner_html, outer_html, SerializeOpts};
use html_core::{parse_document, Document, ParseOpts};
use std::process::Command;

fn ser(doc: &Document) -> String {
    outer_html(doc, doc.document_element().unwrap(), SerializeOpts::default())
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C') {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = false;
            out.push(c);
        }
    }
    out
}

#[test]
fn serializer_known_answers() {
    let cases: &[(&str, &str)] = &[
        // escaping in text and attribute mode (§13.3 "escaping a string"; < and > are escaped in attributes too)
        ("<p title='a&b\"c<d>e\u{a0}'>x&amp;y&lt;z&gt;\u{a0}\"</p>",
         "<html><head></head><body><p title=\"a&amp;b&quot;c&lt;d&gt;e&nbsp;\">x&amp;y&lt;z&gt;&nbsp;\"</p></body></html>"),
        // void elements and "serializes as void" legacy ones
        ("<br><img src=a><input><hr><wbr><keygen>",
         "<html><head></head><body><br><img src=\"a\"><input><hr><wbr><keygen></body></html>"),
        // raw-text parents are literal; noscript is escaped when scripting is off
        ("<style>a>b{}&amp;</style><script>if(a<b&&c)</script><xmp>&<</xmp>",
         "<html><head><style>a>b{}&amp;</style><script>if(a<b&&c)</script></head><body><xmp>&<</xmp></body></html>"),
        ("<body><noscript>&lt;b&gt;</noscript>",
         "<html><head></head><body><noscript>&lt;b&gt;</noscript></body></html>"),
        // template contents serialize as the template's children
        ("<template><tr><td>1</td></tr></template>",
         "<html><head><template><tr><td>1</td></tr></template></head><body></body></html>"),
        // namespaced attributes, SVG case fixing
        ("<svg viewbox='0 0 1 1' xlink:href=#a xml:lang=en xmlns:xlink=x><lineargradient/></svg>",
         "<html><head></head><body><svg viewBox=\"0 0 1 1\" xlink:href=\"#a\" xml:lang=\"en\" xmlns:xlink=\"x\"><linearGradient></linearGradient></svg></body></html>"),
        // comments and a leading LF in pre is not re-added
        ("<pre>\n\nx</pre><!--c-->", "<html><head></head><body><pre>\nx</pre><!--c--></body></html>"),
    ];
    for (input, want) in cases {
        let doc = parse_document(input, ParseOpts::default());
        assert_eq!(&ser(&doc), want, "input {input:?}");
    }
    // the doctype (Document-level serialization)
    let doc = parse_document("<!doctype html><p>", ParseOpts::default());
    assert_eq!(inner_html(&doc, Document::ROOT, SerializeOpts::default()), "<!DOCTYPE html><html><head></head><body><p></p></body></html>");
    // no recursion limit in the serializer: a 200k-deep tree (built directly; the parser's scope scans are
    // O(depth) per tag by the spec, so a 200k-deep *parse* is quadratic) serializes without overflowing the stack
    let mut doc = Document::new();
    let mut parent = Document::ROOT;
    for _ in 0..200_000 {
        let d = doc.create_element(html_core::Namespace::Html, "div", Vec::new());
        doc.append(parent, d);
        parent = d;
    }
    let s = inner_html(&doc, Document::ROOT, SerializeOpts::default());
    assert_eq!(s.matches("<div>").count(), 200_000);
    assert!(s.ends_with("</div></div>"));
    // and a 5k-deep parse round-trips
    let doc = parse_document(&"<div>".repeat(5_000), ParseOpts::default());
    assert_eq!(ser(&doc).matches("</div>").count(), 5_000);
}

#[test]
fn chromium_oracle_eyes_corpus() {
    let dir = crate_dir().join("tests/data/eyes");
    let mut pages: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            n.ends_with(".html") && !n.ends_with(".chromium.html")
        })
        .collect();
    pages.sort();
    assert_eq!(pages.len(), 21, "the 21 EYES pages");
    let (mut norm_eq, mut raw_eq) = (0, 0);
    let mut fails = Vec::new();
    for p in &pages {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let input = std::fs::read_to_string(p).unwrap();
        let want = std::fs::read_to_string(dir.join(format!("{name}.chromium.html"))).unwrap();
        let got = ser(&parse_document(&input, ParseOpts::default()));
        let raw = got == want;
        let norm = normalize(&got) == normalize(&want);
        println!(
            "oracle {name:24} {:5} bytes  normalized:{}  raw:{}",
            got.len(),
            if norm { "EQUAL" } else { "DIFF " },
            if raw { "EQUAL" } else { "DIFF" }
        );
        raw_eq += raw as usize;
        norm_eq += norm as usize;
        if !norm {
            fails.push(format!("{name}\n--- chromium\n{want}\n--- ours\n{got}"));
        }
    }
    println!("oracle TOTAL normalized-equal {norm_eq}/21, raw byte-equal {raw_eq}/21");
    for f in &fails {
        println!("{f}");
    }
    assert!(fails.is_empty());
}

fn json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[test]
fn chromium_domparser_on_html5lib_corpus() {
    let npm_root = Command::new("npm").args(["root", "-g"]).output().ok().and_then(|o| String::from_utf8(o.stdout).ok());
    let Some(npm_root) = npm_root.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        eprintln!("SKIP: no node/npm");
        return;
    };
    let browsers = std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or_else(|_| "/opt/pw-browsers".into());
    if !std::path::Path::new(&browsers).exists() {
        eprintln!("SKIP: no Playwright browsers at {browsers}");
        return;
    }
    let mut inputs = Vec::new();
    for (url, sha) in vectors("tree") {
        let Some(text) = fetch(&url, &sha) else { return };
        for (i, t) in common::parse_dat(&text).into_iter().enumerate() {
            if t.fragment.is_none() && t.script != Some(true) {
                inputs.push((format!("{}#{i}", common::file_name(&url)), t.data));
            }
        }
    }
    let inp = cache_dir().join("domparser-in.json");
    let outp = cache_dir().join("domparser-out.json");
    let json = format!("[{}]", inputs.iter().map(|(_, d)| json_string(d)).collect::<Vec<_>>().join(","));
    std::fs::write(&inp, json).unwrap();
    let st = Command::new("node")
        .arg(crate_dir().join("tests/oracle/outer_html.cjs"))
        .args(["--dat"])
        .arg(&inp)
        .arg(&outp)
        .env("NODE_PATH", &npm_root)
        .env("PLAYWRIGHT_BROWSERS_PATH", &browsers)
        .status();
    if !matches!(st, Ok(s) if s.success()) {
        eprintln!("SKIP: the Chromium oracle script did not run");
        return;
    }
    let outs = parse_json(&std::fs::read_to_string(&outp).unwrap());
    let outs = outs.arr();
    assert_eq!(outs.len(), inputs.len());
    let mut agree = 0;
    let mut diffs = Vec::new();
    for ((name, data), want) in inputs.iter().zip(outs) {
        let want = want.str().unwrap_or_default();
        let got = ser(&parse_document(data, ParseOpts::default()));
        if got == want {
            agree += 1;
        } else {
            diffs.push(format!("{name} data={data:?}\n  chromium {want:?}\n  ours     {got:?}"));
        }
    }
    println!("chromium-domparser html5lib documents: byte-equal {agree}/{}", inputs.len());
    for d in &diffs {
        println!("DIFF {d}");
    }
    // Where Chromium 141 departs from the spec, html5lib (and we) follow the spec; these are the only allowed
    // disagreements (each one passes its html5lib vector in m2_tree):
    //   noscript01.dat#12 — Chromium closes <noscript> at the ignored <head> start tag (comment lands in <head>);
    //   tests25.dat#7     — Chromium closes the obsolete <command> immediately (void-like), "A" becomes its sibling;
    //   webkit02.dat#15   — Chromium's adoption agency reopens <em> as well as <b> inside <aside>.
    const KNOWN_CHROMIUM_DEVIATIONS: &[&str] = &["noscript01.dat#12 ", "tests25.dat#7 ", "webkit02.dat#15 "];
    let unexpected: Vec<_> = diffs.iter().filter(|d| !KNOWN_CHROMIUM_DEVIATIONS.iter().any(|k| d.starts_with(k))).collect();
    assert!(unexpected.is_empty(), "{} unexpected disagreements with Chromium", unexpected.len());
}
