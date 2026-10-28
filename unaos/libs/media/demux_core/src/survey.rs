// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core (NLEPROBE, LEDGER SR195: the editor's probe of a container file).
//!
//! What a non-linear editor needs to know about a file before it puts it on a timeline: every stream's codec,
//! time base, duration and frame count, the keyframe index (the presentation times of the sync samples, where
//! a frame-accurate seek may start decoding) and the composition offsets (pts − dts per sample, the B-frame
//! reorder). Read from the one parse playback uses ([`crate::Demuxer::open`]) and the one sample table its
//! seek searches — no second parser, no second table. The editor's two reads are
//! [`crate::Demuxer::keyframe_before`] and [`crate::Demuxer::samples_from`]. Pure.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{Codec, DecodeTime, Demuxer, EditSegment, Error, Format, Timebase, TrackKind};

/// One stream of a [`Survey`]. Times are in the stream's [`Timebase`] ticks unless named `_ns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSurvey {
    /// Index into [`Demuxer::tracks`] (the `track_index` the editor's reads take).
    pub track_index: usize,
    /// The container's own id (MP4 `track_ID`, Matroska `TrackNumber`).
    pub id: u32,
    pub kind: TrackKind,
    pub codec: Codec,
    /// The raw codec name: an MP4 sample-entry fourcc or a Matroska CodecID.
    pub codec_name: String,
    pub timebase: Timebase,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// The earliest presentation time over the stream's samples (0 for an empty stream).
    pub first_pts: i64,
    /// The stream's presentation span in ns ([`crate::Track::duration_ns`]).
    pub duration_ns: u64,
    /// Samples (frames / access units) the file holds for the stream.
    pub frame_count: u64,
    /// The presentation times of the sync samples, ascending — the keyframe index.
    pub keyframes: Vec<i64>,
    /// pts − dts of every sample, in decode order (MP4 `ctts` / `trun` less the edit shift; in Matroska 0 while
    /// [`StreamSurvey::decode_time`] is `Presentation`, the derived reorder once it is `Derived`).
    pub composition_offsets: Vec<i64>,
    /// The samples say decode order is not presentation order: some pts − dts is non-zero, or pts is not
    /// monotone in decode order. Read from the samples, never from the codec name.
    pub reordered: bool,
    /// DEMUXSEEK (SR200): where the decode times come from — `Presentation` says dts = pts (a Matroska track no reorder
    /// depth was given for), so the composition offsets carry no reorder fact.
    pub decode_time: DecodeTime,
    /// DEMUXSEEK (SR200): the file was opened by its index ([`Demuxer::open_indexed`]): `keyframes` are the index's points
    /// for this stream, and `frame_count`, `first_pts`, `composition_offsets` and `reordered` cover the loaded spans only.
    pub indexed: bool,
    /// DEMUXSEEK (SR200): the stream's edit list, every entry ([`Demuxer::edit_list`]).
    pub edits: Vec<EditSegment>,
    /// NLEPROBEHDR (SR226): the stream's colour and HDR metadata ([`crate::Track::colour`]); its `Display` is the probe's token.
    pub colour: Option<crate::colour::Colour>,
}

/// The editor's probe of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    pub format: Format,
    /// The container's declared duration, else the longest stream ([`Demuxer::duration_ns`]).
    pub duration_ns: u64,
    pub streams: Vec<StreamSurvey>,
}

/// The survey of a whole container file (`Err` when it is neither ISO-BMFF nor EBML, or will not parse).
pub fn survey_of(bytes: &[u8]) -> Result<Survey, Error> {
    Ok(Demuxer::open(bytes.to_vec())?.survey())
}

impl Demuxer {
    /// NLEPROBE (SR195): the editor's probe of this file — one [`StreamSurvey`] per track, from the sample table.
    pub fn survey(&self) -> Survey {
        let streams = self
            .tracks()
            .iter()
            .enumerate()
            .map(|(ti, t)| {
                let mut keyframes = Vec::new();
                let mut composition_offsets = Vec::new();
                let mut first_pts: Option<i64> = None;
                let mut prev_pts: Option<i64> = None;
                let mut reordered = false;
                for s in self.track_samples(ti) {
                    if s.keyframe {
                        keyframes.push(s.pts);
                    }
                    let off = s.pts - s.dts;
                    composition_offsets.push(off);
                    if off != 0 || prev_pts.is_some_and(|p| s.pts < p) {
                        reordered = true;
                    }
                    prev_pts = Some(s.pts);
                    first_pts = Some(first_pts.map_or(s.pts, |f| f.min(s.pts)));
                }
                let indexed = self.indexed();
                if indexed {
                    keyframes = self.index().iter().filter(|x| x.track_index == ti).map(|x| t.timebase.from_ns(x.pts_ns)).collect();
                }
                keyframes.sort_unstable();
                StreamSurvey {
                    track_index: ti,
                    id: t.id,
                    kind: t.kind,
                    codec: t.codec.clone(),
                    codec_name: t.codec_name.clone(),
                    timebase: t.timebase,
                    width: t.width,
                    height: t.height,
                    sample_rate: t.sample_rate,
                    channels: t.channels,
                    first_pts: first_pts.unwrap_or(0),
                    duration_ns: t.duration_ns,
                    frame_count: if self.indexed() { self.track_samples(ti).count() as u64 } else { t.sample_count },
                    keyframes,
                    composition_offsets,
                    reordered,
                    decode_time: self.decode_time.get(ti).copied().unwrap_or(DecodeTime::Presentation),
                    indexed,
                    edits: self.edit_list(ti).to_vec(), colour: t.colour,
                }
            })
            .collect();
        Survey { format: self.format(), duration_ns: self.duration_ns(), streams }
    }
}
