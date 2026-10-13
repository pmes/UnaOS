//! Subresource Integrity (W3C SRI §3): `integrity="sha384-… sha512-…"` on `<script>` / `<link>` and the
//! `integrity` member of a `fetch()` Request. Digests are UnaOS's own (crypto_core SHA-2, SR27).
//!
//! §3.3.3 parse metadata: whitespace-separated `alg-base64[?options]`; unknown algorithms are ignored, and
//! metadata with no recognised item means "no metadata" (the resource passes). §3.3.4/§3.3.5: only the
//! STRONGEST algorithm present counts; the resource matches when its digest equals any item of that
//! algorithm. A response that is not eligible for integrity (an opaque, cross-origin `no-cors` answer) fails
//! whenever there is metadata (§3.3.6 step 3).

use base64::Engine as _;

/// A recognised hash algorithm, ordered by strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Alg {
    Sha256,
    Sha384,
    Sha512,
}

impl Alg {
    fn parse(s: &str) -> Option<Alg> {
        match s.to_ascii_lowercase().as_str() {
            "sha256" => Some(Alg::Sha256),
            "sha384" => Some(Alg::Sha384),
            "sha512" => Some(Alg::Sha512),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Alg::Sha256 => "sha256",
            Alg::Sha384 => "sha384",
            Alg::Sha512 => "sha512",
        }
    }

    /// The digest of `bytes`, base64-encoded (standard alphabet, padded).
    pub fn digest_b64(self, bytes: &[u8]) -> String {
        let e = &base64::engine::general_purpose::STANDARD;
        match self {
            Alg::Sha256 => e.encode(crypto_core::sha2::sha256(bytes)),
            Alg::Sha384 => e.encode(crypto_core::sha2::sha384(bytes)),
            Alg::Sha512 => e.encode(crypto_core::sha2::sha512(bytes)),
        }
    }
}

/// Normalises a base64 / base64url digest for comparison (CSP3 and SRI accept both alphabets).
pub fn normalize_b64(s: &str) -> String {
    s.trim_end_matches('=').replace('-', "+").replace('_', "/")
}

/// §3.3.3: the recognised (algorithm, base64 digest) items of `metadata`.
pub fn parse(metadata: &str) -> Vec<(Alg, String)> {
    metadata
        .split_ascii_whitespace()
        .filter_map(|item| {
            let item = item.split('?').next().unwrap_or(item);
            let (alg, digest) = item.split_once('-')?;
            Some((Alg::parse(alg)?, digest.to_string()))
        })
        .collect()
}

/// §3.3.5 "do bytes match metadataList": `Ok` when there is no recognised metadata or a strongest-algorithm
/// item matches; `Err(message)` otherwise. `eligible` is false for an opaque response.
pub fn check(metadata: &str, bytes: &[u8], eligible: bool) -> Result<(), String> {
    let items = parse(metadata);
    let Some(strongest) = items.iter().map(|(a, _)| *a).max() else { return Ok(()) };
    if !eligible {
        return Err("the resource is not eligible for integrity checks (an opaque cross-origin response: add crossorigin / CORS)".into());
    }
    let got = strongest.digest_b64(bytes);
    let got_n = normalize_b64(&got);
    if items.iter().any(|(a, d)| *a == strongest && normalize_b64(d) == got_n) {
        return Ok(());
    }
    Err(format!("none of the {} hashes in the integrity attribute match the content; the computed hash is '{}-{}'", strongest.name(), strongest.name(), got))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strongest_wins() {
        let body = b"alert('Hello, world.');";
        let s384 = Alg::Sha384.digest_b64(body);
        let s256 = Alg::Sha256.digest_b64(body);
        assert!(check(&format!("sha384-{s384}"), body, true).is_ok());
        assert!(check(&format!("sha256-{s256} sha384-bogus"), body, true).is_err());
        assert!(check(&format!("sha256-{s256} md5-xxx"), body, true).is_ok());
        assert!(check("md5-xxx", body, false).is_ok());
        assert!(check(&format!("sha256-{s256}"), body, false).is_err());
        assert!(check(&format!("sha384-{s384}?ct=x"), body, true).is_ok());
        // The W3C example: sha256 of "alert('Hello, world.');".
        assert_eq!(s256, "qznLcsROx4GACP2dm0UCKCzCG+HiZ1guq6ZZDob/Tng=");
    }
}
