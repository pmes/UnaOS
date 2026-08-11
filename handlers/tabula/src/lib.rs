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
use std::fs;
#[cfg(feature = "gtk")]
use std::path::Path;

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

        Self { view, container }
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

        match fs::read_to_string(path) {
            Ok(content) => buffer.set_text(&content),
            Err(e) => buffer.set_text(&format!(
                "// UNAOS: FAILED TO LOAD {:?}\n// ERROR: {}",
                path, e
            )),
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

        match crate::logview::load_log(path) {
            Ok(log) => buffer.set_text(&log.text),
            Err(e) => buffer.set_text(&format!(":: TABULA: cannot read {:?} — {} ::\n", path, e)),
        }
    }
}
