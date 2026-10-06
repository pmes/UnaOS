// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Holocron — shared-core
//!
//! THE store root (HOLOCRONROOT, rmbp-ledger B448; ARCHREVIEW F4). Holocron keeps ONE store per user:
//! `<home>/.holocron/.ring` and `<home>/.holocron/<ns>/<name>`. This module is the only place that root is
//! spelled; the kernel (`keyring.rs`), HOLOCRON.ELF and the host handler all read it from here.
//!
//! Before B448 the metal (kernel + HOLOCRON.ELF, B355) wrote `<home>/.config/unaos/holocron` while the host
//! and this crate's format doc used `<home>/.holocron`. [`LEGACY`] names the old spelling and [`migrate`]
//! moves its records ONCE — secrets first, then the ring, then the legacy files are removed — so an
//! interrupted move simply re-runs. The AEAD associated data binds `ns` and `name`, never the root, so a
//! moved file opens unchanged. A DIFFERENT ring already at the new root is a conflict: nothing moves
//! (secrets sealed under another ring's key would be unreadable there), and the caller says so.

use crate::format::parse_secret;
use crate::name;
use crate::service::{Store, StoreError};
use alloc::string::String;
use alloc::vec::Vec;

/// The store directory under a user's home.
pub const DIR: &str = ".holocron";
/// The ring file inside [`DIR`].
pub const RING: &str = ".ring";
/// Earlier spellings of the root (relative to the home), migrated by [`migrate`] — never written.
pub const LEGACY: &[&str] = &[".config/unaos/holocron"];

fn join(home: &str, rel: &str) -> String {
    let h = home.trim_end_matches('/');
    let mut s = String::with_capacity(h.len() + 1 + rel.len());
    s.push_str(h);
    s.push('/');
    s.push_str(rel);
    s
}

/// `<home>/.holocron` (no trailing slash).
pub fn root(home: &str) -> String {
    join(home, DIR)
}

/// `<home>/.holocron/.ring`.
pub fn ring_path(home: &str) -> String {
    let mut s = root(home);
    s.push('/');
    s.push_str(RING);
    s
}

/// `<home>/.holocron/<ns>/<name>`.
pub fn secret_path(home: &str, ns: &str, name: &str) -> String {
    let mut s = root(home);
    s.push('/');
    s.push_str(ns);
    s.push('/');
    s.push_str(name);
    s
}

/// Every legacy root under `home`, oldest spelling last.
pub fn legacy_roots(home: &str) -> Vec<String> {
    LEGACY.iter().map(|r| join(home, r)).collect()
}

/// The namespaces in a directory listing of a root: one entry per line, a directory ends in `/` (the
/// metal's `PATH_R_LIST` and the host's `read_dir` both render to this). Only valid names survive; files
/// (`.ring`) and anything a path could reinterpret are dropped.
pub fn namespaces(listing: &[u8]) -> Vec<String> {
    listing
        .split(|&c| c == b'\n')
        .filter_map(|l| l.strip_suffix(b"/"))
        .filter_map(|l| core::str::from_utf8(l).ok())
        .filter(|n| name::valid(n))
        .map(String::from)
        .collect()
}

/// What the ring did in a [`migrate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingMove {
    /// The legacy root had no ring.
    None,
    /// The legacy ring now lives at the new root.
    Moved,
    /// The new root already held the same ring (an interrupted move, re-run).
    Same,
    /// The new root holds a DIFFERENT ring: nothing was moved.
    Conflict,
}

impl RingMove {
    /// The wire word.
    pub fn word(self) -> &'static str {
        match self {
            RingMove::None => "none",
            RingMove::Moved => "moved",
            RingMove::Same => "same",
            RingMove::Conflict => "conflict",
        }
    }
}

/// The outcome of a [`migrate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved {
    /// The ring.
    pub ring: RingMove,
    /// Secrets now at the new root (copied, or already there byte-identical).
    pub secrets: usize,
    /// Secrets left at the legacy root: an unparseable file, an invalid name, or a different file of the
    /// same name already at the new root.
    pub skipped: usize,
}

impl Moved {
    /// True when the legacy root held nothing at all.
    pub fn empty(&self) -> bool {
        self.ring == RingMove::None && self.secrets == 0 && self.skipped == 0
    }
    /// The verdict word: `ok`, `partial` (something stayed behind) or `conflict`.
    pub fn verdict(&self) -> &'static str {
        match (self.ring, self.skipped) {
            (RingMove::Conflict, _) => "conflict",
            (_, 0) => "ok",
            _ => "partial",
        }
    }
}

/// Move every record of `from` (a legacy root) into `to` (THE root), once. `namespaces` are the legacy
/// root's namespace directories ([`namespaces`] over its listing). Order: each secret is written to `to`,
/// then the ring, and only then are the legacy files removed — so a crash at any point leaves every
/// record readable at one root or the other, and the next call finishes the move.
pub fn migrate<A: Store, B: Store>(from: &mut A, to: &mut B, namespaces: &[String]) -> Result<Moved, StoreError> {
    let old_ring = from.read_ring()?;
    let new_ring = to.read_ring()?;
    let ring = match (&old_ring, &new_ring) {
        (None, _) => RingMove::None,
        (Some(_), None) => RingMove::Moved,
        (Some(a), Some(b)) if crate::ct_eq(a, b) => RingMove::Same,
        (Some(_), Some(_)) => RingMove::Conflict,
    };
    let mut out = Moved { ring, secrets: 0, skipped: 0 };
    if ring == RingMove::Conflict {
        for ns in namespaces {
            out.skipped += from.list(ns)?.len();
        }
        return Ok(out);
    }
    let mut done: Vec<(String, String)> = Vec::new();
    for ns in namespaces.iter().filter(|n| name::valid(n)) {
        for nm in from.list(ns)? {
            let Some(file) = (if name::valid(&nm) { from.read(ns, &nm)? } else { None }) else {
                out.skipped += 1;
                continue;
            };
            let Ok((hdr, _, _)) = parse_secret(&file) else {
                out.skipped += 1;
                continue;
            };
            match to.read(ns, &nm)? {
                Some(there) if crate::ct_eq(&there, &file) => {}
                Some(_) => {
                    out.skipped += 1;
                    continue;
                }
                None => to.write(ns, &nm, &file, &hdr.meta)?,
            }
            out.secrets += 1;
            done.push((ns.clone(), nm));
        }
    }
    if let (RingMove::Moved, Some(r)) = (ring, &old_ring) {
        to.write_ring(r)?;
    }
    for (ns, nm) in &done {
        from.remove(ns, nm)?;
    }
    if ring != RingMove::None && out.skipped == 0 {
        from.remove_ring()?;
    }
    Ok(out)
}
