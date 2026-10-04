// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Minimal container *writers*: the test media the KATs are built from, the test-pattern streams
//! Stria plays before a real decoder exists, and the remux path (MP4 → WebM, MP4 → fragmented
//! MP4) that lets Chromium judge this crate's fragmented-MP4 and Matroska paths on a real AV1
//! stream. They write exactly the boxes/elements [`crate::mp4`] and [`crate::mkv`] read, in
//! the shapes browsers accept; they are not a general muxer.

use alloc::vec::Vec;

use crate::TrackKind;

// ---------------------------------------------------------------------------------------------
// The UnaOS test-pattern "codec"
// ---------------------------------------------------------------------------------------------

/// The payload of one `utp1` / `V_UNAOS/TESTPATTERN` packet: the frame's ordinal and size. A
/// decoder renders it as colour bars plus the ordinal in digits, so a frame on glass says which
/// packet produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestPatternPacket {
    pub frame: u32,
    pub width: u16,
    pub height: u16,
}

impl TestPatternPacket {
    pub const MAGIC: [u8; 4] = *b"UTP1";
    pub const LEN: usize = 12;
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(Self::LEN);
        v.extend_from_slice(&Self::MAGIC);
        v.extend_from_slice(&self.frame.to_be_bytes());
        v.extend_from_slice(&self.width.to_be_bytes());
        v.extend_from_slice(&self.height.to_be_bytes());
        v
    }
    pub fn decode(b: &[u8]) -> Option<TestPatternPacket> {
        if b.len() < Self::LEN || b[..4] != Self::MAGIC {
            return None;
        }
        Some(TestPatternPacket {
            frame: u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
            width: u16::from_be_bytes([b[8], b[9]]),
            height: u16::from_be_bytes([b[10], b[11]]),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Input model
// ---------------------------------------------------------------------------------------------

/// One track to write. Sample times are in `timescale` ticks per second.
#[derive(Debug, Clone)]
pub struct TrackSpec {
    pub id: u32,
    pub kind: TrackKind,
    /// MP4 sample-entry type (`av01`, `utp1`, `Opus`, ...).
    pub fourcc: [u8; 4],
    /// MP4 configuration box type written inside the sample entry (`av1C`, ...), if any.
    pub config_box: Option<[u8; 4]>,
    /// Matroska CodecID.
    pub codec_id: &'static str,
    pub config: Vec<u8>,
    pub timescale: u32,
    pub width: u16,
    pub height: u16,
    pub sample_rate: u32,
    pub channels: u16,
    /// Matroska DefaultDuration in ns (0 = omit).
    pub default_duration_ns: u64,
}

#[derive(Debug, Clone)]
pub struct SampleSpec {
    pub data: Vec<u8>,
    pub dts: i64,
    pub pts: i64,
    pub duration: u32,
    pub keyframe: bool,
}

#[derive(Debug, Clone)]
pub struct MediaTrack {
    pub spec: TrackSpec,
    pub samples: Vec<SampleSpec>,
}

impl TrackSpec {
    /// A test-pattern video track at `fps` (integer), timescale `fps × 1000`.
    pub fn test_pattern(id: u32, width: u16, height: u16, fps: u32) -> TrackSpec {
        TrackSpec {
            id,
            kind: TrackKind::Video,
            fourcc: *b"utp1",
            config_box: None,
            codec_id: "V_UNAOS/TESTPATTERN",
            config: Vec::new(),
            timescale: fps * 1000,
            width,
            height,
            sample_rate: 0,
            channels: 0,
            default_duration_ns: 1_000_000_000 / fps as u64,
        }
    }
}

/// `n` test-pattern frames at `fps`, a keyframe every `gop` frames, pts == dts.
pub fn test_pattern_track(id: u32, width: u16, height: u16, fps: u32, n: u32, gop: u32) -> MediaTrack {
    let spec = TrackSpec::test_pattern(id, width, height, fps);
    let samples = (0..n)
        .map(|i| SampleSpec {
            data: TestPatternPacket { frame: i, width, height }.encode(),
            dts: i as i64 * 1000,
            pts: i as i64 * 1000,
            duration: 1000,
            keyframe: i % gop.max(1) == 0,
        })
        .collect();
    MediaTrack { spec, samples }
}

// ---------------------------------------------------------------------------------------------
// ISOBMFF
// ---------------------------------------------------------------------------------------------

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn fbx(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 4);
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p);
    }
    v
}

trait Put {
    fn u16b(&mut self, x: u16);
    fn u32b(&mut self, x: u32);
    fn u64b(&mut self, x: u64);
}
impl Put for Vec<u8> {
    fn u16b(&mut self, x: u16) {
        self.extend_from_slice(&x.to_be_bytes());
    }
    fn u32b(&mut self, x: u32) {
        self.extend_from_slice(&x.to_be_bytes());
    }
    fn u64b(&mut self, x: u64) {
        self.extend_from_slice(&x.to_be_bytes());
    }
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

/// Options for [`mp4`].
#[derive(Debug, Clone, Default)]
pub struct Mp4Options {
    /// Movie timescale (mvhd); 0 → 1000.
    pub movie_timescale: u32,
    /// Samples per chunk (progressive); 0 → 1.
    pub chunk: usize,
    /// Edit list per track id: (empty edit duration in movie ticks, media_time in media ticks).
    pub edits: Vec<(u32, u32, i64)>,
    /// Use `co64` instead of `stco`.
    pub co64: bool,
    /// Write fragmented: an empty-table moov with mvex, then one moof+mdat per `fragment`
    /// samples of each track (0 → progressive).
    pub fragment: usize,
}

fn track_duration(t: &MediaTrack) -> u64 {
    t.samples.iter().map(|s| (s.pts + s.duration as i64).max(0) as u64).max().unwrap_or(0)
}

fn sample_entry(t: &TrackSpec) -> Vec<u8> {
    let mut e = alloc::vec![0u8; 6];
    e.u16b(1);
    match t.kind {
        TrackKind::Audio => {
            e.extend_from_slice(&[0u8; 8]);
            e.u16b(t.channels);
            e.u16b(16);
            e.u32b(0);
            e.u32b(t.sample_rate << 16);
        }
        _ => {
            e.extend_from_slice(&[0u8; 16]);
            e.u16b(t.width);
            e.u16b(t.height);
            e.u32b(0x0048_0000);
            e.u32b(0x0048_0000);
            e.u32b(0);
            e.u16b(1);
            e.extend_from_slice(&[0u8; 32]);
            e.u16b(0x0018);
            e.u16b(0xFFFF);
        }
    }
    if let Some(k) = t.config_box {
        e.extend_from_slice(&bx(&k, &t.config));
    }
    bx(&t.fourcc, &e)
}

fn rle<T: PartialEq + Copy>(xs: impl Iterator<Item = T>) -> Vec<(u32, T)> {
    let mut out: Vec<(u32, T)> = Vec::new();
    for x in xs {
        match out.last_mut() {
            Some((n, v)) if *v == x => *n += 1,
            _ => out.push((1, x)),
        }
    }
    out
}

fn trak(t: &MediaTrack, opts: &Mp4Options, movie_ts: u32, chunk_offsets: &[u64], progressive: bool) -> Vec<u8> {
    let s = &t.spec;
    let media_dur = if progressive { track_duration(t) } else { 0 };
    let movie_dur = media_dur * movie_ts as u64 / s.timescale.max(1) as u64;

    let mut tkhd = Vec::new();
    tkhd.u32b(0);
    tkhd.u32b(0);
    tkhd.u32b(s.id);
    tkhd.u32b(0);
    tkhd.u32b(movie_dur as u32);
    tkhd.extend_from_slice(&[0u8; 8]);
    tkhd.u16b(0);
    tkhd.u16b(0);
    tkhd.u16b(if s.kind == TrackKind::Audio { 0x0100 } else { 0 });
    tkhd.u16b(0);
    for m in MATRIX {
        tkhd.u32b(m);
    }
    tkhd.u32b((s.width as u32) << 16);
    tkhd.u32b((s.height as u32) << 16);
    let tkhd = fbx(b"tkhd", 0, 3, &tkhd);

    let mut mdhd = Vec::new();
    mdhd.u32b(0);
    mdhd.u32b(0);
    mdhd.u32b(s.timescale);
    mdhd.u32b(media_dur as u32);
    mdhd.u16b(0x55C4);
    mdhd.u16b(0);
    let mdhd = fbx(b"mdhd", 0, 0, &mdhd);

    let mut hdlr = Vec::new();
    hdlr.u32b(0);
    hdlr.extend_from_slice(match s.kind {
        TrackKind::Video => b"vide",
        TrackKind::Audio => b"soun",
        TrackKind::Other => b"meta",
    });
    hdlr.extend_from_slice(&[0u8; 12]);
    hdlr.extend_from_slice(b"UnaOS\0");
    let hdlr = fbx(b"hdlr", 0, 0, &hdlr);

    let xmhd = match s.kind {
        TrackKind::Audio => fbx(b"smhd", 0, 0, &[0u8; 4]),
        _ => fbx(b"vmhd", 0, 1, &[0u8; 8]),
    };
    let mut dref = Vec::new();
    dref.u32b(1);
    dref.extend_from_slice(&fbx(b"url ", 0, 1, &[]));
    let dinf = bx(b"dinf", &fbx(b"dref", 0, 0, &dref));

    let mut stsd = Vec::new();
    stsd.u32b(1);
    stsd.extend_from_slice(&sample_entry(s));
    let stsd = fbx(b"stsd", 0, 0, &stsd);

    let samples: &[SampleSpec] = if progressive { &t.samples } else { &[] };
    let mut stts = Vec::new();
    let runs = rle(samples.iter().map(|x| x.duration));
    stts.u32b(runs.len() as u32);
    for (n, d) in runs {
        stts.u32b(n);
        stts.u32b(d);
    }
    let stts = fbx(b"stts", 0, 0, &stts);

    let mut ctts_box = Vec::new();
    if samples.iter().any(|x| x.pts != x.dts) {
        let runs = rle(samples.iter().map(|x| (x.pts - x.dts) as i32));
        let mut c = Vec::new();
        c.u32b(runs.len() as u32);
        for (n, o) in runs {
            c.u32b(n);
            c.u32b(o as u32);
        }
        ctts_box = fbx(b"ctts", 1, 0, &c);
    }

    let chunk = opts.chunk.max(1);
    let mut stsc = Vec::new();
    let full = samples.len() / chunk;
    let rem = samples.len() % chunk;
    let mut entries: Vec<(u32, u32)> = Vec::new();
    if full > 0 {
        entries.push((1, chunk as u32));
    }
    if rem > 0 {
        entries.push((full as u32 + 1, rem as u32));
    }
    stsc.u32b(entries.len() as u32);
    for (first, per) in entries {
        stsc.u32b(first);
        stsc.u32b(per);
        stsc.u32b(1);
    }
    let stsc = fbx(b"stsc", 0, 0, &stsc);

    let mut stsz = Vec::new();
    stsz.u32b(0);
    stsz.u32b(samples.len() as u32);
    for x in samples {
        stsz.u32b(x.data.len() as u32);
    }
    let stsz = fbx(b"stsz", 0, 0, &stsz);

    let mut co = Vec::new();
    co.u32b(chunk_offsets.len() as u32);
    for &o in chunk_offsets {
        if opts.co64 { co.u64b(o) } else { co.u32b(o as u32) }
    }
    let co = if opts.co64 { fbx(b"co64", 0, 0, &co) } else { fbx(b"stco", 0, 0, &co) };

    let mut stss_box = Vec::new();
    if samples.iter().any(|x| !x.keyframe) {
        let keys: Vec<u32> =
            samples.iter().enumerate().filter(|(_, x)| x.keyframe).map(|(i, _)| i as u32 + 1).collect();
        let mut b = Vec::new();
        b.u32b(keys.len() as u32);
        for k in keys {
            b.u32b(k);
        }
        stss_box = fbx(b"stss", 0, 0, &b);
    }

    let stbl = bx(b"stbl", &cat(&[&stsd, &stts, &ctts_box, &stsc, &stsz, &co, &stss_box]));
    let minf = bx(b"minf", &cat(&[&xmhd, &dinf, &stbl]));
    let mdia = bx(b"mdia", &cat(&[&mdhd, &hdlr, &minf]));

    let mut edts = Vec::new();
    if let Some(&(_, empty, media_time)) = opts.edits.iter().find(|e| e.0 == s.id) {
        let mut el = Vec::new();
        let n = if empty > 0 { 2 } else { 1 };
        el.u32b(n);
        if empty > 0 {
            el.u32b(empty);
            el.u32b(u32::MAX); // media_time −1
            el.u32b(0x0001_0000);
        }
        el.u32b(movie_dur as u32);
        el.u32b(media_time as i32 as u32);
        el.u32b(0x0001_0000);
        edts = bx(b"edts", &fbx(b"elst", 0, 0, &el));
    }
    bx(b"trak", &cat(&[&tkhd, &edts, &mdia]))
}

fn mvhd(movie_ts: u32, dur: u64, next_id: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.u32b(0);
    b.u32b(0);
    b.u32b(movie_ts);
    b.u32b(dur as u32);
    b.u32b(0x0001_0000);
    b.u16b(0x0100);
    b.extend_from_slice(&[0u8; 10]);
    for m in MATRIX {
        b.u32b(m);
    }
    b.extend_from_slice(&[0u8; 24]);
    b.u32b(next_id);
    fbx(b"mvhd", 0, 0, &b)
}

/// Write an MP4 (progressive, or fragmented when `opts.fragment > 0`).
pub fn mp4(tracks: &[MediaTrack], opts: &Mp4Options) -> Vec<u8> {
    let movie_ts = if opts.movie_timescale == 0 { 1000 } else { opts.movie_timescale };
    let next_id = tracks.iter().map(|t| t.spec.id).max().unwrap_or(0) + 1;
    let ftyp = {
        let mut b = Vec::new();
        b.extend_from_slice(b"isom");
        b.u32b(0x200);
        for brand in [b"isom", b"iso6", b"av01", b"mp41"] {
            b.extend_from_slice(brand);
        }
        bx(b"ftyp", &b)
    };
    if opts.fragment > 0 {
        return mp4_fragmented(tracks, opts, movie_ts, next_id, ftyp);
    }
    let movie_dur = tracks
        .iter()
        .map(|t| track_duration(t) * movie_ts as u64 / t.spec.timescale.max(1) as u64)
        .max()
        .unwrap_or(0);
    let chunk = opts.chunk.max(1);
    // Layout: chunks interleaved track by track in decode order.
    let build_moov = |offsets: &[Vec<u64>]| -> Vec<u8> {
        let mut m = mvhd(movie_ts, movie_dur, next_id);
        for (i, t) in tracks.iter().enumerate() {
            m.extend_from_slice(&trak(t, opts, movie_ts, &offsets[i], true));
        }
        bx(b"moov", &m)
    };
    let nchunks: Vec<usize> = tracks.iter().map(|t| t.samples.len().div_ceil(chunk)).collect();
    let placeholder: Vec<Vec<u64>> = nchunks.iter().map(|&n| alloc::vec![0u64; n]).collect();
    let moov_len = build_moov(&placeholder).len();
    let mdat_body_start = (ftyp.len() + moov_len + 8) as u64;
    let mut offsets: Vec<Vec<u64>> = nchunks.iter().map(|&n| Vec::with_capacity(n)).collect();
    let mut mdat = Vec::new();
    let maxc = nchunks.iter().copied().max().unwrap_or(0);
    for c in 0..maxc {
        for (i, t) in tracks.iter().enumerate() {
            if c >= nchunks[i] {
                continue;
            }
            offsets[i].push(mdat_body_start + mdat.len() as u64);
            for s in t.samples.iter().skip(c * chunk).take(chunk) {
                mdat.extend_from_slice(&s.data);
            }
        }
    }
    let moov = build_moov(&offsets);
    cat(&[&ftyp, &moov, &bx(b"mdat", &mdat)])
}

fn mp4_fragmented(tracks: &[MediaTrack], opts: &Mp4Options, movie_ts: u32, next_id: u32, ftyp: Vec<u8>) -> Vec<u8> {
    let movie_dur = tracks
        .iter()
        .map(|t| track_duration(t) * movie_ts as u64 / t.spec.timescale.max(1) as u64)
        .max()
        .unwrap_or(0);
    let mut moov = mvhd(movie_ts, 0, next_id);
    for t in tracks {
        moov.extend_from_slice(&trak(t, opts, movie_ts, &[], false));
    }
    let mut mvex = Vec::new();
    let mut mehd = Vec::new();
    mehd.u32b(movie_dur as u32);
    mvex.extend_from_slice(&fbx(b"mehd", 0, 0, &mehd));
    for t in tracks {
        let mut b = Vec::new();
        b.u32b(t.spec.id);
        b.u32b(1);
        b.u32b(0);
        b.u32b(0);
        b.u32b(0x0101_0000); // default: depends on others, non-sync
        mvex.extend_from_slice(&fbx(b"trex", 0, 0, &b));
    }
    moov.extend_from_slice(&bx(b"mvex", &mvex));
    let mut out = cat(&[&ftyp, &bx(b"moov", &moov)]);

    let per = opts.fragment;
    let nfrag = tracks.iter().map(|t| t.samples.len().div_ceil(per)).max().unwrap_or(0);
    for f in 0..nfrag {
        // Build trafs with data offsets relative to moof start (default-base-is-moof); the moof
        // size is needed first, so build once with zero offsets to measure.
        let build = |moof_len: usize| -> (Vec<u8>, Vec<u8>) {
            let mut trafs = Vec::new();
            let mut data = Vec::new();
            for t in tracks {
                let chunk: Vec<&SampleSpec> = t.samples.iter().skip(f * per).take(per).collect();
                if chunk.is_empty() {
                    continue;
                }
                let mut tfhd = Vec::new();
                tfhd.u32b(t.spec.id);
                let tfhd = fbx(b"tfhd", 0, 0x2_0000, &tfhd);
                let mut tfdt = Vec::new();
                tfdt.u64b(chunk[0].dts as u64);
                let tfdt = fbx(b"tfdt", 1, 0, &tfdt);
                let mut trun = Vec::new();
                trun.u32b(chunk.len() as u32);
                trun.u32b((moof_len + 8 + data.len()) as u32);
                for s in &chunk {
                    trun.u32b(s.duration);
                    trun.u32b(s.data.len() as u32);
                    trun.u32b(if s.keyframe { 0x0200_0000 } else { 0x0101_0000 });
                    trun.u32b((s.pts - s.dts) as i32 as u32);
                    data.extend_from_slice(&s.data);
                }
                let trun = fbx(b"trun", 1, 0x1 | 0x100 | 0x200 | 0x400 | 0x800, &trun);
                trafs.extend_from_slice(&bx(b"traf", &cat(&[&tfhd, &tfdt, &trun])));
            }
            let mut mfhd = Vec::new();
            mfhd.u32b(f as u32 + 1);
            let moof = bx(b"moof", &cat(&[&fbx(b"mfhd", 0, 0, &mfhd), &trafs]));
            (moof, data)
        };
        let (probe, _) = build(0);
        let (moof, data) = build(probe.len());
        out.extend_from_slice(&moof);
        out.extend_from_slice(&bx(b"mdat", &data));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Matroska / WebM
// ---------------------------------------------------------------------------------------------

/// Lacing used for audio blocks by [`mkv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lacing {
    Xiph,
    Fixed,
    Ebml,
}

#[derive(Debug, Clone)]
pub struct MkvOptions {
    pub webm: bool,
    /// ns per tick; 0 → 1_000_000.
    pub timestamp_scale: u64,
    /// Start a new Cluster every this many blocks (0 → 30).
    pub cluster_blocks: usize,
    pub unknown_size_segment: bool,
    pub unknown_size_cluster: bool,
    /// Write video frames as BlockGroup (with ReferenceBlock on non-keyframes, BlockDuration on
    /// every block) instead of SimpleBlock.
    pub block_groups: bool,
    /// Lace this many consecutive frames of non-video tracks into one block (≤ 1 = no lacing).
    pub lace: usize,
    pub lacing: Lacing,
    /// Write Info/Duration.
    pub duration: bool,
}

impl Default for MkvOptions {
    fn default() -> Self {
        MkvOptions {
            webm: true,
            timestamp_scale: 0,
            cluster_blocks: 0,
            unknown_size_segment: false,
            unknown_size_cluster: false,
            block_groups: false,
            lace: 0,
            lacing: Lacing::Xiph,
            duration: true,
        }
    }
}

fn vint_len(v: u64) -> usize {
    let mut len = 1;
    while len < 8 && v >= (1u64 << (7 * len)) - 1 {
        len += 1;
    }
    len
}

fn put_vint(out: &mut Vec<u8>, v: u64, len: usize) {
    let marked = v | (1u64 << (7 * len));
    out.extend_from_slice(&marked.to_be_bytes()[8 - len..]);
}

fn put_id(out: &mut Vec<u8>, id: u32) {
    let b = id.to_be_bytes();
    let skip = b.iter().position(|&x| x != 0).unwrap_or(3);
    out.extend_from_slice(&b[skip..]);
}

fn el(id: u32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 12);
    put_id(&mut v, id);
    put_vint(&mut v, body.len() as u64, vint_len(body.len() as u64));
    v.extend_from_slice(body);
    v
}

fn el_unknown(id: u32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 12);
    put_id(&mut v, id);
    v.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    v.extend_from_slice(body);
    v
}

fn el_uint(id: u32, x: u64) -> Vec<u8> {
    let b = x.to_be_bytes();
    let skip = b.iter().position(|&c| c != 0).unwrap_or(7);
    el(id, &b[skip..])
}

fn el_f64(id: u32, x: f64) -> Vec<u8> {
    el(id, &x.to_bits().to_be_bytes())
}

struct Frame<'a> {
    track: usize,
    ns: i64,
    s: &'a SampleSpec,
}

fn to_ns(ticks: i64, ts: u32) -> i64 {
    (ticks as i128 * 1_000_000_000 / ts.max(1) as i128) as i64
}

/// Write a Matroska/WebM file. Frames are written in decode order merged across tracks;
/// Matroska block times are the presentation times (`pts`).
pub fn mkv(tracks: &[MediaTrack], opts: &MkvOptions) -> Vec<u8> {
    let scale = if opts.timestamp_scale == 0 { 1_000_000 } else { opts.timestamp_scale };
    let mut ebml = Vec::new();
    ebml.extend_from_slice(&el_uint(0x4286, 1));
    ebml.extend_from_slice(&el_uint(0x42F7, 1));
    ebml.extend_from_slice(&el_uint(0x42F2, 4));
    ebml.extend_from_slice(&el_uint(0x42F3, 8));
    ebml.extend_from_slice(&el(0x4282, if opts.webm { b"webm" } else { b"matroska" }));
    ebml.extend_from_slice(&el_uint(0x4287, 4));
    ebml.extend_from_slice(&el_uint(0x4285, 2));
    let header = el(0x1A45_DFA3, &ebml);

    let end_ns = tracks
        .iter()
        .flat_map(|t| t.samples.iter().map(move |s| to_ns(s.pts + s.duration as i64, t.spec.timescale)))
        .max()
        .unwrap_or(0);
    let mut info = el_uint(0x2A_D7B1, scale);
    if opts.duration {
        info.extend_from_slice(&el_f64(0x4489, end_ns as f64 / scale as f64));
    }
    info.extend_from_slice(&el(0x4D80, b"UnaOS demux_core"));
    info.extend_from_slice(&el(0x5741, b"UnaOS demux_core"));
    let info = el(0x1549_A966, &info);

    let mut tr = Vec::new();
    for t in tracks {
        let s = &t.spec;
        let mut e = el_uint(0xD7, s.id as u64);
        e.extend_from_slice(&el_uint(0x73C5, s.id as u64));
        e.extend_from_slice(&el_uint(0x83, if s.kind == TrackKind::Video { 1 } else { 2 }));
        e.extend_from_slice(&el(0x86, s.codec_id.as_bytes()));
        if !s.config.is_empty() {
            e.extend_from_slice(&el(0x63A2, &s.config));
        }
        if s.default_duration_ns > 0 {
            e.extend_from_slice(&el_uint(0x23_E383, s.default_duration_ns));
        }
        if s.kind == TrackKind::Video {
            let v = cat(&[&el_uint(0xB0, s.width as u64), &el_uint(0xBA, s.height as u64)]);
            e.extend_from_slice(&el(0xE0, &v));
        } else {
            let a = cat(&[&el_f64(0xB5, s.sample_rate as f64), &el_uint(0x9F, s.channels as u64)]);
            e.extend_from_slice(&el(0xE1, &a));
        }
        tr.extend_from_slice(&el(0xAE, &e));
    }
    let tracks_el = el(0x1654_AE6B, &tr);

    // Merge frames in decode order.
    let mut frames: Vec<Frame> = Vec::new();
    let mut idx = alloc::vec![0usize; tracks.len()];
    loop {
        let mut pick: Option<(usize, i64)> = None;
        for (i, t) in tracks.iter().enumerate() {
            if let Some(s) = t.samples.get(idx[i]) {
                let d = to_ns(s.dts, t.spec.timescale);
                if pick.is_none_or(|(_, pd)| d < pd) {
                    pick = Some((i, d));
                }
            }
        }
        let Some((i, _)) = pick else { break };
        let s = &tracks[i].samples[idx[i]];
        frames.push(Frame { track: i, ns: to_ns(s.pts, tracks[i].spec.timescale), s });
        idx[i] += 1;
    }

    // Group into blocks (lacing consecutive non-video frames of one track).
    let mut blocks: Vec<Vec<&Frame>> = Vec::new();
    for f in &frames {
        let lace_ok = opts.lace > 1 && tracks[f.track].spec.kind != TrackKind::Video;
        if let Some(last) = blocks.last_mut().filter(|l| lace_ok && l[0].track == f.track && l.len() < opts.lace) {
            last.push(f);
            continue;
        }
        blocks.push(alloc::vec![f]);
    }

    let per_cluster = if opts.cluster_blocks == 0 { 30 } else { opts.cluster_blocks };
    let mut clusters = Vec::new();
    for group in blocks.chunks(per_cluster) {
        let cluster_ticks = group.iter().map(|b| b[0].ns / scale as i64).min().unwrap_or(0).max(0);
        let mut body = el_uint(0xE7, cluster_ticks as u64);
        for b in group {
            let f0 = b[0];
            let t = &tracks[f0.track];
            let rel = (f0.ns / scale as i64 - cluster_ticks) as i16;
            let mut blk = Vec::new();
            put_vint(&mut blk, t.spec.id as u64, vint_len(t.spec.id as u64));
            blk.extend_from_slice(&rel.to_be_bytes());
            let laced = b.len() > 1;
            let lacing_bits = if laced {
                match opts.lacing {
                    Lacing::Xiph => 0x02,
                    Lacing::Fixed => 0x04,
                    Lacing::Ebml => 0x06,
                }
            } else {
                0
            };
            let video = t.spec.kind == TrackKind::Video;
            let use_group = opts.block_groups && video;
            let key = b.iter().all(|f| f.s.keyframe);
            let flags = lacing_bits | if key && !use_group { 0x80 } else { 0 };
            blk.push(flags);
            if laced {
                blk.push((b.len() - 1) as u8);
                match opts.lacing {
                    Lacing::Xiph => {
                        for f in &b[..b.len() - 1] {
                            let mut n = f.s.data.len();
                            while n >= 255 {
                                blk.push(255);
                                n -= 255;
                            }
                            blk.push(n as u8);
                        }
                    }
                    Lacing::Ebml => {
                        let first = b[0].s.data.len() as u64;
                        put_vint(&mut blk, first, vint_len(first));
                        let mut prev = first as i64;
                        for f in &b[1..b.len() - 1] {
                            let cur = f.s.data.len() as i64;
                            let diff = cur - prev;
                            // Smallest length whose signed range holds diff.
                            let mut len = 1;
                            while len < 8 && !(diff.abs() < (1i64 << (7 * len - 1)) - 1) {
                                len += 1;
                            }
                            let bias = (1i64 << (7 * len - 1)) - 1;
                            put_vint(&mut blk, (diff + bias) as u64, len);
                            prev = cur;
                        }
                    }
                    Lacing::Fixed => {}
                }
            }
            for f in b {
                blk.extend_from_slice(&f.s.data);
            }
            if use_group {
                let mut g = el(0xA1, &blk);
                let dur_ns = to_ns(f0.s.duration as i64, t.spec.timescale);
                g.extend_from_slice(&el_uint(0x9B, (dur_ns / scale as i64) as u64));
                if !key {
                    g.extend_from_slice(&el(0xFB, &[0xFF])); // −1: the previous frame
                }
                body.extend_from_slice(&el(0xA0, &g));
            } else {
                body.extend_from_slice(&el(0xA3, &blk));
            }
        }
        if opts.unknown_size_cluster {
            clusters.extend_from_slice(&el_unknown(0x1F43_B675, &body));
        } else {
            clusters.extend_from_slice(&el(0x1F43_B675, &body));
        }
    }
    let seg_body = cat(&[&info, &tracks_el, &clusters]);
    let seg = if opts.unknown_size_segment { el_unknown(0x1853_8067, &seg_body) } else { el(0x1853_8067, &seg_body) };
    cat(&[&header, &seg])
}

// ---------------------------------------------------------------------------------------------
// Remux
// ---------------------------------------------------------------------------------------------

/// Lift a demuxed file back into writer input, keeping every packet byte-exact: the remux path
/// (e.g. progressive MP4 → WebM or fragmented MP4) used to put a real AV1 stream into container
/// shapes Chromium can then judge. Returns `None` for a track whose codec has no mapping in both
/// containers here (AV1, VP8, VP9, Opus, test pattern).
///
/// The source must be MP4 (or carry only test-pattern tracks): an MP4 configuration record
/// (`av1C`, `vpcC`, `dOps`) is what both writers emit, and Matroska's CodecPrivate for VP9
/// (absent) and Opus (`OpusHead`, little-endian) is not convertible here — that is the ceiling.
pub fn remux_tracks(d: &crate::Demuxer) -> Option<Vec<MediaTrack>> {
    use crate::Codec;
    let mut out = Vec::new();
    if d.format() != crate::Format::Mp4 && d.tracks().iter().any(|t| t.codec != Codec::TestPattern) {
        return None;
    }
    for (i, t) in d.tracks().iter().enumerate() {
        let (fourcc, config_box, codec_id): ([u8; 4], Option<[u8; 4]>, &'static str) = match t.codec {
            Codec::Av1 => (*b"av01", Some(*b"av1C"), "V_AV1"),
            Codec::Vp9 => (*b"vp09", Some(*b"vpcC"), "V_VP9"),
            Codec::Vp8 => (*b"vp08", Some(*b"vpcC"), "V_VP8"),
            Codec::Opus => (*b"Opus", Some(*b"dOps"), "A_OPUS"),
            Codec::TestPattern => (*b"utp1", None, "V_UNAOS/TESTPATTERN"),
            _ => return None,
        };
        // Writer timescale: the MP4 media timescale, or 1 GHz-ns for Matroska input (reduced to
        // 1 MHz so 32-bit MP4 fields hold it).
        let (ts, div) = if t.timebase.num == 1 && t.timebase.den <= u32::MAX as u64 {
            (t.timebase.den as u32, 1i64)
        } else {
            (1_000_000u32, 1000i64)
        };
        let conv = |ticks: i64| if div == 1 { ticks } else { t.to_ns(ticks) / div };
        let samples = d
            .track_samples(i)
            .map(|s| SampleSpec {
                data: d.packet_at(s).data,
                dts: conv(s.dts),
                pts: conv(s.pts),
                duration: conv(s.duration as i64) as u32,
                keyframe: s.keyframe,
            })
            .collect();
        out.push(MediaTrack {
            spec: TrackSpec {
                id: t.id,
                kind: t.kind,
                fourcc,
                config_box,
                codec_id,
                config: t.config.clone(),
                timescale: ts,
                width: t.width as u16,
                height: t.height as u16,
                sample_rate: t.sample_rate,
                channels: t.channels,
                default_duration_ns: 0,
            },
            samples,
        });
    }
    Some(out)
}
