//! Server identity (RFC 6125 §6) and name constraints (RFC 5280 §4.2.1.10): dNSName, iPAddress, rfc822Name,
//! uniformResourceIdentifier and directoryName are enforced; for otherName / x400Address / ediPartyName /
//! registeredID a constraint refuses any certificate below it that carries a name of that type.

use alloc::string::String;
use alloc::vec::Vec;

/// Parses an IPv4 dotted-quad or an IPv6 literal (RFC 4291 §2.2, `::` compression, optional embedded IPv4).
pub fn parse_ip(s: &str) -> Option<Vec<u8>> {
    parse_ipv4(s).map(|a| a.to_vec()).or_else(|| parse_ipv6(s).map(|a| a.to_vec()))
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut n = 0;
    for part in s.split('.') {
        if n == 4 || part.is_empty() || part.len() > 3 || !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        let v: u32 = part.parse().ok()?;
        if v > 255 {
            return None;
        }
        out[n] = v as u8;
        n += 1;
    }
    if n == 4 { Some(out) } else { None }
}

fn parse_ipv6(s: &str) -> Option<[u8; 16]> {
    if !s.contains(':') {
        return None;
    }
    let (head, tail) = match s.find("::") {
        Some(i) => (&s[..i], Some(&s[i + 2..])),
        None => (s, None),
    };
    if tail.map_or(false, |t| t.contains("::")) {
        return None;
    }
    fn groups(part: &str, allow_v4: bool) -> Option<Vec<u16>> {
        let mut v = Vec::new();
        if part.is_empty() {
            return Some(v);
        }
        let pieces: Vec<&str> = part.split(':').collect();
        for (i, p) in pieces.iter().enumerate() {
            if allow_v4 && i == pieces.len() - 1 && p.contains('.') {
                let a = parse_ipv4(p)?;
                v.push(u16::from_be_bytes([a[0], a[1]]));
                v.push(u16::from_be_bytes([a[2], a[3]]));
            } else {
                if p.is_empty() || p.len() > 4 {
                    return None;
                }
                v.push(u16::from_str_radix(p, 16).ok()?);
            }
        }
        Some(v)
    }
    let h = groups(head, tail.is_none())?;
    let t = match tail {
        Some(t) => groups(t, true)?,
        None => Vec::new(),
    };
    let total = h.len() + t.len();
    if (tail.is_none() && total != 8) || (tail.is_some() && total > 7) {
        return None;
    }
    let mut all = h;
    all.resize(8 - t.len(), 0);
    all.extend_from_slice(&t);
    let mut out = [0u8; 16];
    for (i, g) in all.iter().enumerate() {
        out[2 * i..2 * i + 2].copy_from_slice(&g.to_be_bytes());
    }
    Some(out)
}

fn valid_dns_name(s: &str, allow_wildcard: bool) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    for (i, label) in s.split('.').enumerate() {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if allow_wildcard && i == 0 && label == "*" {
            continue;
        }
        if !label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
    }
    true
}

/// RFC 6125 §6.4: does SAN dNSName `pattern` cover `host`? Case-insensitive; a wildcard is honoured only as the
/// whole left-most label (`*.example.com`), matches exactly one label, and needs at least two labels after it.
/// Partial-label wildcards (`f*.example.com`) and wildcards anywhere else never match.
pub fn dns_name_matches(pattern: &str, host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let pattern = pattern.strip_suffix('.').unwrap_or(pattern);
    if !valid_dns_name(host, false) || !valid_dns_name(pattern, true) {
        return false;
    }
    if let Some(rest) = pattern.strip_prefix("*.") {
        if rest.split('.').count() < 2 {
            return false;
        }
        match host.split_once('.') {
            Some((first, host_rest)) => !first.is_empty() && host_rest.eq_ignore_ascii_case(rest),
            None => false,
        }
    } else {
        pattern.eq_ignore_ascii_case(host)
    }
}

/// The leaf's identities from its subjectAltName.
#[derive(Debug, Clone, Default)]
pub struct SubjectAltNames {
    pub present: bool,
    pub dns: Vec<String>,
    pub ip: Vec<Vec<u8>>,
    /// rfc822Name [1]
    pub email: Vec<String>,
    /// uniformResourceIdentifier [6]
    pub uri: Vec<String>,
    /// directoryName [4]: each Name's full DER.
    pub dir: Vec<Vec<u8>>,
    /// Bits ([`general_name_bit`]) of GeneralName types present but not parsed.
    pub other_types: u16,
}

/// A bit per GeneralName CHOICE number (0..=8).
pub const fn general_name_bit(n: u8) -> u16 {
    if n <= 8 { 1 << n } else { 1 << 15 }
}

/// A name-constraint subtree of a type beyond dNSName / iPAddress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subtree {
    Email(String),
    Uri(String),
    /// A directoryName base: the Name's full DER.
    Dir(Vec<u8>),
}

/// RFC 5280 §4.2.1.10 rfc822Name: "user@host" is one mailbox, "host" every mailbox at that host, ".domain" every
/// mailbox at any host below the domain. Host parts compare case-insensitively.
pub fn email_within_subtree(addr: &str, base: &str) -> bool {
    let Some((local, host)) = addr.rsplit_once('@') else { return false };
    if base.contains('@') {
        let Some((bl, bh)) = base.rsplit_once('@') else { return false };
        return local == bl && host.eq_ignore_ascii_case(bh);
    }
    if base.starts_with('.') {
        return host.len() > base.len() && host.to_ascii_lowercase().ends_with(&base.to_ascii_lowercase());
    }
    host.eq_ignore_ascii_case(base)
}

/// The host of a URI (`scheme://[userinfo@]host[:port]…`), or None (no authority, or an IP literal — RFC 5280
/// §4.2.1.10 says URI constraints apply to a fully qualified domain name host).
pub fn uri_host(uri: &str) -> Option<&str> {
    let rest = &uri[uri.find("://")? + 3..];
    let auth = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
    if host.starts_with('[') {
        return None;
    }
    let host = host.split(':').next()?;
    if host.is_empty() || parse_ip(host).is_some() { None } else { Some(host) }
}

/// URI subtree: ".domain" covers hosts below it, "host" exactly that host.
pub fn uri_within_subtree(uri: &str, base: &str) -> bool {
    let Some(host) = uri_host(uri) else { return false };
    let (h, b) = (host.to_ascii_lowercase(), base.to_ascii_lowercase());
    if b.starts_with('.') { h.len() > b.len() && h.ends_with(&b) } else { h == b }
}

/// The RDN encodings of a Name (its SEQUENCE's elements, raw).
fn rdns(name: &[u8]) -> Option<Vec<&[u8]>> {
    use super::der::{tag, Der};
    let mut d = Der::new(name);
    let seq = d.expect(tag::SEQUENCE).ok()?;
    let mut s = Der::new(seq.value);
    let mut v = Vec::new();
    while !s.is_empty() {
        v.push(s.tlv().ok()?.raw);
    }
    Some(v)
}

/// directoryName subtree: `base`'s RDN sequence is a prefix of `name`'s (RDNs compared by encoding — Web PKI CAs
/// encode identically; RFC 5280 §7.1 normalisation is not applied, as for chaining).
pub fn dir_within_subtree(name: &[u8], base: &[u8]) -> bool {
    match (rdns(name), rdns(base)) {
        (Some(n), Some(b)) => b.len() <= n.len() && n.iter().zip(b.iter()).all(|(x, y)| x == y),
        _ => false,
    }
}

/// RFC 6125 §6: a server name (DNS name or IP literal) against the SAN. The subject CN is NOT consulted
/// (RFC 6125 §6.4.4 permits it only without any SAN; CA/B Baseline Requirements make SAN mandatory).
pub fn matches_server_name(san: &SubjectAltNames, server_name: &str) -> bool {
    if let Some(ip) = parse_ip(server_name) {
        return san.ip.iter().any(|a| *a == ip);
    }
    san.dns.iter().any(|p| dns_name_matches(p, server_name))
}

/// RFC 5280 §4.2.1.10 dNSName subtree: `base` "example.com" covers "example.com" and any "*.example.com" name;
/// a base with a leading dot covers only proper subdomains; an empty base covers everything.
pub fn dns_within_subtree(name: &str, base: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let base = base.strip_suffix('.').unwrap_or(base).to_ascii_lowercase();
    if base.is_empty() {
        return true;
    }
    if let Some(b) = base.strip_prefix('.') {
        return name.len() > b.len() && name.ends_with(b) && name.as_bytes()[name.len() - b.len() - 1] == b'.';
    }
    if name == base {
        return true;
    }
    name.len() > base.len() && name.ends_with(&base) && name.as_bytes()[name.len() - base.len() - 1] == b'.'
}

/// iPAddress subtree: `constraint` = address || mask (8 or 32 octets).
pub fn ip_within_subtree(addr: &[u8], constraint: &[u8]) -> bool {
    if constraint.len() != addr.len() * 2 {
        return false;
    }
    let (base, mask) = constraint.split_at(addr.len());
    addr.iter().zip(base).zip(mask).all(|((a, b), m)| a & m == b & m)
}

/// Name constraints we enforce. Subtrees of other GeneralName types are recorded so a critical extension with
/// only unenforceable types can be refused rather than silently ignored.
#[derive(Debug, Clone, Default)]
pub struct NameConstraints {
    pub permitted_dns: Vec<String>,
    pub excluded_dns: Vec<String>,
    pub permitted_ip: Vec<Vec<u8>>,
    pub excluded_ip: Vec<Vec<u8>>,
    /// otherName / x400Address / ediPartyName / registeredID subtrees (counted; see `unenforced_types`).
    pub unenforced: u32,
    pub has_permitted_dns: bool,
    pub has_permitted_ip: bool,
    /// rfc822Name / URI / directoryName subtrees.
    pub permitted_other: Vec<Subtree>,
    pub excluded_other: Vec<Subtree>,
    /// [`general_name_bit`]s of the constraint types we cannot evaluate.
    pub unenforced_types: u16,
}

impl NameConstraints {
    pub fn push(&mut self, t: Subtree, permitted: bool) {
        if permitted { self.permitted_other.push(t) } else { self.excluded_other.push(t) }
    }

    /// RFC 5280 §6.1.3 (b)/(c) for one certificate below the constraining CA: its subject DN (when not empty) and
    /// every SAN name. For each type, a name must lie in some permitted subtree of that type (when any exist) and
    /// in no excluded one.
    pub fn permits(&self, subject: &[u8], san: &SubjectAltNames) -> bool {
        if !self.allows(san) {
            return false;
        }
        if san.other_types & self.unenforced_types != 0 {
            return false;
        }
        fn check<'a>(names: impl Iterator<Item = &'a str>, nc: &NameConstraints, within: fn(&str, &str) -> bool, pick: fn(&Subtree) -> Option<&str>) -> bool {
            let permitted: Vec<&str> = nc.permitted_other.iter().filter_map(pick).collect();
            let excluded: Vec<&str> = nc.excluded_other.iter().filter_map(pick).collect();
            for n in names {
                if excluded.iter().any(|b| within(n, b)) {
                    return false;
                }
                if !permitted.is_empty() && !permitted.iter().any(|b| within(n, b)) {
                    return false;
                }
            }
            true
        }
        fn em(t: &Subtree) -> Option<&str> {
            if let Subtree::Email(s) = t { Some(s.as_str()) } else { None }
        }
        fn ur(t: &Subtree) -> Option<&str> {
            if let Subtree::Uri(s) = t { Some(s.as_str()) } else { None }
        }
        if !check(san.email.iter().map(|s| s.as_str()), self, email_within_subtree, em) {
            return false;
        }
        if !check(san.uri.iter().map(|s| s.as_str()), self, uri_within_subtree, ur) {
            return false;
        }
        let pdir: Vec<&[u8]> = self.permitted_other.iter().filter_map(|t| if let Subtree::Dir(d) = t { Some(d.as_slice()) } else { None }).collect();
        let xdir: Vec<&[u8]> = self.excluded_other.iter().filter_map(|t| if let Subtree::Dir(d) = t { Some(d.as_slice()) } else { None }).collect();
        let empty_subject = rdns(subject).is_none_or(|r| r.is_empty());
        let dirs = san.dir.iter().map(|d| d.as_slice()).chain((!empty_subject).then_some(subject));
        for d in dirs {
            if xdir.iter().any(|b| dir_within_subtree(d, b)) {
                return false;
            }
            if !pdir.is_empty() && !pdir.iter().any(|b| dir_within_subtree(d, b)) {
                return false;
            }
        }
        true
    }

    /// Checks the leaf's SAN identities against these constraints.
    pub fn allows(&self, san: &SubjectAltNames) -> bool {
        for d in &san.dns {
            if self.excluded_dns.iter().any(|b| dns_within_subtree(d, b)) {
                return false;
            }
            if self.has_permitted_dns && !self.permitted_dns.iter().any(|b| dns_within_subtree(d, b)) {
                return false;
            }
        }
        for a in &san.ip {
            if self.excluded_ip.iter().any(|c| ip_within_subtree(a, c)) {
                return false;
            }
            if self.has_permitted_ip && !self.permitted_ip.iter().any(|c| ip_within_subtree(a, c)) {
                return false;
            }
        }
        true
    }
}
