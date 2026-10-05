// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `dsp` — Gneiss's signal-processing library (CODEX §3: "The Signal Processing Graph. Audio and
//! Video codecs"), the shared pipeline pieces Stria's player is assembled from (PLAYBACK, LEDGER
//! SR26) and the codec face Ring 3 handlers (Stria, Aether, Facet) reach. Each codec family lives
//! in a `no_std` core under `unaos/libs/media/` that the kernel links too.
//!
//! * [`audio`] — AUDIOCODEC's face over `audio_core` (WAV/AIFF/FLAC/Ogg/Opus/Vorbis/MP3/AAC/MP4).
//! * [`demux`] — re-export of `demux_core`, the `no_std` container core under
//!   `unaos/libs/media/demux_core` (MP4 progressive + fragmented, Matroska/WebM).
//! * [`avsync`] — the playback clock: a master clock (audio when present, else wall), pts →
//!   present-time scheduling with a drop/repeat policy, and drift measurement.
//! * [`video`] — the `VideoDecoder` seam, the `Frame` it yields, the deterministic
//!   `TestPattern` decoder, and the decoder registry. AV1 arrives as `video::av1` from AVCODEC
//!   (LEDGER SR24, `unaos/libs/media/av1_core`) behind the `av1` feature.
//! * [`audio_track`] — the per-packet audio decoder seam the player feeds resonance from (PCM
//!   built in; compressed audio through [`audio`]'s per-packet decoders).
//!
//! Nothing here owns a device, a window or the bus: Stria owns the player and its bus verbs.

pub mod audio;
pub mod audio_track;
pub mod avsync;
pub mod video;

/// The container core, re-exported so pipeline users name one library.
pub use demux_core as demux;
