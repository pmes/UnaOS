// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RC4 (the ARC4 stream cipher; its keystream test vectors are RFC 6229). PDFCRYPT (LEDGER SR315): PDF's standard
//! security handler encrypts with RC4 at /V 1 and /V 2 (and /CFM /V2 crypt filters), and Algorithms 3, 4, 5 and 7
//! (ISO 32000-1 §7.6.3.3-4) run RC4 over the password entries.
//!
//! LEGACY-ONLY. RC4 is BROKEN (keystream biases; RFC 7465 prohibits it in TLS). It is here for reading encrypted PDF
//! files and for nothing else: PDF's standard security handler (`pdf_core::crypt`) is its one consumer.
//!
//! NOT CONSTANT-TIME: the state permutation is indexed by secret bytes by construction (the key schedule and every
//! output byte), so cache timing can leak key material. Acceptable only because the consumer decrypts files the user
//! already holds the password for.

use crate::ct::Zeroize;
use crate::Error;

/// An RC4 keystream state. Zeroized on drop. LEGACY-ONLY (see the module doc).
#[derive(Clone)]
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    /// The key-scheduling algorithm over a 1..=256-byte key. `Error::Length` for an empty or longer key.
    pub fn new(key: &[u8]) -> Result<Self, Error> {
        if key.is_empty() || key.len() > 256 {
            return Err(Error::Length);
        }
        let mut s = [0u8; 256];
        for (i, x) in s.iter_mut().enumerate() {
            *x = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        Ok(Rc4 { s, i: 0, j: 0 })
    }

    /// XOR the next `data.len()` keystream bytes into `data` (encryption and decryption are the same operation).
    pub fn apply(&mut self, data: &mut [u8]) {
        for b in data.iter_mut() {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k = self.s[self.s[self.i as usize].wrapping_add(self.s[self.j as usize]) as usize];
            *b ^= k;
        }
    }
}

impl Drop for Rc4 {
    fn drop(&mut self) {
        self.s.zeroize();
        self.i = 0;
        self.j = 0;
    }
}

/// One-shot: RC4 under `key` applied to `data` in place. `Error::Length` for an empty or over-256-byte key.
pub fn rc4(key: &[u8], data: &mut [u8]) -> Result<(), Error> {
    Rc4::new(key)?.apply(data);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut o = [0u8; N];
        for i in 0..N {
            o[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        o
    }

    /// RFC 6229 §2: the keystream at offsets 0, 16, 240 and 256 for the 40-bit key 0x0102030405 and the 128-bit key
    /// 0x0102...10.
    #[test]
    fn rfc6229_keystream() {
        let k40: [u8; 5] = [1, 2, 3, 4, 5];
        let k128: [u8; 16] = core::array::from_fn(|i| i as u8 + 1);
        let cases: [(&[u8], [(usize, &str); 4]); 2] = [
            (
                &k40,
                [
                    (0, "b2396305f03dc027ccc3524a0a1118a8"),
                    (16, "6982944f18fc82d589c403a47a0d0919"),
                    (240, "28cb1132c96ce286421dcaadb8b69eae"),
                    (256, "1cfcf62b03eddb641d77dfcf7f8d8c93"),
                ],
            ),
            (
                &k128,
                [
                    (0, "9ac7cc9a609d1ef7b2932899cde41b97"),
                    (16, "5248c4959014126a6e8a84f11d1a9e1c"),
                    (240, "065902e4b620f6cc36c8589f66432f2b"),
                    (256, "d39d566bc6bce3010768151549f3873f"),
                ],
            ),
        ];
        for (key, rows) in cases {
            let mut ks = [0u8; 272];
            rc4(key, &mut ks).unwrap();
            for (off, want) in rows {
                assert_eq!(ks[off..off + 16], unhex::<16>(want), "offset {off}");
            }
        }
    }

    /// The classic plaintext examples ("Key"/"Plaintext", "Wiki"/"pedia", "Secret"/"Attack at dawn"), and streaming
    /// in pieces equals one call.
    #[test]
    fn classic_examples_and_streaming() {
        let mut p = *b"Plaintext";
        rc4(b"Key", &mut p).unwrap();
        assert_eq!(p, unhex::<9>("bbf316e8d940af0ad3"));
        let mut p = *b"pedia";
        rc4(b"Wiki", &mut p).unwrap();
        assert_eq!(p, unhex::<5>("1021bf0420"));
        let mut p = *b"Attack at dawn";
        rc4(b"Secret", &mut p).unwrap();
        assert_eq!(p, unhex::<14>("45a01f645fc35b383552544b9bf5"));
        let mut a = [0u8; 100];
        rc4(b"Secret", &mut a).unwrap();
        let mut b = [0u8; 100];
        let mut st = Rc4::new(b"Secret").unwrap();
        st.apply(&mut b[..37]);
        st.apply(&mut b[37..]);
        assert_eq!(a, b);
        assert_eq!(Rc4::new(&[]).err(), Some(Error::Length));
    }
}
