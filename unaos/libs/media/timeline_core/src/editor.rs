// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The editor's state (NLE2 LEDGER SR201's model, moved here by NLE6 LEDGER SR207).
//!
//! CHARTER: Stria — shared-core. ONE editor state both faces drive (BP-handlers-4/-14): the host face
//! (`stria::nle_face`, which re-exports this module) and the on-UnaOS face (the kernel Player's EDL mode).
//!
//! [`EditorState`]: the timeline, the selected clip, the playhead, the zoom (px per tick) and the undo / redo stacks.
//! Every edit is ONE of this crate's pure ops (`split`, `trim_in`, `trim_out`, `ripple_delete`) and goes through ONE
//! `commit`: the op's returned `Timeline` replaces the current one, which goes on the undo stack (undo is the stack of
//! returned values; there are no inverse ops). A refused edit leaves the state unchanged. No painter lives here.

use alloc::vec::Vec;

use crate::{Clip, EditError, Timeline, Transition, edl};

/// Undo depth kept.
pub const UNDO_MAX: usize = 256;
pub const ZOOM_MIN: u32 = 1;
pub const ZOOM_MAX: u32 = 64;
/// The zoom a fresh state opens at (px per tick).
pub const ZOOM_DEFAULT: u32 = 8;

/// Which edge of the selected clip a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    In,
    Out,
}

/// The editor's state (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct EditorState {
    pub timeline: Timeline,
    /// `(track, clip index)`.
    pub selection: Option<(usize, usize)>,
    /// NLE2c (SR217): every selected `(track, clip index)`, sorted and deduped; empty exactly when `selection` (the
    /// primary) is `None`, and holding it otherwise.
    pub set: Vec<(usize, usize)>,
    pub playhead: i64,
    /// Pixels per tick, `ZOOM_MIN..=ZOOM_MAX`.
    pub zoom: u32,
    undo: Vec<Timeline>,
    redo: Vec<Timeline>,
    /// NLE3d (SR302): the selected transition, `(track, index into tracks[track].transitions)`; the face's widget.
    pub transition: Option<(usize, usize)>,
}

impl EditorState {
    pub fn new(timeline: Timeline) -> EditorState {
        EditorState { timeline, selection: None, set: Vec::new(), playhead: 0, zoom: ZOOM_DEFAULT, undo: Vec::new(), redo: Vec::new(), transition: None }
    }
    /// Open a CMX 3600 EDL (`fps` when it carries no `* UNA FPS` line).
    pub fn open(text: &str, fps: u32) -> Result<EditorState, edl::EdlError> {
        Ok(EditorState::new(edl::parse(text, fps)?))
    }
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }
    /// The selected clip.
    pub fn selected(&self) -> Option<Clip> {
        let (t, i) = self.selection?;
        self.timeline.tracks.get(t)?.clips.get(i).copied()
    }
    pub fn select(&mut self, track: usize, idx: usize) -> Result<(), EditError> {
        let tr = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?;
        if idx >= tr.clips.len() {
            return Err(EditError::NoClip);
        }
        self.selection = Some((track, idx));
        self.set.clear();
        self.set.push((track, idx));
        Ok(())
    }
    /// Select the clip on `track` showing tick `t`.
    pub fn select_at(&mut self, track: usize, t: i64) -> Result<(), EditError> {
        if track >= self.timeline.tracks.len() {
            return Err(EditError::NoTrack);
        }
        let idx = self.timeline.clip_at(track, t).ok_or(EditError::NoClip)?;
        self.select(track, idx)
    }
    pub fn deselect(&mut self) {
        self.selection = None;
        self.set.clear();
    }
    /// Move the playhead to `t`, clamped to `0..=timeline.end()`.
    pub fn seek(&mut self, t: i64) {
        self.playhead = t.clamp(0, self.timeline.end().max(0));
    }
    pub fn nudge(&mut self, dt: i64) {
        self.seek(self.playhead.saturating_add(dt));
    }
    /// Cut the selected track's clip at the playhead (`Timeline::split`); the right half is selected.
    pub fn split_at_playhead(&mut self) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.split(track, self.playhead)?;
        self.commit(tl);
        self.select_at(track, self.playhead).unwrap_or_else(|_| self.deselect());
        Ok(())
    }
    /// Move the selected clip's in (`Timeline::trim_in`) or out (`Timeline::trim_out`) point by `delta` source ticks.
    pub fn trim(&mut self, edge: Edge, delta: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        let tl = match edge {
            Edge::In => self.timeline.trim_in(track, idx, c.src_in + delta)?,
            Edge::Out => self.timeline.trim_out(track, idx, c.src_out + delta)?,
        };
        self.commit(tl);
        Ok(())
    }
    /// Move the selected clip to `to_at` on `to_track` (`Timeline::move_clip`); the selection follows the clip.
    /// (NLE2B SR208's op, carried with the move.)
    pub fn move_selected(&mut self, to_track: usize, to_at: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.move_clip(track, idx, to_track, to_at)?;
        self.commit(tl);
        self.select_at(to_track, to_at).unwrap_or_else(|_| self.deselect());
        Ok(())
    }
    /// Remove the selected clips and close the holes (NLE2c: `Timeline::ripple_delete_many` over the set, ONE edit; a set
    /// of one is `Timeline::ripple_delete`).
    pub fn ripple_delete(&mut self) -> Result<(), EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.ripple_delete_many(&self.set)?;
        self.commit(tl);
        self.deselect();
        Ok(())
    }
    /// Step back one edit; false when there is none.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        let cur = core::mem::replace(&mut self.timeline, prev);
        self.redo.push(cur);
        self.settle();
        true
    }
    /// Re-apply the last undone edit; false when there is none.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        let cur = core::mem::replace(&mut self.timeline, next);
        self.undo.push(cur);
        self.settle();
        true
    }
    /// Double (`step > 0`) or halve (`step < 0`) the zoom, `|step|` times, within `ZOOM_MIN..=ZOOM_MAX`.
    pub fn zoom_by(&mut self, step: i32) {
        let mut z = self.zoom;
        for _ in 0..step.unsigned_abs().min(8) {
            z = if step > 0 { z.saturating_mul(2) } else { z / 2 };
        }
        self.zoom = z.clamp(ZOOM_MIN, ZOOM_MAX);
    }
    /// THE one edit path: the op's value replaces the timeline, the old one goes on the undo stack.
    fn commit(&mut self, tl: Timeline) {
        let old = core::mem::replace(&mut self.timeline, tl);
        self.undo.push(old);
        if self.undo.len() > UNDO_MAX {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.settle();
    }
    /// Keep the selection and the playhead valid for the current timeline.
    fn settle(&mut self) {
        let tl = &self.timeline;
        self.set.retain(|&(t, i)| tl.tracks.get(t).is_some_and(|tr| i < tr.clips.len()));
        self.transition = self.transition.filter(|&(t, i)| tl.tracks.get(t).is_some_and(|tr| i < tr.transitions.len()));
        if self.selected().is_none() {
            self.selection = self.set.first().copied();
        }
        if let Some(p) = self.selection.filter(|p| !self.set.contains(p)) {
            self.set.push(p);
            self.set.sort_unstable();
        }
        self.seek(self.playhead);
    }
}

/// NLE2c (LEDGER SR217): the selection as a SET (`set`, with `selection` its primary) and the edits over it, each ONE
/// `commit` of one of timeline_core's set ops. no_std-clean (it moves with `EditorState` into `timeline_core::editor`).
impl EditorState {
    /// Add a clip to the set (a shift-press); it becomes the primary.
    pub fn select_add(&mut self, track: usize, idx: usize) -> Result<(), EditError> {
        let tr = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?;
        if idx >= tr.clips.len() {
            return Err(EditError::NoClip);
        }
        self.selection = Some((track, idx));
        if !self.set.contains(&(track, idx)) {
            self.set.push((track, idx));
            self.set.sort_unstable();
        }
        Ok(())
    }
    /// Select `picks` (a marquee): added to the set when `additive`, else replacing it (none -> nothing selected). The
    /// primary is the first pick unless `additive` keeps one.
    pub fn select_many(&mut self, picks: &[(usize, usize)], additive: bool) {
        if !additive {
            self.deselect();
        }
        let keep = self.selection;
        for &(t, i) in picks {
            let _ = self.select_add(t, i);
        }
        if keep.is_some() {
            self.selection = keep;
        } else {
            self.selection = self.set.first().copied();
        }
    }
    /// Select every clip on every track (Cmd / Ctrl+A).
    pub fn select_all(&mut self) {
        let mut all = Vec::new();
        for (t, tr) in self.timeline.tracks.iter().enumerate() {
            for i in 0..tr.clips.len() {
                all.push((t, i));
            }
        }
        self.select_many(&all, false);
    }
    /// Remove the selected clips and leave the holes (`Timeline::lift_many`, ONE edit).
    pub fn lift_selected(&mut self) -> Result<(), EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.lift_many(&self.set)?;
        self.commit(tl);
        self.deselect();
        Ok(())
    }
    /// Move every selected clip by `delta` ticks on its own track (`Timeline::move_many`, ONE edit; refused whole if any
    /// would overlap). The set and the primary follow their clips.
    pub fn move_set(&mut self, delta: i64) -> Result<(), EditError> {
        let primary = self.selection.ok_or(EditError::NoClip)?;
        let tl = match self.pair_pick() { Some((t, i)) => self.timeline.move_pair(t, i, delta)?, None => self.timeline.move_many(&self.set, delta)? };
        let at = |st: &EditorState, (t, i): (usize, usize)| st.timeline.tracks[t].clips[i].at + delta;
        let landed: Vec<(usize, i64)> = self.set.iter().map(|&p| (p.0, at(self, p))).collect();
        let p_at = (primary.0, at(self, primary));
        self.commit(tl);
        self.set.clear();
        for &(t, a) in &landed {
            if let Some(i) = self.timeline.clip_at(t, a) {
                self.set.push((t, i));
            }
        }
        self.set.sort_unstable();
        self.selection = self.timeline.clip_at(p_at.0, p_at.1).map(|i| (p_at.0, i));
        self.settle();
        Ok(())
    }
}

/// NLE3d (LEDGER SR302): the transition selection (the face's widget on a cut) and its edits, each ONE `commit` of one of
/// timeline_core's transition ops: `remove_transition`, and a length change as `add_transition` /
/// `add_transition_from_black` at the same `at` (the op replaces the transition there and keeps its index).
impl EditorState {
    /// Select the transition `idx` of `track` (the clips are deselected).
    pub fn select_transition(&mut self, track: usize, idx: usize) -> Result<(), EditError> {
        let tr = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?;
        if idx >= tr.transitions.len() {
            return Err(EditError::Transition);
        }
        self.deselect();
        self.transition = Some((track, idx));
        Ok(())
    }
    /// The selected transition.
    pub fn selected_transition(&self) -> Option<Transition> {
        let (t, i) = self.transition?;
        self.timeline.tracks.get(t)?.transitions.get(i).copied()
    }
    /// Take the selected transition off its cut (`Timeline::remove_transition`); the selection goes.
    pub fn remove_selected_transition(&mut self) -> Result<(), EditError> {
        let (track, _) = self.transition.ok_or(EditError::Transition)?;
        let x = self.selected_transition().ok_or(EditError::Transition)?;
        let tl = self.timeline.remove_transition(track, x.at)?;
        self.commit(tl);
        self.transition = None;
        Ok(())
    }
    /// Give the selected transition `dur` ticks (the add op of its kind at the same `at`; `validate` refuses `dur < 1`
    /// and a handle too short). The selection stays on it.
    pub fn set_transition_dur(&mut self, dur: i64) -> Result<(), EditError> {
        let (track, idx) = self.transition.ok_or(EditError::Transition)?;
        let x = self.selected_transition().ok_or(EditError::Transition)?;
        let tl = if x.to_black { self.timeline.add_transition_to_black(track, x.at, x.kind, dur)? } else if x.from_black {
            self.timeline.add_transition_from_black(track, x.at, x.kind, dur)?
        } else {
            self.timeline.add_transition(track, x.at, x.kind, dur)?
        };
        self.commit(tl);
        self.transition = Some((track, idx));
        Ok(())
    }
}

/// NLE6KEYS (rmbp-ledger B961): THE verb dispatch both faces call — the host face's buttons and keys (`stria::nle_face`
/// `Face::verb` / `Face::key`) and the kernel Player's editing keys (`video/vplay.rs` `edit_key`). Each face keeps its own
/// key BINDINGS (the host's `KEYS` names, the kernel's byte table) and maps them onto these words; the arms are the host
/// face's, moved here unchanged. `None`: not an editor verb (the face's own transport, e.g. `play`, stays the face's).
/// NLE6KEYSLABELS (rmbp-ledger B968): `zoom0` puts the zoom back to [`ZOOM_DEFAULT`] (the kernel's `0` key).
impl EditorState {
    pub fn verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let ph = self.playhead; if let Some(r) = self.size_verb(v) { return Some(r); } if let Some(r) = self.look_verb(v) { return Some(r); } if let Some(r) = self.gain_verb(v) { return Some(r); } if let Some(r) = self.track_verb(v) { return Some(r); } if let Some(r) = self.marker_verb(v) { return Some(r); } if let Some(r) = self.roll_verb(v) { return Some(r); } if let Some(r) = self.insert_verb(v) { return Some(r); } // STRIACLIPGAINFACE (SR838): the gain / fade words (file tail); NLETRANSITIONFACE (SR691): `kind` / `ease` (file tail); NLESIZEPOLICYUI (SR402): the `size` words (file tail); STRIATRACKMUTESOLO (SR837): `mute` / `solo` (file tail); STRIAROLLEDIT (SR851): the `roll` words (file tail); STRIAINSERTOVERWRITE (SR852): insert / overwrite (file tail)
        Some(match v {
            "prev" => {
                self.nudge(-1);
                Ok(())
            }
            "next" => {
                self.nudge(1);
                Ok(())
            }
            "split" => self.split_at_playhead(),
            "delete" => self.ripple_delete(),
            "undo" => {
                self.undo();
                Ok(())
            }
            "redo" => {
                self.redo();
                Ok(())
            }
            "zoom-" => {
                self.zoom_by(-1);
                Ok(())
            }
            "zoom+" => {
                self.zoom_by(1);
                Ok(())
            }
            "zoom0" => {
                self.zoom = ZOOM_DEFAULT;
                Ok(())
            }
            "[" => self.trim(Edge::In, -1),
            "]" => self.trim(Edge::In, 1),
            "{" => self.trim(Edge::Out, -1),
            "}" => self.trim(Edge::Out, 1),
            "lift" => self.lift_selected(),
            "mark-in" => match self.selected() {
                Some(c) => self.trim(Edge::In, c.src_at(ph) - c.src_in), // STRIACLIPSPEED (SR829): the playhead's source tick (1x: ph - at)
                None => Err(EditError::NoClip),
            },
            "mark-out" => match self.selected() {
                Some(c) => self.trim(Edge::Out, c.src_at(ph) - c.src_out), // STRIACLIPSPEED (SR829): the playhead's source tick (1x: ph - end)
                None => Err(EditError::NoClip),
            },
            "home" => {
                self.seek(0);
                Ok(())
            }
            "end" => {
                let e = self.timeline.end();
                self.seek(e);
                Ok(())
            }
            "nudge-" => self.move_set(-1),
            "nudge+" => self.move_set(1),
            _ if rtrim_words(v).is_some() => self.rtrim_verb(v),
            _ if slipslide_words(v).is_some() => self.slipslide_verb(v),
            _ if mark_words(v).is_some() => self.marks_verb(v),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod verb_tests {
    use super::*;
    use crate::Kind;

    /// stria::nle_face's `synth_fixture` cut list (A 13..23 at 0, B 0..10 at 10, A 23..33 at 25), video only.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
        EditorState::new(tl)
    }

    /// NLE6KEYS: each verb is the op the host face ran by hand; a face word that is not an editor verb is `None`.
    #[test]
    fn verb_is_the_host_dispatch() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        st.seek(13);
        assert_eq!(st.verb("mark-in"), Some(Ok(())));
        let want1 = tl0.trim_in(0, 1, 3).unwrap();
        assert_eq!(st.timeline, want1);
        assert_eq!(st.verb("["), Some(Ok(())));
        assert_eq!(st.timeline, want1.trim_in(0, 1, 2).unwrap());
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.timeline, want1);
        st.seek(16);
        assert_eq!(st.verb("split"), Some(Ok(())));
        assert_eq!(st.timeline, want1.split(0, 16).unwrap());
        assert_eq!(st.verb("end"), Some(Ok(())));
        assert_eq!(st.playhead, st.timeline.end());
        assert_eq!(st.verb("play"), None);
        st.deselect();
        assert_eq!(st.verb("]"), Some(Err(EditError::NoClip)));
    }
}

// ── NLESIZEPOLICYUI (LEDGER SR402): the reel's size policy from the editor (handlers.md §5 NLEPROBE-HDR's NLE-SIZE-POLICY-UI) ──
// The `size` words ride THE verb dispatch ([`EditorState::verb`]) both faces call: `size <reel> <policy>` sets a reel's
// [`crate::SizePolicy`] through `Timeline::set_size_policy` and the ONE `commit` (undo-able; the EDL's `* UNA SIZE` line is
// written by `edl::write` wherever a face saves), bare `size` cycles the selected clip's reel through `SizePolicy::next`
// (the host face's Size button). Refused by name: an unknown reel `EditError::NoSource`, an unknown policy word (or the
// wrong number of words) `EditError::Policy`, a bare `size` with no clip selected `EditError::NoClip`.
impl EditorState {
    /// Reel `reel`'s size policy set to `p` (one commit; the same policy again commits nothing).
    pub fn set_size_policy(&mut self, reel: &str, p: crate::SizePolicy) -> Result<(), EditError> {
        let src = self.timeline.source_by_reel(reel).ok_or(EditError::NoSource)?;
        if src.size_policy == p {
            return Ok(());
        }
        let tl = self.timeline.set_size_policy(src.id, p)?;
        self.commit(tl);
        Ok(())
    }
    /// The selected clip's reel steps to its next size policy (Letterbox -> Fill -> Stretch -> Adopt -> Letterbox).
    pub fn cycle_size(&mut self) -> Result<(), EditError> {
        let c = self.selected().ok_or(EditError::NoClip)?;
        let src = self.timeline.source(c.src).ok_or(EditError::NoSource)?;
        let tl = self.timeline.set_size_policy(src.id, src.size_policy.next())?;
        self.commit(tl);
        Ok(())
    }
    /// The `size` words of [`EditorState::verb`]; `None` when `v` is not a `size` verb.
    fn size_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        if w.next() != Some("size") {
            return None;
        }
        Some(match (w.next(), w.next(), w.next()) {
            (None, _, _) => self.cycle_size(),
            (Some(reel), Some(word), None) => match crate::SizePolicy::from_name(word) {
                Some(p) => self.set_size_policy(reel, p),
                None if self.timeline.source_by_reel(reel).is_none() => Err(EditError::NoSource),
                None => Err(EditError::Policy),
            },
            (Some(reel), None, _) if self.timeline.source_by_reel(reel).is_none() => Err(EditError::NoSource),
            _ => Err(EditError::Policy),
        })
    }
}

// ---- NLE3ETRIMSTRETCH (LEDGER SR692): the ripple trim and the transitioned pair's move from the editor (named tail block) ----
// `rtrim <in|out> <delta>` rides THE verb dispatch both faces call: the selected clip's edge moves by `delta` source ticks
// through `Timeline::ripple_trim_in` / `ripple_trim_out` (later clips of its track follow; ONE commit). `move_set` moves a
// set that is exactly a transitioned pair (two butted clips of one track with a two-clip transition on their cut) through
// `Timeline::move_pair`, so the transition goes with them; any other set is `move_many` as before.
impl EditorState {
    /// The ripple twin of [`EditorState::trim`]: the selected clip's in or out point by `delta` source ticks, keeping its
    /// start; the rest of its track follows.
    pub fn ripple_trim(&mut self, edge: Edge, delta: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        let tl = match edge {
            Edge::In => self.timeline.ripple_trim_in(track, idx, c.src_in + delta)?,
            Edge::Out => self.timeline.ripple_trim_out(track, idx, c.src_out + delta)?,
        };
        self.commit(tl);
        Ok(())
    }
    /// The set as a transitioned pair: `(track, outgoing index)` when it is exactly clips `i` and `i + 1` of one track,
    /// butted, with a two-clip (not BL) transition on their cut.
    fn pair_pick(&self) -> Option<(usize, usize)> {
        let [(t, i), (u, j)] = self.set[..] else { return None };
        let tr = self.timeline.tracks.get(t)?;
        let (o, n) = (tr.clips.get(i)?, tr.clips.get(j)?);
        let cut = o.end();
        let ok = t == u && j == i + 1 && n.at == cut && tr.transitions.iter().any(|x| x.at == cut && !x.from_black && !x.to_black);
        ok.then_some((t, i))
    }
    /// The `rtrim` words of [`EditorState::verb`].
    fn rtrim_verb(&mut self, v: &str) -> Result<(), EditError> {
        match rtrim_words(v) {
            Some((edge, delta)) => self.ripple_trim(edge, delta),
            None => Err(EditError::NoClip),
        }
    }
}

/// `rtrim in <delta>` / `rtrim out <delta>` (delta in source ticks, signed); any other shape is not this verb.
fn rtrim_words(v: &str) -> Option<(Edge, i64)> {
    let mut w = v.split_ascii_whitespace();
    if w.next() != Some("rtrim") {
        return None;
    }
    let edge = match w.next()? {
        "in" => Edge::In,
        "out" => Edge::Out,
        _ => return None,
    };
    let delta = w.next()?.parse::<i64>().ok()?;
    w.next().is_none().then_some((edge, delta))
}

// ---- NLE3ESLIP (LEDGER SR704): slip and slide from the editor (named tail block) ----
// `slip <delta>` / `slide <delta>` ride THE verb dispatch both faces call (and the `--do` tokens): the selected clip slips
// its source window (`Timeline::slip`) or slides along its track with its neighbours' edges following (`Timeline::slide`),
// ONE commit each (undo restores the whole pre-op timeline); a refusal leaves the state and names its `EditError`.
impl EditorState {
    /// Slip the selected clip by `delta` source ticks (`Timeline::slip`).
    pub fn slip_selected(&mut self, delta: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.slip(track, idx, delta)?;
        self.commit(tl);
        Ok(())
    }
    /// Slide the selected clip by `delta` timeline ticks (`Timeline::slide`); the selection stays on it.
    pub fn slide_selected(&mut self, delta: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.slide(track, idx, delta)?;
        self.commit(tl);
        Ok(())
    }
    /// The `slip` / `slide` words of [`EditorState::verb`].
    fn slipslide_verb(&mut self, v: &str) -> Result<(), EditError> {
        match slipslide_words(v) {
            Some((true, delta)) => self.slip_selected(delta),
            Some((false, delta)) => self.slide_selected(delta),
            None => Err(EditError::NoClip),
        }
    }
}

/// `slip <delta>` -> `(true, delta)`, `slide <delta>` -> `(false, delta)` (signed ticks); any other shape is not this verb.
fn slipslide_words(v: &str) -> Option<(bool, i64)> {
    let mut w = v.split_ascii_whitespace();
    let slip = match w.next()? {
        "slip" => true,
        "slide" => false,
        _ => return None,
    };
    let delta = w.next()?.parse::<i64>().ok()?;
    w.next().is_none().then_some((slip, delta))
}

// ---- NLE5DRANGESAVE (LEDGER SR723): the export range's marks from the editor through the one commit (named tail block) ----
// The marks live on the `Timeline` (`Timeline::marks`), so setting one is ONE commit (undo-able, it dirties the document,
// `edl::write` saves it as `* UNA MARK`). `mark in` / `mark out` set that mark at the playhead keeping the other, `mark
// clear` clears both; they ride THE verb dispatch both faces call. The host face's `in` / `out` toggles (SR717) call
// `set_marks` too. Refused by name through `Timeline::set_marks`: `EditError::Marks`, `EditError::Negative`.
impl EditorState {
    /// The export marks set to `m` (one commit; the same marks again commit nothing).
    pub fn set_marks(&mut self, m: (Option<i64>, Option<i64>)) -> Result<(), EditError> {
        if self.timeline.marks == m {
            return Ok(());
        }
        let tl = self.timeline.set_marks(m)?;
        self.commit(tl);
        Ok(())
    }
    /// The `mark` words of [`EditorState::verb`].
    fn marks_verb(&mut self, v: &str) -> Result<(), EditError> {
        let (ph, (a, b)) = (self.playhead, self.timeline.marks);
        match mark_words(v) {
            Some(Some(Edge::In)) => self.set_marks((Some(ph), b)),
            Some(Some(Edge::Out)) => self.set_marks((a, Some(ph))),
            Some(None) => self.set_marks((None, None)),
            None => Err(EditError::Marks),
        }
    }
}

/// `mark in` -> `Some(Some(Edge::In))`, `mark out` -> `Some(Some(Edge::Out))`, `mark clear` -> `Some(None)`; any other
/// shape is not this verb.
fn mark_words(v: &str) -> Option<Option<Edge>> {
    let mut w = v.split_ascii_whitespace();
    if w.next() != Some("mark") {
        return None;
    }
    let m = match w.next()? {
        "in" => Some(Edge::In),
        "out" => Some(Edge::Out),
        "clear" => None,
        _ => return None,
    };
    w.next().is_none().then_some(m)
}

// ---- NLETRANSITIONFACE (LEDGER SR691): the transition's kind and ease verbs (named tail block) ----
// The `kind` and `ease` words ride THE verb dispatch ([`EditorState::verb`]) both faces call, so the host face's buttons,
// `stria-view --do` and the kernel Player's keys all reach them. `kind <code>` reads its code through the EDL's ONE parser
// (`edl::transition_kind`: `D` or a `W<nnn>` [`crate::WipeDir::from_code`] builds) and re-puts the selected transition with
// that kind (`Timeline::put_transition`, which keeps its `dur`, its black flags and its ease); bare `kind` steps Dissolve ->
// every built wipe in ascending SMPTE code order -> Dissolve. `ease <word>` ([`crate::Ease::from_name`]) takes the transition
// off its cut and puts it back with the new ease (two pure ops, ONE commit: put_transition alone keeps the old ease); bare
// `ease` steps [`crate::Ease::ALL`]. Each change is ONE `commit` (undo-able); the same value again commits nothing. Refused
// as `EditError::Transition`: no transition selected, a `C` / unknown / refused code, an unknown ease word, extra words.
impl EditorState {
    /// The selected transition given `kind` (one commit; the same kind again commits nothing).
    pub fn set_transition_kind(&mut self, kind: crate::TransitionKind) -> Result<(), EditError> {
        let (track, idx) = self.transition.ok_or(EditError::Transition)?;
        let x = self.selected_transition().ok_or(EditError::Transition)?;
        if x.kind == kind {
            return Ok(());
        }
        let tl = self.timeline.put_transition(track, Transition { kind, ..x })?;
        self.commit(tl);
        self.transition = Some((track, idx));
        Ok(())
    }
    /// The selected transition's next kind: Dissolve -> the built wipes in ascending SMPTE code -> Dissolve.
    pub fn cycle_transition_kind(&mut self) -> Result<(), EditError> {
        let x = self.selected_transition().ok_or(EditError::Transition)?;
        let from = match x.kind {
            crate::TransitionKind::Dissolve => 0,
            crate::TransitionKind::Wipe(d) => d.code(),
        };
        let next = (from.saturating_add(1)..=999).find_map(crate::WipeDir::from_code);
        self.set_transition_kind(next.map_or(crate::TransitionKind::Dissolve, crate::TransitionKind::Wipe))
    }
    /// The selected transition given `ease` (one commit of `remove_transition` + `put_transition`; the same ease again
    /// commits nothing).
    pub fn set_transition_ease(&mut self, ease: crate::Ease) -> Result<(), EditError> {
        let (track, idx) = self.transition.ok_or(EditError::Transition)?;
        let x = self.selected_transition().ok_or(EditError::Transition)?;
        if x.ease == ease {
            return Ok(());
        }
        let tl = self.timeline.remove_transition(track, x.at)?.put_transition(track, Transition { ease, ..x })?;
        self.commit(tl);
        self.transition = Some((track, idx));
        Ok(())
    }
    /// The selected transition's next ease in [`crate::Ease::ALL`] (InOut wraps to Linear).
    pub fn cycle_transition_ease(&mut self) -> Result<(), EditError> {
        let x = self.selected_transition().ok_or(EditError::Transition)?;
        let all = crate::Ease::ALL;
        let i = all.iter().position(|&e| e == x.ease).unwrap_or(0);
        self.set_transition_ease(all[(i + 1) % all.len()])
    }
    /// The `kind` / `ease` words of [`EditorState::verb`]; `None` when `v` is neither.
    fn look_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        let word = w.next().filter(|&w| w == "kind" || w == "ease")?;
        Some(match (word, w.next(), w.next()) {
            ("kind", None, _) => self.cycle_transition_kind(),
            ("kind", Some(code), None) => match edl::transition_kind(code) {
                Some(Some(k)) => self.set_transition_kind(k),
                _ => Err(EditError::Transition),
            },
            ("ease", None, _) => self.cycle_transition_ease(),
            ("ease", Some(name), None) => match crate::Ease::from_name(name) {
                Some(e) => self.set_transition_ease(e),
                None => Err(EditError::Transition),
            },
            _ => Err(EditError::Transition),
        })
    }
}

// ---- STRIATITLECLIP (LEDGER SR828): a title card laid at the playhead as ONE commit (named tail block) ----
impl EditorState {
    /// A new generated title source (reel the first free `T<n>`, its text `text`, `dur` ticks) inserted at the playhead on
    /// the first video track through `Timeline::place` (`Mode::Insert`): ONE commit, so undo removes the source and the clip;
    /// the new clip is selected. Refused by name: no video track `EditError::NoTrack`; `dur < 1` `EditError::Empty`.
    pub fn add_title(&mut self, text: &str, dur: i64) -> Result<(), EditError> {
        let track = self.timeline.tracks.iter().position(|t| t.kind == crate::Kind::Video).ok_or(EditError::NoTrack)?;
        let reel = (1u32..).map(|n| alloc::format!("T{n}")).find(|r| self.timeline.source_by_reel(r).is_none()).expect("a free T<n> reel");
        let mut tl = self.timeline.clone();
        let id = tl.add_source(&reel, dur.max(0));
        if let Some(s) = tl.sources.last_mut() {
            s.title = Some(alloc::string::String::from(text));
        }
        let at = self.playhead;
        let tl = tl.place(track, crate::Clip::new(id, 0, dur, at), crate::Mode::Insert)?;
        self.commit(tl);
        let _ = self.select_at(track, at);
        Ok(())
    }
}

// ---- STRIACLIPGAINFACE (LEDGER SR838): the selected clip's gain and fades from the editor through the one commit (named tail block) ----
// The NLE4 model and EDL already hold a clip's gain and fades; these words ride THE verb dispatch both faces call:
// `gain-` / `gain+` step the selected clip's gain by [`GAIN_STEP_CDB`], bare `gain` puts it back to unity, `gain <dB>`
// sets it; `fade-in <frames|tc>` / `fade-out <frames|tc>` set one fade keeping the other (`0` clears), bare `fade-in` /
// `fade-out` run the fade to the playhead. Each is ONE commit through `Timeline::set_gain` / `Timeline::set_fades` (their
// `validate`): a refusal (`EditError::Gain` out of range, `EditError::Fade` longer than the clip) leaves the state.
/// STRIACLIPGAINFACE (SR838): one press of `gain-` / `gain+`, centi-dB (1 dB).
pub const GAIN_STEP_CDB: i32 = 100;

impl EditorState {
    /// The selected clip's gain set to `cdb` centi-dB (`Timeline::set_gain`, ONE commit; the same gain commits nothing).
    pub fn set_selected_gain(&mut self, cdb: i32) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        if c.gain_cdb == cdb {
            return Ok(());
        }
        let tl = self.timeline.set_gain(track, idx, cdb)?;
        self.commit(tl);
        Ok(())
    }
    /// The selected clip's fades set to `fade_in` / `fade_out` ticks (`Timeline::set_fades`, ONE commit; the same fades
    /// commit nothing).
    pub fn set_selected_fades(&mut self, fade_in: i64, fade_out: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        if (c.fade_in, c.fade_out) == (fade_in, fade_out) {
            return Ok(());
        }
        let tl = self.timeline.set_fades(track, idx, fade_in, fade_out)?;
        self.commit(tl);
        Ok(())
    }
    /// The `gain` / `fade` words of [`EditorState::verb`]; `None` when `v` is none of them.
    fn gain_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        let word = w.next().filter(|w| matches!(*w, "gain-" | "gain+" | "gain" | "fade-in" | "fade-out"))?;
        let (arg, extra) = (w.next(), w.next());
        Some(self.gain_word(word, arg, extra.is_some()))
    }
    fn gain_word(&mut self, word: &str, arg: Option<&str>, extra: bool) -> Result<(), EditError> {
        let c = self.selected().ok_or(EditError::NoClip)?;
        let refused = if word.starts_with("gain") { EditError::Gain } else { EditError::Fade };
        if extra {
            return Err(refused);
        }
        let fps = self.timeline.fps().unwrap_or(25);
        let ph = self.playhead;
        match (word, arg) {
            ("gain-", None) => self.set_selected_gain(c.gain_cdb.saturating_sub(GAIN_STEP_CDB)),
            ("gain+", None) => self.set_selected_gain(c.gain_cdb.saturating_add(GAIN_STEP_CDB)),
            ("gain", None) => self.set_selected_gain(0),
            ("gain", Some(db)) => match cdb_of(db) {
                Some(cdb) => self.set_selected_gain(cdb),
                None => Err(refused),
            },
            ("fade-in", None) => self.set_selected_fades(ph - c.at, c.fade_out),
            ("fade-out", None) => self.set_selected_fades(c.fade_in, c.end() - ph),
            ("fade-in", Some(n)) => match ticks_of(n, fps) {
                Some(n) => self.set_selected_fades(n, c.fade_out),
                None => Err(refused),
            },
            ("fade-out", Some(n)) => match ticks_of(n, fps) {
                Some(n) => self.set_selected_fades(c.fade_in, n),
                None => Err(refused),
            },
            _ => Err(refused),
        }
    }
}

/// A gain word in dB (`-3`, `2.5`) as centi-dB, rounded half away from zero; `None` for a word that is not a finite
/// number. The cast saturates, so an absurd value is refused by `validate`'s `EditError::Gain`.
fn cdb_of(s: &str) -> Option<i32> {
    let db = s.parse::<f64>().ok().filter(|x| x.is_finite())?;
    let v = db * 100.0;
    Some((if v < 0.0 { v - 0.5 } else { v + 0.5 }) as i32)
}

/// A fade length: a timecode `HH:MM:SS:FF` (through `edl::parse_tc`, the EDL's one parser) or a whole number of frames.
fn ticks_of(s: &str, fps: u32) -> Option<i64> {
    if s.contains(':') { edl::parse_tc(s, fps, 0).ok() } else { s.parse::<i64>().ok() }
}

#[cfg(test)]
mod striaclipgainface_tests {
    use super::*;
    use crate::Kind;

    /// One audio track: A 0..20 at 0 (a 20-tick clip), selected.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st
    }

    #[test]
    fn gain_words_step_set_and_refuse_through_one_commit() {
        let mut st = state();
        assert_eq!(st.verb("gain+"), Some(Ok(())));
        assert_eq!(st.verb("gain+"), Some(Ok(())));
        assert_eq!(st.selected().unwrap().gain_cdb, 200);
        assert_eq!(st.verb("gain-"), Some(Ok(())));
        assert_eq!(st.selected().unwrap().gain_cdb, 100);
        assert_eq!(st.verb("gain -3.5"), Some(Ok(())));
        assert_eq!(st.selected().unwrap().gain_cdb, -350);
        assert_eq!(st.undo_depth(), 4);
        assert_eq!(st.verb("gain -200"), Some(Err(EditError::Gain)));
        assert_eq!(st.verb("gain loud"), Some(Err(EditError::Gain)));
        assert_eq!(st.verb("gain+ 3"), Some(Err(EditError::Gain)));
        assert_eq!((st.selected().unwrap().gain_cdb, st.undo_depth()), (-350, 4));
        assert_eq!(st.verb("gain"), Some(Ok(())));
        assert_eq!(st.selected().unwrap().gain_cdb, 0);
        assert_eq!(st.verb("gain"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 5, "the same gain commits nothing");
        assert!(st.undo());
        assert_eq!(st.selected().unwrap().gain_cdb, -350);
        assert_eq!(st.verb("gainx"), None);
        st.deselect();
        assert_eq!(st.verb("gain+"), Some(Err(EditError::NoClip)));
    }

    #[test]
    fn fade_words_frames_timecode_playhead_and_refusal() {
        let mut st = state();
        assert_eq!(st.verb("fade-in 10"), Some(Ok(())));
        assert_eq!(st.verb("fade-out 15"), Some(Err(EditError::Fade)));
        assert_eq!((st.selected().unwrap().fade_in, st.selected().unwrap().fade_out), (10, 0));
        assert_eq!(st.verb("fade-out 00:00:00:05"), Some(Ok(())));
        assert_eq!((st.selected().unwrap().fade_in, st.selected().unwrap().fade_out), (10, 5));
        assert_eq!(st.verb("fade-in 0"), Some(Ok(())));
        assert_eq!(st.selected().unwrap().fade_in, 0);
        st.seek(4);
        assert_eq!(st.verb("fade-in"), Some(Ok(())));
        assert_eq!(st.selected().unwrap().fade_in, 4);
        st.seek(12);
        assert_eq!(st.verb("fade-out"), Some(Ok(())));
        assert_eq!(st.selected().unwrap().fade_out, 8);
        assert_eq!(st.verb("fade-in -1"), Some(Err(EditError::Fade)));
        assert_eq!(st.verb("fade-in soon"), Some(Err(EditError::Fade)));
        assert_eq!(st.verb("fade-in 1 2"), Some(Err(EditError::Fade)));
        assert_eq!((st.selected().unwrap().fade_in, st.selected().unwrap().fade_out), (4, 8));
    }
}

// ---- STRIATRACKMUTESOLO (LEDGER SR837): `mute` / `solo` on the selected clip's track through the one commit (named tail block) ----
// The words ride THE verb dispatch both faces call. RULE: the word toggles the flag of the SELECTED clip's track (the
// primary of the set); with nothing selected it is refused `EditError::NoTrack`. Each toggle is ONE `commit` (undo-able; it
// dirties the document, and `edl::write` saves it as `* UNA MUTE` / `* UNA SOLO`); the selection stays.
impl EditorState {
    /// Toggle the selected clip's track's mute flag (one commit).
    pub fn toggle_mute(&mut self) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoTrack)?;
        let t = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?;
        let tl = self.timeline.set_track_flags(track, !t.muted, t.solo)?;
        self.commit(tl);
        Ok(())
    }
    /// Toggle the selected clip's track's solo flag (one commit).
    pub fn toggle_solo(&mut self) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoTrack)?;
        let t = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?;
        let tl = self.timeline.set_track_flags(track, t.muted, !t.solo)?;
        self.commit(tl);
        Ok(())
    }
    /// The `mute` / `solo` words of [`EditorState::verb`]; `None` for any other word.
    fn track_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        match v {
            "mute" => Some(self.toggle_mute()),
            "solo" => Some(self.toggle_solo()),
            _ => None,
        }
    }
}

// ---- STRIACLIPSPEED (LEDGER SR829): the selected clip's speed as ONE commit (named tail block) ----
impl EditorState {
    /// The selected clip at speed `rate_q16` (Q16.16) through `Timeline::set_rate`: ONE commit, so undo restores the whole
    /// pre-op timeline; a refusal (`Rate`, `Overlap`, `Transition`, `NoClip`) leaves the state.
    pub fn set_speed(&mut self, rate_q16: u32) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.set_rate(track, idx, rate_q16)?;
        self.commit(tl);
        Ok(())
    }
}

// ---- STRIAMARKERS (LEDGER SR835): drop / remove a named marker at the playhead through the one commit (named tail block) ----
// `mark [<name>]` drops a marker at the playhead (default name `Timeline::marker_name`, the first free `M<n>`), `unmark`
// removes the one at the playhead; each is ONE `commit` (undo-able, it dirties the document, `edl::write` keeps it as
// `* UNA MARKER`), riding THE verb dispatch both faces call. SR723's export words `mark in` / `mark out` / `mark clear` keep
// their meaning (`mark_words` takes them first), so a marker cannot be named exactly `in`, `out` or `clear`. Refused by
// name: a taken tick or nothing to remove `EditError::Marker`.

/// STRIAMARKERS (SR835): a marker word of [`EditorState::verb`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerWord<'a> {
    /// `mark` (`None`: the default name) or `mark <name>`.
    Mark(Option<&'a str>),
    /// `unmark`.
    Unmark,
}

/// STRIAMARKERS (SR835): `unmark`, `mark`, `mark <name>` (the rest trimmed); `None` for any other word, SR723's export
/// words included.
pub fn marker_word(v: &str) -> Option<MarkerWord<'_>> {
    let v = v.trim();
    match v {
        "unmark" => return Some(MarkerWord::Unmark),
        "mark" => return Some(MarkerWord::Mark(None)),
        _ => {}
    }
    let rest = v.strip_prefix("mark")?;
    if !rest.starts_with(char::is_whitespace) || mark_words(v).is_some() {
        return None;
    }
    Some(MarkerWord::Mark(Some(rest.trim())))
}

impl EditorState {
    /// A marker at the playhead named `name` (`None`: [`Timeline::marker_name`]): ONE commit. Refused: a marker already
    /// there (`EditError::Marker`).
    pub fn mark(&mut self, name: Option<&str>) -> Result<(), EditError> {
        let name = name.map_or_else(|| self.timeline.marker_name(), alloc::string::String::from);
        let tl = self.timeline.mark(self.playhead, &name)?;
        self.commit(tl);
        Ok(())
    }
    /// The marker at the playhead removed: ONE commit. Refused: none there (`EditError::Marker`).
    pub fn unmark(&mut self) -> Result<(), EditError> {
        let tl = self.timeline.unmark(self.playhead).ok_or(EditError::Marker)?;
        self.commit(tl);
        Ok(())
    }
    /// The marker words of [`EditorState::verb`]; `None` for any other word.
    fn marker_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        Some(match marker_word(v)? {
            MarkerWord::Mark(name) => self.mark(name),
            MarkerWord::Unmark => self.unmark(),
        })
    }
}

// ---- STRIAROLLEDIT (LEDGER SR851): the roll edit from the editor (named tail block) ----
// `roll-` / `roll+` (one frame = one tick) and `roll <d>` (signed ticks) ride THE verb dispatch both faces call, so the
// host face's buttons, `stria-view --do` and a future kernel EDIT_KEYS binding reach the same words. The rule: the roll
// moves the cut at the SELECTED clip's OUT edge (`Timeline::roll` on the selected clip and its butted next clip), ONE
// commit per word (undo restores the whole pre-op timeline); the selection stays on the selected clip. Refused by name:
// nothing selected or no butted next clip `EditError::NoClip`, and whatever `Timeline::roll` refuses.
impl EditorState {
    /// Roll the cut at the selected clip's out edge by `d` ticks (`Timeline::roll`).
    pub fn roll_selected(&mut self, d: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.roll(track, idx, d)?;
        self.commit(tl);
        Ok(())
    }
    /// The `roll` words of [`EditorState::verb`]; `None` when `v` is not a roll word.
    fn roll_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let d = roll_words(v)?;
        Some(self.roll_selected(d))
    }
}

/// `roll-` -> -1, `roll+` -> 1, `roll <d>` -> d (signed ticks); any other shape is not this verb.
fn roll_words(v: &str) -> Option<i64> {
    match v {
        "roll-" => return Some(-1),
        "roll+" => return Some(1),
        _ => {}
    }
    let mut w = v.split_ascii_whitespace();
    if w.next() != Some("roll") {
        return None;
    }
    let d = w.next()?.parse::<i64>().ok()?;
    w.next().is_none().then_some(d)
}

// ---- STRIAINSERTOVERWRITE (LEDGER SR852): insert / overwrite a source range at the playhead through place (named tail block) ----
// `insert` / `overwrite` ride THE verb dispatch both faces call and lay a source range at the PLAYHEAD through
// `Timeline::place` (`Mode::Insert` splits what spans the playhead and ripples THAT track only; `Mode::Overwrite` carves
// the hole) and the ONE `commit` (undo restores the whole pre-op timeline). Three-point editing without a source viewer:
// bare, the range is the SELECTED clip's `(src, src_in, src_out)` on its track (nothing selected: `EditError::NoClip`);
// `insert <reel> <in> <out>` names it (frames or a non-drop `HH:MM:SS:FF` through `edl::parse_tc`) on the FIRST track
// holding a clip of that reel (none: `EditError::NoTrack`; an unknown reel `EditError::NoSource`). The laid clip is a fresh
// `Clip::new` (unity gain, no fades). The timeline's marks are the export range and are not consulted. After the edit
// the laid clip is selected and the playhead sits at its end, so a repeat appends. A malformed shape is not this verb.
impl EditorState {
    /// Lay source `src` ticks `src_in..src_out` at the playhead on `track` (`Timeline::place` in `mode`, ONE commit); the
    /// laid clip is selected and the playhead moves to its end.
    pub fn place_at_playhead(&mut self, track: usize, src: crate::SourceId, src_in: i64, src_out: i64, mode: crate::Mode) -> Result<(), EditError> {
        let clip = Clip::new(src, src_in, src_out, self.playhead);
        let tl = self.timeline.place(track, clip, mode)?;
        self.commit(tl);
        self.select_at(track, clip.at).unwrap_or_else(|_| self.deselect());
        self.seek(clip.end());
        Ok(())
    }
    /// The selected clip's source range laid at the playhead on its own track.
    pub fn insert_selected(&mut self, mode: crate::Mode) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        self.place_at_playhead(track, c.src, c.src_in, c.src_out, mode)
    }
    /// Reel `reel`'s ticks `src_in..src_out` laid at the playhead on the first track holding a clip of that reel.
    pub fn insert_reel(&mut self, reel: &str, src_in: i64, src_out: i64, mode: crate::Mode) -> Result<(), EditError> {
        let src = self.timeline.source_by_reel(reel).ok_or(EditError::NoSource)?.id;
        let track = self.timeline.tracks.iter().position(|t| t.clips.iter().any(|c| c.src == src)).ok_or(EditError::NoTrack)?;
        self.place_at_playhead(track, src, src_in, src_out, mode)
    }
    /// The `insert` / `overwrite` words of [`EditorState::verb`]; `None` when `v` is neither (or malformed).
    fn insert_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let (mode, range) = insert_words(v, self.timeline.fps())?;
        Some(match range {
            None => self.insert_selected(mode),
            Some((reel, a, b)) => self.insert_reel(reel, a, b, mode),
        })
    }
}

/// `insert` / `overwrite` bare -> `(mode, None)`; `insert <reel> <in> <out>` -> `(mode, Some((reel, in, out)))`; any other
/// shape is not this verb.
fn insert_words(v: &str, fps: Option<u32>) -> Option<(crate::Mode, Option<(&str, i64, i64)>)> {
    let mut w = v.split_ascii_whitespace();
    let mode = match w.next()? {
        "insert" => crate::Mode::Insert,
        "overwrite" => crate::Mode::Overwrite,
        _ => return None,
    };
    let Some(reel) = w.next() else { return Some((mode, None)) };
    let a = insert_tick(w.next()?, fps)?;
    let b = insert_tick(w.next()?, fps)?;
    w.next().is_none().then_some((mode, Some((reel, a, b))))
}

/// A source tick: an integer frame count, else a non-drop `HH:MM:SS:FF` at `fps` through the EDL's one parser.
fn insert_tick(s: &str, fps: Option<u32>) -> Option<i64> {
    s.parse::<i64>().ok().or_else(|| edl::parse_tc(s, fps?, 0).ok())
}

#[cfg(test)]
mod striainsertoverwrite_tests {
    use super::*;
    use crate::{Kind, Mode, SourceId};

    /// `verb_tests`' cut list (A 13..23 at 0, B 0..10 at 10, A 23..33 at 25) on V1 and on A1.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_source("C", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
        }
        EditorState::new(tl)
    }

    #[test]
    fn striainsertoverwrite_insert_splits_ripples_the_track_and_undoes_once() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        st.seek(15);
        assert_eq!(st.verb("insert"), Some(Ok(())));
        let want = tl0.place(0, Clip::new(SourceId(1), 0, 10, 15), Mode::Insert).unwrap();
        assert_eq!(st.timeline, want);
        let v1 = &st.timeline.tracks[0].clips;
        assert_eq!(v1[1], Clip::new(SourceId(1), 0, 5, 10), "the spanned clip split at the playhead");
        assert_eq!(v1[2], Clip::new(SourceId(1), 0, 10, 15), "the laid clip at the playhead");
        assert_eq!(v1[3], Clip::new(SourceId(1), 5, 10, 25), "its tail moved later by the range's length");
        assert_eq!(v1[4], Clip::new(SourceId(0), 23, 33, 35));
        assert_eq!(st.timeline.tracks[1], tl0.tracks[1], "insert ripples only the target track");
        assert_eq!((st.selection, st.playhead), (Some((0, 2)), 25), "laid clip selected, playhead at its end");
        assert_eq!(st.undo_depth(), 1);
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.timeline, tl0, "ONE undo restores");
    }

    #[test]
    fn striainsertoverwrite_overwrite_cuts_the_hole_and_keeps_the_end() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        st.seek(12);
        assert_eq!(st.verb("overwrite"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 10, 12), Mode::Overwrite).unwrap());
        assert_eq!(st.timeline.end(), tl0.end(), "the range fits: the end is unchanged");
        assert_eq!(st.timeline.tracks[0].clips[1], Clip::new(SourceId(1), 0, 2, 10));
        assert_eq!(st.timeline.tracks[0].clips[2], Clip::new(SourceId(1), 0, 10, 12));
        assert_eq!(st.playhead, 22);
    }

    #[test]
    fn striainsertoverwrite_argument_form_by_frames_and_timecode() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.seek(10);
        assert_eq!(st.verb("insert A 40 45"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.place(0, Clip::new(SourceId(0), 40, 45, 10), Mode::Insert).unwrap(), "reel A's first track is V1");
        let mut tc = state();
        tc.seek(10);
        assert_eq!(tc.verb("insert A 00:00:01:15 00:00:01:20"), Some(Ok(())));
        assert_eq!(tc.timeline, st.timeline, "the timecode form is the frame form");
        let mut ow = state();
        ow.seek(10);
        assert_eq!(ow.verb("overwrite B 00:00:00:00 5"), Some(Ok(())));
        assert_eq!(ow.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 5, 10), Mode::Overwrite).unwrap());
    }

    #[test]
    fn striainsertoverwrite_refusals_by_name_leave_the_state() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.seek(10);
        assert_eq!(st.verb("insert"), Some(Err(EditError::NoClip)));
        assert_eq!(st.verb("overwrite"), Some(Err(EditError::NoClip)));
        assert_eq!(st.verb("insert Z 0 5"), Some(Err(EditError::NoSource)));
        assert_eq!(st.verb("insert A 5 5"), Some(Err(EditError::Empty)));
        assert_eq!(st.verb("insert A 9 3"), Some(Err(EditError::Empty)));
        assert_eq!(st.verb("insert A 45 51"), Some(Err(EditError::OutOfSource)));
        assert_eq!(st.verb("insert A -1 5"), Some(Err(EditError::Negative)));
        assert_eq!(st.verb("insert C 0 5"), Some(Err(EditError::NoTrack)), "a reel on no track");
        assert_eq!((st.timeline.clone(), st.undo_depth(), st.playhead), (tl0, 0, 10));
        for v in ["insert A", "insert A 1", "insert A 1 x", "insert A 1 2 3", "inserts", "overwrite A 00:00:00:25 30"] {
            assert_eq!(st.verb(v), None, "{v}: not this verb");
        }
    }

    #[test]
    fn striainsertoverwrite_repeat_appends_and_edl_round_trips() {
        let mut st = state();
        st.select(0, 1).unwrap();
        st.seek(35);
        assert_eq!(st.verb("insert"), Some(Ok(())));
        assert_eq!(st.verb("insert"), Some(Ok(())));
        let v1 = &st.timeline.tracks[0].clips;
        assert_eq!((v1[3], v1[4]), (Clip::new(SourceId(1), 0, 10, 35), Clip::new(SourceId(1), 0, 10, 45)));
        assert_eq!(st.playhead, 55);
        let text = edl::write(&st.timeline, "INSERT").unwrap();
        assert_eq!(edl::parse(&text, 25).unwrap(), st.timeline);
    }
}

// ---- STRIAZOOMTOFIT (LEDGER SR859): the fit zoom (named tail block) ----
// A view change, not an edit: no `commit`, no undo step, the timeline untouched. The range is `0..end` (the whole
// timeline) or `min at .. max end` over the selection SET; the zoom is the largest integer px-per-tick that puts the
// range inside `w` px, within `ZOOM_MIN..=ZOOM_MAX`; a range wider than `w` ticks gets ZOOM_MIN (no fractional zoom).
// The origin (the range's start) is the caller's to anchor: the host face writes it into `Face.view`; the kernel
// Player's fit is owed (NLE-ZOOMFIT-KERNEL). Nothing to fit (an empty timeline, an empty set): `EditError::NoClip`.
impl EditorState {
    /// Fit the whole timeline (`sel` false) or the selection set (`sel` true) into `w` px: sets the zoom, returns the
    /// origin tick (the range's start).
    pub fn fit(&mut self, w: u32, sel: bool) -> Result<i64, EditError> {
        let (start, end) = self.fit_range(sel).ok_or(EditError::NoClip)?;
        let span = end - start;
        if span < 1 {
            return Err(EditError::NoClip);
        }
        self.zoom = (w as i64 / span).clamp(ZOOM_MIN as i64, ZOOM_MAX as i64) as u32;
        Ok(start)
    }
    /// The ticks a fit covers: `0..end`, or the selection set's `min at .. max end`.
    fn fit_range(&self, sel: bool) -> Option<(i64, i64)> {
        if !sel {
            return Some((0, self.timeline.end()));
        }
        let clips = self.set.iter().filter_map(|&(t, i)| self.timeline.tracks.get(t)?.clips.get(i));
        clips.fold(None, |r, c| Some(r.map_or((c.at, c.end()), |(a, b): (i64, i64)| (a.min(c.at), b.max(c.end())))))
    }
}
