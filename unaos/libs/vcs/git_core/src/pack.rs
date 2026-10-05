// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Packfiles (`gitformat-pack(5)`): the v2/v3 pack stream (`PACK`, version, count, entries, trailing
//! digest), entry headers (type + varint size), OFS_DELTA (negative offset varint) and REF_DELTA
//! (base id) entries, pack index v1 and v2 (fan-out, sorted ids, CRC-32s, 31-bit offsets with the
//! 64-bit extension table, pack and index digests), `index-pack` (every entry walked, every delta
//! chain resolved breadth-from-the-base, ids computed, thin-pack bases supplied by the caller),
//! the multi-pack-index (`MIDX`: PNAM/OIDF/OIDL/OOFF/LOFF chunks), and a pack WRITER with git's
//! delta-window search (objects ordered by kind, path hash and size; each tries the previous
//! `window` same-kind objects as a base, depth-limited, written as OFS_DELTA).
//!
//! Oracle (tests/pack.rs): `git verify-pack -v` on every pack written here; `git index-pack` on
//! the same pack yields an idx BYTE-IDENTICAL to ours; `git multi-pack-index write` read back; this
//! repository's own shallow history indexed, every object round-tripped, re-packed and fsck'd.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use pixel_core::crc::Crc32;

use crate::deflate;
use crate::delta;
use crate::hash::{HashKind, ObjectId};
use crate::object::{self, Kind};
use crate::zlib;
use crate::{Error, Result};

/// Pack entry type numbers.
pub mod types {
    /// Offset delta.
    pub const OFS_DELTA: u8 = 6;
    /// Reference delta.
    pub const REF_DELTA: u8 = 7;
}

/// What an entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A whole object.
    Base(Kind),
    /// A delta against the entry at this absolute pack offset.
    OfsDelta(u64),
    /// A delta against the object with this id.
    RefDelta(ObjectId),
}

/// A parsed entry header.
#[derive(Debug, Clone, Copy)]
pub struct EntryHeader {
    /// Kind / base.
    pub kind: EntryKind,
    /// Inflated size (of the object, or of the delta instructions).
    pub size: u64,
    /// Offset of the zlib stream.
    pub data_offset: usize,
}

/// Parse the entry header at `offset`.
pub fn parse_entry_header(pack: &[u8], offset: usize, hk: HashKind) -> Result<EntryHeader> {
    let mut i = offset;
    let b = *pack.get(i).ok_or(Error::Corrupt("pack: entry beyond end"))?;
    i += 1;
    let t = (b >> 4) & 7;
    let mut size = (b & 15) as u64;
    let mut shift = 4;
    let mut c = b;
    while c & 0x80 != 0 {
        c = *pack.get(i).ok_or(Error::Corrupt("pack: truncated size"))?;
        i += 1;
        if shift > 60 {
            return Err(Error::Corrupt("pack: size overflow"));
        }
        size |= ((c & 0x7f) as u64) << shift;
        shift += 7;
    }
    let kind = match t {
        1..=4 => EntryKind::Base(Kind::from_pack(t).unwrap()),
        types::OFS_DELTA => {
            let mut c = *pack.get(i).ok_or(Error::Corrupt("pack: truncated offset"))?;
            i += 1;
            let mut ofs = (c & 0x7f) as u64;
            while c & 0x80 != 0 {
                c = *pack.get(i).ok_or(Error::Corrupt("pack: truncated offset"))?;
                i += 1;
                ofs = ofs.checked_add(1).and_then(|o| o.checked_mul(128)).ok_or(Error::Corrupt("pack: offset overflow"))? | (c & 0x7f) as u64;
            }
            if ofs == 0 || ofs > offset as u64 {
                return Err(Error::Corrupt("pack: delta base offset out of range"));
            }
            EntryKind::OfsDelta(offset as u64 - ofs)
        }
        types::REF_DELTA => {
            let n = hk.len();
            let id = pack.get(i..i + n).ok_or(Error::Corrupt("pack: truncated base id"))?;
            i += n;
            EntryKind::RefDelta(ObjectId::from_bytes(hk, id))
        }
        _ => return Err(Error::Corrupt("pack: invalid entry type")),
    };
    Ok(EntryHeader { kind, size, data_offset: i })
}

/// Append an entry header (type 1..=7, inflated size).
pub fn write_entry_header(out: &mut Vec<u8>, t: u8, mut size: u64) {
    let mut b = (t << 4) | (size & 15) as u8;
    size >>= 4;
    while size != 0 {
        out.push(b | 0x80);
        b = (size & 0x7f) as u8;
        size >>= 7;
    }
    out.push(b);
}

/// Append an OFS_DELTA negative offset.
pub fn write_ofs(out: &mut Vec<u8>, mut ofs: u64) {
    let mut buf = [0u8; 10];
    let mut pos = buf.len() - 1;
    buf[pos] = (ofs & 0x7f) as u8;
    ofs >>= 7;
    while ofs != 0 {
        ofs -= 1;
        pos -= 1;
        buf[pos] = 0x80 | (ofs & 0x7f) as u8;
        ofs >>= 7;
    }
    out.extend_from_slice(&buf[pos..]);
}

/// A pack in memory.
#[derive(Clone, Copy)]
pub struct Pack<'a> {
    /// The bytes.
    pub data: &'a [u8],
    /// Object format.
    pub hash: HashKind,
    /// Number of entries.
    pub count: u32,
    /// Version (2 or 3).
    pub version: u32,
}

impl<'a> Pack<'a> {
    /// Check the header (and, when `verify`, the trailing digest).
    pub fn parse(data: &'a [u8], hk: HashKind, verify: bool) -> Result<Self> {
        if data.len() < 12 + hk.len() || &data[..4] != b"PACK" {
            return Err(Error::Corrupt("pack: bad signature"));
        }
        let version = u32::from_be_bytes(data[4..8].try_into().unwrap());
        if version != 2 && version != 3 {
            return Err(Error::Unsupported("pack version"));
        }
        let count = u32::from_be_bytes(data[8..12].try_into().unwrap());
        if verify {
            let end = data.len() - hk.len();
            if hk.digest(&data[..end]).as_bytes() != &data[end..] {
                return Err(Error::Corrupt("pack: trailing checksum mismatch"));
            }
        }
        Ok(Pack { data, hash: hk, count, version })
    }

    /// The trailing digest.
    pub fn checksum(&self) -> ObjectId {
        ObjectId::from_bytes(self.hash, &self.data[self.data.len() - self.hash.len()..])
    }

    /// Inflate an entry's payload: (bytes, compressed length).
    pub fn inflate_entry(&self, h: &EntryHeader) -> Result<(Vec<u8>, usize)> {
        let limit = self.data.len() - self.hash.len();
        if h.data_offset > limit {
            return Err(Error::Corrupt("pack: entry data beyond end"));
        }
        zlib::inflate_exact(&self.data[h.data_offset..limit], h.size as usize)
    }

    /// The object at `offset`, resolving delta chains; REF_DELTA bases outside the pack come from
    /// `lookup` (an id → offset map for this pack is consulted first through `find`).
    pub fn object_at(
        &self,
        offset: u64,
        find: &dyn Fn(&ObjectId) -> Option<u64>,
        external: &mut dyn FnMut(&ObjectId) -> Option<(Kind, Vec<u8>)>,
    ) -> Result<(Kind, Vec<u8>)> {
        let mut chain: Vec<Vec<u8>> = Vec::new();
        let mut at = offset;
        let (kind, mut data) = loop {
            if chain.len() > 10_000 {
                return Err(Error::Corrupt("pack: delta chain too deep"));
            }
            let h = parse_entry_header(self.data, at as usize, self.hash)?;
            match h.kind {
                EntryKind::Base(k) => break (k, self.inflate_entry(&h)?.0),
                EntryKind::OfsDelta(b) => {
                    chain.push(self.inflate_entry(&h)?.0);
                    at = b;
                }
                EntryKind::RefDelta(id) => {
                    chain.push(self.inflate_entry(&h)?.0);
                    match find(&id) {
                        Some(o) => at = o,
                        None => break external(&id).ok_or(Error::Missing(id))?,
                    }
                }
            }
        };
        while let Some(d) = chain.pop() {
            data = delta::apply(&data, &d)?;
        }
        Ok((kind, data))
    }
}

// ---------------------------------------------------------------------------------------------
// Pack index
// ---------------------------------------------------------------------------------------------

/// A pack index (v1 or v2).
#[derive(Clone, Copy)]
pub struct Idx<'a> {
    data: &'a [u8],
    /// Object format.
    pub hash: HashKind,
    /// 1 or 2.
    pub version: u32,
    /// Number of objects.
    pub count: u32,
}

impl<'a> Idx<'a> {
    /// Parse (checks sizes and the fan-out's monotonicity; `verify` also checks the idx digest).
    pub fn parse(data: &'a [u8], hk: HashKind, verify: bool) -> Result<Self> {
        let n = hk.len();
        let (version, fan) = if data.len() >= 8 && data[..4] == [0xff, b't', b'O', b'c'] {
            let v = u32::from_be_bytes(data[4..8].try_into().unwrap());
            if v != 2 {
                return Err(Error::Unsupported("idx version"));
            }
            (2, 8)
        } else {
            (1, 0)
        };
        if data.len() < fan + 1024 + 2 * n {
            return Err(Error::Corrupt("idx: too short"));
        }
        let mut prev = 0;
        for k in 0..256 {
            let v = u32::from_be_bytes(data[fan + 4 * k..fan + 4 * k + 4].try_into().unwrap());
            if v < prev {
                return Err(Error::Corrupt("idx: fan-out not monotonic"));
            }
            prev = v;
        }
        let count = prev;
        let c = count as usize;
        let min = if version == 2 { 8 + 1024 + c * (n + 8) + 2 * n } else { 1024 + c * (n + 4) + 2 * n };
        if data.len() < min {
            return Err(Error::Corrupt("idx: truncated tables"));
        }
        if version == 2 && verify {
            let large = data[8 + 1024 + c * (n + 4)..8 + 1024 + c * (n + 8)]
                .chunks(4)
                .filter(|w| w[0] & 0x80 != 0)
                .count();
            if data.len() != min + large * 8 {
                return Err(Error::Corrupt("idx: size disagrees with large-offset count"));
            }
        } else if version == 1 && data.len() != min {
            return Err(Error::Corrupt("idx: v1 size mismatch"));
        }
        if verify {
            let end = data.len() - n;
            if hk.digest(&data[..end]).as_bytes() != &data[end..] {
                return Err(Error::Corrupt("idx: checksum mismatch"));
            }
        }
        Ok(Idx { data, hash: hk, version, count })
    }

    fn fan(&self, k: usize) -> u32 {
        let base = if self.version == 2 { 8 } else { 0 };
        u32::from_be_bytes(self.data[base + 4 * k..base + 4 * k + 4].try_into().unwrap())
    }

    /// The i-th id in sorted order.
    pub fn oid(&self, i: usize) -> ObjectId {
        let n = self.hash.len();
        let at = if self.version == 2 { 8 + 1024 + i * n } else { 1024 + i * (n + 4) + 4 };
        ObjectId::from_bytes(self.hash, &self.data[at..at + n])
    }

    /// The i-th object's pack offset.
    pub fn offset(&self, i: usize) -> u64 {
        let n = self.hash.len();
        let c = self.count as usize;
        if self.version == 1 {
            let at = 1024 + i * (n + 4);
            return u32::from_be_bytes(self.data[at..at + 4].try_into().unwrap()) as u64;
        }
        let at = 8 + 1024 + c * (n + 4) + 4 * i;
        let v = u32::from_be_bytes(self.data[at..at + 4].try_into().unwrap());
        if v & 0x8000_0000 == 0 {
            return v as u64;
        }
        let k = (v & 0x7fff_ffff) as usize;
        let at = 8 + 1024 + c * (n + 8) + 8 * k;
        match self.data.get(at..at + 8) {
            Some(b) => u64::from_be_bytes(b.try_into().unwrap()),
            None => u64::MAX,
        }
    }

    /// The i-th object's CRC-32 (v2 only).
    pub fn crc(&self, i: usize) -> Option<u32> {
        if self.version != 2 {
            return None;
        }
        let at = 8 + 1024 + self.count as usize * self.hash.len() + 4 * i;
        Some(u32::from_be_bytes(self.data[at..at + 4].try_into().unwrap()))
    }

    /// The pack digest recorded in the index.
    pub fn pack_checksum(&self) -> ObjectId {
        let n = self.hash.len();
        ObjectId::from_bytes(self.hash, &self.data[self.data.len() - 2 * n..self.data.len() - n])
    }

    /// Position of `id`.
    pub fn find(&self, id: &ObjectId) -> Option<usize> {
        let k = id.first_byte() as usize;
        let mut lo = if k == 0 { 0 } else { self.fan(k - 1) as usize };
        let mut hi = self.fan(k) as usize;
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.oid(mid).as_bytes().cmp(id.as_bytes()) {
                core::cmp::Ordering::Equal => return Some(mid),
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// Offset of `id`.
    pub fn lookup(&self, id: &ObjectId) -> Option<u64> {
        self.find(id).map(|i| self.offset(i))
    }

    /// Every id with the hex prefix `p` (for short-id resolution).
    pub fn prefix_matches(&self, p: &[u8], out: &mut Vec<ObjectId>) {
        if p.len() < 2 {
            for i in 0..self.count as usize {
                let o = self.oid(i);
                if o.starts_with_hex(p) {
                    out.push(o);
                }
            }
            return;
        }
        let hexv = |c: u8| (c as char).to_digit(16).map(|d| d as usize);
        let (Some(a), Some(b)) = (hexv(p[0]), hexv(p[1])) else { return };
        let k = a * 16 + b;
        let lo = if k == 0 { 0 } else { self.fan(k - 1) as usize };
        for i in lo..self.fan(k) as usize {
            let o = self.oid(i);
            if o.starts_with_hex(p) {
                out.push(o);
            }
        }
    }
}

/// Write an idx v2 for `entries` (id, offset, crc) — sorted here — and the pack digest.
pub fn write_idx(hk: HashKind, entries: &mut [(ObjectId, u64, u32)], pack_checksum: &ObjectId) -> Vec<u8> {
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut o = Vec::with_capacity(8 + 1024 + entries.len() * (hk.len() + 8) + 2 * hk.len());
    o.extend_from_slice(&[0xff, b't', b'O', b'c', 0, 0, 0, 2]);
    let mut counts = [0u32; 256];
    for e in entries.iter() {
        counts[e.0.first_byte() as usize] += 1;
    }
    let mut acc = 0u32;
    for c in counts {
        acc += c;
        o.extend_from_slice(&acc.to_be_bytes());
    }
    for e in entries.iter() {
        o.extend_from_slice(e.0.as_bytes());
    }
    for e in entries.iter() {
        o.extend_from_slice(&e.2.to_be_bytes());
    }
    let mut large = Vec::new();
    for e in entries.iter() {
        if e.1 < 0x8000_0000 {
            o.extend_from_slice(&(e.1 as u32).to_be_bytes());
        } else {
            o.extend_from_slice(&(0x8000_0000 | large.len() as u32).to_be_bytes());
            large.push(e.1);
        }
    }
    for l in large {
        o.extend_from_slice(&l.to_be_bytes());
    }
    o.extend_from_slice(pack_checksum.as_bytes());
    let d = hk.digest(&o);
    o.extend_from_slice(d.as_bytes());
    o
}

// ---------------------------------------------------------------------------------------------
// index-pack
// ---------------------------------------------------------------------------------------------

/// One object found by [`index_pack`].
#[derive(Debug, Clone)]
pub struct Indexed {
    /// Its id.
    pub id: ObjectId,
    /// Entry offset in the pack.
    pub offset: u64,
    /// CRC-32 of the entry's bytes.
    pub crc: u32,
    /// The resolved object kind.
    pub kind: Kind,
    /// The object's size.
    pub size: u64,
    /// Delta chain depth (0 for a whole object).
    pub depth: u32,
}

/// The result of indexing a pack.
#[derive(Debug, Clone)]
pub struct IndexResult {
    /// Every object, in pack order.
    pub objects: Vec<Indexed>,
    /// The pack's digest.
    pub checksum: ObjectId,
    /// External bases a thin pack needed (ids), in the order they were supplied.
    pub external_bases: Vec<ObjectId>,
}

impl IndexResult {
    /// An idx v2 for this pack.
    pub fn idx(&self, hk: HashKind) -> Vec<u8> {
        let mut e: Vec<(ObjectId, u64, u32)> = self.objects.iter().map(|o| (o.id, o.offset, o.crc)).collect();
        write_idx(hk, &mut e, &self.checksum)
    }
}

struct Raw {
    offset: u64,
    end: u64,
    kind: EntryKind,
    delta: Option<Vec<u8>>,
    resolved: Option<(ObjectId, Kind, u64, u32)>,
}

/// Walk and resolve every entry of a pack (what `git index-pack` does). `visit` sees every
/// resolved object (id, kind, bytes) exactly once — the round-trip and fsck hooks ride on it.
pub fn index_pack(
    data: &[u8],
    hk: HashKind,
    external: &mut dyn FnMut(&ObjectId) -> Option<(Kind, Vec<u8>)>,
    visit: &mut dyn FnMut(&ObjectId, Kind, &[u8]) -> Result<()>,
) -> Result<IndexResult> {
    let pack = Pack::parse(data, hk, true)?;
    let end_of_entries = data.len() - hk.len();
    let mut raws: Vec<Raw> = Vec::with_capacity(pack.count as usize);
    let mut at = 12usize;
    for _ in 0..pack.count {
        let h = parse_entry_header(data, at, hk)?;
        let (bytes, used) = pack.inflate_entry(&h)?;
        let end = h.data_offset + used;
        if end > end_of_entries {
            return Err(Error::Corrupt("pack: entry runs into the trailer"));
        }
        let mut r = Raw { offset: at as u64, end: end as u64, kind: h.kind, delta: None, resolved: None };
        match h.kind {
            EntryKind::Base(k) => {
                let id = object::hash_object(hk, k, &bytes);
                visit(&id, k, &bytes)?;
                r.resolved = Some((id, k, bytes.len() as u64, 0));
            }
            _ => r.delta = Some(bytes),
        }
        raws.push(r);
        at = end;
    }
    if at != end_of_entries {
        return Err(Error::Corrupt("pack: garbage between the last entry and the trailer"));
    }
    // Children maps.
    let pos_of = |raws: &[Raw], off: u64| raws.binary_search_by(|r| r.offset.cmp(&off)).ok();
    let mut ofs_children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut ref_children: BTreeMap<ObjectId, Vec<usize>> = BTreeMap::new();
    for (i, r) in raws.iter().enumerate() {
        match r.kind {
            EntryKind::OfsDelta(b) => {
                let p = pos_of(&raws, b).ok_or(Error::Corrupt("pack: OFS_DELTA base is not an entry start"))?;
                ofs_children.entry(p).or_default().push(i);
            }
            EntryKind::RefDelta(id) => ref_children.entry(id).or_default().push(i),
            _ => {}
        }
    }
    let mut external_bases = Vec::new();
    // Resolve from every whole object that has children.
    let roots: Vec<usize> = (0..raws.len()).filter(|&i| raws[i].resolved.is_some()).collect();
    let resolve_from = |raws: &mut Vec<Raw>,
                            root_data: Rc<Vec<u8>>,
                            root_idx: Option<usize>,
                            root_id: ObjectId,
                            root_kind: Kind,
                            visit: &mut dyn FnMut(&ObjectId, Kind, &[u8]) -> Result<()>,
                            ofs_children: &BTreeMap<usize, Vec<usize>>,
                            ref_children: &mut BTreeMap<ObjectId, Vec<usize>>|
     -> Result<()> {
        let mut stack: Vec<(Option<usize>, ObjectId, Rc<Vec<u8>>, u32)> = vec![(root_idx, root_id, root_data, 0)];
        while let Some((idx, id, base, depth)) = stack.pop() {
            let mut kids: Vec<usize> = Vec::new();
            if let Some(i) = idx {
                if let Some(v) = ofs_children.get(&i) {
                    kids.extend_from_slice(v);
                }
            }
            if let Some(v) = ref_children.remove(&id) {
                kids.extend(v);
            }
            for c in kids {
                if raws[c].resolved.is_some() {
                    continue;
                }
                let d = raws[c].delta.take().ok_or(Error::Corrupt("pack: delta resolved twice"))?;
                let out = delta::apply(&base, &d)?;
                let cid = object::hash_object(hk, root_kind, &out);
                visit(&cid, root_kind, &out)?;
                raws[c].resolved = Some((cid, root_kind, out.len() as u64, depth + 1));
                stack.push((Some(c), cid, Rc::new(out), depth + 1));
            }
        }
        Ok(())
    };
    for i in roots {
        let (id, kind, _, _) = raws[i].resolved.unwrap();
        if !ofs_children.contains_key(&i) && !ref_children.contains_key(&id) {
            continue;
        }
        let h = parse_entry_header(data, raws[i].offset as usize, hk)?;
        let (bytes, _) = pack.inflate_entry(&h)?;
        resolve_from(&mut raws, Rc::new(bytes), Some(i), id, kind, visit, &ofs_children, &mut ref_children)?;
    }
    // Thin pack: bases from outside.
    while let Some((&base_id, _)) = ref_children.iter().next() {
        let (kind, bytes) = external(&base_id).ok_or(Error::Missing(base_id))?;
        if object::hash_object(hk, kind, &bytes) != base_id {
            return Err(Error::HashMismatch);
        }
        external_bases.push(base_id);
        resolve_from(&mut raws, Rc::new(bytes), None, base_id, kind, visit, &ofs_children, &mut ref_children)?;
    }
    let mut objects = Vec::with_capacity(raws.len());
    for r in &raws {
        let (id, kind, size, depth) = r.resolved.ok_or(Error::Corrupt("pack: unresolvable delta"))?;
        let mut crc = Crc32::new();
        crc.update(&data[r.offset as usize..r.end as usize]);
        objects.push(Indexed { id, offset: r.offset, crc: crc.finish(), kind, size, depth });
    }
    Ok(IndexResult { objects, checksum: pack.checksum(), external_bases })
}

// ---------------------------------------------------------------------------------------------
// Pack writer
// ---------------------------------------------------------------------------------------------

/// One object to pack.
#[derive(Debug, Clone)]
pub struct PackObject {
    /// Kind.
    pub kind: Kind,
    /// Payload.
    pub data: Vec<u8>,
    /// The path it was reached by (a delta-ordering hint; optional).
    pub path: Option<Vec<u8>>,
}

/// Pack writer options.
#[derive(Debug, Clone, Copy)]
pub struct WriteOptions {
    /// zlib level (git's `pack.compression` default is 6 via core.compression -1).
    pub level: u8,
    /// Delta window (`pack.window`, default 10); 0 disables deltas.
    pub window: usize,
    /// Maximum delta chain depth (`pack.depth`, default 50).
    pub depth: u32,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions { level: deflate::DEFAULT_LEVEL, window: 10, depth: 50 }
    }
}

/// git's `pack_name_hash`: the last characters of a path dominate, so same-named files cluster.
pub fn name_hash(path: &[u8]) -> u32 {
    let mut h = 0u32;
    for &c in path {
        if c.is_ascii_whitespace() {
            continue;
        }
        h = (h >> 2).wrapping_add((c as u32) << 24);
    }
    h
}

/// A written pack.
pub struct Written {
    /// The pack bytes.
    pub pack: Vec<u8>,
    /// (id, offset, crc) per object.
    pub entries: Vec<(ObjectId, u64, u32)>,
    /// How many entries were written as deltas.
    pub deltas: usize,
}

impl Written {
    /// The idx v2.
    pub fn idx(&self, hk: HashKind) -> Vec<u8> {
        let mut e = self.entries.clone();
        let ck = ObjectId::from_bytes(hk, &self.pack[self.pack.len() - hk.len()..]);
        write_idx(hk, &mut e, &ck)
    }
}

/// Write a v2 pack of `objects` (duplicates by id are written once).
pub fn write_pack(hk: HashKind, objects: &[PackObject], opts: WriteOptions) -> Written {
    let ids: Vec<ObjectId> = objects.iter().map(|o| object::hash_object(hk, o.kind, &o.data)).collect();
    let mut order: Vec<usize> = (0..objects.len()).collect();
    // Delta ordering: kind, path hash, size descending (git's type_size_sort).
    order.sort_by(|&a, &b| {
        let (oa, ob) = (&objects[a], &objects[b]);
        (oa.kind as u8)
            .cmp(&(ob.kind as u8))
            .then(name_hash(ob.path.as_deref().unwrap_or(b"")).cmp(&name_hash(oa.path.as_deref().unwrap_or(b""))))
            .then(ob.data.len().cmp(&oa.data.len()))
            .then(a.cmp(&b))
    });
    let mut seen = alloc::collections::BTreeSet::new();
    order.retain(|&i| seen.insert(ids[i]));
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(order.len() as u32).to_be_bytes());
    let mut entries = Vec::with_capacity(order.len());
    // window of (object index, offset, depth)
    let mut window: Vec<(usize, u64, u32, Option<delta::DeltaIndex<'_>>)> = Vec::new();
    let mut deltas = 0;
    for &i in &order {
        let o = &objects[i];
        let offset = out.len() as u64;
        let mut best: Option<(Vec<u8>, u64, u32)> = None;
        if opts.window > 0 && o.data.len() >= 32 {
            let mut limit = o.data.len() / 2;
            for (b, boff, bdepth, bindex) in window.iter_mut().rev() {
                let (b, boff, bdepth) = (*b, *boff, *bdepth);
                let base = &objects[b];
                if base.kind != o.kind || bdepth >= opts.depth {
                    continue;
                }
                if base.data.len() < o.data.len() / 32 || base.data.len() / 32 > o.data.len() {
                    continue;
                }
                let index = bindex.get_or_insert_with(|| delta::DeltaIndex::new(&base.data));
                if let Some(d) = index.delta(&o.data, limit) {
                    limit = d.len().saturating_sub(1);
                    best = Some((d, boff, bdepth + 1));
                }
            }
        }
        let start = out.len();
        let depth = match &best {
            Some((d, boff, depth)) => {
                write_entry_header(&mut out, types::OFS_DELTA, d.len() as u64);
                write_ofs(&mut out, offset - boff);
                out.extend_from_slice(&deflate::zlib_compress(d, opts.level));
                deltas += 1;
                *depth
            }
            None => {
                write_entry_header(&mut out, o.kind as u8, o.data.len() as u64);
                out.extend_from_slice(&deflate::zlib_compress(&o.data, opts.level));
                0
            }
        };
        let mut crc = Crc32::new();
        crc.update(&out[start..]);
        entries.push((ids[i], offset, crc.finish()));
        if opts.window > 0 {
            window.push((i, offset, depth, None));
            if window.len() > opts.window {
                window.remove(0);
            }
        }
    }
    let d = hk.digest(&out);
    out.extend_from_slice(d.as_bytes());
    Written { pack: out, entries, deltas }
}

// ---------------------------------------------------------------------------------------------
// multi-pack-index
// ---------------------------------------------------------------------------------------------

/// A multi-pack-index (`objects/pack/multi-pack-index`), read side.
pub struct Midx<'a> {
    data: &'a [u8],
    /// Object format.
    pub hash: HashKind,
    /// Pack index names (`pack-….idx`) in pack-int-id order.
    pub packs: Vec<&'a [u8]>,
    /// Number of objects.
    pub count: u32,
    oidf: usize,
    oidl: usize,
    ooff: usize,
    loff: Option<(usize, usize)>,
}

impl<'a> Midx<'a> {
    /// Parse and validate the chunk table.
    pub fn parse(data: &'a [u8], verify: bool) -> Result<Self> {
        if data.len() < 12 || &data[..4] != b"MIDX" {
            return Err(Error::Corrupt("midx: bad signature"));
        }
        if data[4] != 1 {
            return Err(Error::Unsupported("midx version"));
        }
        let hash = match data[5] {
            1 => HashKind::Sha1,
            2 => HashKind::Sha256,
            _ => return Err(Error::Corrupt("midx: bad oid version")),
        };
        let chunks = data[6] as usize;
        if data[7] != 0 {
            return Err(Error::Unsupported("midx: incremental base files"));
        }
        let npacks = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
        let table_end = 12 + (chunks + 1) * 12;
        if data.len() < table_end + hash.len() {
            return Err(Error::Corrupt("midx: truncated chunk table"));
        }
        if verify {
            let end = data.len() - hash.len();
            if hash.digest(&data[..end]).as_bytes() != &data[end..] {
                return Err(Error::Corrupt("midx: checksum mismatch"));
            }
        }
        let mut table: Vec<([u8; 4], usize)> = Vec::new();
        for k in 0..=chunks {
            let at = 12 + 12 * k;
            let id: [u8; 4] = data[at..at + 4].try_into().unwrap();
            let off = u64::from_be_bytes(data[at + 4..at + 12].try_into().unwrap()) as usize;
            if off > data.len() - hash.len() {
                return Err(Error::Corrupt("midx: chunk offset out of range"));
            }
            table.push((id, off));
        }
        let span = |name: &[u8; 4]| -> Option<(usize, usize)> {
            let k = table[..chunks].iter().position(|(id, _)| id == name)?;
            let start = table[k].1;
            let end = table[k + 1].1;
            (end >= start).then_some((start, end))
        };
        let (pnam, pnam_end) = span(b"PNAM").ok_or(Error::Corrupt("midx: no PNAM"))?;
        let (oidf, oidf_end) = span(b"OIDF").ok_or(Error::Corrupt("midx: no OIDF"))?;
        let (oidl, _) = span(b"OIDL").ok_or(Error::Corrupt("midx: no OIDL"))?;
        let (ooff, _) = span(b"OOFF").ok_or(Error::Corrupt("midx: no OOFF"))?;
        let loff = span(b"LOFF");
        if oidf_end - oidf != 1024 {
            return Err(Error::Corrupt("midx: OIDF size"));
        }
        let count = u32::from_be_bytes(data[oidf + 1020..oidf + 1024].try_into().unwrap());
        let c = count as usize;
        if data.len() < oidl + c * hash.len() || data.len() < ooff + c * 8 {
            return Err(Error::Corrupt("midx: tables truncated"));
        }
        let packs: Vec<&[u8]> = data[pnam..pnam_end].split(|&b| b == 0).filter(|s| !s.is_empty()).collect();
        if packs.len() != npacks {
            return Err(Error::Corrupt("midx: PNAM count disagrees with header"));
        }
        Ok(Midx { data, hash, packs, count, oidf, oidl, ooff, loff })
    }

    fn fan(&self, k: usize) -> u32 {
        u32::from_be_bytes(self.data[self.oidf + 4 * k..self.oidf + 4 * k + 4].try_into().unwrap())
    }

    /// The i-th id.
    pub fn oid(&self, i: usize) -> ObjectId {
        let n = self.hash.len();
        ObjectId::from_bytes(self.hash, &self.data[self.oidl + i * n..self.oidl + (i + 1) * n])
    }

    /// (pack-int-id, offset) of the i-th object.
    pub fn location(&self, i: usize) -> Result<(u32, u64)> {
        let at = self.ooff + 8 * i;
        let pack = u32::from_be_bytes(self.data[at..at + 4].try_into().unwrap());
        let v = u32::from_be_bytes(self.data[at + 4..at + 8].try_into().unwrap());
        let off = if v & 0x8000_0000 != 0 {
            let (s, e) = self.loff.ok_or(Error::Corrupt("midx: large offset without LOFF"))?;
            let k = (v & 0x7fff_ffff) as usize;
            let a = s + 8 * k;
            if a + 8 > e {
                return Err(Error::Corrupt("midx: LOFF index out of range"));
            }
            u64::from_be_bytes(self.data[a..a + 8].try_into().unwrap())
        } else {
            v as u64
        };
        Ok((pack, off))
    }

    /// Look up `id`: (pack-int-id, offset).
    pub fn lookup(&self, id: &ObjectId) -> Option<(u32, u64)> {
        let k = id.first_byte() as usize;
        let mut lo = if k == 0 { 0 } else { self.fan(k - 1) as usize };
        let mut hi = self.fan(k) as usize;
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.oid(mid).as_bytes().cmp(id.as_bytes()) {
                core::cmp::Ordering::Equal => return self.location(mid).ok(),
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }
}
