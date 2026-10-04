// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Name resolution and a blocking TCP stream over the NETRING3 syscalls. The kernel's socket verbs are
//! NON-BLOCKING (the IF-masked handler never blocks): -EAGAIN means "nothing yet", so this type sleeps
//! and re-calls, bounded, so a dead peer is an error and never a hang (the user-net adapter, reused).

use crate::sys::{sleep_ms, sys};
use una_abi::{EAGAIN, EINPROGRESS, RESOLVE_OUT_LEN, SYS_CLOSE, SYS_CONNECT, SYS_RESOLVE, SYS_SEND, SYS_SOCKET, SYS_SOCK_RECV};

/// Cancelled by the caller (the Watched tick said stop).
pub const ECANCELED: i64 = -125;
/// Connect polls x 50 ms: 15 s.
const CONNECT_TRIES: u32 = 300;
/// Idle polls x 10 ms a read or write waits for progress: 120 s (a model may think before its first byte).
const IO_IDLE_TRIES: u32 = 12_000;

/// The caller's wait hook: called every ~10 ms while a read, write or connect waits (poll input, repaint);
/// returning `false` cancels the operation with [`ECANCELED`]. A ring-3 program is single-threaded here,
/// so one process-wide hook is the whole story.
static TICK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub fn set_tick(f: Option<fn() -> bool>) {
    TICK.store(f.map_or(0, |f| f as usize), core::sync::atomic::Ordering::Relaxed);
}

fn tick() -> bool {
    let p = TICK.load(core::sync::atomic::Ordering::Relaxed);
    if p == 0 {
        return true;
    }
    let f: fn() -> bool = unsafe { core::mem::transmute(p) };
    f()
}

/// Sleep `ms` in waits, running the hook; `Err(ECANCELED)` if it says stop.
fn wait(ms: u64) -> Result<(), i64> {
    if !tick() {
        return Err(ECANCELED);
    }
    sleep_ms(ms);
    Ok(())
}

/// `host` → its IPv4 address through the kernel resolver.
pub fn resolve(host: &str) -> Result<[u8; 4], i64> {
    let mut out = [0u8; RESOLVE_OUT_LEN];
    let r = sys(SYS_RESOLVE, host.as_ptr() as u64, host.len() as u64, out.as_mut_ptr() as u64, 0);
    if r != 0 {
        return Err(if r < 0 { r } else { una_abi::EIO });
    }
    Ok([out[0], out[1], out[2], out[3]])
}

pub struct Tcp {
    h: u64,
    open: bool,
}

impl Tcp {
    pub fn connect(ip: [u8; 4], port: u16) -> Result<Tcp, i64> {
        let h = sys(SYS_SOCKET, 2, 1, 0, 0);
        if h < 0 {
            return Err(h);
        }
        let t = Tcp { h: h as u64, open: true };
        let pb = port.to_le_bytes();
        let hdr = [ip[0], ip[1], ip[2], ip[3], pb[0], pb[1], 0, 0];
        let mut c = EINPROGRESS;
        for _ in 0..CONNECT_TRIES {
            c = sys(SYS_CONNECT, t.h, hdr.as_ptr() as u64, hdr.len() as u64, 0);
            if c != EINPROGRESS {
                break;
            }
            if let Err(e) = wait(50) {
                c = e;
                break;
            }
        }
        if c != 0 {
            t.close();
            return Err(c);
        }
        Ok(t)
    }

    /// One non-blocking read: `Ok(None)` = nothing queued yet.
    pub fn try_recv(&mut self, b: &mut [u8]) -> Result<Option<usize>, i64> {
        let r = sys(SYS_SOCK_RECV, self.h, b.as_mut_ptr() as u64, b.len() as u64, 0);
        if r == EAGAIN {
            Ok(None)
        } else if r < 0 {
            Err(r)
        } else {
            Ok(Some(r as usize))
        }
    }

    pub fn write_some(&mut self, b: &[u8]) -> Result<usize, i64> {
        for _ in 0..IO_IDLE_TRIES {
            let r = sys(SYS_SEND, self.h, b.as_ptr() as u64, b.len() as u64, 0);
            if r == EAGAIN || r == 0 {
                wait(10)?;
                continue;
            }
            return if r < 0 { Err(r) } else { Ok(r as usize) };
        }
        Err(EAGAIN)
    }

    pub fn read_some(&mut self, b: &mut [u8]) -> Result<usize, i64> {
        for _ in 0..IO_IDLE_TRIES {
            match self.try_recv(b)? {
                Some(n) => return Ok(n),
                None => wait(10)?,
            }
        }
        Err(EAGAIN)
    }

    pub fn close(mut self) {
        self.shut();
    }

    fn shut(&mut self) {
        if self.open {
            sys(SYS_CLOSE, self.h, 0, 0, 0);
            self.open = false;
        }
    }
}

impl Drop for Tcp {
    fn drop(&mut self) {
        self.shut();
    }
}

impl vein_core::client::Transport for Tcp {
    fn send_all(&mut self, mut b: &[u8]) -> Result<(), i64> {
        while !b.is_empty() {
            let n = self.write_some(b)?;
            b = &b[n..];
        }
        Ok(())
    }
    fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
        self.read_some(b)
    }
}
