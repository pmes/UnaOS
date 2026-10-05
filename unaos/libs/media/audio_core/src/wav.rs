//! WAV (RIFF/WAVE, RF64/BW64): integer PCM of any container width 8–32 (8-bit unsigned, the rest signed
//! little-endian), IEEE float 32/64, G.711 A-law and µ-law, and WAVE_FORMAT_EXTENSIBLE (whose SubFormat
//! GUID names one of the above; `wValidBitsPerSample` < container width is honoured by an arithmetic
//! right shift — the samples are stored left-justified).
use crate::io::ByteStream;
use crate::{Codec, Error, Format, Info, Pcm, Result, Source};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Int,
    Float,
    ALaw,
    MuLaw,
}

pub struct WavDecoder {
    s: ByteStream,
    kind: Kind,
    ch: usize,
    rate: u32,
    container: usize, // bytes per sample
    valid: u32,       // valid bits
    left: u64,        // data bytes remaining
    frames: u64,
}

fn le16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn le64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }

impl WavDecoder {
    pub fn new(mut s: ByteStream) -> Result<WavDecoder> {
        let h = s.take(12)?;
        let rf64 = &h[0..4] != b"RIFF";
        let mut ds64_data: Option<u64> = None;
        let mut fmt: Option<(Kind, usize, u32, usize, u32)> = None;
        loop {
            let c = s.take(8)?;
            let id = [c[0], c[1], c[2], c[3]];
            let len = le32(&c, 4) as u64;
            match &id {
                b"ds64" if rf64 => {
                    let b = s.take(len as usize)?;
                    if b.len() >= 16 { ds64_data = Some(le64(&b, 8)); }
                    if len & 1 == 1 { s.skip(1)?; }
                }
                b"fmt " => {
                    let f = s.take(len as usize)?;
                    if f.len() < 16 { return Err(Error::Invalid("short fmt chunk")); }
                    let mut tag = le16(&f, 0);
                    let ch = le16(&f, 2) as usize;
                    let rate = le32(&f, 4);
                    let align = le16(&f, 12) as usize;
                    let bits = le16(&f, 14) as u32;
                    let mut valid = bits;
                    if tag == 0xFFFE {
                        if f.len() < 40 { return Err(Error::Invalid("short WAVE_FORMAT_EXTENSIBLE")); }
                        let v = le16(&f, 18) as u32;
                        if v != 0 { valid = v; }
                        tag = le16(&f, 24); // SubFormat GUID's first two bytes are the format tag
                    }
                    if ch == 0 { return Err(Error::Invalid("zero channels")); }
                    let container = if ch > 0 && align / ch > 0 { align / ch } else { (bits as usize).div_ceil(8) };
                    let kind = match tag {
                        1 => Kind::Int,
                        3 => Kind::Float,
                        6 => Kind::ALaw,
                        7 => Kind::MuLaw,
                        _ => return Err(Error::Unsupported("WAV format tag (not PCM/float/G.711)")),
                    };
                    match kind {
                        Kind::Int if !(1..=4).contains(&container) || valid == 0 || valid as usize > container * 8 => {
                            return Err(Error::Unsupported("WAV integer width"));
                        }
                        Kind::Float if container != 4 && container != 8 => return Err(Error::Unsupported("WAV float width")),
                        Kind::ALaw | Kind::MuLaw if container != 1 => return Err(Error::Invalid("G.711 must be 8-bit")),
                        _ => {}
                    }
                    if kind == Kind::Float { valid = container as u32 * 8; }
                    if matches!(kind, Kind::ALaw | Kind::MuLaw) { valid = 16; }
                    fmt = Some((kind, ch, rate, container, valid));
                    if len & 1 == 1 { s.skip(1)?; }
                }
                b"data" => {
                    let (kind, ch, rate, container, valid) = fmt.ok_or(Error::Invalid("data before fmt"))?;
                    let mut dlen = if rf64 && len == 0xFFFF_FFFF { ds64_data.unwrap_or(u64::MAX) } else { len };
                    if dlen == 0 && !rf64 { dlen = u64::MAX; } // streamed WAV with an unset size: read to EOF
                    let fb = (ch * container) as u64;
                    let frames = if dlen == u64::MAX { 0 } else { dlen / fb };
                    return Ok(WavDecoder { s, kind, ch, rate, container, valid, left: dlen, frames });
                }
                _ => s.skip(len + (len & 1))?,
            }
        }
    }
}

fn alaw(v: u8) -> i32 {
    let a = v ^ 0x55;
    let seg = ((a >> 4) & 7) as i32;
    let mant = (a & 15) as i32;
    let mag = if seg == 0 { (mant << 4) + 8 } else { ((mant << 4) + 0x108) << (seg - 1) };
    if a & 0x80 != 0 { mag } else { -mag }
}
fn mulaw(v: u8) -> i32 {
    let u = !v;
    let seg = ((u >> 4) & 7) as i32;
    let mag = ((((u & 15) as i32) << 3) + 0x84) << seg;
    if u & 0x80 != 0 { 0x84 - mag } else { mag - 0x84 }
}

impl Source for WavDecoder {
    fn info(&self) -> Info {
        Info { rate: self.rate, channels: self.ch as u16, bits: self.valid as u16, frames: if self.left == u64::MAX { None } else { Some(self.frames) }, format: Format::Wav, codec: Codec::Pcm, float: self.kind == Kind::Float }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        let fb = self.ch * self.container;
        let want = (4096 * fb) as u64;
        let n = self.s.fill(want.min(self.left) as usize)?.min(want.min(self.left) as usize);
        let nf = n / fb;
        if nf == 0 { return Ok(false); }
        let d = &self.s.data()[..nf * fb];
        let (ch, cb) = (self.ch, self.container);
        match self.kind {
            Kind::Float => {
                pcm.set_float(ch, nf);
                for i in 0..nf {
                    for c in 0..ch {
                        let o = (i * ch + c) * cb;
                        pcm.flt[c][i] = if cb == 4 { f32::from_le_bytes(d[o..o + 4].try_into().unwrap()) } else { f64::from_le_bytes(d[o..o + 8].try_into().unwrap()) as f32 };
                    }
                }
            }
            Kind::Int => {
                let shift = (cb * 8) as u32 - self.valid;
                pcm.set_int(ch, nf, self.valid);
                for i in 0..nf {
                    for c in 0..ch {
                        let o = (i * ch + c) * cb;
                        let v: i32 = match cb {
                            1 => d[o] as i32 - 128,
                            2 => i16::from_le_bytes([d[o], d[o + 1]]) as i32,
                            3 => (i32::from_le_bytes([0, d[o], d[o + 1], d[o + 2]])) >> 8,
                            _ => i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]),
                        };
                        pcm.int[c][i] = v >> shift;
                    }
                }
            }
            Kind::ALaw | Kind::MuLaw => {
                pcm.set_int(ch, nf, 16);
                for i in 0..nf {
                    for c in 0..ch {
                        let b = d[i * ch + c];
                        pcm.int[c][i] = if self.kind == Kind::ALaw { alaw(b) } else { mulaw(b) };
                    }
                }
            }
        }
        self.s.consume(nf * fb);
        if self.left != u64::MAX { self.left -= (nf * fb) as u64; }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn g711_tables() {
        // ITU-T G.711 Table 1/2 end points (16-bit-scaled, as the reference decoders produce them).
        assert_eq!(alaw(0xD5), 8);
        assert_eq!(alaw(0x55), -8);
        assert_eq!(alaw(0xAA), 32256);
        assert_eq!(mulaw(0xFF), 0);
        assert_eq!(mulaw(0x80), 32124);
        assert_eq!(mulaw(0x00), -32124);
    }
}
