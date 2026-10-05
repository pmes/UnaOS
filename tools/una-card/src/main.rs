// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// una-card — the x86 card image with a UnaFS system volume (UNAFSX86 M3, SH-2 rung 1), now laid by
// `amber_core` (SELFINSTALL2, rmbp-ledger B310). Replaces `unaos/scripts/make-x86-card.py`.
//
//   p1  EFI System Partition  FAT32, holds target/x86_64_esp/ + target/x86_64_data/ (mkfs.vfat + mcopy).
//   p2  UnaFS system volume   the bytes `tools/unafs init` produced; found by the kernel by SUPERBLOCK
//                             MAGIC (`locate_unafs`), the type GUID is advisory.
//
// The GPT (protective MBR, both headers, both arrays, CRCs) is `amber_core::Plan::for_image` +
// `Plan::gpt` — byte-for-byte the table the retired script wrote (the core's golden KAT pins it) —
// laid by `amber_core::plan_apply::apply` (AMBER1), the same write list `amber plan` dry-runs.
//
// Usage: una-card --esp DIR --data DIR --unafs IMG -o OUT [--fat-mb N]
//        una-card show IMG          (print the plan read back off an image)

use amber_core::plan::{PartKind, PartReq, Plan, Size};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SECTOR: u64 = 512;

fn die(msg: &str) -> ! {
    eprintln!("una-card: {msg}");
    std::process::exit(1);
}

fn tree_bytes(d: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            match e.metadata() {
                Ok(m) if m.is_dir() => total += tree_bytes(&p),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn which(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(tool).is_file()))
}

fn run(cmd: &mut Command) {
    let st = cmd.status().unwrap_or_else(|e| die(&format!("cannot run {:?}: {e}", cmd.get_program())));
    if !st.success() {
        die(&format!("{:?} failed ({st})", cmd.get_program()));
    }
}

fn build_fat(path: &Path, sectors: u64, trees: &[PathBuf]) {
    for t in ["mkfs.vfat", "mcopy"] {
        if !which(t) {
            die(&format!("'{t}' not found — install dosfstools + mtools"));
        }
    }
    File::create(path).and_then(|f| f.set_len(sectors * SECTOR)).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    run(Command::new("mkfs.vfat").args(["-F", "32", "-n", "UNAOS"]).arg(path).stdout(Stdio::null()));
    for d in trees {
        let mut names: Vec<_> = fs::read_dir(d).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        names.sort();
        for e in names {
            run(Command::new("mcopy").args(["-s", "-b", "-o", "-Q", "-i"]).arg(path).arg(&e).arg("::/"));
        }
    }
}

fn copy_into(out: &mut File, src: &Path, first_lba: u64) -> io::Result<()> {
    out.seek(SeekFrom::Start(first_lba * SECTOR))?;
    let mut i = File::open(src)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = i.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        out.write_all(&buf[..n])?;
    }
}

fn show(img: &Path) {
    let bytes = fs::read(img).unwrap_or_else(|e| die(&format!("{}: {e}", img.display())));
    let t = amber_core::gpt::parse_image(&bytes).unwrap_or_else(|e| die(&format!("{}: {e}", img.display())));
    for (slot, e) in &t.entries {
        println!(
            "slot {slot} {:<5} lba {}..{} {} MiB {}",
            amber_core::gpt::type_name(&e.type_guid),
            e.first_lba,
            e.last_lba,
            amber_core::sectors_mib(e.sectors()),
            e.name_string()
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("show") {
        match args.get(1) {
            Some(p) => return show(Path::new(p)),
            None => die("usage: una-card show IMG"),
        }
    }
    let (mut esp, mut data, mut unafs, mut out, mut fat_mb) = (None, None, None, None, 0u64);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| die(&format!("{a} needs a value")));
        match a.as_str() {
            "--esp" => esp = Some(PathBuf::from(val())),
            "--data" => data = Some(PathBuf::from(val())),
            "--unafs" => unafs = Some(PathBuf::from(val())),
            "-o" | "--out" => out = Some(PathBuf::from(val())),
            "--fat-mb" => fat_mb = val().parse().unwrap_or_else(|_| die("--fat-mb takes a number")),
            _ => die("usage: una-card --esp DIR --data DIR --unafs IMG -o OUT [--fat-mb N] | una-card show IMG"),
        }
    }
    let (Some(esp), Some(data), Some(unafs), Some(out)) = (esp, data, unafs, out) else {
        die("usage: una-card --esp DIR --data DIR --unafs IMG -o OUT [--fat-mb N]");
    };

    let trees: Vec<PathBuf> = [esp, data].into_iter().filter(|d| d.is_dir()).collect();
    let fat_mb = if fat_mb > 0 { fat_mb } else { 128.max((trees.iter().map(|d| tree_bytes(d)).sum::<u64>() >> 20) + 64) };
    let fat_sectors = fat_mb * 2048;
    let unafs_bytes = fs::metadata(&unafs).unwrap_or_else(|e| die(&format!("{}: {e}", unafs.display()))).len();
    if unafs_bytes % 4096 != 0 {
        die(&format!("{} is not a whole number of 4 KiB blocks", unafs.display()));
    }
    let unafs_sectors = unafs_bytes / SECTOR;

    let plan = Plan::for_image(
        b"UNAOS-X86-CARD",
        &[
            PartReq { kind: PartKind::Esp, size: Size::Sectors(fat_sectors), name: "UNAOS-ESP", seed: b"UNAOS-X86-ESP" },
            PartReq { kind: PartKind::UnaFS, size: Size::Sectors(unafs_sectors), name: "UNAOS-UNAFS", seed: b"UNAOS-X86-UFS" },
        ],
    )
    .unwrap_or_else(|e| die(&e.to_string()));
    let (p1, p2) = (&plan.parts[0], &plan.parts[1]);

    let tmp = out.with_extension("esp.fat.tmp");
    build_fat(&tmp, fat_sectors, &trees);
    let res = (|| -> io::Result<()> {
        File::create(&out)?.set_len(plan.disk_sectors * SECTOR)?;
        // AMBER1 (SR34): the table goes down through `amber_core::plan_apply::apply` — the write
        // list the `amber plan` dry run shows, issued, flushed and verified on read-back.
        {
            let mut blk = amber_core::file_block::FileBlock::open_rw(&out)?;
            amber_core::plan_apply::apply(&plan, &mut blk, amber_core::plan_apply::Mode::Write)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        let mut o = std::fs::OpenOptions::new().write(true).open(&out)?;
        copy_into(&mut o, &tmp, p1.first)?;
        copy_into(&mut o, &unafs, p2.first)?;
        o.sync_all()
    })();
    let _ = fs::remove_file(&tmp);
    if let Err(e) = res {
        die(&format!("{}: {e}", out.display()));
    }
    for line in plan.lines() {
        println!("{line}");
    }
    // The line the retired script printed, unchanged, so a log reader keys on the same words.
    println!(
        ":: X86-CARD: p1=esp lba={} sectors={} p2=unafs lba={} sectors={} total_mb={} ::",
        p1.first,
        p1.sectors(),
        p2.first,
        p2.sectors(),
        plan.disk_sectors / 2048
    );
}
