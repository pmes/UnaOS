//! PEM (RFC 7468) and base64 (RFC 4648 §4) — for the trust bundle.

use alloc::vec::Vec;

fn b64val(c: u8) -> Option<u8> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    })
}

/// Strict-ish base64 decode: whitespace ignored, '=' padding only at the end, no other characters.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut n = 0;
    let mut pad = 0;
    for c in s.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            pad += 1;
            continue;
        }
        if pad > 0 {
            return None;
        }
        acc = (acc << 6) | b64val(c)? as u32;
        n += 1;
        if n == 4 {
            out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8, acc as u8]);
            acc = 0;
            n = 0;
        }
    }
    match (n, pad) {
        (0, 0) => {}
        (2, 2) | (2, 0) => out.push((acc >> 4) as u8),
        (3, 1) | (3, 0) => out.extend_from_slice(&[(acc >> 10) as u8, (acc >> 2) as u8]),
        _ => return None,
    }
    Some(out)
}

/// Every `-----BEGIN <label>-----` … `-----END <label>-----` block with the given label, decoded. Malformed blocks
/// are skipped and counted.
pub fn pem_blocks(text: &str, label: &str) -> (Vec<Vec<u8>>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    let begin = alloc::format!("-----BEGIN {}-----", label);
    let end = alloc::format!("-----END {}-----", label);
    let mut rest = text;
    while let Some(i) = rest.find(&begin) {
        let after = &rest[i + begin.len()..];
        match after.find(&end) {
            Some(j) => {
                match base64_decode(&after[..j]) {
                    Some(der) => out.push(der),
                    None => bad += 1,
                }
                rest = &after[j + end.len()..];
            }
            None => {
                bad += 1;
                break;
            }
        }
    }
    (out, bad)
}
