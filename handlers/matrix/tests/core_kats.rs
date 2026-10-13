// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// MATRIXCORE (LEDGER SR99) host KATs: the host Matrix and `matrix_core` give the same answers on one fixture
// tree — the genesis topology (host `build_genesis_tree` over std::fs vs the core over a `MemVolume` of the same
// rows), the Finder's listing, the sandboxed resolver, the token indexer's payload and DAG — plus the core's own
// ignore rules and query model. Fixtures are built in a tempdir so every expectation is deterministic.

use std::fs;
use std::path::Path;

use matrix::matrix_core::{self, Ignore, Kind, MemVolume, Rank, ScanDepth};
use matrix::{Finder, MatrixScanner, StdVolume};

const LIB_RS: &str = "use crate::a::{b, c};\nmod inner;\n// use decoy::path;\npub fn alpha() {}\npub struct Beta {}\nfn hidden() {}\nlet s = \"use fake::x;\";\n";

/// The fixture as rows: (path, contents) for files, `None` for an empty directory.
const ROWS: &[(&str, Option<&str>)] = &[
    ("src/lib.rs", Some(LIB_RS)),
    ("src/z_last.md", Some("z")),
    ("src/A_upper.txt", Some("A")),
    ("assets/logo.png", Some("png")),
    ("empty", None),
    ("deep/er/still/leaf.toml", Some("t")),
    ("deep/hollow", None),
    ("target/debug/out.bin", Some("noise")),
    (".git/HEAD", Some("ref")),
    ("node_modules/x/index.js", Some("js")),
    ("sub/target", Some("a FILE named target is noise too")),
    (".hidden.cfg", Some("h")),
    ("Cargo.toml", Some("[package]")),
    ("README.md", Some("r")),
];

fn build_disk(root: &Path) {
    for (p, c) in ROWS {
        let abs = root.join(p);
        match c {
            Some(text) => {
                fs::create_dir_all(abs.parent().unwrap()).unwrap();
                fs::write(&abs, text).unwrap();
            }
            None => fs::create_dir_all(&abs).unwrap(),
        }
    }
}

fn build_mem() -> MemVolume {
    // Rows in REVERSE so the core is proven to do its own ordering (a MemVolume lists in insertion order).
    let mut v = MemVolume::new();
    for (p, c) in ROWS.iter().rev() {
        v = match c {
            Some(text) => v.file(p, text.as_bytes()),
            None => v.dir(p),
        };
    }
    v
}

fn render_host(nodes: &[bandy::state::TopologyNode], depth: usize, out: &mut Vec<String>) {
    for n in nodes {
        out.push(format!("{}:{}:{}", depth, n.id, n.label));
        render_host(&n.children, depth + 1, out);
    }
}

fn render_core(nodes: &[matrix_core::Node], depth: usize, out: &mut Vec<String>) {
    for n in nodes {
        out.push(format!("{}:{}:{}", depth, n.id, n.label));
        render_core(&n.children, depth + 1, out);
    }
}

const GENESIS_GOLDEN: &[&str] = &[
    "0:assets:assets",
    "1:assets/logo.png:logo.png",
    "0:deep:deep",
    "1:deep/er:er",
    "2:deep/er/still:still",
    "3:deep/er/still/leaf.toml:leaf.toml",
    "0:src:src",
    "1:src/A_upper.txt:A_upper.txt",
    "1:src/lib.rs:lib.rs",
    "1:src/z_last.md:z_last.md",
    "0:.hidden.cfg:.hidden.cfg",
    "0:Cargo.toml:Cargo.toml",
    "0:README.md:README.md",
];

#[test]
fn genesis_host_and_core_identical_on_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_disk(root);

    let mut host = Vec::new();
    render_host(&MatrixScanner::build_genesis_tree(root, root), 0, &mut host);

    let mut core = Vec::new();
    render_core(&matrix_core::genesis(&mut build_mem(), "", &Ignore::build_noise()), 0, &mut core);

    // The core over the host's own std volume, too: the same walk on the same disk.
    let mut core_std = Vec::new();
    render_core(&matrix_core::genesis(&mut StdVolume::new(root), "", &Ignore::build_noise()), 0, &mut core_std);

    assert_eq!(host, GENESIS_GOLDEN);
    assert_eq!(core, GENESIS_GOLDEN);
    assert_eq!(core_std, GENESIS_GOLDEN);
}

#[cfg(unix)]
#[test]
fn genesis_never_follows_links_host_and_core() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/f.rs"), "x").unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("loop")).unwrap();
    std::os::unix::fs::symlink(root.join("real/f.rs"), root.join("alias.rs")).unwrap();

    let mut host = Vec::new();
    render_host(&MatrixScanner::build_genesis_tree(root, root), 0, &mut host);
    let mem = &mut MemVolume::new().file("real/f.rs", b"x").link("loop").link("alias.rs");
    let mut core = Vec::new();
    render_core(&matrix_core::genesis(mem, "", &Ignore::build_noise()), 0, &mut core);

    assert_eq!(host, vec!["0:real:real", "1:real/f.rs:f.rs"]);
    assert_eq!(core, host);
}

#[test]
fn finder_listing_host_and_core_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_disk(root);
    let f = Finder::new(root.to_path_buf());

    for dir in ["", "src", "deep", "sub"] {
        let host: Vec<(String, String, bool)> = f
            .list(dir)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| (e.path, e.name, e.kind == bandy::state::BrowseKind::Dir))
            .collect();
        let core: Vec<(String, String, bool)> = matrix_core::list_dir(&mut build_mem(), dir, &Ignore::build_noise())
            .unwrap()
            .into_iter()
            .map(|it| (it.path, it.name, it.kind == Kind::Dir))
            .collect();
        assert_eq!(host, core, "listing of {dir:?}");
    }

    // The root listing shows the EMPTY dir (the Finder is not the genesis scan) and hides the noise.
    let names: Vec<String> = f.list("").unwrap().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(names, vec!["assets", "deep", "empty", "src", "sub", ".hidden.cfg", "Cargo.toml", "README.md"]);
    // `sub` holds only a FILE named `target`: listed (empty) by the Finder, pruned by genesis.
    assert!(f.list("sub").unwrap().entries.is_empty());
}

#[test]
fn listing_frame_matches_the_finder() {
    let tmp = tempfile::tempdir().unwrap();
    build_disk(tmp.path());
    let f = Finder::new(tmp.path().to_path_buf());
    let l = f.list("deep/er/still").unwrap();
    let (parent, crumbs) = matrix_core::breadcrumbs("deep/er/still");
    assert_eq!(l.parent, parent);
    assert_eq!(l.breadcrumbs, crumbs);
    assert_eq!(parent.as_deref(), Some("deep/er"));
    assert_eq!(matrix_core::breadcrumbs("").0, None);
}

#[test]
fn resolve_sandbox_reasons_and_normal_form() {
    let mem = &mut MemVolume::new().file("a/b/c.rs", b"").link("a/ln");
    assert_eq!(matrix_core::resolve(mem, "").unwrap(), "");
    assert_eq!(matrix_core::resolve(mem, "./a//b/").unwrap(), "a/b");
    assert_eq!(matrix_core::resolve(mem, "a/new/deeper").unwrap(), "a/new/deeper");
    assert_eq!(matrix_core::resolve(mem, "/etc/passwd"), Err(matrix_core::tree::DENY_ABSOLUTE));
    assert_eq!(matrix_core::resolve(mem, "a/../../x"), Err(matrix_core::tree::DENY_ESCAPE));
    assert_eq!(matrix_core::resolve(mem, "a/ln/x"), Err(matrix_core::tree::DENY_SYMLINK));

    // The host Finder refuses with the core's reasons, verbatim.
    let tmp = tempfile::tempdir().unwrap();
    let f = Finder::new(tmp.path().to_path_buf());
    match f.resolve("../x") {
        Err(bandy::state::FsOutcome::Denied { reason }) => assert_eq!(reason, matrix_core::tree::DENY_ESCAPE),
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[test]
fn bare_name_and_parent_rel() {
    assert_eq!(matrix_core::bare_name("  ok.txt "), Some("ok.txt"));
    for bad in ["", " ", ".", "..", "a/b", "a\\b", "a\0b"] {
        assert_eq!(matrix_core::bare_name(bad), None, "{bad:?}");
    }
    assert_eq!(matrix_core::parent_rel("a/b/c"), "a/b");
    assert_eq!(matrix_core::parent_rel("top"), "");
    assert_eq!(matrix_core::parent_rel("a/b/"), "a");
}

#[test]
fn indexer_single_file_payload_identical_to_host() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_disk(root);

    let host = MatrixScanner::map_topology(&[root.join("src/lib.rs")], root, ScanDepth::Interface).unwrap();

    let mut ix = matrix_core::Indexer::new();
    ix.scan_file("src/lib.rs", Some(LIB_RS.as_bytes()), ScanDepth::Interface, true);
    let core = ix.finish();

    assert_eq!(host, core);
    // dict: node, symbols (pub only at Interface), then use/mod deps; the decoys never appear.
    assert_eq!(core.0, "src/lib.rs,fn alpha,struct Beta,crate::a::b,crate::a::c,inner$0:1,2,3,4,5");
    assert!(core.1.contains("[src/lib.rs] relies on: fn alpha, struct Beta, crate::a::b, crate::a::c, inner\n"));
    assert!(core.1.contains("[inner] operates independently.\n"));
    assert!(!core.0.contains("decoy") && !core.0.contains("fake") && !core.0.contains("hidden"));
}

#[test]
fn indexer_tree_walk_matches_host_on_one_source_dir() {
    // A directory holding ONE Rust source: the host's read_dir order cannot differ from the core's walk.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), LIB_RS).unwrap();
    fs::write(root.join("src/notes.md"), "use not::rust;").unwrap();

    let host = MatrixScanner::map_topology(&[root.join("src")], root, ScanDepth::Interface).unwrap();

    let mut ix = matrix_core::Indexer::new();
    let mem = &mut MemVolume::new().file("src/lib.rs", LIB_RS.as_bytes()).file("src/notes.md", b"use not::rust;");
    matrix_core::index::index_tree(mem, "src", "src", &Ignore::build_noise(), ScanDepth::Interface, &mut ix);
    assert_eq!(host, ix.finish());
    // A directory scan runs no symbol pass.
    assert_eq!(host.0, "src/lib.rs,crate::a::b,crate::a::c,inner$0:1,2,3");
}

#[test]
fn gitignore_subset_and_hidden() {
    let ig = Ignore::build_noise().gitignore(
        "",
        "# comment\n\n*.log\n/build/\ndocs/*.tmp\n!keep.log\n**/cache\nout/**\n\\#literal\n",
    );
    let t = |p: &str, dir: bool| ig.ignored(p, matrix_core::base_name(p), dir);
    assert!(t("a/b/x.log", false));
    assert!(!t("a/keep.log", false), "negation: the last matching rule wins");
    assert!(t("build", true));
    assert!(!t("build", false), "trailing / = directories only");
    assert!(!t("src/build", true), "leading / anchors at the base");
    assert!(t("docs/a.tmp", false));
    assert!(!t("other/docs/a.tmp", false), "a pattern with an inner / is anchored");
    assert!(!t("docs/sub/a.tmp", false), "* never crosses /");
    assert!(t("x/y/cache", true));
    assert!(t("out/a/b.bin", false));
    assert!(t("#literal", false));
    assert!(t("deep/target", true), "the build noise stays");

    let hid = Ignore::none().hidden(true);
    assert!(hid.ignored(".env", ".env", false));
    assert!(!hid.ignored("env", "env", false));

    // An ignored directory is never descended: its children cannot come back.
    let mem = &mut MemVolume::new().file("build/keep.log", b"").file("src/a.rs", b"");
    let tree = matrix_core::genesis(mem, "", &ig);
    let mut out = Vec::new();
    render_core(&tree, 0, &mut out);
    assert_eq!(out, vec!["0:src:src", "1:src/a.rs:a.rs"]);
}

#[test]
fn query_ranks_and_keeps_tree_order() {
    let mut v = build_mem().file("src/lib_extra.rs", b"").file("readme_lib", b"");
    let tree = matrix_core::genesis(&mut v, "", &Ignore::build_noise());
    let hits: Vec<(String, Rank)> = matrix_core::query(&tree, "LIB.rs").into_iter().map(|h| (h.id, h.rank)).collect();
    // `lib_extra.rs` holds l-i-b-.-r-s in order: a subsequence hit, ranked last.
    assert_eq!(hits, vec![("src/lib.rs".to_string(), Rank::Exact), ("src/lib_extra.rs".to_string(), Rank::Subsequence)]);

    let hits: Vec<(String, Rank)> = matrix_core::query(&tree, "lib").into_iter().map(|h| (h.id, h.rank)).collect();
    assert_eq!(
        hits,
        vec![
            ("src/lib.rs".to_string(), Rank::Prefix),
            ("src/lib_extra.rs".to_string(), Rank::Prefix),
            ("readme_lib".to_string(), Rank::Substring),
        ]
    );

    let hits: Vec<String> = matrix_core::query(&tree, "lgo").into_iter().map(|h| h.id).collect();
    assert_eq!(hits, vec!["assets/logo.png".to_string()]);

    let hits: Vec<String> = matrix_core::query(&tree, "deep/er/still").into_iter().map(|h| h.id).collect();
    assert_eq!(hits, vec!["deep/er/still".to_string(), "deep/er/still/leaf.toml".to_string()]);

    assert!(matrix_core::query(&tree, "").is_empty());

    // The host face answers with the same model over its std volume.
    let tmp = tempfile::tempdir().unwrap();
    build_disk(tmp.path());
    let host: Vec<String> = MatrixScanner::find(tmp.path(), "logo").into_iter().map(|h| h.id).collect();
    assert_eq!(host, vec!["assets/logo.png".to_string()]);
}
