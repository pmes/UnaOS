// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGPHOTOGRID (LEDGER SR503): the photo face end to end on photo_core's fixture.

use std::path::PathBuf;
use std::process::Command;

use photo_core::fixture::{self, Spec};

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[test]
fn tool_writes_doc() {
    let png = tmp("photo-good.png");
    let out = tmp("photo-good.vugdoc");
    std::fs::write(&png, fixture::photo_png()).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_photo"))
        .args([png.to_str().unwrap(), "--pitch", "5", "-o", out.to_str().unwrap(), "--thickness", "6", "--report"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    eprintln!("{stdout}");
    assert_eq!(run.status.code(), Some(0), "{stdout}");
    let line = stdout.lines().next().unwrap();
    assert!(line.starts_with("photo: w=900 h=720 grid="), "{line}");
    assert!(line.contains(" holes=2 primitives=4/1/2 dof=10"), "{line}");
    let text = std::fs::read_to_string(&out).unwrap();
    let doc = doc_core::read(&text).expect("the vug-doc reads");
    let r = doc_core::replay(&doc, doc.tol);
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert_eq!(r.sketches.len(), 1);
    assert_eq!(r.solids.len(), 1);
}

#[test]
fn tool_refuses_no_grid() {
    let png = tmp("photo-nogrid.png");
    let out = tmp("photo-nogrid.vugdoc");
    std::fs::write(&png, fixture::png(&Spec::no_grid())).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_photo"))
        .args([png.to_str().unwrap(), "--pitch", "5", "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(run.status.code(), Some(1), "{stdout}");
    assert!(stdout.starts_with("photo: refused NoGrid"), "{stdout}");
}

#[test]
fn tool_usage() {
    let run = Command::new(env!("CARGO_BIN_EXE_photo")).args(["--pitch"]).output().unwrap();
    assert_eq!(run.status.code(), Some(2));
}
