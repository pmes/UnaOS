// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Matroska / WebM demuxing (RFC 9559, Matroska Media Container; EBML per RFC 8794).
//!
//! Section map: EBML variable-size integers RFC 8794 §4 (element ID keeps its marker bit, data
//! size strips it, all-ones = unknown size §6.2); EBML header §11.2 with DocType; Segment §5.1;
//! Info §5.1.2 (TimestampScale 0x2AD7B1, Duration 0x4489); Tracks §5.1.4 (TrackEntry 0xAE:
//! TrackNumber 0xD7, TrackType 0x83, CodecID 0x86, CodecPrivate 0x63A2, DefaultDuration
//! 0x23E383, CodecDelay 0x56AA, Video 0xE0 → PixelWidth 0xB0 / PixelHeight 0xBA, Audio 0xE1 →
//! SamplingFrequency 0xB5 / Channels 0x9F, ContentEncodings 0x6D80); Cluster §5.1.3 (Timestamp
//! 0xE7, SimpleBlock 0xA3, BlockGroup 0xA0 → Block 0xA1, BlockDuration 0x9B, ReferenceBlock
//! 0xFB); Block structure §10 and lacing §10.3 (Xiph, fixed-size, EBML); unknown-size Cluster
//! termination by the next top-level element §6.2.
//!
//! Timing: every Matroska track here uses a nanosecond timebase. A block's time is
//! `(Cluster.Timestamp + Block.relative) × TimestampScale`; frame k of a laced block adds
//! `k × DefaultDuration`. Matroska stores no decode timestamps — blocks are in decode order — so
//! `dts = pts` (exact for AV1/VP8/VP9/Opus/Vorbis; for reordering codecs such as AVC the dts is
//! the ceiling). A frame's duration is BlockDuration, else DefaultDuration, else the gap to the
//! track's next frame, else (last frame) the remainder of the declared segment duration.

use alloc::string::String;
use alloc::vec::Vec;

use crate::read::Reader;
use crate::{Codec, Error, Format, Parsed, Sample, Timebase, Track, TrackKind};

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
const ID_PIXEL_HEIGHT: u32 = 0xBA;
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

pub(crate) fn parse(data: &[u8]) -> Result<Parsed, Error> {
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

    let mut scale: u64 = 1_000_000;
    let mut duration_ticks: Option<f64> = None;
    let mut tracks: Vec<TrackState> = Vec::new();
    let mut p = seg.body;
    while p < seg.end {
        let e = read_el(data, p, seg.end)?;
        match e.id {
            ID_INFO => {
                for c in kids(data, e.body, e.end)? {
                    match c.id {
                        ID_TIMESTAMP_SCALE => scale = uint(data, &c)?.max(1),
                        ID_DURATION => duration_ticks = Some(float(data, &c)?),
                        _ => {}
                    }
                }
                p = e.end;
            }
            ID_TRACKS => {
                for te in kids(data, e.body, e.end)? {
                    if te.id == ID_TRACK_ENTRY {
                        tracks.push(parse_track(data, &te)?);
                    }
                }
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

    // Fill durations a block did not carry: gap to the track's next frame.
    let mut out_tracks = Vec::new();
    let mut out_samples = Vec::new();
    for (i, mut t) in tracks.into_iter().enumerate() {
        let n = t.samples.len();
        for k in 0..n {
            t.samples[k].track_index = i;
            if t.has_duration[k] {
                continue;
            }
            let d = if k + 1 < n {
                (t.samples[k + 1].pts - t.samples[k].pts).max(0) as u64
            } else {
                match declared {
                    Some(end) if end as i64 > t.samples[k].pts => end - t.samples[k].pts as u64,
                    _ if k > 0 => t.samples[k - 1].duration,
                    _ => 0,
                }
            };
            t.samples[k].duration = d;
        }
        out_tracks.push(t.track);
        out_samples.push(t.samples);
    }
    Ok(Parsed { format, tracks: out_tracks, samples: out_samples, declared_duration_ns: declared })
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
        smpb: None,
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
                        ID_PIXEL_HEIGHT => t.height = uint(data, &v)? as u32,
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
        });
        ts.has_duration.push(explicit.is_some());
        off += sz;
    }
    Ok(())
}
