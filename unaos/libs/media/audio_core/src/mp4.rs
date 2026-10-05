// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ISO base media file format (ISO/IEC 14496-12; MP4/M4A, ISO/IEC 14496-14) — just enough to pull the first
//! audio track's access units out: `moov/trak` (`hdlr` = `soun`), the sample tables (`stsd`, `stsc`,
//! `stsz`/`stz2`, `stco`/`co64`), fragmented files (`moof/traf/tfhd/trun`, `mvex/trex` defaults), and the
//! gapless trim from the edit list (`elst`) or Apple's `iTunSMPB`.
//!
//! Codecs: `mp4a` with an `esds` naming MPEG-4 audio (0x40) or MPEG-2 AAC (0x66–0x68) → [`crate::aac`];
//! MPEG-1/2 audio (0x69, 0x6B) → [`crate::mp3`]. The file is read whole (the `moov` may follow the `mdat`).
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::io::ByteStream;
use crate::{Error, Result, Source};

fn be16(d: &[u8], o: usize) -> Option<u32> { d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32) }
fn be32(d: &[u8], o: usize) -> Option<u32> { d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])) }
fn be64(d: &[u8], o: usize) -> Option<u64> { d.get(o..o + 8).map(|b| u64::from_be_bytes(b.try_into().unwrap())) }

/// Iterate the boxes in `d[start..end]`: (type, payload start, payload end, box start).
fn boxes(d: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize, usize)> {
    let mut v = vec![];
    let mut p = start;
    let end = end.min(d.len());
    while p + 8 <= end {
        let Some(sz) = be32(d, p) else { break };
        let ty: [u8; 4] = d[p + 4..p + 8].try_into().unwrap();
        let (hdr, size) = match sz {
            0 => (8usize, (end - p) as u64),
            1 => match be64(d, p + 8) { Some(s) => (16, s), None => break },
            s => (8, s as u64),
        };
        if size < hdr as u64 || p as u64 + size > end as u64 {
            // a truncated last box: take what is there
            if size >= hdr as u64 { v.push((ty, p + hdr, end, p)); }
            break;
        }
        v.push((ty, p + hdr, p + size as usize, p));
        p += size as usize;
    }
    v
}

fn find(d: &[u8], s: usize, e: usize, ty: &[u8; 4]) -> Option<(usize, usize)> {
    boxes(d, s, e).into_iter().find(|b| &b.0 == ty).map(|b| (b.1, b.2))
}

/// MPEG-4 descriptor header (tag, payload start, payload end).
fn descriptor(d: &[u8], mut p: usize, end: usize) -> Option<(u8, usize, usize)> {
    let tag = *d.get(p)?;
    p += 1;
    let mut len = 0usize;
    for _ in 0..4 {
        let b = *d.get(p)?;
        p += 1;
        len = (len << 7) | (b & 0x7f) as usize;
        if b & 0x80 == 0 { break; }
    }
    if p + len > end { return Some((tag, p, end)); }
    Some((tag, p, p + len))
}

#[derive(Default)]
struct Track {
    id: u32,
    sound: bool,
    timescale: u32,
    oti: u8,
    dsi: Vec<u8>,
    rate: u32,
    channels: u32,
    sizes: Vec<u32>,
    offsets: Vec<u64>,
    elst: Option<(u64, i64)>,
    // trex defaults
    def_size: u32,
}

fn parse_esds(d: &[u8], s: usize, e: usize, t: &mut Track) {
    // FullBox header, then ES_Descriptor
    let Some((tag, mut p, end)) = descriptor(d, s + 4, e) else { return };
    if tag != 3 { return; }
    let Some(&flags) = d.get(p + 2) else { return };
    p += 3;
    if flags & 0x80 != 0 { p += 2; }
    if flags & 0x40 != 0 { p += 1 + *d.get(p).unwrap_or(&0) as usize; }
    if flags & 0x20 != 0 { p += 2; }
    let Some((tag, q, qend)) = descriptor(d, p, end) else { return };
    if tag != 4 { return; }
    t.oti = *d.get(q).unwrap_or(&0);
    if let Some((tag, r, rend)) = descriptor(d, q + 13, qend) {
        if tag == 5 { t.dsi = d[r..rend.min(d.len())].to_vec(); }
    }
}

fn parse_stbl(d: &[u8], s: usize, e: usize, t: &mut Track) {
    let mut stsc: Vec<(u32, u32)> = vec![];
    let mut chunks: Vec<u64> = vec![];
    for (ty, bs, be, _) in boxes(d, s, e) {
        match &ty {
            b"stsd" => {
                // first entry
                if let Some((ety, es, ee, _)) = boxes(d, bs + 8, be).into_iter().next() {
                    if &ety == b"mp4a" || &ety == b".mp3" {
                        let ver = be16(d, es + 8).unwrap_or(0);
                        t.channels = be16(d, es + 16).unwrap_or(0);
                        t.rate = be32(d, es + 24).unwrap_or(0) >> 16;
                        let child = es + 28 + match ver { 1 => 16, 2 => 36, _ => 0 };
                        if &ety == b".mp3" { t.oti = 0x6B; }
                        for (cty, cs, ce, _) in boxes(d, child, ee) {
                            if &cty == b"esds" { parse_esds(d, cs, ce, t); }
                            if &cty == b"wave" { if let Some((ws, we)) = find(d, cs, ce, b"esds") { parse_esds(d, ws, we, t); } }
                        }
                    }
                }
            }
            b"stsz" => {
                let uni = be32(d, bs + 4).unwrap_or(0);
                let n = be32(d, bs + 8).unwrap_or(0) as usize;
                t.sizes = if uni != 0 { vec![uni; n.min(d.len() / uni as usize + 1)] } else { (0..n).map_while(|i| be32(d, bs + 12 + 4 * i)).collect() };
            }
            b"stz2" => {
                let fs = *d.get(bs + 7).unwrap_or(&0);
                let n = be32(d, bs + 8).unwrap_or(0) as usize;
                t.sizes = (0..n).map_while(|i| match fs {
                    4 => d.get(bs + 12 + i / 2).map(|b| if i % 2 == 0 { (b >> 4) as u32 } else { (b & 15) as u32 }),
                    8 => d.get(bs + 12 + i).map(|&b| b as u32),
                    _ => be16(d, bs + 12 + 2 * i),
                }).collect();
            }
            b"stsc" => {
                let n = be32(d, bs + 4).unwrap_or(0) as usize;
                stsc = (0..n).map_while(|i| Some((be32(d, bs + 8 + 12 * i)?, be32(d, bs + 12 + 12 * i)?))).collect();
            }
            b"stco" => {
                let n = be32(d, bs + 4).unwrap_or(0) as usize;
                chunks = (0..n).map_while(|i| be32(d, bs + 8 + 4 * i).map(|x| x as u64)).collect();
            }
            b"co64" => {
                let n = be32(d, bs + 4).unwrap_or(0) as usize;
                chunks = (0..n).map_while(|i| be64(d, bs + 8 + 8 * i)).collect();
            }
            _ => {}
        }
    }
    // sample offsets from chunk offsets, samples-per-chunk runs and sizes
    let mut si = 0usize;
    for (ci, &co) in chunks.iter().enumerate() {
        let c1 = ci as u32 + 1;
        let spc = stsc.iter().rev().find(|r| r.0 <= c1).map(|r| r.1).unwrap_or(0);
        let mut o = co;
        for _ in 0..spc {
            if si >= t.sizes.len() { break; }
            t.offsets.push(o);
            o += t.sizes[si] as u64;
            si += 1;
        }
    }
    t.sizes.truncate(t.offsets.len());
}

fn parse_trak(d: &[u8], s: usize, e: usize, t: &mut Track) {
    if let Some((ts, _)) = find(d, s, e, b"tkhd") { t.id = if d.get(ts) == Some(&1) { be32(d, ts + 20) } else { be32(d, ts + 12) }.unwrap_or(0); }
    if let Some((es, ee)) = find(d, s, e, b"edts") {
        if let Some((ls, _)) = find(d, es, ee, b"elst") {
            let v1 = d.get(ls) == Some(&1);
            let n = be32(d, ls + 4).unwrap_or(0) as usize;
            let w = if v1 { 20 } else { 12 };
            for i in 0..n.min(16) {
                let p = ls + 8 + i * w;
                let (dur, mt) = if v1 { (be64(d, p).unwrap_or(0), be64(d, p + 8).unwrap_or(0) as i64) } else { (be32(d, p).unwrap_or(0) as u64, be32(d, p + 4).unwrap_or(0) as i32 as i64) };
                if mt >= 0 { t.elst = Some((dur, mt)); break; }
            }
        }
    }
    let Some((ms, me)) = find(d, s, e, b"mdia") else { return };
    if let Some((hs, _)) = find(d, ms, me, b"mdhd") { t.timescale = if d.get(hs) == Some(&1) { be32(d, hs + 20) } else { be32(d, hs + 12) }.unwrap_or(0); }
    if let Some((hs, _)) = find(d, ms, me, b"hdlr") { t.sound = d.get(hs + 8..hs + 12) == Some(b"soun"); }
    let Some((is, ie)) = find(d, ms, me, b"minf") else { return };
    if let Some((ss, se)) = find(d, is, ie, b"stbl") { parse_stbl(d, ss, se, t); }
}

/// iTunSMPB: " 00000000 PPPPPPPP QQQQQQQQ TTTTTTTTTTTTTTTT ..." → (priming, total samples).
fn itunsmpb(d: &[u8], s: usize, e: usize) -> Option<(u64, u64)> {
    let m = &d[s..e];
    let k = m.windows(8).position(|w| w == b"iTunSMPB")?;
    let rest = &m[k + 8..];
    let j = rest.windows(4).position(|w| w == b"data")?;
    let txt = &rest[j + 12..];
    let mut f = vec![];
    let mut cur: Option<u64> = None;
    for &c in txt.iter().take(120) {
        let v = (c as char).to_digit(16);
        match v { Some(x) => cur = Some(cur.unwrap_or(0) * 16 + x as u64), None => { if let Some(x) = cur.take() { f.push(x); } if c != b' ' { break; } } }
    }
    if let Some(x) = cur { f.push(x); }
    if f.len() < 4 { return None; }
    Some((f[1], f[3]))
}

pub fn open(mut s: ByteStream) -> Result<Box<dyn Source>> {
    let data = s.rest()?;
    let d = &data[..];
    let top = boxes(d, 0, d.len());
    let (mvs, mve) = top.iter().find(|b| &b.0 == b"moov").map(|b| (b.1, b.2)).ok_or(Error::Invalid("mp4: no moov"))?;
    let mut movie_ts = 0u32;
    if let Some((hs, _)) = find(d, mvs, mve, b"mvhd") { movie_ts = if d.get(hs) == Some(&1) { be32(d, hs + 20) } else { be32(d, hs + 12) }.unwrap_or(0); }
    let mut track = None;
    for (ty, ts, te, _) in boxes(d, mvs, mve) {
        if &ty != b"trak" { continue; }
        let mut t = Track::default();
        parse_trak(d, ts, te, &mut t);
        if t.sound && t.oti != 0 { track = Some(t); break; }
        if t.sound && track.is_none() { track = Some(t); }
    }
    let mut t = track.ok_or(Error::Unsupported("mp4: no audio track"))?;
    if t.oti == 0 { return Err(Error::Unsupported("mp4: audio codec (owed: only AAC and MP3 in MP4)")); }
    // fragments
    if let Some((xs, xe)) = find(d, mvs, mve, b"mvex") {
        for (ty, bs, _, _) in boxes(d, xs, xe) {
            if &ty == b"trex" && be32(d, bs + 4) == Some(t.id) { t.def_size = be32(d, bs + 16).unwrap_or(0); }
        }
    }
    for &(ty, ms, me, mstart) in &top {
        if &ty != b"moof" { continue; }
        for (fty, fs, fe, _) in boxes(d, ms, me) {
            if &fty != b"traf" { continue; }
            let Some((hs, _)) = find(d, fs, fe, b"tfhd") else { continue };
            let flags = be32(d, hs).unwrap_or(0) & 0xff_ffff;
            if be32(d, hs + 4) != Some(t.id) { continue; }
            let mut p = hs + 8;
            let mut base = mstart as u64;
            let mut def_size = t.def_size;
            if flags & 1 != 0 { base = be64(d, p).unwrap_or(0); p += 8; }
            if flags & 2 != 0 { p += 4; }
            if flags & 8 != 0 { p += 4; }
            if flags & 0x10 != 0 { def_size = be32(d, p).unwrap_or(0); }
            let mut pos = base;
            for (rty, rs, _, _) in boxes(d, fs, fe) {
                if &rty != b"trun" { continue; }
                let rf = be32(d, rs).unwrap_or(0) & 0xff_ffff;
                let n = be32(d, rs + 4).unwrap_or(0) as usize;
                let mut q = rs + 8;
                if rf & 1 != 0 { pos = (base as i64 + be32(d, q).unwrap_or(0) as i32 as i64) as u64; q += 4; }
                if rf & 4 != 0 { q += 4; }
                for _ in 0..n.min(d.len()) {
                    if rf & 0x100 != 0 { q += 4; }
                    let size = if rf & 0x200 != 0 { let v = be32(d, q).unwrap_or(0); q += 4; v } else { def_size };
                    if rf & 0x400 != 0 { q += 4; }
                    if rf & 0x800 != 0 { q += 4; }
                    t.offsets.push(pos);
                    t.sizes.push(size);
                    pos += size as u64;
                }
            }
        }
    }
    let units: Vec<(usize, usize)> = t.offsets.iter().zip(&t.sizes).map(|(&o, &s)| (o as usize, s as usize)).collect();
    if units.is_empty() { return Err(Error::Invalid("mp4: no samples")); }
    match t.oti {
        0x40 | 0x66 | 0x67 | 0x68 => {
            let asc = if t.dsi.is_empty() {
                // MPEG-2 AAC without a DecoderSpecificInfo: synthesise LC at the sample entry's rate
                let sfi = crate::aac::RATES.iter().position(|&r| r == t.rate).unwrap_or(4) as u16;
                let v: u16 = (2 << 11) | (sfi << 7) | ((t.channels as u16 & 15) << 3);
                v.to_be_bytes().to_vec()
            } else { t.dsi.clone() };
            let a = crate::aac::Asc::parse(&asc)?;
            let rate = crate::aac::RATES[a.sf_index] as u64;
            let mut skip = 0u64;
            let mut total = None;
            let ts = if t.timescale != 0 { t.timescale as u64 } else { rate };
            if let Some((dur, mt)) = t.elst {
                if mt > 0 || dur > 0 {
                    skip = (mt as u64 * rate + ts / 2) / ts;
                    if dur > 0 && movie_ts != 0 { total = Some((dur * rate + movie_ts as u64 / 2) / movie_ts as u64); }
                }
            }
            if skip == 0 {
                if let Some((pri, tot)) = itunsmpb(d, mvs, mve) { skip = pri; if tot > 0 { total = Some(tot); } }
            }
            Ok(crate::aac::AacSource::new(data, units, &asc, skip, total)?.boxed())
        }
        0x69 | 0x6B => {
            let mut v = Vec::new();
            for &(o, n) in &units { if let Some(b) = data.get(o..o + n) { v.extend_from_slice(b); } }
            let bs = ByteStream::new(Box::new(crate::io::VecReader::new(v)));
            Ok(Box::new(crate::mp3::Mp3Stream::new(bs)?))
        }
        _ => Err(Error::Unsupported("mp4: audio codec (owed: only AAC and MP3 in MP4)")),
    }
}
