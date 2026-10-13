// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL M3 (LEDGER SR96): the page's interaction model on the image — what the pointer and the keys act on.
//
// * HIT. `layout_core::hit::hit` (the host's `selection.rs` hit test, now ONE function both rings call) over the
//   page's text boxes (`layout_core::hit::text_boxes`, document coordinates), the caret measured with the face
//   the painter draws the box in (`paint::inks` → `txt::Fonts`), the DOM node's arena index as its identity.
// * TEXT. [`Flat`]: the page's painted text in paint order — each text box's fragment, a `\n` where the next box
//   starts a new line box, a space where two boxes on one line leave a gap — and where each box starts in it. A
//   selection is two offsets into it (anchor, focus: the hit's nearest grapheme boundary in its box), a find match
//   a range of it, Ctrl+C copies a slice of it; [`Flat::marks`] turns a range back into per-fragment byte ranges
//   for the painter.
// * LINKS. Every box knows the `a[href]` it sits in ([`link_of`], computed per layout), so hover is one lookup
//   after the hit, and Tab walks [`links`] (document order).
// * CLIPBOARD. `SYS_CLIP_SET` (LUMENUX B348, 64) — the verb LUMEN.ELF copies with; the kernel takes printable ASCII,
//   `\n` and `\t`, so [`clip_text`] folds the page's typography to that (dashes, quotes, NBSP, ellipsis) and marks
//   anything else `?`.

use alloc::string::String;
use alloc::vec::Vec;
use html_core::{Document, NodeId};
use layout_core::hit::TextBox;
use layout_core::inline::Frag;
use layout_core::NodeMap;

use crate::paint::{Mark, MarkKind};
use crate::style::Tree;

/// The page's painted text and where each text box starts in it.
#[derive(Default)]
pub struct Flat {
    pub text: String,
    pub starts: Vec<usize>,
    pub lens: Vec<usize>,
}

/// The fragment text of box `b`.
pub fn frag_text<'t>(tree: &'t Tree, b: &TextBox) -> &'t str {
    match tree.inline.get(&b.ifc).and_then(|il| il.frags.get(b.frag)) {
        Some(Frag::Text { text, .. }) => text.as_str(),
        _ => "",
    }
}

impl Flat {
    pub fn build(tree: &Tree, boxes: &[TextBox]) -> Flat {
        let mut f = Flat::default();
        for (i, b) in boxes.iter().enumerate() {
            let t = frag_text(tree, b);
            if i > 0 {
                let p = &boxes[i - 1];
                if (b.top - p.top).abs() >= 0.5 {
                    f.text.push('\n');
                } else if b.x > p.x + p.w + 1.0 && !f.text.ends_with(' ') && !t.starts_with(' ') {
                    f.text.push(' ');
                }
            }
            f.starts.push(f.text.len());
            f.lens.push(t.len());
            f.text.push_str(t);
        }
        f
    }

    /// The offset of byte `byte` of box `i`.
    pub fn at(&self, i: usize, byte: usize) -> usize {
        match self.starts.get(i) {
            Some(&s) => s + byte.min(self.lens[i]),
            None => self.text.len(),
        }
    }

    /// The range [`a`, `b`) as per-fragment marks, keyed by IFC root.
    pub fn marks(&self, boxes: &[TextBox], a: usize, b: usize, kind: MarkKind, out: &mut NodeMap<Vec<Mark>>) {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if a == b {
            return;
        }
        for (i, bx) in boxes.iter().enumerate() {
            let (s0, len) = (self.starts[i], self.lens[i]);
            if s0 >= b {
                break;
            }
            if s0 + len <= a {
                continue;
            }
            let s = a.max(s0) - s0;
            let e = b.min(s0 + len) - s0;
            if e > s {
                out.entry(bx.ifc).or_default().push(Mark { frag: bx.frag, s, e, kind });
            }
        }
    }

    /// Every match of `q` (ASCII case folded) as flat start offsets.
    pub fn find(&self, q: &str) -> Vec<usize> {
        let mut out = Vec::new();
        if q.is_empty() {
            return out;
        }
        let hay = self.text.to_ascii_lowercase();
        let needle = q.to_ascii_lowercase();
        let mut from = 0;
        while let Some(k) = hay[from..].find(needle.as_str()) {
            out.push(from + k);
            from += k + needle.len().max(1);
            if from >= hay.len() {
                break;
            }
        }
        out
    }

    /// The box holding flat offset `o` (the last box starting at or before it).
    pub fn box_at(&self, o: usize) -> Option<usize> {
        match self.starts.binary_search(&o) {
            Ok(i) => Some(i),
            Err(0) => None,
            Err(i) => Some(i - 1),
        }
    }

    /// A char-boundary-safe slice [`a`, `b`).
    pub fn slice(&self, a: usize, b: usize) -> &str {
        let (mut a, mut b) = (a.min(b).min(self.text.len()), a.max(b).min(self.text.len()));
        while a > 0 && !self.text.is_char_boundary(a) {
            a -= 1;
        }
        while b < self.text.len() && !self.text.is_char_boundary(b) {
            b += 1;
        }
        &self.text[a..b]
    }
}

/// The `a[href]` element at or above DOM node `id`.
pub fn link_at(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut cur = Some(id);
    while let Some(n) = cur {
        if let Some(e) = doc.element(n) {
            if e.is_html("a") && e.attr("href").is_some() {
                return Some(n);
            }
        }
        cur = doc.parent(n);
    }
    None
}

/// Per text box, the link it sits in.
pub fn link_of(doc: &Document, tree: &Tree, boxes: &[TextBox]) -> Vec<Option<NodeId>> {
    boxes.iter().map(|b| tree.node_map.get(&b.node).and_then(|n| link_at(doc, n.id))).collect()
}

/// Every `a[href]` of the document, in document order.
pub fn links(doc: &Document) -> Vec<NodeId> {
    doc.descendants(Document::ROOT)
        .filter(|&n| doc.element(n).is_some_and(|e| e.is_html("a") && e.attr("href").is_some()))
        .collect()
}

/// A link's `href`, as written.
pub fn href(doc: &Document, link: NodeId) -> String {
    doc.element(link).and_then(|e| e.attr("href")).map(String::from).unwrap_or_default()
}

/// The page text as the kernel's clipboard takes it (printable ASCII, `\n`, `\t`; at most `CLIP_CAP` bytes).
pub fn clip_text(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let rep: &[u8] = match c {
            ' '..='~' | '\n' | '\t' => {
                out.push(c as u8);
                continue;
            }
            '\u{a0}' | '\u{2000}'..='\u{200a}' | '\u{3000}' => b" ",
            '\u{2010}'..='\u{2015}' | '\u{2212}' => b"-",
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{2032}' => b"'",
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{2033}' => b"\"",
            '\u{2026}' => b"...",
            '\u{b7}' | '\u{2022}' => b"*",
            '\u{200b}' | '\u{feff}' | '\r' => b"",
            _ => b"?",
        };
        out.extend_from_slice(rep);
    }
    if out.len() > una_abi::CLIP_CAP {
        out.truncate(una_abi::CLIP_CAP);
    }
    out
}

/// The find bar: the query, its matches (flat offsets), the current one.
#[derive(Default)]
pub struct Find {
    pub q: String,
    pub hits: Vec<usize>,
    pub cur: usize,
}
