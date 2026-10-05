// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! Vein's shared core (VEINCORE B304; reshaped by LUMENAPP B323 under R82). Vein is a LIBRARY, never a
//! daemon: the host handler (`handlers/vein`), the ring-3 Lumen app (`crates/user-lumen` → `APPS/LUMEN.ELF`)
//! and later the smart installer's diagnosis program link this crate and run it inside themselves.
//! Everything here is pure and host-unit-tested; the syscall-backed half (DNS, TCP, TLS, the PREFS bus
//! reads, the UnaFS key file) is `unaos/libs/sys/vein_ring3`, behind [`client::Transport`].
//!
//! * [`claude`] — the Claude Messages request encoder and the incremental SSE/JSON [`claude::StreamDecoder`].
//! * [`http`] — HTTP/1.1 request head, response head parser, chunked transfer decoder.
//! * [`client`] — the [`client::Transport`] trait and [`client::exchange`]: one request → streamed deltas.
//! * [`prefs`] — the resolution rules: Principia values (`vein.provider`, `vein.model`, `vein.endpoint`,
//!   `vein.key_file`) + the key's state + whether the server can be verified → which provider runs and
//!   whether the key may be sent (only over a verified TLS connection, VEINTLS SR36).
//! * [`provider`] — the offline `Echo` provider (no key, no network: the window still answers).
//! * [`md`] / [`scroll`] / [`history`] (LUMENUX B348) — the markdown line renderer, the scrollback ring of
//!   rendered rows, and the conversation-file codec the ring-3 Lumen window uses.
//! * [`json`] — the minimal no-alloc JSON string escape/scan the above share.
//! * [`model`] / [`context`] (feature `alloc`) — the conversation model and the pure context assembler.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod claude;
pub mod client;
pub mod http;
pub mod history;
pub mod json;
pub mod md;
pub mod prefs;
pub mod provider;
pub mod role;
pub mod scroll;

#[cfg(feature = "alloc")]
pub mod context;
#[cfg(feature = "alloc")]
pub mod model;

pub use role::Role;

/// A bounded byte writer over a caller buffer: never panics, records overflow.
pub struct Out<'a> {
    buf: &'a mut [u8],
    n: usize,
    over: bool,
}

impl<'a> Out<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Out { buf, n: 0, over: false }
    }
    pub fn put(&mut self, b: &[u8]) {
        if self.over || self.n + b.len() > self.buf.len() {
            self.over = true;
            return;
        }
        self.buf[self.n..self.n + b.len()].copy_from_slice(b);
        self.n += b.len();
    }
    pub fn dec(&mut self, mut v: u64) {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..]);
    }
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// The written bytes, or `None` if anything overflowed.
    pub fn done(self) -> Option<usize> {
        if self.over { None } else { Some(self.n) }
    }
}
