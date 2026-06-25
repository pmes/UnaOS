// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The video subsystem.
//!
//! - [`framebuffer::FrameBuffer`] — the one pixel-format-aware drawing surface; every pixel on
//!   screen goes through it.
//! - [`fbcon`] — the boot/panic text console (a log sink for hardware with no serial port),
//!   built on its own `FrameBuffer` handle.
//! - [`WRITER`] — the primary display surface the GUI renderer draws to (via `pal`/`console`).
//!
//! Both `WRITER` and `fbcon` are handles to the *same* physical framebuffer; they are used at
//! different times (fbcon during boot, the GUI after a successful boot repaints over it) and
//! each is serialised by its own lock. Phase 3 (double-buffering / a compositor) is where this
//! grows a back buffer and a single flush path.

pub mod fbcon;
pub mod framebuffer;

pub use framebuffer::FrameBuffer;

use spin::Mutex;

/// The primary display surface. The GUI renderer (`pal::TargetPal` → `console`) draws here.
/// Initialised once in `kernel_main` from `BootInfo` (UEFI GOP, or the Pi mailbox framebuffer).
pub static WRITER: Mutex<FrameBuffer> = Mutex::new(FrameBuffer::new());
