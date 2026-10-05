// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! `unafs dump --records` (UNAFSCODEC M4): every record of a volume, field by
//! field, walked from the byte-level spec
//! (`docs/dev/OS/09_FILESYSTEM/unafs-records.md`, §R).
//!
//! The walker reads raw blocks and steps through each record with the §R1
//! primitives (`unafs::codec::Reader`), printing one line per field:
//!
//! ```text
//! <record> @<offset> <Type.field> <type> = <value>
//! ```
//!
//! It does NOT call the library's record decoders — it is a second reading
//! of the same spec, so a test can hold the two against each other
//! (`tests/dump_records.rs`). It never mounts and never writes: the inode
//! map is walked from the root record (§R13) and object data is read through
//! the extents (§R5) — so the dump shows exactly what is on the medium, even
//! a volume whose mount would first drain a reclaim queue.

use anyhow::{Result, anyhow, bail};
use std::fmt::Write as _;
use unafs::codec::{MAX_RECORD_BYTES, Reader};
use unafs::{BLOCK_SIZE, BlockDevice, FileDevice};

const BS: usize = BLOCK_SIZE as usize;

/// A field-printing cursor over one record.
struct Rec<'a, 'o> {
    r: Reader<'a>,
    tag: String,
    out: &'o mut String,
}

impl<'a, 'o> Rec<'a, 'o> {
    fn new(bytes: &'a [u8], tag: impl Into<String>, out: &'o mut String) -> Self {
        Self { r: Reader::new(bytes, MAX_RECORD_BYTES, "dump"), tag: tag.into(), out }
    }
    fn line(&mut self, at: usize, field: &str, ty: &str, value: impl std::fmt::Display) {
        let _ = writeln!(self.out, "{} @{} {} {} = {}", self.tag, at, field, ty, value);
    }
    fn u32(&mut self, field: &'static str) -> Result<u32> {
        let at = self.r.position();
        let v = self.r.u32(field)?;
        self.line(at, field, "u32", v);
        Ok(v)
    }
    fn u64(&mut self, field: &'static str) -> Result<u64> {
        let at = self.r.position();
        let v = self.r.u64(field)?;
        self.line(at, field, "u64", v);
        Ok(v)
    }
    fn len(&mut self, field: &'static str, min: usize) -> Result<usize> {
        let at = self.r.position();
        let n = self.r.len(field, min)?;
        self.line(at, field, "len", n);
        Ok(n)
    }
    fn string(&mut self, field: &'static str) -> Result<String> {
        let at = self.r.position();
        let s = self.r.string(field)?;
        self.line(at, field, "String", format!("{s:?}"));
        Ok(s)
    }
    fn kind(&mut self, field: &'static str) -> Result<u32> {
        let at = self.r.position();
        let v = self.r.variant(field, 4)?;
        let name = ["File", "Directory", "Symlink", "System"][v as usize];
        self.line(at, field, "FileKind", format!("{v} {name}"));
        Ok(v)
    }
    fn extent(&mut self, base: &'static str) -> Result<(u64, u64, u64)> {
        let at = self.r.position();
        let lo = self.r.u64("Extent.logical_offset")?;
        let pb = self.r.u64("Extent.physical_block")?;
        let ln = self.r.u64("Extent.length")?;
        self.line(at, base, "Extent", format!("logical_offset={lo} physical_block={pb} length={ln}"));
        Ok((lo, pb, ln))
    }
    fn extents(&mut self, field: &'static str, elem: &'static str) -> Result<Vec<(u64, u64, u64)>> {
        let n = self.len(field, 24)?;
        (0..n).map(|_| self.extent(elem)).collect()
    }
    /// §R5 AttributeValue.
    fn attr(&mut self, field: &'static str) -> Result<String> {
        let at = self.r.position();
        let tag = self.r.variant("AttributeValue.tag", 5)?;
        let v = match tag {
            0 => format!("Int({})", self.r.i64("AttributeValue.Int")?),
            1 => format!("Float({:?})", self.r.f64("AttributeValue.Float")?),
            2 => format!("String({:?})", self.r.string("AttributeValue.String")?),
            3 => format!("Blob({:?})", self.r.bytes("AttributeValue.Blob")?),
            _ => {
                let n = self.r.len("AttributeValue.Vector", 4)?;
                let v: Vec<f32> = (0..n)
                    .map(|_| self.r.f32("AttributeValue.Vector[]"))
                    .collect::<Result<_, _>>()?;
                format!("Vector({v:?})")
            }
        };
        self.line(at, field, "AttributeValue", &v);
        Ok(v)
    }
    fn pos(&self) -> usize {
        self.r.position()
    }
}

/// Bytes of an extent-mapped object (spec §R5: an extent maps
/// `[logical_offset, +length)` onto `physical_block`'s bytes).
fn read_extents(dev: &mut FileDevice, extents: &[(u64, u64, u64)]) -> Result<Vec<u8>> {
    let total = extents
        .iter()
        .map(|&(lo, _, ln)| lo.checked_add(ln).ok_or_else(|| anyhow!("extent overflows")))
        .try_fold(0u64, |m, e| e.map(|e| m.max(e)))?;
    if total as usize > MAX_RECORD_BYTES {
        bail!("extent-mapped record of {total} B exceeds the record budget");
    }
    let mut out = vec![0u8; total as usize];
    let mut block = vec![0u8; BS];
    for &(lo, pb, ln) in extents {
        let mut done = 0u64;
        while done < ln {
            dev.read_block(pb + done / BLOCK_SIZE, &mut block)?;
            let n = (ln - done).min(BLOCK_SIZE) as usize;
            let o = (lo + done) as usize;
            out[o..o + n].copy_from_slice(&block[..n]);
            done += n as u64;
        }
    }
    Ok(out)
}

fn dump_superblock(b: &[u8], out: &mut String) -> Result<u32> {
    let _ = writeln!(out, "# §R2 superblock (block 0)");
    let mut r = Rec::new(b, "superblock", out);
    let at = r.pos();
    let magic: [u8; 5] = r.r.array("Superblock.magic")?;
    r.line(at, "Superblock.magic", "[u8;5]", format!("{:?}", String::from_utf8_lossy(&magic)));
    let version = r.u32("Superblock.version")?;
    r.u32("Superblock.block_size")?;
    r.u64("Superblock.block_count")?;
    r.u64("Superblock.root_inode")?;
    r.u64("Superblock.catalog_inode")?;
    Ok(version)
}

/// §R12, both slots. Returns the active slot's (next_inode, imap_block,
/// imap_leaves, flags).
fn dump_roots(b: &[u8], out: &mut String) -> Result<(u64, u64, u64, u64)> {
    let mut best: Option<(u64, (u64, u64, u64, u64), char)> = None;
    for (slot, s) in [('A', &b[0..512]), ('B', &b[512..1024])] {
        let _ = writeln!(out, "# §R12 root record slot {slot} (block 1 sector {})", if slot == 'A' { 0 } else { 1 });
        let tag = format!("root{slot}");
        let rd = |o: usize| u64::from_le_bytes(s[o..o + 8].try_into().unwrap());
        let valid = s[0..8] == *b"UNAFSRT1"
            && unafs::hash::hash_bytes(&s[0..72]) == rd(72)
            && rd(8) != 0;
        let _ = writeln!(out, "{tag} @0 RootRecord.magic [u8;8] = {:?}", String::from_utf8_lossy(&s[0..8]));
        for (o, f) in [
            (8, "generation"),
            (16, "imap_block"),
            (24, "imap_leaves"),
            (32, "next_inode"),
            (40, "refmap_block"),
            (48, "refmap_leaves"),
            (56, "free_blocks"),
            (64, "flags"),
            (72, "checksum"),
        ] {
            let _ = writeln!(out, "{tag} @{o} RootRecord.{f} u64 = {}", rd(o));
        }
        let _ = writeln!(out, "{tag} valid = {valid}");
        if valid && best.is_none_or(|(g, _, _)| rd(8) > g) {
            best = Some((rd(8), (rd(32), rd(16), rd(24), rd(64)), slot));
        }
    }
    let (g, next, slot) = best.ok_or_else(|| anyhow!("no valid root slot"))?;
    let _ = writeln!(out, "active slot {slot} generation {g}");
    Ok(next)
}

/// §R3/§R6–§R8: one inode block. Returns (kind, inline chunks, trailer index,
/// overflow_len, large attribute extents).
struct InodeView {
    kind: u32,
    size: u64,
    chunks: Vec<(u64, u64, u64)>,
    spill: Option<(Vec<(u64, u64, u64)>, u64)>,
    large: Vec<(String, Vec<(u64, u64, u64)>)>,
}

fn dump_inode_block(id: u64, pb: u64, b: &[u8], indexed: bool, out: &mut String) -> Result<InodeView> {
    let _ = writeln!(out, "# §R6 inode {id} (block {pb})");
    let tag = format!("inode#{id}");
    let mut r = Rec::new(b, tag.clone(), out);
    r.u64("Inode.id")?;
    let kind = r.kind("Inode.kind")?;
    let size = r.u64("Inode.size")?;
    let chunks = r.extents("Inode.chunks", "Inode.chunks[]")?;
    let n = r.len("Inode.attributes", 20)?;
    for _ in 0..n {
        r.string("Inode.attributes.key")?;
        r.attr("Inode.attributes.value")?;
    }
    let n = r.len("Inode.large_attributes", 16)?;
    let mut large = Vec::new();
    for _ in 0..n {
        let k = r.string("Inode.large_attributes.key")?;
        let e = r.extents("Inode.large_attributes.value", "Inode.large_attributes.value[]")?;
        large.push((k, e));
    }
    let mut off = r.pos();
    let _ = writeln!(r.out, "{tag} record_len = {off}");
    // §R7 meta trailer (v6+).
    let tag8 = |o: usize| b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()));
    if indexed && tag8(off) == Some(u64::from_le_bytes(*b"UNAFSMT1")) {
        let m = &b[off..];
        if m.len() < 42 {
            bail!("inode {id}: truncated meta trailer");
        }
        let rd = |o: usize| u64::from_le_bytes(m[o..o + 8].try_into().unwrap());
        let _ = writeln!(out, "# §R7 meta trailer");
        let _ = writeln!(out, "{tag}.meta @{off} Meta.magic [u8;8] = \"UNAFSMT1\"");
        for (o, f) in [(8, "parent"), (16, "ctime"), (24, "mtime"), (32, "atime")] {
            let _ = writeln!(out, "{tag}.meta @{} Meta.{f} u64 = {}", off + o, rd(o));
        }
        let nl = u16::from_le_bytes([m[40], m[41]]);
        let _ = writeln!(out, "{tag}.meta @{} Meta.name_len u16 = {nl}", off + 40);
        let mut len = 42;
        if nl != 0xFFFF {
            let n = nl as usize;
            if n > 255 || m.len() < 42 + n {
                bail!("inode {id}: meta name length out of range");
            }
            let name = std::str::from_utf8(&m[42..42 + n]).map_err(|_| anyhow!("inode {id}: meta name not UTF-8"))?;
            let _ = writeln!(out, "{tag}.meta @{} Meta.name String = {name:?}", off + 42);
            len += n;
        }
        off += len;
    }
    // §R8 spill trailer.
    let mut spill = None;
    if tag8(off) == Some(unafs::inode::INODE_SPILL_MAGIC) {
        let _ = writeln!(out, "# §R8 spill trailer");
        let mut t = Rec::new(&b[off..], format!("{tag}.spill"), out);
        t.u64("IndirectTrailer.magic")?;
        t.u64("IndirectTrailer.total_extents")?;
        let ol = t.u64("IndirectTrailer.overflow_len")?;
        let idx = t.extents("IndirectTrailer.index", "IndirectTrailer.index[]")?;
        spill = Some((idx, ol));
    }
    Ok(InodeView { kind, size, chunks, spill, large })
}

fn dump_list(bytes: &[u8], what: &str, tag: &str, out: &mut String) -> Result<()> {
    let mut r = Rec::new(bytes, tag, out);
    match what {
        "DirEntry" => {
            let n = r.len("Vec<DirEntry>.len", 20)?;
            for _ in 0..n {
                r.string("DirEntry.name")?;
                r.u64("DirEntry.inode_id")?;
                r.kind("DirEntry.kind")?;
            }
        }
        "CatalogEntry" => {
            let n = r.len("Vec<CatalogEntry>.len", 24)?;
            for _ in 0..n {
                r.u64("CatalogEntry.key_hash")?;
                r.u64("CatalogEntry.val_hash")?;
                r.u64("CatalogEntry.inode_id")?;
            }
        }
        "SnapshotEntry" => {
            let n = r.len("Vec<SnapshotEntry>.len", 48)?;
            for _ in 0..n {
                r.u64("SnapshotEntry.generation")?;
                r.u64("SnapshotEntry.imap_block")?;
                r.u64("SnapshotEntry.imap_leaves")?;
                r.string("SnapshotEntry.name")?;
                r.string("SnapshotEntry.creator")?;
                r.u64("SnapshotEntry.timestamp")?;
            }
        }
        "ReclaimEntry" => {
            let n = r.len("Vec<ReclaimEntry>.len", 16)?;
            for _ in 0..n {
                r.u64("ReclaimEntry.generation")?;
                let m = r.len("ReclaimEntry.blocks", 8)?;
                for _ in 0..m {
                    r.u64("ReclaimEntry.blocks[]")?;
                }
            }
        }
        "Extent" => {
            r.extents("Vec<Extent>.len", "Vec<Extent>[]")?;
        }
        "AttributeValue" => {
            r.attr("AttributeValue")?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// §R13: the inode map, walked from the root record. Returns (id, block)
/// for every allocated slot. `paged`: v7 nodes (255 × (block, sum, used),
/// hole = block 0, child sums checked); else one legacy block of raw leaf
/// pointers.
fn walk_imap(
    raw: &mut FileDevice,
    top: u64,
    leaves: u64,
    paged: bool,
    out: &mut String,
) -> Result<Vec<(u64, u64)>> {
    let mut slots = Vec::new();
    let mut block = vec![0u8; BS];
    let rd = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let read_leaf = |raw: &mut FileDevice, leaf: u64, pb: u64, sum: Option<u32>, slots: &mut Vec<(u64, u64)>, out: &mut String| -> Result<()> {
        let mut b = vec![0u8; BS];
        raw.read_block(pb, &mut b)?;
        if let Some(s) = sum {
            let ok = unafs::maptree::block_sum(&b) == s;
            if !ok {
                let _ = writeln!(out, "imap leaf {leaf} block {pb} checksum MISMATCH");
            }
        }
        for e in 0..512u64 {
            let v = rd(&b, (e * 8) as usize);
            if v != 0 {
                slots.push((leaf * 512 + e, v));
            }
        }
        Ok(())
    };
    if !paged {
        raw.read_block(top, &mut block)?;
        let _ = writeln!(out, "# §R13 inode map: legacy index block {top}, {leaves} leaves");
        let ptrs: Vec<u64> = (0..leaves.min(512)).map(|l| rd(&block, (l * 8) as usize)).collect();
        for (l, pb) in ptrs.into_iter().enumerate() {
            read_leaf(raw, l as u64, pb, None, &mut slots, out)?;
        }
        return Ok(slots);
    }
    let mut levels = 1;
    let mut cap = 255u64;
    while cap < leaves {
        levels += 1;
        cap = cap.saturating_mul(255);
    }
    let _ = writeln!(out, "# §R13 inode map: paged, top node {top}, {leaves} leaves, {levels} level(s)");
    // (block, level, first leaf, expected sum)
    let mut stack = vec![(top, levels, 0u64, None::<u32>)];
    while let Some((nb, level, first, sum)) = stack.pop() {
        raw.read_block(nb, &mut block)?;
        if &block[4080..4088] != b"UNAFSMN1" || unafs::hash::hash_bytes(&block[..4088]) != rd(&block, 4088) {
            bail!("imap node {nb}: bad magic or trailer checksum");
        }
        if let Some(s) = sum && unafs::maptree::block_sum(&block) != s {
            let _ = writeln!(out, "imap node {nb} checksum MISMATCH");
        }
        let span = 255u64.pow(level as u32 - 1);
        for e in 0..255usize {
            let o = e * 16;
            let cb = rd(&block, o);
            let cs = u32::from_le_bytes(block[o + 8..o + 12].try_into().unwrap());
            let child_first = first + e as u64 * span;
            if cb == 0 || child_first >= leaves {
                continue;
            }
            if level == 1 {
                read_leaf(raw, child_first, cb, Some(cs), &mut slots, out)?;
            } else {
                stack.push((cb, level - 1, child_first, Some(cs)));
            }
        }
    }
    slots.sort_unstable();
    Ok(slots)
}

/// The whole dump. Reads the image only (never mounts, never writes).
pub fn dump_records(img: &str) -> Result<String> {
    let mut out = String::new();
    let mut raw = FileDevice::open_read_only(img)?;
    let mut block = vec![0u8; BS];
    raw.read_block(0, &mut block)?;
    let version = dump_superblock(&block, &mut out)?;
    raw.read_block(1, &mut block)?;
    let (next_inode, imap_block, imap_leaves, flags) = dump_roots(&block, &mut out)?;
    let indexed = version >= unafs::superblock::VERSION_INDEXED;
    let paged = version >= unafs::superblock::VERSION_PAGED_MAPS || flags & unafs::root::ROOT_FLAG_MIGRATE != 0;
    let slots = walk_imap(&mut raw, imap_block, imap_leaves, paged, &mut out)?;

    for (id, pb) in slots {
        if id >= next_inode {
            let _ = writeln!(out, "imap slot {id} >= next_inode {next_inode} (ignored)");
            continue;
        }
        raw.read_block(pb, &mut block)?;
        let v = dump_inode_block(id, pb, &block, indexed, &mut out)?;
        let mut all = v.chunks.clone();
        if let Some((idx, ol)) = &v.spill {
            let bytes = read_extents(&mut raw, idx)?;
            let n = (*ol as usize).min(bytes.len());
            let _ = writeln!(out, "# §R8 overflow extent list of inode {id}");
            dump_list(&bytes[..n], "Extent", &format!("overflow#{id}"), &mut out)?;
            let mut r = Reader::new(&bytes[..n], MAX_RECORD_BYTES, "overflow");
            let m = r.len("Vec<Extent>.len", 24)?;
            for _ in 0..m {
                all.push((r.u64("lo")?, r.u64("pb")?, r.u64("len")?));
            }
        }
        for (k, e) in &v.large {
            let bytes = read_extents(&mut raw, e)?;
            let _ = writeln!(out, "# §R10 large attribute {k:?} of inode {id}");
            dump_list(&bytes, "AttributeValue", &format!("large#{id}:{k}"), &mut out)?;
        }
        // Object data that is itself a record (§R9–§R11).
        let what = match (v.kind, id) {
            (1, _) => Some(("DirEntry", "§R9 directory")),
            (3, 2) if indexed => Some(("CatalogRecord", "§R11 catalog record")),
            (3, 2) => Some(("CatalogEntry", "§R10 flat catalog")),
            (3, 3) => Some(("SnapshotEntry", "§R11 snapshot index")),
            (3, 4) => Some(("ReclaimEntry", "§R11 reclaim queue")),
            _ => None,
        };
        let Some((what, title)) = what else { continue };
        if v.size as usize > MAX_RECORD_BYTES {
            bail!("inode {id}: {title} of {} B exceeds the record budget", v.size);
        }
        let mut bytes = read_extents(&mut raw, &all)?;
        bytes.resize(v.size as usize, 0);
        let _ = writeln!(out, "# {title} (inode {id} data, {} B)", bytes.len());
        if bytes.is_empty() {
            continue;
        }
        if what == "CatalogRecord" {
            if bytes.len() >= 40 {
                let b = &bytes;
                let rd = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
                let _ = writeln!(out, "data#2 @0 CatalogRecord.magic [u8;8] = {:?}", String::from_utf8_lossy(&b[0..8]));
                for (o, f) in [(8, "eq_root"), (16, "ord_root"), (24, "entries"), (32, "checksum")] {
                    let _ = writeln!(out, "data#2 @{o} CatalogRecord.{f} u64 = {}", rd(o));
                }
            }
        } else {
            dump_list(&bytes, what, &format!("data#{id}"), &mut out)?;
        }
    }
    Ok(out)
}
