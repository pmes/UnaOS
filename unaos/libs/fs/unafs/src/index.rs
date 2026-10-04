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
//! F3/F4 (B302): the attribute catalog as two on-disk B+trees.
//!
//! On a v6 volume the catalog inode's data is a 40 B [`CatalogRecord`] naming
//! two [`Btree`](crate::btree::Btree) roots, both under [`LexCmp`] over
//! fixed-shape composite keys with an EMPTY value:
//!
//! * the **equality tree** — `key_hash ‖ val_hash ‖ inode_id` (24 B, all u64
//!   big-endian): `key == v` is one prefix range; "every inode carrying
//!   `key`" (`!=`, similarity) is the `key_hash` prefix range;
//! * the **ordered tree** — `key_hash ‖ tag ‖ ordered value bytes ‖ inode_id`
//!   for `Int` (tag 1, sign-flipped big-endian), `Float` (tag 2, total-order
//!   bits big-endian) and `String` (tag 3, bytes with NUL escaped `00 FF`,
//!   capped at [`STRING_KEY_CAP`], terminated `00 00` — or `00 01`, the SPILL
//!   MARKER, when the string was truncated). Lexicographic order of these keys
//!   is the value order, so `>`/`<`/`>=`/`<=`/ranges are range scans.
//!
//! Index scans produce a candidate SUPERSET (bounds are widened, truncated
//! strings tie); the query engine verifies every candidate against the
//! inode's real value, so the index can only ever cost reads, never answers.
//!
//! The tree nodes are allocated from the volume's `RefMap` through
//! [`DeviceStore`](crate::btree::DeviceStore) and written fresh (path-copy
//! CoW), so an index mutation joins the caller's transaction and is made
//! durable by the same single root-sector flip: root record → inode map →
//! catalog inode → catalog record → tree roots.

use crate::btree::{BtreeError, NodeStore};
use crate::catalog::hash_value;
use crate::hash::hash_bytes;
use crate::inode::AttributeValue;
use crate::storage::BlockDevice;
use alloc::vec::Vec;

/// Catalog-record magic: "UNAFSCX1".
pub const CATALOG_MAGIC: [u8; 8] = *b"UNAFSCX1";
/// The exact packed size of a [`CatalogRecord`].
pub const CATALOG_RECORD_SIZE: usize = 40;

/// Ordered-key type tags.
pub const TAG_INT: u8 = 1;
pub const TAG_FLOAT: u8 = 2;
pub const TAG_STRING: u8 = 3;

/// Bytes of a String value the ordered key carries before truncating and
/// stamping the spill marker. 96 B → worst case 8 + 1 + 2·96 + 2 + 8 = 211 B,
/// well under the tree's 384 B key ceiling.
pub const STRING_KEY_CAP: usize = 96;

/// The v6 catalog inode's data: the two tree roots, hand-packed
/// little-endian, FNV-checksummed.
///
/// ```text
///   off len  field
///     0   8  magic "UNAFSCX1"
///     8   8  eq_root   (block of the equality tree's root node)
///    16   8  ord_root  (block of the ordered tree's root node)
///    24   8  entries   (equality-tree entries = indexed (inode, key) pairs)
///    32   8  checksum  FNV-1a over bytes 0..32
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogRecord {
    pub eq_root: u64,
    pub ord_root: u64,
    pub entries: u64,
}

impl CatalogRecord {
    /// Pack the record.
    pub fn to_bytes(&self) -> [u8; CATALOG_RECORD_SIZE] {
        let mut b = [0u8; CATALOG_RECORD_SIZE];
        b[0..8].copy_from_slice(&CATALOG_MAGIC);
        b[8..16].copy_from_slice(&self.eq_root.to_le_bytes());
        b[16..24].copy_from_slice(&self.ord_root.to_le_bytes());
        b[24..32].copy_from_slice(&self.entries.to_le_bytes());
        let sum = hash_bytes(&b[0..32]);
        b[32..40].copy_from_slice(&sum.to_le_bytes());
        b
    }

    /// Parse and validate (magic, checksum, non-zero roots inside the volume).
    pub fn from_bytes(b: &[u8], block_count: u64) -> Option<CatalogRecord> {
        if b.len() < CATALOG_RECORD_SIZE || b[0..8] != CATALOG_MAGIC {
            return None;
        }
        let rd = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        if hash_bytes(&b[0..32]) != rd(32) {
            return None;
        }
        let rec = CatalogRecord {
            eq_root: rd(8),
            ord_root: rd(16),
            entries: rd(24),
        };
        let ok = |r: u64| r > crate::root::ROOT_BLOCK && r < block_count;
        if !ok(rec.eq_root) || !ok(rec.ord_root) {
            return None;
        }
        Some(rec)
    }
}

/// One indexed fact: inode `inode_id` carries attribute `key` = `value`.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexFact {
    pub key_hash: u64,
    pub value: AttributeValue,
    pub inode_id: u64,
}

impl IndexFact {
    pub fn new(key: &str, value: &AttributeValue, inode_id: u64) -> Self {
        Self {
            key_hash: hash_bytes(key.as_bytes()),
            value: value.clone(),
            inode_id,
        }
    }

    /// The equality-tree key.
    pub fn eq_key(&self) -> [u8; 24] {
        eq_key(self.key_hash, hash_value(&self.value), self.inode_id)
    }

    /// The ordered-tree key (`None` for Blob/Vector: equality-only types).
    pub fn ord_key(&self) -> Option<Vec<u8>> {
        let (tag, bytes) = ordered_bytes(&self.value)?;
        let mut k = Vec::with_capacity(8 + 1 + bytes.len() + 8);
        k.extend_from_slice(&self.key_hash.to_be_bytes());
        k.push(tag);
        k.extend_from_slice(&bytes);
        k.extend_from_slice(&self.inode_id.to_be_bytes());
        Some(k)
    }
}

/// `key_hash ‖ val_hash ‖ inode_id`, big-endian.
pub fn eq_key(key_hash: u64, val_hash: u64, inode_id: u64) -> [u8; 24] {
    let mut k = [0u8; 24];
    k[0..8].copy_from_slice(&key_hash.to_be_bytes());
    k[8..16].copy_from_slice(&val_hash.to_be_bytes());
    k[16..24].copy_from_slice(&inode_id.to_be_bytes());
    k
}

/// The inode id every index key ends with.
pub fn inode_of_key(k: &[u8]) -> Option<u64> {
    if k.len() < 16 {
        return None;
    }
    Some(u64::from_be_bytes(k[k.len() - 8..].try_into().ok()?))
}

/// `[lo, hi)` covering every equality key with this `key_hash` (and, when
/// given, this `val_hash`).
pub fn eq_range(key_hash: u64, val_hash: Option<u64>) -> (Vec<u8>, Vec<u8>) {
    let mut lo = Vec::with_capacity(25);
    lo.extend_from_slice(&key_hash.to_be_bytes());
    if let Some(v) = val_hash {
        lo.extend_from_slice(&v.to_be_bytes());
    }
    let mut hi = lo.clone();
    // Every key under the prefix is the prefix + bytes; prefix + 0xFF×17
    // is strictly above all of them (they are 24 B long).
    hi.extend_from_slice(&[0xFF; 17]);
    (lo, hi)
}

/// Int → 8 bytes whose unsigned order is the signed order.
pub fn int_ord(i: i64) -> [u8; 8] {
    ((i as u64) ^ (1u64 << 63)).to_be_bytes()
}

/// Float → u64 whose unsigned order is the IEEE total order (−NaN < −∞ < … <
/// −0 < +0 < … < +∞ < +NaN).
pub fn float_ord(f: f64) -> u64 {
    let b = f.to_bits();
    if b >> 63 == 1 { !b } else { b | (1u64 << 63) }
}

/// String → escaped bytes (NUL → `00 FF`), capped at [`STRING_KEY_CAP`] source
/// bytes, then `00 00` (complete) or `00 01` (truncated: the spill marker).
pub fn string_ord(s: &[u8]) -> Vec<u8> {
    let (body, truncated) = if s.len() > STRING_KEY_CAP {
        (&s[..STRING_KEY_CAP], true)
    } else {
        (s, false)
    };
    let mut out = string_ord_open(body);
    out.push(0x00);
    out.push(if truncated { 0x01 } else { 0x00 });
    out
}

/// The escaped body of [`string_ord`] with no terminator (a range LOWER
/// bound: it sorts at or below every encoding with this body as prefix).
pub fn string_ord_open(body: &[u8]) -> Vec<u8> {
    let body = &body[..core::cmp::min(body.len(), STRING_KEY_CAP)];
    let mut out = Vec::with_capacity(body.len() + 2);
    for &c in body {
        out.push(c);
        if c == 0 {
            out.push(0xFF);
        }
    }
    out
}

/// The ordered encoding of a value, or `None` for equality-only types.
pub fn ordered_bytes(value: &AttributeValue) -> Option<(u8, Vec<u8>)> {
    match value {
        AttributeValue::Int(i) => Some((TAG_INT, int_ord(*i).to_vec())),
        AttributeValue::Float(f) => Some((TAG_FLOAT, float_ord(*f).to_be_bytes().to_vec())),
        AttributeValue::String(s) => Some((TAG_STRING, string_ord(s.as_bytes()))),
        AttributeValue::Blob(_) | AttributeValue::Vector(_) => None,
    }
}

/// One ordered-tree scan: `[lo, hi)` in key space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrdScan {
    pub lo: Vec<u8>,
    pub hi: Vec<u8>,
}

fn scan(key_hash: u64, tag: u8, lo: Option<Vec<u8>>, hi: Option<Vec<u8>>) -> OrdScan {
    let mut l = Vec::with_capacity(32);
    l.extend_from_slice(&key_hash.to_be_bytes());
    l.push(tag);
    let mut h = l.clone();
    match lo {
        Some(b) => l.extend_from_slice(&b),
        None => {}
    }
    match hi {
        // Inclusive of every key whose value bytes are ≤ `b` as a prefix
        // (any id suffix, any terminator): `b ‖ FF×9` is above them all.
        Some(b) => {
            h.extend_from_slice(&b);
            h.extend_from_slice(&[0xFF; 9]);
        }
        None => {
            // Open top: the next tag.
            h.pop();
            h.push(tag.wrapping_add(1));
        }
    }
    OrdScan { lo: l, hi: h }
}

/// The ordered-tree scans whose union is a SUPERSET of the inodes whose
/// `key` value `v` satisfies `lo ≤ v ≤ hi` (each bound optional and treated
/// as inclusive — strictness is the verifier's job). Numeric bounds scan BOTH
/// the Int and the Float tag (ordering compares Int and Float numerically);
/// String bounds scan the String tag. Mixed or unordered bound types (Blob,
/// Vector, String-vs-number, a NaN bound) can match nothing: no scans.
pub fn ordered_scans(
    key_hash: u64,
    lo: Option<&AttributeValue>,
    hi: Option<&AttributeValue>,
) -> Vec<OrdScan> {
    use AttributeValue as V;
    let numeric = |v: Option<&V>| matches!(v, None | Some(V::Int(_)) | Some(V::Float(_)));
    let stringy = |v: Option<&V>| matches!(v, None | Some(V::String(_)));
    if lo.is_none() && hi.is_none() {
        return Vec::new();
    }
    if numeric(lo) && numeric(hi) {
        if matches!(lo, Some(V::Float(f)) if f.is_nan()) || matches!(hi, Some(V::Float(f)) if f.is_nan()) {
            return Vec::new();
        }
        // Int tag: integer bounds widened outward (floor / ceil, saturating).
        let int_lo = lo.map(|v| match v {
            V::Int(i) => *i,
            V::Float(f) => f64_to_i64_sat(libm::floor(*f)),
            _ => i64::MIN,
        });
        let int_hi = hi.map(|v| match v {
            V::Int(i) => *i,
            V::Float(f) => f64_to_i64_sat(libm::ceil(*f)),
            _ => i64::MAX,
        });
        // Float tag: bounds widened one step outward in total-order space so
        // an Int bound that is not exactly representable still covers.
        let f_lo = lo.map(|v| match v {
            V::Int(i) => float_ord(*i as f64).saturating_sub(1),
            V::Float(f) => float_ord(*f),
            _ => 0,
        });
        let f_hi = hi.map(|v| match v {
            V::Int(i) => float_ord(*i as f64).saturating_add(1),
            V::Float(f) => float_ord(*f),
            _ => u64::MAX,
        });
        let mut out = Vec::new();
        if int_lo.zip(int_hi).is_none_or(|(a, b)| a <= b) {
            out.push(scan(
                key_hash,
                TAG_INT,
                int_lo.map(|i| int_ord(i).to_vec()),
                int_hi.map(|i| int_ord(i).to_vec()),
            ));
        }
        if f_lo.zip(f_hi).is_none_or(|(a, b)| a <= b) {
            out.push(scan(
                key_hash,
                TAG_FLOAT,
                f_lo.map(|b| b.to_be_bytes().to_vec()),
                f_hi.map(|b| b.to_be_bytes().to_vec()),
            ));
        }
        return out;
    }
    if stringy(lo) && stringy(hi) {
        let s = |v: Option<&V>| match v {
            Some(V::String(s)) => Some(s.as_bytes().to_vec()),
            _ => None,
        };
        let (l, h) = (s(lo), s(hi));
        // Upper bound: the open body plus `00 02` sits above `00 00` (exact)
        // and `00 01` (spill) yet below `00 FF` (an escaped NUL — a longer
        // string that differs inside the cap).
        let hi_key = h.map(|b| {
            let mut k = string_ord_open(&b);
            k.push(0x00);
            k.push(0x02);
            k
        });
        return alloc::vec![scan(key_hash, TAG_STRING, l.map(|b| string_ord_open(&b)), hi_key)];
    }
    Vec::new()
}

fn f64_to_i64_sat(f: f64) -> i64 {
    if f <= i64::MIN as f64 {
        i64::MIN
    } else if f >= i64::MAX as f64 {
        i64::MAX
    } else {
        f as i64
    }
}

/// A read-only [`NodeStore`] over a device: the query and reachability walks
/// read trees through it (a live mount and a snapshot alike) and can never
/// allocate or write.
pub struct ReadStore<'a, D: BlockDevice> {
    pub device: &'a mut D,
}

impl<D: BlockDevice> NodeStore for ReadStore<'_, D> {
    fn alloc(&mut self) -> Result<u64, BtreeError> {
        Err(BtreeError::Corrupt("read-only index store"))
    }
    fn release(&mut self, _block: u64) {}
    fn read(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BtreeError> {
        self.device.read_block(block, buf)?;
        Ok(())
    }
    fn write(&mut self, _block: u64, _buf: &[u8]) -> Result<(), BtreeError> {
        Err(BtreeError::Corrupt("read-only index store"))
    }
}
