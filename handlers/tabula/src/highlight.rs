// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! SYNTAXCORE (SR89, R104/R105): Tabula's highlighting.
//!
//! The UnaOS syntax core (`unaos/libs/text/syntax_core`) is INSERTED beside GtkSourceView's own
//! highlighting, which stays as the other-OS face. Which one paints the GTK view is the Principia
//! preference `tabula.highlight` (`core` | `sourceview`, default `core`; declared in
//! `prefs_core::schema::SCHEMA`). With `core`, the sourceview buffer's own syntax highlighting is switched
//! off and [`highlight_with_core`] applies the core's spans as `GtkTextTag`s, re-painting only the lines the
//! core re-lexed; with `sourceview`, GtkSourceView paints exactly as before and the core's tags are unused.
//!
//! The core reads the buffer of record (edit_core's piece table) through [`CoreSource`], one line at a
//! time — never a copy of the document as one `String`.

use std::ops::Range;

use bandy::PrefValue;
pub use syntax_core::{Highlight, HighlightKind, Theme};

/// The preference namespace Tabula reads (the `tabula` domain file of `<home>/settings`).
pub const PREF_NS: &str = "tabula";

/// Who paints the GTK view's syntax colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightEngine {
    /// The UnaOS syntax core, through a `GtkTextTag` layer.
    #[default]
    Core,
    /// GtkSourceView's own language definitions (the other-OS face, R104).
    SourceView,
}

impl HighlightEngine {
    pub fn parse(v: &str) -> Option<HighlightEngine> {
        match v {
            "core" => Some(HighlightEngine::Core),
            "sourceview" => Some(HighlightEngine::SourceView),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            HighlightEngine::Core => "core",
            HighlightEngine::SourceView => "sourceview",
        }
    }
}

/// Everything highlighting reads from the preference store: the engine, the theme map (kind →
/// appearance token, `tabula.theme.<kind>`), and whether the appearance is dark (`system.appearance.mode`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HighlightPrefs {
    pub engine: HighlightEngine,
    pub theme: Theme,
    pub dark: bool,
}

impl HighlightPrefs {
    /// Build from lookups in namespace `tabula` (`get("highlight")`, `get("theme.keyword")`, ...).
    /// Defaults live here, with the consumer: an unset or unknown `highlight` is `core`.
    pub fn from_prefs(get: impl Fn(&str) -> Option<PrefValue>, dark: bool) -> Self {
        let engine = match get("highlight") {
            Some(PrefValue::Str(s)) => HighlightEngine::parse(s.trim()).unwrap_or_default(),
            _ => HighlightEngine::Core,
        };
        let theme = Theme::from_prefs(&|k: &str| match get(k) {
            Some(PrefValue::Str(s)) => Some(s),
            _ => None,
        });
        HighlightPrefs { engine, theme, dark }
    }

    /// Read the standard store (`principia::default_prefs_path()`, read-only — Principia stays the
    /// writer of record). An unreadable store is the defaults. `auto` appearance paints light here.
    pub fn load() -> Self {
        match principia::PrefStore::load(principia::default_prefs_path()) {
            Ok(store) => {
                let dark = matches!(store.get("system", "appearance.mode"), Some(PrefValue::Str(ref m)) if m == "dark");
                Self::from_prefs(|k| store.get(PREF_NS, k), dark)
            }
            Err(_) => Self::default(),
        }
    }
}

/// edit_core's buffer as the syntax core's [`syntax_core::Source`]: line count, line start, one line's
/// content (terminator excluded) — read from the piece table per line.
pub struct CoreSource<'a>(pub &'a edit_core::Buffer);

impl syntax_core::Source for CoreSource<'_> {
    fn line_count(&self) -> usize {
        self.0.line_count()
    }
    fn line_start(&self, line: usize) -> usize {
        self.0.text().line_start(line)
    }
    fn read_line(&self, line: usize, out: &mut String) {
        let t = self.0.text();
        out.push_str(&t.slice(t.line_start(line), t.line_end(line)));
    }
}

/// The host face's palette for the appearance tokens the theme map may name. These are the values of the
/// painter's own sets (kernel `video/theme.rs`: `LIGHT` / `DARK` rows `ContentText`, `TitleTextInactive`,
/// `ErrorText`, `SyntaxCode`, `SyntaxNum`, `SyntaxLit`, and the kit accent `ACCENT`), so the GTK face and
/// the metal paint the same ink. An unknown name falls back to `ContentText`.
pub fn token_rgb(token: &str, dark: bool) -> u32 {
    let (light, dark_v) = match token {
        "TitleTextInactive" => (0x007A_7A7E, 0x008E_8E94),
        "ErrorText" => (0x00A0_2020, 0x00E5_7373),
        "SyntaxCode" => (0x0020_8040, 0x006C_C88A),
        "SyntaxNum" => (0x00B0_5000, 0x00E3_9A55),
        "SyntaxLit" => (0x0080_30A0, 0x00C3_8BE0),
        "Accent" => (0x004A_73AA, 0x004A_73AA),
        _ => (0x0021_201E, 0x00E6_E4DF),
    };
    if dark { dark_v } else { light }
}

/// The tag name the core's spans of kind `k` carry in a GtkTextBuffer.
pub fn tag_name(k: HighlightKind) -> String {
    format!("una-syntax:{}", k.name())
}

#[cfg(feature = "gtk")]
fn ensure_tag(table: &gtk4::TextTagTable, k: HighlightKind, prefs: &HighlightPrefs) -> gtk4::TextTag {
    let name = tag_name(k);
    if let Some(t) = table.lookup(&name) {
        return t;
    }
    let rgb = token_rgb(prefs.theme.token(k), prefs.dark);
    let rgba = gtk4::gdk::RGBA::new(
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        1.0,
    );
    let b = gtk4::TextTag::builder().name(name.as_str()).foreground_rgba(&rgba);
    let b = match k {
        HighlightKind::Comment | HighlightKind::DocComment | HighlightKind::Emphasis => b.style(gtk4::pango::Style::Italic),
        HighlightKind::Heading => b.weight(700),
        HighlightKind::Link => b.underline(gtk4::pango::Underline::Single),
        HighlightKind::Error => b.underline(gtk4::pango::Underline::Error),
        _ => b,
    };
    let tag = b.build();
    let _ = table.add(&tag);
    tag
}

/// The `highlight_with_core` path: apply the core's spans for `lines` (all lines when `None`) of `doc` to
/// `buf` as tags, after clearing the core's tags on those lines. When the view and the record disagree on
/// the line structure, or the document has no grammar, the core's tags are cleared and nothing is painted.
#[cfg(feature = "gtk")]
pub fn highlight_with_core(
    buf: &gtk4::TextBuffer,
    doc: &crate::TabulaDocument,
    prefs: &HighlightPrefs,
    lines: Option<Range<usize>>,
) {
    use gtk4::prelude::*;
    let table = buf.tag_table();
    let tags: Vec<gtk4::TextTag> = HighlightKind::ALL.iter().map(|&k| ensure_tag(&table, k, prefs)).collect();
    let hl = doc.highlighter();
    let n = hl.line_count();
    if doc.syntax.is_none() || buf.line_count().max(0) as usize != n {
        let (s, e) = buf.bounds();
        for t in &tags {
            buf.remove_tag(t, &s, &e);
        }
        return;
    }
    let r = lines.unwrap_or(0..n);
    let (first, last) = (r.start.min(n), r.end.min(n));
    if first >= last {
        return;
    }
    let s = buf.iter_at_line(first as i32).unwrap_or_else(|| buf.start_iter());
    let e = if last >= n { buf.end_iter() } else { buf.iter_at_line(last as i32).unwrap_or_else(|| buf.end_iter()) };
    for t in &tags {
        buf.remove_tag(t, &s, &e);
    }
    for li in first..last {
        for tok in hl.line_tokens(li) {
            let a = buf.iter_at_line_index(li as i32, tok.start as i32);
            let b = buf.iter_at_line_index(li as i32, tok.end as i32);
            if let (Some(a), Some(b)) = (a, b) {
                buf.apply_tag(&tags[tok.kind.index()], &a, &b);
            }
        }
    }
}

/// The spans of `lines` in absolute byte offsets of the record (a non-GTK face, a test, a ring-3 port).
pub fn spans(doc: &mut crate::TabulaDocument, lines: Range<usize>) -> Vec<Highlight> {
    doc.highlight_spans(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_preference_defaults_to_core() {
        let none = |_: &str| None;
        assert_eq!(HighlightPrefs::from_prefs(none, false).engine, HighlightEngine::Core);
        let sv = |k: &str| (k == "highlight").then(|| PrefValue::Str("sourceview".into()));
        assert_eq!(HighlightPrefs::from_prefs(sv, false).engine, HighlightEngine::SourceView);
        let bad = |k: &str| (k == "highlight").then(|| PrefValue::Str("tree-sitter".into()));
        assert_eq!(HighlightPrefs::from_prefs(bad, false).engine, HighlightEngine::Core, "R105: unknown is core");
        for e in [HighlightEngine::Core, HighlightEngine::SourceView] {
            assert_eq!(HighlightEngine::parse(e.name()), Some(e));
        }
    }

    #[test]
    fn theme_preferences_reach_the_map_and_the_palette() {
        let get = |k: &str| (k == "theme.comment").then(|| PrefValue::Str("ErrorText".into()));
        let p = HighlightPrefs::from_prefs(get, true);
        assert_eq!(p.theme.token(HighlightKind::Comment), "ErrorText");
        assert_eq!(token_rgb(p.theme.token(HighlightKind::Comment), p.dark), 0x00E5_7373);
        assert_eq!(token_rgb("NoSuchToken", false), 0x0021_201E);
        assert_eq!(tag_name(HighlightKind::DocComment), "una-syntax:doc-comment");
    }

    #[test]
    fn core_source_reads_the_piece_table_by_line() {
        use syntax_core::Source;
        let b = edit_core::Buffer::new("a\r\nbc\n");
        let src = CoreSource(&b);
        assert_eq!(src.line_count(), 3);
        assert_eq!(src.line_start(1), 3);
        let mut s = String::new();
        src.read_line(0, &mut s);
        src.read_line(1, &mut s);
        assert_eq!(s, "abc", "terminators excluded");
    }
}
