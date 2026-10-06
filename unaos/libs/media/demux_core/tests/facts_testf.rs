// SPDX-License-Identifier: LGPL-3.0-or-later
//! ATTRCOLUMNS (rmbp-ledger B402): `facts_of` over the test-f containers (`$UNAOS_TESTF_DIR` or
//! `unaos/target/testf`). Absent samples are SKIPPED out loud.
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

#[test]
fn testf_container_facts() {
    for (name, video, codec) in [("TEST.MP4", true, "av1"), ("TEST.WEBM", true, "vp8"), ("TEST.M4A", false, "aac")] {
        let Ok(b) = std::fs::read(dir().join(name)) else {
            eprintln!("SKIP {name}: not fetched");
            continue;
        };
        let f = demux_core::facts::facts_of(&b).unwrap_or_else(|| panic!("{name}: no facts"));
        eprintln!("{name}: {f:?}");
        assert_eq!(f.video, video, "{name}");
        assert!(f.duration_ms > 0, "{name}");
        if video {
            assert_eq!((f.width, f.height), (320, 240), "{name}: bear is 320x240");
        }
        assert_eq!(f.codec, codec, "{name}");
    }
    assert!(demux_core::facts::facts_of(b"not a container").is_none());
}
