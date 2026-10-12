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

pub mod document;
pub mod logview;
pub use document::TabulaDocument;
pub use logview::{default_console_log, is_log_path, LogText};

#[cfg(feature = "gtk")]
use gtk4::prelude::*;
#[cfg(feature = "gtk")]
use gtk4::{ScrolledWindow, Widget};
#[cfg(feature = "gtk")]
use sourceview5::prelude::*;
#[cfg(feature = "gtk")]
use sourceview5::{LanguageManager, View as SourceView};
#[cfg(feature = "gtk")]
use std::cell::{Cell, RefCell};
#[cfg(feature = "gtk")]
use std::path::Path;
#[cfg(feature = "gtk")]
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum EditorMode {
    Code(String), // Language ID
    Prose,
    Log,
}

#[cfg(feature = "gtk")]
pub struct TabulaView {
    pub view: SourceView,
    container: ScrolledWindow,
    /// The mode this view was built in — kept so `load_file` can restore the
    /// view after `load_log` has put it in the read-only Console treatment.
    mode: EditorMode,
    /// EDITCORE (SR73, R104): the buffer of record. GtkSourceView keeps doing
    /// the editing on this OS; every GtkTextBuffer insert/delete is mirrored
    /// into this document's `edit_core::Buffer`, and every core edit made
    /// through [`TabulaView::edit`] is replayed into the GtkTextBuffer.
    doc: Rc<RefCell<TabulaDocument>>,
    /// Set while the view applies a core change (or loads a file) so the
    /// GtkTextBuffer signals it raises are not echoed back into the core.
    guard: Rc<Cell<bool>>,
}

/// The byte offset in the core of a GtkTextIter. GTK and the core agree on
/// lines when their line counts agree (both end lines at LF, a CRLF being one
/// ending), so the iter's (line, byte-in-line) maps straight to a `Pos`; when
/// they do not (a lone CR, which GTK also breaks on), count the bytes before it.
#[cfg(feature = "gtk")]
fn iter_byte_offset(buf: &gtk4::TextBuffer, core: &edit_core::Buffer, it: &gtk4::TextIter) -> usize {
    if buf.line_count().max(0) as usize == core.line_count() {
        core.pos_to_offset(edit_core::Pos::new(it.line().max(0) as usize, it.line_index().max(0) as usize))
    } else {
        buf.text(&buf.start_iter(), it, true).len()
    }
}

#[cfg(feature = "gtk")]
impl TabulaView {
    pub fn new(mode: EditorMode) -> Self {
        let view = SourceView::builder().auto_indent(true).build();

        match &mode {
            EditorMode::Code(lang_id) => {
                view.set_monospace(true);
                view.set_show_line_numbers(true);
                view.set_wrap_mode(gtk4::WrapMode::None);

                let lm = LanguageManager::default();
                if let Some(lang) = lm.language(lang_id) {
                    if let Some(buffer) = view.buffer().downcast::<sourceview5::Buffer>().ok() {
                        buffer.set_language(Some(&lang));
                    }
                }
            }
            EditorMode::Prose => {
                view.set_monospace(false);
                view.set_show_line_numbers(false);
                view.set_wrap_mode(gtk4::WrapMode::WordChar);
                view.set_left_margin(12);
                view.set_right_margin(12);
                // Note: libspelling adapter can be re-attached here if needed.
            }
            EditorMode::Log => {
                view.set_monospace(true);
                view.set_show_line_numbers(false);
                view.set_editable(false);
                view.set_wrap_mode(gtk4::WrapMode::WordChar);
            }
        }

        let container = ScrolledWindow::builder()
            .child(&view)
            .hexpand(true)
            .vexpand(true)
            .build();

        // The mirror, GTK → core. Both handlers run BEFORE the default
        // handler, so the iters still describe the text the core holds.
        let doc = Rc::new(RefCell::new(TabulaDocument::new()));
        let guard = Rc::new(Cell::new(false));
        let buffer = view.buffer();
        {
            let (doc, guard) = (doc.clone(), guard.clone());
            buffer.connect_insert_text(move |buf, iter, text| {
                if guard.get() {
                    return;
                }
                let mut d = doc.borrow_mut();
                let at = iter_byte_offset(buf, &d.core, iter);
                d.replace_range(at..at, text);
            });
        }
        {
            let (doc, guard) = (doc.clone(), guard.clone());
            buffer.connect_delete_range(move |buf, start, end| {
                if guard.get() {
                    return;
                }
                let mut d = doc.borrow_mut();
                let a = iter_byte_offset(buf, &d.core, start);
                let b = iter_byte_offset(buf, &d.core, end);
                d.replace_range(a.min(b)..a.max(b), "");
            });
        }

        Self {
            view,
            container,
            mode,
            doc,
            guard,
        }
    }

    /// The document of record behind this view (shared with the mirror).
    pub fn document(&self) -> Rc<RefCell<TabulaDocument>> {
        self.doc.clone()
    }

    /// The mirror, core → GTK: run an edit on the buffer of record and replay
    /// its changes into the GtkTextBuffer, replacing only each changed range.
    pub fn edit(&self, f: impl FnOnce(&mut edit_core::Buffer) -> Vec<edit_core::Change>) -> Vec<edit_core::Change> {
        let changes = self.doc.borrow_mut().edit(f);
        self.replay(&changes);
        changes
    }

    fn replay(&self, changes: &[edit_core::Change]) {
        let buf = self.view.buffer();
        self.guard.set(true);
        let mut in_step = true;
        for c in changes {
            let s = buf.iter_at_line_index(c.start_pos.line as i32, c.start_pos.byte as i32);
            let e = buf.iter_at_line_index(c.old_end_pos.line as i32, c.old_end_pos.byte as i32);
            match (s, e) {
                (Some(mut s), Some(mut e)) => {
                    buf.delete(&mut s, &mut e);
                    buf.insert(&mut s, &c.text);
                }
                _ => {
                    in_step = false;
                    break;
                }
            }
        }
        let d = self.doc.borrow();
        if !in_step || buf.line_count().max(0) as usize != d.core.line_count() {
            // The view and the record disagree on structure: resync wholesale.
            buf.set_text(&d.text());
        }
        self.guard.set(false);
    }

    /// Show `doc` in the view and make it the record, without echoing the
    /// load back into the core as an edit.
    fn show_document(&self, doc: TabulaDocument) {
        let text = doc.text();
        *self.doc.borrow_mut() = doc;
        self.guard.set(true);
        self.view.buffer().set_text(&text);
        self.guard.set(false);
    }

    pub fn widget(&self) -> Widget {
        self.container.clone().upcast()
    }

    pub fn load_file(&self, path: &Path) {
        // Console/serial logs take the log path: NUL padding trimmed, escape
        // sequences stripped, control bytes made visible, tail-capped — and
        // the view goes read-only so the rendering can never be saved back
        // over the record.
        if crate::logview::is_log_path(path) {
            self.load_log(path);
            return;
        }

        let buffer = self
            .view
            .buffer()
            .downcast::<sourceview5::Buffer>()
            .unwrap();

        // Undo what a previous `load_log` did to this view: the Console
        // treatment is per-file, and a view that opened a log once must not
        // stay uneditable for every source file after it. A view built in
        // `EditorMode::Log` was uneditable to begin with and stays that way.
        if !matches!(self.mode, EditorMode::Log) {
            self.view.set_editable(true);
            self.view.set_wrap_mode(match self.mode {
                EditorMode::Prose => gtk4::WrapMode::WordChar,
                _ => gtk4::WrapMode::None,
            });
            self.view
                .set_monospace(!matches!(self.mode, EditorMode::Prose));
        }

        // Auto-detect language based on extension
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            let lm = LanguageManager::default();
            let lang_id = match ext {
                "rs" => "rust",
                "toml" => "toml",
                "md" => "markdown",
                "py" => "python",
                "js" | "ts" => "javascript",
                "json" => "json",
                "c" | "h" | "cpp" => "c",
                _ => "txt",
            };
            if let Some(lang) = lm.language(lang_id) {
                buffer.set_language(Some(&lang));
            } else {
                buffer.set_language(None::<&sourceview5::Language>);
            }
        }

        // EDITCORE: the file loads into the buffer of record and the view
        // shows it; a read failure becomes a clean, path-less document so the
        // view and the record still hold the same text.
        match TabulaDocument::load(path) {
            Ok(doc) => self.show_document(doc),
            Err(e) => self.show_document(TabulaDocument::with_text(&format!(
                "// UNAOS: FAILED TO LOAD {:?}\n// ERROR: {}",
                path, e
            ))),
        }
    }

    /// The Console view: render a console/serial log into this view,
    /// read-only, monospace, no highlighting.
    ///
    /// `load_file` routes here on its own for log paths; call it directly to
    /// force the log treatment on a file whose name does not say "log".
    pub fn load_log(&self, path: &Path) {
        let buffer = self
            .view
            .buffer()
            .downcast::<sourceview5::Buffer>()
            .unwrap();
        buffer.set_language(None::<&sourceview5::Language>);

        self.view.set_monospace(true);
        self.view.set_editable(false);
        self.view.set_wrap_mode(gtk4::WrapMode::WordChar);

        match TabulaDocument::load_log(path) {
            Ok(doc) => self.show_document(doc),
            Err(e) => {
                let mut doc =
                    TabulaDocument::with_text(&format!(":: TABULA: cannot read {:?} — {} ::\n", path, e));
                doc.read_only = true;
                self.show_document(doc);
            }
        }
    }
}
