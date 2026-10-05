// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AMBER1 (SR34) END TO END — the `amber` binary on image files: plan → apply (signed) → verify →
//! recover, the bus wire over `amber bus`, and the refusals. ORACLES (each skips when absent):
//! util-linux `partx` (the table), dosfstools `fsck.fat` (the FAT32 volume), mtools `minfo` (the
//! FAT32 volume read in place inside the image), and the card `tools/una-card` writes (the table is
//! the golden card plan's — `amber_core::kat::golden_card_plan` — pinned byte for byte here AND
//! against una-card's output in `tools/una-card/tests/card_table.rs`). UnaFS has no third-party
//! reader: its oracle is the unafs crate's own mount + fsck, run on the bytes as written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CARD: &str = "amber-layout v1\ndisk-seed UNAOS-X86-CARD\n\
    part esp 262144s UNAOS-ESP seed=UNAOS-X86-ESP format=fat32 label=UNAOS\n\
    part unafs 131072s UNAOS-UNAFS seed=UNAOS-X86-UFS format=unafs\n";

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("amber_cli").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn amber(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_amber")).args(args).env_remove("AMBER_ALLOW").output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn tool(name: &str) -> Option<String> {
    ["/usr/sbin", "/sbin", "/usr/bin", "/bin"].iter().map(|d| format!("{d}/{name}")).find(|p| Path::new(p).exists()).or_else(|| {
        eprintln!("SKIP: oracle `{name}` not installed");
        None
    })
}

fn sha_line(out: &str) -> String {
    out.lines().find_map(|l| l.strip_prefix("sha256 ")).expect("plan prints `sha256 <hex>`").trim().to_string()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// plan → apply --create → the table is the golden card's; verify passes with both filesystems
/// probed; partx / fsck.fat / minfo read it.
#[test]
fn card_end_to_end() {
    let d = dir("card");
    let (layout, img) = (d.join("card.layout"), d.join("card.img"));
    fs::write(&layout, CARD).unwrap();

    let (rc, out, err) = amber(&["plan", s(&img), "--layout", s(&layout)]);
    assert_eq!(rc, 0, "{err}");
    assert!(!img.exists(), "plan must not create anything");
    let sha = sha_line(&out);
    assert!(out.contains("formats: 2") && out.contains("p1 fat32") && out.contains("p2 unafs"), "{out}");

    // A wrong signature is refused and creates nothing it can write to... (the image is created
    // first by --create, then the apply is refused: it must stay all zero).
    let (rc, _, err) = amber(&["apply", s(&img), "--layout", s(&layout), "--sha256", &"0".repeat(64), "--create"]);
    assert_eq!(rc, 2);
    assert!(err.contains("does not match"), "{err}");
    let zero = fs::read(&img).unwrap();
    assert!(zero.iter().all(|&b| b == 0), "a refused apply wrote");
    fs::remove_file(&img).unwrap();

    let (rc, out, err) = amber(&["apply", s(&img), "--layout", s(&layout), "--sha256", &sha, "--create"]);
    assert_eq!(rc, 0, "{out}{err}");
    assert!(out.contains("p1 fat32") && out.contains("read back ok") && out.contains("fsck clean"), "{out}");

    let bytes = fs::read(&img).unwrap();
    let plan = amber_core::kat::golden_card_plan();
    assert_eq!(bytes.len() as u64, plan.disk_sectors * 512);
    for w in amber_core::plan_apply::writes(&plan).unwrap() {
        let o = w.lba as usize * 512;
        assert_eq!(&bytes[o..o + w.data.len()], &w.data[..], "{} differs from the golden card plan", w.what);
    }
    assert_eq!(amber_core::crc32(&bytes[..512]), amber_core::kat::CARD_MBR_CRC);

    let (rc, out, _) = amber(&["verify", s(&img)]);
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("verify: PASS"), "{out}");
    assert!(out.contains("fat32 label \"UNAOS\""), "{out}");
    assert!(out.contains("unafs v") && out.contains("fsck clean"), "{out}");

    // Re-planning the written image gives the same signature (the plan is a function of layout+size).
    let (_, out2, _) = amber(&["plan", s(&img), "--layout", s(&layout)]);
    assert_eq!(sha_line(&out2), sha);

    if let Some(partx) = tool("partx") {
        let o = Command::new(partx).args(["-g", "-r", "-o", "NR,START,END,NAME", s(&img)]).output().unwrap();
        let t = String::from_utf8_lossy(&o.stdout);
        eprintln!("partx: {t}");
        assert_eq!(t.trim(), "1 2048 264191 UNAOS-ESP\n2 264192 395263 UNAOS-UNAFS");
    }
    let esp = &bytes[2048 * 512..264192 * 512];
    if let Some(fsck) = tool("fsck.fat") {
        let p1 = d.join("p1.img");
        fs::write(&p1, esp).unwrap();
        let o = Command::new(fsck).args(["-n", "-v", s(&p1)]).output().unwrap();
        let t = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        eprintln!("fsck.fat: {t}");
        assert!(o.status.success(), "fsck.fat rejects p1: {t}");
        assert!(!t.contains("Free cluster summary wrong") && !t.contains("differ"), "{t}");
    }
    if let Some(minfo) = tool("minfo") {
        let o = Command::new(minfo).env("MTOOLS_SKIP_CHECK", "1").args(["-i", &format!("{}@@{}", s(&img), 2048 * 512), "::"]).output().unwrap();
        let t = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "minfo: {}", String::from_utf8_lossy(&o.stderr));
        assert!(t.contains("UNAOS") && t.contains("FAT32"), "{t}");
    }
    let _ = fs::remove_dir_all(&d);
}

/// The `--part` CLI form, `--dry-run` (checks the signature, writes nothing), and a plan for one
/// size refused on another.
#[test]
fn dry_run_and_size_bound_signature() {
    let d = dir("dry");
    let img = d.join("disk.img");
    fs::File::create(&img).unwrap().set_len(64 << 20).unwrap();
    let parts = ["--part", "esp:40M:BOOT:fat32", "--part", "data:rest:REST", "--seed", "T1"];
    let (rc, out, err) = amber(&[&["plan", s(&img)][..], &parts].concat());
    assert_eq!(rc, 0, "{err}");
    let sha = sha_line(&out);
    let (rc, out, err) = amber(&[&["apply", s(&img), "--sha256", &sha, "--dry-run"][..], &parts].concat());
    assert_eq!(rc, 0, "{err}");
    assert!(out.contains("dry run"), "{out}");
    assert!(fs::read(&img).unwrap().iter().all(|&b| b == 0), "dry run wrote");
    // Grow the medium: the same layout is a different plan, and the old signature is refused.
    fs::OpenOptions::new().write(true).open(&img).unwrap().set_len(65 << 20).unwrap();
    let (rc, _, err) = amber(&[&["apply", s(&img), "--sha256", &sha][..], &parts].concat());
    assert_eq!(rc, 2, "{err}");
    assert!(err.contains("does not match"));
    let _ = fs::remove_dir_all(&d);
}

fn applied_card(d: &Path) -> (PathBuf, Vec<u8>) {
    let (layout, img) = (d.join("card.layout"), d.join("card.img"));
    fs::write(&layout, CARD).unwrap();
    let (_, out, _) = amber(&["plan", s(&img), "--layout", s(&layout)]);
    let (rc, out2, err) = amber(&["apply", s(&img), "--layout", s(&layout), "--sha256", &sha_line(&out), "--create"]);
    assert_eq!(rc, 0, "{out2}{err}");
    let b = fs::read(&img).unwrap();
    (img, b)
}

/// Lose the primary header, then the backup, then grow the disk: `recover` (dry run first, which
/// writes nothing) restores the table, byte-identical to the original where the original stands.
#[test]
fn recover_end_to_end() {
    let d = dir("recover");
    let (img, orig) = applied_card(&d);
    let total = orig.len() / 512;

    // 1. Primary header zeroed.
    let mut b = orig.clone();
    b[512..1024].fill(0);
    fs::write(&img, &b).unwrap();
    let (rc, out, _) = amber(&["verify", s(&img)]);
    assert_eq!(rc, 1, "verify must fail a lost primary: {out}");
    let (rc, out, _) = amber(&["recover", s(&img)]);
    assert_eq!(rc, 0);
    assert!(out.contains("primary-from-backup") && out.contains("dry run"), "{out}");
    assert_eq!(fs::read(&img).unwrap(), b, "recover dry run wrote");
    let (rc, out, err) = amber(&["recover", s(&img), "--write"]);
    assert_eq!(rc, 0, "{out}{err}");
    assert_eq!(fs::read(&img).unwrap(), orig, "the restored primary is not the original");

    // 2. Backup header and array zeroed.
    let mut b = orig.clone();
    b[(total - 33) * 512..].fill(0);
    fs::write(&img, &b).unwrap();
    assert_eq!(amber(&["verify", s(&img)]).0, 1);
    let (rc, out, _) = amber(&["recover", s(&img), "--write"]);
    assert!(rc == 0 && out.contains("backup-from-primary"), "{out}");
    assert_eq!(fs::read(&img).unwrap(), orig, "the restored backup is not the original");

    // 3. Grown by 1 MiB: the backup is relocated to the new end and verify passes.
    let f = fs::OpenOptions::new().write(true).open(&img).unwrap();
    f.set_len(orig.len() as u64 + (1 << 20)).unwrap();
    drop(f);
    let (rc, out, _) = amber(&["recover", s(&img), "--write"]);
    assert!(rc == 0 && out.contains("backup moves"), "{out}");
    let (rc, out, _) = amber(&["verify", s(&img)]);
    assert_eq!(rc, 0, "{out}");
    let _ = fs::remove_dir_all(&d);
}

/// The bus wire: `amber bus` answers JSON requests — PlanLayout's signature drives Apply, Verify
/// reads the result, a malformed request is an Error, and an Apply under another scheme is refused.
#[test]
fn bus_wire_end_to_end() {
    use amber_bytes::bus::{Request, Response};
    let d = dir("bus");
    let img = d.join("bus.img");
    fs::File::create(&img).unwrap().set_len(amber_core::kat::CARD_TOTAL * 512).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_amber")).arg("bus").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    use std::io::{BufRead, BufReader, Write};
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut call = |r: &str| -> Response {
        writeln!(stdin, "{r}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let plan = Request::PlanLayout { layout: CARD.into(), target: Some(img.clone()), disk_sectors: None };
    let Response::Planned { signature, scheme, layout, .. } = call(&serde_json::to_string(&plan).unwrap()) else { panic!("PlanLayout") };
    assert_eq!(scheme, "sha256");
    let bad = Request::Apply { target: img.clone(), layout: layout.clone(), scheme: "hmac-sha256".into(), signature: signature.clone(), dry_run: false };
    assert!(matches!(call(&serde_json::to_string(&bad).unwrap()), Response::Error { .. }));
    let apply = Request::Apply { target: img.clone(), layout, scheme, signature, dry_run: false };
    let Response::Applied { written: true, lines } = call(&serde_json::to_string(&apply).unwrap()) else { panic!("Apply") };
    assert!(lines.iter().any(|l| l.contains("fsck clean")), "{lines:?}");
    let Response::Verified { ok: true, worst, .. } = call(&serde_json::to_string(&Request::Verify { target: img.clone() }).unwrap()) else { panic!("Verify") };
    assert_eq!(worst, "PASS");
    let Response::Recovered { direction, written: false, .. } = call(&serde_json::to_string(&Request::Recover { target: img.clone(), dry_run: true }).unwrap()) else { panic!("Recover") };
    assert_eq!(direction, "healthy");
    assert!(matches!(call("{\"verb\":\"Format\"}"), Response::Error { .. }));
    assert!(matches!(call(&serde_json::to_string(&Request::DiskList).unwrap()), Response::Disks { .. }));
    drop(stdin);
    assert!(child.wait().unwrap().success());
    // The bus-applied image is the CLI-applied one, byte for byte, outside the UnaFS volume (whose
    // inode timestamps carry the format clock).
    let b = fs::read(&img).unwrap();
    let d2 = dir("bus-cli");
    let (_, cli) = applied_card(&d2);
    assert_eq!(&b[..264192 * 512], &cli[..264192 * 512], "table + ESP differ between bus and CLI");
    assert_eq!(&b[(b.len() / 512 - 33) * 512..], &cli[(cli.len() / 512 - 33) * 512..]);
    let _ = fs::remove_dir_all(&d);
    let _ = fs::remove_dir_all(&d2);
}

/// The PARTINSTALL fixture (Python's own GPT writer, five foreign partitions): verify passes,
/// every partition probed, recover finds it healthy and writes nothing.
#[test]
fn python_fixture_verifies_and_is_healthy() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../unaos/scripts/make-gpt-fixture.py");
    if tool("python3").is_none() && Command::new("python3").arg("-V").output().is_err() {
        return;
    }
    let d = dir("fixture");
    let img = d.join("fixture.img");
    let st = Command::new("python3").arg(&script).arg("-o").arg(&img).output().unwrap();
    assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
    let before = fs::read(&img).unwrap();
    let (rc, out, _) = amber(&["verify", s(&img)]);
    assert_eq!(rc, 0, "{out}");
    assert_eq!(out.lines().filter(|l| l.trim_start().starts_with("slot")).count(), 5, "{out}");
    assert!(out.contains("fat32"), "the foreign FAT32 partition is probed: {out}");
    let (rc, out, _) = amber(&["recover", s(&img), "--write"]);
    assert!(rc == 0 && out.contains("healthy"), "{out}");
    assert_eq!(fs::read(&img).unwrap(), before, "a healthy recover wrote");
    let _ = fs::remove_dir_all(&d);
}
