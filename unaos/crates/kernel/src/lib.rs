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

// NET-PHY: the shared, arch-neutral smoltcp phy::Device adapter (SmoltcpPhy<N: RawNic>). Hosts the
// phy::Device / RxToken / TxToken boilerplate ONCE for every NIC seam — x86 smolnet (e1000e), aarch64
// net4 (RTL8168), aarch64 vnet (virtio-net). Lives at the crate root (NOT under a `net` module: the
// extern crate `net` would be shadowed). Gated on any net feature => vanishes with its smoltcp dep when
// all are off. See net_phy.rs / unaos/docs/dev/OS/08_NET/networking.md.
#[cfg(any(feature = "net4", feature = "vnet", feature = "smolnet", feature = "genet"))]
pub mod net_phy;

// SOCK-1: the smoltcp Device adapter over the e1000e. x86-only + feature-gated so aarch64 and
// knob-off builds never see it (byte-identical). See smolnet.rs / unaos/docs/dev/OS/08_NET/networking.md.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
pub mod smolnet;

pub mod allocator;
pub mod shell;
pub mod selftest;

// INSTALL-CORE: the storage-agnostic installer engine (GPT writer + FAT32 formatter + extent
// content-verify) over the arch-neutral `InstallTarget` trait. The engine is arch-neutral and
// compiles on both arches under `installdemo` (UNAOS_INSTALLDEMO=1); its witness `run_demo` is invoked
// only from the x86_64 boot path this arc (the QEMU scratch disk is x86). Default OFF => the module +
// its call site vanish and every image is byte-identical to baseline. See docs/dev/OS/10_INSTALL/.
// ORIN-INSTALL-1 (`install_target`) also needs the engine: the aarch64 microSD installer flow drives
// this same arch-neutral module, so either feature brings it in (its `run_demo` x86 witness stays
// `installdemo`-only; on an `install_target`-only build the module is compiled and driven from
// `arch::sdmmc_tegra`, no x86 witness call site).
// INSTALL-PI (`piinstall`) also needs the engine: the Pi 4 emmc2 microSD installer flow (crate::install::pi)
// drives this same arch-neutral module onto the seated card via `drivers::emmc2`, gated by the three-gate
// UNAOS_PIINSTALL family and reached from the aarch64 bare-metal boot path (no x86 witness call site).
#[cfg(any(feature = "installdemo", feature = "install_target", feature = "piinstall"))]
pub mod install;

// FLIGHT-RECORDER: capture the serial boot log into a bounded ring and flush it to UNAOS.LOG on the
// FAT boot volume, so a consumer who boots the vm-image with no serial capture can copy the log off
// the image afterward. x86-only (the capture tap lives in arch/x86_64/serial.rs); aarch64 unaffected.
#[cfg(target_arch = "x86_64")]
pub mod flight_recorder;

// RAST-1 / RAST-TEGRA: the `rast` software-rasterizer demo (spinning flat-shaded cube through the
// panel framebuffer path). The module is platform-neutral by construction — it draws only through
// the public `Screen` API — so it is `rast`-feature-gated (UNAOS_RAST=1) but NOT arch-gated: x86/virt
// (RAST-1), aarch64/virt (the QEMU-witnessable panel path), and aarch64/tegra (RAST-TEGRA, the Orin
// panel) all link the same code. Knob-off builds never link it (byte-identical, both arches). See
// docs/dev/OS/08_VIDEO/rasterizer.md.
#[cfg(feature = "rast")]
pub mod rast_demo;

// GUI-WITNESS M1: the boot-milestone recorder — a lock-light, heap-free ring of short milestone
// tags stamped with arch::ms(), written from existing milestone call sites and surfaced on GUI
// builds (where serial is silent and fbcon detaches) via the `bootlog` shell verb. Always linked;
// its record() calls are additive at each site.
pub mod bootlog;

pub mod pal;
pub mod ui;
pub mod ui_status;
pub mod video;
pub mod clock;
pub mod console;
pub mod user;
pub mod splash;
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
