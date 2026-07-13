// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Minimal `no_std` warning hook (BeFS-K3 observability seam).
//!
//! Under `std` the crate has always printed operational warnings (the dirty-mount
//! notice in [`crate::fs::UnaFS::mount`]) with `println!` — which compiles out
//! under `no_std`, leaving a kernel mount SILENT about a torn journal. This
//! module gives an embedder one function pointer to receive those warnings:
//! the kernel wires it to its console once at boot, hosts can ignore it (the
//! `std` `println!` path is unchanged and independent of the hook).
//!
//! The hook is a bare `fn(&str)` stored in an atomic, so the seam adds no
//! dependency and no lock; setting it twice last-write-wins.

use core::sync::atomic::{AtomicUsize, Ordering};

/// The registered warning sink, stored as a `usize`-cast `fn(&str)`.
/// 0 = no hook installed.
static WARN_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Install a warning sink. Warnings emitted by the crate (e.g. the dirty-mount
/// notice) are forwarded to `hook` as single-line messages without a trailing
/// newline. Call once at bring-up; a later call replaces the hook.
pub fn set_warn_hook(hook: fn(&str)) {
    WARN_HOOK.store(hook as usize, Ordering::Release);
}

/// Forward `msg` to the installed hook, if any.
pub(crate) fn warn(msg: &str) {
    let raw = WARN_HOOK.load(Ordering::Acquire);
    if raw != 0 {
        // SAFETY: the only writer is `set_warn_hook`, which stores a valid
        // `fn(&str)`; a fn pointer round-trips through usize losslessly.
        let f: fn(&str) = unsafe { core::mem::transmute::<usize, fn(&str)>(raw) };
        f(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicBool;

    static FIRED: AtomicBool = AtomicBool::new(false);

    fn sink(msg: &str) {
        assert!(msg.contains("test-warning"));
        FIRED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn hook_receives_warnings() {
        warn("dropped: no hook yet"); // must not crash with no hook
        set_warn_hook(sink);
        warn("test-warning");
        assert!(FIRED.load(Ordering::SeqCst));
    }
}
