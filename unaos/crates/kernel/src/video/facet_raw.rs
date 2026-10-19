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
//! * HALF (RAWVIEW, B711 — the one call in [`preview`] when the file carries NO embedded JPEG): the strip streamed
//!   through `raw_core::Binner` at `k = 2 * j` with a `raw_core::WbTone` (the as-shot white balance, a 2.2 gamma) —
//!   the half-size demosaic, reduced by `j` only when the half image is larger than the asked box.
//!
//! Wire: `[facet] raw path=<p> w=<n> h=<n> compression=<1 or 32767> preview=<ok|none> demosaic_ms=<n> k=<n>`;
//! `[raw] arw=<craw|plain> mosaic=<w>x<h> bits=<n> demosaic=half ms=<n> half=<w>x<h> j=<n> wb=<asshot|sony|none>`.
//! `tests rawcore` writes the synthetic ARWs (`raw_core::synth`) to the home, or — a home no fixture may write
//! (QEMU's FAT root) — runs the same legs on the bytes in RAM (UNAFSGROW's `img=ram` leg), opens them all three
//! ways, reads their facts: `:: RAWCORE: preview=ok mosaic=ok half=<w>x<h> store=<home|ram> … -> PASS ::`. R80:
//! run only when asked. CR2 and NEF stay typed-only (`image/tiff` by their container): no decoder for them here.

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

/// Where a raw's bytes come from: a VFS path, or a buffer already in memory (RAWVIEW M1: `tests rawcore` on a
/// medium no fixture may write).
enum Src<'a> {
    Vfs(&'a MountTable, &'a str),
    Mem(&'a [u8]),
}

impl Src<'_> {
    fn name(&self) -> &str {
        match self {
            Src::Vfs(_, p) => p,
            Src::Mem(_) => "ram",
        }
    }
}

fn rd(src: &Src, off: u64, len: usize) -> Result<Vec<u8>, FacetError> {
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve_exact(len).is_err() {
        return Err(FacetError::OutOfMemory(len));
    }
    let (mt, path) = match src {
        Src::Mem(b) => {
            let s = usize::try_from(off).ok().and_then(|o| b.get(o..o.checked_add(len)?)).ok_or_else(|| FacetError::Vfs(String::from("short-read")))?;
            out.extend_from_slice(s);
            return Ok(out);
        }
        Src::Vfs(mt, path) => (*mt, *path),
    };
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

fn info_of(src: &Src, size: u64) -> Result<raw_core::RawInfo, FacetError> {
    let head = rd(src, 0, core::cmp::min(size, HEAD) as usize)?;
    raw_core::parse(&head).map_err(|e| FacetError::Pixel(map_err(e)))
}

/// FAST: the embedded JPEG, decoded and box-reduced into `bw x bh`.
pub fn preview(mt: &MountTable, path: &str, size: u64, bw: usize, bh: usize) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    preview_src(&Src::Vfs(mt, path), size, bw, bh)
}

fn preview_src(src: &Src, size: u64, bw: usize, bh: usize) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    let info = info_of(src, size)?;
    let Some((o, l)) = info.preview_in(size).filter(|&(_, l)| l <= MAX_PREVIEW) else {
        // RAWVIEW (B711): no embedded JPEG — Quick Look shows the half-size develop (white-balanced, gamma 2.2).
        return half_with(src, size, bw, bh, &info);
    };
    let jpeg = rd(src, o, l as usize)?;
    let img = pixel_core::decode_jpeg(&jpeg).map_err(FacetError::Pixel)?;
    drop(jpeg);
    reduce_rgba(&img, bw, bh)
}

/// FULL: the raw strip developed into `bw x bh` (one `[facet] raw` line).
pub fn develop(mt: &MountTable, path: &str, size: u64, bw: usize, bh: usize) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    develop_src(&Src::Vfs(mt, path), size, bw, bh)
}

fn develop_src(src: &Src, size: u64, bw: usize, bh: usize) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    let info = info_of(src, size)?;
    if info.strip.is_none() {
        // A plain TIFF: its preview is the picture this core can show.
        return preview_src(src, size, bw, bh);
    }
    develop_with(src, size, bw, bh, &info)
}

/// Stream mosaic rows `0..rows` of strip `s` through `d`, `CHUNK`-sized runs at a time, handing each decoded row
/// to `f` — one row of samples held, never the mosaic.
fn stream_rows(src: &Src, s: &raw_core::Strip, d: &raw_core::RowDecoder, rows: usize, mut f: impl FnMut(usize, &[u16])) -> Result<(), FacetError> {
    let px_err = |e: raw_core::Error| FacetError::Pixel(map_err(e));
    let mut row: Vec<u16> = Vec::new();
    if row.try_reserve_exact(d.width).is_err() {
        return Err(FacetError::OutOfMemory(d.width * 2));
    }
    row.resize(d.width, 0);
    let rb = d.row_bytes();
    let per = core::cmp::max(1, CHUNK / rb.max(1));
    let mut y = 0usize;
    while y < rows {
        let n = core::cmp::min(per, rows - y);
        let run = rd(src, s.offset + (y * rb) as u64, n * rb)?;
        for i in 0..n {
            d.row(&run[i * rb..(i + 1) * rb], &mut row).map_err(px_err)?;
            f(y + i, &row[..]);
        }
        y += n;
    }
    Ok(())
}

fn pack(rgba: &[u8], n: usize) -> Result<Vec<u32>, FacetError> {
    let mut px: Vec<u32> = Vec::new();
    if px.try_reserve_exact(n).is_err() {
        return Err(FacetError::OutOfMemory(n * 4));
    }
    px.extend(rgba.chunks_exact(4).take(n).map(|p| 0xFF00_0000 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32));
    Ok(px)
}

/// HALF (RAWVIEW, B711): the half-size demosaic of the strip — each 2x2 CFA cell one RGB, white-balanced, gamma
/// 2.2 — reduced by `j` when the half image exceeds `bw x bh`. The `Ihdr` names the HALF image. One `[raw]` line.
fn half_with(src: &Src, size: u64, bw: usize, bh: usize, info: &raw_core::RawInfo) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
    let t0 = crate::arch::ms();
    let px_err = |e: raw_core::Error| FacetError::Pixel(map_err(e));
    let s = info.strip.as_ref().ok_or(px_err(raw_core::Error::NoRaw))?;
    if s.offset.checked_add(s.len).map(|e| e > size).unwrap_or(true) {
        return Err(px_err(raw_core::Error::Truncated));
    }
    let d = raw_core::RowDecoder::new(s).map_err(px_err)?;
    let tone = raw_core::WbTone::new(s.black, s.white, d.max_code, info.wb.as_ref()).map_err(px_err)?;
    let (hw, hh) = (d.width / 2, d.height / 2);
    let (j, ow, oh) = fit(hw as u32, hh as u32, bw, bh).ok_or(FacetError::NoWindow("fit"))?;
    let k = 2 * j;
    let mut bin = raw_core::Binner::new(k, ow, oh, s.cfa).map_err(px_err)?;
    stream_rows(src, s, &d, oh * k, |y, row| bin.row(y, row, &tone))?;
    let px = pack(&bin.rgba, ow * oh)?;
    serial_println!(
        "[raw] arw={} mosaic={}x{} bits={} demosaic=half ms={} half={}x{} j={} wb={} path={}",
        d.coding(), s.width, s.height, s.bits, crate::arch::ms().saturating_sub(t0), hw, hh, j, raw_core::Wb::name(info.wb.as_ref()), src.name()
    );
    Ok((Ihdr { width: hw as u32, height: hh as u32, depth: 8, colour: 6, interlaced: false }, j, ow, oh, px))
}

fn develop_with(src: &Src, size: u64, bw: usize, bh: usize, info: &raw_core::RawInfo) -> Result<(Ihdr, usize, usize, usize, Vec<u32>), FacetError> {
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
        let strip = rd(src, s.offset, rb * d.height)?;
        let m = d.all(&strip).map_err(px_err)?;
        drop(strip);
        let full = raw_core::bilinear_rgba(&m, d.width, d.height, &s.cfa, &tone).map_err(px_err)?;
        // `fit` drops nothing at k == 1, so the full image is the base.
        full
    } else {
        let mut bin = raw_core::Binner::new(k, ow, oh, s.cfa).map_err(px_err)?;
        // rows past the last full block are dropped, as `fit` says
        stream_rows(src, s, &d, oh * k, |y, row| bin.row(y, row, &tone))?;
        bin.rgba
    };
    let (w, h) = if k == 1 { (s.width as usize, s.height as usize) } else { (ow, oh) };
    let px = pack(&rgba, w * h)?;
    serial_println!(
        "[facet] raw path={} w={} h={} compression={} preview={} demosaic_ms={} k={}",
        src.name(), s.width, s.height, s.compression, if info.preview_in(size).is_some() { "ok" } else { "none" }, crate::arch::ms().saturating_sub(t0), k
    );
    Ok((Ihdr { width: s.width, height: s.height, depth: 8, colour: 6, interlaced: false }, k, w, h, px))
}

// ── `tests rawcore` ─────────────────────────────────────────────────────────────────────────────────

/// `tests rawcore` — three SYNTHETIC ARWs (`raw_core::synth`): A = 64x32 uncompressed 14-bit with its preview;
/// C = a 64x2 cRAW twin; B = 64x32 with NO preview and Sony's as-shot levels (2, 1, 1.5) in an encrypted SR2SubIFD
/// over a grey scene. Written to the home — or, when the home is a medium no fixture may write (QEMU's FAT root,
/// B588), held in RAM and read through the same `Src` (UNAFSGROW's `img=ram` leg): never a SKIP for the medium.
/// Legs: the type (`image/x-sony-arw`, A and B), the container (IFDs, strip), the FAST path (A's preview through
/// pixel_core's JPEG decoder), the FULL path at k=1 and k=2 and C's cRAW, the MOSAIC (every sample of A's 14-bit
/// strip and C's delta blocks against the codes written), the HALF path (Quick Look on B: 32x16, white-balanced
/// grey, R = G = B), and ATTRCOLUMNS's facts (stored on a writable home, computed otherwise). R80: only when asked.
/// No real Sony file is involved: a real ARW's first flight is the `[facet] raw` / `[raw]` line on Peter's card.
pub fn selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    use raw_core::synth::{self, Coding, Opts};
    let mt = crate::shell::vfs_mount_table();
    let home = crate::fs::trash::home_base();
    let (pa, pc, pb) = (alloc::format!("{}/RAWTEST.ARW", home), alloc::format!("{}/RAWTESTC.ARW", home), alloc::format!("{}/RAWTESTB.ARW", home));
    let (w, h) = (64u32, 32u32);
    let file = synth::arw(w, h, &synth::ramp(w, h), Coding::Plain14);
    let codes: Vec<u16> = (0..64 * 2).map(|i| 200 + (i % 64) as u16).collect();
    let craw = synth::arw(64, 2, &codes, Coding::Craw);
    let lv = [2048u16, 1024, 1024, 1536];
    let bare = synth::arw_with(w, h, &synth::grey(w, h, lv), Coding::Plain14, Opts { preview: false, wb_rggb: Some(lv), ..Opts::default() });
    let unlink_all = || {
        let _ = mt.unlink(&pa, P);
        let _ = mt.unlink(&pc, P);
        let _ = mt.unlink(&pb, P);
    };
    // M1 (B711): the home when a fixture may write it (QEMUSKIP's one question), else RAM.
    let mut why = "home";
    if let Some(k) = crate::fs::vfs::home_unwritable(&mt, &home) {
        why = if k == "fat" { "home-not-writable(fat)" } else { "home-not-writable" };
    } else {
        for (p, b) in [(&pa, &file), (&pc, &craw), (&pb, &bare)] {
            let _ = mt.unlink(p, P);
            if mt.create(p, NodeKind::File, P).is_err() || mt.write(p, 0, b, P).is_err() {
                why = "home-write-refused";
                unlink_all();
                break;
            }
        }
    }
    let on_home = why == "home";
    let (sa, sc, sb) = if on_home { (Src::Vfs(&mt, &pa), Src::Vfs(&mt, &pc), Src::Vfs(&mt, &pb)) } else { (Src::Mem(&file), Src::Mem(&craw), Src::Mem(&bare)) };
    let (mime, tsrc) = if on_home {
        let (m, s) = crate::fs::filetype::type_of_in(&mt, &pa);
        (m, s.name())
    } else {
        (String::from(crate::fs::filetype::sniff(&file).unwrap_or("-")), "sniffed")
    };
    let mime_b = crate::fs::filetype::sniff(&bare).unwrap_or("-");
    let size = file.len() as u64;
    let info = raw_core::parse(&file);
    let tiff_ok = info.as_ref().map(|i| i.strip.is_some()).unwrap_or(false);
    let ifds = info.as_ref().map(|i| i.ifds).unwrap_or(0);
    let prev = preview_src(&sa, size, 32, 32).map(|(i, k, ow, oh, _)| (i.width, i.height, k, ow, oh));
    let full = develop_src(&sa, size, 128, 128).map(|(_, k, ow, oh, px)| (k, ow, oh, px.len()));
    let bin = develop_src(&sa, size, 32, 16).map(|(_, k, ow, oh, px)| (k, ow, oh, px.len()));
    let cr = develop_src(&sc, craw.len() as u64, 128, 128).map(|(_, k, ow, oh, _)| (k, ow, oh));
    let j = pixel_core::decode_jpeg(synth::PREVIEW_JPEG).map(|i| (i.width, i.height)).ok();
    let preview_ok = matches!((&prev, j), (Ok((pw, ph, ..)), Some((jw, jh))) if *pw == jw && *ph == jh);
    let demosaic_ok = matches!(full, Ok((1, 64, 32, 2048))) && matches!(bin, Ok((2, 32, 16, 512))) && matches!(cr, Ok((1, 64, 2)));
    // M2: the mosaic, sample for sample — A's 14-bit containers and C's 8-bit delta blocks (no curve tag: dcraw's
    // default, `curve[code << 1] >> 2` = 8 * code).
    let decode_all = |f: &[u8]| -> Option<Vec<u16>> {
        let s = raw_core::parse(f).ok()?.strip?;
        let d = raw_core::RowDecoder::new(&s).ok()?;
        d.all(f.get(s.offset as usize..(s.offset + s.len) as usize)?).ok()
    };
    let mosaic_ok = decode_all(&file).as_deref() == Some(&synth::ramp(w, h)[..])
        && decode_all(&craw).map(|m| m.iter().zip(&codes).all(|(&got, &c)| got == c * 8) && m.len() == codes.len()).unwrap_or(false);
    // M3: Quick Look on B (no preview) — the half-size develop, white-balanced: every pixel grey.
    let half = preview_src(&sb, bare.len() as u64, 128, 128).map(|(i, k, ow, oh, px)| {
        let grey = px.iter().all(|&p| (p >> 16) & 0xff == p & 0xff && (p >> 8) & 0xff == p & 0xff);
        (i.width, i.height, k, ow, oh, grey)
    });
    let wb_b = raw_core::parse(&bare).ok().and_then(|i| i.wb);
    let half_ok = matches!(half, Ok((32, 16, 1, 32, 16, true))) && wb_b.map(|x| x.src) == Some(raw_core::WbSource::Sony);
    let (hw, hh) = half.as_ref().map(|t| (t.0, t.1)).unwrap_or((0, 0));
    let r = if on_home { Some(crate::fs::attrfacts::refresh_in(&mt, &pa, true)) } else { None };
    let facts = match r {
        Some(crate::fs::attrfacts::Refresh::Wrote(n)) => n,
        _ => pixel_core::raw::facts(&file).map(|f| f.count()).unwrap_or(0),
    };
    let taken = if on_home { mt.get_attr(&pa, crate::fs::attrfacts::TAKEN, P).ok() } else { None };
    let attrs_ok = match r {
        Some(crate::fs::attrfacts::Refresh::Wrote(_)) => taken == Some(crate::fs::vfs::AttrValue::Int(synth::TAKEN_UNIX)),
        Some(crate::fs::attrfacts::Refresh::NoAttrs) | None => true, // a FAT home or RAM: the facts are computed, not stored
        _ => false,
    };
    if on_home {
        unlink_all();
    }
    serial_println!(
        "[rawcore] store={} type={} src={} type_b={} prev={:?} full={:?} bin={:?} craw={:?} half={:?} facts_write={:?}",
        why, mime, tsrc, mime_b, prev.as_ref().map(|p| (p.0, p.1)).map_err(|e| e.reason()), full.map_err(|e| e.reason()), bin.map_err(|e| e.reason()), cr.map_err(|e| e.reason()),
        half.as_ref().map_err(|e| e.reason()), r
    );
    let typed = mime == crate::fs::filetype::IMAGE_ARW && mime_b == crate::fs::filetype::IMAGE_ARW;
    let ok = typed && tiff_ok && ifds == 4 && preview_ok && demosaic_ok && mosaic_ok && half_ok && facts >= 8 && attrs_ok;
    serial_println!(
        ":: RAWCORE: preview={} mosaic={} half={}x{} store={} type={} tiff={} ifds={} demosaic={} wb={} facts={} -> {} :: source=synthetic(no real ARW yet) cr2=typed-only nef=typed-only",
        if preview_ok { "ok" } else { "fail" }, if mosaic_ok { "ok" } else { "fail" }, hw, hh, if on_home { "home" } else { "ram" }, mime,
        if tiff_ok { "ok" } else { "fail" }, ifds, if demosaic_ok { "ok" } else { "fail" }, raw_core::Wb::name(wb_b.as_ref()), facts,
        if ok { "PASS" } else { "FAIL" }
    );
}
