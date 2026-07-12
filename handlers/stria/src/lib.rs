// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! `stria` — the Studio handler: the resonance engine as a bus-driven service.
//!
//! Where `apps/phonolite` gives the engine a face, stria gives it a *nerve
//! ending*. It owns the whole engine lifecycle — build the graph, open the
//! device, keep the stream alive — and integrates it with the rest of UnaOS
//! over the [`bandy`] Synapse: it publishes the engine's output level as
//! [`SMessage::Spectrum`] beats through a real [`BandyMember`], and it exposes
//! a programmatic control surface (frequency / gain / running-state) that a
//! bus generator or a test can drive.
//!
//! # Design notes carried from the AV-A1 review
//!
//! - **Stop/start ordering contract.** `ResonanceHandle::stop()` is
//!   queue-routed (it drains at the next block boundary), while `start()` is
//!   atomic-direct. A rapid programmatic `stop(); start()` can therefore net
//!   out *stopped* when the queued `Stop` drains after the direct start.
//!   Unreachable at GUI timescales, but real at bus-driven rates — which is
//!   exactly stria's mode. stria **respects the contract** rather than
//!   assuming ordering: all control flows through a single owning task
//!   ([`control_loop`]), and a running-state transition that follows a stop
//!   waits one settle window ([`settle_after_stop`], ≥ one block period) so the
//!   queued `Stop` has certainly drained before the direct `start`.
//!
//! - **No re-entrant borrows.** The AV-A1 panel held a `borrow_mut()` across
//!   its user callback (safe there, a hazard for any copy). stria sidesteps the
//!   class of bug entirely: the [`ResonanceHandle`] has exactly one owner (the
//!   control task), reached only by message; nothing borrows it across a call.
//!
//! - **Liveness never desyncs.** The level cadence reads the engine's shared
//!   liveness flag and *gates the level on it* ([`governed_level`]): a dead
//!   device (the callback stops running and the peak atomic goes stale) still
//!   reports a truthful zero, and a persistent desired-but-inactive state is
//!   surfaced once via [`DeathWatch`].
//!
//! Nothing in this crate touches the Synapse from the real-time audio callback;
//! the callback only ever writes an atomic, and the cadence task turns that
//! into bus traffic on a calm ~30 Hz beat.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bandy::{BandyMember, SMessage, Synapse};
use resonance::{AudioEngine, BLOCK_SIZE, ResonanceMeter, create_test_graph};
use tokio::sync::{broadcast, mpsc};

/// The test graph's node layout (see `resonance::create_test_graph`):
/// node 1 = the gain node, its param 0 = base_gain.
pub const GAIN_NODE: usize = 1;
pub const GAIN_PARAM: usize = 0;

/// The level-meter cadence: ~30 Hz — fluid for a meter, far below any rate the
/// audio thread would care about (it only writes an atomic).
pub const LEVEL_BEAT: Duration = Duration::from_millis(33);

/// How many consecutive desired-but-inactive cadence ticks before stria
/// concludes the device is gone and says so (once). At [`LEVEL_BEAT`] this is
/// ~0.25 s — long enough to ride out a stop/start settle, short enough to
/// notice a dead device promptly.
const DEATH_GRACE_TICKS: u32 = 8;

// ---------------------------------------------------------------------------
// THE BUS FACE — a real BandyMember (no longer a println stub)
// ---------------------------------------------------------------------------

/// stria's nerve ending: a [`BandyMember`] that publishes onto a live
/// [`Synapse`]. This is the seam AV-A1 deliberately left dead on
/// `resonance::AudioEngine` (its `publish` only printed); here it delivers.
#[derive(Clone)]
pub struct StriaBus {
    synapse: Synapse,
}

impl StriaBus {
    pub fn new(synapse: Synapse) -> Self {
        Self { synapse }
    }

    /// Wrap a block of output samples as an [`SMessage::AudioChunk`] and
    /// publish it — the honest realization of the old `process_frame` stub.
    ///
    /// Not driven by the level cadence (which sees only the block peak, not the
    /// samples); it is the ready seam for a future real-time-safe sample tap
    /// and is exercised directly by callers and tests today.
    pub fn process_frame(&self, samples: Vec<f32>, sample_rate: u32) -> anyhow::Result<()> {
        self.publish(
            "system/audio/output",
            SMessage::AudioChunk {
                source_id: "stria".to_string(),
                samples,
                sample_rate,
            },
        )
    }
}

impl BandyMember for StriaBus {
    fn publish(&self, _topic: &str, msg: SMessage) -> anyhow::Result<()> {
        // The Synapse broadcast drops when no lobe is listening — that is the
        // intended nervous-system semantics, not an error.
        self.synapse.fire(msg);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PROGRAMMATIC CONTROL
// ---------------------------------------------------------------------------

/// A control intent for the engine. Every intent flows through the single
/// [`control_loop`] task, in order — the contract-respecting path (see the
/// module docs' stop/start note).
#[derive(Debug, Clone, PartialEq)]
pub enum StriaControl {
    /// Master frequency in Hz (targets the oscillator, node 0).
    Frequency(f64),
    /// Master gain (targets [`GAIN_NODE`] / [`GAIN_PARAM`]).
    Gain(f64),
    /// Desired running state — `true` resumes, `false` stops.
    Running(bool),
}

// ---------------------------------------------------------------------------
// PURE LOGIC — unit-tested without a device
// ---------------------------------------------------------------------------

/// The settle a `start` must wait when it follows a `stop`, so the queued
/// `Stop` has drained before the atomic-direct `start` (the ordering contract).
///
/// One block is `BLOCK_SIZE / sample_rate` seconds; we wait two block periods
/// plus a 1 ms scheduling margin. A zero/absurd rate falls back to 48 kHz so
/// this never divides by zero or returns a runaway delay.
pub fn settle_after_stop(sample_rate: u32) -> Duration {
    let rate = if sample_rate == 0 { 48_000 } else { sample_rate };
    let block_secs = BLOCK_SIZE as f64 / rate as f64;
    Duration::from_secs_f64(block_secs * 2.0) + Duration::from_millis(1)
}

/// The level to publish this tick. When the engine is not active — user-stopped
/// *or* device-dead — the truthful level is zero, regardless of what the
/// (possibly stale) peak atomic still holds. This is what keeps a dead device
/// from reporting a frozen non-zero meter.
pub fn governed_level(is_active: bool, raw_level: f32) -> f32 {
    if is_active { raw_level } else { 0.0 }
}

/// One level-meter beat: a single-bin [`SMessage::Spectrum`], clamped to the
/// meter's `0.0..=1.0` domain. Matches the AV-A1 convention (level = single-bin
/// Spectrum, bandy untouched).
pub fn level_beat(level: f32) -> SMessage {
    SMessage::Spectrum {
        magnitude: vec![level.clamp(0.0, 1.0)],
    }
}

/// Tracks a run of desired-but-inactive ticks so a dead device is reported
/// exactly once (not every 33 ms). Pure — the cadence task feeds it
/// observations and acts on the returned edge.
#[derive(Debug, Default)]
pub struct DeathWatch {
    inactive_ticks: u32,
    reported: bool,
}

impl DeathWatch {
    /// Feed one observation. Returns `true` on the single tick where a
    /// persistent desired-but-inactive condition first crosses the grace
    /// threshold — the caller logs once on that edge.
    pub fn observe(&mut self, desired_running: bool, engine_active: bool) -> bool {
        if !desired_running || engine_active {
            // Healthy (or intentionally stopped): reset the watch.
            self.inactive_ticks = 0;
            self.reported = false;
            return false;
        }
        self.inactive_ticks = self.inactive_ticks.saturating_add(1);
        if self.inactive_ticks >= DEATH_GRACE_TICKS && !self.reported {
            self.reported = true;
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// THE HANDLER
// ---------------------------------------------------------------------------

/// A live stria handler: the resonance engine plus its bus integration and
/// control surface. Keep it alive for as long as you want sound — dropping it
/// tears down the cpal stream and signals the tasks to stop.
///
/// `StriaHandler` is `Send` but not `Sync` (it owns the cpal stream, which is
/// `Send`-not-`Sync` on macOS); hold it on one thread and drive it through its
/// methods.
pub struct StriaHandler {
    control_tx: mpsc::UnboundedSender<StriaControl>,
    meter: ResonanceMeter,
    shutdown_tx: broadcast::Sender<()>,
    /// The engine owns the cpal stream and must outlive the tasks.
    _engine: AudioEngine,
}

impl StriaHandler {
    /// Ignite the handler: build the graph, open the default output device,
    /// and spawn the control + level-cadence tasks onto the current Tokio
    /// runtime. **Must be called from within a Tokio runtime.**
    ///
    /// The engine starts running; drive it afterward with [`Self::set_running`]
    /// / [`Self::set_frequency`] / [`Self::set_gain`].
    pub fn ignite(synapse: Synapse) -> anyhow::Result<Self> {
        let graph = create_test_graph();
        let (engine, handle) = AudioEngine::new(graph)?;
        let sample_rate = engine.sample_rate;
        log::info!("[STRIA] :: Audio engine live at {sample_rate} Hz device rate.");

        let meter = handle.meter();
        let desired = Arc::new(AtomicBool::new(true));
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, _) = broadcast::channel(1);

        // Control task: the sole owner of the ResonanceHandle. No shared
        // mutability, no re-entrant borrows.
        tokio::spawn(control_loop(
            handle,
            control_rx,
            desired.clone(),
            settle_after_stop(sample_rate),
        ));

        // Level cadence: meter -> Synapse, gating level on liveness.
        let bus = StriaBus::new(synapse);
        tokio::spawn(level_loop(
            bus,
            meter.clone(),
            desired,
            shutdown_tx.subscribe(),
        ));

        Ok(Self {
            control_tx,
            meter,
            shutdown_tx,
            _engine: engine,
        })
    }

    /// Set the master frequency in Hz. Non-blocking; applied in order by the
    /// control task.
    pub fn set_frequency(&self, hz: f64) {
        let _ = self.control_tx.send(StriaControl::Frequency(hz));
    }

    /// Set the master gain. Non-blocking; applied in order by the control task.
    pub fn set_gain(&self, gain: f64) {
        let _ = self.control_tx.send(StriaControl::Gain(gain));
    }

    /// Set the desired running state. A resume that follows a stop is settled
    /// against the ordering contract by the control task.
    pub fn set_running(&self, running: bool) {
        let _ = self.control_tx.send(StriaControl::Running(running));
    }

    /// A cloneable, `Send` read-only probe of the engine's level + liveness,
    /// for callers that want to meter the output themselves.
    pub fn meter(&self) -> ResonanceMeter {
        self.meter.clone()
    }
}

impl Drop for StriaHandler {
    fn drop(&mut self) {
        // Signal the cadence task to wind down; the control task ends when the
        // sender (held here) drops.
        let _ = self.shutdown_tx.send(());
    }
}

/// The single owner of the [`resonance::ResonanceHandle`]: applies control
/// intents in arrival order, settling a resume that follows a stop so the
/// queued `Stop` cannot clobber the direct `start` (the ordering contract).
async fn control_loop(
    mut handle: resonance::ResonanceHandle,
    mut rx: mpsc::UnboundedReceiver<StriaControl>,
    desired: Arc<AtomicBool>,
    settle: Duration,
) {
    // When the last stop happened, so a following start can settle against it.
    let mut last_stop: Option<tokio::time::Instant> = None;

    while let Some(ctrl) = rx.recv().await {
        match ctrl {
            StriaControl::Frequency(hz) => {
                if !handle.set_frequency(hz) {
                    log::warn!("[STRIA] :: command queue full (frequency)");
                }
            }
            StriaControl::Gain(gain) => {
                if !handle.set_param(GAIN_NODE, GAIN_PARAM, gain) {
                    log::warn!("[STRIA] :: command queue full (gain)");
                }
            }
            StriaControl::Running(true) => {
                // Respect the contract: if a stop is still possibly in flight,
                // let it drain before the atomic-direct start.
                if let Some(t) = last_stop.take() {
                    let elapsed = t.elapsed();
                    if elapsed < settle {
                        tokio::time::sleep(settle - elapsed).await;
                    }
                }
                handle.start();
                desired.store(true, Ordering::Release);
            }
            StriaControl::Running(false) => {
                if !handle.stop() {
                    log::warn!("[STRIA] :: command queue full (stop)");
                }
                desired.store(false, Ordering::Release);
                last_stop = Some(tokio::time::Instant::now());
            }
        }
    }
    log::info!("[STRIA] :: Control loop terminating (handler dropped).");
}

/// The level cadence: turns the engine's per-block peak atomic into
/// [`SMessage::Spectrum`] beats on the bus, gating the level on liveness and
/// surfacing a dead device once.
async fn level_loop(
    bus: StriaBus,
    meter: ResonanceMeter,
    desired: Arc<AtomicBool>,
    mut shutdown: broadcast::Receiver<()>,
) {
    let mut cadence = tokio::time::interval(LEVEL_BEAT);
    cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut death = DeathWatch::default();

    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                log::info!("[STRIA] :: Level beat terminating cleanly.");
                break;
            }
            _ = cadence.tick() => {
                let active = meter.is_active();
                let level = governed_level(active, meter.level());
                let _ = bus.publish("system/audio/level", level_beat(level));

                if death.observe(desired.load(Ordering::Acquire), active) {
                    log::warn!(
                        "[STRIA] :: engine desired-running but inactive — device appears gone."
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TESTS — pure logic + the live bus face, no audio device required
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_covers_at_least_one_block_and_never_divides_by_zero() {
        // 48 kHz: one block is 64/48000 ≈ 1.333 ms; settle is ~2 blocks + 1 ms.
        let s48 = settle_after_stop(48_000);
        let block = Duration::from_secs_f64(BLOCK_SIZE as f64 / 48_000.0);
        assert!(s48 > block, "settle must exceed one block period");
        assert!(s48 < Duration::from_millis(10), "settle should stay small");

        // A zero rate must fall back, not panic or blow up.
        let s0 = settle_after_stop(0);
        assert_eq!(s0, s48, "zero rate falls back to 48 kHz");

        // Lower rate => longer block => longer settle.
        assert!(settle_after_stop(44_100) > s48);
    }

    #[test]
    fn governed_level_zeroes_when_inactive() {
        assert_eq!(governed_level(true, 0.42), 0.42);
        // A stale non-zero peak must NOT leak through once inactive.
        assert_eq!(governed_level(false, 0.42), 0.0);
        assert_eq!(governed_level(false, 0.0), 0.0);
    }

    #[test]
    fn level_beat_is_a_single_clamped_magnitude() {
        for (input, expected) in [(0.3f32, 0.3f32), (-1.0, 0.0), (7.5, 1.0)] {
            match level_beat(input) {
                SMessage::Spectrum { magnitude } => {
                    assert_eq!(magnitude, vec![expected], "input {input}");
                }
                other => panic!("level_beat must yield Spectrum, got {other:?}"),
            }
        }
    }

    #[test]
    fn deathwatch_fires_once_after_the_grace_window() {
        let mut dw = DeathWatch::default();
        // Desired running but inactive: silent until the grace threshold.
        for _ in 0..(DEATH_GRACE_TICKS - 1) {
            assert!(!dw.observe(true, false));
        }
        assert!(dw.observe(true, false), "must fire on crossing the threshold");
        // ...and never again while the condition persists.
        for _ in 0..20 {
            assert!(!dw.observe(true, false));
        }
    }

    #[test]
    fn deathwatch_resets_when_healthy_or_intentionally_stopped() {
        let mut dw = DeathWatch::default();
        for _ in 0..(DEATH_GRACE_TICKS + 3) {
            dw.observe(true, false);
        }
        // Engine comes back: watch resets, can fire again on a fresh outage.
        assert!(!dw.observe(true, true));
        for _ in 0..(DEATH_GRACE_TICKS - 1) {
            assert!(!dw.observe(true, false));
        }
        assert!(dw.observe(true, false));

        // A deliberate stop (desired == false) is never a death.
        let mut dw2 = DeathWatch::default();
        for _ in 0..100 {
            assert!(!dw2.observe(false, false));
        }
    }

    #[tokio::test]
    async fn bus_publish_delivers_onto_the_synapse() {
        let synapse = Synapse::new();
        let mut rx = synapse.subscribe();
        let bus = StriaBus::new(synapse);

        bus.publish("t", level_beat(0.5)).unwrap();
        match rx.recv().await.unwrap() {
            SMessage::Spectrum { magnitude } => assert_eq!(magnitude, vec![0.5]),
            other => panic!("expected Spectrum, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn process_frame_publishes_an_audio_chunk() {
        let synapse = Synapse::new();
        let mut rx = synapse.subscribe();
        let bus = StriaBus::new(synapse);

        bus.process_frame(vec![0.1, -0.2, 0.3], 48_000).unwrap();
        match rx.recv().await.unwrap() {
            SMessage::AudioChunk {
                source_id,
                samples,
                sample_rate,
            } => {
                assert_eq!(source_id, "stria");
                assert_eq!(samples, vec![0.1, -0.2, 0.3]);
                assert_eq!(sample_rate, 48_000);
            }
            other => panic!("expected AudioChunk, got {other:?}"),
        }
    }
}
