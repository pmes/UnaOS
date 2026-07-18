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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// An in-memory editor document, decoupled from any UI toolkit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabulaDocument {
    /// Backing file, if this document has been loaded from / bound to one.
    pub path: Option<PathBuf>,
    /// The full text content of the buffer.
    pub buffer: String,
    /// sourceview-style language id (e.g. "rust", "toml", "txt").
    pub language: String,
    /// True when `buffer` has unsaved edits relative to disk.
    pub dirty: bool,
}

impl Default for TabulaDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl TabulaDocument {
    /// A fresh, empty, path-less document with the fallback language.
    pub fn new() -> Self {
        Self {
            path: None,
            buffer: String::new(),
            language: "txt".to_string(),
            dirty: false,
        }
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
        Ok(Self {
            path: Some(path.to_path_buf()),
            buffer,
            language,
            dirty: false,
        })
    }

    /// Replace the buffer contents, marking the document dirty.
    pub fn set_buffer(&mut self, content: impl Into<String>) {
        self.buffer = content.into();
        self.dirty = true;
    }

    /// Write the buffer to the bound path, clearing the dirty flag.
    /// Errors if the document has no path.
    pub fn save(&mut self) -> io::Result<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "TabulaDocument has no path to save to")
        })?;
        fs::write(path, self.buffer.as_bytes())?;
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
}
