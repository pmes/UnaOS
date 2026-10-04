// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PRINCIPIA2 (SR32, B287) — the schema gate.
//
// 1. `docs/dev/PREFS-SCHEMA.md` is GENERATED from `prefs_core::schema::SCHEMA`; the committed file must
//    equal the generated one byte for byte (`PREFS_SCHEMA_BLESS=1` rewrites it).
// 2. `tools/prefs-schema-check.py` scans the tree for every preference key referenced in code and fails
//    when one is absent from the generated document — i.e. from the table. Its own `--selftest` proves it
//    goes red on an undeclared key.

use std::path::PathBuf;
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().unwrap()
}

#[test]
fn the_committed_schema_document_is_the_generated_one() {
    let path = repo().join("docs/dev/PREFS-SCHEMA.md");
    let want = prefs_core::schema::render_markdown();
    if std::env::var_os("PREFS_SCHEMA_BLESS").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "docs/dev/PREFS-SCHEMA.md is not the generated schema — run \
         `PREFS_SCHEMA_BLESS=1 cargo test -p prefs_core --test schema_gate` and commit it"
    );
}

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
}

fn run_check(args: &[&str]) -> Option<std::process::Output> {
    let Some(py) = python() else {
        eprintln!("SKIP: no python3 on this host — tools/prefs-schema-check.py not run");
        return None;
    };
    Some(Command::new(py).arg(repo().join("tools/prefs-schema-check.py")).args(args).current_dir(repo()).output().unwrap())
}

#[test]
fn every_key_referenced_in_the_tree_is_declared() {
    // The document must be current first, or the script checks a stale table.
    the_committed_schema_document_is_the_generated_one();
    if let Some(out) = run_check(&[]) {
        assert!(
            out.status.success(),
            "keys referenced in the tree but absent from prefs_core::schema::SCHEMA:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn the_check_goes_red_on_an_undeclared_key() {
    if let Some(out) = run_check(&["--selftest"]) {
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}
