// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! MP4/M4A (ISO/IEC 14496-12, -14) through `demux_core`, the one ISOBMFF parser both rings link — MP4ONE
//! (rmbp-ledger B464, R79: a core depending on a core is the seam). `demux_core` builds the sample tables
//! (`stsc`/`stco`/`co64`/`stsz`/`stz2`, fragmented `moof/traf/trun`), reads the `esds`, the edit list and Apple's
//! `iTunSMPB`; this file picks the first AAC or MP3 audio track — a video track beside it is ignored — and hands
//! its access units to [`crate::aac::AacSource`] or [`crate::mp3::Mp3Stream`]. The gapless trim is computed here
//! in the codec's sample units from the file's raw numbers (`Track::edit`, `Track::smpb`), as the retired
//! `audio_core::mp4` reader did, so the decode is byte-identical to it (`tests/mp4one_kat.rs`).
//!
//! A file cut short plays to its cut (`Demuxer::open_partial`). The file is read whole (the `moov` may follow
//! the `mdat`).
use alloc::boxed::Box;
use alloc::vec::Vec;

use demux_core::{Codec as DCodec, Demuxer, TrackKind};

use crate::io::ByteStream;
use crate::{Error, Result, Source};

fn err(e: demux_core::Error) -> Error {
    match e {
        demux_core::Error::Invalid(m) => Error::Invalid(m),
        demux_core::Error::Unsupported(m) => Error::Unsupported(m),
        demux_core::Error::Truncated => Error::Invalid("mp4: truncated container"),
        demux_core::Error::UnknownFormat => Error::Invalid("mp4: not an ISOBMFF file"),
    }
}

pub fn open(mut s: ByteStream) -> Result<Box<dyn Source>> {
    open_demuxed(Demuxer::open_partial(s.rest()?).map_err(err)?)
}

/// The first AAC or MP3 audio track of an opened MP4 as a [`Source`] (a player that already demuxed the file
/// hands its `Demuxer` over; the file's bytes move, they are not copied).
pub fn open_demuxed(d: Demuxer) -> Result<Box<dyn Source>> {
    let audio = |c: &DCodec| matches!(c, DCodec::Aac | DCodec::Mp3);
    let idx = d
        .tracks()
        .iter()
        .position(|t| t.kind == TrackKind::Audio && audio(&t.codec))
        .ok_or_else(|| match d.audio_track() {
            Some(_) => Error::Unsupported("mp4: audio codec (owed: only AAC and MP3 in MP4)"),
            None => Error::Unsupported("mp4: no audio track"),
        })?;
    let t = d.tracks()[idx].clone();
    let units: Vec<(usize, usize)> = d.track_samples(idx).map(|s| (s.offset as usize, s.size as usize)).collect();
    if units.is_empty() {
        return Err(Error::Invalid("mp4: no samples"));
    }
    let data = d.into_data();
    if t.codec == DCodec::Mp3 {
        let mut v = Vec::new();
        for &(o, n) in &units {
            if let Some(b) = data.get(o..o + n) { v.extend_from_slice(b); }
        }
        let bs = ByteStream::new(Box::new(crate::io::VecReader::new(v)));
        return Ok(Box::new(crate::mp3::Mp3Stream::new(bs)?));
    }
    let asc = if t.config.is_empty() {
        // MPEG-2 AAC without a DecoderSpecificInfo: synthesise LC at the sample entry's rate
        let sfi = crate::aac::RATES.iter().position(|&r| r == t.sample_rate).unwrap_or(4) as u16;
        let v: u16 = (2 << 11) | (sfi << 7) | ((t.channels & 15) << 3);
        v.to_be_bytes().to_vec()
    } else {
        t.config.clone()
    };
    let a = crate::aac::Asc::parse(&asc)?;
    let rate = crate::aac::RATES[a.sf_index] as u64;
    let ts = t.timebase.den.max(1);
    let (mut skip, mut total) = (0u64, None);
    if let Some(e) = t.edit.filter(|e| e.media_time >= 0) {
        let (dur, mt) = (e.segment, e.media_time as u64);
        if mt > 0 || dur > 0 {
            skip = (mt * rate + ts / 2) / ts;
            let mts = e.movie_timescale as u64;
            if dur > 0 && mts != 0 { total = Some((dur * rate + mts / 2) / mts); }
        }
    }
    if skip == 0 {
        if let Some((pri, tot)) = t.smpb {
            skip = pri;
            if tot > 0 { total = Some(tot); }
        }
    }
    Ok(crate::aac::AacSource::new(data, units, &asc, skip, total)?.boxed())
}
