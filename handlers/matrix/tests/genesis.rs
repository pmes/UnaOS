// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Golden-input tests for `MatrixScanner::build_genesis_tree` — the all-asset
// genesis scan (MATRIX-REOPEN M1). Fixtures are built in a tempdir so the
// expected topology is fully deterministic.

use std::fs;
use std::path::Path;

use matrix::MatrixScanner;

/// Flatten a TopologyNode forest into `(depth, id, label)` lines for golden
/// comparison (TopologyNode has no PartialEq).
fn render(nodes: &[bandy::state::TopologyNode], depth: usize, out: &mut Vec<String>) {
    for n in nodes {
        out.push(format!("{}:{}:{}", depth, n.id, n.label));
        render(&n.children, depth + 1, out);
    }
}

fn rendered(root: &Path) -> Vec<String> {
    let nodes = MatrixScanner::build_genesis_tree(root, root);
    let mut out = Vec::new();
    render(&nodes, 0, &mut out);
    out
}

fn touch(path: &Path) {
    fs::write(path, b"x").unwrap();
}

/// Mixed asset types (code, docs, image, config) ALL appear as nodes,
/// dirs sorted before files, both alphabetically.
#[test]
fn genesis_includes_all_asset_types() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let src = root.join("src");
    let assets = root.join("assets");
    fs::create_dir(&src).unwrap();
    fs::create_dir(&assets).unwrap();

    touch(&src.join("main.rs"));
    touch(&src.join("notes.md"));
    touch(&assets.join("logo.png"));
    touch(&root.join("Cargo.toml"));
    touch(&root.join("README.md"));

    assert_eq!(
        rendered(root),
        vec![
            "0:assets:assets",
            "1:assets/logo.png:logo.png",
            "0:src:src",
            "1:src/main.rs:main.rs",
            "1:src/notes.md:notes.md",
            "0:Cargo.toml:Cargo.toml",
            "0:README.md:README.md",
        ]
    );
}

/// `target`, `.git`, and `node_modules` are excluded even when they hold files.
#[test]
fn genesis_excludes_build_noise() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    for noise in ["target", ".git", "node_modules"] {
        let dir = root.join(noise);
        fs::create_dir(&dir).unwrap();
        touch(&dir.join("payload.bin"));
    }
    touch(&root.join("keep.md"));

    assert_eq!(rendered(root), vec!["0:keep.md:keep.md"]);
}

/// Naturally-empty directories still prune, transitively.
#[test]
fn genesis_prunes_empty_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // empty/ and hollow/inner/ contain no files at any depth.
    fs::create_dir(root.join("empty")).unwrap();
    fs::create_dir_all(root.join("hollow").join("inner")).unwrap();
    // full/ has one file, so it survives.
    let full = root.join("full");
    fs::create_dir(&full).unwrap();
    touch(&full.join("data.json"));

    assert_eq!(
        rendered(root),
        vec!["0:full:full", "1:full/data.json:data.json"]
    );
}

/// Symlinks are never followed — neither file links, dir links, nor a
/// self-referential cycle. The scan terminates and links produce no nodes.
#[cfg(unix)]
#[test]
fn genesis_skips_symlinks() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let real_dir = root.join("real");
    fs::create_dir(&real_dir).unwrap();
    touch(&real_dir.join("file.txt"));

    // Link to a file, link to a dir, and a cycle back to the root itself.
    symlink(real_dir.join("file.txt"), root.join("file_link.txt")).unwrap();
    symlink(&real_dir, root.join("dir_link")).unwrap();
    symlink(root, real_dir.join("cycle")).unwrap();

    assert_eq!(
        rendered(root),
        vec!["0:real:real", "1:real/file.txt:file.txt"]
    );
}

/// A directory whose only content is symlinks prunes like an empty one.
#[cfg(unix)]
#[test]
fn genesis_prunes_dir_of_only_symlinks() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    touch(&root.join("anchor.md"));
    let links = root.join("links");
    fs::create_dir(&links).unwrap();
    symlink(root.join("anchor.md"), links.join("alias.md")).unwrap();

    assert_eq!(rendered(root), vec!["0:anchor.md:anchor.md"]);
}
