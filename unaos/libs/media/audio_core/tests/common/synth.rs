//! Deterministic WAV/AIFF writers for the PCM variants no fetched file covers. The signal is a sum of
//! three tones plus LCG noise, full scale minus 1 dB, so every bit of every width is exercised.
#![allow(dead_code)]

pub fn signal(frames: usize, ch: usize, bits: u32, seed: u32) -> Vec<i32> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    let full = ((1i64 << (bits - 1)) - 1) as f64 * 0.89;
    let mut v = Vec::with_capacity(frames * ch);
    for i in 0..frames {
        for c in 0..ch {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (x >> 8) as f64 / (1u32 << 24) as f64 - 0.5;
            let t = i as f64;
            let s = 0.45 * (t * 0.0313 * (c as f64 + 1.0)).sin() + 0.3 * (t * 0.271).sin() + 0.15 * (t * 1.9).sin() + 0.1 * n;
            v.push((s * full).round() as i32);
        }
    }
    v
}

pub fn fsignal(frames: usize, ch: usize, seed: u32) -> Vec<f64> {
    signal(frames, ch, 32, seed).iter().map(|&s| s as f64 / 2147483648.0 * 1.05).collect()
}

/// `kind`: "int" | "float" ; `container` bytes per sample; `valid` bits (EXTENSIBLE when != container*8 or ext)
pub fn wav_int(rate: u32, ch: usize, container: usize, valid: u32, ext: bool, samples: &[i32]) -> Vec<u8> {
    let mut data = vec![];
    for &s in samples {
        let v = (s as i64) << (container as u32 * 8 - valid);
        match container {
            1 => data.push((v + 128) as u8),
            _ => data.extend_from_slice(&v.to_le_bytes()[..container]),
        }
    }
    wav(rate, ch, container, valid, if ext { 0xFFFE } else { 1 }, 1, &data)
}

pub fn wav_float(rate: u32, ch: usize, container: usize, samples: &[f64]) -> Vec<u8> {
    let mut data = vec![];
    for &s in samples {
        if container == 4 { data.extend_from_slice(&(s as f32).to_le_bytes()); } else { data.extend_from_slice(&s.to_le_bytes()); }
    }
    wav(rate, ch, container, container as u32 * 8, 3, 3, &data)
}

fn wav(rate: u32, ch: usize, container: usize, valid: u32, tag: u16, sub: u16, data: &[u8]) -> Vec<u8> {
    let mut f = vec![];
    let ext = tag == 0xFFFE;
    let fmt_len = if ext { 40 } else { 16 };
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&((4 + 8 + fmt_len + 8 + 8 + data.len()) as u32).to_le_bytes());
    f.extend_from_slice(b"WAVE");
    // an unknown chunk before fmt, to exercise the chunk walk
    f.extend_from_slice(b"junk");
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&(fmt_len as u32).to_le_bytes());
    f.extend_from_slice(&tag.to_le_bytes());
    f.extend_from_slice(&(ch as u16).to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&(rate * (ch * container) as u32).to_le_bytes());
    f.extend_from_slice(&((ch * container) as u16).to_le_bytes());
    f.extend_from_slice(&((container * 8) as u16).to_le_bytes());
    if ext {
        f.extend_from_slice(&22u16.to_le_bytes());
        f.extend_from_slice(&(valid as u16).to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes()); // channel mask
        f.extend_from_slice(&if sub == 3 { 3u16 } else { 1u16 }.to_le_bytes());
        f.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
    }
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend_from_slice(data);
    if data.len() & 1 == 1 { f.push(0); }
    f
}

fn ext80(rate: u32) -> [u8; 10] {
    let e = 31 - rate.leading_zeros();
    let mant = (rate as u64) << (63 - e);
    let mut b = [0u8; 10];
    b[..2].copy_from_slice(&((16383 + e) as u16).to_be_bytes());
    b[2..].copy_from_slice(&mant.to_be_bytes());
    b
}

/// AIFF / AIFF-C. `comp`: None (plain AIFF), Some(b"NONE"), Some(b"sowt"), Some(b"fl32").
pub fn aiff(rate: u32, ch: usize, bits: u32, comp: Option<&[u8; 4]>, ints: &[i32], floats: &[f64]) -> Vec<u8> {
    let cb = (bits as usize).div_ceil(8);
    let mut data = vec![];
    match comp {
        Some(b"fl32") => for &s in floats { data.extend_from_slice(&(s as f32).to_be_bytes()); },
        Some(b"sowt") => for &s in ints { data.extend_from_slice(&(((s as i64) << (cb as u32 * 8 - bits)) as i32).to_le_bytes()[..cb]); },
        _ => for &s in ints { let v = ((s as i64) << (cb as u32 * 8 - bits)) as i32; data.extend_from_slice(&v.to_be_bytes()[4 - cb..]); },
    }
    let frames = (data.len() / (cb * ch)) as u32;
    let mut comm = vec![];
    comm.extend_from_slice(&(ch as u16).to_be_bytes());
    comm.extend_from_slice(&frames.to_be_bytes());
    comm.extend_from_slice(&(bits as u16).to_be_bytes());
    comm.extend_from_slice(&ext80(rate));
    if let Some(c) = comp { comm.extend_from_slice(c); comm.extend_from_slice(&[0, 0]); }
    let mut f = vec![];
    f.extend_from_slice(b"FORM");
    let total = 4 + 8 + comm.len() + 8 + 8 + data.len() + (data.len() & 1);
    f.extend_from_slice(&(total as u32).to_be_bytes());
    f.extend_from_slice(if comp.is_some() { b"AIFC" } else { b"AIFF" });
    f.extend_from_slice(b"COMM");
    f.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    f.extend_from_slice(&comm);
    f.extend_from_slice(b"SSND");
    f.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
    f.extend_from_slice(&[0; 8]);
    f.extend_from_slice(&data);
    if data.len() & 1 == 1 { f.push(0); }
    f
}
