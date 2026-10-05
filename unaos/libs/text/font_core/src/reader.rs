//! Big-endian, bounds-checked reads. Every table parser goes through these; a read past the end is `None`,
//! never a panic — the fuzz test in `tests/fuzz.rs` holds the crate to that.

#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    pub fn at(data: &'a [u8], pos: usize) -> Option<Self> {
        if pos > data.len() { None } else { Some(Reader { data, pos }) }
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn skip(&mut self, n: usize) -> Option<()> {
        let e = self.pos.checked_add(n)?;
        if e > self.data.len() {
            return None;
        }
        self.pos = e;
        Some(())
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let e = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..e)?;
        self.pos = e;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        let v = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    pub fn i8(&mut self) -> Option<i8> {
        self.u8().map(|v| v as i8)
    }
    pub fn u16(&mut self) -> Option<u16> {
        let b = self.bytes(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    pub fn u32(&mut self) -> Option<u32> {
        let b = self.bytes(4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    /// F2DOT14 (signed 2.14 fixed).
    pub fn f2dot14(&mut self) -> Option<f32> {
        self.i16().map(|v| v as f32 / 16384.0)
    }
}

#[inline]
pub fn u8_at(d: &[u8], off: usize) -> Option<u8> {
    d.get(off).copied()
}
#[inline]
pub fn u16_at(d: &[u8], off: usize) -> Option<u16> {
    let b = d.get(off..off.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}
#[inline]
pub fn i16_at(d: &[u8], off: usize) -> Option<i16> {
    u16_at(d, off).map(|v| v as i16)
}
#[inline]
pub fn u32_at(d: &[u8], off: usize) -> Option<u32> {
    let b = d.get(off..off.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
/// A sub-slice `d[off..off+len]`, or `None` if it leaves `d`.
#[inline]
pub fn slice(d: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    d.get(off..off.checked_add(len)?)
}
/// `d[off..]`, or `None` past the end.
#[inline]
pub fn tail(d: &[u8], off: usize) -> Option<&[u8]> {
    d.get(off..)
}
