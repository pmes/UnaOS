//! HPACK (RFC 7541): integer representation (§5.1), string literals with the static Huffman code (§5.2,
//! Appendix B), the static table (Appendix A), the dynamic table with eviction (§4), every field
//! representation (§6) and dynamic table size updates (§6.3, §4.2). A decoder and an encoder.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use super::huffman_table::CODES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpackError {
    /// The block ended inside a representation.
    Truncated,
    /// An integer that does not fit (more than 2^32 or more than 5 continuation octets).
    IntegerOverflow,
    /// Index 0, or past the end of static + dynamic tables.
    BadIndex,
    /// Huffman: EOS inside a string, padding longer than 7 bits, or padding that is not all ones (§5.2).
    BadHuffman,
    /// A dynamic table size update above the SETTINGS limit, or not at the start of a block (§4.2).
    BadSizeUpdate,
    /// A string longer than the limit we accept.
    TooLong,
}

impl fmt::Display for HpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "hpack: {self:?}")
    }
}

/// RFC 7541 Appendix A.
pub const STATIC_TABLE: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

pub type Field = (Vec<u8>, Vec<u8>);

/// §4.1: an entry's size is name + value + 32.
fn entry_size(n: &[u8], v: &[u8]) -> usize {
    n.len() + v.len() + 32
}

/// The dynamic table (§2.3.2): newest entry first.
#[derive(Debug, Clone)]
pub struct DynamicTable {
    entries: alloc::collections::VecDeque<Field>,
    size: usize,
    max: usize,
}

impl DynamicTable {
    pub fn new(max: usize) -> Self {
        DynamicTable { entries: alloc::collections::VecDeque::new(), size: 0, max }
    }
    pub fn size(&self) -> usize {
        self.size
    }
    pub fn max(&self) -> usize {
        self.max
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    fn evict(&mut self) {
        while self.size > self.max {
            let (n, v) = self.entries.pop_back().expect("size > 0 means an entry");
            self.size -= entry_size(&n, &v);
        }
    }
    /// §4.3 resize.
    pub fn set_max(&mut self, max: usize) {
        self.max = max;
        self.evict();
    }
    /// §4.4 add (an entry larger than the table empties it).
    pub fn insert(&mut self, n: Vec<u8>, v: Vec<u8>) {
        let s = entry_size(&n, &v);
        if s > self.max {
            self.entries.clear();
            self.size = 0;
            return;
        }
        self.size += s;
        self.entries.push_front((n, v));
        self.evict();
    }
    fn get(&self, i: usize) -> Option<&Field> {
        self.entries.get(i)
    }
}

// ---------------------------------------------------------------- §5.1 integers

/// Decode an integer with an `n`-bit prefix starting at `buf[*p]`.
pub fn decode_int(buf: &[u8], p: &mut usize, n: u8) -> Result<u64, HpackError> {
    let mask = ((1u16 << n) - 1) as u8;
    let first = *buf.get(*p).ok_or(HpackError::Truncated)?;
    *p += 1;
    let mut v = (first & mask) as u64;
    if v < mask as u64 {
        return Ok(v);
    }
    let mut m = 0u32;
    loop {
        let b = *buf.get(*p).ok_or(HpackError::Truncated)?;
        *p += 1;
        if m > 28 {
            return Err(HpackError::IntegerOverflow);
        }
        v += ((b & 0x7F) as u64) << m;
        m += 7;
        if v > u32::MAX as u64 {
            return Err(HpackError::IntegerOverflow);
        }
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
}

/// Encode `v` with an `n`-bit prefix; `flags` are the bits above the prefix in the first octet.
pub fn encode_int(out: &mut Vec<u8>, v: u64, n: u8, flags: u8) {
    let mask = ((1u16 << n) - 1) as u64;
    if v < mask {
        out.push(flags | v as u8);
        return;
    }
    out.push(flags | mask as u8);
    let mut r = v - mask;
    while r >= 128 {
        out.push((r % 128) as u8 | 0x80);
        r /= 128;
    }
    out.push(r as u8);
}

// ---------------------------------------------------------------- §5.2 Huffman

/// A decoding tree: node i has children [2i], [2i+1]: values < 0x8000 are node indices, 0x8000|sym leaves.
struct Tree {
    nodes: Vec<[u16; 2]>,
}

fn tree() -> Tree {
    let mut nodes: Vec<[u16; 2]> = vec![[0, 0]];
    for (sym, &(code, len)) in CODES.iter().enumerate() {
        let mut cur = 0usize;
        for i in (0..len).rev() {
            let bit = ((code >> i) & 1) as usize;
            if i == 0 {
                nodes[cur][bit] = 0x8000 | sym as u16;
            } else {
                if nodes[cur][bit] == 0 {
                    nodes.push([0, 0]);
                    let idx = (nodes.len() - 1) as u16;
                    nodes[cur][bit] = idx;
                }
                cur = nodes[cur][bit] as usize;
            }
        }
    }
    Tree { nodes }
}

/// Decode a Huffman-coded string literal.
pub fn huffman_decode(input: &[u8]) -> Result<Vec<u8>, HpackError> {
    let t = tree();
    let mut out = Vec::with_capacity(input.len() * 8 / 5);
    let mut cur = 0usize;
    let mut depth = 0u32;
    let mut all_ones = true;
    for &byte in input {
        for i in (0..8).rev() {
            let bit = ((byte >> i) & 1) as usize;
            all_ones &= bit == 1;
            depth += 1;
            let next = t.nodes[cur][bit];
            if next & 0x8000 != 0 {
                let sym = next & 0x7FFF;
                if sym == 256 {
                    return Err(HpackError::BadHuffman);
                }
                out.push(sym as u8);
                cur = 0;
                depth = 0;
                all_ones = true;
            } else if next == 0 {
                return Err(HpackError::BadHuffman);
            } else {
                cur = next as usize;
            }
        }
    }
    // Padding: at most 7 bits, all ones (the most significant bits of EOS).
    if depth > 7 || !all_ones {
        return Err(HpackError::BadHuffman);
    }
    Ok(out)
}

/// Huffman-encode `input`.
pub fn huffman_encode(input: &[u8], out: &mut Vec<u8>) {
    let (mut acc, mut bits) = (0u64, 0u32);
    for &b in input {
        let (code, len) = CODES[b as usize];
        acc = (acc << len) | code as u64;
        bits += len as u32;
        while bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= (1u64 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (8 - bits)) | ((1u64 << (8 - bits)) - 1)) as u8);
    }
}

pub fn huffman_len(input: &[u8]) -> usize {
    (input.iter().map(|&b| CODES[b as usize].1 as usize).sum::<usize>() + 7) / 8
}

const MAX_STRING: u64 = 1 << 20;

fn decode_string(buf: &[u8], p: &mut usize) -> Result<Vec<u8>, HpackError> {
    let h = *buf.get(*p).ok_or(HpackError::Truncated)? & 0x80 != 0;
    let len = decode_int(buf, p, 7)?;
    if len > MAX_STRING {
        return Err(HpackError::TooLong);
    }
    let end = p.checked_add(len as usize).ok_or(HpackError::Truncated)?;
    let raw = buf.get(*p..end).ok_or(HpackError::Truncated)?;
    *p = end;
    if h { huffman_decode(raw) } else { Ok(raw.to_vec()) }
}

fn encode_string(out: &mut Vec<u8>, s: &[u8]) {
    let hl = huffman_len(s);
    if hl < s.len() {
        encode_int(out, hl as u64, 7, 0x80);
        huffman_encode(s, out);
    } else {
        encode_int(out, s.len() as u64, 7, 0);
        out.extend_from_slice(s);
    }
}

// ---------------------------------------------------------------- the decoder (§6, §3)

pub struct Decoder {
    table: DynamicTable,
    /// The SETTINGS_HEADER_TABLE_SIZE we advertised: the ceiling for size updates.
    limit: usize,
}

impl Decoder {
    pub fn new(limit: usize) -> Self {
        Decoder { table: DynamicTable::new(limit), limit }
    }
    pub fn table(&self) -> &DynamicTable {
        &self.table
    }
    /// A new SETTINGS_HEADER_TABLE_SIZE acknowledged by the peer.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }

    fn lookup(&self, idx: u64) -> Result<Field, HpackError> {
        let i = idx as usize;
        if i == 0 {
            return Err(HpackError::BadIndex);
        }
        if i <= STATIC_TABLE.len() {
            let (n, v) = STATIC_TABLE[i - 1];
            return Ok((n.as_bytes().to_vec(), v.as_bytes().to_vec()));
        }
        self.table.get(i - STATIC_TABLE.len() - 1).cloned().ok_or(HpackError::BadIndex)
    }

    /// Decode one complete header block.
    pub fn decode(&mut self, block: &[u8]) -> Result<Vec<Field>, HpackError> {
        let mut out = Vec::new();
        let mut p = 0;
        let mut fields_seen = false;
        while p < block.len() {
            let b = block[p];
            if b & 0x80 != 0 {
                let idx = decode_int(block, &mut p, 7)?;
                out.push(self.lookup(idx)?);
                fields_seen = true;
            } else if b & 0x40 != 0 {
                let idx = decode_int(block, &mut p, 6)?;
                let name = if idx == 0 { decode_string(block, &mut p)? } else { self.lookup(idx)?.0 };
                let value = decode_string(block, &mut p)?;
                self.table.insert(name.clone(), value.clone());
                out.push((name, value));
                fields_seen = true;
            } else if b & 0x20 != 0 {
                // §4.2: a size update must come first in the block, and stay within the limit.
                if fields_seen {
                    return Err(HpackError::BadSizeUpdate);
                }
                let size = decode_int(block, &mut p, 5)?;
                if size as usize > self.limit {
                    return Err(HpackError::BadSizeUpdate);
                }
                self.table.set_max(size as usize);
            } else {
                // 0000 without indexing, 0001 never indexed: same shape, 4-bit prefix.
                let idx = decode_int(block, &mut p, 4)?;
                let name = if idx == 0 { decode_string(block, &mut p)? } else { self.lookup(idx)?.0 };
                let value = decode_string(block, &mut p)?;
                out.push((name, value));
                fields_seen = true;
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- the encoder

pub struct Encoder {
    table: DynamicTable,
    pending_size_update: Option<usize>,
}

/// Fields that must not enter the dynamic table (§7.1.3: credentials are "never indexed").
fn sensitive(name: &[u8]) -> bool {
    matches!(name, b"authorization" | b"cookie" | b"proxy-authorization" | b"x-api-key" | b"x-goog-api-key")
}

impl Encoder {
    pub fn new(max: usize) -> Self {
        Encoder { table: DynamicTable::new(max), pending_size_update: None }
    }
    /// The peer's SETTINGS_HEADER_TABLE_SIZE changed: emit a size update with the next block.
    pub fn set_max(&mut self, max: usize) {
        self.table.set_max(max);
        self.pending_size_update = Some(max);
    }

    fn find(&self, n: &[u8], v: &[u8]) -> (Option<usize>, Option<usize>) {
        let (mut full, mut name) = (None, None);
        for (i, (sn, sv)) in STATIC_TABLE.iter().enumerate() {
            if sn.as_bytes() == n {
                name.get_or_insert(i + 1);
                if sv.as_bytes() == v {
                    full = Some(i + 1);
                    break;
                }
            }
        }
        if full.is_none() {
            for (i, (dn, dv)) in self.table.entries.iter().enumerate() {
                if dn.as_slice() == n {
                    name.get_or_insert(STATIC_TABLE.len() + 1 + i);
                    if dv.as_slice() == v {
                        full = Some(STATIC_TABLE.len() + 1 + i);
                        break;
                    }
                }
            }
        }
        (full, name)
    }

    /// Encode one header block (names must already be lowercase, RFC 9113 §8.2.1).
    pub fn encode<'a>(&mut self, fields: impl IntoIterator<Item = (&'a [u8], &'a [u8])>) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(s) = self.pending_size_update.take() {
            encode_int(&mut out, s as u64, 5, 0x20);
        }
        for (n, v) in fields {
            let (full, name) = self.find(n, v);
            if let Some(i) = full {
                encode_int(&mut out, i as u64, 7, 0x80);
            } else if sensitive(n) {
                encode_int(&mut out, name.unwrap_or(0) as u64, 4, 0x10);
                if name.is_none() {
                    encode_string(&mut out, n);
                }
                encode_string(&mut out, v);
            } else {
                encode_int(&mut out, name.unwrap_or(0) as u64, 6, 0x40);
                if name.is_none() {
                    encode_string(&mut out, n);
                }
                encode_string(&mut out, v);
                self.table.insert(n.to_vec(), v.to_vec());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_table_is_a_complete_prefix_code() {
        // Kraft: Σ 2^-len == 1 exactly (a complete code), and the tree build put every symbol on a leaf.
        let total: u128 = CODES.iter().map(|&(_, l)| 1u128 << (30 - l as u32)).sum();
        assert_eq!(total, 1u128 << 30);
        let t = tree();
        assert_eq!(t.nodes.len(), 256, "a complete binary code over 257 leaves has 256 internal nodes");
    }

    /// RFC 7541 §C.1 integer examples.
    #[test]
    fn rfc7541_c1_integers() {
        let mut o = Vec::new();
        encode_int(&mut o, 10, 5, 0);
        assert_eq!(o, [0x0a]);
        o.clear();
        encode_int(&mut o, 1337, 5, 0);
        assert_eq!(o, [0x1f, 0x9a, 0x0a]);
        o.clear();
        encode_int(&mut o, 42, 8, 0);
        assert_eq!(o, [0x2a]);
        let mut p = 0;
        assert_eq!(decode_int(&[0x1f, 0x9a, 0x0a], &mut p, 5), Ok(1337));
        let mut p = 0;
        assert_eq!(decode_int(&[0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0x0f], &mut p, 5), Err(HpackError::IntegerOverflow));
    }

    fn unhex(s: &str) -> Vec<u8> {
        let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(core::str::from_utf8(&s[i..i + 2]).unwrap(), 16).unwrap()).collect()
    }

    fn fields(v: &[Field]) -> Vec<(&str, &str)> {
        v.iter().map(|(n, v)| (core::str::from_utf8(n).unwrap(), core::str::from_utf8(v).unwrap())).collect()
    }

    /// RFC 7541 §C.4 (requests with Huffman) and §C.6 (responses with Huffman, a 256-octet table: eviction).
    #[test]
    fn rfc7541_c4_and_c6() {
        let mut d = Decoder::new(4096);
        let r = d.decode(&unhex("8286 8441 8cf1 e3c2 e5f2 3a6b a0ab 90f4 ff")).unwrap();
        assert_eq!(fields(&r), [(":method", "GET"), (":scheme", "http"), (":path", "/"), (":authority", "www.example.com")]);
        let r = d.decode(&unhex("8286 84be 5886 a8eb 1064 9cbf")).unwrap();
        assert_eq!(fields(&r)[4], ("cache-control", "no-cache"));
        let r = d.decode(&unhex("8287 85bf 4088 25a8 49e9 5ba9 7d7f 8925 a849 e95b b8e8 b4bf")).unwrap();
        assert_eq!(fields(&r)[4], ("custom-key", "custom-value"));
        assert_eq!(d.table().size(), 164);

        let mut d = Decoder::new(256);
        d.decode(&unhex("4882 6402 5885 aec3 771a 4b61 96d0 7abe 9410 54d4 44a8 2005 9504 0b81 66e0 82a6 2d1b ff6e 919d 29ad 1718 63c7 8f0b 97c8 e9ae 82ae 43d3")).unwrap();
        assert_eq!(d.table().size(), 222);
        let r = d.decode(&unhex("4883 640e ffc1 c0bf")).unwrap();
        assert_eq!(fields(&r)[0], (":status", "307"));
        assert_eq!(d.table().size(), 222);
        let r = d.decode(&unhex(
            "88c1 6196 d07a be94 1054 d444 a820 0595 040b 8166 e084 a62d 1bff c05a 839b d9ab 77ad 94e7 821d d7f2 e6c7 b335 dfdf cd5b 3960 d5af 2708 7f36 72c1 ab27 0fb5 291f 9587 3160 65c0 03ed 4ee5 b106 3d50 07",
        ))
        .unwrap();
        assert_eq!(fields(&r)[5], ("set-cookie", "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1"));
        assert_eq!(d.table().size(), 215);
    }

    #[test]
    fn huffman_refusals_and_round_trip() {
        // "www.example.com" Huffman-coded, then corrupt the padding.
        let mut good = Vec::new();
        huffman_encode(b"www.example.com", &mut good);
        assert_eq!(good, unhex("f1e3 c2e5 f23a 6ba0 ab90 f4ff"));
        assert_eq!(huffman_decode(&good).unwrap(), b"www.example.com");
        let mut bad = good.clone();
        *bad.last_mut().unwrap() &= 0xfe; // padding not all ones
        assert!(huffman_decode(&bad).is_err());
        assert_eq!(huffman_decode(&[0xff, 0xff, 0xff, 0xff]), Err(HpackError::BadHuffman)); // EOS
        for s in [&b""[..], b"a", b"\x00\xff\x7f", b"Una \xc3\xa9t\xc3\xa9"] {
            let mut e = Vec::new();
            huffman_encode(s, &mut e);
            assert_eq!(huffman_decode(&e).unwrap(), s);
        }
    }
}
