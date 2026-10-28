// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Matroska / WebM demuxing (RFC 9559, Matroska Media Container; EBML per RFC 8794).
//!
//! Section map: EBML variable-size integers RFC 8794 §4 (element ID keeps its marker bit, data
//! size strips it, all-ones = unknown size §6.2); EBML header §11.2 with DocType; Segment §5.1;
//! Info §5.1.2 (TimestampScale 0x2AD7B1, Duration 0x4489); Tracks §5.1.4 (TrackEntry 0xAE:
//! TrackNumber 0xD7, TrackType 0x83, CodecID 0x86, CodecPrivate 0x63A2, DefaultDuration
//! 0x23E383, CodecDelay 0x56AA, Video 0xE0 → PixelWidth 0xB0 / PixelHeight 0xBA / Colour 0x55B0, Audio 0xE1 →
//! SamplingFrequency 0xB5 / Channels 0x9F, ContentEncodings 0x6D80); Cluster §5.1.3 (Timestamp
//! 0xE7, SimpleBlock 0xA3, BlockGroup 0xA0 → Block 0xA1, BlockDuration 0x9B, ReferenceBlock
//! 0xFB); Block structure §10 and lacing §10.3 (Xiph, fixed-size, EBML); unknown-size Cluster
//! termination by the next top-level element §6.2.
//!
//! Timing: every Matroska track here uses a nanosecond timebase. A block's time is
//! `(Cluster.Timestamp + Block.relative) × TimestampScale`; frame k of a laced block adds
//! `k × DefaultDuration`. Matroska stores no decode timestamps — blocks are in decode order — so
//! `dts = pts` (exact for AV1/VP8/VP9/Opus/Vorbis) until the caller hands
//! [`crate::Demuxer::derive_dts`] the codec's reorder depth (DEMUXSEEK SR200: [`derive_dts`] below). A
//! frame's duration is BlockDuration, else DefaultDuration, else the gap to the track's next frame in
//! PRESENTATION order ([`fill_durations`]), else (last frame) the remainder of the declared segment
//! duration.
//!
//! DEMUXSEEK (SR200): SeekHead 0x114D9B74 (Seek 0x4DBB → SeekID 0x53AB, SeekPosition 0x53AC) and Cues
//! 0x1C53BB6B (CuePoint 0xBB → CueTime 0xB3, CueTrackPositions 0xB7 → CueTrack 0xF7,
//! CueClusterPosition 0xF1; RFC 9559 §5.1.5) are read by [`parse_index`], the open by the file's index.

use alloc::string::String;
use alloc::vec::Vec;

use crate::read::Reader;
use crate::{Codec, Error, Format, IndexPoint, IndexedOpen, Parsed, Sample, SpanReader, Timebase, Track, TrackKind};

const ID_EBML: u32 = 0x1A45_DFA3;
const ID_DOCTYPE: u32 = 0x4282;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_INFO: u32 = 0x1549_A966;
const ID_TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;
const ID_CODEC_PRIVATE: u32 = 0x63A2;
const ID_DEFAULT_DURATION: u32 = 0x23_E383;
const ID_CODEC_DELAY: u32 = 0x56AA;
const ID_VIDEO: u32 = 0xE0;
const ID_PIXEL_WIDTH: u32 = 0xB0;
const ID_PIXEL_HEIGHT: u32 = 0xBA; const ID_COLOUR: u32 = 0x55B0;
const ID_AUDIO: u32 = 0xE1;
const ID_SAMPLING_FREQ: u32 = 0xB5;
const ID_CHANNELS: u32 = 0x9F;
const ID_BIT_DEPTH: u32 = 0x6264;
const ID_CONTENT_ENCODINGS: u32 = 0x6D80;
const ID_CONTENT_ENCODING: u32 = 0x6240;
const ID_CONTENT_COMPRESSION: u32 = 0x5034;
const ID_CONTENT_COMP_ALGO: u32 = 0x4254;
const ID_CONTENT_COMP_SETTINGS: u32 = 0x4255;
const ID_CONTENT_ENCRYPTION: u32 = 0x5035;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_CLUSTER_TIMESTAMP: u32 = 0xE7;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const ID_BLOCK_DURATION: u32 = 0x9B;
const ID_REFERENCE_BLOCK: u32 = 0xFB;
/// BlockGroup DiscardPadding (signed ns; RFC 9559 §5.1.3.5.6) — AUDIOTRACK (SR45).
const ID_DISCARD_PADDING: u32 = 0x75A2;
// DEMUXSEEK (SR200): the index elements (RFC 9559 §5.1.1 SeekHead, §5.1.5 Cues).
const ID_SEEK_HEAD: u32 = 0x114D_9B74;
const ID_SEEK: u32 = 0x4DBB;
const ID_SEEK_ID: u32 = 0x53AB;
const ID_SEEK_POSITION: u32 = 0x53AC;
const ID_CUES: u32 = 0x1C53_BB6B;
const ID_CUE_POINT: u32 = 0xBB;
const ID_CUE_TIME: u32 = 0xB3;
const ID_CUE_TRACK_POSITIONS: u32 = 0xB7;
const ID_CUE_TRACK: u32 = 0xF7;
const ID_CUE_CLUSTER_POSITION: u32 = 0xF1;

/// Top-level Segment children: an unknown-size Cluster ends where one of these begins.
const SEGMENT_CHILDREN: [u32; 8] = [
    ID_CLUSTER, 0x114D_9B74, ID_INFO, ID_TRACKS, 0x1C53_BB6B, 0x1941_A469, 0x1043_A770, 0x1254_C367,
];

const UNKNOWN: u64 = u64::MAX;

/// Element ID (marker bit kept), 1..=4 bytes.
pub fn read_id(r: &mut Reader) -> Result<u32, Error> {
    let first = r.u8()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return Err(Error::Invalid("EBML ID longer than 4 bytes"));
    }
    let mut v = first as u32;
    for _ in 1..len {
        v = (v << 8) | r.u8()? as u32;
    }
    Ok(v)
}

/// Data size / vint value (marker stripped), 1..=8 bytes; all value bits set → [`UNKNOWN`].
pub fn read_vint(r: &mut Reader) -> Result<(u64, usize), Error> {
    let first = r.u8()?;
    if first == 0 {
        return Err(Error::Invalid("EBML vint longer than 8 bytes"));
    }
    let len = first.leading_zeros() as usize + 1;
    let mut v = (first as u64) & (0xFF >> len);
    let mut all_ones = v == (0xFF >> len) as u64;
    for _ in 1..len {
        let b = r.u8()?;
        all_ones &= b == 0xFF;
        v = (v << 8) | b as u64;
    }
    Ok((if all_ones { UNKNOWN } else { v }, len))
}

#[derive(Debug, Clone, Copy)]
struct El {
    id: u32,
    body: usize,
    /// `UNKNOWN`-sized elements get `end = parent end` and `unknown = true`.
    end: usize,
    unknown: bool,
}

fn read_el(data: &[u8], p: usize, limit: usize) -> Result<El, Error> {
    let mut r = Reader::new(&data[p..limit]);
    let id = read_id(&mut r)?;
    let (size, _) = read_vint(&mut r)?;
    let body = p + r.pos();
    if size == UNKNOWN {
        return Ok(El { id, body, end: limit, unknown: true });
    }
    let end = body.checked_add(size as usize).ok_or(Error::Truncated)?;
    if end > limit {
        return Err(Error::Truncated);
    }
    Ok(El { id, body, end, unknown: false })
}

fn kids(data: &[u8], from: usize, to: usize) -> Result<Vec<El>, Error> {
    let mut out = Vec::new();
    let mut p = from;
    while p < to {
        let e = read_el(data, p, to)?;
        p = e.end;
        out.push(e);
    }
    Ok(out)
}

fn uint(data: &[u8], e: &El) -> Result<u64, Error> {
    Reader::new(&data[e.body..e.end]).uint_n(e.end - e.body)
}

fn float(data: &[u8], e: &El) -> Result<f64, Error> {
    let b = &data[e.body..e.end];
    match b.len() {
        0 => Ok(0.0),
        4 => Ok(f32::from_bits(u32::from_be_bytes([b[0], b[1], b[2], b[3]])) as f64),
        8 => Ok(f64::from_bits(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))),
        _ => Err(Error::Invalid("float element must be 0, 4 or 8 bytes")),
    }
}

fn string(data: &[u8], e: &El) -> String {
    let b = &data[e.body..e.end];
    let b = match b.iter().position(|&c| c == 0) {
        Some(z) => &b[..z],
        None => b,
    };
    String::from_utf8_lossy(b).into_owned()
}

fn round_f64(x: f64) -> u64 {
    if x <= 0.0 { 0 } else { (x + 0.5) as u64 }
}

struct TrackState {
    track: Track,
    default_duration: u64,
    samples: Vec<Sample>,
    /// Which samples had an explicit duration (BlockDuration or DefaultDuration).
    has_duration: Vec<bool>,
}

/// The EBML header's DocType and the Segment element (any Void/CRC between skipped) — the start both opens share.
fn segment(data: &[u8]) -> Result<(Format, El), Error> {
    let header = read_el(data, 0, data.len())?;
    if header.id != ID_EBML {
        return Err(Error::UnknownFormat);
    }
    let mut format = Format::Matroska;
    for e in kids(data, header.body, header.end)? {
        if e.id == ID_DOCTYPE {
            match string(data, &e).as_str() {
                "webm" => format = Format::WebM,
                "matroska" => format = Format::Matroska,
                _ => return Err(Error::Unsupported("EBML DocType is neither matroska nor webm")),
            }
        }
    }
    // Find the Segment (skipping any Void/CRC elements between).
    let mut p = header.end;
    let seg = loop {
        if p >= data.len() {
            return Err(Error::Invalid("no Segment element"));
        }
        let e = read_el(data, p, data.len())?;
        if e.id == ID_SEGMENT {
            break e;
        }
        p = e.end;
    };
    Ok((format, seg))
}

/// Info: TimestampScale and Duration.
fn info(data: &[u8], e: &El, scale: &mut u64, duration_ticks: &mut Option<f64>) -> Result<(), Error> {
    for c in kids(data, e.body, e.end)? {
        match c.id {
            ID_TIMESTAMP_SCALE => *scale = uint(data, &c)?.max(1),
            ID_DURATION => *duration_ticks = Some(float(data, &c)?),
            _ => {}
        }
    }
    Ok(())
}

/// Tracks: every TrackEntry.
fn track_entries(data: &[u8], e: &El, tracks: &mut Vec<TrackState>) -> Result<(), Error> {
    for te in kids(data, e.body, e.end)? {
        if te.id == ID_TRACK_ENTRY {
            tracks.push(parse_track(data, &te)?);
        }
    }
    Ok(())
}

pub(crate) fn parse(data: &[u8]) -> Result<Parsed, Error> {
    let (format, seg) = segment(data)?;
    let mut scale: u64 = 1_000_000;
    let mut duration_ticks: Option<f64> = None;
    let mut tracks: Vec<TrackState> = Vec::new();
    let mut p = seg.body;
    while p < seg.end {
        let e = read_el(data, p, seg.end)?;
        match e.id {
            ID_INFO => {
                info(data, &e, &mut scale, &mut duration_ticks)?;
                p = e.end;
            }
            ID_TRACKS => {
                track_entries(data, &e, &mut tracks)?;
                p = e.end;
            }
            ID_CLUSTER => {
                p = parse_cluster(data, &e, seg.end, scale, &mut tracks)?;
            }
            _ => {
                if e.unknown {
                    return Err(Error::Unsupported("unknown-size element other than Segment/Cluster"));
                }
                p = e.end;
            }
        }
    }

    let declared = duration_ticks.map(|d| round_f64(d * scale as f64));

    // Fill durations a block did not carry: gap to the track's next frame in presentation order.
    let mut out_tracks = Vec::new();
    let mut out_samples = Vec::new();
    for (i, mut t) in tracks.into_iter().enumerate() {
        for s in t.samples.iter_mut() {
            s.track_index = i;
        }
        fill_durations(&mut t.samples, &t.has_duration, declared);
        out_tracks.push(t.track);
        out_samples.push(t.samples);
    }
    let edits = alloc::vec![Vec::new(); out_tracks.len()];
    Ok(Parsed { format, tracks: out_tracks, samples: out_samples, declared_duration_ns: declared, edits })
}

fn parse_track(data: &[u8], te: &El) -> Result<TrackState, Error> {
    let mut t = Track {
        id: 0,
        kind: TrackKind::Other,
        codec: Codec::Other(String::new()),
        codec_name: String::new(),
        config: Vec::new(),
        timebase: Timebase::NANOS,
        width: 0,
        height: 0,
        sample_rate: 0,
        channels: 1,
        codec_delay_ns: 0,
        sample_count: 0,
        duration_ns: 0,
        frame_prefix: Vec::new(),
        trim_start_ns: 0,
        play_ns: None,
        edit: None,
        smpb: None, colour: None,
    };
    let mut default_duration = 0u64;
    let mut sample_rate = 8000.0f64;
    let mut bit_depth = 0u16;
    for c in kids(data, te.body, te.end)? {
        match c.id {
            ID_TRACK_NUMBER => t.id = uint(data, &c)? as u32,
            ID_TRACK_TYPE => {
                t.kind = match uint(data, &c)? {
                    1 => TrackKind::Video,
                    2 => TrackKind::Audio,
                    _ => TrackKind::Other,
                }
            }
            ID_CODEC_ID => t.codec_name = string(data, &c),
            ID_CODEC_PRIVATE => t.config = data[c.body..c.end].to_vec(),
            ID_DEFAULT_DURATION => default_duration = uint(data, &c)?,
            ID_CODEC_DELAY => t.codec_delay_ns = uint(data, &c)?,
            ID_VIDEO => {
                for v in kids(data, c.body, c.end)? {
                    match v.id {
                        ID_PIXEL_WIDTH => t.width = uint(data, &v)? as u32,
                        ID_PIXEL_HEIGHT => t.height = uint(data, &v)? as u32, ID_COLOUR => t.colour = colour_master(data, &v),
                        _ => {}
                    }
                }
            }
            ID_AUDIO => {
                for a in kids(data, c.body, c.end)? {
                    match a.id {
                        ID_SAMPLING_FREQ => sample_rate = float(data, &a)?,
                        ID_CHANNELS => t.channels = uint(data, &a)? as u16,
                        ID_BIT_DEPTH => bit_depth = uint(data, &a)? as u16,
                        _ => {}
                    }
                }
            }
            ID_CONTENT_ENCODINGS => {
                for enc in kids(data, c.body, c.end)? {
                    if enc.id != ID_CONTENT_ENCODING {
                        continue;
                    }
                    for x in kids(data, enc.body, enc.end)? {
                        match x.id {
                            ID_CONTENT_ENCRYPTION => {
                                return Err(Error::Unsupported("encrypted Matroska track"));
                            }
                            ID_CONTENT_COMPRESSION => {
                                let mut algo = 0u64; // default: zlib
                                let mut settings = Vec::new();
                                for y in kids(data, x.body, x.end)? {
                                    match y.id {
                                        ID_CONTENT_COMP_ALGO => algo = uint(data, &y)?,
                                        ID_CONTENT_COMP_SETTINGS => settings = data[y.body..y.end].to_vec(),
                                        _ => {}
                                    }
                                }
                                if algo != 3 {
                                    return Err(Error::Unsupported("compressed Matroska track (only header stripping)"));
                                }
                                t.frame_prefix = settings;
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if t.kind == TrackKind::Audio {
        t.sample_rate = round_f64(sample_rate) as u32;
    }
    t.codec = match t.codec_name.as_str() {
        "V_AV1" => Codec::Av1,
        "V_VP8" => Codec::Vp8,
        "V_VP9" => Codec::Vp9,
        "V_MPEG4/ISO/AVC" => Codec::Avc,
        "V_MPEGH/ISO/HEVC" => Codec::Hevc,
        "V_UNAOS/TESTPATTERN" => Codec::TestPattern,
        "A_OPUS" => Codec::Opus,
        "A_VORBIS" => Codec::Vorbis,
        "A_FLAC" => Codec::Flac,
        "A_MPEG/L3" => Codec::Mp3,
        // RFC 9559 §5.1.4.1.28 codec mappings: PCM bit depth from Audio/BitDepth (16 if absent).
        "A_PCM/INT/LIT" => Codec::Pcm { bits: if bit_depth == 0 { 16 } else { bit_depth }, float: false, big_endian: false },
        "A_PCM/INT/BIG" => Codec::Pcm { bits: if bit_depth == 0 { 16 } else { bit_depth }, float: false, big_endian: true },
        "A_PCM/FLOAT/IEEE" => Codec::Pcm { bits: if bit_depth == 0 { 32 } else { bit_depth }, float: true, big_endian: false },
        s if s.starts_with("A_AAC") => Codec::Aac,
        s => Codec::Other(String::from(s)),
    };
    if t.id == 0 {
        return Err(Error::Invalid("TrackEntry without TrackNumber"));
    }
    Ok(TrackState { track: t, default_duration, samples: Vec::new(), has_duration: Vec::new() })
}

/// Parse one Cluster; returns where the next Segment child starts.
fn parse_cluster(data: &[u8], cl: &El, seg_end: usize, scale: u64, tracks: &mut [TrackState]) -> Result<usize, Error> {
    let mut cluster_ts: u64 = 0;
    let mut p = cl.body;
    let end = cl.end;
    while p < end {
        if cl.unknown {
            // An unknown-size Cluster ends at the next Segment-level element.
            let mut r = Reader::new(&data[p..seg_end]);
            let id = read_id(&mut r)?;
            if SEGMENT_CHILDREN.contains(&id) {
                return Ok(p);
            }
        }
        let e = read_el(data, p, end)?;
        match e.id {
            ID_CLUSTER_TIMESTAMP => cluster_ts = uint(data, &e)?,
            ID_SIMPLE_BLOCK => {
                block(data, e.body, e.end, cluster_ts, scale, None, None, tracks)?;
            }
            ID_BLOCK_GROUP => {
                let mut blk: Option<El> = None;
                let mut dur: Option<u64> = None;
                let mut referenced = false;
                let mut discard = 0u64;
                for c in kids(data, e.body, e.end)? {
                    match c.id {
                        ID_BLOCK => blk = Some(c),
                        ID_BLOCK_DURATION => dur = Some(uint(data, &c)?),
                        ID_REFERENCE_BLOCK => referenced = true,
                        ID_DISCARD_PADDING => {
                            // a signed integer, big-endian, 1-8 bytes; negative values are not
                            // meaningful for audio padding and are ignored
                            let b = &data[c.body..c.end];
                            if !b.is_empty() && b.len() <= 8 {
                                let mut v: i64 = if b[0] & 0x80 != 0 { -1 } else { 0 };
                                for &x in b {
                                    v = (v << 8) | x as i64;
                                }
                                discard = v.max(0) as u64;
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(b) = blk {
                    let first = tracks.iter().map(|t| t.samples.len()).collect::<Vec<_>>();
                    block(data, b.body, b.end, cluster_ts, scale, Some(!referenced), dur, tracks)?;
                    if discard > 0 {
                        // the padding belongs to the block's last frame
                        for (t, n0) in tracks.iter_mut().zip(first) {
                            if t.samples.len() > n0 {
                                if let Some(s) = t.samples.last_mut() {
                                    s.discard_ns = discard;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        p = e.end;
    }
    Ok(end)
}

/// One (Simple)Block: header, lacing, frames → samples. `key` is `None` for a SimpleBlock (its
/// flag byte says), `Some(..)` for a Block inside a BlockGroup.
#[allow(clippy::too_many_arguments)]
fn block(
    data: &[u8],
    from: usize,
    to: usize,
    cluster_ts: u64,
    scale: u64,
    key: Option<bool>,
    block_duration: Option<u64>,
    tracks: &mut [TrackState],
) -> Result<(), Error> {
    let mut r = Reader::new(&data[from..to]);
    let (track_no, _) = read_vint(&mut r)?;
    let rel = r.i16()? as i64;
    let flags = r.u8()?;
    let Some(ts) = tracks.iter_mut().find(|t| t.track.id as u64 == track_no) else {
        return Ok(()); // a block for an undeclared track is skipped
    };
    let keyframe = key.unwrap_or(flags & 0x80 != 0);
    let pts_ticks = cluster_ts as i64 + rel;
    let pts = pts_ticks.saturating_mul(scale as i64);

    // Frame sizes per lacing mode.
    let lacing = (flags >> 1) & 3;
    let mut sizes: Vec<usize> = Vec::new();
    if lacing == 0 {
        sizes.push(r.remaining());
    } else {
        let n = r.u8()? as usize + 1;
        match lacing {
            1 => {
                // Xiph: each of the first n−1 sizes is a run of 255s plus a final byte.
                for _ in 0..n - 1 {
                    let mut s = 0usize;
                    loop {
                        let b = r.u8()?;
                        s += b as usize;
                        if b != 255 {
                            break;
                        }
                    }
                    sizes.push(s);
                }
            }
            3 => {
                // EBML: first size a vint, then signed differences (vint − bias).
                let (first, _) = read_vint(&mut r)?;
                if first == UNKNOWN {
                    return Err(Error::Invalid("EBML lace size is unknown"));
                }
                let mut s = first as i64;
                sizes.push(s as usize);
                for _ in 1..n - 1 {
                    let (raw, len) = read_vint(&mut r)?;
                    let bias = (1i64 << (7 * len - 1)) - 1;
                    s += raw as i64 - bias;
                    if s < 0 {
                        return Err(Error::Invalid("negative EBML lace size"));
                    }
                    sizes.push(s as usize);
                }
            }
            _ => {
                // Fixed-size lacing: the rest splits evenly.
                if !r.remaining().is_multiple_of(n) {
                    return Err(Error::Invalid("fixed lacing does not divide the block"));
                }
                let each = r.remaining() / n;
                for _ in 0..n - 1 {
                    sizes.push(each);
                }
            }
        }
        let used: usize = sizes.iter().sum();
        if used > r.remaining() {
            return Err(Error::Truncated);
        }
        sizes.push(r.remaining() - used);
    }

    let n = sizes.len() as u64;
    let mut off = from + r.pos();
    let dd = ts.default_duration;
    // BlockDuration covers the whole block; split it across laced frames.
    let explicit: Option<u64> = match block_duration {
        Some(bd) => Some(bd.saturating_mul(scale) / n),
        None if dd > 0 => Some(dd),
        None => None,
    };
    for (k, &sz) in sizes.iter().enumerate() {
        let step = explicit.unwrap_or(0) as i64 * k as i64;
        ts.samples.push(Sample {
            track_index: 0,
            offset: off as u64,
            size: sz as u32,
            pts: pts + step,
            dts: pts + step,
            duration: explicit.unwrap_or(0),
            keyframe: keyframe && k == 0 || (keyframe && ts.track.kind != TrackKind::Video),
            discard_ns: 0,
            depends: 0,
        });
        ts.has_duration.push(explicit.is_some());
        off += sz;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// DEMUXSEEK (LEDGER SR200): presentation-order durations, decode times from the reorder depth, the open by Cues.
// ---------------------------------------------------------------------------------------------------

/// Fill the durations a block did not carry: the gap to the frame that follows in PRESENTATION order (indices stably
/// sorted by pts — identical to the decode-order gap on a track whose pts never fall), the last such frame the remainder of
/// the declared duration, else the previous frame's duration.
fn fill_durations(samples: &mut [Sample], has_duration: &[bool], declared: Option<u64>) {
    let n = samples.len();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by_key(|&k| samples[k].pts);
    for j in 0..n {
        let k = idx[j];
        if has_duration.get(k).copied().unwrap_or(false) {
            continue;
        }
        let pts = samples[k].pts;
        let d = if j + 1 < n {
            (samples[idx[j + 1]].pts - pts).max(0) as u64
        } else {
            match declared {
                Some(end) if end as i64 > pts => end - pts as u64,
                _ if j > 0 => samples[idx[j - 1]].duration,
                _ => 0,
            }
        };
        samples[k].duration = d;
    }
}

/// DEMUXSEEK (SR200): one track's decode times from the codec's reorder depth `depth` (see [`crate::DecodeTime`]): the pts
/// sorted ascending give `dts[i] = sorted[i - depth]`, the first `depth` extrapolated back by the smallest positive pts step.
/// With reorder depth d at least i-d+1 of the first i+1 decoded frames are presented no later than frame i (H.264 E.2.1
/// `max_num_reorder_frames`), so `sorted[i-d] <= pts[i]`. A depth the samples contradict (a dts past its pts, or a dts that
/// does not increase) changes nothing and returns false.
pub(crate) fn derive_dts(samples: &mut [Sample], depth: u32) -> bool {
    let n = samples.len();
    if n == 0 {
        return true;
    }
    let d = depth as usize;
    let mut sorted: Vec<i64> = samples.iter().map(|s| s.pts).collect();
    sorted.sort_unstable();
    let step = sorted
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&g| g > 0)
        .min()
        .unwrap_or_else(|| (samples[0].duration as i64).max(1));
    let dts: Vec<i64> = (0..n)
        .map(|i| if i >= d { sorted[i - d] } else { sorted[0].saturating_sub(((d - i) as i64).saturating_mul(step)) })
        .collect();
    if dts.iter().zip(samples.iter()).any(|(&t, s)| t > s.pts) || dts.windows(2).any(|w| w[1] <= w[0]) {
        return false;
    }
    for (s, t) in samples.iter_mut().zip(dts) {
        s.dts = t;
    }
    true
}

/// DEMUXSEEK (SR200): what a span load needs of the headers.
#[derive(Clone)]
pub(crate) struct Spans {
    seg_end: usize,
    scale: u64,
    tracks: Vec<Track>,
    default_durations: Vec<u64>,
    declared: Option<u64>,
}

/// DEMUXSEEK (SR200): the open by the file's Cues. The Segment's children up to the first Cluster are read (Info, Tracks,
/// SeekHead, Cues); Cues the head does not hold are read where the SeekHead points. Every CueClusterPosition (relative to
/// the Segment's body) must begin a Cluster. `None` when the file has no usable Cues (the caller opens it whole).
pub(crate) fn parse_index(data: &[u8]) -> Result<Option<IndexedOpen>, Error> {
    let (format, seg) = segment(data)?;
    let mut scale: u64 = 1_000_000;
    let mut duration_ticks: Option<f64> = None;
    let mut tracks: Vec<TrackState> = Vec::new();
    let mut cues: Option<El> = None;
    let mut cues_at: Option<usize> = None;
    let mut first_cluster: Option<usize> = None;
    let mut p = seg.body;
    while p < seg.end {
        let e = read_el(data, p, seg.end)?;
        match e.id {
            ID_INFO => info(data, &e, &mut scale, &mut duration_ticks)?,
            ID_TRACKS => track_entries(data, &e, &mut tracks)?,
            ID_SEEK_HEAD => {
                for sk in kids(data, e.body, e.end)?.iter().filter(|x| x.id == ID_SEEK) {
                    let (mut id, mut pos) = (0u64, None);
                    for c in kids(data, sk.body, sk.end)? {
                        match c.id {
                            ID_SEEK_ID => id = uint(data, &c)?,
                            ID_SEEK_POSITION => pos = Some(uint(data, &c)?),
                            _ => {}
                        }
                    }
                    if id == ID_CUES as u64 {
                        cues_at = pos.and_then(|x| seg.body.checked_add(x as usize));
                    }
                }
            }
            ID_CUES => cues = Some(e),
            ID_CLUSTER => {
                first_cluster = Some(p);
                break;
            }
            _ => {}
        }
        if e.unknown {
            return Ok(None);
        }
        p = e.end;
    }
    let Some(first_cluster) = first_cluster else { return Ok(None) };
    if cues.is_none() {
        if let Some(at) = cues_at.filter(|&at| at < seg.end) {
            let e = read_el(data, at, seg.end)?;
            if e.id == ID_CUES && !e.unknown {
                cues = Some(e);
            }
        }
    }
    let Some(cues) = cues else { return Ok(None) };

    let mut index: Vec<IndexPoint> = Vec::new();
    for cp in kids(data, cues.body, cues.end)?.iter().filter(|x| x.id == ID_CUE_POINT) {
        let mut time: Option<u64> = None;
        let mut at: Vec<(u64, u64)> = Vec::new();
        for c in kids(data, cp.body, cp.end)? {
            match c.id {
                ID_CUE_TIME => time = Some(uint(data, &c)?),
                ID_CUE_TRACK_POSITIONS => {
                    let (mut trk, mut pos) = (None, None);
                    for x in kids(data, c.body, c.end)? {
                        match x.id {
                            ID_CUE_TRACK => trk = Some(uint(data, &x)?),
                            ID_CUE_CLUSTER_POSITION => pos = Some(uint(data, &x)?),
                            _ => {}
                        }
                    }
                    if let (Some(t), Some(q)) = (trk, pos) {
                        at.push((t, q));
                    }
                }
                _ => {}
            }
        }
        let Some(time) = time else { continue };
        for (trk, rel) in at {
            let Some(ti) = tracks.iter().position(|t| t.track.id as u64 == trk) else { continue };
            let pos = seg.body.checked_add(rel as usize).ok_or(Error::Invalid("CueClusterPosition overflow"))?;
            if pos >= seg.end || pos < first_cluster {
                return Ok(None);
            }
            let mut r = Reader::new(&data[pos..seg.end]);
            if read_id(&mut r).ok() != Some(ID_CLUSTER) {
                return Ok(None);
            }
            index.push(IndexPoint { track_index: ti, pts_ns: (time as i64).saturating_mul(scale as i64), pos: pos as u64 });
        }
    }
    if index.is_empty() {
        return Ok(None);
    }
    let mut starts: Vec<u64> = index.iter().map(|x| x.pos).collect();
    starts.sort_unstable();
    starts.dedup();
    if starts[0] > first_cluster as u64 {
        starts.insert(0, first_cluster as u64);
    }
    let spans: Vec<(u64, u64)> =
        starts.iter().enumerate().map(|(i, &a)| (a, starts.get(i + 1).copied().unwrap_or(seg.end as u64))).collect();

    let declared = duration_ticks.map(|d| round_f64(d * scale as f64));
    let sp = Spans {
        seg_end: seg.end,
        scale,
        tracks: tracks.iter().map(|t| t.track.clone()).collect(),
        default_durations: tracks.iter().map(|t| t.default_duration).collect(),
        declared,
    };
    let n = tracks.len();
    let parsed = Parsed {
        format,
        tracks: tracks.into_iter().map(|t| t.track).collect(),
        samples: alloc::vec![Vec::new(); n],
        declared_duration_ns: declared,
        edits: alloc::vec![Vec::new(); n],
    };
    Ok(Some(IndexedOpen { parsed, index, spans, reader: SpanReader::Mkv(sp) }))
}

/// DEMUXSEEK (SR200): one span `[from, to)` of an indexed open — its Clusters through [`parse_cluster`] (a Cues, Tags or
/// Void child skipped), durations by [`fill_durations`] (the declared remainder only in the `last` span).
pub(crate) fn load_span(data: &[u8], sp: &Spans, from: usize, to: usize, last: bool) -> Result<Vec<Vec<Sample>>, Error> {
    let mut ts: Vec<TrackState> = sp
        .tracks
        .iter()
        .zip(&sp.default_durations)
        .map(|(t, &dd)| TrackState { track: t.clone(), default_duration: dd, samples: Vec::new(), has_duration: Vec::new() })
        .collect();
    let mut p = from;
    while p < to.min(sp.seg_end) {
        let e = read_el(data, p, sp.seg_end)?;
        if e.id == ID_CLUSTER {
            p = parse_cluster(data, &e, sp.seg_end, sp.scale, &mut ts)?;
        } else if e.unknown {
            return Err(Error::Unsupported("unknown-size element other than Segment/Cluster"));
        } else {
            p = e.end;
        }
    }
    let mut out = Vec::with_capacity(ts.len());
    for (i, mut t) in ts.into_iter().enumerate() {
        for s in t.samples.iter_mut() {
            s.track_index = i;
        }
        fill_durations(&mut t.samples, &t.has_duration, if last { sp.declared } else { None });
        out.push(t.samples);
    }
    Ok(out)
}

/// NLEPROBEHDR (SR226): a video track's Colour master (RFC 9559 §5.1.4.1.28.18): MatrixCoefficients 0x55B1,
/// Range 0x55B9 (1 broadcast = limited, 2 full; 0 unspecified and 3 "defined by matrix/transfer" say nothing),
/// TransferCharacteristics 0x55BA, Primaries 0x55BB (H.273 code points, 2 when absent), MaxCLL 0x55BC, MaxFALL 0x55BD, and
/// MasteringMetadata 0x55D0 (PrimaryR/G/B ChromaticityX/Y 0x55D1-0x55D6, WhitePointChromaticityX/Y 0x55D7/0x55D8,
/// LuminanceMax 0x55D9, LuminanceMin 0x55DA: floats, chromaticity 0..1 and cd/m², rounded to ST 2086 units). `None` when the
/// master does not parse.
fn colour_master(data: &[u8], e: &El) -> Option<crate::colour::Colour> {
    use crate::colour::{Colour, Light, Mastering};
    let code = |v: u64| u8::try_from(v).unwrap_or(2);
    let mut c = Colour { primaries: 2, transfer: 2, matrix: 2, full_range: None, mastering: None, light: None };
    let (mut cll, mut fall) = (None, None);
    for x in kids(data, e.body, e.end).ok()? {
        match x.id {
            0x55B1 => c.matrix = code(uint(data, &x).ok()?),
            0x55B9 => {
                c.full_range = match uint(data, &x).ok()? {
                    1 => Some(false),
                    2 => Some(true),
                    _ => None,
                }
            }
            0x55BA => c.transfer = code(uint(data, &x).ok()?),
            0x55BB => c.primaries = code(uint(data, &x).ok()?),
            0x55BC => cll = Some(uint(data, &x).ok()?.min(u16::MAX as u64) as u16),
            0x55BD => fall = Some(uint(data, &x).ok()?.min(u16::MAX as u64) as u16),
            0x55D0 => {
                let mut m = Mastering { primaries: [[0; 2]; 3], white: [0; 2], max_lum: 0, min_lum: 0 };
                for y in kids(data, x.body, x.end).ok()? {
                    let v = float(data, &y).ok()?;
                    let xy = round_f64(v * 50_000.0).min(u16::MAX as u64) as u16;
                    let lum = round_f64(v * 10_000.0).min(u32::MAX as u64) as u32;
                    match y.id {
                        0x55D1..=0x55D6 => {
                            let i = (y.id - 0x55D1) as usize;
                            m.primaries[i / 2][i % 2] = xy;
                        }
                        0x55D7 => m.white[0] = xy,
                        0x55D8 => m.white[1] = xy,
                        0x55D9 => m.max_lum = lum,
                        0x55DA => m.min_lum = lum,
                        _ => {}
                    }
                }
                c.mastering = Some(m);
            }
            _ => {}
        }
    }
    if cll.is_some() || fall.is_some() {
        c.light = Some(Light { max_cll: cll.unwrap_or(0), max_fall: fall.unwrap_or(0) });
    }
    Some(c)
}
