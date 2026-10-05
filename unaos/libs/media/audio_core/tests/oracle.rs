//! The ORACLE: Chromium's Web Audio `decodeAudioData` decodes the same file; the lossless formats must
//! match it bit-for-bit (our int→f32 rule `s / 2^(b-1)` is the one Chromium applies). Generated WAV variants
//! cover the widths no fetched file has; AIFF (which this Chromium cannot decode) is checked against the
//! generator's own samples — exact by construction.
mod common;
use audio_core::{decode_all, decode_all_i32, AudioDecoder, Decoder, Format, Info};
use common::synth;
use std::path::PathBuf;

fn generated() -> Vec<PathBuf> {
    let dir = common::root().join("vectors").join("gen");
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = vec![];
    let mut put = |name: &str, bytes: Vec<u8>| {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        out.push(p);
    };
    put("u8-mono-8000.wav", synth::wav_int(8000, 1, 1, 8, false, &synth::signal(4000, 1, 8, 1)));
    put("s16-stereo-44100.wav", synth::wav_int(44100, 2, 2, 16, false, &synth::signal(9000, 2, 16, 2)));
    put("s24-stereo-48000.wav", synth::wav_int(48000, 2, 3, 24, false, &synth::signal(9000, 2, 24, 3)));
    put("s32-stereo-96000.wav", synth::wav_int(96000, 2, 4, 32, false, &synth::signal(9000, 2, 32, 4)));
    put("f32-stereo-44100.wav", synth::wav_float(44100, 2, 4, &synth::fsignal(9000, 2, 5)));
    put("f64-mono-22050.wav", synth::wav_float(22050, 1, 8, &synth::fsignal(9000, 1, 6)));
    put("ext-s24in32-stereo-48000.wav", synth::wav_int(48000, 2, 4, 24, true, &synth::signal(9000, 2, 24, 7)));
    put("ext-s16-6ch-48000.wav", synth::wav_int(48000, 6, 2, 16, true, &synth::signal(4800, 6, 16, 8)));
    put("ext-s20in24-mono-44100.wav", synth::wav_int(44100, 1, 3, 20, true, &synth::signal(9000, 1, 20, 9)));
    out
}

/// Chromium's own int→float step (media/base/audio_sample_types.h): FFmpeg hands it u8 (WAV ≤ 8-bit), s16
/// (any other source ≤ 16 bits, left-justified) or s32; it scales positive values by 1/(2^(n-1)-1) and
/// negative ones by 1/2^(n-1) — asymmetric. Our API maps s/2^(b-1) (symmetric); to compare bit-for-bit we
/// apply Chromium's rule to our integers here. Float sources compare untouched.
fn chromium_rule(info: &Info, ours_i32: &[i32], ours_f32: &[f32]) -> Vec<f32> {
    if info.float { return ours_f32.to_vec(); }
    let (shift, max, min) = if info.bits <= 8 && info.format == Format::Wav { (24, 127.0f32, 128.0f32) }
        else if info.bits <= 16 { (16, 32767.0, 32768.0) } else { (0, 2147483647.0, 2147483648.0) };
    ours_i32.iter().map(|&s| { let v = (s >> shift) as f32; if v < 0.0 { v * (1.0 / min) } else { v * (1.0 / max) } }).collect()
}

#[test]
fn lossless_vs_chromium() {
    let mut files: Vec<PathBuf> = generated();
    for v in common::vectors("lossless") {
        if common::fetch(&v).is_some() { files.push(v.path.clone()); }
    }
    // a slice of the FLAC testbench too (small files: all predictor kinds, 8/12/20/24-bit, 3–8 channels)
    for v in common::vectors("flac") {
        let n = v.path.file_name().unwrap().to_string_lossy().to_string();
        if ["subset-03", "subset-11", "subset-14", "subset-16", "subset-17", "subset-22", "subset-23", "subset-26", "subset-37", "subset-38", "subset-43", "subset-60", "subset-63", "subset-64"].iter().any(|k| n.starts_with(k)) && common::fetch(&v).is_some() {
            files.push(v.path.clone());
        }
    }
    let mut decoded = vec![];
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let (info, pcm) = decode_all(&bytes).unwrap_or_else(|e| panic!("{}: {:?}", f.display(), e));
        let (_, ints) = decode_all_i32(&bytes).unwrap();
        let pcm = chromium_rule(&info, &ints, &pcm);
        decoded.push((info, pcm));
    }
    let jobs: Vec<(PathBuf, u32)> = files.iter().zip(&decoded).map(|(f, (i, _))| (f.clone(), i.rate)).collect();
    let refs = common::chromium(&jobs);
    let (mut exact, mut compared) = (0, 0);
    for ((f, (info, pcm)), r) in files.iter().zip(&decoded).zip(refs) {
        let name = f.file_name().unwrap().to_string_lossy();
        let Some(r) = r else { eprintln!("SKIP {:<44} (no Chromium reference)", name); continue };
        compared += 1;
        let c = common::compare(pcm, info.channels as usize, &r, 0);
        let ok = c.frames_ours == c.frames_ref && c.mismatches == 0;
        eprintln!("{:<44} {:?} {}ch {}Hz {}bit frames ours={} chromium={} mismatches={} max|d|={:.3e} -> {}", name, info.codec, info.channels, info.rate, info.bits, c.frames_ours, c.frames_ref, c.mismatches, c.max_abs, if ok { "EXACT" } else { "DIFF" });
        if ok { exact += 1; }
    }
    eprintln!("lossless vs Chromium: {}/{} bit-exact", exact, compared);
    assert_eq!(exact, compared);
}

#[test]
fn aiff_exact_against_generator() {
    let dir = common::root().join("vectors").join("gen");
    std::fs::create_dir_all(&dir).unwrap();
    let cases: Vec<(&str, u32, usize, u32, Option<&[u8; 4]>)> = vec![
        ("s16-stereo.aiff", 44100, 2, 16, None),
        ("s8-mono.aiff", 22050, 1, 8, None),
        ("s24-mono.aifc", 48000, 1, 24, Some(b"NONE")),
        ("s12-stereo.aiff", 32000, 2, 12, None),
        ("s16-stereo-sowt.aifc", 44100, 2, 16, Some(b"sowt")),
        ("s32-stereo.aiff", 96000, 2, 32, None),
        ("f32-stereo.aifc", 44100, 2, 32, Some(b"fl32")),
    ];
    for (name, rate, ch, bits, comp) in cases {
        let ints = synth::signal(5000, ch, bits.max(8), 11);
        let floats = synth::fsignal(5000, ch, 12);
        let bytes = synth::aiff(rate, ch, bits, comp, &ints, &floats);
        std::fs::write(dir.join(name), &bytes).unwrap();
        let mut d = Decoder::open_bytes(bytes).unwrap();
        let info = d.info();
        assert_eq!((info.rate, info.channels as usize), (rate, ch), "{}", name);
        let mut out = vec![0f32; 5000 * ch + 16];
        let n = d.next(&mut out).unwrap();
        assert_eq!(n, 5000, "{}", name);
        for i in 0..5000 * ch {
            let want = if comp == Some(b"fl32") { floats[i] as f32 } else { ints[i] as f32 / (1u64 << (bits - 1)) as f32 };
            assert_eq!(out[i], want, "{} sample {}", name, i);
        }
        eprintln!("{:<24} {}Hz {}ch {}bit: 5000 frames exact", name, rate, ch, bits);
    }
}
