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
            read_only: false,
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
            read_only: false,
        })
    }

    /// Load a console/serial log for viewing: read-only, control-byte-safe,
    /// size-capped. See [`crate::logview`] for exactly what is done to the
    /// bytes and why.
    pub fn load_log(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let rendered = crate::logview::load_log(path)?;
        Ok(Self {
            path: Some(path.to_path_buf()),
            buffer: rendered.text,
            language: "txt".to_string(),
            dirty: false,
            read_only: true,
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
    /// Inert on a read-only document: the view may still be editable in the
    /// host toolkit, but a log's held state stays exactly what was read.
    pub fn set_buffer(&mut self, content: impl Into<String>) {
        if self.read_only {
            return;
        }
        self.buffer = content.into();
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
