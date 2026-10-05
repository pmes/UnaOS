// SPDX-License-Identifier: LGPL-3.0-or-later
//! `audio-check <file> [--out pcm.wav]` — decode with audio_core, print the stream facts, the decode
//! speed and the peak/RMS level, and optionally write the PCM as a WAV: integer sources keep their depth
//! (16 → s16, ≤ 24 → s24, else s32), float and lossy sources become 32-bit float WAV.
use audio_core::{sniff, AudioDecoder, Decoder};
use std::io::Write;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: audio-check <file> [--out pcm.wav]");
        std::process::exit(2);
    }
    let path = &args[1];
    let out = args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)).cloned();
    let bytes = std::fs::read(path).unwrap_or_else(|e| { eprintln!("{}: {}", path, e); std::process::exit(1) });
    let fmt = sniff(&bytes[..bytes.len().min(64)]);
    let t0 = Instant::now();
    let mut d = match Decoder::open_bytes(bytes) {
        Ok(d) => d,
        Err(e) => { eprintln!("{}: format={} refused: {}", path, fmt.name(), e); std::process::exit(1) }
    };
    let info = d.info();
    let ch = info.channels as usize;
    let mut pcm: Vec<i32> = Vec::new();
    let mut flt: Vec<f32> = Vec::new();
    let mut buf = vec![0f32; 4096 * ch];
    let mut ibuf = vec![0i32; 4096 * ch];
    let mut result = Ok(());
    loop {
        let r = if info.float { d.next(&mut buf) } else { d.next_i32(&mut ibuf) };
        match r {
            Ok(0) => break,
            Ok(n) => if info.float { flt.extend_from_slice(&buf[..n * ch]) } else { pcm.extend_from_slice(&ibuf[..n * ch]) },
            Err(e) => { result = Err(e); break; }
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    let samples: Vec<f64> = if info.float { flt.iter().map(|&x| x as f64).collect() } else { pcm.iter().map(|&x| x as f64 / 2147483648.0).collect() };
    let frames = samples.len() / ch.max(1);
    let peak = samples.iter().fold(0f64, |m, &x| m.max(x.abs()));
    let rms = (samples.iter().map(|x| x * x).sum::<f64>() / samples.len().max(1) as f64).sqrt();
    let dur = frames as f64 / info.rate as f64;
    println!("{}: format={} codec={:?} rate={} ch={} bits={} float={} frames={} (stated {:?}) dur={:.3}s peak={:.1}dBFS rms={:.1}dBFS decode={:.3}s ({:.0}x realtime){}",
        path, info.format.name(), info.codec, info.rate, ch, info.bits, info.float, frames, info.frames, dur,
        20.0 * peak.max(1e-12).log10(), 20.0 * rms.max(1e-12).log10(), secs, dur / secs.max(1e-9),
        match result { Ok(()) => String::new(), Err(e) => format!(" ERROR after {} frames: {}", frames, e) });
    if let Some(o) = out {
        let (tag, bits) = if info.float { (3u16, 32u16) } else if info.bits <= 16 { (1, 16) } else if info.bits <= 24 { (1, 24) } else { (1, 32) };
        let bps = bits as usize / 8;
        let mut data = Vec::with_capacity(frames * ch * bps);
        if info.float { for &x in &flt { data.extend_from_slice(&x.to_le_bytes()); } }
        else { for &x in &pcm { data.extend_from_slice(&x.to_le_bytes()[4 - bps..]); } }
        let mut f = std::fs::File::create(&o).unwrap();
        let mut h = Vec::new();
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&tag.to_le_bytes());
        h.extend_from_slice(&(ch as u16).to_le_bytes());
        h.extend_from_slice(&info.rate.to_le_bytes());
        h.extend_from_slice(&(info.rate * (ch * bps) as u32).to_le_bytes());
        h.extend_from_slice(&((ch * bps) as u16).to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.write_all(&h).unwrap();
        f.write_all(&data).unwrap();
        println!("wrote {} ({} {}-bit {})", o, if info.float { "float" } else { "int" }, bits, ch);
    }
    if result.is_err() { std::process::exit(1); }
}
