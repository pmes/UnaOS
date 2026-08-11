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

//! Console-log reading for the editor: the "Console view" core.
//!
//! UnaOS logs are not ordinary text files. The kernel flight recorder writes
//! `UNAOS.LOG` into a **fixed-size** reservation on the FAT boot volume
//! (`unaos/crates/kernel/src/flight_recorder.rs`), so every such file is
//! `RESERVE_BYTES` long with the capture as its prefix and NUL padding after
//! it. Serial captures (`ttyUSB*.log`, squawk `*.out`) come straight off a
//! cable and can carry stray control bytes and — if a byte is mangled in
//! transit — sequences that are not valid UTF-8 at all. This is the same
//! hazard the "inspect with `awk`, not `grep`" law exists for.
//!
//! Three rules follow, and this module is all three:
//!
//! 1. **Never fail on bytes.** Read as bytes, decode lossily. A log with one
//!    corrupt byte still opens.
//! 2. **Never let a control byte wreck the view.** Trailing NUL padding is
//!    trimmed, ANSI/VT escape sequences are removed, CR/CRLF is normalised,
//!    and every remaining C0 control (plus DEL) is rendered as its Unicode
//!    Control Picture (`␀`, `␛`, …) so it is *visible* rather than either
//!    invisible or destructive.
//! 3. **Never hang on size.** The recorder reserves 256 KiB today and a
//!    long-running capture is unbounded; loads are capped and keep the
//!    **tail** (the newest lines), with an explicit banner saying so.
//!
//! The result is read-only by construction — see `TabulaDocument::read_only`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Default cap on how much of a log is pulled into the editor buffer.
///
/// 4 MiB is ~16 flight-recorder reservations and comfortably more than a
/// multi-hour bench capture's useful tail, while staying small enough that a
/// GTK text buffer takes it without a visible stall.
pub const DEFAULT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// A log file rendered for display, plus what had to be done to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogText {
    /// Display-safe text: no NUL padding, no escape sequences, no raw C0.
    pub text: String,
    /// Bytes dropped from the FRONT because the file exceeded the cap.
    pub elided_bytes: usize,
    /// Trailing NUL bytes trimmed (the flight recorder's fixed-size padding).
    pub padding_bytes: usize,
    /// C0 controls / DEL replaced with a Control Picture in `text`.
    pub controls_shown: usize,
    /// Escape sequences (ANSI CSI/OSC and friends) removed from `text`.
    pub escapes_stripped: usize,
    /// True when the bytes were not valid UTF-8 and were decoded lossily.
    pub lossy: bool,
}

/// Is this path a console/serial log rather than a source file?
///
/// Matches what actually lands on this bench: the flight recorder's
/// `UNAOS.LOG` (and archived `*.LOG.saved` copies), serial captures
/// (`ttyUSB0.log`), and squawk/waker `*.out` transcripts.
///
/// Matching is by dot-separated *component*, not substring: `x.logic.rs` and
/// `my.logrotate.conf` are source, not logs. A substring test on `".log"`
/// claims both, and this decision is not cosmetic — it sends the file to the
/// read-only Console view.
pub fn is_log_path(path: &Path) -> bool {
    let name = match path.file_name().and_then(|s| s.to_str()) {
        Some(n) => n.to_ascii_lowercase(),
        None => return false,
    };
    // Everything after the stem's first dot.
    let Some((_stem, exts)) = name.split_once('.') else {
        return false;
    };
    // `UNAOS.LOG`, `s73-UNAOS.LOG.saved`, `foo.log`, `foo.log.1`
    if exts.split('.').any(|p| p == "log") {
        return true;
    }
    // squawk / waker transcripts — final component only, so `checkout.txt`
    // and friends stay source.
    exts.rsplit('.').next() == Some("out")
}

/// Render raw log bytes as display-safe text.
///
/// Pure and allocation-only: no I/O, so it is the unit under test for every
/// control-byte rule. Tabs and newlines survive; nothing else in C0 does.
pub fn sanitize(bytes: &[u8]) -> LogText {
    // 1. Trailing NUL padding — the flight recorder's fixed-size reservation.
    let end = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    let padding_bytes = bytes.len() - end;
    let body = &bytes[..end];

    // 2. Lossy UTF-8: one mangled byte on the cable must not cost the file.
    let decoded = String::from_utf8_lossy(body);
    let lossy = matches!(decoded, std::borrow::Cow::Owned(_));

    let mut text = String::with_capacity(decoded.len());
    let mut controls_shown = 0usize;
    let mut escapes_stripped = 0usize;

    let chars: Vec<char> = decoded.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\u{1b}' => {
                escapes_stripped += 1;
                i = skip_escape(&chars, i);
            }
            // CRLF and lone CR both become one LF: a serial stream uses both.
            '\r' => {
                if chars.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
                text.push('\n');
                i += 1;
            }
            '\n' | '\t' => {
                text.push(c);
                i += 1;
            }
            // Remaining C0 controls and DEL: show them, do not obey them.
            c if (c as u32) < 0x20 => {
                controls_shown += 1;
                text.push(control_picture(c));
                i += 1;
            }
            '\u{7f}' => {
                controls_shown += 1;
                text.push('\u{2421}'); // ␡
                i += 1;
            }
            c => {
                text.push(c);
                i += 1;
            }
        }
    }

    LogText {
        text,
        elided_bytes: 0,
        padding_bytes,
        controls_shown,
        escapes_stripped,
        lossy,
    }
}

/// How far an escape sequence is allowed to run before the scanner gives up
/// on it. A real CSI is a handful of bytes and a real OSC is a title or a
/// colour query; anything longer is cable garbage, and letting it run is how a
/// viewer eats a log.
const MAX_ESCAPE_LEN: usize = 256;

/// Index just past the escape sequence starting at `i` (`chars[i]` is ESC).
///
/// **Bounded, always.** A stray `ESC ]` off the cable used to open an OSC
/// string that ran to end-of-file, silently deleting every line after it —
/// including a panic. So a sequence ends at its proper terminator, at the
/// first C0 control (a newline is never inside an escape sequence), or after
/// `MAX_ESCAPE_LEN` characters, whichever comes first; on either bail-out the
/// returned index points AT the offending character so it is rendered
/// normally rather than swallowed.
fn skip_escape(chars: &[char], i: usize) -> usize {
    debug_assert_eq!(chars[i], '\u{1b}');
    let limit = (i + MAX_ESCAPE_LEN).min(chars.len());
    // A control byte that cannot legally appear inside a sequence: stop here.
    let aborts = |c: char| (c as u32) < 0x20 && c != '\u{1b}' && c != '\u{7}';

    match chars.get(i + 1).copied() {
        // CSI: ESC '[' params.. final @..~
        Some('[') => {
            let mut j = i + 2;
            while j < limit {
                let c = chars[j];
                if aborts(c) {
                    return j;
                }
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    return j + 1;
                }
                j += 1;
            }
            j
        }
        // String sequences: OSC/DCS/SOS/PM/APC, terminated by BEL or ESC '\'.
        Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
            let mut j = i + 2;
            while j < limit {
                let c = chars[j];
                if c == '\u{7}' {
                    return j + 1;
                }
                if c == '\u{1b}' {
                    return if chars.get(j + 1) == Some(&'\\') { j + 2 } else { j };
                }
                if aborts(c) {
                    return j;
                }
                j += 1;
            }
            j
        }
        // ESC + one byte (RI, NEL, charset select, …).
        Some(_) => i + 2,
        None => i + 1,
    }
}

/// U+2400 CONTROL PICTURES maps 1:1 onto C0: `␀` is NUL, `␛` is ESC, …
fn control_picture(c: char) -> char {
    char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}')
}

/// Read a log file for display, capped at `max_bytes` and keeping the tail.
///
/// The cap applies to the *log*, not to the file: trailing NUL padding is
/// trimmed first, so a fixed-size reservation never spends the budget on its
/// own padding. When what remains is larger than the cap the kept region is
/// advanced to the next line break (so the view never opens mid-line, unless
/// that would leave nothing to show) and a banner naming the elided and kept
/// byte counts is prepended.
pub fn load_log_capped(path: impl AsRef<Path>, max_bytes: usize) -> io::Result<LogText> {
    let bytes = fs::read(path.as_ref())?;

    // Trim the reservation's trailing NUL padding BEFORE the cap, or the cap
    // spends its whole budget on padding: a flight-recorder file larger than
    // `max_bytes` whose tail is padding would elide the entire capture and
    // present an empty view.
    let end = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    let padding_bytes = bytes.len() - end;
    let body = &bytes[..end];

    let (mut slice, mut elided) = if body.len() > max_bytes {
        let cut = body.len() - max_bytes;
        (&body[cut..], cut)
    } else {
        (body, 0)
    };

    // Don't start mid-line — unless doing so would leave nothing to show. A
    // log whose only line break sits at the very end of the kept region (one
    // enormous line, as a hex dump or a stuck emitter produces) must still
    // open, mid-line, rather than render as a banner over an empty buffer.
    let advance = if elided == 0 {
        None
    } else {
        slice
            .iter()
            .position(|&b| b == b'\n')
            .filter(|&i| i + 1 < slice.len())
    };
    if let Some(i) = advance {
        elided += i + 1;
        slice = &slice[i + 1..];
    }
    let kept_bytes = slice.len();

    let mut out = sanitize(slice);
    // `sanitize` saw a padding-free slice; the padding was trimmed above.
    out.padding_bytes = padding_bytes;
    out.elided_bytes = elided;
    if elided > 0 {
        out.text.insert_str(
            0,
            &format!(
                ":: TABULA: log truncated for display — {} leading byte(s) elided, showing the last {} byte(s) of the log ::\n",
                elided, kept_bytes
            ),
        );
    }
    Ok(out)
}

/// `load_log_capped` at the default cap.
pub fn load_log(path: impl AsRef<Path>) -> io::Result<LogText> {
    load_log_capped(path, DEFAULT_MAX_BYTES)
}

/// Directories that plausibly hold a console log right now: every removable
/// mount point (a shard's FAT volume, once the card is in the host) plus the
/// bench capture tree.
pub fn console_log_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for base in ["/run/media", "/media"] {
        // /run/media/<user>/<volume>
        if let Ok(users) = fs::read_dir(base) {
            for user in users.flatten() {
                if let Ok(vols) = fs::read_dir(user.path()) {
                    for vol in vols.flatten() {
                        roots.push(vol.path());
                    }
                }
            }
        }
    }
    if let Ok(mnt) = fs::read_dir("/mnt") {
        for e in mnt.flatten() {
            roots.push(e.path());
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("unaos-bench/capture"));
    }
    roots
}

/// The newest console log found directly inside `roots`.
///
/// Split out from `console_log_roots` so it is testable against a scratch
/// directory instead of the machine's real mount table.
pub fn newest_log_in(roots: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if !is_log_path(&path) {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                best = Some((mtime, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// The console log to open when the operator asks for "the console" and names
/// no file: the newest log on any mounted volume or in the capture tree.
pub fn default_console_log() -> Option<PathBuf> {
    newest_log_in(&console_log_roots())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_paths_are_recognised() {
        assert!(is_log_path(Path::new("/run/media/pmes/UNAOS/UNAOS.LOG")));
        assert!(is_log_path(Path::new("s73-UNAOS.LOG.saved")));
        assert!(is_log_path(Path::new("/x/ttyUSB0.log")));
        assert!(is_log_path(Path::new("gr24-squawk.out")));
        assert!(!is_log_path(Path::new("src/lib.rs")));
        assert!(!is_log_path(Path::new("README.md")));
        assert!(!is_log_path(Path::new("/")));
    }

    /// Routing sends a file to a READ-ONLY view, so a substring match on
    /// ".log" is not a harmless over-reach: it locks source files.
    #[test]
    fn log_matching_is_by_component_not_substring() {
        assert!(!is_log_path(Path::new("x.logic.rs")));
        assert!(!is_log_path(Path::new("my.logrotate.conf")));
        assert!(!is_log_path(Path::new("changelog.md")));
        assert!(!is_log_path(Path::new("dialog.rs")));
        assert!(!is_log_path(Path::new("checkout.txt")));
        assert!(!is_log_path(Path::new("logbook")));
        // …while the real shapes still match.
        assert!(is_log_path(Path::new("UNAOS.LOG")));
        assert!(is_log_path(Path::new("ttyUSB0.log.1")));
        assert!(is_log_path(Path::new("gr25-bootC.out")));
    }

    #[test]
    fn trailing_nul_padding_is_trimmed() {
        let mut raw = b":: line ::\n".to_vec();
        raw.extend(std::iter::repeat_n(0u8, 4096));
        let out = sanitize(&raw);
        assert_eq!(out.text, ":: line ::\n");
        assert_eq!(out.padding_bytes, 4096);
        assert_eq!(out.controls_shown, 0);
    }

    #[test]
    fn interior_control_bytes_are_shown_not_obeyed() {
        let out = sanitize(b"a\x00b\x07c\x08d\n");
        assert_eq!(out.text, "a\u{2400}b\u{2407}c\u{2408}d\n");
        assert_eq!(out.controls_shown, 3);
        // Nothing was dropped and nothing was executed.
        assert!(!out.text.contains('\u{0}'));
    }

    #[test]
    fn tabs_and_newlines_survive() {
        let out = sanitize(b"a\tb\nc\n");
        assert_eq!(out.text, "a\tb\nc\n");
        assert_eq!(out.controls_shown, 0);
    }

    #[test]
    fn crlf_and_lone_cr_normalise_to_lf() {
        let out = sanitize(b"a\r\nb\rc\n");
        assert_eq!(out.text, "a\nb\nc\n");
        assert_eq!(out.controls_shown, 0);
    }

    #[test]
    fn ansi_escapes_are_stripped() {
        // CSI colour, CSI erase-line, OSC title, and a bare two-byte escape.
        let out = sanitize(b"\x1b[31mred\x1b[0m\x1b[2Kx\x1b]0;title\x07y\x1bMz\n");
        assert_eq!(out.text, "redxyz\n");
        assert_eq!(out.escapes_stripped, 5);
        assert_eq!(out.controls_shown, 0);
    }

    #[test]
    fn truncated_escape_at_end_of_buffer_does_not_panic() {
        assert_eq!(sanitize(b"tail\x1b").text, "tail");
        assert_eq!(sanitize(b"tail\x1b[").text, "tail");
        assert_eq!(sanitize(b"tail\x1b]0;no-terminator").text, "tail");
    }

    /// A stray `ESC ]` off the cable opens an OSC string. If the scanner
    /// honours it to end-of-file, every line after that byte disappears from
    /// the view — a panic included — with nothing to say so.
    #[test]
    fn an_unterminated_osc_cannot_eat_the_rest_of_the_log() {
        let mut raw = b"line one\n".to_vec();
        raw.extend_from_slice(b"\x1b]");
        raw.extend_from_slice(b"line two\nline three\n:: PANIC ::\n");
        let out = sanitize(&raw);
        assert!(out.text.contains(":: PANIC ::"), "OSC ate the log: {:?}", out.text);
        // The sequence ends at the newline; the rest of that line is title
        // payload and is consumed, but nothing beyond it is.
        assert!(out.text.starts_with("line one\n"));
        assert!(out.text.contains("line three"));
    }

    /// Same hazard for a CSI whose final byte never arrives.
    #[test]
    fn an_unterminated_csi_stops_at_the_line_break() {
        let out = sanitize(b"a\n\x1b[38;2;1;2;3\nb\nc\n");
        assert_eq!(out.text, "a\n\nb\nc\n");
    }

    /// And for a sequence that is simply longer than any real one.
    #[test]
    fn an_overlong_escape_gives_up_and_resumes() {
        let mut raw = b"\x1b]".to_vec();
        raw.extend(std::iter::repeat_n(b'A', MAX_ESCAPE_LEN * 2));
        raw.extend_from_slice(b"\ntail\n");
        let out = sanitize(&raw);
        assert!(out.text.ends_with("tail\n"), "{:?}", out.text);
    }

    #[test]
    fn invalid_utf8_decodes_lossily() {
        let out = sanitize(b"ok \xff\xfe bad\n");
        assert!(out.lossy);
        assert!(out.text.starts_with("ok "));
        assert!(out.text.ends_with(" bad\n"));
    }

    #[test]
    fn valid_utf8_is_not_flagged_lossy() {
        let out = sanitize("⚡ kernel features: wc\n".as_bytes());
        assert!(!out.lossy);
        assert_eq!(out.text, "⚡ kernel features: wc\n");
    }

    #[test]
    fn empty_and_all_nul_inputs_are_safe() {
        assert_eq!(sanitize(b"").text, "");
        let out = sanitize(&[0u8; 32]);
        assert_eq!(out.text, "");
        assert_eq!(out.padding_bytes, 32);
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tabula_log_{}_{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn oversized_log_keeps_the_tail_and_says_so() {
        let dir = scratch_dir("cap");
        let path = dir.join("big.log");
        let mut body = String::new();
        for i in 0..2000 {
            body.push_str(&format!("[{:6}ms] line {}\n", i, i));
        }
        fs::write(&path, &body).unwrap();

        let out = load_log_capped(&path, 4096).unwrap();
        assert!(out.elided_bytes > 0);
        assert!(out.text.starts_with(":: TABULA: log truncated for display"));
        // The last line of the file is present; the first is not.
        assert!(out.text.contains("line 1999"));
        assert!(!out.text.contains("line 0\n"));
        // No partial first line: after the banner, the next line starts with '['.
        let after_banner = out.text.split_once('\n').unwrap().1;
        assert!(after_banner.starts_with('['), "kept region opened mid-line: {:?}", &after_banner[..40]);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The line-break advance must never be allowed to consume the whole kept
    /// region: one enormous line (a hex dump, a stuck emitter) whose only
    /// newline lands at the end used to render as a banner over an empty
    /// buffer, claiming "showing the last 0".
    #[test]
    fn a_single_enormous_line_still_shows_something() {
        let dir = scratch_dir("oneline");
        let path = dir.join("huge.log");
        let mut body = vec![b'x'; 8191];
        body.push(b'\n');
        fs::write(&path, &body).unwrap();

        let out = load_log_capped(&path, 4096).unwrap();
        let after_banner = out.text.split_once('\n').unwrap().1;
        assert!(!after_banner.is_empty(), "the view is empty: {:?}", out.text);
        // The kept region is the last 4096 bytes: 4095 x's and the newline.
        assert_eq!(after_banner, format!("{}\n", "x".repeat(4095)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The cap is on the log, not on the file: a fixed-size reservation whose
    /// padding exceeds the cap must not push its own capture out of the view.
    #[test]
    fn padding_is_trimmed_before_the_cap_is_applied() {
        let dir = scratch_dir("padcap");
        let path = dir.join("UNAOS.LOG");
        let mut body = b"[     1ms] the only real line\n".to_vec();
        let real = body.len();
        body.resize(20_000, 0u8);
        fs::write(&path, &body).unwrap();

        // Cap well under the file size but well over the capture size.
        let out = load_log_capped(&path, 4096).unwrap();
        assert_eq!(out.elided_bytes, 0, "padding was counted as elided log");
        assert_eq!(out.padding_bytes, 20_000 - real);
        assert_eq!(out.text, "[     1ms] the only real line\n");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A file exactly at the cap is not truncated (off-by-one at the boundary).
    #[test]
    fn a_file_exactly_at_the_cap_is_whole() {
        let dir = scratch_dir("exact");
        let path = dir.join("exact.log");
        let mut body = vec![b'y'; 4095];
        body.push(b'\n');
        fs::write(&path, &body).unwrap();

        let out = load_log_capped(&path, 4096).unwrap();
        assert_eq!(out.elided_bytes, 0);
        assert!(!out.text.starts_with(":: TABULA:"));
        assert_eq!(out.text.len(), 4096);

        // One byte over, and it truncates.
        fs::write(&path, [&body[..], b"z\n"].concat()).unwrap();
        assert!(load_log_capped(&path, 4096).unwrap().elided_bytes > 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// An empty file and an all-NUL file both open, and neither gets a banner.
    #[test]
    fn empty_and_all_nul_files_open_clean() {
        let dir = scratch_dir("empties");
        let empty = dir.join("empty.log");
        fs::write(&empty, b"").unwrap();
        let out = load_log_capped(&empty, 4096).unwrap();
        assert_eq!(out.text, "");
        assert_eq!(out.elided_bytes, 0);

        let nuls = dir.join("nuls.log");
        fs::write(&nuls, [0u8; 20_000]).unwrap();
        let out = load_log_capped(&nuls, 4096).unwrap();
        assert_eq!(out.text, "");
        assert_eq!(out.padding_bytes, 20_000);
        assert_eq!(out.elided_bytes, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn under_cap_log_is_untouched_and_unbannered() {
        let dir = scratch_dir("small");
        let path = dir.join("small.log");
        fs::write(&path, b"[     1ms] hello\n").unwrap();
        let out = load_log(&path).unwrap();
        assert_eq!(out.elided_bytes, 0);
        assert_eq!(out.text, "[     1ms] hello\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn newest_log_in_picks_the_freshest_log_and_ignores_source() {
        let dir = scratch_dir("newest");
        fs::write(dir.join("old.log"), b"old\n").unwrap();
        fs::write(dir.join("notes.rs"), b"fn main(){}\n").unwrap();
        // Ensure a distinct, later mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.join("UNAOS.LOG"), b"new\n").unwrap();

        let found = newest_log_in(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(found.file_name().unwrap(), "UNAOS.LOG");

        // A root that does not exist is skipped, not an error.
        assert!(newest_log_in(&[dir.join("nope")]).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
