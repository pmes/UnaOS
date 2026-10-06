// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Facet — shared-core
//!
//! RAWCORE (rmbp-ledger B444) — Facet opens a camera raw (Sony ARW; a plain TIFF by its preview). The decoder
//! is `raw_core` (the `no_std` core lux re-exports on the host), reached through `pixel_core::raw` — the seam;
//! this file only moves bytes from the VFS into it, so the kernel holds no second parser.
//!
//! * FULL ([`develop`], Facet's open): the IFDs are read from the file's head; the raw strip is STREAMED in
//!   `CHUNK`-sized runs of rows through `raw_core::RowDecoder` into `raw_core::Binner` at the window's integer
//!   scale `k` — one row of samples and one accumulator row are held, never the 24-to-60-megapixel mosaic
//!   (a 48 MiB kernel heap could not). A raw that fits the window (`k == 1`) is demosaiced bilinear whole.
//! * FAST ([`preview`], Quick Look and the wallpaper through `decode_file`): the embedded JPEG, decoded by
//!   pixel_core's JPEG decoder, box-reduced by the viewer's own `reduce_rgba`.
//!
//! Wire: `[facet] raw path=<p> w=<n> h=<n> compression=<1 or 32767> preview=<ok|none> demosaic_ms=<n> k=<n>`.
//! `tests rawcore` writes the synthetic ARW (`raw_core::synth`) to the home, opens it both ways, reads its facts:
//! `:: RAWCORE: tiff=ok ifds=<n> preview=<ok> demosaic=<ok> facts=<n> -> PASS ::`. R80: run only when asked.

use alloc::string::String;
use alloc::vec::Vec;

use super::{fit, reduce_rgba, vfs_why, FacetError, Ihdr, CHUNK};
use crate::fs::vfs::MountTable;
use pixel_core::raw::{map_err, raw_core};

/// The largest raw file Facet streams (the strip is never held whole, so the bound is the medium's, not the heap's).
pub const MAX_RAW: u64 = 1 << 30;
/// The head the IFDs are read from (an ARW's IFD0, SubIFD and EXIF IFD sit in its first few KiB).
const HEAD: u64 = 256 << 10;
/// The largest embedded preview read (Sony's full-screen preview is ~1616x1080, a few hundred KiB).
const MAX_PREVIEW: u64 = 16 << 20;

/// Is `path` a TIFF container Facet takes on this path (`II*\0` / `MM\0*`, at most [`MAX_RAW`])?
pub fn takes(mt: &MountTable, path: &str, size: u64) -> bool {
    size >= 8 && size <= MAX_RAW && mt.read(path, 0, 8).map(|h| raw_core::is_tiff(&h)).unwrap_or(false)
}

fn rd(mt: &MountTable, path: &str, off: u64, len: usize) -> Result<Vec<u8>, FacetError> {
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve_exact(len).is_err() {
        return Err(FacetError::OutOfMemory(len));
    }
    while out.len() < len {
        let want = core::cmp::min(CHUNK, len - out.len());
        let b = mt.read(path, off + out.len() as u64, want).map_err(|e| FacetError::Vfs(vfs_why(e)))?;
        if b.is_empty() {
            return Err(FacetError::Vfs(String::from("short-read")));
        }
        out.extend_from_slice(&b);
    }
    Ok(out)
}

fn info_of(mt: &MountTable, path: &str, size: u64) -> Result<raw_core::RawInfo, FacetError> {
    let head = rd(mt, path, 0, core::cmp::min(size, HEAD) as usize)?;
    raw_core::parse(&head).map_err(|e| FacetError::Pixel(map_err(e)))
}

/// FAST: the embedded JPEG, decoded and box-reduced into `bw x bh`.
pub fn preview(mt: &MountTable, path: &str, size: u64, bw: usize, bh: usize) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    let info = info_of(mt, path, size)?;
    let Some((o, l)) = info.preview_in(size).filter(|&(_, l)| l <= MAX_PREVIEW) else {
        // No preview (a TIFF this core reads only by its strip): the full path, at the asked size.
        return develop_with(mt, path, size, bw, bh, &info);
    };
    let jpeg = rd(mt, path, o, l as usize)?;
    let img = pixel_core::decode_jpeg(&jpeg).map_err(FacetError::Pixel)?;
    drop(jpeg);
    reduce_rgba(&img, bw, bh)
}

/// FULL: the raw strip developed into `bw x bh` (one `[facet] raw` line).
pub fn develop(mt: &MountTable, path: &str, size: u64, bw: usize, bh: usize) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    let info = info_of(mt, path, size)?;
    if info.strip.is_none() {
        // A plain TIFF: its preview is the picture this core can show.
        return preview(mt, path, size, bw, bh);
    }
    develop_with(mt, path, size, bw, bh, &info)
}

fn develop_with(mt: &MountTable, path: &str, size: u64, bw: usize, bh: usize, info: &raw_core::RawInfo) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    let t0 = crate::arch::ms();
    let px_err = |e: raw_core::Error| FacetError::Pixel(map_err(e));
    let s = info.strip.as_ref().ok_or(px_err(raw_core::Error::NoRaw))?;
    if s.offset.checked_add(s.len).map(|e| e > size).unwrap_or(true) {
        return Err(px_err(raw_core::Error::Truncated));
    }
    let d = raw_core::RowDecoder::new(s).map_err(px_err)?;
    let tone = raw_core::Tone::new(s.black, s.white, d.max_code).map_err(px_err)?;
    let (k, ow, oh) = fit(s.width, s.height, bw, bh).ok_or(FacetError::NoWindow("fit"))?;
    let rb = d.row_bytes();
    let rgba: Vec<u8> = if k == 1 {
        let strip = rd(mt, path, s.offset, rb * d.height)?;
        let m = d.all(&strip).map_err(px_err)?;
        drop(strip);
        let full = raw_core::bilinear_rgba(&m, d.width, d.height, &s.cfa, &tone).map_err(px_err)?;
        // `fit` drops nothing at k == 1, so the full image is the base.
        full
    } else {
        let mut bin = raw_core::Binner::new(k, ow, oh, s.cfa).map_err(px_err)?;
        let mut row: Vec<u16> = Vec::new();
        if row.try_reserve_exact(d.width).is_err() {
            return Err(FacetError::OutOfMemory(d.width * 2));
        }
        row.resize(d.width, 0);
        let rows = oh * k; // rows past the last full block are dropped, as `fit` says
        let per = core::cmp::max(1, CHUNK / rb);
        let mut y = 0usize;
        while y < rows {
            let n = core::cmp::min(per, rows - y);
            let run = rd(mt, path, s.offset + (y * rb) as u64, n * rb)?;
            for i in 0..n {
                d.row(&run[i * rb..(i + 1) * rb], &mut row).map_err(px_err)?;
                bin.row(y + i, &row, &tone);
            }
            y += n;
        }
        bin.rgba
    };
    let (w, h) = if k == 1 { (s.width as usize, s.height as usize) } else { (ow, oh) };
    let mut px: Vec<u32> = Vec::new();
    if px.try_reserve_exact(w * h).is_err() {
        return Err(FacetError::OutOfMemory(w * h * 4));
    }
    px.extend(rgba.chunks_exact(4).map(|p| 0xFF00_0000 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32));
    serial_println!(
        "[facet] raw path={} w={} h={} compression={} preview={} demosaic_ms={} k={}",
        path, s.width, s.height, s.compression, if info.preview_in(size).is_some() { "ok" } else { "none" }, crate::arch::ms().saturating_sub(t0), k
    );
    Ok((Ihdr { width: s.width, height: s.height, depth: 8, colour: 6, interlaced: false }, k, w, h, px))
}

// ── `tests rawcore` ─────────────────────────────────────────────────────────────────────────────────

/// `tests rawcore` — the SYNTHETIC ARW (`raw_core::synth`, 64x32, uncompressed 14-bit; a 64x2 cRAW twin) written to
/// the home, then: its type (`image/x-sony-arw`), the container (IFDs, strip), the FAST path (the preview through
/// pixel_core's JPEG decoder), the FULL path at k=1 (bilinear) and k=2 (the streaming binner), the cRAW twin, and
/// ATTRCOLUMNS's facts. Removed after. R80: only when asked. No real Sony file is involved: a real ARW's first
/// flight is the `[facet] raw` line on Peter's card.
pub fn selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    use raw_core::synth::{self, Coding};
    let mt = crate::shell::vfs_mount_table();
    let home = crate::fs::trash::home_base();
    let (pa, pc) = (alloc::format!("{}/RAWTEST.ARW", home), alloc::format!("{}/RAWTESTC.ARW", home));
    let (w, h) = (64u32, 32u32);
    let file = synth::arw(w, h, &synth::ramp(w, h), Coding::Plain14);
    let codes: Vec<u16> = (0..64 * 2).map(|i| 200 + (i % 64) as u16).collect();
    let craw = synth::arw(64, 2, &codes, Coding::Craw);
    for (p, b) in [(&pa, &file), (&pc, &craw)] {
        let _ = mt.unlink(p, P);
        if mt.create(p, NodeKind::File, P).is_err() || mt.write(p, 0, b, P).is_err() {
            serial_println!(":: RAWCORE: could not write {} -> FAIL ::", p);
            let _ = mt.unlink(&pa, P);
            let _ = mt.unlink(&pc, P);
            return;
        }
    }
    let size = file.len() as u64;
    let (mime, src) = crate::fs::filetype::type_of_in(&mt, &pa);
    let info = raw_core::parse(&file);
    let tiff_ok = info.as_ref().map(|i| i.strip.is_some()).unwrap_or(false);
    let ifds = info.as_ref().map(|i| i.ifds).unwrap_or(0);
    let prev = preview(&mt, &pa, size, 32, 32).map(|(i, k, ow, oh, _)| (i.width, i.height, k, ow, oh));
    let full = develop(&mt, &pa, size, 128, 128).map(|(_, k, ow, oh, px)| (k, ow, oh, px.len()));
    let bin = develop(&mt, &pa, size, 32, 16).map(|(_, k, ow, oh, px)| (k, ow, oh, px.len()));
    let cr = develop(&mt, &pc, craw.len() as u64, 128, 128).map(|(_, k, ow, oh, _)| (k, ow, oh));
    let j = pixel_core::decode_jpeg(synth::PREVIEW_JPEG).map(|i| (i.width, i.height)).ok();
    let preview_ok = matches!((&prev, j), (Ok((pw, ph, ..)), Some((jw, jh))) if *pw == jw && *ph == jh);
    let demosaic_ok = matches!(full, Ok((1, 64, 32, 2048))) && matches!(bin, Ok((2, 32, 16, 512))) && matches!(cr, Ok((1, 64, 2)));
    let r = crate::fs::attrfacts::refresh_in(&mt, &pa, true);
    let facts = match r {
        crate::fs::attrfacts::Refresh::Wrote(n) => n,
        _ => pixel_core::raw::facts(&file).map(|f| f.count()).unwrap_or(0),
    };
    let taken = mt.get_attr(&pa, crate::fs::attrfacts::TAKEN, P).ok();
    let attrs_ok = match r {
        crate::fs::attrfacts::Refresh::Wrote(_) => taken == Some(crate::fs::vfs::AttrValue::Int(synth::TAKEN_UNIX)),
        crate::fs::attrfacts::Refresh::NoAttrs => true, // a FAT home: the facts are computed, not stored
        _ => false,
    };
    let _ = mt.unlink(&pa, P);
    let _ = mt.unlink(&pc, P);
    serial_println!(
        "[rawcore] type={} src={} prev={:?} full={:?} bin={:?} craw={:?} facts_write={:?}",
        mime, src.name(), prev.as_ref().map(|p| (p.0, p.1)).map_err(|e| e.reason()), full.map_err(|e| e.reason()), bin.map_err(|e| e.reason()), cr.map_err(|e| e.reason()), r
    );
    let ok = mime == crate::fs::filetype::IMAGE_ARW && tiff_ok && ifds == 4 && preview_ok && demosaic_ok && facts >= 8 && attrs_ok;
    serial_println!(
        ":: RAWCORE: tiff={} ifds={} preview={} demosaic={} facts={} -> {} :: source=synthetic(no real ARW yet)",
        if tiff_ok { "ok" } else { "fail" }, ifds, if preview_ok { "ok" } else { "fail" }, if demosaic_ok { "ok" } else { "fail" }, facts,
        if ok { "PASS" } else { "FAIL" }
    );
}
