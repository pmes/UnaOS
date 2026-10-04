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
