// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (docs/CODEX.md §2 A/V) — the ONE codec-blind mid-stream video decoder swap the Player
//! ([`crate::media::Player`]) and the editor render ([`crate::nle`]) share.
//!
//! SWAPBACK (LEDGER SR329; HEVCPAL-SWAP SR304 factored out of the Player): a decoder that refuses the stream MID-STREAM
//! (`DecodeError::Unsupported`: an in-band SPS turning 13-bit HEVC (above 12: HEVCREXT-TOOLS), an H.264 stream switching to 4:2:2, ...)
//! hands the picture to the labelled stand-in ([`TestPattern::stand_in`] at the track's size) ONCE: the frames it still
//! holds are drained first, one `video decode: swapped to stand-in` line is logged, and the refused packet is drawn by the
//! stand-in so its interval is not a gap. The refused decoder is PARKED, reset (every decoder's `reset` forgets its
//! pictures and keeps its parameter sets, so the stream's in-band SPS / PPS stay known: a fresh registry decoder would
//! know only the container's and decode a bare IDR of the unsupported span as if it were supported). Every later
//! KEYFRAME packet (the demuxer's own flag) is offered to the parked decoder once — one decode, never per packet, the
//! bound on the cost and on a swap storm; when it decodes, it is the stream's decoder again (one `video decode: swapped
//! back at <pts ns>` line); when it refuses, it is reset and parked again and the stand-in draws the packet. An open-time
//! stand-in (no decoder fits the track: `is_real` false, nothing parked) is never probed. Any other error is the caller's.

use gneiss_pal::dsp::demux::{Packet, Track};
use gneiss_pal::dsp::video::{DecodeError, Frame, TestPattern, VideoDecoder};

/// One video stream's swap state, owned by its decode loop (the Player's tick thread, one render run): no lock.
#[derive(Default)]
pub struct DecodeSwap {
    /// The real decoder that refused the stream, reset, while the stand-in plays.
    parked: Option<Box<dyn VideoDecoder>>,
}

/// Which way a decode swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swap {
    ToStandIn,
    Back,
}

/// What one packet's decode gave: the frames to present, in order, and the swap it made (if any).
pub struct Decoded {
    pub frames: Vec<Frame>,
    pub swap: Option<Swap>,
}

impl DecodeSwap {
    /// True while the stand-in plays for a parked real decoder.
    pub fn on_stand_in(&self) -> bool {
        self.parked.is_some()
    }

    /// Decode `p` (a packet of `track`) on `dec`, swapping `dec` to the stand-in or back as the module doc says.
    pub fn decode(&mut self, dec: &mut Box<dyn VideoDecoder>, track: &Track, p: &Packet) -> Result<Decoded, DecodeError> {
        if p.keyframe {
            if let Some(mut real) = self.parked.take() {
                match real.decode(p) {
                    Ok(f) => {
                        *dec = real;
                        log::info!("[STRIA] :: video decode: swapped back at {}", track.to_ns(p.pts));
                        return Ok(Decoded { frames: f.into_iter().collect(), swap: Some(Swap::Back) });
                    }
                    Err(_) => {
                        real.reset();
                        self.parked = Some(real);
                    }
                }
            }
        }
        match dec.decode(p) {
            Ok(f) => Ok(Decoded { frames: f.into_iter().collect(), swap: None }),
            Err(e @ DecodeError::Unsupported(_)) if dec.is_real() => {
                let mut frames = dec.flush();
                let mut real = std::mem::replace(dec, Box::new(TestPattern::stand_in(track)));
                real.reset();
                self.parked = Some(real);
                log::warn!("[STRIA] :: video decode: swapped to stand-in: {e}");
                if let Ok(Some(f)) = dec.decode(p) {
                    frames.push(f);
                }
                Ok(Decoded { frames, swap: Some(Swap::ToStandIn) })
            }
            Err(e) => Err(e),
        }
    }
}
