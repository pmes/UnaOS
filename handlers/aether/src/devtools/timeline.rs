//! The Performance panel: script, style, layout and paint milliseconds per frame.
//!
//! Seam: the style and layout figures are `css::last_style_timing` — the wall time of the last
//! `apply_stylesheets` (the cascade up to the folded box styles, then the re-layout), which the engine
//! already measures on every relayout. Script and paint are the pane's own clock around the two calls a
//! frame is made of: `AetherEngine::tick` (timers, microtasks, rAF, history ops and any relayout they
//! cause) and `AetherEngine::render_frame` (the paint, run only when the tick asked for one). Script is
//! the tick's time less the cascade + layout that ran inside it. A cascade ran inside the frame exactly
//! when `last_style_timing` changed across it (two cascades of identical nanosecond durations would read
//! as none — the cost of keeping the engine free of a frame counter for this panel).
//!
//! The samples are the pane's state, a bounded ring the face drives by calling [`Timeline::frame`] in
//! place of `tick` + `render_frame` while the pane records.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::AetherEngine;

/// Frames the ring keeps.
pub const CAPACITY: usize = 600;

/// One frame's figures.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameSample {
    /// The frame's ordinal since the pane started recording (0 is the load sample).
    pub frame: u64,
    pub script_ms: f64,
    pub style_ms: f64,
    pub layout_ms: f64,
    pub paint_ms: f64,
}

/// The ring of samples.
#[derive(Debug)]
pub struct Timeline {
    samples: VecDeque<FrameSample>,
    next: u64,
    /// `css::last_style_timing` as last read (the change detector).
    seen: (Duration, Duration),
    /// Whether [`Timeline::frame`] records (it always runs the frame).
    pub recording: bool,
}

impl Default for Timeline {
    fn default() -> Self {
        Self { samples: VecDeque::new(), next: 0, seen: (Duration::ZERO, Duration::ZERO), recording: true }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Timeline {
    fn push(&mut self, mut s: FrameSample) {
        s.frame = self.next;
        self.next += 1;
        if self.samples.len() == CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(s);
    }

    /// The cascade + layout that ran since the last read, if one did.
    fn fresh_style(&mut self) -> Option<(Duration, Duration)> {
        let now = crate::css::last_style_timing();
        (now != self.seen).then(|| {
            self.seen = now;
            now
        })
    }

    /// Records the load's cascade and layout (call after `load_page` / `load_html`; its script ran inside
    /// the load and is not separated from it, so script reads 0).
    pub fn sample_load(&mut self, engine: &mut AetherEngine) {
        let (style, layout) = self.fresh_style().unwrap_or_default();
        let t = Instant::now();
        engine.render_frame();
        let paint = t.elapsed();
        if self.recording {
            self.push(FrameSample { frame: 0, script_ms: 0.0, style_ms: ms(style), layout_ms: ms(layout), paint_ms: ms(paint) });
        }
    }

    /// One frame: the engine's tick, then its paint when the tick asks for one. Returns the paint's
    /// damage rects (None when nothing was painted) — what the face would have got from `render_frame`.
    pub fn frame(&mut self, engine: &mut AetherEngine) -> Option<Vec<(u32, u32, u32, u32)>> {
        self.seen = crate::css::last_style_timing();
        let t0 = Instant::now();
        let repaint = engine.tick();
        let tick = t0.elapsed();
        let (style, layout) = self.fresh_style().unwrap_or_default();
        let script = tick.saturating_sub(style + layout);
        let (damage, paint) = if repaint {
            let t1 = Instant::now();
            let d = engine.render_frame();
            (Some(d), t1.elapsed())
        } else {
            (None, Duration::ZERO)
        };
        if self.recording && (repaint || style > Duration::ZERO || script >= Duration::from_micros(50)) {
            self.push(FrameSample { frame: 0, script_ms: ms(script), style_ms: ms(style), layout_ms: ms(layout), paint_ms: ms(paint) });
        }
        damage
    }

    /// The samples, oldest first.
    pub fn samples(&self) -> impl Iterator<Item = &FrameSample> {
        self.samples.iter()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The sums over the ring (`frame` is the sample count).
    pub fn totals(&self) -> FrameSample {
        self.samples.iter().fold(FrameSample { frame: self.samples.len() as u64, ..FrameSample::default() }, |mut a, s| {
            a.script_ms += s.script_ms;
            a.style_ms += s.style_ms;
            a.layout_ms += s.layout_ms;
            a.paint_ms += s.paint_ms;
            a
        })
    }

    /// Forget every sample (the panel's Clear).
    pub fn clear(&mut self) {
        self.samples.clear();
        self.next = 0;
    }
}
