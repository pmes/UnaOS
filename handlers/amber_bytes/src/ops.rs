// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The OPERATIONS behind every bus verb and CLI command, over a `&mut dyn Block` (an image file, a
//! device the policy opened, or memory). Every one of them is `amber_core` / `unafs` underneath —
//! nothing here encodes a table or a filesystem itself:
//!
//! * [`plan`] — a layout on a medium of N sectors → the table's exact writes
//!   (`amber_core::plan_apply::writes`), each partition's format writes (FAT32's exact list from
//!   `amber_core::fat32_format::writes`; UnaFS by its parameters), and the CANONICAL bytes the
//!   signature covers;
//! * [`apply`] — refuse unless the signature verifies over the plan re-derived for THIS medium; then
//!   lay the table (read back through `verify` by the core), format every partition, and prove each
//!   format by reading it back (FAT32: the boot sector, FSInfo and both FATs' first sectors against
//!   the write list; UnaFS: mount + fsck);
//! * [`verify`] — the core's whole-table verify plus a probe of every partition's filesystem;
//! * [`recover`] — the core's backup-header restore, dry run or write, verified after.

use std::fmt::Write as _;

use amber_core::block::{Block, BlockError, Recorder, Window};
use amber_core::fat32_format::{self, FormatParams as FatParams};
use amber_core::gpt::{self, SECTOR};
use amber_core::plan::Plan;
use amber_core::plan_apply::{self, Mode, SectorWrite};
use amber_core::recover::{self, Direction};
use amber_core::verify::{self, VerifyReport};
use amber_core::crc32;

use crate::layout::{Fs, LayoutSpec};
use crate::signer::Signer;

/// A `unafs::SectorDevice` over an `amber_core` [`Block`] — the one bridge between the two seams.
pub struct Sectors<'a>(pub &'a mut dyn Block);

fn sector_err(e: BlockError, lba: u64) -> unafs::SectorError {
    match e {
        BlockError::OutOfRange => unafs::SectorError::OutOfBounds(lba),
        e => unafs::SectorError::Io(e.to_string()),
    }
}

impl unafs::SectorDevice for Sectors<'_> {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), unafs::SectorError> {
        self.0.read(lba, buf).map_err(|e| sector_err(e, lba))
    }
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), unafs::SectorError> {
        self.0.write(lba, buf).map_err(|e| sector_err(e, lba))
    }
    fn sector_count(&self) -> u64 {
        self.0.sectors()
    }
    fn flush(&mut self) -> Result<(), unafs::SectorError> {
        self.0.flush().map_err(|e| sector_err(e, 0))
    }
}

/// One partition's format.
#[derive(Clone, Debug)]
pub enum FormatStep {
    Fat32 { part: usize, first: u64, sectors: u64, params: FatParams, writes: Vec<SectorWrite> },
    Unafs { part: usize, first: u64, sectors: u64, params: unafs::FormatParams, blocks: u64 },
}

impl FormatStep {
    fn line(&self) -> String {
        match self {
            FormatStep::Fat32 { part, first, sectors, params, writes } => format!(
                "p{} fat32 lba {} x{} label {:?} serial {:08X} spc {} ({} writes)",
                part + 1,
                first,
                sectors,
                String::from_utf8_lossy(&params.label).trim_end(),
                params.volume_id,
                params.sectors_per_cluster.unwrap_or_else(|| fat32_format::default_spc(*sectors)),
                writes.len()
            ),
            FormatStep::Unafs { part, first, sectors, params, blocks } => {
                format!("p{} unafs lba {} x{} v{} blocks {}", part + 1, first, sectors, params.version, blocks)
            }
        }
    }
}

/// A layout laid on a medium: everything `Apply` will do, before it does it.
#[derive(Clone, Debug)]
pub struct Planned {
    pub layout: LayoutSpec,
    pub plan: Plan,
    pub table: Vec<SectorWrite>,
    pub formats: Vec<FormatStep>,
    /// What the signature covers: `amber_core::plan_apply::canonical` of the table, then the format
    /// section (every FAT32 write with its absolute LBA and CRC-32; each UnaFS volume's parameters —
    /// its bytes carry the format clock, so they are not part of the digest).
    pub canonical: Vec<u8>,
}

impl Planned {
    /// The dry run as text (the canonical bytes are UTF-8).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.canonical).into_owned()
    }
}

/// Lay `layout` on a medium of `disk_sectors` sectors (nothing is read or written).
pub fn plan(layout: &LayoutSpec, disk_sectors: u64) -> Result<Planned, String> {
    let plan = layout.plan(disk_sectors)?;
    let table = plan_apply::writes(&plan).map_err(|e| e.to_string())?;
    let mut formats = Vec::new();
    for (i, (spec, part)) in layout.parts.iter().zip(&plan.parts).enumerate() {
        match spec.format {
            Fs::None => {}
            Fs::Fat32 => {
                let seed = spec.seed_for(&layout.disk_seed);
                let params = FatParams {
                    label: FatParams::label_from(spec.label.as_deref().unwrap_or(&spec.name)),
                    volume_id: crc32(seed.as_bytes()),
                    sectors_per_cluster: spec.spc,
                    hidden: part.first,
                };
                let (_, writes) = fat32_format::writes(part.sectors(), &params).map_err(|e| format!("p{}: {e}", i + 1))?;
                formats.push(FormatStep::Fat32 { part: i, first: part.first, sectors: part.sectors(), params, writes });
            }
            Fs::Unafs => {
                let params = unafs::FormatParams::default();
                let blocks = unafs::format::volume_blocks(part.sectors() / 8, &params).map_err(|e| format!("p{}: {e}", i + 1))?;
                formats.push(FormatStep::Unafs { part: i, first: part.first, sectors: part.sectors(), params, blocks });
            }
        }
    }
    let mut canonical = plan_apply::canonical(&plan).map_err(|e| e.to_string())?;
    let mut s = String::new();
    let _ = writeln!(s, "formats: {}", formats.len());
    for f in &formats {
        let _ = writeln!(s, "  {}", f.line());
        if let FormatStep::Fat32 { first, writes, .. } = f {
            for w in writes {
                let abs = SectorWrite { lba: w.lba + first, ..w.clone() };
                let _ = writeln!(s, "    {}", abs);
            }
        }
    }
    canonical.extend_from_slice(s.as_bytes());
    Ok(Planned { layout: layout.clone(), plan, table, formats, canonical })
}

/// What [`apply`] did.
#[derive(Clone, Debug)]
pub struct Applied {
    pub planned: Planned,
    pub written: bool,
    pub lines: Vec<String>,
}

/// Apply `layout` to `dev` under `sig` (see the module doc). `Mode::DryRun` checks the signature
/// too — a dry run of an Apply is the Apply minus the writes.
pub fn apply(dev: &mut dyn Block, layout: &LayoutSpec, signer: &dyn Signer, sig: &str, mode: Mode) -> Result<Applied, String> {
    let planned = plan(layout, dev.sectors())?;
    if !signer.verify(&planned.canonical, sig) {
        return Err(format!(
            "the {} signature does not match the plan for this {}-sector medium — run `plan` on this medium and present what it shows",
            signer.scheme(),
            dev.sectors()
        ));
    }
    let mut lines = vec![format!("signature ok ({})", signer.scheme())];
    if mode == Mode::DryRun {
        lines.push("dry run: nothing written".into());
        return Ok(Applied { planned, written: false, lines });
    }
    let rep = plan_apply::apply(&planned.plan, dev, Mode::Write).map_err(|e| e.to_string())?;
    lines.push(format!("table: {} writes, read back {}", rep.writes.len(), rep.verify.as_ref().map_or("?", |v| v.worst().tag())));
    for f in &planned.formats {
        match f {
            FormatStep::Fat32 { part, first, sectors, writes, .. } => {
                let mut win = Window::new(dev, *first, *sectors).ok_or("partition outside the medium")?;
                plan_apply::commit(writes, &mut win).map_err(|e| format!("p{}: {e}", part + 1))?;
                // Read back every byte-carrying write (the zero runs are the FAT bodies).
                for w in writes.iter().filter(|w| w.zeros == 0) {
                    let got = amber_core::block::read_vec(&mut win, w.lba, w.sectors()).map_err(|e| e.to_string())?;
                    if got != w.data {
                        return Err(format!("p{}: {} at lba {} did not read back", part + 1, w.what, w.lba + first));
                    }
                }
                lines.push(format!("p{} fat32: {} writes, read back ok", part + 1, writes.len()));
            }
            FormatStep::Unafs { part, first, sectors, params, .. } => {
                let mut s = Sectors(dev);
                let made = unafs::format_partition(&mut s, *first, *sectors, params).map_err(|e| format!("p{}: {e}", part + 1))?;
                let fsck = unafs_probe(dev, *first, made.blocks)?;
                lines.push(format!("p{} unafs: v{} {} blocks, {}", part + 1, made.version, made.blocks, fsck));
            }
        }
    }
    dev.flush().map_err(|e| e.to_string())?;
    Ok(Applied { planned, written: true, lines })
}

/// Mount the UnaFS at `first` through a write-capturing [`Recorder`] (so a probe never writes the
/// medium even if the mount drains a reclaim queue) and fsck it without repair.
pub fn unafs_probe(dev: &mut dyn Block, first: u64, blocks: u64) -> Result<String, String> {
    let mut rec = Recorder::new(dev);
    let mut s = Sectors(&mut rec);
    let adapter = unafs::BlockAdapter::new(&mut s, first, blocks);
    let mut fs = unafs::UnaFS::mount(adapter).map_err(|e| format!("unafs mount: {e}"))?;
    let r = fs.fsck(false).map_err(|e| format!("unafs fsck: {e}"))?;
    if !r.is_clean() {
        return Err(format!("unafs fsck: not clean: {r:?}"));
    }
    Ok(format!("mounted, fsck clean, generation {}, {} free blocks", fs.root_generation(), fs.free_blocks()))
}

/// What a partition holds, by its first sector(s).
pub fn probe_part(dev: &mut dyn Block, first: u64, sectors: u64) -> String {
    let Ok(s0) = amber_core::block::read_vec(dev, first, 1) else { return "unreadable".into() };
    if s0[510] == 0x55 && s0[511] == 0xAA && &s0[82..90] == b"FAT32   " {
        let label = String::from_utf8_lossy(&s0[71..82]).trim_end().to_string();
        let serial = u32::from_le_bytes([s0[67], s0[68], s0[69], s0[70]]);
        return format!("fat32 label {label:?} serial {serial:08X}");
    }
    if sectors >= 8 {
        if let Ok(b0) = amber_core::block::read_vec(dev, first, 8) {
            if let Ok(sb) = unafs::Superblock::from_bytes(&b0) {
                let blocks = sb.block_count.min(sectors / 8);
                return match unafs_probe(dev, first, blocks) {
                    Ok(s) => format!("unafs v{} {} blocks: {s}", sb.version, sb.block_count),
                    Err(e) => format!("unafs v{} {} blocks: {e}", sb.version, sb.block_count),
                };
            }
        }
    }
    if s0.iter().all(|&b| b == 0) { "empty (first sector zero)".into() } else { "unknown".into() }
}

/// The core's verify, plus a probe of every partition.
pub fn verify(dev: &mut dyn Block) -> (VerifyReport, Vec<String>) {
    let rep = verify::verify(dev);
    let mut lines = rep.lines();
    for (slot, e) in rep.entries.clone() {
        let what = probe_part(dev, e.first_lba, e.sectors());
        lines.push(format!(
            "  slot {:<3} {:<5} lba {}..{} {} MiB {:?}: {}",
            slot,
            gpt::type_name(&e.type_guid),
            e.first_lba,
            e.last_lba,
            amber_core::sectors_mib(e.sectors()),
            e.name_string(),
            what
        ));
    }
    (rep, lines)
}

/// What [`recover`] did.
#[derive(Clone, Debug)]
pub struct Recovered {
    pub direction: Direction,
    pub writes: Vec<SectorWrite>,
    pub written: bool,
    pub lines: Vec<String>,
}

/// Restore the lost GPT copy from the good one (dry run: the exact writes only).
pub fn recover(dev: &mut dyn Block, mode: Mode, max_scan: u64) -> Result<Recovered, String> {
    let r = recover::plan_restore(dev, max_scan).map_err(|e| e.to_string())?;
    let mut lines = vec![format!("recover: {}", r.direction.tag())];
    lines.extend(r.notes.iter().cloned());
    for w in &r.writes {
        lines.push(format!("  {w}"));
    }
    if mode == Mode::DryRun || r.writes.is_empty() {
        return Ok(Recovered { direction: r.direction, writes: r.writes, written: false, lines });
    }
    plan_apply::commit(&r.writes, dev).map_err(|e| e.to_string())?;
    let after = verify::verify(dev);
    lines.push(format!("after: {}", after.lines().last().cloned().unwrap_or_default()));
    if !after.ok() {
        return Err(format!("recover wrote but the table still fails: {}", after.first_failure()));
    }
    Ok(Recovered { direction: r.direction, writes: r.writes, written: true, lines })
}

/// Sector size, re-exported for callers sizing images.
pub const SECTOR_BYTES: u64 = SECTOR as u64;
