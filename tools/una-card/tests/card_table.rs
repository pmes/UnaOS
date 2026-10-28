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

// NVME4KN-FIXTURE (zenbook-ledger Z40): `--lba 4096` lays the same card at 4 KiB LBAs — the header at byte 4096,
// the array at byte 8192, the extents in 4 KiB LBAs (the golden plan / 8), offsets checked BY HAND; p2's bytes
// land at the same byte offset as on the 512 card. Skips when dosfstools/mtools are absent.
#[test]
fn una_card_4kn_table_offsets() {
    if !have("mkfs.vfat") || !have("mcopy") {
        eprintln!("SKIP: mkfs.vfat/mcopy not installed");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("una_card_4kn");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("esp/EFI/BOOT")).unwrap();
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("esp/EFI/BOOT/BOOTX64.EFI"), b"not a real loader").unwrap();
    let mut ufs = vec![0u8; 64 << 20];
    ufs[0..8].copy_from_slice(b"UNAFSTST");
    fs::write(dir.join("unafs.img"), &ufs).unwrap();
    let out = dir.join("card.img");
    let path = format!("{}:/usr/sbin:/sbin", std::env::var("PATH").unwrap_or_default());
    let st = Command::new(env!("CARGO_BIN_EXE_una-card"))
        .env("PATH", path)
        .args(["--esp", dir.join("esp").to_str().unwrap(), "--data", dir.join("data").to_str().unwrap()])
        .args(["--unafs", dir.join("unafs.img").to_str().unwrap(), "-o", out.to_str().unwrap(), "--fat-mb", "128", "--lba", "4096"])
        .output()
        .unwrap();
    assert!(st.status.success(), "una-card failed: {}", String::from_utf8_lossy(&st.stderr));
    assert!(String::from_utf8_lossy(&st.stdout).contains(":: X86-CARD: lba=4096 p1 lba=256 p2 lba=33024 total_lbas=49664 "));
    let card = fs::read(&out).unwrap();
    let total = 49_664u64;
    assert_eq!(card.len() as u64, total * 4096, "same bytes as the 512 card");
    let u64at = |o: usize| u64::from_le_bytes(card[o..o + 8].try_into().unwrap());
    assert_eq!(&card[510..512], &[0x55, 0xAA]);
    assert_eq!(card[450], 0xEE);
    assert!(card[512..4096].iter().all(|&b| b == 0), "no 512 B header");
    assert_eq!(&card[4096..4104], b"EFI PART");
    assert_eq!((u64at(4096 + 24), u64at(4096 + 32)), (1, total - 1));
    assert_eq!((u64at(4096 + 40), u64at(4096 + 48), u64at(4096 + 72)), (6, total - 6, 2));
    assert_eq!(&card[8192..8208], &amber_core::gpt::ESP_TYPE);
    assert_eq!((u64at(8192 + 32), u64at(8192 + 40)), (256, 33_023));
    assert_eq!(&card[8320..8336], &amber_core::gpt::UNAFS_TYPE);
    assert_eq!((u64at(8320 + 32), u64at(8320 + 40)), (33_024, 49_407));
    let bh = (total - 1) as usize * 4096;
    assert_eq!(&card[bh..bh + 8], b"EFI PART");
    assert_eq!(u64at(bh + 72), total - 5);
    // p2's first bytes at 4 KiB LBA 33 024 = 512 B sector 264 192: the volume did not move.
    assert_eq!(&card[33_024 * 4096..33_024 * 4096 + 8], b"UNAFSTST");
    let _ = fs::remove_dir_all(&dir);
}
