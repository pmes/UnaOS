// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! TERMCOLOR (R75) — the console's per-cell attributes and the escape-sequence parser.
//!
//! The view store keeps each line's PLAIN text in a `String` and, beside it, a run-length list of
//! [`Span`]s (`start_col`, fg, bg, bold) — at most [`SPANS_MAX`] per line, none for a plain line (an
//! empty `Vec` does not allocate). [`parse_line`] turns one ingested line, escapes and all, into that
//! pair. It understands SGR (`ESC [ … m`: 0, 1, 22, 30–37, 90–97, 40–47, 100–107, 39, 49, 38/48;5;n,
//! 38/48;2;r;g;b), erase-line (`K`), clear-screen (`2J`), home (`H`), cursor C/D as caret moves within
//! the line, `\r`, `\t`, `\b`; every other CSI/OSC/charset sequence is consumed silently.
use alloc::string::String;
use alloc::vec::Vec;

/// Spans kept per line; further attribute changes on the same line extend the last span.
pub const SPANS_MAX: usize = 16;
/// Colour word: `0` = the default; otherwise `SET | 0xRRGGBB`.
pub const SET: u32 = 0x0100_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Attr { pub fg: u32, pub bg: u32, pub bold: bool }
impl Attr {
    pub const DEFAULT: Attr = Attr { fg: 0, bg: 0, bold: false };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span { pub start: u16, pub fg: u32, pub bg: u32, pub bold: bool }

pub struct Parsed { pub text: String, pub spans: Vec<Span>, pub cleared: bool }

/// xterm 256-colour index -> RGB.
pub fn palette256(n: u32) -> u32 {
    let n = n.min(255) as usize;
    if n < 16 { return crate::video::theme::ANSI16[n]; }
    if n >= 232 { let v = (8 + (n as u32 - 232) * 10) & 0xFF; return v << 16 | v << 8 | v; }
    let i = (n - 16) as u32;
    let lv = |c: u32| if c == 0 { 0 } else { 55 + c * 40 };
    lv(i / 36) << 16 | lv((i / 6) % 6) << 8 | lv(i % 6)
}

/// A span's colour as paint colour (`def` for the default); bold lifts toward white.
pub fn resolve(word: u32, def: u32) -> u32 { if word & SET != 0 { word & 0x00FF_FFFF } else { def } }
pub fn lighten(c: u32) -> u32 {
    let f = |s: u32| { let v = (c >> s) & 0xFF; (v + (255 - v) / 3) << s };
    f(16) | f(8) | f(0)
}

/// The attribute in force at char column `col`.
pub fn attr_at(spans: &[Span], col: usize) -> Attr {
    let mut a = Attr::DEFAULT;
    for s in spans { if (s.start as usize) <= col { a = Attr { fg: s.fg, bg: s.bg, bold: s.bold }; } else { break; } }
    a
}

/// True when `text` needs the full parser (a control byte), or the pen is not default.
pub fn needs_parse(text: &str, pen: &Attr) -> bool { *pen != Attr::DEFAULT || text.bytes().any(|b| b < 0x20 || b == 0x7F) }

fn sgr(pen: &mut Attr, p: &[u32]) {
    let mut i = 0;
    while i < p.len() {
        let v = p[i];
        match v {
            0 => *pen = Attr::DEFAULT,
            1 => pen.bold = true,
            22 => pen.bold = false,
            30..=37 => pen.fg = SET | crate::video::theme::ANSI16[(v - 30) as usize],
            90..=97 => pen.fg = SET | crate::video::theme::ANSI16[(v - 90 + 8) as usize],
            40..=47 => pen.bg = SET | crate::video::theme::ANSI16[(v - 40) as usize],
            100..=107 => pen.bg = SET | crate::video::theme::ANSI16[(v - 100 + 8) as usize],
            39 => pen.fg = 0,
            49 => pen.bg = 0,
            38 | 48 => {
                let col = match p.get(i + 1) {
                    Some(5) => { let c = p.get(i + 2).copied().map(palette256); i += 2; c }
                    Some(2) => {
                        let c = if i + 4 < p.len() { Some((p[i + 2].min(255) << 16) | (p[i + 3].min(255) << 8) | p[i + 4].min(255)) } else { None };
                        i += 4; c
                    }
                    _ => None,
                };
                if let Some(c) = col { if v == 38 { pen.fg = SET | c } else { pen.bg = SET | c } }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Parse one line (no `\n`) against the running `pen`; the pen carries across lines like a terminal's.
pub fn parse_line(pen: &mut Attr, text: &str) -> Parsed {
    let mut cells: Vec<(char, Attr)> = Vec::with_capacity(text.len());
    let mut caret = 0usize;
    let mut cleared = false;
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.peek().copied() {
                Some('[') => {
                    it.next();
                    let mut params: Vec<u32> = Vec::new();
                    let (mut cur, mut have) = (0u32, false);
                    let mut fin = '\0';
                    while let Some(ch) = it.next() {
                        match ch {
                            '0'..='9' => { cur = cur.saturating_mul(10).saturating_add(ch as u32 - '0' as u32); have = true; }
                            ';' => { if params.len() < 16 { params.push(cur); } cur = 0; have = false; }
                            '\x40'..='\x7e' => { fin = ch; break; }
                            _ => {} // private markers ('?', '>') and intermediates
                        }
                    }
                    if have || !params.is_empty() { if params.len() < 16 { params.push(cur); } }
                    let n1 = params.first().copied().unwrap_or(0);
                    let n = (n1 as usize).max(1);
                    match fin {
                        'm' => { if params.is_empty() { *pen = Attr::DEFAULT } else { sgr(pen, &params) } }
                        'K' => match n1 {
                            0 => cells.truncate(caret),
                            1 => for k in 0..caret.min(cells.len()) { cells[k] = (' ', *pen); },
                            _ => cells.clear(),
                        },
                        'J' => if n1 >= 2 { cells.clear(); caret = 0; cleared = true; } else if n1 == 0 { cells.truncate(caret); },
                        'H' | 'f' => caret = params.get(1).copied().unwrap_or(1).max(1) as usize - 1,
                        'C' => caret = caret.saturating_add(n).min(4096),
                        'D' => caret = caret.saturating_sub(n),
                        'G' => caret = n - 1,
                        _ => {} // A/B/s/u/h/l/… — ignored, nothing printed
                    }
                }
                Some(']') => { // OSC: to BEL or ESC \
                    it.next();
                    while let Some(ch) = it.next() { if ch == '\x07' { break; } if ch == '\x1b' { it.next(); break; } }
                }
                Some('(') | Some(')') | Some('*') | Some('+') => { it.next(); it.next(); }
                Some(_) => { it.next(); }
                None => {}
            },
            '\r' => caret = 0,
            '\x08' => caret = caret.saturating_sub(1),
            '\t' => { let to = (caret / 8 + 1) * 8; while caret < to { put(&mut cells, caret, ' ', *pen); caret += 1; } }
            c if (c as u32) < 0x20 || c == '\x7f' => {}
            c => { put(&mut cells, caret, c, *pen); caret += 1; }
        }
    }
    let mut out = String::with_capacity(cells.len());
    let mut spans: Vec<Span> = Vec::new();
    let mut prev = Attr::DEFAULT;
    for (i, (ch, a)) in cells.iter().enumerate() {
        out.push(*ch);
        if *a != prev && spans.len() < SPANS_MAX {
            spans.push(Span { start: i.min(u16::MAX as usize) as u16, fg: a.fg, bg: a.bg, bold: a.bold });
            prev = *a;
        }
    }
    Parsed { text: out, spans, cleared }
}

fn put(cells: &mut Vec<(char, Attr)>, at: usize, c: char, a: Attr) {
    while cells.len() < at { cells.push((' ', Attr::DEFAULT)); }
    if at < cells.len() { cells[at] = (c, a); } else { cells.push((c, a)); }
}

/// `tests termcolor` — a line with four SGR changes must yield the exact span list; `ESC[2J` must
/// clear the live view; erase-line must truncate; the palette must span 16+256+rgb.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
pub fn selftest() {
    use alloc::format;
    let mut con = crate::console::Console::new();
    con.mark_in_window();
    con.place_for_fixture("\x1b[31mred\x1b[0m \x1b[1;32mgrn\x1b[0m \x1b[44mbg\x1b[0m \x1b[38;2;1;2;3mtc\x1b[0m end");
    let sp = con.spans_of(0);
    let txt_ok = con.row_text(con.hist_base_for_fixture()) == "red grn bg tc end";
    let sgr_ok = txt_ok && sp.len() == 8
        && sp[0] == Span { start: 0, fg: SET | 0xCD3131, bg: 0, bold: false }
        && sp[1].start == 3 && sp[1].fg == 0
        && sp[2] == Span { start: 4, fg: SET | 0x0DBC79, bg: 0, bold: true }
        && sp[4].start == 8 && sp[4].bg == SET | 0x2472C8
        && sp[6].start == 11 && sp[6].fg == SET | 0x010203;
    // Caret moves and erase-line: "abcdef" -> back 3, erase to end => "abc"; CR overwrite; garbage ignored.
    let mut pen = Attr::DEFAULT;
    let a = parse_line(&mut pen, "abcdef\x1b[3D\x1b[K").text == "abc";
    let b = parse_line(&mut pen, "hello\rHE\x1b[?25l\x1b[3A\x1b]0;t\x07\x1b(B!").text == "HE!lo";
    let c = parse_line(&mut pen, "x\x1b[1;1Hyz\x1b[1D\x1b[1K").text == " z";
    con.place_for_fixture("before");
    con.place_for_fixture("\x1b[2J");
    let cleared = con.live_rows() == 0;
    con.place_for_fixture("after");
    let erase_ok = a && b && c && cleared && con.live_rows() == 1 && con.spans_of(0).len() == 8;
    let long: String = (0..30).map(|i| format!("\x1b[{}m{}", 31 + i % 7, i % 10)).collect();
    let capped = parse_line(&mut Attr::DEFAULT, &long).spans.len() == SPANS_MAX;
    let pal_ok = capped && palette256(1) == crate::video::theme::ANSI16[1] && palette256(16) == 0 && palette256(231) == 0xFFFFFF && palette256(232) == 0x080808
        && parse_line(&mut Attr::DEFAULT, "\x1b[38;5;196mx").spans[0].fg == SET | 0xFF0000;
    let t = |b: bool| if b { "ok" } else { "bad" };
    crate::serial_println!(
        ":: TERMCOLOR: spans_max={} sgr_ok={} erase_ok={} palette={} -> {} ::",
        SPANS_MAX, t(sgr_ok), t(erase_ok), if pal_ok { "16+256+rgb" } else { "bad" }, if sgr_ok && erase_ok && pal_ok { "PASS" } else { "FAIL" }
    );
}
