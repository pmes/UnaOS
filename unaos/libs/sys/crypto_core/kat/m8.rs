// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M8 (WPA2CRYPTO, rmbp-ledger B882) — the three RSN primitives against the standards' own published
// vectors, all embedded (nothing fetched):
//   prf80211/  IEEE Std 802.11-2020 Annex J.3 (the PRF test vectors, PRF-192/256/384/512);
//   ccm/       RFC 3610 §8 packet vectors #1-#3 (M=8, L=2) and NIST SP 800-38C Appendix C Examples 1-3
//              (M=4/6/8 with 7/8/12-octet nonces, i.e. L=8/7/3 — the generic M and L);
//   keywrap/   RFC 3394 §4.1-§4.6 (128/192/256-bit KEKs over 128/192/256-bit key data);
//   pbkdf2/    IEEE Std 802.11-2020 Annex J.4 (the passphrase-to-PSK mapping, PBKDF2-HMAC-SHA1, 4096
//              iterations, 256 bits). Named `pbkdf2/` so it runs in M1's pbkdf2 sets (`m1_hashes_macs_kdfs`).
// Negative legs: a flipped tag bit (CCM) or wrapped byte (KW) must give Error::Auth and release nothing.

use super::*;
use crypto_core::ccm::{ccm_decrypt, ccm_encrypt, AesCcm};
use crypto_core::keywrap::{unwrap, wrap};
use crypto_core::pbkdf2::pbkdf2_hmac;
use crypto_core::prf80211::{prf, MAX_OUT};
use crypto_core::{Error, Sha1};

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("prf80211/ieee80211-annexj3", prf_annex_j3),
    ("ccm/rfc3610", ccm_rfc3610),
    ("ccm/sp800-38c", ccm_sp800_38c),
    ("keywrap/rfc3394", keywrap_rfc3394),
    ("pbkdf2/ieee80211-annexj4-sha1", psk_annex_j4),
];

fn prf_annex_j3() -> Tally {
    let mut t = Tally::new("");
    // IEEE 802.11-2020 Annex J.3 (PRF test vectors): (Key, Prefix, Data, Length in bits, PRF output).
    // J.3 test case 1: Key = 0x0b x 20, Prefix = "prefix", Data = "Hi There", Length = 192,
    //   PRF-192 = bcd4c650b30b9684951829e0d75f9d54b862175ed9f00606
    // J.3 test case 2: Key = "Jefe", Prefix = "prefix-2", Data = "what do ya want for nothing?", Length = 256,
    //   PRF-256 = 47c4908e30c947521ad20be9053450ecbea23d3aa604b77326d8b3825ff7475c
    // J.3 test case 3: Key = 0xaa x 80, Prefix = "prefix-3", Data = "Test Using Larger Than Block-Size Key - Hash
    //   Key First", Length = 384, PRF-384 = 0ab6c33ccf70d0d736f4b04c8a7373255511abc5073713163bd0b8c9eeb7e195
    //   6fa066820a73ddee3f6d3bd407e0682a
    // J.3 test case 4: Key = 0x0b x 20, Prefix = "prefix-4", Data = "Hi There Again", Length = 512,
    //   PRF-512 = 248cfbc532ab38ffa483c8a2e40bf170eb542a2e0916d7bf6d97da2c4c5ca877736c53a65b03fa4b3745ce7613f6ad68
    //   e0e4a798b7cf691c96176fd634a59a49
    let k1 = [0x0bu8; 20];
    let k3 = [0xaau8; 80];
    let cases: &[(&[u8], &[u8], &[u8], usize, &str)] = &[
        (&k1, b"prefix", b"Hi There", 192, "bcd4c650b30b9684951829e0d75f9d54b862175ed9f00606"),
        (b"Jefe", b"prefix-2", b"what do ya want for nothing?", 256, "47c4908e30c947521ad20be9053450ecbea23d3aa604b77326d8b3825ff7475c"),
        (
            &k3,
            b"prefix-3",
            b"Test Using Larger Than Block-Size Key - Hash Key First",
            384,
            "0ab6c33ccf70d0d736f4b04c8a7373255511abc5073713163bd0b8c9eeb7e1956fa066820a73ddee3f6d3bd407e0682a",
        ),
        (
            &k1,
            b"prefix-4",
            b"Hi There Again",
            512,
            "248cfbc532ab38ffa483c8a2e40bf170eb542a2e0916d7bf6d97da2c4c5ca877736c53a65b03fa4b3745ce7613f6ad68e0e4a798b7cf691c96176fd634a59a49",
        ),
    ];
    for (k, label, data, bits, want) in cases {
        let w = unhex(want);
        let mut out = vec![0u8; bits / 8];
        let r = prf(k, label, &[*data], &mut out);
        t.check(r.is_ok() && out == w, || format!("J.3 PRF-{bits} got {}", hex(&out)));
        // The data split across parts must not matter (the PTK passes Min/Max pieces).
        let mut out2 = vec![0u8; bits / 8];
        let (d0, d1) = data.split_at(data.len() / 2);
        let r = prf(k, label, &[d0, &[], d1], &mut out2);
        t.check(r.is_ok() && out2 == w, || format!("J.3 PRF-{bits} split data got {}", hex(&out2)));
    }
    let mut big = vec![0u8; MAX_OUT + 1];
    t.check(prf(b"k", b"l", &[], &mut big) == Err(Error::Length), || "PRF beyond 255 blocks accepted".into());
    t.check(prf(b"k", b"l", &[], &mut []) == Err(Error::Length), || "empty PRF output accepted".into());
    t
}

/// One CCM vector: encrypt (ciphertext ‖ tag must equal `want`), one-shot and keyed agree, decrypt round
/// trip, and a flipped tag bit gives `Error::Auth` with the buffer back to ciphertext.
fn ccm_case(t: &mut Tally, name: &str, key: &[u8], nonce: &[u8], aad: &[u8], msg: &[u8], m: usize, want: &[u8]) {
    let mut buf = msg.to_vec();
    let mut tag = vec![0u8; m];
    let r = ccm_encrypt(key, nonce, aad, &mut buf, &mut tag);
    let mut got = buf.clone();
    got.extend_from_slice(&tag);
    t.check(r.is_ok() && got == want, || format!("{name} encrypt got {}", hex(&got)));
    let c = AesCcm::new(key).unwrap();
    let mut buf2 = msg.to_vec();
    let mut tag2 = vec![0u8; m];
    let r = c.encrypt(nonce, aad, &mut buf2, &mut tag2);
    t.check(r.is_ok() && buf2 == buf && tag2 == tag, || format!("{name} keyed encrypt differs"));
    let ct = buf.clone();
    let r = ccm_decrypt(key, nonce, aad, &mut buf, &tag);
    t.check(r.is_ok() && buf == msg, || format!("{name} decrypt got {}", hex(&buf)));
    let mut bad = tag.clone();
    bad[m - 1] ^= 1;
    let mut buf3 = ct.clone();
    let r = c.decrypt(nonce, aad, &mut buf3, &bad);
    t.check(r == Err(Error::Auth) && buf3 == ct, || format!("{name} forged tag: {r:?}, buffer released"));
}

fn ccm_rfc3610() -> Tally {
    let mut t = Tally::new("");
    // RFC 3610 §8, Packet Vectors #1-#3: AES Key = C0 C1 ... CF, M = 8, L = 2, 8 cleartext header octets
    // (00 01 ... 07, the AAD) then the payload (08 09 ... to the packet length). Output = header ‖ ciphertext ‖ MAC.
    // #1: Nonce = 00 00 00 03 02 01 00 A0 A1 A2 A3 A4 A5, 31 octets in;
    //   out = 00010203 04050607 588C979A 61C663D2 F066D0C2 C0F98980 6D5F6B61 DAC38417 E8D12CFD F926E0
    // #2: Nonce = 00 00 00 04 03 02 01 A0 A1 A2 A3 A4 A5, 32 octets in;
    //   out = 00010203 04050607 72C91A36 E135F8CF 291CA894 085C87E3 CC15C439 C9E43A3B A091D56E 10400916
    // #3: Nonce = 00 00 00 05 04 03 02 A0 A1 A2 A3 A4 A5, 33 octets in;
    //   out = 00010203 04050607 51B1E5F4 4A197D1D A46B0F8E 2D282AE8 71E838BB 64DA8596 574ADAA7 6FBD9FB0 C5
    let key = unhex("c0c1c2c3c4c5c6c7c8c9cacbcccdcecf");
    let cases: &[(&str, usize, &str)] = &[
        ("00000003020100a0a1a2a3a4a5", 31, "0001020304050607588C979A61C663D2F066D0C2C0F989806D5F6B61DAC38417E8D12CFDF926E0"),
        ("00000004030201a0a1a2a3a4a5", 32, "000102030405060772C91A36E135F8CF291CA894085C87E3CC15C439C9E43A3BA091D56E10400916"),
        ("00000005040302a0a1a2a3a4a5", 33, "000102030405060751B1E5F44A197D1DA46B0F8E2D282AE871E838BB64DA8596574ADAA76FBD9FB0C5"),
    ];
    for (i, (nonce, len, out)) in cases.iter().enumerate() {
        let packet: Vec<u8> = (0..*len as u8).collect();
        let want = unhex(out);
        ccm_case(&mut t, &format!("RFC 3610 #{}", i + 1), &key, &unhex(nonce), &packet[..8], &packet[8..], 8, &want[8..]);
    }
    // Parameter refusals (RFC 3610 §2): an odd tag length, a 6-octet nonce, a 2^16-octet message at L = 2.
    let mut tag = [0u8; 5];
    t.check(ccm_encrypt(&key, &[0u8; 13], &[], &mut [0u8; 4], &mut tag) == Err(Error::Length), || "M=5 accepted".into());
    let mut tag = [0u8; 8];
    t.check(ccm_encrypt(&key, &[0u8; 6], &[], &mut [0u8; 4], &mut tag) == Err(Error::Length), || "6-octet nonce accepted".into());
    let mut long = vec![0u8; 1 << 16];
    t.check(ccm_encrypt(&key, &[0u8; 13], &[], &mut long, &mut tag) == Err(Error::Length), || "2^16 octets accepted at L=2".into());
    t
}

fn ccm_sp800_38c() -> Tally {
    let mut t = Tally::new("");
    // NIST SP 800-38C Appendix C (example vectors), K = 40414243 44454647 48494a4b 4c4d4e4f:
    // Example 1: Tlen = 32, Nlen = 56, Alen = 64, Plen = 32; N = 10111213 141516; A = 00010203 04050607;
    //   P = 20212223; C = 7162015b 4dac255d
    // Example 2: Tlen = 48, Nlen = 64, Alen = 128, Plen = 128; N = 10111213 14151617; A = 00010203 ... 0e0f;
    //   P = 20212223 ... 2e2f; C = d2a1f0e0 51ea5f62 081a7792 073d593d 1fc64fbf accd
    // Example 3: Tlen = 64, Nlen = 96, Alen = 160, Plen = 192; N = 10111213 14151617 18191a1b;
    //   A = 00010203 ... 1213; P = 20212223 ... 3637;
    //   C = e3b201a9 f5b71a7a 9b1ceaec cd97e70b 6176aad9 a4428aa5 484392fb c1b09951
    let key: Vec<u8> = (0x40..0x50).collect();
    let cases: &[(&str, usize, u8, u8, usize, &str)] = &[
        ("10111213141516", 8, 0x20, 0x24, 4, "7162015b4dac255d"),
        ("1011121314151617", 16, 0x20, 0x30, 6, "d2a1f0e051ea5f62081a7792073d593d1fc64fbfaccd"),
        ("101112131415161718191a1b", 20, 0x20, 0x38, 8, "e3b201a9f5b71a7a9b1ceaeccd97e70b6176aad9a4428aa5484392fbc1b09951"),
    ];
    for (i, (nonce, alen, p0, p1, m, c)) in cases.iter().enumerate() {
        let aad: Vec<u8> = (0..*alen as u8).collect();
        let msg: Vec<u8> = (*p0..*p1).collect();
        ccm_case(&mut t, &format!("SP 800-38C Example {}", i + 1), &key, &unhex(nonce), &aad, &msg, *m, &unhex(c));
    }
    t
}

fn keywrap_rfc3394() -> Tally {
    let mut t = Tally::new("");
    // RFC 3394 §4 (Test Vectors). KEK = 00 01 02 ... (16, 24 or 32 octets); Key Data = the first 16, 24 or 32
    // octets of 00112233 44556677 8899AABB CCDDEEFF 00010203 04050607 08090A0B 0C0D0E0F.
    // §4.1 128-bit KEK, 128-bit key: 1FA68B0A8112B447 AEF34BD8FB5A7B82 9D3E862371D2CFE5
    // §4.2 192-bit KEK, 128-bit key: 96778B25AE6CA435 F92B5B97C050AED2 468AB8A17AD84E5D
    // §4.3 256-bit KEK, 128-bit key: 64E8C3F9CE0F5BA2 63E9777905818A2A 93C8191E7D6E8AE7
    // §4.4 192-bit KEK, 192-bit key: 031D33264E15D332 68F24EC260743EDC E1C6C7DDEE725A93 6BA814915C6762D2
    // §4.5 256-bit KEK, 192-bit key: A8F9BC1612C68B3F F6E6F4FBE30E71E4 769C8B80A32CB895 8CD5D17D6B254DA1
    // §4.6 256-bit KEK, 256-bit key: 28C9F404C4B810F4 CBCCB35CFB87F826 3F5786E2D80ED326 CBC7F0E71A99F43B
    //                                FB988B9B7A02DD21
    let data = unhex("00112233445566778899AABBCCDDEEFF000102030405060708090A0B0C0D0E0F");
    let cases: &[(&str, usize, usize, &str)] = &[
        ("4.1", 16, 16, "1FA68B0A8112B447AEF34BD8FB5A7B829D3E862371D2CFE5"),
        ("4.2", 24, 16, "96778B25AE6CA435F92B5B97C050AED2468AB8A17AD84E5D"),
        ("4.3", 32, 16, "64E8C3F9CE0F5BA263E9777905818A2A93C8191E7D6E8AE7"),
        ("4.4", 24, 24, "031D33264E15D33268F24EC260743EDCE1C6C7DDEE725A936BA814915C6762D2"),
        ("4.5", 32, 24, "A8F9BC1612C68B3FF6E6F4FBE30E71E4769C8B80A32CB8958CD5D17D6B254DA1"),
        ("4.6", 32, 32, "28C9F404C4B810F4CBCCB35CFB87F8263F5786E2D80ED326CBC7F0E71A99F43BFB988B9B7A02DD21"),
    ];
    for (sec, klen, dlen, want) in cases {
        let kek: Vec<u8> = (0..*klen as u8).collect();
        let key = &data[..*dlen];
        let w = unhex(want);
        let mut out = vec![0u8; dlen + 8];
        let r = wrap(&kek, key, &mut out);
        t.check(r.is_ok() && out == w, || format!("RFC 3394 §{sec} wrap got {}", hex(&out)));
        let mut back = vec![0u8; *dlen];
        let r = unwrap(&kek, &w, &mut back);
        t.check(r.is_ok() && back == key, || format!("RFC 3394 §{sec} unwrap got {}", hex(&back)));
        let mut bad = w.clone();
        bad[dlen + 7] ^= 0x01;
        let mut back = vec![0u8; *dlen];
        let r = unwrap(&kek, &bad, &mut back);
        t.check(r == Err(Error::Auth) && back.iter().all(|&b| b == 0), || format!("RFC 3394 §{sec} forged: {r:?}, out not zeroed"));
    }
    // n = 1 (one 64-bit block) is RFC 5649's, not RFC 3394's; a ragged length is refused.
    let kek = [0u8; 16];
    t.check(wrap(&kek, &[0u8; 8], &mut [0u8; 16]) == Err(Error::Length), || "n=1 wrap accepted".into());
    t.check(wrap(&kek, &[0u8; 20], &mut [0u8; 28]) == Err(Error::Length), || "ragged wrap accepted".into());
    t.check(unwrap(&kek, &[0u8; 16], &mut [0u8; 8]) == Err(Error::Length), || "n=1 unwrap accepted".into());
    t
}

fn psk_annex_j4() -> Tally {
    let mut t = Tally::new("");
    // IEEE 802.11-2020 Annex J.4 (suggested pass-phrase-to-PSK mapping test vectors):
    // PSK = PBKDF2(passphrase, ssid, 4096, 256), PBKDF2 with HMAC-SHA1 as the PRF (§J.4.1).
    // J.4.2 test 1: passphrase = "password", SSID = "IEEE",
    //   PSK = f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e
    // J.4.2 test 2: passphrase = "ThisIsAPassword", SSID = "ThisIsASSID",
    //   PSK = 0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af
    // J.4.2 test 3: passphrase = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" (32 x 'a'),
    //   SSID = "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ" (32 x 'Z'),
    //   PSK = becb93866bb8c3832cb777c2f559807c8c59afcb6eae734885001300a981cc62
    let cases: &[(&[u8], &[u8], &str)] = &[
        (b"password", b"IEEE", "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"),
        (b"ThisIsAPassword", b"ThisIsASSID", "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af"),
        (&[b'a'; 32], &[b'Z'; 32], "becb93866bb8c3832cb777c2f559807c8c59afcb6eae734885001300a981cc62"),
    ];
    for (pass, ssid, want) in cases {
        let mut psk = [0u8; 32];
        let r = pbkdf2_hmac::<Sha1>(pass, ssid, 4096, &mut psk);
        t.check(r.is_ok() && psk[..] == unhex(want)[..], || format!("J.4 PSK got {}", hex(&psk)));
    }
    t
}
