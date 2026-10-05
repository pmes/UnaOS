// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AUDIOTRACK (LEDGER SR45) M1: the audio oracle, compressed tracks. Each vector plays through
//! Stria's player in `play-check` with a headless device (audio is the master clock), and what
//! the device played is held against Chromium's `AudioContext.decodeAudioData` of the same file
//! (`audio_core/oracle/chromium-oracle.cjs`, one browser session for every missing reference),
//! sample for sample, at AUDIOCODEC's floors — or, for AAC (this Chromium has none), against
//! `audio_core`'s own file decode, exactly. `+av` vectors also keep the video oracle: every
//! presented pts within one frame of Chromium's mediaTime, same count, with audio now driving
//! the clock. MP3 in MP4 is made here (demux_core's writer, the frames of `sfx.mp3`) and judged
//! by Chromium too. Offline, or without node/Chromium, a vector is SKIPPED loudly.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn cache() -> PathBuf {
    let d = root().join("target/audio-vectors");
    std::fs::create_dir_all(d.join("oracle")).unwrap();
    d
}

struct V {
    name: String,
    sha: String,
    floor: String,
    flags: String,
    url: String,
}

fn vectors() -> Vec<V> {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/audio-vectors.txt"))
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            V { name: f[0].into(), sha: f[1].into(), floor: f[2].into(), flags: f[3].into(), url: f[4].into() }
        })
        .collect()
}

fn sha256(p: &Path) -> String {
    let o = Command::new("sha256sum").arg(p).output().unwrap();
    String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("").to_string()
}

fn fetch(v: &V) -> Option<PathBuf> {
    let p = cache().join(&v.name);
    if !p.exists() {
        let tmp = p.with_extension("part");
        let ok = Command::new("curl").args(["-sSfL", "-m", "120", "-o"]).arg(&tmp).arg(&v.url).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            eprintln!("SKIP {}: not fetched (offline?)", v.name);
            return None;
        }
        std::fs::rename(&tmp, &p).ok()?;
    }
    assert_eq!(sha256(&p), v.sha, "sha256 of {}", v.name);
    Some(p)
}

fn play_check(args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_play-check")).args(args).output().unwrap();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned())
}

fn field<'a>(json: &'a str, key: &str) -> &'a str {
    let k = format!("\"{key}\":");
    let i = json.find(&k).map(|i| i + k.len()).unwrap_or(json.len());
    let rest = &json[i..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim_matches('"')
}

/// Chromium references for (file, rate), produced in one browser session; None = unavailable.
fn chromium_refs(jobs: &[(PathBuf, u32)]) -> Vec<Option<PathBuf>> {
    let out_of = |p: &Path, r: u32| cache().join("oracle").join(format!("{}.r{r}.f32", p.file_name().unwrap().to_string_lossy()));
    let missing: Vec<String> = jobs
        .iter()
        .filter(|(p, r)| !out_of(p, *r).exists())
        .map(|(p, r)| format!("{{\"in\":{:?},\"rate\":{r},\"out\":{:?}}}", p.to_string_lossy(), out_of(p, *r).to_string_lossy()))
        .collect();
    if !missing.is_empty() {
        let jf = cache().join("oracle/jobs.json");
        std::fs::write(&jf, format!("[{}]", missing.join(","))).unwrap();
        let script = root().join("unaos/libs/media/audio_core/oracle/chromium-oracle.cjs");
        match Command::new("node")
            .arg(&script)
            .arg(&jf)
            .env("NODE_PATH", std::env::var("NODE_PATH").unwrap_or_else(|_| "/opt/node22/lib/node_modules".into()))
            .env("PLAYWRIGHT_BROWSERS_PATH", std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or_else(|_| "/opt/pw-browsers".into()))
            .output()
        {
            Ok(o) => eprint!("{}", String::from_utf8_lossy(&o.stdout)),
            Err(e) => eprintln!("SKIP Chromium oracle: node unavailable ({e})"),
        }
    }
    jobs.iter().map(|(p, r)| Some(out_of(p, *r)).filter(|q| q.exists())).collect()
}

/// `audio_core`'s file decode as a reference dump (u32 ch, frames, rate; planar f32).
fn core_ref(file: &Path) -> PathBuf {
    let (info, pcm) = gneiss_pal::dsp::audio::decode_all(&std::fs::read(file).unwrap()).unwrap();
    let ch = info.channels as usize;
    let frames = pcm.len() / ch;
    let mut b = Vec::new();
    for v in [ch as u32, frames as u32, info.rate] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for c in 0..ch {
        for i in 0..frames {
            b.extend_from_slice(&pcm[i * ch + c].to_le_bytes());
        }
    }
    let out = cache().join("oracle").join(format!("{}.core.f32", file.file_name().unwrap().to_string_lossy()));
    std::fs::write(&out, b).unwrap();
    out
}

/// MP3 in MP4: `sfx.mp3`'s frames (Xing/Info frame left out) in demux_core's MP4 writer.
fn mp3_in_mp4(mp3: &Path) -> PathBuf {
    use gneiss_pal::dsp::audio::mp3::Header;
    use gneiss_pal::dsp::demux::TrackKind;
    use gneiss_pal::dsp::demux::build::{self, MediaTrack, Mp4Options, SampleSpec, TrackSpec};
    let d = std::fs::read(mp3).unwrap();
    let mut p = 0usize;
    if &d[..3] == b"ID3" {
        p = 10 + (((d[6] as usize) << 21) | ((d[7] as usize) << 14) | ((d[8] as usize) << 7) | d[9] as usize);
    }
    let (mut samples, mut t, mut rate, mut ch) = (Vec::new(), 0i64, 0, 0);
    while p + 4 <= d.len() {
        let Some(h) = Header::parse(u32::from_be_bytes(d[p..p + 4].try_into().unwrap())) else { break };
        let len = h.frame_len();
        if len == 0 || p + len > d.len() {
            break;
        }
        let off = 4 + if h.crc { 2 } else { 0 } + h.side_len();
        let tag = &d[p + off..p + off + 4];
        if !(samples.is_empty() && (tag == b"Xing" || tag == b"Info")) {
            rate = h.rate();
            ch = h.channels() as u16;
            samples.push(SampleSpec { data: d[p..p + len].to_vec(), dts: t, pts: t, duration: h.samples() as u32, keyframe: true });
            t += h.samples() as i64;
        }
        p += len;
    }
    // MP4 audio: `mp4a` with an ES descriptor naming MPEG-1 audio (ObjectTypeIndication 0x6B)
    let dc = [vec![0x04, 13, 0x6B, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]].concat();
    let mut es = vec![0x03, (3 + dc.len() + 3) as u8, 0, 1, 0];
    es.extend_from_slice(&dc);
    es.extend_from_slice(&[0x06, 1, 2]);
    let mut esds = vec![0, 0, 0, 0];
    esds.extend_from_slice(&es);
    let spec = TrackSpec { id: 1, kind: TrackKind::Audio, fourcc: *b"mp4a", config_box: Some(*b"esds"), codec_id: "A_MPEG/L3", config: esds, timescale: rate, width: 0, height: 0, sample_rate: rate, channels: ch, bit_depth: 16, default_duration_ns: 0 };
    let out = cache().join("sfx-mp3-in.mp4");
    std::fs::write(&out, build::mp4(&[MediaTrack { spec, samples }], &Mp4Options::default())).unwrap();
    out
}

#[test]
fn compressed_tracks_play_as_chromium_decodes_them() {
    let vs = vectors();
    let mut files: Vec<(PathBuf, &V)> = Vec::new();
    for v in &vs {
        if let Some(p) = fetch(v) {
            files.push((p, v));
        }
    }
    if files.is_empty() {
        eprintln!("SKIP: no vector fetched (offline)");
        return;
    }
    // MP3 in MP4, made here from sfx.mp3
    let made = files.iter().find(|(_, v)| v.name == "sfx.mp3").map(|(p, _)| mp3_in_mp4(p));
    let mp3v = V { name: "sfx-mp3-in.mp4".into(), sha: String::new(), floor: "lsb:2".into(), flags: "chromium".into(), url: String::new() };
    if let Some(m) = &made {
        files.push((m.clone(), &mp3v));
    }
    // rates as Stria plays them (the OfflineAudioContext runs at the file's own rate)
    let rates: Vec<u32> = files.iter().map(|(p, _)| play_check(&[p.to_str().unwrap(), "--audio-out", "/dev/null"]).1).map(|j| field(&j, "sample_rate").parse().unwrap_or(0)).collect();
    let jobs: Vec<(PathBuf, u32)> = files.iter().zip(&rates).filter(|((_, v), _)| v.flags.starts_with("chromium")).map(|((p, _), r)| (p.clone(), *r)).collect();
    let refs = chromium_refs(&jobs);
    let oracle_jsonl = root().join("unaos/libs/media/demux_core/tests/data/chromium-oracle.jsonl");
    let mut checked = 0;
    let mut table = Vec::new();
    for (p, v) in &files {
        let reference = if v.flags.starts_with("core") {
            Some(core_ref(p))
        } else {
            jobs.iter().position(|(q, _)| q == p).and_then(|i| refs[i].clone())
        };
        let Some(reference) = reference else {
            eprintln!("SKIP {}: no Chromium reference", v.name);
            continue;
        };
        let played = cache().join("oracle").join(format!("{}.played.f32", v.name));
        let mut args = vec![p.to_str().unwrap().to_string(), "--audio-out".into(), played.to_str().unwrap().into(), "--audio-ref".into(), reference.to_str().unwrap().into(), "--audio-floor".into(), v.floor.clone()];
        if v.flags.contains("+max") {
            args.push("--ref-frames-max".into());
        }
        if v.flags.contains("+av") {
            args.push("--oracle".into());
            args.push(oracle_jsonl.to_str().unwrap().into());
        }
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let (ok, out) = play_check(&a);
        let o = &out[out.find("\"audio_oracle\"").unwrap_or(0)..];
        table.push(format!(
            "{:<52} {:<7} {:<9} frames {}/{} snr {} max {} LSB clock={}",
            v.name,
            field(&out, "audio_decoder"),
            v.floor,
            field(o, "frames_ours"),
            field(o, "frames_ref"),
            field(o, "snr_db"),
            field(o, "max_lsb16"),
            field(&out, "audio_clock")
        ));
        assert!(ok, "{}: {out}", v.name);
        assert_eq!(field(&out, "audio_clock"), "true", "{}: audio must be the master clock", v.name);
        if v.flags.contains("+av") {
            assert!(out.contains("\"within_one_frame\":true,\"count_match\":true"), "{}: {out}", v.name);
            assert!(out.contains("\"dropped\":0"), "{}: {out}", v.name);
        }
        checked += 1;
    }
    for l in &table {
        eprintln!("{l}");
    }
    eprintln!("audio oracle: {checked} vectors");
    assert!(checked >= 1);
}
