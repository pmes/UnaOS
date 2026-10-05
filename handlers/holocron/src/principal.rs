// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The host's principal stamp. On UnaOS the kernel stamps every bus frame with the caller's principal
//! (BANDY3); on a host the kernel's equivalent is the peer credential of a Unix socket (SO_PEERCRED),
//! which the caller cannot forge. Both project to the same canonical string, `user:<name>#<uid>`
//! ([`holocron_core::wire::user_principal`]).

use std::os::unix::net::UnixStream;

/// The login name for `uid` from `/etc/passwd` (`None` if absent or the name is not a valid UnaOS
/// user name).
pub fn user_name(uid: u32) -> Option<String> {
    let pw = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in pw.lines() {
        let mut f = line.split(':');
        let (Some(name), _, Some(id)) = (f.next(), f.next(), f.next()) else { continue };
        if id.parse::<u32>().ok() == Some(uid) {
            return holocron_core::name::valid(name).then(|| name.to_string());
        }
    }
    None
}

/// `user:<name>#<uid>` for a uid, or `None` when the uid has no valid name.
pub fn principal_of_uid(uid: u32) -> Option<String> {
    user_name(uid).map(|n| holocron_core::wire::user_principal(&n, uid))
}

/// The principal the OS stamps on the peer of `s`.
pub fn peer_principal(s: &UnixStream) -> Option<String> {
    let cred = s.peer_cred().ok()?;
    principal_of_uid(cred.uid)
}

/// The principal of this process.
pub fn my_principal() -> Option<String> {
    principal_of_uid(crate::store::my_uid())
}
