//! Big-endian TLS presentation-language codec (RFC 8446 §3).

use alloc::vec::Vec;

use crate::error::TlsError;

/// A cursor over a received byte slice. Every read is bounds-checked and fails with `Decode`.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let r = &self.buf[self.pos..];
        self.pos = self.buf.len();
        r
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], TlsError> {
        if self.remaining() < n {
            return Err(TlsError::Decode("truncated"));
        }
        let r = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(r)
    }
    pub fn u8(&mut self) -> Result<u8, TlsError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, TlsError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn u24(&mut self) -> Result<u32, TlsError> {
        let b = self.take(3)?;
        Ok(((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32)
    }
    pub fn u32(&mut self) -> Result<u32, TlsError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// opaque<0..2^8-1>
    pub fn vec8(&mut self) -> Result<&'a [u8], TlsError> {
        let n = self.u8()? as usize;
        self.take(n)
    }
    /// opaque<0..2^16-1>
    pub fn vec16(&mut self) -> Result<&'a [u8], TlsError> {
        let n = self.u16()? as usize;
        self.take(n)
    }
    /// opaque<0..2^24-1>
    pub fn vec24(&mut self) -> Result<&'a [u8], TlsError> {
        let n = self.u24()? as usize;
        self.take(n)
    }
    pub fn expect_end(&self) -> Result<(), TlsError> {
        if self.is_empty() { Ok(()) } else { Err(TlsError::Decode("trailing bytes")) }
    }
}

pub fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
pub fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
pub fn put_u24(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes()[1..]);
}

/// Writes a length-prefixed block: reserves `width` bytes, runs `f`, back-patches the length.
pub fn put_len_prefixed(out: &mut Vec<u8>, width: usize, f: impl FnOnce(&mut Vec<u8>)) {
    let at = out.len();
    out.resize(at + width, 0);
    f(out);
    let n = out.len() - at - width;
    let be = (n as u32).to_be_bytes();
    out[at..at + width].copy_from_slice(&be[4 - width..]);
}
pub fn put_vec8(out: &mut Vec<u8>, data: &[u8]) {
    out.push(data.len() as u8);
    out.extend_from_slice(data);
}
pub fn put_vec16(out: &mut Vec<u8>, data: &[u8]) {
    put_u16(out, data.len() as u16);
    out.extend_from_slice(data);
}
pub fn put_vec24(out: &mut Vec<u8>, data: &[u8]) {
    put_u24(out, data.len() as u32);
    out.extend_from_slice(data);
}

/// Constant-time equality for MACs.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}
