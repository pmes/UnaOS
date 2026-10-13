// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VTT (AETHERMEDIA2, LEDGER SR134) — WebVTT (W3C WebVTT: The Web Video Text Tracks Format) for
//! `<track kind=subtitles|captions>`: the file parser (§6.1 "WebVTT parser algorithm": the `WEBVTT`
//! signature, blocks separated by blank lines, `NOTE` / `STYLE` / `REGION` blocks skipped, a cue's
//! optional identifier, its timing line `start --> end settings`, and its payload up to the next
//! blank line or a line holding `-->`) and the cue text's plain form for the UA's caption box
//! (§6.4 cue text parsing reduced to its text: tags dropped, the six character references decoded).
//!
//! CHARTER: Stria — shared-core (`no_std`; Aether's text track model and any Ring 0 player link it).

use alloc::string::String;
use alloc::vec::Vec;

/// One cue as the file states it. Times are nanoseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub id: String,
    pub start_ns: i64,
    pub end_ns: i64,
    /// The cue settings after the end time (`line:`, `position:`, `size:`, `align:`, `vertical:`,
    /// `region:`), as written.
    pub settings: String,
    /// The cue payload (cue text with its markup), lines joined by `\n`.
    pub text: String,
}

/// Why a file is not WebVTT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VttError {
    /// The first line is not `WEBVTT` (optionally followed by a space or tab and text).
    NoSignature,
}

/// `[hh:]mm:ss.ttt` → ns (§6.1 "collect a WebVTT timestamp").
pub fn timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    let (hms, ms) = s.split_once('.')?;
    if ms.len() != 3 || !ms.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let parts: Vec<&str> = hms.split(':').collect();
    let num = |p: &str| -> Option<i64> {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse::<i64>().ok()
    };
    let (h, m, sec) = match parts.as_slice() {
        [m, s] => {
            if m.len() != 2 {
                return None;
            }
            (0, num(m)?, num(s)?)
        }
        [h, m, s] => {
            if h.len() < 2 || m.len() != 2 {
                return None;
            }
            (num(h)?, num(m)?, num(s)?)
        }
        _ => return None,
    };
    if m > 59 || sec > 59 || parts.last().map(|p| p.len()) != Some(2) {
        return None;
    }
    let ms = num(ms)?;
    Some(((h * 3600 + m * 60 + sec) * 1000 + ms) * 1_000_000)
}

/// `start --> end settings` → (start, end, settings).
fn timing_line(line: &str) -> Option<(i64, i64, String)> {
    let (a, b) = line.split_once("-->")?;
    let start = timestamp(a)?;
    let b = b.trim_start();
    let (end_s, settings) = match b.find([' ', '\t']) {
        Some(i) => (&b[..i], b[i..].trim()),
        None => (b, ""),
    };
    let end = timestamp(end_s)?;
    Some((start, end, String::from(settings)))
}

/// Parse a WebVTT file. Cues whose timing line does not parse are skipped (§6.1: "bad cue").
pub fn parse(src: &str) -> Result<Vec<Cue>, VttError> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    // §6.1: CRLF and CR are line terminators too
    let norm: String = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = norm.split('\n').peekable();
    let first = lines.next().unwrap_or("");
    let ok = first == "WEBVTT" || first.strip_prefix("WEBVTT").is_some_and(|r| r.starts_with(' ') || r.starts_with('\t'));
    if !ok {
        return Err(VttError::NoSignature);
    }
    // the header runs to the first blank line
    for l in lines.by_ref() {
        if l.is_empty() {
            break;
        }
    }
    let mut cues = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    fn flush<'a>(block: &mut Vec<&'a str>, cues: &mut Vec<Cue>) {
        if block.is_empty() {
            return;
        }
        let head = block[0];
        if head.starts_with("NOTE") && (head.len() == 4 || head[4..].starts_with([' ', '\t'])) {
            block.clear();
            return;
        }
        let (id, t) = if head.contains("-->") { (String::new(), 0) } else { (String::from(head), 1) };
        if (head == "STYLE" || head == "REGION") && t == 1 && block.get(1).is_none_or(|l| !l.contains("-->")) {
            block.clear();
            return;
        }
        if let Some((start_ns, end_ns, settings)) = block.get(t).and_then(|l| timing_line(l)) {
            let mut text = String::new();
            for (i, l) in block[t + 1..].iter().enumerate() {
                if l.contains("-->") {
                    break;
                }
                if i > 0 {
                    text.push('\n');
                }
                text.push_str(l);
            }
            cues.push(Cue { id, start_ns, end_ns, settings, text });
        }
        block.clear();
    }
    for l in lines {
        if l.is_empty() {
            flush(&mut block, &mut cues);
        } else {
            block.push(l);
        }
    }
    flush(&mut block, &mut cues);
    Ok(cues)
}

/// The cue text as the caption box shows it: tags (`<b>`, `<i>`, `<u>`, `<c.x>`, `<v Name>`,
/// `<lang>`, `<ruby>`, `<rt>`, timestamps) dropped; `&amp;` `&lt;` `&gt;` `&nbsp;` `&lrm;` `&rlm;`
/// decoded.
pub fn plain_text(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut ent: Option<String> = None;
    for c in text.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
            }
            continue;
        }
        if let Some(e) = ent.as_mut() {
            if c == ';' {
                let r = match e.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "nbsp" => Some('\u{A0}'),
                    "lrm" => Some('\u{200E}'),
                    "rlm" => Some('\u{200F}'),
                    _ => None,
                };
                match r {
                    Some(r) => out.push(r),
                    None => {
                        out.push('&');
                        out.push_str(e);
                        out.push(';');
                    }
                }
                ent = None;
                continue;
            }
            if c.is_ascii_alphanumeric() && e.len() < 8 {
                e.push(c);
                continue;
            }
            out.push('&');
            out.push_str(e);
            ent = None;
        }
        match c {
            '<' => in_tag = true,
            '&' => ent = Some(String::new()),
            _ => out.push(c),
        }
    }
    if let Some(e) = ent {
        out.push('&');
        out.push_str(&e);
    }
    out
}
