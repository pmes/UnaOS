// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! STRIAIMPORTWAV (LEDGER SR889) — RIFF/WAVE as a one-track PCM table.
//!
//! CHARTER: Stria — shared-core. The editor's reel open reads a `.wav` as the same [`Track`] / [`Sample`] shape the MP4 and
//! Matroska arms produce (one `Codec::Pcm` audio track, packets of [`FRAMES`] sample frames), so its PCM decoder and mix
//! need no second road. It is reached ONLY through [`Demuxer::open_wav`]: [`crate::probe`] and [`Demuxer::open`] stay
//! WAV-blind, because a player plays a bare WAV through audio_core's own reader exactly when the demuxer refuses it.
//! Accepted: `fmt ` tag 1 (8-bit unsigned, 16 / 24 / 32-bit integer), tag 3 (32 / 64-bit float), and WAVE_FORMAT_EXTENSIBLE
//! whose SubFormat is one of those with valid bits at or below the container (decoded at the container width), RF64 / BW64 with its `ds64` data size (STRIAWAVVALIDBITS, SR937); 1..=8 channels; a `data` size 0 / 0xFFFFFFFF (streamed) runs to the end of the file (STRIAIMPORTWAVWIDE, SR920). Everything else is refused by name (`wav: ...`).

use alloc::string::String;
use alloc::vec::Vec;

use crate::{Codec, Demuxer, Error, Format, Parsed, Sample, Timebase, Track, TrackKind};

/// Sample frames per packet (the last packet is shorter).
pub const FRAMES: u64 = 1024;

/// True when `b` starts with a RIFF/WAVE header (`RIFF` at 0, `WAVE` at 8).
pub fn is_wav(b: &[u8]) -> bool {
    b.len() >= 12 && (&b[0..4] == b"RIFF" || &b[0..4] == b"RF64" || &b[0..4] == b"BW64") && &b[8..12] == b"WAVE" // STRIAWAVVALIDBITS (SR937): RF64 / BW64 (EBU Tech 3306 / ITU-R BS.2088) are WAV too
}

impl Demuxer {
    /// STRIAIMPORTWAV (SR889): open a RIFF/WAVE file as a one-track PCM demuxer (the editor's door; [`Demuxer::open`]
    /// refuses a WAV). Not RIFF/WAVE: [`Error::UnknownFormat`].
    pub fn open_wav(data: Vec<u8>) -> Result<Demuxer, Error> {
        if !is_wav(&data) {
            return Err(Error::UnknownFormat);
        }
        Demuxer::open_with(data, false, true)
    }
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The `fmt ` chunk's body as (bits, float, channels, rate), or the refusal by name.
fn fmt_of(f: &[u8]) -> Result<(u16, bool, u16, u32), Error> {
    if f.len() < 16 {
        return Err(Error::Invalid("wav: short fmt chunk"));
    }
    let mut tag = le16(f, 0);
    let ch = le16(f, 2);
    let rate = le32(f, 4);
    let align = le16(f, 12);
    let bits = le16(f, 14);
    if tag == 0xFFFE {
        if f.len() < 40 {
            return Err(Error::Invalid("wav: short WAVE_FORMAT_EXTENSIBLE"));
        }
        let valid = le16(f, 18);
        if valid > bits { // STRIAWAVVALIDBITS (SR937): valid bits at or below the container are left-justified, decoded at the container width
            return Err(Error::Unsupported("wav: sample layout (8/16/24/32-bit integer or 32/64-bit float)"));
        }
        tag = le16(f, 24); // the SubFormat GUID's first two bytes are the format tag
    }
    let float = match (tag, bits) {
        (1, 8 | 16 | 24 | 32) => false,
        (3, 32 | 64) => true,
        (1, _) | (3, _) => return Err(Error::Unsupported("wav: sample layout (8/16/24/32-bit integer or 32/64-bit float)")),
        _ => return Err(Error::Unsupported("wav: compressed format tag")),
    };
    if !(1..=8).contains(&ch) {
        return Err(Error::Unsupported("wav: channels"));
    }
    if rate == 0 {
        return Err(Error::Invalid("wav: sample rate 0"));
    }
    if align as u32 != ch as u32 * (bits as u32 / 8) {
        return Err(Error::Invalid("wav: block align"));
    }
    Ok((bits, float, ch, rate))
}

/// The one-track table of a RIFF/WAVE file (the caller checked [`is_wav`]).
pub(crate) fn parse(data: &[u8]) -> Result<Parsed, Error> {
    let mut fmt: Option<(u16, bool, u16, u32)> = None;
    let mut p = 12usize;
    loop {
        if p + 8 > data.len() {
            return Err(Error::Invalid(if fmt.is_none() { "wav: no fmt chunk" } else { "wav: no samples" }));
        }
        let id = &data[p..p + 4];
        let len = le32(data, p + 4) as usize;
        let body = p + 8;
        match id {
            b"fmt " => {
                let end = body.checked_add(len).filter(|&e| e <= data.len()).ok_or(Error::Truncated)?;
                fmt = Some(fmt_of(&data[body..end])?);
            }
            b"data" => {
                let (bits, float, ch, rate) = fmt.ok_or(Error::Invalid("wav: no fmt chunk"))?;
                let len = if len == 0xFFFF_FFFF && &data[0..4] != b"RIFF" { rf64_data_len(data)? } else if len == 0 || len == 0xFFFF_FFFF { data.len() - body } else { len }; // STRIAWAVVALIDBITS (SR937): an RF64 / BW64 data size comes from ds64. STRIAIMPORTWAVWIDE (SR920): a streamed data size runs to the end of the file
                if !body.checked_add(len).is_some_and(|e| e <= data.len()) {
                    return Err(Error::Invalid("wav: data chunk runs past the end of the file"));
                }
                let fb = ch as u64 * (bits as u64 / 8);
                let frames = len as u64 / fb;
                if frames == 0 {
                    return Err(Error::Invalid("wav: no samples"));
                }
                let mut samples = Vec::with_capacity(frames.div_ceil(FRAMES) as usize);
                let mut at = 0u64;
                while at < frames {
                    let n = FRAMES.min(frames - at);
                    samples.push(Sample {
                        track_index: 0,
                        offset: body as u64 + at * fb,
                        size: (n * fb) as u32,
                        pts: at as i64,
                        dts: at as i64,
                        duration: n,
                        keyframe: true,
                        discard_ns: 0,
                        depends: 0,
                    });
                    at += n;
                }
                let track = Track {
                    id: 1,
                    kind: TrackKind::Audio,
                    codec: Codec::Pcm { bits, float, big_endian: false },
                    codec_name: String::from(if float { "WAVE_FORMAT_IEEE_FLOAT" } else { "WAVE_FORMAT_PCM" }),
                    config: Vec::new(),
                    timebase: Timebase { num: 1, den: rate as u64 },
                    width: 0,
                    height: 0,
                    sample_rate: rate,
                    channels: ch,
                    codec_delay_ns: 0,
                    sample_count: 0,
                    duration_ns: 0,
                    frame_prefix: Vec::new(),
                    trim_start_ns: 0,
                    play_ns: None,
                    edit: None,
                    smpb: None, colour: None, btrt: None,
                };
                return Ok(Parsed { format: Format::Wav, tracks: alloc::vec![track], samples: alloc::vec![samples], declared_duration_ns: None, edits: alloc::vec![Vec::new()] });
            }
            _ => {}
        }
        p = body.checked_add(len).and_then(|e| e.checked_add(len & 1)).ok_or(Error::Truncated)?;
    }
}

// ---- STRIAWAVVALIDBITS (LEDGER SR937): RF64 / BW64 ds64 data size (named tail block) ----

/// STRIAWAVVALIDBITS (SR937): the 64-bit `data` size of an RF64 / BW64 file, read from its `ds64` chunk, which must be
/// the first chunk (offset 12: riffSize u64, dataSize u64, sampleCount u64, tableLength u32). A size past `usize` is
/// `usize::MAX`, which the caller refuses as running past the end of the file.
fn rf64_data_len(data: &[u8]) -> Result<usize, Error> {
    if data.len() < 20 + 28 || &data[12..16] != b"ds64" || le32(data, 16) < 28 {
        return Err(Error::Invalid("wav: RF64 without a ds64 chunk"));
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&data[28..36]);
    Ok(usize::try_from(u64::from_le_bytes(b)).unwrap_or(usize::MAX))
}
