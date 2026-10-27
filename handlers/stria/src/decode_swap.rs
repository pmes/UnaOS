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
//! stand-in (no decoder fits the track: `is_real` false, nothing parked) is never probed. Any other error is the caller's. SWAPBACKSEEK (SR353): a seek (the Player's) or a run open (the editor's) first calls [`DecodeSwap::reseed`] — a fresh decoder primed with the in-band sets in effect at the target — and [`open`] is both opens' decoder-or-stand-in.

use gneiss_pal::dsp::demux::{Demuxer, Packet, Track};
use gneiss_pal::dsp::video::{DecodeError, Frame, TestPattern, VideoDecoder, decoder_for, param_sets};

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

/// SWAPBACKSEEK (LEDGER SR353): the decode run restarts at a seek target, so the swap state is re-evaluated there.
/// Every decoder's `reset` keeps the last parameter sets it decoded, so after a seek the decoder (or the parked one)
/// would decode the target keyframe with the PRE-seek in-band sets: garbage at a keyframe inside a refused span that does
/// not repeat its sets, a refusal at one after it. `reseed` swaps in a fresh registry decoder (the container's sets:
/// the fallback) and feeds it the parameter sets of the nearest keyframe at or before the target that carries any
/// (`dsp::video::param_sets`), then resets it (pictures go, sets stay) and forgets the parked decoder. The ordinary
/// [`DecodeSwap::decode`] then decides at the target: a seek INTO a refused span goes to the stand-in at the target,
/// and a seek OUT of it plays the real decoder at once. An open-time stand-in, and a codec without parameter sets
/// (VP8, VP9, AV1: their `reset` stays the seek), are left alone.
impl DecodeSwap {
    /// Before the first packet of a run that starts at the keyframe at or before `t_ns` (track `track` of `demux`).
    pub fn reseed(&mut self, dec: &mut Box<dyn VideoDecoder>, track: &Track, demux: &Demuxer, t_ns: i64) {
        if (!dec.is_real() && self.parked.is_none()) || param_sets(track, &[]).is_none() {
            return;
        }
        let Some((key, _)) = demux.track_index(track.id).and_then(|i| demux.keyframe_before(i, t_ns)) else { return };
        let Ok(mut fresh) = decoder_for(track) else { return };
        if let Some(sets) = sets_before(track, demux, key) {
            let _ = fresh.decode(&sets);
            fresh.reset();
        }
        self.parked = None;
        *dec = fresh;
    }
}

/// SWAPBACKSEEK (SR353), NLE-STANDIN-OPEN: the ONE decoder-or-stand-in both opens use (the Player's open, the editor's
/// run open): the registry's decoder for `track`, else the labelled stand-in at the track's size.
pub fn open(track: &Track) -> Box<dyn VideoDecoder> {
    match decoder_for(track) {
        Ok(d) => d,
        Err(e) => {
            log::info!("[STRIA] :: {e}; test-pattern stand-in");
            Box::new(TestPattern::stand_in(track))
        }
    }
}

/// The parameter sets of the nearest keyframe of `track` at or before sample `key` (an index into
/// [`Demuxer::samples`]) that carries any, as one packet of that keyframe; `None` when none does (the container's apply).
fn sets_before(track: &Track, demux: &Demuxer, key: usize) -> Option<Packet> {
    let ti = demux.track_index(track.id)?;
    demux.samples().get(..=key)?.iter().rev().filter(|s| s.track_index == ti && s.keyframe).find_map(|s| {
        let mut p = demux.packet_at(s);
        p.data = param_sets(track, &p.data).filter(|d| !d.is_empty())?;
        Some(p)
    })
}
