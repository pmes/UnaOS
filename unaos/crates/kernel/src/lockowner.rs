// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — kernel-by-ruling
//! STACKGUARD2 (rmbp-ledger B403) — WHO HOLDS a lock a dead task may have taken with it.
//!
//! A task halted by a stack overflow (`arch/x86_64/stackguard.rs`) never runs its guards' `Drop`s, so every
//! lock it held stays held. Flight 25's FLAC wedge left the wire SILENT: on the rMBP the wire IS the FTDI
//! capture ring (`drivers/xhci/ftdi.rs`), a `try_lock` sink — a holder that never returns turns every later
//! line into a staged line nobody drains. This is the smallest honest answer: a tracked lock records its
//! holder's TASK ID for exactly as long as it is held (an [`Own`] token declared AFTER the guard, so it drops
//! first), and the overflow path releases only the locks whose recorded holder IS the dead task — what it is
//! KNOWN to hold, never a guess. Tracking is off until the scheduler's per-CPU state exists ([`enable`]); on
//! aarch64 it stays off (no stack-guard fault path there yet), so the token is a no-op.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The FTDI capture ring (`drivers/xhci/ftdi.rs` `RING`) — the rMBP's wire.
pub const SINK: usize = 0;
/// The 16550 (`arch::serial::SERIAL1`).
pub const UART: usize = 1;
/// The UnaFS mount (`fs/unafs.rs` `MOUNT`).
pub const UNAFS: usize = 2;
const N: usize = 3;
/// The names `[lock] released-on-death name=<sink|uart|unafs> …` prints (STACKGUARD3 B832; `sync::release_task`).
pub const NAMES: [&str; N] = ["sink", "uart", "unafs"];

static ON: AtomicBool = AtomicBool::new(false);
static OWNER: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

/// Turn tracking on (x86: once the BSP's per-CPU block exists — `stackguard::arm_cpu(0)`).
pub fn enable() {
    ON.store(true, Ordering::Release);
}

#[inline]
fn me() -> u64 {
    if !ON.load(Ordering::Acquire) {
        return 0;
    }
    #[cfg(target_arch = "x86_64")]
    {
        // GS must be the kernel's per-CPU block: a print from a GS-free context (#MC, a #DB in the syscall
        // entry window) reads no per-CPU state — it records no holder, which only costs a release.
        if crate::serial_ring::in_panic_mode() {
            return 0;
        }
        let gs = unsafe { x86_64::registers::model_specific::Msr::new(0xC000_0101).read() };
        let (Some(c0), Some(cl)) = (crate::arch::percpu::cpu(0), crate::arch::percpu::cpu(crate::arch::gdt::MAX_CPUS - 1)) else { return 0 };
        let (lo, hi) = (c0 as *const _ as u64, cl as *const _ as u64);
        if gs < lo || gs > hi {
            return 0;
        }
        crate::arch::sched::current_id().unwrap_or(0)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

/// The holder token: records the current task as `lock`'s holder; clears it on drop. Declare it right
/// AFTER the guard it shadows so it drops BEFORE the lock is released.
pub struct Own(usize, u64);

#[inline]
pub fn own(lock: usize) -> Own {
    let id = me();
    if id != 0 {
        OWNER[lock].store(id, Ordering::Release);
    }
    Own(lock, id)
}

impl Drop for Own {
    #[inline]
    fn drop(&mut self) {
        if self.1 != 0 {
            let _ = OWNER[self.0].compare_exchange(self.1, 0, Ordering::Release, Ordering::Relaxed);
        }
    }
}

/// Is `lock` recorded as held by task `tid`? (The overflow path, on the dead task's own core.)
pub fn held_by(lock: usize, tid: u64) -> bool {
    tid != 0 && OWNER[lock].load(Ordering::Acquire) == tid
}

/// Forget `lock`'s holder (after its owner force-released it).
pub fn clear(lock: usize) {
    OWNER[lock].store(0, Ordering::Release);
}

// ── STACKGUARD3 (rmbp-ledger B832) — the dead task's tracked locks, released FIRST by `sync::release_task` ──────────
// `stackguard::release_held`'s body, MOVED here (it was called beside `release_task` at three death sites). Each of the
// three is a `sync::Mutex` whose `force_unlock` forgets its registry entry, so `release_task`'s walk never sees it again:
// these three are released UNPOISONED (their owners' releases are their recovery: the sink and the UART restart at a
// line boundary, the mount is leaked and re-bound from the medium by the next `with_unafs`).

/// Force-release every tracked lock whose recorded holder is `tid` (a task that never runs again); the bit set of what
/// was released (bit `i` = [`NAMES`]`[i]`). Off on aarch64 (no tracked holder is ever recorded there).
pub fn release_dead(tid: u64) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        let mut out = 0u32;
        if held_by(SINK, tid) {
            clear(SINK);
            crate::drivers::xhci::ftdi::lockowner_release();
            out |= 1 << SINK;
        }
        if held_by(UART, tid) {
            clear(UART);
            crate::arch::serial::lockowner_release();
            out |= 1 << UART;
        }
        #[cfg(feature = "unafs")]
        if held_by(UNAFS, tid) {
            clear(UNAFS);
            crate::fs::unafs::lockowner_release();
            out |= 1 << UNAFS;
        }
        out
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = tid;
        0
    }
}
