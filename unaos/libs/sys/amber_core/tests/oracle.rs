// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AMBER1 (SR34) ORACLES — the bytes amber_core writes, read back by implementations that are not
//! UnaOS's: the PARTINSTALL fixture's Python GPT writer (`unaos/scripts/make-gpt-fixture.py`),
//! util-linux `partx` + `blkid` (libblkid's GPT and DOS readers), dosfstools `fsck.fat` and
//! `mkfs.vfat`, and mtools `minfo`/`mdir`. Each test SKIPS (and says so on stderr) when its oracle
//! is not installed, so the suite stays green offline; the evidence doc records a run with all of
//! them present.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use amber_core::block::Block;
use amber_core::file_block::FileBlock;
use amber_core::gpt::{self, SECTOR};
use amber_core::plan_apply::{self, Mode};
use amber_core::{fat32_format, kat, kat_write, mbr, verify};

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("amber_oracle");
    fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn have(tool: &str) -> bool {
    let ok = Command::new("sh").arg("-c").arg(format!("command -v {tool} || test -x /usr/sbin/{tool} || test -x /sbin/{tool}")).output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: oracle `{tool}` not installed");
    }
    ok
}

fn tool(name: &str) -> String {
    for d in ["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
        let p = format!("{d}/{name}");
        if Path::new(&p).exists() {
            return p;
        }
    }
    name.to_string()
}

fn sparse(path: &Path, sectors: u64) -> FileBlock {
    let f = File::create(path).unwrap();
    f.set_len(sectors * SECTOR as u64).unwrap();
    drop(f);
    FileBlock::open_rw(path).unwrap()
}

fn run(cmd: &str, args: &[&str]) -> (bool, String) {
    let o = Command::new(tool(cmd)).args(args).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

/// The Python writer's whole fixture image vs this crate's table, byte for byte; then verify reads it.
#[test]
fn python_fixture_is_byte_identical() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/make-gpt-fixture.py");
    if !have("python3") || !script.exists() {
        return;
    }
    let out = tmp("part-fixture.img");
    let st = Command::new("python3").arg(&script).arg("-o").arg(&out).output().unwrap();
    assert!(st.status.success(), "fixture script failed");
    let bytes = fs::read(&out).unwrap();
    let img = kat_write::fixture_image();
    let mut ours = vec![0u8; bytes.len()];
    for (lba, b) in img.writes() {
        let o = lba as usize * SECTOR;
        ours[o..o + b.len()].copy_from_slice(b);
    }
    let t = kat_write::FIXTURE_TOTAL as usize;
    assert_eq!(&bytes[..34 * SECTOR], &ours[..34 * SECTOR], "primary region differs from the Python writer");
    assert_eq!(&bytes[(t - 33) * SECTOR..], &ours[(t - 33) * SECTOR..], "backup region differs from the Python writer");
    let mut fb = FileBlock::open(&out).unwrap();
    let rep = verify::verify(&mut fb);
    for l in rep.lines() {
        eprintln!("fixture verify: {l}");
    }
    assert!(rep.ok() && rep.worst() == verify::Level::Pass && rep.entries.len() == 5);
    let _ = fs::remove_file(&out);
}

/// libblkid reads what `apply` lays: the card plan, every extent, name and GUID.
#[test]
fn partx_and_blkid_read_the_card_table() {
    if !have("partx") || !have("blkid") {
        return;
    }
    let plan = kat::golden_card_plan();
    let path = tmp("card.img");
    let mut fb = sparse(&path, plan.disk_sectors);
    let r = plan_apply::apply(&plan, &mut fb, Mode::Write).unwrap();
    assert!(r.verify.unwrap().ok());
    drop(fb);
    let (ok, out) = run("partx", &["-g", "-r", "-o", "NR,START,END,NAME,UUID,TYPE", path.to_str().unwrap()]);
    assert!(ok, "partx failed: {out}");
    eprintln!("partx: {out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), plan.parts.len());
    for (i, (l, p)) in lines.iter().zip(&plan.parts).enumerate() {
        let f: Vec<&str> = l.split(' ').collect();
        assert_eq!(f[0], format!("{}", i + 1));
        assert_eq!(f[1].parse::<u64>().unwrap(), p.first);
        assert_eq!(f[2].parse::<u64>().unwrap(), p.last);
        assert_eq!(f[3], p.name);
        assert_eq!(f[4].to_uppercase(), gpt::guid_string(&p.guid));
        assert_eq!(f[5].to_uppercase(), gpt::guid_string(&p.kind.type_guid()));
    }
    let (ok, out) = run("blkid", &["-p", "-o", "export", path.to_str().unwrap()]);
    assert!(ok, "blkid failed: {out}");
    eprintln!("blkid: {out}");
    assert!(out.contains("PTTYPE=gpt"));
    assert!(out.to_uppercase().contains(&format!("PTUUID={}", gpt::guid_string(&plan.disk_guid))));
    let _ = fs::remove_file(&path);
}

/// libblkid's DOS reader reads a legacy table `mbr::Mbr::legacy` encodes.
#[test]
fn partx_reads_a_legacy_mbr() {
    if !have("partx") {
        return;
    }
    let total = 2_000_000u64;
    let parts = [
        mbr::part(mbr::TYPE_FAT32_LBA, 2048, 524_288, true).unwrap(),
        mbr::part(mbr::TYPE_LINUX, 526_336, 1_048_576, false).unwrap(),
    ];
    let m = mbr::Mbr::legacy(total, 0x554E_4153, &parts).unwrap();
    let path = tmp("legacy.img");
    let mut fb = sparse(&path, total);
    fb.write(0, &m.encode(&[])).unwrap();
    drop(fb);
    let (ok, out) = run("partx", &["-g", "-r", "-o", "NR,START,SECTORS,TYPE,FLAGS", path.to_str().unwrap()]);
    assert!(ok, "partx failed: {out}");
    eprintln!("partx dos: {out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, ["1 2048 524288 0xc 0x80", "2 526336 1048576 0x83 0x0"]);
    let (ok, out) = run("blkid", &["-p", "-o", "export", path.to_str().unwrap()]);
    assert!(ok && out.contains("PTTYPE=dos") && out.contains("PTUUID=554e4153"), "blkid: {out}");
    let _ = fs::remove_file(&path);
}

fn bpb(b: &[u8]) -> (u8, u32, u32, u32) {
    (b[13], u32::from_le_bytes(b[32..36].try_into().unwrap()), u32::from_le_bytes(b[36..40].try_into().unwrap()), u32::from_le_bytes(b[44..48].try_into().unwrap()))
}

/// dosfstools checks a volume `fat32_format` lays, mtools reads it, and `mkfs.vfat` given the same
/// parameters lands on the same geometry.
#[test]
fn fsck_fat_and_mkfs_vfat_agree_with_the_format() {
    if !have("fsck.fat") || !have("mkfs.vfat") || !have("minfo") {
        return;
    }
    for (sectors, spc) in [(131_072u64, None), (262_144, Some(1u8)), (1_048_576, None)] {
        let path = tmp(&format!("fat-{sectors}.img"));
        let mut fb = sparse(&path, sectors);
        let p = fat32_format::FormatParams { label: fat32_format::FormatParams::label_from("UNA ESP"), volume_id: 0x554E_4153, sectors_per_cluster: spc, hidden: 0 };
        let g = fat32_format::format(&mut fb, &p).unwrap();
        drop(fb);
        let (ok, out) = run("fsck.fat", &["-n", "-v", path.to_str().unwrap()]);
        eprintln!("fsck.fat {sectors}: {out}");
        assert!(ok, "fsck.fat refused the {sectors}-sector volume: {out}");
        assert!(!out.contains("differ") && !out.contains("Free cluster summary wrong") && !out.contains("Dirty bit"), "fsck.fat complaint: {out}");
        let (ok, out) = run("minfo", &["-i", path.to_str().unwrap(), "::"]);
        assert!(ok, "minfo: {out}");
        assert!(out.contains(&format!("cluster size: {} sectors", g.spc)) && out.contains(&format!("free clusters={}", g.count_of_clusters - 1)) && out.contains(&format!("Big fatlen={}", g.fat_sz)), "minfo spc: {out}");
        assert!(out.contains(&format!("big size: {} sectors", sectors)), "minfo size: {out}");
        let (ok, out) = run("mdir", &["-i", path.to_str().unwrap(), "::"]);
        assert!(ok && out.contains("Volume in drive : is UNA ESP"), "mdir: {out}");

        // mkfs.vfat with the same parameters.
        let mpath = tmp(&format!("mkfs-{sectors}.img"));
        File::create(&mpath).unwrap().set_len(sectors * SECTOR as u64).unwrap();
        let spcs = format!("{}", g.spc);
        let (ok, out) = run("mkfs.vfat", &["-F", "32", "-s", &spcs, "-R", "32", "-f", "2", "-n", "UNA ESP", "-i", "554E4153", "-h", "0", "-a", mpath.to_str().unwrap()]);
        assert!(ok, "mkfs.vfat: {out}");
        let ours = fs::read(&path).unwrap();
        let theirs = fs::read(&mpath).unwrap();
        let (o, t) = (bpb(&ours), bpb(&theirs));
        eprintln!("bpb (spc, tot_sec, fat_sz, root) ours {o:?} mkfs.vfat {t:?}");
        // Same cluster size, size and root; the FAT size is fatgen103 §3.5's closed form on our side
        // (the spec's — "may be slightly larger than needed") and dosfstools' exact solve on theirs:
        // ours is never smaller, and the excess is a handful of sectors.
        assert_eq!((o.0, o.1, o.3), (t.0, t.1, t.3), "BPB differs from mkfs.vfat at {sectors} sectors");
        assert!(o.2 >= t.2 && o.2 - t.2 <= t.2 / 100 + 8, "FAT size {} vs mkfs.vfat {}", o.2, t.2);
        let free = |b: &[u8]| u32::from_le_bytes(b[512 + 488..512 + 492].try_into().unwrap());
        let t_clusters = (t.1 - 32 - 2 * t.2) / t.0 as u32;
        eprintln!("fsinfo free ours {} (clusters {}) mkfs.vfat {} (clusters {})", free(&ours), g.count_of_clusters, free(&theirs), t_clusters);
        assert_eq!(free(&ours), g.count_of_clusters - 1);
        assert_eq!(free(&theirs), t_clusters - 1, "mkfs.vfat's FSInfo follows the same rule");
        assert_eq!(&ours[71..82], &theirs[71..82], "label");
        assert_eq!(&ours[67..71], &theirs[67..71], "serial");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&mpath);
    }
}
