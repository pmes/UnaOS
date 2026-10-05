// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The host daemon: Holocron's two sockets over ONE service.
//!
//! * the **bus** socket (`<root>/.bus.sock`, or `$HOLOCRON_SOCK`) — the eight verbs of
//!   [`holocron_core::wire`], framed `[u32 len LE][u8 verb][body]` → `[u32 len LE][i32 status LE][body]`
//!   (`len` counts what follows it). The caller's principal is the socket's peer credential
//!   (SO_PEERCRED), taken once per connection — the host's equivalent of the kernel's BANDY3 stamp.
//! * the **agent** socket (`<root>/.agent.sock`, what `SSH_AUTH_SOCK` names) — the SSH agent protocol
//!   ([`holocron_core::agent`]), the same owner check.
//!
//! Both sockets are created `0600` inside the `0700` root, so the filesystem already keeps other users
//! out; the owner check is the second wall, and the one that holds on the metal.

use crate::principal;
use holocron_core::agent;
use holocron_core::seal::{Entropy, Sealer, Signer};
use holocron_core::service::{Holocron, Store};
use holocron_core::wire::{self, Reply};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The bus socket's file name inside the root.
pub const BUS_SOCK: &str = ".bus.sock";
/// The agent socket's file name inside the root.
pub const AGENT_SOCK: &str = ".agent.sock";

/// The bus socket path: `$HOLOCRON_SOCK`, else `<root>/.bus.sock`.
pub fn bus_socket(root: &Path) -> PathBuf {
    std::env::var_os("HOLOCRON_SOCK").map(PathBuf::from).unwrap_or_else(|| root.join(BUS_SOCK))
}

/// The agent socket path: `<root>/.agent.sock`.
pub fn agent_socket(root: &Path) -> PathBuf {
    root.join(AGENT_SOCK)
}

/// Raw `/dev/urandom` as a Holocron [`Entropy`] (tests and tools; the daemon draws from
/// [`crate::HostEntropy`], CRYPTOCORE's DRBG over the same source). A failed read is an error, never a
/// panic and never a short fill.
#[derive(Debug, Default)]
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), holocron_core::seal::EntropyError> {
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(buf))
            .map_err(|_| holocron_core::seal::EntropyError)
    }
}

/// The service shared by both sockets, plus the idle lock.
pub struct Shared<S: Sealer, G: Signer, T: Store, E: Entropy> {
    svc: Mutex<(Holocron<S, G, T, E>, Instant)>,
    start: Instant,
    idle_lock: Option<Duration>,
}

impl<S: Sealer, G: Signer, T: Store, E: Entropy> Shared<S, G, T, E> {
    /// Wrap a service. With `idle_lock`, a request arriving after that long without one locks the
    /// ring first (the key does not outlive an abandoned session).
    pub fn new(svc: Holocron<S, G, T, E>, idle_lock: Option<Duration>) -> Arc<Self> {
        let now = Instant::now();
        Arc::new(Shared { svc: Mutex::new((svc, now)), start: now, idle_lock })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Holocron<S, G, T, E>, u64, i64) -> R) -> R {
        let mut g = self.svc.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        if let Some(idle) = self.idle_lock {
            if now.duration_since(g.1) >= idle && g.0.is_unlocked() {
                g.0.lock_now();
                eprintln!(":: HOLOCRON: idle {}s -> locked ::", idle.as_secs());
            }
        }
        g.1 = now;
        let ms = now.duration_since(self.start).as_millis() as u64;
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        f(&mut g.0, ms, unix)
    }

    /// Lock now (signal, session end).
    pub fn lock_now(&self) {
        self.svc.lock().unwrap_or_else(|p| p.into_inner()).0.lock_now();
    }
}

/// Bind a private Unix socket at `path` (mode 0600). A stale socket nobody answers on is replaced; a
/// live one, or anything that is not a socket, is an error.
pub fn bind_private(path: &Path) -> io::Result<UnixListener> {
    if let Ok(m) = std::fs::symlink_metadata(path) {
        if !m.file_type().is_socket() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "not a socket"));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, "a Holocron is already listening"));
        }
        std::fs::remove_file(path)?;
    }
    let l = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

fn read_exact_or_eof(s: &mut UnixStream, buf: &mut [u8]) -> io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match s.read(&mut buf[got..])? {
            0 if got == 0 => return Ok(false),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => got += n,
        }
    }
    Ok(true)
}

/// Write one bus reply frame.
pub fn write_reply(s: &mut impl Write, r: &Reply) -> io::Result<()> {
    let mut f = Vec::with_capacity(8 + r.body.len());
    f.extend_from_slice(&((4 + r.body.len()) as u32).to_le_bytes());
    f.extend_from_slice(&r.status.to_le_bytes());
    f.extend_from_slice(&r.body);
    let res = s.write_all(&f);
    holocron_core::zero::wipe(&mut f);
    res
}

/// Serve one bus connection until EOF. A frame longer than `1 + BODY_MAX` or of length 0 ends the
/// connection after an `INVALID` reply (the stream can no longer be trusted to be in sync).
pub fn serve_bus_conn<S: Sealer, G: Signer, T: Store, E: Entropy>(sh: &Shared<S, G, T, E>, mut s: UnixStream) -> io::Result<()> {
    let caller = principal::peer_principal(&s);
    let mut len = [0u8; 4];
    while read_exact_or_eof(&mut s, &mut len)? {
        let n = u32::from_le_bytes(len) as usize;
        if n == 0 || n > 1 + wire::BODY_MAX {
            return write_reply(&mut s, &Reply::err(wire::status::INVALID));
        }
        let mut frame = vec![0u8; n];
        if !read_exact_or_eof(&mut s, &mut frame)? {
            return Ok(());
        }
        let r = sh.with(|h, ms, unix| h.handle(caller.as_deref(), frame[0], &frame[1..], ms, unix));
        holocron_core::zero::wipe(&mut frame);
        write_reply(&mut s, &r)?;
    }
    Ok(())
}

/// Serve one agent connection until EOF.
pub fn serve_agent_conn<S: Sealer, G: Signer, T: Store, E: Entropy>(sh: &Shared<S, G, T, E>, mut s: UnixStream) -> io::Result<()> {
    let caller = principal::peer_principal(&s);
    let mut len = [0u8; 4];
    while read_exact_or_eof(&mut s, &mut len)? {
        let n = u32::from_be_bytes(len) as usize;
        if n == 0 || n > agent::MAX_LEN {
            return s.write_all(&agent::frame(&[agent::FAILURE]));
        }
        let mut msg = vec![0u8; n];
        if !read_exact_or_eof(&mut s, &mut msg)? {
            return Ok(());
        }
        let rsp = sh.with(|h, ms, unix| agent::handle(h, caller.as_deref(), &msg, ms, unix));
        holocron_core::zero::wipe(&mut msg);
        s.write_all(&agent::frame(&rsp))?;
    }
    Ok(())
}

/// Accept forever on `l`, one thread per connection, with `conn` as the handler.
pub fn accept_loop<S, G, T, E>(
    l: UnixListener,
    sh: Arc<Shared<S, G, T, E>>,
    conn: fn(&Shared<S, G, T, E>, UnixStream) -> io::Result<()>,
) where
    S: Sealer + Send + 'static,
    G: Signer + Send + 'static,
    T: Store + Send + 'static,
    E: Entropy + Send + 'static,
{
    for s in l.incoming() {
        let Ok(s) = s else { continue };
        let sh = sh.clone();
        std::thread::spawn(move || {
            let _ = conn(&sh, s);
        });
    }
}

/// Run both sockets (the bus on this thread, the agent on a second). Never returns unless binding fails.
pub fn run<S, G, T, E>(sh: Arc<Shared<S, G, T, E>>, bus: &Path, agent_path: Option<&Path>) -> io::Result<()>
where
    S: Sealer + Send + 'static,
    G: Signer + Send + 'static,
    T: Store + Send + 'static,
    E: Entropy + Send + 'static,
{
    let bl = bind_private(bus)?;
    if let Some(a) = agent_path {
        let al = bind_private(a)?;
        let sh2 = sh.clone();
        std::thread::spawn(move || accept_loop(al, sh2, serve_agent_conn::<S, G, T, E>));
    }
    accept_loop(bl, sh, serve_bus_conn::<S, G, T, E>);
    Ok(())
}
