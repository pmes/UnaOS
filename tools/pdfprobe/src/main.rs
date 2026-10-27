// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `pdfprobe` — the headless probe of `pdf_core` (PDF1, LEDGER SR300); `nle` is to `timeline_core` what this is to
//! `pdf_core`: argument plumbing and printing, no parsing logic.
//!
//! ```text
//! pdfprobe <file>
//! pdfprobe --render <file> <page> <out.png>
//! ```
//!
//! Prints `pages=<n> version=<v> xref=<table|stream|rebuilt> info=<title|->`, then one line per page:
//! `page=<i> media=<w>x<h> rotate=<r> ops=<n> fonts=<n> images=<n>` (fonts = the /Resources /Font entries; images =
//! the /Resources /XObject entries whose /Subtype is /Image, plus the inline images among the page's ops).
//! `--render` (PDF2, LEDGER SR313) draws page `<page>` (0-based) at 72 dpi through `Page::render`, writes it as a PNG, and
//! prints `page=<i> render=<w>x<h> runs=<text runs> refused=<names|->` (text runs: the strings the page shows, recorded,
//! not drawn — PDF3 draws glyphs).
//! A refused file prints `error=<reason>` and exits 1.

use pdf_core::content::ops;
use pdf_core::render::{TextLog, png};
use pdf_core::{Document, Object, Page};

fn usage() -> ! {
    eprintln!("usage: pdfprobe <file> | pdfprobe --render <file> <page> <out.png>");
    std::process::exit(2)
}

fn fail(why: impl std::fmt::Display) -> ! {
    println!("error={why}");
    std::process::exit(1)
}

/// A number as the shortest text: `612`, not `612.0`.
fn num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v}") }
}

/// One-line text: whitespace runs become one space.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn page_line(doc: &Document, i: usize, p: &Page) -> String {
    let o = ops(&p.contents);
    let dict_len = |key: &[u8]| match doc.get(p.resources.get(key).unwrap_or(&Object::Null)) {
        Object::Dict(d) => d,
        _ => Default::default(),
    };
    let fonts = dict_len(b"Font").len();
    let xobjects = dict_len(b"XObject");
    let images = xobjects.iter().filter(|(_, v)| doc.get(v).as_dict().and_then(|d| d.name(b"Subtype")) == Some(&b"Image"[..])).count()
        + o.iter().filter(|op| op.operator == b"BI").count();
    format!(
        "page={i} media={}x{} rotate={} ops={} fonts={fonts} images={images}",
        num(p.media_box[2] - p.media_box[0]),
        num(p.media_box[3] - p.media_box[1]),
        p.rotate,
        o.len()
    )
}

/// `--render <file> <page> <out.png>`.
fn render(file: &str, page: &str, out: &str) {
    let index: usize = page.parse().unwrap_or_else(|_| usage());
    let bytes = std::fs::read(file).unwrap_or_else(|e| fail(e));
    let doc = Document::parse(&bytes).unwrap_or_else(|e| fail(e));
    let pages = doc.pages().unwrap_or_else(|e| fail(e));
    let p = pages.get(index).unwrap_or_else(|| fail(format!("no page {index} (pages={})", pages.len())));
    let mut log = TextLog::default();
    let r = p.render(&doc, 72.0, &mut log).unwrap_or_else(|e| fail(e));
    let encoded = png(&r.pixmap).unwrap_or_else(|e| fail(e));
    std::fs::write(out, encoded).unwrap_or_else(|e| fail(e));
    let refused = if r.refused.is_empty() { "-".to_string() } else { r.refused.join(",") };
    println!("page={index} render={}x{} runs={} refused={refused}", r.pixmap.w, r.pixmap.h, log.runs.len());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 5 && args[1] == "--render" {
        render(&args[2], &args[3], &args[4]);
        return;
    }
    if args.len() != 2 {
        usage();
    }
    let bytes = std::fs::read(&args[1]).unwrap_or_else(|e| fail(e));
    let doc = Document::parse(&bytes).unwrap_or_else(|e| fail(e));
    let pages = doc.pages().unwrap_or_else(|e| fail(e));
    let title = doc.info().title.map(|t| one_line(&t)).filter(|t| !t.is_empty()).unwrap_or_else(|| "-".into());
    let version = if doc.version().is_empty() { "-" } else { doc.version() };
    println!("pages={} version={version} xref={} info={title}", pages.len(), doc.xref_kind().as_str());
    for (i, p) in pages.iter().enumerate() {
        println!("{}", page_line(&doc, i, p));
    }
}
