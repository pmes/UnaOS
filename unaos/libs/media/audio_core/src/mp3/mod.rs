//! MPEG-1/2/2.5 Audio Layer III (ISO/IEC 11172-3, ISO/IEC 13818-3 LSF, and the 2.5 extension):
//! frame sync (incl. free format), side information, the bit reservoir, scalefactors (scfsi; LSF partitions
//! and the intensity-stereo table), Huffman big-values/count1 decoding, requantisation, short-block
//! reordering, mid/side and intensity stereo (MPEG-1 tan table, LSF power table), alias reduction, the
//! 36/12-point IMDCT with the four window shapes and overlap, frequency inversion and the 32-band polyphase
//! synthesis filterbank. Containers: ID3v2 tags skipped, the Xing/Info frame skipped and its LAME tag's
//! encoder delay/padding applied (gapless, as FFmpeg/Chromium do), trailing ID3v1/APE ignored. Float.
pub mod tables;

use crate::bits::BitReader;
use crate::io::ByteStream;
use crate::math;
use crate::{Codec, Error, Format, Info, Pcm, Result, Source};
use alloc::vec;
use alloc::vec::Vec;
use tables::*;

// ---------------------------------------------------------------- header

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u8, // 0 = MPEG-2.5, 2 = MPEG-2, 3 = MPEG-1
    pub layer: u8,   // 1, 2, 3
    pub crc: bool,
    pub bitrate_index: u8,
    pub sr_index: u8, // 0..8 in FFmpeg order: 44.1/48/32 (MPEG-1), 22.05/24/16, 11.025/12/8
    pub padding: bool,
    pub mode: u8,
    pub mode_ext: u8,
}

const BITRATES_L3: [[u32; 15]; 2] = [
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
];
const RATES: [u32; 9] = [44100, 48000, 32000, 22050, 24000, 16000, 11025, 12000, 8000];

impl Header {
    pub fn parse(h: u32) -> Option<Header> {
        if h >> 21 != 0x7FF { return None; }
        let version = ((h >> 19) & 3) as u8;
        let layer = 4 - ((h >> 17) & 3) as u8;
        let bitrate_index = ((h >> 12) & 15) as u8;
        let sri = ((h >> 10) & 3) as u8;
        if version == 1 || layer == 4 || bitrate_index == 15 || sri == 3 { return None; }
        let sr_index = match version { 3 => sri, 2 => 3 + sri, _ => 6 + sri };
        Some(Header {
            version, layer,
            crc: (h >> 16) & 1 == 0,
            bitrate_index, sr_index,
            padding: (h >> 9) & 1 != 0,
            mode: ((h >> 6) & 3) as u8,
            mode_ext: ((h >> 4) & 3) as u8,
        })
    }
    pub fn lsf(&self) -> bool { self.version != 3 }
    pub fn rate(&self) -> u32 { RATES[self.sr_index as usize] }
    pub fn channels(&self) -> usize { if self.mode == 3 { 1 } else { 2 } }
    pub fn samples(&self) -> usize { if self.layer == 3 && self.lsf() { 576 } else if self.layer == 1 { 384 } else { 1152 } }
    /// Frame length in bytes (0 = free format).
    pub fn frame_len(&self) -> usize {
        if self.bitrate_index == 0 || self.layer != 3 { return 0; }
        let br = BITRATES_L3[self.lsf() as usize][self.bitrate_index as usize] * 1000;
        let k = if self.lsf() { 72 } else { 144 };
        (k * br / self.rate()) as usize + self.padding as usize
    }
    pub fn side_len(&self) -> usize {
        match (self.lsf(), self.channels()) { (false, 1) => 17, (false, _) => 32, (true, 1) => 9, (true, _) => 17 }
    }
    fn compatible(&self, o: &Header) -> bool { self.version == o.version && self.layer == o.layer && self.sr_index == o.sr_index }
}

// ---------------------------------------------------------------- Huffman

struct Huff {
    /// 8-bit lookahead: (sym << 8 | len) for len ≤ 8, else 0xFFFF_FFFF (walk the tree from the root)
    table: Vec<u32>,
    tree: Vec<[i32; 2]>,
}

impl Huff {
    /// Codewords assigned in list order from the lengths (each next code is the previous plus one, aligned).
    fn from_lengths(lens: &[u8], syms: &[u16]) -> Huff {
        let mut code: u64 = 0;
        let mut tree = vec![[i32::MIN, i32::MIN]];
        let mut table = vec![0xFFFF_FFFFu32; 256];
        for (i, &l) in lens.iter().enumerate() {
            let l = l as u32;
            let w = (code >> (32 - l)) as u32;
            code += 1u64 << (32 - l);
            let mut node = 0usize;
            for b in (0..l).rev() {
                let bit = ((w >> b) & 1) as usize;
                if b == 0 { tree[node][bit] = -(syms[i] as i32) - 1; } else {
                    let c = tree[node][bit];
                    if c >= 0 { node = c as usize; } else {
                        tree.push([i32::MIN, i32::MIN]);
                        let n = tree.len() - 1;
                        tree[node][bit] = n as i32;
                        node = n;
                    }
                }
            }
            if l <= 8 {
                let base = (w << (8 - l)) as usize;
                for t in table[base..base + (1 << (8 - l))].iter_mut() { *t = ((syms[i] as u32) << 8) | l; }
            }
        }
        Huff { table, tree }
    }
    #[inline]
    fn decode(&self, r: &mut BitReader) -> Result<u16> {
        let t = self.table[r.peek(8) as usize];
        if t != 0xFFFF_FFFF {
            r.skip((t & 0xFF) as usize)?;
            return Ok((t >> 8) as u16);
        }
        let mut node = 0usize;
        loop {
            let c = self.tree[node][r.read(1)? as usize];
            if c == i32::MIN { return Err(Error::Invalid("MP3 invalid Huffman code")); }
            if c < 0 { return Ok((-(c + 1)) as u16); }
            node = c as usize;
        }
    }
}

struct Tables {
    big: Vec<Huff>, // index 1..=15 (FFmpeg numbering), 0 unused
    quad: [Huff; 2],
    pow43: Vec<f32>,
    is_mpeg1: [[f32; 16]; 2],
    imdct_win: [[f32; 36]; 4],
    cos36: Vec<f32>, // [i][k] 36 x 18
    cos12: Vec<f32>, // 12 x 6
    synth_n: Vec<f32>, // 64 x 32
    window_d: [f32; 512],
    cs: [f32; 8],
    ca: [f32; 8],
    band_long: [[u16; 23]; 9],
}

impl Tables {
    fn new() -> Tables {
        let mut big = vec![Huff { table: vec![], tree: vec![] }];
        let mut off = 0usize;
        for i in 0..15 {
            let n = HUFF_SIZES_MINUS_ONE[i] as usize + 1;
            let syms: Vec<u16> = HUFF_SYMS[off..off + n].iter().map(|&s| s as u16).collect();
            big.push(Huff::from_lengths(&HUFF_LENS[off..off + n], &syms));
            off += n;
        }
        // quad tables are given as (code, bits) per symbol: sort by code to list codeword order
        let quad = [0usize, 1].map(|t| {
            let mut v: Vec<(u32, u8, u16)> = (0..16).map(|s| {
                let bits = QUAD_BITS[t * 16 + s];
                let code = (QUAD_CODES[t * 16 + s] as u32) << (32 - bits as u32);
                (code, bits, s as u16)
            }).collect();
            v.sort_by_key(|x| x.0);
            let lens: Vec<u8> = v.iter().map(|x| x.1).collect();
            let syms: Vec<u16> = v.iter().map(|x| x.2).collect();
            Huff::from_lengths(&lens, &syms)
        });
        let pow43 = (0..8207).map(|i| math::pow(i as f64, 4.0 / 3.0) as f32).collect();
        let pi = core::f64::consts::PI;
        let mut is_mpeg1 = [[0f32; 16]; 2];
        for i in 0..7 {
            let v = if i != 6 { let f = math::sin(i as f64 * pi / 12.0) / math::cos(i as f64 * pi / 12.0); f / (1.0 + f) } else { 1.0 };
            is_mpeg1[0][i] = v as f32;
            is_mpeg1[1][6 - i] = v as f32;
        }
        let mut imdct_win = [[0f32; 36]; 4];
        for i in 0..36 {
            let s36 = math::sin(pi / 36.0 * (i as f64 + 0.5));
            imdct_win[0][i] = s36 as f32;
            imdct_win[1][i] = if i < 18 { s36 } else if i < 24 { 1.0 } else if i < 30 { math::sin(pi / 12.0 * (i as f64 - 18.0 + 0.5)) } else { 0.0 } as f32;
            imdct_win[3][i] = if i < 6 { 0.0 } else if i < 12 { math::sin(pi / 12.0 * (i as f64 - 6.0 + 0.5)) } else if i < 18 { 1.0 } else { s36 } as f32;
            imdct_win[2][i] = if i < 12 { math::sin(pi / 12.0 * (i as f64 + 0.5)) as f32 } else { 0.0 };
        }
        let mut cos36 = vec![0f32; 36 * 18];
        for i in 0..36 { for k in 0..18 { cos36[i * 18 + k] = math::cos(pi / 72.0 * (2 * i + 1 + 18) as f64 * (2 * k + 1) as f64) as f32; } }
        let mut cos12 = vec![0f32; 12 * 6];
        for i in 0..12 { for k in 0..6 { cos12[i * 6 + k] = math::cos(pi / 24.0 * (2 * i + 1 + 6) as f64 * (2 * k + 1) as f64) as f32; } }
        let mut synth_n = vec![0f32; 64 * 32];
        for i in 0..64 { for k in 0..32 { synth_n[i * 32 + k] = math::cos((16 + i) as f64 * (2 * k + 1) as f64 * pi / 64.0) as f32; } }
        let mut window_d = [0f32; 512];
        for i in 0..257 {
            let v = ENWINDOW[i] as f32 / 65536.0;
            window_d[i] = v;
            if i != 0 { window_d[512 - i] = if i & 63 != 0 { -v } else { v }; }
        }
        let ci = [-0.6f64, -0.535, -0.33, -0.185, -0.095, -0.041, -0.0142, -0.0037];
        let mut cs = [0f32; 8];
        let mut ca = [0f32; 8];
        for i in 0..8 {
            let d = math::sqrt(1.0 + ci[i] * ci[i]);
            cs[i] = (1.0 / d) as f32;
            ca[i] = (ci[i] / d) as f32;
        }
        let mut band_long = [[0u16; 23]; 9];
        for s in 0..9 {
            let mut k = 0u16;
            for j in 0..22 { band_long[s][j] = k; k += BAND_SIZE_LONG[s * 22 + j] as u16; }
            band_long[s][22] = k;
        }
        Tables { big, quad, pow43, is_mpeg1, imdct_win, cos36, cos12, synth_n, window_d, cs, ca, band_long }
    }
}

// ---------------------------------------------------------------- side info

#[derive(Clone, Copy)]
struct Granule {
    part2_3_length: usize,
    big_values: usize,
    global_gain: i32,
    scalefac_compress: u32,
    block_type: u8,
    switch_point: bool,
    table_select: [u8; 3],
    subblock_gain: [i32; 3],
    region_size: [usize; 3],
    preflag: bool,
    scalefac_scale: bool,
    count1table_select: bool,
    long_end: usize,
    short_start: usize,
    scale_factors: [u8; 40],
    /// LSF intensity stereo: the illegal position (2^slen - 1) of each scalefactor (ISO 13818-3 §2.4.3.2)
    is_illegal: [u8; 40],
}

impl Default for Granule {
    fn default() -> Self {
        Granule { part2_3_length: 0, big_values: 0, global_gain: 0, scalefac_compress: 0, block_type: 0, switch_point: false, table_select: [0; 3],
            subblock_gain: [0; 3], region_size: [0; 3], preflag: false, scalefac_scale: false, count1table_select: false, long_end: 22, short_start: 13, scale_factors: [0; 40], is_illegal: [7; 40] }
    }
}

struct SideInfo {
    main_data_begin: usize,
    scfsi: [[bool; 4]; 2],
    gr: [[Granule; 2]; 2], // [granule][channel]
}

fn parse_side(h: &Header, d: &[u8], band_long: &[[u16; 23]; 9]) -> Result<SideInfo> {
    let mut r = BitReader::new(d);
    let ch = h.channels();
    let lsf = h.lsf();
    let mut si = SideInfo { main_data_begin: 0, scfsi: [[false; 4]; 2], gr: [[Granule::default(); 2]; 2] };
    if lsf {
        si.main_data_begin = r.read(8)? as usize;
        r.skip(if ch == 1 { 1 } else { 2 })?;
    } else {
        si.main_data_begin = r.read(9)? as usize;
        r.skip(if ch == 1 { 5 } else { 3 })?;
        for c in 0..ch { for b in 0..4 { si.scfsi[c][b] = r.bit()?; } }
    }
    let ngr = if lsf { 1 } else { 2 };
    let sri = h.sr_index as usize;
    for g in 0..ngr {
        for c in 0..ch {
            let gr = &mut si.gr[g][c];
            gr.part2_3_length = r.read(12)? as usize;
            gr.big_values = r.read(9)? as usize;
            if gr.big_values > 288 { return Err(Error::Invalid("MP3 big_values > 288")); }
            gr.global_gain = r.read(8)? as i32;
            if (h.mode_ext & 3) == 2 && h.mode == 1 { gr.global_gain -= 2; } // MS-only: fold 1/sqrt(2)
            gr.scalefac_compress = r.read(if lsf { 9 } else { 4 })?;
            if r.bit()? {
                gr.block_type = r.read(2)? as u8;
                if gr.block_type == 0 { return Err(Error::Invalid("MP3 block type 0 with window switching")); }
                gr.switch_point = r.bit()?;
                for i in 0..2 { gr.table_select[i] = r.read(5)? as u8; }
                for i in 0..3 { gr.subblock_gain[i] = r.read(3)? as i32; }
                gr.region_size[0] = if gr.block_type == 2 { if sri != 8 { 18 } else { 36 } } else if sri <= 2 { 18 } else if sri != 8 { 27 } else { 54 };
                gr.region_size[1] = 288;
            } else {
                gr.block_type = 0;
                gr.switch_point = false;
                for i in 0..3 { gr.table_select[i] = r.read(5)? as u8; }
                let ra1 = r.read(4)? as usize;
                let ra2 = r.read(3)? as usize;
                gr.region_size[0] = (band_long[sri][ra1 + 1] / 2) as usize;
                let l = (ra1 + ra2 + 2).min(22);
                gr.region_size[1] = (band_long[sri][l] / 2) as usize;
            }
            gr.region_size[2] = 288;
            let mut j = 0;
            for i in 0..3 {
                let k = gr.region_size[i].min(gr.big_values);
                gr.region_size[i] = k - j;
                j = k;
            }
            if gr.block_type == 2 {
                if gr.switch_point {
                    gr.long_end = if sri <= 2 { 8 } else { 6 };
                    gr.short_start = 3;
                } else {
                    gr.long_end = 0;
                    gr.short_start = 0;
                }
            } else {
                gr.short_start = 13;
                gr.long_end = 22;
            }
            if !lsf { gr.preflag = r.bit()?; }
            gr.scalefac_scale = r.bit()?;
            gr.count1table_select = r.bit()?;
        }
    }
    Ok(si)
}

// ---------------------------------------------------------------- the decoder

pub struct Mp3Decoder {
    t: Tables,
    reservoir: Vec<u8>,
    overlap: [[f32; 576]; 2],
    v: [[f32; 1024]; 2],
    v_off: [usize; 2],
}

fn lsf_sf_expand(sf: u32, n1: u32, n2: u32, n3: u32) -> [u32; 4] {
    let mut sf = sf;
    let mut s = [0u32; 4];
    if n3 != 0 { s[3] = sf % n3; sf /= n3; }
    if n2 != 0 { s[2] = sf % n2; sf /= n2; }
    s[1] = sf % n1;
    sf /= n1;
    s[0] = sf;
    s
}

impl Default for Mp3Decoder { fn default() -> Self { Self::new() } }

impl Mp3Decoder {
    pub fn new() -> Mp3Decoder {
        Mp3Decoder { t: Tables::new(), reservoir: Vec::new(), overlap: [[0.0; 576]; 2], v: [[0.0; 1024]; 2], v_off: [0; 2] }
    }
    pub fn reset(&mut self) {
        self.reservoir.clear();
        self.overlap = [[0.0; 576]; 2];
        self.v = [[0.0; 1024]; 2];
        self.v_off = [0; 2];
    }

    fn read_scalefactors(&self, h: &Header, r: &mut BitReader, g: &mut Granule, prev: Option<&Granule>, scfsi: &[bool; 4], ch: usize) -> Result<()> {
        if !h.lsf() {
            let slen1 = SLEN_TABLE[g.scalefac_compress as usize] as u32;
            let slen2 = SLEN_TABLE[16 + g.scalefac_compress as usize] as u32;
            if g.block_type == 2 {
                let n = if g.switch_point { 17 } else { 18 };
                let mut j = 0;
                for _ in 0..n { g.scale_factors[j] = r.read(slen1)? as u8; j += 1; }
                for _ in 0..18 { g.scale_factors[j] = r.read(slen2)? as u8; j += 1; }
                for _ in 0..3 { g.scale_factors[j] = 0; j += 1; }
            } else {
                const BANDS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
                for (k, &(a, b)) in BANDS.iter().enumerate() {
                    let slen = if k < 2 { slen1 } else { slen2 };
                    if let (Some(p), true) = (prev, scfsi[k]) {
                        for i in a..b { g.scale_factors[i] = p.scale_factors[i]; }
                    } else {
                        for i in a..b { g.scale_factors[i] = r.read(slen)? as u8; }
                    }
                }
                g.scale_factors[21] = 0;
            }
        } else {
            let tindex = if g.block_type == 2 { if g.switch_point { 2 } else { 1 } } else { 0 };
            let mut sf = g.scalefac_compress;
            let (slen, tindex2);
            if (h.mode_ext & 1) != 0 && h.mode == 1 && ch == 1 {
                sf >>= 1;
                if sf < 180 { slen = lsf_sf_expand(sf, 6, 6, 0); tindex2 = 3; }
                else if sf < 244 { slen = lsf_sf_expand(sf - 180, 4, 4, 0); tindex2 = 4; }
                else { slen = lsf_sf_expand(sf - 244, 3, 0, 0); tindex2 = 5; }
            } else if sf < 400 { slen = lsf_sf_expand(sf, 5, 4, 4); tindex2 = 0; }
            else if sf < 500 { slen = lsf_sf_expand(sf - 400, 5, 4, 0); tindex2 = 1; }
            else { slen = lsf_sf_expand(sf - 500, 3, 0, 0); tindex2 = 2; g.preflag = true; }
            let mut j = 0;
            for k in 0..4 {
                let n = LSF_NSF_TABLE[tindex2 * 12 + tindex * 4 + k] as usize;
                for _ in 0..n {
                    if j < 40 {
                        g.scale_factors[j] = r.read(slen[k])? as u8;
                        // a band coded with 0 bits has position 0, which is legal
                        g.is_illegal[j] = if slen[k] == 0 { 255 } else { ((1u32 << slen[k]) - 1) as u8 };
                    }
                    j += 1;
                }
            }
            while j < 40 { g.scale_factors[j] = 0; g.is_illegal[j] = 255; j += 1; }
        }
        Ok(())
    }

    /// Requantisation exponents per line (FFmpeg's `exponents_from_scale_factors`): quarter-steps.
    fn exponents(&self, h: &Header, g: &Granule, e: &mut [i32; 576]) {
        let sri = h.sr_index as usize;
        let gain = g.global_gain - 210;
        let shift = g.scalefac_scale as i32 + 1;
        let pre = if g.preflag { &PRETAB[22..44] } else { &PRETAB[0..22] };
        let mut p = 0usize;
        for i in 0..g.long_end {
            let v0 = gain - ((g.scale_factors[i] as i32 + pre[i] as i32) << shift);
            for _ in 0..BAND_SIZE_LONG[sri * 22 + i] { if p < 576 { e[p] = v0; p += 1; } }
        }
        if g.short_start < 13 {
            let gains = [gain - (g.subblock_gain[0] << 3), gain - (g.subblock_gain[1] << 3), gain - (g.subblock_gain[2] << 3)];
            let mut k = g.long_end;
            for i in g.short_start..13 {
                let len = BAND_SIZE_SHORT[sri * 13 + i];
                for l in 0..3 {
                    let v0 = gains[l] - ((g.scale_factors[k.min(39)] as i32) << shift);
                    k += 1;
                    for _ in 0..len { if p < 576 { e[p] = v0; p += 1; } }
                }
            }
        }
        while p < 576 { e[p] = gain; p += 1; }
    }

    #[inline]
    fn requant(&self, v: u32, e: i32) -> f32 {
        self.t.pow43[v as usize] * exp2q(e)
    }

    fn huffman(&self, r: &mut BitReader, g: &Granule, e: &[i32; 576], end: usize, xr: &mut [f32; 576]) -> Result<()> {
        let mut s = 0usize;
        for i in 0..3 {
            let n = g.region_size[i];
            if n == 0 { continue; }
            let (vlc, linbits) = (HUFF_DATA[g.table_select[i] as usize * 2] as usize, HUFF_DATA[g.table_select[i] as usize * 2 + 1] as u32);
            if vlc == 0 {
                for v in xr[s..s + 2 * n].iter_mut() { *v = 0.0; }
                s += 2 * n;
                continue;
            }
            let huff = &self.t.big[vlc];
            for _ in 0..n {
                if r.bit_pos() >= end { // ran out of the granule's bits: the rest is zero
                    xr[s] = 0.0; xr[s + 1] = 0.0; s += 2; continue;
                }
                let sym = huff.decode(r)?;
                let mut x = (sym >> 4) as u32;
                let mut y = (sym & 15) as u32;
                if linbits > 0 && x == 15 { x += r.read(linbits)?; }
                xr[s] = if x != 0 { let v = self.requant(x.min(8206), e[s]); if r.bit()? { -v } else { v } } else { 0.0 };
                if linbits > 0 && y == 15 { y += r.read(linbits)?; }
                xr[s + 1] = if y != 0 { let v = self.requant(y.min(8206), e[s + 1]); if r.bit()? { -v } else { v } } else { 0.0 };
                s += 2;
            }
        }
        let quad = &self.t.quad[g.count1table_select as usize];
        let mut last_pos = 0usize;
        while s <= 572 {
            let pos = r.bit_pos();
            if pos >= end {
                if pos > end && last_pos != 0 {
                    s -= 4; // the last quadruple overran the granule: drop it
                    r.seek_bits(last_pos);
                }
                break;
            }
            last_pos = pos;
            let code = match quad.decode(r) { Ok(c) => c, Err(_) => break };
            for k in 0..4 {
                let bit = 8 >> k;
                xr[s + k] = if code & bit != 0 {
                    let v = self.requant(1, e[s + k]);
                    match r.bit() { Ok(true) => -v, Ok(false) => v, Err(_) => v }
                } else { 0.0 };
            }
            s += 4;
        }
        for v in xr[s..].iter_mut() { *v = 0.0; }
        r.seek_bits(end);
        Ok(())
    }

    fn stereo(&self, h: &Header, _g0: &Granule, g1: &Granule, x0: &mut [f32; 576], x1: &mut [f32; 576]) {
        let sri = h.sr_index as usize;
        let ms = h.mode == 1 && h.mode_ext & 2 != 0;
        let is = h.mode == 1 && h.mode_ext & 1 != 0;
        let isq = core::f32::consts::FRAC_1_SQRT_2;
        let msb = |a: &mut [f32], b: &mut [f32]| for j in 0..a.len() { let (t0, t1) = (a[j], b[j]); a[j] = (t0 + t1) * isq; b[j] = (t0 - t1) * isq; };
        if is {
            // MPEG-1: tan(is_pos·π/12) ratios, position 7+ illegal. LSF (ISO 13818-3 §2.4.3.2): powers of
            // 2^(-1/4) or 2^(-1/2), illegal at 2^slen - 1 of that band (FFmpeg stops at 16; the ISO
            // reference does not, and l3-test45 needs the full range).
            let lsf = h.lsf();
            let io_shift = (g1.scalefac_compress & 1) as i32 + 1;
            // The top band (21 long, 12 short) carries no position: it takes the band below's when that
            // band is intensity-coded too, else the default (MPEG-1: 3, centre; LSF: 0) — ISO 11172-3
            // §2.4.3.4.9.3, as the reference decoder does (FFmpeg always copies band 20).
            let default_pos: u8 = if lsf { 0 } else { 3 };
            let ratio_pos = |sf: u8, illegal: u8| -> Option<(f32, f32)> {
                if !lsf {
                    if sf >= 7 { None } else { Some((self.t.is_mpeg1[0][sf as usize], self.t.is_mpeg1[1][sf as usize])) }
                } else if sf >= illegal {
                    None
                } else {
                    let f = exp2q(-io_shift * ((sf as i32 + 1) >> 1));
                    Some(if sf & 1 == 1 { (f, 1.0) } else { (1.0, f) })
                }
            };
            let ratio = |k: usize| -> Option<(f32, f32)> {
                let sf = g1.scale_factors[k];
                if !lsf {
                    if sf >= 7 { None } else { Some((self.t.is_mpeg1[0][sf as usize], self.t.is_mpeg1[1][sf as usize])) }
                } else if sf >= g1.is_illegal[k] {
                    None
                } else {
                    let f = exp2q(-io_shift * ((sf as i32 + 1) >> 1));
                    Some(if sf & 1 == 1 { (f, 1.0) } else { (1.0, f) })
                }
            };
            let mut p = 576usize;
            let mut nz_short = [false; 3];
            let mut k = (13 - g1.short_start) as isize * 3 + g1.long_end as isize - 3;
            let mut i = 12isize;
            while i >= g1.short_start as isize {
                if i != 11 { k -= 3; }
                let len = BAND_SIZE_SHORT[sri * 13 + i as usize] as usize;
                for l in (0..3).rev() {
                    p -= len;
                    let mut found = nz_short[l];
                    if !found {
                        if x1[p..p + len].iter().any(|&v| v != 0.0) { nz_short[l] = true; found = true; }
                    }
                    if !found {
                        let kk = (k + l as isize).clamp(0, 39) as usize;
                        let r = if i == 12 {
                            // band 11 of this window: lines p - 3·len11 + ... ; it is IS-coded when its right is zero
                            let len11 = BAND_SIZE_SHORT[sri * 13 + 11] as usize;
                            let b11 = p.saturating_sub(3 * len11);
                            if x1[b11..b11 + len11].iter().any(|&v| v != 0.0) { ratio_pos(default_pos, 255) } else { ratio(kk) }
                        } else { ratio(kk) };
                        match r {
                            None => found = true,
                            Some((v1, v2)) => for j in 0..len { let t = x0[p + j]; x0[p + j] = t * v1; x1[p + j] = t * v2; },
                        }
                    }
                    if found && ms { msb(&mut x0[p..p + len], &mut x1[p..p + len]); }
                }
                i -= 1;
            }
            let mut nz = nz_short[0] | nz_short[1] | nz_short[2];
            for i in (0..g1.long_end).rev() {
                let len = BAND_SIZE_LONG[sri * 22 + i] as usize;
                p -= len;
                let mut found = nz;
                if !found && x1[p..p + len].iter().any(|&v| v != 0.0) { nz = true; found = true; }
                if !found {
                    let r = if i == 21 {
                        let len20 = BAND_SIZE_LONG[sri * 22 + 20] as usize;
                        if x1[p - len20..p].iter().any(|&v| v != 0.0) { ratio_pos(default_pos, 255) } else { ratio(20) }
                    } else { ratio(i) };
                    match r {
                        None => found = true,
                        Some((v1, v2)) => for j in 0..len { let t = x0[p + j]; x0[p + j] = t * v1; x1[p + j] = t * v2; },
                    }
                }
                if found && ms { msb(&mut x0[p..p + len], &mut x1[p..p + len]); }
            }
        } else if ms {
            // the 1/sqrt(2) is already in the global gain
            for j in 0..576 { let (t0, t1) = (x0[j], x1[j]); x0[j] = t0 + t1; x1[j] = t0 - t1; }
        }
    }

    /// Reorder short-block lines from (sfb, window, freq) to (subband, window-interleaved) as the IMDCT
    /// wants: line f of window w lands at 18·(f/6)·… — FFmpeg's `reorder_block` layout (dst[3i+w]).
    fn reorder(&self, h: &Header, g: &Granule, xr: &mut [f32; 576]) {
        if g.block_type != 2 { return; }
        let sri = h.sr_index as usize;
        let mut tmp = [0f32; 576];
        let mut p = if g.switch_point { if sri != 8 { 36 } else { 72 } } else { 0 };
        let start_i = if g.switch_point { 3 } else { 0 };
        for i in start_i..13 {
            let len = BAND_SIZE_SHORT[sri * 13 + i] as usize;
            if p + 3 * len > 576 { break; }
            for j in 0..len { for w in 0..3 { tmp[3 * j + w] = xr[p + w * len + j]; } }
            xr[p..p + 3 * len].copy_from_slice(&tmp[..3 * len]);
            p += 3 * len;
        }
    }

    fn antialias(&self, g: &Granule, xr: &mut [f32; 576]) {
        let n = if g.block_type == 2 { if g.switch_point { 1 } else { return } } else { 31 };
        for sb in 0..n {
            for i in 0..8 {
                let a = 18 * sb + 17 - i;
                let b = 18 * (sb + 1) + i;
                let (bu, bd) = (xr[a], xr[b]);
                xr[a] = bu * self.t.cs[i] - bd * self.t.ca[i];
                xr[b] = bd * self.t.cs[i] + bu * self.t.ca[i];
            }
        }
    }

    /// IMDCT + windowing + overlap-add + frequency inversion: xr (576) → 18 time samples × 32 subbands.
    fn imdct(&mut self, g: &Granule, ch: usize, xr: &[f32; 576], out: &mut [[f32; 32]; 18]) {
        for sb in 0..32 {
            let mut z = [0f32; 36];
            let long = g.block_type != 2 || (g.switch_point && sb < 2);
            if long {
                // ISO 11172-3 §2.4.2.7: with mixed_block_flag the two lowest subbands use the normal window,
                // whatever the block type
                let bt = if g.block_type == 2 || (g.switch_point && sb < 2) { 0 } else { g.block_type as usize };
                let x = &xr[18 * sb..18 * sb + 18];
                if x.iter().any(|&v| v != 0.0) {
                    for i in 0..36 {
                        let c = &self.t.cos36[i * 18..i * 18 + 18];
                        let mut s = 0f32;
                        for k in 0..18 { s += x[k] * c[k]; }
                        z[i] = s * self.t.imdct_win[bt][i];
                    }
                }
            } else {
                for w in 0..3 {
                    let mut x = [0f32; 6];
                    for k in 0..6 { x[k] = xr[18 * sb + 3 * k + w]; }
                    if x.iter().all(|&v| v == 0.0) { continue; }
                    for i in 0..12 {
                        let c = &self.t.cos12[i * 6..i * 6 + 6];
                        let mut s = 0f32;
                        for k in 0..6 { s += x[k] * c[k]; }
                        z[6 + 6 * w + i] += s * self.t.imdct_win[2][i];
                    }
                }
            }
            let ov = &mut self.overlap[ch][18 * sb..18 * sb + 18];
            for i in 0..18 {
                let mut v = z[i] + ov[i];
                if sb & 1 == 1 && i & 1 == 1 { v = -v; }
                out[i][sb] = v;
                ov[i] = z[18 + i];
            }
        }
    }

    /// The polyphase synthesis filterbank (ISO 11172-3 Fig. A.2): 32 subband samples → 32 PCM samples.
    fn synth(&mut self, ch: usize, s: &[f32; 32], out: &mut [f32]) {
        let off = (self.v_off[ch] + 1024 - 64) & 1023;
        self.v_off[ch] = off;
        let v = &mut self.v[ch];
        for i in 0..64 {
            let n = &self.t.synth_n[i * 32..i * 32 + 32];
            let mut acc = 0f32;
            for k in 0..32 { acc += n[k] * s[k]; }
            v[(off + i) & 1023] = acc;
        }
        let d = &self.t.window_d;
        for j in 0..32 {
            let mut acc = 0f32;
            for i in 0..8 {
                // U[i*64 + j] = V[i*128 + j], U[i*64 + 32 + j] = V[i*128 + 96 + j]
                acc += v[(off + i * 128 + j) & 1023] * d[i * 64 + j];
                acc += v[(off + i * 128 + 96 + j) & 1023] * d[i * 64 + 32 + j];
            }
            out[j] = acc;
        }
    }

    /// Decode one frame (header + rest of frame); returns samples per channel written (planar into `pcm`).
    pub fn decode_frame(&mut self, h: &Header, frame: &[u8], pcm: &mut [Vec<f32>]) -> Result<usize> {
        if h.layer != 3 { return Err(Error::Unsupported("MPEG audio layer I/II (owed)")); }
        let nch = h.channels();
        let mut p = 4 + if h.crc { 2 } else { 0 };
        let sl = h.side_len();
        if frame.len() < p + sl { return Err(Error::Eof); }
        let si = parse_side(h, &frame[p..p + sl], &self.t.band_long)?;
        p += sl;
        let main = &frame[p..];
        let ngr = if h.lsf() { 1 } else { 2 };
        let p23: usize = (0..ngr).flat_map(|g| (0..nch).map(move |c| (g, c))).map(|(g, c)| si.gr[g][c].part2_3_length).sum();
        if p23 > (main.len() + si.main_data_begin) * 8 { return Err(Error::Invalid("MP3 part2_3 lengths exceed the frame")); }
        let spf = 576 * ngr;
        for c in pcm.iter_mut().take(nch) { c.clear(); c.resize(spf, 0.0); }
        // bit reservoir. When the stream starts (or resumes) with main_data_begin pointing before the bytes
        // we hold, the missing data belongs to the earliest granules: those are skipped (zero spectrum, the
        // overlap still runs) until the rest lies in what we have — the rule FFmpeg/Chromium apply.
        let have = self.reservoir.len();
        let mdb = si.main_data_begin;
        let mut buf = Vec::with_capacity(mdb.min(have) + main.len());
        let mut first_gr = 0usize;
        let mut start_bit = 0usize;
        if mdb <= have {
            buf.extend_from_slice(&self.reservoir[have - mdb..]);
        } else {
            buf.extend_from_slice(&self.reservoir);
            let mut lbs = have * 8;
            while first_gr < ngr && (lbs >> 3) < mdb {
                for c in 0..nch { lbs += si.gr[first_gr][c].part2_3_length; }
                first_gr += 1;
            }
            start_bit = lbs.saturating_sub(8 * mdb);
        }
        buf.extend_from_slice(main);
        self.reservoir.extend_from_slice(main);
        if self.reservoir.len() > 4096 { let cut = self.reservoir.len() - 4096; self.reservoir.drain(..cut); }
        let ok = true;
        let mut r = BitReader::new(&buf);
        r.seek_bits(start_bit.min(buf.len() * 8));
        let mut granules = si.gr;
        let mut xr = [[0f32; 576]; 2];
        let mut sub = [[0f32; 32]; 18];
        let mut e = [0i32; 576];
        for gi in 0..ngr {
            for c in 0..nch {
                let start = r.bit_pos();
                let end = start + granules[gi][c].part2_3_length;
                let mut good = ok && gi >= first_gr && end <= buf.len() * 8;
                if gi < first_gr {
                    xr[c] = [0.0; 576];
                    continue;
                }
                if good {
                    let prev = if gi == 1 { Some(granules[0][c]) } else { None };
                    let mut g = granules[gi][c];
                    let res = self.read_scalefactors(h, &mut r, &mut g, prev.as_ref(), &si.scfsi[c], c)
                        .and_then(|_| { self.exponents(h, &g, &mut e); self.huffman(&mut r, &g, &e, end, &mut xr[c]) });
                    granules[gi][c] = g;
                    if res.is_err() { good = false; }
                }
                if !good {
                    xr[c] = [0.0; 576];
                    if end <= buf.len() * 8 { r.seek_bits(end); } else { r.seek_bits(buf.len() * 8); }
                }
            }
            if nch == 2 {
                let (a, b) = xr.split_at_mut(1);
                self.stereo(h, &granules[gi][0], &granules[gi][1], &mut a[0], &mut b[0]);
            }
            for c in 0..nch {
                let g = granules[gi][c];
                self.reorder(h, &g, &mut xr[c]);
                self.antialias(&g, &mut xr[c]);
                self.imdct(&g, c, &xr[c], &mut sub);
                for t in 0..18 {
                    let o = gi * 576 + t * 32;
                    let s = sub[t];
                    self.synth(c, &s, &mut pcm[c][o..o + 32]);
                }
            }
        }
        Ok(spf)
    }
}

#[inline]
fn exp2q(e: i32) -> f32 {
    const Q: [f32; 4] = [1.0, 1.189_207_1, 1.414_213_6, 1.681_792_8];
    let i = e >> 2;
    let f = Q[(e & 3) as usize];
    if i < -126 { 0.0 } else if i > 127 { f32::MAX } else { f * f32::from_bits(((i + 127) as u32) << 23) }
}

// ---------------------------------------------------------------- the stream

/// An MP3 file: ID3v2 skipped, frames found by sync (verified against the next header), Xing/Info + LAME
/// gapless trimming.
pub struct Mp3Stream {
    s: ByteStream,
    dec: Mp3Decoder,
    first: Header,
    channels: usize,
    rate: u32,
    free_len: usize,
    skip: u64,
    total: Option<u64>,
    emitted: u64,
    planes: Vec<Vec<f32>>,
    done: bool,
    /// The previous frame ended exactly where the stream now is (no resync needed).
    in_sync: bool,
}

fn skip_id3v2(s: &mut ByteStream) -> Result<()> {
    loop {
        if s.fill(10)? < 10 { return Ok(()); }
        let d = s.data();
        if &d[0..3] != b"ID3" || d[3] == 0xFF || d[4] == 0xFF || d[6..10].iter().any(|&b| b & 0x80 != 0) { return Ok(()); }
        let size = ((d[6] as u64) << 21) | ((d[7] as u64) << 14) | ((d[8] as u64) << 7) | d[9] as u64;
        let footer = if d[5] & 0x10 != 0 { 10 } else { 0 };
        s.consume(10);
        s.skip(size + footer)?;
    }
}

impl Mp3Stream {
    pub fn new(mut s: ByteStream) -> Result<Mp3Stream> {
        skip_id3v2(&mut s)?;
        let mut st = Mp3Stream { s, dec: Mp3Decoder::new(), first: Header::parse(0xFFFB_9000).unwrap(), channels: 0, rate: 0, free_len: 0, skip: 0, total: None, emitted: 0, planes: vec![], done: false, in_sync: false };
        let (h, len) = st.sync(None)?.ok_or(Error::Invalid("MP3: no frame found"))?;
        st.first = h;
        st.rate = h.rate();
        st.channels = st.scan_channels(&h)?;
        // Xing / Info / VBRI tag frame?
        let frame = st.s.need(len)?.to_vec();
        let off = 4 + if h.crc { 2 } else { 0 } + h.side_len();
        if frame.len() >= off + 8 && (&frame[off..off + 4] == b"Xing" || &frame[off..off + 4] == b"Info") {
            let flags = u32::from_be_bytes(frame[off + 4..off + 8].try_into().unwrap());
            let mut q = off + 8;
            let mut frames = None;
            if flags & 1 != 0 && frame.len() >= q + 4 { frames = Some(u32::from_be_bytes(frame[q..q + 4].try_into().unwrap()) as u64); q += 4; }
            if flags & 2 != 0 { q += 4; }
            if flags & 4 != 0 { q += 100; }
            if flags & 8 != 0 { q += 4; }
            // LAME tag: 9-byte version string, then (at +21) 12-bit delay and 12-bit padding
            if frame.len() >= q + 24 && (&frame[q..q + 4] == b"LAME" || &frame[q..q + 4] == b"Lavc" || &frame[q..q + 4] == b"Lavf") {
                let d = &frame[q + 21..q + 24];
                let delay = ((d[0] as u64) << 4) | (d[1] as u64 >> 4);
                let pad = (((d[1] & 15) as u64) << 8) | d[2] as u64;
                st.skip = delay + 529;
                if let Some(f) = frames {
                    let spf = h.samples() as u64;
                    // the decoder latency (529) is in the padding when LAME wrote it; a Lavf tag with pad < 529
                    // still loses those samples at the end
                    st.total = Some((f * spf).saturating_sub(delay + pad.max(529)));
                }
            } else if let Some(f) = frames {
                st.total = Some(f * h.samples() as u64);
            }
            st.s.consume(len);
        }
        Ok(st)
    }

    /// Mode can change from frame to frame (ISO allows it; l3-he_mode does it): look ahead over the first
    /// frames and hand out stereo if any of them is stereo (mono frames are then duplicated).
    fn scan_channels(&mut self, h: &Header) -> Result<usize> {
        if h.channels() == 2 { return Ok(2); }
        let n = self.s.fill(1 << 16)?;
        let d = self.s.data();
        let mut i = 0usize;
        let mut frames = 0;
        while i + 4 <= n && frames < 256 {
            let w = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
            match Header::parse(w).filter(|x| x.compatible(h)) {
                Some(x) if x.frame_len() > 0 => {
                    if x.channels() == 2 { return Ok(2); }
                    i += x.frame_len();
                    frames += 1;
                }
                _ => break,
            }
        }
        Ok(1)
    }

    /// Find the next frame: returns its header and length, the stream positioned at the frame.
    fn sync(&mut self, prev: Option<&Header>) -> Result<Option<(Header, usize)>> {
        let mut scanned = 0usize;
        loop {
            if self.s.fill(4)? < 4 { return Ok(None); }
            let d = self.s.data();
            let w = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
            if let Some(h) = Header::parse(w).filter(|h| h.layer == 3 && prev.map(|p| p.compatible(h)).unwrap_or(true)) {
                let mut len = h.frame_len();
                if len == 0 {
                    // free format: the distance to the next compatible header (cached once found)
                    if self.free_len == 0 { self.free_len = self.find_free_len(&h)?; }
                    len = self.free_len + h.padding as usize;
                }
                if len > 4 + h.side_len() {
                    // a header right where the last frame ended is trusted; one found by scanning must be
                    // confirmed by a compatible header after it (or by the stream ending exactly there)
                    let trusted = self.in_sync && scanned == 0;
                    let n = self.s.fill(len + 4)?;
                    let ok = if n >= len + 4 {
                        let d = self.s.data();
                        let w2 = u32::from_be_bytes([d[len], d[len + 1], d[len + 2], d[len + 3]]);
                        // a trailing tag (ID3v1 "TAG", APEv2 "APET", Lyrics3 "LYRI", another "ID3") also ends a frame
                        let tag = matches!(&d[len..len + 3], b"TAG" | b"API" | b"APE" | b"LYR" | b"ID3");
                        trusted || tag || Header::parse(w2).map(|h2| h2.compatible(&h)).unwrap_or(false)
                    } else { n == len || (trusted && n >= len) };
                    if ok { return Ok(Some((h, len))); }
                }
            }
            self.s.consume(1);
            scanned += 1;
            if scanned > 1 << 20 { return Ok(None); }
        }
    }

    fn find_free_len(&mut self, h: &Header) -> Result<usize> {
        let n = self.s.fill(8192)?;
        let d = self.s.data();
        for i in (4 + h.side_len())..n.saturating_sub(4) {
            let w = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
            if let Some(h2) = Header::parse(w) {
                if h2.compatible(h) && h2.bitrate_index == 0 { return Ok(i - h.padding as usize); }
            }
        }
        Err(Error::Invalid("MP3 free-format frame length"))
    }
}

impl Source for Mp3Stream {
    fn info(&self) -> Info {
        Info { rate: self.rate, channels: self.channels as u16, bits: 0, frames: self.total, format: Format::Mp3, codec: Codec::Mp3, float: true }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        loop {
            if self.done { return Ok(false); }
            let first = self.first;
            let Some((h, len)) = self.sync(Some(&first))? else { self.done = true; return Ok(false); };
            let frame = self.s.take(len)?;
            self.in_sync = true;
            self.planes.resize_with(2, Vec::new);
            let n = match self.dec.decode_frame(&h, &frame, &mut self.planes) {
                Ok(n) => n,
                Err(_) => continue,
            };
            let hc = h.channels();
            let mut start = 0usize;
            if self.skip > 0 { let s = (self.skip as usize).min(n); start = s; self.skip -= s as u64; }
            // a stereo frame in a stream handed out as mono (only possible past the look-ahead): downmix
            if hc == 2 && self.channels == 1 {
                let (a, b) = self.planes.split_at_mut(1);
                for (x, y) in a[0].iter_mut().zip(b[0].iter()) { *x = 0.5 * (*x + *y); }
            }
            let mut end = n;
            if let Some(t) = self.total {
                let remain = t.saturating_sub(self.emitted) as usize;
                end = end.min(start + remain);
                if remain == 0 { self.done = true; return Ok(false); }
            }
            if end <= start { continue; }
            let frames = end - start;
            pcm.set_float(self.channels, frames);
            for c in 0..self.channels {
                let src = &self.planes[c.min(hc - 1)];
                pcm.flt[c].copy_from_slice(&src[start..end]);
            }
            self.emitted += frames as u64;
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn huff24_code() {
        let t = Tables::new();
        let d = [0b0001_0001u8, 0, 0, 0];
        let mut r = BitReader::new(&d);
        let s = t.big[15].decode(&mut r).unwrap();
        assert_eq!((s >> 4, s & 15, r.bit_pos()), (7, 15, 8));
        let d = [0b0011_0000u8, 0, 0, 0];
        let mut r = BitReader::new(&d);
        assert_eq!(t.big[15].decode(&mut r).unwrap(), 0xFF);
    }
}
