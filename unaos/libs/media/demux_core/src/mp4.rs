// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ISOBMFF / MP4 demuxing (ISO/IEC 14496-12), progressive and fragmented.
//!
//! Section map: box header §4.2 (32-bit size, `size == 1` → 64-bit largesize, `size == 0` → to the
//! end of the enclosing container, `uuid` extended type); `mvhd` §8.2.2; `tkhd` §8.3.2; `elst`
//! §8.6.6; `mdhd` §8.4.2; `hdlr` §8.4.3; `stsd` §8.5.2 with VisualSampleEntry/AudioSampleEntry
//! §12.1.3/§12.2.3; `stts` §8.6.1.2; `ctts` §8.6.1.3; `stss` §8.6.2; `stsz`/`stz2` §8.7.3;
//! `stsc` §8.7.4; `stco`/`co64` §8.7.5; `mehd` §8.8.2; `trex` §8.8.3; `tfhd` §8.8.7; `trun`
//! §8.8.8; `tfdt` §8.8.12; sample flags §8.8.3.1.
//!
//! Timing: `dts` accumulates `stts` deltas (or a fragment's `tfdt` + `trun` durations); `pts =
//! dts + ctts − shift`, where `shift` comes from the edit list: the first non-empty edit's
//! `media_time` is subtracted and any leading empty edits (`media_time == −1`) add their
//! duration, converted from the movie timescale to the media timescale. Only that common
//! "one optional empty edit + one media edit" shape is honoured; dwell edits (rate 0) and
//! multi-segment edit lists are the ceiling.

use alloc::string::String;
use alloc::vec::Vec;

use crate::read::Reader;
use crate::{Codec, Error, Format, Parsed, Sample, Timebase, Track, TrackKind};

/// One box: its type and where its payload lies in the file.
#[derive(Debug, Clone, Copy)]
pub struct BoxHeader {
    pub kind: [u8; 4],
    pub start: usize,
    pub body: usize,
    pub end: usize,
}

/// Iterate the boxes laid end to end in `data[from..to]`.
pub fn boxes(data: &[u8], from: usize, to: usize) -> Result<Vec<BoxHeader>, Error> {
    let mut out = Vec::new();
    let mut p = from;
    while p < to {
        if to - p < 8 {
            // Trailing padding shorter than a header is tolerated at top level only.
            break;
        }
        let mut r = Reader::new(&data[p..to]);
        let size32 = r.u32()?;
        let kind = r.fourcc()?;
        let (size, mut hdr) = match size32 {
            0 => ((to - p) as u64, 8usize),
            1 => (r.u64()?, 16usize),
            n => (n as u64, 8usize),
        };
        if &kind == b"uuid" {
            r.skip(16)?;
            hdr += 16;
        }
        if size < hdr as u64 || size > (to - p) as u64 {
            return Err(Error::Invalid("box size outside its parent"));
        }
        let end = p + size as usize;
        out.push(BoxHeader { kind, start: p, body: p + hdr, end });
        p = end;
    }
    Ok(out)
}

fn child<'a>(list: &'a [BoxHeader], kind: &[u8; 4]) -> Option<&'a BoxHeader> {
    list.iter().find(|b| &b.kind == kind)
}

fn children(data: &[u8], b: &BoxHeader) -> Result<Vec<BoxHeader>, Error> {
    boxes(data, b.body, b.end)
}

fn body<'a>(data: &'a [u8], b: &BoxHeader) -> Reader<'a> {
    Reader::new(&data[b.body..b.end])
}

/// Full box: version byte + 24-bit flags.
fn full(r: &mut Reader) -> Result<(u8, u32), Error> {
    let v = r.u8()?;
    let f = r.u24()?;
    Ok((v, f))
}

#[derive(Default, Clone, Copy)]
struct Trex {
    track_id: u32,
    sdi: u32,
    duration: u32,
    size: u32,
    flags: u32,
}

struct TrakState {
    track: Track,
    /// pts shift in media timescale (subtracted).
    shift: i64,
    samples: Vec<Sample>,
    /// Next decode time for a fragment with no `tfdt`.
    next_dts: i64,
}

const NON_SYNC: u32 = 0x0001_0000;

pub(crate) fn parse(data: &[u8]) -> Result<Parsed, Error> {
    let top = boxes(data, 0, data.len())?;
    let moov = child(&top, b"moov").ok_or(Error::Invalid("no moov box"))?;
    let moov_kids = children(data, moov)?;

    let mvhd = child(&moov_kids, b"mvhd").ok_or(Error::Invalid("no mvhd box"))?;
    let mut r = body(data, mvhd);
    let (v, _) = full(&mut r)?;
    let (movie_ts, movie_dur) = if v == 1 {
        r.skip(16)?;
        (r.u32()?, r.u64()?)
    } else {
        r.skip(8)?;
        (r.u32()?, r.u32()? as u64)
    };
    if movie_ts == 0 {
        return Err(Error::Invalid("mvhd timescale is zero"));
    }

    let mut trex: Vec<Trex> = Vec::new();
    let mut fragment_dur: Option<u64> = None;
    if let Some(mvex) = child(&moov_kids, b"mvex") {
        for b in children(data, mvex)? {
            let mut r = body(data, &b);
            match &b.kind {
                b"trex" => {
                    full(&mut r)?;
                    trex.push(Trex {
                        track_id: r.u32()?,
                        sdi: r.u32()?,
                        duration: r.u32()?,
                        size: r.u32()?,
                        flags: r.u32()?,
                    });
                }
                b"mehd" => {
                    let (v, _) = full(&mut r)?;
                    fragment_dur = Some(if v == 1 { r.u64()? } else { r.u32()? as u64 });
                }
                _ => {}
            }
        }
    }

    let mut traks: Vec<TrakState> = Vec::new();
    for b in moov_kids.iter().filter(|b| &b.kind == b"trak") {
        traks.push(parse_trak(data, b, movie_ts)?);
    }

    // Fragments, in file order.
    for m in top.iter().filter(|b| &b.kind == b"moof") {
        parse_moof(data, m, &mut traks, &trex)?;
    }

    let declared = match fragment_dur {
        Some(d) if d > 0 => Some(d),
        _ if movie_dur > 0 && movie_dur != u32::MAX as u64 && movie_dur != u64::MAX => Some(movie_dur),
        _ => None,
    }
    .map(|d| Timebase { num: 1, den: movie_ts as u64 }.to_ns(d as i64) as u64);

    let mut tracks = Vec::new();
    let mut samples = Vec::new();
    for (i, mut t) in traks.into_iter().enumerate() {
        for s in &mut t.samples {
            s.track_index = i;
        }
        tracks.push(t.track);
        samples.push(t.samples);
    }
    Ok(Parsed { format: Format::Mp4, tracks, samples, declared_duration_ns: declared })
}

fn parse_trak(data: &[u8], trak: &BoxHeader, movie_ts: u32) -> Result<TrakState, Error> {
    let kids = children(data, trak)?;
    let tkhd = child(&kids, b"tkhd").ok_or(Error::Invalid("trak without tkhd"))?;
    let mut r = body(data, tkhd);
    let (v, _) = full(&mut r)?;
    r.skip(if v == 1 { 16 } else { 8 })?;
    let track_id = r.u32()?;

    let mdia = child(&kids, b"mdia").ok_or(Error::Invalid("trak without mdia"))?;
    let mdia_kids = children(data, mdia)?;
    let mdhd = child(&mdia_kids, b"mdhd").ok_or(Error::Invalid("mdia without mdhd"))?;
    let mut r = body(data, mdhd);
    let (v, _) = full(&mut r)?;
    r.skip(if v == 1 { 16 } else { 8 })?;
    let timescale = r.u32()?;
    if timescale == 0 {
        return Err(Error::Invalid("mdhd timescale is zero"));
    }

    let kind = match child(&mdia_kids, b"hdlr") {
        Some(h) => {
            let mut r = body(data, h);
            full(&mut r)?;
            r.skip(4)?;
            match &r.fourcc()? {
                b"vide" => TrackKind::Video,
                b"soun" => TrackKind::Audio,
                _ => TrackKind::Other,
            }
        }
        None => TrackKind::Other,
    };

    // Edit list → presentation shift (media timescale).
    let mut shift = 0i64;
    if let Some(edts) = child(&kids, b"edts") {
        if let Some(elst) = child(&children(data, edts)?, b"elst") {
            let mut r = body(data, elst);
            let (v, _) = full(&mut r)?;
            let n = r.u32()?;
            let mut empty_movie = 0i64;
            for _ in 0..n {
                let (dur, media_time) =
                    if v == 1 { (r.u64()? as i64, r.i64()?) } else { (r.u32()? as i64, r.i32()? as i64) };
                r.skip(4)?; // media_rate_integer + fraction
                if media_time == -1 {
                    empty_movie += dur;
                } else {
                    let empty_media = Timebase { num: 1, den: movie_ts as u64 }
                        .to_ns(empty_movie);
                    let empty_media = Timebase { num: 1, den: timescale as u64 }.from_ns(empty_media);
                    shift = media_time - empty_media;
                    break;
                }
            }
        }
    }

    let mut track = Track {
        id: track_id,
        kind,
        codec: Codec::Other(String::new()),
        codec_name: String::new(),
        config: Vec::new(),
        timebase: Timebase { num: 1, den: timescale as u64 },
        width: 0,
        height: 0,
        sample_rate: 0,
        channels: 0,
        codec_delay_ns: 0,
        sample_count: 0,
        duration_ns: 0,
        frame_prefix: Vec::new(),
    };

    let minf = child(&mdia_kids, b"minf").ok_or(Error::Invalid("mdia without minf"))?;
    let stbl_box = *child(&children(data, minf)?, b"stbl").ok_or(Error::Invalid("minf without stbl"))?;
    let stbl = children(data, &stbl_box)?;

    if let Some(stsd) = child(&stbl, b"stsd") {
        parse_stsd(data, stsd, &mut track)?;
    }

    let samples = build_table(data, &stbl, shift)?;
    let next_dts = samples.last().map(|s| s.dts + s.duration as i64).unwrap_or(0);
    Ok(TrakState { track, shift, samples, next_dts })
}

fn parse_stsd(data: &[u8], stsd: &BoxHeader, track: &mut Track) -> Result<(), Error> {
    let mut r = body(data, stsd);
    full(&mut r)?;
    let count = r.u32()?;
    if count == 0 {
        return Ok(());
    }
    // The first sample entry describes the stream (multiple entries are the ceiling).
    let entries = boxes(data, stsd.body + 8, stsd.end)?;
    let e = entries.first().ok_or(Error::Invalid("stsd entry missing"))?;
    let fourcc = e.kind;
    track.codec_name = String::from_utf8_lossy_fourcc(&fourcc);
    let mut r = body(data, e);
    r.skip(6)?; // reserved
    r.skip(2)?; // data_reference_index
    let child_start;
    match &fourcc {
        b"av01" | b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"vp08" | b"vp09" | b"utp1" | b"encv" => {
            r.skip(16)?; // pre_defined, reserved, pre_defined[3]
            track.width = r.u16()? as u32;
            track.height = r.u16()? as u32;
            // horiz/vert resolution 8, reserved 4, frame_count 2, compressorname 32, depth 2,
            // pre_defined 2  → VisualSampleEntry is 78 bytes after the box header.
            child_start = e.body + 78;
        }
        b"mp4a" | b"Opus" | b"fLaC" | b"enca" | b"ac-3" | b"ec-3" => {
            let version = r.u16()?;
            r.skip(6)?; // revision, vendor
            track.channels = r.u16()?;
            r.skip(2)?; // samplesize
            r.skip(4)?; // pre_defined, reserved
            track.sample_rate = r.u32()? >> 16;
            // QuickTime sound description v1/v2 extensions.
            child_start = e.body + 28 + match version { 1 => 16, 2 => 36, _ => 0 };
        }
        _ => child_start = e.end,
    }
    let kids = if child_start < e.end { boxes(data, child_start, e.end)? } else { Vec::new() };
    let cfg = |k: &[u8; 4]| child(&kids, k).map(|b| data[b.body..b.end].to_vec());
    track.codec = match &fourcc {
        b"av01" => {
            track.config = cfg(b"av1C").unwrap_or_default();
            Codec::Av1
        }
        b"avc1" | b"avc3" => {
            track.config = cfg(b"avcC").unwrap_or_default();
            Codec::Avc
        }
        b"hvc1" | b"hev1" => {
            track.config = cfg(b"hvcC").unwrap_or_default();
            Codec::Hevc
        }
        b"vp09" => {
            track.config = cfg(b"vpcC").unwrap_or_default();
            Codec::Vp9
        }
        b"vp08" => {
            track.config = cfg(b"vpcC").unwrap_or_default();
            Codec::Vp8
        }
        b"utp1" => {
            track.config = cfg(b"utpC").unwrap_or_default();
            Codec::TestPattern
        }
        b"Opus" => {
            let c = cfg(b"dOps").unwrap_or_default();
            // dOps: Version(1) OutputChannelCount(1) PreSkip(2) InputSampleRate(4) ...; pre-skip
            // is in 48 kHz samples.
            if c.len() >= 4 {
                let pre = u16::from_be_bytes([c[2], c[3]]) as u64;
                track.codec_delay_ns = pre * 1_000_000_000 / 48_000;
            }
            track.config = c;
            Codec::Opus
        }
        b"fLaC" => {
            track.config = cfg(b"dfLa").unwrap_or_default();
            Codec::Flac
        }
        b"mp4a" => match child(&kids, b"esds") {
            Some(b) => {
                let (oti, dsi) = parse_esds(&data[b.body..b.end])?;
                track.config = dsi;
                match oti {
                    0x40 | 0x66 | 0x67 | 0x68 => Codec::Aac,
                    0xDD => Codec::Vorbis,
                    0xAD => Codec::Opus,
                    _ => Codec::Other(String::from("mp4a")),
                }
            }
            None => Codec::Other(String::from("mp4a")),
        },
        _ => Codec::Other(track.codec_name.clone()),
    };
    Ok(())
}

trait FourccString {
    fn from_utf8_lossy_fourcc(f: &[u8; 4]) -> String;
}
impl FourccString for String {
    fn from_utf8_lossy_fourcc(f: &[u8; 4]) -> String {
        f.iter().map(|&c| if c.is_ascii_graphic() || c == b' ' { c as char } else { '?' }).collect()
    }
}

/// ES_Descriptor (ISO/IEC 14496-1 §7.2.6.5) → (objectTypeIndication, DecoderSpecificInfo bytes).
fn parse_esds(b: &[u8]) -> Result<(u8, Vec<u8>), Error> {
    let mut r = Reader::new(b);
    full(&mut r)?;
    fn desc(r: &mut Reader) -> Result<(u8, usize), Error> {
        let tag = r.u8()?;
        let mut len = 0usize;
        for _ in 0..4 {
            let c = r.u8()?;
            len = (len << 7) | (c & 0x7F) as usize;
            if c & 0x80 == 0 {
                break;
            }
        }
        Ok((tag, len))
    }
    let (tag, _) = desc(&mut r)?;
    if tag != 0x03 {
        return Err(Error::Invalid("esds without ES_Descriptor"));
    }
    r.skip(2)?; // ES_ID
    let fl = r.u8()?;
    if fl & 0x80 != 0 {
        r.skip(2)?;
    }
    if fl & 0x40 != 0 {
        let n = r.u8()? as usize;
        r.skip(n)?;
    }
    if fl & 0x20 != 0 {
        r.skip(2)?;
    }
    let (tag, _) = desc(&mut r)?;
    if tag != 0x04 {
        return Err(Error::Invalid("esds without DecoderConfigDescriptor"));
    }
    let oti = r.u8()?;
    r.skip(12)?; // streamType.., bufferSizeDB, maxBitrate, avgBitrate
    let mut dsi = Vec::new();
    if !r.is_empty() {
        let (tag, len) = desc(&mut r)?;
        if tag == 0x05 {
            dsi = r.bytes(len.min(r.remaining()))?.to_vec();
        }
    }
    Ok((oti, dsi))
}

/// Expand stts/ctts/stsc/stsz/stco/stss into a decode-order sample table.
fn build_table(data: &[u8], stbl: &[BoxHeader], shift: i64) -> Result<Vec<Sample>, Error> {
    // Sizes.
    let mut sizes: Vec<u32> = Vec::new();
    if let Some(b) = child(stbl, b"stsz") {
        let mut r = body(data, b);
        full(&mut r)?;
        let fixed = r.u32()?;
        let n = r.u32()? as usize;
        if fixed != 0 {
            sizes = alloc::vec![fixed; n];
        } else {
            if n > r.remaining() / 4 {
                return Err(Error::Truncated);
            }
            sizes.reserve(n);
            for _ in 0..n {
                sizes.push(r.u32()?);
            }
        }
    } else if let Some(b) = child(stbl, b"stz2") {
        let mut r = body(data, b);
        full(&mut r)?;
        r.skip(3)?;
        let field = r.u8()?;
        let n = r.u32()? as usize;
        match field {
            4 => {
                let mut i = 0;
                while i < n {
                    let byte = r.u8()?;
                    sizes.push((byte >> 4) as u32);
                    if i + 1 < n {
                        sizes.push((byte & 0xF) as u32);
                    }
                    i += 2;
                }
            }
            8 => {
                for _ in 0..n {
                    sizes.push(r.u8()? as u32);
                }
            }
            16 => {
                for _ in 0..n {
                    sizes.push(r.u16()? as u32);
                }
            }
            _ => return Err(Error::Invalid("stz2 field size")),
        }
    }
    let n = sizes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    // Chunk offsets.
    let mut chunks: Vec<u64> = Vec::new();
    if let Some(b) = child(stbl, b"stco") {
        let mut r = body(data, b);
        full(&mut r)?;
        let c = r.u32()? as usize;
        if c > r.remaining() / 4 {
            return Err(Error::Truncated);
        }
        for _ in 0..c {
            chunks.push(r.u32()? as u64);
        }
    } else if let Some(b) = child(stbl, b"co64") {
        let mut r = body(data, b);
        full(&mut r)?;
        let c = r.u32()? as usize;
        if c > r.remaining() / 8 {
            return Err(Error::Truncated);
        }
        for _ in 0..c {
            chunks.push(r.u64()?);
        }
    } else {
        return Err(Error::Invalid("stbl without stco/co64"));
    }

    // Sample-to-chunk runs.
    let mut stsc: Vec<(u32, u32)> = Vec::new();
    if let Some(b) = child(stbl, b"stsc") {
        let mut r = body(data, b);
        full(&mut r)?;
        let c = r.u32()?;
        for _ in 0..c {
            let first = r.u32()?;
            let per = r.u32()?;
            r.skip(4)?;
            stsc.push((first, per));
        }
    }
    if stsc.is_empty() {
        return Err(Error::Invalid("stbl without stsc entries"));
    }

    let mut out: Vec<Sample> = Vec::with_capacity(n);
    let mut si = 0usize;
    'chunks: for (ci, &off) in chunks.iter().enumerate() {
        let chunk_no = ci as u32 + 1;
        let run = stsc.iter().rev().find(|(first, _)| *first <= chunk_no).map(|r| r.1).unwrap_or(0);
        let mut o = off;
        for _ in 0..run {
            if si >= n {
                break 'chunks;
            }
            out.push(Sample {
                track_index: 0,
                offset: o,
                size: sizes[si],
                pts: 0,
                dts: 0,
                duration: 0,
                keyframe: true,
            });
            o += sizes[si] as u64;
            si += 1;
        }
    }
    if out.len() != n {
        return Err(Error::Invalid("stsc/stco describe fewer samples than stsz"));
    }

    // Decode times.
    let stts = child(stbl, b"stts").ok_or(Error::Invalid("stbl without stts"))?;
    let mut r = body(data, stts);
    full(&mut r)?;
    let c = r.u32()?;
    let mut i = 0usize;
    let mut dts = 0i64;
    for _ in 0..c {
        let count = r.u32()?;
        let delta = r.u32()?;
        for _ in 0..count {
            if i >= n {
                break;
            }
            out[i].dts = dts;
            out[i].duration = delta as u64;
            dts += delta as i64;
            i += 1;
        }
    }
    while i < n {
        // stts shorter than stsz: repeat the last delta (lenient, as players are).
        let d = if i > 0 { out[i - 1].duration } else { 0 };
        out[i].dts = dts;
        out[i].duration = d;
        dts += d as i64;
        i += 1;
    }

    // Composition offsets.
    let mut cto = alloc::vec![0i64; n];
    if let Some(b) = child(stbl, b"ctts") {
        let mut r = body(data, b);
        let (v, _) = full(&mut r)?;
        let c = r.u32()?;
        let mut i = 0usize;
        for _ in 0..c {
            let count = r.u32()?;
            let raw = r.u32()?;
            let off = if v == 1 { raw as i32 as i64 } else { raw as i64 };
            for _ in 0..count {
                if i < n {
                    cto[i] = off;
                }
                i += 1;
            }
        }
    }
    for (s, c) in out.iter_mut().zip(cto) {
        s.pts = s.dts + c - shift;
    }

    // Sync samples.
    if let Some(b) = child(stbl, b"stss") {
        for s in out.iter_mut() {
            s.keyframe = false;
        }
        let mut r = body(data, b);
        full(&mut r)?;
        let c = r.u32()?;
        for _ in 0..c {
            let k = r.u32()? as usize;
            if k >= 1 && k <= n {
                out[k - 1].keyframe = true;
            }
        }
    }
    Ok(out)
}

fn parse_moof(data: &[u8], moof: &BoxHeader, traks: &mut [TrakState], trex: &[Trex]) -> Result<(), Error> {
    let kids = children(data, moof)?;
    let mut prev_traf_end: Option<u64> = None;
    for traf in kids.iter().filter(|b| &b.kind == b"traf") {
        let tk = children(data, traf)?;
        let tfhd = child(&tk, b"tfhd").ok_or(Error::Invalid("traf without tfhd"))?;
        let mut r = body(data, tfhd);
        let (_, fl) = full(&mut r)?;
        let track_id = r.u32()?;
        let base_data_offset = if fl & 0x1 != 0 { Some(r.u64()?) } else { None };
        let ex = trex.iter().find(|t| t.track_id == track_id).copied().unwrap_or_default();
        let _sdi = if fl & 0x2 != 0 { r.u32()? } else { ex.sdi };
        let def_dur = if fl & 0x8 != 0 { r.u32()? } else { ex.duration };
        let def_size = if fl & 0x10 != 0 { r.u32()? } else { ex.size };
        let def_flags = if fl & 0x20 != 0 { r.u32()? } else { ex.flags };
        let default_base_is_moof = fl & 0x2_0000 != 0;

        let Some(ti) = traks.iter().position(|t| t.track.id == track_id) else {
            continue; // a traf for a track moov does not declare: skip it
        };
        let base = match base_data_offset {
            Some(b) => b,
            None if default_base_is_moof => moof.start as u64,
            None => prev_traf_end.unwrap_or(moof.start as u64),
        };

        let mut dts = traks[ti].next_dts;
        if let Some(t) = child(&tk, b"tfdt") {
            let mut r = body(data, t);
            let (v, _) = full(&mut r)?;
            dts = if v == 1 { r.u64()? as i64 } else { r.u32()? as i64 };
        }

        let is_video = traks[ti].track.kind == TrackKind::Video;
        let shift = traks[ti].shift;
        let mut cursor = base;
        for trun in tk.iter().filter(|b| &b.kind == b"trun") {
            let mut r = body(data, trun);
            let (v, tf) = full(&mut r)?;
            let count = r.u32()?;
            if tf & 0x1 != 0 {
                let off = r.i32()? as i64;
                cursor = (base as i64 + off).max(0) as u64;
            }
            let first_flags = if tf & 0x4 != 0 { Some(r.u32()?) } else { None };
            let per = [0x100, 0x200, 0x400, 0x800].iter().filter(|&&b| tf & b != 0).count();
            if (count as usize).saturating_mul(per * 4) > r.remaining() {
                return Err(Error::Truncated);
            }
            for k in 0..count {
                let dur = if tf & 0x100 != 0 { r.u32()? } else { def_dur };
                let size = if tf & 0x200 != 0 { r.u32()? } else { def_size };
                let flags = if tf & 0x400 != 0 {
                    r.u32()?
                } else if k == 0 {
                    first_flags.unwrap_or(def_flags)
                } else {
                    def_flags
                };
                let cto = if tf & 0x800 != 0 {
                    let raw = r.u32()?;
                    if v == 1 { raw as i32 as i64 } else { raw as i64 }
                } else {
                    0
                };
                traks[ti].samples.push(Sample {
                    track_index: 0,
                    offset: cursor,
                    size,
                    pts: dts + cto - shift,
                    dts,
                    duration: dur as u64,
                    keyframe: !is_video || flags & NON_SYNC == 0,
                });
                cursor += size as u64;
                dts += dur as i64;
            }
        }
        traks[ti].next_dts = dts;
        prev_traf_end = Some(cursor);
    }
    Ok(())
}
