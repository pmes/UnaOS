// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The consumer rule: a program that needs a credential asks Holocron FIRST and falls back to its old
//! source (an env var, a key file) only when Holocron says the secret is not there. ONE function, linked
//! by the host `handlers/vein` today and by `vein_ring3` on the metal at its fold, so both rings decide
//! the same way.
//!
//! | Holocron's answer | decision |
//! |---|---|
//! | OK + bytes | use the bytes |
//! | `NOT_FOUND` (-2) | fall back |
//! | unavailable — no Holocron is running (host: no socket; metal: the kernel's own -ENOENT for an unregistered verb, which is the same errno and the same decision) | fall back |
//! | `LOCKED` | refuse, naming the fix (`holocron unlock`) — the key IS in the ring; reading a stale env copy instead would hide the lock |
//! | `DENIED` | refuse — another principal's ring answered |
//! | anything else | refuse with the status |

use crate::wire::status;
use crate::zero::SecretBytes;

/// Holocron's namespace for Vein.
pub const VEIN_NS: &str = "vein";
/// The Claude API key's name in that namespace.
pub const CLAUDE_API_KEY: &str = "claude.api_key";

/// What asking Holocron produced.
#[derive(Debug)]
pub enum Answer {
    /// The secret.
    Found(SecretBytes),
    /// A reply status other than OK.
    Status(i32),
    /// No Holocron to ask.
    Unavailable,
}

/// The decision.
#[derive(Debug)]
pub enum KeySource {
    /// Use these bytes.
    Holocron(SecretBytes),
    /// Use the old source.
    Fallback,
    /// Use nothing; the reason names the fix.
    Refuse(&'static str),
}

/// Apply the rule.
pub fn decide(a: Answer) -> KeySource {
    match a {
        Answer::Found(b) => KeySource::Holocron(b),
        Answer::Unavailable | Answer::Status(status::NOT_FOUND) => KeySource::Fallback,
        Answer::Status(status::LOCKED) => KeySource::Refuse("Holocron is locked: run `holocron unlock`"),
        Answer::Status(status::DENIED) => KeySource::Refuse("Holocron refused this principal"),
        Answer::Status(status::CORRUPT) => KeySource::Refuse("Holocron could not authenticate the stored key"),
        Answer::Status(_) => KeySource::Refuse("Holocron failed to answer"),
    }
}
