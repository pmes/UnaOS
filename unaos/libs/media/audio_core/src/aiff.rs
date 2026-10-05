//! AIFF 1.3 and AIFF-C: big-endian signed integer PCM 1–32 bits (`NONE`/`twos`), little-endian (`sowt`),
//! IEEE float (`fl32`/`FL32`, `fl64`/`FL64`). COMM's 80-bit extended sample rate is decoded exactly.
use crate::io::ByteStream;
use crate::{Codec, Error, Format, Info, Pcm, Result, Source};

pub struct AiffDecoder {
    s: ByteStream,
    ch: usize,
    rate: u32,
    bits: u32,
    container: usize,
    little: bool,
    float: bool,
    left: u64,
    frames: u64,
}

fn be16(b: &[u8], o: usize) -> u16 { u16::from_be_bytes([b[o], b[o + 1]]) }
fn be32(b: &[u8], o: usize) -> u32 { u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }

/// IEEE 754 80-bit extended → integer Hz (truncated).
pub fn ext80_to_u32(b: &[u8]) -> u32 {
    let exp = (be16(b, 0) & 0x7FFF) as i32 - 16383;
    let mant = u64::from_be_bytes(b[2..10].try_into().unwrap());
    if b[0] & 0x80 != 0 || !(0..=63).contains(&exp) { return 0; }
    (mant >> (63 - exp)) as u32
}

impl AiffDecoder {
    pub fn new(mut s: ByteStream) -> Result<AiffDecoder> {
        let h = s.take(12)?;
        let aifc = &h[8..12] == b"AIFC";
        let mut comm: Option<(usize, u64, u32, u32, bool, bool)> = None;
        loop {
            let c = s.take(8)?;
            let len = be32(&c, 4) as u64;
            match &c[0..4] {
                b"COMM" => {
                    let f = s.take(len as usize)?;
                    if f.len() < 18 { return Err(Error::Invalid("short COMM")); }
                    let ch = be16(&f, 0) as usize;
                    let frames = be32(&f, 2) as u64;
                    let bits = be16(&f, 6) as u32;
                    let rate = ext80_to_u32(&f[8..18]);
                    let (mut little, mut float) = (false, false);
                    if aifc && f.len() >= 22 {
                        match &f[18..22] {
                            b"NONE" | b"twos" => {}
                            b"sowt" => little = true,
                            b"fl32" | b"FL32" | b"fl64" | b"FL64" => float = true,
                            _ => return Err(Error::Unsupported("AIFF-C compression type")),
                        }
                    }
                    if ch == 0 || rate == 0 { return Err(Error::Invalid("COMM channels/rate")); }
                    let bits = if float { if matches!(&f[18..22], b"fl64" | b"FL64") { 64 } else { 32 } } else { bits };
                    if !float && !(1..=32).contains(&bits) { return Err(Error::Unsupported("AIFF sample size")); }
                    comm = Some((ch, frames, bits, rate, little, float));
                    if len & 1 == 1 { s.skip(1)?; }
                }
                b"SSND" => {
                    let (ch, frames, bits, rate, little, float) = comm.ok_or(Error::Invalid("SSND before COMM"))?;
                    let o = s.take(8)?;
                    let off = be32(&o, 0) as u64;
                    s.skip(off)?;
                    let container = (bits as usize).div_ceil(8);
                    let left = (len.saturating_sub(8 + off)).min(frames * (ch * container) as u64);
                    return Ok(AiffDecoder { s, ch, rate, bits, container, little, float, left, frames });
                }
                _ => s.skip(len + (len & 1))?,
            }
        }
    }
}

impl Source for AiffDecoder {
    fn info(&self) -> Info {
        Info { rate: self.rate, channels: self.ch as u16, bits: self.bits as u16, frames: Some(self.frames), format: Format::Aiff, codec: Codec::Pcm, float: self.float }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        let fb = self.ch * self.container;
        let want = ((4096 * fb) as u64).min(self.left) as usize;
        let nf = self.s.fill(want)?.min(want) / fb;
        if nf == 0 { return Ok(false); }
        let d = &self.s.data()[..nf * fb];
        let (ch, cb) = (self.ch, self.container);
        if self.float {
            pcm.set_float(ch, nf);
            for i in 0..nf {
                for c in 0..ch {
                    let o = (i * ch + c) * cb;
                    pcm.flt[c][i] = if cb == 4 { f32::from_be_bytes(d[o..o + 4].try_into().unwrap()) } else { f64::from_be_bytes(d[o..o + 8].try_into().unwrap()) as f32 };
                }
            }
        } else {
            let shift = (cb * 8) as u32 - self.bits;
            pcm.set_int(ch, nf, self.bits);
            for i in 0..nf {
                for c in 0..ch {
                    let o = (i * ch + c) * cb;
                    let mut w = [0u8; 4];
                    for k in 0..cb { w[k] = if self.little { d[o + cb - 1 - k] } else { d[o + k] }; }
                    let v = i32::from_be_bytes(w) >> (32 - cb * 8); // sign-extend the container
                    pcm.int[c][i] = v >> shift; // left-justified in the container (AIFF 1.3 §"Sound Data")
                }
            }
        }
        self.s.consume(nf * fb);
        self.left -= (nf * fb) as u64;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ext80() {
        assert_eq!(super::ext80_to_u32(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]), 44100);
        assert_eq!(super::ext80_to_u32(&[0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]), 48000);
    }
}
