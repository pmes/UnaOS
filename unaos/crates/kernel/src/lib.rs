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

// SNTP-X86: the shared, arch-neutral RFC 4330 SNTP reply parser + request builder. The x86 smolnet
// SNTP client (`smolnet::sntp_*`) renders civil time through it; the pi/genet PI-NET-16 client migrates
// onto it in a later fold. Pure no_std/no-alloc parser (all `pub`, so no dead-code warning where a given
// arch has no consumer yet). See net_sntp.rs.
pub mod net_sntp;

// SOCK-8: the shared, arch-neutral DNS A-record query builder + response parser. The x86 smolnet resolver
// (`smolnet::resolve`) renders name lookups through it; the pi/genet PI-NET-14 client migrates onto it in a
// later fold. Pure no_std/no-alloc (all `pub`, so no dead-code warning where a given arch has no consumer
// yet). See net_dns.rs.
pub mod net_dns;

// WIFI-1: the BCM4331 firmware-load path (`UNAOS_WIFI=1`) — PCI-config identification of the AirPort
// radio (class 0x02 / subclass 0x80), cross-checked against the metal facts bcm4331.md §0 pinned,
// plus the user-supplied firmware SET located, validated and staged off the program-source FAT
// volume. Read-only in arc 1: no BAR mapping, no device MMIO, no association. x86_64-only (the radio
// is an rMBP part) and `wifi` is stripped by arroyo's `arm_features`, so arming it never shifts a
// Pi/Jetson media hash. Default OFF => the module and its three main-loop call sites vanish and every
// image is byte-identical to baseline. See wifi/mod.rs,
// docs/dev/OS/06_NETWORK_STACK/wifi_bcma.md and docs/dev/OS/06_NETWORK_STACK/bcm4331.md.
#[cfg(all(feature = "wifi", target_arch = "x86_64"))]
pub mod wifi;

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

// BPACE: the boot-phase TIMING ledger — bootlog's sibling, stamped with the free-running counter
// (rdtsc / CNTVCT) instead of the APIC tick, so it can measure the whole boot including everything
// that happens before `apic::calibrate`. Always linked, never gated: the ledger has to exist in the
// build that actually boots on metal.
pub mod bootpace;

pub mod pal;
pub mod ui;
pub mod ui_status;
// The in-kernel 3D sculptor and the full-screen `pulse` monitor — the `vug`, `vug bebox`, `vug wire`
// and `pulse` shell verbs, plus `parked_display_witness` (called from `arch::aarch64::sched`).
//
// Aarch64 only, and the gate is the whole story of the module's ownership. The x86 trunk deleted
// vug.rs deliberately; the R23 merge took the pi4 body wholesale and restored the verbs on x86 as a
// side effect, so this gate is what puts x86 back where it meant to be while leaving the Pi exactly
// as it is. Nothing outside aarch64 needs the module: the pieces that ARE shared — the meter palette
// (`METER_DIM`/`METER_BREATH`/`METER_PARKED`), the `PARKED` sentinel and the VUG-HONESTY
// `classify_load_scaled` rule the instrument strip reads — were moved into `ui_status`, which owns
// them outright and compiles on both arches; `vug` imports them back. `shell.rs` gates its `vug` and
// `pulse` arms to match, so on x86 those words reach the ordinary unknown-command reply.
//
// (The comment this replaces justified the restore with "`run_bsp` is why user vugs run". `run_bsp`
// appears nowhere in vug.rs — it lives in `arch/{aarch64,x86_64}/sched.rs`, and the EL0 vug is the
// `VUG.ELF` vessel, not this module. Only the classifier half of that claim was ever true, and it
// now points at `ui_status`.)
#[cfg(target_arch = "aarch64")]
pub mod vug;
pub mod video;
pub mod clock;
#[cfg(feature = "logts")]
pub mod logts;
pub mod console;
pub mod user;
pub mod splash;
pub mod gui_watchdog;
// WEDGE-2: the last-words breadcrumb instrument for the TAB->focus-raise chain. ALWAYS declared (its
// public surface degrades to empty `#[inline(always)]` shims when the `wedge2` feature is off), so the
// call sites in `video/wm.rs` and the focus seam stay `#[cfg]`-free and arch-neutral — which is what
// makes this diff inheritable by the x86 tree. Knob-off is byte-inert.
pub mod wedge2;
// SERWIT-1: the serial staging ring — the arch-neutral half of "no line leaves the wire unaccounted
// for". Declared unconditionally (both arches' `_print` go through it and the panic escape hatch is
// not knob-gated); costs ~16 KiB of `.bss` and introduces no lock. See the module docs for the
// deadlock analysis that keeps the panic and WEDGE-2/WEDGE-4 breadcrumb paths unblockable.
pub mod serial_ring;
// TERM_RING (MIDDEN_CONVERGENCE §3, M2): the bounded terminal OUTPUT transport between a producer and
// whatever task owns the console view. Built on `serial_ring::LineRing` — lock-free, alloc-free,
// drop-newest with a counted refusal — so a producer in an IRQ-masked or print-locked context can emit
// a console line without blocking and without allocating. Arch-neutral; costs ~15 KiB of `.bss`.
pub mod termring;

pub fn init() {
    arch::init();
}



pub fn hlt_loop() -> ! {
    arch::hlt_loop()
}

pub fn hlt() {
    arch::hlt()
}
