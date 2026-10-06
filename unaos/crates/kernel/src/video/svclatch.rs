//! CHARTER: Kernel — wm
//!
//! SVCLATCH (rmbp-ledger B462, PERFREVIEW F8) — the desktop service passes' latches, checked before the lock.
//!
//! A click or a key stores a request in a `Mutex<Option<T>>` (the router may not do the work); the render
//! pass drains it. Before this, every pass took the lock to find it empty. A [`Latch`] is the flag the
//! producer raises AFTER it stores: the pass reads it (one relaxed load when quiet), lowers it, and only
//! then takes the lock. A producer racing the drain leaves the flag up, so the worst case is one empty
//! lock on the next pass (counted: `idle_locks`), never a lost request. QUERYFOLDER's counter model.
//!
//! `tests svclatch` (R80: a verb, never at boot) prints the four patches' witness:
//! `:: SVCLATCH: passes_idle_locks=<n> comp_alloc_per_pass=<n> damage_gen=<ok|FAIL> -> PASS|FAIL :: … ::`.

use crate::sync::Mutex;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Passes that found a latch posted and took its lock.
static OPENS: AtomicU32 = AtomicU32::new(0);
/// Of those, the ones that found nothing under the lock (a producer raced the previous drain).
static IDLE_LOCKS: AtomicU32 = AtomicU32::new(0);

/// A posted-flag in front of one latch mutex.
pub struct Latch(AtomicBool);

impl Latch {
    pub const fn new() -> Self {
        Latch(AtomicBool::new(false))
    }

    /// The producer, AFTER its store under the latch's lock returned.
    #[inline]
    pub fn post(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// The pass: true when something was posted since the last drain (the flag is lowered; the caller
    /// then takes its lock). A quiet pass is one relaxed load and no lock.
    #[inline]
    pub fn open(&self) -> bool {
        if !self.0.load(Ordering::Relaxed) {
            return false;
        }
        if !self.0.swap(false, Ordering::AcqRel) {
            return false;
        }
        OPENS.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Re-raise (a partial drain left work queued). Called under the latch's own lock.
    #[inline]
    pub fn settle(&self, more: bool) {
        self.0.store(more, Ordering::Release);
    }

    /// `m.lock().take()`, gated.
    #[inline]
    pub fn take<T>(&self, m: &Mutex<Option<T>>) -> Option<T> {
        if !self.open() {
            return None;
        }
        let v = m.lock().take();
        found(v.is_some());
        v
    }

    /// `mem::take(&mut *m.lock())`, gated (an empty `Vec` allocates nothing).
    #[inline]
    pub fn take_vec<T>(&self, m: &Mutex<Vec<T>>) -> Vec<T> {
        if !self.open() {
            return Vec::new();
        }
        let v = core::mem::take(&mut *m.lock());
        found(!v.is_empty());
        v
    }
}

/// A gated pass reports whether its lock held anything.
#[inline]
pub fn found(hit: bool) {
    if !hit {
        IDLE_LOCKS.fetch_add(1, Ordering::Relaxed);
    }
}

// ── `tests svclatch` (R80: never at boot) ───────────────────────────────────────────────────────

/// Registered lazily from `wm::service_damage` (every flush, both arches): one relaxed load once done.
#[inline]
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.load(Ordering::Relaxed) && !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("svclatch", selftest);
    }
}

fn selftest() {
    let opens = OPENS.load(Ordering::Relaxed);
    let idle = IDLE_LOCKS.load(Ordering::Relaxed);
    let (passes, grows, heap) = super::wm::comp_scratch_counts();
    let per_pass = if passes == 0 { 0 } else { grows / passes };
    let (walks, skips, gen_ok) = super::wm::damage_gen_check();
    let sgen = crate::prefs::session_gen();
    let pass = idle == 0 && per_pass == 0 && gen_ok;
    serial_println!(
        ":: SVCLATCH: passes_idle_locks={} comp_alloc_per_pass={} damage_gen={} -> {} :: opens={} comp_passes={} comp_grows={} heap={} dmg_walks={} dmg_skips={} session_gen={} ::",
        idle,
        per_pass,
        if gen_ok { "ok" } else { "FAIL" },
        if pass { "PASS" } else { "FAIL" },
        opens,
        passes,
        grows,
        heap,
        walks,
        skips,
        sgen
    );
}
