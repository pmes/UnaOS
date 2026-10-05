// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! play-check against its oracles, as a test: the test-pattern counter of frame N == N in both
//! containers, and for each public vector the presented pts sequence vs Chromium's
//! `requestVideoFrameCallback` mediaTime (committed in demux_core's chromium-oracle.jsonl) —
//! within one frame and the same frame count. Vectors are the ones demux_core's sample test
//! fetched into `target/media-vectors` (URL + sha256 in its `tests/data/vectors.txt`); a missing
//! vector is skipped loudly, never passed silently.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_play-check")).args(args).output().unwrap();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned())
}

#[test]
fn test_pattern_frame_n_reads_n() {
    let dir = std::env::temp_dir().join(format!("play-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for ext in ["mp4", "webm"] {
        let f = dir.join(format!("utp.{ext}"));
        let f = f.to_str().unwrap();
        assert!(run(&["--make-utp", f, "90", "30"]).0);
        for n in [0u32, 1, 42, 89] {
            let png = format!("{f}.{n}.png");
            let (ok, out) = run(&[f, "--frame", &n.to_string(), "--out", &png]);
            assert!(ok, "{out}");
            assert!(out.contains(&format!("\"counter\":{n},")), "{out}");
            assert!(out.contains("\"presented\":90,\"dropped\":0"), "{out}");
            let p = std::fs::read(&png).unwrap();
            assert_eq!(&p[1..4], b"PNG");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vectors_match_chromium_media_time() {
    let oracle = root().join("unaos/libs/media/demux_core/tests/data/chromium-oracle.jsonl");
    let vec_dir = std::env::var_os("UNAOS_MEDIA_VECTORS").map(PathBuf::from).unwrap_or_else(|| root().join("target/media-vectors"));
    let mut checked = 0;
    for name in ["av1.mp4", "vp9.mp4", "test-1s.webm", "test-av-384k-44100Hz-1ch-320x240-30fps-10kfr.webm"] {
        let f = vec_dir.join(name);
        if !f.exists() {
            eprintln!("SKIP {name}: not fetched (run `cargo test -p demux_core --release` online first)");
            continue;
        }
        let (ok, out) = run(&[f.to_str().unwrap(), "--oracle", oracle.to_str().unwrap()]);
        assert!(ok, "{out}");
        assert!(out.contains("\"within_one_frame\":true,\"count_match\":true"), "{out}");
        assert!(out.contains("\"dropped\":0"), "{out}");
        checked += 1;
    }
    eprintln!("checked {checked} vectors against Chromium");
}
