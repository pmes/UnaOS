// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — fs-core (the grow is the unafs crate's `UnaFS::grow`, the function `tools/unafs grow` and the installer call; this file is only its in-kernel fixture)
//!
//! UNAFSGROW (rmbp-ledger B347) M4 — `tests unafsgrow`.
//!
//! 1. Format a scratch UnaFS image of [`FROM`] blocks in RAM (`MemDevice`, the crate's own backend),
//!    write a witness file, extend the image by 1 MiB and `UnaFS::grow` it to [`FROM`] + [`ADD`];
//!    `fsck(false)` must be clean and the file intact.
//! 2. Persist the grown image to `/var/tmp/unafsgrow.img` on the running UnaFS root (one write, one
//!    commit — a block device over a file of the live root would commit per block), read it back.
//! 3. Mount the read-back bytes, `fsck(false)` again, the file again.
//!
//! `:: UNAFSGROW: from=<blocks> to=<blocks> fsck=ok -> PASS ::` (after one `[unafsgrow]` detail line).
//! With no UnaFS root mounted the image stays in RAM (`img=ram`, legs 1 and 3 over the in-RAM bytes).

use ::unafs::{MemDevice, UnaFS};
use alloc::vec::Vec;

/// The scratch volume's size before the grow: 512 blocks = 2 MiB.
pub const FROM: u64 = 512;
/// The grow: 1 MiB of 4 KiB blocks.
pub const ADD: u64 = 256;
const DIR: &str = "/var/tmp";
const NAME: &str = "unafsgrow.img";
const WITNESS: &[u8] = b"UNAFSGROW witness: grown, not lost";

fn witness_ok<D: ::unafs::BlockDevice>(fs: &mut UnaFS<D>) -> bool {
    let Ok(id) = fs.resolve_path("/w.txt") else { return false };
    fs.read_data(id, 0, WITNESS.len() as u64).is_ok_and(|d| d == WITNESS)
}

/// Find or make `/var/tmp`, replace `unafsgrow.img` with `bytes`, and read it back.
fn persist(k: &mut crate::fs::unafs::KernelUnaFS, bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let root = k.superblock.root_inode;
    let var = match k.resolve_path("/var") {
        Ok(id) => id,
        Err(_) => k.mkdir(root, "var".into()).map_err(|_| "mkdir /var failed")?,
    };
    let tmp = match k.resolve_path(DIR) {
        Ok(id) => id,
        Err(_) => k.mkdir(var, "tmp".into()).map_err(|_| "mkdir /var/tmp failed")?,
    };
    if k.resolve_path("/var/tmp/unafsgrow.img").is_ok() {
        k.unlink(tmp, NAME).map_err(|_| "unlink of the old image failed")?;
    }
    let id = k.create_file(tmp, NAME.into()).map_err(|_| "create failed")?;
    k.write_data(id, 0, bytes).map_err(|_| "write failed")?;
    k.read_data(id, 0, bytes.len() as u64).map_err(|_| "readback failed")
}

pub fn selftest() {
    let to = FROM + ADD;
    // ── leg 1: format, write, grow, fsck — in RAM ───────────────────────────────────────────────
    let mut fs = match UnaFS::format(MemDevice::with_blocks(FROM), 0) {
        Ok(fs) => fs,
        Err(_) => {
            serial_println!(":: UNAFSGROW: from={} to={} fsck=none -> FAIL (format of the scratch image failed) ::", FROM, to);
            return;
        }
    };
    let root = fs.superblock.root_inode;
    let wrote = fs.create_file(root, "w.txt".into()).and_then(|id| fs.write_data(id, 0, WITNESS)).is_ok();
    fs.device.resize_blocks(to);
    let r = match fs.grow(to) {
        Ok(r) => r,
        Err(_) => {
            serial_println!(":: UNAFSGROW: from={} to={} fsck=none -> FAIL (UnaFS::grow refused the scratch image) ::", FROM, to);
            return;
        }
    };
    let fsck1 = fs.fsck(false).is_ok_and(|rep| rep.is_clean());
    let file1 = wrote && witness_ok(&mut fs);
    let free = (r.free_before, r.free_after);
    let bytes = core::mem::take(&mut fs.device).into_bytes();
    drop(fs);
    // ── leg 2: /var/tmp on the running root ─────────────────────────────────────────────────────
    let (img, back, why) = match crate::fs::unafs::with_unafs(|k| persist(k, &bytes)) {
        Ok(Ok(b)) => ("/var/tmp/unafsgrow.img", b, ""),
        Ok(Err(why)) => ("ram", bytes.clone(), why),
        Err(_) => ("ram", bytes.clone(), "no UnaFS root mounted"),
    };
    let eq = back == bytes;
    drop(bytes);
    // ── leg 3: mount the read-back image, fsck again ────────────────────────────────────────────
    let (fsck2, file2, size2) = match UnaFS::mount(MemDevice::from_bytes(back)) {
        Ok(mut m) => (m.fsck(false).is_ok_and(|rep| rep.is_clean()), witness_ok(&mut m), m.superblock.block_count),
        Err(_) => (false, false, 0),
    };
    let fsck_ok = fsck1 && fsck2;
    let pass = fsck_ok && file1 && file2 && eq && r.from == FROM && r.to == to && size2 == to;
    serial_println!(
        "[unafsgrow] img={}{}{} readback={} remount_blocks={} file={} free={}->{} (+{} blocks, the map's own growth {})",
        img,
        if why.is_empty() { "" } else { " — " },
        why,
        if eq { "eq" } else { "NE" },
        size2,
        if file1 && file2 { "intact" } else { "LOST" },
        free.0,
        free.1,
        ADD,
        (free.0 + ADD).saturating_sub(free.1)
    );
    serial_println!(
        ":: UNAFSGROW: from={} to={} fsck={} -> {} ::",
        r.from,
        r.to,
        if fsck_ok { "ok" } else { "FAIL" },
        if pass { "PASS" } else { "FAIL" }
    );
}
