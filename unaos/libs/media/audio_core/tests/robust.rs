//! Hostile input: every vector, truncated at many points and with random byte flips, must decode to Ok or a
//! named Err — never a panic (the kernel links this code). Deterministic LCG, so a failure reproduces.
mod common;

fn mutate_all(kind: &str, rounds: usize) -> usize {
    let mut x: u64 = 0x9E3779B97F4A7C15;
    let mut rnd = move || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (x >> 33) as usize };
    let mut runs = 0;
    for v in common::vectors(kind) {
        let Some(bytes) = common::fetch(&v) else { continue };
        if bytes.len() > 400_000 { continue; }
        for k in 0..rounds {
            let mut b = bytes.clone();
            if k % 3 == 0 {
                b.truncate(rnd() % b.len().max(1));
            } else {
                for _ in 0..1 + rnd() % 8 { let i = rnd() % b.len(); b[i] ^= 1 << (rnd() % 8); }
            }
            let r = std::panic::catch_unwind(|| { let _ = audio_core::decode_all(&b); });
            assert!(r.is_ok(), "{} mutation {} panicked", v.path.display(), k);
            runs += 1;
        }
    }
    runs
}

#[test]
fn lossless_mutations_never_panic() {
    let n = mutate_all("lossless", 40);
    eprintln!("lossless: {} mutated decodes, no panic", n);
}

#[test]
fn flac_mutations_never_panic() {
    let n = mutate_all("flac", 6);
    eprintln!("flac: {} mutated decodes, no panic", n);
}

/// Opus: every packet of the twelve KAT streams bit-flipped, truncated and garbage-filled, decoded through
/// a live decoder (so the corruption also hits the inter-frame state) — Ok or a named Err, never a panic.
#[test]
fn opus_mutations_never_panic() {
    use audio_core::opus::OpusDecoder;
    let data = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/opus");
    let mut seed = 0x1234_5678u32;
    let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed };
    let mut n = 0usize;
    for name in ["t01", "t02", "t03", "t04", "t05", "t06", "t07", "t08", "t09", "t10", "t11", "t12"] {
        let bits = std::fs::read(data.join(format!("{}.bit", name))).unwrap();
        let mut dec = OpusDecoder::new(2);
        let mut pcm = vec![0i16; 5760 * 2];
        let mut p = 0;
        let mut k = 0;
        while p + 8 <= bits.len() {
            let len = u32::from_be_bytes(bits[p..p + 4].try_into().unwrap()) as usize;
            let mut pk = bits[p + 8..p + 8 + len].to_vec();
            p += 8 + len;
            k += 1;
            if !pk.is_empty() {
                match k % 4 {
                    0 => { let i = rnd() as usize % (pk.len() * 8); pk[i / 8] ^= 1 << (i % 8); }
                    1 => { let cut = rnd() as usize % pk.len(); pk.truncate(cut.max(1)); }
                    2 => { for b in pk.iter_mut().skip(1) { *b = rnd() as u8; } }
                    _ => { let i = 1 + rnd() as usize % pk.len().max(2); if i < pk.len() { pk[i] = !pk[i]; } }
                }
            }
            let _ = dec.decode(Some(&pk), &mut pcm, 5760);
            if k % 7 == 0 { let _ = dec.decode(None, &mut pcm, 960); }
            n += 1;
        }
    }
    eprintln!("{} mutated Opus packets decoded without a panic", n);
}

/// Vorbis: setup headers with flipped bits must be rejected or accepted, never panic; audio packets
/// bit-flipped / truncated / garbage-filled through a live decoder — never a panic.
#[test]
fn vorbis_mutations_never_panic() {
    use audio_core::ogg::OggReader;
    use audio_core::vorbis::{Setup, VorbisDecoder};
    use audio_core::{ByteStream, VecReader};
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/vorbis");
    let mut seed = 0x9abc_def1u32;
    let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed };
    let (mut nsetup, mut npk) = (0, 0);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    files.sort();
    for f in files {
        let mut r = OggReader::new(ByteStream::new(Box::new(VecReader::new(std::fs::read(&f).unwrap()))));
        let id = r.next_packet().unwrap().unwrap().data;
        let _ = r.next_packet().unwrap();
        let setup = r.next_packet().unwrap().unwrap().data;
        for k in 0..60 {
            let mut s = setup.clone();
            if k % 3 == 0 { s.truncate(rnd() as usize % s.len()); } else {
                for _ in 0..1 + rnd() % 4 { let i = 7 + rnd() as usize % (s.len() - 7); s[i] ^= 1 << (rnd() % 8); }
            }
            let _ = std::panic::catch_unwind(|| Setup::parse(&id, &s)).expect("setup parse panicked");
            nsetup += 1;
        }
        let mut d = VorbisDecoder::new(Setup::parse(&id, &setup).unwrap());
        let mut out = vec![];
        let mut k = 0u32;
        while let Some(p) = r.next_packet().unwrap() {
            let mut pk = p.data.clone();
            k += 1;
            if !pk.is_empty() {
                match k % 4 {
                    0 => { let i = rnd() as usize % (pk.len() * 8); pk[i / 8] ^= 1 << (i % 8); }
                    1 => { let c = rnd() as usize % pk.len(); pk.truncate(c); }
                    2 => { for b in pk.iter_mut().skip(1) { *b = rnd() as u8; } }
                    _ => {}
                }
            }
            let _ = d.decode(&pk, &mut out);
            npk += 1;
        }
    }
    eprintln!("{} mutated Vorbis setup headers, {} mutated audio packets: no panic", nsetup, npk);
}

#[test]
fn mp3_mutations_never_panic() {
    // Whole-stream damage on the ISO/Chromium MP3 vectors (fetched; skipped offline): bit flips across
    // headers, side info and main data, random truncation, garbage runs. decode_all must return, never panic.
    let mut seed = 0x1357_9bdfu32;
    let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed };
    let mut files = vec![];
    for kind in ["mp3iso", "lossy"] {
        for v in common::vectors(kind) {
            let n = v.path.to_string_lossy().to_string();
            if (n.ends_with(".bit") || n.ends_with(".mp3")) && common::fetch(&v).is_some() { files.push(v.path.clone()); }
        }
    }
    let mut runs = 0;
    for f in &files {
        let orig = std::fs::read(f).unwrap();
        for k in 0..12 {
            let mut b = orig.clone();
            match k % 4 {
                0 => { for _ in 0..1 + rnd() % 64 { let i = rnd() as usize % (b.len() * 8); b[i / 8] ^= 1 << (i % 8); } }
                1 => { let c = rnd() as usize % b.len(); b.truncate(c); }
                2 => { let s = rnd() as usize % b.len(); let e = (s + 1 + rnd() as usize % 4096).min(b.len()); for x in &mut b[s..e] { *x = rnd() as u8; } }
                _ => { for _ in 0..1 + rnd() % 256 { let i = rnd() as usize % b.len(); b[i] = rnd() as u8; } }
            }
            let r = std::panic::catch_unwind(|| { let _ = audio_core::decode_all(&b); });
            assert!(r.is_ok(), "{}: mutation {} panicked", f.display(), k);
            runs += 1;
        }
    }
    eprintln!("mp3 mutations: {} damaged streams over {} files, no panic", runs, files.len());
}

#[test]
fn aac_mutations_never_panic() {
    // ADTS and MP4 (plain, fragmented, iTunSMPB, edit list) with bit flips, truncation and garbage runs —
    // headers, sample tables and raw blocks all get hit. decode_all must return, never panic.
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/aac");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "aac" || x == "m4a").unwrap_or(false)).collect();
    files.sort();
    let mut seed = 0x2468_ace1u32;
    let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed };
    let mut runs = 0;
    for f in &files {
        let orig = std::fs::read(f).unwrap();
        for k in 0..40 {
            let mut b = orig.clone();
            match k % 5 {
                0 => { for _ in 0..1 + rnd() % 32 { let i = rnd() as usize % (b.len() * 8); b[i / 8] ^= 1 << (i % 8); } }
                1 => { let c = rnd() as usize % b.len(); b.truncate(c); }
                2 => { let s = rnd() as usize % b.len(); let e = (s + 1 + rnd() as usize % 512).min(b.len()); for x in &mut b[s..e] { *x = rnd() as u8; } }
                3 => { for _ in 0..1 + rnd() % 8 { let i = rnd() as usize % b.len().min(2048); b[i] = rnd() as u8; } }
                _ => { for _ in 0..1 + rnd() % 128 { let i = rnd() as usize % b.len(); b[i] = rnd() as u8; } }
            }
            let r = std::panic::catch_unwind(|| { let _ = audio_core::decode_all(&b); });
            assert!(r.is_ok(), "{}: mutation {} panicked", f.display(), k);
            runs += 1;
        }
    }
    eprintln!("aac mutations: {} damaged streams over {} files, no panic", runs, files.len());
}
