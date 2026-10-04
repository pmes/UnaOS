// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! TLS 1.3 over [`Tcp`] — `embedded-tls` 0.19 (no_std, no alloc), the crate the NETRING3 spike built for
//! this target and measured at ~78 KiB of code + ~20 KiB of record buffers; it fits now that a ring-3
//! program can link in the 4 MiB ELF window (RING3WIN). Cipher suite TLS_AES_128_GCM_SHA256, key share
//! P-256, randomness from SYS_GETRANDOM (the kernel DRBG).
//!
//! **NO CERTIFICATE IS VERIFIED** (`UnsecureProvider`). The trust store (`/system/trust/roots.pem`,
//! NETRING3 §Trust store) is the owed rung; until it lands an active attacker on the path could read
//! what is sent, which is why the key goes over this only on `vein.tls = "insecure"`.

use crate::net::Tcp;
use core::sync::atomic::{AtomicI64, Ordering};
use embedded_io::{ErrorKind, ErrorType, Read, Write};
use embedded_tls::blocking::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext, TlsError, UnsecureProvider};

/// `-EPROTO`: the TLS layer failed (see [`last_error`]).
pub const EPROTO: i64 = -71;

/// The record buffers (a full 16 KiB TLS record must fit the read buffer).
pub struct TlsBufs {
    rb: [u8; 16640],
    wb: [u8; 4096],
}

impl TlsBufs {
    pub const fn new() -> Self {
        TlsBufs { rb: [0; 16640], wb: [0; 4096] }
    }
}

impl Default for TlsBufs {
    fn default() -> Self {
        Self::new()
    }
}

static LAST_SOCK_ERR: AtomicI64 = AtomicI64::new(0);
static mut LAST_TLS: [u8; 64] = [0; 64];
static LAST_TLS_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The last TLS error, as its Debug name (for the window and the wire).
pub fn last_error() -> &'static str {
    let n = LAST_TLS_N.load(Ordering::Relaxed);
    let b = unsafe { &*core::ptr::addr_of!(LAST_TLS) };
    core::str::from_utf8(&b[..n]).unwrap_or("?")
}

fn record(e: &TlsError) -> i64 {
    struct W(usize);
    impl core::fmt::Write for W {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = unsafe { &mut *core::ptr::addr_of_mut!(LAST_TLS) };
            for &c in s.as_bytes() {
                if self.0 < b.len() {
                    b[self.0] = c;
                    self.0 += 1;
                }
            }
            Ok(())
        }
    }
    let mut w = W(0);
    let _ = core::fmt::write(&mut w, format_args!("{:?}", e));
    LAST_TLS_N.store(w.0, Ordering::Relaxed);
    // A socket failure underneath keeps its own errno (ECANCELED from the caller's hook included).
    match e {
        TlsError::Io(_) | TlsError::IoError => {
            let s = LAST_SOCK_ERR.load(Ordering::Relaxed);
            if s < 0 { s } else { EPROTO }
        }
        _ => EPROTO,
    }
}

/// The embedded-io face of the socket.
pub struct Io<'t>(&'t mut Tcp);

impl ErrorType for Io<'_> {
    type Error = ErrorKind;
}

fn kind(e: i64) -> ErrorKind {
    LAST_SOCK_ERR.store(e, Ordering::Relaxed);
    match e {
        crate::net::ECANCELED => ErrorKind::Interrupted,
        una_abi::EAGAIN => ErrorKind::TimedOut,
        una_abi::ECONNRESET => ErrorKind::ConnectionReset,
        una_abi::ENOTCONN => ErrorKind::NotConnected,
        _ => ErrorKind::Other,
    }
}

impl Read for Io<'_> {
    fn read(&mut self, b: &mut [u8]) -> Result<usize, ErrorKind> {
        self.0.read_some(b).map_err(kind)
    }
}

impl Write for Io<'_> {
    fn write(&mut self, b: &[u8]) -> Result<usize, ErrorKind> {
        self.0.write_some(b).map_err(kind)
    }
    fn flush(&mut self) -> Result<(), ErrorKind> {
        Ok(())
    }
}

/// The RNG the handshake draws from: the kernel DRBG.
struct Rng;

impl rand_core::RngCore for Rng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, d: &mut [u8]) {
        if crate::sys::getrandom(d).is_err() {
            // No entropy is no TLS: never hand the handshake predictable bytes.
            crate::sys::write(b":: VEIN: getrandom failed -- TLS aborted ::\n");
            crate::sys::sys(una_abi::SYS_EXIT, 4, 0, 0, 0);
        }
    }
    fn try_fill_bytes(&mut self, d: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(d);
        Ok(())
    }
}

impl rand_core::CryptoRng for Rng {}

/// An open TLS session over a borrowed socket.
pub struct Tls<'a> {
    c: TlsConnection<'a, Io<'a>, Aes128GcmSha256>,
}

impl<'a> Tls<'a> {
    /// Handshake with `host` (SNI). `Err` = a negative errno (`EPROTO` + [`last_error`] for TLS faults).
    pub fn open(tcp: &'a mut Tcp, host: &'a str, bufs: &'a mut TlsBufs) -> Result<Self, i64> {
        LAST_SOCK_ERR.store(0, Ordering::Relaxed);
        let cfg = TlsConfig::new().with_server_name(host);
        let mut c: TlsConnection<'a, Io<'a>, Aes128GcmSha256> = TlsConnection::new(Io(tcp), &mut bufs.rb, &mut bufs.wb);
        c.open(TlsContext::new(&cfg, UnsecureProvider::new::<Aes128GcmSha256>(Rng))).map_err(|e| record(&e))?;
        Ok(Tls { c })
    }
}

impl vein_core::client::Transport for Tls<'_> {
    fn send_all(&mut self, mut b: &[u8]) -> Result<(), i64> {
        while !b.is_empty() {
            let n = self.c.write(b).map_err(|e| record(&e))?;
            if n == 0 {
                return Err(EPROTO);
            }
            b = &b[n..];
        }
        self.c.flush().map_err(|e| record(&e))
    }
    fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
        match self.c.read(b) {
            Ok(n) => Ok(n),
            Err(TlsError::ConnectionClosed) => Ok(0),
            Err(e) => Err(record(&e)),
        }
    }
}
