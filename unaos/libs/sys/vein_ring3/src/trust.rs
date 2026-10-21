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

    /// AETHERNET2 (LEDGER SR187): the reader's errno for a caller's wire (`code=`): `Open`/`Read` carry the verb's
    /// answer (`-2` from SYS_PATH_READ = the builder staged no bundle on the volume), the others 0.
    pub fn errno(&self) -> i64 {
        match self {
            TrustFail::Open(e) | TrustFail::Read(e) => *e,
            TrustFail::TooLarge | TrustFail::Empty => 0,
        }
    }
}

/// CTCORE (SR60): Google's CT log list as `tools/trust-bundle` stages it (8.3: SYSTEM/TRUST/CTLOGS.JSN).
pub const CT_LOGS_PATH: &str = "/system/trust/ctlogs.jsn";
const _: () = assert!(CT_LOGS_PATH.len() <= SYS_OPEN_NAME_MAX);
/// Today's list is 50 KB.
pub const CT_LOGS_MAX: usize = 512 * 1024;

/// CTCORE (SR60): Mozilla's CCADB intermediates, an UNTRUSTED pool (8.3: SYSTEM/TRUST/INTERS.PEM).
pub const INTERS_PATH: &str = "/system/trust/inters.pem";
const _: () = assert!(INTERS_PATH.len() <= SYS_OPEN_NAME_MAX);
/// The CCADB set is a few MB; a bigger file is refused (the pool is optional).
pub const INTERS_MAX: usize = 8 * 1024 * 1024;

/// Load and parse the bundle at [`ROOTS_PATH`], plus the intermediate pool at [`INTERS_PATH`] when staged.
pub fn load() -> Result<(TrustStore, LoadReport), TrustFail> {
    let (mut store, rep) = parse(&read_file(ROOTS_PATH, ROOTS_MAX)?)?;
    if let Ok(text) = read_file(INTERS_PATH, INTERS_MAX) {
        if let Ok(t) = core::str::from_utf8(&text) {
            store.add_intermediates_pem(t);
        }
    }
    Ok((store, rep))
}

/// The CT log list at [`CT_LOGS_PATH`] (report mode): `None` when absent or unparseable — the handshake then
/// reports `ct=off`, it does not fail.
pub fn load_ct(p: &dyn tls_core::CryptoProvider) -> Option<tls_core::ct::CtConfig> {
    let text = read_file(CT_LOGS_PATH, CT_LOGS_MAX).ok()?;
    let list = tls_core::ct::LogList::parse_v3(p, &text, tls_core::ct::LIST_GOOGLE).ok()?;
    Some(tls_core::ct::CtConfig { list, mode: tls_core::ct::CtMode::Report })
}

/// AETHERNET2 (LEDGER SR187): a trust file through the kernel VFS — SELFDIAG's `SYS_PATH_READ`, the crate's ONE caller
/// [`crate::files::path_read`] — because that is where `/system` lives on a UnaFS root (the x86 card puts `system/` on
/// p2; x86 `SYS_OPEN` reaches only a root leaf or, under `irqstorage`, the FAT p1, so it answered `-ENOENT` on every
/// card and every https page read `tls-no-trust-store`). An image without the verb (`-ENOSYS` on the first chunk)
/// falls back to the classic handle verbs, [`read_open`].
fn read_file(path: &str, max: usize) -> Result<Vec<u8>, TrustFail> {
    match read_chunks(max, |off, buf| crate::files::path_read(path.as_bytes(), off, buf)) {
        Err(TrustFail::Open(e)) if e == una_abi::ENOSYS => read_open(path, max),
        r => r,
    }
}

/// The chunk loop over one positional read verb `step(offset, buf) -> bytes | -errno` (SYS_PATH_READ's shape, at
/// most `PATH_IO_MAX` per call): the first call's refusal is `Open`, a later one `Read`, more than `max` bytes
/// `TooLarge`; a short (or empty) answer is the end of the file. Split out so the host test drives it with a stub.
fn read_chunks(max: usize, mut step: impl FnMut(u64, &mut [u8]) -> i64) -> Result<Vec<u8>, TrustFail> {
    let mut text: Vec<u8> = Vec::new();
    let mut chunk = alloc::vec![0u8; crate::files::next::PATH_IO_MAX];
    loop {
        let k = step(text.len() as u64, &mut chunk);
        if k < 0 {
            return Err(if text.is_empty() { TrustFail::Open(k) } else { TrustFail::Read(k) });
        }
        let k = (k as usize).min(chunk.len());
        if text.len() + k > max {
            return Err(TrustFail::TooLarge);
        }
        text.extend_from_slice(&chunk[..k]);
        if k < chunk.len() {
            return Ok(text);
        }
    }
}

/// The classic handle verbs (SYS_STAT for the size hint, SYS_OPEN + SYS_READ): the reader before AETHERNET2, kept
/// for an image whose kernel has no `SYS_PATH_READ`.
fn read_open(path: &str, max: usize) -> Result<Vec<u8>, TrustFail> {
    let mut st = [0u8; USER_STAT_LEN];
    let hint = if sys(SYS_STAT, path.as_ptr() as u64, path.len() as u64, st.as_mut_ptr() as u64, 0) == 0 {
        u64::from_le_bytes([st[8], st[9], st[10], st[11], st[12], st[13], st[14], st[15]]) as usize
    } else {
        0
    };
    if hint > max {
        return Err(TrustFail::TooLarge);
    }
    let h = sys(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
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
        if text.len() + k as usize > max {
            break Err(TrustFail::TooLarge);
        }
        text.extend_from_slice(&chunk[..k as usize]);
    };
    sys(SYS_CLOSE, h as u64, 0, 0, 0);
    r?;
    Ok(text)
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

#[cfg(test)]
mod tests {
    //! AETHERNET2 (LEDGER SR187): the trust reader's chunk loop over a STUB `SYS_PATH_READ` (no kernel on the host):
    //! a file spanning several chunks comes back whole, the first call's refusal is `Open(errno)` (ENOSYS is what
    //! `read_file` falls back on), a refusal mid-file is `Read`, a file past the cap is `TooLarge`.
    use super::*;
    use crate::files::next::PATH_IO_MAX;

    /// A stub SYS_PATH_READ over `file`: at most `buf.len()` bytes from `off`, 0 at the end.
    fn serve(file: &[u8]) -> impl FnMut(u64, &mut [u8]) -> i64 + '_ {
        move |off, buf| {
            let off = (off as usize).min(file.len());
            let n = buf.len().min(file.len() - off);
            buf[..n].copy_from_slice(&file[off..off + n]);
            n as i64
        }
    }

    #[test]
    fn multi_chunk_file_comes_back_whole() {
        let file: Vec<u8> = (0..(2 * PATH_IO_MAX + 123)).map(|i| (i % 251) as u8).collect();
        assert_eq!(read_chunks(ROOTS_MAX, serve(&file)).unwrap(), file);
        // A file of exactly one chunk ends on the following empty read.
        let one = alloc::vec![7u8; PATH_IO_MAX];
        assert_eq!(read_chunks(ROOTS_MAX, serve(&one)).unwrap(), one);
        assert_eq!(read_chunks(ROOTS_MAX, serve(&[])).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn refusals_are_named() {
        assert_eq!(read_chunks(ROOTS_MAX, |_, _| una_abi::ENOSYS), Err(TrustFail::Open(una_abi::ENOSYS)));
        assert_eq!(read_chunks(ROOTS_MAX, |_, _| una_abi::ENOENT), Err(TrustFail::Open(una_abi::ENOENT)));
        assert_eq!(TrustFail::Open(una_abi::ENOENT).errno(), una_abi::ENOENT);
        assert_eq!(TrustFail::TooLarge.errno(), 0);
        let file = alloc::vec![1u8; PATH_IO_MAX + 10];
        let mut inner = serve(&file);
        let mid = |off: u64, buf: &mut [u8]| if off == 0 { inner(off, buf) } else { una_abi::EIO };
        assert_eq!(read_chunks(ROOTS_MAX, mid), Err(TrustFail::Read(una_abi::EIO)));
        assert_eq!(read_chunks(PATH_IO_MAX, serve(&file)), Err(TrustFail::TooLarge));
    }

    #[test]
    fn junk_is_not_a_store() {
        assert_eq!(parse(b"not a pem bundle").err(), Some(TrustFail::Empty));
    }
}
