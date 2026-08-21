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

//! FATFIX M2 — **the conviction instrument for the slow listing and the slow launch.**
//!
//! Peter, on metal (baton pi-1 item 8a, and again at the PA44 sitting): the FAT listing is *VERY
//! SLOW*, and a double-click takes a long time to start the program. Today's wire already ruled out
//! the second half of the launch — `spawn`→first-present is fast — which leaves the READ. This
//! module measures the read instead of arguing about it.
//!
//! What it prints, once per operation, and nothing else:
//!
//! ```text
//! [fatperf] op=list path=/fat sectors=222 us=13456
//! [fatperf] op=read path=/fat/VUG.ELF sectors=41 us=2210
//! ```
//!
//! Four RAW words. `sectors` is 512-byte sector reads the block layer actually performed under the
//! operation — counted at the FAT driver's two read funnels ([`crate::fs::fat`]'s `read_sector` and
//! `read_sectors`), so a chunked multi-block transfer counts as the sectors it moved, not as one
//! call. `us` is wall time across the whole VFS operation, from `MountTable::resolve` to the
//! backend's answer, which is what an operator waits for. Neither is a rate, a ratio or a verdict:
//! a derived number is a number someone has already interpreted, and the point of this arc is to
//! hand the FIX arc the two terms it needs to divide for itself.
//!
//! **The one honest caveat on `sectors`.** It is a delta of a GLOBAL census across the operation's
//! window, not a per-operation counter, so a FAT read issued by another context inside that window
//! is attributed to this line. On this kernel's serialised boot that is rare; making it exact would
//! need a per-CPU or per-call accumulator, which is a mechanism to add when a measurement actually
//! turns on the distinction rather than before.
//!
//! **Why the clock is `CNTVCT_EL0` and not `ticks()`.** On QEMU raspi4b the periodic timer IRQ is
//! never delivered and `timer::ticks()` stays frozen at 0 (UVUG-7's measurement), so a tick-derived
//! elapsed time would read `us=0` for every operation on the entire QEMU battery — the instrument
//! would be vacuous exactly where it is first exercised. `now_cycles()` is the free-running virtual
//! counter and `cntfrq()` its rate, which is the same pair `arch::ms()` was moved onto for the same
//! reason. A board whose `CNTFRQ_EL0` reads 0 gets `us=0` rather than a divide fault, and says so by
//! being obviously zero rather than plausibly small.
//!
//! **Scope.** This is an instrument, not a fix. It changes no behaviour, caches nothing and refuses
//! nothing. The repair — cluster-chain caching, or migration onto ORIN's UnaFS — is deliberately a
//! later arc: the baton forbids building on the FAT namespace twice, and a fix chosen before the
//! cost is measured is a guess.
//!
//! **Knob.** `UNAOS_FATPERF=1` (cargo feature `fatperf`). OFF, this file is not compiled, the wrapper
//! in [`crate::fs`] inlines to nothing, the two `fat.rs` funnel calls do not exist before MIR, and
//! `kernel8.img` is byte-identical to baseline (measured: `3a280f9d…` both ways). `crate::fs::perf_op`
//! carries the part of that which was NOT obvious and had to be measured.

use core::sync::atomic::{AtomicU64, Ordering};

/// Every 512-byte sector the FAT driver has read since boot, counted at the driver's two read
/// funnels. `Relaxed` throughout: this is a census, not a synchronisation point, and no decision
/// anywhere in the kernel is taken on its value.
static SECTORS: AtomicU64 = AtomicU64::new(0);

/// Note `n` sectors read. Called from `fat::read_sector` (n = 1) and `fat::read_sectors`
/// (n = the chunk's sector count), which between them are every path by which a FAT byte reaches
/// memory — the BPB probe, the FAT table walk, each directory sector and each data cluster.
#[inline]
pub fn note_sectors(n: u64) {
    SECTORS.fetch_add(n, Ordering::Relaxed);
}

/// Convert an ELAPSED count of virtual-counter cycles to microseconds, at the counter's own rate.
///
/// The conversion is applied to the DELTA and never to an absolute `CNTVCT_EL0` reading, which is
/// not fussiness: `delta * 1_000_000` overflows `u64` past ~1.8e13 cycles, and an absolute counter
/// reaches that in a few days of uptime while a single directory read never will. `CNTFRQ_EL0 == 0`
/// (a board that never programmed the timebase) answers 0 rather than dividing by it — obviously
/// zero, rather than plausibly small.
#[inline]
fn cycles_to_us(delta: u64) -> u64 {
    let hz = crate::arch::aarch64::timer::cntfrq();
    if hz == 0 {
        return 0;
    }
    delta.saturating_mul(1_000_000) / hz
}

/// Run one VFS operation with the counter and the clock read either side of it, and put the two
/// raw words on the wire.
///
/// The measurement brackets the WHOLE operation — including `FatBackend::read_dir`'s first line,
/// which is a full `fat::mount_source` volume probe (LBA 0, the MBR census, the superfloppy BPB
/// attempt, the GPT scan, and a BPB sector per accepted partition). That probe is the term
/// `quarry.md` §11.1 convicted as the per-CALL cost, and it belongs inside the bracket precisely
/// because it is what the operator is waiting through.
pub fn measure<T>(op: &str, path: &str, f: impl FnOnce() -> T) -> T {
    let s0 = SECTORS.load(Ordering::Relaxed);
    let c0 = crate::arch::now_cycles();
    let out = f();
    let us = cycles_to_us(crate::arch::now_cycles().saturating_sub(c0));
    let sectors = SECTORS.load(Ordering::Relaxed).saturating_sub(s0);
    serial_println!(
        "[fatperf] op={} path={} sectors={} us={}",
        op,
        path,
        sectors,
        us
    );
    out
}
