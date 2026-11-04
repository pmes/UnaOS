// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VUGMAKEMACHINELIBFACE (SR548): DirLib over a real directory (cargo's per-test scratch dir).

use doc_core::Lib;
use doc_host::DirLib;
use std::path::Path;

#[test]
fn a_library_beside_the_part_resolves_and_a_missing_one_does_not() {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("doc_host-beside");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    std::fs::write(d.join("shop.vugmachine"), "vug-doc version=1\n").expect("library");
    let l = DirLib::beside(&d.join("part.vug"));
    assert_eq!(l.resolve("shop").as_deref(), Some("vug-doc version=1\n"));
    assert_eq!(l.resolve("nosuch"), None);
    assert_eq!(l.resolve("../doc_host-beside/shop"), None, "an escape is refused by name");
}
