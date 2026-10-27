// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER (SAVEDIALOG, LEDGER SR364): the ONE host file writer: the bytes go to `<to>.part`, then the part is renamed
//! over `to`, so a half-written file never carries the real name. Every save lands through it: Aether's downloads and
//! its Save / Save As (`chrome::host`), and Facet's `export`. Ungated (std::fs only), so Facet's
//! `default-features = false` links it without the network face. Naming policy (a unique `(n)` name in a folder) is
//! the caller's: this module writes where it is told.

use std::io;
use std::path::{Path, PathBuf};

/// The sibling a landing file is written to first: `<to>.part`.
pub fn part(to: &Path) -> PathBuf {
    let mut name = to.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    to.with_file_name(name)
}

/// The complete [`part`] takes its real name (a file already at `to` is replaced).
pub fn land(to: &Path) -> io::Result<()> {
    std::fs::rename(part(to), to)
}

/// Write `bytes` to `to` through [`part`] and [`land`]; on any failure the part is removed and `to` is untouched.
pub fn save_bytes(to: &Path, bytes: &[u8]) -> io::Result<()> {
    let p = part(to);
    std::fs::write(&p, bytes).and_then(|()| land(to)).map_err(|e| {
        let _ = std::fs::remove_file(&p);
        e
    })
}

#[cfg(test)]
mod tests {
    use super::{part, save_bytes};

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gneiss-fs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// SAVEDIALOG (SR364): the writer writes, replaces, and leaves no `.part` behind.
    #[test]
    fn savedialog_fs_write_replace() {
        let dir = scratch("write");
        let to = dir.join("form.pdf");
        assert_eq!(part(&to), dir.join("form.pdf.part"));
        save_bytes(&to, b"one").unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"one");
        save_bytes(&to, b"two").unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"two", "an existing file is replaced");
        assert!(!part(&to).exists(), "no .part left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// SAVEDIALOG (SR364): a write into a folder that does not exist fails and leaves nothing.
    #[test]
    fn savedialog_fs_missing_folder() {
        let dir = scratch("missing");
        let to = dir.join("no-such-folder").join("x.png");
        assert!(save_bytes(&to, b"x").is_err());
        assert!(!to.exists() && !part(&to).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
