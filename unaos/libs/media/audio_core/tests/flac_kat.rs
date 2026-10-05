//! FLAC KAT: the FLAC decoder testbench subset. Each file is decoded completely and the MD5 of our decoded
//! PCM must equal the MD5 the encoder stored in STREAMINFO (RFC 9639 §8.2) — an independent, exact proof.
mod common;
use audio_core::flac;

fn run(kind: &str) -> (usize, usize) {
    let (mut pass, mut seen) = (0, 0);
    for v in common::vectors(kind) {
        let Some(bytes) = common::fetch(&v) else { continue };
        seen += 1;
        let name = v.path.file_name().unwrap().to_string_lossy().to_string();
        match flac::decode_file(&bytes) {
            Ok((si, pcm, md5)) => {
                let frames = pcm.len() as u64 / si.channels as u64;
                let total_ok = si.total == 0 || si.total == frames;
                eprintln!("{:<16} {}ch {:>6}Hz {:>2}bit frames={:<9} md5={:?} total_ok={}", name, si.channels, si.rate, si.bps, frames, md5, total_ok);
                assert_eq!(md5, Some(true), "{}: MD5 must be present and match", name);
                assert!(total_ok, "{}: total samples", name);
                pass += 1;
            }
            Err(e) => panic!("{}: {:?}", name, e),
        }
    }
    (pass, seen)
}

#[test]
fn flac_subset() {
    let (p, s) = run("flac");
    eprintln!("FLAC subset: {}/{} MD5-exact", p, s);
}

#[test]
fn flac_subset_big() {
    if !common::big() { eprintln!("flac-big: set AUDIO_CORE_BIG=1"); return; }
    let (p, s) = run("flac-big");
    eprintln!("FLAC subset (big): {}/{} MD5-exact", p, s);
}
