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
//! Where `vessels/phonolite` gives the engine a face, stria gives it a *nerve
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
//!   exactly stria's mode. And the drain is *device-buffer paced*, not block
//!   paced: queued commands drain only inside the cpal callback
//!   (`process_commands` runs from `write_output_f32`), which fires at the
//!   device buffer period — commonly 256–4096 frames (~5–85 ms) on CoreAudio
//!   since the stream uses the default buffer size. No fixed time budget is
//!   safe. stria therefore **respects the contract timing-free**: all control
//!   flows through a single owning task ([`control_loop`]), and a resume that
//!   follows a still-pending stop polls the engine's liveness flag until the
//!   `Stop` has *observably* drained (`is_active()` reads false) before the
//!   atomic-direct `start` — bounded by [`STOP_DRAIN_TIMEOUT`], after which it
//!   starts anyway and logs (a full command ring is the only way there).
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
use resonance::{AudioEngine, ResonanceHandle, ResonanceMeter, create_test_graph};
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
/// ~0.25 s — longer than [`STOP_DRAIN_TIMEOUT`], so even the timeout path of a
/// resume cannot masquerade as a device death; short enough to notice a dead
/// device promptly.
const DEATH_GRACE_TICKS: u32 = 8;

/// Upper bound on waiting for a queued `Stop` to observably drain before an
/// atomic-direct `start`. Queued commands drain at the *device buffer* period
/// (commonly ~5–85 ms on CoreAudio); 200 ms covers any sane buffer with room
/// to spare. Hitting it means the callback never drained (full ring or dead
/// device) — we start anyway and log.
pub const STOP_DRAIN_TIMEOUT: Duration = Duration::from_millis(200);

/// How often the resume path re-checks the liveness flag while waiting for a
/// pending `Stop` to drain.
const DRAIN_POLL: Duration = Duration::from_millis(2);

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

/// The engine-control face the control task drives. `resonance::ResonanceHandle`
/// is the real implementor; tests substitute a mock whose command drain is
/// arbitrarily delayed, so the ordering guarantee is asserted against the REAL
/// hazard (drain paced by the device buffer, not by any time constant).
pub trait EngineControl {
    /// Queue a master-frequency change. False if the command ring is full.
    fn set_frequency(&mut self, hz: f64) -> bool;
    /// Queue an arbitrary node-parameter change. False if the ring is full.
    fn set_param(&mut self, node: usize, param: usize, value: f64) -> bool;
    /// Queue a stop (drains at the audio callback). False if the ring is full.
    fn stop(&mut self) -> bool;
    /// Atomic-direct resume.
    fn start(&mut self);
    /// The shared liveness flag: false once a queued `Stop` has drained.
    fn is_active(&self) -> bool;
}

impl EngineControl for ResonanceHandle {
    fn set_frequency(&mut self, hz: f64) -> bool {
        ResonanceHandle::set_frequency(self, hz)
    }
    fn set_param(&mut self, node: usize, param: usize, value: f64) -> bool {
        ResonanceHandle::set_param(self, node, param, value)
    }
    fn stop(&mut self) -> bool {
        ResonanceHandle::stop(self)
    }
    fn start(&mut self) {
        ResonanceHandle::start(self)
    }
    fn is_active(&self) -> bool {
        ResonanceHandle::is_active(self)
    }
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
        tokio::spawn(control_loop(handle, control_rx, desired.clone()));

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

/// The single owner of the engine handle: applies control intents in arrival
/// order. A resume that follows a still-pending stop waits — timing-free —
/// until the queued `Stop` has *observably* drained (the liveness flag reads
/// false) before the atomic-direct `start`, so the late-draining `Stop` cannot
/// clobber the resume (the ordering contract). The wait is bounded by
/// [`STOP_DRAIN_TIMEOUT`]; on timeout it starts anyway and logs.
async fn control_loop<E: EngineControl>(
    mut handle: E,
    mut rx: mpsc::UnboundedReceiver<StriaControl>,
    desired: Arc<AtomicBool>,
) {
    // True while a Stop we successfully queued may still be in flight
    // (i.e. not yet observed as drained).
    let mut stop_pending = false;

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
                // Respect the contract: if our Stop may still be queued, wait
                // for PROOF it drained (liveness flips false) before the
                // atomic-direct start. Drain pace is the device buffer period,
                // so no fixed sleep is correct — poll the flag instead.
                if stop_pending {
                    let deadline = tokio::time::Instant::now() + STOP_DRAIN_TIMEOUT;
                    while handle.is_active() {
                        if tokio::time::Instant::now() >= deadline {
                            log::warn!(
                                "[STRIA] :: queued Stop never drained within {:?}; \
                                 starting anyway (full ring or dead device?)",
                                STOP_DRAIN_TIMEOUT
                            );
                            break;
                        }
                        tokio::time::sleep(DRAIN_POLL).await;
                    }
                    stop_pending = false;
                }
                handle.start();
                desired.store(true, Ordering::Release);
            }
            StriaControl::Running(false) => {
                if handle.stop() {
                    stop_pending = true;
                } else {
                    log::warn!("[STRIA] :: command queue full (stop)");
                }
                desired.store(false, Ordering::Release);
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

    /// A mock engine whose command drain is ARBITRARILY delayed — the real
    /// hazard: queued commands drain at the device-buffer period (~5–85 ms),
    /// not at any block-derived constant. `stop()` only queues; the "callback"
    /// drains it after `drain_after` further `is_active` polls, flipping the
    /// shared flag false exactly then — later than any fixed settle window
    /// would have waited.
    #[derive(Default)]
    struct MockEngine {
        active: bool,
        queued_stop: bool,
        /// How many `is_active` polls before a queued Stop drains.
        drain_after: u32,
        polls: u32,
        /// Ordered trace of externally visible transitions.
        trace: Vec<&'static str>,
    }

    struct MockHandle(std::rc::Rc<std::cell::RefCell<MockEngine>>);

    impl EngineControl for MockHandle {
        fn set_frequency(&mut self, _hz: f64) -> bool {
            true
        }
        fn set_param(&mut self, _n: usize, _p: usize, _v: f64) -> bool {
            true
        }
        fn stop(&mut self) -> bool {
            let mut e = self.0.borrow_mut();
            e.queued_stop = true;
            e.polls = 0;
            e.trace.push("stop_queued");
            true
        }
        fn start(&mut self) {
            let mut e = self.0.borrow_mut();
            e.active = true;
            e.trace.push("start");
        }
        fn is_active(&self) -> bool {
            let mut e = self.0.borrow_mut();
            // Simulate the device-paced callback: the queued Stop drains only
            // after `drain_after` polls have elapsed.
            if e.queued_stop {
                e.polls += 1;
                if e.polls >= e.drain_after {
                    e.queued_stop = false;
                    e.active = false;
                    e.trace.push("stop_drained");
                }
            }
            e.active
        }
    }

    /// THE ordering invariant (AV-A1 note 1, seat must-fix): a rapid
    /// `stop(); start()` must net RUNNING even when the queued Stop drains
    /// far later than any block-derived settle window — the control loop must
    /// wait for OBSERVED drain, not a timed guess.
    #[tokio::test]
    async fn stop_then_start_nets_running_despite_delayed_drain() {
        let engine = std::rc::Rc::new(std::cell::RefCell::new(MockEngine {
            active: true,
            drain_after: 25, // drains only after 25 liveness polls (~50 ms real time)
            ..Default::default()
        }));
        let desired = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::unbounded_channel();

        // Rapid programmatic stop();start() — the bus-rate pattern.
        tx.send(StriaControl::Running(false)).unwrap();
        tx.send(StriaControl::Running(true)).unwrap();
        drop(tx); // loop exits after processing both

        // control_loop is !Send with MockHandle (Rc) — run it on a LocalSet.
        let local = tokio::task::LocalSet::new();
        local
            .run_until(control_loop(
                MockHandle(engine.clone()),
                rx,
                desired.clone(),
            ))
            .await;

        let e = engine.borrow();
        assert!(
            e.active,
            "stop();start() must net RUNNING — the late Stop must not clobber the start"
        );
        assert!(!e.queued_stop, "no Stop may remain in flight after the resume");
        assert_eq!(
            e.trace.last().copied(),
            Some("start"),
            "start must be issued AFTER the observed drain, not before: {:?}",
            e.trace
        );
        assert!(
            e.trace.contains(&"stop_drained"),
            "the queued Stop must have observably drained before start: {:?}",
            e.trace
        );
        assert!(desired.load(Ordering::Acquire), "desired must end true");
    }

    /// The timeout path: a Stop that NEVER drains (full ring / dead device)
    /// must not wedge the control loop — after STOP_DRAIN_TIMEOUT the resume
    /// proceeds anyway.
    #[tokio::test(start_paused = true)]
    async fn resume_times_out_if_the_stop_never_drains() {
        let engine = std::rc::Rc::new(std::cell::RefCell::new(MockEngine {
            active: true,
            drain_after: u32::MAX, // never drains
            ..Default::default()
        }));
        let desired = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::unbounded_channel();

        tx.send(StriaControl::Running(false)).unwrap();
        tx.send(StriaControl::Running(true)).unwrap();
        drop(tx);

        // Paused tokio time auto-advances through the poll sleeps, so this
        // completes instantly in wall-clock terms while still exercising the
        // full STOP_DRAIN_TIMEOUT deadline logic.
        let local = tokio::task::LocalSet::new();
        local
            .run_until(control_loop(
                MockHandle(engine.clone()),
                rx,
                desired.clone(),
            ))
            .await;

        let e = engine.borrow();
        assert_eq!(
            e.trace.last().copied(),
            Some("start"),
            "the loop must not wedge: start proceeds after timeout"
        );
        assert!(desired.load(Ordering::Acquire));
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
