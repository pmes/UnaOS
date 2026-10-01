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

//! CHARTER: Kernel — fs-core
//!
//! B302 M4: the timestamp source for the v6 inode meta trailer
//! (`ctime`/`mtime`/`atime`, unix seconds).
//!
//! The crate never owns a clock. An embedder installs one with
//! [`set_clock_hook`] (the kernel: its RTC-backed wall clock); without a hook a
//! `std` build reads `SystemTime`, and a bare `no_std` build stamps 0 — an
//! honest "unknown", never a guess. Same shape as [`crate::warnlog`]: a bare
//! `fn() -> u64` in an atomic, no lock, no dependency, last write wins.

use core::sync::atomic::{AtomicUsize, Ordering};

/// The registered clock, stored as a `usize`-cast `fn() -> u64`. 0 = none.
static CLOCK_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Install the wall clock (unix seconds) the crate stamps inodes with.
pub fn set_clock_hook(hook: fn() -> u64) {
    CLOCK_HOOK.store(hook as usize, Ordering::Release);
}

/// Remove an installed clock hook (tests; an embedder whose clock went bad).
pub fn clear_clock_hook() {
    CLOCK_HOOK.store(0, Ordering::Release);
}

/// Now, in unix seconds: the hook if installed, else `SystemTime` under
/// `std`, else 0.
pub fn now() -> u64 {
    let raw = CLOCK_HOOK.load(Ordering::Acquire);
    if raw != 0 {
        // SAFETY: the only writer is `set_clock_hook`, which stores a valid
        // `fn() -> u64`; a fn pointer round-trips through usize losslessly.
        let f: fn() -> u64 = unsafe { core::mem::transmute::<usize, fn() -> u64>(raw) };
        return f();
    }
    #[cfg(feature = "std")]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
    #[cfg(not(feature = "std"))]
    {
        0
    }
}
