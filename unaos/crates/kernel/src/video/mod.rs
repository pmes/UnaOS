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
//! - [`screen::Screen`] — a double-buffered surface with damage tracking, built on two
//!   `FrameBuffer`s (the framebuffer + a cached-RAM back buffer). The steady-state GUI renderer.
//! - [`fbcon`] — the boot/panic text console (a log sink for hardware with no serial port),
//!   drawn straight to its own `FrameBuffer` handle (it runs pre-heap, so no back buffer).
//! - [`wm`] — the window table and compositor: EL0 surfaces composited onto the panel with
//!   kernel-drawn chrome. `screen::present_surface` is a compat shim over its window 0.
//! - [`WRITER`] — the framebuffer the GUI's `Screen` flushes to and that fbcon mirrors onto.
//!
//! `WRITER` and `fbcon` are handles to the *same* physical framebuffer; they are used at
//! different times (fbcon during boot, the GUI after a successful boot repaints over it) and
//! each is serialised by its own lock.

pub mod fbcon;
pub mod framebuffer;
// CRISPY-PI: the Crispy theme lifted from `kits/crispy/theme.json` (@ us-crispy 08b42ede) into a
// kernel `const` table — packed 0x00RRGGBB palette roles + integer metrics, no float at runtime.
// Both arches source chrome constants from here; no per-arch invented numbers. Nothing consumes it
// yet (wiring is a follow-up arc), so it is byte-inert: all `const`, no statics, no code.
pub mod theme;
pub mod screen;
// WC-A: the window table + compositor. Owns which pixels of which surface reach the panel; the
// aarch64 window syscalls (WC-B) are thin fail-closed wrappers over its API.
pub mod wm;
// CURSOR-1: the system cursor sprite, drawn into the FRONT framebuffer as the last painter of a
// pass (save-under, not a recomposite) so it sits on top of both the console and every window.
// Pointer position + auto-hide state stay in `pal::cursor`; this module only paints.
pub mod cursor;
// VWIT: headless regression witness for the damage-tracked `Screen` present path (arch-neutral;
// runs only when the `tste` self-test is invoked). See `docs/dev/OS/08_VIDEO/engine.md` §7.
pub mod witness;
// WC-F: the scan-out ground-truth probe — the live FrameBuffer checked against the firmware's own
// geometry, plus a twin-pattern render (compositor addressing vs firmware-pitch addressing) that
// discriminates a blit-path defect from a scan-out one. `witness`-gated AND aarch64-only, so the
// flashable Pi media and every x86 artifact are byte-identical with it absent.
#[cfg(all(target_arch = "aarch64", feature = "witness", feature = "baremetal"))]
pub mod wcf;
// WC-G: the window PRESENT path instrumented while it runs — four checksums of one surface around
// one blit, a scan-out read-back, and the blit's duration, which together separate a source race
// from a coherency fault from a blit-path defect from an unbuffered-copy timing defect. Same gating
// as `wcf` (witness + aarch64 only), so the flashable media stay byte-identical.
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
pub mod wcg;
// VPERF instrumentation (counters, fbmem readout, PCI display probe, scripted scroll scenario).
// x86-only AND knob-gated: with the feature off — or on aarch64 regardless — nothing here
// compiles, so those artifacts stay byte-identical.
#[cfg(all(target_arch = "x86_64", feature = "videobench"))]
pub mod vperf;

pub use framebuffer::FrameBuffer;
pub use screen::Screen;

use spin::Mutex;

/// The primary display surface. The GUI renderer (`pal::TargetPal` → `console`) draws here.
/// Initialised once in `kernel_main` from `BootInfo` (UEFI GOP, or the Pi mailbox framebuffer).
pub static WRITER: Mutex<FrameBuffer> = Mutex::new(FrameBuffer::new());
