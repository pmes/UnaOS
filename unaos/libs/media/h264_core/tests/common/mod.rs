// SPDX-License-Identifier: LGPL-3.0-or-later
//! Shared KAT plumbing: the vector cache (fetched at test time with `curl` from the URLs in
//! `tests/vectors.txt`, verified by `sha256sum` against the listed digest; offline => the caller
//! prints SKIP), the JVT conformance package layout (a zip holding the bitstream and the reference
//! decoder's YUV output, unpacked with `unzip`), and the frame-by-frame comparison.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn cache_dir() -> PathBuf {
    std::env::var_os("H264_VECTORS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("h264-vectors"))
}

fn entry(rel: &str) -> Option<(String, String)> {
    entry_in(include_str!("../vectors.txt"), rel).map(|(sum, url, _)| (sum, url))
}

/// The row of `rel` in a vectors list (`<sha256|-> <path> <url> [more columns]`): its digest, its URL and the trailing
/// columns (HEVCCONF SR321: hevc_core's list carries the reference YUV md5 and the bitstream's name there).
pub fn entry_in(list: &str, rel: &str) -> Option<(String, String, Vec<String>)> {
    list.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).find_map(|l| {
        let mut it = l.split_whitespace();
        let (sum, path, url) = (it.next()?, it.next()?, it.next()?);
        (path == rel).then(|| (sum.to_string(), url.to_string(), it.map(str::to_string).collect()))
    })
}

fn sha256(p: &Path) -> Option<String> {
    let out = Command::new("sha256sum").arg(p).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).split_whitespace().next()?.to_string())
}

/// The path of a listed vector in the cache, fetching (and verifying) it on first use. `None` =
/// offline or the digest did not match; the test prints SKIP. A digest of `-` is a vector whose
/// pin is still owed: it is fetched, its sha256 printed as `UNPINNED <path> <sha256>` for the
/// vectors.txt row, and used.
pub fn fetch(rel: &str) -> Option<PathBuf> {
    fetch_in(include_str!("../vectors.txt"), cache_dir(), rel)
}

/// [`fetch`] over any vectors list and cache directory (HEVCCONF SR321: hevc_core's conformance test links this file
/// by `#[path]` and passes its own list, so there is one fetcher).
pub fn fetch_in(list: &str, cache: PathBuf, rel: &str) -> Option<PathBuf> {
    let (sum, url, _) = entry_in(list, rel).unwrap_or_else(|| panic!("{rel} is not in vectors.txt"));
    let path = cache.join(rel);
    let pinned = sum != "-";
    if path.exists() {
        match sha256(&path) {
            Some(s) if !pinned || s == sum => return Some(path),
            _ => {}
        }
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    let part = path.with_extension("part");
    let ok = Command::new("curl").args(["-sSfL", "--retry", "2", "-o"]).arg(&part).arg(&url).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP {rel}: fetch failed (offline?)");
        let _ = std::fs::remove_file(&part);
        return None;
    }
    let got = sha256(&part);
    if pinned && got.as_deref() != Some(sum.as_str()) {
        eprintln!("SKIP {rel}: sha256 mismatch ({got:?})");
        let _ = std::fs::remove_file(&part);
        return None;
    }
    if !pinned {
        eprintln!("UNPINNED {rel} {}", got.unwrap_or_default());
    }
    std::fs::rename(&part, &path).ok()?;
    Some(path)
}

/// A JVT conformance package: the bitstream and the reference YUV (I420, cropped, every output
/// frame in output order).
pub struct Package {
    pub stream: Vec<u8>,
    pub yuv: Vec<u8>,
    pub stream_name: String,
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

/// Fetch and unpack a conformance zip; pick its bitstream (.264 / .h264 / .jsv / .26l / .jvt /
/// .avc / .bit) and its decoded YUV (a .yuv, preferring a name with "dec", then the largest).
pub fn package(rel: &str) -> Option<Package> {
    let zip = fetch(rel)?;
    let files = unpack(&zip, rel)?;
    let streams: Vec<&PathBuf> = files.iter().filter(|p| matches!(ext_of(p).as_str(), "264" | "h264" | "jsv" | "26l" | "jvt" | "avc" | "bit")).collect();
    let mut yuvs: Vec<&PathBuf> = files.iter().filter(|p| ext_of(p) == "yuv").collect();
    yuvs.sort_by_key(|p| {
        let name = p.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
        (!name.contains("dec"), std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)))
    });
    let stream = streams.iter().max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))?;
    let yuv = yuvs.first()?;
    Some(Package { stream: std::fs::read(stream).ok()?, yuv: std::fs::read(yuv).ok()?, stream_name: stream.file_name().unwrap().to_string_lossy().into_owned() })
}

/// Decode a whole Annex B stream (every frame, flushed at the end).
pub fn decode_all(stream: &[u8]) -> Result<Vec<h264_core::Frame>, h264_core::Error> {
    let mut d = h264_core::Decoder::new();
    let mut frames = Vec::new();
    for u in h264_core::nal::annexb_units(stream) {
        d.decode_nal(u)?;
        frames.extend(d.take_frames());
    }
    d.flush();
    frames.extend(d.take_frames());
    Ok(frames)
}

/// Compare decoded frames with the reference YUV, frame by frame; returns the frame count.
pub fn compare(name: &str, frames: &[h264_core::Frame], yuv: &[u8]) -> usize {
    assert!(!frames.is_empty(), "{name}: no frames decoded");
    let fsize = (frames[0].width * frames[0].height * 3 / 2) as usize;
    assert_eq!(yuv.len() % fsize, 0, "{name}: reference YUV is not a whole number of {}x{} frames", frames[0].width, frames[0].height);
    let want = yuv.len() / fsize;
    for (i, f) in frames.iter().enumerate() {
        let got = f.i420();
        assert_eq!(got.len(), fsize, "{name}: frame {i} is {}x{}", f.width, f.height);
        let Some(r) = yuv.get(i * fsize..(i + 1) * fsize) else { break };
        if got != r {
            let k = got.iter().zip(r).position(|(a, b)| a != b).unwrap();
            let (w, h) = (f.width as usize, f.height as usize);
            let (plane, off) = if k < w * h { ("Y", k) } else if k < w * h * 5 / 4 { ("Cb", k - w * h) } else { ("Cr", k - w * h * 5 / 4) };
            let pw = if plane == "Y" { w } else { w / 2 };
            panic!("{name}: frame {i} (POC {}) differs first at {plane} ({}, {}): got {} want {}", f.poc, off % pw, off / pw, got[k], r[k]);
        }
    }
    assert_eq!(frames.len(), want, "{name}: output frame count");
    want
}

/// Unpack a fetched conformance zip (once, beside it) and list every file in it; `None` (SKIP printed) when `unzip`
/// fails.
pub fn unpack(zip: &Path, rel: &str) -> Option<Vec<PathBuf>> {
    let dir = zip.with_extension("d");
    if !dir.exists() {
        let ok = Command::new("unzip").args(["-o", "-q"]).arg(zip).arg("-d").arg(&dir).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            eprintln!("SKIP {rel}: unzip failed");
            let _ = std::fs::remove_dir_all(&dir);
            return None;
        }
    }
    let mut files = Vec::new();
    walk(&dir, &mut files);
    Some(files)
}
