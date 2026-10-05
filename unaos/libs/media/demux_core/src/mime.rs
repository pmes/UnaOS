// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! OPENERS (rmbp-ledger B379) — the MIME type of a container file, for the kernel's type table
//! (`unaos/crates/kernel/src/fs/filetype.rs`) and any ring-3 caller. CHARTER: Stria — shared-core (the same
//! container knowledge [`crate::Demuxer`] demuxes by; no second parser).
//!
//! * **ISO-BMFF** (ISO/IEC 14496-12; RFC 4337 for the media types): `audio/mp4` when the file holds sound and no
//!   visual track, `video/mp4` when it holds video. Decided, in this order, by the tracks themselves — every
//!   `moov/trak/mdia/hdlr` handler type (§8.4.3: `vide`, `soun`) — then by a brand that names the content (`ftyp`
//!   §4.3: `M4A ` `M4B ` `M4P ` `F4A ` `F4B ` audio; `M4V ` `M4VH` `M4VP` `F4V ` `av01` `avc1` `qt  ` video). The
//!   general brands (`isom`, `iso2`..`iso9`, `mp41`, `mp42`, `dash`, …) say nothing, and a file nothing decides is
//!   `video/mp4` (RFC 4337 §2: the type for the general case).
//! * **Matroska** (RFC 9559 §11.2): the EBML header's DocType — `webm` → `video/webm`, `matroska` →
//!   `video/x-matroska`.
//!
//! No I/O: the caller hands it bytes. [`iso_box_header`] lets a caller that reads a file in pieces walk the
//! top-level boxes to a `moov` that sits after the `mdat` without reading the media.
use crate::mkv::{read_id, read_vint};
use crate::read::Reader;
use crate::{mp4, TrackKind};

pub const AUDIO_MP4: &str = "audio/mp4";
pub const VIDEO_MP4: &str = "video/mp4";
pub const VIDEO_WEBM: &str = "video/webm";
pub const VIDEO_MATROSKA: &str = "video/x-matroska";

/// One top-level ISO-BMFF box header at the front of `b`: `(type, size, header length)`. `size == 0` means
/// "to the end of the file" (§4.2); a 64-bit `largesize` (`size == 1`) is read. `None` when `b` is shorter
/// than the header or the size is smaller than the header. Pure.
pub fn iso_box_header(b: &[u8]) -> Option<([u8; 4], u64, usize)> {
    let s = u32::from_be_bytes(b.get(0..4)?.try_into().ok()?);
    let ty: [u8; 4] = b.get(4..8)?.try_into().ok()?;
    let (size, hdr) = match s {
        0 => (0u64, 8usize),
        1 => (u64::from_be_bytes(b.get(8..16)?.try_into().ok()?), 16usize),
        n => (n as u64, 8usize),
    };
    if size != 0 && size < hdr as u64 {
        return None;
    }
    Some((ty, size, hdr))
}

/// What an `ftyp` payload's brands say, when one of them names the content. Pure.
pub fn iso_brand_kind(ftyp_body: &[u8]) -> Option<TrackKind> {
    let brand = |b: &[u8]| -> Option<TrackKind> {
        match b {
            b"M4A " | b"M4B " | b"M4P " | b"F4A " | b"F4B " => Some(TrackKind::Audio),
            b"M4V " | b"M4VH" | b"M4VP" | b"F4V " | b"av01" | b"avc1" | b"qt  " => Some(TrackKind::Video),
            _ => None,
        }
    };
    // major_brand, minor_version, then compatible_brands to the end of the box.
    if let Some(k) = ftyp_body.get(0..4).and_then(brand) {
        return Some(k);
    }
    ftyp_body.get(8..).unwrap_or(&[]).chunks_exact(4).find_map(brand)
}

/// What a `moov` payload's tracks are: `Video` if any `trak`'s handler is `vide`, else `Audio` if any is `soun`,
/// else `None`. Pure.
pub fn iso_moov_kind(moov_body: &[u8]) -> Option<TrackKind> {
    let traks = mp4::boxes(moov_body, 0, moov_body.len()).ok()?;
    let (mut audio, mut video) = (false, false);
    for t in traks.iter().filter(|b| &b.kind == b"trak") {
        let Ok(kids) = mp4::boxes(moov_body, t.body, t.end) else { continue };
        let Some(mdia) = kids.iter().find(|b| &b.kind == b"mdia") else { continue };
        let Ok(mk) = mp4::boxes(moov_body, mdia.body, mdia.end) else { continue };
        let Some(h) = mk.iter().find(|b| &b.kind == b"hdlr") else { continue };
        // FullBox (version + flags, 4), pre_defined (4), handler_type (4).
        match moov_body.get(h.body + 8..h.body + 12) {
            Some(b"vide") => video = true,
            Some(b"soun") => audio = true,
            _ => {}
        }
    }
    if video {
        Some(TrackKind::Video)
    } else if audio {
        Some(TrackKind::Audio)
    } else {
        None
    }
}

/// The ISO-BMFF type from whichever of the `ftyp` and `moov` payloads the caller found: the tracks first, then
/// the brands, then `video/mp4`. Pure.
pub fn iso_mime(ftyp_body: Option<&[u8]>, moov_body: Option<&[u8]>) -> &'static str {
    let kind = moov_body.and_then(iso_moov_kind).or_else(|| ftyp_body.and_then(iso_brand_kind));
    if kind == Some(TrackKind::Audio) { AUDIO_MP4 } else { VIDEO_MP4 }
}

/// The Matroska DocType's type from the front of the file (the EBML header). `None` if not EBML. Pure.
pub fn ebml_mime(b: &[u8]) -> Option<&'static str> {
    let mut r = Reader::new(b);
    if read_id(&mut r).ok()? != 0x1A45_DFA3 {
        return None;
    }
    let (len, _) = read_vint(&mut r).ok()?;
    let end = r.pos().saturating_add(usize::try_from(len).unwrap_or(usize::MAX)).min(b.len());
    while r.pos() < end {
        let id = read_id(&mut r).ok()?;
        let (n, _) = read_vint(&mut r).ok()?;
        let body = r.bytes(usize::try_from(n).ok()?).ok()?;
        if id == 0x4282 {
            let s = body.split(|&c| c == 0).next().unwrap_or(body);
            return Some(if s == b"webm" { VIDEO_WEBM } else { VIDEO_MATROSKA });
        }
    }
    Some(VIDEO_MATROSKA)
}

/// The container type of a file from bytes at its front (all of it, or a head): Matroska by its DocType;
/// ISO-BMFF by whatever `ftyp` and complete `moov` lie inside `b` ([`iso_mime`]). `None` if neither. Pure.
pub fn mime_of(b: &[u8]) -> Option<&'static str> {
    match crate::probe(b)? {
        crate::Format::Matroska | crate::Format::WebM => ebml_mime(b),
        crate::Format::Mp4 => {
            let (mut ftyp, mut moov) = (None, None);
            let mut p = 0usize;
            for _ in 0..64 {
                let Some((ty, size, hdr)) = b.get(p..).and_then(iso_box_header) else { break };
                let end = if size == 0 { b.len() as u64 } else { p as u64 + size };
                let Ok(end) = usize::try_from(end) else { break };
                if end > b.len() {
                    break;
                }
                match &ty {
                    b"ftyp" => ftyp = Some(&b[p + hdr..end]),
                    b"moov" => moov = Some(&b[p + hdr..end]),
                    _ => {}
                }
                if end <= p {
                    break;
                }
                p = end;
            }
            Some(iso_mime(ftyp, moov))
        }
    }
}
