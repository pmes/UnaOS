// CHARTER: Facet (Images) — host tool over the shared cores (tools/raster, LEDGER SR233)
// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `raster` — Facet's headless face (RASTERCLI, LEDGER SR233): `nle` is to `stria::nle` what this is to
//! raster_core / brush_core / adjust_core: argument plumbing and files, no editing logic.
//!
//! ```text
//! raster --do <script>      (script: a file, or `-` for stdin; one verb per line, `#` comments)
//! ```
//!
//! The script runs against ONE in-memory `raster_core::Document` and its `History`, then ONE JSON line is printed:
//! `{"ok","verbs","last","line","width","height","layers","undo","redo","selection","fnv","error"}` (`fnv` = FNV-1a 64
//! of the composite, nle's fnv). Exit 0 ok, 1 a verb failed (named in `last` / `error`), 2 a usage or script error
//! (nothing ran). Grammar, verb -> Command table and fields: `docs/dev/evidence/raster/RASTERCLI.md`.

mod ops;
mod script;

use std::io::Read;

use ops::State;
use raster_core::Selection;

/// JSON string body: `"` and `\` escaped, control characters as `\u00XX`.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// The one status line.
fn status(st: &State, ok: bool, verbs: usize, last: &str, line: usize, error: &str) -> String {
    let (w, h, layers, sel, fnv) = match &st.doc {
        Some(d) => {
            let sel = match &d.selection {
                None => "none",
                Some(Selection::Rect { .. }) => "rect",
                Some(Selection::Ellipse { .. }) => "ellipse",
                Some(Selection::Polygon(_)) => "poly",
                Some(Selection::Coverage(_)) => "coverage",
            };
            let fnv = d.composite().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
            (d.width, d.height, d.layers.len(), sel, fnv)
        }
        None => (0, 0, 0, "none", 0),
    };
    format!(
        "{{\"ok\":{ok},\"verbs\":{verbs},\"last\":\"{}\",\"line\":{line},\"width\":{w},\"height\":{h},\"layers\":{layers},\"undo\":{},\"redo\":{},\"selection\":\"{sel}\",\"fnv\":\"{fnv:016x}\",\"error\":\"{}\"}}",
        esc(last),
        st.hist.past.len(),
        st.hist.future.len(),
        esc(error)
    )
}

fn finish(st: &State, ok: bool, verbs: usize, last: &str, line: usize, error: &str, code: i32) -> ! {
    println!("{}", status(st, ok, verbs, last, line, error));
    std::process::exit(code)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let st = State::default();
    let [flag, path] = args.as_slice() else {
        eprintln!("usage: raster --do <script|->");
        finish(&st, false, 0, "", 0, "usage", 2)
    };
    if flag != "--do" {
        eprintln!("usage: raster --do <script|->");
        finish(&st, false, 0, "", 0, "usage", 2)
    }
    let mut text = String::new();
    let read = if path == "-" {
        std::io::stdin().read_to_string(&mut text).map(|_| ())
    } else {
        std::fs::read_to_string(path).map(|t| text = t)
    };
    if let Err(e) = read {
        eprintln!("raster: {path}: {e}");
        finish(&st, false, 0, "", 0, "usage", 2)
    }
    let steps = match script::parse(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("raster: line {}: not a verb of the grammar: {}", e.line, e.name);
            finish(&st, false, 0, &e.name, e.line, "script", 2)
        }
    };
    let mut st = st;
    let (mut verbs, mut last, mut line) = (0usize, String::new(), 0usize);
    for s in &steps {
        (last, line) = (s.name.clone(), s.line);
        if let Err(word) = st.run(&s.op) {
            eprintln!("raster: line {}: {}: {word}", s.line, s.name);
            finish(&st, false, verbs, &last, line, word, 1)
        }
        verbs += 1;
    }
    finish(&st, true, verbs, &last, line, "", 0)
}
