//! The three CRCs the containers carry. All are MSB-first (non-reflected), no final XOR.
//! FLAC frame header CRC-8 (poly 0x07, RFC 9639 §9.1.8), FLAC frame CRC-16 (poly 0x8005, §9.3),
//! Ogg page CRC-32 (poly 0x04C11DB7, init 0, RFC 3533 §6).

const fn t8() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut k = 0;
        while k < 8 { c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 }; k += 1; }
        t[i] = c;
        i += 1;
    }
    t
}
const fn t16() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 { c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 }; k += 1; }
        t[i] = c;
        i += 1;
    }
    t
}
const fn t32() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut k = 0;
        while k < 8 { c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 }; k += 1; }
        t[i] = c;
        i += 1;
    }
    t
}
static T8: [u8; 256] = t8();
static T16: [u16; 256] = t16();
static T32: [u32; 256] = t32();

pub fn crc8(d: &[u8]) -> u8 { d.iter().fold(0u8, |c, &b| T8[(c ^ b) as usize]) }
pub fn crc16(d: &[u8]) -> u16 { d.iter().fold(0u16, |c, &b| (c << 8) ^ T16[((c >> 8) as u8 ^ b) as usize]) }
pub fn crc32_ogg(d: &[u8]) -> u32 { crc32_ogg_update(0, d) }
pub fn crc32_ogg_update(c: u32, d: &[u8]) -> u32 { d.iter().fold(c, |c, &b| (c << 8) ^ T32[((c >> 24) as u8 ^ b) as usize]) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_values() {
        // The CRC catalogue's "check" for "123456789": CRC-8/SMBUS 0xF4, CRC-16/UMTS (BUYPASS) 0xFEE8,
        // CRC-32/MPEG-2 without init/xorout = CRC-32/CKSUM's core 0x89A1897F.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
        assert_eq!(crc32_ogg(b"123456789"), 0x89A1_897F);
    }
}
