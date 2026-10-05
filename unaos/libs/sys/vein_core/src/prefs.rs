// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The resolution rules: what Principia says (namespace `vein`) plus the state of the key → which
//! provider answers and whether the key may leave the machine. Pure, so every caller of Vein (Lumen, the
//! installer's diagnosis program, the host) decides the same way.
//!
//! Keys: `vein.provider` (`"claude"` | `"echo"`; unset = claude when it can run), `vein.model`,
//! `vein.endpoint` (a URL; unset = `https://api.anthropic.com/v1/messages`; an `http://` URL is a relay
//! that holds the key itself, so the key is never sent to it), `vein.tls` (`"verify"` default |
//! `"insecure"`), `vein.key_file` (the absolute path of the key file on the UnaFS volume, conventionally
//! `<home>/.config/unaos/vein.key`; RING3ABI2 (B333): when the preference is unset, ring 3 composes that
//! conventional path from `SYS_WHOAMI`'s home — `vein_ring3::key::default_path` — and the preference
//! overrides it).

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsPolicy {
    Verify,
    Insecure,
}

pub fn tls_policy(v: Option<&[u8]>) -> TlsPolicy {
    if v.map(unquote) == Some(b"insecure") { TlsPolicy::Insecure } else { TlsPolicy::Verify }
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
    TlsUnverified,
    BadEndpoint,
}

impl Reason {
    /// One line for the window.
    pub fn text(self) -> &'static str {
        match self {
            Reason::Chosen => "vein.provider = \"echo\"",
            Reason::NoKey => "no API key: set vein.key_file to your key file on the UnaFS volume",
            Reason::KeyOnFat => "key refused: the key file is on FAT (no owner, no mode); keep it on UnaFS",
            Reason::TlsUnverified => "TLS certificate checks are owed (trust store); set vein.tls = \"insecure\" to send your key anyway, or use an http:// vein.endpoint relay",
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

/// The rule. `verify_available` = this build can check the server's certificate chain.
pub fn plan(p: ProviderPref, ep: Option<&Endpoint<'_>>, key: KeyState, tls: TlsPolicy, verify_available: bool) -> Plan {
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
    if !verify_available && tls == TlsPolicy::Verify {
        return Plan::Echo(Reason::TlsUnverified);
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
        assert_eq!(plan(ProviderPref::Echo, https, UnaFs, TlsPolicy::Insecure, true), Plan::Echo(Reason::Chosen));
        assert_eq!(plan(ProviderPref::Unset, https, None, TlsPolicy::Verify, true), Plan::Echo(Reason::NoKey));
        assert_eq!(plan(ProviderPref::Claude, https, OnFat, TlsPolicy::Verify, true), Plan::Echo(Reason::KeyOnFat));
        assert_eq!(plan(ProviderPref::Claude, https, UnaFs, TlsPolicy::Verify, false), Plan::Echo(Reason::TlsUnverified));
        assert_eq!(plan(ProviderPref::Claude, https, UnaFs, TlsPolicy::Insecure, false), Plan::Claude { send_key: true });
        assert_eq!(plan(ProviderPref::Unset, https, UnaFs, TlsPolicy::Verify, true), Plan::Claude { send_key: true });
        assert_eq!(plan(ProviderPref::Unset, Some(&relay), None, TlsPolicy::Verify, false), Plan::Claude { send_key: false });
        assert_eq!(plan(ProviderPref::Claude, Option::None, UnaFs, TlsPolicy::Verify, true), Plan::Echo(Reason::BadEndpoint));
        assert_eq!(provider_pref(Some(b"\"echo\"")), ProviderPref::Echo);
        assert_eq!(provider_pref(Some(b"claude")), ProviderPref::Claude);
        assert_eq!(provider_pref(Some(b"\"relay\"")), ProviderPref::Unset);
        assert_eq!(tls_policy(Some(b"\"insecure\"")), TlsPolicy::Insecure);
        assert_eq!(tls_policy(Option::None), TlsPolicy::Verify);
    }

    #[test]
    fn key_file_trimmed_and_refused_when_odd() {
        assert_eq!(key_from_file(b"  sk-ant-abc\n"), Some("sk-ant-abc"));
        assert_eq!(key_from_file(b"sk ant"), Option::None);
        assert_eq!(key_from_file(b"\n\n"), Option::None);
    }
}
