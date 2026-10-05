//! The Opus range decoder, RFC 6716 §4.1 (`ec_dec`): the byte-wise range decoder that SILK and CELT share,
//! the raw bits read from the END of the frame (§4.1.4), `ec_tell` / `ec_tell_frac` (§4.1.6), and the final
//! range state every conformance check compares (`OPUS_GET_FINAL_RANGE`).

const SYM_BITS: u32 = 8;
const CODE_BITS: u32 = 32;
const SYM_MAX: u32 = (1 << SYM_BITS) - 1;
const CODE_TOP: u32 = 1 << (CODE_BITS - 1);
const CODE_BOT: u32 = CODE_TOP >> SYM_BITS;
const CODE_EXTRA: u32 = (CODE_BITS - 2) % SYM_BITS + 1;
const WINDOW_SIZE: i32 = 32;
const UINT_BITS: i32 = 8;
pub const BITRES: i32 = 3;

/// `EC_ILOG`: the number of bits needed to hold `v` (0 for 0).
#[inline]
pub fn ilog(v: u32) -> i32 { 32 - v.leading_zeros() as i32 }

pub struct RangeDecoder<'a> {
    buf: &'a [u8],
    pub storage: u32,
    end_offs: u32,
    end_window: u32,
    nend_bits: i32,
    pub nbits_total: i32,
    offs: u32,
    pub rng: u32,
    val: u32,
    ext: u32,
    rem: i32,
    pub error: bool,
}

impl<'a> RangeDecoder<'a> {
    pub fn new(buf: &'a [u8]) -> RangeDecoder<'a> {
        let mut d = RangeDecoder {
            buf,
            storage: buf.len() as u32,
            end_offs: 0,
            end_window: 0,
            nend_bits: 0,
            nbits_total: (CODE_BITS + 1 - ((CODE_BITS - CODE_EXTRA) / SYM_BITS) * SYM_BITS) as i32,
            offs: 0,
            rng: 1 << CODE_EXTRA,
            val: 0,
            ext: 0,
            rem: 0,
            error: false,
        };
        d.rem = d.read_byte();
        d.val = d.rng - 1 - (d.rem as u32 >> (SYM_BITS - CODE_EXTRA));
        d.normalize();
        d
    }
    #[inline]
    fn read_byte(&mut self) -> i32 {
        if self.offs < self.storage { let b = self.buf[self.offs as usize]; self.offs += 1; b as i32 } else { 0 }
    }
    #[inline]
    fn read_byte_from_end(&mut self) -> i32 {
        if self.end_offs < self.storage {
            self.end_offs += 1;
            self.buf[(self.storage - self.end_offs) as usize] as i32
        } else { 0 }
    }
    #[inline]
    fn normalize(&mut self) {
        while self.rng <= CODE_BOT {
            self.nbits_total += SYM_BITS as i32;
            self.rng <<= SYM_BITS;
            let mut sym = self.rem;
            self.rem = self.read_byte();
            sym = ((sym << SYM_BITS) | self.rem) >> (SYM_BITS - CODE_EXTRA);
            self.val = (self.val.wrapping_shl(SYM_BITS).wrapping_add(SYM_MAX & !(sym as u32))) & (CODE_TOP - 1);
        }
    }
    /// `ec_decode`: the cumulative frequency the next symbol falls in, of total `ft`.
    #[inline]
    pub fn decode(&mut self, ft: u32) -> u32 {
        self.ext = self.rng / ft;
        let s = self.val / self.ext;
        ft - (s + 1).min(ft)
    }
    #[inline]
    pub fn decode_bin(&mut self, bits: u32) -> u32 {
        self.ext = self.rng >> bits;
        let s = self.val / self.ext;
        (1u32 << bits) - (s + 1).min(1u32 << bits)
    }
    #[inline]
    pub fn update(&mut self, fl: u32, fh: u32, ft: u32) {
        let s = self.ext.wrapping_mul(ft - fh);
        self.val = self.val.wrapping_sub(s);
        self.rng = if fl > 0 { self.ext.wrapping_mul(fh - fl) } else { self.rng.wrapping_sub(s) };
        self.normalize();
    }
    /// A binary symbol whose probability of a 1 is `1/2^logp`.
    #[inline]
    pub fn bit_logp(&mut self, logp: u32) -> bool {
        let r = self.rng;
        let d = self.val;
        let s = r >> logp;
        let ret = d < s;
        if !ret { self.val = d - s; }
        self.rng = if ret { s } else { r - s };
        self.normalize();
        ret
    }
    /// A symbol from an inverse-CDF table with `1 << ftb` total.
    #[inline]
    pub fn icdf(&mut self, icdf: &[u8], ftb: u32) -> usize {
        let mut s = self.rng;
        let d = self.val;
        let r = s >> ftb;
        let mut ret = 0usize;
        let mut t;
        loop {
            t = s;
            s = r.wrapping_mul(icdf[ret] as u32);
            if d >= s { break; }
            ret += 1;
        }
        self.val = d - s;
        self.rng = t - s;
        self.normalize();
        ret
    }
    pub fn icdf16(&mut self, icdf: &[u16], ftb: u32) -> usize {
        let mut s = self.rng;
        let d = self.val;
        let r = s >> ftb;
        let mut ret = 0usize;
        let mut t;
        loop {
            t = s;
            s = r.wrapping_mul(icdf[ret] as u32);
            if d >= s { break; }
            ret += 1;
        }
        self.val = d - s;
        self.rng = t - s;
        self.normalize();
        ret
    }
    /// A uniformly distributed integer in `[0, ft)`.
    pub fn dec_uint(&mut self, ft: u32) -> u32 {
        let ft1 = ft - 1;
        let mut ftb = ilog(ft1);
        if ftb > UINT_BITS {
            ftb -= UINT_BITS;
            let f = (ft1 >> ftb) + 1;
            let s = self.decode(f);
            self.update(s, s + 1, f);
            let t = (s << ftb) | self.bits(ftb as u32);
            if t <= ft1 { return t; }
            self.error = true;
            ft1
        } else {
            let s = self.decode(ft);
            self.update(s, s + 1, ft);
            s
        }
    }
    /// Raw bits from the end of the frame.
    pub fn bits(&mut self, bits: u32) -> u32 {
        let mut window = self.end_window;
        let mut available = self.nend_bits;
        if (available as u32) < bits {
            loop {
                window |= (self.read_byte_from_end() as u32) << available;
                available += SYM_BITS as i32;
                if available > WINDOW_SIZE - SYM_BITS as i32 { break; }
            }
        }
        let ret = if bits == 32 { window } else { window & ((1u32 << bits) - 1) };
        window = if bits == 32 { 0 } else { window >> bits };
        available -= bits as i32;
        self.end_window = window;
        self.nend_bits = available;
        self.nbits_total += bits as i32;
        ret
    }
    #[inline]
    pub fn tell(&self) -> i32 { self.nbits_total - ilog(self.rng) }
    pub fn tell_frac(&self) -> u32 {
        const CORRECTION: [u32; 8] = [35733, 38967, 42495, 46340, 50535, 55109, 60097, 65535];
        let nbits = (self.nbits_total as u32) << BITRES;
        let mut l = ilog(self.rng);
        let r = self.rng >> (l - 16);
        let mut b = (r >> 12) - 8;
        b += (r > CORRECTION[b as usize]) as u32;
        l = (l << 3) + b as i32;
        nbits.wrapping_sub(l as u32)
    }
    pub fn range_bytes(&self) -> u32 { self.offs }
}
