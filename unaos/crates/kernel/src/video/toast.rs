//! CHARTER: Kernel — wm
//!
//! TOAST (rmbp-ledger B395, DIALOG M3; the seed of NOTIFY, MACPARITY row 24) — the one-line transient. A
//! program's start banner (the holocron client's `BUS_VERB_NOTICE` line) and a fault in a program nobody
//! launched from the glass are NOT dialogs: they show here, at the top right under the bar, for
//! [`TOAST_MS`], and go. The row is a chromeless compat row (`wm::overlay_open`, owner 0): `wm::hit_test`
//! never names it and nothing focuses it, so it cannot take a key or a press from anything (R88).
//!
//! Posting is QUEUE-ONLY ([`post`]: `try_lock`, no heap, no `wm`), safe from the bus verb and a fault handler;
//! the row opens and closes in [`service`] (the storage pass, beside the dialog's). Witness:
//! `[toast] show title=<t> ms=3000 focus=kept win=<n>` and `[toast] closed by=timeout title=<t>`.
//!
//! Also here: the GLASS-LAUNCH provenance `Program stopped` needs — the dock/Quarry verb drain arms
//! [`note_glass`], the x86 background spawn consumes it into a per-slot bit ([`note_spawn`]), and the fault's
//! notice reads it for the faulting slot ([`current_is_glass`]).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::wm;

/// How long a toast stays.
pub const TOAST_MS: u64 = 3000;
const TL: usize = 24;
const LL: usize = 52;
const QCAP: usize = 4;

#[derive(Clone, Copy)]
struct T {
    title: [u8; TL],
    tl: u8,
    line: [u8; LL],
    ll: u8,
}

impl T {
    const EMPTY: T = T { title: [0; TL], tl: 0, line: [0; LL], ll: 0 };
    fn make(title: &[u8], body: &[u8]) -> T {
        let mut t = T::EMPTY;
        for &b in title.iter().take(TL) {
            t.title[t.tl as usize] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
            t.tl += 1;
        }
        // one line: the body's first line (a banner is one line, never a dialog's paragraph)
        for &b in body.iter().take_while(|&&b| b != b'\n').take(LL) {
            t.line[t.ll as usize] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
            t.ll += 1;
        }
        t
    }
    fn title(&self) -> &[u8] { &self.title[..self.tl as usize] }
}

struct Q {
    q: [T; QCAP],
    n: usize,
    cur: Option<T>,
    win: wm::WinId,
    until: u64,
}

static TQ: spin::Mutex<Q> = spin::Mutex::new(Q { q: [T::EMPTY; QCAP], n: 0, cur: None, win: wm::WIN_NONE, until: 0 });
#[allow(dead_code)] static HEADLESS: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicU32 = AtomicU32::new(0);

/// Queue a toast (QUEUE ONLY). `false` when the queue is contended or full (counted).
pub fn post(title: &[u8], body: &[u8]) -> bool {
    let t = T::make(title, body);
    let Some(mut g) = TQ.try_lock() else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    if g.n >= QCAP {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let i = g.n;
    g.q[i] = t;
    g.n += 1;
    true
}

/// A toast is showing.
pub fn showing() -> bool {
    super::notify::banners() > 0 // NOTIFY (B418): the toast's card is NOTIFY's stack
}

/// NOTIFY (B418): the service is NOTIFY's — it takes this queue ([`take`]) into the stack and the Center.
pub fn service() {
    super::notify::service();
}

/// `tests notice` (DIALOG M4): a toast posts, shows without moving focus, closes on its time (model-only).
pub fn fixture() -> bool {
    super::notify::toast_fixture() // NOTIFY (B418): the same leg, over the stack
}

/// NOTIFY (B418): the oldest queued toast as `(title, line)`, for NOTIFY's service (the window-safe pass).
pub fn take() -> Option<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
    let mut g = TQ.try_lock()?;
    if g.n == 0 {
        return None;
    }
    let first = g.q[0];
    let n = g.n;
    for i in 1..n {
        g.q[i - 1] = g.q[i];
    }
    g.n -= 1;
    Some((first.title().to_vec(), first.line[..first.ll as usize].to_vec()))
}

// ── glass-launch provenance (`Program stopped` is a dialog only for a program launched from the glass) ──

/// `arch::ms()` of the last glass launch handed to the shell (the dock's / Quarry's verb drain); `0` none.
static GLASS_ARMED: AtomicU64 = AtomicU64::new(0);
/// Per user slot: launched from the glass (bit set) or typed / autostarted (clear).
static GLASS_SLOTS: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

/// The dock / Quarry handed a launch line to the shell: the next spawn within 3 s is the glass's.
pub fn note_glass(_verb: &str) {
    GLASS_ARMED.store(crate::arch::ms().max(1), Ordering::Release);
}

/// A background program took `slot`: it is the glass's when a glass launch is armed (consumed), else not.
pub fn note_spawn(slot: usize) {
    let a = GLASS_ARMED.swap(0, Ordering::AcqRel);
    let glass = a != 0 && crate::arch::ms().saturating_sub(a) < 3000;
    if slot < 256 {
        let (w, b) = (slot / 64, 1u64 << (slot % 64));
        if glass { GLASS_SLOTS[w].fetch_or(b, Ordering::AcqRel); } else { GLASS_SLOTS[w].fetch_and(!b, Ordering::AcqRel); }
    }
}

/// The faulting (current) task's slot was launched from the glass. Atomics only (the fault path).
pub fn current_is_glass() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if let Some(s) = crate::arch::memory::current_slot() {
            return s < 256 && GLASS_SLOTS[s / 64].load(Ordering::Acquire) & (1u64 << (s % 64)) != 0;
        }
    }
    false
}
