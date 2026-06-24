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

pub mod allocator;
pub mod shell;

pub mod pal;
pub mod writer;
pub mod fbcon;
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
