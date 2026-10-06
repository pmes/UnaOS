//! The byte source every decoder reads from: a minimal `no_std` [`Read`], an owned in-memory source, and
//! [`ByteStream`] — a refillable window the container parsers peek into and consume from. Decoders never
//! seek backwards; anything that needs the whole file (MP4's `moov` at the tail) asks for it explicitly.
use crate::{Error, Result};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// A forward-only byte source (the kernel implements it over the VFS, the host over a file or a slice).
pub trait Read: Send {
    /// Fill as much of `buf` as is available; `Ok(0)` is end of stream.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    /// SEEKTABLE (rmbp B433): reposition to absolute byte `off`; `Ok(false)` = this source cannot (the default).
    fn seek(&mut self, _off: u64) -> Result<bool> { Ok(false) }
    /// The source's total length in bytes, when known (a CBR estimate needs it).
    fn len(&self) -> Option<u64> { None }
}

/// An owned in-memory source.
pub struct VecReader {
    data: Vec<u8>,
    pos: usize,
}
impl VecReader {
    pub fn new(data: Vec<u8>) -> VecReader { VecReader { data, pos: 0 } }
}
impl Read for VecReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = buf.len().min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
    fn seek(&mut self, off: u64) -> Result<bool> { self.pos = (off.min(self.data.len() as u64)) as usize; Ok(true) }
    fn len(&self) -> Option<u64> { Some(self.data.len() as u64) }
}

/// A refillable window over a [`Read`]: `fill(n)` makes `n` bytes visible (or all that remain), `data()` is
/// the window, `consume(n)` drops from the front. `offset()` is the stream position of `data()[0]`.
pub struct ByteStream {
    src: Box<dyn Read>,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
    base: u64,
}
impl ByteStream {
    pub fn new(src: Box<dyn Read>) -> ByteStream { ByteStream { src, buf: Vec::new(), pos: 0, eof: false, base: 0 } }
    /// A stream that starts with `head` already read (the sniffed prefix) followed by `src`.
    pub fn with_head(head: Vec<u8>, src: Box<dyn Read>) -> ByteStream { ByteStream { src, buf: head, pos: 0, eof: false, base: 0 } }
    pub fn data(&self) -> &[u8] { &self.buf[self.pos..] }
    pub fn offset(&self) -> u64 { self.base + self.pos as u64 }
    pub fn at_eof(&mut self) -> Result<bool> { Ok(self.fill(1)? == 0) }
    /// Make at least `n` bytes visible unless the source ends first; returns the visible count.
    pub fn fill(&mut self, n: usize) -> Result<usize> {
        while self.buf.len() - self.pos < n && !self.eof {
            if self.pos > 0 && (self.pos >= 1 << 16 || self.pos * 2 >= self.buf.len()) {
                self.buf.drain(..self.pos);
                self.base += self.pos as u64;
                self.pos = 0;
            }
            // grow in bounded steps, so a lying length field never allocates more than the source delivers
            let want = (n - (self.buf.len() - self.pos)).clamp(1 << 15, 1 << 20);
            let old = self.buf.len();
            self.buf.resize(old + want, 0);
            let got = self.src.read(&mut self.buf[old..])?;
            self.buf.truncate(old + got);
            if got == 0 { self.eof = true; }
        }
        Ok(self.buf.len() - self.pos)
    }
    /// Exactly `n` bytes or [`Error::Eof`].
    pub fn need(&mut self, n: usize) -> Result<&[u8]> {
        if self.fill(n)? < n { return Err(Error::Eof); }
        Ok(&self.buf[self.pos..self.pos + n])
    }
    pub fn consume(&mut self, n: usize) { self.pos = (self.pos + n).min(self.buf.len()); }
    /// Take exactly `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<Vec<u8>> {
        let v = self.need(n)?.to_vec();
        self.consume(n);
        Ok(v)
    }
    /// Skip `n` bytes (forward only); [`Error::Eof`] if the stream ends first.
    pub fn skip(&mut self, mut n: u64) -> Result<()> {
        loop {
            let have = (self.buf.len() - self.pos) as u64;
            if have >= n { self.pos += n as usize; return Ok(()); }
            n -= have;
            self.pos = self.buf.len();
            if self.fill(1)? == 0 { return Err(Error::Eof); }
        }
    }
    /// Read everything that remains.
    pub fn rest(&mut self) -> Result<Vec<u8>> {
        while !self.eof { let n = self.buf.len() - self.pos + (1 << 20); self.fill(n)?; }
        let v = self.buf[self.pos..].to_vec();
        self.pos = self.buf.len();
        Ok(v)
    }
}

/// SEEKTABLE (rmbp B433): repositioning. The window is kept when the target lies inside it.
impl ByteStream {
    /// Move to absolute stream offset `off`. `Ok(false)` when the target is outside the window and the source
    /// cannot seek (the stream is then unchanged).
    pub fn seek(&mut self, off: u64) -> Result<bool> {
        if off >= self.base && off <= self.base + self.buf.len() as u64 {
            self.pos = (off - self.base) as usize;
            return Ok(true);
        }
        if !self.src.seek(off)? { return Ok(false); }
        self.buf.clear();
        self.pos = 0;
        self.base = off;
        self.eof = false;
        Ok(true)
    }
    /// Can this stream reposition outside its window? (Asked by seeking the source to where it already is: the
    /// window's end.)
    pub fn seekable(&mut self) -> bool { let end = self.base + self.buf.len() as u64; self.src.seek(end).unwrap_or(false) }
    /// The source's total length, when known.
    pub fn len(&self) -> Option<u64> { self.src.len() }
}
