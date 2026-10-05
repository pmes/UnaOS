//! Merkle tree proofs (RFC 9162 §2.1; the same hashing as RFC 6962 §2.1): a caller that fetched an inclusion
//! proof from a log (`get-proof-by-hash`, or a tile read) checks it here against a signed tree head's root.
//!
//! leaf hash = SHA-256(0x00 ‖ leaf), node hash = SHA-256(0x01 ‖ left ‖ right).

use alloc::vec::Vec;

use super::{Entry, Sct};
use crate::crypto::{CryptoProvider, HashAlg};

pub type Hash = [u8; 32];

fn h(p: &dyn CryptoProvider, parts: &[&[u8]]) -> Hash {
    let mut o = [0u8; 32];
    o.copy_from_slice(p.hash(HashAlg::Sha256, parts).as_bytes());
    o
}

pub fn leaf_hash(p: &dyn CryptoProvider, leaf: &[u8]) -> Hash {
    h(p, &[&[0x00], leaf])
}

pub fn node_hash(p: &dyn CryptoProvider, left: &Hash, right: &Hash) -> Hash {
    h(p, &[&[0x01], left, right])
}

/// RFC 9162 §2.1.3.2: does `proof` prove `leaf` (a leaf HASH) at `index` in the tree of `size` with `root`?
pub fn verify_inclusion(p: &dyn CryptoProvider, leaf: &Hash, index: u64, size: u64, proof: &[Hash], root: &Hash) -> bool {
    if index >= size {
        return false;
    }
    let (mut fnn, mut sn) = (index, size - 1);
    let mut r = *leaf;
    for pe in proof {
        if sn == 0 {
            return false;
        }
        if fnn & 1 == 1 || fnn == sn {
            r = node_hash(p, pe, &r);
            if fnn & 1 == 0 {
                while fnn & 1 == 0 && fnn != 0 {
                    fnn >>= 1;
                    sn >>= 1;
                }
            }
        } else {
            r = node_hash(p, &r, pe);
        }
        fnn >>= 1;
        sn >>= 1;
    }
    sn == 0 && crate::codec::ct_eq(&r, root)
}

/// RFC 6962 §3.4 MerkleTreeLeaf for an SCT's entry: `version v1(0) ‖ leaf_type timestamped_entry(0) ‖
/// TimestampedEntry { timestamp, entry_type, signed_entry, extensions }` — what a log hashes for the leaf an
/// SCT promises to include.
pub fn sct_leaf(sct: &Sct, entry: &Entry<'_>) -> Vec<u8> {
    let mut v = Vec::with_capacity(64);
    v.push(0);
    v.push(0);
    v.extend_from_slice(&sct.timestamp.to_be_bytes());
    entry.encode(&mut v);
    crate::codec::put_vec16(&mut v, &sct.extensions);
    v
}

/// The root of the tree over `leaves` (RFC 9162 §2.1.1 MTH) — for tests and for small caller-held trees.
pub fn tree_root(p: &dyn CryptoProvider, leaves: &[Hash]) -> Hash {
    match leaves.len() {
        0 => h(p, &[]),
        1 => leaves[0],
        n => {
            let k = 1usize << (usize::BITS - 1 - (n - 1).leading_zeros()); // largest power of two < n
            node_hash(p, &tree_root(p, &leaves[..k]), &tree_root(p, &leaves[k..]))
        }
    }
}
