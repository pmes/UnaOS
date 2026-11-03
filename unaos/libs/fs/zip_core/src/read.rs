//! The reader: the end of central directory record found by a backward scan, the central directory walked record
//! by record, each local header checked against its central record, and the data inflated only when asked
//! ([`Entry::data`]). Sizes and the CRC come from the central directory (APPNOTE 4.4.4: with general purpose bit 3
//! the local header's are zero and a data descriptor follows the data; the central record always carries them).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use pixel_core::inflate::{self, ByteSource, InflateError, Sink};

use crate::{FLAG_ENCRYPTED, Method, Refusal, SIG_CENTRAL, SIG_EOCD, SIG_LOCAL, SIG_ZIP64_LOCATOR};

/// The fixed part of the end of central directory record (APPNOTE 4.3.16).
const EOCD_LEN: usize = 22;
/// The fixed part of a central directory file header (APPNOTE 4.3.12).
const CENTRAL_LEN: usize = 46;
/// The fixed part of a local file header (APPNOTE 4.3.7).
const LOCAL_LEN: usize = 30;
/// The ZIP64 end of central directory locator's length (APPNOTE 4.3.15).
const ZIP64_LOCATOR_LEN: usize = 20;

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    let s = b.get(i..i.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    let s = b.get(i..i.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// One entry of an [`Archive`], borrowing the archive's bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The name as the central directory spells it (`/`-separated, UTF-8).
    pub name: &'a str,
    pub method: Method,
    /// The CRC-32 the central directory declares for the uncompressed data.
    pub crc32: u32,
    /// The uncompressed size the central directory declares.
    pub size: u64,
    /// The entry's data as stored in the archive (deflated for [`Method::Deflated`]).
    pub compressed: &'a [u8],
}

/// A read archive: its entries in central directory order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive<'a> {
    entries: Vec<Entry<'a>>,
}

impl<'a> Archive<'a> {
    pub fn entries(&self) -> &[Entry<'a>] {
        &self.entries
    }

    /// The first entry whose name is exactly `name`.
    pub fn by_name(&self, name: &str) -> Option<&Entry<'a>> {
        self.entries.iter().find(|e| e.name == name)
    }
}

struct Src<'s> {
    d: &'s [u8],
    p: usize,
}

impl ByteSource for Src<'_> {
    fn next(&mut self) -> Option<u8> {
        let b = self.d.get(self.p).copied();
        self.p += 1;
        b
    }
}

/// Collects inflated bytes, refusing the byte past `max` (the declared size) so a bomb stops at the declaration.
struct VecSink {
    out: Vec<u8>,
    max: u64,
}

impl Sink for VecSink {
    fn push(&mut self, b: u8) -> Result<(), ()> {
        if self.out.len() as u64 >= self.max {
            return Err(());
        }
        self.out.push(b);
        Ok(())
    }
}

impl Entry<'_> {
    /// The uncompressed data: copied when stored, inflated when deflated; its length and CRC-32 checked against
    /// the central directory's.
    pub fn data(&self) -> Result<Vec<u8>, Refusal> {
        let out = match self.method {
            Method::Stored => self.compressed.to_vec(),
            Method::Deflated => {
                let mut src = Src { d: self.compressed, p: 0 };
                let cap = self.size.min(1 << 26) as usize;
                let mut sink = VecSink { out: Vec::with_capacity(cap), max: self.size };
                match inflate::inflate_raw(&mut src, &mut sink) {
                    Ok(_) => {}
                    Err(InflateError::SinkRejected) => return Err(Refusal::SizeMismatch { name: self.name.to_string() }),
                    Err(e) => {
                        return Err(Refusal::Inflate { name: self.name.to_string(), reason: inflate::inflate_reason(e) });
                    }
                }
                sink.out
            }
        };
        if out.len() as u64 != self.size {
            return Err(Refusal::SizeMismatch { name: self.name.to_string() });
        }
        if pixel_core::crc::crc32(&out) != self.crc32 {
            return Err(Refusal::CrcMismatch { name: self.name.to_string() });
        }
        Ok(out)
    }
}

/// The offset of the end of central directory record: the last signature in the final 22 + 65 535 bytes whose
/// comment fits inside the buffer.
fn find_eocd(b: &[u8]) -> Option<usize> {
    if b.len() < EOCD_LEN {
        return None;
    }
    let last = b.len() - EOCD_LEN;
    let first = last.saturating_sub(0xFFFF);
    (first..=last).rev().find(|&i| {
        u32_at(b, i) == Some(SIG_EOCD)
            && u16_at(b, i + 20).is_some_and(|c| i + EOCD_LEN + c as usize <= b.len())
    })
}

/// Read an archive: the end record, the central directory, every local header. Data is not inflated here.
pub fn read(b: &[u8]) -> Result<Archive<'_>, Refusal> {
    let e = find_eocd(b).ok_or(Refusal::NotZip)?;
    if e >= ZIP64_LOCATOR_LEN && u32_at(b, e - ZIP64_LOCATOR_LEN) == Some(SIG_ZIP64_LOCATOR) {
        return Err(Refusal::Zip64);
    }
    let trunc = Refusal::Truncated { what: "end of central directory" };
    let disk = u16_at(b, e + 4).ok_or(trunc.clone())?;
    let cd_disk = u16_at(b, e + 6).ok_or(trunc.clone())?;
    let on_disk = u16_at(b, e + 8).ok_or(trunc.clone())?;
    let total = u16_at(b, e + 10).ok_or(trunc.clone())?;
    let cd_size = u32_at(b, e + 12).ok_or(trunc.clone())?;
    let cd_off = u32_at(b, e + 16).ok_or(trunc)?;
    if total == 0xFFFF || on_disk == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF {
        return Err(Refusal::Zip64);
    }
    if disk != 0 || cd_disk != 0 || on_disk != total {
        return Err(Refusal::MultiDisk);
    }
    let cd_start = cd_off as usize;
    let cd_end = cd_start.checked_add(cd_size as usize).filter(|&x| x <= e);
    let cd = match cd_end {
        Some(end) => &b[cd_start..end],
        None => return Err(Refusal::Truncated { what: "central directory" }),
    };

    let mut entries = Vec::with_capacity(total as usize);
    let mut p = 0usize;
    for index in 0..total as u32 {
        let trunc = || Refusal::Truncated { what: "central directory" };
        if cd.len() < p + CENTRAL_LEN {
            return Err(trunc());
        }
        if u32_at(cd, p) != Some(SIG_CENTRAL) {
            return Err(Refusal::BadSignature { what: "central directory file header" });
        }
        let flags = u16_at(cd, p + 8).ok_or_else(trunc)?;
        let method = u16_at(cd, p + 10).ok_or_else(trunc)?;
        let crc32 = u32_at(cd, p + 16).ok_or_else(trunc)?;
        let csize = u32_at(cd, p + 20).ok_or_else(trunc)?;
        let usize_ = u32_at(cd, p + 24).ok_or_else(trunc)?;
        let nlen = u16_at(cd, p + 28).ok_or_else(trunc)? as usize;
        let xlen = u16_at(cd, p + 30).ok_or_else(trunc)? as usize;
        let clen = u16_at(cd, p + 32).ok_or_else(trunc)? as usize;
        let local = u32_at(cd, p + 42).ok_or_else(trunc)?;
        let name_bytes = cd.get(p + CENTRAL_LEN..p + CENTRAL_LEN + nlen).ok_or_else(trunc)?;
        let name = core::str::from_utf8(name_bytes).map_err(|_| Refusal::BadName { index })?;
        p += CENTRAL_LEN + nlen + xlen + clen;
        if p > cd.len() {
            return Err(trunc());
        }
        if csize == 0xFFFF_FFFF || usize_ == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
            return Err(Refusal::Zip64);
        }
        if flags & FLAG_ENCRYPTED != 0 {
            return Err(Refusal::Encrypted { name: String::from(name) });
        }
        let method = match method {
            0 => Method::Stored,
            8 => Method::Deflated,
            m => return Err(Refusal::Method { name: String::from(name), method: m }),
        };

        let l = local as usize;
        let ltrunc = Refusal::Truncated { what: "local file header" };
        if b.len() < l.saturating_add(LOCAL_LEN) {
            return Err(ltrunc);
        }
        if u32_at(b, l) != Some(SIG_LOCAL) {
            return Err(Refusal::BadSignature { what: "local file header" });
        }
        let lmethod = u16_at(b, l + 8).ok_or(ltrunc.clone())?;
        let lnlen = u16_at(b, l + 26).ok_or(ltrunc.clone())? as usize;
        let lxlen = u16_at(b, l + 28).ok_or(ltrunc.clone())? as usize;
        let lname = b.get(l + LOCAL_LEN..l + LOCAL_LEN + lnlen).ok_or(ltrunc)?;
        if lmethod != method.code() || lname != name_bytes {
            return Err(Refusal::LocalMismatch { name: String::from(name) });
        }
        let start = l + LOCAL_LEN + lnlen + lxlen;
        let compressed = start
            .checked_add(csize as usize)
            .and_then(|end| b.get(start..end))
            .ok_or(Refusal::Truncated { what: "entry data" })?;
        entries.push(Entry { name, method, crc32, size: usize_ as u64, compressed });
    }
    Ok(Archive { entries })
}
