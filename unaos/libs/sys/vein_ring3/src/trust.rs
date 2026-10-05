// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The trust store (VEINTLS, SR36): `/system/trust/roots.pem`, the Mozilla CA bundle `tools/trust-bundle`
//! fetches and the builder stages onto the boot volume and the data volume (sha256 pinned in
//! `system/trust/roots.pem.sha256`). Read once with SYS_STAT + SYS_OPEN + SYS_READ and parsed by
//! `tls_core::x509::TrustStore::from_pem`; the PEM text is freed after parsing.
//!
//! The path is 23 bytes, inside SYS_OPEN's 40-byte name bound (`MAX_NAME` on both arches, DIRNS SO20);
//! the kernel walks it on the mounted FAT volume as `SYSTEM/TRUST/ROOTS.PEM` (every component is 8.3).

use alloc::vec::Vec;
use tls_core::x509::{LoadReport, TrustStore};
use una_abi::{SYS_CLOSE, SYS_OPEN, SYS_READ, SYS_STAT, USER_STAT_LEN};

use crate::sys::sys;

/// Where the builder puts the bundle.
pub const ROOTS_PATH: &str = "/system/trust/roots.pem";
/// SYS_OPEN's name bound — the kernel's `MAX_NAME` (x86_64 and aarch64 `syscall.rs`, DIRNS SO20).
pub const SYS_OPEN_NAME_MAX: usize = 40;
const _: () = assert!(ROOTS_PATH.len() <= SYS_OPEN_NAME_MAX);
/// A bundle larger than this is not the Mozilla bundle (today 240 KB): refused rather than read.
pub const ROOTS_MAX: usize = 1024 * 1024;

/// Why there is no trust store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustFail {
    /// SYS_OPEN refused (`-ENOENT`: the builder staged none — run `tools/trust-bundle`, rebuild).
    Open(i64),
    Read(i64),
    TooLarge,
    /// The file parsed to zero usable anchors.
    Empty,
}

impl TrustFail {
    pub fn why(&self) -> &'static str {
        match self {
            TrustFail::Open(_) => "no trust store at /system/trust/roots.pem (run tools/trust-bundle, rebuild the image)",
            TrustFail::Read(_) => "the trust store could not be read",
            TrustFail::TooLarge => "the trust store is over 1 MiB: not the Mozilla bundle",
            TrustFail::Empty => "the trust store holds no certificate",
        }
    }
}

/// Load and parse the bundle at [`ROOTS_PATH`].
pub fn load() -> Result<(TrustStore, LoadReport), TrustFail> {
    let mut st = [0u8; USER_STAT_LEN];
    let hint = if sys(SYS_STAT, ROOTS_PATH.as_ptr() as u64, ROOTS_PATH.len() as u64, st.as_mut_ptr() as u64, 0) == 0 {
        u64::from_le_bytes([st[8], st[9], st[10], st[11], st[12], st[13], st[14], st[15]]) as usize
    } else {
        0
    };
    if hint > ROOTS_MAX {
        return Err(TrustFail::TooLarge);
    }
    let h = sys(SYS_OPEN, ROOTS_PATH.as_ptr() as u64, ROOTS_PATH.len() as u64, 0, 0);
    if h < 0 {
        return Err(TrustFail::Open(h));
    }
    let mut text: Vec<u8> = Vec::with_capacity(if hint > 0 { hint } else { 256 * 1024 });
    let mut chunk = [0u8; 4096];
    let r = loop {
        let k = sys(SYS_READ, h as u64, chunk.as_mut_ptr() as u64, chunk.len() as u64, 0);
        if k < 0 {
            break Err(TrustFail::Read(k));
        }
        if k == 0 {
            break Ok(());
        }
        if text.len() + k as usize > ROOTS_MAX {
            break Err(TrustFail::TooLarge);
        }
        text.extend_from_slice(&chunk[..k as usize]);
    };
    sys(SYS_CLOSE, h as u64, 0, 0, 0);
    r?;
    parse(&text)
}

/// Parse a PEM bundle (split out so the host test runs the same rule on the real bundle).
pub fn parse(text: &[u8]) -> Result<(TrustStore, LoadReport), TrustFail> {
    let s = core::str::from_utf8(text).map_err(|_| TrustFail::Empty)?;
    let (store, rep) = TrustStore::from_pem(s);
    if store.anchors.is_empty() {
        return Err(TrustFail::Empty);
    }
    Ok((store, rep))
}
