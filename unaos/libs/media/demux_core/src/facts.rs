// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ATTRCOLUMNS (rmbp-ledger B402) — a container file's FACTS (MP4/M4A, Matroska/WebM): the presentation length,
//! the codec of the track a viewer would name it by (the video track, else the audio track), and the coded pixel
//! size. Read through [`crate::Demuxer::open`] — the same parse playback uses — so the facts and the player cannot
//! disagree. The kernel writes them as `media:duration_ms`, `media:codec`, `media:width`, `media:height`. Pure.

use crate::{Codec, Demuxer, TrackKind};

/// What [`facts_of`] learned. `width`/`height` are 0 for an audio-only file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFacts {
    pub duration_ms: u64,
    pub codec: alloc::string::String,
    pub width: u32,
    pub height: u32,
    pub video: bool,
}

/// The short codec token (`av1`, `avc`, `aac`, …; an unknown codec keeps the container's own name).
pub fn codec_token(c: &Codec) -> &str {
    match c {
        Codec::Av1 => "av1",
        Codec::Avc => "avc",
        Codec::Hevc => "hevc",
        Codec::Vp8 => "vp8",
        Codec::Vp9 => "vp9",
        Codec::Aac => "aac",
        Codec::Opus => "opus",
        Codec::Vorbis => "vorbis",
        Codec::Flac => "flac",
        Codec::Mp3 => "mp3",
        Codec::Pcm { .. } => "pcm",
        Codec::TestPattern => "utp1",
        Codec::Other(s) => s.as_str(),
    }
}

/// The facts of a whole container file (`None` when it is neither ISO-BMFF nor EBML, or will not parse).
pub fn facts_of(bytes: &[u8]) -> Option<MediaFacts> {
    crate::probe(bytes)?;
    let d = Demuxer::open(bytes.to_vec()).ok()?;
    let t = d.video_track().or_else(|| d.audio_track()).or_else(|| d.tracks().first())?;
    Some(MediaFacts {
        duration_ms: d.duration_ns() / 1_000_000,
        codec: alloc::string::String::from(codec_token(&t.codec)),
        width: t.width,
        height: t.height,
        video: t.kind == TrackKind::Video,
    })
}
