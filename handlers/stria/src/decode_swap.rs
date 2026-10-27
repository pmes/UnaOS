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
//! stand-in (no decoder fits the track: `is_real` false, nothing parked) is never probed. Any other error is the caller's. SWAPBACKSEEK (SR353): a seek (the Player's) or a run open (the editor's) first calls [`DecodeSwap::reseed`] — a fresh decoder primed with the in-band sets in effect at the target — and [`open`] is both opens' decoder-or-stand-in. SWAPBACKSPANCACHE (SR370): `reseed` replays EVERY in-band set blob in effect at the target ([`SetIndex`], built once per open, an indexed reel's earlier spans read through a share) and keeps two parked decoders keyed by their set state, so a seek back into a span reuses its decoder (`video reseed: ... cache=hit|miss`).

use std::sync::Arc;

use gneiss_pal::dsp::demux::{Demuxer, Packet, Track};
use gneiss_pal::dsp::video::{DecodeError, Frame, TestPattern, VideoDecoder, decoder_for, param_sets};

/// One video stream's swap state, owned by its decode loop (the Player's tick thread, one render run): no lock.
#[derive(Default)]
pub struct DecodeSwap {
    /// The real decoder that refused the stream, reset, while the stand-in plays.
    parked: Option<Box<dyn VideoDecoder>>,
    /// SWAPBACKSPANCACHE (SR370): real decoders parked by a reseed, keyed by the set state they hold (LRU, [`SPAN_CACHE`]).
    spans: Vec<(SetKey, Box<dyn VideoDecoder>)>,
    /// The set state the current real decoder was primed to at the last reseed.
    seeded: Option<SetKey>,
    /// Decode time (ns) of the last packet a REAL decoder took since the last reseed (the current one or a parked offer).
    last_at: Option<i64>,
    /// Fresh decoders `reseed` constructed (a cache miss each).
    built: u32,
    /// The open's in-band set index (built at the first reseed, or handed in by [`DecodeSwap::with_index`]).
    index: Option<Arc<SetIndex>>,
}

/// SWAPBACKSPANCACHE (SR370): how many reseed-parked decoders a stream keeps (SWAPBACK-CACHESIZE owes the knob).
pub const SPAN_CACHE: usize = 2;

/// A set state: the [`SetIndex`] blob ids [`SetIndex::replay`] gives, in replay order.
type SetKey = Vec<usize>;

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
                self.last_at = Some(track.to_ns(p.dts));
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
        if dec.is_real() {
            self.last_at = Some(track.to_ns(p.dts));
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
    /// SWAPBACKSPANCACHE (SR370): the decoder is primed with EVERY in-band set blob in effect at the keyframe
    /// ([`SetIndex::replay`]), and it is the outgoing real decoder when that already holds this set state, else a parked one
    /// keyed by it, else a fresh `decoder_for` (a miss); the outgoing real decoder is parked under its own key. A hit is
    /// re-primed too (a parked decoder was offered keyframes it refused), so its tables equal a fresh decoder's.
    pub fn reseed(&mut self, dec: &mut Box<dyn VideoDecoder>, track: &Track, demux: &Demuxer, t_ns: i64) {
        if (!dec.is_real() && self.parked.is_none()) || param_sets(track, &[]).is_none() {
            return;
        }
        let Some(ti) = demux.track_index(track.id) else { return };
        let Some((key, key_ns)) = demux.keyframe_before(ti, t_ns) else { return };
        let Some(&s) = demux.samples().get(key) else { return };
        let index = self.index_for(track, demux);
        let want = index.replay(track.to_ns(s.dts));
        let out_key = self.last_at.map(|a| index.replay(a)).or_else(|| self.seeded.clone()).unwrap_or_default();
        let hit = if out_key == want {
            if !dec.is_real() {
                if let Some(real) = self.parked.take() {
                    *dec = real;
                }
            }
            true
        } else {
            let found = self.spans.iter().position(|(k, _)| *k == want).map(|i| self.spans.remove(i).1);
            let hit = found.is_some();
            let next = match found {
                Some(d) => d,
                None => {
                    let Ok(d) = decoder_for(track) else { return };
                    self.built += 1;
                    d
                }
            };
            let old = std::mem::replace(dec, next);
            let out = if old.is_real() { Some(old) } else { self.parked.take() };
            if let Some(o) = out {
                self.spans.push((out_key, o));
                if self.spans.len() > SPAN_CACHE {
                    self.spans.remove(0);
                }
            }
            hit
        };
        dec.reset();
        for &b in &want {
            let data = index.blobs[b].clone();
            let _ = dec.decode(&Packet { track: track.id, pts: s.pts, dts: s.dts, duration: 0, keyframe: true, data, discard_ns: 0 });
        }
        dec.reset();
        log::info!("[STRIA] :: video reseed: at {key_ns} sets={} cache={}", want.len(), if hit { "hit" } else { "miss" });
        self.parked = None;
        self.seeded = Some(want);
        self.last_at = None;
    }

    /// SWAPBACKSPANCACHE (SR370): a run's swap state over an open's [`SetIndex`] already built (the editor's `Media` keeps one
    /// per reel, so each run open does not walk the reel again).
    pub fn with_index(index: Arc<SetIndex>) -> DecodeSwap {
        DecodeSwap { index: Some(index), ..Default::default() }
    }

    /// Fresh decoders [`DecodeSwap::reseed`] constructed (each a cache miss).
    pub fn built(&self) -> u32 {
        self.built
    }

    /// The open's set index, rebuilt (and the parked spans, keyed by its blob ids, dropped) when `demux` is not the one it
    /// was built over (a live demuxer rebuilt after an append).
    fn index_for(&mut self, track: &Track, demux: &Demuxer) -> Arc<SetIndex> {
        if let Some(i) = self.index.as_ref().filter(|i| i.stamp == stamp(demux)) {
            return i.clone();
        }
        if self.index.is_some() {
            self.spans.clear();
            self.seeded = None;
        }
        let i = Arc::new(SetIndex::of(track, demux));
        self.index = Some(i.clone());
        i
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

/// SWAPBACKSPANCACHE (SR370), SWAPBACK-SEEK-IDS / -INDEXED: one open's in-band parameter sets — every keyframe of the
/// track whose `param_sets` blob is non-empty (a carrier), each distinct blob interned once. Stria reads no set ids
/// (gneiss_pal's `param_sets` is THE set reader), so [`SetIndex::replay`] keeps ids right by order instead: the distinct
/// blobs at or before a time, each at its LAST occurrence, in stream order — the last NAL of every (kind, id) is then
/// replayed after every other NAL of that id (its blob's last occurrence is its own), and the replay is a subsequence of
/// the stream. An indexed open's carriers are read span by span through ONE `share` (the caller's window never moves).
pub struct SetIndex {
    stamp: (usize, usize),
    blobs: Vec<Vec<u8>>,
    /// (decode time ns, blob id) per carrier, in decode order.
    carriers: Vec<(i64, usize)>,
}

/// Which demuxer an index was built over: its bytes and, on a full open, its sample count (an indexed open's window moves).
fn stamp(demux: &Demuxer) -> (usize, usize) {
    (Arc::as_ptr(&demux.bytes()) as usize, if demux.indexed() { usize::MAX } else { demux.samples().len() })
}

impl SetIndex {
    /// Walk `demux` (track `track`) once: a full open's sample table, or every span of an indexed open.
    pub fn of(track: &Track, demux: &Demuxer) -> SetIndex {
        let mut ix = SetIndex { stamp: stamp(demux), blobs: Vec::new(), carriers: Vec::new() };
        let Some(ti) = demux.track_index(track.id) else { return ix };
        if param_sets(track, &[]).is_none() {
            return ix;
        }
        if !demux.indexed() {
            ix.scan(track, demux, ti);
            return ix;
        }
        let mut c = demux.share();
        let mut points = demux.index().to_vec();
        points.sort_by_key(|x| (x.pts_ns, x.pos));
        let (mut seen, mut covered) = (Vec::new(), i64::MIN);
        for x in points {
            if x.pts_ns <= covered || c.load_at(x.track_index, x.pts_ns).is_err() {
                continue;
            }
            let Some(first) = c.samples().first().map(|s| s.offset) else { continue };
            covered = c.samples().iter().map(|s| c.tracks()[s.track_index].to_ns(s.pts)).max().unwrap_or(covered);
            if seen.contains(&first) {
                continue;
            }
            seen.push(first);
            ix.scan(track, &c, ti);
        }
        ix.carriers.sort_by_key(|e| e.0);
        ix
    }

    fn scan(&mut self, track: &Track, demux: &Demuxer, ti: usize) {
        for s in demux.samples().iter().filter(|s| s.track_index == ti && s.keyframe) {
            let sets = if track.frame_prefix.is_empty() { param_sets(track, demux.sample_data(s)) } else { param_sets(track, &demux.packet_at(s).data) };
            let Some(blob) = sets.filter(|b| !b.is_empty()) else { continue };
            let id = match self.blobs.iter().position(|b| *b == blob) {
                Some(i) => i,
                None => {
                    self.blobs.push(blob);
                    self.blobs.len() - 1
                }
            };
            self.carriers.push((track.to_ns(s.dts), id));
        }
    }

    /// The set state at decode time `at_ns`: the distinct blobs of every carrier at or before it, ordered by their last
    /// occurrence (empty: the container's sets alone).
    pub fn replay(&self, at_ns: i64) -> Vec<usize> {
        let mut last: Vec<(usize, usize)> = Vec::new();
        for (pos, &(_, b)) in self.carriers.iter().enumerate().take_while(|(_, e)| e.0 <= at_ns) {
            match last.iter_mut().find(|e| e.1 == b) {
                Some(e) => e.0 = pos,
                None => last.push((pos, b)),
            }
        }
        last.sort_by_key(|e| e.0);
        last.into_iter().map(|e| e.1).collect()
    }
}
