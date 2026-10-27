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
//!   the pass's tail puts it back). PERFHUD2 (B828): the HUD is the THIRD strip tenant (`strip::OVERLAY_SLOT`,
//!   [`strip_rect`] = the rect it last painted), so the window blits (`wm::occ_clip`), the erase drain and the shimmer
//!   (`wm::erase_clip`) and the desktop present withhold its rect; the strip writers that do not consult the registry
//!   latch a full repaint (`strip::overlay_take_hit`). So a pass repaints only what changed: the sampler diffs each new
//!   image against the cached one into nine field bands, and the pass paints the dirty bands; it paints the WHOLE
//!   image only on a first paint, a move, a latched hit or a colour-filter change, and nothing at all otherwise.
//!   Measured: `paint_us` = the steady (band) paint, budget 300; the full paint is reported as `full_us=`/`full_px=`.
//! * Hidden: the rect is VACATED once (`strip::vacate`: erased and handed back to the scene and the windows under it,
//!   the furniture's own restore), then [`compose`] costs one relaxed load.
//!
//! Wire: `[hud] visible=<0|1> via=<chord|settings|login|test>` on a change; `[hud] shown rect=<w>x<h>+<x>+<y>
//! paint_us=<n> budget_us=300 fields_dirty=9 px=<n> -> UNDER|OVER` (the full paint after a show); every 10 s while
//! visible `[hud] ft_p50=<us> ft_p99=<us> missed=<n> k2p_p99=<us> heap=<kb>/<kb> win=<n> proc=<n>`.
//! `tests perfhud`: `:: PERFHUD: visible=1 paint_us=<steady> fields=9 -> PASS :: pixels=3/3 budget_us=300 under=1
//! fields_dirty=<n> px=<n> full_us=<n> full_px=<n> scale=<s> rect=… max_paint_us=<n>`.

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

/// PERFHUD2 (B828): the HUD as a strip tenant (`strip::TENANTS[strip::OVERLAY_SLOT]`): the rect it last painted, read
/// lock-free (two atomics), `None` while nothing is painted — and on a desktop family whose compositor does not paint
/// it. The panel size is not needed: the rect is the one on the glass.
pub fn strip_rect(_pw: usize, _ph: usize) -> Option<super::strip::Rect> {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    return glass::painted();
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    None
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
    use super::super::blitter::a11yfilter as filt;
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
    /// PERFHUD2: the last paint's pixels and field bands (9 = a full paint).
    static LAST_PX: AtomicU64 = AtomicU64::new(0);
    static LAST_FIELDS: AtomicUsize = AtomicUsize::new(0);
    /// PERFHUD2: the last FULL paint — its cost, pixels and rect, stored at that paint (the shown line reads these, never
    /// `SLOT`, which a hide clears).
    static FULL_US: AtomicU64 = AtomicU64::new(0);
    static FULL_PX: AtomicU64 = AtomicU64::new(0);
    static SHOWN_RECT: AtomicU64 = AtomicU64::new(0);
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
        /// PERFHUD2: per field, the image-local band whose pixels changed since the panel's copy was painted (`None` =
        /// clean). Unioned by the sampler, cleared by the pass that paints it.
        dirty: [Option<strip::Rect>; FIELDS],
        /// The whole image is owed (a first paint, a new size, a change outside the field bands, a hit, a move).
        full: bool,
        /// The colour-filter word the panel's copy was last painted under ([`filter_word`]).
        fw: u32,
    }

    static CACHE: crate::sync::Mutex<Cache> =
        crate::sync::Mutex::new(Cache { px: Vec::new(), w: 0, h: 0, sig: 0, dirty: [None; FIELDS], full: true, fw: 0 });

    fn us(dt: u64) -> u64 {
        strip::cycles_to_us(dt)
    }

    pub fn pass_begin() {
        if VISIBLE.load(Relaxed) {
            PASS_T0.store(crate::arch::now_cycles(), Relaxed);
        }
    }

    /// The rect on the glass ([`super::strip_rect`]): `SLOT`'s, lock-free.
    pub fn painted() -> Option<strip::Rect> {
        let p = SLOT.packed();
        if p == 0 { None } else { Some(strip::unpack_rect(p)) }
    }

    /// The FIXED body cell (HUDSCALE B860: `text::Face::Body`, `font::CELL_W` x `font::CELL_H` device px at every scale) and the panel's inner pad: `(cw, ch, pad)` — what [`render`] lays out with and the bands
    /// are cut by.
    fn geom() -> (usize, usize, usize) {
        let face = text::Face::Body; // HUDSCALE (B860): the unscaled body cell, not the dpi-scaled Grid — a diagnostic overlay, its text stays small at 2.0 (BP-video-27)
        (face.cell_w(), face.cell_h(), strip::PAD() / 2 + 1)
    }

    /// The colour filter the scan-out copy applies (BP-video-25): a change repaints the whole image.
    fn filter_word() -> u32 {
        ((filt::mode() as u32) << 1) | filt::invert() as u32
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
        // Never wait in the pass: a cache the sampler is swapping is a skipped paint (the bands stay owed).
        let Some(mut g) = CACHE.try_lock() else { return false };
        let c = &mut *g;
        let Some(r) = rect(pw, ph, c.w, c.h) else {
            drop(g);
            return vacate_painted();
        };
        let mut owed = false;
        let old = SLOT.packed();
        let moved = old != strip::pack_rect(Some(r));
        if old != 0 && moved {
            owed |= strip::vacate(NAME, strip::unpack_rect(old), Some(r), true);
        }
        // PERFHUD2: the whole image on a first paint, a move, a write another strip writer latched (taken AFTER the move's
        // own vacate, so the HUD's own erase is consumed here), or a filter change; otherwise only the dirty bands.
        let fw = filter_word();
        if strip::overlay_take_hit() || old == 0 || moved || c.fw != fw {
            c.full = true;
        }
        let (w, h) = (c.w, c.h);
        let tp = crate::arch::now_cycles();
        let (mut px, mut nf, mut full_paint) = (0usize, 0usize, false);
        if c.full {
            let img = &c.px;
            if strip::paint(NAME, r, |out, j| out[..w].copy_from_slice(&img[j * w..(j + 1) * w])) {
                c.full = false;
                c.dirty = [None; FIELDS];
                c.fw = fw;
                (px, nf, full_paint) = (w * h, FIELDS, true);
            }
        } else { let (sy0, sy1) = band_rows(&c.dirty); let sh = strip::span_hold(r.1 + sy0, r.1 + sy1, ph); // BEAMSPLIT (B860): ONE beam hold over the dirty bands' rows (`band_rows`, module tail; `strip::span_hold`). ⚠ LINE-NEUTRAL fold, code first.
            for k in 0..FIELDS {
                let Some((bx, by, bw, bh)) = c.dirty[k] else { continue };
                let img = &c.px;
                let ok = strip::paint(NAME, (r.0 + bx, r.1 + by, bw, bh), |out, j| {
                    let o = (by + j) * w + bx;
                    out[..bw].copy_from_slice(&img[o..o + bw]);
                });
                if ok {
                    c.dirty[k] = None;
                    px += bw * bh;
                    nf += 1;
                }
            } strip::span_settle(sh); // BEAMSPLIT (B860): the span bracket closed after the last band. ⚠ LINE-NEUTRAL fold, code first.
        }
        if nf == 0 {
            return owed;
        }
        let p = us(crate::arch::now_cycles().saturating_sub(tp));
        LAST_PAINT_US.store(p, Relaxed);
        MAX_PAINT_US.fetch_max(p, Relaxed);
        LAST_PX.store(px as u64, Relaxed);
        LAST_FIELDS.store(nf, Relaxed);
        if full_paint {
            FULL_US.store(p, Relaxed);
            FULL_PX.store(px as u64, Relaxed);
            SHOWN_RECT.store(strip::pack_rect(Some(r)), Relaxed);
        }
        SLOT.store(c.sig | 1, Some(r));
        PAINTS.fetch_add(1, Release);
        true
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
        let face = text::Face::Body; // HUDSCALE (B860): the same fixed cell `geom` lays out on
        let (cw, ch, pad) = geom();
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

    /// Sample, and re-render when the text (or the theme, the face, the cell) changed. `true` = a new image is in; its
    /// changed pixels are owed as field bands ([`diff_bands`]), or the whole image when they cannot be.
    fn refresh() -> (bool, usize) {
        let s = sample();
        let mut sig = strip::FNV_BASIS;
        for (t, hot) in lines(&s).iter() {
            for b in t.bytes() {
                sig = strip::fnv1a(sig, b);
            }
            sig = fnv(sig, *hot as u64);
        }
        let (cw, ch, pad) = geom();
        sig = strip::seal(fnv(fnv(fnv(sig, text::epoch() as u64), cw as u64), ch as u64)); // PERFHUD2: the theme epoch and the wallpaper through the one sealer
        sig &= !1;
        let same = { let c = CACHE.lock(); c.sig == sig && c.w != 0 };
        if same {
            return (false, FIELDS);
        }
        let Some((px, w, h, fields)) = render(&s) else { return (false, 0) };
        let old = {
            let mut g = CACHE.lock();
            let c = &mut *g;
            // The diff runs under the hold (this task, IRQs on); a pass meanwhile skips its paint and the bands stay owed.
            if c.full || c.w != w || c.h != h || c.px.len() != w * h {
                c.full = true;
            } else {
                let mut d = c.dirty;
                if diff_bands(&c.px, &px, w, h, ch, pad, &mut d) {
                    c.dirty = d;
                } else {
                    c.full = true;
                }
            }
            let old = core::mem::replace(&mut c.px, px);
            c.w = w;
            c.h = h;
            c.sig = sig;
            old
        };
        drop(old);
        (true, fields)
    }

    /// PERFHUD2: union the pixels `new` changes from `old` (both `w` x `h`) into the field bands of `dirty`: per changed
    /// row the first..last differing column, in the band of field `(row - pad) / ch`. `false` = a change outside the nine
    /// field rows (the border or the padding: a theme change), so the whole image is owed.
    fn diff_bands(old: &[u32], new: &[u32], w: usize, h: usize, ch: usize, pad: usize, dirty: &mut [Option<strip::Rect>; FIELDS]) -> bool {
        let mut span: [Option<(usize, usize)>; FIELDS] = [None; FIELDS];
        for j in 0..h {
            let (a, b) = (&old[j * w..(j + 1) * w], &new[j * w..(j + 1) * w]);
            let Some(x0) = (0..w).find(|&i| a[i] != b[i]) else { continue };
            let x1 = (x0..w).rev().find(|&i| a[i] != b[i]).unwrap_or(x0);
            let k = if j >= pad && ch != 0 { (j - pad) / ch } else { FIELDS };
            if k >= FIELDS {
                return false;
            }
            span[k] = Some(match span[k] { Some((l, r)) => (l.min(x0), r.max(x1)), None => (x0, x1) });
        }
        for k in 0..FIELDS {
            if let Some((l, r)) = span[k] {
                let y0 = pad + k * ch;
                let band = (l, y0, r + 1 - l, ch.min(h - y0));
                dirty[k] = Some(match dirty[k] { Some(d) => union(d, band), None => band });
            }
        }
        true
    }

    /// The smallest rect holding both.
    fn union(a: strip::Rect, b: strip::Rect) -> strip::Rect {
        let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
        let (x1, y1) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// PERFHUD2: is a paint owed that the sampler's tick must drive (a latched hit, or bands/the whole image unpainted)?
    fn owed() -> bool {
        desktop_up() && (strip::overlay_pending() || CACHE.try_lock().is_some_and(|c| c.full || c.dirty.iter().any(|d| d.is_some())))
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

    /// The first paint after a show — always FULL (the hide's vacate cleared `SLOT`): its rect, cost and pixels as stored
    /// at that paint, once.
    fn shown_line() {
        let at = SHOWN_AT.load(Relaxed);
        if at == SHOWN_DONE || PAINTS.load(Acquire) <= at {
            return;
        }
        let (x, y, w, h) = strip::unpack_rect(SHOWN_RECT.load(Relaxed));
        let us = FULL_US.load(Relaxed);
        serial_println!(
            "[hud] shown rect={}x{}+{}+{} paint_us={} budget_us={} fields_dirty={} px={} -> {}",
            w, h, x, y, us, PAINT_BUDGET_US, FIELDS, FULL_PX.load(Relaxed), if us <= PAINT_BUDGET_US { "UNDER" } else { "OVER" }
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
            if refresh().0 | owed() {
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

    /// Drive composites until a paint past `p` lands (bounded 2 s).
    fn wait_paint(p: u64) -> bool {
        let t_end = crate::arch::ms() + 2_000;
        while PAINTS.load(Acquire) == p && crate::arch::ms() < t_end {
            drive();
            let w = crate::arch::ms() + 17;
            while PAINTS.load(Acquire) == p && crate::arch::ms() < w {
                core::hint::spin_loop();
            }
        }
        PAINTS.load(Acquire) > p
    }

    /// `tests perfhud`: toggle on, render inline. Leg 1: the FULL paint (driven until it lands, bounded 2 s). Leg 2: the
    /// STEADY paint — line 0's value cells marked dirty, driven until painted. Then three pixels of the margin read back
    /// from the panel (each must be `surface.overlay` under the applied colour filter, BP-video-25); restore the toggle.
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
        // Leg 1: force a fresh image, owed whole (the sampler may not have run yet).
        let p0 = PAINTS.load(Acquire);
        {
            let mut c = CACHE.lock();
            c.sig = 0;
            c.full = true;
        }
        let (_, fields) = refresh();
        let painted = wait_paint(p0);
        // Leg 2: the steady paint — line 0's value cells (columns 3..18) owed as one band.
        let (cw, ch, pad) = geom();
        let p1 = PAINTS.load(Acquire);
        {
            let mut c = CACHE.lock();
            if c.w >= pad + COLS * cw && c.h >= pad + ch {
                c.dirty[0] = Some((pad + 3 * cw, pad, (COLS - 3) * cw, ch));
            }
        }
        let steady = painted && wait_paint(p1);
        let (paint_us, px, nf) = (LAST_PAINT_US.load(Relaxed), LAST_PX.load(Relaxed), LAST_FIELDS.load(Relaxed));
        let (x, y, w, h) = SLOT.rect();
        let want = filt::color(theme::surface_overlay());
        let mut hits = 0;
        if steady && w > 6 && h > 6 {
            if let Some(fb) = super::super::panel_snapshot() {
                for (px, py) in [(x + 2, y + 2), (x + w - 3, y + 2), (x + 2, y + h - 3)] {
                    if fb.read_pixel(px, py) == Some(want) {
                        hits += 1;
                    }
                }
            }
        }
        if !was {
            let _ = super::latch(false, "test");
            drive();
        }
        let under = paint_us <= PAINT_BUDGET_US;
        let ok = painted && steady && hits == 3 && fields == FIELDS && vis == 1 && under; let ds = super::drain_sample(); // DRAINMETER (B859): the drain present's sample, reported, not gated
        serial_println!(
            ":: PERFHUD: visible={} paint_us={} fields={} -> {} :: pixels={}/3 budget_us={} under={} fields_dirty={} px={} full_us={} full_px={} scale={} rect={}x{}+{}+{} max_paint_us={} hud_paint_us={} cell={}x{} src=fixed ap={} present={} drain_n={} drain_us={} drain_max_us={}",
            vis, paint_us, fields, if ok { "PASS" } else { "FAIL" }, hits, PAINT_BUDGET_US, under as u8, nf, px,
            FULL_US.load(Relaxed), FULL_PX.load(Relaxed), super::super::dpi::scale_str(super::super::dpi::s2()), w, h, x, y,
            MAX_PAINT_US.load(Relaxed), FULL_US.load(Relaxed), cw, ch, crate::arch::memory::aperture_pat().map_or("-", crate::arch::memory::mem_type_name), if ds.0 == 0 { "-" } else { "drain" }, ds.0, ds.1, ds.2
        );
    }
    /// BEAMSPLIT (B860): the image-local rows `[top, bottom)` the dirty field bands span (`(0, 0)` = none dirty) —
    /// the one `strip::span_hold` bracket the band loop in [`compose`] paints inside.
    fn band_rows(dirty: &[Option<strip::Rect>; FIELDS]) -> (usize, usize) {
        let (mut y0, mut y1) = (usize::MAX, 0usize);
        for &(_, by, _, bh) in dirty.iter().flatten() {
            y0 = y0.min(by);
            y1 = y1.max(by + bh);
        }
        if y1 == 0 { (0, 0) } else { (y0, y1) }
    }
}

// DRAINMETER (rmbp-ledger B859; BP-video-11, BP-video-27) — TAIL-APPENDED so no line above moves. The drain-only present
// (`wm::drain_present_once`, SHIMMERPRESENT B840) takes no `lag::pass`, k2p or frame-time stamp; its cost is a
// present-kind term BESIDE the frame-time ring, never in it (the ring is the whole pass, the HUD's `ft` field; a drain
// mixed in would lie about frame time, and the HUD's painter is not this arc's). Recorded whether or not the HUD is shown
// (three relaxed atomics per drain present); `tests perfhud` reads it: `present=<drain|-> drain_n= drain_us= drain_max_us=`.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static DRAIN_N: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static DRAIN_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static DRAIN_MAX_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `wm::drain_present_once`'s tail (under COMP_GATE): one drain present ran, opened at cycle `t0`.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn drain_present(t0: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let us = super::blitter::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    DRAIN_N.fetch_add(1, Relaxed);
    DRAIN_US.store(us, Relaxed);
    DRAIN_MAX_US.fetch_max(us, Relaxed);
}

/// `(drain presents since boot, the last one's us, the worst us)`.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn drain_sample() -> (u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (DRAIN_N.load(Relaxed), DRAIN_US.load(Relaxed), DRAIN_MAX_US.load(Relaxed))
}
