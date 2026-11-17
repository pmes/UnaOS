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
    pub source: Option<SourceMon>, // STRIASOURCEVIEWER (SR9045): the loaded source monitor, session state (file tail)
}

impl EditorState {
    pub fn new(timeline: Timeline) -> EditorState {
        EditorState { timeline, selection: None, set: Vec::new(), playhead: 0, zoom: ZOOM_DEFAULT, undo: Vec::new(), redo: Vec::new(), transition: None, source: None } // STRIASOURCEVIEWER (SR9045): no source loaded
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
        let ph = self.playhead; if let Some(r) = self.source_verb(v) { return Some(r); } if let Some(r) = self.size_verb(v) { return Some(r); } if let Some(r) = self.look_verb(v) { return Some(r); } if let Some(r) = self.insert_verb(v) { return Some(r); } // NLETRANSITIONFACE (SR691): `kind` / `ease` (file tail); NLESIZEPOLICYUI (SR402): the `size` words (file tail); STRIAINSERTOVERWRITE (SR852): insert / overwrite (file tail); STRIASOURCEVIEWER (SR9045): the source monitor's words (file tail)
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
        self.place_tracks_at_playhead(&[track], src, src_in, src_out, mode) // STRIABINMORE (SR976): the one-track case of the several-track lay (file tail)
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
            None => self.insert_source(mode).unwrap_or_else(|| self.insert_selected(mode)), // STRIASOURCEVIEWER (SR9045): a loaded source's marked range first (file tail)
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

// ---- STRIABINMORE (LEDGER SR976): one source range laid on several tracks in ONE commit (named tail block) ----
// THE placing road: `Timeline::place` on each named track in turn (each track's own Insert ripple / Overwrite hole, the
// same clip on every one, so the tracks stay in sync) and the ONE `commit`, so one undo restores all of them; any refusal
// commits nothing. SR852's `place_at_playhead` is its one-track case; the bin's unused AV reel lays its video and audio.
impl EditorState {
    /// Lay source `src` ticks `src_in..src_out` at the playhead on every track of `tracks` (`Timeline::place` in `mode`
    /// on each, ONE commit); the clip on the first track is selected and the playhead moves to its end. No track:
    /// `EditError::NoTrack`.
    pub fn place_tracks_at_playhead(&mut self, tracks: &[usize], src: crate::SourceId, src_in: i64, src_out: i64, mode: crate::Mode) -> Result<(), EditError> {
        let (&first, rest) = tracks.split_first().ok_or(EditError::NoTrack)?;
        let clip = Clip::new(src, src_in, src_out, self.playhead);
        let mut tl = self.timeline.place(first, clip, mode)?;
        for &t in rest {
            tl = tl.place(t, clip, mode)?;
        }
        self.commit(tl);
        self.select_at(first, clip.at).unwrap_or_else(|_| self.deselect());
        self.seek(clip.end());
        Ok(())
    }
}

#[cfg(test)]
mod striabinmore_tests {
    use super::*;
    use crate::{Kind, Mode, SourceId};

    /// A 13..23 at 0 on V1 and A1; C (50 ticks) on no track.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_source("B", 50);
        tl.add_source("C", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0)];
        }
        EditorState::new(tl)
    }

    #[test]
    fn striabinmore_two_tracks_one_commit_one_undo() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.seek(5);
        let c = Clip::new(SourceId(2), 0, 50, 5);
        assert_eq!(st.place_tracks_at_playhead(&[0, 1], SourceId(2), 0, 50, Mode::Insert), Ok(()));
        let want = tl0.place(0, c, Mode::Insert).unwrap().place(1, c, Mode::Insert).unwrap();
        assert_eq!(st.timeline, want);
        assert_eq!(st.timeline.tracks[0].clips, st.timeline.tracks[1].clips, "both tracks carry the same cut");
        assert_eq!((st.selection, st.selected(), st.playhead, st.undo_depth()), (Some((0, 1)), Some(c), 55, 1));
        assert!(st.undo());
        assert_eq!(st.timeline, tl0, "ONE undo restores both tracks");
    }

    #[test]
    fn striabinmore_one_track_is_place_at_playhead() {
        let mut a = state();
        let mut b = state();
        a.seek(5);
        b.seek(5);
        assert_eq!(a.place_at_playhead(1, SourceId(1), 0, 10, Mode::Overwrite), Ok(()));
        assert_eq!(b.place_tracks_at_playhead(&[1], SourceId(1), 0, 10, Mode::Overwrite), Ok(()));
        assert_eq!((a.timeline.clone(), a.selection, a.playhead), (b.timeline.clone(), b.selection, b.playhead));
    }

    #[test]
    fn striabinmore_refusals_commit_nothing() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        st.seek(5);
        assert_eq!(st.place_tracks_at_playhead(&[], SourceId(2), 0, 50, Mode::Insert), Err(EditError::NoTrack));
        assert_eq!(st.place_tracks_at_playhead(&[0, 7], SourceId(2), 0, 50, Mode::Insert), Err(EditError::NoTrack), "a missing second track");
        assert_eq!(st.place_tracks_at_playhead(&[0, 1], SourceId(2), 0, 51, Mode::Insert), Err(EditError::OutOfSource));
        assert_eq!((st.timeline.clone(), st.undo_depth(), st.playhead), (tl0, 0, 5));
    }
}

// ---- STRIASOURCEVIEWER (LEDGER SR9045): the source monitor, session state, and the three-point edit from it (named tail block) ----
// A source is LOADED into [`EditorState::source`] ([`SourceMon`]: the reel, its own source playhead and its own in / out
// marks, out exclusive like the timeline's): SESSION state, never the document's (no commit, no undo step, no dirty, no
// EDL line; `Timeline::marks` stays the export range). The words ride THE dispatch both faces call: `source <reel>` (at 0),
// bare `source` (the selected clip's reel at the source tick under the playhead: match frame), `unload`, `src-prev` /
// `src-next` (one tick, clamped to the reel), `src-in` / `src-out` (at the source playhead, keeping the other; in not before
// out: `EditError::Marks`; nothing loaded: `EditError::NoSource`). With a source loaded, bare `insert` / `overwrite` lay its
// marked range (in or 0 .. out or the duration) at the TIMELINE playhead through SR976's `place_tracks_at_playhead` (ONE
// commit): on each of its kinds' first track holding the reel, else that kind's first track. The source stays loaded.
// The host face previews it through its one frame road over [`EditorState::source_view`] (a one-clip timeline).

/// A loaded source: the reel, the source playhead (`0..duration`), its own in / out marks and the track kinds an insert
/// lays it on.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMon {
    pub src: crate::SourceId,
    pub at: i64,
    pub marks: (Option<i64>, Option<i64>),
    pub kinds: Vec<crate::Kind>,
}

impl EditorState {
    /// Load source `src` at its tick 0, no marks. `kinds`: the kinds an insert lays it on (empty: the kinds of the tracks
    /// its clips sit on). Unknown `src`: `EditError::NoSource`. Pushes nothing.
    pub fn load_source(&mut self, src: crate::SourceId, kinds: &[crate::Kind]) -> Result<(), EditError> {
        self.load_source_at(src, 0, kinds)
    }
    /// [`EditorState::load_source`] at source tick `at` (clamped to the reel).
    fn load_source_at(&mut self, src: crate::SourceId, at: i64, kinds: &[crate::Kind]) -> Result<(), EditError> {
        let dur = self.timeline.source(src).ok_or(EditError::NoSource)?.duration;
        let kinds: Vec<crate::Kind> = if kinds.is_empty() {
            [crate::Kind::Video, crate::Kind::Audio].into_iter().filter(|k| self.timeline.tracks.iter().any(|t| t.kind == *k && t.clips.iter().any(|c| c.src == src))).collect()
        } else {
            kinds.to_vec()
        };
        self.source = Some(SourceMon { src, at: at.clamp(0, (dur - 1).max(0)), marks: (None, None), kinds });
        Ok(())
    }
    /// The tracks a loaded source's insert lays on: for each of its kinds, the first track of that kind holding a clip of
    /// the reel, else the first track of that kind (deduped, in kind order). Empty when nothing is loaded.
    pub fn source_tracks(&self) -> Vec<usize> {
        let Some(m) = self.source.as_ref() else { return Vec::new() };
        let tracks = &self.timeline.tracks;
        let mut out = Vec::new();
        for k in &m.kinds {
            let holding = tracks.iter().position(|t| t.kind == *k && t.clips.iter().any(|c| c.src == m.src));
            if let Some(t) = holding.or_else(|| tracks.iter().position(|t| t.kind == *k)) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        out
    }
    /// The loaded source's marked range laid at the playhead (ONE commit through `place_tracks_at_playhead`); `None` when
    /// nothing is loaded (SR852's selected-clip road decides). No track for its kinds: `EditError::NoTrack`.
    pub fn insert_source(&mut self, mode: crate::Mode) -> Option<Result<(), EditError>> {
        let m = self.source.clone()?;
        let Some(dur) = self.timeline.source(m.src).map(|s| s.duration) else { return Some(Err(EditError::NoSource)) };
        let (a, b) = source_marked(&m, dur); // STRIASOURCEDRAG (SR9081): the one marked-range rule, shared with the drop (file tail)
        let tracks = self.source_tracks();
        Some(self.place_tracks_at_playhead(&tracks, m.src, a, b, mode))
    }
    /// The loaded source as a one-clip timeline: one video track showing source tick `at` at tick 0 (the face's frame road
    /// renders its tick 0). `None` when nothing is loaded or the reel is gone.
    pub fn source_view(&self) -> Option<Timeline> {
        let m = self.source.as_ref()?;
        self.timeline.source(m.src)?;
        let mut v = self.timeline.clone();
        v.tracks.clear();
        v.audio_xfades.clear();
        v.marks = (None, None);
        let t = v.add_track(crate::Kind::Video);
        v.tracks[t].clips.push(Clip::new(m.src, m.at, m.at + 1, 0));
        Some(v)
    }
    /// The face's standing line while a source is loaded: `source A 00:00:00:12  in 00:00:00:05  out --` (ticks when the
    /// timebase is not a whole fps).
    pub fn source_line(&self) -> Option<alloc::string::String> {
        let m = self.source.as_ref()?;
        let reel = self.timeline.source(m.src).map_or_else(|| alloc::format!("{}", m.src.0), |s| s.reel.clone());
        let fps = self.timeline.fps();
        let tc = |t: Option<i64>| -> alloc::string::String {
            match (t, fps) {
                (None, _) => alloc::string::String::from("--"),
                (Some(t), Some(f)) => edl::timecode(t, f),
                (Some(t), None) => alloc::format!("{t}"),
            }
        };
        Some(alloc::format!("source {reel} {}  in {}  out {}", tc(Some(m.at)), tc(m.marks.0), tc(m.marks.1)))
    }
    /// The selected clip's reel loaded at the source tick under the playhead (clamped into the clip).
    fn load_selected(&mut self) -> Result<(), EditError> {
        let c = self.selected().ok_or(EditError::NoClip)?;
        let at = c.src_in + (self.playhead - c.at).clamp(0, (c.src_out - c.src_in - 1).max(0));
        self.load_source_at(c.src, at, &[])
    }
    /// The source playhead stepped `dt` ticks, clamped to the reel.
    fn source_step(&mut self, dt: i64) -> Result<(), EditError> {
        let src = self.source.as_ref().ok_or(EditError::NoSource)?.src;
        let dur = self.timeline.source(src).ok_or(EditError::NoSource)?.duration;
        let m = self.source.as_mut().ok_or(EditError::NoSource)?;
        m.at = (m.at + dt).clamp(0, (dur - 1).max(0));
        Ok(())
    }
    /// The source's `edge` mark set at the source playhead, keeping the other; in not before out: `EditError::Marks`.
    fn source_mark(&mut self, edge: Edge) -> Result<(), EditError> {
        let m = self.source.as_mut().ok_or(EditError::NoSource)?;
        let marks = match edge {
            Edge::In => (Some(m.at), m.marks.1),
            Edge::Out => (m.marks.0, Some(m.at)),
        };
        if let (Some(a), Some(b)) = marks {
            if a >= b {
                return Err(EditError::Marks);
            }
        }
        m.marks = marks;
        Ok(())
    }
    /// The source monitor's words of [`EditorState::verb`]; `None` for any other word or shape.
    fn source_verb(&mut self, v: &str) -> Option<Result<(), EditError>> {
        Some(match source_words(v)? {
            ("source", Some(reel)) => match self.timeline.source_by_reel(reel).map(|s| s.id) {
                Some(id) => self.load_source(id, &[]),
                None => Err(EditError::NoSource),
            },
            ("source", None) => self.load_selected(),
            ("unload", None) => {
                self.source = None;
                Ok(())
            }
            ("src-prev", None) => self.source_step(-1),
            ("src-next", None) => self.source_step(1),
            ("src-in", None) => self.source_mark(Edge::In),
            ("src-out", None) => self.source_mark(Edge::Out),
            _ => return None,
        })
    }
}

/// `source [<reel>]`, `unload`, `src-prev`, `src-next`, `src-in`, `src-out` -> `(word, argument)`; any other word, or more
/// than one argument, is not this verb.
fn source_words(v: &str) -> Option<(&str, Option<&str>)> {
    let mut w = v.split_ascii_whitespace();
    let head = w.next()?;
    let arg = w.next();
    if w.next().is_some() || !matches!(head, "source" | "unload" | "src-prev" | "src-next" | "src-in" | "src-out") {
        return None;
    }
    Some((head, arg))
}

#[cfg(test)]
mod striasourceviewer_tests {
    use super::*;
    use crate::{Kind, Mode, SourceId};

    /// A 13..23 at 0 on V1 and A1; B and C (50 ticks each) on no track.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_source("B", 50);
        tl.add_source("C", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0)];
        }
        EditorState::new(tl)
    }

    fn words(st: &mut EditorState, vs: &[&str]) {
        for v in vs {
            assert_eq!(st.verb(v), Some(Ok(())), "{v}");
        }
    }

    /// Loading, stepping and marking push nothing: the timeline, undo, redo and playhead stay; the step clamps to the reel.
    #[test]
    fn striasourceviewer_load_step_mark_push_nothing() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("source A"), Some(Ok(())));
        assert_eq!(st.source, Some(SourceMon { src: SourceId(0), at: 0, marks: (None, None), kinds: alloc::vec![Kind::Video, Kind::Audio] }));
        words(&mut st, &["src-next", "src-next", "src-in", "src-next", "src-next", "src-next", "src-out", "src-prev"]);
        assert_eq!(st.source.as_ref().map(|m| (m.at, m.marks)), Some((4, (Some(2), Some(5)))));
        assert_eq!((st.timeline.clone(), st.undo_depth(), st.redo_depth(), st.playhead), (tl0, 0, 0, 0));
        words(&mut st, &["source C", "src-prev"]);
        assert_eq!(st.source.as_ref().map(|m| (m.src, m.at, m.marks, m.kinds.len())), Some((SourceId(2), 0, (None, None), 0)), "C on no track: no kinds");
        for _ in 0..60 {
            words(&mut st, &["src-next"]);
        }
        assert_eq!(st.source.as_ref().map(|m| m.at), Some(49), "clamped to the reel's last tick");
        words(&mut st, &["unload", "unload"]);
        assert_eq!(st.source, None);
    }

    /// With a source loaded, bare insert / overwrite lay its marked range at the timeline playhead on V1 and A1 in ONE
    /// commit; the source stays loaded with its marks; one undo restores.
    #[test]
    fn striasourceviewer_insert_lays_the_marked_range() {
        let mut st = state();
        let tl0 = st.timeline.clone();
        assert_eq!(st.load_source(SourceId(2), &[Kind::Video, Kind::Audio]), Ok(()));
        for _ in 0..10 {
            words(&mut st, &["src-next"]);
        }
        words(&mut st, &["src-in"]);
        for _ in 0..10 {
            words(&mut st, &["src-next"]);
        }
        words(&mut st, &["src-out"]);
        st.seek(5);
        assert_eq!(st.verb("insert"), Some(Ok(())));
        let c = Clip::new(SourceId(2), 10, 20, 5);
        assert_eq!(st.timeline, tl0.place(0, c, Mode::Insert).unwrap().place(1, c, Mode::Insert).unwrap());
        assert_eq!((st.undo_depth(), st.playhead, st.selected()), (1, 15, Some(c)));
        assert_eq!(st.source.as_ref().map(|m| m.marks), Some((Some(10), Some(20))), "the source stays loaded, marks kept");
        assert!(st.undo());
        assert_eq!(st.timeline, tl0);
        let mut o = state();
        assert_eq!(o.load_source(SourceId(1), &[Kind::Video]), Ok(()));
        o.seek(2);
        assert_eq!(o.verb("overwrite"), Some(Ok(())));
        assert_eq!(o.timeline, state().timeline.place(0, Clip::new(SourceId(1), 0, 50, 2), Mode::Overwrite).unwrap(), "no marks: the whole reel, V1 only");
    }

    /// Refusals by name, each changing nothing; a malformed shape is not this verb.
    #[test]
    fn striasourceviewer_refusals() {
        let mut st = state();
        for v in ["src-in", "src-out", "src-prev", "src-next"] {
            assert_eq!(st.verb(v), Some(Err(EditError::NoSource)), "{v}");
        }
        assert_eq!(st.verb("source Z"), Some(Err(EditError::NoSource)));
        assert_eq!(st.verb("source A B"), None);
        words(&mut st, &["source A", "src-next", "src-next", "src-next", "src-next", "src-next", "src-out", "src-next", "src-next", "src-next"]);
        assert_eq!(st.verb("src-in"), Some(Err(EditError::Marks)));
        assert_eq!(st.source.as_ref().map(|m| m.marks), Some((None, Some(5))));
        words(&mut st, &["source B"]);
        let tl0 = st.timeline.clone();
        assert_eq!(st.verb("insert"), Some(Err(EditError::NoTrack)), "B sits on no track and names no kind");
        assert_eq!((st.timeline.clone(), st.undo_depth()), (tl0, 0));
    }

    /// Bare `source` is match frame: the selected clip's reel at the source tick under the playhead; none selected: NoClip.
    #[test]
    fn striasourceviewer_bare_source_matches_the_frame() {
        let mut st = state();
        assert_eq!(st.verb("source"), Some(Err(EditError::NoClip)));
        assert_eq!(st.select(0, 0), Ok(()));
        st.seek(4);
        assert_eq!(st.verb("source"), Some(Ok(())));
        assert_eq!(st.source.as_ref().map(|m| (m.src, m.at)), Some((SourceId(0), 17)));
    }

    /// The one-clip view and the standing line follow the source; nothing loaded: neither.
    #[test]
    fn striasourceviewer_view_and_line() {
        let mut st = state();
        assert_eq!((st.source_view(), st.source_line()), (None, None));
        words(&mut st, &["source A", "src-next", "src-next", "src-next"]);
        let mut want = st.timeline.clone();
        want.tracks.clear();
        want.audio_xfades.clear();
        want.marks = (None, None);
        want.add_track(Kind::Video);
        want.tracks[0].clips = alloc::vec![Clip::new(SourceId(0), 3, 4, 0)];
        assert_eq!(st.source_view(), Some(want));
        assert_eq!(st.source_line().as_deref(), Some("source A 00:00:00:03  in --  out --"));
        words(&mut st, &["src-in"]);
        assert_eq!(st.source_line().as_deref(), Some("source A 00:00:00:03  in 00:00:00:03  out --"));
    }
}

// ---- STRIASOURCEDRAG (LEDGER SR9081): the loaded source dropped on a track at a tick through the one placing road (named tail block) ----
// THE drag model's `DragKind::Source` (view.rs) finishes here: the loaded source's marked range (`source_marked`, the rule
// `insert_source` reads too) laid at the drop tick through SR976's `place_tracks_at_playhead` (ONE commit, one undo) on
// `source_tracks` with the dropped row in place of its kind's track (an AV reel dropped on V2 lays V2 and A1). The tick
// is reached by `seek` (clamped to the timeline: a drop past the end lays at the end); after the drop the playhead sits at
// the laid clip's end, as after an insert; refused: the playhead restored and nothing committed.

/// A loaded source's marked range: in or 0 .. out or the reel's duration `dur`.
fn source_marked(m: &SourceMon, dur: i64) -> (i64, i64) {
    (m.marks.0.unwrap_or(0), m.marks.1.unwrap_or(dur))
}

impl EditorState {
    /// Lay the loaded source's marked range at tick `at` with `track` as its kind's track (`mode`, ONE commit). Nothing
    /// loaded: `EditError::NoSource`; `track` missing or of a kind the source is not laid on: `EditError::NoTrack`.
    pub fn drop_source(&mut self, track: usize, at: i64, mode: crate::Mode) -> Result<(), EditError> {
        let m = self.source.clone().ok_or(EditError::NoSource)?;
        let dur = self.timeline.source(m.src).ok_or(EditError::NoSource)?.duration;
        let kind = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?.kind;
        if !m.kinds.contains(&kind) {
            return Err(EditError::NoTrack);
        }
        let tracks: Vec<usize> = self.source_tracks().into_iter().map(|t| if self.timeline.tracks[t].kind == kind { track } else { t }).collect();
        let (a, b) = source_marked(&m, dur);
        let ph = self.playhead;
        self.seek(at);
        let r = self.place_tracks_at_playhead(&tracks, m.src, a, b, mode);
        if r.is_err() {
            self.seek(ph);
        }
        r
    }
}

#[cfg(test)]
mod striasourcedrag_tests {
    use super::*;
    use crate::{Kind, Mode, SourceId};

    /// A 13..23 at 0 on V1, V2 and A1; B (50 ticks) on no track, loaded for video and audio.
    fn state() -> EditorState {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..3 {
            tl.tracks[t].clips = alloc::vec![Clip::new(a, 13, 23, 0)];
        }
        let mut st = EditorState::new(tl);
        assert_eq!(st.load_source(SourceId(1), &[Kind::Video, Kind::Audio]), Ok(()));
        st
    }

    /// Dropped on V2 at 4, B's marked range overwrites V2 and A1 (V1 untouched) in ONE undo step; the source stays loaded.
    #[test]
    fn striasourcedrag_drop_on_the_named_row_one_commit() {
        let mut st = state();
        st.source.as_mut().unwrap().marks = (Some(2), None);
        let tl0 = st.timeline.clone();
        assert_eq!(st.drop_source(1, 4, Mode::Overwrite), Ok(()));
        let c = Clip::new(SourceId(1), 2, 50, 4);
        assert_eq!(st.timeline, tl0.place(1, c, Mode::Overwrite).unwrap().place(2, c, Mode::Overwrite).unwrap());
        assert_eq!((st.undo_depth(), st.playhead, st.source.is_some()), (1, 52, true));
        assert!(st.undo());
        assert_eq!(st.timeline, tl0);
    }

    /// Refusals commit nothing and leave the playhead: nothing loaded, no such track, a kind the source is not laid on.
    #[test]
    fn striasourcedrag_drop_refusals() {
        let mut st = state();
        st.seek(7);
        st.source.as_mut().unwrap().kinds = alloc::vec![Kind::Video];
        let tl0 = st.timeline.clone();
        assert_eq!(st.drop_source(2, 3, Mode::Insert), Err(EditError::NoTrack), "A1 with a video-only source");
        assert_eq!(st.drop_source(9, 3, Mode::Insert), Err(EditError::NoTrack));
        st.source = None;
        assert_eq!(st.drop_source(0, 3, Mode::Insert), Err(EditError::NoSource));
        assert_eq!((&st.timeline, st.undo_depth(), st.playhead), (&tl0, 0, 7));
    }
}
