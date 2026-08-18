// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Finder capability tests — navigation cursor, file verbs, and the sandbox.
// Fixtures live in a tempdir so every outcome is deterministic. Each op is a
// round-trip against the real filesystem; a test that could not fail is a
// defect.

use std::fs;
use std::path::Path;

use bandy::state::{BrowseKind, FsOutcome, FsVerb};
use bandy::{MatrixEvent, Origin};
use matrix::finder::Finder;
use matrix::MatrixScanner;

fn touch(p: &Path) {
    fs::write(p, b"x").unwrap();
}

fn names(listing: &bandy::state::BrowseListing) -> Vec<String> {
    listing.entries.iter().map(|e| e.name.clone()).collect()
}

fn principal() -> Origin {
    Origin::LocalUser("peter".to_string())
}

// --- NAVIGATION ------------------------------------------------------------

#[test]
fn list_root_orders_dirs_first_then_files_and_keeps_empty_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir(root.join("empty")).unwrap(); // a Finder SHOWS empty dirs
    touch(&root.join("src/main.rs"));
    touch(&root.join("Cargo.toml"));
    touch(&root.join("README.md"));

    let f = Finder::new(root.to_path_buf());
    let listing = f.list("").unwrap();

    // dirs (empty, src) first, then files (Cargo.toml, README.md), each sorted.
    assert_eq!(names(&listing), vec!["empty", "src", "Cargo.toml", "README.md"]);
    assert_eq!(listing.path, "");
    assert_eq!(listing.parent, None); // ascent stops at root
    assert_eq!(listing.breadcrumbs, vec![(String::new(), String::new())]);

    // The empty dir is a Dir entry with size 0.
    let empty = listing.entries.iter().find(|e| e.name == "empty").unwrap();
    assert_eq!(empty.kind, BrowseKind::Dir);
}

#[test]
fn descend_reports_parent_and_breadcrumbs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("a/b")).unwrap();
    touch(&root.join("a/b/leaf.txt"));

    let f = Finder::new(root.to_path_buf());
    let listing = f.list("a/b").unwrap();

    assert_eq!(listing.path, "a/b");
    assert_eq!(listing.parent.as_deref(), Some("a"));
    assert_eq!(
        listing.breadcrumbs,
        vec![
            (String::new(), String::new()),
            ("a".to_string(), "a".to_string()),
            ("b".to_string(), "a/b".to_string()),
        ]
    );
    assert_eq!(names(&listing), vec!["leaf.txt"]);
}

#[test]
fn ascend_from_child_reaches_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("a/b")).unwrap();
    touch(&root.join("a/sibling.txt"));

    let f = Finder::new(root.to_path_buf());
    let child = f.list("a/b").unwrap();
    let parent_rel = child.parent.unwrap();
    let parent = f.list(&parent_rel).unwrap();
    assert!(parent.entries.iter().any(|e| e.name == "sibling.txt"));
    assert!(parent.entries.iter().any(|e| e.name == "b"));
}

#[test]
fn list_excludes_build_noise_and_trash() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for noise in ["target", ".git", "node_modules", ".una-trash"] {
        fs::create_dir(root.join(noise)).unwrap();
        touch(&root.join(noise).join("payload"));
    }
    touch(&root.join("keep.md"));

    let f = Finder::new(root.to_path_buf());
    assert_eq!(names(&f.list("").unwrap()), vec!["keep.md"]);
}

#[test]
fn resolve_rejects_dotdot_escape() {
    let tmp = tempfile::tempdir().unwrap();
    let f = Finder::new(tmp.path().to_path_buf());
    let out = f.list("../secrets");
    assert!(matches!(out, Err(FsOutcome::Denied { .. })), "got {out:?}");
}

#[cfg(unix)]
#[test]
fn symlink_shown_flagged_but_never_followed() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let real = root.join("real");
    fs::create_dir(&real).unwrap();
    touch(&real.join("inside.txt"));
    symlink(&real, root.join("link")).unwrap();

    let f = Finder::new(root.to_path_buf());
    let listing = f.list("").unwrap();
    let link = listing.entries.iter().find(|e| e.name == "link").unwrap();
    assert!(link.is_symlink, "symlink must be flagged");
    assert_eq!(link.kind, BrowseKind::Other, "link classified without following");

    // Navigating THROUGH the symlink is refused.
    let out = f.list("link");
    assert!(matches!(out, Err(FsOutcome::Denied { .. })), "got {out:?}");
    // And an op through the link is refused too.
    assert!(matches!(f.open("link/inside.txt"), FsOutcome::Denied { .. }));
}

// --- FILE OPERATIONS -------------------------------------------------------

#[test]
fn new_folder_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let f = Finder::new(root.to_path_buf());

    let out = f.new_folder("", "docs");
    assert_eq!(out, FsOutcome::Ok { path: "docs".to_string() });
    assert!(root.join("docs").is_dir());
    assert!(f.list("").unwrap().entries.iter().any(|e| e.name == "docs"));

    // Rejections: duplicate, and a name with a separator.
    assert!(matches!(f.new_folder("", "docs"), FsOutcome::Denied { .. }));
    assert!(matches!(f.new_folder("", "a/b"), FsOutcome::Denied { .. }));
}

#[test]
fn rename_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    touch(&root.join("old.md"));
    let f = Finder::new(root.to_path_buf());

    let out = f.rename("old.md", "new.md");
    assert_eq!(out, FsOutcome::Ok { path: "new.md".to_string() });
    assert!(!root.join("old.md").exists());
    assert!(root.join("new.md").exists());

    // A separator in the new name is refused (no relocation via rename).
    assert!(matches!(f.rename("new.md", "sub/x.md"), FsOutcome::Denied { .. }));
}

#[test]
fn copy_file_and_dir_recursively() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join("dst")).unwrap();
    fs::create_dir_all(root.join("srcdir/nested")).unwrap();
    fs::write(root.join("srcdir/a.txt"), b"hello").unwrap();
    fs::write(root.join("srcdir/nested/b.txt"), b"deep").unwrap();
    touch(&root.join("file.txt"));

    let f = Finder::new(root.to_path_buf());

    // File copy.
    let out = f.copy("file.txt", "dst");
    assert_eq!(out, FsOutcome::Ok { path: "dst/file.txt".to_string() });
    assert!(root.join("dst/file.txt").exists());
    assert!(root.join("file.txt").exists(), "copy leaves the original");

    // Recursive dir copy.
    let out = f.copy("srcdir", "dst");
    assert_eq!(out, FsOutcome::Ok { path: "dst/srcdir".to_string() });
    assert_eq!(fs::read(root.join("dst/srcdir/a.txt")).unwrap(), b"hello");
    assert_eq!(fs::read(root.join("dst/srcdir/nested/b.txt")).unwrap(), b"deep");

    // Copying a dir into itself is refused.
    assert!(matches!(f.copy("srcdir", "srcdir/nested"), FsOutcome::Denied { .. }));
}

#[test]
fn move_relocates_the_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join("dst")).unwrap();
    touch(&root.join("m.txt"));
    let f = Finder::new(root.to_path_buf());

    let out = f.mv("m.txt", "dst");
    assert_eq!(out, FsOutcome::Ok { path: "dst/m.txt".to_string() });
    assert!(!root.join("m.txt").exists());
    assert!(root.join("dst/m.txt").exists());
}

#[test]
fn delete_confirms_then_moves_to_trash_reversibly() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    touch(&root.join("doomed.txt"));
    let f = Finder::new(root.to_path_buf());

    // Unconfirmed: nothing happens, the file survives.
    assert_eq!(f.delete("doomed.txt", false), FsOutcome::NeedsConfirm);
    assert!(root.join("doomed.txt").exists());

    // Confirmed: moved into .una-trash (NOT hard-deleted → reversible).
    let out = f.delete("doomed.txt", true);
    let FsOutcome::Ok { path } = out else {
        panic!("expected Ok, got {out:?}");
    };
    assert!(path.starts_with(".una-trash/"), "trashed to {path}");
    assert!(!root.join("doomed.txt").exists());
    assert!(root.join(&path).exists(), "the trashed copy is recoverable");
    assert_eq!(fs::read(root.join(&path)).unwrap(), b"x");
}

#[test]
fn open_accepts_files_and_refuses_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    touch(&root.join("f.txt"));
    fs::create_dir(root.join("d")).unwrap();
    let f = Finder::new(root.to_path_buf());

    assert_eq!(f.open("f.txt"), FsOutcome::Ok { path: "f.txt".to_string() });
    assert!(matches!(f.open("d"), FsOutcome::Denied { .. }));
    assert!(matches!(f.open("nope.txt"), FsOutcome::Error { .. } | FsOutcome::Denied { .. }));
}

#[cfg(unix)]
#[test]
fn write_to_readonly_dir_surfaces_loud_denial() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let ro = root.join("ro");
    fs::create_dir(&ro).unwrap();
    // Read + execute, no write: a create inside is refused by the OS.
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();

    let f = Finder::new(root.to_path_buf());
    let out = f.new_folder("ro", "child");
    // The FAT-verb posture: a read-only volume answers Denied, loudly — not a
    // silent no-op and not a generic Error.
    assert!(matches!(out, FsOutcome::Denied { .. }), "got {out:?}");
    assert!(!ro.join("child").exists());

    // Restore perms so the tempdir can be cleaned up.
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
}

// --- DISPATCH (the event mapping the bus fires) ----------------------------

#[test]
fn dispatch_browse_to_yields_dir_listed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    touch(&root.join("a.txt"));
    let f = Finder::new(root.to_path_buf());

    let out = f.dispatch(&MatrixEvent::BrowseTo { principal: principal(), path: String::new() });
    assert_eq!(out.len(), 1);
    match &out[0] {
        MatrixEvent::DirListed(listing) => {
            assert!(listing.entries.iter().any(|e| e.name == "a.txt"));
        }
        other => panic!("expected DirListed, got {other:?}"),
    }
}

#[test]
fn dispatch_file_op_returns_result_and_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let f = Finder::new(root.to_path_buf());

    let out = f.dispatch(&MatrixEvent::FileOp {
        principal: principal(),
        verb: FsVerb::NewFolder,
        path: String::new(),
        arg: Some("made".to_string()),
        confirmed: false,
    });
    // FsOpResult (principal preserved) + a refreshed DirListed.
    assert_eq!(out.len(), 2);
    match &out[0] {
        MatrixEvent::FsOpResult { principal: p, verb, outcome, .. } => {
            assert_eq!(p, &principal());
            assert_eq!(*verb, FsVerb::NewFolder);
            assert_eq!(*outcome, FsOutcome::Ok { path: "made".to_string() });
        }
        other => panic!("expected FsOpResult, got {other:?}"),
    }
    assert!(matches!(&out[1], MatrixEvent::DirListed(l) if l.entries.iter().any(|e| e.name == "made")));
    assert!(root.join("made").is_dir());
}

#[test]
fn dispatch_denied_browse_reports_result_not_listing() {
    let tmp = tempfile::tempdir().unwrap();
    let f = Finder::new(tmp.path().to_path_buf());
    let out = f.dispatch(&MatrixEvent::BrowseTo { principal: principal(), path: "../escape".to_string() });
    assert_eq!(out.len(), 1);
    assert!(matches!(&out[0], MatrixEvent::FsOpResult { outcome: FsOutcome::Denied { .. }, .. }));
}

#[test]
fn dispatch_ignores_non_finder_events() {
    let tmp = tempfile::tempdir().unwrap();
    let f = Finder::new(tmp.path().to_path_buf());
    let out = f.dispatch(&MatrixEvent::FocusSector("euclase".to_string()));
    assert!(out.is_empty());
}

// --- COEXISTENCE: the Finder does not change the DAG genesis behaviour ------

#[test]
fn finder_and_genesis_are_distinct_capabilities() {
    // The genesis DAG PRUNES empty dirs; the Finder cursor SHOWS them. Proving
    // they differ on the same tree proves the added capability did not alter
    // the existing topology scan.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join("empty")).unwrap();
    touch(&root.join("keep.rs"));

    let genesis = MatrixScanner::build_genesis_tree(root, root);
    assert!(
        !genesis.iter().any(|n| n.label == "empty"),
        "genesis DAG still prunes empty dirs"
    );

    let f = Finder::new(root.to_path_buf());
    assert!(
        f.list("").unwrap().entries.iter().any(|e| e.name == "empty"),
        "Finder cursor shows empty dirs"
    );
}
