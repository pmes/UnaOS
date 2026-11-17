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
        let ph = self.playhead; if let Some(r) = self.crop_verb(v) { return Some(r); } if let Some(r) = self.opacity_verb(v) { return Some(r); } if let Some(r) = self.xform_verb(v) { return Some(r); } if let Some(r) = self.size_verb(v) { return Some(r); } if let Some(r) = self.look_verb(v) { return Some(r); } // STRIAOPACITY (SR868): `opacity <pct>` (file tail); STRIAPOSITIONSCALE (SR879): `pip` / `scale` (file tail); NLETRANSITIONFACE (SR691): `kind` / `ease` (file tail); NLESIZEPOLICYUI (SR402): the `size` words (file tail)
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

// ---- STRIACROP (LEDGER SR867): the selected clip's crop through the one commit (named tail block) ----
// `crop <l> <t> <r> <b>` (four integers, 1/1000 of the frame per side; `crop 0 0 0 0` clears) rides THE verb dispatch
// ([`EditorState::verb`]), so the host face, `stria-view --do crop:l:t:r:b` and the kernel Player's keys reach it. ONE
// `commit` of `Timeline::set_crop` (undo-able); the same crop again commits nothing. Refused: no clip selected
// `EditError::NoClip`; an audio clip `EditError::Kind`; bare `crop`, a wrong word count, a non-number or a crop keeping no
// picture `EditError::Crop`.
impl EditorState {
    /// The selected clip cropped by `crop` (one commit; the same crop again commits nothing).
    pub fn set_crop(&mut self, crop: crate::Crop) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.set_crop(track, idx, crop)?;
        if tl == self.timeline {
            return Ok(());
        }
        self.commit(tl);
        Ok(())
    }
    /// The `crop` words of [`EditorState::verb`]; `None` when `v` is not a `crop` word.
    fn crop_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        if w.next() != Some("crop") {
            return None;
        }
        let n: Vec<Option<u16>> = w.map(|x| x.parse().ok()).collect();
        Some(match n.as_slice() {
            [Some(l), Some(t), Some(r), Some(b)] => self.set_crop(crate::Crop { l: *l, t: *t, r: *r, b: *b }),
            _ => Err(EditError::Crop),
        })
    }
}

// ---- STRIAOPACITY (LEDGER SR868): the selection's opacity from the editor through the one commit (named tail block) ----
// `opacity <pct>` (percent 0..=100) rides THE verb dispatch both faces call: every selected clip set through
// `Timeline::set_opacity` as ONE commit (one undo step; `edl::write` keeps it as `* UNA ALPHA`). Refused by name: no
// selection `EditError::NoClip`; a bare `opacity`, a word that is not 0..=100, or an audio clip `EditError::Opacity`.
impl EditorState {
    /// Every selected clip at opacity `pct` (percent), one commit; nothing commits when each already has it.
    pub fn set_opacity(&mut self, pct: u8) -> Result<(), EditError> {
        if self.set.is_empty() {
            return Err(EditError::NoClip);
        }
        let picks = self.set.clone();
        let tl = self.timeline.set_opacity(&picks, pct)?;
        if tl != self.timeline {
            self.commit(tl);
        }
        Ok(())
    }
    /// The `opacity` word of [`EditorState::verb`]; `None` when `v` is not an `opacity` verb.
    fn opacity_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        if w.next() != Some("opacity") {
            return None;
        }
        Some(match (w.next().and_then(|p| p.parse::<u8>().ok()).filter(|&p| p <= crate::OPAQUE), w.next()) {
            (Some(p), None) => self.set_opacity(p),
            _ => Err(EditError::Opacity),
        })
    }
}

// ---- STRIAPOSITIONSCALE (LEDGER SR879): the selected clip's position / scale through the one commit (named tail block) ----
// `pip` (the bottom-right quarter, [`crate::Xform::PIP`]), `pip off` (the whole frame), `pip <x> <y>` (the top-left, the
// scale kept) and `scale <s>` (the scale, the top-left kept), each in 1/1000 of the frame, ride THE verb dispatch
// ([`EditorState::verb`]), so the host face, `stria-view --do pip:x:y` / `scale:s` and the kernel Player's keys reach
// them. ONE `commit` of `Timeline::set_xform` (undo-able); the same placement again commits nothing. Refused: no clip
// selected `EditError::NoClip`; an audio clip `EditError::Kind`; a malformed word or a rect off the frame `EditError::Xform`.
impl EditorState {
    /// The selected clip placed by `xf` (one commit; the same placement again commits nothing).
    pub fn set_xform(&mut self, xf: crate::Xform) -> Result<(), EditError> {
        if let Some(r) = self.xkey_set(xf) { return r; } let (track, idx) = self.selection.ok_or(EditError::NoClip)?; // STRIAXFORMKEYS (SR9049): a keyed clip's placement word edits the point at the playhead (file tail)
        let tl = self.timeline.set_xform(track, idx, xf)?;
        if tl == self.timeline {
            return Ok(());
        }
        self.commit(tl);
        Ok(())
    }
    /// The `pip` / `scale` words of [`EditorState::verb`]; `None` when `v` is neither.
    fn xform_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if let Some(r) = self.xform_preset_verb(v) { return Some(r); } let w: Vec<&str> = v.split_ascii_whitespace().collect(); // STRIAXFORMPRESETS (SR938): `pip tl|tr|bl|br`, `scale-` / `scale+` (file tail)
        let (Some(&word), args) = (w.first(), w.get(1..).unwrap_or(&[])) else { return None };
        if word != "pip" && word != "scale" {
            return None;
        }
        let Some(cur) = self.selected().map(|c| c.xform_at(self.playhead)) else { return Some(Err(EditError::NoClip)) };
        let n: Vec<Option<u16>> = args.iter().map(|x| x.parse().ok()).collect();
        let xf = match (word, args, n.as_slice()) {
            ("pip", [], _) => crate::Xform::PIP,
            ("pip", ["off"], _) => crate::Xform::NONE,
            ("pip", _, [Some(x), Some(y)]) => crate::Xform { x: *x, y: *y, ..cur },
            ("scale", _, [Some(s)]) => crate::Xform { s: *s, ..cur },
            _ => return Some(Err(EditError::Xform)),
        };
        Some(self.set_xform(xf))
    }
}

// ---- STRIAXFORMPRESETS (LEDGER SR938): the placement's corner presets and scale steps through the one commit (named tail block) ----
// `pip tl|tr|bl|br` ([`crate::Xform::corner`]) and `scale-` / `scale+` ([`crate::Xform::stepped`] by
// [`crate::Xform::SCALE_STEP`] about the rect's centre) on the selected clip, each ONE commit of SR879's
// [`EditorState::set_xform`]. Refused: no clip selected `EditError::NoClip`; an audio clip `EditError::Kind`; a step that
// would take the rect off the frame `EditError::Xform`. Any other word falls through to SR879's `pip` / `scale` arms.
impl EditorState {
    /// The preset / step words of [`EditorState::verb`]; `None` when `v` is none of them.
    fn xform_preset_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if let Some(r) = self.xform_nudge_verb(v) { return Some(r); } let w: Vec<&str> = v.split_ascii_whitespace().collect(); // STRIAXFORMMORE (SR975): `pip left|right|up|down` (file tail)
        let d = crate::Xform::SCALE_STEP as i32;
        let xf = match w.as_slice() {
            ["pip", c] => crate::Xform::corner(c)?,
            [step @ ("scale-" | "scale+")] => {
                let Some(cur) = self.selected().map(|c| c.xform_at(self.playhead)) else { return Some(Err(EditError::NoClip)) };
                match cur.fit_step(if *step == "scale+" { d } else { -d }) { // STRIAXFORMMORE (SR975): a step at an edge clamps (file tail)
                    Some(xf) => xf,
                    None => return Some(Err(EditError::Xform)),
                }
            }
            _ => return None,
        };
        Some(self.set_xform(xf))
    }
}

// ---- STRIAXFORMMORE (LEDGER SR975): the placement's nudge words through the one commit (named tail block) ----
// `pip left|right|up|down` move the selected clip's rect by [`crate::Xform::NUDGE_STEP`], stopped at the frame's edges
// ([`crate::Xform::nudged`]), each ONE commit of SR879's [`EditorState::set_xform`]. Refused: no clip selected
// `EditError::NoClip`; an audio clip `EditError::Kind`; a rect that cannot move that way (at the edge, or the whole
// frame) `EditError::Xform`, committing nothing. Any other word falls through to SR938's presets / steps.
impl EditorState {
    /// The nudge words of [`EditorState::verb`]; `None` when `v` is none of them.
    fn xform_nudge_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if let Some(r) = self.xkey_verb(v) { return Some(r); } let n = crate::Xform::NUDGE_STEP as i32; // STRIAXFORMKEYS (SR9049): `pip key|unkey`, `pip unkey all` (file tail)
        let (dx, dy) = match v.split_ascii_whitespace().collect::<Vec<&str>>().as_slice() {
            ["pip", "left"] => (-n, 0),
            ["pip", "right"] => (n, 0),
            ["pip", "up"] => (0, -n),
            ["pip", "down"] => (0, n),
            _ => return None,
        };
        let Some(cur) = self.selected().map(|c| c.xform_at(self.playhead)) else { return Some(Err(EditError::NoClip)) };
        Some(match cur.nudged(dx, dy) {
            Some(xf) => self.set_xform(xf),
            None => self.set_xform(cur).and(Err(EditError::Xform)),
        })
    }
}

// ---- STRIAXFORMKEYS (LEDGER SR9049): the placement keyframe words through the one commit (named tail block) ----
// `pip key` adds (or replaces) a point at the playhead holding the placement shown there; `pip unkey` removes the point
// at the playhead; `pip unkey all` clears every point. On a keyed clip every placement word (SR879 / SR938 / SR975) edits
// the point at the playhead through [`EditorState::set_xform`]'s fold ([`EditorState::xkey_set`]); the placement shown
// there unchanged commits nothing. Each ONE `commit` (undo-able). Refused: no clip selected `EditError::NoClip`; an audio
// clip `EditError::Kind`; the playhead off the selected clip, a ninth point, no point to remove `EditError::XKey`.
impl EditorState {
    /// The selected clip with the points `keys` (one commit; the same points again commit nothing).
    pub fn set_xkeys(&mut self, keys: crate::XKeys) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.set_xkeys(track, idx, keys)?;
        if tl == self.timeline {
            return Ok(());
        }
        self.commit(tl);
        Ok(())
    }
    /// A placement on a KEYED selected clip: the point at the playhead set to `xf`; `None` when the selected clip has no
    /// points (SR879's static placement, as before).
    fn xkey_set(&mut self, xf: crate::Xform) -> Option<Result<(), EditError>> {
        let c = self.selected().filter(|c| !c.xkeys.is_empty())?;
        if !c.covers(self.playhead) {
            return Some(Err(EditError::XKey));
        }
        if !xf.ok() {
            return Some(Err(EditError::Xform));
        }
        if c.xform_at(self.playhead) == xf {
            return Some(Ok(()));
        }
        Some(match c.xkeys.with(c.src_tick(self.playhead), xf) {
            Some(k) => self.set_xkeys(k),
            None => Err(EditError::XKey),
        })
    }
    /// The key words of [`EditorState::verb`]; `None` when `v` is none of them.
    fn xkey_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if let Some(r) = self.xease_verb(v) { return Some(r); } let w: Vec<&str> = v.split_ascii_whitespace().collect(); // STRIAXFORMKEYSEASE (SR9070): `pip ease`, `pip ease <name>` (file tail)
        if !matches!(w.as_slice(), ["pip", "key"] | ["pip", "unkey"] | ["pip", "unkey", "all"]) {
            return None;
        }
        let Some(c) = self.selected() else { return Some(Err(EditError::NoClip)) };
        if self.selection.is_some_and(|(t, _)| self.timeline.tracks.get(t).is_some_and(|tr| tr.kind != crate::Kind::Video)) {
            return Some(Err(EditError::Kind));
        }
        let ph = self.playhead;
        let keys = match w.as_slice() {
            ["pip", "unkey", "all"] if !c.xkeys.is_empty() => Some(crate::XKeys::NONE),
            ["pip", "unkey", "all"] => None,
            _ if !c.covers(ph) => None,
            ["pip", "key"] => c.xkeys.with(c.src_tick(ph), c.xform_at(ph)),
            _ => c.xkeys.without(c.src_tick(ph)),
        };
        Some(match keys {
            Some(k) => self.set_xkeys(k),
            None => Err(EditError::XKey),
        })
    }
}

// ---- STRIAXFORMKEYSEASE (LEDGER SR9070): the placement points' ease words through the one key commit (named tail block) ----
// `pip ease` steps the selected keyed clip's curve through [`crate::Ease::ALL`] (InOut wraps to Linear); `pip ease <name>`
// ([`crate::Ease::from_name`]: linear / in / out / inout) sets it. Each ONE commit of SR9049's [`EditorState::set_xkeys`];
// the same curve again commits nothing. Refused: no clip selected `EditError::NoClip`; an audio clip `EditError::Kind`;
// a clip with no points, an unknown name or extra words `EditError::XKey`.
impl EditorState {
    /// The ease words of [`EditorState::verb`]; `None` when `v` is none of them.
    fn xease_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let w: Vec<&str> = v.split_ascii_whitespace().collect();
        if !matches!(w.as_slice(), ["pip", "ease", ..]) {
            return None;
        }
        let Some(c) = self.selected() else { return Some(Err(EditError::NoClip)) };
        if self.selection.is_some_and(|(t, _)| self.timeline.tracks.get(t).is_some_and(|tr| tr.kind != crate::Kind::Video)) {
            return Some(Err(EditError::Kind));
        }
        let keys = c.xkeys;
        let all = crate::Ease::ALL;
        let e = match w.as_slice() {
            ["pip", "ease"] => Some(all[(all.iter().position(|&e| e == keys.ease()).unwrap_or(0) + 1) % all.len()]),
            ["pip", "ease", name] => crate::Ease::from_name(name),
            _ => None,
        };
        Some(match e.and_then(|e| keys.eased(e)) {
            Some(k) => self.set_xkeys(k),
            None => Err(EditError::XKey),
        })
    }
}
