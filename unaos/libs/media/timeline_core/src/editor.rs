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

use crate::{Clip, EditError, Timeline, edl};

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
    pub playhead: i64,
    /// Pixels per tick, `ZOOM_MIN..=ZOOM_MAX`.
    pub zoom: u32,
    undo: Vec<Timeline>,
    redo: Vec<Timeline>,
}

impl EditorState {
    pub fn new(timeline: Timeline) -> EditorState {
        EditorState { timeline, selection: None, playhead: 0, zoom: ZOOM_DEFAULT, undo: Vec::new(), redo: Vec::new() }
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
        Ok(())
    }
    /// Select the clip on `track` showing tick `t`.
    pub fn select_at(&mut self, track: usize, t: i64) -> Result<(), EditError> {
        if track >= self.timeline.tracks.len() {
            return Err(EditError::NoTrack);
        }
        let idx = self.timeline.clip_at(track, t).ok_or(EditError::NoClip)?;
        self.selection = Some((track, idx));
        Ok(())
    }
    pub fn deselect(&mut self) {
        self.selection = None;
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
        self.selection = self.timeline.clip_at(track, self.playhead).map(|i| (track, i));
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
        self.selection = self.timeline.clip_at(to_track, to_at).map(|i| (to_track, i));
        Ok(())
    }
    /// Remove the selected clip and close the hole (`Timeline::ripple_delete`).
    pub fn ripple_delete(&mut self) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.ripple_delete(track, idx)?;
        self.commit(tl);
        self.selection = None;
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
        if self.selected().is_none() {
            self.selection = None;
        }
        self.seek(self.playhead);
    }
}
