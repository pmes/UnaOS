# UnaFS B+tree: the one on-disk index (F3)

Status: **F3 landed, standalone** — `unaos/libs/fs/unafs/src/btree.rs` is a complete,
copy-on-write, checksummed B+tree over the crate's existing `BlockDevice` + `RefMap`
primitives, with 29 host tests (`tests/btree_logic.rs`, `tests/btree_kats.rs`). It is
**not yet wired into any filesystem path**: F4 (attribute indexes) and F6 (B+tree
directories) are the consumers, and keeping this arc standalone is what let it land in
parallel with work on the FS core.

The module's own rustdoc is the authoritative specification — this page is the map.

## 1. Why one tree

BeFS's insight, and the roadmap's: an attribute index and a directory are the same data
structure with different keys. F3 builds that structure **once**:

| consumer | key | value |
| --- | --- | --- |
| F4 attribute index (equality) | attribute value bytes | inode id |
| F4 attribute index (range) | length-prefixed `(attr, value)` composite | inode id |
| F6 directory | file name bytes | inode id |
| F8 extent tree (candidate) | big-endian file offset | extent record |

What varies between them is the **key order**, so the order is a plug: any `KeyCmp`
implementation. Three ship today — `LexCmp` (byte-lexicographic: case-sensitive names,
big-endian integer keys, and length-prefixed composites all sort correctly under it),
`U64Cmp` (little-endian u64 keys ordered numerically), and `AsciiFoldCmp` (case-folded
grouping with the exact bytes as a tie-break, so distinct names stay distinct).

The comparator is **not stored on disk**. It is a property of the index, exactly as
BeFS ties a comparator to an index's declared type; the owner opens the tree with the
comparator it was built with. Reads validate that the node's keys really are ascending
under that comparator, so opening a tree with the wrong one fails loudly rather than
silently returning wrong answers.

## 2. Node shape: btrfs-style separators

Every node is one 4096 B slotted block of `(key, value)` entries.

* **Leaf** — the value is the caller's payload.
* **Internal** — the value is an 8-byte LE child block id, and the key is that child's
  **lower bound**, i.e. the smallest key in the child's subtree.

`n` keys therefore address exactly `n` children: there is no ragged "n+1st pointer".
That one decision collapses every structural operation into a single primitive —
"replace this run of the parent's slots with that list of `(first_key, block)` pairs" —
so split (1→2 slots), merge (2→1), borrow (2→2) and plain rewrite (1→1) are the same
patch with different arity, and a parent separator can never go stale, because it is
literally re-read off the child that was just written.

### Format, version 1

```
 off  len  field
   0    8  magic "UNAFSBT1"
   8    2  format version (1)
  10    1  kind: 0 = leaf, 1 = internal
  11    1  reserved
  12    2  entry count
  14    2  reserved
  16    4  heap_start — low-water mark of the heap
  20    4  reserved
  24    8  checksum: FNV-1a over the whole block with these 8 bytes zeroed
  32   32  reserved
  64  8*n  slot directory: key_off u16, key_len u16, val_off u16, val_len u16
 ...       free space
 heap_start .. 4096   heap: key then value per entry, carved DOWNWARD
```

Keys and values cap at 384 B each. That is not arbitrary: with an 8 B slot, a maximum
entry is 776 B, so at least five fit one node's 4032 usable bytes — which is what makes
"a byte-median split always yields two non-empty halves that each fit one block" a
theorem rather than a hope. A compile-time assertion in `tests/btree_kats.rs` holds the
constants to it.

The **checksum is the F7 metadata-checksum line, built in now and verified on every
read** — no node is ever decoded without it. Golden KATs in `tests/btree_kats.rs` pin
the exact bytes of an empty leaf root, a three-entry leaf with uneven key/value widths,
and an internal node with its two children; a drift there is an on-disk format change
and must be a deliberate version bump.

## 3. Copy-on-write

No node block is ever overwritten. A mutation descends to the target leaf keeping the
whole path in RAM, edits the leaf, then walks back up writing **each level to a freshly
allocated block** and repointing its parent's slot — which dirties the parent, and so on
to a new root. `Btree::root()` then names a new block; the tree's owner records it and
swaps it in the owner's transaction. This module never touches the root record.

Two properties follow, and both are proven in the test battery rather than asserted:

* **Snapshots for free.** `Btree::open(prior_root, cmp)` still reads the prior contents,
  because not one block reachable from that root was written. `RefMap`'s
  `current`/`frozen` split is what keeps that true across the retirement `decref`s: a
  block released during a transaction has `current == 0` but `frozen == 1`, and
  `RefMap::allocate` hands out only blocks free in **both** views. Additionally the
  tree parks its own releases until a mutation completes, so even a root captured
  mid-transaction survives the very next mutation.
* **Power-cut convergence.** Every block a mutation writes is unreachable from the old
  root, so a crash before the owner's root swap leaves the old tree bit-identical (the
  fresh blocks are a leak, not a corruption), and a crash after it leaves the new tree
  complete. There is no state in which a reader sees a hybrid.

Fill factors are the standard half-full rule measured in **bytes** (variable-length
entries make a count-based rule meaningless), with merge preferred over borrow when the
two siblings fit one block. `Btree::with_fanout_cap(n)` additionally caps entries per
node; it is a pure in-RAM knob with **no format impact** — a capped tree simply writes
less-full nodes, and any handle reads the result correctly. The tests use it to reach
four-level trees and every merge/borrow path in tens of operations instead of tens of
thousands.

## 4. Cursors

`cursor_first` / `cursor_last` / `cursor_seek` (first entry `>= key`) /
`cursor_seek_back` (last entry `<= key`), stepped by `cursor_next` and `cursor_prev`,
plus a `range(lo, hi, reverse)` convenience over the half-open interval. There are no
sibling pointers on disk: a cursor caches its whole root-to-leaf path, so stepping
inside a leaf is free and crossing a boundary costs one read per level climbed. This is
what F4's "true range queries" — the thing the O(n) catalog scan cannot do — is built
on.

## 5. Hardening

The volume is untrusted. Every field a reader trusts is validated before use: magic,
version, kind, entry count, `heap_start`, each slot's extent against the heap and the
block, the length ceilings, the checksum, ascending key order, child-pointer width,
self-referential children, and a hard `MAX_DEPTH` on every descent and level walk. All
disk-derived arithmetic is `checked_*`; overflow is a clean error, never a wrap. A
malformed node is a `BtreeError`, never a panic and never a wild read — the same posture
as the BEFS-HARDEN work on the codec seam.

## 6. Wiring it up (for F4/F6)

`DeviceStore::new(&mut device, &mut refmap)` is the seam: it borrows the live device and
the live refmap, so tree mutations join the **same** transaction as everything else and
are made durable by the **same** root-sector flip. The only thing the owner must do is
persist `tree.root()` in its own record before committing.
