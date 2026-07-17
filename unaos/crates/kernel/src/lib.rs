#![no_std]
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

#![cfg_attr(test, no_main)]
// abi_x86_interrupt is only used by the x86_64 interrupt handlers; gating it keeps the
// aarch64 build free of the "unused feature" warning.
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]
#![allow(unsafe_op_in_unsafe_fn)]

extern crate alloc;

#[macro_use]
pub mod arch;

pub mod drivers;
pub mod fs;

// SOCK-1: the smoltcp Device adapter over the e1000e. x86-only + feature-gated so aarch64 and
// knob-off builds never see it (byte-identical). See smolnet.rs / docs/dev/OS/08_NET/networking.md.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
pub mod smolnet;

pub mod allocator;
pub mod shell;
pub mod selftest;

// FLIGHT-RECORDER: capture the serial boot log into a bounded ring and flush it to UNAOS.LOG on the
// FAT boot volume, so a consumer who boots the vm-image with no serial capture can copy the log off
// the image afterward. x86-only (the capture tap lives in arch/x86_64/serial.rs); aarch64 unaffected.
#[cfg(target_arch = "x86_64")]
pub mod flight_recorder;

pub mod pal;
pub mod ui;
pub mod video;
pub mod clock;
pub mod console;
pub mod user;
pub mod vug;

pub fn init() {
    arch::init();
}



pub fn hlt_loop() -> ! {
    arch::hlt_loop()
}

pub fn hlt() {
    arch::hlt()
}
