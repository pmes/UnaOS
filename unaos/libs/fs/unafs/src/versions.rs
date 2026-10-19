// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! CHARTER: Kernel — fs-core
//!
//! FILEVERSIONS (rmbp-ledger B714; BP-bar-14, research B §6: an overwrite is never the only copy) — the
//! file system keeps the previous contents of a file when a write replaces them. Local, no cloud, no app
//! support: the version is the OLD EXTENT LIST, kept instead of reclaimed.
//!
//! * **The record** (`una:versions`, unafs-records.md §R17): a hand-packed trailer, LAST in the inode
//!   block (after the meta trailer and any spill trailer), magic `UNAFSVR1`, inline or extent-backed. Its
//!   payload is a RING on a file inode and the volume's LEDGER on the reclaim-queue inode (id 4). The
//!   §R6 record and the superblock stamp do not change: a reader built before this module stops before
//!   the trailer and mounts, reads and writes the volume as it always did.
//! * **The ring**: at most [`RING_MAX`] entries `{seq, mtime, size, kept, extents}`, oldest first. A
//!   version holds ONE reference on every block its extent list covers (taken before the write releases
//!   the live file's references), so a block shared by the live file and N versions counts N + 1 — the
//!   refcount map, fsck's walk and a snapshot's pin all agree. `kept` is what the version COSTS: the
//!   blocks the overwrite released from the live file (the blocks it alone kept alive when captured).
//! * **The bound**: the ledger rows `{inode, oldest_seq, count, kept}` sum every ring's `kept`; past
//!   [`BOUND_PCT`] % of the volume's blocks the OLDEST version anywhere (lowest `seq`, a volume-monotone
//!   counter the ledger carries) is dropped and its references released, until the sum fits.
//! * **What makes one**: `write_data` over bytes below the old size, and `truncate_data` shrinking — on a
//!   `File` inode, while [`UnaFS::set_versioning`] is on. Appends make none. `restore_version` makes one
//!   of the current contents (its references MOVE to the version; the live file takes new references on
//!   the restored blocks — no data is copied). `unlink` releases every version with the inode; a
//!   Trash move is a rename, so the versions travel with the file.
//! * **The flag** (VERSIONSCOMPAT SR154, §R12): the first version kept sets the root record's ROCOMPAT bit
//!   `versions` ([`crate::root::ROOT_FLAG_VERSIONS`]); a reader that does not know a ROCOMPAT bit mounts
//!   read-only and refuses `fsck --repair` — the repair that would free every block only a ring names.

use super::*;
use crate::inode::EXTENT_ENC_LEN;

/// Versions one file keeps (the oldest is dropped past it).
pub const RING_MAX: usize = 8;
/// The volume bound: versions together keep at most this percentage of the volume's blocks.
pub const BOUND_PCT: u64 = 10;
/// The ring payload's tag (a file inode's trailer).
const RING_TAG: [u8; 4] = *b"RING";
/// The ledger payload's tag (the reclaim-queue inode's trailer).
const LEDGER_TAG: [u8; 4] = *b"LDGR";

/// One kept version of a file, as stored in its ring.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionEntry {
    /// Volume-monotone capture number (the bound drops the lowest first).
    pub seq: u64,
    /// The contents' modification time (unix seconds) when they were replaced.
    pub mtime: u64,
    /// The contents' byte size.
    pub size: u64,
    /// Blocks this version cost when captured (released from the live file).
    pub kept: u64,
    /// The contents' extents (one reference held on every block they cover).
    pub extents: ExtentList,
}

/// A version as the API reports it — `n` is its index for [`UnaFS::read_version`] and
/// [`UnaFS::restore_version`] (0 = the newest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionInfo {
    pub n: usize,
    pub seq: u64,
    pub mtime: u64,
    pub size: u64,
    pub kept_blocks: u64,
}

/// One ledger row: a file's ring summarised for the volume bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerRow {
    pub inode: u64,
    pub oldest_seq: u64,
    pub count: u64,
    pub kept: u64,
}

/// The volume's version ledger (the reclaim-queue inode's trailer payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    /// The next capture's `seq`.
    pub next_seq: u64,
    /// One row per file with versions, ascending by inode.
    pub rows: Vec<LedgerRow>,
}

impl Default for Ledger {
    fn default() -> Self {
        Ledger { next_seq: 1, rows: Vec::new() }
    }
}

impl Ledger {
    /// The blocks every ring together keeps.
    pub fn kept(&self) -> u64 {
        self.rows.iter().fold(0u64, |a, r| a.saturating_add(r.kept))
    }

    /// Re-summarise `inode`'s row from its ring (removed when the ring is empty).
    fn set_row(&mut self, inode: u64, ring: &[VersionEntry]) {
        let at = self.rows.binary_search_by_key(&inode, |r| r.inode);
        match (ring.first(), at) {
            (None, Ok(i)) => {
                self.rows.remove(i);
            }
            (None, Err(_)) => {}
            (Some(first), at) => {
                let row = LedgerRow { inode, oldest_seq: first.seq, count: ring.len() as u64, kept: ring_kept(ring) };
                match at {
                    Ok(i) => self.rows[i] = row,
                    Err(i) => self.rows.insert(i, row),
                }
            }
        }
    }
}

/// The census fsck takes off the inodes themselves (the ledger is checked against it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionCensus {
    /// Versions kept, every file together.
    pub versions: u64,
    /// Blocks they cost (the rings' `kept` summed).
    pub kept: u64,
    /// The ledger rows the rings imply.
    pub rows: Vec<LedgerRow>,
    /// The highest capture number any ring holds.
    pub newest_seq: u64,
}

/// The pre-write contents a capture holds (references already taken).
pub(crate) struct Held {
    chunks: ExtentList,
    size: u64,
    mtime: u64,
}

fn ring_kept(ring: &[VersionEntry]) -> u64 {
    ring.iter().fold(0u64, |a, e| a.saturating_add(e.kept))
}

fn rd_u64(b: &[u8], at: &mut usize) -> Result<u64, FileSystemError> {
    let end = at.checked_add(8).ok_or(FileSystemError::CorruptVolume("versions payload"))?;
    let s = b.get(*at..end).ok_or(FileSystemError::CorruptVolume("versions payload truncated"))?;
    *at = end;
    Ok(u64::from_le_bytes(s.try_into().unwrap()))
}

/// Encode a ring (oldest first). An empty ring encodes to nothing (no trailer).
pub fn encode_ring(ring: &[VersionEntry]) -> Vec<u8> {
    if ring.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend_from_slice(&RING_TAG);
    out.extend_from_slice(&(ring.len() as u32).to_le_bytes());
    for e in ring {
        for v in [e.seq, e.mtime, e.size, e.kept, e.extents.len() as u64] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for x in &e.extents {
            for v in [x.logical_offset, x.physical_block, x.length] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    out
}

/// Decode a file's ring. Empty bytes = no versions; a LEDGER payload is not a ring (empty). Bounded: the
/// count against [`RING_MAX`], every extent count against the bytes left.
pub fn decode_ring(b: &[u8]) -> Result<Vec<VersionEntry>, FileSystemError> {
    if b.is_empty() || b.get(..4) == Some(&LEDGER_TAG[..]) {
        return Ok(Vec::new());
    }
    if b.len() < 8 || b[..4] != RING_TAG {
        return Err(FileSystemError::CorruptVolume("versions ring tag"));
    }
    let count = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    if count > RING_MAX {
        return Err(FileSystemError::CorruptVolume("versions ring count"));
    }
    let mut at = 8usize;
    let mut ring = Vec::with_capacity(count);
    for _ in 0..count {
        let (seq, mtime, size, kept, n) =
            (rd_u64(b, &mut at)?, rd_u64(b, &mut at)?, rd_u64(b, &mut at)?, rd_u64(b, &mut at)?, rd_u64(b, &mut at)?);
        if n > ((b.len() - at) / EXTENT_ENC_LEN) as u64 {
            return Err(FileSystemError::CorruptVolume("versions extent count"));
        }
        let mut extents = Vec::with_capacity(n as usize);
        for _ in 0..n {
            extents.push(Extent {
                logical_offset: rd_u64(b, &mut at)?,
                physical_block: rd_u64(b, &mut at)?,
                length: rd_u64(b, &mut at)?,
            });
        }
        ring.push(VersionEntry { seq, mtime, size, kept, extents });
    }
    Ok(ring)
}

/// Encode the ledger.
pub fn encode_ledger(l: &Ledger) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + l.rows.len() * 32);
    out.extend_from_slice(&LEDGER_TAG);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&l.next_seq.to_le_bytes());
    out.extend_from_slice(&(l.rows.len() as u64).to_le_bytes());
    for r in &l.rows {
        for v in [r.inode, r.oldest_seq, r.count, r.kept] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Decode the ledger (empty bytes = a volume that never kept a version).
pub fn decode_ledger(b: &[u8]) -> Result<Ledger, FileSystemError> {
    if b.is_empty() {
        return Ok(Ledger::default());
    }
    if b.len() < 8 || b[..4] != LEDGER_TAG {
        return Err(FileSystemError::CorruptVolume("versions ledger tag"));
    }
    let mut at = 8usize;
    let next_seq = rd_u64(b, &mut at)?;
    let n = rd_u64(b, &mut at)?;
    if n > ((b.len() - at) / 32) as u64 {
        return Err(FileSystemError::CorruptVolume("versions ledger count"));
    }
    let mut rows = Vec::with_capacity(n as usize);
    for _ in 0..n {
        rows.push(LedgerRow {
            inode: rd_u64(b, &mut at)?,
            oldest_seq: rd_u64(b, &mut at)?,
            count: rd_u64(b, &mut at)?,
            kept: rd_u64(b, &mut at)?,
        });
    }
    rows.sort_by_key(|r| r.inode);
    Ok(Ledger { next_seq: next_seq.max(1), rows })
}

/// Every extent a ring's versions reference (one entry per version: a block two versions share is listed
/// twice — it carries two references). Empty for a ledger payload or none.
pub fn ring_extents(payload: &[u8]) -> Result<Vec<Extent>, FileSystemError> {
    Ok(decode_ring(payload)?.into_iter().flat_map(|e| e.extents).collect())
}

/// The distinct blocks an extent list covers, bounded to the volume.
fn block_set(extents: &ExtentList, block_count: u64) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    for e in extents {
        for i in 0..e.length.div_ceil(BLOCK_SIZE) {
            match e.physical_block.checked_add(i) {
                Some(b) if b < block_count => {
                    out.insert(b);
                }
                _ => break,
            }
        }
    }
    out
}

impl<D: BlockDevice> UnaFS<D> {
    /// FILEVERSIONS (B714): keep a version when a write replaces a file's data (on), or reclaim the old
    /// extents at once as before (off — the default of a fresh mount). Existing versions are kept,
    /// listed, restored and released with their file either way.
    pub fn set_versioning(&mut self, on: bool) {
        self.versioning = on;
    }

    /// FILEVERSIONS (B714): whether writes keep versions.
    pub fn versioning(&self) -> bool {
        self.versioning
    }

    /// FILEVERSIONS (B714): the volume bound in blocks ([`BOUND_PCT`] % of the volume).
    pub fn version_bound_blocks(&self) -> u64 {
        self.superblock.block_count.saturating_mul(BOUND_PCT) / 100
    }

    /// FILEVERSIONS (B714): the volume's version ledger.
    pub fn version_ledger(&mut self) -> Result<Ledger, FileSystemError> {
        let inode = self.read_inode(RECLAIM_INODE_ID)?;
        decode_ledger(&inode.versions)
    }

    fn put_ledger(&mut self, l: &Ledger) -> Result<(), FileSystemError> {
        let mut inode = self.read_inode(RECLAIM_INODE_ID)?;
        let bytes = encode_ledger(l);
        if inode.versions == bytes {
            return Ok(());
        }
        inode.versions = bytes;
        self.write_inode(&inode)
    }

    /// FILEVERSIONS (B714): a file's versions, NEWEST first (`n` = 0 is the contents the last replacing
    /// write overwrote).
    pub fn versions(&mut self, inode_id: u64) -> Result<Vec<VersionInfo>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        let ring = decode_ring(&inode.versions)?;
        Ok(ring
            .iter()
            .rev()
            .enumerate()
            .map(|(n, e)| VersionInfo { n, seq: e.seq, mtime: e.mtime, size: e.size, kept_blocks: e.kept })
            .collect())
    }

    /// FILEVERSIONS (B714): read `length` bytes at `offset` of version `n` (0 = newest) — the same bounded
    /// extent reader a live read uses. `NotFound` past the ring.
    pub fn read_version(&mut self, inode_id: u64, n: usize, offset: u64, length: u64) -> Result<Vec<u8>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        let ring = decode_ring(&inode.versions)?;
        let i = ring.len().checked_sub(1).and_then(|l| l.checked_sub(n)).ok_or(FileSystemError::NotFound)?; // n = usize::MAX is NotFound, never an overflow
        let e = &ring[i];
        self.read_from_extents(&e.extents, offset, length, e.size)
    }

    /// FILEVERSIONS (B714): make version `n` (0 = newest) the file's contents again. Restoring is itself a
    /// write that keeps a version: the current contents' references MOVE to a new newest version, and the
    /// live file takes a fresh reference on every block of the restored one — no data block is copied or
    /// written; the inode is rewritten (mtime = now). The restored version stays in the ring. One
    /// transaction. `NotFound` past the ring or on a non-file.
    pub fn restore_version(&mut self, inode_id: u64, n: usize) -> Result<(), FileSystemError> {
        let mut inode = self.read_inode(inode_id)?;
        if inode.kind != FileKind::File {
            return Err(FileSystemError::NotFound);
        }
        let ring = decode_ring(&inode.versions)?;
        let i = ring.len().checked_sub(1).and_then(|l| l.checked_sub(n)).ok_or(FileSystemError::NotFound)?; // n = usize::MAX is NotFound, never an overflow
        let chosen = ring[i].clone();
        let bc = self.superblock.block_count;
        self.version_refs_intact(&ring, &inode.chunks, &chosen.extents)?;
        self.incref_extents(&chosen.extents);
        let released = block_set(&inode.chunks, bc).difference(&block_set(&chosen.extents, bc)).count() as u64;
        let held = Held { chunks: core::mem::take(&mut inode.chunks), size: inode.size, mtime: inode.mtime };
        if held.size > 0 || !held.chunks.is_empty() {
            self.version_keep(&mut inode, held, released)?;
        }
        inode.chunks = chosen.extents;
        inode.size = chosen.size;
        stamp_data(&mut inode);
        self.write_inode(&inode)?;
        self.versions_bound()?;
        self.maybe_commit()
    }

    /// The restore's guard (the stopgap for a volume a pre-B714 `fsck --repair` touched: that build counts no
    /// ring reference, so it frees blocks only a version still names, and a later restore would point the
    /// live file at blocks the allocator may have handed out again). Every block the restored version names
    /// must still carry at least the references this inode's ring and live extents hold on it; one that
    /// carries fewer refuses the restore (`CorruptVolume`) and nothing changes. A freed block that was
    /// ALREADY reallocated carries the new owner's reference and can pass — only a record an older repair
    /// refuses to rewrite (a ROCOMPAT bit, or a stamp) closes that; see the FILEVERSIONS ledger row.
    fn version_refs_intact(&mut self, ring: &[VersionEntry], live: &ExtentList, chosen: &ExtentList) -> Result<(), FileSystemError> {
        let bc = self.superblock.block_count;
        let mut need: alloc::collections::BTreeMap<u64, u32> = alloc::collections::BTreeMap::new();
        for list in ring.iter().map(|e| &e.extents).chain(core::iter::once(live)) {
            for extent in list {
                for i in 0..extent.length.div_ceil(BLOCK_SIZE) {
                    match extent.physical_block.checked_add(i) {
                        Some(b) if b < bc => *need.entry(b).or_insert(0) += 1,
                        _ => break,
                    }
                }
            }
        }
        for b in block_set(chosen, bc) {
            let want = need.get(&b).copied().unwrap_or(1);
            if self.refmap.refcount(&mut self.device, b) < want {
                return Err(FileSystemError::CorruptVolume("a version's block lost its references (an older fsck repair?): restore refused"));
            }
        }
        Ok(())
    }

    /// Take one reference on every block an extent list covers (bounded like [`decref_extents`]).
    pub(crate) fn incref_extents(&mut self, extents: &ExtentList) {
        for extent in extents {
            for i in 0..extent.length.div_ceil(BLOCK_SIZE) {
                let block = match extent.physical_block.checked_add(i) {
                    Some(b) if b < self.superblock.block_count => b,
                    _ => break,
                };
                self.refmap.incref(&mut self.device, block);
            }
        }
    }

    /// Before a replacing write releases anything: the version's references on the current contents.
    pub(crate) fn version_hold(&mut self, inode: &Inode) -> Held {
        self.incref_extents(&inode.chunks);
        Held { chunks: inode.chunks.clone(), size: inode.size, mtime: inode.mtime }
    }

    /// Push `held` as `inode`'s newest version (it cost `released` blocks), drop the ring's oldest past
    /// [`RING_MAX`], and re-summarise the inode's ledger row. The caller writes `inode`, then runs
    /// [`versions_bound`](Self::versions_bound).
    pub(crate) fn version_keep(&mut self, inode: &mut Inode, held: Held, released: u64) -> Result<(), FileSystemError> {
        let mut ledger = self.version_ledger()?;
        let seq = ledger.next_seq;
        ledger.next_seq = seq.saturating_add(1);
        let mut ring = decode_ring(&inode.versions)?;
        ring.push(VersionEntry { seq, mtime: held.mtime, size: held.size, kept: released, extents: held.chunks });
        while ring.len() > RING_MAX {
            let gone = ring.remove(0);
            self.decref_extents(&gone.extents);
        }
        inode.versions = encode_ring(&ring);
        ledger.set_row(inode.id, &ring);
        // VERSIONSCOMPAT (SR154): the volume now holds references only a ring names — the root says so
        // (ROCOMPAT `versions`), so a reader that cannot see rings mounts it read-only and never repairs it.
        self.mark_rocompat(crate::root::ROOT_FLAG_VERSIONS);
        self.put_ledger(&ledger)
    }

    /// The volume bound: while the rings together keep more than [`version_bound_blocks`](Self::version_bound_blocks),
    /// drop the OLDEST version anywhere (lowest `seq`) and release its references.
    pub(crate) fn versions_bound(&mut self) -> Result<(), FileSystemError> {
        let bound = self.version_bound_blocks();
        let mut ledger = self.version_ledger()?;
        let mut changed = false;
        while ledger.kept() > bound && self.drop_oldest_in(&mut ledger)? {
            changed = true;
        }
        if changed {
            self.put_ledger(&ledger)?;
        }
        Ok(())
    }

    /// Drop the OLDEST version anywhere (lowest `seq`) out of `ledger` and release its references (a stale
    /// row whose inode is gone is just removed). `false` = the ledger has no rows. The caller puts the ledger.
    fn drop_oldest_in(&mut self, ledger: &mut Ledger) -> Result<bool, FileSystemError> {
        let Some(pos) = (0..ledger.rows.len()).min_by_key(|&i| (ledger.rows[i].oldest_seq, ledger.rows[i].inode)) else {
            return Ok(false);
        };
        let id = ledger.rows[pos].inode;
        let mut inode = match self.read_inode(id) {
            Ok(i) => i,
            Err(FileSystemError::NotFound) => {
                ledger.rows.remove(pos);
                return Ok(true);
            }
            Err(e) => return Err(e),
        };
        let mut ring = decode_ring(&inode.versions)?;
        if !ring.is_empty() {
            let gone = ring.remove(0);
            self.decref_extents(&gone.extents);
            inode.versions = encode_ring(&ring);
            self.write_inode(&inode)?;
        }
        ledger.set_row(id, &ring);
        Ok(true)
    }

    /// A full volume's room (`write_data`/`truncate_data` on `NoSpace`): drop the oldest version anywhere, one
    /// row's oldest entry, in the open transaction. `false` = no version is kept anywhere.
    pub(crate) fn version_drop_oldest(&mut self) -> Result<bool, FileSystemError> {
        let mut ledger = self.version_ledger()?;
        let dropped = self.drop_oldest_in(&mut ledger)?;
        if dropped {
            self.put_ledger(&ledger)?;
        }
        Ok(dropped)
    }

    /// `unlink`: release every version of `inode` (their references) and its ledger row. The ring's own
    /// storage goes with the inode block.
    pub(crate) fn versions_release(&mut self, inode: &Inode) -> Result<(), FileSystemError> {
        let ring = decode_ring(&inode.versions)?;
        if ring.is_empty() {
            return Ok(());
        }
        for e in &ring {
            self.decref_extents(&e.extents);
        }
        let mut ledger = self.version_ledger()?;
        ledger.set_row(inode.id, &[]);
        self.put_ledger(&ledger)
    }

    /// FILEVERSIONS (B714): count the versions off the inodes themselves (fsck) — every allocated inode's
    /// ring, and the ledger rows those rings imply.
    pub fn version_census(&mut self) -> Result<VersionCensus, FileSystemError> {
        let mut c = VersionCensus::default();
        let ids: Vec<u64> = (1..self.imap.len() as u64).filter(|&id| self.imap[id as usize] != 0).collect();
        for id in ids {
            if id == RECLAIM_INODE_ID {
                continue;
            }
            let inode = self.read_inode(id)?;
            let ring = decode_ring(&inode.versions)?;
            if let Some(first) = ring.first() {
                c.versions += ring.len() as u64;
                c.kept = c.kept.saturating_add(ring_kept(&ring));
                c.rows.push(LedgerRow { inode: id, oldest_seq: first.seq, count: ring.len() as u64, kept: ring_kept(&ring) });
                c.newest_seq = c.newest_seq.max(ring.iter().map(|e| e.seq).max().unwrap_or(0));
            }
        }
        Ok(c)
    }

    /// fsck repair: rewrite the ledger's rows to what the rings say (an older build that rewrote the
    /// reclaim-queue inode dropped the trailer). The capture counter only moves forward.
    pub(crate) fn version_ledger_rebuild(&mut self, census: &VersionCensus) -> Result<(), FileSystemError> {
        let mut l = self.version_ledger().unwrap_or_default();
        l.next_seq = l.next_seq.max(census.newest_seq.saturating_add(1));
        l.rows = census.rows.clone();
        self.put_ledger(&l)
    }
}
