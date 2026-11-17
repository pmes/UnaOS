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
    pub target: Option<usize>, // STRIAINSERTMARKSKEYSNEXT (SR985): the insert words' target track, session state like `selection` (file tail)
}

impl EditorState {
    pub fn new(timeline: Timeline) -> EditorState {
        EditorState { timeline, selection: None, set: Vec::new(), playhead: 0, zoom: ZOOM_DEFAULT, undo: Vec::new(), redo: Vec::new(), transition: None, target: None }
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
        let ph = self.playhead; if let Some(r) = self.target_verb(v) { return Some(r); } if let Some(r) = self.size_verb(v) { return Some(r); } if let Some(r) = self.look_verb(v) { return Some(r); } if let Some(r) = self.insert_verb(v) { return Some(r); } // NLETRANSITIONFACE (SR691): `kind` / `ease` (file tail); NLESIZEPOLICYUI (SR402): the `size` words (file tail); STRIAINSERTOVERWRITE (SR852): insert / overwrite (file tail); STRIAINSERTMARKSKEYSNEXT (SR985): `target` (file tail)
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
        let (track, _) = self.selection.ok_or(EditError::NoClip)?; let track = self.aim(track)?; // STRIAINSERTMARKSKEYSNEXT (SR985): the target track when set (file tail)
        let c = self.selected().ok_or(EditError::NoClip)?;
        self.place_at_playhead(track, c.src, c.src_in, c.src_out, mode)
    }
    /// Reel `reel`'s ticks `src_in..src_out` laid at the playhead on the first track holding a clip of that reel.
    pub fn insert_reel(&mut self, reel: &str, src_in: i64, src_out: i64, mode: crate::Mode) -> Result<(), EditError> {
        let src = self.timeline.source_by_reel(reel).ok_or(EditError::NoSource)?.id;
        let track = self.timeline.tracks.iter().position(|t| t.clips.iter().any(|c| c.src == src)).ok_or(EditError::NoTrack)?; let track = self.aim(track)?; // STRIAINSERTMARKSKEYSNEXT (SR985): the target track when set (file tail)
        self.place_at_playhead(track, src, src_in, src_out, mode)
    }
    /// The `insert` / `overwrite` words of [`EditorState::verb`]; `None` when `v` is neither (or malformed).
    fn insert_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let Some((mode, range)) = insert_words(v, self.timeline.fps()) else { return self.marked_verb(v); }; // STRIAINSERTMARKS (SR905): else the marked words, a malformed insert shape refused by name (file tail)
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
        "insert-all" => crate::Mode::InsertAll, // STRIAINSERTMARKSKEYSNEXT (SR985): at the playhead, every track rippled
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
            assert_eq!(st.verb(v), if v == "inserts" { None } else { Some(Err(EditError::InsertForm)) }, "{v}: not this verb, or refused by name"); // STRIAINSERTMARKS (SR905)
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

// ---- STRIAINSERTMARKS (LEDGER SR905): three-point editing into the marked record range; the malformed insert shapes refused by name (named tail block) ----
// `insert-marked` / `overwrite-marked` lay the SELECTED clip's source into the timeline's in / out marks (the export
// range, `Timeline::marks`): the record range's length wins, the source range is taken from the selected clip's in point
// for that length, laid on its track at the in mark through SR852's `place_at_playhead` (`Timeline::place`, the ONE
// `commit`). Refused by name: a mark unset `EditError::NoMarks`, nothing selected `EditError::NoClip`, a source shorter
// than the range `EditError::OutOfSource` (`Timeline::validate`); a refusal leaves the state (playhead included)
// unchanged. The marks are kept. Any other shape after `insert` / `overwrite` (or words after a marked word) is
// `EditError::InsertForm`, so the face names it instead of reading an unknown word.
impl EditorState {
    /// The selected clip's source from its in point, for the marked length, laid at the in mark on its track (`mode`).
    pub fn place_marked(&mut self, mode: crate::Mode) -> Result<(), EditError> {
        let (Some(a), Some(b)) = self.timeline.marks else { return Err(EditError::NoMarks) };
        let (track, _) = self.selection.ok_or(EditError::NoClip)?; let track = self.aim(track)?; // STRIAINSERTMARKSKEYSNEXT (SR985): the target track when set (file tail)
        let c = self.selected().ok_or(EditError::NoClip)?;
        let ph = core::mem::replace(&mut self.playhead, a);
        let r = self.place_at_playhead(track, c.src, c.src_in, c.src_in.saturating_add(b - a), mode);
        if r.is_err() {
            self.playhead = ph;
        }
        r
    }
    /// The marked words, and the refusal of a malformed `insert` / `overwrite` shape; `None` for any other word.
    fn marked_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        let mode = match w.next()? {
            "insert-marked" => crate::Mode::Insert,
            "overwrite-marked" => crate::Mode::Overwrite,
            "insert-marked-all" => crate::Mode::InsertAll, // STRIAINSERTMARKSKEYSNEXT (SR985): into the marks, every track rippled
            "insert" | "overwrite" | "insert-all" => return Some(Err(EditError::InsertForm)),
            _ => return None,
        };
        Some(if w.next().is_none() { self.place_marked(mode) } else { Err(EditError::InsertForm) })
    }
}

#[cfg(test)]
mod striainsertmarks_tests {
    use super::*;
    use crate::{Kind, Mode, SourceId};

    /// SR852's cut list (A 13..23 at 0, B 0..10 at 10, A 23..33 at 25) on V1 and A1, with the export marks `m`.
    fn state(m: (Option<i64>, Option<i64>)) -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
        }
        EditorState::new(tl.set_marks(m).unwrap())
    }

    #[test]
    fn striainsertmarks_insert_fills_the_marked_range_from_the_clip_in() {
        let mut st = state((Some(5), Some(9)));
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        st.seek(30);
        assert_eq!(st.verb("insert-marked"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 4, 5), Mode::Insert).unwrap(), "B 0..4 at the in mark");
        assert_eq!(st.timeline.tracks[1], tl0.tracks[1], "insert ripples only the target track");
        assert_eq!(st.selected(), Some(Clip::new(SourceId(1), 0, 4, 5)));
        assert_eq!((st.playhead, st.undo_depth(), st.timeline.marks), (9, 1, (Some(5), Some(9))), "playhead at the out mark, marks kept");
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.timeline, tl0, "ONE undo restores");
    }

    #[test]
    fn striainsertmarks_overwrite_fills_the_marked_range_and_keeps_the_end() {
        let mut st = state((Some(12), Some(20)));
        let tl0 = st.timeline.clone();
        st.select(0, 0).unwrap();
        assert_eq!(st.verb("overwrite-marked"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.place(0, Clip::new(SourceId(0), 13, 21, 12), Mode::Overwrite).unwrap());
        assert_eq!(st.timeline.end(), tl0.end());
        assert_eq!((st.selected(), st.playhead), (Some(Clip::new(SourceId(0), 13, 21, 12)), 20));
    }

    #[test]
    fn striainsertmarks_refusals_by_name_leave_the_state() {
        let mut st = state((Some(0), Some(30)));
        let tl0 = st.timeline.clone();
        st.select(0, 2).unwrap();
        st.seek(7);
        assert_eq!(st.verb("insert-marked"), Some(Err(EditError::OutOfSource)), "A from 23 for 30 runs past 50");
        assert_eq!((st.timeline.clone(), st.playhead, st.undo_depth()), (tl0, 7, 0));
        for m in [(None, None), (Some(5), None), (None, Some(9))] {
            let mut s = state(m);
            s.select(0, 1).unwrap();
            assert_eq!(s.verb("insert-marked"), Some(Err(EditError::NoMarks)), "{m:?}");
            assert_eq!(s.verb("overwrite-marked"), Some(Err(EditError::NoMarks)), "{m:?}");
        }
        let mut s = state((Some(5), Some(9)));
        assert_eq!(s.verb("insert-marked"), Some(Err(EditError::NoClip)));
        s.select(0, 1).unwrap();
        for v in ["insert A", "insert A 1", "insert A 1 x", "insert A 1 2 3", "overwrite A 00:00:00:25 30", "insert-marked now", "overwrite-marked 1"] {
            assert_eq!(s.verb(v), Some(Err(EditError::InsertForm)), "{v}");
        }
        for v in ["inserts", "insert-mark", "marked"] {
            assert_eq!(s.verb(v), None, "{v}: not this verb");
        }
        assert_eq!(s.undo_depth(), 0);
        assert_eq!(EditError::NoMarks.to_string(), "no in / out marks set");
    }
}

// ---- STRIAINSERTMARKSKEYSNEXT (LEDGER SR985): the all-tracks insert words and the insert target track (named tail block) ----
// `insert-all` (bare, or SR852's `<reel> <in> <out>`) and `insert-marked-all` are SR852's / SR905's roads in
// `Mode::InsertAll`: THE one `Timeline::place` ripples every track, the ONE `commit` makes it one undo step. `target`
// sets the insert words' target track: session state like `selection` (not in the Timeline, not in the EDL, no undo step,
// the document stays clean). `target` alone cycles off -> track 0 -> ... -> the last -> off; `target V2` / `target A1` name
// a track by its `Timeline::track_number` label; `target off` clears it; an unknown label (or more words) is
// `EditError::NoTrack`. Every insert road (`insert_selected`, `insert_reel`, `place_marked`: all six insert words) lays on
// the target when one is set; a target of another kind than the road's own track is `EditError::Kind` (`move_clip`'s rule).
// Owed: NLE-INSERT-TARGET-PAINT (the band marks the target's header), NLE-INSERT-KERNEL (no key).
impl EditorState {
    /// The track an insert road lays on: the target when one is set (another kind than `track`'s: `Kind`; gone:
    /// `NoTrack`), else `track`.
    fn aim(&self, track: usize) -> Result<usize, EditError> {
        let Some(t) = self.target else { return Ok(track) };
        let to = self.timeline.tracks.get(t).ok_or(EditError::NoTrack)?.kind;
        let from = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?.kind;
        if to != from {
            return Err(EditError::Kind);
        }
        Ok(t)
    }
    /// Set (or clear) the insert words' target track; no such track: `NoTrack`. No undo step.
    pub fn set_target(&mut self, target: Option<usize>) -> Result<(), EditError> {
        if target.is_some_and(|t| t >= self.timeline.tracks.len()) {
            return Err(EditError::NoTrack);
        }
        self.target = target;
        Ok(())
    }
    /// The target track's label (`V2`, `A1`); `None` with no target.
    pub fn target_label(&self) -> Option<alloc::string::String> {
        let t = self.target?;
        let k = self.timeline.tracks.get(t)?.kind;
        let n = self.timeline.track_number(t)?;
        Some(alloc::format!("{}{n}", if k == crate::Kind::Video { 'V' } else { 'A' }))
    }
    /// The `target` words of [`EditorState::verb`]; `None` for any other word.
    fn target_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        if w.next()? != "target" {
            return None;
        }
        let arg = w.next();
        if w.next().is_some() {
            return Some(Err(EditError::NoTrack));
        }
        let n = self.timeline.tracks.len();
        let to = match arg {
            None => match self.target {
                None if n > 0 => Some(0),
                Some(t) if t + 1 < n => Some(t + 1),
                _ => None,
            },
            Some("off") => None,
            Some(l) => match track_by_label(&self.timeline, l) {
                Some(t) => Some(t),
                None => return Some(Err(EditError::NoTrack)),
            },
        };
        Some(self.set_target(to))
    }
}

/// `V2` / `a1` -> the index of the second video / first audio track; `None` for any other word or a missing track.
fn track_by_label(tl: &Timeline, l: &str) -> Option<usize> {
    let k = match l.as_bytes().first()? {
        b'V' | b'v' => crate::Kind::Video,
        b'A' | b'a' => crate::Kind::Audio,
        _ => return None,
    };
    let n = l.get(1..)?.parse::<usize>().ok()?.checked_sub(1)?;
    tl.tracks.iter().enumerate().filter(|(_, t)| t.kind == k).nth(n).map(|(i, _)| i)
}

#[cfg(test)]
mod striainsertmarkskeysnext_tests {
    use super::*;
    use crate::{Kind, Mode, SourceId};

    /// SR852's cut list (A 13..23 at 0, B 0..10 at 10, A 23..33 at 25) on V1 and A1, and an empty V2 (index 2).
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.add_track(Kind::Video);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
        }
        EditorState::new(tl)
    }

    /// `insert-all` at 15 with B selected: V1 is place's Insert, A1 is split at 15 and rippled by 10; ONE undo restores.
    #[test]
    fn striainsertmarkskeysnext_insert_all_ripples_every_track_and_undoes_once() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        st.seek(15);
        assert_eq!(st.verb("insert-all"), Some(Ok(())));
        let want = tl0.place(0, Clip::new(SourceId(1), 0, 10, 15), Mode::InsertAll).unwrap();
        assert_eq!(st.timeline, want);
        assert_eq!(st.timeline.tracks[0], tl0.place(0, Clip::new(SourceId(1), 0, 10, 15), Mode::Insert).unwrap().tracks[0]);
        let a1 = &st.timeline.tracks[1].clips;
        assert_eq!(a1[..], [Clip::new(SourceId(0), 13, 23, 0), Clip::new(SourceId(1), 0, 5, 10), Clip::new(SourceId(1), 5, 10, 25), Clip::new(SourceId(0), 23, 33, 35)]);
        assert!(st.timeline.tracks[2].clips.is_empty());
        assert_eq!((st.selection, st.playhead, st.undo_depth()), (Some((0, 2)), 25, 1));
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.timeline, tl0, "ONE undo restores");
    }

    /// `insert-marked-all` into 5..9 ripples A1 by 4 from 5; `insert-all A 13 23` names the range; malformed shapes are
    /// `InsertForm`.
    #[test]
    fn striainsertmarkskeysnext_marked_and_reel_shapes() {
        let mut st = state();
        st.timeline = st.timeline.set_marks((Some(5), Some(9))).unwrap();
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        assert_eq!(st.verb("insert-marked-all"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 4, 5), Mode::InsertAll).unwrap());
        assert_eq!(st.timeline.tracks[1].clips[1], Clip::new(SourceId(0), 18, 23, 9), "A1's head split at 5, its tail at 9");
        assert_eq!(st.timeline.marks, (Some(5), Some(9)), "marks kept");
        let mut s = state();
        s.seek(10);
        assert_eq!(s.verb("insert-all A 0 5"), Some(Ok(())));
        assert_eq!(s.timeline.tracks[1].clips[1], Clip::new(SourceId(1), 0, 10, 15), "A1's clip at 10 moved by 5");
        for v in ["insert-all A", "insert-all A 1 2 3", "insert-marked-all now"] {
            assert_eq!(s.verb(v), Some(Err(EditError::InsertForm)), "{v}");
        }
    }

    /// `target V2` steers `insert` onto V2 (V1 untouched); a target of the other kind is `Kind`; the target words take no
    /// undo step and leave the timeline; `target` alone cycles and wraps to off; unknown labels are `NoTrack`.
    #[test]
    fn striainsertmarkskeysnext_target_steers_the_insert_roads() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("target V2"), Some(Ok(())));
        assert_eq!((st.target, st.target_label().as_deref(), st.undo_depth()), (Some(2), Some("V2"), 0));
        assert_eq!(st.timeline, tl0, "the target is not the document");
        st.select(0, 1).unwrap();
        st.seek(5);
        assert_eq!(st.verb("insert"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.place(2, Clip::new(SourceId(1), 0, 10, 5), Mode::Insert).unwrap());
        assert_eq!(st.timeline.tracks[0], tl0.tracks[0], "V1 untouched");
        assert_eq!(st.selection, Some((2, 0)), "the laid clip on V2 selected");
        assert_eq!(st.verb("target a1"), Some(Ok(())));
        st.select(0, 0).unwrap();
        let before = st.timeline.clone();
        assert_eq!(st.verb("insert"), Some(Err(EditError::Kind)));
        assert_eq!(st.verb("insert A 0 5"), Some(Err(EditError::Kind)), "the reel's first track is V1");
        assert_eq!(st.timeline, before);
        assert_eq!(st.verb("target off"), Some(Ok(())));
        assert_eq!(st.target, None);
        for want in [Some(0), Some(1), Some(2), None] {
            assert_eq!(st.verb("target"), Some(Ok(())));
            assert_eq!(st.target, want);
        }
        for v in ["target V3", "target A0", "target X1", "target V1 now"] {
            assert_eq!(st.verb(v), Some(Err(EditError::NoTrack)), "{v}");
        }
        assert_eq!(st.verb("targets"), None);
    }
}

// ---- STRIAKINSERT (LEDGER SR9062): the laying words, held once for both faces (named tail block) ----
// The six words that LAY a clip (SR852 `insert` / `overwrite`, SR905 `insert-marked` / `overwrite-marked`, SR985
// `insert-all` / `insert-marked-all`): after `Some(Ok(()))` from one of them the editor's playhead sits at the laid clip's
// end (`place_at_playhead`), so a repeat appends. The host face's laid line (stria nle_face `laid_verb`) and the kernel
// Player's restart (vplay.rs `edit_key`, its `w` / `d` / `W` / `D` keys) both read this list; neither keeps a copy.

/// `v`'s first word is a laying word (the six insert / overwrite words), with or without arguments.
pub fn lays(v: &str) -> bool {
    matches!(v.split_ascii_whitespace().next(), Some("insert" | "overwrite" | "insert-marked" | "overwrite-marked" | "insert-all" | "insert-marked-all"))
}

#[cfg(test)]
mod striakinsert_tests {
    use super::*;
    use crate::{Kind, Mode, SourceId};

    /// SR852's cut list (A 13..23 at 0, B 0..10 at 10, A 23..33 at 25) on V1 and A1.
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

    /// The kernel Player's `edit_key` by hand: the playhead at `t`, the shown clip (V1 here) selected, then the word.
    fn key(st: &mut EditorState, t: i64, word: &str) -> Option<Result<(), EditError>> {
        st.seek(t);
        match st.timeline.clip_at(0, t) {
            Some(i) => st.select(0, i).unwrap(),
            None => st.deselect(),
        }
        st.verb(word)
    }

    #[test]
    fn striakinsert_lays_the_six_words() {
        for v in ["insert", "overwrite", "insert-marked", "overwrite-marked", "insert-all", "insert-marked-all", "insert B 0 10", "overwrite-marked x"] {
            assert!(lays(v), "{v}");
        }
        for v in ["split", "target", "target V2", "insertx", "inserts", "", "help"] {
            assert!(!lays(v), "{v}");
        }
    }

    /// `w` at 15 inserts the shown clip's source at 15 and lands at 25 (the restart); a second `w` there lays the clip shown at
    /// 25 at 25: nothing is split, the first laid clip stands.
    #[test]
    fn striakinsert_player_sequence_w_then_w_appends() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(key(&mut st, 15, "insert"), Some(Ok(())));
        let tl1 = tl0.place(0, Clip::new(SourceId(1), 0, 10, 15), Mode::Insert).unwrap();
        assert_eq!(st.timeline, tl1);
        assert!(lays("insert"));
        assert_eq!(st.playhead, 25, "the Player restarts at the laid end");
        assert_eq!(key(&mut st, st.playhead, "insert"), Some(Ok(())));
        assert_eq!(st.timeline, tl1.place(0, Clip::new(SourceId(1), 5, 10, 25), Mode::Insert).unwrap());
        assert_eq!(st.timeline.tracks[0].clips.iter().filter(|c| c.src == SourceId(1) && c.src_in == 0 && c.src_out == 10).count(), 1, "the first laid clip is not split");
        assert_eq!(st.playhead, 30);
        assert_eq!(st.undo_depth(), 2);
    }

    /// `W` with marks 5..9 lays the shown clip's source from its in point for 4 ticks at 5 and lands at 9; `D` with a mark
    /// unset is refused by name and leaves the timeline and the playhead.
    #[test]
    fn striakinsert_player_sequence_marked() {
        let mut st = state();
        st.timeline.marks = (Some(5), Some(9));
        let tl0 = st.timeline.clone();
        assert_eq!(key(&mut st, 5, "insert-marked"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.place(0, Clip::new(SourceId(0), 13, 17, 5), Mode::Insert).unwrap());
        assert_eq!(st.playhead, 9);
        let mut st = state();
        st.timeline.marks = (Some(5), None);
        let tl0 = st.timeline.clone();
        assert_eq!(key(&mut st, 12, "overwrite-marked"), Some(Err(EditError::NoMarks)));
        assert_eq!(st.timeline, tl0);
        assert_eq!(st.playhead, 12);
    }
}
