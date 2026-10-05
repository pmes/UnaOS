//! MPEG audio conformance: the ISO/IEC 11172-4 / 13818-4 Layer III compliance bitstreams with the reference
//! decoder's PCM (16-bit), fetched at test time (vectors.txt kind `mp3iso`). ISO 11172-4: a "full accuracy"
//! decoder stays within 2^-15/sqrt(12) RMS and 2^-14 peak of the reference; against a reference rounded to
//! 16 bits we measure our output rounded the same way and require max ≤ 1 LSB and RMS ≤ 0.2 LSB.
//! Two properties of the reference files are accounted for: the ISO decoder does not emit the stream's final
//! frame (it needs the next sync), so ours may be exactly one frame longer; and l3-he_mode changes between
//! mono and stereo frame by frame — its reference is each frame in its own layout, ours is stereo throughout
//! (mono frames duplicated), so that file is compared frame-aware.
mod common;
use audio_core::decode_all;

fn rounded(x: f32) -> f64 { (x as f64 * 32768.0).round().clamp(-32768.0, 32767.0) }

/// Expand a per-frame-layout reference into a stereo stream, walking the bitstream's own headers.
fn he_mode_reference(bits: &[u8], refpcm: &[i16]) -> Vec<i16> {
    let mut out = vec![];
    let (mut i, mut r) = (0usize, 0usize);
    while i + 4 <= bits.len() {
        let h = u32::from_be_bytes(bits[i..i + 4].try_into().unwrap());
        let Some(hd) = audio_core::mp3::Header::parse(h) else { i += 1; continue };
        let len = hd.frame_len();
        if len == 0 { break; }
        let n = 1152;
        if hd.channels() == 1 {
            if r + n > refpcm.len() { break; }
            for k in 0..n { out.push(refpcm[r + k]); out.push(refpcm[r + k]); }
            r += n;
        } else {
            if r + 2 * n > refpcm.len() { break; }
            out.extend_from_slice(&refpcm[r..r + 2 * n]);
            r += 2 * n;
        }
        i += len;
    }
    out
}

pub fn compare_iso(name: &str, bits: &[u8], refpcm: &[u8]) -> (bool, String) {
    let (info, pcm) = match decode_all(bits) { Ok(x) => x, Err(e) => return (refpcm.is_empty(), format!("{}: {:?}", name, e)) };
    let mut r: Vec<i16> = refpcm.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    if name.contains("he_mode") { r = he_mode_reference(bits, &r); }
    let ch = info.channels as usize;
    let spf = if info.rate >= 32000 { 1152 } else { 576 };
    let n = pcm.len().min(r.len());
    let (mut e2, mut s2, mut maxd) = (0f64, 0f64, 0f64);
    for i in 0..n {
        let d = rounded(pcm[i]) - r[i] as f64;
        e2 += d * d;
        s2 += (r[i] as f64).powi(2);
        maxd = maxd.max(d.abs());
    }
    let rms = (e2 / n.max(1) as f64).sqrt();
    let snr = 10.0 * (s2 / e2.max(1e-30)).log10();
    let len_ok = pcm.len() == r.len() || pcm.len() == r.len() + spf * ch;
    let ok = len_ok && maxd <= 1.0 && rms <= 0.2;
    (ok, format!("{:<40} {}ch {}Hz samples ours={} ref={} max|d|={} LSB rms={:.4} LSB SNR={:.1} dB {}", name, ch, info.rate, pcm.len(), r.len(), maxd, rms, snr, if ok { "OK" } else { "FAIL" }))
}

#[test]
fn mp3_iso_compliance() {
    let mut fails = vec![];
    let mut n = 0;
    let vs = common::vectors("mp3iso");
    for v in vs.iter().filter(|v| v.path.extension().map(|e| e == "bit").unwrap_or(false)) {
        let Some(bits) = common::fetch(v) else { continue };
        let rv = vs.iter().find(|x| x.path == v.path.with_extension("pcm")).expect("reference listed");
        let Some(refpcm) = common::fetch(rv) else { continue };
        let (ok, line) = compare_iso(&v.path.file_name().unwrap().to_string_lossy(), &bits, &refpcm);
        eprintln!("{}", line);
        n += 1;
        if !ok { fails.push(line); }
    }
    eprintln!("Layer III conformance: {}/{} within 1 LSB of the reference", n - fails.len(), n);
    assert!(fails.is_empty(), "{:#?}", fails);
}
