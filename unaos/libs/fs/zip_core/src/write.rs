//! The writer: each entry as a local file header and its data, then the central directory and the end record.
//! Deflated data is `git_core::deflate::deflate` at level 6 (raw RFC 1951, what method 8 stores); the CRC-32 is
//! pixel_core's. Every entry carries the fixed DOS time 1980-01-01 00:00 (time 0x0000, date 0x0021) and general
//! purpose bit 11 (UTF-8 names), so the same entries always give the same bytes.

use alloc::vec::Vec;

use crate::{FLAG_UTF8, Method, Refusal, SIG_CENTRAL, SIG_EOCD, SIG_LOCAL};

/// The DOS date of 1980-01-01 (day 1, month 1, year 1980 + 0).
const DOS_DATE: u16 = 0x0021;
/// Version needed to extract: 1.0 for stored, 2.0 for deflated (APPNOTE 4.4.3.2).
fn version_needed(m: Method) -> u16 {
    match m {
        Method::Stored => 10,
        Method::Deflated => 20,
    }
}

/// Builds an archive entry by entry; [`Writer::finish`] appends the central directory.
#[derive(Clone, Debug, Default)]
pub struct Writer {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u32,
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `data` as `name`, stored or deflated. Refuses a name over 65 535 bytes and anything that would need
    /// ZIP64 (the 65 536th entry, or a size or offset past `u32`).
    pub fn add(&mut self, name: &str, data: &[u8], method: Method) -> Result<(), Refusal> {
        if name.len() > 0xFFFF {
            return Err(Refusal::NameTooLong);
        }
        if self.count >= 0xFFFF {
            return Err(Refusal::Zip64);
        }
        let body = match method {
            Method::Stored => data.to_vec(),
            Method::Deflated => git_core::deflate::deflate(data, 6),
        };
        let fits = |n: usize| n < 0xFFFF_FFFF;
        let offset = self.out.len();
        if !fits(data.len()) || !fits(body.len()) || !fits(offset) || !fits(offset + LOCAL_FIXED + name.len() + body.len() + self.central.len() + 46 + name.len() + 22) {
            return Err(Refusal::Zip64);
        }
        let crc = pixel_core::crc::crc32(data);

        let o = &mut self.out;
        put32(o, SIG_LOCAL);
        put16(o, version_needed(method));
        put16(o, FLAG_UTF8);
        put16(o, method.code());
        put16(o, 0);
        put16(o, DOS_DATE);
        put32(o, crc);
        put32(o, body.len() as u32);
        put32(o, data.len() as u32);
        put16(o, name.len() as u16);
        put16(o, 0);
        o.extend_from_slice(name.as_bytes());
        o.extend_from_slice(&body);

        let c = &mut self.central;
        put32(c, SIG_CENTRAL);
        put16(c, 20); // version made by: MS-DOS host (0), spec 2.0
        put16(c, version_needed(method));
        put16(c, FLAG_UTF8);
        put16(c, method.code());
        put16(c, 0);
        put16(c, DOS_DATE);
        put32(c, crc);
        put32(c, body.len() as u32);
        put32(c, data.len() as u32);
        put16(c, name.len() as u16);
        put16(c, 0); // extra field length
        put16(c, 0); // comment length
        put16(c, 0); // disk number start
        put16(c, 0); // internal attributes
        put32(c, 0); // external attributes
        put32(c, offset as u32);
        c.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    /// The archive: the entries, the central directory and the end of central directory record (no comment).
    pub fn finish(self) -> Vec<u8> {
        let Writer { mut out, central, count } = self;
        let cd_off = out.len() as u32;
        out.extend_from_slice(&central);
        put32(&mut out, SIG_EOCD);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, count as u16);
        put16(&mut out, count as u16);
        put32(&mut out, central.len() as u32);
        put32(&mut out, cd_off);
        put16(&mut out, 0);
        out
    }
}

/// The fixed part of a local file header.
const LOCAL_FIXED: usize = 30;
