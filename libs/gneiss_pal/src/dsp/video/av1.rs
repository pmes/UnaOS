// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The AV1 adapter seam (PLAYBACK M3) — STUBBED until AVCODEC (LEDGER SR24) folds.
//!
//! AVCODEC's core is `unaos/libs/media/av1_core` (branch `exec-media-av1`). Its frame entry
//! point is `av1_core::image::decode_obus(data, config_obus, filters) -> Result<Planes>`, with
//! `planes_to_rgba` for display. The fold replaces the body of [`Av1Decoder::decode`] with:
//! `config_obus` = the `configOBUs` tail of the track's `av1C` record (`track.config[4..]`) for
//! MP4, or the CodecPrivate tail for Matroska `V_AV1` (same record, RFC 9559 / AV1-ISOBMFF §2.3);
//! each packet's `data` is a temporal unit (low-overhead OBUs, `obu_has_size_field` = 1); the
//! frame is `Pixels::I420` from the planes (8-bit) stamped with `track.to_ns(pkt.pts)`; AV1 has
//! no reorder delay at the container level (frames leave in pts order with `show_frame` /
//! `show_existing_frame`), so `flush` returns nothing.
//!
//! Until then `new` refuses with [`DecodeError::Unsupported`] naming the missing fold, so a
//! player built with `--features av1` behaves exactly like one without it (stand-in test pattern,
//! labelled) instead of failing to compile.

use super::{DecodeError, Frame, VideoDecoder};
use crate::dsp::demux::{Packet, Track};

pub struct Av1Decoder {
    _config_obus: Vec<u8>,
}

impl Av1Decoder {
    pub fn new(track: &Track) -> Result<Av1Decoder, DecodeError> {
        let _ = track.config.get(4..).map(|c| c.to_vec()).unwrap_or_default();
        Err(DecodeError::Unsupported("AV1: av1_core (AVCODEC, LEDGER SR24) is not folded yet".into()))
    }
}

impl VideoDecoder for Av1Decoder {
    fn name(&self) -> &'static str {
        "av1 (stub)"
    }
    fn decode(&mut self, _pkt: &Packet) -> Result<Option<Frame>, DecodeError> {
        Err(DecodeError::Unsupported("AV1: av1_core not folded".into()))
    }
}
