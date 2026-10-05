// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The resolution rules: what Principia says (namespace `vein`) plus the state of the key → which
//! provider answers and whether the key may leave the machine. Pure, so every caller of Vein (Lumen, the
//! installer's diagnosis program, the host) decides the same way.
//!
//! Keys: `vein.provider` (`"claude"` | `"echo"`; unset = claude when it can run), `vein.model`,
//! `vein.endpoint` (a URL; unset = `https://api.anthropic.com/v1/messages`; an `http://` URL is a relay
//! that holds the key itself, so the key is never sent to it), `vein.key_file` (the absolute path of the
//! key file on the UnaFS volume, conventionally
//! `<home>/.config/unaos/vein.key`; a ring-3 program has no way to learn its home, so the path is a
//! preference).
//!
//! THE KEY RULE (VEINTLS, SR36): the key crosses the wire ONLY on a verified TLS connection. There is no
//! `vein.tls` and no insecure mode (the `"insecure"` value LUMENAPP carried while `embedded-tls` checked no
//! certificate is deleted, not deprecated). [`Verify`] says whether this build and boot can verify a
//! server at all (a crypto provider, a trust store, a set clock); when it cannot, Echo answers and the
//! window says which of the three is missing.

use crate::claude;

/// A PREF_GET reply is a TOML scalar literal: a string comes back quoted. Strips one pair of quotes.
pub fn unquote(v: &[u8]) -> &[u8] {
    if v.len() >= 2 && v[0] == b'"' && v[v.len() - 1] == b'"' { &v[1..v.len() - 1] } else { v }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPref {
    Unset,
    Claude,
    Echo,
}

pub fn provider_pref(v: Option<&[u8]>) -> ProviderPref {
    match v.map(unquote) {
        None => ProviderPref::Unset,
        Some(b"claude") => ProviderPref::Claude,
        Some(b"echo") => ProviderPref::Echo,
        Some(_) => ProviderPref::Unset, // an unknown name is not a reason to stop working
    }
}

/// Whether a TLS server can be verified here: the last input to [`plan`]. Anything but `Ready` keeps the
/// key on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    Ready,
    /// The build links no crypto provider able to verify (or it could not be seeded).
    NoProvider,
    /// No trust store (`/system/trust/roots.pem` absent, unreadable or empty).
    NoTrustStore,
    /// The wall clock is not set, so certificate validity cannot be judged.
    NoClock,
}

/// Where the request goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint<'a> {
    pub tls: bool,
    pub host: &'a str,
    pub port: u16,
    pub path: &'a str,
}

pub const DEFAULT_ENDPOINT: Endpoint<'static> = Endpoint { tls: true, host: claude::HOST, port: 443, path: claude::PATH };

/// Parse `http://host[:port][/path]` or `https://…`. A missing path is the Messages path.
pub fn parse_endpoint(url: &str) -> Option<Endpoint<'_>> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return None;
    };
    let (hp, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, claude::PATH),
    };
    let (host, port) = match hp.rfind(':') {
        Some(i) => (&hp[..i], hp[i + 1..].parse::<u16>().ok()?),
        None => (hp, if tls { 443 } else { 80 }),
    };
    if host.is_empty() || port == 0 || !host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-') {
        return None;
    }
    Some(Endpoint { tls, host, port, path })
}

/// The key file's state as the caller found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// Read from a file on the UnaFS volume (the file stats with an inode id).
    UnaFs,
    /// No `vein.key_file`, or no file there.
    None,
    /// The file exists but on FAT (no ids, no owner): refused, never read.
    OnFat,
}

impl KeyState {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyState::UnaFs => "unafs",
            KeyState::None => "none",
            KeyState::OnFat => "fat-refused",
        }
    }
}

/// The trimmed key from the file's bytes: one token of printable ASCII, at most 256 bytes.
pub fn key_from_file(b: &[u8]) -> Option<&str> {
    let t = {
        let mut t = b;
        while let [c, r @ ..] = t {
            if c.is_ascii_whitespace() { t = r } else { break }
        }
        while let [r @ .., c] = t {
            if c.is_ascii_whitespace() { t = r } else { break }
        }
        t
    };
    if t.is_empty() || t.len() > 256 || t.iter().any(|&c| !(0x21..=0x7e).contains(&c)) {
        return None;
    }
    core::str::from_utf8(t).ok()
}

/// Why Echo answers instead of Claude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Chosen,
    NoKey,
    KeyOnFat,
    /// The server could not be verified, so the key is not sent (never "sent anyway").
    TlsUnverified(Verify),
    BadEndpoint,
}

impl Reason {
    /// One line for the window.
    pub fn text(self) -> &'static str {
        match self {
            Reason::Chosen => "vein.provider = \"echo\"",
            Reason::NoKey => "no API key: set vein.key_file to your key file on the UnaFS volume",
            Reason::KeyOnFat => "key refused: the key file is on FAT (no owner, no mode); keep it on UnaFS",
            Reason::TlsUnverified(Verify::NoTrustStore) => "key not sent: no trust store at /system/trust/roots.pem to verify the server (run tools/trust-bundle and rebuild the image), or use an http:// vein.endpoint relay",
            Reason::TlsUnverified(Verify::NoClock) => "key not sent: the clock is not set, so the server's certificate cannot be checked (wait for SNTP or set the date)",
            Reason::TlsUnverified(Verify::NoProvider) => "key not sent: this build has no crypto provider that can verify the server",
            Reason::TlsUnverified(Verify::Ready) => "key not sent",
            Reason::BadEndpoint => "vein.endpoint is not an http:// or https:// URL",
        }
    }
}

/// What runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Talk to the endpoint; `send_key` = put `x-api-key` on the request.
    Claude { send_key: bool },
    Echo(Reason),
}

/// The rule. `verify` = whether this build and boot can check the server's certificate chain.
pub fn plan(p: ProviderPref, ep: Option<&Endpoint<'_>>, key: KeyState, verify: Verify) -> Plan {
    if p == ProviderPref::Echo {
        return Plan::Echo(Reason::Chosen);
    }
    let Some(ep) = ep else { return Plan::Echo(Reason::BadEndpoint) };
    if !ep.tls {
        return Plan::Claude { send_key: false }; // a plain-HTTP relay holds the key; it never crosses the wire
    }
    match key {
        KeyState::None => return Plan::Echo(Reason::NoKey),
        KeyState::OnFat => return Plan::Echo(Reason::KeyOnFat),
        KeyState::UnaFs => {}
    }
    if verify != Verify::Ready {
        return Plan::Echo(Reason::TlsUnverified(verify));
    }
    Plan::Claude { send_key: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_parse() {
        assert_eq!(parse_endpoint("https://api.anthropic.com/v1/messages"), Some(DEFAULT_ENDPOINT));
        assert_eq!(parse_endpoint("http://10.0.0.5:8080"), Some(Endpoint { tls: false, host: "10.0.0.5", port: 8080, path: "/v1/messages" }));
        assert_eq!(parse_endpoint("http://relay/x").unwrap().port, 80);
        assert!(parse_endpoint("ftp://x").is_none());
        assert!(parse_endpoint("http://:80").is_none());
        assert!(parse_endpoint("http://a b").is_none());
    }

    #[test]
    fn the_rules() {
        let https = Some(&DEFAULT_ENDPOINT);
        let relay = parse_endpoint("http://relay:8080").unwrap();
        use KeyState::*;
        use Verify::{NoClock, NoProvider, NoTrustStore, Ready};
        assert_eq!(plan(ProviderPref::Echo, https, UnaFs, Ready), Plan::Echo(Reason::Chosen));
        assert_eq!(plan(ProviderPref::Unset, https, None, Ready), Plan::Echo(Reason::NoKey));
        assert_eq!(plan(ProviderPref::Claude, https, OnFat, Ready), Plan::Echo(Reason::KeyOnFat));
        // The key crosses the wire only when the server can be verified: each missing input keeps it home.
        assert_eq!(plan(ProviderPref::Claude, https, UnaFs, NoTrustStore), Plan::Echo(Reason::TlsUnverified(NoTrustStore)));
        assert_eq!(plan(ProviderPref::Claude, https, UnaFs, NoClock), Plan::Echo(Reason::TlsUnverified(NoClock)));
        assert_eq!(plan(ProviderPref::Unset, https, UnaFs, NoProvider), Plan::Echo(Reason::TlsUnverified(NoProvider)));
        assert_eq!(plan(ProviderPref::Unset, https, UnaFs, Ready), Plan::Claude { send_key: true });
        // A plain-HTTP relay never gets the key, verified or not.
        assert_eq!(plan(ProviderPref::Unset, Some(&relay), None, NoTrustStore), Plan::Claude { send_key: false });
        assert_eq!(plan(ProviderPref::Unset, Some(&relay), UnaFs, Ready), Plan::Claude { send_key: false });
        assert_eq!(plan(ProviderPref::Claude, Option::None, UnaFs, Ready), Plan::Echo(Reason::BadEndpoint));
        for v in [NoClock, NoProvider, NoTrustStore] {
            assert!(Reason::TlsUnverified(v).text().starts_with("key not sent"));
            assert!(!Reason::TlsUnverified(v).text().contains("insecure"));
        }
        assert_eq!(provider_pref(Some(b"\"echo\"")), ProviderPref::Echo);
        assert_eq!(provider_pref(Some(b"claude")), ProviderPref::Claude);
        assert_eq!(provider_pref(Some(b"\"relay\"")), ProviderPref::Unset);
    }

    #[test]
    fn key_file_trimmed_and_refused_when_odd() {
        assert_eq!(key_from_file(b"  sk-ant-abc\n"), Some("sk-ant-abc"));
        assert_eq!(key_from_file(b"sk ant"), Option::None);
        assert_eq!(key_from_file(b"\n\n"), Option::None);
    }
}
