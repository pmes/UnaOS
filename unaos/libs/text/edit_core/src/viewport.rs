// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The visible rows of a pane, soft-wrapped through a caller-supplied measure (the core knows no fonts).

use crate::text::Text;
use crate::unicode;
use alloc::string::String;
use alloc::vec::Vec;

/// One visual row: bytes `start..end` of logical line `line`'s content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualRow {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// `true` for the first visual row of its logical line (where a line number is drawn).
    pub first: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Viewport {
    pub rows: Vec<VisualRow>,
}

/// Soft-wrap points of one line's content: the `(start, end)` byte ranges of its visual rows. Rows break at
/// grapheme boundaries, preferring the last point after whitespace; whitespace may hang past `width`; a
/// row always holds at least one grapheme, so a too-narrow width still terminates.
pub fn wrap_line(content: &str, width: u32, measure: &dyn Fn(&str) -> u32) -> Vec<(usize, usize)> {
    let mut rows = Vec::new();
    if content.is_empty() {
        rows.push((0, 0));
        return rows;
    }
    let gb = unicode::grapheme_boundaries(content);
    let mut row_start = 0;
    let mut opportunity = 0; // last break point after whitespace within the row (0 = none)
    let mut i = 1;
    while i < gb.len() {
        let (gs, ge) = (gb[i - 1], gb[i]);
        let g = &content[gs..ge];
        let is_space = g.chars().all(char::is_whitespace);
        if gs > row_start && !is_space && measure(&content[row_start..ge]) > width {
            let cut = if opportunity > row_start { opportunity } else { gs };
            rows.push((row_start, cut));
            row_start = cut;
            opportunity = 0;
            // re-examine graphemes from the new row start
            i = gb.iter().position(|&b| b == cut).unwrap_or(i - 1) + 1;
            continue;
        }
        if is_space {
            opportunity = ge;
        }
        i += 1;
    }
    rows.push((row_start, content.len()));
    rows
}

pub(crate) fn build(t: &Text, first_line: usize, rows: usize, width: u32, measure: &dyn Fn(&str) -> u32) -> Viewport {
    let mut out = Vec::with_capacity(rows);
    let mut l = first_line;
    while out.len() < rows && l < t.line_count() {
        let content = t.line(l);
        for (k, (s, e)) in wrap_line(&content, width, measure).into_iter().enumerate() {
            if out.len() == rows {
                break;
            }
            out.push(VisualRow { line: l, start: s, end: e, text: String::from(&content[s..e]), first: k == 0 });
        }
        l += 1;
    }
    Viewport { rows: out }
}
