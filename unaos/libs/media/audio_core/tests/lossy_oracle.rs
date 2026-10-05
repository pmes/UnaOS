//! The ORACLE for the lossy codecs: Chromium's Web Audio `decodeAudioData` (libopus/libvorbis/FFmpeg-mp3,
//! floating point) decodes the same file; ours must agree to a high SNR with the frame counts equal.
//! (The Opus decoder here is the fixed-point reference, so it is bit-exact with libopus-fixed — see
//! tests/opus_kat.rs — and differs from Chromium's float build only by the fixed/float arithmetic.)
mod common;
use audio_core::{decode_all, Codec};
use common::oggmux;
use std::path::PathBuf;

fn opus_files() -> Vec<PathBuf> {
    let dir = common::root().join("vectors").join("gen");
    std::fs::create_dir_all(&dir).unwrap();
    let data = common::root().join("data").join("opus");
    let mut out = vec![];
    for name in ["t01", "t02", "t03", "t04", "t05", "t06", "t07", "t08", "t09", "t10", "t11", "t12"] {
        let bits = std::fs::read(data.join(format!("{}.bit", name))).unwrap();
        let mut packets = vec![];
        let mut p = 0;
        while p + 8 <= bits.len() {
            let len = u32::from_be_bytes(bits[p..p + 4].try_into().unwrap()) as usize;
            let pk = bits[p + 8..p + 8 + len].to_vec();
            p += 8 + len;
            if pk.is_empty() { continue; }
            let (_, _, sizes) = audio_core::opus::decoder::parse_packet(&pk).unwrap();
            let d = sizes.len() as u64 * audio_core::opus::decoder::packet_samples_per_frame(pk[0], 48000) as u64;
            packets.push((pk, d));
        }
        let f = dir.join(format!("opus-{}.opus", name));
        std::fs::write(&f, oggmux::ogg_opus(2, 312, 101, &packets)).unwrap();
        out.push(f);
    }
    out
}

#[test]
fn lossy_vs_chromium() {
    let mut files = opus_files();
    for v in common::vectors("lossy") {
        if common::fetch(&v).is_some() { files.push(v.path.clone()); }
    }
    let mut decoded = vec![];
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let (info, pcm) = decode_all(&bytes).unwrap_or_else(|e| panic!("{}: {:?}", f.display(), e));
        decoded.push((info, pcm));
    }
    let jobs: Vec<(PathBuf, u32)> = files.iter().zip(&decoded).map(|(f, (i, _))| (f.clone(), i.rate)).collect();
    let refs = common::chromium(&jobs);
    let (mut good, mut compared) = (0, 0);
    let mut worst = f64::INFINITY;
    for ((f, (info, pcm)), r) in files.iter().zip(&decoded).zip(refs) {
        let name = f.file_name().unwrap().to_string_lossy();
        let Some(r) = r else { eprintln!("SKIP {:<40} (no Chromium reference)", name); continue };
        compared += 1;
        let c = common::compare(pcm, info.channels as usize, &r, 0);
        // The floor. Opus here IS libopus-fixed bit for bit, Chromium runs libopus-float: the two reference
        // builds themselves differ by 55.5 dB on t10 (measured with opus_demo; ours vs Chromium: 55.5 dB),
        // so Opus is held to 50 dB and the bit-exact KAT carries the real proof.
        let floor = match info.codec { Codec::Opus => 50.0, _ => 60.0 };
        let ok = c.frames_ours == c.frames_ref && c.snr_db >= floor;
        eprintln!("{:<40} {:?} {}ch {}Hz frames ours={} chromium={} SNR={:.1} dB max|d|={:.2e} -> {}", name, info.codec, info.channels, info.rate,
            c.frames_ours, c.frames_ref, c.snr_db, c.max_abs, if ok { "OK" } else { "FAIL" });
        if ok { good += 1; }
        worst = worst.min(c.snr_db);
    }
    eprintln!("lossy vs Chromium: {}/{} within the SNR floor; worst {:.1} dB", good, compared, worst);
    assert_eq!(good, compared);
}

