//! `data:` URLs (RFC 2397) for `<image>`: media type, `;base64` (RFC 4648 §4, whitespace ignored) or
//! percent-encoded payloads. External references are not fetched — an SVG used as an image has no network
//! or file access (the same rule browsers apply to SVG in `<img>`).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = |c: u8| (c as char).to_digit(16);
            if let (Some(a), Some(c)) = (h(b[i + 1]), h(b[i + 2])) {
                out.push((a * 16 + c) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `(media type, bytes)` of a `data:` URL.
pub fn parse_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let u = url.trim();
    let rest = u.strip_prefix("data:").or_else(|| if u.len() > 5 && u[..5].eq_ignore_ascii_case("data:") { Some(&u[5..]) } else { None })?;
    let comma = rest.find(',')?;
    let meta = &rest[..comma];
    let payload = &rest[comma + 1..];
    let mut parts = meta.split(';');
    let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let b64 = meta.split(';').any(|p| p.trim().eq_ignore_ascii_case("base64"));
    let data = if b64 { base64_decode(&String::from_utf8(percent_decode(payload)).ok()?)? } else { percent_decode(payload) };
    Some((if mime.is_empty() { "text/plain".to_string() } else { mime }, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_urls() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVs\nbG8").unwrap(), b"hello");
        let (m, d) = parse_data_url("data:image/png;base64,iVBORw==").unwrap();
        assert_eq!(m, "image/png");
        assert_eq!(d, [0x89, b'P', b'N', b'G']);
        let (m, d) = parse_data_url("data:image/svg+xml;utf8,%3Csvg%3E").unwrap();
        assert_eq!(m, "image/svg+xml");
        assert_eq!(d, b"<svg>");
        assert!(parse_data_url("image.png").is_none());
    }
}
