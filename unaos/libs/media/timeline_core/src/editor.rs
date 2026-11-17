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
        let ph = self.playhead; if let Some(r) = self.size_verb(v) { return Some(r); } if let Some(r) = self.look_verb(v) { return Some(r); } if let Some(r) = self.marker_verb(v) { return Some(r); } if let Some(r) = self.jump_verb(v) { return Some(r); } if let Some(r) = self.marker_style_verb(v) { return Some(r); } if let Some(r) = self.marker_marks_verb(v) { return Some(r); } // STRIAMARKERRANGES (SR9052): `marker marks` (file tail); STRIAMARKERJUMPKEYSNEXT (SR984): `marker colour` / `marker range` (file tail); NLETRANSITIONFACE (SR691): `kind` / `ease` (file tail); NLESIZEPOLICYUI (SR402): the `size` words (file tail)
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
                Some(c) => self.trim(Edge::In, ph - c.at),
                None => Err(EditError::NoClip),
            },
            "mark-out" => match self.selected() {
                Some(c) => self.trim(Edge::Out, ph - c.end()),
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

// ---- STRIAMARKERJUMP (LEDGER SR894): marker- / marker+ / cut- / cut+ jump the playhead (named tail block) ----
// View words (no commit: no undo step, the document is not dirtied): `marker-` / `marker+` move the playhead to the
// previous / next named marker (`Timeline.markers`, already sorted), `cut-` / `cut+` to the previous / next clip edge on any
// track (`view::snap_targets`, THE clip-edge walk, called unchanged). The move is `seek` (THE clamped playhead road); a
// target past `timeline.end()` is skipped; with nowhere to go the playhead stays (like `next` at the end).

/// STRIAMARKERJUMP (SR894): what a jump word steps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jump {
    /// The named markers.
    Marker,
    /// Every clip's `at` and `end` on any track.
    Cut,
}

/// STRIAMARKERJUMP (SR894): `marker-` / `marker+` / `cut-` / `cut+` (trimmed) as `(what, forward)`; `None` for any other word.
pub fn jump_word(v: &str) -> Option<(Jump, bool)> {
    match v.trim() {
        "marker-" => Some((Jump::Marker, false)),
        "marker+" => Some((Jump::Marker, true)),
        "cut-" => Some((Jump::Cut, false)),
        "cut+" => Some((Jump::Cut, true)),
        _ => None,
    }
}

impl EditorState {
    /// The ticks a `j` jump may land on, sorted ascending: the markers' ticks, or the clip edges of
    /// [`crate::view::snap_targets`] (no exclusion, no playhead).
    pub fn jump_targets(&self, j: Jump) -> Vec<i64> {
        match j {
            Jump::Cut => crate::view::snap_targets(self, &[], false),
            Jump::Marker => self.marker_jump_targets(), // STRIAMARKERRANGES (SR9052): a range end is a target too (file tail)
        }
    }
    /// The playhead to the first target after it (`forward`, at most `timeline.end()`) or the last target before it,
    /// through `seek`. False when there is none (the playhead stays).
    pub fn jump(&mut self, j: Jump, forward: bool) -> bool {
        let (ph, end) = (self.playhead, self.timeline.end().max(0));
        let t = self.jump_targets(j);
        let to = if forward { t.iter().copied().find(|&x| x > ph && x <= end) } else { t.iter().rev().copied().find(|&x| x < ph) };
        match to {
            Some(x) => {
                self.seek(x);
                true
            }
            None => false,
        }
    }
    /// The jump words of [`EditorState::verb`] (always `Ok`: a view word); `None` for any other word.
    fn jump_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let (j, forward) = jump_word(v)?;
        self.jump(j, forward);
        Some(Ok(()))
    }
}

#[cfg(test)]
mod striamarkerjump_tests {
    use super::*;
    use crate::Kind;

    /// stria::nle_face's `synth_fixture` cut list on two tracks: edges 0, 10, 20, 25, 35.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
        }
        EditorState::new(tl)
    }

    #[test]
    fn striamarkerjump_cut_steps_every_clip_edge_and_stops_at_the_ends() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.jump_targets(Jump::Cut), alloc::vec![0, 10, 20, 25, 35]);
        for want in [10, 20, 25, 35, 35] {
            assert_eq!(st.verb("cut+"), Some(Ok(())));
            assert_eq!(st.playhead, want);
        }
        for want in [25, 20, 10, 0, 0] {
            assert_eq!(st.verb("cut-"), Some(Ok(())));
            assert_eq!(st.playhead, want);
        }
        st.seek(12);
        assert_eq!(st.verb("cut-"), Some(Ok(())));
        assert_eq!(st.playhead, 10);
        assert_eq!(st.timeline, tl0);
        assert!(!st.undo(), "a jump is not an undo step");
    }

    #[test]
    fn striamarkerjump_marker_steps_the_markers_and_skips_past_the_end() {
        let mut st = state();
        assert!(!st.jump(Jump::Marker, true), "no markers: nowhere to go");
        assert_eq!(st.playhead, 0);
        st.timeline = st.timeline.mark(5, "A").unwrap().mark(30, "B").unwrap().mark(99, "C").unwrap();
        let tl0 = st.timeline.clone();
        for want in [5, 30, 30] {
            assert_eq!(st.verb("marker+"), Some(Ok(())));
            assert_eq!(st.playhead, want);
        }
        assert_eq!(st.verb("marker-"), Some(Ok(())));
        assert_eq!(st.playhead, 5);
        assert!(!st.jump(Jump::Marker, false));
        assert_eq!(st.playhead, 5);
        assert_eq!(st.timeline, tl0);
        assert!(!st.undo(), "a jump is not an undo step");
    }

    #[test]
    fn striamarkerjump_words() {
        assert_eq!(jump_word(" cut+ "), Some((Jump::Cut, true)));
        assert_eq!(jump_word("marker-"), Some((Jump::Marker, false)));
        for v in ["marker", "cut", "mark -", "markers+", "cut +"] {
            assert_eq!(jump_word(v), None, "{v}");
        }
        assert_eq!(marker_word("marker+"), None, "SR835's mark road never takes a jump word");
    }
}

// ---- STRIAMARKERJUMPKEYSNEXT (LEDGER SR984): `marker colour` / `marker range` through the one commit (named tail block) ----
// `marker colour` cycles the colour of the marker AT the playhead to the next of `MARKER_COLOURS`, `marker colour <name>`
// sets it (an unknown name is not a word); `marker range` gives the last marker at or before the playhead the length
// playhead - at (on the marker's own tick: 0, a point marker again). Each is ONE commit (undo step, dirty); nothing to
// change (no marker, the same colour / length) is `EditError::Marker` with no commit. Neither word starts `mark `, so
// SR835's `marker_word` never takes it as a marker name, and neither is SR894's jump word.

/// STRIAMARKERJUMPKEYSNEXT (SR984): a marker style word of [`EditorState::verb`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerStyle {
    /// `marker colour` (`None`: the next colour) or `marker colour <name>`.
    Colour(Option<u8>),
    /// `marker range`.
    Range,
}

/// STRIAMARKERJUMPKEYSNEXT (SR984): `marker colour`, `marker colour <name>`, `marker range` (trimmed); `None` for any
/// other word, an unknown colour name included.
pub fn marker_style_word(v: &str) -> Option<MarkerStyle> {
    let mut w = v.split_ascii_whitespace();
    if w.next() != Some("marker") {
        return None;
    }
    let s = match (w.next()?, w.next()) {
        ("range", None) => MarkerStyle::Range,
        ("colour", None) => MarkerStyle::Colour(None),
        ("colour", Some(name)) => MarkerStyle::Colour(Some(crate::marker_colour(name)?)),
        _ => return None,
    };
    if w.next().is_some() {
        return None;
    }
    Some(s)
}

impl EditorState {
    /// The marker at the playhead coloured `colour` (`None`: the next colour of the table, wrapping): ONE commit.
    /// Refused: no marker there, or it already has the colour (`EditError::Marker`).
    pub fn colour_marker(&mut self, colour: Option<u8>) -> Result<(), EditError> {
        let m = self.timeline.marker_at(self.playhead).ok_or(EditError::Marker)?;
        let c = colour.unwrap_or(((usize::from(m.colour) + 1) % crate::MARKER_COLOURS.len()) as u8);
        let tl = self.timeline.colour_marker(self.playhead, c).ok_or(EditError::Marker)?;
        self.commit(tl);
        Ok(())
    }
    /// The last marker at or before the playhead spanning to the playhead: ONE commit. Refused: no such marker, or it
    /// already ends there (`EditError::Marker`).
    pub fn range_marker(&mut self) -> Result<(), EditError> {
        let at = self.timeline.marker_before(self.playhead).ok_or(EditError::Marker)?.at;
        let tl = self.timeline.range_marker(at, self.playhead - at).ok_or(EditError::Marker)?;
        self.commit(tl);
        Ok(())
    }
    /// The marker style words of [`EditorState::verb`]; `None` for any other word.
    fn marker_style_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        Some(match marker_style_word(v)? {
            MarkerStyle::Colour(c) => self.colour_marker(c),
            MarkerStyle::Range => self.range_marker(),
        })
    }
}

#[cfg(test)]
mod striamarkerjumpkeysnext_tests {
    use super::*;
    use crate::Kind;

    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 35, 0)];
        EditorState::new(tl.mark(12, "M1").unwrap())
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: the words parse; no other word (a marker name, a jump word) is taken.
    #[test]
    fn striamarkerjumpkeysnext_words() {
        assert_eq!(marker_style_word(" marker colour "), Some(MarkerStyle::Colour(None)));
        assert_eq!(marker_style_word("marker colour green"), Some(MarkerStyle::Colour(Some(3))));
        assert_eq!(marker_style_word("marker range"), Some(MarkerStyle::Range));
        for v in ["marker colour mauve", "marker range 3", "marker", "marker+", "mark colour", "marker colour red x"] {
            assert_eq!(marker_style_word(v), None, "{v}");
        }
        assert_eq!(marker_word("marker colour"), None, "SR835's mark road never takes a style word");
        assert_eq!(jump_word("marker range"), None);
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: `marker colour` cycles and `marker colour <name>` sets the playhead's marker, one undo
    /// step each; with no marker there, or the same colour, nothing is committed.
    #[test]
    fn striamarkerjumpkeysnext_colour_is_one_commit() {
        let mut st = state();
        assert_eq!(st.verb("marker colour"), Some(Err(EditError::Marker)), "no marker at 0");
        assert_eq!(st.undo_depth(), 0);
        st.seek(12);
        assert_eq!(st.verb("marker colour"), Some(Ok(())));
        assert_eq!((st.timeline.markers[0].colour, st.undo_depth()), (1, 1));
        assert_eq!(st.verb("marker colour green"), Some(Ok(())));
        assert_eq!((st.timeline.markers[0].colour, st.undo_depth()), (3, 2));
        assert_eq!(st.verb("marker colour green"), Some(Err(EditError::Marker)), "already green");
        assert_eq!(st.undo_depth(), 2);
        assert_eq!(st.verb("marker colour pink"), Some(Ok(())));
        assert_eq!(st.verb("marker colour"), Some(Ok(())));
        assert_eq!(st.timeline.markers[0].colour, 0, "the cycle wraps to orange");
        assert!(st.undo());
        assert_eq!(st.timeline.markers[0].colour, 7);
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: `marker range` spans the last marker at or before the playhead to the playhead; on
    /// its own tick it clears; before every marker, or unchanged, nothing is committed.
    #[test]
    fn striamarkerjumpkeysnext_range_is_one_commit() {
        let mut st = state();
        st.seek(5);
        assert_eq!(st.verb("marker range"), Some(Err(EditError::Marker)), "no marker at or before 5");
        st.seek(20);
        assert_eq!(st.verb("marker range"), Some(Ok(())));
        assert_eq!((st.timeline.markers[0].len, st.undo_depth()), (8, 1));
        assert_eq!(st.verb("marker range"), Some(Err(EditError::Marker)), "already ends at 20");
        assert_eq!(st.undo_depth(), 1);
        st.seek(12);
        assert_eq!(st.verb("marker range"), Some(Ok(())));
        assert_eq!((st.timeline.markers[0].len, st.undo_depth()), (0, 2));
        assert_eq!(st.timeline.validate(), Ok(()));
    }
}

// ---- STRIAMARKERRANGES (LEDGER SR9052): range ends as marker jump targets; marks to a marker's range (named tail block) ----
// A ranged marker (`Marker.len > 0`, SR984) has two jump targets for SR894's `marker-` / `marker+`: its `at` and its end
// `at + len` (`marker_jump_targets`, the `Jump::Marker` list `jump` walks, sorted, one entry per tick). `marker marks` sets
// the export marks (`Timeline::marks`) to the range of the last marker at or before the playhead through SR723's
// `set_marks` (ONE commit, or none when the marks already equal it). Refused as `EditError::Marker` with no commit: no
// marker at or before the playhead, or that marker is a point (`len == 0`).

/// STRIAMARKERRANGES (SR9052): exactly the two words `marker marks` (whitespace trimmed).
pub fn marker_marks_word(v: &str) -> bool {
    let mut w = v.split_ascii_whitespace();
    w.next() == Some("marker") && w.next() == Some("marks") && w.next().is_none()
}

impl EditorState {
    /// The marker jump targets: every marker's `at` and every ranged marker's end `at + len`, sorted ascending, deduped.
    pub fn marker_jump_targets(&self) -> Vec<i64> {
        let mut t: Vec<i64> = self.timeline.markers.iter().flat_map(|m| core::iter::once(m.at).chain((m.len > 0).then_some(m.at + m.len))).collect();
        t.sort_unstable();
        t.dedup();
        t
    }
    /// The export marks set to the range of the last marker at or before the playhead (SR723's `set_marks`: one commit,
    /// or none when the marks already equal it). Refused: no such marker, or it is a point marker (`EditError::Marker`).
    pub fn marks_to_marker(&mut self) -> Result<(), EditError> {
        let m = self.timeline.marker_before(self.playhead).ok_or(EditError::Marker)?;
        if m.len <= 0 {
            return Err(EditError::Marker);
        }
        let (a, b) = (m.at, m.at + m.len);
        self.set_marks((Some(a), Some(b)))
    }
    /// The `marker marks` word of [`EditorState::verb`]; `None` for any other word.
    fn marker_marks_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        marker_marks_word(v).then(|| self.marks_to_marker())
    }
}

#[cfg(test)]
mod striamarkerranges_tests {
    use super::*;
    use crate::Kind;

    /// One 35-tick clip; marker A at 5 spanning 4 (end 9), marker B at 12 (a point).
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 35, 0)];
        let tl = tl.mark(5, "A").unwrap().mark(12, "B").unwrap().range_marker(5, 4).unwrap();
        EditorState::new(tl)
    }

    /// STRIAMARKERRANGES M3: a range end is a `marker+` / `marker-` stop between the markers; an end on the next marker's
    /// tick is one target; a jump is not an undo step.
    #[test]
    fn striamarkerranges_range_end_is_a_jump_target() {
        let mut st = state();
        assert_eq!(st.jump_targets(Jump::Marker), alloc::vec![5, 9, 12]);
        for want in [5, 9, 12, 12] {
            assert_eq!(st.verb("marker+"), Some(Ok(())));
            assert_eq!(st.playhead, want);
        }
        for want in [9, 5, 5] {
            assert_eq!(st.verb("marker-"), Some(Ok(())));
            assert_eq!(st.playhead, want);
        }
        assert_eq!(st.undo_depth(), 0, "a jump is not an undo step");
        st.timeline = st.timeline.range_marker(5, 7).unwrap();
        assert_eq!(st.jump_targets(Jump::Marker), alloc::vec![5, 12], "A's end on B's tick: one target");
    }

    /// STRIAMARKERRANGES M3: `marker marks` sets the export marks to the range of the marker at or before the playhead in
    /// ONE undo step; again, nothing; at a point marker or before every marker, `Marker` and no step.
    #[test]
    fn striamarkerranges_marks_to_marker_is_one_commit() {
        let mut st = state();
        assert_eq!(st.verb("marker marks"), Some(Err(EditError::Marker)), "no marker at or before 0");
        st.seek(7);
        assert_eq!(st.verb("marker marks"), Some(Ok(())));
        assert_eq!((st.timeline.marks, st.undo_depth()), ((Some(5), Some(9)), 1));
        assert_eq!(st.verb("marker marks"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 1, "the same marks commit nothing");
        st.seek(20);
        assert_eq!(st.verb("marker marks"), Some(Err(EditError::Marker)), "B is a point marker");
        assert_eq!(st.undo_depth(), 1);
        assert!(st.undo());
        assert_eq!(st.timeline.marks, (None, None));
    }

    /// STRIAMARKERRANGES M3: the word parses exactly; no other parser takes it.
    #[test]
    fn striamarkerranges_word() {
        assert!(marker_marks_word(" marker  marks "));
        for v in ["marker marks x", "marks", "marker", "marker mark"] {
            assert!(!marker_marks_word(v), "{v}");
        }
        assert_eq!(marker_word("marker marks"), None);
        assert_eq!(marker_style_word("marker marks"), None);
        assert_eq!(jump_word("marker marks"), None);
        assert_eq!(mark_words("marker marks"), None);
    }
}
