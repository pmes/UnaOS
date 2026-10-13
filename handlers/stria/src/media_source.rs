// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AETHERMEDIA2 (LEDGER SR134): the `MediaSourceFeed` — a Stria session fed by appended segments
//! (W3C Media Source Extensions) instead of a file.
//!
//! The page (Aether's `MediaSource` / `SourceBuffer`) turns every mutation into a record of
//! `demux_core::fragment::Op` and applies it to its own `MediaSourceModel`; this feed applies the
//! SAME records to the SAME model, so the page's `buffered` and what Stria plays are one
//! computation. The records reach Stria through the session's journal: the bus key is
//! `mediasource:<journal path>` (the page writes the journal in Aether's media cache, the shape
//! AUDIOTRACK M4 already uses for http(s) media — the bus carries the key, the bytes ride a file),
//! and [`MediaSourceFeed::poll`] reads what was appended since the last tick.
//! [`MediaSourceFeed::push_records`] takes the same bytes in-process (tests; a bus verb that carries
//! records — see the ledger row — would call it too).
//!
//! The player side is [`crate::media::Player::open_live`] / [`crate::media::Player::refresh_live`]:
//! the feed's [`MediaSourceFeed::demuxer`] is every buffered frame of every SourceBuffer as one
//! demuxer; the player resumes after the last packet it decoded, holds its clock at the buffered end
//! (`waiting`), seeks inside buffered ranges, and ends after endOfStream.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use gneiss_pal::dsp::demux::Demuxer;
use gneiss_pal::dsp::demux::fragment::{MediaSourceModel, Op, ReadyState};

/// The bus key prefix of a MediaSource session.
pub const SCHEME: &str = "mediasource:";

/// The journal a `mediasource:` key names.
pub fn journal_path(url: &str) -> Option<PathBuf> {
    let p = url.strip_prefix(SCHEME)?;
    let p = p.strip_prefix("file://").unwrap_or(p);
    (!p.is_empty()).then(|| PathBuf::from(p))
}

pub struct MediaSourceFeed {
    path: Option<PathBuf>,
    /// Bytes of the journal already read.
    offset: u64,
    /// A record cut by the read boundary.
    partial: Vec<u8>,
    pub model: MediaSourceModel,
    seen: u64,
    /// Records that failed to decode (a damaged journal stops the feed).
    pub broken: Option<String>,
}

impl MediaSourceFeed {
    /// A feed with no journal: records arrive through [`MediaSourceFeed::push_records`].
    pub fn new() -> MediaSourceFeed {
        MediaSourceFeed { path: None, offset: 0, partial: Vec::new(), model: MediaSourceModel::new(), seen: 0, broken: None }
    }

    /// A feed that tails the journal of a `mediasource:` key.
    pub fn for_url(url: &str) -> Option<MediaSourceFeed> {
        let path = journal_path(url)?;
        let mut f = MediaSourceFeed::new();
        f.path = Some(path);
        Some(f)
    }

    /// Apply journal bytes (whole or partial records). Returns the records applied.
    pub fn push_records(&mut self, bytes: &[u8]) -> usize {
        if self.broken.is_some() {
            return 0;
        }
        self.partial.extend_from_slice(bytes);
        let mut n = 0;
        let mut p = 0;
        loop {
            match Op::decode(&self.partial[p..]) {
                Ok(Some((op, used))) => {
                    self.model.apply(&op);
                    p += used;
                    n += 1;
                }
                Ok(None) => break,
                Err(e) => {
                    self.broken = Some(e.to_string());
                    break;
                }
            }
        }
        self.partial.drain(..p);
        n
    }

    /// Apply one record directly.
    pub fn apply(&mut self, op: &Op) {
        self.model.apply(op);
    }

    /// Read what the journal gained since the last poll. True when the model changed.
    pub fn poll(&mut self) -> bool {
        if let Some(path) = self.path.clone() {
            if let Ok(mut f) = std::fs::File::open(&path) {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                if len > self.offset && f.seek(SeekFrom::Start(self.offset)).is_ok() {
                    let mut buf = Vec::with_capacity((len - self.offset) as usize);
                    if f.read_to_end(&mut buf).is_ok() {
                        self.offset += buf.len() as u64;
                        self.push_records(&buf);
                    }
                }
            }
        }
        self.take_changed()
    }

    /// True (once) when the model changed since the last ask.
    pub fn take_changed(&mut self) -> bool {
        let g = self.model.generation;
        let changed = g != self.seen;
        self.seen = g;
        changed
    }

    /// Every SourceBuffer has its initialization segment: the session can open.
    pub fn ready(&self) -> bool {
        self.model.all_init()
    }

    /// endOfStream was called (without an error).
    pub fn ended(&self) -> bool {
        self.model.ready == ReadyState::Ended && self.model.error == 0
    }

    /// The MediaSource's duration (0 while NaN or +Infinity).
    pub fn duration_ns(&self) -> u64 {
        match self.model.duration_ns {
            Some(d) if d != i64::MAX && d > 0 => d as u64,
            _ => 0,
        }
    }

    /// Every buffered frame as one demuxer (None until [`MediaSourceFeed::ready`]).
    pub fn demuxer(&self) -> Option<Demuxer> {
        self.model.demuxer()
    }

    /// The audio rate the session's output opens at (0 without audio).
    pub fn audio_rate(&self) -> u32 {
        self.demuxer()
            .and_then(|d| d.audio_track().map(|t| if t.codec == gneiss_pal::dsp::demux::Codec::Opus { 48_000 } else { t.sample_rate }))
            .unwrap_or(0)
    }
}

impl Default for MediaSourceFeed {
    fn default() -> Self {
        Self::new()
    }
}
