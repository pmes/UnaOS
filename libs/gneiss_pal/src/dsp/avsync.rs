// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The playback clock (PLAYBACK M2).
//!
//! Three pieces, each pure and testable on a synthetic timeline:
//!
//! * **Clocks.** A [`TimeSource`] is a monotonic nanosecond counter ([`SystemTime`] on a real
//!   run, [`ManualTime`] in tests). [`WallClock`] maps it to *media time* with play / pause /
//!   seek / rate. [`AudioClock`] derives media time from what the audio device has actually
//!   consumed (frames played ÷ sample rate, minus output latency), interpolating between device
//!   callbacks with the time source but never by more than one callback period, and never
//!   running backwards. [`MasterClock`] is the one the player follows: audio when the stream has
//!   audio that is being played, else wall.
//! * **Scheduling.** [`Scheduler::decide`] turns (frame pts, next frame pts, master now) into
//!   [`Decision::Wait`] / [`Decision::Present`] / [`Decision::Drop`]: a frame is presented once
//!   its pts is due (within `early_ns`), dropped when the frame after it is already due (it would
//!   only ever be seen late, and showing it would delay the one that is on time), and the
//!   previous frame is *repeated* on any display tick where nothing new is due
//!   ([`Scheduler::on_display_tick`]). That gives 3:2 cadence for 24 fps on 60 Hz, 2:2 for 30 on
//!   60, and drops every other frame of 60 fps on 30 Hz.
//! * **Drift.** [`DriftMeter`] collects (wall, media) pairs and reports the master clock's offset
//!   from wall time, its least-squares slope in parts per million, and the worst excursion; the
//!   player also records each presented frame's pts against the master clock
//!   ([`DriftMeter::av_offset`]) — the A/V sync error the oracle asks for.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// A monotonic nanosecond counter.
pub trait TimeSource: Send + Sync {
    fn now_ns(&self) -> u64;
}

/// The host's monotonic clock.
pub struct SystemTime(Instant);
impl SystemTime {
    pub fn new() -> Self {
        SystemTime(Instant::now())
    }
}
impl Default for SystemTime {
    fn default() -> Self {
        Self::new()
    }
}
impl TimeSource for SystemTime {
    fn now_ns(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

/// A clock tests drive by hand; clones share the same counter.
#[derive(Clone, Default)]
pub struct ManualTime(Arc<AtomicU64>);
impl ManualTime {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&self, ns: u64) {
        self.0.store(ns, Ordering::SeqCst);
    }
    pub fn advance(&self, ns: u64) {
        self.0.fetch_add(ns, Ordering::SeqCst);
    }
}
impl TimeSource for ManualTime {
    fn now_ns(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Media time from a time source: `media = anchor_media + (now − anchor_src) × rate` while
/// playing, frozen while paused.
pub struct WallClock {
    src: Arc<dyn TimeSource>,
    anchor_src: u64,
    anchor_media: i64,
    /// Playback rate in parts per million of 1.0 (1_000_000 = normal speed).
    rate_ppm: i64,
    playing: bool,
}

impl WallClock {
    pub fn new(src: Arc<dyn TimeSource>) -> Self {
        let now = src.now_ns();
        WallClock { src, anchor_src: now, anchor_media: 0, rate_ppm: 1_000_000, playing: false }
    }
    pub fn media_ns(&self) -> i64 {
        if !self.playing {
            return self.anchor_media;
        }
        let dt = self.src.now_ns().saturating_sub(self.anchor_src) as i128;
        self.anchor_media + (dt * self.rate_ppm as i128 / 1_000_000) as i64
    }
    fn rebase(&mut self) {
        self.anchor_media = self.media_ns();
        self.anchor_src = self.src.now_ns();
    }
    pub fn play(&mut self) {
        if !self.playing {
            self.anchor_src = self.src.now_ns();
            self.playing = true;
        }
    }
    pub fn pause(&mut self) {
        if self.playing {
            self.rebase();
            self.playing = false;
        }
    }
    pub fn is_playing(&self) -> bool {
        self.playing
    }
    pub fn seek(&mut self, media_ns: i64) {
        self.anchor_media = media_ns;
        self.anchor_src = self.src.now_ns();
    }
    pub fn set_rate_ppm(&mut self, ppm: i64) {
        self.rebase();
        self.rate_ppm = ppm.max(0);
    }
}

/// Media time from audio actually played.
///
/// The output callback reports `on_played(frames)` after handing `frames` sample frames to the
/// device; media time is `start + (played − latency) / rate`. Between callbacks the clock
/// advances with the time source, capped at `max_interp_ns` (one callback period) past the
/// last report so a stalled device freezes the clock instead of letting video run on alone.
pub struct AudioClock {
    src: Arc<dyn TimeSource>,
    sample_rate: u32,
    latency_frames: u64,
    start_media: i64,
    played: u64,
    report_src: u64,
    max_interp_ns: u64,
    playing: bool,
    /// Last value returned: the clock never runs backwards across a report.
    last: std::sync::atomic::AtomicI64,
}

impl AudioClock {
    pub fn new(src: Arc<dyn TimeSource>, sample_rate: u32, latency_frames: u64, max_interp_ns: u64) -> Self {
        let now = src.now_ns();
        AudioClock {
            src,
            sample_rate: sample_rate.max(1),
            latency_frames,
            start_media: 0,
            played: 0,
            report_src: now,
            max_interp_ns,
            playing: false,
            last: std::sync::atomic::AtomicI64::new(i64::MIN),
        }
    }
    fn frames_ns(&self, frames: u64) -> i64 {
        (frames as i128 * 1_000_000_000 / self.sample_rate as i128) as i64
    }
    /// The device consumed `frames` more sample frames.
    pub fn on_played(&mut self, frames: u64) {
        self.played += frames;
        self.report_src = self.src.now_ns();
    }
    pub fn media_ns(&self) -> i64 {
        let audible = self.played.saturating_sub(self.latency_frames);
        let mut t = self.start_media + self.frames_ns(audible);
        if self.playing && audible > 0 {
            let dt = self.src.now_ns().saturating_sub(self.report_src).min(self.max_interp_ns);
            t += dt as i64;
        }
        let prev = self.last.load(Ordering::Relaxed);
        let t = t.max(if prev == i64::MIN { t } else { prev });
        self.last.store(t, Ordering::Relaxed);
        t
    }
    pub fn play(&mut self) {
        self.playing = true;
        self.report_src = self.src.now_ns();
    }
    pub fn pause(&mut self) {
        self.playing = false;
    }
    /// Restart at `media_ns` with nothing played (after a seek the output is flushed).
    pub fn seek(&mut self, media_ns: i64) {
        self.start_media = media_ns;
        self.played = 0;
        self.report_src = self.src.now_ns();
        self.last.store(i64::MIN, Ordering::Relaxed);
    }
}

/// The clock the player follows.
pub enum MasterClock {
    Audio(AudioClock),
    Wall(WallClock),
}

impl MasterClock {
    /// Audio master when the stream has an audio track that is actually being played, else wall.
    pub fn select(src: Arc<dyn TimeSource>, audio: Option<(u32, u64, u64)>) -> MasterClock {
        match audio {
            Some((rate, latency, interp)) => MasterClock::Audio(AudioClock::new(src, rate, latency, interp)),
            None => MasterClock::Wall(WallClock::new(src)),
        }
    }
    pub fn media_ns(&self) -> i64 {
        match self {
            MasterClock::Audio(a) => a.media_ns(),
            MasterClock::Wall(w) => w.media_ns(),
        }
    }
    pub fn play(&mut self) {
        match self {
            MasterClock::Audio(a) => a.play(),
            MasterClock::Wall(w) => w.play(),
        }
    }
    pub fn pause(&mut self) {
        match self {
            MasterClock::Audio(a) => a.pause(),
            MasterClock::Wall(w) => w.pause(),
        }
    }
    pub fn seek(&mut self, media_ns: i64) {
        match self {
            MasterClock::Audio(a) => a.seek(media_ns),
            MasterClock::Wall(w) => w.seek(media_ns),
        }
    }
    /// Forward the audio device's consumption to the audio clock (no-op on the wall clock).
    pub fn on_audio_played(&mut self, frames: u64) {
        if let MasterClock::Audio(a) = self {
            a.on_played(frames);
        }
    }
    pub fn is_audio(&self) -> bool {
        matches!(self, MasterClock::Audio(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Not due yet: come back in this many ns of media time.
    Wait(u64),
    /// Due: show it now.
    Present,
    /// Superseded: the next frame is already due; discard this one unseen.
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SchedStats {
    pub presented: u64,
    pub dropped: u64,
    /// Display ticks on which the previous frame was held because nothing new was due.
    pub repeated: u64,
}

pub struct Scheduler {
    /// A frame this close to due is presented now (half a display period is the usual choice).
    pub early_ns: u64,
    pub stats: SchedStats,
    presented_this_tick: bool,
}

impl Scheduler {
    pub fn new(early_ns: u64) -> Self {
        Scheduler { early_ns, stats: SchedStats::default(), presented_this_tick: false }
    }
    /// Decide for the frame at the head of the queue.
    pub fn decide(&mut self, pts_ns: i64, next_pts_ns: Option<i64>, now_ns: i64) -> Decision {
        let due = now_ns + self.early_ns as i64;
        if pts_ns > due {
            return Decision::Wait((pts_ns - due) as u64);
        }
        if let Some(n) = next_pts_ns {
            if n <= due {
                self.stats.dropped += 1;
                return Decision::Drop;
            }
        }
        self.stats.presented += 1;
        self.presented_this_tick = true;
        Decision::Present
    }
    /// Call once per display refresh, after deciding; counts a repeat when nothing new was shown.
    pub fn on_display_tick(&mut self, have_frame: bool) {
        if !self.presented_this_tick && have_frame {
            self.stats.repeated += 1;
        }
        self.presented_this_tick = false;
    }
}

/// Clock drift and A/V offset measurement.
#[derive(Default, Clone)]
pub struct DriftMeter {
    samples: Vec<(i64, i64)>,
    av: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftReport {
    /// (media − wall) at the last sample minus at the first, ns.
    pub drift_ns: i64,
    /// Least-squares slope of media against wall, minus one, in parts per million.
    pub slope_ppm: f64,
    /// Worst |media − wall − (media0 − wall0)| over the run, ns.
    pub max_excursion_ns: i64,
    /// Presented frames' |pts − master clock| at presentation: max and mean, ns.
    pub av_max_ns: i64,
    pub av_mean_ns: f64,
    pub av_count: usize,
}

impl DriftMeter {
    pub fn record(&mut self, wall_ns: i64, media_ns: i64) {
        self.samples.push((wall_ns, media_ns));
    }
    /// A frame with `pts_ns` was presented when the master clock read `clock_ns`.
    pub fn av_offset(&mut self, pts_ns: i64, clock_ns: i64) {
        self.av.push(pts_ns - clock_ns);
    }
    pub fn report(&self) -> DriftReport {
        let mut r = DriftReport { drift_ns: 0, slope_ppm: 0.0, max_excursion_ns: 0, av_max_ns: 0, av_mean_ns: 0.0, av_count: self.av.len() };
        if let (Some(&(w0, m0)), Some(&(w1, m1))) = (self.samples.first(), self.samples.last()) {
            r.drift_ns = (m1 - w1) - (m0 - w0);
            for &(w, m) in &self.samples {
                r.max_excursion_ns = r.max_excursion_ns.max(((m - w) - (m0 - w0)).abs());
            }
            let n = self.samples.len() as f64;
            if n >= 2.0 {
                let mw = self.samples.iter().map(|s| (s.0 - w0) as f64).sum::<f64>() / n;
                let mm = self.samples.iter().map(|s| (s.1 - m0) as f64).sum::<f64>() / n;
                let mut sxy = 0.0;
                let mut sxx = 0.0;
                for &(w, m) in &self.samples {
                    let x = (w - w0) as f64 - mw;
                    sxy += x * ((m - m0) as f64 - mm);
                    sxx += x * x;
                }
                if sxx > 0.0 {
                    r.slope_ppm = (sxy / sxx - 1.0) * 1e6;
                }
            }
        }
        if !self.av.is_empty() {
            r.av_max_ns = self.av.iter().map(|x| x.abs()).max().unwrap_or(0);
            r.av_mean_ns = self.av.iter().map(|&x| x.abs() as f64).sum::<f64>() / self.av.len() as f64;
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a synthetic timeline: frames at `pts`, a display at `hz`, the master clock `clock`
    /// advanced by `step` each display tick. Returns, per display tick, the index of the frame on
    /// glass (None before the first).
    fn run(pts: &[i64], hz: u64, ticks: usize, time: &ManualTime, clock: &mut MasterClock, sched: &mut Scheduler, meter: &mut DriftMeter, on_tick: &mut dyn FnMut(&mut MasterClock, u64)) -> Vec<Option<usize>> {
        let period = 1_000_000_000 / hz;
        let mut head = 0usize;
        let mut shown: Option<usize> = None;
        let mut out = Vec::new();
        clock.play();
        for _ in 0..ticks {
            let now = clock.media_ns();
            meter.record(time.now_ns() as i64, now);
            while head < pts.len() {
                match sched.decide(pts[head], pts.get(head + 1).copied(), now) {
                    Decision::Wait(_) => break,
                    Decision::Present => {
                        meter.av_offset(pts[head], now);
                        shown = Some(head);
                        head += 1;
                        break;
                    }
                    Decision::Drop => head += 1,
                }
            }
            sched.on_display_tick(shown.is_some());
            out.push(shown);
            time.advance(period);
            on_tick(clock, period);
        }
        out
    }

    fn frames(fps_num: i64, fps_den: i64, n: usize) -> Vec<i64> {
        (0..n as i64).map(|i| i * 1_000_000_000 * fps_den / fps_num).collect()
    }

    fn wall(time: &ManualTime) -> MasterClock {
        MasterClock::Wall(WallClock::new(Arc::new(time.clone())))
    }

    fn holds(v: &[Option<usize>]) -> Vec<usize> {
        // How many consecutive ticks each frame stayed on glass.
        let mut out = Vec::new();
        let mut cur = None;
        for &x in v {
            if x != cur {
                out.push(1);
                cur = x;
            } else if let Some(l) = out.last_mut() {
                *l += 1;
            }
        }
        out
    }

    #[test]
    fn cadence_30_on_60_is_2_2() {
        let t = ManualTime::new();
        let mut c = wall(&t);
        let mut s = Scheduler::new(8_333_333);
        let mut m = DriftMeter::default();
        let v = run(&frames(30, 1, 30), 60, 60, &t, &mut c, &mut s, &mut m, &mut |_, _| {});
        assert!(holds(&v).iter().all(|&h| h == 2), "{:?}", holds(&v));
        assert_eq!((s.stats.presented, s.stats.dropped, s.stats.repeated), (30, 0, 30));
        // Only integer-ns rounding separates pts from the tick (1e9/60 and 1e9/30 truncate).
        assert!(m.report().av_max_ns < 100, "{}", m.report().av_max_ns);
    }

    #[test]
    fn cadence_24_on_60_is_3_2() {
        let t = ManualTime::new();
        let mut c = wall(&t);
        let mut s = Scheduler::new(8_333_333);
        let mut m = DriftMeter::default();
        let v = run(&frames(24, 1, 24), 60, 60, &t, &mut c, &mut s, &mut m, &mut |_, _| {});
        let h = holds(&v);
        assert_eq!(&h[..8], &[3, 2, 3, 2, 3, 2, 3, 2], "{h:?}");
        assert_eq!(s.stats.presented, 24);
        assert_eq!(s.stats.dropped, 0);
        // Every frame lands within half a display period of its pts.
        assert!(m.report().av_max_ns <= 8_333_333);
    }

    #[test]
    fn sixty_on_thirty_drops_every_other() {
        let t = ManualTime::new();
        let mut c = wall(&t);
        // Early window 12 ms, not exactly half the 33.3 ms period: every odd frame sits exactly
        // on the half-period tie, where integer-ns rounding would pick parity frame by frame.
        let mut s = Scheduler::new(12_000_000);
        let mut m = DriftMeter::default();
        let v = run(&frames(60, 1, 60), 30, 30, &t, &mut c, &mut s, &mut m, &mut |_, _| {});
        assert_eq!(s.stats.presented, 30);
        assert_eq!(s.stats.dropped, 29, "the last frame has no successor to supersede it");
        assert!(v.iter().flatten().all(|&i| i % 2 == 0 || i == 59));
    }

    #[test]
    fn pause_freezes_and_seek_jumps() {
        let t = ManualTime::new();
        let mut w = WallClock::new(Arc::new(t.clone()));
        w.play();
        t.advance(500_000_000);
        assert_eq!(w.media_ns(), 500_000_000);
        w.pause();
        t.advance(10_000_000_000);
        assert_eq!(w.media_ns(), 500_000_000);
        w.seek(3_000_000_000);
        w.play();
        t.advance(250_000_000);
        assert_eq!(w.media_ns(), 3_250_000_000);
        w.set_rate_ppm(2_000_000);
        t.advance(100_000_000);
        assert_eq!(w.media_ns(), 3_450_000_000);
    }

    #[test]
    fn audio_clock_tracks_played_frames_and_interpolates() {
        let t = ManualTime::new();
        let mut a = AudioClock::new(Arc::new(t.clone()), 48_000, 480, 10_000_000);
        a.play();
        assert_eq!(a.media_ns(), 0);
        // One 10 ms callback (480 frames) fills the latency: still silent.
        t.advance(10_000_000);
        a.on_played(480);
        assert_eq!(a.media_ns(), 0);
        t.advance(10_000_000);
        a.on_played(480);
        assert_eq!(a.media_ns(), 10_000_000);
        t.advance(4_000_000);
        assert_eq!(a.media_ns(), 14_000_000, "interpolated between callbacks");
        t.advance(100_000_000);
        assert_eq!(a.media_ns(), 20_000_000, "a stalled device freezes the clock after one period");
        a.seek(5_000_000_000);
        assert_eq!(a.media_ns(), 5_000_000_000);
    }

    #[test]
    fn audio_master_fast_device_measured_and_video_follows() {
        // The audio device runs 100 ppm fast against wall time (48 004.8 frames per wall second).
        // Video follows the audio clock: every presented frame within half a display period of
        // the audio clock, and the drift meter reports the device's +100 ppm.
        let t = ManualTime::new();
        let mut c = MasterClock::select(Arc::new(t.clone()), Some((48_000, 0, 20_000_000)));
        assert!(c.is_audio());
        let mut s = Scheduler::new(8_333_333);
        let mut m = DriftMeter::default();
        let mut acc = 0.0f64;
        let pts = frames(30, 1, 1800);
        let v = run(&pts, 60, 3600, &t, &mut c, &mut s, &mut m, &mut |c, period| {
            if let MasterClock::Audio(a) = c {
                acc += period as f64 * 48_004.8 / 1e9;
                let whole = acc.floor();
                acc -= whole;
                a.on_played(whole as u64);
            }
        });
        let r = m.report();
        assert!((r.slope_ppm - 100.0).abs() < 2.0, "slope {}", r.slope_ppm);
        assert!((r.drift_ns - 6_000_000).abs() < 100_000, "60 s × 100 ppm = 6 ms, got {}", r.drift_ns);
        assert!(r.av_max_ns <= 8_333_333, "A/V {}", r.av_max_ns);
        // Running fast, the audio clock reaches the end early: a few frames are dropped, none
        // are out of order.
        let shown: Vec<usize> = v.iter().flatten().copied().collect();
        assert!(shown.windows(2).all(|w| w[0] <= w[1]));
        assert!(s.stats.presented >= 1795);
    }

    #[test]
    fn wall_master_has_no_drift() {
        let t = ManualTime::new();
        let mut c = MasterClock::select(Arc::new(t.clone()), None);
        let mut s = Scheduler::new(8_333_333);
        let mut m = DriftMeter::default();
        run(&frames(25, 1, 250), 60, 600, &t, &mut c, &mut s, &mut m, &mut |_, _| {});
        let r = m.report();
        assert_eq!(r.drift_ns, 0);
        assert!(r.slope_ppm.abs() < 1e-6);
        assert_eq!(s.stats.presented, 250);
    }
}
