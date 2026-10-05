// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The client consumers link: one connection to the Holocron bus socket, the eight verbs, and the
//! consumer rule ([`holocron_core::keysource`]) packaged for Vein.

use holocron_core::keysource::{self, Answer, KeySource};
use holocron_core::wire::{self, Reply, Request};
use holocron_core::zero::SecretBytes;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// A connection to Holocron.
pub struct Client {
    s: UnixStream,
}

/// The bus socket a client uses: `$HOLOCRON_SOCK`, else `<default root>/.bus.sock`.
pub fn default_socket() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HOLOCRON_SOCK") {
        return Some(PathBuf::from(p));
    }
    crate::store::default_root().map(|r| r.join(crate::daemon::BUS_SOCK))
}

impl Client {
    /// Connect to the socket at `path`.
    pub fn connect(path: &Path) -> io::Result<Client> {
        Ok(Client { s: UnixStream::connect(path)? })
    }

    /// Send one request and read its reply.
    pub fn call(&mut self, req: &Request) -> io::Result<Reply> {
        let mut body = req.encode_body();
        let mut f = Vec::with_capacity(5 + body.len());
        f.extend_from_slice(&((1 + body.len()) as u32).to_le_bytes());
        f.push(req.verb());
        f.extend_from_slice(&body);
        let w = self.s.write_all(&f);
        holocron_core::zero::wipe(&mut body);
        holocron_core::zero::wipe(&mut f);
        w?;
        let mut len = [0u8; 4];
        self.s.read_exact(&mut len)?;
        let n = u32::from_le_bytes(len) as usize;
        if !(4..=4 + wire::BODY_MAX).contains(&n) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad reply length"));
        }
        let mut rest = vec![0u8; n];
        self.s.read_exact(&mut rest)?;
        let status = i32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        let body = rest[4..].to_vec();
        holocron_core::zero::wipe(&mut rest);
        Ok(Reply { status, body })
    }

    /// `SecretGet(ns, name)` as a consumer [`Answer`].
    pub fn ask(&mut self, ns: &str, name: &str) -> io::Result<Answer> {
        let mut r = self.call(&Request::Get { ns: ns.into(), name: name.into() })?;
        Ok(if r.status == wire::status::OK {
            Answer::Found(SecretBytes::new(core::mem::take(&mut r.body)))
        } else {
            Answer::Status(r.status)
        })
    }
}

/// Ask the Holocron at `sock` for `ns/name`: no socket, or a socket nobody answers on, is
/// [`Answer::Unavailable`] (the consumer falls back).
pub fn ask_at(sock: Option<&Path>, ns: &str, name: &str) -> Answer {
    let Some(sock) = sock else { return Answer::Unavailable };
    match Client::connect(sock) {
        Ok(mut c) => c.ask(ns, name).unwrap_or(Answer::Unavailable),
        Err(_) => Answer::Unavailable,
    }
}

/// Vein's Claude API key, by the consumer rule: Holocron first (`vein/claude.api_key`), fall back on
/// NotFound or no Holocron, refuse on Locked/Denied/Corrupt.
pub fn claude_api_key(sock: Option<&Path>) -> KeySource {
    keysource::decide(ask_at(sock, keysource::VEIN_NS, keysource::CLAUDE_API_KEY))
}
