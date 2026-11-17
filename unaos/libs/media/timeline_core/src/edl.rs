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
//! NLESIZEPOLICY (LEDGER SR394): `* UNA SIZE <reel> <policy>` is the reel's [`crate::SizePolicy`] (`letterbox`, `fill`,
//! `stretch`, `adopt`; absent = letterbox), written right after that reel's `* UNA SOURCE` line and only when not letterbox,
//! so an EDL with no policy writes as before. A SIZE line may sit anywhere (it applies once every reel exists); one naming a
//! reel nothing cites, an unknown policy, or a second SIZE line for one reel is a syntax error.
//!
//! NLEWIPES (LEDGER SR679): `* UNA EASE <in|out|inout>` is the transition's [`crate::Ease`], written right after its
//! INCOMING line (the `D` / `W` line, or a to-black's `BL` line) and only when not linear, so an EDL with no ease writes as
//! before. One with no transition's incoming line above it, an unknown word, or a second EASE for one transition is a
//! syntax error. Wipe codes gain W007, W023-W026, W043-W047, W102, W119, W205-W207, W211-W214; the rest of the SMPTE table
//! is refused, each by name in [`WipeDir::REFUSED`].
//! NLEXFADEPOWER (LEDGER SR678): a `D` event on an AUDIO channel (`A`, `A2`..`A9`; `AA` / `AA/V` stay refused as
//! channels) is an explicit audio crossfade ([`crate::AudioXfade`]), two lines like a video dissolve; `* UNA XFADE EQUAL`
//! under its incoming line makes it equal-power ([`crate::Curve::EqualPower`]; absent = linear, so a linear one writes no
//! comment). A wipe or `BL` on an audio channel is refused (`Transition`), as is an XFADE line with no audio `D` line above
//! it, under a video one, twice, or with another word (`Syntax`).
//!
//! [`write`] produces exactly what [`parse`] reads back: `write(parse(write(tl)))` is byte-identical.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{Clip, Ease, EditError, Kind, SizePolicy, Timeline, Transition, TransitionKind, WipeDir};

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
    // NLESIZEPOLICY (SR394): each `* UNA SIZE` read: (reel, policy, line), applied once every source exists.
    let mut sizes: Vec<(&str, SizePolicy, usize)> = Vec::new();
    // NLEWIPES (SR679): the transition (index into `trans`) whose incoming line is the event line just above.
    let mut last_trans: Option<usize> = None;
    // NLEXFADEPOWER (SR678): the `trans` index of the D/W line just read, and each XFADE line read: (its D line, its line).
    let (mut xq, mut eq): (Option<usize>, Vec<(usize, usize)>) = (None, Vec::new());
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
            title_comment(l, line, &mut tl)?; mark_comment(l, line, &mut tl)?; if let Some(r) = l.strip_prefix("* UNA SIZE ") {
                sizes.push(size_comment(r, line)?);
            }
            want_comment(l, line, last_trans.and_then(|i| trans.get_mut(i)).filter(|x| x.0 == Kind::Video).map(|x| &mut x.2))?; if let Some(r) = l.strip_prefix("* UNA EASE ") {
                ease_comment(r, line, last_trans.and_then(|i| trans.get_mut(i)).map(|x| &mut x.2))?; // NLEWIPES (SR679)
            }
            if l.starts_with("* UNA GAIN ") || l.starts_with("* UNA FADE ") {
                audio_comment(l, fps, line, cur.and_then(|i| events.get_mut(i)).map(|e| &mut e.3))?;
            }
            marker_comment(l, fps, line, &mut tl)?; if l.starts_with("* UNA XFADE") {
                xfade_comment(l, line, xq.map(|k| trans[k].5), &mut eq)?; // NLEXFADEPOWER (SR678)
            }
            continue;
        }
        let w: Vec<&str> = l.split_whitespace().collect();
        xq = None; // NLEXFADEPOWER: an XFADE line belongs to the event line just above it
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
        last_trans = None; // NLEWIPES (SR679): set again below by a transition's incoming line
        if let Some(tk) = tk {
            // The incoming line of a transition: it needs its outgoing line (same number, channel and record point).
            let dur = edl_frames(w[4]).ok_or(EdlError::Transition { line })?;
            match from.take() {
                Some((n, k, c, reel, o)) if n == w[0] && (k, c) == (kind, num) && o.at == clip.at && !(reel == "BL" && w[1] == "BL") => {
                    let x = Transition { at: clip.at, kind: tk, dur, from_black: reel == "BL", to_black: w[1] == "BL", ease: Ease::Linear, want: None };
                    xq = Some(trans.len());
                    trans.push((kind, num, x, reel, o.src_in, line));
                    last_trans = Some(trans.len() - 1); // NLEWIPES (SR679)
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
        audio_transition_ok(kind, &x, line, &eq)?; // NLEXFADEPOWER (SR678)
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
        if kind == Kind::Audio {
            // NLEXFADEPOWER: an audio D event is an explicit audio crossfade (validate checks its incoming clip).
            let curve = if eq.iter().any(|e| e.0 == line) { crate::Curve::EqualPower } else { crate::Curve::Linear };
            tl.audio_xfades.push(crate::AudioXfade { track: ti, at: x.at, dur: x.dur, curve });
            tl.audio_xfades.sort_by_key(|x| (x.track, x.at));
            continue;
        }
        t.transitions.push(x);
        t.transitions.sort_by_key(|x| x.at);
    }
    for (n, &(reel, p, line)) in sizes.iter().enumerate() {
        if sizes[..n].iter().any(|s| s.0 == reel) {
            return Err(EdlError::Syntax { line });
        }
        let id = tl.source_by_reel(reel).map(|s| s.id).ok_or(EdlError::Syntax { line })?;
        tl = tl.set_size_policy(id, p).map_err(EdlError::Edit)?;
    }
    tl.validate().map_err(EdlError::Edit)?;
    Ok(tl)
}

/// NLE3: a CMX transition code: `C` (a cut: `None`), `D`, or `W001` / `W002`; `None` outside for any other.
pub(crate) fn transition_kind(s: &str) -> Option<Option<TransitionKind>> {
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
        if src.size_policy != SizePolicy::Letterbox {
            s.push_str(&format!("* UNA SIZE {} {}\n", src.reel, src.size_policy.name())); // NLESIZEPOLICY (SR394)
        }
        s.push_str(&title_line(src)); // STRIATITLECLIP (SR828): a title source's `* UNA TITLE` line, right after its SOURCE (file tail)
    }
    s.push_str(&mark_line(tl)); s.push_str(&marker_lines(tl, fps)); s.push('\n'); // NLE5DRANGESAVE (SR723): the document's `* UNA MARK` line, last in the header (file tail); STRIAMARKERS (SR835): one `* UNA MARKER` line per marker after it (file tail)
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
        let (out_line, code) = match audio_xfade_lines(tl, *ti, c, n + 1, &ch, fps) { Some(r) => r?, None => transition_lines(tl, *ti, c, n + 1, &ch, fps)? }; // NLEXFADEPOWER (SR678)
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
        s.push_str(&ease_line(tl.tracks[*ti].transitions.iter().find(|x| x.at == c.at))); s.push_str(&want_line(tl.tracks[*ti].transitions.iter().find(|x| x.at == c.at))); // NLEWIPES (SR679)
        if c.gain_cdb != 0 {
            s.push_str(&format!("* UNA GAIN {}\n", c.gain_cdb));
        }
        if c.fade_in != 0 || c.fade_out != 0 {
            s.push_str(&format!("* UNA FADE {} {}\n", timecode(c.fade_in, fps), timecode(c.fade_out, fps)));
        }
        s.push_str(xfade_line(tl, *ti, c.at)); // NLEXFADEPOWER (SR678)
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
    s.push_str(&ease_line(Some(x))); s.push_str(&want_line(Some(x))); // NLEWIPES (SR679)
    Ok(s)
}

/// NLESIZEPOLICY (LEDGER SR394): the words after `* UNA SIZE `: a reel name and a policy word.
fn size_comment(r: &str, line: usize) -> Result<(&str, SizePolicy, usize), EdlError> {
    let w: Vec<&str> = r.split_whitespace().collect();
    if w.len() != 2 || !reel_ok(w[0]) {
        return Err(EdlError::Syntax { line });
    }
    let p = SizePolicy::from_name(w[1]).ok_or(EdlError::Syntax { line })?;
    Ok((w[0], p, line))
}

// ---- NLEWIPES (LEDGER SR679): the `* UNA EASE` line (named tail block) ----

/// NLEWIPES: the words after `* UNA EASE ` applied to the transition whose incoming line is just above (`None`: no such
/// line). `in` / `out` / `inout` only (linear is the absence of the line); a second EASE for one transition is refused.
fn ease_comment(r: &str, line: usize, x: Option<&mut Transition>) -> Result<(), EdlError> {
    let x = x.ok_or(EdlError::Syntax { line })?;
    let e = Ease::from_name(r.trim()).filter(|e| *e != Ease::Linear).ok_or(EdlError::Syntax { line })?;
    if x.ease != Ease::Linear {
        return Err(EdlError::Syntax { line });
    }
    x.ease = e;
    Ok(())
}

/// NLEWIPES: the `* UNA EASE` line of the transition on an incoming line (empty for none or a linear one).
fn ease_line(x: Option<&Transition>) -> String {
    match x.map(|x| x.ease) {
        Some(e) if e != Ease::Linear => format!("* UNA EASE {}\n", e.name()),
        _ => String::new(),
    }
}

// ---- NLEXFADEPOWER (LEDGER SR678): the A-only `D` event and `* UNA XFADE EQUAL` (named tail block) ----

/// A `* UNA XFADE ...` line: only `EQUAL`, under a D/W line (`d_line`, the line just above), once per event; else `Syntax`.
fn xfade_comment(l: &str, line: usize, d_line: Option<usize>, eq: &mut Vec<(usize, usize)>) -> Result<(), EdlError> {
    let d = d_line.ok_or(EdlError::Syntax { line })?;
    if l != "* UNA XFADE EQUAL" || eq.iter().any(|e| e.0 == d) {
        return Err(EdlError::Syntax { line });
    }
    eq.push((d, line));
    Ok(())
}

/// A transition read on an audio channel is a plain dissolve between two clips (a wipe or `BL`: `Transition` at its line);
/// an XFADE line under a video transition's line is `Syntax` at the XFADE line.
fn audio_transition_ok(kind: Kind, x: &Transition, line: usize, eq: &[(usize, usize)]) -> Result<(), EdlError> {
    if kind == Kind::Audio && (x.from_black || x.to_black || x.kind != TransitionKind::Dissolve) {
        return Err(EdlError::Transition { line });
    }
    match eq.iter().find(|e| e.0 == line) {
        Some(&(_, xl)) if kind == Kind::Video => Err(EdlError::Syntax { line: xl }),
        _ => Ok(()),
    }
}

/// The two-line event of the explicit audio crossfade on clip `c`'s cut, if one sits there: the outgoing's zero-length
/// `C` line (as [`transition_lines`] writes it) and the incoming line's field `D    ddd` (a duration past 999: `Transition`).
fn audio_xfade_lines(tl: &Timeline, ti: usize, c: &Clip, num: usize, ch: &str, fps: u32) -> Option<Result<(String, String), EdlError>> {
    let x = tl.audio_xfades.iter().find(|x| x.track == ti && x.at == c.at)?;
    Some((|| {
        if x.dur > 999 {
            return Err(EdlError::Transition { line: 0 });
        }
        let o = tl.tracks[ti].clips.iter().find(|o| o.end() == x.at).ok_or(EdlError::Edit(EditError::Transition))?;
        let reel = &tl.source(o.src).ok_or(EdlError::Edit(EditError::NoSource))?.reel;
        let (p, r) = (timecode(o.src_out, fps), timecode(x.at, fps));
        Ok((format!("{:03}  {:<8} {:<5} {:<8} {p} {p} {r} {r}\n", num, reel, ch, "C"), format!("D    {:03}", x.dur)))
    })())
}

/// `* UNA XFADE EQUAL\n` under the incoming line of an equal-power explicit audio crossfade at `(ti, at)`; else nothing.
fn xfade_line(tl: &Timeline, ti: usize, at: i64) -> &'static str {
    if tl.audio_xfades.iter().any(|x| x.track == ti && x.at == at && x.curve == crate::Curve::EqualPower) { "* UNA XFADE EQUAL\n" } else { "" }
}

// ---- NLE3ETRIMGROW (LEDGER SR712): the `* UNA WANT` line (named tail block) ----
// `* UNA WANT <n>` is the asked length (ticks) of a transition a trim fitted below it ([`crate::Transition::want`]),
// written right after its incoming line's EASE line and only when the transition is a fit, so an EDL with no fitted
// transition writes as before and an older EDL reads unchanged. One with no video transition's incoming line above it,
// a word that is not an integer, a second WANT for one transition, or a length not above the transition's own is a
// syntax error.

/// NLE3ETRIMGROW: a `* UNA WANT <n>` line applied to the video transition whose incoming line is just above (`None`: no
/// such line); any other line is left alone.
fn want_comment(l: &str, line: usize, x: Option<&mut Transition>) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA WANT ") else { return Ok(()) };
    let x = x.ok_or(EdlError::Syntax { line })?;
    let w: i64 = r.trim().parse().map_err(|_| EdlError::Syntax { line })?;
    if x.want.is_some() || w <= x.dur {
        return Err(EdlError::Syntax { line });
    }
    x.want = Some(w);
    Ok(())
}

/// NLE3ETRIMGROW: the `* UNA WANT` line of the transition on an incoming line (empty for none or one at its ask).
fn want_line(x: Option<&Transition>) -> String {
    match x.and_then(|x| x.want.filter(|&w| w != x.dur)) {
        Some(w) => format!("* UNA WANT {w}\n"),
        None => String::new(),
    }
}

// ---- NLE5DRANGESAVE (LEDGER SR723): the `* UNA MARK` line (named tail block) ----
// `* UNA MARK <in> <out>` is the document's export range ([`crate::Timeline::marks`]): each word a tick or `-` for an unset
// mark, written ONCE as the header's last line (after the FPS / SOURCE / SIZE lines, before the first event) and only when
// a mark is set, so an EDL with no marks writes as before and a CMX reader skips it as a comment. Wrong word count, a word
// that is neither an integer nor `-`, both `-`, a second MARK line, or marks [`crate::Timeline::set_marks`] refuses (a
// negative tick, in >= out) are a syntax error.

/// NLE5DRANGESAVE: a `* UNA MARK` line applied to the timeline being read; any other line is left alone.
fn mark_comment(l: &str, line: usize, tl: &mut Timeline) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA MARK ") else { return Ok(()) };
    let w: Vec<&str> = r.split_whitespace().collect();
    if w.len() != 2 || tl.marks != (None, None) {
        return Err(EdlError::Syntax { line });
    }
    let tick = |s: &str| -> Result<Option<i64>, EdlError> {
        if s == "-" { Ok(None) } else { s.parse::<i64>().map(Some).map_err(|_| EdlError::Syntax { line }) }
    };
    let m = (tick(w[0])?, tick(w[1])?);
    if m == (None, None) {
        return Err(EdlError::Syntax { line });
    }
    *tl = tl.set_marks(m).map_err(|_| EdlError::Syntax { line })?;
    Ok(())
}

/// NLE5DRANGESAVE: the header's `* UNA MARK` line (empty when no mark is set).
fn mark_line(tl: &Timeline) -> String {
    let word = |m: Option<i64>| m.map_or_else(|| String::from("-"), |t| format!("{t}"));
    match tl.marks {
        (None, None) => String::new(),
        (a, b) => format!("* UNA MARK {} {}\n", word(a), word(b)),
    }
}

// ---- STRIATITLECLIP (LEDGER SR828): the `* UNA TITLE` line (named tail block) ----
// `* UNA TITLE <reel> "<text>"` is a generated title source's text ([`crate::Source::title`]), written right after that
// reel's `* UNA SOURCE` (and SIZE) line and only for a titled source, so an EDL with no title writes as before. The text is
// quoted: `\` -> `\\`, `"` -> `\"`, LF -> `\n`, CR -> `\r`. A TITLE line naming a reel no SOURCE line above it declared, a
// second TITLE for one reel, or a malformed quote (an unknown escape, no closing quote, words after it) is a syntax error.

/// STRIATITLECLIP: a `* UNA TITLE` line applied to its (already declared) source; any other line is left alone.
fn title_comment(l: &str, line: usize, tl: &mut Timeline) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA TITLE ") else { return Ok(()) };
    let (reel, q) = r.split_once(' ').ok_or(EdlError::Syntax { line })?;
    let text = unquote(q).ok_or(EdlError::Syntax { line })?;
    let src = tl.sources.iter_mut().find(|s| s.reel == reel).filter(|s| s.title.is_none()).ok_or(EdlError::Syntax { line })?;
    src.title = Some(text);
    Ok(())
}

/// STRIATITLECLIP: a source's `* UNA TITLE` line (empty for a media reel).
fn title_line(src: &crate::Source) -> String {
    match &src.title {
        Some(t) => format!("* UNA TITLE {} {}\n", src.reel, quote(t)),
        None => String::new(),
    }
}

/// STRIATITLECLIP: `text` in double quotes, `\`, `"`, LF and CR escaped (the one inverse of [`unquote`]).
fn quote(text: &str) -> String {
    let mut q = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => q.push_str("\\\\"),
            '"' => q.push_str("\\\""),
            '\n' => q.push_str("\\n"),
            '\r' => q.push_str("\\r"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// STRIATITLECLIP: the text of a [`quote`]d string that is the whole of `q`; `None` for anything else.
fn unquote(q: &str) -> Option<String> {
    let mut it = q.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match it.next()? {
            '"' => return it.next().is_none().then_some(out),
            '\\' => out.push(match it.next()? {
                '\\' => '\\',
                '"' => '"',
                'n' => '\n',
                'r' => '\r',
                _ => return None,
            }),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod striatitleclip_tests {
    use super::*;
    use crate::{Clip, Kind, Timeline};

    fn titled(text: &str) -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let t = tl.add_source("T1", 125);
        tl.sources[1].title = Some(String::from(text));
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(t, 0, 20, 10)];
        tl
    }

    #[test]
    fn striatitleclip_title_round_trips_with_quote_and_backslash() {
        let text = "He said \"hi\" \\ back\\slash";
        let tl = titled(text);
        let s = write(&tl, "TITLES").unwrap();
        assert!(s.contains("* UNA SOURCE T1 00:00:05:00\n* UNA TITLE T1 \"He said \\\"hi\\\" \\\\ back\\\\slash\"\n"), "{s}");
        let back = parse(&s, 25).unwrap();
        assert_eq!(back, tl);
        assert_eq!(back.sources[1].title.as_deref(), Some(text));
        assert_eq!(write(&back, "TITLES").unwrap(), s, "write(parse(write)) byte-identical");
    }

    #[test]
    fn striatitleclip_quote_pair_and_malformed() {
        for t in ["", "plain", "a\"b", "c\\d", "line\nbreak\rcr", "  spaced  ", "\u{00e9}t\u{00e9}"] {
            assert_eq!(unquote(&quote(t)).as_deref(), Some(t), "{t:?}");
        }
        for bad in ["", "plain", "\"open", "\"x\" tail", "\"bad\\q\"", "\"end\\\"", "x\"y\""] {
            assert_eq!(unquote(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn striatitleclip_bad_title_lines_are_syntax() {
        let good = write(&titled("Hello"), "T").unwrap();
        let unknown = good.replace("* UNA TITLE T1", "* UNA TITLE T9");
        assert!(matches!(parse(&unknown, 25), Err(EdlError::Syntax { .. })), "unknown reel");
        let twice = good.replace("* UNA TITLE T1 \"Hello\"\n", "* UNA TITLE T1 \"Hello\"\n* UNA TITLE T1 \"Again\"\n");
        assert!(matches!(parse(&twice, 25), Err(EdlError::Syntax { .. })), "second title");
        let bad = good.replace("\"Hello\"", "\"Hello");
        assert!(matches!(parse(&bad, 25), Err(EdlError::Syntax { .. })), "bad quote");
        let first = good.replace("* UNA SOURCE T1 00:00:05:00\n* UNA TITLE T1 \"Hello\"\n", "* UNA TITLE T1 \"Hello\"\n* UNA SOURCE T1 00:00:05:00\n");
        assert!(matches!(parse(&first, 25), Err(EdlError::Syntax { .. })), "title before its source");
    }

    #[test]
    fn striatitleclip_no_title_writes_no_title_line() {
        let mut tl = titled("x");
        tl.sources[1].title = None;
        let s = write(&tl, "T").unwrap();
        assert!(!s.contains("UNA TITLE"), "{s}");
        assert_eq!(parse(&s, 25).unwrap().sources[1].title, None);
    }
}

// ---- STRIAMARKERS (LEDGER SR835): the `* UNA MARKER` lines (named tail block) ----
// `* UNA MARKER <HH:MM:SS:FF> "<name>"` is one named marker ([`crate::Timeline::markers`]), one line per marker in tick order,
// written in the header right after the `* UNA MARK` line and only when markers exist, so an EDL with none writes as before
// and a CMX reader skips them as comments. The name is quoted by STRIATITLECLIP's [`quote`] / [`unquote`] (the one quote
// grammar). A bad timecode is `Timecode`; a missing or malformed quote (an unknown escape, no closing quote, words after it)
// or a second marker at one tick ([`crate::Timeline::mark`] refuses) is a syntax error.

/// STRIAMARKERS: a `* UNA MARKER` line applied to the timeline being read; any other line is left alone.
fn marker_comment(l: &str, fps: u32, line: usize, tl: &mut Timeline) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA MARKER ") else { return Ok(()) };
    let (tc, q) = r.split_once(' ').ok_or(EdlError::Syntax { line })?;
    let at = parse_tc(tc, fps, line)?;
    let name = unquote(q).ok_or(EdlError::Syntax { line })?;
    *tl = tl.mark(at, &name).map_err(|_| EdlError::Syntax { line })?;
    Ok(())
}

/// STRIAMARKERS: the header's `* UNA MARKER` lines (empty with no marker).
fn marker_lines(tl: &Timeline, fps: u32) -> String {
    tl.markers.iter().map(|m| format!("* UNA MARKER {} {}\n", timecode(m.at, fps), quote(&m.name))).collect()
}

#[cfg(test)]
mod striamarkers_tests {
    use super::*;
    use crate::{Clip, Kind, Timeline};

    fn cut() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl
    }

    /// STRIAMARKERS M3: two markers (one name with a quote and a backslash) round-trip byte for byte, right after the
    /// `* UNA MARK` line; a timeline with none writes no MARKER line.
    #[test]
    fn striamarkers_markers_round_trip_after_the_mark_line() {
        let plain = write(&cut(), "T").unwrap();
        assert!(!plain.contains("UNA MARKER"), "{plain}");
        assert_eq!(write(&parse(&plain, 25).unwrap(), "T").unwrap(), plain);
        let tl = cut().set_marks((Some(2), Some(18))).unwrap().mark(37, "say \"hi\" \\ there").unwrap().mark(12, "M1").unwrap();
        let s = write(&tl, "T").unwrap();
        let want = "* UNA MARK 2 18\n* UNA MARKER 00:00:00:12 \"M1\"\n* UNA MARKER 00:00:01:12 \"say \\\"hi\\\" \\\\ there\"\n\n";
        assert!(s.contains(want), "{s}");
        let back = parse(&s, 25).unwrap();
        assert_eq!(back, tl);
        assert_eq!(back.markers[1].name, "say \"hi\" \\ there");
        assert_eq!(write(&back, "T").unwrap(), s);
    }

    /// STRIAMARKERS M3: a malformed MARKER line is refused by name.
    #[test]
    fn striamarkers_malformed_marker_lines_refused() {
        let head = write(&cut(), "T").unwrap();
        let with = |l: &str| head.replacen("* UNA FPS 25\n", &format!("* UNA FPS 25\n{l}\n"), 1);
        assert_eq!(parse(&with("* UNA MARKER 00:00:00:12 M1"), 25), Err(EdlError::Syntax { line: 4 }));
        assert_eq!(parse(&with("* UNA MARKER 00:00:00:12 \"M1"), 25), Err(EdlError::Syntax { line: 4 }));
        assert_eq!(parse(&with("* UNA MARKER 00:00:00:12 \"M1\" x"), 25), Err(EdlError::Syntax { line: 4 }));
        assert_eq!(parse(&with("* UNA MARKER 00:00:00:12"), 25), Err(EdlError::Syntax { line: 4 }));
        assert_eq!(parse(&with("* UNA MARKER 00:00:0x:12 \"M1\""), 25), Err(EdlError::Timecode { line: 4 }));
        let twice = head.replacen("* UNA FPS 25\n", "* UNA FPS 25\n* UNA MARKER 00:00:00:12 \"a\"\n* UNA MARKER 00:00:00:12 \"b\"\n", 1);
        assert_eq!(parse(&twice, 25), Err(EdlError::Syntax { line: 5 }));
        let ok = parse(&with("* UNA MARKER 00:00:00:12 \"M1\""), 25).unwrap();
        assert_eq!(ok.markers, alloc::vec![crate::Marker { at: 12, name: "M1".into() }]);
    }
}
