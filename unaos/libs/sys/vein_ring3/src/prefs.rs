// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Principia reads over the bus: BUS_VERB_PREF_GET `<ns>.<key>` → the TOML scalar literal (fulfilled
//! in-kernel by PREFS over the ONE store, `<home>/.config/unaos/preferences.toml`). Synchronous: one
//! request, then block for its reply (`crate::bus::ask`: a frame met first is kept for the program's reader).

use una_abi::{BUS_VERB_PREF_DECLARE, BUS_VERB_PREF_GET};
use vein_core::prefs::{self as rules, Endpoint, KeyState, Plan, ProviderPref, Verify};

/// One preference into `out` (the raw literal, quotes included). `Ok(None)` = unset; `Err` = the bus refused.
/// DROPLOOPS (SR179): one ask through `crate::bus::ask` (`una_abi::bus_ask`): a frame met before the reply is kept.
pub fn get(key: &str, out: &mut [u8]) -> Result<Option<usize>, i64> {
    match crate::bus::ask(BUS_VERB_PREF_GET, key.as_bytes(), out) {
        Err(una_abi::ENOENT) => Ok(None),
        r => r.map(Some),
    }
}

/// The `vein` namespace, read once.
pub struct Config {
    pub provider: ProviderPref,
    model: [u8; 64],
    model_n: usize,
    endpoint: [u8; 160],
    endpoint_n: usize,
    key_file: [u8; 64],
    key_file_n: usize,
    /// The first bus error, if Principia could not be asked at all (the defaults then apply).
    pub bus_err: i64,
}

fn unq(b: &[u8], n: usize) -> &str {
    core::str::from_utf8(rules::unquote(&b[..n])).unwrap_or("")
}

impl Config {
    pub fn read() -> Config {
        let mut c = Config { provider: ProviderPref::Unset, model: [0; 64], model_n: 0, endpoint: [0; 160], endpoint_n: 0, key_file: [0; 64], key_file_n: 0, bus_err: 0 };
        let mut v = [0u8; 160];
        let one = |k: &str, v: &mut [u8], err: &mut i64| -> Option<usize> {
            match get(k, v) {
                Ok(n) => n,
                Err(e) => {
                    if *err == 0 {
                        *err = e;
                    }
                    None
                }
            }
        };
        let mut err = 0;
        c.provider = rules::provider_pref(one("vein.provider", &mut v, &mut err).map(|n| &v[..n]));
        if let Some(n) = one("vein.model", &mut v, &mut err) {
            let n = n.min(c.model.len());
            c.model[..n].copy_from_slice(&v[..n]);
            c.model_n = n;
        }
        if let Some(n) = one("vein.endpoint", &mut v, &mut err) {
            let n = n.min(c.endpoint.len());
            c.endpoint[..n].copy_from_slice(&v[..n]);
            c.endpoint_n = n;
        }
        if let Some(n) = one("vein.key_file", &mut v, &mut err) {
            let n = n.min(c.key_file.len());
            c.key_file[..n].copy_from_slice(&v[..n]);
            c.key_file_n = n;
        }
        c.bus_err = err;
        c
    }

    /// `vein.model`, or the default.
    pub fn model(&self) -> &str {
        let m = unq(&self.model, self.model_n);
        if m.is_empty() { vein_core::claude::DEFAULT_MODEL } else { m }
    }

    /// `vein.endpoint`, or the Anthropic API; `None` = set but not a URL.
    pub fn endpoint(&self) -> Option<Endpoint<'_>> {
        let e = unq(&self.endpoint, self.endpoint_n);
        if e.is_empty() { Some(rules::DEFAULT_ENDPOINT) } else { rules::parse_endpoint(e) }
    }

    /// `vein.key_file`, if set.
    pub fn key_file(&self) -> Option<&str> {
        let k = unq(&self.key_file, self.key_file_n);
        if k.is_empty() { None } else { Some(k) }
    }

    /// The rule, applied. `verify` = whether this program can verify a TLS server right now (provider,
    /// trust store, clock — the caller knows; see `crate::Tls::verify`).
    pub fn plan(&self, key: KeyState, verify: Verify) -> Plan {
        let ep = self.endpoint();
        rules::plan(self.provider, ep.as_ref(), key, verify)
    }
}

// ── SETTINGSFILES (rmbp-ledger B407, R98): a program's own settings stanza ─────────────────────────────

/// Lumen's stanza (`app.lumen.*`, stored in `<home>/settings/lumen`): the window frame MACPARITY row 12
/// (WINMEMORY) remembers per app. The body is `prefs_core::declare::body`'s shape, spelled as bytes.
pub const LUMEN_STANZA: &[u8] = b"lumen\0window.frame\tstr:32\t\"\"\tLumen's window frame as x,y,w,h in logical px; empty = centred (MACPARITY row 12)\n";

/// PrefDeclare: declare this program's stanza ONCE (`<name>` NUL `<key>\t<spec>\t<default>\t<doc>` lines).
/// `Ok(())` = held by Principia for the session; `Err` = the bus refused (an older kernel: -EINVAL).
pub fn declare(body: &[u8]) -> Result<(), i64> {
    crate::bus::ask(BUS_VERB_PREF_DECLARE, body, &mut []).map(|_| ())
}
