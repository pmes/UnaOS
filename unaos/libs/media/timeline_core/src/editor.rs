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
    redo: Vec<Timeline>, hist: UndoNames, // STRIAUNDOHISTORY (SR876): each undo / redo entry's word, moved with the stacks (file tail)
    /// NLE3d (SR302): the selected transition, `(track, index into tracks[track].transitions)`; the face's widget.
    pub transition: Option<(usize, usize)>,
    pub clipboard: Vec<(usize, crate::Kind, Clip)>, // STRIACOPYPASTE (SR854): the copied clips as (track, its kind, clip), replaced whole by `copy` / `cut` (file tail)
    commits: usize, // STRIAAUTOSAVECAP (SR913): every timeline change (commit, undo, redo), monotonic past UNDO_MAX; read by `commits()` (file tail)
}

impl EditorState {
    pub fn new(timeline: Timeline) -> EditorState {
        EditorState { timeline, selection: None, set: Vec::new(), playhead: 0, zoom: ZOOM_DEFAULT, undo: Vec::new(), redo: Vec::new(), transition: None, clipboard: Vec::new(), hist: UndoNames::default(), commits: 0 }
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
        self.set.push((track, idx)); self.add_mates(); // STRIACLIPLINK (SR863): a linked clip brings its group into the set (file tail)
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
        let tl = self.timeline.split_linked(track, self.playhead)?; // STRIACLIPLINK (SR863): the linked split, `split` for group 0 (lib.rs tail)
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
        let tl = tl.linked(track, idx, |t, k, i, m| match edge { Edge::In => t.trim_in(k, i, m.src_in + delta), Edge::Out => t.trim_out(k, i, m.src_out + delta) })?; self.commit(tl); // STRIALINKTRIM (SR910): every link mate trims by the same source delta, all-or-nothing (lib.rs tail)
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
        self.redo.push(cur); self.hist.undone(); self.commits = self.commits.wrapping_add(1);
        self.settle();
        true
    }
    /// Re-apply the last undone edit; false when there is none.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        let cur = core::mem::replace(&mut self.timeline, next);
        self.undo.push(cur); self.hist.redone(); self.commits = self.commits.wrapping_add(1);
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
        self.undo.push(old); self.hist.pushed();
        if self.undo.len() > UNDO_MAX {
            self.undo.remove(0);
        }
        self.redo.clear(); self.commits = self.commits.wrapping_add(1);
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
            self.set.push((track, idx)); self.add_mates(); // STRIACLIPLINK (SR863): a linked clip brings its group into the set (file tail)
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
    fn verb_unnamed(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if let Some(r) = self.freeze_verb(v) { return Some(r); } if let Some(r) = self.crop_face_verb(v) { return Some(r); } let ph = self.playhead; if let Some(r) = self.crop_verb(v) { return Some(r); } if let Some(r) = self.opacity_verb(v) { return Some(r); } if let Some(r) = self.xform_verb(v) { return Some(r); } if let Some(r) = self.link_verb(v) { return Some(r); } if let Some(r) = self.grade_step_verb(v) { return Some(r); } if let Some(r) = self.grade_verb(v) { return Some(r); } if let Some(r) = self.size_verb(v) { return Some(r); } if let Some(r) = self.look_verb(v) { return Some(r); } if let Some(r) = self.gain_verb(v) { return Some(r); } if let Some(r) = self.track_verb(v) { return Some(r); } if let Some(r) = self.marker_verb(v) { return Some(r); } if let Some(r) = self.roll_verb(v) { return Some(r); } if let Some(r) = self.insert_verb(v) { return Some(r); } if let Some(r) = self.transition_dur_verb(v) { return Some(r); } if let Some(r) = self.xfade_verb(v) { return Some(r); } if let Some(r) = self.clipboard_verb(v) { return Some(r); } if let Some(r) = self.gap_verb(v) { return Some(r); } if let Some(r) = self.tracks_verb(v) { return Some(r); } if let Some(r) = self.track_gain_verb(v) { return Some(r); } if let Some(r) = self.pan_verb(v) { return Some(r); } if let Some(r) = self.track_order_verb(v) { return Some(r); } if let Some(r) = self.paste_mode_verb(v) { return Some(r); } if let Some(r) = self.jump_verb(v) { return Some(r); } if let Some(r) = self.track_gain_step_verb(v) { return Some(r); } if let Some(r) = self.xfade_cycle_verb(v) { return Some(r); } // STRIACLIPGAINFACE (SR838): the gain / fade words (file tail); STRIATRANSITIONLENGTH (SR870): `dur <n>` (file tail); STRIAOPACITY (SR868): `opacity <pct>` (file tail); STRIACLIPLINK (SR863): `link` / `unlink` (file tail); STRIAGRADEFACE (SR899): the six grade step words (file tail); STRIACOLORLIFT (SR869): `color` (file tail); STRIAPAN (SR861): `pan <x>` (file tail); STRIATRACKREORDER (SR877): `track-up` / `track-down` (file tail); STRIAPOSITIONSCALE (SR879): `pip` / `scale` (file tail); NLETRANSITIONFACE (SR691): `kind` / `ease` (file tail); NLESIZEPOLICYUI (SR402): the `size` words (file tail); STRIATRACKMUTESOLO (SR837): `mute` / `solo` (file tail); STRIAROLLEDIT (SR851): the `roll` words (file tail); STRIAINSERTOVERWRITE (SR852): insert / overwrite (file tail); STRIACOPYPASTE (SR854): copy / cut / paste (file tail); STRIAGAPCLOSE (SR857): `close-gaps` / `gap <n>` (file tail); STRIATRACKADDREMOVE (SR856): `track+` / `track-` (file tail); STRIATRACKGAIN (SR864): `track-gain` (file tail); STRIAPASTEINSERT (SR896): paste-insert / paste-over (file tail); STRIAFREEZEFRAME (SR880): `freeze` (file tail); STRIATRACKGAINSTEP (SR901): `track-gain-` / `track-gain+` (file tail)
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
            _ if nudge_words(v).is_some() => self.move_set(nudge_words(v).unwrap_or(0)), // STRIANUDGE (SR855): `nudge <d>` moves the set d ticks (file tail)
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
        let tl = tl.linked(track, idx, |t, k, i, m| match edge { Edge::In => t.ripple_trim_in(k, i, m.src_in + delta), Edge::Out => t.ripple_trim_out(k, i, m.src_out + delta) })?; self.commit(tl); // STRIALINKTRIM (SR910): every link mate ripple-trims by the same source delta, all-or-nothing (lib.rs tail)
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
        let tl = tl.linked(track, idx, |t, k, i, _| t.slip(k, i, delta))?; self.commit(tl); // STRIALINKTRIM (SR910): every link mate slips by the same source delta, all-or-nothing (lib.rs tail)
        Ok(())
    }
    /// Slide the selected clip by `delta` timeline ticks (`Timeline::slide`); the selection stays on it.
    pub fn slide_selected(&mut self, delta: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.slide(track, idx, delta)?;
        let tl = tl.linked(track, idx, |t, k, i, _| t.slide(k, i, delta))?; self.commit(tl); // STRIALINKSLIDE (SR927): every link mate slides by the same timeline delta, all-or-nothing (lib.rs SR910 tail)
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
        self.selected().ok_or(EditError::NoClip)?; // STRIAFADESET (SR906): the NoClip check stays; every word now reads the set's own clips (file tail)
        let refused = if word.starts_with("gain") { EditError::Gain } else { EditError::Fade };
        if extra {
            return Err(refused);
        }
        let fps = self.timeline.fps().unwrap_or(25);
        let ph = self.playhead;
        match (word, arg) {
            ("gain-", None) => self.gain_set(|g| g.saturating_sub(GAIN_STEP_CDB)), // STRIAMULTISELECT (SR853): every clip of the selection steps by its own gain (file tail)
            ("gain+", None) => self.gain_set(|g| g.saturating_add(GAIN_STEP_CDB)), // STRIAMULTISELECT (SR853): the whole selection (file tail)
            ("gain", None) => self.gain_set(|_| 0), // STRIAMULTISELECT (SR853): the whole selection (file tail)
            ("gain", Some(db)) => match cdb_of(db) {
                Some(cdb) => self.gain_set(|_| cdb), // STRIAMULTISELECT (SR853): the whole selection (file tail)
                None => Err(refused),
            },
            ("fade-in", None) => self.fades_set(|k| (k.at..=k.end()).contains(&ph).then(|| (ph - k.at, k.fade_out))), // STRIAFADESET (SR906): every clip of the set the playhead is on; one off it is skipped, none on refused (file tail)
            ("fade-out", None) => self.fades_set(|k| (k.at..=k.end()).contains(&ph).then(|| (k.fade_in, k.end() - ph))), // STRIAFADESET (SR906): the set, as fade-in (file tail)
            ("fade-in", Some(n)) => match ticks_of(n, fps) {
                Some(n) => self.fades_set(|k| Some((n, k.fade_out))), // STRIAFADESET (SR906): the whole selection, all-or-nothing (file tail)
                None => Err(refused),
            },
            ("fade-out", Some(n)) => match ticks_of(n, fps) {
                Some(n) => self.fades_set(|k| Some((k.fade_in, n))), // STRIAFADESET (SR906): the whole selection, all-or-nothing (file tail)
                None => Err(refused),
            },
            _ => Err(refused),
        } // STRIAPANFACE (SR897): and the face's `pan` / `pan-` / `pan+` (file tail)
    }
}

/// A gain word in dB (`-3`, `2.5`) as centi-dB, rounded half away from zero; `None` for a word that is not a finite
/// number. The cast saturates, so an absurd value is refused by `validate`'s `EditError::Gain`.
pub fn cdb_of(s: &str) -> Option<i32> {
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
            "mute" => Some(self.toggle_mute()), "link-av" => Some(self.link_av_all()), // STRIALINKAUTOFUZZ (SR928): link_av over the whole timeline, one commit (file tail)
            "solo" => Some(self.toggle_solo()), "unlink-all" => Some(self.unlink_all()), // STRIALINKAUTO (SR911): every link in the timeline parted, one commit (file tail)
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
    UnmarkAll, // STRIAMARKERRENAME (SR895): `unmark all` removes every marker in ONE commit (file tail)
}

/// STRIAMARKERS (SR835): `unmark`, `mark`, `mark <name>` (the rest trimmed); `None` for any other word, SR723's export
/// words included.
pub fn marker_word(v: &str) -> Option<MarkerWord<'_>> {
    let v = v.trim();
    match v {
        "unmark" => return Some(MarkerWord::Unmark), "unmark all" => return Some(MarkerWord::UnmarkAll), // STRIAMARKERRENAME (SR895): exactly `unmark all` after the trim (file tail)
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
            MarkerWord::Mark(Some(n)) if self.timeline.marker_at(self.playhead).is_some() => self.rename_marker(n), MarkerWord::Mark(name) => self.mark(name), // STRIAMARKERRENAME (SR895): a named mark on a taken tick renames that marker (file tail)
            MarkerWord::Unmark => self.unmark(), MarkerWord::UnmarkAll => self.unmark_all(), // STRIAMARKERRENAME (SR895): (file tail)
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

// ---- STRIAMULTISELECT (LEDGER SR853): gain over the whole selection (named tail block) ----
// NLE2c's set already carries shift-click (`select_add`), the marquee (`select_many` over `view::Layout::crossing`), move
// (`move_set`) and delete (`ripple_delete` / `lift_selected` over the set ops). Gain joins them: [`EditorState::gain_set`]
// folds the existing per-clip `Timeline::set_gain` (its own `validate`) over every clip of `set` and lands as ONE `commit`;
// any clip refused (`EditError::Gain`) refuses the whole edit and leaves the state, and a map that changes no clip commits
// nothing. The set and its primary stay (no clip moves). SR838's `gain-` / `gain+` / `gain [<dB>]` words call it.
impl EditorState {
    /// Every selected clip's gain set to `gain(its own gain_cdb)` (ONE commit; refused whole; no change commits nothing).
    pub fn gain_set(&mut self, gain: impl Fn(i32) -> i32) -> Result<(), EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        let mut tl = self.timeline.clone();
        let mut changed = false;
        for &(t, i) in &self.set {
            let c = tl.tracks.get(t).and_then(|tr| tr.clips.get(i)).copied().ok_or(EditError::NoClip)?;
            let g = gain(c.gain_cdb);
            if g != c.gain_cdb {
                tl = tl.set_gain(t, i, g)?;
                changed = true;
            }
        }
        if changed {
            self.commit(tl);
        }
        Ok(())
    }
}

#[cfg(test)]
mod striamultiselect_tests {
    use super::*;
    use crate::Kind;

    /// V1: A 0..10 at 0, B 0..10 at 10; A1: A 0..20 at 0 at -3 dB. The set: V1's B (primary) and A1's clip.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10).gained(200)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0).gained(-300)];
        let mut st = EditorState::new(tl);
        st.select(1, 0).unwrap();
        st.select_add(0, 1).unwrap();
        st
    }
    fn gains(st: &EditorState) -> [i32; 3] {
        let g = |t: usize, i: usize| st.timeline.tracks[t].clips[i].gain_cdb;
        [g(0, 0), g(0, 1), g(1, 0)]
    }

    #[test]
    fn striamultiselect_gain_steps_every_clip_of_the_set() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("gain+"), Some(Ok(())));
        assert_eq!(gains(&st), [0, 300, -200], "each selected clip by its own gain; the unselected clip stays");
        assert_eq!(st.undo_depth(), 1, "ONE commit for the whole set");
        assert_eq!((st.selection, st.set.clone()), (Some((0, 1)), alloc::vec![(0, 1), (1, 0)]));
        assert_eq!(edl::parse(&edl::write(&st.timeline, "T").unwrap(), 25).unwrap(), st.timeline, "the EDL carries every gain");
        assert_eq!(st.verb("gain -6"), Some(Ok(())));
        assert_eq!(gains(&st), [0, -600, -600]);
        assert_eq!(st.verb("gain"), Some(Ok(())));
        assert_eq!(gains(&st), [0, 0, 0]);
        assert_eq!(st.undo_depth(), 3);
        assert!(st.undo() && st.undo() && st.undo());
        assert_eq!(st.timeline, tl0, "one undo per word");
    }

    #[test]
    fn striamultiselect_gain_refused_whole() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.gain_set(|g| g + crate::GAIN_MAX_CDB - 100), Err(EditError::Gain), "B lands over the max");
        assert_eq!((&st.timeline, st.undo_depth()), (&tl0, 0), "nothing of the set changed");
        assert_eq!(st.gain_set(|g| g), Ok(()));
        assert_eq!(st.undo_depth(), 0, "no change commits nothing");
        st.deselect();
        assert_eq!(st.gain_set(|_| 0), Err(EditError::NoClip));
        assert_eq!(st.verb("gain+"), Some(Err(EditError::NoClip)));
    }
}

// ---- STRIATRANSITIONLENGTH (LEDGER SR870): the `dur <n>` word (named tail block) ----
// The edge drag (SR691's `Drag::begin_transition_edge`) and the host face's `[` / `]` change a transition's length by the
// pointer or by one tick; `dur <n>` gives the selected transition exactly `n` ticks through THE length op
// `set_transition_dur` (one commit; kind, ease and black kept; the ask fresh). The same length again commits nothing.
// Refused as `EditError::Transition`: no transition selected, a bare `dur`, a count that is not an integer, extra words;
// `validate` refuses `n < 1` (`Transition`) and a length past a handle (`Handle`).
impl EditorState {
    /// The `dur <n>` word of [`EditorState::verb`]; `None` when the first word is not `dur`.
    fn transition_dur_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        w.next().filter(|&w| w == "dur")?;
        Some(match (self.selected_transition(), w.next().map(str::parse::<i64>), w.next()) {
            (Some(x), Some(Ok(n)), None) if n == x.dur => Ok(()),
            (Some(_), Some(Ok(n)), None) => self.set_transition_dur(n),
            _ => Err(EditError::Transition),
        })
    }
}

// ---- STRIAAUDIOCROSSFADE (LEDGER SR860): the audio crossfade word (named tail block) ----
// NLEXFADEPOWER (SR678) built the explicit audio crossfade in the model (`Timeline::audio_xfades`, put / taken by
// `Timeline::add_audio_xfade` / `remove_audio_xfade`), the EDL (`* UNA XFADE EQUAL`) and the schedule (THE curve,
// `schedule::xfade_weight`, read into the runs both mixes play); no face reached it. `xfade [<ticks>]` rides THE verb
// dispatch both faces call: the cut is the HEAD of the selected clip on its track. Bare `xfade` takes the explicit record
// off that cut when one is there, else puts an equal-power one: the derived crossfade's length when a video transition
// implies one there, else half a second clamped to the incoming clip and the outgoing source's handle (at least 1).
// `xfade <n>` puts (replaces) an equal-power one of `n` ticks. Each change is ONE `commit`; refused by name: no selection
// (`NoClip`), the clip not on an audio track or no clip of its track ending at its start (`Transition`), a non-number or
// a third word (`Transition`), and the op's own `validate` (`n < 1`: `Transition`, past a handle: `Handle`).
/// The bare word's default length: one second over this (half a second), clamped to the cut's handles.
pub const XFADE_DEFAULT_S_DIV: u32 = 2;

impl EditorState {
    /// Put (`dur`: `Some(n)`) or toggle (`None`) the explicit audio crossfade on the selected clip's head cut; `curve` `None`: equal-power and the toggle, `Some(c)`: put `c`, never take off (STRIAXFADECURVE SR904).
    pub fn toggle_audio_xfade(&mut self, dur: Option<i64>, curve: Option<crate::Curve>) -> Result<(), EditError> {
        let (track, ci) = self.selection.ok_or(EditError::NoClip)?;
        let tl = &self.timeline;
        let t = tl.tracks.get(track).ok_or(EditError::NoTrack)?;
        let i = *t.clips.get(ci).ok_or(EditError::NoClip)?;
        if t.kind != crate::Kind::Audio {
            return Err(EditError::Transition);
        }
        let o = *t.clips.iter().find(|c| c.end() == i.at).ok_or(EditError::Transition)?;
        let next = match dur {
            Some(n) => tl.add_audio_xfade(track, i.at, n, curve.unwrap_or(crate::Curve::EqualPower))?,
            None if curve.is_none() && tl.audio_xfades.iter().any(|x| x.track == track && x.at == i.at) => tl.remove_audio_xfade(track, i.at)?,
            None => {
                let handle = tl.source(o.src).ok_or(EditError::NoSource)?.duration - o.src_out;
                let half = (tl.fps().unwrap_or(25) / XFADE_DEFAULT_S_DIV) as i64;
                let n = tl.crossfade_at(track, ci).map_or(half.min(i.len()).min(handle), |x| x.dur).max(1);
                tl.add_audio_xfade(track, i.at, n, curve.unwrap_or(crate::Curve::EqualPower))?
            }
        };
        self.commit(next);
        Ok(())
    }
    /// The `xfade` words of [`EditorState::verb`]; `None` when `v` is not one.
    fn xfade_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        w.next().filter(|&w| w == "xfade")?;
        Some(match (w.next(), w.next()) {
            (None, _) => self.toggle_audio_xfade(None, None),
            (Some(n), None) => match n.parse::<i64>() {
                Ok(n) => self.toggle_audio_xfade(Some(n), None),
                Err(_) => self.xfade_curve_word(None, n, None),
            },
            (Some(n), Some(c)) => self.xfade_curve_word(Some(n), c, w.next()),
        })
    }
}

// ---- STRIACOPYPASTE (LEDGER SR854): copy / cut / paste of the selection at the playhead (named tail block) ----
// `copy` / `cut` / `paste` ride THE verb dispatch both faces call. The clipboard is a value on the state beside the
// selection, outside the Timeline: not undone, not in the EDL, not dirtying the document, replaced WHOLE by each `copy` /
// `cut` (so it holds at most the set's clips; `view::Drag::shown` builds its scratch state through `EditorState::new`,
// so the clipboard never rides a drag preview). `copy` takes the set's clips (no commit); `cut` takes them then lifts them
// through `lift_selected` (ONE commit, the holes stay). `paste` keeps the copied layout, its lowest track on the
// selected clip's track (none selected: where it was copied from) and its earliest clip at the playhead, through
// `Timeline::paste` (THE `place` road, a paste never overwrites: `EditError::Overlap`) and ONE commit; the pasted clips
// become the set and the playhead moves to their end, so a repeat appends. Refused by name: nothing selected / an empty
// clipboard `EditError::NoClip`, `NoTrack`, `Kind`, `Overlap`.
impl EditorState {
    /// The set's clips as clipboard entries `(track, its kind, clip)`, in set order.
    fn copied(&self) -> Result<Vec<(usize, crate::Kind, Clip)>, EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        let tl = &self.timeline;
        self.set.iter().map(|&(t, i)| tl.tracks.get(t).and_then(|tr| Some((t, tr.kind, *tr.clips.get(i)?))).ok_or(EditError::NoClip)).collect()
    }
    /// The selected clips onto the clipboard (no edit).
    pub fn copy_selected(&mut self) -> Result<(), EditError> {
        self.clipboard = self.copied()?;
        Ok(())
    }
    /// The selected clips onto the clipboard, then lifted (`lift_selected`, ONE commit); the clipboard changes only
    /// when the lift lands.
    pub fn cut_selected(&mut self) -> Result<(), EditError> {
        let c = self.copied()?;
        self.lift_selected()?;
        self.clipboard = c;
        Ok(())
    }
    /// The clipboard laid at the playhead on the selected track (see the block comment; ONE commit). STRIAPASTEINSERT (SR896): `mode` as `Timeline::paste_as` (None never overwrites, Insert ripples, Overwrite carves).
    pub fn paste_as(&mut self, mode: Option<crate::Mode>) -> Result<(), EditError> {
        let t0 = self.clipboard.iter().map(|e| e.0).min().ok_or(EditError::NoClip)?;
        let a0 = self.clipboard.iter().map(|e| e.2.at).min().ok_or(EditError::NoClip)?;
        let base = self.selection.map_or(t0, |p| p.0);
        let items: Vec<(usize, crate::Kind, Clip)> = self.clipboard.iter().map(|&(t, k, c)| (t - t0 + base, k, Clip { at: c.at - a0 + self.playhead, ..c })).collect();
        let tl = self.timeline.paste_as(&items, mode)?;
        self.commit(tl);
        self.set.clear();
        for &(t, _, c) in &items {
            if let Some(i) = self.timeline.clip_at(t, c.at) {
                self.set.push((t, i));
            }
        }
        let first = items.first().and_then(|&(t, _, c)| self.timeline.clip_at(t, c.at).map(|i| (t, i)));
        self.set.sort_unstable();
        self.selection = first;
        self.settle();
        let end = items.iter().map(|e| e.2.end()).max().unwrap_or(self.playhead);
        self.seek(end);
        Ok(())
    }
    /// The `copy` / `cut` / `paste` words of [`EditorState::verb`]; `None` for any other word.
    fn clipboard_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        Some(match v {
            "copy" => self.copy_selected(),
            "cut" => self.cut_selected(),
            "paste" => self.paste(),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod striacopypaste_tests {
    use super::*;
    use crate::{Kind, SourceId};

    /// `verb_tests`' cut list (A 13..23 at 0, B 0..10 at 10, A 23..33 at 25) on V1 and on A1.
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
    fn striacopypaste_copy_paste_at_the_playhead_one_undo() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        assert_eq!(st.verb("copy"), Some(Ok(())));
        assert_eq!((st.timeline == tl0, st.undo_depth()), (true, 0), "copy is no edit");
        assert_eq!(st.clipboard, alloc::vec![(0, Kind::Video, Clip::new(SourceId(1), 0, 10, 10))]);
        st.seek(35);
        assert_eq!(st.verb("paste"), Some(Ok(())));
        assert_eq!(st.timeline.tracks[0].clips[3], Clip::new(SourceId(1), 0, 10, 35));
        assert_eq!(st.timeline.tracks[1], tl0.tracks[1], "only the selected track");
        assert_eq!((st.selection, st.set.clone(), st.playhead), (Some((0, 3)), alloc::vec![(0, 3)], 45));
        assert_eq!(st.verb("paste"), Some(Ok(())), "a repeat appends");
        assert_eq!(st.timeline.tracks[0].clips[4], Clip::new(SourceId(1), 0, 10, 45));
        assert_eq!(st.undo_depth(), 2);
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.timeline, tl0, "one undo per paste");
        assert_eq!(st.clipboard.len(), 1, "the clipboard is not undone");
    }

    #[test]
    fn striacopypaste_cut_lifts_and_paste_puts_it_back() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.select(0, 1).unwrap();
        assert_eq!(st.verb("cut"), Some(Ok(())));
        assert_eq!(st.timeline, tl0.lift(0, 1).unwrap(), "the hole stays");
        assert_eq!((st.selection, st.undo_depth()), (None, 1));
        st.seek(10);
        assert_eq!(st.verb("paste"), Some(Ok(())), "no selection: the track it was copied from");
        assert_eq!(st.timeline, tl0);
        assert_eq!(st.undo_depth(), 2);
    }

    #[test]
    fn striacopypaste_set_keeps_its_layout_across_tracks() {
        let mut st = state();
        st.select_many(&[(0, 0), (1, 1)], false);
        assert_eq!(st.verb("copy"), Some(Ok(())));
        st.seek(35);
        st.select(0, 2).unwrap();
        assert_eq!(st.verb("paste"), Some(Ok(())));
        assert_eq!(st.timeline.tracks[0].clips[3], Clip::new(SourceId(0), 13, 23, 35));
        assert_eq!(st.timeline.tracks[1].clips[3], Clip::new(SourceId(1), 0, 10, 45));
        assert_eq!((st.set.clone(), st.playhead), (alloc::vec![(0, 3), (1, 3)], 55));
    }

    #[test]
    fn striacopypaste_refusals_by_name_leave_the_state() {
        let mut st = state();
        assert_eq!(st.verb("copy"), Some(Err(EditError::NoClip)));
        assert_eq!(st.verb("cut"), Some(Err(EditError::NoClip)));
        assert_eq!(st.verb("paste"), Some(Err(EditError::NoClip)), "empty clipboard");
        st.select(0, 1).unwrap();
        st.verb("copy").unwrap().unwrap();
        let tl0 = st.timeline.clone();
        st.seek(12);
        assert_eq!(st.verb("paste"), Some(Err(EditError::Overlap)), "a paste never overwrites");
        st.seek(35);
        st.select(1, 0).unwrap();
        assert_eq!(st.verb("paste"), Some(Err(EditError::Kind)));
        assert_eq!((st.timeline == tl0, st.undo_depth()), (true, 0));
        assert_eq!(st.verb("copyx"), None);
    }
}

// ---- STRIAGAPCLOSE (LEDGER SR857): close-gaps / gap <n> from the editor through the one commit (named tail block) ----
// `close-gaps` closes every gap on the selected clip's track (`Timeline::close_gaps`, the leading gap included); `gap <n>`
// opens `n` ticks of gap at the playhead on that track (`Timeline::insert_gap`). They ride THE verb dispatch both faces call;
// each is ONE `commit` (one undo step), an equal value commits nothing, nothing selected is `EditError::NoClip`. After a
// `gap` that split a clip, the selected indices past it on that track step by one, so the selection follows its clip.
impl EditorState {
    /// Close every gap on the selected clip's track (one commit; no gap commits nothing).
    pub fn close_gaps(&mut self) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.close_gaps(track)?;
        if tl != self.timeline {
            self.commit(tl);
        }
        Ok(())
    }
    /// Open `n` ticks of gap at the playhead on the selected clip's track (one commit).
    pub fn insert_gap(&mut self, n: i64) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoClip)?;
        let ph = self.playhead;
        let split = self.timeline.tracks.get(track).and_then(|t| t.clips.iter().position(|c| c.at < ph && ph < c.end()));
        let tl = self.timeline.insert_gap(track, ph, n)?;
        self.commit(tl);
        if let Some(s) = split {
            for p in self.set.iter_mut().chain(self.selection.iter_mut()).filter(|p| p.0 == track && p.1 > s) {
                p.1 += 1;
            }
        }
        Ok(())
    }
    /// The `close-gaps` / `gap <n>` words of [`EditorState::verb`]; `None` when `v` is neither.
    fn gap_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if v.split_ascii_whitespace().eq(["close-gaps", "all"]) { return Some(self.close_gaps_all()); } if v.trim() == "gap" { let s = self.timeline.tb.from_ns(1_000_000_000).max(1); return Some(self.insert_gap(s)); } Some(match gap_words(v)? { // STRIAGAPSBLACK (SR909): a bare `gap` opens one second; STRIAGAPSALLTRACKS (SR929): `close-gaps all` (file tail)
            None => self.close_gaps(),
            Some(n) => self.insert_gap(n),
        })
    }
}

/// `close-gaps` -> `Some(None)`, `gap <n>` (n an unsigned 32-bit count) -> `Some(Some(n))`; any other shape is not this verb.
fn gap_words(v: &str) -> Option<Option<i64>> {
    let mut w = v.split_ascii_whitespace();
    let n = match w.next()? {
        "close-gaps" => None,
        "gap" => Some(i64::from(w.next()?.parse::<u32>().ok()?)),
        _ => return None,
    };
    w.next().is_none().then_some(n)
}

// ---- STRIANUDGE (LEDGER SR855): the nudge by a count (named tail block) ----
// `nudge <d>` rides THE verb dispatch both faces call: the selection set moves `d` ticks (one tick is one frame) through
// [`EditorState::move_set`], the same op and ONE commit `nudge-` / `nudge+` take; a refusal leaves the state unchanged.

/// `nudge <d>` (signed, nonzero ticks) -> `d`; any other shape (`nudge 0`, `nudge x`, `nudge 1 2`) is not this verb.
pub fn nudge_words(v: &str) -> Option<i64> {
    let mut w = v.split_ascii_whitespace();
    if w.next() != Some("nudge") {
        return None;
    }
    let d = w.next()?.parse::<i64>().ok().filter(|&d| d != 0)?;
    w.next().is_none().then_some(d)
}

// ---- STRIATRACKADDREMOVE (LEDGER SR856): `track+` / `track-` through the one commit, the selection remapped (named tail block) ----
// The words ride THE verb dispatch both faces call. `track+ video` / `track+ audio` add an empty track at its canonical
// slot (`Timeline::insert_track`); `track- <ch>` removes the track its channel word names (`V`, `V2`..`V9`, `A`, `A1`..`A9`,
// the EDL's one channel parser), bare `track-` the primary selection's track, with its clips (`Timeline::remove_track`).
// Each is ONE `commit` (undo restores the whole timeline). The selection, the set and the selected transition carry a track
// index: they are remapped to the new layout BEFORE the commit (a removed track's entries are dropped), so `settle` checks
// the new layout. Refused by name: a tenth track of a kind `EditError::Tracks`; bare `track-` with nothing selected, or a
// channel word naming no track, `EditError::NoTrack`. Any other `track+` shape is not this verb.
impl EditorState {
    /// Add an empty track of `kind` at its canonical slot (one commit); later selected indices move up one track.
    pub fn add_track(&mut self, kind: crate::Kind) -> Result<(), EditError> {
        let slot = self.timeline.track_slot(kind);
        let tl = self.timeline.insert_track(kind)?;
        self.retrack(|t| Some(if t >= slot { t + 1 } else { t }));
        self.commit(tl);
        Ok(())
    }
    /// Remove track `track` with its clips (one commit); its selected entries go, later ones move down one track.
    pub fn remove_track(&mut self, track: usize) -> Result<(), EditError> {
        let tl = self.timeline.remove_track(track)?;
        self.retrack(|t| match t {
            t if t == track => None,
            t if t > track => Some(t - 1),
            t => Some(t),
        });
        self.commit(tl);
        Ok(())
    }
    /// The track `track-` words name: `Some(Ok(index))`, `Some(Err(NoTrack))` for none, `None` when `v` is no `track-` word.
    /// A face reads it before the verb to name what went (the message line).
    pub fn track_minus_target(&self, v: &str) -> Option<Result<usize, EditError>> {
        let mut w = v.split_ascii_whitespace();
        if w.next() != Some("track-") {
            return None;
        }
        Some(match (w.next(), w.next()) {
            (None, _) => self.selection.map(|p| p.0).filter(|&t| t < self.timeline.tracks.len()).ok_or(EditError::NoTrack),
            (Some(ch), None) => edl::channel(ch).and_then(|(k, n)| self.timeline.track_of(k, n)).ok_or(EditError::NoTrack),
            _ => Err(EditError::NoTrack),
        })
    }
    /// Every held track index through `f` (`None`: dropped): the set, the primary, the selected transition.
    fn retrack(&mut self, f: impl Fn(usize) -> Option<usize>) {
        self.set = self.set.iter().filter_map(|&(t, i)| f(t).map(|t| (t, i))).collect();
        self.set.sort_unstable();
        self.selection = self.selection.and_then(|(t, i)| f(t).map(|t| (t, i)));
        self.transition = self.transition.and_then(|(t, i)| f(t).map(|t| (t, i)));
    }
    /// The `track+` / `track-` words of [`EditorState::verb`]; `None` for any other word.
    fn tracks_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if let Some(target) = self.track_minus_target(v) {
            return Some(target.and_then(|t| self.remove_track(t)));
        }
        match v {
            "track+ video" => Some(self.add_track(crate::Kind::Video)),
            "track+ audio" => Some(self.add_track(crate::Kind::Audio)),
            _ => None,
        }
    }
}

// ---- STRIATRACKGAIN (LEDGER SR864): `track-gain` on the selected clip's track through the one commit (named tail block) ----
// The word rides THE verb dispatch both faces call. RULE (SR837's): it acts on the SELECTED clip's track; with nothing
// selected it is refused `EditError::NoTrack`. `track-gain <dB>` sets the track's gain (SR838's `cdb_of`: decimal dB,
// rounded half away from zero), bare `track-gain` puts it back to unity (the stria-view button). A word that is not a
// number, an extra word, or a gain outside the clip range is refused `EditError::Gain`; the state stays. Each change is ONE
// `commit` (undo-able; it dirties the document, and `edl::write` saves it as `* UNA TGAIN`); the same gain commits nothing.
impl EditorState {
    /// The selected clip's track's gain set to `cdb` centi-dB (`Timeline::set_track_gain`, ONE commit; the same gain
    /// commits nothing).
    pub fn set_selected_track_gain(&mut self, cdb: i32) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoTrack)?;
        let t = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?;
        if t.gain_cdb == cdb {
            return Ok(());
        }
        let tl = self.timeline.set_track_gain(track, cdb)?;
        self.commit(tl);
        Ok(())
    }
    /// The `track-gain` word of [`EditorState::verb`]; `None` for any other word.
    fn track_gain_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        if w.next() != Some("track-gain") {
            return None;
        }
        if self.selection.is_none() {
            return Some(Err(EditError::NoTrack));
        }
        Some(match (w.next(), w.next()) {
            (None, _) => self.set_selected_track_gain(0),
            (Some(db), None) => cdb_of(db).map_or(Err(EditError::Gain), |cdb| self.set_selected_track_gain(cdb)),
            _ => Err(EditError::Gain),
        })
    }
}

#[cfg(test)]
mod striatrackgain_tests {
    use super::*;
    use crate::Kind;

    /// A1: A 0..20 at 0, A2: A 0..20 at 0; the clip on A2 selected.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        for _ in 0..2 {
            let t = tl.add_track(Kind::Audio);
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        }
        let mut st = EditorState::new(tl);
        st.select(1, 0).unwrap();
        st
    }

    /// M3: `track-gain -6` sets the selected clip's track (not the other, not the clip) in ONE undo step; the same value
    /// again pushes none; bare `track-gain` is unity; undo / redo walk it.
    #[test]
    fn striatrackgain_verb_sets_through_one_commit() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("track-gain -6"), Some(Ok(())));
        assert_eq!((st.timeline.tracks[1].gain_cdb, st.timeline.tracks[0].gain_cdb, st.timeline.tracks[1].clips[0].gain_cdb), (-600, 0, 0));
        assert_eq!(st.undo_depth(), 1);
        assert_eq!(st.verb("track-gain -6.0"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 1, "the same gain commits nothing");
        assert_eq!(st.verb("track-gain 2.5"), Some(Ok(())));
        assert_eq!((st.timeline.tracks[1].gain_cdb, st.undo_depth()), (250, 2));
        assert_eq!(st.verb("track-gain"), Some(Ok(())));
        assert_eq!((st.timeline.tracks[1].gain_cdb, st.undo_depth()), (0, 3));
        st.undo();
        assert_eq!(st.timeline.tracks[1].gain_cdb, 250);
        st.undo();
        st.undo();
        assert_eq!(st.timeline, tl0);
        st.redo();
        assert_eq!(st.timeline.tracks[1].gain_cdb, -600);
    }

    /// M3: each refusal by name, the state and the undo depth untouched.
    #[test]
    fn striatrackgain_verb_refusals() {
        let mut none = state();
        none.selection = None;
        let tl0 = none.timeline.clone();
        assert_eq!(none.verb("track-gain -6"), Some(Err(EditError::NoTrack)));
        assert_eq!(none.verb("track-gain"), Some(Err(EditError::NoTrack)));
        let mut st = state();
        for bad in ["track-gain x", "track-gain 1 2", "track-gain 30", "track-gain -145", "track-gain inf"] {
            assert_eq!(st.verb(bad), Some(Err(EditError::Gain)), "{bad}");
        }
        assert_eq!((&st.timeline, st.undo_depth(), &none.timeline), (&tl0, 0, &tl0));
        assert_eq!(st.verb("track-gainx"), None, "not this word");
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
        if self.set.len() > 1 { return self.crop_set(|_| crop); } let (track, idx) = self.selection.ok_or(EditError::NoClip)?; // STRIACROPFACE (SR900): a set of more than one clip crops through `crop_set` (file tail)
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
        if let Some(d) = opacity_step(v) { return Some(self.step_opacity(d)); } let mut w = v.split_ascii_whitespace(); // STRIAOPACITYFACE (SR898): the `opacity-` / `opacity+` step words (file tail)
        if w.next() != Some("opacity") {
            return None;
        }
        Some(match (w.next().and_then(|p| p.parse::<u8>().ok()).filter(|&p| p <= crate::OPAQUE), w.next()) {
            (Some(p), None) => self.set_opacity(p),
            _ => Err(EditError::Opacity),
        })
    }
}

// ---- STRIACLIPLINK (LEDGER SR863): link selection and the `link` / `unlink` words (named tail block) ----
// Selecting a clip selects its whole link group (`Timeline::mates`), so the set ops already carry it: a move is ONE
// `move_many`, `delete` / `lift` take every member, `split` cuts them all (`Timeline::split_linked`); trims, slip and
// slide: trims and slip carry every mate by the same source delta (STRIALINKTRIM SR910), slide carries every mate by the same timeline delta (STRIALINKSLIDE SR927). `link` gives the set (two or more clips) ONE fresh group, `unlink` gives it 0; ONE commit
// each through THE verb dispatch both faces call. Refused `EditError::NoClip`: nothing selected, `link` of one clip.
impl EditorState {
    /// The set grown by every pick's link mates (sorted, deduped).
    fn add_mates(&mut self) {
        let more: Vec<(usize, usize)> = self.set.iter().flat_map(|&(t, i)| self.timeline.mates(t, i)).collect();
        self.set.extend(more);
        self.set.sort_unstable();
        self.set.dedup();
    }
    /// Link the selected clips into one fresh group (one commit).
    pub fn link_selected(&mut self) -> Result<(), EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        if self.set.len() < 2 {
            return Err(EditError::NoClip);
        }
        let tl = self.timeline.set_group(&self.set, self.timeline.next_group())?;
        self.commit(tl);
        Ok(())
    }
    /// Unlink the selected clips (one commit; none linked commits nothing).
    pub fn unlink_selected(&mut self) -> Result<(), EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        let tl = &self.timeline;
        if !self.set.iter().any(|&(t, i)| tl.tracks.get(t).and_then(|tr| tr.clips.get(i)).is_some_and(|c| c.group != 0)) {
            return Ok(());
        }
        let tl = self.timeline.set_group(&self.set, 0)?;
        self.commit(tl);
        Ok(())
    }
    /// The `link` / `unlink` words of [`EditorState::verb`]; `None` for any other word.
    fn link_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        match v {
            "link" => Some(self.link_selected()),
            "unlink" => Some(self.unlink_selected()),
            _ => None,
        }
    }
}

// ---- STRIACOLORLIFT (LEDGER SR869): the `color` word through the one commit (named tail block) ----
// `color <b> <c> <s>` (three integers in 1/1000, each -1000..=1000) grades the selected clip ([`crate::Grade`]) through
// `Timeline::set_grade` and the ONE `commit` (undo-able; `edl::write` saves it as `* UNA COLOR`); bare `color` clears it.
// It rides THE verb dispatch both faces call. Refused by name: no clip selected `EditError::NoClip`; a misshapen word or a
// value out of range `EditError::Grade`; a graded audio clip `EditError::Kind`. The same grade again commits nothing.
impl EditorState {
    /// The selected clip graded `g` (one commit; the same grade again commits nothing).
    pub fn set_grade(&mut self, g: crate::Grade) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        if c.grade == g {
            return Ok(());
        }
        let tl = self.timeline.set_grade(track, idx, g)?;
        self.commit(tl);
        Ok(())
    }
    /// The `color` words of [`EditorState::verb`]; `None` when `v` is not one.
    fn grade_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        if w.next() != Some("color") {
            return None;
        }
        let n: Vec<&str> = w.collect();
        let vals: Option<Vec<i16>> = n.iter().map(|x| x.parse::<i16>().ok()).collect();
        Some(match (n.len(), vals) {
            (0, _) => self.set_grade(crate::Grade::IDENTITY),
            (3, Some(x)) => self.set_grade(crate::Grade { brightness: x[0], contrast: x[1], saturation: x[2] }),
            _ => Err(EditError::Grade),
        })
    }
}

// ---- STRIAPAN (LEDGER SR861): the clip's pan from the editor (named tail block) ----
// `pan <x>` (x a decimal in -1..=1, `round(x * 127)` as `Clip::pan_cq8`) rides THE verb dispatch both faces call: it sets the
// selected clip's pan through `Timeline::set_pan` and the ONE `commit` (one undo step; the same pan again commits nothing).
// Refused by name: no clip selected `EditError::NoClip`. A malformed word (`pan`, `pan 2`, `pan x`, extra words) is not this
// verb (`None`), the `slip` / `mark` parsers' rule.
impl EditorState {
    /// The selected clip's pan set to `pan_cq8` (one commit; the same pan again commits nothing).
    pub fn set_selected_pan(&mut self, pan_cq8: i8) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        if c.pan_cq8 == pan_cq8 {
            return Ok(());
        }
        let tl = self.timeline.set_pan(track, idx, pan_cq8)?;
        self.commit(tl);
        Ok(())
    }
    /// The `pan <x>` word of [`EditorState::verb`]; `None` when `v` is not that shape.
    fn pan_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        pan_words(v).or_else(|| self.pan_step(v)).map(|p| self.set_selected_pan(p)) // STRIAPANFACE (SR897): and the face's `pan` / `pan-` / `pan+` (file tail)
    }
}

/// `pan <x>` with `x` a finite decimal in -1..=1 -> `round(x * 127)`; any other shape is not this verb.
fn pan_words(v: &str) -> Option<i8> {
    let mut w = v.split_ascii_whitespace();
    if w.next() != Some("pan") {
        return None;
    }
    let x = w.next()?.parse::<f64>().ok().filter(|x| (-1.0..=1.0).contains(x))?;
    w.next().is_none().then_some(libm::round(x * 127.0) as i8)
}

// ---- STRIATRACKREORDER (LEDGER SR877): track-up / track-down through the one commit (named tail block) ----
// `track-up` / `track-down` ride THE verb dispatch both faces call: the selected clip's track swaps with its nearest
// same-kind neighbour (`Timeline::swap_track`; up = toward row 0 on screen = down in the video composite) as ONE `commit`
// (undo-able, it dirties the document). The selection, the set and the selected transition carry track indices that
// `settle` only bounds-checks, so `retrack_swap` remaps them BEFORE the commit: the selection follows its clip. FOLD NOTE:
// SR856's `EditorState::retrack` remaps the same indices; `retrack_swap` and `retrack` MUST become ONE fn at the fold.
// Refused by name: no clip selected `EditError::NoClip`, no same-kind track that way `EditError::NoTrack`.
impl EditorState {
    /// The selected clip's track swapped with its same-kind neighbour above (`up`) or below (one commit).
    pub fn move_track(&mut self, up: bool) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoClip)?;
        let j = self.timeline.track_neighbour(track, up)?;
        let tl = self.timeline.swap_track(track, up)?;
        self.retrack_swap(track, j);
        self.commit(tl);
        Ok(())
    }
    /// STRIATRACKREORDER: the selection, the set and the selected transition after tracks `a` and `b` swap.
    fn retrack_swap(&mut self, a: usize, b: usize) {
        let m = |t: usize| crate::track_swapped(t, a, b);
        self.selection = self.selection.map(|(t, i)| (m(t), i));
        for p in self.set.iter_mut() {
            p.0 = m(p.0);
        }
        self.set.sort_unstable();
        self.transition = self.transition.map(|(t, i)| (m(t), i));
    }
    /// The `track-up` / `track-down` words of [`EditorState::verb`]; `None` for any other word.
    fn track_order_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        match v {
            "track-up" => Some(self.move_track(true)),
            "track-down" => Some(self.move_track(false)),
            _ => None,
        }
    }
}

// ---- STRIAOPACITYFACE (LEDGER SR898): the opacity step words (named tail block) ----
// `opacity-` / `opacity+` (the face's buttons) step the primary selected clip's opacity by [`OPACITY_STEP`] percent,
// clamped to 0..=[`crate::OPAQUE`], and set the whole selection to it through SR868's `EditorState::set_opacity` (ONE
// commit; at a bound nothing commits). Refused by name: no selection `EditError::NoClip`; an audio clip `EditError::Opacity`.

/// STRIAOPACITYFACE: one opacity step (percent).
pub const OPACITY_STEP: i16 = 10;

/// STRIAOPACITYFACE: the step of an opacity step word; `None` for every other word.
fn opacity_step(v: &str) -> Option<i16> {
    match v {
        "opacity-" => Some(-OPACITY_STEP),
        "opacity+" => Some(OPACITY_STEP),
        _ => None,
    }
}

impl EditorState {
    /// STRIAOPACITYFACE: the selection at the primary selected clip's opacity + `d` (percent, clamped), one commit.
    pub fn step_opacity(&mut self, d: i16) -> Result<(), EditError> {
        let base = self.selected().ok_or(EditError::NoClip)?.opacity;
        let pct = (base as i16 + d).clamp(0, crate::OPAQUE as i16) as u8;
        self.set_opacity(pct)
    }
}

// ---- STRIAMARKERRENAME (LEDGER SR895): rename the playhead's marker / remove every marker through the one commit (named tail block) ----
// `mark <name>` on a tick that already holds a marker RENAMES it (SR835 refused it); bare `mark` there is still refused (a
// default `M<n>` would erase a chosen name). `unmark all` removes every marker. Each is ONE `commit` (one undo step; the
// document dirty; `edl::write` keeps the new name in its `* UNA MARKER` line). Refused by name, no commit:
// `EditError::Marker` for a rename to the name it already carries (or no marker there) and for `unmark all` with none.

impl EditorState {
    /// The marker at the playhead renamed `name`: ONE commit. Refused: no marker there, or it is already `name`
    /// (`EditError::Marker`).
    pub fn rename_marker(&mut self, name: &str) -> Result<(), EditError> {
        let tl = self.timeline.rename_marker(self.playhead, name).ok_or(EditError::Marker)?;
        self.commit(tl);
        Ok(())
    }
    /// Every marker removed: ONE commit. Refused: no marker (`EditError::Marker`).
    pub fn unmark_all(&mut self) -> Result<(), EditError> {
        let tl = self.timeline.unmark_all().ok_or(EditError::Marker)?;
        self.commit(tl);
        Ok(())
    }
}

// ---- STRIAPASTEINSERT (LEDGER SR896): paste-insert / paste-over through the one paste walk (named tail block) ----
// `paste-insert` lays the clipboard through `Timeline::place(.., Mode::Insert)` (each target track ripples) and `paste-over`
// through `Mode::Overwrite` WITHOUT SR854's overlap refusal (it carves): the same anchor rule as `paste` (lowest copied
// track on the selected clip's track, earliest clip at the playhead), the same walk (`paste_as`), ONE commit each,
// all-or-nothing; the pasted clips become the set and the playhead moves to their end.
impl EditorState {
    /// SR854's paste: the clipboard laid at the playhead, never overwriting (`paste_as(None)`).
    pub fn paste(&mut self) -> Result<(), EditError> {
        self.paste_as(None)
    }
    /// The `paste-insert` / `paste-over` words of [`EditorState::verb`]; `None` for any other word.
    fn paste_mode_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        Some(match v {
            "paste-insert" => self.paste_as(Some(crate::Mode::Insert)),
            "paste-over" => self.paste_as(Some(crate::Mode::Overwrite)),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod striapasteinsert_tests {
    use super::*;
    use crate::{Kind, SourceId};

    /// SR854's fixture: A 13..23 at 0, B 0..10 at 10, A 23..33 at 25 on V1 and on A1.
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
    fn striapasteinsert_paste_insert_ripples_the_track_one_undo() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        let (a, b) = (SourceId(0), SourceId(1));
        st.select(0, 1).unwrap();
        st.verb("copy").unwrap().unwrap();
        st.seek(12);
        assert_eq!(st.verb("paste-insert"), Some(Ok(())));
        assert_eq!(st.timeline.tracks[0].clips, alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 2, 10), Clip::new(b, 0, 10, 12), Clip::new(b, 2, 10, 22), Clip::new(a, 23, 33, 35)]);
        assert_eq!(st.timeline.tracks[1], tl0.tracks[1], "only the selected track ripples");
        assert_eq!((st.selection, st.set.clone(), st.playhead, st.undo_depth()), (Some((0, 2)), alloc::vec![(0, 2)], 22, 1));
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.timeline, tl0, "one undo per paste-insert");
    }

    #[test]
    fn striapasteinsert_paste_over_carves_one_undo() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        let (a, b) = (SourceId(0), SourceId(1));
        st.select(0, 1).unwrap();
        st.verb("copy").unwrap().unwrap();
        st.seek(12);
        assert_eq!(st.verb("paste"), Some(Err(EditError::Overlap)), "the plain paste still never overwrites");
        assert_eq!(st.verb("paste-over"), Some(Ok(())));
        assert_eq!(st.timeline.tracks[0].clips, alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 2, 10), Clip::new(b, 0, 10, 12), Clip::new(a, 23, 33, 25)]);
        assert_eq!(st.timeline.tracks[1], tl0.tracks[1]);
        assert_eq!((st.selection, st.playhead, st.undo_depth()), (Some((0, 2)), 22, 1));
        assert_eq!(st.verb("undo"), Some(Ok(())));
        assert_eq!(st.timeline, tl0);
    }

    #[test]
    fn striapasteinsert_contiguous_set_inserts_contiguously() {
        let mut st = state();
        let (a, b) = (SourceId(0), SourceId(1));
        st.select_many(&[(0, 1), (0, 0)], false);
        st.verb("copy").unwrap().unwrap();
        st.seek(25);
        st.select(0, 2).unwrap();
        assert_eq!(st.verb("paste-insert"), Some(Ok(())));
        assert_eq!(st.timeline.tracks[0].clips, alloc::vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 13, 23, 25), Clip::new(b, 0, 10, 35), Clip::new(a, 23, 33, 45)]);
        assert_eq!((st.set.clone(), st.playhead, st.undo_depth()), (alloc::vec![(0, 2), (0, 3)], 45, 1));
    }

    #[test]
    fn striapasteinsert_refusals_by_name_leave_the_state() {
        let mut st = state();
        assert_eq!(st.verb("paste-insert"), Some(Err(EditError::NoClip)), "empty clipboard");
        assert_eq!(st.verb("paste-over"), Some(Err(EditError::NoClip)));
        st.select(0, 1).unwrap();
        st.verb("copy").unwrap().unwrap();
        let tl0 = st.timeline.clone();
        st.seek(35);
        st.select(1, 0).unwrap();
        assert_eq!(st.verb("paste-insert"), Some(Err(EditError::Kind)));
        assert_eq!(st.verb("paste-over"), Some(Err(EditError::Kind)));
        assert_eq!((st.timeline == tl0, st.undo_depth()), (true, 0));
        assert_eq!(st.verb("paste-under"), None);
    }
}

// ---- STRIAGRADEFACE (LEDGER SR899): the grade step words (named tail block) ----
// `bright-` / `bright+`, `contrast-` / `contrast+`, `sat-` / `sat+` step the selected clip's brightness / contrast /
// saturation by [`GRADE_STEP`] (1/1000) through SR869's `EditorState::set_grade` (ONE commit per press). Refused by name:
// nothing selected `EditError::NoClip`; a step past -1000..=1000 `EditError::Grade` (validate; not clamped); an audio
// clip `EditError::Kind`. The face's buttons fire these words (stria `nle_face::VERBS`).

/// The step of one grade word, in 1/1000.
pub const GRADE_STEP: i16 = 50;

impl EditorState {
    /// The grade step words of [`EditorState::verb`]; `None` when `v` is not one.
    fn grade_step_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let (field, d) = match v {
            "bright-" => (0, -GRADE_STEP),
            "bright+" => (0, GRADE_STEP),
            "contrast-" => (1, -GRADE_STEP),
            "contrast+" => (1, GRADE_STEP),
            "sat-" => (2, -GRADE_STEP),
            "sat+" => (2, GRADE_STEP),
            _ => return None,
        };
        let Some(c) = self.selected() else { return Some(Err(EditError::NoClip)) };
        let mut g = c.grade;
        let f = match field {
            0 => &mut g.brightness,
            1 => &mut g.contrast,
            _ => &mut g.saturation,
        };
        *f = f.saturating_add(d);
        Some(self.set_grade(g))
    }
}

// ---- STRIAAUTOSAVE (LEDGER SR883): a whole timeline restored through the one commit (named tail block) ----
// The host face's `recover` word applies the document's autosave (`<stem>.autosave.edl`, parsed by `edl::parse`) as ONE
// edit: the current timeline goes on the undo stack, so `undo` returns to the document as opened.

impl EditorState {
    /// Replace the timeline with `tl` through [`EditorState::commit`] (one undo step); false, and nothing pushed, when
    /// `tl` is the current timeline.
    pub fn restore(&mut self, tl: Timeline) -> bool {
        if tl == self.timeline {
            return false;
        }
        self.commit(tl);
        true
    }
}

// ---- STRIAUNDOHISTORY (LEDGER SR876): op names on the undo entries and the history line (named tail block) ----
// The undo stack holds whole-timeline snapshots and no op name; [`UndoNames`] is its parallel stack of words, moved at the
// stack's three mutation sites (`commit` pushes, `undo` / `redo` move the top entry across), so it stays in lockstep through
// the UNDO_MAX cap and through every commit road. The word is known in ONE place, a verb call: the public
// [`EditorState::verb`] arms it, runs the dispatch (`verb_unnamed`), disarms; `commit` takes the armed word (a second
// commit in one call is unnamed). The OUTERMOST armed word wins (a face word that calls this dispatch keeps its own word).
// A commit outside any verb (a drag's `Drag::finish`, a direct method call) is an unnamed step. Bounded: at most UNDO_MAX
// entries across both stacks, each word clipped to [`WORD_MAX`] bytes on a char boundary.

/// How many entries the history line lists (newest first).
pub const HISTORY_N: usize = 10;
/// The longest word kept on an undo entry (bytes; clipped on a char boundary).
pub const WORD_MAX: usize = 24;

/// The words of the undo and redo entries (`None`: an unnamed step), and the word armed by the running verb call.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UndoNames {
    undo: Vec<Option<alloc::string::String>>,
    redo: Vec<Option<alloc::string::String>>,
    word: Option<alloc::string::String>,
}

impl UndoNames {
    /// `commit` pushed a snapshot: the armed word (taken) names it; the same cap; the redo words go with the redo stack.
    fn pushed(&mut self) {
        let w = self.word.take();
        self.undo.push(w);
        if self.undo.len() > UNDO_MAX {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    /// `undo` moved the top snapshot to the redo stack.
    fn undone(&mut self) {
        let w = self.undo.pop().flatten();
        self.redo.push(w);
    }
    /// `redo` moved the top snapshot back to the undo stack.
    fn redone(&mut self) {
        let w = self.redo.pop().flatten();
        self.undo.push(w);
    }
}

/// `v` clipped to [`WORD_MAX`] bytes on a char boundary.
fn clip_word(v: &str) -> alloc::string::String {
    let mut n = v.len().min(WORD_MAX);
    while !v.is_char_boundary(n) {
        n -= 1;
    }
    alloc::string::String::from(&v[..n])
}

impl EditorState {
    /// THE verb dispatch both faces call (see `verb_unnamed`), with `v` armed as the name of any undo step it commits.
    pub fn verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let armed = self.arm_word(v);
        let r = self.verb_unnamed(v);
        self.disarm_word(armed);
        r
    }
    /// Arm `v` as the next commit's name; false (nothing armed) when a word is already armed (the outer word wins).
    pub fn arm_word(&mut self, v: &str) -> bool {
        if self.hist.word.is_some() {
            return false;
        }
        self.hist.word = Some(clip_word(v));
        true
    }
    /// Drop the armed word when this caller armed it ([`EditorState::arm_word`]'s result).
    pub fn disarm_word(&mut self, armed: bool) {
        if armed {
            self.hist.word = None;
        }
    }
    /// The top `n` undo entries, newest first: `(stack position, oldest kept = 1; its word, None when unnamed)`.
    pub fn history(&self, n: usize) -> Vec<(usize, Option<&str>)> {
        let depth = self.undo.len();
        let names = &self.hist.undo;
        (0..n.min(depth))
            .map(|k| (depth - k, names.len().checked_sub(k + 1).and_then(|j| names[j].as_deref())))
            .collect()
    }
    /// The message line: `history: 3 split | 2 (unnamed) | 1 mark in` (the top `n`, newest first), or
    /// `history: nothing to undo`.
    pub fn history_line(&self, n: usize) -> alloc::string::String {
        let h = self.history(n);
        if h.is_empty() {
            return alloc::string::String::from("history: nothing to undo");
        }
        let items: Vec<alloc::string::String> = h.iter().map(|&(i, w)| alloc::format!("{i} {}", w.unwrap_or("(unnamed)"))).collect();
        alloc::format!("history: {}", items.join(" | "))
    }
}

// ---- STRIAFREEZEFRAME (LEDGER SR880): `freeze` holds the playhead's frame for FREEZE_SECS as ONE commit (named tail block) ----
impl EditorState {
    /// Hold the frame the preview shows at the playhead for `ticks` ticks through `Timeline::freeze` (a rated one-tick clip
    /// laid by `place` Insert; STRIAFREEZELEN (SR916): `all` lays it through `Timeline::freeze_all`, every other track
    /// rippled by the same length): ONE commit, so undo restores the whole pre-op timeline; the hold is selected. The op's
    /// refusals (`Empty`, `NoClip`, `Rate`, `Hold`, `Transition`, ...) leave the state.
    pub fn freeze(&mut self, ticks: i64, all: bool) -> Result<(), EditError> {
        let ph = self.playhead;
        let (tl, track) = if all { self.timeline.freeze_all(ph, ticks)? } else { self.timeline.freeze(ph, ticks)? };
        self.commit(tl);
        let _ = self.select_at(track, ph);
        Ok(())
    }
    /// STRIAFREEZEFRAME: the `freeze` word (`None` for any other word); STRIAFREEZELEN (SR916): and `freeze-all`, each with an
    /// optional length ([`freeze_words`]).
    fn freeze_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        freeze_words(v, self.timeline.fps()).map(|w| w.and_then(|(n, all)| self.freeze(n, all)))
    }
}

// ---- STRIAPANFACE (LEDGER SR897): the face's pan words — centre and a step of 1/8 on the eighth grid (named tail block) ----
// `pan` centres the selected clip; `pan-` / `pan+` step it one eighth of the swing left / right on the grid
// `round(k * 127 / 8)`, k in -8..=8 (0, 16, 32, 48, 64, 79, 95, 111, 127: the values `pan 0.125` ... `pan 1` give). An
// off-grid pan steps from its nearest grid point. At a hard end the step repeats the value (Ok, no commit). All ride
// SR861's `set_selected_pan` and its ONE `commit`; with nothing selected it refuses `EditError::NoClip`.
impl EditorState {
    /// The pan the word `v` sets (`pan`, `pan-`, `pan+`); `None` for any other word.
    fn pan_step(&self, v: &str) -> Option<i8> {
        let d: i32 = match v {
            "pan" => return Some(0),
            "pan-" => -1,
            "pan+" => 1,
            _ => return None,
        };
        let cur = self.selected().map_or(0, |c| c.pan_cq8) as i32;
        let m = cur * 8;
        let k = ((m + m.signum() * 63) / 127 + d).clamp(-8, 8);
        let n = k * 127;
        Some(((n + n.signum() * 4) / 8) as i8)
    }
}

// ---- STRIACROPFACE (LEDGER SR900): the crop words and the crop over the whole selection set (named tail block) ----
// SR867's crop reaches the face: bare `crop` clears it, `crop-l+` / `crop-l-` (and `t`, `r`, `b`) step one side by
// [`CROP_STEP`] (1/1000 of the frame; a minus stops at 0). Every crop word, and `crop <l> <t> <r> <b>` on a set of more than
// one clip (through [`EditorState::set_crop`]), acts on the whole selection SET by [`EditorState::crop_set`], the shape of
// SR853's `gain_set`: `Timeline::set_crop` (its own refusals) folded over every VIDEO clip of `set` into ONE `commit`; the
// set's audio clips are skipped (a crop has no audio meaning), a set with no video clip is `EditError::Kind`, any clip
// refused refuses the whole edit and leaves the state, and an edit changing no clip commits nothing.
/// STRIACROPFACE (SR900): one press of a `crop-<side>±` word, 1/1000 of the frame.
pub const CROP_STEP: u16 = 10;

impl EditorState {
    /// Every selected video clip's crop set to `crop(its own crop)` (ONE commit; refused whole; no change commits nothing;
    /// audio clips of the set skipped; no video clip in the set `EditError::Kind`).
    pub fn crop_set(&mut self, crop: impl Fn(crate::Crop) -> crate::Crop) -> Result<(), EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        let mut tl = self.timeline.clone();
        let (mut video, mut changed) = (false, false);
        for &(t, i) in &self.set {
            if tl.tracks.get(t).ok_or(EditError::NoTrack)?.kind != crate::Kind::Video {
                continue;
            }
            video = true;
            let c = tl.tracks[t].clips.get(i).copied().ok_or(EditError::NoClip)?;
            let k = crop(c.crop);
            if k != c.crop {
                tl = tl.set_crop(t, i, k)?;
                changed = true;
            }
        }
        if !video {
            return Err(EditError::Kind);
        }
        if changed {
            self.commit(tl);
        }
        Ok(())
    }
    /// The face's crop words of [`EditorState::verb`] (bare `crop`, `crop-[ltrb][+-]`); `None` when `v` is none of them.
    fn crop_face_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if v == "crop" {
            return Some(self.crop_set(|_| crate::Crop::NONE));
        }
        let (side, plus) = match v.strip_prefix("crop-")?.as_bytes() {
            [s @ (b'l' | b't' | b'r' | b'b'), g @ (b'+' | b'-')] => (*s, *g == b'+'),
            _ => return None,
        };
        Some(self.crop_set(|c| crop_step(c, side, plus)))
    }
}

/// STRIACROPFACE: `c` with side `side` (`b'l'`, `b't'`, `b'r'`, `b'b'`) one [`CROP_STEP`] wider (`plus`) or narrower (stops
/// at 0); a step keeping no picture is refused by `Timeline::set_crop`.
fn crop_step(c: crate::Crop, side: u8, plus: bool) -> crate::Crop {
    let f = |x: u16| if plus { x.saturating_add(CROP_STEP) } else { x.saturating_sub(CROP_STEP) };
    let mut c = c;
    match side {
        b'l' => c.l = f(c.l),
        b't' => c.t = f(c.t),
        b'r' => c.r = f(c.r),
        _ => c.b = f(c.b),
    }
    c
}

#[cfg(test)]
mod striacropface_tests {
    use super::*;
    use crate::{Crop, Kind};

    /// V1: A 0..10 at 0, B 0..10 at 10, C 0..10 at 20; A1: A 0..20 at 0. The set: V1's A (primary), V1's C and A1's clip.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 10, 20, 20)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.select_add(0, 2).unwrap();
        st.select_add(1, 0).unwrap();
        st
    }
    fn crops(st: &EditorState) -> [Crop; 3] {
        let c = |i: usize| st.timeline.tracks[0].clips[i].crop;
        [c(0), c(1), c(2)]
    }

    #[test]
    fn striacropface_steps_and_clear_over_the_set_one_commit() {
        let mut st = state();
        let l = Crop { l: CROP_STEP, t: 0, r: 0, b: 0 };
        assert_eq!(st.verb("crop-l+"), Some(Ok(())));
        assert_eq!(crops(&st), [l, Crop::NONE, l], "every selected video clip; the unselected B stays");
        assert_eq!(st.timeline.tracks[1].clips[0].crop, Crop::NONE, "the audio clip skipped");
        assert_eq!(st.undo_depth(), 1, "ONE commit for the whole set");
        assert_eq!((st.selection, st.set.clone()), (Some((0, 0)), alloc::vec![(0, 0), (0, 2), (1, 0)]));
        assert_eq!(st.verb("crop-l-"), Some(Ok(())));
        assert_eq!(st.verb("crop-l-"), Some(Ok(())));
        assert_eq!((crops(&st)[0], st.undo_depth()), (Crop::NONE, 2), "a minus stops at 0 and commits nothing more");
        assert_eq!(st.verb("crop-b+"), Some(Ok(())));
        assert_eq!(crops(&st)[2], Crop { l: 0, t: 0, r: 0, b: CROP_STEP });
        assert_eq!(st.verb("crop"), Some(Ok(())));
        assert_eq!((crops(&st), st.undo_depth()), ([Crop::NONE; 3], 4));
        assert_eq!(st.verb("crop"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 4, "already clear: no step");
        assert!(st.undo());
        assert_eq!(crops(&st)[0], Crop { l: 0, t: 0, r: 0, b: CROP_STEP });
        assert_eq!(st.verb("crop-x+"), None);
        assert_eq!(st.verb("crop-l"), None);
        st.deselect();
        assert_eq!(st.verb("crop"), Some(Err(EditError::NoClip)));
        assert_eq!(st.verb("crop-t+"), Some(Err(EditError::NoClip)));
    }

    #[test]
    fn striacropface_set_crop_refusals_refuse_the_whole_set() {
        let mut st = state();
        assert_eq!(st.verb("crop 250 0 250 0"), Some(Ok(())));
        let c = Crop { l: 250, t: 0, r: 250, b: 0 };
        assert_eq!((crops(&st), st.undo_depth()), ([c, Crop::NONE, c], 1), "`crop l t r b` over the set");
        assert_eq!(st.verb("crop 600 0 400 0"), Some(Err(EditError::Crop)));
        let before = st.timeline.clone();
        st.timeline = st.timeline.set_crop(0, 2, Crop { l: 250, t: 0, r: 745, b: 0 }).unwrap();
        let before2 = st.timeline.clone();
        assert_eq!(st.verb("crop-r+"), Some(Err(EditError::Crop)), "C would keep no picture");
        assert_eq!(st.timeline, before2, "refused whole: A did not step either");
        st.timeline = before;
        st.select(1, 0).unwrap();
        assert_eq!(st.verb("crop"), Some(Err(EditError::Kind)), "an audio-only set");
        assert_eq!(st.undo_depth(), 1);
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
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
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
        let Some(cur) = self.selected().map(|c| c.xform) else { return Some(Err(EditError::NoClip)) };
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

// ---- STRIAFADESET (LEDGER SR906): fades over the whole selection (named tail block) ----
// SR838's `fade-in` / `fade-out` words act on every clip of NLE2c's `set`, in the shape of SR853's `gain_set`:
// [`EditorState::fades_set`] folds the existing per-clip `Timeline::set_fades` (its own `validate`) over the set and lands
// ONE `commit`; any clip refused (`EditError::Fade`) refuses the whole edit. The bare forms are playhead-relative PER CLIP:
// a clip the playhead is on (`at <= playhead <= end`) fades to it, a clip the playhead is not on is SKIPPED, and a word
// with the playhead on NO clip of the set is refused `EditError::Fade` (so a one-clip set behaves as SR838's did).
impl EditorState {
    /// Every selected clip's fades set to `fades(the clip)` (`None` skips that clip). ONE commit; refused whole; no clip
    /// taken refuses `EditError::Fade`; no change commits nothing.
    pub fn fades_set(&mut self, fades: impl Fn(Clip) -> Option<(i64, i64)>) -> Result<(), EditError> {
        self.selection.ok_or(EditError::NoClip)?;
        let mut tl = self.timeline.clone();
        let (mut any, mut changed) = (false, false);
        for &(t, i) in &self.set {
            let c = tl.tracks.get(t).and_then(|tr| tr.clips.get(i)).copied().ok_or(EditError::NoClip)?;
            let Some((fi, fo)) = fades(c) else { continue };
            any = true;
            if (fi, fo) != (c.fade_in, c.fade_out) {
                tl = tl.set_fades(t, i, fi, fo)?;
                changed = true;
            }
        }
        if !any {
            return Err(EditError::Fade);
        }
        if changed {
            self.commit(tl);
        }
        Ok(())
    }
}

#[cfg(test)]
mod striafadeset_tests {
    use super::*;
    use crate::Kind;

    /// V1: A 0..10 at 0, B 0..10 at 10; A1: A 0..20 at 0. The set: V1's B (primary, 10 ticks at 10) and A1's clip (20 at 0).
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        let mut st = EditorState::new(tl);
        st.select(1, 0).unwrap();
        st.select_add(0, 1).unwrap();
        st
    }
    fn fades(st: &EditorState) -> [(i64, i64); 3] {
        let f = |t: usize, i: usize| (st.timeline.tracks[t].clips[i].fade_in, st.timeline.tracks[t].clips[i].fade_out);
        [f(0, 0), f(0, 1), f(1, 0)]
    }

    #[test]
    fn striafadeset_numeric_fades_every_clip_of_the_set_one_commit() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("fade-in 4"), Some(Ok(())));
        assert_eq!(fades(&st), [(0, 0), (4, 0), (4, 0)], "both selected clips; the unselected clip stays");
        assert_eq!(st.undo_depth(), 1, "ONE commit for the whole set");
        assert_eq!(st.verb("fade-out 3"), Some(Ok(())));
        assert_eq!(fades(&st), [(0, 0), (4, 3), (4, 3)], "each keeps its own fade-in");
        assert_eq!(st.undo_depth(), 2);
        assert_eq!(st.verb("fade-out 3"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 2, "the same fades commit nothing");
        assert_eq!((st.selection, st.set.clone()), (Some((0, 1)), alloc::vec![(0, 1), (1, 0)]));
        assert_eq!(edl::parse(&edl::write(&st.timeline, "T").unwrap(), 25).unwrap(), st.timeline, "the EDL carries every fade");
        assert!(st.undo() && st.undo());
        assert_eq!(st.timeline, tl0, "one undo per word");
    }

    #[test]
    fn striafadeset_numeric_fade_refused_whole() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("fade-in 15"), Some(Err(EditError::Fade)), "longer than B (10 ticks), not than A1's clip (20)");
        assert_eq!((&st.timeline, st.undo_depth()), (&tl0, 0), "nothing of the set changed");
    }

    #[test]
    fn striafadeset_bare_fade_skips_clips_off_the_playhead() {
        let mut st = state();
        st.seek(15); // on B [10,20] and on A1's clip [0,20]
        assert_eq!(st.verb("fade-in"), Some(Ok(())));
        assert_eq!(fades(&st), [(0, 0), (5, 0), (15, 0)], "each clip fades to the playhead from its own start");
        assert_eq!(st.undo_depth(), 1);
        st.seek(5); // on A1's clip only: B is skipped
        assert_eq!(st.verb("fade-out"), Some(Err(EditError::Fade)), "A1's clip would be 15 + 15 > 20: refused whole");
        assert_eq!((fades(&st), st.undo_depth()), ([(0, 0), (5, 0), (15, 0)], 1));
        assert_eq!(st.verb("fade-in 0"), Some(Ok(())));
        assert_eq!(st.verb("fade-out"), Some(Ok(())));
        assert_eq!(fades(&st), [(0, 0), (0, 0), (0, 15)], "B, off the playhead, skipped");
        assert_eq!(st.undo_depth(), 3);
        st.select(0, 0).unwrap(); // the set is V1's A [0,10] alone
        st.seek(15); // on no clip of the set
        assert_eq!(st.verb("fade-in"), Some(Err(EditError::Fade)), "a one-clip set off the playhead: refused, as SR838's");
        assert_eq!(st.undo_depth(), 3, "refused: no commit");
        st.deselect();
        assert_eq!(st.verb("fade-in"), Some(Err(EditError::NoClip)));
        assert_eq!(st.fades_set(|k| Some((k.fade_in, k.fade_out))), Err(EditError::NoClip));
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
            Jump::Marker => self.timeline.markers.iter().map(|m| m.at).collect(),
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
        let (track, _) = self.selection.ok_or(EditError::NoClip)?;
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
            "insert" | "overwrite" => return Some(Err(EditError::InsertForm)),
            _ => return None,
        };
        Some(if w.next().is_none() { self.place_marked(mode) } else { Err(EditError::InsertForm) })
    }
}

#[cfg(test)]
mod striainsertmarks_tests {
    use alloc::string::ToString;
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

// ---- STRIAXFADECURVE (LEDGER SR904): the crossfade curve words and the audio refusal text (named tail block) ----
// SR860's `xfade` put only the equal-power curve and its refusals read `EditError`'s video-transition text. `xfade
// linear|equal` and `xfade <n> linear|equal` ride the same `toggle_audio_xfade` (its `curve` parameter): a curve word
// puts that curve, keeping an existing record's length (else the parent's default length), and never takes one off.
// `xfade_refusal` words an `xfade` refusal from the UNCHANGED state (a refusal commits nothing); the host face's one
// error line reads it first, no new `EditError` variant.
impl EditorState {
    /// `xfade [<n>] <curve>`: `n` the optional tick count word, `c` the curve word, `extra` a third word (refused).
    fn xfade_curve_word(&mut self, n: Option<&str>, c: &str, extra: Option<&str>) -> Result<(), EditError> {
        if extra.is_some() {
            return Err(EditError::Transition);
        }
        let curve = match c {
            "linear" => crate::Curve::Linear,
            "equal" => crate::Curve::EqualPower,
            _ => return Err(EditError::Transition),
        };
        let dur = match n {
            Some(n) => Some(n.parse::<i64>().map_err(|_| EditError::Transition)?),
            None => None,
        };
        self.toggle_audio_xfade(dur, Some(curve))
    }
    /// The honest text of a refused `xfade` word `v` (`e` its error), read against the state the refusal left alone;
    /// `None` when `v` is not an `xfade` word or `e` has no audio wording (the caller shows `e`'s own text).
    pub fn xfade_refusal(&self, v: &str, e: EditError) -> Option<&'static str> {
        if v.split_ascii_whitespace().next() != Some("xfade") {
            return None;
        }
        match e {
            EditError::NoClip if self.selection.is_none() => Some("no clip selected (select the incoming audio clip)"),
            EditError::Transition => {
                let (track, ci) = self.selection?;
                let t = self.timeline.tracks.get(track)?;
                let i = t.clips.get(ci)?;
                Some(if t.kind != crate::Kind::Audio {
                    "the selected clip is not on an audio track"
                } else if !t.clips.iter().any(|c| c.end() == i.at) {
                    "no clip ends where the selected clip starts"
                } else {
                    "usage: xfade [<ticks>] [linear|equal], ticks at least 1"
                })
            }
            EditError::Handle => Some("the crossfade is longer than the incoming clip or the outgoing clip's handle"),
            _ => None,
        }
    }
}

// ---- STRIATRACKGAINSTEP (LEDGER SR901): `track-gain-` / `track-gain+` step the selected clip's track by one dB (named tail block) ----
// The step words ride THE verb dispatch: each is SR864's `set_selected_track_gain` of the track's gain one
// [`GAIN_STEP_CDB`] down / up (SR838's step), so it is ONE commit, NoTrack with nothing selected, and a step past the
// clip range is refused by `validate`'s `EditError::Gain` with the state kept. An extra word is refused `EditError::Gain`.
impl EditorState {
    /// The `track-gain-` / `track-gain+` words of [`EditorState::verb`]; `None` for any other word.
    fn track_gain_step_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        let mut w = v.split_ascii_whitespace();
        let up = match w.next()? {
            "track-gain-" => false,
            "track-gain+" => true,
            _ => return None,
        };
        let Some(g) = self.selection.and_then(|(t, _)| self.timeline.tracks.get(t)).map(|t| t.gain_cdb) else {
            return Some(Err(EditError::NoTrack));
        };
        if w.next().is_some() {
            return Some(Err(EditError::Gain));
        }
        Some(self.set_selected_track_gain(if up { g.saturating_add(GAIN_STEP_CDB) } else { g.saturating_sub(GAIN_STEP_CDB) }))
    }
}

#[cfg(test)]
mod striatrackgainstep_tests {
    use super::*;
    use crate::Kind;

    /// A1: A 0..20 at 0, A2: A 0..20 at 0; the clip on A2 selected.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        for _ in 0..2 {
            let t = tl.add_track(Kind::Audio);
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        }
        let mut st = EditorState::new(tl);
        st.select(1, 0).unwrap();
        st
    }

    /// M3: each step moves the selected clip's track one dB in ONE undo step (not the other track, not the clip); undo
    /// walks them back.
    #[test]
    fn striatrackgainstep_steps_one_db_each() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("track-gain-"), Some(Ok(())));
        assert_eq!(st.verb("track-gain-"), Some(Ok(())));
        assert_eq!((st.timeline.tracks[1].gain_cdb, st.timeline.tracks[0].gain_cdb, st.timeline.tracks[1].clips[0].gain_cdb, st.undo_depth()), (-200, 0, 0, 2));
        assert_eq!(st.verb("track-gain+"), Some(Ok(())));
        assert_eq!((st.timeline.tracks[1].gain_cdb, st.undo_depth()), (-100, 3));
        st.undo();
        st.undo();
        st.undo();
        assert_eq!(st.timeline, tl0);
    }

    /// M3: each refusal by name, the state and the undo depth untouched; a step past the clip range is `Gain`.
    #[test]
    fn striatrackgainstep_refusals() {
        let mut none = state();
        none.selection = None;
        let tl0 = none.timeline.clone();
        assert_eq!(none.verb("track-gain-"), Some(Err(EditError::NoTrack)));
        assert_eq!(none.verb("track-gain+"), Some(Err(EditError::NoTrack)));
        let mut st = state();
        assert_eq!(st.verb("track-gain+ 3"), Some(Err(EditError::Gain)));
        assert_eq!((&st.timeline, st.undo_depth(), &none.timeline), (&tl0, 0, &tl0));
        assert_eq!(st.verb("track-gain 24"), Some(Ok(())));
        let top = st.timeline.clone();
        assert_eq!(st.verb("track-gain+"), Some(Err(EditError::Gain)));
        assert_eq!((&st.timeline, st.undo_depth()), (&top, 1));
        assert_eq!(st.verb("track-gain-x"), None, "not this word");
    }
}

// ---- STRIALINKAUTO (LEDGER SR911): the `unlink-all` word (named tail block) ----
// Every linked clip of the timeline gets group 0 through SR863's `Timeline::set_group`, ONE commit through THE verb
// dispatch (`link_verb`'s `unlink-all` arm); no linked clip commits nothing; no selection is needed and none moves.
impl EditorState {
    /// Unlink every clip in the timeline (one commit; none linked commits nothing).
    pub fn unlink_all(&mut self) -> Result<(), EditError> {
        let linked: Vec<(usize, usize)> = self.timeline.tracks.iter().enumerate().flat_map(|(t, tr)| tr.clips.iter().enumerate().filter(|(_, c)| c.group != 0).map(move |(i, _)| (t, i))).collect();
        if linked.is_empty() {
            return Ok(());
        }
        let tl = self.timeline.set_group(&linked, 0)?;
        self.commit(tl);
        Ok(())
    }
}

// ---- STRIAAUTOSAVECAP (LEDGER SR913): a monotonic count of timeline changes (named tail block) ----
// `undo_depth()` stops moving at UNDO_MAX (each commit pushes one and drops the oldest), so a reader counting edits by it
// goes blind there. `commits` moves by one at every `commit`, `undo` and `redo` (the three roads that change the timeline)
// and never falls back; a reader keeps a mark and takes `commits().wrapping_sub(mark)`.

impl EditorState {
    /// Timeline changes since this state was built (`commit`, `undo`, `redo`; refused edits count nothing).
    pub fn commits(&self) -> usize {
        self.commits
    }
}

// ---- STRIATRACKORDERKEYS (LEDGER SR922): the message row's line for a track move (named tail block) ----
// Read from the state AFTER `track-up` / `track-down` committed: the moved track is the selection's (the selection follows
// its clip), and the track it passed is its nearest same-kind neighbour the other way (nothing of their kind lies between
// them, so that is the moved track's old slot). The line names both as they were BEFORE the move: `track-up: V2 above V1`,
// `track-down: V1 below V2`. A pure read; no state, no undo step.
impl EditorState {
    /// STRIATRACKORDERKEYS: `track-up: V2 above V1` / `track-down: V1 below V2` after a move (`None`: nothing selected).
    pub fn track_move_line(&self, up: bool) -> Option<alloc::string::String> {
        let (n, _) = self.selection?;
        let m = self.timeline.track_neighbour(n, !up).ok()?;
        let (word, rel) = if up { ("track-up", "above") } else { ("track-down", "below") };
        Some(alloc::format!("{word}: {} {rel} {}", self.timeline.track_name(m)?, self.timeline.track_name(n)?))
    }
}

// ---- STRIAFREEZELEN (LEDGER SR916): the `freeze` / `freeze-all` words and their length (named tail block) ----
/// STRIAFREEZELEN: a `freeze [<len>]` or `freeze-all [<len>]` word read at `fps` (the timeline's `Timeline::fps`) as `(ticks,
/// all)`; `None`: not this word. No length is `FREEZE_SECS`; `<len>` is read by [`hold_ticks`]. Refused by name: a time base
/// that is not `1/fps` `Rate`; more words, or a length neither reader reads, `Hold`. A 0-tick length is left to the op (`Empty`).
pub fn freeze_words(v: &str, fps: Option<u32>) -> Option<Result<(i64, bool), EditError>> {
    let mut w = v.split_ascii_whitespace();
    let all = match w.next()? {
        "freeze" => false,
        "freeze-all" => true,
        _ => return None,
    };
    let Some(fps) = fps else { return Some(Err(EditError::Rate)) };
    let len = w.next();
    if w.next().is_some() {
        return Some(Err(EditError::Hold));
    }
    Some(match len {
        None => Ok((crate::FREEZE_SECS * fps as i64, all)),
        Some(s) => hold_ticks(s, fps).map(|n| (n, all)).ok_or(EditError::Hold),
    })
}

/// STRIAFREEZELEN: a hold length in ticks at `fps`: with a `:` the EDL's timecode `HH:MM:SS:FF` (`edl::parse_tc`), else decimal
/// seconds (THE decimal reader `speed_q16` with `per` = 1) rounded half up to a whole tick. `None`: neither reads it.
pub fn hold_ticks(s: &str, fps: u32) -> Option<i64> {
    if s.contains(':') {
        return edl::parse_tc(s, fps, 0).ok();
    }
    let q = crate::speed_q16(s, 1)?;
    Some(((q as i128 * fps as i128 + 0x8000) >> 16) as i64)
}

// ---- STRIAXFADECURVEBUTTON (LEDGER SR925): the crossfade curve cycle word and the xfade reading (named tail block) ----
// `xfade-curve` (the face's `C` key and `Curve` button) swaps the curve of the crossfade on the selected clip's head cut
// (explicit or derived, `crossfade_at` with a clip partner): equal-power -> linear, linear -> equal, at the same length,
// through SR904's `xfade_curve_word` (ONE commit). No crossfade there: `Transition`, worded by `xfade_curve_refusal`
// (every other refusal reads SR904's `xfade_refusal` text). `xfade_reading` is what the face's message row shows after
// any xfade word succeeds: `<ticks> <linear|equal>` of the head cut's explicit record, `off` when it has none.
impl EditorState {
    /// The `xfade-curve` word of [`EditorState::verb`]; `None` when `v` is not it.
    fn xfade_cycle_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        if v != "xfade-curve" {
            return None;
        }
        let Some((track, ci)) = self.selection else { return Some(Err(EditError::NoClip)) };
        Some(match self.timeline.crossfade_at(track, ci) {
            Some(x) if x.partner == crate::Partner::Clip => {
                let next = if x.curve == crate::Curve::EqualPower { "linear" } else { "equal" };
                self.xfade_curve_word(None, next, None)
            }
            _ => Err(EditError::Transition),
        })
    }
    /// The honest text of a refused `xfade-curve` (`e` its error), read against the unchanged state: no crossfade on a
    /// cut that has one to take, else SR904's [`EditorState::xfade_refusal`] text; `None` for any other word.
    pub fn xfade_curve_refusal(&self, v: &str, e: EditError) -> Option<&'static str> {
        if v != "xfade-curve" {
            return None;
        }
        if e == EditError::Transition {
            if let Some((track, ci)) = self.selection {
                let t = self.timeline.tracks.get(track)?;
                let i = t.clips.get(ci)?;
                if t.kind == crate::Kind::Audio && t.clips.iter().any(|c| c.end() == i.at) {
                    return Some("no crossfade on the selected clip's head cut (xfade puts one)");
                }
            }
        }
        self.xfade_refusal("xfade", e)
    }
    /// After an xfade word `v` (`xfade ...` or `xfade-curve`): the selected clip's head cut's explicit crossfade as
    /// `<ticks> <linear|equal>`, `off` when it has none; `None` for any other word or no selected clip.
    pub fn xfade_reading(&self, v: &str) -> Option<alloc::string::String> {
        if v.split_ascii_whitespace().next() != Some("xfade") && v != "xfade-curve" {
            return None;
        }
        let (track, ci) = self.selection?;
        let i = self.timeline.tracks.get(track)?.clips.get(ci)?;
        Some(match self.timeline.audio_xfades.iter().find(|x| x.track == track && x.at == i.at) {
            Some(x) => alloc::format!("{} {}", x.dur, if x.curve == crate::Curve::EqualPower { "equal" } else { "linear" }),
            None => alloc::string::String::from("off"),
        })
    }
}

// ---- STRIALINKAUTOFUZZ (LEDGER SR928): the `link-av` word (named tail block) ----
// `Timeline::link_av` (exact pairs, then the near pass) over the current timeline as ONE commit through THE verb
// dispatch (`link_verb`'s `link-av` arm): a document opened from an EDL has no auto-link. Nothing newly linked commits
// nothing; no selection is needed; a selected clip that just got linked brings its mate into the set (as `select`).
impl EditorState {
    /// Link every V1 / A1 pair `Timeline::link_av` finds (one commit; none found commits nothing).
    pub fn link_av_all(&mut self) -> Result<(), EditError> {
        let tl = self.timeline.link_av();
        if tl == self.timeline {
            return Ok(());
        }
        self.commit(tl);
        self.add_mates();
        Ok(())
    }
}

// ---- STRIAGAPSALLTRACKS (LEDGER SR929): `close-gaps all` through the one commit (named tail block) ----
// Every track's gaps close as ONE commit (one undo step) through `Timeline::close_gaps_all`; no selection is needed, a
// refusal by any track refuses the whole word and commits nothing, and no gap anywhere commits nothing.
impl EditorState {
    /// Close every gap on every track (one commit; no gap commits nothing).
    pub fn close_gaps_all(&mut self) -> Result<(), EditError> {
        let tl = self.timeline.close_gaps_all().map_err(|(_, e)| e)?;
        if tl != self.timeline {
            self.commit(tl);
        }
        Ok(())
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
        let w: Vec<&str> = v.split_ascii_whitespace().collect();
        let d = crate::Xform::SCALE_STEP as i32;
        let xf = match w.as_slice() {
            ["pip", c] => crate::Xform::corner(c)?,
            [step @ ("scale-" | "scale+")] => {
                let Some(cur) = self.selected().map(|c| c.xform) else { return Some(Err(EditError::NoClip)) };
                match cur.stepped(if *step == "scale+" { d } else { -d }) {
                    Some(xf) => xf,
                    None => return Some(Err(EditError::Xform)),
                }
            }
            _ => return None,
        };
        Some(self.set_xform(xf))
    }
}
