// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AMBER1 (SR34): the card `una-card` actually writes carries, byte for byte, the table the
// `amber plan` dry run lists for the card plan (`amber_core::plan_apply::writes` of
// `kat::golden_card_plan`) — run end to end through the real binary (mkfs.vfat + mcopy for p1's
// content, a zero 64 MiB UnaFS image for p2). Skips when dosfstools/mtools are absent.

use std::fs::{self, File};
use std::path::PathBuf;
use std::process::Command;

fn have(t: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(t).is_file()))
        || ["/usr/sbin", "/sbin"].iter().any(|d| std::path::Path::new(d).join(t).is_file())
}

#[test]
fn una_card_table_is_the_dry_run() {
    if !have("mkfs.vfat") || !have("mcopy") {
        eprintln!("SKIP: mkfs.vfat/mcopy not installed");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("una_card_table");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("esp/EFI/BOOT")).unwrap();
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("esp/EFI/BOOT/BOOTX64.EFI"), b"not a real loader").unwrap();
    File::create(dir.join("unafs.img")).unwrap().set_len(64 << 20).unwrap();
    let out = dir.join("card.img");
    let path = format!("{}:/usr/sbin:/sbin", std::env::var("PATH").unwrap_or_default());
    let st = Command::new(env!("CARGO_BIN_EXE_una-card"))
        .env("PATH", path)
        .args(["--esp", dir.join("esp").to_str().unwrap(), "--data", dir.join("data").to_str().unwrap()])
        .args(["--unafs", dir.join("unafs.img").to_str().unwrap(), "-o", out.to_str().unwrap(), "--fat-mb", "128"])
        .output()
        .unwrap();
    assert!(st.status.success(), "una-card failed: {}", String::from_utf8_lossy(&st.stderr));
    let card = fs::read(&out).unwrap();
    let plan = amber_core::kat::golden_card_plan();
    assert_eq!(card.len() as u64, plan.disk_sectors * 512, "card size is the plan's");
    let ws = amber_core::plan_apply::writes(&plan).unwrap();
    for w in &ws {
        let o = w.lba as usize * 512;
        assert_eq!(&card[o..o + w.data.len()], &w.data[..], "{} at lba {} differs from the dry run", w.what, w.lba);
    }
    // Between the arrays and p1, and between p2 and the backup array: untouched zeros.
    assert!(card[34 * 512..2048 * 512].iter().all(|&b| b == 0));
    let mut blk = amber_core::block::MemBlock::from_bytes(card);
    let rep = amber_core::verify::verify(&mut blk);
    assert!(rep.ok() && rep.matches_plan(&plan).is_ok(), "{:?}", rep.lines());
    let _ = fs::remove_dir_all(&dir);
}
