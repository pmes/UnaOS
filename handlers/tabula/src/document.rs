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

//! Pure, toolkit-free editor document core.
//!
//! `TabulaDocument` is the portable heart of the editor: a path, a text
//! buffer, a language id, and a dirty flag. It has no GTK/sourceview
//! dependency, so it builds anywhere. The GTK `TabulaView` (behind the `gtk`
//! feature) is a rendering shell over this state.
//!
//! EDITCORE (SR73, R104): the buffer of record is [`edit_core::Buffer`] — UnaOS's
//! own no_std editing core (`unaos/libs/text/edit_core`), the same bytes a
//! ring-3 editor on the image links. `load`, `save`, `set_buffer` and every
//! edit go through it; the GTK view mirrors it both ways (`src/lib.rs`).
//!
//! SYNTAXCORE (SR89, R105): the document also carries its grammar — `syntax`, UnaOS's own syntax core
//! chosen by `language` — and the core's per-line lexer checkpoints (`highlights`). Every edit made
//! through this type splices the changed line range into the checkpoints and re-lexes from there
//! (`crate::highlight`), so a one-character edit re-lexes one line in the common case.

use std::fmt;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

pub use edit_core::{Buffer as EditBuffer, Change};
use syntax_core::{Highlight, Highlighter, Language};

use crate::highlight::CoreSource;

/// An in-memory editor document, decoupled from any UI toolkit.
///
/// `Clone`, `PartialEq` and `Debug` are written by hand: the grammar is a `Box<dyn Language>`, compared
/// and cloned by its id.
pub struct TabulaDocument {
    /// Backing file, if this document has been loaded from / bound to one.
    pub path: Option<PathBuf>,
    /// The buffer of record: UnaOS's editing core (text, line index,
    /// cursors, grouped undo). `save()` writes THIS.
    pub core: edit_core::Buffer,
    /// A plain-`String` snapshot of `core`'s text, folded forward on every
    /// edit made through this type, for readers that still take a `String`
    /// (`vessels/una/src/main.rs` reads `doc.buffer`; TABULABUS SR74 moves it
    /// onto [`Self::text`] and this snapshot goes). It is not the record:
    /// writing it changes nothing that is saved.
    pub buffer: String,
    /// sourceview-style language id (e.g. "rust", "toml", "txt").
    pub language: String,
    /// True when `buffer` has unsaved edits relative to disk.
    pub dirty: bool,
    /// True when this document must never be written back — set for console
    /// logs. Two reasons stack, and the deeper one is not the app's:
    ///
    /// 1. **Ownership (the real denial).** On the shard a kernel flight-recorder
    ///    log is owned by *root*; the UnaFS ownership ACL (`acl-<lba>-<off>`
    ///    rows carrying `owner`/`grants:*`) refuses any write from a user-owned
    ///    vessel — see `docs/SECURITY.md`. A user app *cannot* write it, flag or
    ///    no flag. This bool mirrors that filesystem fact so the host viewer
    ///    behaves the same way the metal filesystem would enforce.
    /// 2. **Rendering fidelity.** The buffer is a *rendering* of the file
    ///    (padding trimmed, control bytes made visible, possibly only the tail),
    ///    so even for a log an operator *did* own, writing this text back would
    ///    destroy the record it came from.
    ///
    /// `save()` refuses and `set_buffer()` is inert.
    pub read_only: bool,
    /// SYNTAXCORE: the grammar for `language` (`syntax_core::language_for_id`), `None` for a language the
    /// core has no grammar for ("txt", a console log). [`Self::set_language`] keeps the two together; a
    /// `language` written directly is noticed on the next edit or [`Self::highlight_spans`].
    pub syntax: Option<Box<dyn Language>>,
    /// The core's per-line lexer checkpoints and tokens for `core`'s text.
    pub highlights: Highlighter,
    /// Lines re-lexed since a view last took them ([`Self::take_syntax_dirty`]).
    syntax_dirty: Option<Range<usize>>,
}

impl Clone for TabulaDocument {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            core: self.core.clone(),
            buffer: self.buffer.clone(),
            language: self.language.clone(),
            dirty: self.dirty,
            read_only: self.read_only,
            syntax: self.syntax.as_ref().and_then(|l| syntax_core::language_for_id(l.id())),
            highlights: self.highlights.clone(),
            syntax_dirty: self.syntax_dirty.clone(),
        }
    }
}

impl PartialEq for TabulaDocument {
    fn eq(&self, o: &Self) -> bool {
        self.path == o.path
            && self.core == o.core
            && self.buffer == o.buffer
            && self.language == o.language
            && self.dirty == o.dirty
            && self.read_only == o.read_only
            && self.syntax.as_ref().map(|l| l.id()) == o.syntax.as_ref().map(|l| l.id())
    }
}

impl Eq for TabulaDocument {}

impl fmt::Debug for TabulaDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TabulaDocument")
            .field("path", &self.path)
            .field("core", &self.core)
            .field("buffer", &self.buffer)
            .field("language", &self.language)
            .field("dirty", &self.dirty)
            .field("read_only", &self.read_only)
            .field("syntax", &self.syntax.as_ref().map(|l| l.id()))
            .finish()
    }
}

impl Default for TabulaDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl TabulaDocument {
    /// A fresh, empty, path-less document with the fallback language.
    pub fn new() -> Self {
        Self::with_text("")
    }

    /// A clean, path-less document holding `text` exactly (line endings kept).
    pub fn with_text(text: &str) -> Self {
        Self {
            path: None,
            core: edit_core::Buffer::new(text),
            buffer: text.to_string(),
            language: "txt".to_string(),
            dirty: false,
            read_only: false,
            syntax: None,
            highlights: Highlighter::new(),
            syntax_dirty: None,
        }
    }

    /// The text of record, as a `String`.
    pub fn text(&self) -> String {
        self.core.to_string()
    }

    /// Run an edit on the core and fold its [`Change`]s into the snapshot;
    /// marks the document dirty when anything changed. Inert (no changes) on
    /// a read-only document. The returned changes are what a view replays.
    pub fn edit(&mut self, f: impl FnOnce(&mut edit_core::Buffer) -> Vec<Change>) -> Vec<Change> {
        if self.read_only {
            return Vec::new();
        }
        let changes = f(&mut self.core);
        self.fold(&changes);
        self.sync_syntax(&changes);
        changes
    }

    /// Replace a byte range of the record (one undo group) — the entry the
    /// host view's mirror uses for a toolkit-side insert or delete.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) -> Vec<Change> {
        self.edit(|b| b.replace(range, text))
    }

    /// Undo the core's newest group.
    pub fn undo(&mut self) -> Vec<Change> {
        self.edit(|b| b.undo().unwrap_or_default())
    }

    /// Redo the core's newest undone group.
    pub fn redo(&mut self) -> Vec<Change> {
        self.edit(|b| b.redo().unwrap_or_default())
    }

    fn fold(&mut self, changes: &[Change]) {
        for c in changes {
            if c.old_end <= self.buffer.len()
                && self.buffer.is_char_boundary(c.start)
                && self.buffer.is_char_boundary(c.old_end)
            {
                self.buffer.replace_range(c.start..c.old_end, &c.text);
            } else {
                // The snapshot was written from outside; re-derive it from the record.
                self.buffer = self.core.to_string();
                break;
            }
        }
        if !changes.is_empty() {
            self.dirty = true;
        }
    }

    /// Choose the language (a sourceview-style id) and its grammar, and lex the document afresh.
    pub fn set_language(&mut self, id: &str) {
        self.language = id.to_string();
        self.syntax = syntax_core::language_for_id(id);
        self.rehighlight();
    }

    /// Lex the whole document with its grammar (or drop the checkpoints when it has none); every line
    /// becomes dirty for the view.
    pub fn rehighlight(&mut self) {
        match self.syntax.as_deref() {
            Some(lang) => {
                self.highlights.reset(lang, &CoreSource(&self.core));
            }
            None => self.highlights = Highlighter::new(),
        }
        let n = self.core.line_count();
        self.mark_syntax_dirty(0..n);
    }

    /// The core's checkpoints and tokens (line-relative), as last lexed.
    pub fn highlighter(&self) -> &Highlighter {
        &self.highlights
    }

    /// The lines re-lexed since the last call (the range a view re-paints), clearing it.
    pub fn take_syntax_dirty(&mut self) -> Option<Range<usize>> {
        self.syntax_dirty.take()
    }

    /// Absolute-offset spans for `lines`, re-choosing the grammar first if `language` was written directly.
    pub fn highlight_spans(&mut self, lines: Range<usize>) -> Vec<Highlight> {
        self.ensure_syntax();
        self.highlights.highlights(&CoreSource(&self.core), lines)
    }

    /// Re-choose the grammar when `language` no longer names the one held.
    fn ensure_syntax(&mut self) {
        let want = syntax_core::language_for_id(&self.language);
        let same = match (&want, &self.syntax) {
            (Some(a), Some(b)) => a.id() == b.id(),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.syntax = want;
            self.rehighlight();
        }
    }

    /// Splice `changes` into the checkpoints and re-lex what they touched.
    fn sync_syntax(&mut self, changes: &[Change]) {
        if changes.is_empty() {
            return;
        }
        let held = self.syntax.as_ref().map(|l| l.id());
        self.ensure_syntax();
        if self.syntax.as_ref().map(|l| l.id()) != held {
            return; // ensure_syntax lexed the whole (already edited) document
        }
        let Some(lang) = self.syntax.as_deref() else {
            return;
        };
        for c in changes {
            self.highlights.splice(c.first_line, c.old_last_line, c.new_last_line);
        }
        let r = self.highlights.relex(lang, &CoreSource(&self.core));
        self.mark_syntax_dirty(r);
    }

    fn mark_syntax_dirty(&mut self, r: Range<usize>) {
        if r.start >= r.end {
            return;
        }
        self.syntax_dirty = Some(match self.syntax_dirty.take() {
            Some(d) => d.start.min(r.start)..d.end.max(r.end),
            None => r,
        });
    }

    /// Map a file extension to a sourceview language id.
    ///
    /// Lifted verbatim from the GTK `load_file` auto-detection table so the
    /// core and the view agree on language selection.
    pub fn language_for(ext: &str) -> &'static str {
        match ext {
            "rs" => "rust",
            "toml" => "toml",
            "md" => "markdown",
            "py" => "python",
            "js" | "ts" => "javascript",
            "json" => "json",
            "c" | "h" | "cpp" => "c",
            "sh" | "bash" => "sh",
            _ => "txt",
        }
    }

    /// Load a document from disk, detecting language from the extension.
    /// The returned document is clean (`dirty == false`).
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let buffer = fs::read_to_string(path)?;
        let language = path
            .extension()
            .and_then(|s| s.to_str())
            .map(Self::language_for)
            .unwrap_or("txt")
            .to_string();
        let mut doc = Self {
            path: Some(path.to_path_buf()),
            core: edit_core::Buffer::new(&buffer),
            buffer,
            language: String::new(),
            dirty: false,
            read_only: false,
            syntax: None,
            highlights: Highlighter::new(),
            syntax_dirty: None,
        };
        doc.set_language(&language);
        Ok(doc)
    }

    /// Load a console/serial log for viewing: read-only, control-byte-safe,
    /// size-capped. See [`crate::logview`] for exactly what is done to the
    /// bytes and why.
    pub fn load_log(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let rendered = crate::logview::load_log(path)?;
        Ok(Self {
            path: Some(path.to_path_buf()),
            core: edit_core::Buffer::new(&rendered.text),
            buffer: rendered.text,
            language: "txt".to_string(),
            dirty: false,
            read_only: true,
            syntax: None,
            highlights: Highlighter::new(),
            syntax_dirty: None,
        })
    }

    /// Open any path the right way: console logs through [`Self::load_log`],
    /// everything else through [`Self::load`]. This is the single entry point
    /// a vessel should call when the operator activates a file.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if crate::logview::is_log_path(path) {
            Self::load_log(path)
        } else {
            Self::load(path)
        }
    }

    /// Replace the buffer contents, marking the document dirty.
    ///
    /// Goes through the core as ONE minimal edit (common prefix and suffix
    /// kept), so it is a single undo group and a view re-renders only the
    /// lines that differ.
    ///
    /// Inert on a read-only document: the view may still be editable in the
    /// host toolkit, but a log's held state stays exactly what was read.
    pub fn set_buffer(&mut self, content: impl Into<String>) {
        if self.read_only {
            return;
        }
        let content = content.into();
        self.edit(|b| b.set_text(&content));
        self.dirty = true;
    }

    /// Write the buffer to the bound path, clearing the dirty flag.
    /// Errors if the document has no path, or is read-only.
    pub fn save(&mut self) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "document is read-only (console log view)",
            ));
        }
        let path = self.path.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "TabulaDocument has no path to save to")
        })?;
        fs::write(path, self.core.to_string().as_bytes())?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection() {
        assert_eq!(TabulaDocument::language_for("rs"), "rust");
        assert_eq!(TabulaDocument::language_for("ts"), "javascript");
        assert_eq!(TabulaDocument::language_for("cpp"), "c");
        assert_eq!(TabulaDocument::language_for("xyz"), "txt");
    }

    #[test]
    fn dirty_lifecycle_and_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("tabula_doc_test.rs");
        let _ = fs::remove_file(&path);

        let mut doc = TabulaDocument::new();
        assert!(!doc.dirty);
        doc.path = Some(path.clone());
        doc.set_buffer("fn main() {}\n");
        assert!(doc.dirty);
        doc.save().unwrap();
        assert!(!doc.dirty);

        let loaded = TabulaDocument::load(&path).unwrap();
        assert_eq!(loaded.buffer, "fn main() {}\n");
        assert_eq!(loaded.language, "rust");
        assert!(!loaded.dirty);

        let _ = fs::remove_file(&path);
    }

    /// No `tempfile` crate is in the workspace, so tests own their scratch
    /// paths: a per-test basename plus the process id keeps parallel runs and
    /// concurrent test binaries from colliding. Callers must clean up.
    fn scratch_path(name: &str) -> PathBuf {
        // pid goes *before* `name` so `name`'s extension stays trailing and
        // language detection sees the real suffix.
        std::env::temp_dir().join(format!("tabula_{}_{}", std::process::id(), name))
    }

    #[test]
    fn language_for_edge_cases() {
        // Every mapped extension resolves to its language id.
        assert_eq!(TabulaDocument::language_for("toml"), "toml");
        assert_eq!(TabulaDocument::language_for("md"), "markdown");
        assert_eq!(TabulaDocument::language_for("py"), "python");
        assert_eq!(TabulaDocument::language_for("js"), "javascript");
        assert_eq!(TabulaDocument::language_for("json"), "json");
        assert_eq!(TabulaDocument::language_for("h"), "c");

        // Unknown and empty extensions fall back to the plain-text id.
        assert_eq!(TabulaDocument::language_for("unknownext"), "txt");
        assert_eq!(TabulaDocument::language_for(""), "txt");
        // Case sensitivity is intentional: only lowercase ids are mapped.
        assert_eq!(TabulaDocument::language_for("RS"), "txt");
    }

    #[test]
    fn load_file_without_extension_is_txt() {
        let path = scratch_path("noext");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"plain body, no extension\n").unwrap();

        let doc = TabulaDocument::load(&path).unwrap();
        assert_eq!(doc.language, "txt");
        assert_eq!(doc.buffer, "plain body, no extension\n");
        assert_eq!(doc.path.as_deref(), Some(path.as_path()));
        assert!(!doc.dirty);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_file_with_unknown_extension_is_txt() {
        let path = scratch_path("weird.zzz");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"content").unwrap();

        let doc = TabulaDocument::load(&path).unwrap();
        assert_eq!(doc.language, "txt");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_errors() {
        let path = scratch_path("does_not_exist.rs");
        let _ = fs::remove_file(&path);
        assert!(TabulaDocument::load(&path).is_err());
    }

    #[test]
    fn save_roundtrip_via_tempdir() {
        let path = scratch_path("roundtrip.toml");
        let _ = fs::remove_file(&path);

        let mut doc = TabulaDocument::new();
        doc.path = Some(path.clone());
        doc.set_buffer("[package]\nname = \"x\"\n");
        assert!(doc.dirty);
        doc.save().unwrap();
        assert!(!doc.dirty, "save clears the dirty flag");

        let reloaded = TabulaDocument::load(&path).unwrap();
        assert_eq!(reloaded.buffer, "[package]\nname = \"x\"\n");
        assert_eq!(reloaded.language, "toml");
        assert!(!reloaded.dirty);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn dirty_flag_transitions() {
        // Fresh and freshly-loaded documents are clean.
        let mut doc = TabulaDocument::new();
        assert!(!doc.dirty);

        // set_buffer dirties.
        doc.set_buffer("a");
        assert!(doc.dirty);

        // Re-setting keeps it dirty (there is no equality short-circuit).
        doc.set_buffer("a");
        assert!(doc.dirty);

        // A failed save (no path) must NOT clear the dirty flag.
        assert!(doc.path.is_none());
        assert!(doc.save().is_err());
        assert!(doc.dirty, "failed save leaves the document dirty");

        // A successful save clears it.
        let path = scratch_path("transitions.txt");
        let _ = fs::remove_file(&path);
        doc.path = Some(path.clone());
        doc.save().unwrap();
        assert!(!doc.dirty);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn core_is_the_record_and_the_snapshot_follows() {
        let path = scratch_path("record.rs");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"fn a() {}\r\nfn b() {}\n").unwrap();
        let mut doc = TabulaDocument::load(&path).unwrap();
        assert_eq!(doc.text(), "fn a() {}\r\nfn b() {}\n");
        assert_eq!(doc.core.line_count(), 3);

        // An edit through the core API: the snapshot follows, the doc is dirty.
        let changes = doc.edit(|b| {
            b.set_cursor(b.pos_to_offset(edit_core::Pos::new(1, 9)));
            b.newline()
        });
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].text, "\n", "the line's own ending (LF) is repeated");
        assert_eq!(doc.buffer, doc.text());
        assert!(doc.dirty);

        // A view-side edit (the mirror's entry) and undo.
        doc.replace_range(0..2, "pub fn");
        assert_eq!(doc.buffer, "pub fn a() {}\r\nfn b() {}\n\n");
        doc.undo();
        assert_eq!(doc.buffer, "fn a() {}\r\nfn b() {}\n\n");

        // Writing the snapshot from outside is not the record: save writes the core.
        doc.buffer.push_str("NOT SAVED");
        doc.save().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"fn a() {}\r\nfn b() {}\n\n", "CRLF kept byte-exact");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn set_buffer_is_one_minimal_undoable_edit() {
        let mut doc = TabulaDocument::with_text("alpha\nbeta\ngamma\n");
        doc.set_buffer("alpha\nBETA\ngamma\n");
        assert_eq!(doc.core.undo_depth(), 1);
        let undo = doc.undo();
        assert_eq!(undo.len(), 1);
        assert_eq!((undo[0].first_line, undo[0].new_last_line), (1, 1), "only line 1 re-renders");
        assert_eq!(doc.buffer, "alpha\nbeta\ngamma\n");
    }

    #[test]
    fn read_only_refuses_core_edits() {
        let mut doc = TabulaDocument::with_text("log line\n");
        doc.read_only = true;
        assert!(doc.replace_range(0..3, "X").is_empty());
        assert!(doc.edit(|b| b.insert("x")).is_empty());
        doc.set_buffer("other");
        assert_eq!(doc.text(), "log line\n");
        assert!(!doc.dirty);
    }

    #[test]
    fn syntax_core_follows_the_language_and_the_edits() {
        let mut doc = TabulaDocument::with_text("fn a() {}\nlet x = 1;\n");
        assert!(doc.syntax.is_none(), "txt has no grammar");
        doc.set_language("rust");
        assert_eq!(doc.syntax.as_ref().map(|l| l.id()), Some("rust"));
        assert_eq!(doc.take_syntax_dirty(), Some(0..3));
        assert_eq!(doc.highlighter().line_tokens(0)[0].kind, syntax_core::HighlightKind::Keyword);
        // "fn a() {}\n" is 10 bytes: the `x` of line 1 is byte 14.
        doc.replace_range(14..15, "y");
        assert_eq!(doc.take_syntax_dirty(), Some(1..2), "a one-character edit re-lexes one line");
        let spans = doc.highlight_spans(1..2);
        assert_eq!(&doc.text()[spans[1].range.clone()], "y");
        // A clone carries the grammar; a language with no grammar drops it.
        assert_eq!(doc.clone(), doc);
        doc.set_language("txt");
        assert!(doc.syntax.is_none());
        // A language written directly is noticed.
        doc.language = "sh".to_string();
        assert!(!doc.highlight_spans(0..1).is_empty());
        assert_eq!(doc.syntax.as_ref().map(|l| l.id()), Some("sh"));
    }

    #[test]
    fn loaded_shell_and_markdown_get_their_grammars() {
        assert_eq!(TabulaDocument::language_for("sh"), "sh");
        let path = scratch_path("syntax.md");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"# Title\n").unwrap();
        let doc = TabulaDocument::load(&path).unwrap();
        assert_eq!(doc.syntax.as_ref().map(|l| l.id()), Some("markdown"));
        assert_eq!(doc.highlighter().line_tokens(0)[0].kind, syntax_core::HighlightKind::Heading);
        assert!(!doc.dirty);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn save_with_no_path_errors() {
        let mut doc = TabulaDocument::new();
        doc.set_buffer("orphan buffer");
        let err = doc.save().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // The buffer and dirty flag are untouched by the failed save.
        assert_eq!(doc.buffer, "orphan buffer");
        assert!(doc.dirty);
    }
}
