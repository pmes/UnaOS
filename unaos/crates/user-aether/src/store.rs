// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Aether — shared-core (the person's browser files over the kernel's whole-path verbs; no store of its own)
//
// AETHERMETAL4 (LEDGER SR109): where the chrome's files live on the image, and the one write path to them.
//
// * WHO. `SYS_WHOAMI` (RING3ABI2 B333) names the caller's home (`/home/<user>`); with no session (a kernel fixture's
//   spawn, an anonymous launch: -ENOENT) nothing is persisted and the chrome runs from memory — the start line says
//   so (`store home=none code=<errno>`).
// * WHERE. The paths the host chrome uses (handlers/aether/src/chrome/host.rs): Aether's data directory is
//   `ProjectDirs("", "UnaOS", "Aether").data_dir()`, which on a Unix host is `$XDG_DATA_HOME/aether` =
//   `<home>/.local/share/aether`; the bookmarks are `bookmarks.tsv` there (the model's format, `URL<TAB>title`), and
//   the visit history beside it is `history.tsv` in the same format (oldest first). Downloads land in
//   `<home>/Desktop` (the brief's folder on the image: the desktop shows it). The home is HOMEPRIV-owned (B528), so
//   the kernel creates every file and directory below it as the caller — no grant is asked for.
// * HOW. `SYS_PATH_WRITE` (60, una-abi's SELFDIAG request layout, dispatched AS THE CALLER under the volume's write
//   ACL): the first chunk with `PATH_W_TRUNC | PATH_W_MKDIRS` (create-or-truncate, missing parents made), every next
//   chunk at its offset (`offset <= size`), at most `PATH_IO_MAX` bytes per call. Reads are `sys::read_file`
//   (SYS_PATH_READ), the same verb the page and the faces come through.

use alloc::string::String;
use alloc::vec::Vec;
use una_abi::{PATH_IO_HDR_LEN, PATH_IO_MAX, PATH_IO_PATH_MAX, PATH_W_MKDIRS, PATH_W_TRUNC, SYS_PATH_WRITE, SYS_WHOAMI};

use crate::sys::{read_file, sys};

/// The data directory below the home (the host's XDG data dir for `ProjectDirs("", "UnaOS", "Aether")`).
pub const DATA_DIR: &str = "/.local/share/aether";
/// The bookmarks file in it (host.rs `bookmarks_path`).
pub const BOOKMARKS: &str = "/bookmarks.tsv";
/// The visit history beside it.
pub const HISTORY: &str = "/history.tsv";
/// Where a download lands below the home.
pub const DOWNLOADS: &str = "/Desktop";
/// The most a chrome file is read (bookmarks, history).
const FILE_MAX: usize = 256 * 1024;

/// The caller's home, or the refusal (`-ENOENT`: no session).
pub fn home() -> Result<String, i64> {
    let mut b = [0u8; una_abi::WHOAMI_MAX];
    let n = sys(SYS_WHOAMI, b.as_mut_ptr() as u64, b.len() as u64, 0, 0);
    if n < 0 {
        return Err(n);
    }
    let w = una_abi::whoami_parse(&b[..(n as usize).min(b.len())]).ok_or(una_abi::EINVAL)?;
    if w.home.is_empty() || w.home[0] != b'/' {
        return Err(una_abi::ENOENT);
    }
    Ok(String::from_utf8_lossy(w.home).into_owned())
}

/// A whole file as text (`Err` the refusal; an absent file is `-ENOENT`).
pub fn read_text(path: &str) -> Result<String, i64> {
    read_file(path.as_bytes(), FILE_MAX).map(|(v, _)| String::from_utf8_lossy(&v).into_owned())
}

/// Whether `path` names something readable (the downloads' unique-name probe).
pub fn exists(path: &str) -> bool {
    read_file(path.as_bytes(), 1).is_ok()
}

/// Write `data` as the whole of `path` (created with its parents, or truncated): bytes written, or the first
/// refusal.
pub fn write(path: &str, data: &[u8]) -> i64 {
    if path.is_empty() || path.len() > PATH_IO_PATH_MAX {
        return una_abi::EINVAL;
    }
    let mut req: Vec<u8> = Vec::new();
    if req.try_reserve(PATH_IO_HDR_LEN + path.len() + PATH_IO_MAX.min(data.len())).is_err() {
        return una_abi::ENOMEM;
    }
    let mut off = 0usize;
    loop {
        let chunk = &data[off..data.len().min(off + PATH_IO_MAX)];
        let flags: u16 = if off == 0 { PATH_W_TRUNC | PATH_W_MKDIRS } else { 0 };
        req.clear();
        req.extend_from_slice(&(path.len() as u16).to_le_bytes());
        req.extend_from_slice(&flags.to_le_bytes());
        req.extend_from_slice(&[0u8; 4]);
        req.extend_from_slice(&(off as u64).to_le_bytes());
        req.extend_from_slice(path.as_bytes());
        req.extend_from_slice(chunk);
        let r = sys(SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0);
        if r < 0 {
            return r;
        }
        if r as usize != chunk.len() {
            return una_abi::EIO;
        }
        off += chunk.len();
        if off >= data.len() {
            return off as i64;
        }
    }
}

/// The visit history file's format — the bookmarks file's (`URL<TAB>title`, `#` comments), oldest first, one line
/// per URL (a revisit moves it to the end), at most [`VISITS_MAX`] lines.
pub const VISITS_MAX: usize = 200;
const VISITS_HEADER: &str = "# Aether history: one visit per line, URL<TAB>title, oldest first\n";

/// Parse the history file.
pub fn visits_parse(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (url, title) = line.split_once('\t').unwrap_or((line, ""));
        let url = url.trim();
        if url.is_empty() {
            continue;
        }
        out.retain(|(u, _)| u != url);
        out.push((String::from(url), String::from(title.trim())));
    }
    let n = out.len();
    if n > VISITS_MAX {
        out.drain(..n - VISITS_MAX);
    }
    out
}

/// The history file's text.
pub fn visits_serialize(v: &[(String, String)]) -> String {
    let clean = |s: &str| -> String { s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect() };
    let mut out = String::from(VISITS_HEADER);
    for (u, t) in v {
        out.push_str(&clean(u));
        out.push('\t');
        out.push_str(&clean(t));
        out.push('\n');
    }
    out
}

/// Record a visit (a revisit moves to the end; the oldest falls off past [`VISITS_MAX`]).
pub fn visit(v: &mut Vec<(String, String)>, url: &str, title: &str) {
    v.retain(|(u, _)| u != url);
    v.push((String::from(url), String::from(title)));
    let n = v.len();
    if n > VISITS_MAX {
        v.drain(..n - VISITS_MAX);
    }
}
