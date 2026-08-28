//! `capture` — read a finished serial log as BYTES and sanitize it into lines.
//!
//! Knows nothing about specs or ports (design §5.1). The sanitization rule is
//! inherited verbatim from `unaos/scripts/mbench.py`: decode UTF-8 with
//! replacement, strip ANSI escapes and C0 control bytes (minus `\t`/`\n`).
//! Serial logs carry control bytes — the reason plain `grep` is banned on them.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// mbench's `ANSI_RE`, character-for-character.
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?|\x1b[@-_]")
        .expect("ANSI_RE is a compile-time constant")
});

/// mbench's `CTRL_RE`: C0 minus `\t`/`\n`; `\r` is handled at the split.
static CTRL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0b-\x1f\x7f]").expect("CTRL_RE is a compile-time constant"));

/// One sanitized line, with its 1-based position in the capture.
#[derive(Debug, Clone)]
pub struct Line {
    pub lineno: usize,
    pub text: String,
}

/// A finished capture, sanitized.
#[derive(Debug, Clone)]
pub struct Capture {
    pub path: PathBuf,
    pub lines: Vec<Line>,
    /// The capture ended MID-LINE (no terminating newline) — direct evidence the
    /// writer was killed in the middle of a write. Only consulted when the spec
    /// declares `COMPLETE` markers (see `verdict::Evaluation::truncated`).
    pub unterminated: bool,
}

impl Capture {
    /// The last non-blank line fed, reported verbatim on a TRUNCATED verdict.
    pub fn last_nonblank(&self) -> (usize, &str) {
        for line in self.lines.iter().rev() {
            if !line.text.trim().is_empty() {
                return (line.lineno, line.text.trim());
            }
        }
        (0, "")
    }
}

/// mbench's `clean_line`: bytes -> UTF-8-with-replacement -> strip ANSI -> strip C0.
pub fn clean_line(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = ANSI_RE.replace_all(&text, "");
    CTRL_RE.replace_all(&text, "").into_owned()
}

/// mbench's `split_lines`: split on `\n`, tolerating `\r\n` and bare `\r`.
/// Returns (complete lines, remainder).
pub fn split_lines(buf: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
    let mut norm: Vec<u8> = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\r' {
            // `\r\n` collapses to `\n`; a bare `\r` becomes `\n`.
            if i + 1 < buf.len() && buf[i + 1] == b'\n' {
                i += 1;
            }
            norm.push(b'\n');
        } else {
            norm.push(buf[i]);
        }
        i += 1;
    }
    if !norm.contains(&b'\n') {
        return (Vec::new(), norm);
    }
    let mut parts: Vec<Vec<u8>> = norm.split(|b| *b == b'\n').map(|s| s.to_vec()).collect();
    let rest = parts.pop().unwrap_or_default();
    (parts, rest)
}

/// Read + sanitize a finished log. No device is opened; this is a file read.
pub fn read(path: &Path) -> std::io::Result<Capture> {
    let data = std::fs::read(path)?;
    Ok(from_bytes(path, &data))
}

/// Sanitize an in-memory capture (used by the tests and by any caller that
/// already holds the bytes — the vessel's own capture store, later).
pub fn from_bytes(path: &Path, data: &[u8]) -> Capture {
    let (mut raw_lines, rest) = split_lines(data);
    let mut unterminated = false;
    if !rest.is_empty() {
        raw_lines.push(rest);
        unterminated = true;
    }
    let lines = raw_lines
        .into_iter()
        .enumerate()
        .map(|(i, raw)| Line { lineno: i + 1, text: clean_line(&raw) })
        .collect();
    Capture { path: path.to_path_buf(), lines, unterminated }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_and_control_bytes() {
        let raw = b"\x1b[2J\x1b[HUEFI firmware noise \x00\x01 garbage";
        assert_eq!(clean_line(raw), "UEFI firmware noise  garbage");
    }

    #[test]
    fn keeps_utf8_and_replaces_bad_bytes() {
        let raw = "\u{1b}[32mU4: process model \u{2014} reaped -> PASS\u{1b}[0m".as_bytes();
        assert_eq!(clean_line(raw), "U4: process model — reaped -> PASS");
    }

    #[test]
    fn split_tolerates_crlf_and_bare_cr() {
        let (lines, rest) = split_lines(b"a\r\nb\rc\nd");
        assert_eq!(lines.len(), 3);
        assert_eq!(rest, b"d".to_vec());
    }

    #[test]
    fn unterminated_capture_is_flagged() {
        let cap = from_bytes(Path::new("x"), b"one\r\ntwo\r\nhalf a li");
        assert!(cap.unterminated);
        assert_eq!(cap.lines.len(), 3);
        assert_eq!(cap.last_nonblank(), (3, "half a li"));
    }

    #[test]
    fn terminated_capture_is_not_flagged() {
        let cap = from_bytes(Path::new("x"), b"one\r\ntwo\r\n");
        assert!(!cap.unterminated);
        assert_eq!(cap.lines.len(), 2);
    }
}
