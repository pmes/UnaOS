//! URL Standard §3 hosts: the host parser (§3.5) with its IPv4, IPv6 and opaque-host parsers, the "ends in a
//! number" checker, and the host serializer (§3.6). Domains go through [`super::idna::domain_to_ascii`].

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::idna;
use super::percent::{hexval, percent_decode, percent_encode, EncodeSet};
use super::UrlError;

/// §3.1 a host: a domain, an IPv4 address, an IPv6 address, an opaque host, or an empty host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
    Domain(String),
    Ipv4(u32),
    Ipv6([u16; 8]),
    Opaque(String),
    Empty,
}

impl fmt::Display for Host {
    /// §3.6 host serializer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Host::Domain(s) | Host::Opaque(s) => f.write_str(s),
            Host::Empty => Ok(()),
            Host::Ipv4(a) => write!(f, "{}.{}.{}.{}", a >> 24, (a >> 16) & 255, (a >> 8) & 255, a & 255),
            Host::Ipv6(p) => {
                f.write_str("[")?;
                write_ipv6(f, p)?;
                f.write_str("]")
            }
        }
    }
}

/// §3.6 IPv6 serializer: lowercase hex pieces, the first longest run of two or more zero pieces compressed.
fn write_ipv6(f: &mut fmt::Formatter<'_>, p: &[u16; 8]) -> fmt::Result {
    let (mut best, mut best_len, mut i) = (None, 1usize, 0usize);
    while i < 8 {
        if p[i] == 0 {
            let s = i;
            while i < 8 && p[i] == 0 {
                i += 1;
            }
            if i - s > best_len {
                best = Some(s);
                best_len = i - s;
            }
        } else {
            i += 1;
        }
    }
    let mut ignore0 = false;
    let mut i = 0;
    while i < 8 {
        if ignore0 && p[i] == 0 {
            i += 1;
            continue;
        }
        ignore0 = false;
        if best == Some(i) {
            f.write_str(if i == 0 { "::" } else { ":" })?;
            ignore0 = true;
            i += 1;
            continue;
        }
        write!(f, "{:x}", p[i])?;
        if i != 7 {
            f.write_str(":")?;
        }
        i += 1;
    }
    Ok(())
}

/// §3.2 forbidden host code point.
pub fn forbidden_host(c: char) -> bool {
    matches!(
        c,
        '\0' | '\t' | '\n' | '\r' | ' ' | '#' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|'
    )
}

/// §3.2 forbidden domain code point: a forbidden host code point, a C0 control, `%` or DEL.
pub fn forbidden_domain(c: char) -> bool {
    forbidden_host(c) || (c as u32) <= 0x1F || c == '%' || c == '\u{7F}'
}

/// §3.5 the host parser; `is_opaque` is "not special".
pub fn parse_host(input: &str, is_opaque: bool) -> Result<Host, UrlError> {
    if let Some(rest) = input.strip_prefix('[') {
        let inner = rest.strip_suffix(']').ok_or(UrlError::InvalidHost)?;
        return parse_ipv6(inner).map(Host::Ipv6);
    }
    if is_opaque {
        return parse_opaque_host(input);
    }
    // UTF-8 decode without BOM of the percent-decoded input: an invalid sequence becomes U+FFFD, which UTS #46
    // disallows, so it fails in domain to ASCII as the standard says it must.
    let domain = String::from_utf8_lossy(&percent_decode(input.as_bytes())).into_owned();
    let ascii = idna::domain_to_ascii(&domain, false)?;
    if ends_in_number(&ascii) {
        return parse_ipv4(&ascii).map(Host::Ipv4);
    }
    Ok(Host::Domain(ascii))
}

/// §3.5 opaque-host parser.
pub fn parse_opaque_host(input: &str) -> Result<Host, UrlError> {
    if input.chars().any(forbidden_host) {
        return Err(UrlError::InvalidHost);
    }
    Ok(Host::Opaque(percent_encode(input, EncodeSet::C0Control)))
}

/// §3.5 the "ends in a number" checker.
pub fn ends_in_number(s: &str) -> bool {
    let mut parts: Vec<&str> = s.split('.').collect();
    if parts.last() == Some(&"") {
        if parts.len() == 1 {
            return false;
        }
        parts.pop();
    }
    let last = parts.last().copied().unwrap_or("");
    if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    ipv4_number(last).is_some()
}

/// §3.5 IPv4 number parser (the validation-error flag is not reported).
fn ipv4_number(mut s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let mut radix = 10u32;
    if s.len() >= 2 && (s.starts_with("0x") || s.starts_with("0X")) {
        radix = 16;
        s = &s[2..];
    } else if s.len() >= 2 && s.starts_with('0') {
        radix = 8;
        s = &s[1..];
    }
    if s.is_empty() {
        return Some(0);
    }
    let mut v: u64 = 0;
    for b in s.bytes() {
        let d = match hexval(b) {
            Some(d) if (d as u32) < radix => d as u64,
            _ => return None,
        };
        v = v.saturating_mul(radix as u64).saturating_add(d);
    }
    Some(v)
}

/// §3.5 IPv4 parser.
pub fn parse_ipv4(s: &str) -> Result<u32, UrlError> {
    let mut parts: Vec<&str> = s.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    if parts.len() > 4 {
        return Err(UrlError::InvalidHost);
    }
    let mut nums = Vec::with_capacity(4);
    for p in &parts {
        nums.push(ipv4_number(p).ok_or(UrlError::InvalidHost)?);
    }
    let n = nums.len();
    if nums[..n - 1].iter().any(|&v| v > 255) {
        return Err(UrlError::InvalidHost);
    }
    if nums[n - 1] >= 256u64.pow((5 - n) as u32) {
        return Err(UrlError::InvalidHost);
    }
    let mut ip = nums[n - 1];
    for (i, v) in nums[..n - 1].iter().enumerate() {
        ip += v * 256u64.pow((3 - i) as u32);
    }
    Ok(ip as u32)
}

/// §3.5 IPv6 parser.
pub fn parse_ipv6(input: &str) -> Result<[u16; 8], UrlError> {
    let s: Vec<char> = input.chars().collect();
    let mut addr = [0u16; 8];
    let mut piece = 0usize;
    let mut compress: Option<usize> = None;
    let mut p = 0usize;
    let at = |i: usize| s.get(i).copied();
    let bad = Err(UrlError::InvalidHost);
    if at(p) == Some(':') {
        if at(p + 1) != Some(':') {
            return bad;
        }
        p += 2;
        piece += 1;
        compress = Some(piece);
    }
    while at(p).is_some() {
        if piece == 8 {
            return bad;
        }
        if at(p) == Some(':') {
            if compress.is_some() {
                return bad;
            }
            p += 1;
            piece += 1;
            compress = Some(piece);
            continue;
        }
        let (mut value, mut length) = (0u32, 0);
        while length < 4 {
            match at(p).and_then(|c| if c.is_ascii() { hexval(c as u8) } else { None }) {
                Some(d) => {
                    value = value * 16 + d as u32;
                    p += 1;
                    length += 1;
                }
                None => break,
            }
        }
        if at(p) == Some('.') {
            if length == 0 {
                return bad;
            }
            p -= length;
            if piece > 6 {
                return bad;
            }
            let mut numbers_seen = 0;
            while at(p).is_some() {
                let mut ipv4_piece: Option<u32> = None;
                if numbers_seen > 0 {
                    if at(p) == Some('.') && numbers_seen < 4 {
                        p += 1;
                    } else {
                        return bad;
                    }
                }
                match at(p) {
                    Some(c) if c.is_ascii_digit() => {}
                    _ => return bad,
                }
                while let Some(c) = at(p).filter(|c| c.is_ascii_digit()) {
                    let n = c as u32 - '0' as u32;
                    match ipv4_piece {
                        None => ipv4_piece = Some(n),
                        Some(0) => return bad,
                        Some(v) => ipv4_piece = Some(v * 10 + n),
                    }
                    if ipv4_piece.unwrap_or(0) > 255 {
                        return bad;
                    }
                    p += 1;
                }
                addr[piece] = (addr[piece] as u32 * 0x100 + ipv4_piece.unwrap_or(0)) as u16;
                numbers_seen += 1;
                if numbers_seen == 2 || numbers_seen == 4 {
                    piece += 1;
                }
            }
            if numbers_seen != 4 {
                return bad;
            }
            break;
        } else if at(p) == Some(':') {
            p += 1;
            if at(p).is_none() {
                return bad;
            }
        } else if at(p).is_some() {
            return bad;
        }
        addr[piece] = value as u16;
        piece += 1;
    }
    if let Some(c) = compress {
        let mut swaps = piece - c;
        piece = 7;
        while piece != 0 && swaps > 0 {
            addr.swap(piece, c + swaps - 1);
            piece -= 1;
            swaps -= 1;
        }
    } else if piece != 8 {
        return bad;
    }
    Ok(addr)
}
