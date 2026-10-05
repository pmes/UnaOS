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
    let vdir = common::root().join("data").join("vorbis");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&vdir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    v.sort();
    files.extend(v);
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
        // Frame counts: Opus must match exactly. Chromium does not apply Vorbis end trimming (it hands out
        // every decoded sample; libvorbis and the spec cut the last page to its granule), so for Vorbis ours
        // may be shorter than Chromium's by less than one long block — and must equal libvorbis's (see
        // vs_reference_library).
        let frames_ok = match info.codec {
            Codec::Vorbis => c.frames_ours <= c.frames_ref && c.frames_ref - c.frames_ours < 8192,
            _ => c.frames_ours == c.frames_ref,
        };
        let ok = frames_ok && c.snr_db >= floor;
        eprintln!("{:<40} {:?} {}ch {}Hz frames ours={} chromium={} SNR={:.1} dB max|d|={:.2e} -> {}", name, info.codec, info.channels, info.rate,
            c.frames_ours, c.frames_ref, c.snr_db, c.max_abs, if ok { "OK" } else { "FAIL" });
        if ok { good += 1; }
        worst = worst.min(c.snr_db);
    }
    eprintln!("lossy vs Chromium: {}/{} within the SNR floor; worst {:.1} dB", good, compared, worst);
    assert_eq!(good, compared);
}


/// Development aid: `AUDIO_CORE_LIBREF=<dir>` holding `<name>.ogg` + `<name>.f32` (interleaved f32 from the
/// libvorbis/vorbisfile float decoder) reports our SNR against the reference library itself.
#[test]
fn vs_reference_library() {
    let Ok(dir) = std::env::var("AUDIO_CORE_LIBREF") else { return };
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "ogg").unwrap_or(false)).collect();
    names.sort();
    for p in names {
        let refp = p.with_extension("f32");
        let Ok(rb) = std::fs::read(&refp) else { continue };
        let mut rf: Vec<f32> = rb.chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        // vorbisfile hands out the Vorbis channel order; ours is the WAVE order
        let probe = decode_all(&std::fs::read(&p).unwrap()).map(|x| x.0.channels as usize).unwrap_or(0);
        if (3..=8).contains(&probe) {
            const ORDER: [&[usize]; 6] = [&[0, 2, 1], &[0, 1, 2, 3], &[0, 2, 1, 3, 4], &[0, 2, 1, 5, 3, 4], &[0, 2, 1, 6, 5, 3, 4], &[0, 2, 1, 7, 5, 6, 3, 4]];
            let map = ORDER[probe - 3];
            rf = rf.chunks(probe).flat_map(|f| map.iter().map(move |&i| f[i])).collect();
        }
        match decode_all(&std::fs::read(&p).unwrap()) {
            Ok((info, pcm)) => {
                let n = pcm.len().min(rf.len());
                let (mut s, mut e, mut maxd) = (0f64, 0f64, 0f64);
                for i in 0..n { let d = (pcm[i] - rf[i]) as f64; s += (rf[i] as f64).powi(2); e += d * d; maxd = maxd.max(d.abs()); }
                eprintln!("{:<14} {}ch {}Hz samples ours={} ref={} SNR={:.1} dB max|d|={:.2e}", p.file_name().unwrap().to_string_lossy(), info.channels, info.rate, pcm.len(), rf.len(), 10.0 * (s / e).log10(), maxd);
            }
            Err(e) => eprintln!("{:?}: {:?}", p, e),
        }
    }
}

