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
