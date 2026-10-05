// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The wall clock certificate validity is judged against (VEINTLS, SR36): SYS_TIME, the kernel's civil clock.
//! An unanchored clock reads 0 here, which is before `tls::CLOCK_FLOOR`, so `tls::with_session` refuses
//! the handshake with `clock-unset` rather than accept or reject certificates against a guessed time.

/// [`tls_core::x509::Clock`] over SYS_TIME.
pub struct SysClock;

impl tls_core::x509::Clock for SysClock {
    fn now(&self) -> i64 {
        crate::sys::unix_time().unwrap_or(0)
    }
}

/// Whether the kernel has a wall clock this boot (`SYS_TIME` answered at or after the floor).
pub fn is_set() -> bool {
    crate::sys::unix_time().is_ok_and(|t| t >= crate::tls::CLOCK_FLOOR)
}
