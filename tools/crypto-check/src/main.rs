// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// crypto-check — run every CRYPTOCORE known-answer set and print the counts.
//
//   cargo run --release -p crypto-check [-- <prefix>]     e.g. `-- p256/` or `-- gcm/`
//
// Vector files listed as FETCH in crypto_core/kat/vectors.txt are downloaded on first use into
// $CRYPTO_VECTORS_DIR (default <tmp>/unaos-crypto-vectors), verified against their pinned sha256, and
// reported SKIPPED when offline (CRYPTO_OFFLINE=1 forces that). CRYPTO_SLOW=1 adds the 1,000,000-step
// X25519 iteration. Exit status: 0 when nothing failed (skips are not failures), 1 otherwise.

#[path = "../../../unaos/libs/sys/crypto_core/kat/mod.rs"]
mod kat;

fn main() {
    let filter = std::env::args().nth(1);
    let t0 = std::time::Instant::now();
    let sets = kat::run_all(filter.as_deref());
    let (mut pass, mut fail, mut skipped, mut unsupported) = (0usize, 0usize, 0usize, 0usize);
    println!("{:<36} {:>8} {:>6} {:>12}", "CRYPTOCORE KAT set", "pass", "fail", "unsupported");
    for t in &sets {
        match &t.skipped {
            Some(why) => {
                skipped += 1;
                println!("{:<36} SKIPPED  {why}", t.name);
            }
            None => {
                println!("{:<36} {:>8} {:>6} {:>12}", t.name, t.pass, t.fail, t.unsupported);
                for f in &t.failures {
                    println!("    FAIL {f}");
                }
            }
        }
        pass += t.pass;
        fail += t.fail;
        unsupported += t.unsupported;
    }
    println!(
        "TOTAL: {} sets, {pass} pass, {fail} fail, {unsupported} unsupported, {skipped} sets skipped (offline) in {:.1}s",
        sets.len(),
        t0.elapsed().as_secs_f64()
    );
    std::process::exit(if fail == 0 { 0 } else { 1 });
}
