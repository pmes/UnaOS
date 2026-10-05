// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CRC-32/ISO-HDLC (the gzip and PNG CRC: reflected polynomial `0xEDB88320`, init all-ones, final
//! complement) as a streaming state. This is the kernel's `hash::Crc32`, restated here so the moved
//! `inflate` keeps its gzip-trailer check without reaching back into the kernel: same table, same
//! three-call API, same answers.

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// A running CRC-32.
pub struct Crc32 {
    crc: u32,
}

impl Crc32 {
    pub fn new() -> Self {
        Self { crc: 0xFFFF_FFFF }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.crc;
        for &b in data {
            crc = (crc >> 8) ^ TABLE[((crc ^ b as u32) & 0xFF) as usize];
        }
        self.crc = crc;
    }

    pub fn finish(&self) -> u32 {
        !self.crc
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// CRC-32 of `data` in one call.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finish()
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_value() {
        // The CRC catalogue's check value for CRC-32/ISO-HDLC.
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }
}
