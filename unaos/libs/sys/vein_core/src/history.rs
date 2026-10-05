// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! LUMENUX M4 (rmbp-ledger B348) — a conversation as a FILE: one markdown file per conversation, appended
//! turn by turn as each completes, reloaded on launch. The file is the markdown the window renders, with one
//! HTML-comment marker line per turn (`<!-- lumen:user -->`, `<!-- lumen:assistant -->`,
//! `<!-- lumen:note -->`), so it reads as a document in any markdown viewer and parses back exactly.
//!
//! The layout (LUMENUX.md §M4): `.config/unaos/lumen/NNNN.md`, a path RELATIVE to the session home (the EL0
//! resolver starts a relative path at `/home/<user>`), 27 bytes — inside the 40-byte ring-3 `SYS_OPEN` cap
//! for every user name. NNNN is a sequence: ring 3 has no wall clock.

use crate::Out;

/// The directory, relative to the session home, and its two parents (created in order).
pub const DIRS: [&str; 3] = [".config", ".config/unaos", ".config/unaos/lumen"];
/// The directory the conversation files live in.
pub const DIR: &str = ".config/unaos/lumen";
/// Highest sequence number (four digits).
pub const SEQ_MAX: u32 = 9999;
/// The longest path [`path`] writes.
pub const PATH_LEN: usize = DIR.len() + 1 + 4 + 3;

const MARK: &[u8] = b"<!-- lumen:";

/// Who a turn is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    User,
    Assistant,
    Note,
}

impl Who {
    fn tag(self) -> &'static [u8] {
        match self {
            Who::User => b"user",
            Who::Assistant => b"assistant",
            Who::Note => b"note",
        }
    }
}

/// `.config/unaos/lumen/0003.md` into `buf`; the written length.
pub fn path(seq: u32, buf: &mut [u8; PATH_LEN]) -> usize {
    let seq = seq.min(SEQ_MAX);
    let d = DIR.as_bytes();
    buf[..d.len()].copy_from_slice(d);
    let mut i = d.len();
    buf[i] = b'/';
    i += 1;
    for k in (0..4).rev() {
        buf[i + 3 - k] = b'0' + ((seq / 10u32.pow(k as u32)) % 10) as u8;
    }
    i += 4;
    buf[i..i + 3].copy_from_slice(b".md");
    i + 3
}

/// The file's first lines: `<!-- lumen:conversation v1 seq=3 boot_ms=… -->` and a title heading.
pub fn header(seq: u32, boot_ms: u64, o: &mut Out<'_>) {
    o.put(b"<!-- lumen:conversation v1 seq=");
    o.dec(seq as u64);
    o.put(b" boot_ms=");
    o.dec(boot_ms);
    o.put(b" -->\n# Lumen conversation ");
    o.dec(seq as u64);
    o.put(b"\n\n");
}

/// One turn as appended: the marker line, the text (a line that would read as a marker is escaped with a
/// leading space, which [`parse`] removes), and a blank separator line.
pub fn turn(who: Who, text: &[u8], o: &mut Out<'_>) {
    o.put(MARK);
    o.put(who.tag());
    o.put(b" -->\n");
    let text = trim_nl(text);
    for l in text.split(|&c| c == b'\n') {
        if l.starts_with(MARK) || (l.first() == Some(&b' ') && l.trim_ascii_start().starts_with(MARK)) {
            o.put(b" ");
        }
        o.put(l);
        o.put(b"\n");
    }
    o.put(b"\n");
}

fn trim_nl(t: &[u8]) -> &[u8] {
    let mut e = t.len();
    while e > 0 && matches!(t[e - 1], b'\n' | b'\r') {
        e -= 1;
    }
    &t[..e]
}

/// What [`parse`] reports.
#[derive(Debug, PartialEq, Eq)]
pub enum Ev<'a> {
    /// A turn begins.
    Begin(Who),
    /// One line of the current turn's text (no `\n`), escape removed. Trailing blank lines of a turn are
    /// not reported.
    Line(&'a [u8]),
}

/// Parse a conversation file. Text before the first marker (the header) is skipped; unknown markers end
/// the current turn and are skipped.
pub fn parse<'a>(file: &'a [u8], on: &mut dyn FnMut(Ev<'a>)) {
    let mut in_turn = false;
    let mut blanks = 0usize;
    for l in file.split(|&c| c == b'\n') {
        let l = l.strip_suffix(b"\r").unwrap_or(l);
        if let Some(rest) = l.strip_prefix(MARK) {
            blanks = 0;
            in_turn = true;
            let who = if rest.starts_with(b"user ") {
                Who::User
            } else if rest.starts_with(b"assistant ") {
                Who::Assistant
            } else if rest.starts_with(b"note ") {
                Who::Note
            } else {
                in_turn = false;
                continue;
            };
            on(Ev::Begin(who));
            continue;
        }
        if !in_turn {
            continue;
        }
        if l.is_empty() {
            blanks += 1;
            continue;
        }
        for _ in 0..blanks {
            on(Ev::Line(b""));
        }
        blanks = 0;
        let l = if l.first() == Some(&b' ') && l[1..].trim_ascii_start().starts_with(MARK) { &l[1..] } else { l };
        on(Ev::Line(l));
    }
}

/// The first line of the first user turn (for `/list`), at most `max` bytes.
pub fn title(file: &[u8], max: usize) -> &[u8] {
    let mut want = false;
    for l in file.split(|&c| c == b'\n') {
        if l.starts_with(MARK) {
            want = l[MARK.len()..].starts_with(b"user ");
            continue;
        }
        if want && !l.trim_ascii().is_empty() {
            let l = l.trim_ascii();
            return &l[..l.len().min(max)];
        }
    }
    b""
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    #[test]
    fn path_fits_the_ring3_open_cap() {
        let mut b = [0u8; PATH_LEN];
        let n = path(3, &mut b);
        assert_eq!(&b[..n], b".config/unaos/lumen/0003.md");
        assert!(n <= 40, "SYS_OPEN caps a ring-3 path at 40 bytes");
        let n = path(123456, &mut b);
        assert_eq!(&b[..n], b".config/unaos/lumen/9999.md");
    }

    #[test]
    fn round_trip_with_markers_inside_text_and_blank_lines() {
        let mut buf = [0u8; 1024];
        let mut o = Out::new(&mut buf);
        header(7, 1234, &mut o);
        turn(Who::User, b"hi\n<!-- lumen:assistant -->\n", &mut o);
        turn(Who::Assistant, b"# Title\n\npara\n\n```\ncode\n```", &mut o);
        turn(Who::Note, b"cancelled", &mut o);
        let n = o.done().unwrap();
        let file = &buf[..n];
        let mut turns: Vec<(Who, String)> = Vec::new();
        parse(file, &mut |e| match e {
            Ev::Begin(w) => turns.push((w, String::new())),
            Ev::Line(l) => {
                let t = &mut turns.last_mut().unwrap().1;
                if !t.is_empty() || l.is_empty() {
                    t.push('\n');
                }
                t.push_str(core::str::from_utf8(l).unwrap());
            }
        });
        // a leading empty line is reported as an empty Line; the joiner above handles it
        let fix = |s: &String| String::from(s.trim_start_matches('\n'));
        assert_eq!(turns.len(), 3);
        assert_eq!((turns[0].0, fix(&turns[0].1)), (Who::User, "hi\n<!-- lumen:assistant -->".into()));
        assert_eq!((turns[1].0, fix(&turns[1].1)), (Who::Assistant, "# Title\n\npara\n\n```\ncode\n```".into()));
        assert_eq!((turns[2].0, fix(&turns[2].1)), (Who::Note, "cancelled".into()));
        assert_eq!(title(file, 40), b"hi");
        assert!(core::str::from_utf8(file).unwrap().starts_with("<!-- lumen:conversation v1 seq=7 boot_ms=1234 -->\n# Lumen conversation 7\n"));
    }
}
