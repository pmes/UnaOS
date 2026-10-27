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

//! `sonde` — the handler of probes (R112; CODEX Amendment IV, drafted in
//! `docs/dev/evidence/sonde/CODEX-AMENDMENT.md` for Peter's signature).
//!
//! CHARTER: Sonde — fulfiller (the probe service on the bus) over `probe_core` (shared-core).
//!
//! Three parts, each with one job:
//!
//! * [`Sonde`] — the probe service. It owns the probes (a [`ProbeSource`] each) and answers the bus:
//!
//!   | in | effect | out |
//!   |---|---|---|
//!   | `ProbeList { probes: [] }` | name the probes | `ProbeList { probes }` |
//!   | `ProbeOpen { id, stream, .. }` | start that stream | `ProbeFrame`… every tick, or `ProbeClose` when it cannot |
//!   | `ProbeClose { id, stream }` | stop it (`stream` empty = all of `id`) | `ProbeClose` per stream stopped |
//!
//! * [`model`] — the pane (R108: a SURFACE, an INPUT STREAM, a RESIZE — nothing widget-shaped). It turns
//!   depth frames into a colour ramp beside the colour stream on one RGBA surface. std-free (core + alloc
//!   only), so the UnaOS-native face links the same code the GTK face is fed by.
//! * [`PaneService`] — the seam between them and a face: `ProbeFrame`s and the face's input in, the pane's
//!   surface out as `SurfaceBlit { url: PANE_URL, .. }` (the bus's existing surface verb, which
//!   quartzite's `probe_view` paints). Input arrives as the bus's generic `Input { target: PANE_URL, text }`
//!   in the model's small grammar ([`model::PaneInput`]).
//!
//! THE LIVE PATH IS OWED. The host has no UVC reader today (no v4l2 / AVFoundation path anywhere in the
//! tree — `handlers/stria` decodes files, `tools/eyes` reads serial), so the one probe this handler
//! offers is [`ReplaySource`]: `probe_core::replay`'s deterministic D435i-shaped frames (a tilted depth
//! plane with a moving hole, YUY2 colour bars, a Y8 gradient). A `ProbeSource` over Linux v4l2 for the
//! D435i's three video nodes is the next rung; nothing above the trait changes when it lands.

extern crate alloc;

pub mod model;

use std::collections::BTreeMap;

use bandy::{SMessage, Synapse};
use probe_core::depth::{DepthScale, Intrinsics};
use probe_core::frame::{Frame, PixelFormat, StreamKind};
use probe_core::replay;

/// The surface id the pane's pixels travel under (`SurfaceBlit.url`) and the `Input.target` the face
/// sends its input to.
pub const PANE_URL: &str = "sonde://pane";

/// The tick: 30 Hz, the D435i's default rate at USB 3 (librealsense `src/ds/d400/d400-factory.cpp:693`).
pub const TICK_HZ: u32 = 30;

/// One frame as a source hands it over: the frame, and for depth its scale and intrinsics.
pub struct SourcedFrame {
    pub frame: Frame,
    pub scale: Option<DepthScale>,
    pub intrinsics: Option<Intrinsics>,
}

/// A probe. `open` arms a stream at a mode (0 = default), `next` produces its next frame.
pub trait ProbeSource: Send {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn streams(&self) -> Vec<StreamKind>;
    /// Arm `stream` at `width x height` (0 = the default mode). Err names why it cannot.
    fn open(&mut self, stream: StreamKind, width: u32, height: u32) -> Result<(), String>;
    fn close(&mut self, stream: StreamKind);
    fn next(&mut self, stream: StreamKind) -> Option<SourcedFrame>;
}

/// The replay probe: a D435i's three video streams from `probe_core::replay`.
///
/// Default modes are the D435i's USB 3 defaults in librealsense (`d400-factory.cpp:689-697`): depth and
/// infrared 848x480, colour 1280x720, 30 fps.
pub struct ReplaySource {
    modes: BTreeMap<&'static str, (u32, u32, u64)>,
}

impl Default for ReplaySource {
    fn default() -> Self {
        ReplaySource { modes: BTreeMap::new() }
    }
}

impl ReplaySource {
    pub const DEPTH_DEFAULT: (u32, u32) = (848, 480);
    pub const COLOR_DEFAULT: (u32, u32) = (1280, 720);
}

impl ProbeSource for ReplaySource {
    fn id(&self) -> &str {
        replay::REPLAY_ID
    }
    fn name(&self) -> &str {
        replay::REPLAY_NAME
    }
    fn streams(&self) -> Vec<StreamKind> {
        vec![StreamKind::Depth, StreamKind::Infrared, StreamKind::Color]
    }
    fn open(&mut self, stream: StreamKind, width: u32, height: u32) -> Result<(), String> {
        let (dw, dh) = match stream {
            StreamKind::Depth | StreamKind::Infrared => Self::DEPTH_DEFAULT,
            StreamKind::Color => Self::COLOR_DEFAULT,
            other => return Err(format!("{} has no {} stream", replay::REPLAY_ID, other.name())),
        };
        let (w, h) = if width == 0 || height == 0 { (dw, dh) } else { (width, height) };
        if w > 4096 || h > 4096 {
            return Err(format!("{w}x{h} is larger than any D435i mode"));
        }
        self.modes.insert(stream.name(), (w, h, 0));
        Ok(())
    }
    fn close(&mut self, stream: StreamKind) {
        self.modes.remove(stream.name());
    }
    fn next(&mut self, stream: StreamKind) -> Option<SourcedFrame> {
        let m = self.modes.get_mut(stream.name())?;
        let (w, h, seq) = *m;
        m.2 += 1;
        Some(match stream {
            StreamKind::Depth => SourcedFrame {
                frame: replay::depth_plane(w, h, 400, (3000 / w.max(1)).max(1) as u16, seq),
                scale: Some(DepthScale::D400_DEFAULT),
                intrinsics: Some(Intrinsics::ideal(w, h)),
            },
            StreamKind::Infrared => SourcedFrame { frame: replay::ir_gradient(w, h, seq), scale: None, intrinsics: None },
            StreamKind::Color => SourcedFrame { frame: replay::color_bars(w, h, seq), scale: None, intrinsics: None },
            _ => return None,
        })
    }
}

/// A frame as the bus carries it.
pub fn frame_message(id: &str, f: &SourcedFrame) -> SMessage {
    let fr = &f.frame;
    SMessage::ProbeFrame {
        id: id.to_string(),
        stream: fr.stream.name().to_string(),
        format: fr.format.name().to_string(),
        seq: fr.seq,
        timestamp_us: fr.timestamp_us,
        width: fr.width,
        height: fr.height,
        stride: fr.stride as u32,
        depth_scale: f.scale.map(|s| s.metres_per_unit).unwrap_or(0.0),
        intrinsics: f.intrinsics.map(|k| [k.fx, k.fy, k.ppx, k.ppy]),
        data: fr.data.clone(),
    }
}

/// The inverse: a `ProbeFrame` back into a validated frame (`None` for any other message, an unknown
/// format or stream name, or bytes that do not hold the stated geometry).
pub fn frame_from_message(msg: &SMessage) -> Option<SourcedFrame> {
    let SMessage::ProbeFrame { stream, format, seq, timestamp_us, width, height, stride, depth_scale, intrinsics, data, .. } =
        msg
    else {
        return None;
    };
    let stream = StreamKind::from_name(stream)?;
    let format = PixelFormat::from_name(format)?;
    let frame = Frame::new(stream, format, *width, *height, *stride as usize, *timestamp_us, *seq, data.clone()).ok()?;
    let scale = (format == PixelFormat::Z16 && *depth_scale > 0.0).then_some(DepthScale { metres_per_unit: *depth_scale });
    let intrinsics = intrinsics.map(|k| Intrinsics::from_rect_params(*width, *height, k));
    Some(SourcedFrame { frame, scale, intrinsics })
}

/// The probe service.
pub struct Sonde {
    sources: Vec<Box<dyn ProbeSource>>,
    /// Open streams, in open order: (source index, stream).
    open: Vec<(usize, StreamKind)>,
}

impl Default for Sonde {
    fn default() -> Self {
        Sonde::new()
    }
}

impl Sonde {
    /// The service with the replay probe (the only probe a host has until the live path lands).
    pub fn new() -> Sonde {
        Sonde::with_sources(vec![Box::new(ReplaySource::default())])
    }

    pub fn with_sources(sources: Vec<Box<dyn ProbeSource>>) -> Sonde {
        Sonde { sources, open: Vec::new() }
    }

    fn find(&self, id: &str) -> Option<usize> {
        self.sources.iter().position(|s| s.id() == id)
    }

    pub fn open_streams(&self) -> Vec<(String, StreamKind)> {
        self.open.iter().map(|(i, s)| (self.sources[*i].id().to_string(), *s)).collect()
    }

    /// Answer one bus message. Replies only; Sonde ignores everything that is not a probe request
    /// (including its own answers: a non-empty `ProbeList`, `ProbeFrame`).
    pub fn dispatch(&mut self, msg: &SMessage) -> Vec<SMessage> {
        match msg {
            SMessage::ProbeList { probes } if probes.is_empty() => {
                let probes = self
                    .sources
                    .iter()
                    .map(|s| {
                        (s.id().to_string(), s.name().to_string(), s.streams().iter().map(|k| k.name().to_string()).collect())
                    })
                    .collect();
                vec![SMessage::ProbeList { probes }]
            }
            SMessage::ProbeOpen { id, stream, width, height, .. } => {
                let closed = || vec![SMessage::ProbeClose { id: id.clone(), stream: stream.clone() }];
                let (Some(i), Some(kind)) = (self.find(id), StreamKind::from_name(stream)) else {
                    log::warn!("[SONDE] :: open refused: no probe {id} or no stream {stream}");
                    return closed();
                };
                match self.sources[i].open(kind, *width, *height) {
                    Ok(()) => {
                        if !self.open.contains(&(i, kind)) {
                            self.open.push((i, kind));
                        }
                        Vec::new()
                    }
                    Err(e) => {
                        log::warn!("[SONDE] :: open {id}/{stream} refused: {e}");
                        closed()
                    }
                }
            }
            SMessage::ProbeClose { id, stream } => {
                let Some(i) = self.find(id) else { return Vec::new() };
                let mut out = Vec::new();
                let mut keep = Vec::new();
                for (si, kind) in self.open.drain(..) {
                    if si == i && (stream.is_empty() || kind.name() == stream) {
                        self.sources[si].close(kind);
                        out.push(SMessage::ProbeClose { id: id.clone(), stream: kind.name().to_string() });
                    } else {
                        keep.push((si, kind));
                    }
                }
                self.open = keep;
                out
            }
            _ => Vec::new(),
        }
    }

    /// One tick: the next frame of every open stream.
    pub fn tick(&mut self) -> Vec<SMessage> {
        let mut out = Vec::new();
        for &(i, kind) in &self.open {
            if let Some(f) = self.sources[i].next(kind) {
                out.push(frame_message(self.sources[i].id(), &f));
            }
        }
        out
    }
}

/// The seam between the probe service and a face: `ProbeFrame`s and `Input { target: PANE_URL }` in,
/// `SurfaceBlit { url: PANE_URL }` out whenever the pane changed.
pub struct PaneService {
    pub pane: model::ProbePane,
}

impl PaneService {
    pub fn new(width: u32, height: u32) -> PaneService {
        PaneService { pane: model::ProbePane::new(width, height) }
    }

    pub fn handle(&mut self, msg: &SMessage) -> Option<SMessage> {
        let changed = match msg {
            SMessage::ProbeFrame { .. } => match frame_from_message(msg) {
                Some(f) => match f.frame.stream {
                    StreamKind::Depth => {
                        self.pane.set_depth(f.frame, f.scale.unwrap_or(DepthScale::D400_DEFAULT), f.intrinsics);
                        true
                    }
                    StreamKind::Color | StreamKind::Infrared => {
                        self.pane.set_color(f.frame);
                        true
                    }
                    _ => false,
                },
                None => false,
            },
            SMessage::Input { target, text } if target == PANE_URL => match model::PaneInput::parse(text) {
                Some(i) => self.pane.input(&i),
                None => false,
            },
            _ => false,
        };
        changed.then(|| {
            let (px, w, h) = self.pane.surface();
            SMessage::SurfaceBlit { url: PANE_URL.to_string(), width: w, height: h, pixels: px.to_vec(), damage: None }
        })
    }
}

/// Subscribe and serve Sonde on `synapse` until the bus closes: requests answered as they arrive, open
/// streams ticked at [`TICK_HZ`], and the pane kept current for whichever face listens.
pub async fn ignite(synapse: Synapse) {
    let rx = synapse.subscribe();
    serve(synapse, rx, Sonde::new(), PaneService::new(960, 360)).await
}

/// The serving loop against a receiver the caller already holds (subscribe first, spawn second, and no
/// request fired in between is missed — `handlers/facet`'s shape).
pub async fn serve(
    synapse: Synapse,
    mut rx: tokio::sync::broadcast::Receiver<SMessage>,
    mut sonde: Sonde,
    mut pane: PaneService,
) {
    use tokio::sync::broadcast::error::RecvError;
    log::info!("[SONDE] :: The Sounding is live ({} probe(s); live UVC path owed, replay offered).", sonde.sources.len());
    let mut beat = tokio::time::interval(std::time::Duration::from_micros(1_000_000 / TICK_HZ as u64));
    loop {
        tokio::select! {
            r = rx.recv() => match r {
                Ok(msg) => {
                    for reply in sonde.dispatch(&msg) {
                        synapse.fire(reply);
                    }
                    if let Some(blit) = pane.handle(&msg) {
                        synapse.fire(blit);
                    }
                }
                Err(RecvError::Lagged(n)) => log::warn!("[SONDE] :: lagged {n} messages behind the Synapse"),
                Err(RecvError::Closed) => {
                    log::info!("[SONDE] :: Synapse closed; The Sounding terminating.");
                    break;
                }
            },
            _ = beat.tick() => {
                for frame in sonde.tick() {
                    synapse.fire(frame);
                }
            }
        }
    }
}
