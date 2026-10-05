//! The gate's vector check: every resvg test-suite file in tests/vectors.txt rendered at its reference size and
//! pinned by CRC-32 of the RGBA output in tests/digests.txt — the very renders the Chromium oracle scored
//! (the frozen per-file scores ride in the same file). Vectors come from `fetch-vectors.sh`; set
//! SVGCORE_VECTORS (default <repo>/target/svg-vectors). Missing vectors → SKIP. SVGCORE_DIGESTS_WRITE=1
//! rewrites the CRC column (scores are kept) after an intended rendering change.
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn pinned_digests() {
    let dir = std::env::var("SVGCORE_VECTORS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../target/svg-vectors")));
    if !dir.join("fonts").exists() {
        eprintln!("SKIP: no vectors at {} (run fetch-vectors.sh)", dir.display());
        return;
    }
    let fonts = common::fonts_from(&dir.join("fonts"));
    let opts = svg_core::Options { fonts: Some(&fonts), image_decoder: Some(common::decoder), languages: vec![] };
    let vectors = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.txt")).unwrap();
    let dpath = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/digests.txt");
    let pinned_text = std::fs::read_to_string(dpath).unwrap_or_default();
    let mut pinned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for l in pinned_text.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<String> = l.split('\t').map(String::from).collect();
        pinned.insert(f[0].clone(), f);
    }
    let write = std::env::var("SVGCORE_DIGESTS_WRITE").is_ok();
    let (mut checked, mut missing, mut bad) = (0, 0, Vec::new());
    let mut out = String::from("# relpath\tcrc32(rgba)\tchromium mean |err|\tchromium share within 8 levels\n");
    for l in vectors.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = l.split('\t').collect();
        if f[0] == "font" {
            continue;
        }
        let (rel, w, h) = (f[1], f[3].parse::<u32>().unwrap(), f[4].parse::<u32>().unwrap());
        let Ok(bytes) = std::fs::read(dir.join(rel)) else {
            missing += 1;
            continue;
        };
        let rgba = match svg_core::Svg::parse(&bytes) {
            Ok(s) => s.render_rgba(w, h, &opts).unwrap(),
            Err(_) => vec![0; (w * h * 4) as usize],
        };
        let crc = format!("{:08x}", common::crc32_of(&rgba));
        checked += 1;
        let row = pinned.get(rel);
        if let Some(p) = row {
            if p[1] != crc {
                bad.push(format!("{rel}: {crc} != pinned {}", p[1]));
            }
        } else {
            bad.push(format!("{rel}: not pinned"));
        }
        let (m, s) = row.map(|p| (p.get(2).cloned().unwrap_or_default(), p.get(3).cloned().unwrap_or_default())).unwrap_or_default();
        out.push_str(&format!("{rel}\t{crc}\t{m}\t{s}\n"));
    }
    eprintln!("digests: {checked} rendered, {missing} vectors missing, {} differ", bad.len());
    if write {
        std::fs::write(dpath, out).unwrap();
        return;
    }
    assert!(bad.is_empty(), "renders differ from the pinned (oracle-scored) digests:\n{}", bad.join("\n"));
}
