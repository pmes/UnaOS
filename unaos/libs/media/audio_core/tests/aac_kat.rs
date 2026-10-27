//! AAC-LC known-answer tests. Twelve ADTS streams made with fdk-aac 2.0.3 (8–96 kHz, mono, stereo, 3 and 5.1
//! channels, 12–256 kb/s CBR and a VBR stream; between them: intensity stereo, M/S, PNS, short windows and
//! TNS), and four MP4 wrappings of them (edit list, iTunSMPB with the moov after the mdat, fragmented, plain)
//! made by `oracle/adts2mp4.py`. The reference is faad2 2.11.4 — a decoder independent of ours — stored as
//! 16-bit FLAC (`oracle/gen-aac.sh`) and read back with our own FLAC decoder (bit-exact on the FLAC suite).
//!
//! Chromium, the oracle for the other lossy codecs, ships without AAC in this build, hence faad2. With PNS
//! switched off in both decoders (a development build of faad2, see AUDIOCODEC.md) every stream agrees to
//! 132.5–134.4 dB, float rounding; here, against faad's normal 16-bit output, the floor is 60 dB for streams
//! without PNS and a spectral match for streams with it.
mod common;
use audio_core::decode_all;
use std::path::PathBuf;

fn data() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/aac") }

/// In-place radix-2 FFT (test-only helper for the spectral comparison).
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j |= bit;
        if i < j { re.swap(i, j); im.swap(i, j); }
    }
    let mut len = 2;
    while len <= n {
        let a = -2.0 * std::f64::consts::PI / len as f64;
        for s in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((a * k as f64).cos(), (a * k as f64).sin());
                let (p, q) = (s + k, s + k + len / 2);
                let (xr, xi) = (re[q] * wr - im[q] * wi, re[q] * wi + im[q] * wr);
                re[q] = re[p] - xr; im[q] = im[p] - xi;
                re[p] += xr; im[p] += xi;
            }
        }
        len <<= 1;
    }
}

/// Third-octave band powers of one 1024-sample Hann-windowed block (from 100 Hz to Nyquist).
fn bands(x: &[f64], rate: f64) -> Vec<f64> {
    let n = 1024;
    let mut re: Vec<f64> = (0..n).map(|i| x[i] * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())).collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    let mut out = vec![];
    let mut f = 100.0f64;
    while f * 2f64.powf(1.0 / 6.0) < rate / 2.0 {
        let (lo, hi) = (f * 2f64.powf(-1.0 / 6.0), f * 2f64.powf(1.0 / 6.0));
        let (a, b) = ((lo / rate * n as f64).ceil() as usize, ((hi / rate * n as f64).floor() as usize).min(n / 2));
        let mut e = 1e-20;
        for k in a..=b.max(a) { e += re[k] * re[k] + im[k] * im[k]; }
        out.push(e);
        f *= 2f64.powf(1.0 / 3.0);
    }
    out
}

#[test]
fn aac_vectors_vs_faad2() {
    let expected = std::fs::read_to_string(data().join("expected.txt")).unwrap();
    let (mut ok, mut n) = (0, 0);
    for line in expected.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (name, refname, offset, frames, mode) = (f[0], f[1], f[2].parse::<usize>().unwrap(), f[3].parse::<usize>().unwrap(), f[4]);
        n += 1;
        let (info, pcm) = decode_all(&std::fs::read(data().join(name)).unwrap()).unwrap_or_else(|e| panic!("{}: {:?}", name, e));
        let (rinfo, mut rf) = decode_all(&std::fs::read(data().join(format!("{}.ref.flac", refname))).unwrap()).unwrap();
        let ch = info.channels as usize;
        assert_eq!(ch, rinfo.channels as usize, "{}: channels", name);
        // faad2's raw output keeps the native order for channelConfiguration 3 (C L R); ours is WAVE (L R C)
        if ch == 3 { rf = rf.chunks(3).flat_map(|c| [c[1], c[2], c[0]]).collect(); }
        let got = pcm.len() / ch;
        let r = &rf[offset * ch..((offset + frames) * ch).min(rf.len())];
        let mut good = got == frames && r.len() / ch == frames;
        let (mut s, mut e) = (0f64, 0f64);
        for i in 0..pcm.len().min(r.len()) { let d = (pcm[i] - r[i]) as f64; s += (r[i] as f64).powi(2); e += d * d; }
        let snr = 10.0 * (s / e).log10();
        let mut detail = format!("SNR={:.1} dB", snr);
        if mode == "snr" {
            good &= snr >= 60.0;
        } else {
            // The noise bands are the ones where the two decodes do not agree in waveform (band SNR under
            // 10 dB): there the levels must agree — mean signed difference (a gain error) under 0.5 dB and
            // mean absolute difference (random level spread) under 2.5 dB. Bands that agree in waveform are
            // what the SNR already measures. Ours is rounded to 16 bits first, so the reference's
            // quantisation floor is in both.
            let (mut sum, mut abs, mut cnt) = (0f64, 0f64, 0usize);
            for c in 0..ch {
                let a: Vec<f64> = (0..got.min(frames)).map(|i| ((pcm[i * ch + c] as f64 * 32768.0).round().clamp(-32768.0, 32767.0)) / 32768.0).collect();
                let b: Vec<f64> = (0..got.min(frames)).map(|i| r[i * ch + c] as f64).collect();
                let d: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x - y).collect();
                for blk in 0..a.len() / 1024 {
                    let rate = info.rate as f64;
                    let (pa, pb, pd) = (bands(&a[blk * 1024..], rate), bands(&b[blk * 1024..], rate), bands(&d[blk * 1024..], rate));
                    let top = pb.iter().cloned().fold(0.0, f64::max);
                    for k in 0..pb.len() {
                        if pd[k] > 0.1 * pb[k] && pb[k] > top * 1e-5 && pb[k] > 1e-4 {
                            let delta = 10.0 * (pa[k] / pb[k]).log10();
                            sum += delta; abs += delta.abs(); cnt += 1;
                        }
                    }
                }
            }
            let (mean, mabs) = (sum / cnt.max(1) as f64, abs / cnt.max(1) as f64);
            detail = format!("{} (PNS) noise bands {}: level bias {:+.2} dB, spread {:.2} dB", detail, cnt, mean, mabs);
            good &= cnt > 0 && mean.abs() < 0.5 && mabs < 2.5;
        }
        eprintln!("{:<8} {}ch {:>5}Hz frames ours={} ref={} {} -> {}", name, ch, info.rate, got, frames, detail, if good { "OK" } else { "FAIL" });
        if good { ok += 1; }
    }
    eprintln!("AAC vs faad2: {}/{}", ok, n);
    assert_eq!(ok, n);
}

#[test]
fn aac_fetched_streams() {
    // (file, rate, channels, frames) — None: must be refused cleanly
    let want: &[(&str, Option<(u32, u16, usize)>)] = &[
        ("chromium-sfx.adts", Some((44100, 1, 14336))),
        ("chromium-bear-audio-lc-aac.aac", Some((48000, 2, 134144))),
        ("chromium-sfx.m4a", Some((44100, 1, 12701))),
        ("chromium-bear-640x360-a_frag.mp4", Some((44100, 2, 123904))),
        ("chromium-bear-mpeg2-aac-only_frag.mp4", Some((44100, 2, 121856))),
        // HEAACSBR (LEDGER SR198): implicit HE-AAC plays at twice the 24 kHz core rate; v2's PS core is duplicated
        ("chromium-bear-audio-implicit-he-aac-v1.aac", Some((48000, 2, 137216))),
        ("chromium-bear-audio-implicit-he-aac-v2.aac", Some((48000, 2, 139264))),
        ("chromium-bear-audio-main-aac.aac", None),
    ];
    let mut checked = 0;
    for v in common::vectors("aac") {
        let Some(bytes) = common::fetch(&v) else { continue };
        let name = v.path.file_name().unwrap().to_string_lossy().to_string();
        let w = want.iter().find(|w| w.0 == name).unwrap_or_else(|| panic!("{}: no expectation", name)).1;
        match (decode_all(&bytes), w) {
            (Ok((info, pcm)), Some((rate, ch, frames))) => {
                let got = pcm.len() / info.channels as usize;
                eprintln!("{:<44} {}ch {}Hz frames={}", name, info.channels, info.rate, got);
                assert_eq!((info.rate, info.channels, got), (rate, ch, frames), "{}", name);
            }
            (Err(e), None) => eprintln!("{:<44} refused: {:?}", name, e),
            (r, w) => panic!("{}: got {:?}, want {:?}", name, r.map(|x| x.0), w),
        }
        checked += 1;
    }
    eprintln!("fetched AAC streams: {} checked", checked);
}

/// HEAACSBR (LEDGER SR198): Chromium's implicit HE-AAC streams (fetched, sha256-pinned in vectors.txt) against
/// FFmpeg 6.1.1's float decode (`he1.ref.flac` 24-bit, `he2.ref.flac` 16-bit; the commands are in expected.txt and
/// oracle/gen-aac.sh). v1 (SBR): waveform SNR >= 50 dB. v2 (SBR + parametric stereo, HEAACSBR-PS LEDGER SR214):
/// both channels through our PS stage, waveform SNR >= 55 dB per channel. Measured (SR214): this stream's PS headers
/// switch IID and ICC off, so FFmpeg's L and R are equal (bit-equal on 133122 of 139264 float samples, the rest in
/// blocks under -82 dBFS) and the 16-bit reference caps the SNR at ~59.8 dB; a left/right difference bound would
/// compare against silence, so the stereo image itself is held by sbr.rs `ps_tests` (a real-stereo HE-AAC v2 vector
/// is owed: handlers.md HEAACSBR-PSVEC).
#[test]
fn heaac_vs_ffmpeg() {
    let mut checked = 0;
    for v in common::vectors("aac") {
        let name = v.path.file_name().unwrap().to_string_lossy().to_string();
        let refname = match name.as_str() {
            "chromium-bear-audio-implicit-he-aac-v1.aac" => "he1",
            "chromium-bear-audio-implicit-he-aac-v2.aac" => "he2",
            _ => continue,
        };
        let Some(bytes) = common::fetch(&v) else { continue };
        // the first ADTS frame through the public probe: implicit SBR found, and PS seen or not
        let (sfi, cc) = (((bytes[2] >> 2) & 15) as usize, ((bytes[2] & 1) << 2) | (bytes[3] >> 6));
        let flen = (((bytes[3] & 3) as usize) << 11) | ((bytes[4] as usize) << 3) | (bytes[5] as usize >> 5);
        let off = if bytes[1] & 1 != 0 { 7 } else { 9 };
        let layout = audio_core::aac::Layout::from_config(cc).unwrap();
        let dec = audio_core::aac::sbr_probe(audio_core::aac::AacDecoder::new(sfi, layout).unwrap(), Some(&bytes[off..flen]), false).unwrap();
        assert_eq!(dec.frame_len(), 2048, "{}: SBR not found", name);
        let ps = if dec.ps_seen { "ps=seen" } else { "ps=none" };
        let (info, pcm) = decode_all(&bytes).unwrap_or_else(|e| panic!("{}: {:?}", name, e));
        let (rinfo, rf) = decode_all(&std::fs::read(data().join(format!("{}.ref.flac", refname))).unwrap()).unwrap();
        let ch = info.channels as usize;
        assert_eq!((info.rate, ch), (rinfo.rate, rinfo.channels as usize), "{}: rate/channels", name);
        assert_eq!(pcm.len(), rf.len(), "{}: length", name);
        let frames = pcm.len() / ch;
        if refname == "he1" {
            assert_eq!(ps, "ps=none", "{}", name);
            let (mut s, mut e) = (0f64, 0f64);
            for i in 0..pcm.len() { let d = (pcm[i] - rf[i]) as f64; s += (rf[i] as f64).powi(2); e += d * d; }
            let snr = 10.0 * (s / e).log10();
            eprintln!("HE-AAC v1 {:<44} {}ch {}Hz frames={} {} SNR={:.1} dB -> {}", name, ch, info.rate, frames, ps, snr, if snr >= 50.0 { "OK" } else { "FAIL" });
            assert!(snr >= 50.0, "{}: SNR {:.1} dB", name, snr);
        } else {
            assert_eq!(ps, "ps=seen", "{}", name);
            assert_eq!(ch, 2, "{}: PS channels", name);
            let snr: Vec<f64> = (0..ch).map(|c| {
                let (mut s, mut e) = (0f64, 0f64);
                for i in 0..frames { let (x, y) = (pcm[i * ch + c] as f64, rf[i * ch + c] as f64); s += y * y; e += (x - y) * (x - y); }
                10.0 * (s / e).log10()
            }).collect();
            let good = snr.iter().all(|&v| v >= 55.0);
            eprintln!("HE-AAC v2 {:<44} {}ch {}Hz frames={} {} (PS synthesised) SNR L={:.1} dB R={:.1} dB -> {}",
                name, ch, info.rate, frames, ps, snr[0], snr[1], if good { "OK" } else { "FAIL" });
            assert!(good, "{}: SNR L {:.1} dB R {:.1} dB", name, snr[0], snr[1]);
        }
        checked += 1;
    }
    eprintln!("HE-AAC streams: {} checked", checked);
}
