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

//! UNAFSMAP (B354): the PAGED MAP TREE — the on-disk shape both persistent
//! maps (the refcount map and the inode map) share from format v7 on, and the
//! in-RAM description of either map's tree in either shape.
//!
//! ## Shape
//!
//! A map is an array of fixed-size LEAF blocks (4096 B of raw entries: 1024
//! u32 refcounts, or 512 u64 inode-map slots) addressed by leaf index, under
//! a radix of INDEX NODES. Level 0 is the leaves; the node at level `L`,
//! index `i` has children `[i·F, i·F + F)` at level `L − 1`; the top level has
//! exactly one node, the one the root record names. The number of levels is a
//! pure function of the leaf count.
//!
//! * [`Shape::Paged`] (v7+): `F` = 255. An index node is 255 × 16 B entries
//!   `(block u64, sum u32, used u32)` followed by a trailer (`NODE_MAGIC`,
//!   FNV-1a of bytes `0..4088`). `sum` is [`block_sum`] of the child's 4096 B
//!   (so every leaf and node below the top is checksummed by its parent, and
//!   the top by its own trailer); `used` is the number of non-zero entries in
//!   the child's subtree. `block == 0` is a HOLE: an all-zero subtree that
//!   owns no block (a fresh 1 TiB volume's map is a handful of blocks).
//! * [`Shape::Legacy`] (v3–v6): `F` = 512, raw u64 child pointers, no sums, no
//!   holes, one level up to 512 leaves and two past that — byte-for-byte the
//!   layout every pre-v7 build reads.
//!
//! ## Copy-on-write commit, dirty nodes only
//!
//! The owner marks leaves DIRTY as their entries change. A commit asks
//! [`Tree::pending`] for every dirty leaf and every index node on a dirty
//! leaf's path that has not been given a fresh block yet, relocates each
//! ([`Tree::relocate`], which returns the block to release), writes the leaves
//! it owns the bytes of, then [`Tree::write_upper`] writes the relocated index
//! nodes bottom-up. Clean subtrees keep their blocks: a 1-block write on a
//! 1 TiB volume rewrites a few leaves and their paths, not 262,144 leaves.
//! Nothing reachable from the committed root is overwritten — the old root
//! stays valid until the root-sector flip, exactly the existing discipline.

use crate::fs::FileSystemError;
use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// Children per paged index node.
pub const PAGED_FANOUT: u64 = 255;
/// Children per legacy (v3–v6) index block.
pub const LEGACY_FANOUT: u64 = BLOCK_SIZE / 8;
/// Paged index-node trailer magic.
pub const NODE_MAGIC: [u8; 8] = *b"UNAFSMN1";
/// Bytes per paged index entry.
const ENTRY: usize = 16;
/// Offset of the paged trailer (magic, then the node's own FNV-1a).
const TRAILER: usize = PAGED_FANOUT as usize * ENTRY;
const _: () = assert!(TRAILER + 16 == BLOCK_SIZE as usize);

/// The 32-bit checksum a parent entry carries for its child's 4096 B.
pub fn block_sum(bytes: &[u8]) -> u32 {
    let h = crate::hash::hash_bytes(bytes);
    (h ^ (h >> 32)) as u32
}

/// Whether `buf` is a well-formed paged index node (magic + trailer sum).
/// Pre-v7 index blocks hold raw pointers bounded by the volume, so their
/// trailer bytes can never spell the magic — this is how a retained root's
/// inode map is told apart across a v6 → v7 migration.
pub fn is_paged_node(buf: &[u8]) -> bool {
    buf.len() == BLOCK_SIZE as usize
        && buf[TRAILER..TRAILER + 8] == NODE_MAGIC
        && u64::from_le_bytes(buf[TRAILER + 8..TRAILER + 16].try_into().unwrap())
            == crate::hash::hash_bytes(&buf[..TRAILER + 8])
}

/// One child pointer: a block (0 = hole), its checksum, its non-zero count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ptr {
    pub block: u64,
    pub sum: u32,
    pub used: u32,
}

/// Which on-disk shape a tree has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// v7+: 255-way checksummed nodes with holes.
    Paged,
    /// v3–v6: raw 512-way pointer blocks, one or two levels, no holes.
    Legacy,
}

impl Shape {
    /// Children per index node.
    pub fn fanout(self) -> u64 {
        match self {
            Shape::Paged => PAGED_FANOUT,
            Shape::Legacy => LEGACY_FANOUT,
        }
    }

    /// Index levels above `leaves` leaves (≥ 1: the top is always a node).
    pub fn levels(self, leaves: u64) -> usize {
        match self {
            Shape::Legacy => {
                if leaves <= LEGACY_FANOUT {
                    1
                } else {
                    2
                }
            }
            Shape::Paged => {
                let mut d = 1;
                let mut cap = PAGED_FANOUT;
                while cap < leaves {
                    d += 1;
                    cap = cap.saturating_mul(PAGED_FANOUT);
                }
                d
            }
        }
    }

    /// Nodes at `level` (0 = leaves) for `leaves` leaves.
    pub fn nodes_at(self, leaves: u64, level: usize) -> u64 {
        let mut n = leaves;
        for _ in 0..level {
            n = n.div_ceil(self.fanout());
        }
        n.max(1)
    }
}

fn try_vec<T: Clone>(n: u64, fill: T) -> Result<Vec<T>, StorageError> {
    let n = usize::try_from(n).map_err(|_| StorageError::AllocRefused(n))?;
    let mut v = Vec::new();
    v.try_reserve_exact(n)
        .map_err(|_| StorageError::AllocRefused((n * core::mem::size_of::<T>()) as u64))?;
    v.resize(n, fill);
    Ok(v)
}

fn rd64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// The in-RAM description of one map's tree: every leaf's and index node's
/// pointer (16 B per leaf — the leaves' CONTENTS are the owner's business),
/// plus the commit bookkeeping.
pub struct Tree {
    pub shape: Shape,
    /// Level 0: one pointer per leaf.
    pub leaves: Vec<Ptr>,
    /// `upper[k]` = the nodes of level `k + 1`; the last level has one node.
    pub upper: Vec<Vec<Ptr>>,
    /// Leaves whose contents changed since the last commit.
    pub dirty: BTreeSet<u64>,
    /// `(level, index)` relocated by the in-flight commit.
    moved: BTreeSet<(usize, u64)>,
    /// Index nodes to rewrite though no dirty leaf lies under them (a grow
    /// that added a level).
    stale: BTreeSet<(usize, u64)>,
    /// Rewrite every index node (the v6 → v7 migration builds them all).
    force_upper: bool,
}

impl Tree {
    /// A tree of `leaves` holes (format). A legacy tree has no holes: the
    /// caller marks every leaf dirty so the first commit materializes it.
    pub fn empty(shape: Shape, leaves: u64) -> Result<Self, StorageError> {
        let leaves = leaves.max(1);
        let mut upper = Vec::new();
        for level in 1..=shape.levels(leaves) {
            upper.push(try_vec(shape.nodes_at(leaves, level), Ptr::default())?);
        }
        Ok(Self {
            shape,
            leaves: try_vec(leaves, Ptr::default())?,
            upper,
            dirty: BTreeSet::new(),
            moved: BTreeSet::new(),
            stale: BTreeSet::new(),
            force_upper: false,
        })
    }

    /// Leaves in the tree.
    pub fn leaf_count(&self) -> u64 {
        self.leaves.len() as u64
    }

    /// Index levels.
    pub fn depth(&self) -> usize {
        self.upper.len()
    }

    /// The top node's block (what the root record names).
    pub fn top(&self) -> u64 {
        self.upper.last().map(|l| l[0].block).unwrap_or(0)
    }

    /// Sum of the leaves' non-zero counts.
    pub fn used_total(&self) -> u64 {
        self.leaves.iter().map(|p| p.used as u64).sum()
    }

    /// Every block the tree owns (non-hole leaves and nodes).
    pub fn blocks(&self) -> impl Iterator<Item = u64> + '_ {
        self.leaves
            .iter()
            .chain(self.upper.iter().flatten())
            .map(|p| p.block)
            .filter(|&b| b != 0)
    }

    /// Number of blocks the tree owns.
    pub fn block_total(&self) -> u64 {
        self.blocks().count() as u64
    }

    /// Grow to `leaves` leaves (new leaves are holes; never shrinks). A level
    /// added on top is marked stale so the next commit writes it; the old top
    /// stays node 0 of its level (both shapes keep their encoding there).
    pub fn resize(&mut self, leaves: u64) -> Result<(), StorageError> {
        if leaves <= self.leaf_count() {
            return Ok(());
        }
        let n = usize::try_from(leaves).map_err(|_| StorageError::AllocRefused(leaves))?;
        self.leaves
            .try_reserve_exact(n - self.leaves.len())
            .map_err(|_| StorageError::AllocRefused(leaves * 16))?;
        self.leaves.resize(n, Ptr::default());
        let old_depth = self.depth();
        let new_depth = self.shape.levels(leaves);
        for level in 1..=new_depth {
            let want = self.shape.nodes_at(leaves, level) as usize;
            if level > self.upper.len() {
                self.upper.push(try_vec(want as u64, Ptr::default())?);
            } else {
                let v = &mut self.upper[level - 1];
                if want > v.len() {
                    v.try_reserve_exact(want - v.len())
                        .map_err(|_| StorageError::AllocRefused(want as u64 * 16))?;
                    v.resize(want, Ptr::default());
                }
            }
        }
        for level in (old_depth + 1)..=new_depth {
            self.stale.insert((level, 0));
        }
        Ok(())
    }

    /// The v6 → v7 migration: re-shape a legacy tree as paged, keeping every
    /// leaf block (a legacy leaf IS a paged leaf — same raw entries; its sum
    /// was computed when the map was loaded). Returns the legacy index blocks
    /// to release; the next commit writes every paged node.
    pub fn convert_to_paged(&mut self) -> Result<Vec<u64>, StorageError> {
        let old: Vec<u64> = self
            .upper
            .iter()
            .flatten()
            .map(|p| p.block)
            .filter(|&b| b != 0)
            .collect();
        self.shape = Shape::Paged;
        let leaves = self.leaf_count();
        self.upper.clear();
        for level in 1..=Shape::Paged.levels(leaves) {
            self.upper
                .push(try_vec(Shape::Paged.nodes_at(leaves, level), Ptr::default())?);
        }
        self.force_upper = true;
        Ok(old)
    }

    fn ptr(&self, level: usize, idx: u64) -> Ptr {
        if level == 0 {
            self.leaves[idx as usize]
        } else {
            self.upper[level - 1][idx as usize]
        }
    }

    fn ptr_mut(&mut self, level: usize, idx: u64) -> &mut Ptr {
        if level == 0 {
            &mut self.leaves[idx as usize]
        } else {
            &mut self.upper[level - 1][idx as usize]
        }
    }

    /// Whether a leaf must own a block this commit (a paged leaf with no
    /// non-zero entry becomes a hole; a legacy leaf always has a block).
    fn leaf_wants_block(&self, idx: u64) -> bool {
        self.shape == Shape::Legacy || self.leaves[idx as usize].used > 0
    }

    /// The nodes the in-flight commit still has to relocate, leaves first:
    /// every dirty leaf not yet moved (or moved to a hole and non-empty
    /// since), then every index node on such a leaf's path (plus stale nodes,
    /// or all of them on a migration) not yet moved. The owner keeps the
    /// leaves' `used` current before asking.
    pub fn pending(&self) -> Vec<(usize, u64)> {
        let mut out = Vec::new();
        let mut paths: BTreeSet<(usize, u64)> = BTreeSet::new();
        let f = self.shape.fanout();
        for &leaf in &self.dirty {
            if leaf >= self.leaf_count() {
                continue;
            }
            let moved = self.moved.contains(&(0, leaf));
            if !moved || (self.leaves[leaf as usize].block == 0 && self.leaf_wants_block(leaf)) {
                out.push((0, leaf));
            }
            let mut i = leaf;
            for level in 1..=self.depth() {
                i /= f;
                paths.insert((level, i));
            }
        }
        for &s in &self.stale {
            paths.insert(s);
        }
        if self.force_upper {
            for level in 1..=self.depth() {
                for i in 0..self.upper[level - 1].len() as u64 {
                    paths.insert((level, i));
                }
            }
        }
        for n in paths {
            if !self.moved.contains(&n) {
                out.push(n);
            }
        }
        out
    }

    /// Whether `pending` node `(level, idx)` should get a block (vs a hole).
    pub fn wants_block(&self, level: usize, idx: u64) -> bool {
        level > 0 || self.leaf_wants_block(idx)
    }

    /// Give `(level, idx)` its new block (0 = hole) for this commit; returns
    /// the block it held (0 = none) for the caller to release.
    pub fn relocate(&mut self, level: usize, idx: u64, block: u64) -> u64 {
        self.moved.insert((level, idx));
        let p = self.ptr_mut(level, idx);
        let old = p.block;
        p.block = block;
        if block == 0 {
            p.sum = 0;
        }
        old
    }

    /// The leaves relocated to a real block this commit (the owner writes
    /// their bytes and records the sum with [`set_leaf_sum`](Self::set_leaf_sum)).
    pub fn moved_leaves(&self) -> Vec<(u64, u64)> {
        self.moved
            .iter()
            .filter(|(l, _)| *l == 0)
            .map(|&(_, i)| (i, self.leaves[i as usize].block))
            .filter(|&(_, b)| b != 0)
            .collect()
    }

    /// Whether `(level, idx)` was relocated by the in-flight commit.
    pub fn is_moved(&self, level: usize, idx: u64) -> bool {
        self.moved.contains(&(level, idx))
    }

    /// Record a written leaf's checksum.
    pub fn set_leaf_sum(&mut self, idx: u64, sum: u32) {
        self.leaves[idx as usize].sum = sum;
    }

    /// Encode node `(level, idx)` from its children's pointers.
    fn encode(&self, level: usize, idx: u64) -> Result<(Vec<u8>, u32), FileSystemError> {
        let f = self.shape.fanout();
        let children: &[Ptr] = if level == 1 { &self.leaves } else { &self.upper[level - 2] };
        let lo = (idx * f) as usize;
        let hi = core::cmp::min(children.len(), lo + f as usize);
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        let mut used = 0u64;
        for (j, c) in children[lo.min(hi)..hi].iter().enumerate() {
            used += c.used as u64;
            match self.shape {
                Shape::Paged => {
                    let o = j * ENTRY;
                    buf[o..o + 8].copy_from_slice(&c.block.to_le_bytes());
                    buf[o + 8..o + 12].copy_from_slice(&c.sum.to_le_bytes());
                    buf[o + 12..o + 16].copy_from_slice(&c.used.to_le_bytes());
                }
                Shape::Legacy => {
                    if c.block == 0 {
                        return Err(FileSystemError::CorruptVolume("legacy map node over a hole"));
                    }
                    buf[j * 8..j * 8 + 8].copy_from_slice(&c.block.to_le_bytes());
                }
            }
        }
        if self.shape == Shape::Paged {
            buf[TRAILER..TRAILER + 8].copy_from_slice(&NODE_MAGIC);
            let s = crate::hash::hash_bytes(&buf[..TRAILER + 8]);
            buf[TRAILER + 8..].copy_from_slice(&s.to_le_bytes());
        }
        let used = u32::try_from(used).map_err(|_| FileSystemError::CorruptVolume("map subtree count overflows"))?;
        Ok((buf, used))
    }

    /// Write every relocated index node, bottom-up (children's sums and
    /// counts are final before a parent is encoded). `write` puts one block.
    pub fn write_upper(
        &mut self,
        write: &mut dyn FnMut(u64, &[u8]) -> Result<(), FileSystemError>,
    ) -> Result<(), FileSystemError> {
        for level in 1..=self.depth() {
            let nodes: Vec<u64> = self
                .moved
                .iter()
                .filter(|(l, _)| *l == level)
                .map(|&(_, i)| i)
                .collect();
            for idx in nodes {
                let (buf, used) = self.encode(level, idx)?;
                let p = self.ptr(level, idx);
                if p.block == 0 {
                    return Err(FileSystemError::CorruptVolume("map node relocated to a hole"));
                }
                write(p.block, &buf)?;
                let sum = block_sum(&buf);
                let q = self.ptr_mut(level, idx);
                q.sum = sum;
                q.used = used;
            }
        }
        Ok(())
    }

    /// The commit landed (or was abandoned by a reload): forget its bookkeeping.
    pub fn finish_commit(&mut self) {
        self.dirty.clear();
        self.moved.clear();
        self.stale.clear();
        self.force_upper = false;
    }

    /// Load a tree's index nodes from disk (leaves are NOT read — their
    /// pointers, sums and counts come from the nodes; a legacy tree's sums
    /// and counts are zero until the owner reads its leaves). Untrusted
    /// input: every pointer is bounded by `block_count`, every paged node's
    /// trailer and parent sum are checked, every count by its subtree size.
    pub fn load<D: BlockDevice>(
        device: &mut D,
        shape: Shape,
        top: u64,
        leaves: u64,
        block_count: u64,
        leaf_entries: u64,
    ) -> Result<Self, FileSystemError> {
        if leaves == 0 {
            return Err(FileSystemError::CorruptVolume("map has no leaves"));
        }
        if top == 0 || top >= block_count {
            return Err(FileSystemError::CorruptVolume("map top node out of bounds"));
        }
        let mut t = Self::empty(shape, leaves)?;
        let depth = t.depth();
        let f = shape.fanout();
        t.upper[depth - 1][0] = Ptr { block: top, sum: 0, used: 0 };
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        // Top-down: each level's nodes fill the level below.
        for level in (1..=depth).rev() {
            let child_cap = leaf_entries.saturating_mul(f.saturating_pow(level as u32 - 1));
            for idx in 0..t.upper[level - 1].len() as u64 {
                let p = t.upper[level - 1][idx as usize];
                if p.block == 0 {
                    if shape == Shape::Legacy {
                        return Err(FileSystemError::CorruptVolume("legacy map node missing"));
                    }
                    continue;
                }
                device.read_block(p.block, &mut buf)?;
                if shape == Shape::Paged {
                    if !is_paged_node(&buf) {
                        return Err(FileSystemError::CorruptVolume("map node checksum mismatch"));
                    }
                    if level < depth && block_sum(&buf) != p.sum {
                        return Err(FileSystemError::CorruptVolume("map node sum mismatch"));
                    }
                }
                let n_children = if level == 1 { leaves } else { t.upper[level - 2].len() as u64 };
                let lo = idx * f;
                let hi = core::cmp::min(n_children, lo + f);
                let mut total = 0u64;
                for j in 0..f {
                    let c = match shape {
                        Shape::Paged => {
                            let o = j as usize * ENTRY;
                            Ptr { block: rd64(&buf, o), sum: rd32(&buf, o + 8), used: rd32(&buf, o + 12) }
                        }
                        Shape::Legacy => {
                            if j as usize * 8 >= BLOCK_SIZE as usize {
                                break;
                            }
                            Ptr { block: rd64(&buf, j as usize * 8), sum: 0, used: 0 }
                        }
                    };
                    let ci = lo + j;
                    if ci >= hi {
                        // Past the last child: a paged node's tail is zero
                        // (legacy tails were never defined — ignored).
                        if shape == Shape::Paged && c != Ptr::default() {
                            return Err(FileSystemError::CorruptVolume("map node tail not empty"));
                        }
                        continue;
                    }
                    if c.block >= block_count {
                        return Err(FileSystemError::CorruptVolume("map pointer out of bounds"));
                    }
                    if c.block == 0 {
                        if shape == Shape::Legacy {
                            return Err(FileSystemError::CorruptVolume("legacy map pointer is zero"));
                        }
                        if c.used != 0 || c.sum != 0 {
                            return Err(FileSystemError::CorruptVolume("map hole carries a count"));
                        }
                    }
                    if c.used as u64 > child_cap {
                        return Err(FileSystemError::CorruptVolume("map count exceeds its subtree"));
                    }
                    total += c.used as u64;
                    *t.ptr_mut(level - 1, ci) = c;
                }
                if shape == Shape::Paged && level < depth && total != p.used as u64 {
                    return Err(FileSystemError::CorruptVolume("map node count mismatch"));
                }
                if level == depth {
                    t.upper[depth - 1][0].used =
                        u32::try_from(total).map_err(|_| FileSystemError::CorruptVolume("map count overflows"))?;
                }
            }
        }
        Ok(t)
    }
}
