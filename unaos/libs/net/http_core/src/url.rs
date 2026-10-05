//! The WHATWG URL Standard (url.spec.whatwg.org) — the basic URL parser (§4.4), the host parser (§3.5)
//! with IPv4 (§3.5 "IPv4 parser") and IPv6 (§3.5 "IPv6 parser") hosts, the serializers (§3.6, §4.5), the
//! percent-encode sets (§1.3) and origins (§4.7). Written from the standard's state machine, state by state;
//! the state names below are the standard's.
//!
//! IDNA: the standard's "domain to ASCII" runs UTS #46 processing. The ASCII fast path is exact. For a
//! non-ASCII label this module does the mapping step for the common cases (Unicode lowercasing, fullwidth
//! ASCII, the four IDNA dot separators, default-ignorable code points), refuses the obvious disallowed code
//! points, and Punycode-encodes (RFC 3492) — the full UTS #46 mapping table, NFC and the Bidi/ContextJ rules
//! are OWED (the WPT pass count says how far that reaches).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// A parse failure (the standard's "failure"; validation errors that are not fatal are not reported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// No scheme and no base (or a base with an opaque path).
    MissingSchemeNonRelative,
    /// `https://` with no host, `http://user@/`, an empty host in a special URL.
    HostMissing,
    /// A host that failed the host parser (forbidden code point, bad IPv4/IPv6, IDNA failure).
    InvalidHost,
    /// A port that is not decimal or exceeds 65535.
    InvalidPort,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UrlError::MissingSchemeNonRelative => "relative URL without a base",
            UrlError::HostMissing => "missing host",
            UrlError::InvalidHost => "invalid host",
            UrlError::InvalidPort => "invalid port",
        })
    }
}

/// §3.1 hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
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

fn write_ipv6(f: &mut fmt::Formatter<'_>, p: &[u16; 8]) -> fmt::Result {
    // The first longest run of two or more zero pieces is compressed.
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

/// A URL's path: an opaque string (`mailto:x`) or a list of segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlPath {
    Opaque(String),
    List(Vec<String>),
}

/// §4.1 a URL record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    scheme: String,
    username: String,
    password: String,
    host: Option<Host>,
    port: Option<u16>,
    path: UrlPath,
    query: Option<String>,
    fragment: Option<String>,
}

/// §4.2 special schemes and their default ports.
pub fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "ftp" => Some(21),
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

pub fn is_special(scheme: &str) -> bool {
    matches!(scheme, "ftp" | "file" | "http" | "https" | "ws" | "wss")
}

// ---------------------------------------------------------------- §1.3 percent-encoding

/// The percent-encode sets of §1.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeSet {
    C0Control,
    Fragment,
    Query,
    SpecialQuery,
    Path,
    Userinfo,
    Component,
    FormUrlencoded,
}

fn in_set(set: EncodeSet, b: u8) -> bool {
    let c0 = b < 0x20 || b > 0x7E;
    match set {
        EncodeSet::C0Control => c0,
        EncodeSet::Fragment => c0 || matches!(b, b' ' | b'"' | b'<' | b'>' | b'`'),
        EncodeSet::Query => c0 || matches!(b, b' ' | b'"' | b'#' | b'<' | b'>'),
        EncodeSet::SpecialQuery => in_set(EncodeSet::Query, b) || b == b'\'',
        EncodeSet::Path => in_set(EncodeSet::Query, b) || matches!(b, b'?' | b'^' | b'`' | b'{' | b'}'),
        EncodeSet::Userinfo => {
            in_set(EncodeSet::Path, b) || matches!(b, b'/' | b':' | b';' | b'=' | b'@' | b'[' | b'\\' | b']' | b'|')
        }
        EncodeSet::Component => in_set(EncodeSet::Userinfo, b) || matches!(b, b'$' | b'%' | b'&' | b'+' | b','),
        EncodeSet::FormUrlencoded => in_set(EncodeSet::Component, b) || matches!(b, b'!' | b'\'' | b'(' | b')' | b'~'),
    }
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// UTF-8 percent-encode `s` with `set`, appending to `out`.
pub fn percent_encode_into(out: &mut String, s: &str, set: EncodeSet) {
    for &b in s.as_bytes() {
        if in_set(set, b) {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        } else {
            out.push(b as char);
        }
    }
}

pub fn percent_encode(s: &str, set: EncodeSet) -> String {
    let mut o = String::with_capacity(s.len());
    percent_encode_into(&mut o, s, set);
    o
}

fn push_char_encoded(out: &mut String, c: char, set: EncodeSet) {
    let mut b = [0u8; 4];
    percent_encode_into(out, c.encode_utf8(&mut b), set);
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// §1.3 percent-decode (bytes).
pub fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(h), Some(l)) = (hexval(input[i + 1]), hexval(input[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------- §3.5 host parsing

fn forbidden_host(c: char) -> bool {
    matches!(
        c,
        '\0' | '\t' | '\n' | '\r' | ' ' | '#' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|'
    )
}

fn forbidden_domain(c: char) -> bool {
    forbidden_host(c) || (c as u32) <= 0x1F || c == '%' || c == '\u{7F}'
}

/// The host parser, with `is_opaque` = "not special".
pub fn parse_host(input: &str, is_opaque: bool) -> Result<Host, UrlError> {
    if let Some(rest) = input.strip_prefix('[') {
        let inner = rest.strip_suffix(']').ok_or(UrlError::InvalidHost)?;
        return parse_ipv6(inner).map(Host::Ipv6);
    }
    if is_opaque {
        if input.chars().any(forbidden_host) {
            return Err(UrlError::InvalidHost);
        }
        return Ok(Host::Opaque(percent_encode(input, EncodeSet::C0Control)));
    }
    let decoded = percent_decode(input.as_bytes());
    let domain = String::from_utf8_lossy(&decoded).into_owned();
    let ascii = domain_to_ascii(&domain)?;
    if ends_in_number(&ascii) {
        return parse_ipv4(&ascii).map(Host::Ipv4);
    }
    Ok(Host::Domain(ascii))
}

/// §3.5 "ends in a number checker".
fn ends_in_number(s: &str) -> bool {
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

/// §3.5 IPv4 number parser: `Some((value, validation_error))`.
fn ipv4_number(mut s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let mut radix = 10;
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
    for v in &nums[..n - 1] {
        if *v > 255 {
            return Err(UrlError::InvalidHost);
        }
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
                    if ipv4_piece.unwrap() > 255 {
                        return bad;
                    }
                    p += 1;
                }
                addr[piece] = (addr[piece] as u32 * 0x100 + ipv4_piece.unwrap()) as u16;
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

// ---------------------------------------------------------------- §3.3 IDNA (domain to ASCII)

/// UTS #46 mapping for the cases this module covers; `Err` = disallowed.
fn uts46_map(c: char, out: &mut String) -> Result<(), UrlError> {
    let u = c as u32;
    match u {
        // Default-ignorable / "ignored" in the UTS #46 table: soft hyphen, ZWSP-family joiners in
        // non-transitional processing are kept (ZWJ/ZWNJ need ContextJ, owed), variation selectors, BOM.
        0x00AD | 0x034F | 0x180B..=0x180F | 0x200B | 0x2060 | 0xFE00..=0xFE0F | 0xFEFF => Ok(()),
        // IDNA dot separators.
        0x3002 | 0xFF0E | 0xFF61 => {
            out.push('.');
            Ok(())
        }
        // Fullwidth ASCII.
        0xFF01..=0xFF5E => {
            let a = char::from_u32(u - 0xFEE0).unwrap();
            for l in a.to_lowercase() {
                out.push(l);
            }
            Ok(())
        }
        // Mapped to SPACE in UTS #46 (disallowed_STD3_mapped) — a space is a forbidden domain code point.
        0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000 => {
            out.push(' ');
            Ok(())
        }
        // Mathematical alphanumeric letters and digits map to ASCII (the Latin letter runs of U+1D400 block).
        0x1D400..=0x1D6A3 => {
            let off = (u - 0x1D400) % 52;
            out.push((b'a' + (off % 26) as u8) as char);
            Ok(())
        }
        0x1D7CE..=0x1D7FF => {
            out.push((b'0' + ((u - 0x1D7CE) % 10) as u8) as char);
            Ok(())
        }
        // Noncharacters.
        0xFDD0..=0xFDEF => Err(UrlError::InvalidHost),
        _ if u & 0xFFFE == 0xFFFE => Err(UrlError::InvalidHost),
        // Disallowed: replacement character, private use, unassigned noncharacters, C1 controls, line/para separators.
        0xFFFD | 0xFFFE | 0xFFFF | 0x80..=0x9F | 0x2028 | 0x2029 | 0xE000..=0xF8FF => Err(UrlError::InvalidHost),
        _ => {
            for l in c.to_lowercase() {
                out.push(l);
            }
            Ok(())
        }
    }
}

/// §3.3 domain to ASCII (beStrict = false).
pub fn domain_to_ascii(domain: &str) -> Result<String, UrlError> {
    let fast = domain.is_ascii()
        && !domain.split('.').any(|l| l.len() >= 4 && l[..4].eq_ignore_ascii_case("xn--"));
    let result = if fast {
        domain.to_ascii_lowercase()
    } else {
        let mut mapped = String::with_capacity(domain.len());
        for c in domain.chars() {
            uts46_map(c, &mut mapped)?;
        }
        let mut out = String::with_capacity(mapped.len());
        for (i, label) in mapped.split('.').enumerate() {
            if i > 0 {
                out.push('.');
            }
            if label.len() >= 4 && label.is_char_boundary(4) && label[..4].eq_ignore_ascii_case("xn--") {
                if !label.is_ascii() {
                    return Err(UrlError::InvalidHost);
                }
                let decoded = punycode_decode(&label[4..]).ok_or(UrlError::InvalidHost)?;
                if !decoded.is_empty() && decoded.is_ascii() {
                    return Err(UrlError::InvalidHost);
                }
                // The decoded label must itself be valid: lowercase-stable and free of disallowed code points.
                let mut re = String::new();
                for c in decoded.chars() {
                    uts46_map(c, &mut re)?;
                }
                if re != decoded || decoded.chars().any(|c| c == '.' || forbidden_domain(c)) {
                    return Err(UrlError::InvalidHost);
                }
                out.push_str(&label.to_ascii_lowercase());
            } else if label.is_ascii() {
                out.push_str(label);
            } else {
                out.push_str("xn--");
                out.push_str(&punycode_encode(label).ok_or(UrlError::InvalidHost)?);
            }
        }
        out
    };
    if result.is_empty() || result.chars().any(forbidden_domain) {
        return Err(UrlError::InvalidHost);
    }
    Ok(result)
}

// RFC 3492 Punycode.
const BASE: u32 = 36;
const TMIN: u32 = 1;
const TMAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;

fn adapt(mut delta: u32, numpoints: u32, first: bool) -> u32 {
    delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / numpoints;
    let mut k = 0;
    while delta > ((BASE - TMIN) * TMAX) / 2 {
        delta /= BASE - TMIN;
        k += BASE;
    }
    k + (((BASE - TMIN + 1) * delta) / (delta + SKEW))
}

fn digit(d: u32) -> char {
    (if d < 26 { b'a' + d as u8 } else { b'0' + (d - 26) as u8 }) as char
}

/// RFC 3492 §6.3 encoding.
pub fn punycode_encode(input: &str) -> Option<String> {
    let cps: Vec<u32> = input.chars().map(|c| c as u32).collect();
    let mut out: String = input.chars().filter(|c| c.is_ascii()).collect();
    let b = out.len() as u32;
    let mut h = b;
    if b > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    while (h as usize) < cps.len() {
        let m = *cps.iter().filter(|&&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(h + 1)?)?;
        n = m;
        for &c in &cps {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, h + 1, h == b);
                delta = 0;
                h += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Some(out)
}

/// RFC 3492 §6.2 decoding.
pub fn punycode_decode(input: &str) -> Option<String> {
    let (basic, rest) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    if !basic.is_ascii() {
        return None;
    }
    let mut out: Vec<u32> = basic.chars().map(|c| c as u32).collect();
    let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let bytes = rest.as_bytes();
    let mut p = 0;
    while p < bytes.len() {
        let oldi = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let c = *bytes.get(p)?;
            p += 1;
            let d = match c {
                b'a'..=b'z' => c - b'a',
                b'A'..=b'Z' => c - b'A',
                b'0'..=b'9' => c - b'0' + 26,
                _ => return None,
            } as u32;
            i = i.checked_add(d.checked_mul(w)?)?;
            let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
            if d < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = out.len() as u32 + 1;
        bias = adapt(i - oldi, len, oldi == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        if n < 0x80 {
            return None;
        }
        out.insert(i as usize, n);
        i += 1;
    }
    out.into_iter().map(char::from_u32).collect()
}

// ---------------------------------------------------------------- §4.4 the basic URL parser

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    SchemeStart,
    Scheme,
    NoScheme,
    SpecialRelativeOrAuthority,
    PathOrAuthority,
    Relative,
    RelativeSlash,
    SpecialAuthoritySlashes,
    SpecialAuthorityIgnoreSlashes,
    Authority,
    Host,
    Port,
    File,
    FileSlash,
    FileHost,
    PathStart,
    Path,
    OpaquePath,
    Query,
    Fragment,
}

fn is_wdl(a: char, b: char) -> bool {
    a.is_ascii_alphabetic() && (b == ':' || b == '|')
}

fn is_windows_drive_letter(s: &str, normalized: bool) -> bool {
    let c: Vec<char> = s.chars().collect();
    c.len() == 2 && c[0].is_ascii_alphabetic() && (c[1] == ':' || (!normalized && c[1] == '|'))
}

fn starts_with_wdl(c: &[char]) -> bool {
    c.len() >= 2 && is_wdl(c[0], c[1]) && (c.len() == 2 || matches!(c[2], '/' | '\\' | '?' | '#'))
}

fn single_dot(s: &str) -> bool {
    s == "." || s.eq_ignore_ascii_case("%2e")
}

fn double_dot(s: &str) -> bool {
    matches!(s, "..") || s.eq_ignore_ascii_case(".%2e") || s.eq_ignore_ascii_case("%2e.") || s.eq_ignore_ascii_case("%2e%2e")
}

impl Url {
    /// Parse `input` with no base.
    pub fn parse(input: &str) -> Result<Url, UrlError> {
        Self::parse_with_base(input, None)
    }

    /// Join `input` against this URL (the standard's "parse with base").
    pub fn join(&self, input: &str) -> Result<Url, UrlError> {
        Self::parse_with_base(input, Some(self))
    }

    /// The basic URL parser (no state override, UTF-8 encoding).
    pub fn parse_with_base(input: &str, base: Option<&Url>) -> Result<Url, UrlError> {
        let trimmed = input.trim_matches(|c: char| (c as u32) <= 0x20);
        let chars: Vec<char> = trimmed.chars().filter(|&c| c != '\t' && c != '\n' && c != '\r').collect();
        let mut url = Url {
            scheme: String::new(),
            username: String::new(),
            password: String::new(),
            host: None,
            port: None,
            path: UrlPath::List(Vec::new()),
            query: None,
            fragment: None,
        };
        let mut state = State::SchemeStart;
        let mut buffer = String::new();
        let (mut at_sign_seen, mut inside_brackets, mut password_token_seen) = (false, false, false);
        let mut p: isize = 0;
        loop {
            let c: Option<char> = if p >= 0 { chars.get(p as usize).copied() } else { None };
            let rest = |p: isize| -> &[char] {
                let i = (p + 1).max(0) as usize;
                if i <= chars.len() { &chars[i..] } else { &[] }
            };
            let special = is_special(&url.scheme);
            match state {
                State::SchemeStart => match c {
                    Some(ch) if ch.is_ascii_alphabetic() => {
                        buffer.push(ch.to_ascii_lowercase());
                        state = State::Scheme;
                    }
                    _ => {
                        state = State::NoScheme;
                        p -= 1;
                    }
                },
                State::Scheme => match c {
                    Some(ch) if ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.') => {
                        buffer.push(ch.to_ascii_lowercase())
                    }
                    Some(':') => {
                        url.scheme = core::mem::take(&mut buffer);
                        if url.scheme == "file" {
                            state = State::File;
                        } else if is_special(&url.scheme) && base.is_some_and(|b| b.scheme == url.scheme) {
                            state = State::SpecialRelativeOrAuthority;
                        } else if is_special(&url.scheme) {
                            state = State::SpecialAuthoritySlashes;
                        } else if rest(p).first() == Some(&'/') {
                            state = State::PathOrAuthority;
                            p += 1;
                        } else {
                            url.path = UrlPath::Opaque(String::new());
                            state = State::OpaquePath;
                        }
                    }
                    _ => {
                        buffer.clear();
                        state = State::NoScheme;
                        p = -1;
                    }
                },
                State::NoScheme => {
                    let b = match base {
                        None => return Err(UrlError::MissingSchemeNonRelative),
                        Some(b) => b,
                    };
                    let opaque = matches!(b.path, UrlPath::Opaque(_));
                    if opaque && c != Some('#') {
                        return Err(UrlError::MissingSchemeNonRelative);
                    } else if opaque {
                        url.scheme = b.scheme.clone();
                        url.path = b.path.clone();
                        url.query = b.query.clone();
                        url.fragment = Some(String::new());
                        state = State::Fragment;
                    } else if b.scheme != "file" {
                        state = State::Relative;
                        p -= 1;
                    } else {
                        state = State::File;
                        p -= 1;
                    }
                }
                State::SpecialRelativeOrAuthority => {
                    if c == Some('/') && rest(p).first() == Some(&'/') {
                        state = State::SpecialAuthorityIgnoreSlashes;
                        p += 1;
                    } else {
                        state = State::Relative;
                        p -= 1;
                    }
                }
                State::PathOrAuthority => {
                    if c == Some('/') {
                        state = State::Authority;
                    } else {
                        state = State::Path;
                        p -= 1;
                    }
                }
                State::Relative => {
                    let b = base.unwrap();
                    url.scheme = b.scheme.clone();
                    let special = is_special(&url.scheme);
                    if c == Some('/') || (special && c == Some('\\')) {
                        state = State::RelativeSlash;
                    } else {
                        url.username = b.username.clone();
                        url.password = b.password.clone();
                        url.host = b.host.clone();
                        url.port = b.port;
                        url.path = b.path.clone();
                        url.query = b.query.clone();
                        match c {
                            Some('?') => {
                                url.query = Some(String::new());
                                state = State::Query;
                            }
                            Some('#') => {
                                url.fragment = Some(String::new());
                                state = State::Fragment;
                            }
                            Some(_) => {
                                url.query = None;
                                url.shorten_path();
                                state = State::Path;
                                p -= 1;
                            }
                            None => {}
                        }
                    }
                }
                State::RelativeSlash => {
                    if special && (c == Some('/') || c == Some('\\')) {
                        state = State::SpecialAuthorityIgnoreSlashes;
                    } else if c == Some('/') {
                        state = State::Authority;
                    } else {
                        let b = base.unwrap();
                        url.username = b.username.clone();
                        url.password = b.password.clone();
                        url.host = b.host.clone();
                        url.port = b.port;
                        state = State::Path;
                        p -= 1;
                    }
                }
                State::SpecialAuthoritySlashes => {
                    if c == Some('/') && rest(p).first() == Some(&'/') {
                        state = State::SpecialAuthorityIgnoreSlashes;
                        p += 1;
                    } else {
                        state = State::SpecialAuthorityIgnoreSlashes;
                        p -= 1;
                    }
                }
                State::SpecialAuthorityIgnoreSlashes => {
                    if c != Some('/') && c != Some('\\') {
                        state = State::Authority;
                        p -= 1;
                    }
                }
                State::Authority => {
                    if c == Some('@') {
                        if at_sign_seen {
                            buffer.insert_str(0, "%40");
                        }
                        at_sign_seen = true;
                        for cp in buffer.chars() {
                            if cp == ':' && !password_token_seen {
                                password_token_seen = true;
                                continue;
                            }
                            let target = if password_token_seen { &mut url.password } else { &mut url.username };
                            push_char_encoded(target, cp, EncodeSet::Userinfo);
                        }
                        buffer.clear();
                    } else if matches!(c, None | Some('/') | Some('?') | Some('#')) || (special && c == Some('\\')) {
                        if at_sign_seen && buffer.is_empty() {
                            return Err(UrlError::HostMissing);
                        }
                        p -= buffer.chars().count() as isize + 1;
                        buffer.clear();
                        state = State::Host;
                    } else {
                        buffer.push(c.unwrap());
                    }
                }
                State::Host => {
                    if c == Some(':') && !inside_brackets {
                        if buffer.is_empty() {
                            return Err(UrlError::HostMissing);
                        }
                        url.host = Some(parse_host(&buffer, !special)?);
                        buffer.clear();
                        state = State::Port;
                    } else if matches!(c, None | Some('/') | Some('?') | Some('#')) || (special && c == Some('\\')) {
                        p -= 1;
                        if special && buffer.is_empty() {
                            return Err(UrlError::HostMissing);
                        }
                        url.host = Some(if buffer.is_empty() { Host::Empty } else { parse_host(&buffer, !special)? });
                        buffer.clear();
                        state = State::PathStart;
                    } else {
                        let ch = c.unwrap();
                        if ch == '[' {
                            inside_brackets = true;
                        }
                        if ch == ']' {
                            inside_brackets = false;
                        }
                        buffer.push(ch);
                    }
                }
                State::Port => match c {
                    Some(ch) if ch.is_ascii_digit() => buffer.push(ch),
                    _ if matches!(c, None | Some('/') | Some('?') | Some('#')) || (special && c == Some('\\')) => {
                        if !buffer.is_empty() {
                            let mut port: u32 = 0;
                            for b in buffer.bytes() {
                                port = port * 10 + (b - b'0') as u32;
                                if port > 65535 {
                                    return Err(UrlError::InvalidPort);
                                }
                            }
                            url.port = if default_port(&url.scheme) == Some(port as u16) { None } else { Some(port as u16) };
                            buffer.clear();
                        }
                        state = State::PathStart;
                        p -= 1;
                    }
                    _ => return Err(UrlError::InvalidPort),
                },
                State::File => {
                    url.scheme = "file".to_string();
                    url.host = Some(Host::Empty);
                    if c == Some('/') || c == Some('\\') {
                        state = State::FileSlash;
                    } else if let Some(b) = base.filter(|b| b.scheme == "file") {
                        url.host = b.host.clone();
                        url.path = b.path.clone();
                        url.query = b.query.clone();
                        match c {
                            Some('?') => {
                                url.query = Some(String::new());
                                state = State::Query;
                            }
                            Some('#') => {
                                url.fragment = Some(String::new());
                                state = State::Fragment;
                            }
                            Some(_) => {
                                url.query = None;
                                if !starts_with_wdl(&chars[p as usize..]) {
                                    url.shorten_path();
                                } else {
                                    url.path = UrlPath::List(Vec::new());
                                }
                                state = State::Path;
                                p -= 1;
                            }
                            None => {}
                        }
                    } else {
                        state = State::Path;
                        p -= 1;
                    }
                }
                State::FileSlash => {
                    if c == Some('/') || c == Some('\\') {
                        state = State::FileHost;
                    } else {
                        if let Some(b) = base.filter(|b| b.scheme == "file") {
                            url.host = b.host.clone();
                            let from = if p >= 0 { &chars[(p as usize).min(chars.len())..] } else { &chars[..] };
                            if !starts_with_wdl(from) {
                                if let UrlPath::List(bp) = &b.path {
                                    if let Some(first) = bp.first().filter(|s| is_windows_drive_letter(s, true)) {
                                        url.path_list().push(first.clone());
                                    }
                                }
                            }
                        }
                        state = State::Path;
                        p -= 1;
                    }
                }
                State::FileHost => {
                    if matches!(c, None | Some('/') | Some('\\') | Some('?') | Some('#')) {
                        p -= 1;
                        if is_windows_drive_letter(&buffer, false) {
                            state = State::Path;
                        } else if buffer.is_empty() {
                            url.host = Some(Host::Empty);
                            state = State::PathStart;
                        } else {
                            let mut host = parse_host(&buffer, false)?;
                            if host == Host::Domain("localhost".into()) {
                                host = Host::Empty;
                            }
                            url.host = Some(host);
                            buffer.clear();
                            state = State::PathStart;
                        }
                    } else {
                        buffer.push(c.unwrap());
                    }
                }
                State::PathStart => {
                    if special {
                        state = State::Path;
                        if c != Some('/') && c != Some('\\') {
                            p -= 1;
                        }
                    } else if c == Some('?') {
                        url.query = Some(String::new());
                        state = State::Query;
                    } else if c == Some('#') {
                        url.fragment = Some(String::new());
                        state = State::Fragment;
                    } else if c.is_some() {
                        state = State::Path;
                        if c != Some('/') {
                            p -= 1;
                        }
                    }
                }
                State::Path => {
                    let slashish = c == Some('/') || (special && c == Some('\\'));
                    if c.is_none() || slashish || c == Some('?') || c == Some('#') {
                        if double_dot(&buffer) {
                            url.shorten_path();
                            if !slashish {
                                url.path_list().push(String::new());
                            }
                        } else if single_dot(&buffer) && !slashish {
                            url.path_list().push(String::new());
                        } else if !single_dot(&buffer) {
                            if url.scheme == "file" && url.path_list().is_empty() && is_windows_drive_letter(&buffer, false) {
                                let first = buffer.chars().next().unwrap();
                                buffer = format!("{first}:");
                            }
                            url.path_list().push(core::mem::take(&mut buffer));
                        }
                        buffer.clear();
                        if c == Some('?') {
                            url.query = Some(String::new());
                            state = State::Query;
                        }
                        if c == Some('#') {
                            url.fragment = Some(String::new());
                            state = State::Fragment;
                        }
                    } else {
                        push_char_encoded(&mut buffer, c.unwrap(), EncodeSet::Path);
                    }
                }
                State::OpaquePath => {
                    let UrlPath::Opaque(ref mut s) = url.path else { unreachable!() };
                    match c {
                        Some('?') => {
                            url.query = Some(String::new());
                            state = State::Query;
                        }
                        Some('#') => {
                            url.fragment = Some(String::new());
                            state = State::Fragment;
                        }
                        Some(' ') => {
                            if matches!(rest(p).first(), Some('?') | Some('#')) {
                                s.push_str("%20");
                            } else {
                                s.push(' ');
                            }
                        }
                        Some(ch) => push_char_encoded(s, ch, EncodeSet::C0Control),
                        None => {}
                    }
                }
                State::Query => {
                    if c == Some('#') || c.is_none() {
                        let set = if special { EncodeSet::SpecialQuery } else { EncodeSet::Query };
                        let q = url.query.get_or_insert_with(String::new);
                        percent_encode_into(q, &buffer, set);
                        buffer.clear();
                        if c == Some('#') {
                            url.fragment = Some(String::new());
                            state = State::Fragment;
                        }
                    } else {
                        buffer.push(c.unwrap());
                    }
                }
                State::Fragment => {
                    if let Some(ch) = c {
                        push_char_encoded(url.fragment.get_or_insert_with(String::new), ch, EncodeSet::Fragment);
                    }
                }
            }
            if p >= chars.len() as isize {
                break;
            }
            p += 1;
        }
        Ok(url)
    }

    fn path_list(&mut self) -> &mut Vec<String> {
        match &mut self.path {
            UrlPath::List(v) => v,
            UrlPath::Opaque(_) => unreachable!("list path expected"),
        }
    }

    /// §4.1 shorten a URL's path.
    fn shorten_path(&mut self) {
        let file = self.scheme == "file";
        let v = self.path_list();
        if file && v.len() == 1 && is_windows_drive_letter(&v[0], true) {
            return;
        }
        v.pop();
    }

    // ------------------------------------------------------------ accessors (the URL API's getters)

    pub fn scheme(&self) -> &str {
        &self.scheme
    }
    pub fn username(&self) -> &str {
        &self.username
    }
    pub fn password(&self) -> &str {
        &self.password
    }
    pub fn host(&self) -> Option<&Host> {
        self.host.as_ref()
    }
    /// The port as written (None when absent or the scheme's default).
    pub fn port(&self) -> Option<u16> {
        self.port
    }
    /// The port, or the scheme's default.
    pub fn port_or_default(&self) -> Option<u16> {
        self.port.or_else(|| default_port(&self.scheme))
    }
    pub fn is_special(&self) -> bool {
        is_special(&self.scheme)
    }
    pub fn path(&self) -> &UrlPath {
        &self.path
    }
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
    pub fn set_fragment(&mut self, f: Option<&str>) {
        self.fragment = f.map(|s| percent_encode(s, EncodeSet::Fragment));
    }

    /// `protocol` getter.
    pub fn protocol(&self) -> String {
        format!("{}:", self.scheme)
    }
    /// `hostname` getter.
    pub fn hostname(&self) -> String {
        self.host.as_ref().map(|h| h.to_string()).unwrap_or_default()
    }
    /// `host` getter (hostname[:port]).
    pub fn host_str(&self) -> String {
        match (&self.host, self.port) {
            (None, _) => String::new(),
            (Some(h), None) => h.to_string(),
            (Some(h), Some(p)) => format!("{h}:{p}"),
        }
    }
    /// `pathname` getter (§4.5 URL path serializer).
    pub fn pathname(&self) -> String {
        match &self.path {
            UrlPath::Opaque(s) => s.clone(),
            UrlPath::List(v) => {
                let mut o = String::new();
                for seg in v {
                    o.push('/');
                    o.push_str(seg);
                }
                o
            }
        }
    }
    /// `search` getter.
    pub fn search(&self) -> String {
        match self.query.as_deref() {
            None | Some("") => String::new(),
            Some(q) => format!("?{q}"),
        }
    }
    /// `hash` getter.
    pub fn hash(&self) -> String {
        match self.fragment.as_deref() {
            None | Some("") => String::new(),
            Some(f) => format!("#{f}"),
        }
    }
    /// The request-target of an HTTP request in origin-form (RFC 9112 §3.2.1): path + `?query`.
    pub fn request_target(&self) -> String {
        let mut t = self.pathname();
        if t.is_empty() {
            t.push('/');
        }
        if let Some(q) = &self.query {
            t.push('?');
            t.push_str(q);
        }
        t
    }

    /// §4.5 the URL serializer.
    pub fn serialize(&self, exclude_fragment: bool) -> String {
        let mut o = String::with_capacity(64);
        o.push_str(&self.scheme);
        o.push(':');
        if let Some(h) = &self.host {
            o.push_str("//");
            if !self.username.is_empty() || !self.password.is_empty() {
                o.push_str(&self.username);
                if !self.password.is_empty() {
                    o.push(':');
                    o.push_str(&self.password);
                }
                o.push('@');
            }
            o.push_str(&h.to_string());
            if let Some(p) = self.port {
                o.push(':');
                o.push_str(&p.to_string());
            }
        } else if let UrlPath::List(v) = &self.path {
            if v.len() > 1 && v[0].is_empty() {
                o.push_str("/.");
            }
        }
        o.push_str(&self.pathname());
        if let Some(q) = &self.query {
            o.push('?');
            o.push_str(q);
        }
        if !exclude_fragment {
            if let Some(f) = &self.fragment {
                o.push('#');
                o.push_str(f);
            }
        }
        o
    }

    /// `href`.
    pub fn href(&self) -> String {
        self.serialize(false)
    }

    /// §4.7 origin, serialized (§3.2 of HTML): `scheme://host[:port]`, or `"null"` for an opaque origin.
    pub fn origin(&self) -> String {
        match self.scheme.as_str() {
            "blob" => match Url::parse(&self.pathname()) {
                Ok(u) if u.scheme == "http" || u.scheme == "https" => u.origin(),
                _ => "null".into(),
            },
            "ftp" | "http" | "https" | "ws" | "wss" => {
                format!("{}://{}", self.scheme, self.host_str())
            }
            _ => "null".into(),
        }
    }

    /// Same origin (scheme, host, effective port) — the tuple comparison of HTML §7.1.1.
    pub fn same_origin(&self, other: &Url) -> bool {
        self.is_special()
            && self.scheme != "file"
            && self.scheme == other.scheme
            && self.host == other.host
            && self.port_or_default() == other.port_or_default()
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.href())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3492_samples() {
        // RFC 3492 §7.1 (A) Arabic (Egyptian) and (L) 3<nen>B<gumi><kinpachi><sensei>.
        let arabic = "\u{0644}\u{064A}\u{0647}\u{0645}\u{0627}\u{0628}\u{062A}\u{0643}\u{0644}\u{0645}\u{0648}\u{0634}\u{0639}\u{0631}\u{0628}\u{064A}\u{061F}";
        assert_eq!(punycode_encode(arabic).unwrap(), "egbpdaj6bu4bxfgehfvwxn");
        assert_eq!(punycode_decode("egbpdaj6bu4bxfgehfvwxn").unwrap(), arabic);
        let l = "3\u{5E74}B\u{7D44}\u{91D1}\u{516B}\u{5148}\u{751F}";
        assert_eq!(punycode_encode(l).unwrap(), "3B-ww4c5e180e575a65lsy2b");
        assert_eq!(punycode_decode("3B-ww4c5e180e575a65lsy2b").unwrap(), l);
        assert_eq!(domain_to_ascii("b\u{FC}cher.de").unwrap(), "xn--bcher-kva.de");
    }

    #[test]
    fn basics() {
        let u = Url::parse("HTTPS://User:Pa ss@EXAMPLE.com:443/a/./b/../c?q=1 2#f g").unwrap();
        assert_eq!(u.href(), "https://User:Pa%20ss@example.com/a/c?q=1%202#f%20g");
        assert_eq!(u.origin(), "https://example.com");
        let b = Url::parse("http://a/b/c/d;p?q").unwrap();
        // RFC 3986 §5.4.1 normal examples.
        for (r, want) in [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g/"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("../..", "http://a/"),
            ("../../../g", "http://a/g"),
        ] {
            assert_eq!(b.join(r).unwrap().href(), want, "{r}");
        }
        assert_eq!(Url::parse("http://[::1]:8080/").unwrap().host_str(), "[::1]:8080");
        assert_eq!(Url::parse("http://0x7f.1/").unwrap().hostname(), "127.0.0.1");
        assert!(Url::parse("http://a b/").is_err());
    }
}
