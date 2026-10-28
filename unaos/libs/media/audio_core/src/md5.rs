//! MD5 (RFC 1321) — FLAC's STREAMINFO carries the MD5 of the decoded PCM, and recomputing it over our own
//! output is the decoder's correctness proof (RFC 9639 §8.2). Not used for anything security-related.
//!
//! MD5-ONE (CRYPTOONE, LEDGER SR330): the one MD5 is crypto_core's (`crypto_core::md5`, RFC 1321 A.5 KATs there);
//! this path re-exports it so `audio_core::md5::Md5` stays the name the FLAC decoder and its tests use.

pub use crypto_core::md5::{md5, Md5};

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(d: [u8; 16]) -> alloc::string::String { d.iter().map(|b| alloc::format!("{:02x}", b)).collect() }
    #[test]
    fn rfc1321_suite() {
        // RFC 1321 Appendix A.5.
        for (m, h) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            ("12345678901234567890123456789012345678901234567890123456789012345678901234567890", "57edf4a22be3c955ac49da2e2107b67a"),
        ] {
            let mut x = Md5::new();
            for c in m.as_bytes().chunks(7) { x.update(c); }
            assert_eq!(hex(x.finalize()), h, "md5({:?})", m);
        }
    }
}
