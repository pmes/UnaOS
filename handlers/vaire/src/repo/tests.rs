// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BOLT-2 fixture suite. The RIDER carries over verbatim from Bolt-1/VAIRE-2:
// every test builds its own tempdir source repositories and its own tempdir
// bolt root. Nothing here reads `bolt.manifest.toml`, the real narino tree, or
// any shared image path.

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be available");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A source repository with one commit of `files`, hermetic (local identity).
fn seed_repo(dir: &Path, files: &[(&str, &str)]) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "loom@unaos.test"]);
    git(dir, &["config", "user.name", "Loom Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    for (name, body) in files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "seed"]);
}

fn mk_manifest(bolt_root: &Path, repos: &[(&str, &Path)]) -> RepoManifest {
    let mut toml = format!(
        "name = 'repo-bolt-test'\nbolt_root = '{}'\n",
        bolt_root.display()
    );
    for (name, src) in repos {
        toml.push_str(&format!(
            "\n[[repo]]\nname = '{name}'\nsource = '{}'\n",
            src.display()
        ));
    }
    RepoManifest::from_toml(&toml).expect("manifest parses")
}

// ---------------------------------------------------------------------------
// Manifest + the credential floor
// ---------------------------------------------------------------------------

#[test]
fn manifest_merges_the_credential_floor_even_when_omitted() {
    let m = RepoManifest::from_toml("name = 'b'\nbolt_root = '/tmp/unused-bolt'\n").unwrap();
    assert!(m.exclude.credentials.iter().any(|p| p == "*.pem"));
    assert!(m.exclude.credentials.iter().any(|p| p == ".netrc"));
    assert!(m.exclude.is_credential(Path::new("deploy/id_rsa.pub")));
    assert!(!m.exclude.is_credential(Path::new("src/main.rs")));
}

#[test]
fn manifest_rejects_a_path_shaped_repo_name() {
    let bad = RepoManifest::from_toml(
        "name = 'b'\nbolt_root = '/tmp/x'\n\n[[repo]]\nname = '../escape'\nsource = '/tmp/s'\n",
    );
    assert!(bad.is_err(), "a repo name must be a plain label");
}

// ---------------------------------------------------------------------------
// Dry-run is the default
// ---------------------------------------------------------------------------

#[test]
fn plan_writes_nothing_and_names_the_clone() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);

    let plan = plan_weave(&m).unwrap();

    assert!(!bolt_root.exists(), "a plan must not create the bolt root");
    assert_eq!(plan.repos.len(), 1);
    assert_eq!(plan.repos[0].action, WeaveAction::Clone);
    assert_eq!(plan.repos[0].mirror_refs, 0);
    assert_eq!(plan.repos[0].source_refs, 1, "one branch");
    assert_eq!(plan.repos[0].ledger_entries, 0);
    assert!(plan.render().contains("CLONE"));
}

#[test]
fn plan_names_an_unreadable_source_instead_of_failing() {
    let not_a_repo = tempfile::tempdir().unwrap();
    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(
        &bolt_dir.path().join("bolt"),
        &[("nope", not_a_repo.path())],
    );

    let plan = plan_weave(&m).unwrap();
    assert_eq!(plan.repos[0].action, WeaveAction::SourceUnreadable);
    assert!(plan.render().contains("SKIP"));
}

// ---------------------------------------------------------------------------
// Weave: the mirror, the ledger, and the source-never-written invariant
// ---------------------------------------------------------------------------

#[test]
fn weave_mirrors_and_writes_a_genesis_ledger_entry() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);

    let report = weave(&m).unwrap();

    assert_eq!(report.repos[0].action, WeaveAction::Clone);
    let mirror = bolt_root.join("mirrors/alpha.git");
    assert!(mirror.join("HEAD").exists(), "a bare mirror was created");
    assert!(bolt_root.join(BOLT_MARKER).exists(), "layout marker written");

    let entries = read_ledger(&bolt_root.join("ledger/alpha.ledger")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].prev, GENESIS, "the first entry is genesis-rooted");
    assert_eq!(entries[0].repo, "alpha");
    assert_eq!(entries[0].refs.len(), 1);
    assert!(entries[0].refs[0].name.ends_with("main"));
    assert_eq!(entries[0].hash.len(), 40, "SHA-1 chain hash");
    assert_eq!(entries[0].recomputed_hash(), entries[0].hash);
}

#[test]
fn weave_never_writes_the_source_repository() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    // Freeze the source read-only+execute: any write attempt would fail loudly.
    let before = fs::read(src.path().join("a.txt")).unwrap();
    let src_head = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(src.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let perms = fs::metadata(src.path()).unwrap().permissions().mode();
    fs::set_permissions(src.path(), fs::Permissions::from_mode(0o555)).unwrap();

    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(&bolt_dir.path().join("bolt"), &[("alpha", src.path())]);
    let report = weave(&m);

    fs::set_permissions(src.path(), fs::Permissions::from_mode(perms)).unwrap();
    report.expect("weave succeeds against a read-only source");

    assert_eq!(fs::read(src.path().join("a.txt")).unwrap(), before);
    let after_head = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(src.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(src_head, after_head, "the source HEAD never moved");
}

#[test]
fn second_weave_updates_and_chains_onto_the_first() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);

    let first = weave(&m).unwrap();
    let first_hash = first.repos[0].entry.as_ref().unwrap().hash.clone();

    // Move the source forward, then weave again.
    fs::write(src.path().join("b.txt"), "beta\n").unwrap();
    git(src.path(), &["add", "-A"]);
    git(src.path(), &["commit", "-q", "-m", "second"]);

    let second = weave(&m).unwrap();
    assert_eq!(second.repos[0].action, WeaveAction::Update);

    let entries = read_ledger(&bolt_root.join("ledger/alpha.ledger")).unwrap();
    assert_eq!(entries.len(), 2, "the ledger is append-only");
    assert_eq!(entries[1].prev, first_hash, "entry 2 chains onto entry 1");
    assert_ne!(
        entries[0].refs[0].oid, entries[1].refs[0].oid,
        "the recorded oid moved with the source"
    );
    assert!(verify(&m, false).unwrap()[0].is_intact());
}

// ---------------------------------------------------------------------------
// Verify: the integrity claims, each falsified by a real tamper
// ---------------------------------------------------------------------------

#[test]
fn verify_is_green_after_a_clean_weave() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(&bolt_dir.path().join("bolt"), &[("alpha", src.path())]);
    weave(&m).unwrap();

    let v = &verify(&m, true).unwrap()[0];
    assert!(v.is_intact(), "breaches: {:?}", v.breaches);
    assert_eq!(v.crystal(), CrystalColor::Green);
    assert_eq!(v.entries, 1);
    assert!(v.credentials.is_empty());
}

#[test]
fn an_edited_entry_is_caught_as_tampered() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();

    // Rewrite a recorded oid, leaving the hash line alone — the classic
    // "quietly fix up the history" edit.
    let path = bolt_root.join("ledger/alpha.ledger");
    let text = fs::read_to_string(&path).unwrap();
    let tampered = text.replace("ref 0", "ref 1").replacen(
        text.lines().find(|l| l.starts_with("ref ")).unwrap(),
        "ref 1111111111111111111111111111111111111111 refs/heads/main",
        1,
    );
    fs::write(&path, tampered).unwrap();

    let v = &verify(&m, false).unwrap()[0];
    assert!(!v.is_intact());
    assert_eq!(v.crystal(), CrystalColor::Red);
    assert!(
        v.breaches
            .iter()
            .any(|b| matches!(b, Breach::EntryTampered { .. })),
        "breaches: {:?}",
        v.breaches
    );
}

#[test]
fn a_dropped_entry_breaks_the_chain() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();
    fs::write(src.path().join("b.txt"), "beta\n").unwrap();
    git(src.path(), &["add", "-A"]);
    git(src.path(), &["commit", "-q", "-m", "second"]);
    weave(&m).unwrap();

    // Excise the FIRST entry; the second's `prev` now points at nothing.
    let path = bolt_root.join("ledger/alpha.ledger");
    let text = fs::read_to_string(&path).unwrap();
    let (_first, rest) = text.split_once("\n\n").unwrap();
    fs::write(&path, rest).unwrap();

    let v = &verify(&m, false).unwrap()[0];
    assert_eq!(v.entries, 1, "one entry survives");
    assert!(
        v.breaches
            .iter()
            .any(|b| matches!(b, Breach::ChainBroken { .. })),
        "a dropped entry must break the chain; breaches: {:?}",
        v.breaches
    );
    assert_eq!(v.crystal(), CrystalColor::Red);
}

#[test]
fn a_mirror_that_moves_without_the_ledger_is_ref_drift() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();

    // Push a new ref straight into the mirror, behind the ledger's back.
    let mirror = bolt_root.join("mirrors/alpha.git");
    git(&mirror, &["update-ref", "refs/heads/smuggled", "refs/heads/main"]);

    let v = &verify(&m, false).unwrap()[0];
    assert!(
        v.breaches.iter().any(|b| matches!(
            b,
            Breach::RefDrift { reference, .. } if reference.ends_with("smuggled")
        )),
        "breaches: {:?}",
        v.breaches
    );
    assert_eq!(v.crystal(), CrystalColor::Red);
}

#[test]
fn a_missing_mirror_is_a_breach_not_a_panic() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();
    fs::remove_dir_all(bolt_root.join("mirrors/alpha.git")).unwrap();

    let v = &verify(&m, false).unwrap()[0];
    assert!(v.breaches.contains(&Breach::MirrorMissing));
    assert_eq!(v.crystal(), CrystalColor::Red);
}

#[test]
fn a_never_woven_bolt_reports_ledger_empty_not_an_error() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(&bolt_dir.path().join("bolt"), &[("alpha", src.path())]);

    let v = &verify(&m, false).unwrap()[0];
    assert!(v.breaches.contains(&Breach::LedgerEmpty));
    assert!(v.breaches.contains(&Breach::MirrorMissing));
}

#[test]
fn a_truncated_ledger_is_rejected_by_the_parser() {
    let err = parse_ledger("entry 20260811T000000Z\nrepo alpha\nprev 0\n").unwrap_err();
    assert!(
        format!("{err}").contains("truncated"),
        "a half-written entry must be named: {err}"
    );
}

// ---------------------------------------------------------------------------
// The credential floor: audit, report, never silent
// ---------------------------------------------------------------------------

#[test]
fn a_committed_credential_is_found_recorded_and_turns_the_bolt_amber() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(
        src.path(),
        &[
            ("src/main.rs", "fn main() {}\n"),
            ("deploy/prod.pem", "-----BEGIN NOT A REAL KEY-----\n"),
            (".env", "TOKEN=nope\n"),
        ],
    );
    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(&bolt_dir.path().join("bolt"), &[("alpha", src.path())]);

    let report = weave(&m).unwrap();
    assert_eq!(report.credential_findings(), 2, "the .pem and the .env");

    let entry = report.repos[0].entry.as_ref().unwrap();
    let paths: Vec<&str> = entry.credentials.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"deploy/prod.pem"), "paths: {paths:?}");
    assert!(paths.contains(&".env"), "paths: {paths:?}");
    assert!(!paths.contains(&"src/main.rs"));

    // Recorded IN the ledger — the finding survives the process that made it.
    let v = &verify(&m, false).unwrap()[0];
    assert!(v.is_intact(), "findings are not corruption");
    assert_eq!(v.credentials.len(), 2);
    assert_eq!(
        v.crystal(),
        CrystalColor::Amber,
        "an intact bolt carrying unresolved findings is Amber"
    );
}

#[test]
fn a_credential_finding_is_covered_by_the_chain_hash() {
    // Silently deleting a `cred` line must not produce a chain that verifies.
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.pem", "key\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();

    let path = bolt_root.join("ledger/alpha.ledger");
    let text = fs::read_to_string(&path).unwrap();
    let scrubbed: String = text
        .lines()
        .filter(|l| !l.starts_with("cred "))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_ne!(scrubbed, text, "there was a finding to scrub");
    fs::write(&path, scrubbed).unwrap();

    let v = &verify(&m, false).unwrap()[0];
    assert!(
        v.breaches
            .iter()
            .any(|b| matches!(b, Breach::EntryTampered { .. })),
        "scrubbing a finding must break the entry hash; breaches: {:?}",
        v.breaches
    );
}

// ---------------------------------------------------------------------------
// STATUS
// ---------------------------------------------------------------------------

#[test]
fn status_is_amber_when_the_source_has_moved_since_the_weave() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(&bolt_dir.path().join("bolt"), &[("alpha", src.path())]);
    weave(&m).unwrap();

    assert_eq!(status(&m).unwrap()[0].crystal, CrystalColor::Green);

    fs::write(src.path().join("b.txt"), "beta\n").unwrap();
    git(src.path(), &["add", "-A"]);
    git(src.path(), &["commit", "-q", "-m", "moved"]);

    let row = &status(&m).unwrap()[0];
    assert!(row.source_ahead, "the source moved past the ledger");
    assert_eq!(row.crystal, CrystalColor::Amber);
    assert_eq!(row.entries, 1);
    assert!(row.last_stamp.is_some());
}

// ---------------------------------------------------------------------------
// The UnaFS bridge — the mapping is executable, not a promise
// ---------------------------------------------------------------------------

#[test]
fn unafs_view_points_usync_at_the_mirrors_and_keeps_the_floor() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();

    let dm = unafs_view(&m).unwrap();
    assert_eq!(dm.penumbra, vec![bolt_root.join("mirrors")]);
    assert!(
        dm.exclude.credentials.iter().any(|p| p == "*.pem"),
        "the floor rides the projection"
    );
}

#[test]
fn a_repo_bolt_weaves_into_a_unafs_image_as_native_objects() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();

    let img_dir = tempfile::tempdir().unwrap();
    let img = img_dir.path().join("repo-bolt.img");
    let dm = unafs_view(&m).unwrap();

    // Dry-run first: the default writes nothing at all.
    let dry = crate::usync::usync(&dm, &img, false, 16).unwrap();
    assert!(!img.exists(), "the dry-run must not create the image");
    assert!(dry.files_written > 0, "the mirror has objects to weave");

    // Apply: native objects + exactly one retained root for the run.
    let applied = crate::usync::usync(&dm, &img, true, 32).unwrap();
    assert!(applied.applied && applied.formatted);
    assert_eq!(applied.files_written, dry.files_written);
    assert!(applied.snapshot.is_some(), "one snapshot per completed sync");

    // Read it back through the read-only status path: the mirror is really in
    // the image, and the unit root carries the vaire.* typed attrs.
    let st = crate::usync::ustatus(&dm, &img).unwrap();
    assert_eq!(st.units.len(), 1);
    assert_eq!(st.units[0].name, "mirrors");
    assert!(st.units[0].files > 0);
    assert_eq!(st.snapshots.len(), 1);
    assert_eq!(st.snapshots[0].creator, "vaire");
    assert_eq!(st.live_files, applied.files_written);
}

#[test]
fn readiness_passes_a_small_bolt_and_names_the_widest_directory() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(&bolt_dir.path().join("bolt"), &[("alpha", src.path())]);
    weave(&m).unwrap();

    let r = unafs_readiness(&m).unwrap();
    assert!(r.fits(), "blockers: {:?}", r.blockers);
    assert!(r.files > 0 && r.total_bytes > 0);
    assert!(r.oversize.is_empty());
    assert!(r.widest_dir.is_some());
}

#[test]
fn readiness_blocks_a_file_over_the_measured_inode_ceiling() {
    // The study's hard wall: an Inode (extent list included) must serialize
    // inside one 4 KiB block, so a large packfile fails with
    // InodeTooLarge(4100, 4096) partway through a weave. Catch it up front.
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let bolt_root = bolt_dir.path().join("bolt");
    let m = mk_manifest(&bolt_root, &[("alpha", src.path())]);
    weave(&m).unwrap();

    // A sparse file of the ceiling size stands in for a repacked monorepo's
    // single packfile — no real bytes are written, only the length.
    let fat = bolt_root.join("mirrors/alpha.git/objects/pack/pack-huge.pack");
    fs::create_dir_all(fat.parent().unwrap()).unwrap();
    let f = fs::File::create(&fat).unwrap();
    f.set_len(UNAFS_MAX_FILE_BYTES + 1).unwrap();
    drop(f);

    let r = unafs_readiness(&m).unwrap();
    assert!(!r.fits(), "an over-ceiling file must block a native weave");
    assert_eq!(r.oversize.len(), 1);
    assert!(
        r.blockers[0].contains("single-file ceiling"),
        "blocker must name the limit: {:?}",
        r.blockers
    );
}

#[test]
fn the_layout_mapping_names_every_artifact_kind() {
    let src = tempfile::tempdir().unwrap();
    seed_repo(src.path(), &[("a.txt", "alpha\n")]);
    let bolt_dir = tempfile::tempdir().unwrap();
    let m = mk_manifest(&bolt_dir.path().join("bolt"), &[("alpha", src.path())]);

    let rows = unafs_layout(&m);
    let hosts: Vec<&str> = rows.iter().map(|r| r.host.as_str()).collect();
    assert!(hosts.iter().any(|h| h.contains("mirrors/alpha.git")));
    assert!(hosts.iter().any(|h| h.contains("ledger/alpha.ledger")));
    assert!(hosts.iter().any(|h| h.contains(BOLT_MARKER)));
    assert!(hosts.contains(&"one weave"));
    // Every claimed native form names a real UnaFS primitive.
    for r in &rows {
        assert!(
            ["create_files_batch", "attr", "snapshot_create", "directory"]
                .iter()
                .any(|k| r.native.contains(k)),
            "unmapped row: {r:?}"
        );
    }
}
