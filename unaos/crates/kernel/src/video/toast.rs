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

use super::{metrics, theme, wm};

/// How long a toast stays.
pub const TOAST_MS: u64 = 3000;
const W: usize = 340;
const H: usize = 52;
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
static HEADLESS: AtomicBool = AtomicBool::new(false);
static SHOWN: AtomicU32 = AtomicU32::new(0);
static DROPPED: AtomicU32 = AtomicU32::new(0);
/// A toast changed the focus owner (must stay 0: the row is never focusable).
static FOCUS_MOVED: AtomicU32 = AtomicU32::new(0);

static SURF_AT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
fn surf() -> &'static mut [u32] {
    let n = metrics::size(W) * metrics::size(H);
    let mut p = SURF_AT.load(Ordering::Acquire);
    if p == 0 {
        let b: &'static mut [u32] = alloc::boxed::Box::leak(alloc::vec![0u32; n].into_boxed_slice());
        p = match SURF_AT.compare_exchange(0, b.as_mut_ptr() as usize, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => b.as_mut_ptr() as usize,
            Err(won) => won,
        };
    }
    // SAFETY: one leaked buffer of `n` words, painted on the window-safe path, read by `wm`'s composite.
    unsafe { core::slice::from_raw_parts_mut(p as *mut u32, n) }
}

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
    TQ.lock().cur.is_some()
}

fn paint(t: &T) {
    let px = surf();
    let (pw, ph) = (metrics::size(W), metrics::size(H));
    metrics::fill(px, pw, 0, 0, W, H, theme::chrome_face());
    for (x, y, w, h) in [(0, 0, W, 1), (0, H - 1, W, 1), (0, 0, 1, H), (W - 1, 0, 1, H)] {
        metrics::fill(px, pw, x, y, w, h, theme::frame_line());
    }
    metrics::fill(px, pw, 12, 12, 28, 28, theme::accent());
    let _ = metrics::text(px, pw, ph, W, 50, 8, t.title(), theme::content_text(), true, crate::video::text::Face::Ui);
    let _ = metrics::text(px, pw, ph, W, 50, 28, &t.line[..t.ll as usize], theme::title_text_active(), false, crate::video::text::Face::Ui);
}

fn open(t: T) -> wm::WinId {
    if HEADLESS.load(Ordering::Relaxed) {
        return wm::WIN_NONE;
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let (pw, _ph) = { let i = super::WRITER.lock().info(); (i.width, i.height) };
        if pw == 0 {
            return wm::WIN_NONE;
        }
        paint(&t);
        let (sw, sh) = (metrics::size(W), metrics::size(H));
        let x = pw.saturating_sub(sw + metrics::size(12));
        let y = wm::TITLE_H() + metrics::size(8);
        return wm::overlay_open(surf().as_mut_ptr() as usize, sw * sh * 4, sw, sh, x, y);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        let _ = (t, paint as fn(&T));
        wm::WIN_NONE // aarch64: wire-only (no chromeless overlay row there yet)
    }
}

fn close(by: &str) {
    let (t, win) = {
        let mut g = TQ.lock();
        let Some(t) = g.cur.take() else { return };
        g.until = 0;
        (t, core::mem::replace(&mut g.win, wm::WIN_NONE))
    };
    if win != wm::WIN_NONE {
        wm::close(win);
    }
    serial_println!("[toast] closed by={} title={}", by, core::str::from_utf8(t.title()).unwrap_or("?"));
}

/// The service (the storage pass): close a toast whose time is up, then show the next.
pub fn service() {
    let (until, up, n) = match TQ.try_lock() { Some(g) => (g.until, g.cur.is_some(), g.n), None => return };
    if up && crate::arch::ms() >= until {
        close("timeout");
    } else if up || n == 0 {
        return;
    }
    let t = {
        let Some(mut g) = TQ.try_lock() else { return };
        if g.cur.is_some() || g.n == 0 {
            return;
        }
        let first = g.q[0];
        let n = g.n;
        for i in 1..n {
            g.q[i - 1] = g.q[i];
        }
        g.n -= 1;
        g.cur = Some(first);
        g.until = crate::arch::ms().saturating_add(TOAST_MS).max(1);
        first
    };
    let f0 = wm::focus_asid();
    let win = open(t);
    TQ.lock().win = win;
    SHOWN.fetch_add(1, Ordering::Relaxed);
    let kept = wm::focus_asid() == f0;
    if !kept {
        FOCUS_MOVED.fetch_add(1, Ordering::Relaxed);
    }
    serial_println!(
        "[toast] show title={} line={} ms={} focus={} win={}",
        core::str::from_utf8(t.title()).unwrap_or("?"), core::str::from_utf8(&t.line[..t.ll as usize]).unwrap_or("?"), TOAST_MS, if kept { "kept" } else { "MOVED" }, win
    );
}

/// `tests notice` (DIALOG M4): a toast posts, shows without moving focus, closes on its time (model-only).
pub fn fixture() -> bool {
    let was = HEADLESS.swap(true, Ordering::Relaxed);
    let saved = { let mut g = TQ.lock(); let s = (g.q, g.n, g.cur.take(), core::mem::replace(&mut g.win, wm::WIN_NONE), g.until); g.n = 0; s };
    let moved0 = FOCUS_MOVED.load(Ordering::Relaxed);
    let posted = post(b"holocron", b"holocron start: ok\nsecond line dropped");
    service();
    let up = showing() && TQ.lock().cur.map(|t| t.ll as usize == b"holocron start: ok".len()).unwrap_or(false);
    TQ.lock().until = 1;
    service();
    let gone = !showing();
    let kept = FOCUS_MOVED.load(Ordering::Relaxed) == moved0;
    {
        let mut g = TQ.lock();
        g.q = saved.0;
        g.n = saved.1;
        g.cur = saved.2;
        g.win = saved.3;
        g.until = saved.4;
    }
    HEADLESS.store(was, Ordering::Relaxed);
    posted && up && gone && kept
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
