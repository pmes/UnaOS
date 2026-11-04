// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), the host side of doc_core (VUGMAKEMACHINELIBFACE, LEDGER SR548): [`DirLib`], the ONE
//! directory lookup of a part's machine library; holds no document logic.
//!
//! doc_core opens no file (BP-vug-4): `machine <id> lib=<name>` is resolved through the caller's `doc_core::Lib`. Every
//! host reader (vug-view's open roads and its journal's re-read, tools/scan's `--against` / `--intent`) reads a part
//! with `doc_core::read_with(text, &DirLib::beside(part))`, so `<name>.vugmachine` is looked up in the part's own
//! directory and nowhere else. A name that is not a word of doc_core's id charset (ASCII alphanumerics, `_`, `-`) is
//! refused by name before any path is built, so `../x`, `a/b` or an empty name never leave the directory; a missing or
//! unreadable library resolves to None and doc_core names it `UnknownLib(name)` by line. VUGFACEJOURNALFAULTFIXMANY
//! (SR584): [`DirLib::machines`] lists the shelf (the names a `lib=` may take), for the face's NoMachine fix.

use doc_core::Lib;
use std::path::{Path, PathBuf};

/// The extension of a machine library document.
pub const EXT: &str = "vugmachine";

/// A part's library shelf: `<dir>/<name>.vugmachine`.
#[derive(Clone, Debug, PartialEq)]
pub struct DirLib {
    dir: PathBuf,
}

impl DirLib {
    /// The shelf at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> DirLib {
        DirLib { dir: dir.into() }
    }

    /// The shelf beside `part`: its parent directory (a bare file name is the current directory).
    pub fn beside(part: &Path) -> DirLib {
        match part.parent() {
            Some(p) if !p.as_os_str().is_empty() => DirLib::new(p),
            _ => DirLib::new("."),
        }
    }

    /// The directory looked in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file `name` names, or None when `name` is not a word of the id charset (a path escape refused by name).
    pub fn path(&self, name: &str) -> Option<PathBuf> {
        if !id_word(name) {
            return None;
        }
        Some(self.dir.join(format!("{name}.{EXT}")))
    }

    /// VUGFACEJOURNALFAULTFIXMANY (SR584): the shelf's library names: every `<name>.vugmachine` in the directory whose name
    /// is a word of the id charset, sorted; an unreadable directory lists none.
    pub fn machines(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut names: Vec<String> = rd
            .filter_map(|e| {
                let p = e.ok()?.path();
                if p.extension()?.to_str()? != EXT {
                    return None;
                }
                let n = p.file_stem()?.to_str()?;
                id_word(n).then(|| n.to_string())
            })
            .collect();
        names.sort();
        names
    }
}

impl Lib for DirLib {
    fn resolve(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path(name)?).ok()
    }
}

/// doc_core's id charset (lex.rs `valid_id`, private there: VUG-MAKE-MACHINELIB-IDPUB owed).
fn id_word(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_escape_name_has_no_path() {
        let l = DirLib::new("shelf");
        for n in ["../x", "a/b", "", ".", "..", "a\\b", "x.vugmachine"] {
            assert_eq!(l.path(n), None, "{n:?}");
            assert_eq!(l.resolve(n), None, "{n:?}");
        }
        assert_eq!(l.path("shop-mill_2"), Some(PathBuf::from("shelf").join("shop-mill_2.vugmachine")));
    }

    #[test]
    fn beside_is_the_parent_directory() {
        assert_eq!(DirLib::beside(Path::new("parts/a.vug")).dir(), Path::new("parts"));
        assert_eq!(DirLib::beside(Path::new("a.vug")).dir(), Path::new("."));
        assert_eq!(DirLib::beside(Path::new("/p/q/a.vug")).dir(), Path::new("/p/q"));
    }
}
