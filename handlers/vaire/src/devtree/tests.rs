// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BOLT-1 fixture suite. RIDER 1: every test runs against tempdir fixtures —
// never `/Volumes/40G`, never the real narino tree.

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

/// Hermetic git repo fixture: one commit of `file`->`contents`, local identity,
/// no global config, no shared state.
fn init_repo(dir: &Path, file: &str, contents: &str) {
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git available");
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "loom@unaos.test"]);
    run(&["config", "user.name", "Loom Test"]);
    run(&["config", "commit.gpgsign", "false"]);
    fs::write(dir.join(file), contents).expect("write");
    run(&["add", file]);
    run(&["commit", "-q", "-m", "seed"]);
}

/// Build a manifest directly (fields are pub) with the credential floor merged
/// in, exactly as `from_toml` would.
fn mk_manifest(
    live_root: &Path,
    target_root: &Path,
    repos: Vec<RepoUnit>,
    penumbra: Vec<PathBuf>,
) -> DevManifest {
    DevManifest {
        name: "test-devtree".into(),
        live: LiveDrive::Narino,
        live_root: live_root.to_path_buf(),
        target_root: target_root.to_path_buf(),
        repos,
        penumbra,
        exclude: ExcludeRules::defaults().with_credential_floor(),
    }
}

// -- manifest parse ---------------------------------------------------------

#[test]
fn manifest_parses_and_enforces_narino_and_credential_floor() {
    let text = r#"
name = "unaos-devtree"
live = "narino"
live_root = "/x/live"
target_root = "/x/40g"
penumbra = ["/x/plans"]

[[repo]]
name = "unaos"
path = "/x/live"

[exclude]
junk = ["target"]
credentials = ["*.pem"]
"#;
    let m = DevManifest::from_toml(text).expect("parse");
    assert_eq!(m.live, LiveDrive::Narino);
    assert_eq!(m.repos.len(), 1);
    // Rider 2 floor merged even though the manifest only listed *.pem.
    for p in ["*.key", "*token*", ".netrc", "*.keychain"] {
        assert!(m.exclude.credentials.iter().any(|c| c == p), "missing {p}");
    }
}

#[test]
fn manifest_rejects_non_narino_live() {
    let text = r#"
name = "x"
live = "40g"
live_root = "/x/live"
target_root = "/x/40g"
"#;
    assert!(DevManifest::from_toml(text).is_err());
}

// -- exclusion classification (Rider 2) -------------------------------------

#[test]
fn exclusions_classify_credentials_distinctly_from_junk() {
    let r = ExcludeRules::defaults().with_credential_floor();
    assert_eq!(r.classify(Path::new("id_rsa")), Some(ExcludeClass::Credential));
    assert_eq!(r.classify(Path::new("server.pem")), Some(ExcludeClass::Credential));
    assert_eq!(r.classify(Path::new("api.token")), Some(ExcludeClass::Credential));
    assert_eq!(r.classify(Path::new(".env")), Some(ExcludeClass::Credential));
    assert_eq!(r.classify(Path::new(".env.local")), Some(ExcludeClass::Credential));
    assert_eq!(r.classify(Path::new("deploy.key")), Some(ExcludeClass::Credential));
    assert_eq!(r.classify(Path::new("my.keychain")), Some(ExcludeClass::Credential));
    assert_eq!(r.classify(Path::new(".netrc")), Some(ExcludeClass::Credential));
    // junk
    assert_eq!(r.classify(Path::new("target/foo")), Some(ExcludeClass::Junk));
    assert_eq!(r.classify(Path::new("a/b/.DS_Store")), Some(ExcludeClass::Junk));
    assert_eq!(r.classify(Path::new("kernel.elf")), Some(ExcludeClass::Junk));
    // allowed
    assert_eq!(r.classify(Path::new("notes.md")), None);
    assert_eq!(r.classify(Path::new("plans/unaos/x.md")), None);
}

// -- STATUS -----------------------------------------------------------------

#[test]
fn status_missing_volume_is_red() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let target = live.path().join("no-such-volume");
    let m = mk_manifest(
        live.path(),
        &target,
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![],
    );
    let s = status(&m).unwrap();
    assert!(!s.target_present);
    assert_eq!(s.crystal, CrystalColor::Red);
    assert_eq!(s.repos[0].mirror_coherent, None);
}

#[test]
fn status_coherent_after_apply_is_green() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("note.md"), "hi\n").unwrap();
    let target = tempfile::tempdir().unwrap();

    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );
    apply_sync(&m).unwrap();

    let s = status(&m).unwrap();
    assert!(s.target_present);
    assert_eq!(s.repos[0].mirror_coherent, Some(true));
    assert!(s.penumbra.iter().all(|p| p.delta == PenumbraDelta::Same));
    assert_eq!(s.crystal, CrystalColor::Green, "status: {s:#?}");
}

#[test]
fn status_dirty_repo_is_amber() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let target = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![],
    );
    apply_sync(&m).unwrap();
    // Dirty a tracked file AFTER the mirror is coherent.
    fs::write(live.path().join("a.txt"), "y\n").unwrap();
    let s = status(&m).unwrap();
    assert!(s.repos[0].dirty_files >= 1);
    assert_eq!(s.crystal, CrystalColor::Amber);
}

#[test]
fn status_penumbra_drift_is_amber() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("note.md"), "one\n").unwrap();
    let target = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );
    apply_sync(&m).unwrap();
    // Change the penumbra file on the live side -> drift.
    fs::write(pen.path().join("note.md"), "two\n").unwrap();
    let s = status(&m).unwrap();
    assert!(s.penumbra.iter().any(|p| p.delta == PenumbraDelta::Differs));
    assert_eq!(s.crystal, CrystalColor::Amber);
}

#[test]
fn status_reports_worktree_pointer_flagged() {
    // Simulate a worktree: a repo whose `.git` is a pointer FILE, not a dir.
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    // Replace .git dir with a pointer file (fixture only; never done to real trees).
    let wt = tempfile::tempdir().unwrap();
    fs::create_dir_all(wt.path().join("proj")).unwrap();
    fs::write(
        wt.path().join("proj").join(".git"),
        "gitdir: /abs/path/.git/worktrees/proj\n",
    )
    .unwrap();
    let target = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "proj".into(), path: wt.path().join("proj"), worktree: true }],
        vec![],
    );
    let s = status(&m).unwrap();
    assert_eq!(s.worktree_pointers.len(), 1);
    assert!(s.worktree_pointers[0].is_worktree_pointer);
    assert_eq!(
        s.worktree_pointers[0].worktree_gitdir.as_deref(),
        Some("/abs/path/.git/worktrees/proj")
    );
}

// -- SNAP -------------------------------------------------------------------

#[test]
fn snap_refuses_when_target_absent() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let m = mk_manifest(
        live.path(),
        &live.path().join("absent-vol"),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![],
    );
    let e = snap(&m).unwrap_err();
    assert!(e.to_string().contains("SNAP refused"), "{e}");
}

#[test]
fn snap_layout_bundles_and_penumbra_under_stamp() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("keep.md"), "hi\n").unwrap();
    fs::write(pen.path().join("id_rsa"), "SECRET\n").unwrap(); // must be excluded
    let target = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "unaos".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );

    let rep = snap_with_stamp(&m, &m.target(), "20260715T120000Z").unwrap();
    assert_eq!(rep.stamp, "20260715T120000Z");
    let base = target.path().join(".vaire-snaps").join("20260715T120000Z");
    assert!(base.join("git").join("unaos.bundle").exists(), "bundle missing");
    let pen_root = pen.path().file_name().unwrap().to_str().unwrap();
    assert!(base.join("penumbra").join(pen_root).join("keep.md").exists());
    // Rider 2: the credential file is NOT captured, and IS reported.
    assert!(!base.join("penumbra").join(pen_root).join("id_rsa").exists());
    assert!(rep.excluded.iter().any(|e| e.rel == Path::new("id_rsa")
        && e.class == ExcludeClass::Credential));
    assert_eq!(rep.penumbra_files, 1);
}

// -- SYNC dry-run -----------------------------------------------------------

#[test]
fn sync_dry_run_plan_is_exact() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("a.md"), "a\n").unwrap();
    fs::write(pen.path().join("b.md"), "b\n").unwrap();
    fs::write(pen.path().join(".env"), "SECRET=1\n").unwrap();
    fs::create_dir_all(pen.path().join("target")).unwrap();
    fs::write(pen.path().join("target").join("junk.o"), "o\n").unwrap();
    let target = tempfile::tempdir().unwrap(); // present but empty -> all New

    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "unaos".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );

    let plan = plan_sync(&m).unwrap();
    let root = pen.path().file_name().unwrap().to_str().unwrap();
    let expect = vec![
        PlannedAction::MirrorRepo { name: "unaos".into(), from: live.path().into() },
        // penumbra entries sorted by rel: .env, a.md, b.md, target
        PlannedAction::SkipExcluded { rel: PathBuf::from(root).join(".env"), class: ExcludeClass::Credential },
        PlannedAction::CopyPenumbra { rel: PathBuf::from(root).join("a.md"), reason: CopyReason::New },
        PlannedAction::CopyPenumbra { rel: PathBuf::from(root).join("b.md"), reason: CopyReason::New },
        // `target` dir excluded as junk (subtree pruned: junk.o never appears)
        PlannedAction::SkipExcluded { rel: PathBuf::from(root).join("target"), class: ExcludeClass::Junk },
    ];
    assert_eq!(plan.actions, expect, "plan render:\n{}", plan.render());
}

#[test]
fn sync_dry_run_writes_nothing() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("a.md"), "a\n").unwrap();
    let target = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );
    let _ = plan_sync(&m).unwrap();
    // Target must remain empty after a dry-run.
    assert!(fs::read_dir(target.path()).unwrap().next().is_none());
}

// -- SYNC apply -------------------------------------------------------------

#[test]
fn sync_apply_snaps_first_then_mirrors_and_copies() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("note.md"), "hi\n").unwrap();
    fs::write(pen.path().join("secret.pem"), "KEY\n").unwrap();
    let target = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "unaos".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );

    let (snap_rep, _plan) = apply_sync(&m).unwrap();

    // SNAP happened first (bundle exists under a stamp).
    assert!(snap_rep.dir.exists());
    assert!(snap_rep.dir.join("git").join("unaos.bundle").exists());

    // Mirror exists and holds HEAD.
    let mirror = target.path().join("git-mirror").join("unaos.git");
    assert!(mirror.exists());
    let head = git_out(live.path(), &["rev-parse", "HEAD"]).unwrap();
    assert!(git_out(&mirror, &["cat-file", "-e", &format!("{head}^{{commit}}")]).is_ok());

    // Penumbra copied (note.md), credential skipped (secret.pem).
    let root = pen.path().file_name().unwrap().to_str().unwrap();
    assert!(target.path().join("penumbra").join(root).join("note.md").exists());
    assert!(!target.path().join("penumbra").join(root).join("secret.pem").exists());
    // last-weave stamped.
    assert!(target.path().join(".vaire-last-weave").exists());
}

#[test]
fn sync_apply_refuses_when_target_absent() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let m = mk_manifest(
        live.path(),
        &live.path().join("absent"),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![],
    );
    assert!(apply_sync(&m).unwrap_err().to_string().contains("refused"));
}

// -- narino-never-written invariant -----------------------------------------

#[test]
fn apply_does_not_write_the_live_side() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("note.md"), "hi\n").unwrap();
    let target = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );

    // Behavioural proof: snapshot the live working files before/after apply.
    let before = snapshot_tree(live.path());
    let pen_before = snapshot_tree(pen.path());

    // fs-permission proof: make the penumbra root read-only during the apply.
    // The penumbra copy must succeed reading it (proving read-only access).
    set_readonly(pen.path(), true);
    let res = apply_sync(&m);
    set_readonly(pen.path(), false); // restore so tempdir cleanup works
    res.expect("apply must succeed with a read-only source penumbra");

    assert_eq!(before, snapshot_tree(live.path()), "live repo files changed");
    assert_eq!(pen_before, snapshot_tree(pen.path()), "penumbra files changed");
    // And the copy really landed on the target.
    let root = pen.path().file_name().unwrap().to_str().unwrap();
    assert!(target.path().join("penumbra").join(root).join("note.md").exists());
}

/// Recursively hash (path -> contents) of a tree, skipping `.git` internals
/// (git may touch ephemeral lock files; the invariant concerns tracked/working
/// content, and this proves the working files are byte-identical).
fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn go(root: &Path, cur: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let dir = root.join(cur);
        let mut es: Vec<_> = fs::read_dir(&dir).map(|r| r.filter_map(|e| e.ok()).collect()).unwrap_or_default();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let name = e.file_name();
            if name == ".git" {
                continue;
            }
            let rel = cur.join(&name);
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                go(root, &rel, out);
            } else {
                out.push((rel, fs::read(root.join(cur).join(&name)).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    go(root, Path::new(""), &mut out);
    out
}

fn set_readonly(root: &Path, ro: bool) {
    fn go(p: &Path, ro: bool) {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.filter_map(|e| e.ok()) {
                let path = e.path();
                if path.is_dir() {
                    go(&path, ro);
                }
                let mode = if ro { 0o555 } else { 0o755 };
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
            }
        }
        let mode = if ro { 0o555 } else { 0o755 };
        let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode));
    }
    go(root, ro);
}

// -- symlinks: skipped but NEVER silently (LC-orin lens should-fix) ----------

#[test]
fn symlinks_are_reported_never_followed_never_silent() {
    let live = tempfile::tempdir().unwrap();
    init_repo(live.path(), "a.txt", "x\n");
    let pen = tempfile::tempdir().unwrap();
    fs::write(pen.path().join("keep.md"), "hi\n").unwrap();
    // An external file a symlink points at — must never be copied.
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("external.md"), "EXTERNAL\n").unwrap();
    std::os::unix::fs::symlink(outside.path().join("external.md"), pen.path().join("link.md"))
        .unwrap();
    // A directory symlink too (Peter's plans tree has legacy unaos-* dir links).
    std::os::unix::fs::symlink(outside.path(), pen.path().join("legacy-dir")).unwrap();
    let target = tempfile::tempdir().unwrap();

    let m = mk_manifest(
        live.path(),
        target.path(),
        vec![RepoUnit { name: "r".into(), path: live.path().into(), worktree: false }],
        vec![pen.path().into()],
    );

    // STATUS reports both symlinks, distinctly classed.
    let s = status(&m).unwrap();
    let links: Vec<_> = s
        .excluded
        .iter()
        .filter(|e| e.class == ExcludeClass::Symlink)
        .map(|e| e.rel.clone())
        .collect();
    assert!(links.contains(&PathBuf::from("link.md")), "excluded: {:?}", s.excluded);
    assert!(links.contains(&PathBuf::from("legacy-dir")));

    // Dry-run plan carries the skips.
    let plan = plan_sync(&m).unwrap();
    let root = pen.path().file_name().unwrap().to_str().unwrap();
    assert!(plan.actions.contains(&PlannedAction::SkipExcluded {
        rel: PathBuf::from(root).join("link.md"),
        class: ExcludeClass::Symlink,
    }));
    assert!(plan.render().contains("symlink (not followed)"));

    // Apply: symlink targets are NOT copied (never followed), skip reported in SNAP.
    let (snap_rep, _) = apply_sync(&m).unwrap();
    assert!(snap_rep.excluded.iter().any(|e| e.class == ExcludeClass::Symlink));
    assert!(target.path().join("penumbra").join(root).join("keep.md").exists());
    assert!(!target.path().join("penumbra").join(root).join("link.md").exists());
    assert!(!target.path().join("penumbra").join(root).join("legacy-dir").exists());
    // Nothing from the external tree leaked anywhere onto the target.
    fn find_named(p: &Path, name: &str, hits: &mut Vec<PathBuf>) {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.filter_map(|e| e.ok()) {
                let path = e.path();
                if path.file_name().and_then(|s| s.to_str()) == Some(name) {
                    hits.push(path.clone());
                }
                if path.is_dir() {
                    find_named(&path, name, hits);
                }
            }
        }
    }
    let mut hits = Vec::new();
    find_named(target.path(), "external.md", &mut hits);
    assert!(hits.is_empty(), "external content leaked: {hits:?}");
}

// -- utc stamp --------------------------------------------------------------

#[test]
fn utc_stamp_is_correct_and_formatted() {
    // 2026-07-15T12:00:00Z == 1784116800
    let t = UNIX_EPOCH + Duration::from_secs(1_784_116_800);
    assert_eq!(utc_stamp(t), "20260715T120000Z");
    // epoch itself
    assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
}
