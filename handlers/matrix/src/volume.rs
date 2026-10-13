// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! MATRIXCORE (SR99): the host's [`matrix_core::Volume`] — `std::fs` under an anchored root. The core's paths
//! are `/`-joined and relative to `root` (`""` = the root itself). Entries are classified by their OWN file
//! type (`DirEntry::file_type` / `symlink_metadata`, which never traverse a link), so the core's never-follow
//! law holds on the host exactly as the genesis scan and the Finder always kept it.

use std::path::{Path, PathBuf};

use matrix_core::{Entry, Kind, Stat, Volume};

pub struct StdVolume {
    root: PathBuf,
}

impl StdVolume {
    pub fn new(root: impl Into<PathBuf>) -> StdVolume {
        StdVolume { root: root.into() }
    }

    /// The absolute host path of core path `rel`.
    pub fn abs(&self, rel: &str) -> PathBuf {
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() { self.root.clone() } else { self.root.join(rel) }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn kind_of(ft: std::fs::FileType) -> Kind {
    if ft.is_symlink() {
        Kind::Symlink
    } else if ft.is_dir() {
        Kind::Dir
    } else if ft.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

impl Volume for StdVolume {
    type Error = std::io::Error;

    /// `read_dir` order (unsorted — the core orders); an entry whose file type cannot be read is skipped (the
    /// host scans always did); a file's size from `DirEntry::metadata` (which does not traverse a link). Names
    /// that are not UTF-8 arrive lossily converted.
    fn list(&mut self, dir: &str) -> std::io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(self.abs(dir))?.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let kind = kind_of(ft);
            let size = if kind == Kind::File { entry.metadata().map(|m| m.len()).unwrap_or(0) } else { 0 };
            out.push(Entry { name: entry.file_name().to_string_lossy().into_owned(), kind, size });
        }
        Ok(out)
    }

    fn stat(&mut self, path: &str) -> Option<Stat> {
        let m = std::fs::symlink_metadata(self.abs(path)).ok()?;
        let kind = kind_of(m.file_type());
        Some(Stat { kind, size: if kind == Kind::File { m.len() } else { 0 } })
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.abs(path)).ok()
    }
}
