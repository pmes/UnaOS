//! Vorbis I, from the Xiph.Org specification (2020-07-04 revision): the three headers, Huffman codebooks
//! with VQ lookup types 1 and 2, floor 0 (LSP) and floor 1 (piecewise linear), residues 0/1/2, channel
//! coupling, the power-sine windows with long/short transitions, the inverse MDCT (an N/4-point complex FFT)
//! and overlap-add; plus the Ogg mapping (§A: granule positions, start and end trimming). Floating point.
pub mod tables;

use crate::bits::LsbReader;
use crate::math;
use crate::ogg::{OggReader, Packet};
use crate::{Codec, Error, Format, Info, Pcm, Result, Source};
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use tables::FLOOR1_INVERSE_DB;

#[inline] fn ilog(v: u32) -> u32 { 32 - v.leading_zeros() }
const UNUSED: i32 = -1;

fn float32_unpack(x: u32) -> f32 {
    let mut mantissa = (x & 0x1fffff) as f64;
    let exponent = ((x & 0x7fe00000) >> 21) as i32;
    if x & 0x80000000 != 0 { mantissa = -mantissa; }
    math::ldexp(mantissa, exponent - 788) as f32
}

fn lookup1_values(entries: u32, dims: u32) -> u32 {
    if entries < 1 || dims == 0 { return 0; }
    let mut vals = math::floor(math::pow(entries as f64, 1.0 / dims as f64)) as i64;
    if vals < 1 { vals = 1; }
    loop {
        let mut acc: i64 = 1;
        let mut acc1: i64 = 1;
        let mut i = 0;
        while i < dims {
            if entries as i64 / vals < acc { break; }
            acc *= vals;
            acc1 = acc1.saturating_mul(vals + 1);
            i += 1;
        }
        if i >= dims && acc <= entries as i64 && acc1 > entries as i64 { return vals as u32; }
        if i < dims || acc > entries as i64 { vals -= 1; } else { vals += 1; }
        if vals < 1 { return 0; }
    }
}

// ---------------------------------------------------------------- codebooks

const TABLE_BITS: u32 = 10;

struct Codebook {
    dims: usize,
    /// For a code of length ≤ TABLE_BITS: (entry << 8) | len; otherwise 0xFFFF_FFFF = walk the tree.
    table: Vec<u32>,
    /// Binary tree in reading order: child[0/1] ≥ 0 is a node, < 0 is -(entry+1); i32::MIN = invalid.
    tree: Vec<[i32; 2]>,
    vq: Vec<f32>,
    has_vq: bool,
}

impl Codebook {
    fn parse(r: &mut LsbReader) -> Result<Codebook> {
        if r.read(24)? != 0x564342 { return Err(Error::Invalid("Vorbis codebook sync")); }
        let dims = r.read(16)? as usize;
        let entries = r.read(24)? as usize;
        if entries == 0 { return Err(Error::Invalid("Vorbis codebook with no entries")); }
        let mut lengths = vec![0u8; entries];
        if !r.bit()? {
            let sparse = r.bit()?;
            for l in lengths.iter_mut() {
                if !sparse || r.bit()? { *l = r.read(5)? as u8 + 1; }
            }
        } else {
            let mut cur = 0usize;
            let mut len = r.read(5)? + 1;
            while cur < entries {
                let n = r.read(ilog((entries - cur) as u32))? as usize;
                if cur + n > entries || len > 32 { return Err(Error::Invalid("Vorbis ordered codebook")); }
                for l in lengths[cur..cur + n].iter_mut() { *l = len as u8; }
                cur += n;
                len += 1;
            }
        }
        let lookup_type = r.read(4)?;
        let mut vq = vec![];
        let mut has_vq = false;
        match lookup_type {
            0 => {}
            1 | 2 => {
                let min = float32_unpack(r.read(32)?);
                let delta = float32_unpack(r.read(32)?);
                let value_bits = r.read(4)? + 1;
                let sequence_p = r.bit()?;
                let lookup_values = if lookup_type == 1 { lookup1_values(entries as u32, dims as u32) as usize } else { entries * dims };
                if lookup_values == 0 && dims > 0 { return Err(Error::Invalid("Vorbis VQ lookup values")); }
                if lookup_values > 1 << 20 { return Err(Error::Unsupported("Vorbis VQ table too large")); }
                let mut mult = vec![0u32; lookup_values];
                for m in mult.iter_mut() { *m = r.read(value_bits)?; }
                if dims as u64 * entries as u64 > 1 << 22 { return Err(Error::Unsupported("Vorbis VQ table too large")); }
                vq = vec![0f32; entries * dims];
                for e in 0..entries {
                    let mut last = 0f32;
                    let mut index_divisor = 1usize;
                    for i in 0..dims {
                        let off = if lookup_type == 1 { (e / index_divisor) % lookup_values } else { e * dims + i };
                        let v = mult[off] as f32 * delta + min + last;
                        vq[e * dims + i] = v;
                        if sequence_p { last = v; }
                        if lookup_type == 1 { index_divisor = index_divisor.saturating_mul(lookup_values); }
                    }
                }
                has_vq = true;
            }
            _ => return Err(Error::Invalid("Vorbis codebook lookup type")),
        }
        let mut cb = Codebook { dims, table: vec![], tree: vec![], vq, has_vq };
        cb.build(&lengths)?;
        Ok(cb)
    }

    fn build(&mut self, lengths: &[u8]) -> Result<()> {
        // codeword assignment (spec §3.2.1; the same marker walk as libvorbis's _make_words)
        let mut marker = [0u32; 33];
        let mut words = vec![0u32; lengths.len()];
        let mut count = 0usize;
        for (i, &l) in lengths.iter().enumerate() {
            let length = l as usize;
            if length == 0 { continue; }
            let mut entry = marker[length];
            if length < 32 && (entry >> length) != 0 { return Err(Error::Invalid("Vorbis codebook overspecified")); }
            words[i] = entry;
            count += 1;
            let mut j = length;
            while j > 0 {
                if marker[j] & 1 != 0 {
                    if j == 1 { marker[1] += 1; } else { marker[j] = marker[j - 1] << 1; }
                    break;
                }
                marker[j] += 1;
                j -= 1;
            }
            for j in length + 1..33 {
                if (marker[j] >> 1) == entry {
                    entry = marker[j];
                    marker[j] = marker[j - 1] << 1;
                } else {
                    break;
                }
            }
        }
        if !(count == 1 && marker[2] == 2) {
            for (i, &m) in marker.iter().enumerate().skip(1) {
                if m & (0xffff_ffffu32 >> (32 - i)) != 0 { return Err(Error::Invalid("Vorbis codebook underspecified")); }
            }
        }
        // tree in reading order (the codeword's MSB is read first)
        self.tree = vec![[i32::MIN, i32::MIN]];
        for (i, &l) in lengths.iter().enumerate() {
            if l == 0 { continue; }
            let w = words[i];
            let mut node = 0usize;
            for b in (0..l as u32).rev() {
                let bit = ((w >> b) & 1) as usize;
                if b == 0 {
                    self.tree[node][bit] = -(i as i32) - 1;
                } else {
                    let c = self.tree[node][bit];
                    if c >= 0 && c != i32::MIN { node = c as usize; } else {
                        self.tree.push([i32::MIN, i32::MIN]);
                        let n = self.tree.len() - 1;
                        self.tree[node][bit] = n as i32;
                        node = n;
                    }
                }
            }
        }
        // fast table indexed by the next TABLE_BITS bits in reading order (LSB of the peek = first bit)
        self.table = vec![0xFFFF_FFFF; 1 << TABLE_BITS];
        for (i, &l) in lengths.iter().enumerate() {
            if l == 0 || l as u32 > TABLE_BITS { continue; }
            let w = words[i];
            let mut rev = 0u32;
            for b in 0..l as u32 { rev |= ((w >> (l as u32 - 1 - b)) & 1) << b; }
            let step = 1u32 << l;
            let mut idx = rev;
            while idx < 1 << TABLE_BITS {
                self.table[idx as usize] = ((i as u32) << 8) | l as u32;
                idx += step;
            }
        }
        Ok(())
    }

    #[inline]
    fn decode(&self, r: &mut LsbReader) -> Result<usize> {
        let (peek, avail) = r.peek(TABLE_BITS);
        let t = self.table[peek as usize];
        if t != 0xFFFF_FFFF {
            let l = (t & 0xFF) as usize;
            if l > avail { r.advance(avail); return Err(Error::Eof); }
            r.advance(l);
            return Ok((t >> 8) as usize);
        }
        let mut node = 0usize;
        loop {
            let bit = r.read(1)? as usize;
            let c = self.tree[node][bit];
            if c == i32::MIN { return Err(Error::Invalid("Vorbis invalid codeword")); }
            if c < 0 { return Ok((-(c + 1)) as usize); }
            node = c as usize;
        }
    }

    #[inline]
    fn vector(&self, entry: usize) -> &[f32] { &self.vq[entry * self.dims..entry * self.dims + self.dims] }
}

// ---------------------------------------------------------------- floors, residues, mappings

enum Floor {
    Zero { order: usize, rate: u32, bark_map_size: u32, amplitude_bits: u32, amplitude_offset: u32, books: Vec<usize> },
    One(Floor1),
}

struct Floor1 {
    partition_class: Vec<usize>,
    class_dims: Vec<usize>,
    class_subclasses: Vec<u32>,
    class_masterbook: Vec<usize>,
    subclass_books: Vec<Vec<i32>>,
    multiplier: i32,
    x: Vec<u32>,
    /// x-sorted order, and the low/high neighbours of each point (§7.2.4)
    sorted: Vec<usize>,
    low: Vec<usize>,
    high: Vec<usize>,
}

struct Residue {
    kind: u32,
    begin: usize,
    end: usize,
    partition_size: usize,
    classifications: usize,
    classbook: usize,
    books: Vec<[i32; 8]>,
}

struct Mapping {
    coupling: Vec<(usize, usize)>,
    mux: Vec<usize>,
    submap_floor: Vec<usize>,
    submap_residue: Vec<usize>,
}

struct Mode { blockflag: bool, mapping: usize }

pub struct Setup {
    pub channels: usize,
    pub rate: u32,
    pub blocksize: [usize; 2],
    books: Vec<Codebook>,
    floors: Vec<Floor>,
    residues: Vec<Residue>,
    mappings: Vec<Mapping>,
    modes: Vec<Mode>,
}

impl Setup {
    pub fn parse_ident(d: &[u8]) -> Result<(usize, u32, [usize; 2])> {
        if d.len() < 30 || d[0] != 1 || &d[1..7] != b"vorbis" { return Err(Error::Invalid("Vorbis identification header")); }
        if u32::from_le_bytes([d[7], d[8], d[9], d[10]]) != 0 { return Err(Error::Unsupported("Vorbis version")); }
        let channels = d[11] as usize;
        let rate = u32::from_le_bytes([d[12], d[13], d[14], d[15]]);
        let b0 = 1usize << (d[28] & 15);
        let b1 = 1usize << (d[28] >> 4);
        if channels == 0 || rate == 0 || b0 < 64 || b1 > 8192 || b0 > b1 || d[29] & 1 == 0 {
            return Err(Error::Invalid("Vorbis identification header"));
        }
        Ok((channels, rate, [b0, b1]))
    }

    pub fn parse(ident: &[u8], setup: &[u8]) -> Result<Setup> {
        let (channels, rate, blocksize) = Setup::parse_ident(ident)?;
        if setup.len() < 7 || setup[0] != 5 || &setup[1..7] != b"vorbis" { return Err(Error::Invalid("Vorbis setup header")); }
        let mut r = LsbReader::new(&setup[7..]);
        let r = &mut r;
        let n = r.read(8)? as usize + 1;
        let mut books = Vec::with_capacity(n);
        for _ in 0..n { books.push(Codebook::parse(r)?); }
        let n = r.read(6)? + 1;
        for _ in 0..n { if r.read(16)? != 0 { return Err(Error::Invalid("Vorbis time domain transform")); } }
        let n = r.read(6)? + 1;
        let mut floors = vec![];
        for _ in 0..n {
            match r.read(16)? {
                0 => {
                    let order = r.read(8)? as usize;
                    let rate = r.read(16)?;
                    let bark_map_size = r.read(16)?;
                    let amplitude_bits = r.read(6)?;
                    let amplitude_offset = r.read(8)?;
                    let nb = r.read(4)? as usize + 1;
                    let mut fb = vec![];
                    for _ in 0..nb {
                        let b = r.read(8)? as usize;
                        if b >= books.len() { return Err(Error::Invalid("Vorbis floor0 book")); }
                        fb.push(b);
                    }
                    if order == 0 || bark_map_size == 0 { return Err(Error::Invalid("Vorbis floor0")); }
                    floors.push(Floor::Zero { order, rate, bark_map_size, amplitude_bits, amplitude_offset, books: fb });
                }
                1 => floors.push(Floor::One(Floor1::parse(r, &books)?)),
                _ => return Err(Error::Invalid("Vorbis floor type")),
            }
        }
        let n = r.read(6)? + 1;
        let mut residues = vec![];
        for _ in 0..n {
            let kind = r.read(16)?;
            if kind > 2 { return Err(Error::Invalid("Vorbis residue type")); }
            let begin = r.read(24)? as usize;
            let end = r.read(24)? as usize;
            let partition_size = r.read(24)? as usize + 1;
            let classifications = r.read(6)? as usize + 1;
            let classbook = r.read(8)? as usize;
            if classbook >= books.len() { return Err(Error::Invalid("Vorbis residue classbook")); }
            let mut cascade = vec![];
            for _ in 0..classifications {
                let low = r.read(3)?;
                let high = if r.bit()? { r.read(5)? } else { 0 };
                cascade.push(high * 8 + low);
            }
            let mut rb = vec![];
            for c in cascade {
                let mut b = [UNUSED; 8];
                for (j, slot) in b.iter_mut().enumerate() {
                    if c & (1 << j) != 0 {
                        let bk = r.read(8)? as usize;
                        if bk >= books.len() || !books[bk].has_vq { return Err(Error::Invalid("Vorbis residue book")); }
                        *slot = bk as i32;
                    }
                }
                rb.push(b);
            }
            residues.push(Residue { kind, begin, end, partition_size, classifications, classbook, books: rb });
        }
        let n = r.read(6)? + 1;
        let mut mappings = vec![];
        for _ in 0..n {
            if r.read(16)? != 0 { return Err(Error::Invalid("Vorbis mapping type")); }
            let submaps = if r.bit()? { r.read(4)? as usize + 1 } else { 1 };
            let mut coupling = vec![];
            if r.bit()? {
                let steps = r.read(8)? + 1;
                let bits = ilog(channels as u32 - 1);
                for _ in 0..steps {
                    let m = r.read(bits)? as usize;
                    let a = r.read(bits)? as usize;
                    if m == a || m >= channels || a >= channels { return Err(Error::Invalid("Vorbis coupling")); }
                    coupling.push((m, a));
                }
            }
            if r.read(2)? != 0 { return Err(Error::Invalid("Vorbis mapping reserved")); }
            let mut mux = vec![0usize; channels];
            if submaps > 1 {
                for m in mux.iter_mut() {
                    *m = r.read(4)? as usize;
                    if *m >= submaps { return Err(Error::Invalid("Vorbis mapping mux")); }
                }
            }
            let mut sf = vec![];
            let mut sr = vec![];
            for _ in 0..submaps {
                r.read(8)?;
                let f = r.read(8)? as usize;
                let rr = r.read(8)? as usize;
                if f >= floors.len() || rr >= residues.len() { return Err(Error::Invalid("Vorbis submap")); }
                sf.push(f);
                sr.push(rr);
            }
            mappings.push(Mapping { coupling, mux, submap_floor: sf, submap_residue: sr });
        }
        let n = r.read(6)? + 1;
        let mut modes = vec![];
        for _ in 0..n {
            let blockflag = r.bit()?;
            let wt = r.read(16)?;
            let tt = r.read(16)?;
            let mapping = r.read(8)? as usize;
            if wt != 0 || tt != 0 || mapping >= mappings.len() { return Err(Error::Invalid("Vorbis mode")); }
            modes.push(Mode { blockflag, mapping });
        }
        if !r.bit()? { return Err(Error::Invalid("Vorbis setup framing bit")); }
        Ok(Setup { channels, rate, blocksize, books, floors, residues, mappings, modes })
    }
}

impl Floor1 {
    fn parse(r: &mut LsbReader, books: &[Codebook]) -> Result<Floor1> {
        let partitions = r.read(5)? as usize;
        let mut partition_class = vec![];
        let mut max_class: i32 = -1;
        for _ in 0..partitions {
            let c = r.read(4)? as usize;
            max_class = max_class.max(c as i32);
            partition_class.push(c);
        }
        let nc = (max_class + 1) as usize;
        let mut class_dims = vec![0usize; nc];
        let mut class_subclasses = vec![0u32; nc];
        let mut class_masterbook = vec![0usize; nc];
        let mut subclass_books = vec![vec![]; nc];
        for c in 0..nc {
            class_dims[c] = r.read(3)? as usize + 1;
            class_subclasses[c] = r.read(2)?;
            if class_subclasses[c] != 0 {
                class_masterbook[c] = r.read(8)? as usize;
                if class_masterbook[c] >= books.len() { return Err(Error::Invalid("Vorbis floor1 masterbook")); }
            }
            for _ in 0..(1 << class_subclasses[c]) {
                let b = r.read(8)? as i32 - 1;
                if b >= books.len() as i32 { return Err(Error::Invalid("Vorbis floor1 subclass book")); }
                subclass_books[c].push(b);
            }
        }
        let multiplier = r.read(2)? as i32 + 1;
        let rangebits = r.read(4)?;
        let mut x = vec![0u32, 1 << rangebits];
        for &c in partition_class.iter() {
            for _ in 0..class_dims[c] { x.push(r.read(rangebits)?); }
        }
        if x.len() > 65 { return Err(Error::Invalid("Vorbis floor1 too many points")); }
        let mut sorted: Vec<usize> = (0..x.len()).collect();
        sorted.sort_by_key(|&i| x[i]);
        for w in sorted.windows(2) { if x[w[0]] == x[w[1]] { return Err(Error::Invalid("Vorbis floor1 duplicate x")); } }
        let mut low = vec![0usize; x.len()];
        let mut high = vec![0usize; x.len()];
        for i in 2..x.len() {
            let (mut lo, mut hi) = (0usize, 1usize);
            let (mut lx, mut hx) = (-1i64, i64::MAX);
            for j in 0..i {
                let xj = x[j] as i64;
                if xj < x[i] as i64 && xj > lx { lx = xj; lo = j; }
                if xj > x[i] as i64 && xj < hx { hx = xj; hi = j; }
            }
            low[i] = lo;
            high[i] = hi;
        }
        Ok(Floor1 { partition_class, class_dims, class_subclasses, class_masterbook, subclass_books, multiplier, x, sorted, low, high })
    }

    /// §7.2.3 packet decode; None = "unused" (zero channel).
    fn decode(&self, r: &mut LsbReader, books: &[Codebook]) -> Result<Option<Vec<i32>>> {
        if !r.bit()? { return Ok(None); }
        let range = [256, 128, 86, 64][(self.multiplier - 1) as usize];
        let bits = ilog(range as u32 - 1);
        let mut y = vec![0i32; self.x.len()];
        y[0] = r.read(bits)? as i32;
        y[1] = r.read(bits)? as i32;
        let mut offset = 2;
        for &class in self.partition_class.iter() {
            let cdim = self.class_dims[class];
            let cbits = self.class_subclasses[class];
            let csub = (1u32 << cbits) - 1;
            let mut cval = 0u32;
            if cbits > 0 { cval = books[self.class_masterbook[class]].decode(r)? as u32; }
            for j in 0..cdim {
                let book = self.subclass_books[class][(cval & csub) as usize];
                cval >>= cbits;
                y[offset + j] = if book >= 0 { books[book as usize].decode(r)? as i32 } else { 0 };
            }
            offset += cdim;
        }
        Ok(Some(y))
    }

    /// §7.2.4 curve computation into `out` (n2 values), multiplied element-wise.
    fn synth(&self, y: &[i32], n2: usize, out: &mut [f32]) {
        let range = [256, 128, 86, 64][(self.multiplier - 1) as usize];
        let n = self.x.len();
        let mut fy = vec![0i32; n];
        let mut step2 = vec![false; n];
        fy[0] = y[0];
        fy[1] = y[1];
        step2[0] = true;
        step2[1] = true;
        for i in 2..n {
            let (lo, hi) = (self.low[i], self.high[i]);
            let predicted = render_point(self.x[lo] as i32, fy[lo], self.x[hi] as i32, fy[hi], self.x[i] as i32);
            let val = y[i];
            let highroom = range - predicted;
            let lowroom = predicted;
            let room = if highroom < lowroom { highroom * 2 } else { lowroom * 2 };
            if val != 0 {
                step2[lo] = true;
                step2[hi] = true;
                step2[i] = true;
                fy[i] = if val >= room {
                    if highroom > lowroom { val - lowroom + predicted } else { predicted - val + highroom - 1 }
                } else if val & 1 != 0 {
                    predicted - (val + 1) / 2
                } else {
                    predicted + val / 2
                };
            } else {
                step2[i] = false;
                fy[i] = predicted;
            }
        }
        let mut floor = vec![0i32; n2];
        let mut hx = 0i32;
        let mut hy = 0i32;
        let mut lx = 0i32;
        let mut ly = fy[self.sorted[0]] * self.multiplier;
        for &i in self.sorted.iter().skip(1) {
            if step2[i] {
                hy = fy[i] * self.multiplier;
                hx = self.x[i] as i32;
                render_line(lx, ly, hx, hy, &mut floor);
                lx = hx;
                ly = hy;
            }
        }
        if (hx as usize) < n2 { render_line(hx, hy, n2 as i32, hy, &mut floor); }
        for i in 0..n2 { out[i] *= FLOOR1_INVERSE_DB[floor[i].clamp(0, 255) as usize]; }
    }
}

fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let dy = y1 - y0;
    let adx = x1 - x0;
    let err = dy.abs() * (x - x0);
    let off = err / adx;
    if dy < 0 { y0 - off } else { y0 + off }
}

fn render_line(x0: i32, y0: i32, x1: i32, y1: i32, v: &mut [i32]) {
    let n = v.len() as i32;
    let dy = y1 - y0;
    let adx = x1 - x0;
    if adx <= 0 { return; }
    let base = dy / adx;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    let ady = dy.abs() - base.abs() * adx;
    let mut y = y0;
    let mut err = 0;
    if x0 < n && x0 >= 0 { v[x0 as usize] = y; }
    let mut x = x0 + 1;
    while x < x1 {
        err += ady;
        if err >= adx { err -= adx; y += sy; } else { y += base; }
        if x < n { v[x as usize] = y; }
        x += 1;
    }
}

fn floor0_synth(order: usize, rate: u32, bark_map_size: u32, amplitude_bits: u32, amplitude_offset: u32, amplitude: u32, coeff: &[f32], n2: usize, out: &mut [f32]) {
    let bark = |x: f64| 13.1 * math::atan(0.00074 * x) + 2.24 * math::atan(0.0000000185 * x * x) + 0.0001 * x;
    let mut map = vec![0i64; n2 + 1];
    let bnyq = bark(0.5 * rate as f64);
    for (i, m) in map.iter_mut().enumerate().take(n2) {
        let v = math::floor(bark(rate as f64 * i as f64 / (2.0 * n2 as f64)) * bark_map_size as f64 / bnyq) as i64;
        *m = v.min(bark_map_size as i64 - 1);
    }
    map[n2] = -1;
    let cos_c: Vec<f64> = coeff.iter().map(|&c| math::cos(c as f64)).collect();
    let mut i = 0usize;
    while i < n2 {
        let w = core::f64::consts::PI * map[i] as f64 / bark_map_size as f64;
        let cw = math::cos(w);
        let (mut p, mut q);
        if order & 1 == 1 {
            p = 1.0 - cw * cw;
            q = 0.25;
            for j in 0..(order - 1) / 2 { let t = cos_c[2 * j + 1] - cw; p *= 4.0 * t * t; }
            for j in 0..(order + 1) / 2 { let t = cos_c[2 * j] - cw; q *= 4.0 * t * t; }
        } else {
            p = (1.0 - cw) / 2.0;
            q = (1.0 + cw) / 2.0;
            for j in 0..order / 2 {
                let t = cos_c[2 * j + 1] - cw; p *= 4.0 * t * t;
                let t = cos_c[2 * j] - cw; q *= 4.0 * t * t;
            }
        }
        let lin = math::exp(0.11512925 * (amplitude as f64 * amplitude_offset as f64 / (((1u64 << amplitude_bits) - 1) as f64 * math::sqrt(p + q)) - amplitude_offset as f64));
        let cur = map[i];
        while i < n2 && map[i] == cur {
            out[i] *= lin as f32;
            i += 1;
        }
    }
}

// ---------------------------------------------------------------- IMDCT

struct Imdct {
    n: usize,
    /// pre-twiddles exp(-iπ(k+1/4)/(n/2)) and post-twiddles exp(-iπk/(n/2)), k < n/4
    tw: Vec<(f32, f32)>,
    tw_post: Vec<(f32, f32)>,
    /// FFT twiddles for size n/4
    fft_tw: Vec<(f32, f32)>,
    bitrev: Vec<u32>,
    window: [Vec<f32>; 2], // power-sine slope for the short and the long half-overlap (len = bs/2 each)
}

impl Imdct {
    fn new(n: usize, blocksize: [usize; 2]) -> Imdct {
        let n2 = n / 2;
        let n4 = n / 4;
        let pi = core::f64::consts::PI;
        let tw = (0..n4).map(|k| { let a = -pi * (k as f64 + 0.25) / n2 as f64; (math::cos(a) as f32, math::sin(a) as f32) }).collect();
        let tw_post = (0..n4).map(|k| { let a = -pi * k as f64 / n2 as f64; (math::cos(a) as f32, math::sin(a) as f32) }).collect();
        let fft_tw = (0..n4 / 2).map(|k| { let a = -2.0 * pi * k as f64 / n4 as f64; (math::cos(a) as f32, math::sin(a) as f32) }).collect();
        let bits = n4.trailing_zeros();
        let bitrev = (0..n4 as u32).map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (32 - bits) }).collect();
        let slope = |len: usize| -> Vec<f32> {
            (0..len).map(|i| {
                let x = (i as f64 + 0.5) / len as f64 * pi / 2.0;
                let s = math::sin(x);
                math::sin(pi / 2.0 * s * s) as f32
            }).collect()
        };
        Imdct { n, tw, tw_post, fft_tw, bitrev, window: [slope(blocksize[0] / 2), slope(blocksize[1] / 2)] }
    }

    fn fft(&self, re: &mut [f32], im: &mut [f32]) {
        let m = re.len();
        for i in 0..m {
            let j = self.bitrev[i] as usize;
            if j > i { re.swap(i, j); im.swap(i, j); }
        }
        let mut len = 2;
        while len <= m {
            let step = m / len;
            for s in (0..m).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = self.fft_tw[k * step];
                    let (a, b) = (s + k, s + k + len / 2);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }

    /// The spec's inverse MDCT: y[n] = Σ_k X[k] cos(2π/N (n + 1/2 + N/4)(k + 1/2)), via a DCT-IV of size
    /// N/2 computed with an N/4-point complex FFT, then unfolded by the DCT-IV symmetries.
    fn inverse(&self, x: &[f32], y: &mut [f32]) {
        let n = self.n;
        let n2 = n / 2;
        let n4 = n / 4;
        let mut re = vec![0f32; n4];
        let mut im = vec![0f32; n4];
        for k in 0..n4 {
            let (a, b) = (x[2 * k], x[n2 - 1 - 2 * k]);
            let (c, s) = self.tw[k];
            re[k] = a * c - b * s;
            im[k] = a * s + b * c;
        }
        self.fft(&mut re, &mut im);
        let mut v = vec![0f32; n2];
        for k in 0..n4 {
            let (c, s) = self.tw_post[k];
            let r = re[k] * c - im[k] * s;
            let i = re[k] * s + im[k] * c;
            v[2 * k] = r;
            v[n2 - 1 - 2 * k] = -i;
        }
        for (i, out) in y.iter_mut().enumerate().take(n) {
            *out = if i < n4 { v[i + n4] } else if i < 3 * n4 { -v[3 * n4 - 1 - i] } else { -v[i - 3 * n4] };
        }
    }
}

// ---------------------------------------------------------------- the decoder

pub struct VorbisDecoder {
    pub setup: Setup,
    imdct: [Imdct; 2],
    /// Previous block's windowed IMDCT output, its right half (prev_n/2 samples from its centre).
    prev: Vec<Vec<f32>>,
    prev_n: usize,
    have_prev: bool,
}

impl VorbisDecoder {
    pub fn new(setup: Setup) -> VorbisDecoder {
        let b = setup.blocksize;
        let ch = setup.channels;
        VorbisDecoder { imdct: [Imdct::new(b[0], b), Imdct::new(b[1], b)], prev: vec![vec![]; ch], prev_n: 0, have_prev: false, setup }
    }

    pub fn reset(&mut self) { self.have_prev = false; }

    /// Decode one audio packet; returns the finished samples (planar), possibly zero frames.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<Vec<f32>>) -> Result<usize> {
        let s = &self.setup;
        let ch = s.channels;
        let mut r = LsbReader::new(packet);
        if r.read(1)? != 0 { return Err(Error::Invalid("Vorbis: not an audio packet")); }
        let mode_bits = ilog(s.modes.len() as u32 - 1);
        let mode_n = r.read(mode_bits)? as usize;
        if mode_n >= s.modes.len() { return Err(Error::Invalid("Vorbis mode number")); }
        let mode = &s.modes[mode_n];
        let bf = mode.blockflag as usize;
        let n = s.blocksize[bf];
        let (prev_flag, next_flag) = if mode.blockflag { (r.bit().unwrap_or(false), r.bit().unwrap_or(false)) } else { (false, false) };
        let mapping = &s.mappings[mode.mapping];
        let n2 = n / 2;
        let mut spec = vec![vec![0f32; n2]; ch];
        // floors (an end-of-packet inside a floor makes that channel "unused")
        let mut floor_y: Vec<Option<Vec<i32>>> = vec![None; ch];
        let mut floor0: Vec<Option<(u32, Vec<f32>)>> = vec![None; ch];
        let mut no_residue = vec![false; ch];
        for c in 0..ch {
            let fl = &s.floors[mapping.submap_floor[mapping.mux[c]]];
            match fl {
                Floor::One(f1) => match f1.decode(&mut r, &s.books) {
                    Ok(Some(y)) => floor_y[c] = Some(y),
                    Ok(None) | Err(Error::Eof) => no_residue[c] = true,
                    Err(e) => return Err(e),
                },
                Floor::Zero { order, books, amplitude_bits, .. } => {
                    match decode_floor0(&mut r, &s.books, *order, books, *amplitude_bits) {
                        Ok(Some(v)) => floor0[c] = Some(v),
                        Ok(None) | Err(Error::Eof) => no_residue[c] = true,
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        for &(m, a) in mapping.coupling.iter() {
            if !no_residue[m] || !no_residue[a] { no_residue[m] = false; no_residue[a] = false; }
        }
        // residues, per submap
        for (sm, &ri) in mapping.submap_residue.iter().enumerate() {
            let chans: Vec<usize> = (0..ch).filter(|&c| mapping.mux[c] == sm).collect();
            let res = &s.residues[ri];
            let dnd: Vec<bool> = chans.iter().map(|&c| no_residue[c]).collect();
            let mut vecs: Vec<Vec<f32>> = chans.iter().map(|_| vec![0f32; n2]).collect();
            // an end-of-packet ends the residue decode; what is decoded stays
            match decode_residue(res, &s.books, &mut r, &dnd, &mut vecs, n2) {
                Ok(()) | Err(Error::Eof) => {}
                Err(e) => return Err(e),
            }
            for (k, &c) in chans.iter().enumerate() { spec[c].copy_from_slice(&vecs[k]); }
        }
        // inverse coupling
        for &(m, a) in mapping.coupling.iter().rev() {
            for i in 0..n2 {
                let (mv, av) = (spec[m][i], spec[a][i]);
                let (nm, na) = if mv > 0.0 {
                    if av > 0.0 { (mv, mv - av) } else { (mv + av, mv) }
                } else if av > 0.0 { (mv, mv + av) } else { (mv - av, mv) };
                spec[m][i] = nm;
                spec[a][i] = na;
            }
        }
        // floor × residue
        for c in 0..ch {
            let fl = &s.floors[mapping.submap_floor[mapping.mux[c]]];
            match fl {
                Floor::One(f1) => match &floor_y[c] {
                    Some(y) => f1.synth(y, n2, &mut spec[c]),
                    None => for v in spec[c].iter_mut() { *v = 0.0; },
                },
                Floor::Zero { order, rate, bark_map_size, amplitude_bits, amplitude_offset, .. } => match &floor0[c] {
                    Some((amp, coeff)) => floor0_synth(*order, *rate, *bark_map_size, *amplitude_bits, *amplitude_offset, *amp, coeff, n2, &mut spec[c]),
                    None => for v in spec[c].iter_mut() { *v = 0.0; },
                },
            }
        }
        // IMDCT, window, overlap-add
        let bs0 = s.blocksize[0];
        let (lws, lwe, lwin) = if mode.blockflag && !prev_flag { (n / 4 - bs0 / 4, n / 4 + bs0 / 4, 0) } else { (0, n2, bf) };
        let (rws, rwe, rwin) = if mode.blockflag && !next_flag { (n * 3 / 4 - bs0 / 4, n * 3 / 4 + bs0 / 4, 0) } else { (n2, n, bf) };
        let mut buf = vec![0f32; n];
        let mut frames = 0usize;
        out.resize_with(ch, Vec::new);
        for c in 0..ch {
            self.imdct[bf].inverse(&spec[c], &mut buf);
            let wl = &self.imdct[bf].window[lwin];
            for i in 0..n {
                let w = if i < lws { 0.0 } else if i < lwe { wl[i - lws] } else if i < rws { 1.0 } else if i < rwe { self.imdct[bf].window[rwin][rwe - 1 - i] } else { 0.0 };
                buf[i] *= w;
            }
            out[c].clear();
            if self.have_prev {
                let pn = self.prev_n;
                let len = pn / 4 + n / 4;
                out[c].resize(len, 0.0);
                for k in 0..len {
                    let p = pn / 2 + k; // index in the previous block
                    let mut v = if p < pn { self.prev[c][p - pn / 2] } else { 0.0 };
                    let ci = k as isize + (n / 4) as isize - (pn / 4) as isize;
                    if ci >= 0 { v += buf[ci as usize]; }
                    out[c][k] = v;
                }
                frames = len;
            }
            self.prev[c].clear();
            self.prev[c].extend_from_slice(&buf[n2..]);
        }
        self.prev_n = n;
        self.have_prev = true;
        Ok(frames)
    }
}

fn decode_floor0(r: &mut LsbReader, books: &[Codebook], order: usize, fbooks: &[usize], amplitude_bits: u32) -> Result<Option<(u32, Vec<f32>)>> {
    let amplitude = r.read(amplitude_bits)?;
    if amplitude == 0 { return Ok(None); }
    let bn = r.read(ilog(fbooks.len() as u32))? as usize;
    if bn >= fbooks.len() { return Err(Error::Invalid("Vorbis floor0 book number")); }
    let book = &books[fbooks[bn]];
    if !book.has_vq || book.dims == 0 { return Err(Error::Invalid("Vorbis floor0 book")); }
    let mut coeff = Vec::with_capacity(order + book.dims);
    let mut last = 0f32;
    while coeff.len() < order {
        let e = book.decode(r)?;
        let v = book.vector(e);
        for &x in v { coeff.push(x + last); }
        last = *coeff.last().unwrap();
    }
    coeff.truncate(order);
    Ok(Some((amplitude, coeff)))
}

fn decode_residue(res: &Residue, books: &[Codebook], r: &mut LsbReader, dnd: &[bool], v: &mut [Vec<f32>], n2: usize) -> Result<()> {
    let ch = v.len();
    if res.kind == 2 {
        if dnd.iter().all(|&d| d) { return Ok(()); }
        let mut big = vec![vec![0f32; n2 * ch]];
        let r2 = decode_residue_inner(res, books, r, &[false], &mut big, n2 * ch, 2);
        for i in 0..n2 { for c in 0..ch { v[c][i] = big[0][i * ch + c]; } }
        return r2;
    }
    decode_residue_inner(res, books, r, dnd, v, n2, res.kind)
}

fn decode_residue_inner(res: &Residue, books: &[Codebook], r: &mut LsbReader, dnd: &[bool], v: &mut [Vec<f32>], size: usize, kind: u32) -> Result<()> {
    let begin = res.begin.min(size);
    let end = res.end.min(size);
    if end <= begin { return Ok(()); }
    let n_to_read = end - begin;
    let ps = res.partition_size;
    let partitions = n_to_read / ps;
    if partitions == 0 { return Ok(()); }
    let cb = &books[res.classbook];
    let cpc = cb.dims.max(1);
    let ch = v.len();
    let mut classes = vec![vec![0usize; partitions + cpc]; ch];
    for pass in 0..8 {
        let mut pc = 0usize;
        while pc < partitions {
            if pass == 0 {
                for j in 0..ch {
                    if dnd[j] { continue; }
                    let mut temp = cb.decode(r)?;
                    for i in (0..cpc).rev() {
                        classes[j][i + pc] = temp % res.classifications;
                        temp /= res.classifications;
                    }
                }
            }
            let mut i = 0;
            while i < cpc && pc < partitions {
                for j in 0..ch {
                    if dnd[j] { continue; }
                    let vqclass = classes[j][pc];
                    let book = res.books[vqclass][pass];
                    if book == UNUSED { continue; }
                    let bk = &books[book as usize];
                    let off = begin + pc * ps;
                    let dim = bk.dims;
                    if kind == 0 {
                        let step = ps / dim;
                        for k in 0..step {
                            let e = bk.decode(r)?;
                            let vec = bk.vector(e);
                            for (d, &x) in vec.iter().enumerate() { v[j][off + k + d * step] += x; }
                        }
                    } else {
                        let mut k = 0;
                        while k < ps {
                            let e = bk.decode(r)?;
                            for &x in bk.vector(e) {
                                if k >= ps { break; }
                                v[j][off + k] += x;
                                k += 1;
                            }
                        }
                    }
                }
                i += 1;
                pc += 1;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- Ogg Vorbis

pub struct OggVorbis {
    r: OggReader,
    dec: VorbisDecoder,
    /// Samples (per channel) decoded so far, before trimming.
    decoded: u64,
    granule_base: Option<i64>,
    skip_start: u64,
    held: VecDeque<Vec<Vec<f32>>>,
    done: bool,
    out: Vec<Vec<f32>>,
}

impl OggVorbis {
    pub fn new(mut r: OggReader, ident: &[u8]) -> Result<OggVorbis> {
        let comment = r.next_packet()?.ok_or(Error::Eof)?;
        if comment.data.len() < 7 || comment.data[0] != 3 || &comment.data[1..7] != b"vorbis" { return Err(Error::Invalid("Vorbis comment header")); }
        let setup = r.next_packet()?.ok_or(Error::Eof)?;
        let s = Setup::parse(ident, &setup.data)?;
        Ok(OggVorbis { r, dec: VorbisDecoder::new(s), decoded: 0, granule_base: None, skip_start: 0, held: VecDeque::new(), done: false, out: vec![] })
    }

    fn emit(&mut self, mut planes: Vec<Vec<f32>>, pcm: &mut Pcm) -> bool {
        // Vorbis I §4.3.9 orders 3–8 channels L,C,R,…; the API hands out the WAVE/SMPTE order every other
        // format here uses (L,R,C,LFE,…), the order FFmpeg and Chromium present too.
        if (3..=8).contains(&planes.len()) {
            const ORDER: [&[usize]; 6] = [&[0, 2, 1], &[0, 1, 2, 3], &[0, 2, 1, 3, 4], &[0, 2, 1, 5, 3, 4], &[0, 2, 1, 6, 5, 3, 4], &[0, 2, 1, 7, 5, 6, 3, 4]];
            let map = ORDER[planes.len() - 3];
            let mut src: Vec<Option<Vec<f32>>> = planes.into_iter().map(Some).collect();
            planes = map.iter().map(|&i| src[i].take().unwrap_or_default()).collect();
        }
        if self.skip_start > 0 {
            let n = planes[0].len();
            let s = (self.skip_start as usize).min(n);
            for p in planes.iter_mut() { p.drain(..s); }
            self.skip_start -= s as u64;
        }
        let frames = planes[0].len();
        if frames == 0 { return false; }
        pcm.set_float(planes.len(), frames);
        for (c, p) in planes.into_iter().enumerate() { pcm.flt[c] = p; }
        true
    }

    fn handle(&mut self, p: &Packet) -> Result<Option<Vec<Vec<f32>>>> {
        let mut out = core::mem::take(&mut self.out);
        let n = match self.dec.decode(&p.data, &mut out) {
            Ok(n) => n,
            Err(_) => 0, // a corrupt audio packet decodes to nothing (and the next overlaps from it)
        };
        let mut planes: Vec<Vec<f32>> = out.iter().map(|c| c[..n.min(c.len())].to_vec()).collect();
        self.out = out;
        let before = self.decoded;
        self.decoded += n as u64;
        if let (None, Some(g)) = (self.granule_base, p.granule) {
            // §A.2: the first page's granule gives the position of the last sample it completes; less than
            // the samples decoded means the stream starts with samples to discard
            // (when that first page is also the last, the spec cuts the end, not the beginning)
            let base = g as i64 - self.decoded as i64;
            if base < 0 && !p.eos { self.skip_start = (-base) as u64; }
            self.granule_base = Some(base.max(0));
        }
        if p.eos {
            if let Some(g) = p.granule {
                let limit = g as i64 - self.granule_base.unwrap_or(0);
                if (limit as u64) < self.decoded && limit >= 0 {
                    let keep = (limit as u64).saturating_sub(before) as usize;
                    for pl in planes.iter_mut() { pl.truncate(keep.min(pl.len())); }
                }
            }
        }
        Ok(if n > 0 { Some(planes) } else { None })
    }
}

impl Source for OggVorbis {
    fn info(&self) -> Info {
        let s = &self.dec.setup;
        Info { rate: s.rate, channels: s.channels as u16, bits: 0, frames: None, format: Format::Ogg, codec: Codec::Vorbis, float: true }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        loop {
            if self.granule_base.is_some() || self.done {
                while let Some(pl) = self.held.pop_front() {
                    if self.emit(pl, pcm) { return Ok(true); }
                }
            }
            if self.done { return Ok(false); }
            let Some(p) = self.r.next_packet()? else {
                self.done = true;
                continue;
            };
            if !p.data.is_empty() && p.data[0] & 1 == 1 { continue; } // a header packet inside the stream
            if let Some(pl) = self.handle(&p)? { self.held.push_back(pl); }
            if p.eos { self.done = true; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imdct_matches_the_definition() {
        for &n in &[16usize, 64, 256, 2048] {
            let m = Imdct::new(n, [n.min(64), n]);
            let x: Vec<f32> = (0..n / 2).map(|k| ((k * 7919 % 113) as f32 - 56.0) / 57.0).collect();
            let mut y = vec![0f32; n];
            m.inverse(&x, &mut y);
            let mut maxe = 0f64;
            for i in 0..n {
                let mut s = 0f64;
                for k in 0..n / 2 {
                    s += x[k] as f64 * math::cos(2.0 * core::f64::consts::PI / n as f64 * (i as f64 + 0.5 + n as f64 / 4.0) * (k as f64 + 0.5));
                }
                maxe = maxe.max((s - y[i] as f64).abs());
            }
            assert!(maxe < 1e-3 * (n as f64).sqrt(), "n={} max err {}", n, maxe);
        }
    }
    #[test]
    fn float32_unpack_known() {
        assert_eq!(float32_unpack(0), 0.0);
        // mantissa 1, exponent 788 -> 1.0
        assert_eq!(float32_unpack((788u32 << 21) | 1), 1.0);
        assert_eq!(float32_unpack(0x8000_0000 | (789u32 << 21) | 3), -6.0);
        assert_eq!(lookup1_values(81, 4), 3);
        assert_eq!(lookup1_values(80, 4), 2);
        assert_eq!(lookup1_values(1, 1), 1);
    }
}
