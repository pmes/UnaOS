// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! F3 Known-Answer Tests — the B+tree NODE byte layout, version 1.
//!
//! The node is a HAND-PACKED little-endian slotted block (not bincode), so its
//! bytes are pinned here directly: header fields at fixed offsets, the slot
//! directory, the downward-growing heap, and the FNV-1a checksum that covers
//! the whole block with its own field read as zeros.
//!
//! Vectors are asserted on the SPARSE spans that carry information — the
//! header, the slot directory, and the heap tail — plus a whole-block digest,
//! so a single hex string does not have to encode four kibibytes of zeros
//! while still pinning every byte of the block.
//!
//! A drift here is an on-disk format change: it breaks every volume already
//! carrying a tree, and must be a deliberate version bump, never a side effect.

use unafs::btree::{
    Btree, DeviceStore, LexCmp, MAX_KEY_LEN, MAX_VALUE_LEN, NODE_HEADER_SIZE, NODE_MAGIC,
    NODE_USABLE, NODE_VERSION, Node, NodeKind, SLOT_SIZE,
};
use unafs::refmap::RefMap;
use unafs::storage::{BLOCK_SIZE, BlockDevice, MemDevice};

fn volume(blocks: u64) -> (MemDevice, RefMap) {
    let mut dev = MemDevice::new();
    let zero = vec![0u8; BLOCK_SIZE as usize];
    for b in 0..blocks {
        dev.write_block(b, &zero).expect("size the volume");
    }
    let mut rm = RefMap::try_new(blocks).expect("refmap");
    rm.incref(0);
    (dev, rm)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Assert a block against its golden spans: the first `head` bytes, the last
/// `tail` bytes, that everything between them is zero, and the FNV-1a digest
/// of the WHOLE block (which is what makes the middle-is-zero claim total).
fn kat_block(what: &str, block: &[u8], head_hex: &str, tail_hex: &str, digest_hex: &str) {
    assert_eq!(block.len(), BLOCK_SIZE as usize, "{what}: not one block");
    let head = unhex(head_hex);
    let tail = unhex(tail_hex);
    assert_eq!(
        hex(&block[..head.len()]),
        head_hex,
        "{what}: HEAD drift — the node header/slot layout changed"
    );
    if !tail.is_empty() {
        assert_eq!(
            hex(&block[block.len() - tail.len()..]),
            tail_hex,
            "{what}: TAIL drift — the node heap layout changed"
        );
    }
    assert!(
        block[head.len()..block.len() - tail.len()]
            .iter()
            .all(|&b| b == 0),
        "{what}: the span between the slots and the heap is not zero-filled"
    );
    assert_eq!(
        format!("{:016x}", unafs::hash::hash_bytes(block)),
        digest_hex,
        "{what}: WHOLE-BLOCK digest drift"
    );
}

// ---------------------------------------------------------------------------
// The format's own constants
// ---------------------------------------------------------------------------

#[test]
fn the_format_constants_are_frozen() {
    assert_eq!(&NODE_MAGIC, b"UNAFSBT1");
    assert_eq!(NODE_VERSION, 1);
    assert_eq!(NODE_HEADER_SIZE, 64);
    assert_eq!(SLOT_SIZE, 8);
    assert_eq!(NODE_USABLE, 4032);
    assert_eq!(BLOCK_SIZE as usize - NODE_HEADER_SIZE, NODE_USABLE);
    assert_eq!(MAX_KEY_LEN, 384);
    assert_eq!(MAX_VALUE_LEN, 384);
}

// The split-always-fits theorem needs at least five max-size entries to share
// a node; if this stops holding, the split proof stops holding. Compile-time,
// because it is a property of the constants, not of any run.
const _: () = assert!(
    NODE_USABLE / (SLOT_SIZE + MAX_KEY_LEN + MAX_VALUE_LEN) >= 5,
    "a node must hold at least five maximum-size entries"
);

// ---------------------------------------------------------------------------
// Golden node images
// ---------------------------------------------------------------------------

#[test]
fn an_empty_leaf_root_is_byte_exact() {
    let (mut dev, mut rm) = volume(8);
    let root = {
        let mut store = DeviceStore::new(&mut dev, &mut rm);
        Btree::create(&mut store, LexCmp).expect("create").root()
    };
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(root, &mut buf).expect("read");
    kat_block(
        "empty leaf root",
        &buf,
        // magic | ver 1 | kind 0 (leaf) | count 0 | heap_start 4096 | cksum | 32 reserved
        "554e4146534254310100000000000000001000000000000006d83b5cce07ff60\
         0000000000000000000000000000000000000000000000000000000000000000",
        "",
        "5257570cee7aa63b",
    );
    // …and it decodes back to what it is.
    let n = Node::decode(root, &buf).expect("decode");
    assert_eq!(n.kind, NodeKind::Leaf);
    assert!(n.entries.is_empty());
}

#[test]
fn a_three_entry_leaf_is_byte_exact() {
    let (mut dev, mut rm) = volume(16);
    let root = {
        let mut store = DeviceStore::new(&mut dev, &mut rm);
        let mut t = Btree::create(&mut store, LexCmp).expect("create");
        // Deliberately UNEVEN key and value widths, so the slot directory and
        // the downward heap carving are both pinned by real variation.
        t.insert(&mut store, b"a", b"1").expect("i");
        t.insert(&mut store, b"bb", b"22").expect("i");
        t.insert(&mut store, b"ccc", b"333").expect("i");
        t.root()
    };
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(root, &mut buf).expect("read");
    kat_block(
        "three-entry leaf",
        &buf,
        // header: leaf, count 3, heap_start 4084 (0x0ff4), checksum, reserved…
        "554e4146534254310100000003000000f40f00000000000038745d6a3cc619a0\
         0000000000000000000000000000000000000000000000000000000000000000\
         ff0f0100fe0f0100fc0f0200fa0f0200f70f0300f40f0300",
        // heap, low address first: "333" "ccc" "22" "bb" "1" "a"
        "333333636363323262623161",
        "5a571c6e840bca20",
    );
    let n = Node::decode(root, &buf).expect("decode");
    assert_eq!(n.kind, NodeKind::Leaf);
    assert_eq!(
        n.entries,
        vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"bb".to_vec(), b"22".to_vec()),
            (b"ccc".to_vec(), b"333".to_vec()),
        ]
    );
}

#[test]
fn an_internal_node_and_its_leaves_are_byte_exact() {
    // A fanout cap of 4 makes the root split at the fifth key: the smallest
    // deterministic tree that pins the INTERNAL node layout (separator key =
    // the child's first key, value = an 8 byte LE child block id).
    let (mut dev, mut rm) = volume(32);
    let root = {
        let mut store = DeviceStore::new(&mut dev, &mut rm);
        let mut t = Btree::create(&mut store, LexCmp)
            .expect("create")
            .with_fanout_cap(4);
        for i in 0..5u8 {
            t.insert(&mut store, &[b'k', b'0' + i], &[b'v', b'0' + i])
                .expect("i");
        }
        t.root()
    };
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(root, &mut buf).expect("read");
    let n = Node::decode(root, &buf).expect("decode");
    assert_eq!(n.kind, NodeKind::Internal);
    assert_eq!(n.entries.len(), 2, "the split produced two children");
    // The byte-median split of 5 equal-width entries puts 3 left, 2 right.
    assert_eq!(n.entries[0].0, b"k0".to_vec(), "separator 0 = child 0's first key");
    assert_eq!(n.entries[1].0, b"k3".to_vec(), "separator 1 = child 1's first key");
    assert_eq!(n.entries[0].1.len(), 8, "a child pointer is 8 bytes");

    kat_block(
        "internal root",
        &buf,
        // header: kind 1 (internal), count 2, heap_start 4076 (0x0fec)
        "554e4146534254310100010002000000ec0f000000000000e03f3502679ebef0\
         0000000000000000000000000000000000000000000000000000000000000000\
         fe0f0200f60f0800f40f0200ec0f0800",
        // heap, low address first: child ptr 3, key "k3", child ptr 2, key "k0"
        "03000000000000006b3302000000000000006b30",
        "b300ba5886f2ece1",
    );

    // Both leaves, reached through the pinned pointers.
    let c0 = u64::from_le_bytes(n.entries[0].1.clone().try_into().unwrap());
    let c1 = u64::from_le_bytes(n.entries[1].1.clone().try_into().unwrap());
    let mut lbuf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(c0, &mut lbuf).expect("read");
    let l0 = Node::decode(c0, &lbuf).expect("decode");
    assert_eq!(l0.kind, NodeKind::Leaf);
    assert_eq!(
        l0.entries,
        vec![
            (b"k0".to_vec(), b"v0".to_vec()),
            (b"k1".to_vec(), b"v1".to_vec()),
            (b"k2".to_vec(), b"v2".to_vec()),
        ]
    );
    dev.read_block(c1, &mut lbuf).expect("read");
    let l1 = Node::decode(c1, &lbuf).expect("decode");
    assert_eq!(l1.kind, NodeKind::Leaf);
    assert_eq!(
        l1.entries,
        vec![
            (b"k3".to_vec(), b"v3".to_vec()),
            (b"k4".to_vec(), b"v4".to_vec()),
        ]
    );
}

// ---------------------------------------------------------------------------
// Encode/decode round-trip laws
// ---------------------------------------------------------------------------

#[test]
fn every_encode_round_trips_through_decode() {
    let cases: Vec<Node> = vec![
        Node::empty(NodeKind::Leaf),
        Node {
            kind: NodeKind::Leaf,
            entries: vec![(vec![], vec![])],
        },
        Node {
            kind: NodeKind::Leaf,
            entries: vec![
                (vec![0u8], vec![0xffu8; 300]),
                (vec![1u8; MAX_KEY_LEN], vec![]),
                (vec![2u8], vec![7u8; MAX_VALUE_LEN]),
            ],
        },
        Node {
            kind: NodeKind::Internal,
            entries: vec![
                (vec![b'a'], 1u64.to_le_bytes().to_vec()),
                (vec![b'b'], u64::MAX.to_le_bytes().to_vec()),
            ],
        },
    ];
    for (i, node) in cases.iter().enumerate() {
        let buf = node.encode().expect("encode");
        assert_eq!(buf.len(), BLOCK_SIZE as usize);
        let back = Node::decode(0, &buf).expect("decode");
        assert_eq!(&back, node, "case {i} did not round-trip");
        // Re-encoding the decoded node reproduces the bytes exactly — the
        // encoding is a function of the value, with no hidden state.
        assert_eq!(back.encode().expect("re-encode"), buf, "case {i} is unstable");
    }
}

#[test]
fn a_node_that_cannot_fit_is_refused_at_encode_time() {
    // Six maximum-size entries exceed one block.
    let node = Node {
        kind: NodeKind::Leaf,
        entries: (0..6u8)
            .map(|i| {
                let mut k = vec![b'k'; MAX_KEY_LEN];
                k[0] = i;
                (k, vec![b'v'; MAX_VALUE_LEN])
            })
            .collect(),
    };
    assert!(node.used_bytes().expect("used") > NODE_USABLE);
    assert!(node.encode().is_err(), "an over-full node must not encode");
}

#[test]
fn the_checksum_covers_its_own_field_as_zeros() {
    let node = Node {
        kind: NodeKind::Leaf,
        entries: vec![(b"key".to_vec(), b"value".to_vec())],
    };
    let buf = node.encode().expect("encode");
    let stored = u64::from_le_bytes(buf[24..32].try_into().unwrap());
    let mut zeroed = buf.clone();
    zeroed[24..32].fill(0);
    assert_eq!(
        stored,
        unafs::hash::hash_bytes(&zeroed),
        "the checksum rule is FNV-1a over the block with the field zeroed"
    );
    assert_ne!(stored, 0, "a real node has a real checksum");
}
