// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The text EDL (NLE1, LEDGER SR194): **CMX 3600**, non-drop frame — the edit decision list every editor exports.
//!
//! ```text
//! TITLE: CUT ONE
//! FCM: NON-DROP FRAME
//! * UNA FPS 25
//! * UNA SOURCE A 00:00:02:00
//! * UNA SOURCE B 00:00:02:00
//!
//! 001  A        V     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10
//! 002  B        V2    C        00:00:00:00 00:00:00:10 00:00:00:10 00:00:00:20
//! 003  A        A     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10
//! ```
//!
//! One event per line: number, reel (at most 8 characters), channel, transition, source in, source out, record in,
//! record out (`HH:MM:SS:FF`, out points exclusive). Channels: `V` (V1), `V2`..`V9`, `A` (A1), `A2`..`A9` (`V1` and
//! `A1` read too). Lines starting `*` are comments; two carry what CMX has no field for, and other readers skip them:
//! `* UNA FPS <n>` (the frame rate; else the caller's) and `* UNA SOURCE <reel> <duration>` (a reel's length; else the
//! furthest source out point that cites it). Reel `BL` (black) is a gap and makes no clip.
//!
//! NLE4 (LEDGER SR203): two more belong to the event line just above them, written only when not the default:
//! `* UNA GAIN <centi-dB>` (the clip's audio gain; absent = 0) and `* UNA FADE <in> <out>` (fade lengths as
//! `HH:MM:SS:FF`; absent = none). One with no event above it (or under a `BL` event) is a syntax error.
//!
//! Refused, each by name: `FCM: DROP FRAME` or a `;` timecode (drop-frame), any other FCM, a transition other than
//! `C`, `D` or a `W<nnn>` that [`WipeDir::from_code`] names (or one not shaped as below), an unknown channel, a record span unequal to its source span,
//! a malformed timecode, and any line that is none of the above. Tracks come back canonical: V1..Vn, then A1..An.
//!
//! Transitions (NLE3, LEDGER SR202) are CMX 3600 two-line events with one event number: the outgoing line (its reel,
//! `C`, source in = out = the outgoing clip's out point, record in = out = the cut), then the incoming clip's line
//! with `D` (dissolve) or `W<nnn>` (wipe) and the duration in frames. The transition starts at the cut:
//!
//! ```text
//! 002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20
//! 002  B        V     D    009 00:00:00:00 00:00:00:20 00:00:00:20 00:00:01:15
//! ```
//!
//! NLE3c (LEDGER SR209): wipe codes `W041` (diagonal), `W101` (box) and `W201` (clock) read and write too, and the
//! outgoing line may be reel `BL` (source in = out = `00:00:00:00`): a transition FROM black
//! ([`crate::Transition::from_black`]). A zero-length `BL` line with no `D` / `W` line after it is a gap. NLE3cBLACK
//! (LEDGER SR323): `BL` as the incoming line is a transition TO black ([`crate::Transition::to_black`]; record span
//! `at..at + dur`, source from `00:00:00:00`, no clip; `BL` on both lines is refused); NLE3cBLACK's wipe codes too.
//!
//! ```text
//! 001  BL       V     C        00:00:00:00 00:00:00:00 00:00:00:00 00:00:00:00
//! 001  A        V     D    010 00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20
//! ```
//!
//! [`write`] produces exactly what [`parse`] reads back: `write(parse(write(tl)))` is byte-identical.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{Clip, EditError, Kind, Timeline, Transition, TransitionKind, WipeDir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdlError {
    /// Drop-frame timecode (`FCM: DROP FRAME` or a `;` separator): only non-drop frame is read.
    DropFrame { line: usize },
    /// An FCM other than `NON-DROP FRAME`.
    Fcm { line: usize },
    /// A transition other than `C`, `D`, a `W<nnn>` `WipeDir::from_code` names, a `D`/`W` line without its outgoing line, or (write: line 0)
    /// a duration past 999 frames.
    Transition { line: usize },
    /// An unknown channel (parse), or a track past V9 / A9 (write: line 0).
    Channel { line: usize },
    /// A malformed timecode (or a field past its range).
    Timecode { line: usize },
    /// Record out - record in differs from source out - source in.
    Duration { line: usize },
    /// A line that is no header, comment or event.
    Syntax { line: usize },
    /// The timeline's time base is not `1/fps` (write), or the FPS comment is malformed or zero (parse).
    Fps,
    /// A reel name is empty, longer than 8 characters or holds whitespace (write).
    Reel,
    /// The events break a timeline invariant.
    Edit(EditError),
}

impl core::fmt::Display for EdlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EdlError::DropFrame { line } => write!(f, "line {line}: drop-frame timecode is not supported (non-drop frame only)"),
            EdlError::Fcm { line } => write!(f, "line {line}: unknown FCM (only NON-DROP FRAME)"),
            EdlError::Transition { line } => write!(f, "line {line}: transition is not a cut, a dissolve or a known SMPTE W<nnn> wipe event"),
            EdlError::Channel { line } => write!(f, "line {line}: unknown channel"),
            EdlError::Timecode { line } => write!(f, "line {line}: bad timecode"),
            EdlError::Duration { line } => write!(f, "line {line}: record span differs from source span"),
            EdlError::Syntax { line } => write!(f, "line {line}: not an EDL line"),
            EdlError::Fps => f.write_str("frame rate is not an integer fps"),
            EdlError::Reel => f.write_str("reel name is not CMX (1-8 characters, no whitespace)"),
            EdlError::Edit(e) => write!(f, "invalid timeline: {e}"),
        }
    }
}

/// `HH:MM:SS:FF` for tick `t` at `fps`.
pub fn timecode(t: i64, fps: u32) -> String {
    let fps = fps.max(1) as i64;
    let t = t.max(0);
    let (ff, s) = (t % fps, t / fps);
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, ff)
}

/// Ticks for `HH:MM:SS:FF` at `fps`.
fn parse_tc(s: &str, fps: u32, line: usize) -> Result<i64, EdlError> {
    if s.contains(';') {
        return Err(EdlError::DropFrame { line });
    }
    let p: Vec<&str> = s.split(':').collect();
    if p.len() != 4 || p.iter().any(|x| x.is_empty() || !x.bytes().all(|b| b.is_ascii_digit()) || x.len() > 4) {
        return Err(EdlError::Timecode { line });
    }
    let n: Vec<i64> = p.iter().map(|x| x.parse::<i64>().unwrap_or(-1)).collect();
    if n[1] >= 60 || n[2] >= 60 || n[3] >= fps as i64 || n.iter().any(|&x| x < 0) {
        return Err(EdlError::Timecode { line });
    }
    Ok(((n[0] * 60 + n[1]) * 60 + n[2]) * fps as i64 + n[3])
}

fn channel(s: &str) -> Option<(Kind, usize)> {
    let (k, rest) = match s.as_bytes().first()? {
        b'V' => (Kind::Video, &s[1..]),
        b'A' => (Kind::Audio, &s[1..]),
        _ => return None,
    };
    let n = match rest {
        "" => 1,
        r if r.len() == 1 && (b'1'..=b'9').contains(&r.as_bytes()[0]) => (r.as_bytes()[0] - b'0') as usize,
        _ => return None,
    };
    Some((k, n))
}

fn reel_ok(r: &str) -> bool {
    !r.is_empty() && r.len() <= 8 && !r.chars().any(|c| c.is_whitespace())
}

/// Read a CMX 3600 EDL. `fps_if_absent` is the frame rate when no `* UNA FPS` comment names one.
pub fn parse(text: &str, fps_if_absent: u32) -> Result<Timeline, EdlError> {
    // Pass 1: the frame rate (it governs every timecode, wherever the comment sits).
    let mut fps = fps_if_absent;
    for l in text.lines() {
        if let Some(r) = l.trim().strip_prefix("* UNA FPS ") {
            fps = r.trim().parse().map_err(|_| EdlError::Fps)?;
        }
    }
    if fps == 0 {
        return Err(EdlError::Fps);
    }
    let mut tl = Timeline::new(fps);
    let mut events: Vec<(&str, Kind, usize, Clip)> = Vec::new();
    // NLE4: the event a `* UNA GAIN` / `* UNA FADE` line belongs to (the event line just above it).
    let mut cur: Option<usize> = None;
    // NLE3: a zero-length `C` line waiting for its `D`/`W` line: (event number, kind, track number, reel, clip).
    let mut from: Option<(&str, Kind, usize, &str, Clip)> = None;
    // NLE3: each transition read: (kind, track number, transition, outgoing reel, outgoing out point, line).
    let mut trans: Vec<(Kind, usize, Transition, &str, i64, usize)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let l = raw.trim();
        if l.is_empty() || l.starts_with("TITLE:") {
            continue;
        }
        if let Some(r) = l.strip_prefix("FCM:") {
            match r.trim() {
                "NON-DROP FRAME" => continue,
                "DROP FRAME" => return Err(EdlError::DropFrame { line }),
                _ => return Err(EdlError::Fcm { line }),
            }
        }
        if l.starts_with('*') {
            if let Some(r) = l.strip_prefix("* UNA SOURCE ") {
                let w: Vec<&str> = r.split_whitespace().collect();
                if w.len() != 2 || !reel_ok(w[0]) {
                    return Err(EdlError::Syntax { line });
                }
                let d = parse_tc(w[1], fps, line)?;
                if tl.source_by_reel(w[0]).is_none() {
                    tl.add_source(w[0], d);
                }
            }
            if l.starts_with("* UNA GAIN ") || l.starts_with("* UNA FADE ") {
                audio_comment(l, fps, line, cur.and_then(|i| events.get_mut(i)).map(|e| &mut e.3))?;
            }
            continue;
        }
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.is_empty() || !w[0].bytes().all(|b| b.is_ascii_digit()) {
            return Err(EdlError::Syntax { line });
        }
        let tk = if w.len() >= 4 { transition_kind(w[3]).ok_or(EdlError::Transition { line })? } else { None };
        let n_words = if tk.is_some() { 9 } else { 8 };
        if w.len() != n_words {
            return Err(EdlError::Syntax { line });
        }
        let (kind, num) = channel(w[2]).ok_or(EdlError::Channel { line })?;
        let tc: Vec<i64> = w[n_words - 4..n_words].iter().map(|s| parse_tc(s, fps, line)).collect::<Result<_, _>>()?;
        if tc[3] - tc[2] != tc[1] - tc[0] {
            return Err(EdlError::Duration { line });
        }
        let clip = Clip::new(crate::SourceId(0), tc[0], tc[1], tc[2]);
        if let Some(tk) = tk {
            // The incoming line of a transition: it needs its outgoing line (same number, channel and record point).
            let dur = edl_frames(w[4]).ok_or(EdlError::Transition { line })?;
            match from.take() {
                Some((n, k, c, reel, o)) if n == w[0] && (k, c) == (kind, num) && o.at == clip.at && !(reel == "BL" && w[1] == "BL") => {
                    let x = Transition { at: clip.at, kind: tk, dur, from_black: reel == "BL", to_black: w[1] == "BL" };
                    trans.push((kind, num, x, reel, o.src_in, line));
                }
                _ => return Err(EdlError::Transition { line }),
            }
            if w[1] == "BL" {
                // NLE3cBLACK: a transition TO black: the black is the track's gap and makes no clip; it spans `dur` at least.
                if clip.len() < dur {
                    return Err(EdlError::Transition { line });
                }
                cur = None;
                continue;
            }
            events.push((w[1], kind, num, clip));
            cur = Some(events.len() - 1);
            continue;
        }
        if let Some((_, k, c, reel, o)) = from.take().filter(|f| f.3 != "BL") {
            events.push((reel, k, c, o)); // not followed by its D/W line: an empty clip, refused by validate
        }
        if w[1] == "BL" && clip.src_in != clip.src_out {
            cur = None;
            continue;
        }
        if clip.src_in == clip.src_out {
            from = Some((w[0], kind, num, w[1], clip));
            cur = None;
            continue;
        }
        events.push((w[1], kind, num, clip));
        cur = Some(events.len() - 1);
    }
    if let Some((_, k, c, reel, o)) = from.take().filter(|f| f.3 != "BL") {
        events.push((reel, k, c, o));
    }
    // Reels cited without a SOURCE line: their length is the furthest out point that cites them.
    for (reel, _, _, c) in &events {
        if !reel_ok(reel) {
            return Err(EdlError::Reel);
        }
        if tl.source_by_reel(reel).is_none() {
            let handles = trans.iter().filter(|x| x.3 == *reel).map(|x| x.4 + x.2.dur);
            let d = events.iter().filter(|e| e.0 == *reel).map(|e| e.3.src_out).chain(handles).max().unwrap_or(c.src_out);
            tl.add_source(reel, d);
        }
    }
    let nv = events.iter().filter(|e| e.1 == Kind::Video).map(|e| e.2).max().unwrap_or(0);
    let na = events.iter().filter(|e| e.1 == Kind::Audio).map(|e| e.2).max().unwrap_or(0);
    for _ in 0..nv {
        tl.add_track(Kind::Video);
    }
    for _ in 0..na {
        tl.add_track(Kind::Audio);
    }
    for (reel, kind, num, mut c) in events {
        c.src = tl.source_by_reel(reel).map(|s| s.id).ok_or(EdlError::Reel)?;
        let ti = if kind == Kind::Video { num - 1 } else { nv + num - 1 };
        tl.tracks[ti].clips.push(c);
    }
    for t in tl.tracks.iter_mut() {
        t.clips.sort_by_key(|c| c.at);
    }
    // NLE3: the clip ending at each transition's cut is the one its outgoing line names.
    for (kind, num, x, reel, out_point, line) in trans {
        let ti = if kind == Kind::Video { num - 1 } else { nv + num - 1 };
        if x.from_black {
            // NLE3c: no outgoing clip to name; validate checks the incoming clip and that nothing ends at the cut.
            tl.tracks[ti].transitions.push(x);
            tl.tracks[ti].transitions.sort_by_key(|x| x.at);
            continue;
        }
        let rid = tl.source_by_reel(reel).map(|s| s.id).ok_or(EdlError::Transition { line })?;
        let t = &mut tl.tracks[ti];
        if !t.clips.iter().any(|c| c.end() == x.at && c.src == rid && c.src_out == out_point) {
            return Err(EdlError::Transition { line });
        }
        t.transitions.push(x);
        t.transitions.sort_by_key(|x| x.at);
    }
    tl.validate().map_err(EdlError::Edit)?;
    Ok(tl)
}

/// NLE3: a CMX transition code: `C` (a cut: `None`), `D`, or `W001` / `W002`; `None` outside for any other.
fn transition_kind(s: &str) -> Option<Option<TransitionKind>> {
    match s {
        "C" => Some(None),
        "D" => Some(Some(TransitionKind::Dissolve)),
        _ => {
            let code = s.strip_prefix('W').filter(|c| c.len() == 3 && c.bytes().all(|b| b.is_ascii_digit()))?;
            WipeDir::from_code(code.parse().ok()?).map(|d| Some(TransitionKind::Wipe(d)))
        }
    }
}

/// NLE3: a transition's duration field (frames, digits only).
fn edl_frames(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// NLE3: the outgoing line of the transition on clip `c`'s cut (if any) and the incoming line's transition field.
fn transition_lines(tl: &Timeline, ti: usize, c: &Clip, num: usize, ch: &str, fps: u32) -> Result<(String, String), EdlError> {
    let t = &tl.tracks[ti];
    let Some(x) = t.transitions.iter().find(|x| x.at == c.at) else { return Ok((String::new(), String::from("C"))) };
    if x.dur > 999 {
        return Err(EdlError::Transition { line: 0 });
    }
    let (reel, p) = if x.from_black {
        (String::from("BL"), timecode(0, fps))
    } else {
        let o = t.clips.iter().find(|o| o.end() == x.at).ok_or(EdlError::Edit(EditError::Transition))?;
        (tl.source(o.src).ok_or(EdlError::Edit(EditError::NoSource))?.reel.clone(), timecode(o.src_out, fps))
    };
    let r = timecode(x.at, fps);
    let out = format!("{:03}  {:<8} {:<5} {:<8} {p} {p} {r} {r}\n", num, reel, ch, "C");
    Ok((out, code_field(x)))
}

/// Write `tl` as a CMX 3600 EDL: the header, the FPS and SOURCE comments, then one event per clip in record order
/// (ties: track order), numbered from 001.
pub fn write(tl: &Timeline, title: &str) -> Result<String, EdlError> {
    let fps = tl.fps().ok_or(EdlError::Fps)?;
    tl.validate().map_err(EdlError::Edit)?;
    let mut s = String::new();
    s.push_str(&format!("TITLE: {title}\nFCM: NON-DROP FRAME\n* UNA FPS {fps}\n"));
    for src in &tl.sources {
        if !reel_ok(&src.reel) || src.reel == "BL" {
            return Err(EdlError::Reel);
        }
        s.push_str(&format!("* UNA SOURCE {} {}\n", src.reel, timecode(src.duration, fps)));
    }
    s.push('\n');
    // NLE3cBLACK: `Err` = a transition TO black, an event at its `at` (no clip).
    let mut ev: Vec<(i64, usize, Result<Clip, Transition>)> = Vec::new();
    for (ti, t) in tl.tracks.iter().enumerate() {
        for c in &t.clips {
            ev.push((c.at, ti, Ok(*c)));
        }
        for x in t.transitions.iter().filter(|x| x.to_black) {
            ev.push((x.at, ti, Err(*x)));
        }
    }
    ev.sort_by_key(|e| (e.0, e.1));
    for (n, (_, ti, e)) in ev.iter().enumerate() {
        let num = tl.track_number(*ti).unwrap_or(1);
        let k = if tl.tracks[*ti].kind == Kind::Video { "V" } else { "A" };
        if num > 9 {
            return Err(EdlError::Channel { line: 0 });
        }
        let ch = if num == 1 { String::from(k) } else { format!("{k}{num}") };
        let c = match e {
            Ok(c) => c,
            Err(x) => {
                s.push_str(&to_black_lines(tl, *ti, x, n + 1, &ch, fps)?);
                continue;
            }
        };
        let reel = &tl.source(c.src).ok_or(EdlError::Edit(EditError::NoSource))?.reel;
        let (out_line, code) = transition_lines(tl, *ti, c, n + 1, &ch, fps)?;
        s.push_str(&out_line);
        s.push_str(&format!(
            "{:03}  {:<8} {:<5} {:<8} {} {} {} {}\n",
            n + 1,
            reel,
            ch,
            code,
            timecode(c.src_in, fps),
            timecode(c.src_out, fps),
            timecode(c.at, fps),
            timecode(c.end(), fps)
        ));
        if c.gain_cdb != 0 {
            s.push_str(&format!("* UNA GAIN {}\n", c.gain_cdb));
        }
        if c.fade_in != 0 || c.fade_out != 0 {
            s.push_str(&format!("* UNA FADE {} {}\n", timecode(c.fade_in, fps), timecode(c.fade_out, fps)));
        }
    }
    Ok(s)
}

/// NLE4: apply a `* UNA GAIN <cdb>` or `* UNA FADE <in> <out>` line to the event above it (`None`: no event).
fn audio_comment(l: &str, fps: u32, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let c = clip.ok_or(EdlError::Syntax { line })?;
    if let Some(r) = l.strip_prefix("* UNA GAIN ") {
        c.gain_cdb = r.trim().parse().map_err(|_| EdlError::Syntax { line })?;
    } else if let Some(r) = l.strip_prefix("* UNA FADE ") {
        let w: Vec<&str> = r.split_whitespace().collect();
        if w.len() != 2 {
            return Err(EdlError::Syntax { line });
        }
        (c.fade_in, c.fade_out) = (parse_tc(w[0], fps, line)?, parse_tc(w[1], fps, line)?);
    }
    Ok(())
}

/// NLE3: a transition's CMX transition field: `D    ddd` or `Wnnn ddd`.
fn code_field(x: &Transition) -> String {
    match x.kind {
        TransitionKind::Dissolve => format!("D    {:03}", x.dur),
        TransitionKind::Wipe(d) => format!("W{:03} {:03}", d.code(), x.dur),
    }
}

/// NLE3cBLACK (LEDGER SR323): a transition TO black as its two-line event: the outgoing's zero-length `C` line at its
/// out point and the cut, then reel `BL` with the transition field over record `at..at + dur` (source from 0).
fn to_black_lines(tl: &Timeline, ti: usize, x: &Transition, num: usize, ch: &str, fps: u32) -> Result<String, EdlError> {
    if x.dur > 999 {
        return Err(EdlError::Transition { line: 0 });
    }
    let o = tl.tracks[ti].clips.iter().find(|o| o.end() == x.at).ok_or(EdlError::Edit(EditError::Transition))?;
    let reel = &tl.source(o.src).ok_or(EdlError::Edit(EditError::NoSource))?.reel;
    let (p, r) = (timecode(o.src_out, fps), timecode(x.at, fps));
    let (z, d, e) = (timecode(0, fps), timecode(x.dur, fps), timecode(x.at + x.dur, fps));
    let mut s = format!("{:03}  {:<8} {:<5} {:<8} {p} {p} {r} {r}\n", num, reel, ch, "C");
    s.push_str(&format!("{:03}  {:<8} {:<5} {:<8} {z} {d} {r} {e}\n", num, "BL", ch, code_field(x)));
    Ok(s)
}
