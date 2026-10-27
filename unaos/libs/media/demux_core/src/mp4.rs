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
//! "one optional empty edit + one media edit" shape shifts the pts; DEMUXSEEK (SR200) keeps EVERY
//! entry, dwells (rate 0) and rates included, as the track's [`crate::EditSegment`] list.
//!
//! DEMUXSEEK (SR200): `sdtp` §8.6.4 and the sample_flags dependency bits (§8.8.3.1) reach
//! [`crate::Sample::depends`]; `sidx` §8.16.3 and `mfra` / `tfra` / `mfro` §8.8.9-8.8.11 are the index
//! [`parse_index`] opens a fragmented file by.

use alloc::string::String;
use alloc::vec::Vec;

use crate::read::Reader;
use crate::{Codec, Edit, EditSegment, Error, Format, IndexPoint, IndexedOpen, Parsed, Sample, SpanReader, Timebase, Track, TrackKind};

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
        let b = box_at(data, p, to)?;
        p = b.end;
        out.push(b);
    }
    Ok(out)
}

/// The one box whose header starts at `p` inside `data[..to]`.
fn box_at(data: &[u8], p: usize, to: usize) -> Result<BoxHeader, Error> {
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
    Ok(BoxHeader { kind, start: p, body: p + hdr, end: p + size as usize })
}

pub(crate) fn child<'a>(list: &'a [BoxHeader], kind: &[u8; 4]) -> Option<&'a BoxHeader> {
    list.iter().find(|b| &b.kind == kind)
}

pub(crate) fn children(data: &[u8], b: &BoxHeader) -> Result<Vec<BoxHeader>, Error> {
    boxes(data, b.body, b.end)
}

pub(crate) fn body<'a>(data: &'a [u8], b: &BoxHeader) -> Reader<'a> {
    Reader::new(&data[b.body..b.end])
}

/// Full box: version byte + 24-bit flags.
pub(crate) fn full(r: &mut Reader) -> Result<(u8, u32), Error> {
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
    /// DEMUXSEEK (SR200): every `elst` entry.
    edits: Vec<EditSegment>,
}

const NON_SYNC: u32 = 0x0001_0000;

/// MP4ONE (rmbp B464): the top level of a file cut short — the last box clamped to the end of the data.
fn boxes_partial(data: &[u8]) -> Vec<BoxHeader> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while data.len() - p >= 8 {
        let mut r = Reader::new(&data[p..]);
        let (Ok(size32), Ok(kind)) = (r.u32(), r.fourcc()) else { break };
        let (size, hdr) = match size32 {
            0 => ((data.len() - p) as u64, 8usize),
            1 => match r.u64() {
                Ok(s) => (s, 16usize),
                Err(_) => break,
            },
            n => (n as u64, 8usize),
        };
        let hdr = if &kind == b"uuid" { hdr + 16 } else { hdr };
        if size < hdr as u64 || p + hdr > data.len() {
            break;
        }
        let end = p + (size.min((data.len() - p) as u64) as usize);
        out.push(BoxHeader { kind, start: p, body: p + hdr, end });
        p = end;
    }
    out
}

/// What `moov` says: the movie timescale and duration, `trex` defaults, `mehd`, and every `trak` with its progressive table.
struct Moov {
    movie_ts: u32,
    movie_dur: u64,
    trex: Vec<Trex>,
    fragment_dur: Option<u64>,
    traks: Vec<TrakState>,
}

pub(crate) fn parse(data: &[u8], partial: bool) -> Result<Parsed, Error> {
    let top = if partial { boxes_partial(data) } else { boxes(data, 0, data.len())? };
    let moov = child(&top, b"moov").ok_or(Error::Invalid("no moov box"))?;
    let Moov { movie_ts, movie_dur, trex, fragment_dur, mut traks } = parse_moov(data, moov)?;

    // Fragments, in file order.
    for m in top.iter().filter(|b| &b.kind == b"moof") {
        parse_moof(data, m, &mut traks, &trex)?;
    }

    let declared = declared_ns(movie_ts, movie_dur, fragment_dur);
    apply_smpb(data, moov, &mut traks);

    let mut tracks = Vec::new();
    let mut samples = Vec::new();
    let mut edits = Vec::new();
    for (i, mut t) in traks.into_iter().enumerate() {
        for s in &mut t.samples {
            s.track_index = i;
        }
        tracks.push(t.track);
        samples.push(t.samples);
        edits.push(t.edits);
    }
    Ok(Parsed { format: Format::Mp4, tracks, samples, declared_duration_ns: declared, edits })
}

/// `moov`: `mvhd`, `mvex` (`trex`, `mehd`) and every `trak` — read once, by the full open and the index open alike.
fn parse_moov(data: &[u8], moov: &BoxHeader) -> Result<Moov, Error> {
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
    Ok(Moov { movie_ts, movie_dur, trex, fragment_dur, traks })
}

/// The declared duration in ns: `mehd`, else `mvhd`'s when it is a real value.
fn declared_ns(movie_ts: u32, movie_dur: u64, fragment_dur: Option<u64>) -> Option<u64> {
    match fragment_dur {
        Some(d) if d > 0 => Some(d),
        _ if movie_dur > 0 && movie_dur != u32::MAX as u64 && movie_dur != u64::MAX => Some(movie_dur),
        _ => None,
    }
    .map(|d| Timebase { num: 1, den: movie_ts as u64 }.to_ns(d as i64) as u64)
}

/// AUDIOTRACK (SR45) / MP4ONE (B464): the iTunSMPB facts on the audio tracks.
fn apply_smpb(data: &[u8], moov: &BoxHeader, traks: &mut [TrakState]) {
    // AUDIOTRACK (SR45): Apple's gapless tag (`moov/udta/meta/ilst/----` "iTunSMPB") for audio
    // tracks no edit list already trimmed: priming to drop and the presented sample count.
    if let Some((pri, total)) = itunsmpb(&data[moov.body..moov.end]) {
        for t in traks.iter_mut().filter(|t| t.track.kind == TrackKind::Audio) {
            t.track.smpb = Some((pri, total)); // MP4ONE (B464): the raw tag, for a decoder's own sample units
        }
        for t in traks.iter_mut() {
            let first = t.samples.iter().map(|s| s.pts).min().unwrap_or(0);
            if t.track.kind == TrackKind::Audio && t.track.sample_rate > 0 && first >= 0 && t.track.play_ns.is_none() {
                let ns = |n: u64| (n as u128 * 1_000_000_000 / t.track.sample_rate as u128) as u64;
                t.track.trim_start_ns = ns(pri);
                if total > 0 {
                    t.track.play_ns = Some(ns(total));
                }
            }
        }
    }
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
    // AUDIOTRACK (SR45): the media edit's segment duration is the presented length (gapless end).
    let mut play_ns: Option<u64> = None;
    let mut edit: Option<Edit> = None;
    let mut raw_edits: Vec<(i64, i64, i32)> = Vec::new();
    if let Some(edts) = child(&kids, b"edts") {
        if let Some(elst) = child(&children(data, edts)?, b"elst") {
            let mut r = body(data, elst);
            let (v, _) = full(&mut r)?;
            let n = r.u32()?;
            let mut empty_movie = 0i64;
            for _ in 0..n {
                if edit.is_some() && r.remaining() < if v == 1 { 20 } else { 12 } {
                    break; // past the first media edit a short elst ends the list, as the shift never needed it
                }
                let (dur, media_time) =
                    if v == 1 { (r.u64()? as i64, r.i64()?) } else { (r.u32()? as i64, r.i32()? as i64) };
                let rate = r.i32()?; // media_rate_integer + fraction, 16.16
                raw_edits.push((dur, media_time, rate)); // DEMUXSEEK (SR200): every entry
                if edit.is_some() {
                    // the first media edit set the shift; later entries are the segment list's only
                } else if media_time == -1 {
                    empty_movie += dur;
                } else {
                    let empty_media = Timebase { num: 1, den: movie_ts as u64 }
                        .to_ns(empty_movie);
                    let empty_media = Timebase { num: 1, den: timescale as u64 }.from_ns(empty_media);
                    shift = media_time - empty_media;
                    edit = Some(Edit { media_time, segment: dur as u64, movie_timescale: movie_ts });
                    if dur > 0 {
                        play_ns = Some(Timebase { num: 1, den: movie_ts as u64 }.to_ns(dur) as u64);
                    }
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
        trim_start_ns: 0,
        play_ns,
        edit,
        smpb: None, colour: None,
    };

    let minf = child(&mdia_kids, b"minf").ok_or(Error::Invalid("mdia without minf"))?;
    let stbl_box = *child(&children(data, minf)?, b"stbl").ok_or(Error::Invalid("minf without stbl"))?;
    let stbl = children(data, &stbl_box)?;

    if let Some(stsd) = child(&stbl, b"stsd") {
        parse_stsd(data, stsd, &mut track)?;
    }

    let samples = build_table(data, &stbl, shift)?;
    let next_dts = samples.last().map(|s| s.dts + s.duration as i64).unwrap_or(0);
    let edits = edit_segments(&raw_edits, movie_ts, shift);
    Ok(TrakState { track, shift, samples, next_dts, edits })
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
    let mut sample_size = 16u16;
    match &fourcc {
        b"av01" | b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"vp08" | b"vp09" | b"utp1" | b"encv" => {
            r.skip(16)?; // pre_defined, reserved, pre_defined[3]
            track.width = r.u16()? as u32;
            track.height = r.u16()? as u32;
            // horiz/vert resolution 8, reserved 4, frame_count 2, compressorname 32, depth 2,
            // pre_defined 2  → VisualSampleEntry is 78 bytes after the box header.
            child_start = e.body + 78;
        }
        b"mp4a" | b".mp3" | b"Opus" | b"fLaC" | b"enca" | b"ac-3" | b"ec-3" | b"sowt" | b"twos" | b"ipcm" | b"fpcm" => {
            let version = r.u16()?;
            r.skip(6)?; // revision, vendor
            track.channels = r.u16()?;
            sample_size = r.u16()?;
            r.skip(4)?; // pre_defined, reserved
            track.sample_rate = r.u32()? >> 16;
            // QuickTime sound description v1/v2 extensions.
            child_start = e.body + 28 + match version { 1 => 16, 2 => 36, _ => 0 };
        }
        _ => child_start = e.end,
    }
    let kids = if child_start < e.end { boxes(data, child_start, e.end)? } else { Vec::new() };
    let cfg = |k: &[u8; 4]| child(&kids, k).map(|b| data[b.body..b.end].to_vec()); track.colour = colour_boxes(data, &kids);
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
        // QuickTime uncompressed: `sowt` little-endian, `twos` big-endian (8-bit is signed),
        // bit depth from the AudioSampleEntry samplesize.
        b"sowt" => Codec::Pcm { bits: sample_size, float: false, big_endian: false },
        b"twos" => Codec::Pcm { bits: sample_size, float: false, big_endian: true },
        // ISO/IEC 23003-5: `pcmC` = FullBox { format_flags(8) bit 0 little-endian,
        // PCM_sample_size(8) }.
        b"ipcm" | b"fpcm" => {
            let c = cfg(b"pcmC").unwrap_or_default();
            let (le, bits) = if c.len() >= 6 { (c[4] & 1 != 0, c[5] as u16) } else { (false, sample_size) };
            track.config = c;
            Codec::Pcm { bits, float: &fourcc == b"fpcm", big_endian: !le }
        }
        // MP4ONE (B464): QuickTime wraps the `esds` in a `wave` box (the retired audio_core reader read both).
        b"mp4a" => match child(&kids, b"esds").copied().or_else(|| {
            let w = child(&kids, b"wave")?;
            child(&boxes(data, w.body, w.end).ok()?, b"esds").copied()
        }) {
            Some(b) => {
                let (oti, dsi) = parse_esds(&data[b.body..b.end])?;
                track.config = dsi;
                match oti {
                    0x40 | 0x66 | 0x67 | 0x68 => Codec::Aac,
                    0xDD => Codec::Vorbis,
                    0xAD => Codec::Opus,
                    0x69 | 0x6B => Codec::Mp3,
                    _ => Codec::Other(String::from("mp4a")),
                }
            }
            None => Codec::Other(String::from("mp4a")),
        },
        b".mp3" => Codec::Mp3,
        _ => Codec::Other(track.codec_name.clone()),
    };
    Ok(())
}

/// `iTunSMPB`: " 00000000 PPPPPPPP QQQQQQQQ TTTTTTTTTTTTTTTT ..." (hex words) → (priming,
/// total samples). The tag's `data` atom follows its name inside the `----` item.
fn itunsmpb(m: &[u8]) -> Option<(u64, u64)> {
    let k = m.windows(8).position(|w| w == b"iTunSMPB")?;
    let rest = &m[k + 8..];
    let j = rest.windows(4).position(|w| w == b"data")?;
    let txt = rest.get(j + 12..)?;
    let mut f: Vec<u64> = Vec::new();
    let mut cur: Option<u64> = None;
    for &c in txt.iter().take(120) {
        match (c as char).to_digit(16) {
            Some(x) => cur = Some(cur.unwrap_or(0).wrapping_mul(16).wrapping_add(x as u64)),
            None => {
                if let Some(x) = cur.take() {
                    f.push(x);
                }
                if c != b' ' {
                    break;
                }
            }
        }
    }
    if let Some(x) = cur {
        f.push(x);
    }
    (f.len() >= 4).then(|| (f[1], f[3]))
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
                discard_ns: 0,
                depends: 0,
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

    // DEMUXSEEK (SR200): sample dependency flags (§8.6.4), one byte per sample.
    if let Some(b) = child(stbl, b"sdtp") {
        let mut r = body(data, b);
        full(&mut r)?;
        for s in out.iter_mut() {
            if r.remaining() == 0 {
                break;
            }
            s.depends = r.u8()?;
        }
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
                    discard_ns: 0,
                    depends: (flags >> 20) as u8, // DEMUXSEEK (SR200): is_leading .. has_redundancy, bits 27..20
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

// ---------------------------------------------------------------------------------------------------
// DEMUXSEEK (LEDGER SR200): the edit list kept whole, and the open by `sidx` / `mfra`.
// ---------------------------------------------------------------------------------------------------

/// Every `elst` entry (segment_duration in movie ticks, media_time in media ticks, media_rate 16.16) as an
/// [`EditSegment`] list: movie-timeline starts summed in ns, media times on the pts axis (less the first media edit's shift).
fn edit_segments(raw: &[(i64, i64, i32)], movie_ts: u32, shift: i64) -> Vec<EditSegment> {
    let mtb = Timebase { num: 1, den: movie_ts as u64 };
    let mut start = 0i64;
    let mut out = Vec::with_capacity(raw.len());
    for &(dur, media_time, rate) in raw {
        let duration_ns = mtb.to_ns(dur).max(0) as u64;
        out.push(EditSegment { start_ns: mtb.to_ns(start), duration_ns, media_pts: (media_time != -1).then(|| media_time - shift), rate });
        start = start.saturating_add(dur);
    }
    out
}

/// DEMUXSEEK (SR200): what a span load needs of `moov`.
#[derive(Clone)]
pub(crate) struct Spans {
    trex: Vec<Trex>,
    tracks: Vec<(Track, i64)>,
}

/// DEMUXSEEK (SR200): the open by a fragmented file's index. The top level is read only up to the first `moof`; the index
/// is the `sidx` boxes there (each type-0 reference that starts with a SAP: positions from the box's end plus `first_offset`,
/// times from `earliest_presentation_time` plus the subsegment durations; a type-1 reference — a hierarchical index — means
/// none), else
/// the `mfra` the trailing `mfro` points at (each `tfra` entry's time and `moof_offset`). Every position must begin a `moof`,
/// and the first `moof` must give every `traf` a `tfdt`. `None` for a progressive file or one without a usable index.
pub(crate) fn parse_index(data: &[u8]) -> Result<Option<IndexedOpen>, Error> {
    let len = data.len();
    let mut top: Vec<BoxHeader> = Vec::new();
    let mut first_moof: Option<usize> = None;
    let mut p = 0usize;
    while len - p >= 8 {
        let b = box_at(data, p, len)?;
        if &b.kind == b"moof" {
            first_moof = Some(p);
            break;
        }
        p = b.end;
        top.push(b);
    }
    let Some(first_moof) = first_moof else { return Ok(None) };
    let Some(moov) = child(&top, b"moov") else { return Ok(None) };
    let Moov { movie_ts, movie_dur, trex, fragment_dur, mut traks } = parse_moov(data, moov)?;
    if traks.iter().any(|t| !t.samples.is_empty()) {
        return Ok(None);
    }
    let ti_of = |id: u32| traks.iter().position(|t| t.track.id == id);

    let mut index: Vec<IndexPoint> = Vec::new();
    for sx in top.iter().filter(|b| &b.kind == b"sidx") {
        let mut r = body(data, sx);
        let (v, _) = full(&mut r)?;
        let ref_id = r.u32()?;
        let timescale = r.u32()?.max(1);
        let (ept, first_offset) = if v == 0 { (r.u32()? as i64, r.u32()? as u64) } else { (r.u64()? as i64, r.u64()?) };
        r.skip(2)?;
        let count = r.u16()?;
        let tb = Timebase { num: 1, den: timescale as u64 };
        let ti = ti_of(ref_id).unwrap_or(0);
        let mut pos = (sx.end as u64).saturating_add(first_offset);
        let mut t = ept;
        for _ in 0..count {
            let w = r.u32()?;
            let dur = r.u32()?;
            let sap = r.u32()?; // starts_with_SAP, SAP_type, SAP_delta_time
            if w & 0x8000_0000 != 0 {
                return Ok(None);
            }
            if sap & 0x8000_0000 != 0 {
                // a subsegment that starts with a stream access point: a sync sample a decode may begin at
                index.push(IndexPoint { track_index: ti, pts_ns: tb.to_ns(t), pos });
            }
            pos = pos.saturating_add((w & 0x7FFF_FFFF) as u64);
            t = t.saturating_add(dur as i64);
        }
    }
    if index.is_empty() && len >= 16 {
        if let Ok(mfro) = box_at(data, len - 16, len) {
            if &mfro.kind == b"mfro" {
                let mut r = body(data, &mfro);
                full(&mut r)?;
                let size = r.u32()? as usize;
                if size >= 8 && size <= len {
                    let mfra = box_at(data, len - size, len)?;
                    if &mfra.kind == b"mfra" {
                        for tf in children(data, &mfra)?.iter().filter(|b| &b.kind == b"tfra") {
                            let mut r = body(data, tf);
                            let (v, _) = full(&mut r)?;
                            let id = r.u32()?;
                            let sizes = r.u32()?;
                            let (lt, lr, ls) = (((sizes >> 4) & 3) as usize + 1, ((sizes >> 2) & 3) as usize + 1, (sizes & 3) as usize + 1);
                            let n = r.u32()?;
                            let Some(ti) = ti_of(id) else { continue };
                            let tb = traks[ti].track.timebase;
                            for _ in 0..n {
                                let (time, off) = if v == 1 { (r.u64()? as i64, r.u64()?) } else { (r.u32()? as i64, r.u32()? as u64) };
                                r.skip(lt + lr + ls)?;
                                index.push(IndexPoint { track_index: ti, pts_ns: tb.to_ns(time), pos: off });
                            }
                        }
                    }
                }
            }
        }
    }
    if index.is_empty() {
        return Ok(None);
    }
    for x in &index {
        let at = x.pos as usize;
        if at < first_moof || at >= len || len - at < 8 || &box_at(data, at, len)?.kind != b"moof" {
            return Ok(None);
        }
    }
    let m0 = box_at(data, first_moof, len)?;
    if !crate::fragment::moof_has_tfdt(&data[m0.start..m0.end])? {
        return Ok(None);
    }

    let mut starts: Vec<u64> = index.iter().map(|x| x.pos).collect();
    starts.sort_unstable();
    starts.dedup();
    if starts[0] > first_moof as u64 {
        starts.insert(0, first_moof as u64);
    }
    let spans: Vec<(u64, u64)> = starts.iter().enumerate().map(|(i, &a)| (a, starts.get(i + 1).copied().unwrap_or(len as u64))).collect();

    let declared = declared_ns(movie_ts, movie_dur, fragment_dur);
    apply_smpb(data, moov, &mut traks);
    let sp = Spans { trex, tracks: traks.iter().map(|t| (t.track.clone(), t.shift)).collect() };
    let n = traks.len();
    let mut tracks = Vec::with_capacity(n);
    let mut edits = Vec::with_capacity(n);
    for t in traks {
        tracks.push(t.track);
        edits.push(t.edits);
    }
    let parsed = Parsed { format: Format::Mp4, tracks, samples: alloc::vec![Vec::new(); n], declared_duration_ns: declared, edits };
    Ok(Some(IndexedOpen { parsed, index, spans, reader: SpanReader::Mp4(sp) }))
}

/// DEMUXSEEK (SR200): one span `[from, to)` of an indexed open — each `moof` through [`parse_moof`] (a `moof` without a
/// `tfdt` in every `traf` is refused: its decode times would depend on the fragments not read), other boxes skipped.
pub(crate) fn load_span(data: &[u8], sp: &Spans, from: usize, to: usize) -> Result<Vec<Vec<Sample>>, Error> {
    let len = data.len();
    let mut traks: Vec<TrakState> = sp
        .tracks
        .iter()
        .map(|(t, shift)| TrakState { track: t.clone(), shift: *shift, samples: Vec::new(), next_dts: 0, edits: Vec::new() })
        .collect();
    let mut p = from;
    while p < to.min(len) && len - p >= 8 {
        let b = box_at(data, p, len)?;
        if &b.kind == b"moof" {
            if !crate::fragment::moof_has_tfdt(&data[b.start..b.end])? {
                return Err(Error::Unsupported("a fragment without tfdt cannot be read by the index"));
            }
            parse_moof(data, &b, &mut traks, &sp.trex)?;
        }
        p = b.end;
    }
    let mut out = Vec::with_capacity(traks.len());
    for (i, mut t) in traks.into_iter().enumerate() {
        for s in t.samples.iter_mut() {
            s.track_index = i;
        }
        out.push(t.samples);
    }
    Ok(out)
}

/// NLEPROBEHDR (SR226): the colour a visual sample entry states, from the children [`parse_stsd`] already walked:
/// `colr` (ISO/IEC 14496-12 §12.1.5: `nclx` = colour_primaries, transfer_characteristics, matrix_coefficients as u16 and
/// full_range_flag in the top bit of the next byte; QuickTime `nclc` = the three u16 and no range; an ICC `rICC` / `prof`
/// is not read), `mdcv` (ISO/IEC 23001-8 / ST 2086: display_primaries G, B, R as u16 x, y pairs, the white point, then
/// max / min luminance u32; reordered here to R, G, B) and `clli` (max_content_light_level, max_pic_average_light_level,
/// u16). `None` when none of the three is present; a short or unknown box is left out, never an error.
fn colour_boxes(data: &[u8], kids: &[BoxHeader]) -> Option<crate::colour::Colour> {
    use crate::colour::{Colour, Light, Mastering};
    let code = |v: u16| u8::try_from(v).unwrap_or(2);
    let colr = child(kids, b"colr").and_then(|b| {
        let mut r = body(data, b);
        let kind = r.fourcc().ok()?;
        if &kind != b"nclx" && &kind != b"nclc" {
            return None;
        }
        let (p, t, m) = (r.u16().ok()?, r.u16().ok()?, r.u16().ok()?);
        let full = if &kind == b"nclx" { Some(r.u8().ok()? & 0x80 != 0) } else { None };
        Some((code(p), code(t), code(m), full))
    });
    let mastering = child(kids, b"mdcv").and_then(|b| {
        let mut r = body(data, b);
        let mut gbr = [[0u16; 2]; 3];
        for p in gbr.iter_mut() {
            *p = [r.u16().ok()?, r.u16().ok()?];
        }
        let white = [r.u16().ok()?, r.u16().ok()?];
        let (max_lum, min_lum) = (r.u32().ok()?, r.u32().ok()?);
        Some(Mastering { primaries: [gbr[2], gbr[0], gbr[1]], white, max_lum, min_lum })
    });
    let light = child(kids, b"clli").and_then(|b| {
        let mut r = body(data, b);
        Some(Light { max_cll: r.u16().ok()?, max_fall: r.u16().ok()? })
    });
    if colr.is_none() && mastering.is_none() && light.is_none() {
        return None;
    }
    let (primaries, transfer, matrix, full_range) = colr.unwrap_or((2, 2, 2, None));
    Some(Colour { primaries, transfer, matrix, full_range, mastering, light })
}
