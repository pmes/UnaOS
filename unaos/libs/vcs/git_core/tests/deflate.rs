// SPDX-License-Identifier: LGPL-3.0-or-later
// M1: the DEFLATE encoder, proven by round trip through pixel_core's inflater (an independent
// decoder) at every level and every forced block type, and by python's zlib (the reference zlib)
// inflating our streams when python3 is present.
mod common;

use git_core::deflate::{self, BlockType};
use git_core::zlib;

fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut v = vec![
        ("empty".to_string(), vec![]),
        ("one".to_string(), b"x".to_vec()),
        ("zeros-1M".to_string(), vec![0u8; 1 << 20]),
        ("random-200k".to_string(), common::prng(7, 200_000)),
        ("abc-run".to_string(), b"abcabcabcabcabcabcabcabcabcabcabcabcabcabc".repeat(5000)),
    ];
    // Text: this crate's own sources.
    let mut text = Vec::new();
    for f in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src")).unwrap() {
        text.extend(std::fs::read(f.unwrap().path()).unwrap());
    }
    v.push(("src-text".to_string(), text));
    // Low-entropy binary: random bytes from a 4-symbol alphabet with repeats.
    let r = common::prng(3, 300_000);
    v.push(("skewed".to_string(), r.iter().map(|b| b"aaab"[(b & 3) as usize]).collect()));
    v.push(("window-edge".to_string(), {
        let mut d = common::prng(11, 40_000);
        let head = d[..1000].to_vec();
        d.extend_from_slice(&head); // a repeat exactly beyond 32 KiB
        d
    }));
    v
}

#[test]
fn roundtrip_every_level() {
    let mut report = String::new();
    for (name, data) in corpus() {
        for level in 0..=9u8 {
            let z = deflate::zlib_compress(&data, level);
            let (back, used) = zlib::inflate(&z, data.len(), usize::MAX).unwrap_or_else(|e| panic!("{name} L{level}: {e}"));
            assert_eq!(back, data, "{name} level {level}");
            assert_eq!(used, z.len(), "{name} level {level}: consumed");
            if level == 6 {
                report.push_str(&format!("{name}: {} -> {}\n", data.len(), z.len()));
            }
        }
    }
    println!("{report}");
}

#[test]
fn forced_block_types() {
    for (name, data) in corpus() {
        if data.len() > 400_000 {
            continue;
        }
        for t in [BlockType::Stored, BlockType::Fixed, BlockType::Dynamic] {
            let raw = deflate::deflate_with(&data, t);
            if !data.is_empty() {
                let btype = (raw[0] >> 1) & 3;
                assert_eq!(btype, t as u8, "{name}: first block type");
            }
            let mut z = vec![0x78, 0x9c];
            z.extend_from_slice(&raw);
            z.extend_from_slice(&deflate::adler32(&data).to_be_bytes());
            let (back, _) = zlib::inflate(&z, data.len(), usize::MAX).unwrap_or_else(|e| panic!("{name} {t:?}: {e}"));
            assert_eq!(back, data, "{name} {t:?}");
        }
    }
}

/// Reference zlib (python3's `zlib`, i.e. madler zlib) decompresses our streams too.
#[test]
fn reference_zlib_accepts() {
    if std::process::Command::new("python3").arg("-c").arg("import zlib").status().map(|s| !s.success()).unwrap_or(true) {
        println!("SKIPPED (no python3)");
        return;
    }
    let dir = common::scratch("refzlib");
    let mut n = 0;
    for (name, data) in corpus() {
        for level in [0u8, 1, 6, 9] {
            let z = deflate::zlib_compress(&data, level);
            let p = dir.join(format!("{name}-{level}.z"));
            std::fs::write(&p, &z).unwrap();
            let out = std::process::Command::new("python3")
                .arg("-c")
                .arg("import sys,zlib,hashlib;d=zlib.decompress(open(sys.argv[1],'rb').read());print(hashlib.sha256(d).hexdigest())")
                .arg(&p)
                .output()
                .unwrap();
            assert!(out.status.success(), "{name} L{level}: {}", String::from_utf8_lossy(&out.stderr));
            let want = git_core::HashKind::Sha256.digest(&data).to_hex();
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), want, "{name} L{level}");
            n += 1;
        }
    }
    println!("reference zlib inflated {n}/{n} streams byte-identical");
}
