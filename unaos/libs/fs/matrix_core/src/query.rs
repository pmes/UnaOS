// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The query model — find a name in the genesis tree.
//!
//! The host handler has no ranking of its own beyond the Finder's ORDER (directories first, then files, each
//! by name in byte order — the genesis tree's pre-order). The query keeps that order as the tie-break and adds
//! one ranking above it, by how the query meets the entry's name (ASCII case folded):
//!
//! 1. [`Rank::Exact`] — the whole name;
//! 2. [`Rank::Prefix`] — the start of the name;
//! 3. [`Rank::Substring`] — anywhere in the name;
//! 4. [`Rank::Subsequence`] — the query's characters in order, gaps allowed, within the one name (`mcr` finds
//!    `matrix_core.rs`).
//!
//! A query containing `/` is matched against the node's whole path (`id`) instead of its name, with the same
//! four ranks. An empty query finds nothing. The sort is stable, so within a rank hits keep the tree order.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Exact,
    Prefix,
    Substring,
    Subsequence,
}

/// One query result: the node's id and label, whether it is a directory, and how it matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub label: String,
    pub dir: bool,
    pub rank: Rank,
}

fn fold(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

/// How `q` meets `hay` (both compared ASCII case folded), or `None`.
pub fn rank(q: &str, hay: &str) -> Option<Rank> {
    let q = q.as_bytes();
    let h = hay.as_bytes();
    if q.is_empty() || q.len() > h.len() {
        return None;
    }
    let eq = |a: &[u8], b: &[u8]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| fold(*x) == fold(*y));
    if eq(q, h) {
        return Some(Rank::Exact);
    }
    if eq(q, &h[..q.len()]) {
        return Some(Rank::Prefix);
    }
    if (1..=h.len() - q.len()).any(|i| eq(q, &h[i..i + q.len()])) {
        return Some(Rank::Substring);
    }
    let mut qi = 0;
    for &c in h {
        if qi < q.len() && fold(c) == fold(q[qi]) {
            qi += 1;
        }
    }
    if qi == q.len() { Some(Rank::Subsequence) } else { None }
}

/// Every node of `tree` the query `q` meets, best rank first, the tree's pre-order within a rank.
pub fn query(tree: &[Node], q: &str) -> Vec<Hit> {
    let q = q.trim();
    let by_path = q.contains('/');
    let mut hits = Vec::new();
    walk(tree, q, by_path, &mut hits);
    hits.sort_by_key(|h: &Hit| h.rank);
    hits
}

fn walk(nodes: &[Node], q: &str, by_path: bool, out: &mut Vec<Hit>) {
    for n in nodes {
        let hay = if by_path { n.id.as_str() } else { n.label.as_str() };
        if let Some(r) = rank(q, hay) {
            out.push(Hit { id: n.id.clone(), label: n.label.clone(), dir: n.dir, rank: r });
        }
        walk(&n.children, q, by_path, out);
    }
}
