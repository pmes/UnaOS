// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ATTRCOLUMNS (rmbp-ledger B402) — an audio file's FACTS from its headers, without decoding: how long it plays,
//! the codec, and the title its tags carry. The kernel writes them as typed attributes (`media:duration_ms`,
//! `media:codec`, `doc:title`) beside `una:type` — BeOS's `Audio:Length` and `Audio:Title`, written by the sniffer
//! that already read the bytes. Header walks only, every read bounds-checked (attacker-shaped input). Pure.
//!
//! The caller passes the file's HEAD (as much of the start as it read), the file's full length, and its TAIL (the
//! last bytes, for the Ogg end granule; the head itself when the whole file was read).

use alloc::string::String;
use alloc::vec::Vec;

/// What [`facts_of`] learned. `duration_ms` is `None` when the headers do not state it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFacts {
    pub duration_ms: Option<u64>,
    pub codec: &'static str,
    pub rate: u32,
    pub channels: u16,
    pub title: Option<String>,
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}
fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
}
fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}
fn le32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
}
fn le64(b: &[u8], i: usize) -> Option<u64> {
    Some(le32(b, i)? as u64 | (le32(b, i + 4)? as u64) << 32)
}
fn ms(frames: u64, rate: u32) -> Option<u64> {
    if rate == 0 { None } else { Some(frames.saturating_mul(1000) / rate as u64) }
}

/// The facts of an audio file (`None` for bytes this core does not sniff as audio; ISO-BMFF is `demux_core`'s).
pub fn facts_of(head: &[u8], file_len: u64, tail: &[u8]) -> Option<AudioFacts> {
    match crate::sniff(head) {
        crate::Format::Wav => wav(head, file_len),
        crate::Format::Aiff => aiff(head),
        crate::Format::Flac if head.starts_with(b"fLaC") => flac(head),
        crate::Format::Ogg => ogg(head, tail),
        crate::Format::Mp3 | crate::Format::Adts | crate::Format::Flac => mpeg(head, file_len),
        _ => None,
    }
}

/// RIFF WAVE: `fmt ` (channels, rate, block align) and the `data` chunk's size.
fn wav(b: &[u8], file_len: u64) -> Option<AudioFacts> {
    let mut at = 12usize;
    let (mut rate, mut ch, mut align, mut tag) = (0u32, 0u16, 0u32, 1u32);
    for _ in 0..64 {
        let id = b.get(at..at + 4)?;
        let len = le32(b, at + 4)?;
        if id == b"fmt " {
            tag = le16(b, at + 8)?;
            ch = le16(b, at + 10)? as u16;
            rate = le32(b, at + 12)?;
            align = le16(b, at + 20)?;
            if tag == 0xFFFE {
                tag = le16(b, at + 32).unwrap_or(1);
            }
        } else if id == b"data" {
            let body = if len == u32::MAX { file_len.saturating_sub(at as u64 + 8) } else { len as u64 };
            let frames = if align > 0 { body / align as u64 } else { 0 };
            let codec = if tag == 3 { "pcm-float" } else { "pcm" };
            return Some(AudioFacts { duration_ms: ms(frames, rate), codec, rate, channels: ch, title: None });
        }
        at = at.checked_add(8 + len as usize + (len as usize & 1))?;
    }
    None
}

/// AIFF/AIFC: the `COMM` chunk (channels, sample frames, the 80-bit rate).
fn aiff(b: &[u8]) -> Option<AudioFacts> {
    let aifc = b.get(8..12)? == b"AIFC";
    let mut at = 12usize;
    for _ in 0..64 {
        let id = b.get(at..at + 4)?;
        let len = be32(b, at + 4)? as usize;
        if id == b"COMM" {
            let ch = be16(b, at + 8)? as u16;
            let frames = be32(b, at + 10)? as u64;
            let rate = crate::aiff::ext80_to_u32(b.get(at + 16..at + 26)?);
            let codec = match (aifc, b.get(at + 26..at + 30)) {
                (true, Some(c)) if c != b"NONE" && c != b"sowt" && c != b"twos" => if c == b"fl32" || c == b"fl64" { "pcm-float" } else { "aifc" },
                _ => "pcm",
            };
            return Some(AudioFacts { duration_ms: ms(frames, rate), codec, rate, channels: ch, title: None });
        }
        at = at.checked_add(8 + len + (len & 1))?;
    }
    None
}

/// Native FLAC: STREAMINFO (rate, channels, total samples) and the VORBIS_COMMENT's TITLE.
fn flac(b: &[u8]) -> Option<AudioFacts> {
    let mut at = 4usize;
    let mut out: Option<AudioFacts> = None;
    let mut title = None;
    for _ in 0..128 {
        let hdr = *b.get(at)?;
        let len = (*b.get(at + 1)? as usize) << 16 | (*b.get(at + 2)? as usize) << 8 | *b.get(at + 3)? as usize;
        let body = b.get(at + 4..at + 4 + len);
        match (hdr & 0x7F, body) {
            (0, Some(s)) if s.len() >= 18 => {
                let rate = (s[10] as u32) << 12 | (s[11] as u32) << 4 | (s[12] as u32) >> 4;
                let ch = ((s[12] >> 1) & 7) as u16 + 1;
                let total = ((s[13] & 0x0F) as u64) << 32 | (u32::from_be_bytes([s[14], s[15], s[16], s[17]]) as u64);
                out = Some(AudioFacts { duration_ms: if total == 0 { None } else { ms(total, rate) }, codec: "flac", rate, channels: ch, title: None });
            }
            (4, Some(s)) => title = vorbis_title(s),
            _ => {}
        }
        if hdr & 0x80 != 0 || body.is_none() {
            break;
        }
        at += 4 + len;
    }
    out.map(|mut f| {
        f.title = title;
        f
    })
}

/// A Vorbis comment block (vendor, then `count` `KEY=value` strings, all little-endian lengths): the TITLE.
fn vorbis_title(s: &[u8]) -> Option<String> {
    let vl = le32(s, 0)? as usize;
    let mut at = 4usize.checked_add(vl)?;
    let n = le32(s, at)?;
    at += 4;
    for _ in 0..n.min(1024) {
        let l = le32(s, at)? as usize;
        let c = s.get(at + 4..at + 4 + l)?;
        at += 4 + l;
        if c.len() > 6 && c[..6].eq_ignore_ascii_case(b"TITLE=") {
            let t: String = String::from(String::from_utf8_lossy(&c[6..]).trim());
            return if t.is_empty() { None } else { Some(t) };
        }
    }
    None
}

/// The pages' packets from an Ogg head, at most `want` packets (packets that span pages are joined).
fn ogg_packets(b: &[u8], want: usize) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut at = 0usize;
    while out.len() < want && b.get(at..at + 4) == Some(b"OggS") {
        let Some(&n) = b.get(at + 26) else { break };
        let Some(segs) = b.get(at + 27..at + 27 + n as usize) else { break };
        let mut p = at + 27 + n as usize;
        for &l in segs {
            let Some(d) = b.get(p..p + l as usize) else { return out };
            cur.extend_from_slice(d);
            p += l as usize;
            if l < 255 {
                out.push(core::mem::take(&mut cur));
                if out.len() >= want {
                    return out;
                }
            }
        }
        at = p;
    }
    out
}

/// Ogg (Opus, Vorbis, FLAC-in-Ogg): the identification header, the comment header's TITLE, and the last page's
/// granule position (RFC 7845 §4: Opus granules are 48 kHz samples, less the pre-skip).
fn ogg(head: &[u8], tail: &[u8]) -> Option<AudioFacts> {
    let pk = ogg_packets(head, 2);
    let id = pk.first()?;
    let (codec, rate, ch, pre, clock) = if id.starts_with(b"OpusHead") {
        ("opus", le32(id, 12)?, *id.get(9)? as u16, le16(id, 10)? as u64, 48_000u32)
    } else if id.starts_with(b"\x01vorbis") {
        let r = le32(id, 12)?;
        ("vorbis", r, *id.get(11)? as u16, 0, r)
    } else if id.starts_with(b"\x7FFLAC") {
        let s = id.get(17..)?;
        let r = (*s.get(10)? as u32) << 12 | (*s.get(11)? as u32) << 4 | (*s.get(12)? as u32) >> 4;
        ("flac", r, ((s.get(12)? >> 1) & 7) as u16 + 1, 0, r)
    } else {
        return None;
    };
    let title = pk.get(1).and_then(|c| {
        if c.starts_with(b"OpusTags") {
            vorbis_title(&c[8..])
        } else if c.starts_with(b"\x03vorbis") {
            vorbis_title(&c[7..])
        } else if c.first().map(|h| h & 0x7F) == Some(4) {
            vorbis_title(c.get(4..)?)
        } else {
            None
        }
    });
    // The LAST page's granule: scan the tail backwards for a capture pattern with a sane header.
    let mut gran = None;
    let mut i = tail.len().saturating_sub(27);
    loop {
        if tail.get(i..i + 4) == Some(b"OggS") && tail.get(i + 4) == Some(&0) {
            if let Some(g) = le64(tail, i + 6) {
                if g != u64::MAX {
                    gran = Some(g);
                    break;
                }
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let duration_ms = gran.and_then(|g| ms(g.saturating_sub(pre), clock));
    Some(AudioFacts { duration_ms, codec, rate, channels: ch, title })
}

/// The ID3v2 tag's size (header included), and its TIT2/TT2 title.
fn id3(b: &[u8]) -> (usize, Option<String>) {
    if b.len() < 10 || &b[..3] != b"ID3" {
        return (0, None);
    }
    let ss = |i: usize| -> usize { b[i..i + 4].iter().fold(0usize, |a, &c| a << 7 | (c & 0x7F) as usize) };
    let size = ss(6);
    let total = 10 + size + if b[5] & 0x10 != 0 { 10 } else { 0 };
    let ver = b[3];
    let end = (10 + size).min(b.len());
    let mut at = 10usize;
    if ver >= 3 && b[5] & 0x40 != 0 {
        // Extended header: v2.4 synchsafe size including itself; v2.3 plain size excluding its 4 bytes.
        if let Some(n) = be32(b, 10) {
            at += if ver == 4 { ss(10) } else { n as usize + 4 };
        }
    }
    let mut title = None;
    while at + 10 <= end {
        let (id, fsize, hl) = if ver == 2 {
            let Some(i) = b.get(at..at + 3) else { break };
            (i, (b[at + 3] as usize) << 16 | (b[at + 4] as usize) << 8 | b[at + 5] as usize, 6)
        } else {
            let n = if ver == 4 { ss(at + 4) } else { be32(b, at + 4).unwrap_or(0) as usize };
            (&b[at..at + 4], n, 10)
        };
        if id[0] == 0 || fsize == 0 {
            break;
        }
        if id == b"TIT2" || id == b"TT2" {
            if let Some(body) = b.get(at + hl..at + hl + fsize) {
                title = id3_text(body);
            }
            break;
        }
        at += hl + fsize;
    }
    (total, title)
}

/// An ID3 text frame's body: the encoding byte (0 Latin-1, 1 UTF-16 with BOM, 2 UTF-16BE, 3 UTF-8), the text.
fn id3_text(body: &[u8]) -> Option<String> {
    let (&enc, t) = body.split_first()?;
    let s: String = match enc {
        0 => t.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect(),
        3 => String::from_utf8_lossy(t.split(|&c| c == 0).next().unwrap_or(&[])).into(),
        1 | 2 => {
            let (le, t) = match t {
                [0xFF, 0xFE, r @ ..] => (true, r),
                [0xFE, 0xFF, r @ ..] => (false, r),
                r => (enc == 1, r),
            };
            let u: Vec<u16> = t.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).take_while(|&c| c != 0).collect();
            char::decode_utf16(u).map(|r| r.unwrap_or('\u{FFFD}')).collect()
        }
        _ => return None,
    };
    let s = String::from(s.trim());
    if s.is_empty() { None } else { Some(s) }
}

const SR_MPEG: [u32; 3] = [44_100, 48_000, 32_000];
const SR_ADTS: [u32; 13] = [96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350];
const BR_V1: [[u16; 14]; 3] = [
    [32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
    [32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
    [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
];
const BR_V2: [[u16; 14]; 2] = [
    [32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
    [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
];

/// MP3 (and ADTS, possibly behind an ID3v2 tag): the first frame header, then the Xing/Info or VBRI frame count,
/// else the CBR estimate from the stream's length; ADTS by walking its frame headers.
fn mpeg(b: &[u8], file_len: u64) -> Option<AudioFacts> {
    let (tag, title) = id3(b);
    let mut i = tag;
    let lim = (tag + 8192).min(b.len().saturating_sub(4));
    while i < lim && !(b[i] == 0xFF && b[i + 1] & 0xE0 == 0xE0) {
        i += 1;
    }
    let h = b.get(i..i + 4)?;
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    if h[1] & 0xF6 == 0xF0 {
        return adts(b, i, file_len, title);
    }
    let ver = (h[1] >> 3) & 3; // 3 MPEG-1, 2 MPEG-2, 0 MPEG-2.5
    let layer = (h[1] >> 1) & 3; // 3 = Layer I, 2 = II, 1 = III
    let (bi, si) = ((h[2] >> 4) as usize, ((h[2] >> 2) & 3) as usize);
    if ver == 1 || layer == 0 || bi == 0 || bi == 15 || si == 3 {
        return None;
    }
    let rate = SR_MPEG[si] >> match ver { 3 => 0, 2 => 1, _ => 2 };
    let kbps = if ver == 3 { BR_V1[3 - layer as usize][bi - 1] } else { BR_V2[if layer == 3 { 0 } else { 1 }][bi - 1] } as u64;
    let spf: u64 = match (layer, ver) {
        (3, _) => 384,
        (2, _) => 1152,
        (_, 3) => 1152,
        _ => 576,
    };
    let mono = h[3] >> 6 == 3;
    let side = match (ver == 3, mono) {
        (true, false) => 32,
        (true, true) => 17,
        (false, false) => 17,
        (false, true) => 9,
    };
    let codec = match layer { 3 => "mp1", 2 => "mp2", _ => "mp3" };
    let ch = if mono { 1 } else { 2 };
    let x = i + 4 + if h[1] & 1 == 0 { 2 } else { 0 } + side;
    // Xing/Info: the frame count, and a LAME/Lavf tag's encoder delay and padding (the samples a gapless decoder
    // drops — this core's own `mp3::Mp3Stream` rule: delay + max(padding, 529)).
    let samples = if b.get(x..x + 4) == Some(b"Xing") || b.get(x..x + 4) == Some(b"Info") {
        let fl = be32(b, x + 4)?;
        let mut q = x + 8;
        let mut frames = None;
        if fl & 1 != 0 {
            frames = be32(b, q).map(|n| n as u64);
            q += 4;
        }
        q += if fl & 2 != 0 { 4 } else { 0 } + if fl & 4 != 0 { 100 } else { 0 } + if fl & 8 != 0 { 4 } else { 0 };
        let lame = matches!(b.get(q..q + 4), Some(t) if t == b"LAME" || t == b"Lavc" || t == b"Lavf");
        frames.map(|f| match b.get(q + 21..q + 24) {
            Some(d) if lame => {
                let delay = (d[0] as u64) << 4 | (d[1] as u64) >> 4;
                let pad = ((d[1] & 15) as u64) << 8 | d[2] as u64;
                (f * spf).saturating_sub(delay + pad.max(529))
            }
            _ => f * spf,
        })
    } else if b.get(i + 36..i + 40) == Some(b"VBRI") {
        be32(b, i + 36 + 14).map(|n| n as u64 * spf)
    } else {
        None
    };
    let duration_ms = match samples {
        Some(n) => ms(n, rate),
        None => {
            let v1 = if b.len() as u64 == file_len && b.len() >= 128 && &b[b.len() - 128..b.len() - 125] == b"TAG" { 128 } else { 0 };
            let bytes = file_len.saturating_sub(i as u64 + v1);
            if kbps == 0 { None } else { Some(bytes * 8 / kbps) }
        }
    };
    Some(AudioFacts { duration_ms, codec, rate, channels: ch, title })
}

/// ADTS (ISO/IEC 13818-7 §6.2): walk the frame headers, 1024 samples per raw data block; a walk that stops at the
/// end of the HEAD (not the file) is scaled by the bytes it covered.
fn adts(b: &[u8], start: usize, file_len: u64, title: Option<String>) -> Option<AudioFacts> {
    let h = b.get(start..start + 7)?;
    let si = ((h[2] >> 2) & 0xF) as usize;
    let rate = *SR_ADTS.get(si)?;
    let ch = (((h[2] & 1) << 2) | (h[3] >> 6)) as u16;
    let (mut at, mut samples) = (start, 0u64);
    while let Some(f) = b.get(at..at + 7) {
        if f[0] != 0xFF || f[1] & 0xF6 != 0xF0 {
            break;
        }
        let len = ((f[3] as usize & 3) << 11) | (f[4] as usize) << 3 | (f[5] as usize) >> 5;
        if len < 7 {
            break;
        }
        samples += 1024 * ((f[6] & 3) as u64 + 1);
        at += len;
    }
    let walked = (at - start) as u64;
    let rest = file_len.saturating_sub(start as u64);
    if (b.len() as u64) < file_len && walked > 0 {
        samples = samples * rest / walked;
    }
    Some(AudioFacts { duration_ms: ms(samples, rate), codec: "aac", rate, channels: ch, title })
}
