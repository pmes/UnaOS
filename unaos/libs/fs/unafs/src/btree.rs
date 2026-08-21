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

//! F3: the generic on-disk **B+tree** — ONE implementation, to be shared by the
//! attribute indexes (F4) and the directories (F6).
//!
//! This module is deliberately **standalone**: it knows about
//! [`BlockDevice`](crate::storage::BlockDevice) and
//! [`RefMap`](crate::refmap::RefMap) and nothing else about the file system.
//! Nothing in `fs.rs` consumes it yet — F4/F6 wire it into the catalog and the
//! directory paths.
//!
//! # The shape: btrfs-style separator keys
//!
//! A node is a slotted 4096 B block holding `count` entries, each a
//! `(key, value)` pair of byte strings.
//!
//! * In a **leaf**, the value is the caller's opaque payload.
//! * In an **internal** node, the value is an 8-byte little-endian child block
//!   id, and the key is that child's **lower bound** — i.e. the smallest key in
//!   the child's subtree. `n` keys therefore address exactly `n` children, and
//!   there is no ragged "n+1st pointer" to special-case. Descent picks the LAST
//!   entry whose key is `<=` the search key (or entry 0 when the search key is
//!   below the whole subtree).
//!
//! Because a node's separator in its parent is literally the child's first key,
//! every structural operation reduces to "replace this run of parent slots with
//! that list of `(first_key, block)` pairs" — split, merge, borrow and plain
//! rewrite are the same patch with different arity.
//!
//! # Copy-on-write: the path-copy argument
//!
//! **No node block is ever overwritten.** A mutation:
//!
//! 1. descends from the root to the target leaf, keeping every node on the path
//!    in RAM;
//! 2. edits the leaf in RAM;
//! 3. walks back UP: each level is written to a **freshly allocated** block
//!    ([`NodeStore::alloc`]), the old block is handed to [`NodeStore::release`],
//!    and the parent's slot for that child is repointed at the fresh block —
//!    which dirties the parent, which is then itself written fresh, and so on
//!    to the root;
//! 4. yields a NEW root block id in [`Btree::root`]. The tree's owner (an inode,
//!    the superblock's catalog pointer, …) stores that id and swaps it in ITS
//!    transaction — this module never touches the root record.
//!
//! Two properties fall out for free:
//!
//! * **Snapshots.** A [`Btree`] handle reopened on a PRIOR root
//!   ([`Btree::open`]) still reads the prior contents, because not one block
//!   reachable from that root was written. [`RefMap`]'s `current`/`frozen`
//!   split is what keeps it true across the release calls: a block released
//!   during the transaction has `current == 0` but `frozen == 1`, and
//!   [`RefMap::allocate`] hands out only blocks free in BOTH views — so a
//!   released-but-still-committed block cannot be reallocated (and therefore
//!   cannot be overwritten) until the owner commits and freezes.
//! * **Power-cut convergence.** Every block written by a mutation is
//!   unreachable from the old root. A crash at any instant before the owner's
//!   root swap leaves the old tree bit-identical (the fresh blocks are a leak,
//!   not a corruption); a crash after it leaves the new tree complete, because
//!   the fresh blocks all reached the medium before the swap. There is no
//!   intermediate state in which a reader sees a hybrid.
//!
//! # Comparators
//!
//! Keys are opaque byte strings ordered by a pluggable [`KeyCmp`], BeFS-style,
//! so ONE tree serves u64-keyed indexes ([`U64Cmp`], or big-endian u64 keys
//! under plain [`LexCmp`]), case-sensitive names ([`LexCmp`]), case-folded
//! names ([`AsciiFoldCmp`]) and future composite keys (length-prefixed
//! concatenations under [`LexCmp`]).
//!
//! The comparator is NOT stored on disk. It is a property of the *index*, and
//! the owner is responsible for opening a tree with the same comparator it was
//! built with — exactly as BeFS ties a comparator to an index's declared type.
//!
//! # Node format (version 1)
//!
//! All integers little-endian. One node = one 4096 B block.
//!
//! ```text
//!   off  len  field
//!     0    8  magic "UNAFSBT1"
//!     8    2  format version (1)
//!    10    1  kind: 0 = leaf, 1 = internal
//!    11    1  reserved (0)
//!    12    2  entry count
//!    14    2  reserved (0)
//!    16    4  heap_start — low-water mark of the heap
//!    20    4  reserved (0)
//!    24    8  checksum: FNV-1a over the whole 4096 B block with these 8
//!              bytes zeroed (F7's metadata-checksum line, built in now and
//!              VERIFIED ON EVERY READ)
//!    32   32  reserved (0)
//!    64  8*n  slot directory: key_off u16, key_len u16, val_off u16, val_len u16
//!   ...       free space
//!  heap_start .. 4096   heap: key bytes then value bytes, entry 0 first,
//!                       carved DOWNWARD from the end of the block
//! ```
//!
//! Every field a reader trusts is validated before use: magic, version, kind,
//! count, `heap_start`, each slot's extent, the length ceilings, the checksum,
//! and the strict ascending key order under the tree's comparator. A malformed
//! node is a clean [`BtreeError`], never a panic and never a wild read.
//!
//! # Arithmetic
//!
//! Every offset/length computation that touches a disk-derived number is
//! `checked_*`; overflow is [`BtreeError::Corrupt`], not a wrap.

use crate::refmap::RefMap;
use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use alloc::vec::Vec;
use core::cmp::Ordering;
use thiserror::Error;

// ===========================================================================
// Format constants
// ===========================================================================

/// Node magic: "UNAFSBT1".
pub const NODE_MAGIC: [u8; 8] = *b"UNAFSBT1";
/// On-disk node format version this module writes (and the only one it reads).
pub const NODE_VERSION: u16 = 1;
/// Bytes of fixed header before the slot directory.
pub const NODE_HEADER_SIZE: usize = 64;
/// Bytes per slot-directory entry.
pub const SLOT_SIZE: usize = 8;
/// Bytes available in a node for slots + heap.
pub const NODE_USABLE: usize = BLOCK_SIZE as usize - NODE_HEADER_SIZE;
/// Largest key the format accepts.
///
/// Chosen so that `SLOT_SIZE + MAX_KEY_LEN + MAX_VALUE_LEN` (776 B) leaves room
/// for at least five entries in a node — which is what makes "a split always
/// yields two non-empty halves that each fit" a theorem rather than a hope.
pub const MAX_KEY_LEN: usize = 384;
/// Largest leaf value the format accepts (see [`MAX_KEY_LEN`]).
pub const MAX_VALUE_LEN: usize = 384;
/// Bytes an internal-node value occupies (a child block id).
pub const CHILD_PTR_LEN: usize = 8;
/// Hard ceiling on tree height — a corrupt tree cannot spin the descent.
pub const MAX_DEPTH: usize = 32;

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_KIND: usize = 10;
const OFF_COUNT: usize = 12;
const OFF_HEAP: usize = 16;
const OFF_CKSUM: usize = 24;

// ===========================================================================
// Errors
// ===========================================================================

/// Everything that can go wrong in the tree.
#[derive(Error, Debug)]
pub enum BtreeError {
    /// The underlying device failed.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// The volume is full — no block could be allocated for a fresh node.
    #[error("no space for a fresh node")]
    NoSpace,
    /// Block {0} does not start with [`NODE_MAGIC`].
    #[error("block {0}: not a B+tree node (bad magic)")]
    BadMagic(u64),
    /// Block {0} carries an unsupported format version.
    #[error("block {0}: unsupported node version {1} (expected {NODE_VERSION})")]
    BadVersion(u64, u16),
    /// Block {0}'s stored checksum does not match its contents.
    #[error("block {0}: node checksum mismatch")]
    BadChecksum(u64),
    /// Block {0} is structurally impossible (bad slot extent, bad heap mark,
    /// out-of-order keys, wrong child-pointer width, arithmetic overflow…).
    #[error("block {0}: malformed node ({1})")]
    Malformed(u64, &'static str),
    /// An invariant broke without a specific block to blame.
    #[error("corrupt tree: {0}")]
    Corrupt(&'static str),
    /// The key exceeds [`MAX_KEY_LEN`].
    #[error("key of {0} bytes exceeds the {MAX_KEY_LEN} byte limit")]
    KeyTooLarge(usize),
    /// The value exceeds [`MAX_VALUE_LEN`].
    #[error("value of {0} bytes exceeds the {MAX_VALUE_LEN} byte limit")]
    ValueTooLarge(usize),
    /// A node was asked to hold more than one block's worth of entries.
    #[error("node overflow: {0} bytes exceed the {NODE_USABLE} byte node capacity")]
    NodeOverflow(usize),
}

// ===========================================================================
// Comparator seam
// ===========================================================================

/// The pluggable key order.
///
/// Implementations MUST be a total order (irreflexive ties only for equal
/// keys), deterministic, and stable across reboots and architectures — the
/// on-disk node order is stored, not recomputed, so a comparator that changes
/// its mind invalidates every tree built with it.
pub trait KeyCmp {
    /// Order two keys.
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering;
}

/// Plain byte-lexicographic order.
///
/// This is the workhorse: case-sensitive names sort naturally, big-endian
/// fixed-width integer keys sort numerically, and length-prefixed composite
/// keys sort component-wise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LexCmp;

impl KeyCmp for LexCmp {
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        a.cmp(b)
    }
}

/// Numeric order over 8-byte keys read as little-endian `u64`.
///
/// Provided for callers that prefer to store index keys in the same LE
/// encoding as the rest of the format. Keys that are not exactly 8 bytes fall
/// back to lexicographic order among themselves and sort BEFORE well-formed
/// keys if shorter, AFTER if longer — a total order in every case, so a
/// malformed key can never break the tree's ordering invariant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct U64Cmp;

impl KeyCmp for U64Cmp {
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        match (a.len(), b.len()) {
            (8, 8) => {
                let av = u64::from_le_bytes(a.try_into().unwrap_or([0u8; 8]));
                let bv = u64::from_le_bytes(b.try_into().unwrap_or([0u8; 8]));
                av.cmp(&bv)
            }
            (la, lb) => la.cmp(&lb).then_with(|| a.cmp(b)),
        }
    }
}

/// ASCII case-folded order, with the exact bytes as the tie-break.
///
/// Folding alone is not a total order (`"A"` and `"a"` would compare equal and
/// the tree could hold only one of them); appending the exact byte comparison
/// as a secondary key keeps distinct names distinct while grouping them
/// case-insensitively — which is what a case-insensitive directory index wants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AsciiFoldCmp;

impl KeyCmp for AsciiFoldCmp {
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        let fold = |x: &u8| x.to_ascii_lowercase();
        a.iter()
            .map(fold)
            .cmp(b.iter().map(fold))
            .then_with(|| a.cmp(b))
    }
}

// ===========================================================================
// The block seam
// ===========================================================================

/// The allocation + I/O seam the tree rides.
///
/// Split out from [`BlockDevice`] because the tree needs *allocation*, and the
/// crate's allocator is [`RefMap`], which lives beside the device rather than
/// inside it. [`DeviceStore`] is the pairing F4/F6 will use; tests use the same
/// one.
pub trait NodeStore {
    /// Hand out a block that is free in the CURRENT **and** FROZEN views —
    /// i.e. one that neither the in-flight transaction nor the last committed
    /// tree can reach. This is the whole never-overwrite guarantee.
    fn alloc(&mut self) -> Result<u64, BtreeError>;
    /// Drop a reference to a block. Under [`RefMap`] this is a `decref`: the
    /// block becomes reusable only once the owner commits and freezes.
    fn release(&mut self, block: u64);
    /// Read a whole 4096 B block.
    fn read(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BtreeError>;
    /// Write a whole 4096 B block.
    fn write(&mut self, block: u64, buf: &[u8]) -> Result<(), BtreeError>;
}

/// A [`NodeStore`] over the crate's own primitives: a [`BlockDevice`] for I/O
/// and a [`RefMap`] for allocation.
///
/// Borrowing both mutably (rather than owning them) is what lets a future
/// `UnaFS` method hand the tree its live device and live refmap for the
/// duration of one operation, so tree mutations join the SAME transaction as
/// everything else and are made durable by the SAME root-sector flip.
pub struct DeviceStore<'a, D: BlockDevice> {
    /// The device the nodes live on.
    pub device: &'a mut D,
    /// The allocator whose current/frozen split enforces CoW.
    pub refmap: &'a mut RefMap,
    /// Node blocks allocated through this store (bench/witness counter).
    pub allocated: u64,
    /// Node blocks released through this store.
    pub released: u64,
    /// Node blocks written through this store.
    pub written: u64,
}

impl<'a, D: BlockDevice> DeviceStore<'a, D> {
    /// Pair a device with an allocator.
    pub fn new(device: &'a mut D, refmap: &'a mut RefMap) -> Self {
        Self {
            device,
            refmap,
            allocated: 0,
            released: 0,
            written: 0,
        }
    }
}

impl<D: BlockDevice> NodeStore for DeviceStore<'_, D> {
    fn alloc(&mut self) -> Result<u64, BtreeError> {
        let b = self.refmap.allocate().ok_or(BtreeError::NoSpace)?;
        self.allocated = self.allocated.saturating_add(1);
        Ok(b)
    }
    fn release(&mut self, block: u64) {
        self.refmap.decref(block);
        self.released = self.released.saturating_add(1);
    }
    fn read(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BtreeError> {
        self.device.read_block(block, buf)?;
        Ok(())
    }
    fn write(&mut self, block: u64, buf: &[u8]) -> Result<(), BtreeError> {
        self.device.write_block(block, buf)?;
        self.written = self.written.saturating_add(1);
        Ok(())
    }
}

// ===========================================================================
// Nodes
// ===========================================================================

/// One key/value pair as the tree hands it back.
pub type Entry = (Vec<u8>, Vec<u8>);

/// Leaf or internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Values are the caller's payloads.
    Leaf = 0,
    /// Values are 8-byte little-endian child block ids.
    Internal = 1,
}

/// A node, decoded into RAM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Leaf or internal.
    pub kind: NodeKind,
    /// Entries in ascending key order. For an internal node the value is an
    /// 8-byte LE child block id and the key is that child's lower bound.
    pub entries: Vec<Entry>,
}

fn rd_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn rd_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn node_checksum(buf: &[u8]) -> u64 {
    // FNV-1a over the whole block with the checksum field read as zeros. We
    // hash the three spans around it rather than copying the block.
    let mut h = crate::hash::FnvHasher::new();
    h.write(&buf[..OFF_CKSUM]);
    h.write(&[0u8; 8]);
    h.write(&buf[OFF_CKSUM + 8..]);
    h.finish()
}

impl Node {
    /// An empty node of the given kind.
    pub fn empty(kind: NodeKind) -> Self {
        Self {
            kind,
            entries: Vec::new(),
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the node holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The node's lower bound (its first key), or `None` when empty.
    pub fn first_key(&self) -> Option<&[u8]> {
        self.entries.first().map(|(k, _)| k.as_slice())
    }

    /// Bytes this node's entries occupy inside [`NODE_USABLE`].
    pub fn used_bytes(&self) -> Result<usize, BtreeError> {
        let mut total: usize = 0;
        for (k, v) in &self.entries {
            total = total
                .checked_add(SLOT_SIZE)
                .and_then(|t| t.checked_add(k.len()))
                .and_then(|t| t.checked_add(v.len()))
                .ok_or(BtreeError::Corrupt("entry byte total overflowed"))?;
        }
        Ok(total)
    }

    /// The child block id of internal entry `i`.
    fn child(&self, i: usize) -> Result<u64, BtreeError> {
        if self.kind != NodeKind::Internal {
            return Err(BtreeError::Corrupt("child pointer read from a leaf"));
        }
        let (_, v) = self
            .entries
            .get(i)
            .ok_or(BtreeError::Corrupt("child index out of range"))?;
        let raw: [u8; CHILD_PTR_LEN] = v
            .as_slice()
            .try_into()
            .map_err(|_| BtreeError::Corrupt("child pointer is not 8 bytes"))?;
        Ok(u64::from_le_bytes(raw))
    }

    /// Encode into a 4096 B block image.
    pub fn encode(&self) -> Result<Vec<u8>, BtreeError> {
        let count = self.entries.len();
        if count > u16::MAX as usize {
            return Err(BtreeError::Corrupt("entry count exceeds u16"));
        }
        let slots_end = count
            .checked_mul(SLOT_SIZE)
            .and_then(|s| s.checked_add(NODE_HEADER_SIZE))
            .ok_or(BtreeError::Corrupt("slot directory overflowed"))?;
        if slots_end > BLOCK_SIZE as usize {
            return Err(BtreeError::NodeOverflow(slots_end));
        }

        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        let mut heap = BLOCK_SIZE as usize;

        for (i, (k, v)) in self.entries.iter().enumerate() {
            if k.len() > MAX_KEY_LEN {
                return Err(BtreeError::KeyTooLarge(k.len()));
            }
            if self.kind == NodeKind::Leaf {
                if v.len() > MAX_VALUE_LEN {
                    return Err(BtreeError::ValueTooLarge(v.len()));
                }
            } else if v.len() != CHILD_PTR_LEN {
                return Err(BtreeError::Corrupt("child pointer is not 8 bytes"));
            }

            let koff = heap
                .checked_sub(k.len())
                .ok_or(BtreeError::NodeOverflow(BLOCK_SIZE as usize))?;
            let voff = koff
                .checked_sub(v.len())
                .ok_or(BtreeError::NodeOverflow(BLOCK_SIZE as usize))?;
            if voff < slots_end {
                return Err(BtreeError::NodeOverflow(self.used_bytes()?));
            }
            buf[koff..koff + k.len()].copy_from_slice(k);
            buf[voff..voff + v.len()].copy_from_slice(v);
            heap = voff;

            let s = NODE_HEADER_SIZE
                .checked_add(i.checked_mul(SLOT_SIZE).ok_or(BtreeError::Corrupt("slot"))?)
                .ok_or(BtreeError::Corrupt("slot"))?;
            buf[s..s + 2].copy_from_slice(&(koff as u16).to_le_bytes());
            buf[s + 2..s + 4].copy_from_slice(&(k.len() as u16).to_le_bytes());
            buf[s + 4..s + 6].copy_from_slice(&(voff as u16).to_le_bytes());
            buf[s + 6..s + 8].copy_from_slice(&(v.len() as u16).to_le_bytes());
        }

        buf[OFF_MAGIC..OFF_MAGIC + 8].copy_from_slice(&NODE_MAGIC);
        buf[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&NODE_VERSION.to_le_bytes());
        buf[OFF_KIND] = self.kind as u8;
        buf[OFF_COUNT..OFF_COUNT + 2].copy_from_slice(&(count as u16).to_le_bytes());
        buf[OFF_HEAP..OFF_HEAP + 4].copy_from_slice(&(heap as u32).to_le_bytes());
        let sum = node_checksum(&buf);
        buf[OFF_CKSUM..OFF_CKSUM + 8].copy_from_slice(&sum.to_le_bytes());
        Ok(buf)
    }

    /// Decode and VALIDATE a node from a 4096 B block image.
    ///
    /// Structural validation only — the key ORDER is checked by
    /// [`Btree::read_node`], which has the comparator.
    pub fn decode(block: u64, buf: &[u8]) -> Result<Node, BtreeError> {
        if buf.len() != BLOCK_SIZE as usize {
            return Err(BtreeError::Malformed(block, "buffer is not one block"));
        }
        if buf[OFF_MAGIC..OFF_MAGIC + 8] != NODE_MAGIC {
            return Err(BtreeError::BadMagic(block));
        }
        let version = rd_u16(buf, OFF_VERSION);
        if version != NODE_VERSION {
            return Err(BtreeError::BadVersion(block, version));
        }
        let stored = u64::from_le_bytes(
            buf[OFF_CKSUM..OFF_CKSUM + 8]
                .try_into()
                .map_err(|_| BtreeError::Malformed(block, "checksum field"))?,
        );
        if node_checksum(buf) != stored {
            return Err(BtreeError::BadChecksum(block));
        }
        let kind = match buf[OFF_KIND] {
            0 => NodeKind::Leaf,
            1 => NodeKind::Internal,
            _ => return Err(BtreeError::Malformed(block, "unknown node kind")),
        };
        let count = rd_u16(buf, OFF_COUNT) as usize;
        let heap_start = rd_u32(buf, OFF_HEAP) as usize;
        let slots_end = count
            .checked_mul(SLOT_SIZE)
            .and_then(|s| s.checked_add(NODE_HEADER_SIZE))
            .ok_or(BtreeError::Malformed(block, "slot directory overflow"))?;
        if slots_end > BLOCK_SIZE as usize {
            return Err(BtreeError::Malformed(block, "slot directory past block"));
        }
        if heap_start < slots_end || heap_start > BLOCK_SIZE as usize {
            return Err(BtreeError::Malformed(block, "heap mark out of range"));
        }

        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| BtreeError::Malformed(block, "entry vector refused"))?;
        for i in 0..count {
            let s = NODE_HEADER_SIZE
                .checked_add(
                    i.checked_mul(SLOT_SIZE)
                        .ok_or(BtreeError::Malformed(block, "slot offset overflow"))?,
                )
                .ok_or(BtreeError::Malformed(block, "slot offset overflow"))?;
            let koff = rd_u16(buf, s) as usize;
            let klen = rd_u16(buf, s + 2) as usize;
            let voff = rd_u16(buf, s + 4) as usize;
            let vlen = rd_u16(buf, s + 6) as usize;
            if klen > MAX_KEY_LEN {
                return Err(BtreeError::Malformed(block, "key length over the limit"));
            }
            if kind == NodeKind::Leaf {
                if vlen > MAX_VALUE_LEN {
                    return Err(BtreeError::Malformed(block, "value length over the limit"));
                }
            } else if vlen != CHILD_PTR_LEN {
                return Err(BtreeError::Malformed(block, "child pointer is not 8 bytes"));
            }
            let kend = koff
                .checked_add(klen)
                .ok_or(BtreeError::Malformed(block, "key extent overflow"))?;
            let vend = voff
                .checked_add(vlen)
                .ok_or(BtreeError::Malformed(block, "value extent overflow"))?;
            if koff < heap_start || kend > BLOCK_SIZE as usize {
                return Err(BtreeError::Malformed(block, "key extent outside the heap"));
            }
            if voff < heap_start || vend > BLOCK_SIZE as usize {
                return Err(BtreeError::Malformed(block, "value extent outside the heap"));
            }
            entries.push((buf[koff..kend].to_vec(), buf[voff..vend].to_vec()));
        }
        Ok(Node { kind, entries })
    }
}

// ===========================================================================
// The tree
// ===========================================================================

/// Structural facts about a tree, for witnesses and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TreeStats {
    /// Levels from the root to a leaf, inclusive (an empty tree is 1).
    pub depth: usize,
    /// Total node blocks reachable from the root.
    pub nodes: usize,
    /// Leaf nodes.
    pub leaves: usize,
    /// Internal nodes.
    pub internal: usize,
    /// Key/value pairs.
    pub entries: usize,
}

/// A copy-on-write B+tree.
///
/// The handle is CHEAP and holds no borrowed state: a root block id, the
/// comparator, and an optional fanout cap. Every operation takes the
/// [`NodeStore`] explicitly, so a tree can be read through one store and
/// mutated through another (that is exactly how the snapshot tests read a
/// prior root while the live root moves on).
#[derive(Debug, Clone)]
pub struct Btree<C: KeyCmp> {
    root: u64,
    cmp: C,
    fanout_cap: Option<usize>,
}

impl<C: KeyCmp> Btree<C> {
    /// Create an EMPTY tree: allocates and writes one empty leaf as the root.
    /// The caller must record [`Btree::root`] in the owning object and commit.
    pub fn create<S: NodeStore>(store: &mut S, cmp: C) -> Result<Self, BtreeError> {
        let tree = Self {
            root: 0,
            cmp,
            fanout_cap: None,
        };
        let block = store.alloc()?;
        let buf = Node::empty(NodeKind::Leaf).encode()?;
        store.write(block, &buf)?;
        Ok(Self { root: block, ..tree })
    }

    /// Open an EXISTING tree at `root`. Opening a PRIOR root is how snapshot
    /// reads work — nothing else is needed, because the prior root's blocks
    /// were never overwritten.
    pub fn open(root: u64, cmp: C) -> Self {
        Self {
            root,
            cmp,
            fanout_cap: None,
        }
    }

    /// Cap the number of entries per node.
    ///
    /// A pure in-RAM tuning/test knob with NO format impact: a capped tree
    /// simply writes less-full nodes, and any handle (capped or not) reads the
    /// result correctly. Tests use it to reach splits, merges and multi-level
    /// depth in tens of operations instead of tens of thousands.
    pub fn with_fanout_cap(mut self, cap: usize) -> Self {
        self.fanout_cap = Some(cap.max(4));
        self
    }

    /// The current root block. Swap this into the owning object's record,
    /// inside the owner's transaction.
    pub fn root(&self) -> u64 {
        self.root
    }

    /// The comparator.
    pub fn comparator(&self) -> &C {
        &self.cmp
    }

    // -- reads ---------------------------------------------------------

    /// Read + fully validate a node, INCLUDING the ascending key order under
    /// this tree's comparator.
    pub fn read_node<S: NodeStore>(&self, store: &mut S, block: u64) -> Result<Node, BtreeError> {
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        store.read(block, &mut buf)?;
        let node = Node::decode(block, &buf)?;
        for w in node.entries.windows(2) {
            if self.cmp.compare(&w[0].0, &w[1].0) != Ordering::Less {
                return Err(BtreeError::Malformed(block, "keys are not strictly ascending"));
            }
        }
        if node.kind == NodeKind::Internal && node.entries.is_empty() {
            return Err(BtreeError::Malformed(block, "internal node with no children"));
        }
        Ok(node)
    }

    /// Index of the child to descend into for `key`: the LAST entry whose key
    /// is `<= key`, or 0 when `key` is below the node's lower bound.
    fn descend_index(&self, node: &Node, key: &[u8]) -> usize {
        let mut lo = 0usize;
        let mut hi = node.entries.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.cmp.compare(&node.entries[mid].0, key) == Ordering::Greater {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo.saturating_sub(1)
    }

    /// Index of the first entry whose key is `>= key`.
    fn lower_bound(&self, node: &Node, key: &[u8]) -> usize {
        let mut lo = 0usize;
        let mut hi = node.entries.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.cmp.compare(&node.entries[mid].0, key) == Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Descend to the leaf that would hold `key`, returning the whole path
    /// root-first. Each level records the index it descended through.
    fn descend<S: NodeStore>(&self, store: &mut S, key: &[u8]) -> Result<Vec<Level>, BtreeError> {
        let mut path: Vec<Level> = Vec::new();
        let mut block = self.root;
        loop {
            if path.len() >= MAX_DEPTH {
                return Err(BtreeError::Corrupt("tree deeper than MAX_DEPTH"));
            }
            let node = self.read_node(store, block)?;
            if node.kind == NodeKind::Leaf {
                let ci = self.lower_bound(&node, key);
                path.push(Level { block, node, ci });
                return Ok(path);
            }
            let ci = self.descend_index(&node, key);
            let child = node.child(ci)?;
            if child == block {
                return Err(BtreeError::Malformed(block, "child pointer loops to self"));
            }
            path.push(Level { block, node, ci });
            block = child;
        }
    }

    /// Look up `key`. `None` when absent.
    pub fn lookup<S: NodeStore>(
        &self,
        store: &mut S,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, BtreeError> {
        let path = self.descend(store, key)?;
        let leaf = path.last().ok_or(BtreeError::Corrupt("empty path"))?;
        match leaf.node.entries.get(leaf.ci) {
            Some((k, v)) if self.cmp.compare(k, key) == Ordering::Equal => Ok(Some(v.clone())),
            _ => Ok(None),
        }
    }

    /// Whether `key` is present.
    pub fn contains<S: NodeStore>(&self, store: &mut S, key: &[u8]) -> Result<bool, BtreeError> {
        Ok(self.lookup(store, key)?.is_some())
    }

    // -- structural witnesses -------------------------------------------

    /// Walk the whole tree and report its shape. O(nodes) reads.
    pub fn stats<S: NodeStore>(&self, store: &mut S) -> Result<TreeStats, BtreeError> {
        let mut st = TreeStats::default();
        let mut frontier = alloc::vec![self.root];
        let mut depth = 0usize;
        while !frontier.is_empty() {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(BtreeError::Corrupt("tree deeper than MAX_DEPTH"));
            }
            let mut next = Vec::new();
            for &b in &frontier {
                let node = self.read_node(store, b)?;
                st.nodes = st
                    .nodes
                    .checked_add(1)
                    .ok_or(BtreeError::Corrupt("node count overflow"))?;
                match node.kind {
                    NodeKind::Leaf => {
                        st.leaves += 1;
                        st.entries = st
                            .entries
                            .checked_add(node.len())
                            .ok_or(BtreeError::Corrupt("entry count overflow"))?;
                    }
                    NodeKind::Internal => {
                        st.internal += 1;
                        for i in 0..node.len() {
                            next.push(node.child(i)?);
                        }
                    }
                }
            }
            frontier = next;
        }
        st.depth = depth;
        Ok(st)
    }

    /// Every block reachable from the root, root first, level by level.
    ///
    /// This is the CoW witness the tests assert on: the set of blocks a
    /// mutation WRITES must be disjoint from the set this returns for the
    /// PRIOR root.
    pub fn reachable_blocks<S: NodeStore>(&self, store: &mut S) -> Result<Vec<u64>, BtreeError> {
        let mut out = Vec::new();
        let mut frontier = alloc::vec![self.root];
        let mut depth = 0usize;
        while !frontier.is_empty() {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(BtreeError::Corrupt("tree deeper than MAX_DEPTH"));
            }
            let mut next = Vec::new();
            for &b in &frontier {
                out.push(b);
                let node = self.read_node(store, b)?;
                if node.kind == NodeKind::Internal {
                    for i in 0..node.len() {
                        next.push(node.child(i)?);
                    }
                }
            }
            frontier = next;
        }
        Ok(out)
    }

    // -- fill-factor policy ---------------------------------------------

    fn over_full(&self, node: &Node) -> Result<bool, BtreeError> {
        if node.used_bytes()? > NODE_USABLE {
            return Ok(true);
        }
        Ok(matches!(self.fanout_cap, Some(c) if node.len() > c))
    }

    /// A node is under-filled when it is below half by the BYTE measure and —
    /// if a fanout cap is in force — below half by the entry-count measure
    /// too. Both measures must agree, so a capped tree rebalances on counts
    /// and an uncapped one on bytes, with no policy branch anywhere else.
    fn under_full(&self, node: &Node) -> Result<bool, BtreeError> {
        let bytes_low = node
            .used_bytes()?
            .checked_mul(2)
            .ok_or(BtreeError::Corrupt("fill measure overflow"))?
            < NODE_USABLE;
        let count_low = match self.fanout_cap {
            Some(c) => node
                .len()
                .checked_mul(2)
                .ok_or(BtreeError::Corrupt("fill measure overflow"))?
                < c,
            None => true,
        };
        Ok(bytes_low && count_low)
    }

    fn fits_merged(&self, a: &Node, b: &Node) -> Result<bool, BtreeError> {
        let bytes = a
            .used_bytes()?
            .checked_add(b.used_bytes()?)
            .ok_or(BtreeError::Corrupt("merge byte total overflow"))?;
        if bytes > NODE_USABLE {
            return Ok(false);
        }
        let count = a
            .len()
            .checked_add(b.len())
            .ok_or(BtreeError::Corrupt("merge count overflow"))?;
        Ok(match self.fanout_cap {
            Some(c) => count <= c,
            None => true,
        })
    }

    /// Split an over-full node in two. Both halves are non-empty and each fits
    /// one block (see [`MAX_KEY_LEN`] for why that is a theorem).
    fn split(&self, node: Node) -> Result<(Node, Node), BtreeError> {
        let n = node.len();
        if n < 2 {
            return Err(BtreeError::Corrupt("cannot split a node of fewer than 2"));
        }
        let total = node.used_bytes()?;
        // Byte-median: the first index at which the prefix reaches half the
        // node's bytes. This is what keeps "both halves fit one block" true
        // even for wildly uneven entry sizes.
        let mut mid = n;
        let mut acc: usize = 0;
        let half = total / 2;
        for (i, (k, v)) in node.entries.iter().enumerate() {
            acc = acc
                .checked_add(SLOT_SIZE)
                .and_then(|t| t.checked_add(k.len()))
                .and_then(|t| t.checked_add(v.len()))
                .ok_or(BtreeError::Corrupt("split byte total overflow"))?;
            if acc >= half {
                mid = i + 1;
                break;
            }
        }
        // Under a fanout cap, neither half may exceed it.
        let mut lo_bound = 1usize;
        let mut hi_bound = n - 1;
        if let Some(c) = self.fanout_cap {
            let l = lo_bound.max(n.saturating_sub(c));
            let h = hi_bound.min(c);
            if l <= h {
                lo_bound = l;
                hi_bound = h;
            }
        }
        let mid = mid.clamp(lo_bound, hi_bound);
        let kind = node.kind;
        let mut left = node.entries;
        let right = left.split_off(mid);
        Ok((
            Node {
                kind,
                entries: left,
            },
            Node {
                kind,
                entries: right,
            },
        ))
    }

    /// Move entries from `sib` into `node` until `node` is no longer
    /// under-filled, without pushing either side out of shape. Returns how
    /// many moved (0 when the sibling cannot spare anything, which leaves a
    /// merely under-filled — still perfectly valid — node in place).
    fn borrow(&self, node: &mut Node, sib: &mut Node, sib_is_right: bool) -> Result<usize, BtreeError> {
        let mut moved = 0usize;
        while self.under_full(node)? && sib.len() > 1 {
            let candidate = if sib_is_right {
                sib.entries[0].clone()
            } else {
                sib.entries[sib.len() - 1].clone()
            };
            let cost = SLOT_SIZE
                .checked_add(candidate.0.len())
                .and_then(|t| t.checked_add(candidate.1.len()))
                .ok_or(BtreeError::Corrupt("borrow cost overflow"))?;
            let after_node = node
                .used_bytes()?
                .checked_add(cost)
                .ok_or(BtreeError::Corrupt("borrow byte total overflow"))?;
            if after_node > NODE_USABLE {
                break;
            }
            if matches!(self.fanout_cap, Some(c) if node.len() >= c) {
                break;
            }
            // Never rob the sibling into an under-fill of its own.
            let sib_after = sib
                .used_bytes()?
                .checked_sub(cost)
                .ok_or(BtreeError::Corrupt("borrow underflow"))?;
            let sib_bytes_low = sib_after
                .checked_mul(2)
                .ok_or(BtreeError::Corrupt("fill measure overflow"))?
                < NODE_USABLE;
            let sib_count_low = match self.fanout_cap {
                Some(c) => (sib.len() - 1) * 2 < c,
                None => true,
            };
            if sib_bytes_low && sib_count_low && moved > 0 {
                break;
            }
            if sib_is_right {
                let e = sib.entries.remove(0);
                node.entries.push(e);
            } else {
                let e = sib
                    .entries
                    .pop()
                    .ok_or(BtreeError::Corrupt("borrow from an empty sibling"))?;
                node.entries.insert(0, e);
            }
            moved = moved
                .checked_add(1)
                .ok_or(BtreeError::Corrupt("borrow counter overflow"))?;
        }
        Ok(moved)
    }

    // -- writes ---------------------------------------------------------

    fn write_fresh<S: NodeStore>(&self, store: &mut S, node: &Node) -> Result<u64, BtreeError> {
        let buf = node.encode()?;
        let block = store.alloc()?;
        store.write(block, &buf)?;
        Ok(block)
    }

    fn ptr(block: u64) -> Vec<u8> {
        block.to_le_bytes().to_vec()
    }

    fn slot_for(node: &Node, block: u64) -> Result<Entry, BtreeError> {
        let key = node
            .first_key()
            .ok_or(BtreeError::Corrupt("slot for an empty node"))?
            .to_vec();
        Ok((key, Self::ptr(block)))
    }

    /// Write a modified NON-ROOT node fresh and patch the parent's slots to
    /// match — splitting, merging or borrowing as the fill factor demands.
    /// `ci` is the node's index in `parent`; the node's old block has already
    /// been released by the caller.
    fn place_child<S: NodeStore>(
        &self,
        store: &mut S,
        node: Node,
        ci: usize,
        parent: &mut Node,
    ) -> Result<(), BtreeError> {
        if ci >= parent.len() {
            return Err(BtreeError::Corrupt("child index past the parent"));
        }
        // The child emptied out: it simply vanishes from the parent.
        if node.is_empty() {
            parent.entries.remove(ci);
            return Ok(());
        }
        // Over-full: split, and the parent grows one slot.
        if self.over_full(&node)? {
            let (a, b) = self.split(node)?;
            let ba = self.write_fresh(store, &a)?;
            let bb = self.write_fresh(store, &b)?;
            parent.entries[ci] = Self::slot_for(&a, ba)?;
            parent.entries.insert(ci + 1, Self::slot_for(&b, bb)?);
            return Ok(());
        }
        // Under-full with a sibling available: merge, else borrow.
        if self.under_full(&node)? && parent.len() > 1 {
            let (si, sib_is_right) = if ci + 1 < parent.len() {
                (ci + 1, true)
            } else {
                (ci - 1, false)
            };
            let sblock = parent.child(si)?;
            let sib = self.read_node(store, sblock)?;
            if sib.kind != node.kind {
                return Err(BtreeError::Malformed(sblock, "sibling of a different kind"));
            }
            if self.fits_merged(&node, &sib)? {
                store.release(sblock);
                let kind = node.kind;
                let mut entries = if sib_is_right {
                    let mut e = node.entries;
                    e.extend(sib.entries);
                    e
                } else {
                    let mut e = sib.entries;
                    e.extend(node.entries);
                    e
                };
                entries.shrink_to_fit();
                let merged = Node { kind, entries };
                let mb = self.write_fresh(store, &merged)?;
                let lo = core::cmp::min(ci, si);
                parent.entries[lo] = Self::slot_for(&merged, mb)?;
                parent.entries.remove(lo + 1);
                return Ok(());
            }
            let mut n = node;
            let mut s = sib;
            let moved = self.borrow(&mut n, &mut s, sib_is_right)?;
            if moved > 0 {
                store.release(sblock);
                let nb = self.write_fresh(store, &n)?;
                let sb = self.write_fresh(store, &s)?;
                parent.entries[ci] = Self::slot_for(&n, nb)?;
                parent.entries[si] = Self::slot_for(&s, sb)?;
                return Ok(());
            }
            // The sibling could spare nothing: an under-filled node is still a
            // correct node, so write it as it stands.
            let nb = self.write_fresh(store, &n)?;
            parent.entries[ci] = Self::slot_for(&n, nb)?;
            return Ok(());
        }
        let nb = self.write_fresh(store, &node)?;
        parent.entries[ci] = Self::slot_for(&node, nb)?;
        Ok(())
    }

    /// Write the modified ROOT: split it (growing the tree a level) or collapse
    /// a single-child internal root (shrinking it). Returns the new root block.
    fn place_root<S: NodeStore>(&self, store: &mut S, node: Node) -> Result<u64, BtreeError> {
        if self.over_full(&node)? {
            let (a, b) = self.split(node)?;
            let ba = self.write_fresh(store, &a)?;
            let bb = self.write_fresh(store, &b)?;
            let root = Node {
                kind: NodeKind::Internal,
                entries: alloc::vec![Self::slot_for(&a, ba)?, Self::slot_for(&b, bb)?],
            };
            return self.write_fresh(store, &root);
        }
        // A root that is an internal node with a single child is a level of
        // pure indirection: drop it and promote the child. The promoted block
        // is NOT rewritten — it was not modified, so CoW has nothing to copy.
        let mut node = node;
        let mut depth_guard = 0usize;
        while node.kind == NodeKind::Internal && node.len() == 1 {
            depth_guard += 1;
            if depth_guard > MAX_DEPTH {
                return Err(BtreeError::Corrupt("root collapse ran away"));
            }
            let child = node.child(0)?;
            let below = self.read_node(store, child)?;
            if below.kind == NodeKind::Internal && below.len() == 1 {
                store.release(child);
                node = below;
                continue;
            }
            return Ok(child);
        }
        if node.is_empty() && node.kind == NodeKind::Internal {
            // Every child vanished: the tree is empty again.
            let root = Node::empty(NodeKind::Leaf);
            return self.write_fresh(store, &root);
        }
        self.write_fresh(store, &node)
    }

    /// The bottom-up half of every mutation: path-copy from the modified leaf
    /// back to a fresh root.
    ///
    /// Runs against a [`Deferred`] store, so no block this mutation retires can
    /// be handed back out as a fresh block *within the same mutation* — which
    /// is what makes "the root as it stood before this call still reads the
    /// contents it had" hold unconditionally, committed or not.
    fn commit_path<S: NodeStore>(
        &mut self,
        store: &mut Deferred<'_, S>,
        mut path: Vec<Level>,
    ) -> Result<(), BtreeError> {
        let mut cur = path.pop().ok_or(BtreeError::Corrupt("empty path"))?;
        loop {
            store.release(cur.block);
            match path.pop() {
                None => {
                    self.root = self.place_root(store, cur.node)?;
                    return Ok(());
                }
                Some(mut parent) => {
                    let ci = parent.ci;
                    self.place_child(store, cur.node, ci, &mut parent.node)?;
                    cur = parent;
                }
            }
        }
    }

    /// Insert or replace `key`. Returns the value it displaced, if any.
    ///
    /// On success [`Btree::root`] names a NEW block; the caller records it in
    /// the owning object and commits.
    pub fn insert<S: NodeStore>(
        &mut self,
        store: &mut S,
        key: &[u8],
        value: &[u8],
    ) -> Result<Option<Vec<u8>>, BtreeError> {
        if key.len() > MAX_KEY_LEN {
            return Err(BtreeError::KeyTooLarge(key.len()));
        }
        if value.len() > MAX_VALUE_LEN {
            return Err(BtreeError::ValueTooLarge(value.len()));
        }
        let mut path = self.descend(store, key)?;
        let leaf = path.last_mut().ok_or(BtreeError::Corrupt("empty path"))?;
        let ci = leaf.ci;
        let hit = matches!(
            leaf.node.entries.get(ci),
            Some((k, _)) if self.cmp.compare(k, key) == Ordering::Equal
        );
        let displaced = if hit {
            let slot = &mut leaf.node.entries[ci];
            Some(core::mem::replace(&mut slot.1, value.to_vec()))
        } else {
            leaf.node.entries.insert(ci, (key.to_vec(), value.to_vec()));
            None
        };
        self.run_path(store, path)?;
        Ok(displaced)
    }

    /// Run [`Self::commit_path`] against a deferred-release wrapper and, only
    /// on success, hand the retired blocks to the real store.
    fn run_path<S: NodeStore>(
        &mut self,
        store: &mut S,
        path: Vec<Level>,
    ) -> Result<(), BtreeError> {
        let freed = {
            let mut deferred = Deferred {
                inner: store,
                freed: Vec::new(),
            };
            self.commit_path(&mut deferred, path)?;
            core::mem::take(&mut deferred.freed)
        };
        for b in freed {
            store.release(b);
        }
        Ok(())
    }

    /// Remove `key`. Returns the value it held, if any. A miss is NOT an
    /// error and costs no writes — the root is unchanged.
    pub fn remove<S: NodeStore>(
        &mut self,
        store: &mut S,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, BtreeError> {
        let mut path = self.descend(store, key)?;
        let leaf = path.last_mut().ok_or(BtreeError::Corrupt("empty path"))?;
        let ci = leaf.ci;
        let hit = matches!(
            leaf.node.entries.get(ci),
            Some((k, _)) if self.cmp.compare(k, key) == Ordering::Equal
        );
        if !hit {
            return Ok(None);
        }
        let (_, old) = leaf.node.entries.remove(ci);
        self.run_path(store, path)?;
        Ok(Some(old))
    }

    // -- cursors ---------------------------------------------------------

    fn descend_edge<S: NodeStore>(
        &self,
        store: &mut S,
        mut block: u64,
        last: bool,
        path: &mut Vec<Level>,
    ) -> Result<(), BtreeError> {
        loop {
            if path.len() >= MAX_DEPTH {
                return Err(BtreeError::Corrupt("tree deeper than MAX_DEPTH"));
            }
            let node = self.read_node(store, block)?;
            if node.kind == NodeKind::Leaf {
                let ci = if last {
                    node.len().saturating_sub(1)
                } else {
                    0
                };
                path.push(Level { block, node, ci });
                return Ok(());
            }
            let ci = if last { node.len() - 1 } else { 0 };
            let child = node.child(ci)?;
            if child == block {
                return Err(BtreeError::Malformed(block, "child pointer loops to self"));
            }
            path.push(Level { block, node, ci });
            block = child;
        }
    }

    /// A cursor on the first entry (empty tree ⇒ an exhausted cursor).
    pub fn cursor_first<S: NodeStore>(&self, store: &mut S) -> Result<Cursor, BtreeError> {
        let mut path = Vec::new();
        self.descend_edge(store, self.root, false, &mut path)?;
        let mut c = Cursor { path, live: true };
        self.settle(store, &mut c, false)?;
        Ok(c)
    }

    /// A cursor on the last entry (empty tree ⇒ an exhausted cursor).
    pub fn cursor_last<S: NodeStore>(&self, store: &mut S) -> Result<Cursor, BtreeError> {
        let mut path = Vec::new();
        self.descend_edge(store, self.root, true, &mut path)?;
        let mut c = Cursor { path, live: true };
        self.settle(store, &mut c, true)?;
        Ok(c)
    }

    /// A cursor on the first entry `>= key` — the forward range seek.
    pub fn cursor_seek<S: NodeStore>(
        &self,
        store: &mut S,
        key: &[u8],
    ) -> Result<Cursor, BtreeError> {
        let path = self.descend(store, key)?;
        let mut c = Cursor { path, live: true };
        // `descend` leaves the leaf index at the lower bound, which may be one
        // past the end of this leaf; settle walks it to the next real entry.
        self.settle(store, &mut c, false)?;
        Ok(c)
    }

    /// A cursor on the last entry `<= key` — the reverse range seek.
    pub fn cursor_seek_back<S: NodeStore>(
        &self,
        store: &mut S,
        key: &[u8],
    ) -> Result<Cursor, BtreeError> {
        let mut c = self.cursor_seek(store, key)?;
        enum Act {
            Keep,
            Back,
            Last,
        }
        let act = match c.current() {
            Some((k, _)) if self.cmp.compare(k, key) == Ordering::Equal => Act::Keep,
            Some(_) => Act::Back,
            // Past the end: the last entry of the tree is the answer.
            None => Act::Last,
        };
        match act {
            Act::Keep => Ok(c),
            Act::Back => {
                self.cursor_prev(store, &mut c)?;
                Ok(c)
            }
            Act::Last => self.cursor_last(store),
        }
    }

    /// If the cursor's leaf index is out of range, walk to the next (or
    /// previous) real entry; mark it exhausted when there is none.
    fn settle<S: NodeStore>(
        &self,
        store: &mut S,
        c: &mut Cursor,
        backward: bool,
    ) -> Result<(), BtreeError> {
        let leaf = c.path.last().ok_or(BtreeError::Corrupt("empty cursor"))?;
        if leaf.ci < leaf.node.len() {
            return Ok(());
        }
        if backward {
            if leaf.node.is_empty() {
                return self.step(store, c, true);
            }
            let n = leaf.node.len();
            if let Some(l) = c.path.last_mut() {
                l.ci = n - 1;
            }
            Ok(())
        } else {
            self.step(store, c, false)
        }
    }

    /// Advance to the next entry in key order.
    pub fn cursor_next<S: NodeStore>(
        &self,
        store: &mut S,
        c: &mut Cursor,
    ) -> Result<(), BtreeError> {
        if !c.live {
            return Ok(());
        }
        if let Some(l) = c.path.last_mut() {
            l.ci = l.ci.saturating_add(1);
        }
        self.step(store, c, false)
    }

    /// Retreat to the previous entry in key order.
    pub fn cursor_prev<S: NodeStore>(
        &self,
        store: &mut S,
        c: &mut Cursor,
    ) -> Result<(), BtreeError> {
        if !c.live {
            return Ok(());
        }
        let leaf = c.path.last().ok_or(BtreeError::Corrupt("empty cursor"))?;
        if leaf.ci > 0 && leaf.ci - 1 < leaf.node.len() {
            if let Some(l) = c.path.last_mut() {
                l.ci -= 1;
            }
            return Ok(());
        }
        self.step(store, c, true)
    }

    /// The shared "this leaf is used up, climb and re-descend" walk.
    fn step<S: NodeStore>(
        &self,
        store: &mut S,
        c: &mut Cursor,
        backward: bool,
    ) -> Result<(), BtreeError> {
        {
            let leaf = c.path.last().ok_or(BtreeError::Corrupt("empty cursor"))?;
            if !backward && leaf.ci < leaf.node.len() {
                return Ok(());
            }
        }
        loop {
            c.path.pop();
            let Some(anc) = c.path.last_mut() else {
                c.live = false;
                return Ok(());
            };
            if backward {
                if anc.ci == 0 {
                    continue;
                }
                anc.ci -= 1;
            } else {
                anc.ci = anc.ci.saturating_add(1);
                if anc.ci >= anc.node.len() {
                    continue;
                }
            }
            let child = anc.node.child(anc.ci)?;
            self.descend_edge(store, child, backward, &mut c.path)?;
            let leaf = c.path.last().ok_or(BtreeError::Corrupt("empty cursor"))?;
            if leaf.node.is_empty() {
                // An empty non-root leaf should not exist, but a hostile
                // volume can produce one: keep walking rather than stalling.
                continue;
            }
            return Ok(());
        }
    }

    /// Collect `[lo, hi)` in key order. `None` bounds are open.
    ///
    /// The building block F4's "true range queries" needs; `reverse` walks the
    /// same half-open interval from the high end down.
    pub fn range<S: NodeStore>(
        &self,
        store: &mut S,
        lo: Option<&[u8]>,
        hi: Option<&[u8]>,
        reverse: bool,
    ) -> Result<Vec<Entry>, BtreeError> {
        let mut out = Vec::new();
        if reverse {
            let mut c = match hi {
                // Half-open at the top: seek back from `hi` and drop `hi` itself.
                Some(h) => {
                    let mut c = self.cursor_seek(store, h)?;
                    if c.current().is_some() {
                        self.cursor_prev(store, &mut c)?;
                    } else {
                        c = self.cursor_last(store)?;
                    }
                    c
                }
                None => self.cursor_last(store)?,
            };
            while let Some((k, v)) = c.current() {
                if let Some(l) = lo
                    && self.cmp.compare(k, l) == Ordering::Less
                {
                    break;
                }
                out.push((k.to_vec(), v.to_vec()));
                self.cursor_prev(store, &mut c)?;
            }
        } else {
            let mut c = match lo {
                Some(l) => self.cursor_seek(store, l)?,
                None => self.cursor_first(store)?,
            };
            while let Some((k, v)) = c.current() {
                if let Some(h) = hi
                    && self.cmp.compare(k, h) != Ordering::Less
                {
                    break;
                }
                out.push((k.to_vec(), v.to_vec()));
                self.cursor_next(store, &mut c)?;
            }
        }
        Ok(out)
    }
}

/// A [`NodeStore`] shim that PARKS releases instead of performing them.
///
/// Every mutation runs against one of these, so the allocator cannot hand a
/// block the mutation just retired straight back out as a fresh block. The
/// parked list is replayed into the real store only after the mutation
/// succeeds; on failure the releases are simply dropped, which leaks blocks
/// into the transaction the owner is about to unwind anyway — never the other
/// way round.
struct Deferred<'s, S: NodeStore> {
    inner: &'s mut S,
    freed: Vec<u64>,
}

impl<S: NodeStore> NodeStore for Deferred<'_, S> {
    fn alloc(&mut self) -> Result<u64, BtreeError> {
        self.inner.alloc()
    }
    fn release(&mut self, block: u64) {
        self.freed.push(block);
    }
    fn read(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BtreeError> {
        self.inner.read(block, buf)
    }
    fn write(&mut self, block: u64, buf: &[u8]) -> Result<(), BtreeError> {
        self.inner.write(block, buf)
    }
}

/// One level of a root-to-leaf path: the block, its decoded node, and the
/// entry index this path went through.
#[derive(Debug, Clone)]
struct Level {
    block: u64,
    node: Node,
    ci: usize,
}

/// A position in the tree, forward and reverse steppable.
///
/// The cursor caches every node on its path, so stepping within a leaf costs
/// nothing and crossing a leaf boundary costs one read per level climbed. It
/// is a SNAPSHOT position: the tree it was opened on is immutable under CoW,
/// so a concurrent mutation (which produces a new root) cannot invalidate it —
/// the cursor simply keeps walking the tree it started on.
#[derive(Debug, Clone)]
pub struct Cursor {
    path: Vec<Level>,
    live: bool,
}

impl Cursor {
    /// The entry under the cursor, or `None` when it is exhausted.
    pub fn current(&self) -> Option<(&[u8], &[u8])> {
        if !self.live {
            return None;
        }
        let leaf = self.path.last()?;
        let (k, v) = leaf.node.entries.get(leaf.ci)?;
        Some((k.as_slice(), v.as_slice()))
    }

    /// Whether the cursor still points at an entry.
    pub fn is_valid(&self) -> bool {
        self.current().is_some()
    }
}
