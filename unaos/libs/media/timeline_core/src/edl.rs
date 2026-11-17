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
pub(crate) fn parse_tc(s: &str, fps: u32, line: usize) -> Result<i64, EdlError> {
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

pub(crate) fn channel(s: &str) -> Option<(Kind, usize)> { // STRIATRACKGAINKEYSNEXT (SR983): pub(crate), `Timeline::track_named` reads it
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
            xform_comment(l, line, cur.and_then(|i| events.get_mut(i)).filter(|e| e.1 == Kind::Video).map(|e| &mut e.3))?; crop_comment(l, line, cur.and_then(|i| events.get_mut(i)).filter(|e| e.1 == Kind::Video).map(|e| &mut e.3))?; group_comment(l, line, cur.and_then(|i| events.get_mut(i)).map(|e| &mut e.3))?; pan_comment(l, line, cur.and_then(|i| events.get_mut(i)).map(|e| &mut e.3))?; if l.starts_with("* UNA GAIN ") || l.starts_with("* UNA FADE ") { // STRIACLIPLINK (SR863): `* UNA GROUP` (file tail) // STRIAPAN (SR861): the `* UNA PAN` line (file tail)
                audio_comment(l, fps, line, cur.and_then(|i| events.get_mut(i)).map(|e| &mut e.3))?;
            }
            marker_comment(l, fps, line, &mut tl)?; marker_style_comment(l, fps, line, &mut tl)?; alpha_comment(l, line, cur.and_then(|i| events.get_mut(i)).map(|e| &mut e.3))?; grade_comment(l, line, cur.and_then(|i| events.get_mut(i)).map(|e| &mut e.3))?; if l.starts_with("* UNA XFADE") { // STRIACOLORLIFT (SR869): `* UNA COLOR` (file tail) // STRIAMARKERJUMPKEYSNEXT (SR984): `* UNA MCOLOUR` / `* UNA MRANGE` (file tail)
                xfade_comment(l, line, xq.map(|k| trans[k].5), &mut eq)?; // NLEXFADEPOWER (SR678)
            }
            continue;
        }
        if l.starts_with("M2 ") { m2_comment(text, i, l, fps, line, cur.and_then(|k| events.get_mut(k)).map(|e| &mut e.3))?; continue; } let w: Vec<&str> = l.split_whitespace().collect(); // STRIACLIPSPEED (SR829): the CMX M2 speed line under its event (file tail)
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
        if tc[3] - tc[2] != tc[1] - tc[0] && !m2_next(text, i) { // STRIACLIPSPEED (SR829): a rated event's spans differ; its M2 line checks them (file tail)
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
    let nv = events.iter().filter(|e| e.1 == Kind::Video).map(|e| e.2).max().unwrap_or(0).max(track_counts(text)?.0); // STRIATRACKADDREMOVE (SR856): `* UNA TRACKS` is a floor (file tail)
    let na = events.iter().filter(|e| e.1 == Kind::Audio).map(|e| e.2).max().unwrap_or(0).max(track_counts(text)?.1); // STRIATRACKADDREMOVE (SR856)
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
    track_flags(text, &mut tl)?; // STRIATRACKMUTESOLO (SR837): the `* UNA MUTE` / `* UNA SOLO` lines, once the tracks exist (file tail)
    track_gains(text, &mut tl)?; // STRIATRACKGAIN (SR864): the `* UNA TGAIN` lines, once the tracks exist (file tail)
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
    s.push_str(&mark_line(tl)); s.push_str(&marker_lines(tl, fps)); s.push_str(&marker_style_lines(tl, fps)); s.push('\n'); s.insert_str(s.len() - 1, &tracks_line(tl)); // NLE5DRANGESAVE (SR723): the document's `* UNA MARK` line, last in the header (file tail)
    s.insert_str(s.len() - 1, &flag_lines(tl, "MUTE", |t| t.muted)); // STRIATRACKMUTESOLO (SR837): the muted tracks' `* UNA MUTE` lines, before the header's blank line (file tail)
    s.insert_str(s.len() - 1, &flag_lines(tl, "SOLO", |t| t.solo)); // STRIATRACKMUTESOLO (SR837): the soloed tracks' `* UNA SOLO` lines (file tail); STRIAMARKERS (SR835): one `* UNA MARKER` line per marker after it (file tail); STRIATRACKADDREMOVE (SR856): `* UNA TRACKS` when a kind has an empty trailing track (file tail)
    s.insert_str(s.len() - 1, &tgain_lines(tl)); // STRIATRACKGAIN (SR864): the gained tracks' `* UNA TGAIN` lines, last in the header (file tail)
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
        s.push_str(&m2_line(reel, c, fps)); s.push_str(&ease_line(tl.tracks[*ti].transitions.iter().find(|x| x.at == c.at))); s.push_str(&want_line(tl.tracks[*ti].transitions.iter().find(|x| x.at == c.at))); // NLEWIPES (SR679)
        s.push_str(&crop_line(c)); s.push_str(&xform_line(c)); if c.gain_cdb != 0 {
            s.push_str(&format!("* UNA GAIN {}\n", c.gain_cdb));
        }
        if c.fade_in != 0 || c.fade_out != 0 {
            s.push_str(&format!("* UNA FADE {} {}\n", timecode(c.fade_in, fps), timecode(c.fade_out, fps)));
        }
        s.push_str(&grade_line(c)); s.push_str(&pan_line(c)); s.push_str(xfade_line(tl, *ti, c.at)); s.push_str(&alpha_line(c)); s.push_str(&group_line(c)); // STRIACLIPLINK (SR863): `* UNA GROUP` (file tail); STRIACOLORLIFT (SR869): the clip's `* UNA COLOR` line (file tail); NLEXFADEPOWER (SR678); STRIAOPACITY (SR868): `* UNA ALPHA` (file tail); STRIAPAN (SR861): `* UNA PAN` (file tail)
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

// ---- STRIATRACKMUTESOLO (LEDGER SR837): the `* UNA MUTE` / `* UNA SOLO` lines (named tail block) ----
// `* UNA MUTE <ch>` / `* UNA SOLO <ch>` set a track's [`crate::Track::muted`] / [`crate::Track::solo`]; `<ch>` is the
// event channel word (`V`, `V2`..`V9`, `A`, `A2`..`A9`; `V1` / `A1` read too). One line per flagged track, MUTE lines then
// SOLO lines, written last in the header (after `* UNA MARK`, before the blank line) and only when a flag is set, so an
// EDL with no flag writes as before. A flag is written only for a track the EDL carries (a track of its kind at or after it
// holds a clip: the reader makes tracks up to the highest channel an event names), so `write(parse(write(tl)))` stays
// byte-identical. Read once the tracks exist; refused as a syntax error: not exactly one word, an unknown channel, a channel
// naming no track, the same word twice for one track.

/// STRIATRACKMUTESOLO: the header's lines for every carried track `pick` flags: `* UNA <word> <ch>\n` each.
fn flag_lines(tl: &Timeline, word: &str, pick: fn(&crate::Track) -> bool) -> String {
    let mut s = String::new();
    for (ti, t) in tl.tracks.iter().enumerate() {
        if let Some(ch) = carried_ch(tl, ti).filter(|_| pick(t)) {
            s.push_str(&format!("* UNA {word} {ch}\n"));
        }
    }
    s
}

/// STRIATRACKMUTESOLO / STRIATRACKGAIN (SR864, lifted here so both header writers share it): track `ti`'s event channel
/// word (`V`, `V2`, `A`, `A2` ...) when the EDL carries the track (a track of its kind at or after it holds a clip: the
/// reader makes tracks up to the highest channel an event names); `None` for a track the reader would not re-create.
fn carried_ch(tl: &Timeline, ti: usize) -> Option<String> {
    let t = tl.tracks.get(ti)?;
    if !tl.tracks[ti..].iter().any(|u| u.kind == t.kind && !u.clips.is_empty()) {
        return None;
    }
    let num = tl.track_number(ti).unwrap_or(1);
    let k = if t.kind == Kind::Video { "V" } else { "A" };
    Some(if num == 1 { String::from(k) } else { format!("{k}{num}") })
}

/// STRIATRACKMUTESOLO: the post-pass over `text` applying every `* UNA MUTE` / `* UNA SOLO` line to `tl` (its two arms).
fn track_flags(text: &str, tl: &mut Timeline) -> Result<(), EdlError> {
    let mut seen: Vec<(bool, usize)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let l = raw.trim();
        let (solo, r) = match (l.strip_prefix("* UNA MUTE "), l.strip_prefix("* UNA SOLO ")) {
            (Some(r), _) => (false, r),
            (_, Some(r)) => (true, r),
            _ => continue,
        };
        let w: Vec<&str> = r.split_whitespace().collect();
        if w.len() != 1 {
            return Err(EdlError::Syntax { line });
        }
        let (kind, num) = channel(w[0]).ok_or(EdlError::Syntax { line })?;
        let ti = (0..tl.tracks.len()).filter(|&t| tl.tracks[t].kind == kind).nth(num - 1).ok_or(EdlError::Syntax { line })?;
        if seen.contains(&(solo, ti)) {
            return Err(EdlError::Syntax { line });
        }
        seen.push((solo, ti));
        let t = &tl.tracks[ti];
        let (m, s) = if solo { (t.muted, true) } else { (true, t.solo) };
        *tl = tl.set_track_flags(ti, m, s).map_err(|_| EdlError::Syntax { line })?;
    }
    Ok(())
}

#[cfg(test)]
mod striatrackmutesolo_tests {
    use super::*;

    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        for k in [Kind::Video, Kind::Video, Kind::Audio, Kind::Audio] {
            let t = tl.add_track(k);
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 10, 0)];
        }
        tl
    }

    /// M3: a muted A2 and a soloed V round trip; the lines sit last in the header; write(parse(write)) is byte-identical;
    /// a timeline with no flag writes no line (the goldens' bytes).
    #[test]
    fn striatrackmutesolo_edl_round_trip() {
        let plain = write(&tl(), "T").unwrap();
        assert!(!plain.contains("* UNA MUTE") && !plain.contains("* UNA SOLO"));
        let f = tl().set_track_flags(3, true, false).unwrap().set_track_flags(0, false, true).unwrap();
        let text = write(&f, "T").unwrap();
        assert!(text.contains("* UNA SOURCE A 00:00:02:00\n* UNA MUTE A2\n* UNA SOLO V\n\n001"), "{text}");
        let back = parse(&text, 25).unwrap();
        assert_eq!(back, f);
        assert_eq!(write(&back, "T").unwrap(), text);
        assert_eq!(plain.replace("\n\n001", "\n* UNA MUTE A2\n* UNA SOLO V\n\n001"), text, "only the two lines differ");
    }

    /// M3: each refusal names its line; an empty trailing track's flag is not written (the reader would not make it).
    #[test]
    fn striatrackmutesolo_edl_refusals() {
        let text = write(&tl(), "T").unwrap();
        let with = |extra: &str| parse(&text.replace("\n\n001", &format!("\n{extra}\n\n001")), 25);
        let at = text.lines().position(|l| l.is_empty()).unwrap() + 1;
        assert_eq!(with("* UNA MUTE"), Ok(tl()), "no space after the word: not this line (a comment)");
        assert_eq!(with("* UNA MUTE A A2"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA SOLO X"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA MUTE A3"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA MUTE V2\n* UNA MUTE V2"), Err(EdlError::Syntax { line: at + 1 }));
        assert_eq!(with("* UNA MUTE V1\n* UNA SOLO V1").map(|t| (t.tracks[0].muted, t.tracks[0].solo)), Ok((true, true)));
        let mut empty = tl();
        let t = empty.add_track(Kind::Audio);
        empty = empty.set_track_flags(t, true, false).unwrap();
        assert_eq!(write(&empty, "T").unwrap(), write(&tl(), "T").unwrap());
    }
}

// ---- STRIACLIPSPEED (LEDGER SR829): the CMX 3600 `M2` motion-memory line (named tail block) ----
// A clip at a rate other than 1x writes ONE line directly under its event line: `M2   <reel> <speed> <source in>`, the
// speed being fps x rate written EXACTLY (`{:03}` integer part, then every decimal digit the 2^-16 fraction needs, at least
// one: `050.0` is 2x at 25 fps, `037.5` 1.5x, `012.5` 0.5x), so `write(parse(write))` stays byte-identical; a 1x clip writes
// none, so every 1x EDL writes as before. The event line keeps source in / out = the source and record in / out = the
// timeline (`at`, `end()`), so a rated event's spans differ: that is accepted only when an M2 line follows it directly, and
// the M2 line checks `len()` at its rate against the record span (`Duration`, at the event's line). Refused by name: an M2
// line not directly under an event that made a clip (none, a `BL`, a transition's zero-length outgoing line), another reel,
// a source in other than the event's, or an unreadable speed (`Syntax`); a `-` or zero speed (`Edit(Rate)`: no reverse, no
// dwell). A hand-written speed is read exactly and rounded once to Q16.16 (`crate::speed_q16`).

/// STRIACLIPSPEED: line `i + 1` of `text` (0-based) is an M2 line.
fn m2_next(text: &str, i: usize) -> bool {
    text.lines().nth(i + 1).is_some_and(|n| n.trim().starts_with("M2 "))
}

/// STRIACLIPSPEED: apply the M2 line `l` (0-based line `i` of `text`, 1-based `line`) to the clip of the event line directly
/// above it (`clip`: `None` when that event made no clip).
fn m2_comment(text: &str, i: usize, l: &str, fps: u32, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let c = clip.ok_or(EdlError::Syntax { line })?;
    let prev = i.checked_sub(1).and_then(|k| text.lines().nth(k)).map(str::trim).ok_or(EdlError::Syntax { line })?;
    let e: Vec<&str> = prev.split_whitespace().collect();
    if e.len() < 8 || !e[0].bytes().all(|b| b.is_ascii_digit()) {
        return Err(EdlError::Syntax { line });
    }
    let w: Vec<&str> = l.split_whitespace().collect();
    if w.len() != 4 || w[0] != "M2" || w[1] != e[1] || parse_tc(w[3], fps, line)? != c.src_in {
        return Err(EdlError::Syntax { line });
    }
    if w[2].starts_with('-') {
        return Err(EdlError::Edit(EditError::Rate));
    }
    let q = crate::speed_q16(w[2], fps).ok_or(EdlError::Syntax { line })?;
    if q == 0 {
        return Err(EdlError::Edit(EditError::Rate));
    }
    let n = e.len();
    let span = parse_tc(e[n - 1], fps, line - 1)? - parse_tc(e[n - 2], fps, line - 1)?;
    c.rate_q16 = q;
    if c.len() != span {
        return Err(EdlError::Duration { line: line - 1 });
    }
    Ok(())
}

/// STRIACLIPSPEED: clip `c`'s M2 line (empty at 1x).
fn m2_line(reel: &str, c: &Clip, fps: u32) -> String {
    if c.rate_q16 == 0x1_0000 {
        return String::new();
    }
    format!("M2   {:<8} {} {}\n", reel, m2_speed(c.rate_q16, fps), timecode(c.src_in, fps))
}

/// STRIACLIPSPEED: `rate_q16 * fps / 0x1_0000` as an exact decimal: `{:03}` integer part, every needed decimal digit (>= 1).
fn m2_speed(rate_q16: u32, fps: u32) -> String {
    let v = rate_q16 as u64 * fps as u64;
    let mut s = format!("{:03}.", v >> 16);
    let mut r = v & 0xFFFF;
    loop {
        r *= 10;
        s.push((b'0' + (r >> 16) as u8) as char);
        r &= 0xFFFF;
        if r == 0 {
            break;
        }
    }
    s
}

#[cfg(test)]
mod striaclipspeed_tests {
    extern crate std;
    use super::*;
    use crate::{Clip, Kind, Timeline};

    /// V1 A[13,23)@0 at `rate`, A[30,40)@25; A1 A[13,23)@0 at `rate`.
    fn rated(rate: u32) -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 13, 23, 0).at_rate(rate), Clip::new(a, 30, 40, 25)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 13, 23, 0).at_rate(rate)];
        tl
    }

    #[test]
    fn striaclipspeed_m2_round_trip() {
        let tl = rated(0x2_0000);
        let s = write(&tl, "SPEED").unwrap();
        assert!(s.contains("001  A        V     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:05\nM2   A        050.0 00:00:00:13\n"), "{s}");
        assert_eq!(s.matches("M2 ").count(), 2, "{s}");
        let back = parse(&s, 25).unwrap();
        assert_eq!(back, tl);
        assert_eq!(write(&back, "SPEED").unwrap(), s, "write(parse(write)) byte-identical");
        for (q, word) in [(0x1_8000, "037.5"), (0x8000, "012.5"), (0x1_5555, "033.3332061767578125")] {
            let t = rated(q);
            let s = write(&t, "SPEED").unwrap();
            assert!(s.contains(&format!("M2   A        {word} 00:00:00:13\n")), "{s}");
            assert_eq!(parse(&s, 25).unwrap(), t, "{word}");
        }
        assert!(!write(&rated(0x1_0000), "SPEED").unwrap().contains("M2"), "a 1x EDL writes no M2");
        std::println!("striaclipspeed: M2 050.0 / 037.5 / 012.5 round trip byte-identical; 1x writes none");
    }

    #[test]
    fn striaclipspeed_m2_refusals_by_name() {
        let s = write(&rated(0x2_0000), "SPEED").unwrap();
        let m2 = "M2   A        050.0 00:00:00:13\n";
        let lost = s.replacen(m2, "", 1);
        assert!(matches!(parse(&lost, 25), Err(EdlError::Duration { .. })), "spans differ with no M2");
        let stray = s.replacen("* UNA SOURCE A 00:00:02:00\n", &format!("* UNA SOURCE A 00:00:02:00\n{m2}"), 1);
        assert!(matches!(parse(&stray, 25), Err(EdlError::Syntax { .. })), "an M2 under no event");
        let reel = s.replacen(m2, "M2   B        050.0 00:00:00:13\n", 1);
        assert!(matches!(parse(&reel, 25), Err(EdlError::Syntax { .. })), "another reel");
        let tc = s.replacen(m2, "M2   A        050.0 00:00:00:14\n", 1);
        assert!(matches!(parse(&tc, 25), Err(EdlError::Syntax { .. })), "another source in");
        let slow = s.replacen(m2, "M2   A        025.0 00:00:00:13\n", 1);
        assert!(matches!(parse(&slow, 25), Err(EdlError::Duration { .. })), "a speed that does not fit the record span");
        for bad in ["-050.0", "000.0"] {
            let t = s.replacen(m2, &format!("M2   A        {bad} 00:00:00:13\n"), 1);
            assert!(matches!(parse(&t, 25), Err(EdlError::Edit(EditError::Rate))), "{bad}");
        }
        let junk = s.replacen(m2, "M2   A        fast 00:00:00:13\n", 1);
        assert!(matches!(parse(&junk, 25), Err(EdlError::Syntax { .. })), "unreadable speed");
        std::println!("striaclipspeed: M2 refusals: stray/reel/tc/junk Syntax, wrong speed Duration, -/0 Rate");
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
        assert_eq!(ok.markers, alloc::vec![crate::Marker { at: 12, name: "M1".into(), colour: 0, len: 0 }]);
    }
}

// ---- STRIAMARKERJUMPKEYSNEXT (LEDGER SR984): the `* UNA MCOLOUR` / `* UNA MRANGE` lines (named tail block) ----
// A marker's non-default colour and range length ([`crate::Marker::colour`] / `len`) are kept as their own header lines,
// written after every `* UNA MARKER` line in tick order and only when not the default, so an EDL whose markers carry
// neither writes as before: `* UNA MCOLOUR <HH:MM:SS:FF> <colour name>` and `* UNA MRANGE <HH:MM:SS:FF> <end HH:MM:SS:FF>`.
// Read after the MARKER lines they belong to. A bad timecode is `Timecode`; no marker at the tick, an unknown colour, an
// end at or before the tick, or the field already set (a second line for one marker) is a syntax error.

/// STRIAMARKERJUMPKEYSNEXT: a `* UNA MCOLOUR` / `* UNA MRANGE` line applied to the timeline being read; any other line is
/// left alone.
fn marker_style_comment(l: &str, fps: u32, line: usize, tl: &mut Timeline) -> Result<(), EdlError> {
    let (colour, r) = match (l.strip_prefix("* UNA MCOLOUR "), l.strip_prefix("* UNA MRANGE ")) {
        (Some(r), _) => (true, r),
        (None, Some(r)) => (false, r),
        (None, None) => return Ok(()),
    };
    let w: Vec<&str> = r.split_whitespace().collect();
    if w.len() != 2 {
        return Err(EdlError::Syntax { line });
    }
    let at = parse_tc(w[0], fps, line)?;
    let m = tl.marker_at(at).ok_or(EdlError::Syntax { line })?;
    let next = if colour {
        let c = crate::marker_colour(w[1]).filter(|_| m.colour == 0).ok_or(EdlError::Syntax { line })?;
        tl.colour_marker(at, c)
    } else {
        let end = parse_tc(w[1], fps, line)?;
        if m.len != 0 || end <= at {
            return Err(EdlError::Syntax { line });
        }
        tl.range_marker(at, end - at)
    };
    *tl = next.ok_or(EdlError::Syntax { line })?;
    Ok(())
}

/// STRIAMARKERJUMPKEYSNEXT: the header's `* UNA MCOLOUR` / `* UNA MRANGE` lines (empty when every marker is a plain
/// orange point).
fn marker_style_lines(tl: &Timeline, fps: u32) -> String {
    let mut s = String::new();
    for m in &tl.markers {
        if m.colour != 0 {
            s.push_str(&format!("* UNA MCOLOUR {} {}\n", timecode(m.at, fps), crate::MARKER_COLOURS[usize::from(m.colour)]));
        }
        if m.len != 0 {
            s.push_str(&format!("* UNA MRANGE {} {}\n", timecode(m.at, fps), timecode(m.at + m.len, fps)));
        }
    }
    s
}

#[cfg(test)]
mod striamarkerjumpkeysnext_tests {
    use super::*;
    use crate::{Clip, Kind, Timeline};

    fn cut() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl.mark(12, "M1").unwrap().mark(15, "M2").unwrap()
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: plain markers write no style line; a red ranged marker writes MCOLOUR then MRANGE right
    /// after the MARKER lines and round-trips byte for byte.
    #[test]
    fn striamarkerjumpkeysnext_style_lines_round_trip() {
        let plain = write(&cut(), "T").unwrap();
        assert!(!plain.contains("UNA MCOLOUR") && !plain.contains("UNA MRANGE"), "{plain}");
        let tl = cut().colour_marker(12, 1).unwrap().range_marker(12, 8).unwrap().colour_marker(15, 5).unwrap();
        let s = write(&tl, "T").unwrap();
        let want = "* UNA MARKER 00:00:00:15 \"M2\"\n* UNA MCOLOUR 00:00:00:12 red\n* UNA MRANGE 00:00:00:12 00:00:00:20\n* UNA MCOLOUR 00:00:00:15 blue\n\n";
        assert!(s.contains(want), "{s}");
        let back = parse(&s, 25).unwrap();
        assert_eq!(back, tl);
        assert_eq!(write(&back, "T").unwrap(), s);
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: a malformed or orphan style line is refused by name.
    #[test]
    fn striamarkerjumpkeysnext_malformed_style_lines_refused() {
        let head = write(&cut(), "T").unwrap();
        let after = "* UNA MARKER 00:00:00:15 \"M2\"\n";
        let at = head.find(after).expect("the MARKER lines") + after.len();
        let n = head[..at].lines().count() + 1;
        let with = |l: &str| alloc::format!("{}{l}\n{}", &head[..at], &head[at..]);
        for bad in ["* UNA MCOLOUR 00:00:00:13 red", "* UNA MCOLOUR 00:00:00:12 mauve", "* UNA MCOLOUR 00:00:00:12", "* UNA MCOLOUR 00:00:00:12 red x",
                    "* UNA MRANGE 00:00:00:12 00:00:00:12", "* UNA MRANGE 00:00:00:12 00:00:00:10", "* UNA MRANGE 00:00:00:13 00:00:00:20",
                    "* UNA MCOLOUR 00:00:00:12 red\n* UNA MCOLOUR 00:00:00:12 blue", "* UNA MRANGE 00:00:00:12 00:00:00:20\n* UNA MRANGE 00:00:00:12 00:00:00:22"] {
            let line = n + bad.matches('\n').count();
            assert_eq!(parse(&with(bad), 25), Err(EdlError::Syntax { line }), "{bad}");
        }
        assert_eq!(parse(&with("* UNA MRANGE 00:00:0x:12 00:00:00:20"), 25), Err(EdlError::Timecode { line: n }));
        let ok = parse(&with("* UNA MCOLOUR 00:00:00:12 green"), 25).unwrap();
        assert_eq!(ok.marker_at(12).map(|m| (m.colour, m.len)), Some((3, 0)));
    }
}

// ---- STRIATRACKADDREMOVE (LEDGER SR856): the `* UNA TRACKS` line (named tail block) ----
// `* UNA TRACKS <nv> <na>` is the timeline's video and audio track counts. The events alone carry a kind's tracks only up
// to the highest channel an event names, so an EMPTY TRAILING track would be dropped by a save; the line is written (last
// in the header, before the blank line) ONLY when a count exceeds what the events carry, so every EDL without such a track
// writes as before. The reader takes the counts as a floor for the tracks it makes (V first, then A). Refused as a syntax
// error: not two integers, a count above `TRACKS_MAX`, a second TRACKS line.

/// STRIATRACKADDREMOVE: the `(video, audio)` floor of every `* UNA TRACKS` line in `text` (`(0, 0)` without one).
fn track_counts(text: &str) -> Result<(usize, usize), EdlError> {
    let mut got: Option<(usize, usize)> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let Some(r) = raw.trim().strip_prefix("* UNA TRACKS ") else { continue };
        let w: Vec<&str> = r.split_whitespace().collect();
        let n = |s: &str| s.parse::<usize>().ok().filter(|&n| n <= crate::TRACKS_MAX);
        let (Some(v), Some(a), 2, None) = (w.first().and_then(|s| n(s)), w.get(1).and_then(|s| n(s)), w.len(), got) else {
            return Err(EdlError::Syntax { line });
        };
        got = Some((v, a));
    }
    Ok(got.unwrap_or((0, 0)))
}

/// STRIATRACKADDREMOVE: the header's `* UNA TRACKS` line, only when a kind holds more tracks than its events carry.
fn tracks_line(tl: &Timeline) -> String {
    let count = |k: Kind| tl.tracks.iter().filter(|t| t.kind == k).count();
    let carried = |k: Kind| (0..tl.tracks.len()).filter(|&i| tl.tracks[i].kind == k && !tl.tracks[i].clips.is_empty()).filter_map(|i| tl.track_number(i)).max().unwrap_or(0);
    let (nv, na) = (count(Kind::Video), count(Kind::Audio));
    if (nv, na) == (carried(Kind::Video), carried(Kind::Audio)) {
        return String::new();
    }
    format!("* UNA TRACKS {nv} {na}\n")
}

// ---- STRIATRACKGAIN (LEDGER SR864): the `* UNA TGAIN` line (named tail block) ----
// `* UNA TGAIN <ch> <cdb>` sets a track's [`crate::Track::gain_cdb`] (centi-dB, like `* UNA GAIN`); `<ch>` is the event
// channel word (SR837's shape: `carried_ch` writes it, `channel` reads it). One line per carried track whose gain is not
// 0, in track order, written last in the header (after the MUTE / SOLO lines, before the blank line), so an EDL with no
// track gain writes as before and `write(parse(write(tl)))` stays byte-identical. Read once the tracks exist; refused as a
// syntax error: not exactly two words, an unknown channel, a channel naming no track, a gain that is not an integer or is
// outside the clip range, the same track twice.

/// STRIATRACKGAIN: the header's `* UNA TGAIN <ch> <cdb>\n` lines.
fn tgain_lines(tl: &Timeline) -> String {
    let mut s = String::new();
    for (ti, t) in tl.tracks.iter().enumerate() {
        if let Some(ch) = carried_ch(tl, ti).filter(|_| t.gain_cdb != 0) {
            s.push_str(&format!("* UNA TGAIN {ch} {}\n", t.gain_cdb));
        }
    }
    s
}

/// STRIATRACKGAIN: the post-pass over `text` applying every `* UNA TGAIN` line to `tl`.
fn track_gains(text: &str, tl: &mut Timeline) -> Result<(), EdlError> {
    let mut seen: Vec<usize> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let Some(r) = raw.trim().strip_prefix("* UNA TGAIN ") else { continue };
        let w: Vec<&str> = r.split_whitespace().collect();
        if w.len() != 2 {
            return Err(EdlError::Syntax { line });
        }
        let (kind, num) = channel(w[0]).ok_or(EdlError::Syntax { line })?;
        let ti = (0..tl.tracks.len()).filter(|&t| tl.tracks[t].kind == kind).nth(num - 1).ok_or(EdlError::Syntax { line })?;
        let cdb: i32 = w[1].parse().map_err(|_| EdlError::Syntax { line })?;
        if seen.contains(&ti) {
            return Err(EdlError::Syntax { line });
        }
        seen.push(ti);
        *tl = tl.set_track_gain(ti, cdb).map_err(|_| EdlError::Syntax { line })?;
    }
    Ok(())
}

#[cfg(test)]
mod striatrackgain_tests {
    use super::*;

    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        for k in [Kind::Video, Kind::Audio, Kind::Audio] {
            let t = tl.add_track(k);
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 10, 0)];
        }
        tl
    }

    /// M3: A2 at -6.02 dB and A at +3 dB round trip; the lines sit last in the header (after SR837's MUTE); the second
    /// write is byte-identical; a timeline with no track gain writes no line (the goldens' bytes).
    #[test]
    fn striatrackgain_edl_round_trip() {
        let plain = write(&tl(), "T").unwrap();
        assert!(!plain.contains("* UNA TGAIN"));
        let g = tl().set_track_gain(2, -602).unwrap().set_track_gain(1, 300).unwrap().set_track_flags(2, true, false).unwrap();
        let text = write(&g, "T").unwrap();
        assert!(text.contains("* UNA MUTE A2\n* UNA TGAIN A 300\n* UNA TGAIN A2 -602\n\n001"), "{text}");
        let back = parse(&text, 25).unwrap();
        assert_eq!(back, g);
        assert_eq!(write(&back, "T").unwrap(), text);
        let only = tl().set_track_gain(2, -602).unwrap();
        assert_eq!(plain.replace("\n\n001", "\n* UNA TGAIN A2 -602\n\n001"), write(&only, "T").unwrap(), "only the one line differs");
    }

    /// M3: each refusal names its line; an empty trailing track's gain is not written (the reader would not make it).
    #[test]
    fn striatrackgain_edl_refusals() {
        let text = write(&tl(), "T").unwrap();
        let with = |extra: &str| parse(&text.replace("\n\n001", &format!("\n{extra}\n\n001")), 25);
        let at = text.lines().position(|l| l.is_empty()).unwrap() + 1;
        assert_eq!(with("* UNA TGAIN"), Ok(tl()), "no space after the word: not this line (a comment)");
        assert_eq!(with("* UNA TGAIN A"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA TGAIN A -600 1"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA TGAIN X -600"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA TGAIN A3 -600"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA TGAIN A -6.0"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA TGAIN A 2401"), Err(EdlError::Syntax { line: at }));
        assert_eq!(with("* UNA TGAIN A2 -600\n* UNA TGAIN A2 -300"), Err(EdlError::Syntax { line: at + 1 }));
        assert_eq!(with("* UNA TGAIN A1 -600").map(|t| t.tracks[1].gain_cdb), Ok(-600));
        let mut empty = tl();
        let t = empty.add_track(Kind::Audio);
        empty = empty.set_track_gain(t, -600).unwrap();
        assert_eq!(write(&empty, "T").unwrap(), write(&tl(), "T").unwrap());
    }
}

// ---- STRIACROP (LEDGER SR867): the clip's `* UNA CROP` line (named tail block) ----
// `* UNA CROP <l> <t> <r> <b>` belongs to the VIDEO event line just above it (the per-clip comment shape of `* UNA GAIN`),
// four integers in 1/1000 of the frame ([`crate::Crop`]), written only when the clip has a crop, so an EDL with no crop
// writes as before. One with no video event above it (none, a `BL` event, an audio event), not four integers, a crop
// keeping no picture, or a second CROP for one event is a syntax error.

/// STRIACROP: apply a `* UNA CROP` line to the video event above it (`None`: none there); any other line is not this one.
fn crop_comment(l: &str, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA CROP ") else { return Ok(()) };
    let c = clip.ok_or(EdlError::Syntax { line })?;
    let n: Vec<Option<u16>> = r.split_whitespace().map(|x| x.parse().ok()).collect();
    let crop = match n.as_slice() {
        [Some(l), Some(t), Some(r), Some(b)] => crate::Crop { l: *l, t: *t, r: *r, b: *b },
        _ => return Err(EdlError::Syntax { line }),
    };
    if !crop.ok() || crop == crate::Crop::NONE || c.crop != crate::Crop::NONE {
        return Err(EdlError::Syntax { line });
    }
    c.crop = crop;
    Ok(())
}

/// STRIACROP: the clip's `* UNA CROP` line, or nothing when it has no crop.
fn crop_line(c: &Clip) -> String {
    if c.crop == crate::Crop::NONE {
        return String::new();
    }
    format!("* UNA CROP {} {} {} {}\n", c.crop.l, c.crop.t, c.crop.r, c.crop.b)
}

// ---- STRIAOPACITY (LEDGER SR868): the `* UNA ALPHA` line (named tail block) ----
// `* UNA ALPHA <pct>` is the event line just above's opacity ([`crate::Clip::opacity`], percent 0..=100), written after
// its GAIN / FADE / XFADE lines and only when not opaque (an opaque timeline writes what it always wrote). One with no
// event above it (or under a `BL` event), or a word that is not 0..=100, is a syntax error; on an audio event `validate`
// refuses it (`EditError::Opacity`).

/// STRIAOPACITY: a `* UNA ALPHA <pct>` line applied to the event above (`None`: no event); any other line is left alone.
fn alpha_comment(l: &str, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA ALPHA ") else { return Ok(()) };
    let c = clip.ok_or(EdlError::Syntax { line })?;
    c.opacity = r.trim().parse::<u8>().ok().filter(|&p| p <= crate::OPAQUE).ok_or(EdlError::Syntax { line })?;
    Ok(())
}

/// STRIAOPACITY: the event's `* UNA ALPHA` line (empty for an opaque clip).
fn alpha_line(c: &Clip) -> String {
    if c.opacity == crate::OPAQUE { String::new() } else { format!("* UNA ALPHA {}\n", c.opacity) }
}

// ---- STRIACLIPLINK (LEDGER SR863): the `* UNA GROUP` line (named tail block) ----
// `* UNA GROUP <n>` (n a u32 > 0) is the event's link group ([`Clip::group`]), written after the event's other UNA lines and
// only when linked (an unlinked document's text is unchanged); no event above it or a bad word is `EdlError::Syntax`.

/// STRIACLIPLINK: a `* UNA GROUP <n>` line applied to the event above it (any other line: nothing).
fn group_comment(l: &str, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA GROUP ") else { return Ok(()) };
    let c = clip.ok_or(EdlError::Syntax { line })?;
    c.group = r.trim().parse::<u32>().ok().filter(|&g| g > 0).ok_or(EdlError::Syntax { line })?;
    Ok(())
}

/// STRIACLIPLINK: the `* UNA GROUP` line of a linked clip (empty for group 0).
fn group_line(c: &Clip) -> String {
    if c.group == 0 { String::new() } else { format!("* UNA GROUP {}\n", c.group) }
}

// ---- STRIACOLORLIFT (LEDGER SR869): the `* UNA COLOR` line (named tail block) ----
// `* UNA COLOR <b> <c> <s>` is the clip's [`crate::Grade`] (three integers in 1/1000), the per-clip comment shape of
// `* UNA GAIN`: it belongs to the event line just above, is written only when not identity (every pinned EDL unchanged) and
// a later one overwrites. No event above it (or under a `BL` event) or misshapen words: `Syntax`; a value out of range or a
// graded audio event is refused by `validate` (`Edit(Grade)` / `Edit(Kind)`).

/// STRIACOLORLIFT: a `* UNA COLOR` line applied to the event above it (`None`: no event); any other line is left alone.
fn grade_comment(l: &str, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA COLOR ") else { return Ok(()) };
    let c = clip.ok_or(EdlError::Syntax { line })?;
    let w: Vec<i16> = r.split_whitespace().map(|x| x.parse::<i16>()).collect::<Result<_, _>>().map_err(|_| EdlError::Syntax { line })?;
    let [b, k, s] = w[..] else { return Err(EdlError::Syntax { line }) };
    c.grade = crate::Grade { brightness: b, contrast: k, saturation: s };
    Ok(())
}

/// STRIACOLORLIFT: the clip's `* UNA COLOR` line (empty at identity).
fn grade_line(c: &Clip) -> String {
    let g = c.grade;
    if g == crate::Grade::IDENTITY { String::new() } else { format!("* UNA COLOR {} {} {}\n", g.brightness, g.contrast, g.saturation) }
}

// ---- STRIAPAN (LEDGER SR861): the `* UNA PAN` line (named tail block) ----
// `* UNA PAN <cq8>` is the event's [`Clip::pan_cq8`] (an integer -128..=127; absent = 0, so a centred clip writes no line),
// written after `* UNA GAIN` / `* UNA FADE` and read like them onto the event line just above it.

/// STRIAPAN: a `* UNA PAN <cq8>` line applied to the event above it; any other line is not this one (`Ok`). No event
/// above, or a value that is not one i8: `Syntax`.
fn pan_comment(l: &str, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA PAN ") else { return Ok(()) };
    clip.ok_or(EdlError::Syntax { line })?.pan_cq8 = r.trim().parse().map_err(|_| EdlError::Syntax { line })?;
    Ok(())
}

/// STRIAPAN: the `* UNA PAN` line of clip `c` (empty when centred).
fn pan_line(c: &Clip) -> String {
    if c.pan_cq8 != 0 { format!("* UNA PAN {}\n", c.pan_cq8) } else { String::new() }
}

// ---- STRIAPOSITIONSCALE (LEDGER SR879): the clip's `* UNA XFORM` line (named tail block) ----
// `* UNA XFORM <x> <y> <s>` belongs to the VIDEO event line just above it (the per-clip comment shape of `* UNA CROP`),
// three integers in 1/1000 of the frame ([`crate::Xform`]), written right after the clip's CROP line and only when it is
// placed, so an EDL with no placement writes as before. One with no video event above it (none, a `BL` event, an audio
// event), not three integers, a rect off the frame, the whole frame, or a second XFORM for one event is a syntax error.

/// STRIAPOSITIONSCALE: apply a `* UNA XFORM` line to the video event above it (`None`: none there); any other line is not this one.
fn xform_comment(l: &str, line: usize, clip: Option<&mut Clip>) -> Result<(), EdlError> {
    let Some(r) = l.strip_prefix("* UNA XFORM ") else { return Ok(()) };
    let c = clip.ok_or(EdlError::Syntax { line })?;
    let n: Vec<Option<u16>> = r.split_whitespace().map(|x| x.parse().ok()).collect();
    let xf = match n.as_slice() {
        [Some(x), Some(y), Some(s)] => crate::Xform { x: *x, y: *y, s: *s },
        _ => return Err(EdlError::Syntax { line }),
    };
    if !xf.ok() || xf == crate::Xform::NONE || c.xform != crate::Xform::NONE {
        return Err(EdlError::Syntax { line });
    }
    c.xform = xf;
    Ok(())
}

/// STRIAPOSITIONSCALE: the clip's `* UNA XFORM` line, or nothing when it is not placed.
fn xform_line(c: &Clip) -> String {
    if c.xform == crate::Xform::NONE {
        return String::new();
    }
    format!("* UNA XFORM {} {} {}\n", c.xform.x, c.xform.y, c.xform.s)
}

// ---- STRIATCMORE (LEDGER SR964): drop-frame timecode text (named tail block) ----
// SMPTE drop-frame labels for the NTSC rates (30000/1001 at nominal 30, 60000/1001 at nominal 60): `drop` = nominal / 15
// frame NUMBERS (2 or 4) are skipped at the start of every minute except each tenth, so the label keeps wall-clock time.
// The tick is renumbered and formatted through `timecode` (ONE formatter), the frame separator `;`. The EDL still refuses
// drop-frame (`parse_tc`): this is the readout's text. Owed: NLE-TC-DROPFRAME-MODEL, NLE-TC-KERNEL.

/// STRIATCMORE (SR964): `HH:MM:SS;FF` drop-frame for tick `t` at `nominal` 30 or 60; any other nominal is [`timecode`].
pub fn timecode_drop(t: i64, nominal: u32) -> String {
    if nominal != 30 && nominal != 60 {
        return timecode(t, nominal);
    }
    let (t, nom) = (t.max(0), nominal as i64);
    let drop = nom / 15;
    let (per10, per1) = (nom * 600 - 9 * drop, nom * 60 - drop);
    let (d, m) = (t / per10, t % per10);
    let n = t + 9 * drop * d + if m >= drop { drop * ((m - drop) / per1) } else { 0 };
    let mut s = timecode(n, nominal);
    if let Some(i) = s.rfind(':') {
        s.replace_range(i..i + 1, ";");
    }
    s
}
