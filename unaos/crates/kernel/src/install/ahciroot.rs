// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the SATA write grant's holders; every GPT read is amber_core's, shared-core; the volume is the unafs crate's, fs-core)
//!
//! AHCIROOT (rmbp-ledger B332, R82: "why not run UnaFS root?") — the rMBP's internal SSD becomes the
//! UnaFS system volume.
//!
//! The block layer owns the grant SLOT (`drivers/block.rs` tail: one port, one LBA range, an RAII hold,
//! -EPERM without it). This file owns the JUDGMENT that lets a grant into the slot outside the installer:
//!
//! * [`ours_unafs_part`] — the disk on a port is OURS (`selfinstall::probe`: every partition a UnaOS ESP
//!   or UnaFS, no foreign type GUID, the ESP carries BOOTX64.EFI + kernel.elf — never a Stranger, never a
//!   blank disk) and the census names its UnaFS partition. The GPT under all of it is read by
//!   `amber_core::gpt::read_table_with`.
//! * [`ensure_root_grant`] — at the root bind, the first OURS disk's UnaFS partition gets the boot's
//!   root grant (silent; the `:: UNAFSX86: root=unafs src=ahci:<port> … ::` line is the boot's one line,
//!   R80). [`bind_ssd_root`] makes the shared UnaFS mount ride it (SSD root over card root).
//! * [`format_fresh`] — `install ssd --write` on a system with no UnaFS volume to mirror formats p2 with
//!   `UnaFS::format` (the crate function `tools/unafs init` calls — one implementation) through the
//!   install grant and the port write path (write + readback + FLUSH).
//! * [`ahciw_selftest`] — `tests ahciw`.
//!
//! The layout reserves [`SCRATCH_SECTORS`] at the tail of p2, outside the volume, for the write proof.

use crate::drivers::block::{self, BlockError, BlockHandle, GrantKind};
use core::sync::atomic::{AtomicU16, Ordering};

/// The last 64 sectors of the installer's UnaFS partition: outside the volume, the only extent
/// `tests ahciw` may write.
pub const SCRATCH_SECTORS: u64 = 64;

/// p2's volume size when the running system has no UnaFS volume to mirror (R82 wants the SSD's UnaFS
/// as the system volume; growing it to the disk is owed).
pub const FRESH_UNAFS_MIB: u64 = 4096;

/// The root grant's port + 1 once minted (0 = none yet). Positive answers only are cached.
static ROOT_PORT: AtomicU16 = AtomicU16::new(0);

/// The UnaFS partition `(first, last)` of the disk on `port` when the disk is judged OURS.
pub fn ours_unafs_part(port: u8) -> Option<(u64, u64)> {
    let sel = block::ahci_info_port(port)?.id(BlockHandle::Ahci { port });
    let p = super::selfinstall::probe(sel, port).ok()?;
    if !matches!(p.verdict, super::selfinstall::Verdict::Ours) || !p.unafs_present {
        return None;
    }
    let t = super::BlockTarget::bind_id(sel).ok()?;
    let c = super::partition::census(&t).ok()?;
    let rows: alloc::vec::Vec<_> = c.rows.iter().filter(|r| r.content == super::partition::Content::UnaFs).collect();
    // Exactly one UnaFS partition: two would make "the root" a guess.
    if rows.len() != 1 {
        return None;
    }
    Some((rows[0].entry.first_lba, rows[0].entry.last_lba))
}

/// The port the boot's root grant names, if it was minted. Never probes.
pub fn root_port() -> Option<u8> {
    match ROOT_PORT.load(Ordering::Acquire) {
        0 => None,
        n => Some((n - 1) as u8),
    }
}

/// Mint and hold the boot's root grant over the first OURS disk's UnaFS partition. Idempotent; silent.
pub fn ensure_root_grant() -> Option<u8> {
    if let Some(p) = root_port() {
        return Some(p);
    }
    // `UNAOS_ROOT_PREFER=sdhc` is the operator's way to boot the card's root over an installed SSD —
    // and so the way to reinstall it (a live root grant makes `install ssd --write` refuse).
    if cfg!(feature = "root-prefer-sdhc") {
        return None;
    }
    if block::ahci_live_grant().is_some() {
        return None; // an installer or a test holds the slot; the root is not taken from under it
    }
    for ix in 0..block::MAX_AHCI_DISKS {
        let Some(d) = block::ahci_disk(ix) else { continue };
        let Some((first, last)) = ours_unafs_part(d.port) else { continue };
        let Some(g) = super::partition::mint_root_grant(d.port, first, last) else { continue };
        if block::hold_ahci_grant_for_boot(g) {
            ROOT_PORT.store(d.port as u16 + 1, Ordering::Release);
            return Some(d.port);
        }
    }
    None
}

/// Make the shared UnaFS mount ride the root-granted SSD: drop a mount bound elsewhere (the card's, from
/// an earlier lazy bind), then bind — `fs::unafs::bind_mount` tries the root port first. `true` when
/// the live mount is on `Ahci { port }`.
pub fn bind_ssd_root(port: u8) -> bool {
    let want = BlockHandle::Ahci { port };
    if crate::fs::unafs::mount_bound_handle().is_some_and(|h| h != want) {
        crate::fs::unafs::force_remount();
    }
    let _ = crate::fs::unafs::with_unafs(|_| ());
    crate::fs::unafs::mount_bound_handle() == Some(want)
}

/// `install ssd --write`'s fresh p2: `UnaFS::format` over `first..` (`mib` MiB) on `port`, through the
/// port write path — so the install grant in the slot (held by the caller) is what admits each sector.
/// Returns the volume's 4 KiB block count.
pub fn format_fresh(port: u8, first: u64, mib: u64) -> Result<u64, &'static str> {
    let dev = crate::fs::unafs::SdSectorDevice::open_on(BlockHandle::Ahci { port }).map_err(|_| "the SSD has no sector device")?;
    let blocks = mib * 256;
    let span = ::unafs::adapter::PartitionSpan { base_lba: first, block_count: blocks };
    let adapter = ::unafs::adapter::BlockAdapter::for_partition(dev, &span);
    let fs = ::unafs::UnaFS::format(adapter, mib).map_err(|_| "UnaFS::format failed")?;
    let n = fs.superblock.block_count;
    drop(fs);
    block::flush_ahci_port(port).map_err(|_| "FLUSH CACHE EXT failed")?;
    Ok(n)
}

/// The volume's end (exclusive, absolute LBA) read off its superblock at `first`, or `None`.
fn volume_end(port: u8, first: u64) -> Option<u64> {
    let mut sb = alloc::vec![0u8; 4096];
    for i in 0..8u64 {
        block::read_block_ahci_port(port, first + i, &mut sb[(i as usize) * 512..(i as usize + 1) * 512]).ok()?;
    }
    let s = ::unafs::superblock::Superblock::from_bytes(&sb).ok()?;
    first.checked_add(s.block_count.checked_mul(s.block_size as u64 / 512)?)
}

/// `tests ahciw` — the grant, proven on the wire:
///
/// 1. WITHOUT a grant covering it, a write to LBA 0 (the protective MBR — never inside any grant this
///    tree mints) is refused -EPERM. The payload is LBA 0's own bytes, so even a broken guard changes
///    nothing.
/// 2. WITH a grant on the 64-sector scratch tail of an OURS disk's UnaFS partition (the root grant when
///    it is live and covers it, else a test grant held for this verb only), one sector is written,
///    flushed, read back and compared, then restored.
///
/// `:: AHCIROOT: grant=<none|port N> write=<refused|ok> readback=<eq|ne> flush=<ok|fail> -> PASS|FAIL|SKIP ::`
pub fn ahciw_selftest() {
    let Some(port) = (0..block::MAX_AHCI_DISKS).find_map(|ix| block::ahci_disk(ix).map(|d| d.port)) else {
        serial_println!(":: AHCIROOT: grant=none write=none readback=none flush=none disks=0 -> SKIP (no AHCI disk) ::");
        return;
    };
    // ── leg 1: the refusal ──────────────────────────────────────────────────────────────────────
    let mut lba0 = [0u8; 512];
    let read0 = block::read_block_ahci_port(port, 0, &mut lba0).is_ok();
    let live = block::ahci_live_grant();
    let r = if read0 { block::write_block_ahci_port(port, 0, &lba0) } else { Err(BlockError::Io) };
    let refused = matches!(r, Err(BlockError::Denied));
    match live {
        None => serial_println!(
            ":: AHCIROOT: grant=none write={} errno={} lba=0 readback=none flush=none -> {} ::",
            if refused { "refused" } else { "ADMITTED" },
            if refused { "-EPERM" } else { "none" },
            if refused { "PASS" } else { "FAIL" }
        ),
        Some((gp, a, b, k)) => serial_println!(
            ":: AHCIROOT: grant=port {} {}..{} kind={} (lba 0 outside) write={} errno={} lba=0 readback=none flush=none -> {} ::",
            gp, a, b, k.tag(),
            if refused { "refused" } else { "ADMITTED" },
            if refused { "-EPERM" } else { "none" },
            if refused { "PASS" } else { "FAIL" }
        ),
    }
    // ── leg 2: the granted write on scratch ─────────────────────────────────────────────────────
    let Some((first, last)) = ours_unafs_part(port) else {
        serial_println!(":: AHCIROOT: grant=none write=none readback=none flush=none port={} -> SKIP (the disk is not OURS or carries no UnaFS partition; scratch exists only on an installed SSD) ::", port);
        return;
    };
    let s0 = last + 1 - SCRATCH_SECTORS;
    match volume_end(port, first) {
        Some(end) if end <= s0 => {}
        end => {
            serial_println!(
                ":: AHCIROOT: grant=none write=none readback=none flush=none scratch={}..{} volume_end={} -> SKIP (the volume reaches the scratch tail — an SSD laid before AHCIROOT; reinstall) ::",
                s0, last, end.unwrap_or(0)
            );
            return;
        }
    }
    let covered = matches!(block::ahci_live_grant(), Some((gp, a, b, _)) if gp == port && a <= s0 && b >= last);
    let _hold = if covered {
        None
    } else {
        let Some(g) = super::partition::mint_scratch_grant(port, first, last) else {
            serial_println!(":: AHCIROOT: grant=none write=none readback=none flush=none -> FAIL (the scratch grant was not minted) ::");
            return;
        };
        match block::hold_ahci_grant(g, GrantKind::Test) {
            Some(h) => Some(h),
            None => {
                serial_println!(":: AHCIROOT: grant=none write=none readback=none flush=none -> SKIP (the grant slot is held elsewhere) ::");
                return;
            }
        }
    };
    let mut orig = [0u8; 512];
    if block::read_block_ahci_port(port, s0, &mut orig).is_err() {
        serial_println!(":: AHCIROOT: grant=port {} write=none readback=none flush=none -> FAIL (scratch read failed) ::", port);
        return;
    }
    let mut pat = [0u8; 512];
    let stamp = crate::arch::now_cycles();
    for (i, b) in pat.iter_mut().enumerate() {
        *b = (i as u8) ^ (stamp >> ((i % 8) * 8)) as u8;
    }
    pat[..8].copy_from_slice(b"AHCIROOT");
    let wrote = block::write_block_ahci_port(port, s0, &pat);
    let flushed = block::flush_ahci_port(port).is_ok();
    let mut back = [0u8; 512];
    let eq = wrote.is_ok() && block::read_block_ahci_port(port, s0, &mut back).is_ok() && back == pat;
    let restored = block::write_block_ahci_port(port, s0, &orig).is_ok();
    let pass = wrote.is_ok() && eq && flushed && restored;
    serial_println!(
        ":: AHCIROOT: grant=port {} write={} readback={} flush={} scratch={}..{} lba={} restored={} -> {} ::",
        port,
        if wrote.is_ok() { "ok" } else { "refused" },
        if eq { "eq" } else { "ne" },
        if flushed { "ok" } else { "fail" },
        s0,
        last,
        s0,
        if restored { "ok" } else { "FAIL" },
        if pass { "PASS" } else { "FAIL" }
    );
}
