#![cfg(target_os = "windows")]
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


//! Windows 11+ Native Embassy (WinUI 3 / Win32)
//!
//! STUB: Awaiting future expansion.
//! This module will handle the FFI boundary for the Windows runtime,
//! ensuring UnaOS maintains its performance characteristics on Microsoft's host.

use crate::{NativeView, NativeWindow};

pub struct Backend;

impl Backend {
    pub fn new<F>(_app_id: &str, _bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static,
    {
        // TODO: Ignite the Win32/WinUI application host.
        Self {}
    }

    pub fn run(&self) {
        // TODO: Engage the Windows message loop.
    }
}
