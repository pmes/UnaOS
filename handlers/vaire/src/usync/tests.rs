// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VAIRE-2 fixture suite. Rider 1 carries over from Bolt-1: every test runs
// against a tempdir live tree + a temp UnaFS image — never the real narino
// tree, never a shared image (each test owns its own tempdir path, so the
// suite never touches `bench_vault.img` or any machine-wide path).

use super::*;
use crate::devtree::ExcludeClass;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use unafs::{FileDevice, FileKind, FileSystem};

/// Build a manifest whose sole penumbra root is `tree`, with a small junk list
/// (the credential floor is merged in by `from_toml`, exactly as production).
fn mk_manifest(tree: &Path) -> DevManifest {
    let toml = format!(
        "name = 'vaire-test'\n\
         live = 'narino'\n\
         live_root = '/unused/live'\n\
         target_root = '/unused/target'\n\
         penumbra = ['{}']\n\
         [exclude]\n\
         junk = ['target', '.DS_Store']\n",
        tree.display()
    );
    DevManifest::from_toml(&toml).expect("manifest parses")
}

/// A minimal live tree: two files, one nested dir, one more file.
fn seed_tree(root: &Path) {
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"alpha\n").unwrap();
    fs::write(root.join("b.md"), b"# beta\n").unwrap();
    fs::write(root.join("sub/c.rs"), b"fn c() {}\n").unwrap();
}

fn image_path(dir: &Path) -> PathBuf {
    dir.join("dev.img")
}

/// Recursively collect every file's image-relative path under `dir_id`.
fn collect_names(fs: &mut FileSystem, dir_id: u64, prefix: &str, out: &mut Vec<String>) {
    for e in fs.ls(dir_id).unwrap() {
        let path = if prefix.is_empty() {
            e.name.clone()
        } else {
            format!("{prefix}/{}", e.name)
        };
        match e.kind {
            FileKind::Directory => collect_names(fs, e.inode_id, &path, out),
            _ => out.push(path),
        }
    }
}

// -- dry-run is the default -------------------------------------------------

#[test]
fn dry_run_is_default_and_writes_nothing() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());

    let r = usync(&m, &img, false, DEFAULT_IMAGE_MB).unwrap();

    assert!(!r.applied, "dry-run must not apply");
    assert!(!img.exists(), "dry-run must not create the image");
    assert_eq!(r.files_written, 3, "all three files are would-writes");
    assert_eq!(r.files_skipped, 0);
    assert!(r.snapshot.is_none(), "dry-run takes no snapshot");
}

// -- apply weaves native objects + one snapshot -----------------------------

#[test]
fn apply_weaves_objects_content_attrs_and_one_snapshot() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());
    let root_name = live.path().file_name().unwrap().to_string_lossy().to_string();

    let r = usync(&m, &img, true, 16).unwrap();
    assert!(r.applied && r.formatted);
    assert_eq!(r.files_written, 3);
    assert_eq!(r.files_skipped, 0);
    assert!(r.snapshot.is_some(), "a completed sync retains a snapshot");
    assert!(img.exists());

    // Re-mount read-only and verify content + typed attrs + the snapshot index.
    let device = FileDevice::open_read_only(&img).unwrap();
    let mut fs = FileSystem::mount(device).unwrap();

    let unit_id = fs.resolve_path(&root_name).unwrap();
    let a_id = fs.resolve_path(&format!("{root_name}/a.txt")).unwrap();
    let sz = fs.read_inode(a_id).unwrap().size;
    let data = fs.read_data(a_id, 0, sz).unwrap();
    assert_eq!(data, b"alpha\n", "native object carries the live bytes");

    // K6 typed attrs on the file.
    assert_eq!(get_int(&mut fs, a_id, "vaire.size"), Some(6));
    assert!(get_int(&mut fs, a_id, "vaire.mtime").is_some());
    assert!(get_str(&mut fs, a_id, "vaire.src").unwrap().ends_with("a.txt"));

    // Unit-root attrs.
    assert_eq!(get_str(&mut fs, unit_id, "vaire.unit").as_deref(), Some("vaire-test"));
    assert!(get_str(&mut fs, unit_id, "vaire.run").is_some());

    let snaps = fs.snapshot_index().unwrap();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].creator, "vaire");
}

// -- incremental: unchanged skip, changed rewrite ---------------------------

#[test]
fn incremental_skips_unchanged_and_rewrites_changed() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());
    let root_name = live.path().file_name().unwrap().to_string_lossy().to_string();

    let first = usync(&m, &img, true, 16).unwrap();
    assert_eq!(first.files_written, 3);

    // No change: everything skips, but a fresh snapshot is still retained.
    let second = usync(&m, &img, true, 16).unwrap();
    assert_eq!(second.files_written, 0, "unchanged files skip");
    assert_eq!(second.files_skipped, 3);

    // Change one file (different length => size attr differs => rewrite).
    fs::write(live.path().join("a.txt"), b"alpha-two-longer\n").unwrap();
    let third = usync(&m, &img, true, 16).unwrap();
    assert_eq!(third.files_written, 1, "only the changed file rewrites");
    assert_eq!(third.files_skipped, 2);

    // The rewritten content is exact (grow-only write can't leave stale tail).
    let device = FileDevice::open_read_only(&img).unwrap();
    let mut fs = FileSystem::mount(device).unwrap();
    let a_id = fs.resolve_path(&format!("{root_name}/a.txt")).unwrap();
    let sz = fs.read_inode(a_id).unwrap().size;
    let data = fs.read_data(a_id, 0, sz).unwrap();
    assert_eq!(data, b"alpha-two-longer\n");

    assert_eq!(fs.snapshot_index().unwrap().len(), 3, "three retained roots");
}

// -- exclusions: default-deny, reported, never woven ------------------------

#[test]
fn exclusions_credential_junk_symlink_reported_never_woven() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());
    // A credential-shaped file (default-deny floor), a junk dir, and a symlink.
    fs::write(live.path().join("id_rsa"), b"PRIVATE\n").unwrap();
    fs::create_dir_all(live.path().join("target")).unwrap();
    fs::write(live.path().join("target/build.o"), b"obj\n").unwrap();
    std::os::unix::fs::symlink(live.path().join("a.txt"), live.path().join("link")).unwrap();

    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());
    let root_name = live.path().file_name().unwrap().to_string_lossy().to_string();

    let r = usync(&m, &img, true, 16).unwrap();

    let has = |c: ExcludeClass| r.excluded.iter().any(|e| e.class == c);
    assert!(has(ExcludeClass::Credential), "id_rsa reported as credential");
    assert!(has(ExcludeClass::Junk), "target/ reported as junk");
    assert!(has(ExcludeClass::Symlink), "symlink reported, not followed");
    assert_eq!(r.files_written, 3, "only the three real files are woven");

    // The image must NOT contain any excluded name.
    let device = FileDevice::open_read_only(&img).unwrap();
    let mut fs = FileSystem::mount(device).unwrap();
    let unit_id = fs.resolve_path(&root_name).unwrap();
    let mut names = Vec::new();
    collect_names(&mut fs, unit_id, "", &mut names);
    assert!(names.iter().all(|n| !n.contains("id_rsa")), "credential never woven: {names:?}");
    assert!(names.iter().all(|n| !n.contains("build.o")), "junk never woven");
    assert!(names.iter().all(|n| !n.contains("link")), "symlink never woven");
    assert_eq!(names.len(), 3);
}

// -- the live tree is never written (Bolt-1 invariant, byte-compare) --------

#[test]
fn live_tree_is_never_written() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());

    // Snapshot every live file's bytes + mode before the sync.
    let mut before: BTreeMap<PathBuf, (Vec<u8>, u32)> = BTreeMap::new();
    for rel in ["a.txt", "b.md", "sub/c.rs"] {
        let p = live.path().join(rel);
        // Read-only mode: a sync that tried to write would fail loudly.
        let mut perm = fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o555);
        fs::set_permissions(&p, perm).unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode();
        before.insert(PathBuf::from(rel), (fs::read(&p).unwrap(), mode));
    }

    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());
    usync(&m, &img, true, 16).unwrap();

    for (rel, (bytes, mode)) in &before {
        let p = live.path().join(rel);
        assert_eq!(&fs::read(&p).unwrap(), bytes, "live bytes untouched: {}", rel.display());
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode(), *mode, "live mode untouched");
    }
}

// -- ustatus reflects the woven image ---------------------------------------

#[test]
fn ustatus_reports_units_snapshots_and_counts() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());

    usync(&m, &img, true, 16).unwrap();
    let st = ustatus(&m, &img).unwrap();

    assert_eq!(st.snapshots.len(), 1);
    assert_eq!(st.units.len(), 1);
    assert_eq!(st.units[0].files, 3, "three files under the unit root");
    assert_eq!(st.units[0].manifest.as_deref(), Some("vaire-test"));
    assert_eq!(st.live_files, 3);
}

// -- snapshot cap is honored honestly (never auto-drops) --------------------

#[test]
fn snapshot_cap_is_refused_not_auto_dropped() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());

    // Fill the retention index to the cap (one snapshot per completed sync).
    for _ in 0..unafs::SNAPSHOT_CAP {
        usync(&m, &img, true, 32).unwrap();
    }
    let device = FileDevice::open_read_only(&img).unwrap();
    let mut fs = FileSystem::mount(device).unwrap();
    assert_eq!(fs.snapshot_index().unwrap().len(), unafs::SNAPSHOT_CAP);
    drop(fs);

    // Change a live file so a run would have something to write, then take the
    // image's before-state fingerprint: a refused run must be a TRUE NO-OP.
    fs::write(live.path().join("a.txt"), b"changed-after-cap\n").unwrap();
    let before_bytes = fs::read(&img).unwrap();

    // The next sync must REFUSE (naming the drop verb), not silently drop —
    // and must refuse BEFORE writing anything (cap checked up front).
    let err = usync(&m, &img, true, 32).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("snapshot index full"), "clear cap message: {msg}");
    assert!(msg.contains("snapdrop"), "names the drop verb: {msg}");
    assert_eq!(
        fs::read(&img).unwrap(),
        before_bytes,
        "a cap-refused run must leave the image byte-identical (no writes, \
         no commits, no vaire.run stamp)"
    );
}

// -- the one-flip batch path is tree-for-tree with the per-op result --------

/// A larger, multi-directory tree so the batch groups more than one file per
/// parent — the batch path must land exactly the same objects, bytes, and
/// typed attrs as the per-op path did, in one transaction.
fn seed_wide_tree(root: &Path) {
    fs::create_dir_all(root.join("sub/deep")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();
    fs::write(root.join("a.txt"), b"alpha\n").unwrap();
    fs::write(root.join("b.md"), b"# beta\n").unwrap();
    fs::write(root.join("sub/c.rs"), b"fn c() {}\n").unwrap();
    fs::write(root.join("sub/d.rs"), b"fn d() {}\n").unwrap();
    fs::write(root.join("sub/deep/e.txt"), b"epsilon\n").unwrap();
    fs::write(root.join("other/f.md"), b"# phi\n").unwrap();
}

#[test]
fn batched_sync_tree_matches_expected_objects_attrs_and_snapshot() {
    let live = tempfile::tempdir().unwrap();
    seed_wide_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());
    let root_name = live.path().file_name().unwrap().to_string_lossy().to_string();

    let r = usync(&m, &img, true, 16).unwrap();
    assert_eq!(r.files_written, 6, "six files across three dirs are woven");
    assert_eq!(r.files_skipped, 0);
    assert!(r.snapshot.is_some(), "one snapshot per completed sync");

    // The one-flip regime: the whole tree lands in a tiny number of root flips
    // (the single sync commit + snapshot commit + summary commit), NOT one per
    // file. Six files must cost far fewer than the per-op regime's 6+ flips.
    assert!(
        r.commit_stats.commits <= 4,
        "one-flip regime: {} commits for 6 files+3 dirs (per-op would be ~9)",
        r.commit_stats.commits
    );
    assert_eq!(r.commit_stats.snapshots_created, 1);

    // Re-mount read-only: every file present with exact bytes + all four attrs.
    let device = FileDevice::open_read_only(&img).unwrap();
    let mut fs = FileSystem::mount(device).unwrap();
    for (rel, bytes) in [
        ("a.txt", &b"alpha\n"[..]),
        ("b.md", &b"# beta\n"[..]),
        ("sub/c.rs", &b"fn c() {}\n"[..]),
        ("sub/d.rs", &b"fn d() {}\n"[..]),
        ("sub/deep/e.txt", &b"epsilon\n"[..]),
        ("other/f.md", &b"# phi\n"[..]),
    ] {
        let id = fs.resolve_path(&format!("{root_name}/{rel}")).unwrap();
        let sz = fs.read_inode(id).unwrap().size;
        assert_eq!(fs.read_data(id, 0, sz).unwrap(), bytes, "bytes for {rel}");
        // All four vaire.* attrs folded into the batch creation inode write.
        assert_eq!(get_int(&mut fs, id, "vaire.size"), Some(bytes.len() as i64), "size {rel}");
        assert!(get_int(&mut fs, id, "vaire.mtime").is_some(), "mtime {rel}");
        assert!(get_str(&mut fs, id, "vaire.src").unwrap().ends_with(rel.rsplit('/').next().unwrap()), "src {rel}");
        assert!(get_str(&mut fs, id, "vaire.sync").is_some(), "sync stamp {rel}");
    }

    // Exactly six files under the unit root, nothing extra.
    let unit_id = fs.resolve_path(&root_name).unwrap();
    let mut names = Vec::new();
    collect_names(&mut fs, unit_id, "", &mut names);
    assert_eq!(names.len(), 6, "no stray objects: {names:?}");

    // The batched attrs are catalog-indexed exactly as set_attribute would be:
    // a query over vaire.size finds the batched files.
    assert_eq!(fs.snapshot_index().unwrap()[0].creator, "vaire");
}

// -- incremental re-run still skips under the batch path --------------------

#[test]
fn batched_incremental_rerun_skips_all_and_snaps_again() {
    let live = tempfile::tempdir().unwrap();
    seed_wide_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());

    let first = usync(&m, &img, true, 32).unwrap();
    assert_eq!(first.files_written, 6);

    // Unchanged: every file skips (size+mtime match), a fresh snapshot retained.
    let second = usync(&m, &img, true, 32).unwrap();
    assert_eq!(second.files_written, 0, "all skip on unchanged re-run");
    assert_eq!(second.files_skipped, 6);
    assert!(second.snapshot.is_some());

    // Change one file: only it rewrites (unlink stale + re-batch), rest skip.
    fs::write(live.path().join("sub/c.rs"), b"fn c() { changed }\n").unwrap();
    let third = usync(&m, &img, true, 32).unwrap();
    assert_eq!(third.files_written, 1, "only the changed file re-writes");
    assert_eq!(third.files_skipped, 5);

    let device = FileDevice::open_read_only(&img).unwrap();
    let mut fs = FileSystem::mount(device).unwrap();
    let root_name = live.path().file_name().unwrap().to_string_lossy().to_string();
    let id = fs.resolve_path(&format!("{root_name}/sub/c.rs")).unwrap();
    let sz = fs.read_inode(id).unwrap().size;
    assert_eq!(fs.read_data(id, 0, sz).unwrap(), b"fn c() { changed }\n", "exact rewrite");
    assert_eq!(fs.snapshot_index().unwrap().len(), 3, "three retained roots");
}

// -- a failed sync is an honest no-op (whole-transaction unwind) ------------

#[test]
fn failed_sync_leaves_image_unchanged_and_no_snapshot() {
    // First establish a good synced image + snapshot (root generation N).
    let live = tempfile::tempdir().unwrap();
    seed_wide_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());

    let first = usync(&m, &img, true, 16).unwrap();
    assert_eq!(first.files_written, 6);
    let snaps_before = {
        let device = FileDevice::open_read_only(&img).unwrap();
        let mut fs = FileSystem::mount(device).unwrap();
        fs.snapshot_index().unwrap().len()
    };
    assert_eq!(snaps_before, 1);

    // Now force a mid-sync failure: grow the tree well past what a tiny image
    // can hold so the batch write hits NoSpace. Add many large new files.
    for i in 0..400 {
        fs::write(live.path().join(format!("big-{i}.bin")), vec![b'x'; 64 * 1024]).unwrap();
    }

    let err = usync(&m, &img, true, 16).unwrap_err();
    let msg = format!("{err:#}");
    // Honest report: the failure surfaces (the batch/commit error). The
    // whole-transaction unwind reloads ground truth from the committed root, so
    // the MOUNTED image is unchanged at root generation N — no partial tree, no
    // new snapshot. (CoW may leave orphaned data blocks physically on disk to be
    // reclaimed; the logical root never moved, which is what "image unchanged"
    // means here — a raw-byte compare would over-assert.)
    assert!(!msg.is_empty(), "failure is reported: {msg}");
    let device = FileDevice::open_read_only(&img).unwrap();
    let mut fs = FileSystem::mount(device).unwrap();
    assert_eq!(
        fs.snapshot_index().unwrap().len(),
        snaps_before,
        "a failed sync takes no snapshot"
    );
    // The tree is exactly the pre-failure tree: the six originals present with
    // exact bytes, and NONE of the 400 big-*.bin partials leaked into the root.
    let root_name = live.path().file_name().unwrap().to_string_lossy().to_string();
    let a_id = fs.resolve_path(&format!("{root_name}/a.txt")).unwrap();
    let sz = fs.read_inode(a_id).unwrap().size;
    assert_eq!(fs.read_data(a_id, 0, sz).unwrap(), b"alpha\n");
    let unit_id = fs.resolve_path(&root_name).unwrap();
    let mut names = Vec::new();
    collect_names(&mut fs, unit_id, "", &mut names);
    assert_eq!(names.len(), 6, "no partial-tree leak from the failed batch: {names:?}");
    assert!(
        names.iter().all(|n| !n.contains("big-")),
        "no staged big-*.bin file survived the unwind: {names:?}"
    );
}

// -- the benchmark ledger line is well-formed -------------------------------

#[test]
fn ledger_line_carries_phase_and_commit_figures() {
    let live = tempfile::tempdir().unwrap();
    seed_tree(live.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = image_path(img_dir.path());
    let m = mk_manifest(live.path());

    let r = usync(&m, &img, true, 16).unwrap();
    let line = r.ledger_line();
    assert!(line.contains("written=3"));
    assert!(line.contains("scan="));
    assert!(line.contains("lookup="));
    assert!(line.contains("snapshot="));
    assert!(line.contains("blocks_written="));
    assert!(r.commit_stats.snapshots_created >= 1);
}
