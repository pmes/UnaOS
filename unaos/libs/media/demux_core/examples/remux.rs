//! `cargo run -p demux_core --example remux -- <in> <out.webm|out.mp4> [fragment]`: rewrite a
//! file's packets byte-exact into WebM or (fragmented) MP4 — the oracle path: Chromium plays
//! the result, and its numbers must match what demux_core reads from it.
use demux_core::build::{self, MkvOptions, Mp4Options};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let d = demux_core::Demuxer::open(std::fs::read(&a[1]).unwrap()).unwrap();
    let tracks = build::remux_tracks(&d).expect("codec without a remux mapping");
    let out = if a[2].ends_with(".webm") {
        build::mkv(&tracks, &MkvOptions::default())
    } else {
        let fragment = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(0);
        build::mp4(&tracks, &Mp4Options { fragment, ..Default::default() })
    };
    std::fs::write(&a[2], out).unwrap();
}
