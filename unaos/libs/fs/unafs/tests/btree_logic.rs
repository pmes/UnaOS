// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! F3: the generic CoW B+tree — the behavioural gate.
//!
//! Every test here is DETERMINISTIC: the randomized batteries run a seeded
//! xorshift, never a clock, so a failure is reproducible from the seed alone.
//!
//! What is proven, in order:
//!   * insert / lookup / delete against a `BTreeMap` oracle over thousands of
//!     randomized operations, uncapped (real 4 KiB fanout) and capped;
//!   * splits and merges are actually EXERCISED — asserted on depth and node
//!     counts, not assumed;
//!   * range cursors, forward and reverse, against oracle ranges;
//!   * copy-on-write discipline: a mutation writes NO block the prior root can
//!     reach, and the prior root still reads the prior contents (snapshots for
//!     free);
//!   * power-cut simulation at every commit boundary converges old-or-new;
//!   * a malformed node (bad checksum / magic / version / geometry) is refused
//!     cleanly.

use std::collections::BTreeMap;
use unafs::btree::{
    AsciiFoldCmp, Btree, BtreeError, DeviceStore, KeyCmp, LexCmp, MAX_KEY_LEN, MAX_VALUE_LEN,
    Node, NodeStore, U64Cmp,
};
use unafs::refmap::RefMap;
use unafs::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError, MemDevice};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A sized volume: a `MemDevice` grown to `blocks` blocks plus a matching
/// `RefMap`. Block 0 is reserved so a "0" pointer is never a valid node.
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

/// Deterministic xorshift64* — no clock, no OS entropy.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn key_of(n: u64) -> Vec<u8> {
    // Big-endian so plain lexicographic order IS numeric order — the exact
    // trick an F4 u64-keyed index will use.
    n.to_be_bytes().to_vec()
}

fn val_of(n: u64) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"v:");
    v.extend_from_slice(&n.to_le_bytes());
    v
}

// ---------------------------------------------------------------------------
// Oracle batteries
// ---------------------------------------------------------------------------

/// Run `ops` randomized insert/remove/lookup operations against a `BTreeMap`
/// oracle and assert perfect agreement, then a full ordered scan.
fn oracle_battery(seed: u64, ops: u64, key_space: u64, cap: Option<usize>, blocks: u64) -> usize {
    let (mut dev, mut rm) = volume(blocks);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp).expect("create");
    if let Some(c) = cap {
        tree = tree.with_fanout_cap(c);
    }
    let mut oracle: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut rng = Rng::new(seed);

    for i in 0..ops {
        let k = key_of(rng.below(key_space));
        match rng.below(100) {
            // 55% insert, 30% remove, 15% lookup — enough churn to force both
            // growth and shrinkage in the same run.
            0..=54 => {
                let v = val_of(i);
                let got = tree.insert(&mut store, &k, &v).expect("insert");
                let want = oracle.insert(k.clone(), v);
                assert_eq!(got, want, "insert displaced value mismatch at op {i}");
            }
            55..=84 => {
                let got = tree.remove(&mut store, &k).expect("remove");
                let want = oracle.remove(&k);
                assert_eq!(got, want, "remove returned value mismatch at op {i}");
            }
            _ => {
                let got = tree.lookup(&mut store, &k).expect("lookup");
                assert_eq!(got, oracle.get(&k).cloned(), "lookup mismatch at op {i}");
            }
        }
    }

    // Every key, one by one.
    for (k, v) in &oracle {
        assert_eq!(
            tree.lookup(&mut store, k).expect("lookup"),
            Some(v.clone()),
            "post-run lookup miss"
        );
    }
    // And the full ordered scan.
    let scan = tree.range(&mut store, None, None, false).expect("scan");
    let want: Vec<(Vec<u8>, Vec<u8>)> = oracle.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(scan, want, "ordered scan differs from the oracle");

    let stats = tree.stats(&mut store).expect("stats");
    assert_eq!(stats.entries, oracle.len(), "entry count differs");
    oracle.len()
}

#[test]
fn oracle_uncapped_thousands_of_ops() {
    // Real 4 KiB fanout: ~168 small entries per leaf, so 6000 ops over a 4000
    // key space builds a genuine multi-level tree.
    let live = oracle_battery(0x5eed_0001, 6000, 4000, None, 4096);
    assert!(live > 500, "battery left only {live} live keys — too shallow");
}

#[test]
fn oracle_capped_fanout_thousands_of_ops() {
    // A tiny fanout drives split/merge/borrow on nearly every operation.
    oracle_battery(0x5eed_0002, 4000, 300, Some(4), 4096);
    oracle_battery(0x5eed_0003, 4000, 300, Some(7), 4096);
    oracle_battery(0x5eed_0004, 4000, 1500, Some(12), 4096);
}

#[test]
fn oracle_large_values_force_byte_median_splits() {
    // Values near the format ceiling: leaves hold only a handful of entries,
    // so the BYTE-median split path (not the count path) is what runs.
    let (mut dev, mut rm) = volume(4096);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp).expect("create");
    let mut oracle: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut rng = Rng::new(0x5eed_0005);
    for i in 0..600u64 {
        let k = key_of(rng.below(400));
        let len = 1 + (rng.below(MAX_VALUE_LEN as u64)) as usize;
        let v = vec![(i % 251) as u8; len];
        assert_eq!(
            tree.insert(&mut store, &k, &v).expect("insert"),
            oracle.insert(k, v)
        );
    }
    let scan = tree.range(&mut store, None, None, false).expect("scan");
    let want: Vec<(Vec<u8>, Vec<u8>)> = oracle.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(scan, want);
    let stats = tree.stats(&mut store).expect("stats");
    assert!(
        stats.depth >= 2,
        "large values should have split the root; depth {}",
        stats.depth
    );
}

// ---------------------------------------------------------------------------
// Split and merge, proven on the shape
// ---------------------------------------------------------------------------

#[test]
fn splits_grow_the_tree_and_merges_shrink_it_back() {
    let (mut dev, mut rm) = volume(2048);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);

    let start = tree.stats(&mut store).expect("stats");
    assert_eq!(start.depth, 1, "a fresh tree is one leaf");
    assert_eq!(start.nodes, 1);

    // --- growth: every insert is sequential, so splits are forced.
    let n = 400u64;
    let mut peak_depth = 1;
    let mut peak_nodes = 1;
    for i in 0..n {
        tree.insert(&mut store, &key_of(i), &val_of(i))
            .expect("insert");
        let st = tree.stats(&mut store).expect("stats");
        assert_eq!(st.entries as u64, i + 1, "entry count tracks the inserts");
        peak_depth = peak_depth.max(st.depth);
        peak_nodes = peak_nodes.max(st.nodes);
    }
    assert!(
        peak_depth >= 4,
        "fanout 4 over {n} keys must build at least 4 levels; got {peak_depth}"
    );
    assert!(
        peak_nodes >= 100,
        "expected a wide tree; got {peak_nodes} nodes"
    );

    // --- shrinkage: delete everything and watch the merges collapse it.
    let mut min_depth_seen = peak_depth;
    for i in 0..n {
        assert_eq!(
            tree.remove(&mut store, &key_of(i)).expect("remove"),
            Some(val_of(i))
        );
        let st = tree.stats(&mut store).expect("stats");
        assert_eq!(st.entries as u64, n - i - 1);
        min_depth_seen = min_depth_seen.min(st.depth);
    }
    let end = tree.stats(&mut store).expect("stats");
    assert_eq!(end.depth, 1, "an emptied tree collapses back to one leaf");
    assert_eq!(end.nodes, 1, "…and to a single node");
    assert_eq!(end.entries, 0);
    assert!(
        min_depth_seen < peak_depth,
        "merges never reduced the depth ({min_depth_seen} vs peak {peak_depth})"
    );

    // The freed nodes really came back: after a freeze the volume is as free
    // as it was at the start, bar the one live root.
    let live = tree.reachable_blocks(&mut store).expect("reachable");
    assert_eq!(live.len(), 1);
}

#[test]
fn deleting_in_reverse_also_merges_to_a_single_leaf() {
    let (mut dev, mut rm) = volume(2048);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(5);
    for i in 0..300u64 {
        tree.insert(&mut store, &key_of(i), &val_of(i)).expect("i");
    }
    for i in (0..300u64).rev() {
        assert_eq!(
            tree.remove(&mut store, &key_of(i)).expect("r"),
            Some(val_of(i))
        );
    }
    let st = tree.stats(&mut store).expect("stats");
    assert_eq!((st.depth, st.nodes, st.entries), (1, 1, 0));
}

#[test]
fn removing_the_minimum_repoints_every_separator_up_to_the_root() {
    // The separator for a child IS the child's first key, so deleting the
    // running minimum rewrites a separator at every level — the case that
    // silently corrupts trees whose parents keep stale separators.
    let (mut dev, mut rm) = volume(2048);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);
    let n = 250u64;
    for i in 0..n {
        tree.insert(&mut store, &key_of(i), &val_of(i)).expect("i");
    }
    for i in 0..n {
        assert_eq!(
            tree.remove(&mut store, &key_of(i)).expect("r"),
            Some(val_of(i)),
            "removing the running minimum {i}"
        );
        // The key is gone, and every survivor above it is still findable.
        assert_eq!(tree.lookup(&mut store, &key_of(i)).expect("l"), None);
        for probe in [i + 1, (i + n) / 2, n - 1] {
            if probe > i && probe < n {
                assert_eq!(
                    tree.lookup(&mut store, &key_of(probe)).expect("l"),
                    Some(val_of(probe)),
                    "survivor {probe} lost after removing minimum {i}"
                );
            }
        }
    }
}

#[test]
fn inserting_a_new_minimum_repoints_separators_downward() {
    let (mut dev, mut rm) = volume(2048);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);
    for i in (0..300u64).rev() {
        tree.insert(&mut store, &key_of(i), &val_of(i)).expect("i");
        assert_eq!(
            tree.lookup(&mut store, &key_of(i)).expect("l"),
            Some(val_of(i))
        );
    }
    let scan = tree.range(&mut store, None, None, false).expect("scan");
    assert_eq!(scan.len(), 300);
    for (i, (k, _)) in scan.iter().enumerate() {
        assert_eq!(k, &key_of(i as u64), "scan out of order at {i}");
    }
}

// ---------------------------------------------------------------------------
// Cursors and ranges
// ---------------------------------------------------------------------------

#[test]
fn range_cursors_match_the_oracle_forward_and_reverse() {
    let (mut dev, mut rm) = volume(4096);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(6);
    let mut oracle: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut rng = Rng::new(0x5eed_0100);
    for _ in 0..900u64 {
        let n = rng.below(2000);
        tree.insert(&mut store, &key_of(n), &val_of(n)).expect("i");
        oracle.insert(key_of(n), val_of(n));
    }

    let mut rng = Rng::new(0x5eed_0101);
    for _ in 0..200 {
        let a = rng.below(2100);
        let b = rng.below(2100);
        let (lo, hi) = (a.min(b), a.max(b));
        let (lk, hk) = (key_of(lo), key_of(hi));

        let want: Vec<(Vec<u8>, Vec<u8>)> = oracle
            .range(lk.clone()..hk.clone())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let fwd = tree
            .range(&mut store, Some(&lk), Some(&hk), false)
            .expect("fwd");
        assert_eq!(fwd, want, "forward range [{lo},{hi}) differs");

        let mut want_rev = want.clone();
        want_rev.reverse();
        let rev = tree
            .range(&mut store, Some(&lk), Some(&hk), true)
            .expect("rev");
        assert_eq!(rev, want_rev, "reverse range [{lo},{hi}) differs");
    }

    // Open bounds, both directions.
    let all: Vec<(Vec<u8>, Vec<u8>)> = oracle.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(tree.range(&mut store, None, None, false).expect("f"), all);
    let mut rall = all.clone();
    rall.reverse();
    assert_eq!(tree.range(&mut store, None, None, true).expect("r"), rall);
}

#[test]
fn cursors_step_both_ways_across_leaf_boundaries() {
    let (mut dev, mut rm) = volume(2048);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);
    let n = 200u64;
    for i in 0..n {
        tree.insert(&mut store, &key_of(i), &val_of(i)).expect("i");
    }

    // Forward from the start.
    let mut c = tree.cursor_first(&mut store).expect("first");
    for i in 0..n {
        let (k, v) = c.current().expect("cursor has an entry");
        assert_eq!((k.to_vec(), v.to_vec()), (key_of(i), val_of(i)));
        tree.cursor_next(&mut store, &mut c).expect("next");
    }
    assert!(!c.is_valid(), "cursor should be exhausted past the last key");

    // Reverse from the end.
    let mut c = tree.cursor_last(&mut store).expect("last");
    for i in (0..n).rev() {
        let (k, v) = c.current().expect("cursor has an entry");
        assert_eq!((k.to_vec(), v.to_vec()), (key_of(i), val_of(i)));
        tree.cursor_prev(&mut store, &mut c).expect("prev");
    }
    assert!(!c.is_valid(), "cursor should be exhausted before the first key");

    // A seek that lands between entries: only even keys present.
    let (mut dev2, mut rm2) = volume(2048);
    let mut store2 = DeviceStore::new(&mut dev2, &mut rm2);
    let mut sparse = Btree::create(&mut store2, LexCmp)
        .expect("create")
        .with_fanout_cap(4);
    for i in (0..100u64).step_by(2) {
        sparse.insert(&mut store2, &key_of(i), &val_of(i)).expect("i");
    }
    for i in (1..99u64).step_by(2) {
        let c = sparse.cursor_seek(&mut store2, &key_of(i)).expect("seek");
        assert_eq!(
            c.current().map(|(k, _)| k.to_vec()),
            Some(key_of(i + 1)),
            "forward seek past a gap at {i}"
        );
        let c = sparse
            .cursor_seek_back(&mut store2, &key_of(i))
            .expect("seek_back");
        assert_eq!(
            c.current().map(|(k, _)| k.to_vec()),
            Some(key_of(i - 1)),
            "reverse seek past a gap at {i}"
        );
    }
    // Off both ends.
    let c = sparse.cursor_seek(&mut store2, &key_of(1000)).expect("s");
    assert!(!c.is_valid());
    let c = sparse.cursor_seek_back(&mut store2, &[]).expect("s");
    assert!(!c.is_valid(), "nothing sorts at or below the empty key here");
}

#[test]
fn an_empty_tree_has_exhausted_cursors_and_empty_ranges() {
    let (mut dev, mut rm) = volume(64);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let tree = Btree::create(&mut store, LexCmp).expect("create");
    assert!(!tree.cursor_first(&mut store).expect("f").is_valid());
    assert!(!tree.cursor_last(&mut store).expect("l").is_valid());
    assert!(!tree.cursor_seek(&mut store, b"x").expect("s").is_valid());
    assert!(tree.range(&mut store, None, None, false).expect("r").is_empty());
    assert!(tree.range(&mut store, None, None, true).expect("r").is_empty());
    assert_eq!(tree.lookup(&mut store, b"x").expect("lk"), None);
    let mut t = tree;
    assert_eq!(t.remove(&mut store, b"x").expect("rm"), None);
}

// ---------------------------------------------------------------------------
// Comparators
// ---------------------------------------------------------------------------

#[test]
fn the_same_tree_serves_three_key_orders() {
    // u64 keys, LE, numerically ordered by U64Cmp.
    let (mut dev, mut rm) = volume(1024);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut t = Btree::create(&mut store, U64Cmp)
        .expect("create")
        .with_fanout_cap(4);
    for n in [300u64, 1, 65536, 255, 256, 0, u64::MAX] {
        t.insert(&mut store, &n.to_le_bytes(), b"x").expect("i");
    }
    let order: Vec<u64> = t
        .range(&mut store, None, None, false)
        .expect("scan")
        .into_iter()
        .map(|(k, _)| u64::from_le_bytes(k.try_into().unwrap()))
        .collect();
    assert_eq!(order, vec![0, 1, 255, 256, 300, 65536, u64::MAX]);

    // Case-sensitive names, plain lexicographic.
    let (mut dev, mut rm) = volume(1024);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut t = Btree::create(&mut store, LexCmp).expect("create");
    for n in ["Zebra", "apple", "Apple", "banana"] {
        t.insert(&mut store, n.as_bytes(), b"x").expect("i");
    }
    let order: Vec<String> = t
        .range(&mut store, None, None, false)
        .expect("scan")
        .into_iter()
        .map(|(k, _)| String::from_utf8(k).unwrap())
        .collect();
    assert_eq!(order, vec!["Apple", "Zebra", "apple", "banana"]);

    // Case-folded names: grouped case-insensitively, still distinct.
    let (mut dev, mut rm) = volume(1024);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut t = Btree::create(&mut store, AsciiFoldCmp).expect("create");
    for n in ["Zebra", "apple", "Apple", "banana"] {
        t.insert(&mut store, n.as_bytes(), b"x").expect("i");
    }
    let order: Vec<String> = t
        .range(&mut store, None, None, false)
        .expect("scan")
        .into_iter()
        .map(|(k, _)| String::from_utf8(k).unwrap())
        .collect();
    assert_eq!(order, vec!["Apple", "apple", "banana", "Zebra"]);
    assert_eq!(t.lookup(&mut store, b"Apple").expect("l"), Some(b"x".to_vec()));
    assert_eq!(t.lookup(&mut store, b"apple").expect("l"), Some(b"x".to_vec()));
}

#[test]
fn composite_keys_sort_component_wise_under_lex() {
    // The F4/F6 composite-key shape: length-prefixed components under LexCmp.
    fn composite(a: &str, b: u64) -> Vec<u8> {
        let mut k = Vec::new();
        k.push(a.len() as u8);
        k.extend_from_slice(a.as_bytes());
        k.extend_from_slice(&b.to_be_bytes());
        k
    }
    let (mut dev, mut rm) = volume(1024);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut t = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);
    for (a, b) in [("cat", 2u64), ("cat", 1), ("ant", 9), ("cat", 10), ("dog", 0)] {
        t.insert(&mut store, &composite(a, b), b"x").expect("i");
    }
    let got: Vec<Vec<u8>> = t
        .range(&mut store, None, None, false)
        .expect("scan")
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        got,
        vec![
            composite("ant", 9),
            composite("cat", 1),
            composite("cat", 2),
            composite("cat", 10),
            composite("dog", 0),
        ]
    );
    // And the range query that F4 wants: every entry for the "cat" attribute.
    let lo = composite("cat", 0);
    let hi = composite("cat", u64::MAX);
    let cats = t.range(&mut store, Some(&lo), Some(&hi), false).expect("r");
    assert_eq!(
        cats,
        vec![
            (composite("cat", 1), b"x".to_vec()),
            (composite("cat", 2), b"x".to_vec()),
            (composite("cat", 10), b"x".to_vec()),
        ],
        "an attribute-scoped range query returns exactly that attribute's rows"
    );
}

// ---------------------------------------------------------------------------
// Copy-on-write discipline
// ---------------------------------------------------------------------------

#[test]
fn a_mutation_writes_no_block_the_prior_root_can_reach() {
    let (mut dev, mut rm) = volume(4096);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);
    for i in 0..300u64 {
        tree.insert(&mut store, &key_of(i), &val_of(i)).expect("i");
    }

    let mut rng = Rng::new(0x5eed_0200);
    for round in 0..120u64 {
        // The "owner commits" step: freeze retires the previous tree.
        store.refmap.freeze();
        let prior = Btree::open(tree.root(), LexCmp);
        let live: std::collections::BTreeSet<u64> = prior
            .reachable_blocks(&mut store)
            .expect("reachable")
            .into_iter()
            .collect();
        let before = prior.range(&mut store, None, None, false).expect("before");

        // Mutate through a store that records every block written.
        let mut witness = WitnessStore {
            inner: &mut store,
            written: Vec::new(),
        };
        let k = key_of(rng.below(400));
        let changed = if round % 3 == 0 {
            tree.remove(&mut witness, &k).expect("remove").is_some()
        } else {
            tree.insert(&mut witness, &k, &val_of(round)).expect("insert");
            true
        };
        let written = witness.written.clone();

        for b in &written {
            assert!(
                !live.contains(b),
                "round {round}: block {b} belongs to the prior root and was OVERWRITTEN"
            );
        }
        // …and the prior root still reads exactly what it read before.
        let after = prior.range(&mut store, None, None, false).expect("after");
        assert_eq!(before, after, "round {round}: the prior root's contents moved");
        if changed {
            assert!(!written.is_empty(), "round {round}: a change wrote nothing");
            assert_ne!(tree.root(), prior.root(), "the root must move on a change");
        } else {
            // A remove that hit nothing is a pure read: no writes, same root.
            assert!(written.is_empty(), "round {round}: a miss wrote blocks");
            assert_eq!(tree.root(), prior.root(), "a miss must not move the root");
        }
    }
}

#[test]
fn a_chain_of_snapshots_each_reads_its_own_generation() {
    let (mut dev, mut rm) = volume(4096);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut tree = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);

    // Generation g holds keys 0..(g*10). Every generation is "committed"
    // (frozen) and its root retained, so NONE of its blocks may be reused.
    let mut roots = Vec::new();
    let mut next = 0u64;
    for g in 1..=8u64 {
        while next < g * 10 {
            tree.insert(&mut store, &key_of(next), &val_of(next)).expect("i");
            next += 1;
        }
        // Retain: incref every block this generation reaches, then freeze.
        for b in tree.reachable_blocks(&mut store).expect("reach") {
            store.refmap.incref(b);
        }
        store.refmap.freeze();
        roots.push(tree.root());
    }
    // Churn hard on the live tree.
    let mut rng = Rng::new(0x5eed_0300);
    for i in 0..500u64 {
        let k = key_of(rng.below(200));
        if i % 2 == 0 {
            tree.insert(&mut store, &k, b"churn").expect("i");
        } else {
            tree.remove(&mut store, &k).expect("r");
        }
        store.refmap.freeze();
    }
    // Every retained generation still reads its own contents.
    for (i, &root) in roots.iter().enumerate() {
        let g = (i + 1) as u64;
        let snap = Btree::open(root, LexCmp);
        let got = snap.range(&mut store, None, None, false).expect("snap scan");
        let want: Vec<(Vec<u8>, Vec<u8>)> =
            (0..g * 10).map(|n| (key_of(n), val_of(n))).collect();
        assert_eq!(got, want, "generation {g} no longer reads its own contents");
    }
}

/// A [`NodeStore`] that records every block written through it.
struct WitnessStore<'a, S: NodeStore> {
    inner: &'a mut S,
    written: Vec<u64>,
}

impl<S: NodeStore> NodeStore for WitnessStore<'_, S> {
    fn alloc(&mut self) -> Result<u64, BtreeError> {
        self.inner.alloc()
    }
    fn release(&mut self, block: u64) {
        self.inner.release(block)
    }
    fn read(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BtreeError> {
        self.inner.read(block, buf)
    }
    fn write(&mut self, block: u64, buf: &[u8]) -> Result<(), BtreeError> {
        self.written.push(block);
        self.inner.write(block, buf)
    }
}

// ---------------------------------------------------------------------------
// Power-cut simulation
// ---------------------------------------------------------------------------

/// A store that performs the first `budget` writes and then fails every one —
/// the power cut. Reads and allocations keep working, exactly as they would in
/// the surviving half of a torn transaction.
struct CutStore<'a, S: NodeStore> {
    inner: &'a mut S,
    budget: usize,
    done: usize,
}

impl<S: NodeStore> NodeStore for CutStore<'_, S> {
    fn alloc(&mut self) -> Result<u64, BtreeError> {
        self.inner.alloc()
    }
    fn release(&mut self, block: u64) {
        self.inner.release(block)
    }
    fn read(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BtreeError> {
        self.inner.read(block, buf)
    }
    fn write(&mut self, block: u64, buf: &[u8]) -> Result<(), BtreeError> {
        if self.done >= self.budget {
            return Err(BtreeError::Storage(StorageError::Io("power cut".into())));
        }
        self.done += 1;
        self.inner.write(block, buf)
    }
}

#[test]
fn a_power_cut_at_any_write_boundary_converges_old_or_new() {
    // For EVERY prefix of the writes a mutation performs, cut the power there
    // and assert the volume still presents the OLD tree (because the owner
    // never got to swap the root) — and that letting the mutation finish
    // presents the NEW tree.
    for (seed, op) in [(0x5eed_0400u64, 0u8), (0x5eed_0401, 1)] {
        let (mut dev, mut rm) = volume(4096);
        let mut store = DeviceStore::new(&mut dev, &mut rm);
        let mut tree = Btree::create(&mut store, LexCmp)
            .expect("create")
            .with_fanout_cap(4);
        let mut rng = Rng::new(seed);
        for i in 0..200u64 {
            tree.insert(&mut store, &key_of(i), &val_of(i)).expect("i");
        }
        // The owner's commit: this generation is now the committed tree.
        store.refmap.freeze();

        let committed_root = tree.root();
        let committed = Btree::open(committed_root, LexCmp);
        let before = committed.range(&mut store, None, None, false).expect("b");

        let k = key_of(rng.below(200));
        // How many writes does the whole mutation take? Measure once on a
        // throwaway clone of the tree handle.
        let mut probe = tree.clone();
        let total = {
            let mut w = WitnessStore {
                inner: &mut store,
                written: Vec::new(),
            };
            if op == 0 {
                probe.insert(&mut w, &k, b"new").expect("probe");
            } else {
                probe.remove(&mut w, &k).expect("probe");
            }
            w.written.len()
        };
        assert!(total > 0, "the probe mutation wrote nothing");

        for budget in 0..total {
            // Fresh volume state per cut: rebuild from the committed image.
            let (mut d2, mut r2) = volume(4096);
            let mut s2 = DeviceStore::new(&mut d2, &mut r2);
            let mut t2 = Btree::create(&mut s2, LexCmp)
                .expect("create")
                .with_fanout_cap(4);
            for i in 0..200u64 {
                t2.insert(&mut s2, &key_of(i), &val_of(i)).expect("i");
            }
            s2.refmap.freeze();
            let old_root = t2.root();

            let mut cut = CutStore {
                inner: &mut s2,
                budget,
                done: 0,
            };
            let mut victim = t2.clone();
            let res = if op == 0 {
                victim.insert(&mut cut, &k, b"new")
            } else {
                victim.remove(&mut cut, &k)
            };
            assert!(
                res.is_err(),
                "budget {budget} of {total}: the mutation should have been cut short"
            );
            // The owner never swapped the root: the OLD tree is what a
            // remount sees, and it is bit-for-bit what it always was.
            let survivor = Btree::open(old_root, LexCmp);
            let after = survivor.range(&mut s2, None, None, false).expect("a");
            assert_eq!(
                after, before,
                "budget {budget} of {total}: the old tree did not survive the cut"
            );
        }

        // The uncut mutation lands the NEW tree, and it is exactly the oracle.
        let mut expect: BTreeMap<Vec<u8>, Vec<u8>> =
            before.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
        if op == 0 {
            expect.insert(k.clone(), b"new".to_vec());
            tree.insert(&mut store, &k, b"new").expect("final");
        } else {
            expect.remove(&k);
            tree.remove(&mut store, &k).expect("final");
        }
        let got = tree.range(&mut store, None, None, false).expect("g");
        let want: Vec<(Vec<u8>, Vec<u8>)> =
            expect.into_iter().collect();
        assert_eq!(got, want, "the completed mutation must present the NEW tree");
        // …and the pre-mutation root is STILL the old tree.
        assert_eq!(
            committed.range(&mut store, None, None, false).expect("c"),
            before
        );
    }
}

// ---------------------------------------------------------------------------
// Malformed nodes
// ---------------------------------------------------------------------------

/// Build a small tree and hand back the volume plus the root block.
fn small_tree() -> (MemDevice, RefMap, u64) {
    let (mut dev, mut rm) = volume(256);
    let root = {
        let mut store = DeviceStore::new(&mut dev, &mut rm);
        let mut t = Btree::create(&mut store, LexCmp)
            .expect("create")
            .with_fanout_cap(4);
        for i in 0..40u64 {
            t.insert(&mut store, &key_of(i), &val_of(i)).expect("i");
        }
        t.root()
    };
    (dev, rm, root)
}

#[test]
fn a_flipped_byte_anywhere_in_a_node_is_refused_by_the_checksum() {
    // Sweep the header's reserved span, the slot directory, and the heap. Not
    // one of these flips is repaired, so the CHECKSUM is what must catch them
    // — this is the F7 metadata-checksum line, verified on read today.
    for off in [11usize, 40, 63, 64, 70, 2048, 4000, 4095] {
        let (mut dev, mut rm, root) = corrupt_root(|b| b[off] ^= 0x01);
        let mut store = DeviceStore::new(&mut dev, &mut rm);
        let tree = Btree::open(root, LexCmp);
        let err = tree
            .lookup(&mut store, &key_of(0))
            .expect_err("a corrupted node must be refused");
        assert!(
            matches!(err, BtreeError::BadChecksum(_)),
            "byte {off}: expected a checksum refusal, got {err:?}"
        );
    }
}

#[test]
fn bad_magic_bad_version_and_bad_geometry_are_each_named() {
    // Magic (checked before the checksum, so no repair needed).
    let (mut dev, mut rm, root) = corrupt_root(|b| b[0] = b'X');
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    assert!(matches!(
        Btree::open(root, LexCmp)
            .lookup(&mut store, b"k")
            .expect_err("magic"),
        BtreeError::BadMagic(_)
    ));

    // Version — bumped, then re-checksummed so the version check is what fires.
    let (mut dev, mut rm, root) = corrupt_root_resum(|b| b[8..10].copy_from_slice(&7u16.to_le_bytes()));
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    assert!(matches!(
        Btree::open(root, LexCmp)
            .lookup(&mut store, b"k")
            .expect_err("version"),
        BtreeError::BadVersion(_, 7)
    ));

    // Geometry: a heap mark that overlaps the slot directory.
    let (mut dev, mut rm, root) = corrupt_root_resum(|b| b[16..20].copy_from_slice(&0u32.to_le_bytes()));
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    assert!(matches!(
        Btree::open(root, LexCmp)
            .lookup(&mut store, b"k")
            .expect_err("geometry"),
        BtreeError::Malformed(_, _)
    ));

    // A slot whose key extent runs off the end of the block.
    let (mut dev, mut rm, root) = corrupt_root_resum(|b| {
        b[64..66].copy_from_slice(&4090u16.to_le_bytes());
        b[66..68].copy_from_slice(&300u16.to_le_bytes());
    });
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    assert!(matches!(
        Btree::open(root, LexCmp)
            .lookup(&mut store, b"k")
            .expect_err("slot extent"),
        BtreeError::Malformed(_, _)
    ));

    // Descending key order — the invariant a checksum cannot catch.
    let (mut dev, mut rm, root) = corrupt_root_resum(|b| {
        // Swap the first two slots' directory entries.
        let mut a = [0u8; 8];
        let mut c = [0u8; 8];
        a.copy_from_slice(&b[64..72]);
        c.copy_from_slice(&b[72..80]);
        b[64..72].copy_from_slice(&c);
        b[72..80].copy_from_slice(&a);
    });
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    assert!(matches!(
        Btree::open(root, LexCmp)
            .lookup(&mut store, b"k")
            .expect_err("order"),
        BtreeError::Malformed(_, _)
    ));
}

/// Corrupt the root block WITHOUT repairing the checksum.
fn corrupt_root<F: FnOnce(&mut [u8])>(f: F) -> (MemDevice, RefMap, u64) {
    let (mut dev, rm, root) = small_tree();
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(root, &mut buf).expect("read");
    f(&mut buf);
    dev.write_block(root, &buf).expect("write");
    (dev, rm, root)
}

/// Corrupt the root block and RE-CHECKSUM it, so the structural validators —
/// not the checksum — are what must refuse the node.
fn corrupt_root_resum<F: FnOnce(&mut [u8])>(f: F) -> (MemDevice, RefMap, u64) {
    let (mut dev, rm, root) = small_tree();
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(root, &mut buf).expect("read");
    f(&mut buf);
    resum(&mut buf);
    dev.write_block(root, &buf).expect("write");
    (dev, rm, root)
}

/// Recompute a node's FNV-1a checksum in place (the format's own rule: the
/// hash covers the whole block with the checksum field read as zeros).
fn resum(buf: &mut [u8]) {
    buf[24..32].copy_from_slice(&0u64.to_le_bytes());
    let sum = unafs::hash::hash_bytes(buf);
    buf[24..32].copy_from_slice(&sum.to_le_bytes());
}

#[test]
fn an_all_zero_block_is_not_a_node() {
    let (mut dev, mut rm, root) = corrupt_root(|b| b.fill(0));
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    assert!(matches!(
        Btree::open(root, LexCmp)
            .lookup(&mut store, b"k")
            .expect_err("zeros"),
        BtreeError::BadMagic(_)
    ));
}

#[test]
fn a_child_pointer_that_loops_is_refused_instead_of_spinning() {
    // Point the root's first child at the root itself.
    let (mut dev, mut rm, root) = corrupt_root_resum(|_| {});
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(root, &mut buf).expect("read");
    let node = Node::decode(root, &buf).expect("decode");
    assert_eq!(node.kind, unafs::btree::NodeKind::Internal);
    // slot 0's value offset/len
    let voff = u16::from_le_bytes([buf[68], buf[69]]) as usize;
    buf[voff..voff + 8].copy_from_slice(&root.to_le_bytes());
    resum(&mut buf);
    dev.write_block(root, &buf).expect("write");
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let err = Btree::open(root, LexCmp)
        .lookup(&mut store, &key_of(0))
        .expect_err("a self-referential child must be refused");
    assert!(matches!(err, BtreeError::Malformed(_, _)), "got {err:?}");
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

#[test]
fn oversized_keys_and_values_are_refused_cleanly() {
    let (mut dev, mut rm) = volume(128);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut t = Btree::create(&mut store, LexCmp).expect("create");
    let big_key = vec![b'k'; MAX_KEY_LEN + 1];
    let big_val = vec![b'v'; MAX_VALUE_LEN + 1];
    assert!(matches!(
        t.insert(&mut store, &big_key, b"v").expect_err("key"),
        BtreeError::KeyTooLarge(_)
    ));
    assert!(matches!(
        t.insert(&mut store, b"k", &big_val).expect_err("value"),
        BtreeError::ValueTooLarge(_)
    ));
    // The exact ceilings ARE accepted, and round-trip.
    let k = vec![b'k'; MAX_KEY_LEN];
    let v = vec![b'v'; MAX_VALUE_LEN];
    t.insert(&mut store, &k, &v).expect("ceiling insert");
    assert_eq!(t.lookup(&mut store, &k).expect("l"), Some(v));
    // Several of them: the node must split rather than overflow.
    for i in 0..40u64 {
        let mut k = vec![b'a'; MAX_KEY_LEN];
        k[0..8].copy_from_slice(&i.to_be_bytes());
        t.insert(&mut store, &k, &vec![b'z'; MAX_VALUE_LEN])
            .expect("max-size insert");
    }
    let st = t.stats(&mut store).expect("stats");
    assert_eq!(st.entries, 41);
    assert!(st.depth >= 2, "max-size entries must have split the root");
}

#[test]
fn a_full_volume_fails_the_insert_instead_of_corrupting() {
    // Ten blocks: the tree runs out of space almost immediately.
    let (mut dev, mut rm) = volume(10);
    let mut store = DeviceStore::new(&mut dev, &mut rm);
    let mut t = Btree::create(&mut store, LexCmp)
        .expect("create")
        .with_fanout_cap(4);
    let mut inserted = 0u64;
    let mut hit_full = false;
    for i in 0..500u64 {
        match t.insert(&mut store, &key_of(i), &val_of(i)) {
            Ok(_) => inserted += 1,
            Err(BtreeError::NoSpace) => {
                hit_full = true;
                break;
            }
            Err(e) => panic!("unexpected error {e:?}"),
        }
        // The refmap has no freeze here, so retired blocks stay parked and the
        // volume drains — which is exactly the pre-commit reality.
    }
    assert!(hit_full, "a 10 block volume must fill up");
    assert!(inserted > 0, "at least one insert should have succeeded");
    // Whatever landed before the wall is still a readable, ordered tree.
    let scan = t.range(&mut store, None, None, false).expect("scan");
    for w in scan.windows(2) {
        assert!(w[0].0 < w[1].0, "the surviving tree is out of order");
    }
}

#[test]
fn the_comparator_is_a_total_order_on_every_supplied_shape() {
    // U64Cmp on ill-sized keys must still be a strict total order, or the
    // ordering validator would reject trees built from them.
    let samples: Vec<Vec<u8>> = vec![
        vec![],
        vec![0],
        vec![1],
        vec![0, 0],
        1u64.to_le_bytes().to_vec(),
        2u64.to_le_bytes().to_vec(),
        vec![9; 9],
    ];
    for a in &samples {
        for b in &samples {
            let ab = U64Cmp.compare(a, b);
            let ba = U64Cmp.compare(b, a);
            assert_eq!(ab, ba.reverse(), "U64Cmp is not antisymmetric on {a:?}/{b:?}");
            assert_eq!(
                ab == std::cmp::Ordering::Equal,
                a == b,
                "U64Cmp equates distinct keys {a:?}/{b:?}"
            );
            let fab = AsciiFoldCmp.compare(a, b);
            assert_eq!(fab, AsciiFoldCmp.compare(b, a).reverse());
            assert_eq!(fab == std::cmp::Ordering::Equal, a == b);
        }
    }
}
