//! CHARTER: Kernel — wm (PERFHUD B710: the compositor's own numbers on the glass; reads the instruments that exist, owns only the HUD's pixels and its toggle)
//!
//! PERFHUD (rmbp-ledger B710, BP-bar-5 / BP-bar-24: "the numbers exist on the wire — the user, and Peter on the glass,
//! should see them without a serial cable"). A small panel in the top-right of the desktop, painted by the compositor
//! as its TOPMOST layer (after every window and furniture strip, before the cursor tail — `wm::composite_pass_half`),
//! showing nine fields:
//!
//! 1. frame time p50 / p99 over the last 120 passes (`ft`) — the pass from `wm::composite_once`'s start
//!    ([`pass_begin`]) to the overlay layer ([`compose`]): every window and strip, NOT the HUD's own paint and not the
//!    cursor tail; the HUD's own refresh passes are not counted;
//! 2. missed vblanks in the last minute (`wm::k2p`'s `[wc] missed_vblank`);
//! 3. k2p p99 (`wm::k2p`'s last `[input] k2p` p99);
//! 4. heap used / limit (MEMLINE's `allocator::heap_used`, the heap's span);
//! 5. windows (live / limit, `wincap`); 6. processes (running / limit);
//! 7. the render core's load and 8. the service core's (`sched::core_load` on the published split; `*` = inferred);
//! 9. the boot's OVER steps and when the last one was seen (`fs::bootstep::tally`).
//!
//! Toggle: Win+F12 (the fn-row router's one entry, [`chord`]; the Windows key is `keymap::start_role` under Softy and
//! the GUI key — Command on a Mac — under Una) or Settings > Accessibility > Show performance (Principia key
//! `system.accessibility.show_performance`; the chord's toggle is written back by the Settings service pass).
//!
//! # Cost model (the compositor is damage-driven and writes straight to the scan-out)
//! * A SAMPLER task on a worker core (`smp::worker_cpu(0)`, BP-cores-1) wakes every 500 ms while the HUD is visible,
//!   reads the numbers (IRQs on, so the heap and table locks are taken where a wait is legal), and only when the
//!   text CHANGED renders the panel into a cached-RAM image and asks for one composite. It never asks for a full
//!   frame: a pass with nothing else damaged repaints only the HUD's rect.
//! * [`compose`] (in the pass, possibly masked) takes no blocking lock and allocates nothing: it copies the cached
//!   rows through `strip::paint` (the row-run staged painter every furniture strip uses; it takes the sprite down and
//!   the pass's tail puts it back). It repaints on every pass while visible, because any pass may have drawn a window
//!   or the shimmer's erase over the rect (the HUD is not an erase-clip tenant). Measured: `paint_us` (budget 300).
//! * Hidden: the rect is VACATED once (`strip::vacate`: erased and handed back to the scene and the windows under it,
//!   the furniture's own restore), then [`compose`] costs one relaxed load.
//!
//! Wire: `[hud] visible=<0|1> via=<chord|settings|login|test>` on a change; `[hud] shown rect=<w>x<h>+<x>+<y>
//! paint_us=<n> budget_us=300 -> UNDER|OVER` on the first paint after a show; every 10 s while visible
//! `[hud] ft_p50=<us> ft_p99=<us> missed=<n> k2p_p99=<us> heap=<kb>/<kb> win=<n> proc=<n>`.
//! `tests perfhud`: `:: PERFHUD: visible=1 paint_us=<n> fields=9 -> PASS :: pixels=3/3 budget_us=300 under=<0|1> rect=…`.

use core::sync::atomic::{AtomicBool, Ordering::{AcqRel, Acquire, Release}};

/// The HUD is on (the toggle; both desktop families latch it, the x86 `wc` compositor paints it).
static VISIBLE: AtomicBool = AtomicBool::new(false);
/// A store write of the toggle is owed (the chord or the Settings row moved it); the Settings service pass drains it.
static OWED: AtomicBool = AtomicBool::new(false);

/// The fn-row usage of F12.
pub const USAGE_F12: u8 = 0x45;

/// Is the HUD on?
#[inline]
pub fn visible() -> bool {
    VISIBLE.load(Acquire)
}

/// Set the toggle (`via` names the wire's mover). `persist` latches a store write for the Settings service pass.
/// Returns whether it moved. Never paints here: the sampler (x86 `wc`) drives the pass that shows or vacates it.
pub fn apply(on: bool, via: &str, persist: bool) -> bool {
    let moved = VISIBLE.swap(on, AcqRel) != on;
    if moved {
        serial_println!("[hud] visible={} via={}", on as u8, via);
        if persist {
            OWED.store(true, Release);
        }
    }
    if on {
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        glass::arm();
    }
    moved
}

/// The chord or the Settings row: set and persist.
pub fn set_visible(on: bool, via: &str) -> bool {
    apply(on, via, true)
}

/// The login's load (or another client's write): set without writing back.
pub fn latch(on: bool, via: &str) -> bool {
    apply(on, via, false)
}

/// The Settings service pass: is a store write of [`visible`] owed?
pub fn take_owed() -> bool {
    OWED.swap(false, AcqRel)
}

/// THE CHORD (the fn-row router's one entry, `video::fnrow::fn_row_usage`): `usage` went down under `modifiers`.
/// Win+F12 toggles the HUD and is consumed (`true`: the F12 volume step does not also run). The Windows key is the
/// live table's Start key under Softy (`keymap::start_role`) and the GUI key under Una (Command on a Mac, Win on a PC).
pub fn chord(usage: u8, modifiers: u8) -> bool {
    if usage != USAGE_F12 {
        return false;
    }
    let role = super::keymap::start_role();
    let win = if role == 0 { crate::drivers::xhci::HID_MOD_GUI } else { role };
    if modifiers & win == 0 {
        return false;
    }
    set_visible(!visible(), "chord");
    true
}

/// `wm::composite_once`, before the pass: stamp its start (x86 `wc`).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
#[inline]
pub fn pass_begin() {
    glass::pass_begin();
}

/// `wm::composite_pass_half`, at the overlay layer: paint (or vacate) the HUD. `true` = the pass owes the sprite.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
#[inline]
pub fn compose() -> bool {
    glass::compose()
}

/// `tests perfhud` (x86 `wc`).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn selftest() {
    glass::selftest();
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod glass {
    use super::super::{strip, text, theme, wm};
    use super::VISIBLE;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};

    /// The HUD's name on the strip painter's census and vacate lines.
    const NAME: &str = "perfhud";
    /// Frame-time ring: the last 120 passes.
    const FT_N: usize = 120;
    /// Sampler cadence and echo cadence.
    const TICK_MS: u64 = 500;
    const ECHO_MS: u64 = 10_000;
    /// The paint budget (BP-bar-5's HUD: <= 0.3 ms).
    pub const PAINT_BUDGET_US: u64 = 300;
    /// The fields the panel shows, one per line.
    pub const FIELDS: usize = 9;
    /// Characters per line (the longest field is 18).
    const COLS: usize = 18;
    /// Budgets a field is inked `ink.warning` past: a frame, and one vblank plus scanout for k2p.
    const FRAME_US: u32 = 16_667;
    const K2P_US: u32 = 33_334;

    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicU32 = AtomicU32::new(0);
    static FT: [AtomicU32; FT_N] = [Z; FT_N];
    static FT_N_SEEN: AtomicUsize = AtomicUsize::new(0);
    static PASS_T0: AtomicU64 = AtomicU64::new(0);
    /// The next pass is the sampler's own refresh: not a frame-time sample.
    static SELF_PASS: AtomicBool = AtomicBool::new(false);
    static SLOT: strip::Slot = strip::Slot::new();
    static PAINTS: AtomicU64 = AtomicU64::new(0);
    static LAST_PAINT_US: AtomicU64 = AtomicU64::new(0);
    static MAX_PAINT_US: AtomicU64 = AtomicU64::new(0);
    static SAMPLER: AtomicBool = AtomicBool::new(false);
    /// The paint count at the last show; the first paint past it is reported once ([`SHOWN_DONE`] = reported).
    static SHOWN_AT: AtomicU64 = AtomicU64::new(SHOWN_DONE);
    const SHOWN_DONE: u64 = u64::MAX;
    /// OVER steps: the tally first seen (u32::MAX = none yet) and when a new one was seen (0 = at boot).
    static OVER_SEEN: AtomicU32 = AtomicU32::new(u32::MAX);
    static OVER_AT_MS: AtomicU64 = AtomicU64::new(0);

    /// The rendered panel: logical `0x00RRGGBB` rows, `w` x `h`, and the signature it was rendered from.
    struct Cache {
        px: Vec<u32>,
        w: usize,
        h: usize,
        sig: u64,
    }

    static CACHE: crate::sync::Mutex<Cache> = crate::sync::Mutex::new(Cache { px: Vec::new(), w: 0, h: 0, sig: 0 });

    fn us(dt: u64) -> u64 {
        strip::cycles_to_us(dt)
    }

    pub fn pass_begin() {
        if VISIBLE.load(Relaxed) {
            PASS_T0.store(crate::arch::now_cycles(), Relaxed);
        }
    }

    /// The desktop is built and not held bare (BP-video-9: before a session the furniture owns no pixels).
    fn desktop_up() -> bool {
        #[cfg(feature = "login")]
        if crate::boot::shot_bare_held() || !super::super::desktopbuild::built() || super::super::lock::veiled() { // crawl A 13: no overlay over the session lock (BP-video-28)
            return false;
        }
        true
    }

    /// The HUD's rect on a `pw` x `ph` panel for a `w` x `h` image: top-right, under the menu bar when there is one.
    fn rect(pw: usize, ph: usize, w: usize, h: usize) -> Option<strip::Rect> {
        let pad = strip::PAD();
        let y = super::super::menubar::strip_rect(pw, ph).map_or(pad, |(_, my, _, mh)| my + mh + pad);
        if w == 0 || h == 0 || w + 2 * pad > pw || y + h + pad > ph {
            return None;
        }
        Some((pw - w - pad, y, w, h))
    }

    /// Vacate what the HUD last painted (hidden, or the desktop went away). `true` = the pass owes the sprite.
    fn vacate_painted() -> bool {
        let old = SLOT.packed();
        if old == 0 {
            return false;
        }
        let r = strip::vacate(NAME, strip::unpack_rect(old), None, true);
        if r {
            SLOT.clear();
        }
        r
    }

    pub fn compose() -> bool {
        let t0 = PASS_T0.swap(0, Relaxed);
        if !VISIBLE.load(Acquire) || !desktop_up() {
            return vacate_painted();
        }
        if t0 != 0 && !SELF_PASS.swap(false, Relaxed) {
            let d = us(crate::arch::now_cycles().saturating_sub(t0)).min(u32::MAX as u64) as u32;
            let i = FT_N_SEEN.fetch_add(1, Relaxed);
            FT[i % FT_N].store(d, Relaxed);
        }
        let Some((pw, ph)) = super::super::WRITER.try_lock().map(|f| *f).filter(|f| f.is_ready()).map(|f| (f.width(), f.height())) else { return false }; // crawl A 16: no blocking WRITER hold inside the pass (BP-video-27)
        // Never wait in the pass: a cache the sampler is swapping is a skipped paint (the next pass repaints).
        let Some(c) = CACHE.try_lock() else { return false };
        let Some(r) = rect(pw, ph, c.w, c.h) else {
            drop(c);
            return vacate_painted();
        };
        let mut owed = false;
        let old = SLOT.packed();
        if old != 0 && old != strip::pack_rect(Some(r)) {
            owed |= strip::vacate(NAME, strip::unpack_rect(old), Some(r), true);
        }
        let w = c.w;
        let tp = crate::arch::now_cycles();
        let ok = strip::paint(NAME, r, |out, j| {
            let row = &c.px[j * w..(j + 1) * w];
            out[..w].copy_from_slice(row);
        });
        if ok {
            let p = us(crate::arch::now_cycles().saturating_sub(tp));
            LAST_PAINT_US.store(p, Relaxed);
            MAX_PAINT_US.fetch_max(p, Relaxed);
            PAINTS.fetch_add(1, Release);
            SLOT.store(c.sig | 1, Some(r));
        }
        owed | ok
    }

    /// One reading of the nine fields.
    #[derive(Clone, Copy, Default)]
    struct Snap {
        ft_p50: u32,
        ft_p99: u32,
        ft_n: usize,
        missed: u32,
        k2p_p99: u32,
        heap_kb: usize,
        heap_lim_kb: usize,
        win: usize,
        win_lim: usize,
        proc: usize,
        proc_lim: usize,
        rcpu: Option<(usize, u32, bool, bool)>,
        scpu: Option<(usize, u32, bool, bool)>,
        over: u32,
        over_age_s: Option<u64>,
    }

    fn core(cpu: Option<usize>) -> Option<(usize, u32, bool, bool)> {
        let c = cpu?;
        let l = crate::arch::sched::core_load(c);
        Some((c, l.busy_pct_recent, l.tracked, l.pegged))
    }

    /// Read the instruments. A task context with IRQs on: the heap and table locks are taken here, never in the pass.
    fn sample() -> Snap {
        let mut v = [0u32; FT_N];
        let seen = FT_N_SEEN.load(Relaxed);
        let n = seen.min(FT_N);
        for (i, x) in v.iter_mut().take(n).enumerate() {
            *x = FT[i].load(Relaxed);
        }
        let (ft_p50, ft_p99) = wm::k2p::p50_p99(&mut v[..n]);
        let (k2p_p99, missed, _) = wm::k2p::hud();
        let (lo, hi) = crate::allocator::heap_bounds();
        let (_, running, _, _) = crate::arch::x86_64::syscall::proc_table_headroom();
        let (over_n, over_age_s) = over();
        Snap {
            ft_p50,
            ft_p99,
            ft_n: n,
            missed,
            k2p_p99,
            heap_kb: crate::allocator::heap_used() >> 10,
            heap_lim_kb: hi.saturating_sub(lo) >> 10,
            win: wm::count(),
            win_lim: super::super::wincap::win_limit(),
            proc: running,
            proc_lim: super::super::wincap::proc_limit(),
            rcpu: core(crate::arch::smp::render_cpu()),
            scpu: core(crate::arch::smp::service_cpu()),
            over: over_n,
            over_age_s,
        }
    }

    /// The boot's OVER steps so far and the age of the newest one this HUD saw appear (`None` = at boot or none).
    fn over() -> (u32, Option<u64>) {
        let n = crate::fs::bootstep::tally().3;
        let now = crate::arch::ms();
        let seen = OVER_SEEN.load(Relaxed);
        if seen == u32::MAX {
            OVER_SEEN.store(n, Relaxed);
        } else if n > seen {
            OVER_SEEN.store(n, Relaxed);
            OVER_AT_MS.store(now.max(1), Relaxed);
        }
        let at = OVER_AT_MS.load(Relaxed);
        (n, if at == 0 { None } else { Some(now.saturating_sub(at) / 1000) })
    }

    fn ms1(us: u32) -> (u32, u32) {
        (us / 1000, (us % 1000) / 100)
    }

    fn pct(c: Option<(usize, u32, bool, bool)>) -> String {
        match c {
            Some((cpu, p, true, pegged)) => alloc::format!("c{} {}%{}", cpu, p, if pegged { "*" } else { "" }),
            Some((cpu, _, false, _)) => alloc::format!("c{} --", cpu),
            None => String::from("--"),
        }
    }

    /// The nine lines and whether each is past its budget (inked `ink.warning`).
    fn lines(s: &Snap) -> [(String, bool); FIELDS] {
        let (a, b) = ms1(s.ft_p50);
        let (c, d) = ms1(s.ft_p99);
        let (e, f) = ms1(s.k2p_p99);
        [
            (if s.ft_n == 0 { String::from("ft --") } else { alloc::format!("ft {}.{}/{}.{} ms", a, b, c, d) }, s.ft_p99 > FRAME_US),
            (alloc::format!("miss {}/min", s.missed), s.missed > 0),
            (if s.k2p_p99 == 0 { String::from("k2p --") } else { alloc::format!("k2p {}.{} ms p99", e, f) }, s.k2p_p99 > K2P_US),
            (alloc::format!("heap {}/{} MB", s.heap_kb >> 10, s.heap_lim_kb >> 10), false),
            (alloc::format!("win  {}/{}", s.win, s.win_lim), false),
            (alloc::format!("proc {}/{}", s.proc, s.proc_lim), false),
            (alloc::format!("rndr {}", pct(s.rcpu)), false),
            (alloc::format!("svc  {}", pct(s.scpu)), false),
            (
                match (s.over, s.over_age_s) {
                    (0, _) => String::from("OVER none"),
                    (n, None) => alloc::format!("OVER {} at boot", n),
                    (n, Some(t)) => alloc::format!("OVER {} {}s ago", n, t),
                },
                s.over_age_s.is_some_and(|t| t < 60),
            ),
        ]
    }

    fn fnv(h: u64, v: u64) -> u64 {
        strip::fnv1a_u64(h, v)
    }

    /// Render the panel for `s` into a fresh image: `surface.overlay` (elev.2), a `line.subtle` hairline, the mono
    /// grid face, `text.primary` (`ink.warning` past budget). `None` = no memory for it (the old image stays).
    fn render(s: &Snap) -> Option<(Vec<u32>, usize, usize, usize)> {
        let face = text::Face::Grid;
        let (cw, ch) = (face.cell_w(), face.cell_h());
        let pad = strip::PAD() / 2 + 1;
        let (w, h) = (COLS * cw + 2 * pad, FIELDS * ch + 2 * pad);
        let ls = lines(s);
        let (bg, line, ink, warn) = (theme::surface_overlay(), theme::line_subtle(), theme::text_primary(), theme::ink_warning());
        let mut px: Vec<u32> = Vec::new();
        px.try_reserve_exact(w * h).ok()?;
        px.resize(w * h, bg);
        for j in 0..h {
            let row = &mut px[j * w..(j + 1) * w];
            if j == 0 || j == h - 1 {
                row.fill(line);
                continue;
            }
            row[0] = line;
            row[w - 1] = line;
            if j >= pad {
                let (k, sy) = ((j - pad) / ch, (j - pad) % ch);
                if k < FIELDS {
                    let (t, hot) = &ls[k];
                    text::draw_row(row, w - 1, t.as_bytes(), pad, sy, if *hot { warn } else { ink }, false, face);
                }
            }
        }
        let fields = ls.iter().filter(|(t, _)| !t.is_empty()).count();
        Some((px, w, h, fields))
    }

    /// Sample, and re-render when the text (or the theme, the face, the cell) changed. `true` = a new image is in.
    fn refresh() -> (bool, usize) {
        let s = sample();
        let mut sig = 0xcbf2_9ce4_8422_2325u64;
        for (t, hot) in lines(&s).iter() {
            for b in t.bytes() {
                sig = strip::fnv1a(sig, b);
            }
            sig = fnv(sig, *hot as u64);
        }
        let (cw, ch) = (text::Face::Grid.cell_w(), text::Face::Grid.cell_h());
        sig = fnv(fnv(fnv(fnv(sig, theme::epoch() as u64), text::epoch() as u64), cw as u64), ch as u64);
        sig &= !1;
        let same = { let c = CACHE.lock(); c.sig == sig && c.w != 0 };
        if same {
            return (false, FIELDS);
        }
        let Some((px, w, h, fields)) = render(&s) else { return (false, 0) };
        let old = {
            let mut c = CACHE.lock();
            let old = core::mem::replace(&mut c.px, px);
            c.w = w;
            c.h = h;
            c.sig = sig;
            old
        };
        drop(old);
        (true, fields)
    }

    /// One composite for the HUD (the gate may decline it; the holder re-runs, the next tick retries).
    fn drive() {
        SELF_PASS.store(true, Relaxed);
        wm::composite();
    }

    /// The M2 echo.
    fn echo() {
        let s = sample();
        serial_println!(
            "[hud] ft_p50={} ft_p99={} missed={} k2p_p99={} heap={}/{} win={} proc={}",
            s.ft_p50, s.ft_p99, s.missed, s.k2p_p99, s.heap_kb, s.heap_lim_kb, s.win, s.proc
        );
    }

    /// The first paint after a show: its rect and cost, once.
    fn shown_line() {
        let at = SHOWN_AT.load(Relaxed);
        if at == SHOWN_DONE || PAINTS.load(Acquire) <= at {
            return;
        }
        let (x, y, w, h) = SLOT.rect();
        let us = LAST_PAINT_US.load(Relaxed);
        serial_println!(
            "[hud] shown rect={}x{}+{}+{} paint_us={} budget_us={} -> {}",
            w, h, x, y, us, PAINT_BUDGET_US, if us <= PAINT_BUDGET_US { "UNDER" } else { "OVER" }
        );
        SHOWN_AT.store(SHOWN_DONE, Relaxed);
    }

    /// Spawn the sampler once per visible span, on a worker core (not the render core, BP-cores-1).
    pub fn arm() {
        if SAMPLER.swap(true, AcqRel) {
            return;
        }
        let Some(cpu) = crate::arch::smp::worker_cpu(0) else {
            SAMPLER.store(false, Release);
            serial_println!("[hud] sampler declined reason=no-worker-core (the toggle holds; tests perfhud renders inline)");
            return;
        };
        SHOWN_AT.store(PAINTS.load(Relaxed), Relaxed);
        crate::arch::sched::spawn("perfhud", sampler, 0, cpu, crate::arch::sched::PRIO_LOW);
    }

    fn sampler(_: usize) {
        let mut next_echo = crate::arch::ms() + ECHO_MS;
        loop {
            if !VISIBLE.load(Acquire) {
                // The vacate pass, then exit; a show that raced this exit re-arms through the swap below.
                drive();
                SAMPLER.store(false, Release);
                if !VISIBLE.load(Acquire) || SAMPLER.swap(true, AcqRel) {
                    return;
                }
            }
            if refresh().0 {
                drive();
            }
            shown_line();
            let now = crate::arch::ms();
            if now >= next_echo {
                echo();
                next_echo = now + ECHO_MS;
            }
            crate::arch::sched::sleep_ms(TICK_MS);
        }
    }

    /// `tests perfhud`: toggle on, render inline, drive composites until the HUD paints (bounded 2 s), read three
    /// pixels of its margin back from the panel (each must be `surface.overlay`), time the paint, restore the toggle.
    pub fn selftest() {
        if super::super::panel_snapshot().filter(|f| f.is_ready()).is_none() {
            serial_println!(":: PERFHUD: -> SKIP :: reason=no-panel");
            return;
        }
        if !desktop_up() {
            serial_println!(":: PERFHUD: -> SKIP :: reason=no-desktop (the furniture is built at login)");
            return;
        }
        let was = super::visible();
        let _ = super::latch(true, "test");
        let vis = super::visible() as u8;
        let p0 = PAINTS.load(Acquire);
        // Force a fresh image (the sampler may not have run yet).
        CACHE.lock().sig = 0;
        let (_, fields) = refresh();
        let t_end = crate::arch::ms() + 2_000;
        while PAINTS.load(Acquire) == p0 && crate::arch::ms() < t_end {
            drive();
            let w = crate::arch::ms() + 17;
            while PAINTS.load(Acquire) == p0 && crate::arch::ms() < w {
                core::hint::spin_loop();
            }
        }
        let painted = PAINTS.load(Acquire) > p0;
        let (x, y, w, h) = SLOT.rect();
        let want = theme::surface_overlay();
        let mut hits = 0;
        if painted && w > 6 && h > 6 {
            if let Some(fb) = super::super::panel_snapshot() {
                for (px, py) in [(x + 2, y + 2), (x + w - 3, y + 2), (x + 2, y + h - 3)] {
                    if fb.read_pixel(px, py) == Some(want) {
                        hits += 1;
                    }
                }
            }
        }
        let paint_us = LAST_PAINT_US.load(Relaxed);
        if !was {
            let _ = super::latch(false, "test");
            drive();
        }
        let ok = painted && hits == 3 && fields == FIELDS && vis == 1;
        serial_println!(
            ":: PERFHUD: visible={} paint_us={} fields={} -> {} :: pixels={}/3 budget_us={} under={} rect={}x{}+{}+{} max_paint_us={}",
            vis, paint_us, fields, if ok { "PASS" } else { "FAIL" }, hits, PAINT_BUDGET_US, (paint_us <= PAINT_BUDGET_US) as u8, w, h, x, y,
            MAX_PAINT_US.load(Relaxed)
        );
    }
}
