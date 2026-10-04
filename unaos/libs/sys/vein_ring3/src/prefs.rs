// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Principia reads over the bus: BUS_VERB_PREF_GET `<ns>.<key>` → the TOML scalar literal (fulfilled
//! in-kernel by PREFS over the ONE store, `<home>/.config/unaos/preferences.toml`). Synchronous: one
//! request, then block for its reply. The caller must not have another reader on its mailbox.

use crate::sys::sys;
use una_abi::{BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC, BUS_VERB_PREF_GET, BUS_VERSION, SYS_MRECV, SYS_MSEND};
use vein_core::prefs::{self as rules, Endpoint, KeyState, Plan, ProviderPref, TlsPolicy};

static mut TX: [u8; BUS_HDR_LEN + 128] = [0; BUS_HDR_LEN + 128];
static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
static mut CORR: u32 = 0x5645_0000;

/// One preference into `out` (the raw literal, quotes included). `Ok(None)` = unset; `Err` = the bus refused.
pub fn get(key: &str, out: &mut [u8]) -> Result<Option<usize>, i64> {
    let (tx, rx) = unsafe { (&mut *core::ptr::addr_of_mut!(TX), &mut *core::ptr::addr_of_mut!(RX)) };
    let k = key.as_bytes();
    if k.len() > tx.len() - BUS_HDR_LEN {
        return Err(una_abi::EINVAL);
    }
    let corr = unsafe {
        CORR = CORR.wrapping_add(1);
        CORR
    };
    tx[..BUS_HDR_LEN].fill(0);
    tx[0..4].copy_from_slice(&BUS_MAGIC);
    tx[4] = BUS_VERSION;
    tx[5] = BUS_KIND_REQUEST;
    tx[6] = BUS_VERB_PREF_GET;
    tx[8..12].copy_from_slice(&corr.to_le_bytes());
    tx[48..52].copy_from_slice(&(k.len() as u32).to_le_bytes());
    tx[BUS_HDR_LEN..BUS_HDR_LEN + k.len()].copy_from_slice(k);
    let s = sys(SYS_MSEND, tx.as_ptr() as u64, (BUS_HDR_LEN + k.len()) as u64, 0, 0);
    if s != 0 {
        return Err(s);
    }
    for _ in 0..8 {
        let n = sys(SYS_MRECV, rx.as_mut_ptr() as u64, rx.len() as u64, 0, 0);
        if n < 0 {
            return Err(n);
        }
        let n = n as usize;
        if n < BUS_HDR_LEN || rx[0..4] != BUS_MAGIC || rx[5] != BUS_KIND_REPLY || rx[8..12] != corr.to_le_bytes() {
            continue; // a stale frame: not ours
        }
        let status = i32::from_le_bytes([rx[12], rx[13], rx[14], rx[15]]) as i64;
        if status == una_abi::ENOENT {
            return Ok(None);
        }
        if status != 0 {
            return Err(status);
        }
        let blen = (u32::from_le_bytes([rx[48], rx[49], rx[50], rx[51]]) as usize).min(n - BUS_HDR_LEN);
        let m = blen.min(out.len());
        out[..m].copy_from_slice(&rx[BUS_HDR_LEN..BUS_HDR_LEN + m]);
        return Ok(Some(m));
    }
    Err(una_abi::EIO)
}

/// The `vein` namespace, read once.
pub struct Config {
    pub provider: ProviderPref,
    pub tls: TlsPolicy,
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
        let mut c = Config { provider: ProviderPref::Unset, tls: TlsPolicy::Verify, model: [0; 64], model_n: 0, endpoint: [0; 160], endpoint_n: 0, key_file: [0; 64], key_file_n: 0, bus_err: 0 };
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
        c.tls = rules::tls_policy(one("vein.tls", &mut v, &mut err).map(|n| &v[..n]));
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

    /// The rule, applied.
    pub fn plan(&self, key: KeyState) -> Plan {
        let ep = self.endpoint();
        rules::plan(self.provider, ep.as_ref(), key, self.tls, crate::VERIFIES_CERTS)
    }
}
