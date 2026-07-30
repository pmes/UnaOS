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

//! Double-buffered display surface with damage tracking.
//!
//! The bare `FrameBuffer` writes straight to the framebuffer. That's fine for the boot console
//! (fbcon, pre-heap) but wrong as the steady-state renderer: framebuffer memory is slow and
//! write-combining on real hardware, so per-pixel pokes — and the GUI console repainting the
//! whole screen on every keystroke — crawl on metal and flicker.
//!
//! `Screen` fixes both. All drawing goes to a back buffer in ordinary cached RAM (fast per-pixel
//! writes, no flicker), and [`Screen::flush`] copies only the *damaged* region to the real
//! framebuffer as bulk sequential row copies (write-combining-friendly). The GUI draws to the
//! back buffer and calls `flush()` (via `pal`'s `render()`) once per frame.
//!
//! The back buffer is a second `FrameBuffer` pointing at heap memory, so it reuses all of the
//! surface's format-aware drawing logic — no duplicated pixel poking here.

use alloc::vec;
use alloc::vec::Vec;
use unaos_boot_info::FrameBufferInfo;

use super::FrameBuffer;

/// A damaged region as a half-open pixel rectangle `[x0, x1) x [y0, y1)`.
#[derive(Clone, Copy)]
struct Damage {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Damage {
    #[inline]
    fn overlaps(&self, o: &Damage) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
    #[inline]
    fn union(&self, o: &Damage) -> Damage {
        Damage {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    #[inline]
    fn area(&self) -> u64 {
        ((self.x1 - self.x0) as u64) * ((self.y1 - self.y0) as u64)
    }
}

/// VUG-FPS — the maximum number of independent damage rectangles tracked between flushes. Disjoint
/// dirty regions (vug's centre crystal, the bottom-left corner meters, the moving cursor, the
/// top-left HUD) each present as their own small row-copy blit rather than collapsing to a single
/// screen-spanning bounding box — the flush is bandwidth-bound, so bounding a rotating full-height
/// crystal + two corner widgets into ONE box reflushed most of the panel every frame (the metal
/// 8–9 fps). On overflow the set coalesces the least-growth pair, so it always presents a correct
/// SUPERSET of the true damage — never drops a dirty pixel, only (rarely) reflushes a clean one.
const MAX_DAMAGE_RECTS: usize = 16;

/// A small bounded set of damage rectangles. `len == 0` means nothing changed (flush is a no-op).
struct DamageSet {
    rects: [Damage; MAX_DAMAGE_RECTS],
    len: usize,
}

impl DamageSet {
    const fn empty() -> Self {
        let z = Damage { x0: 0, y0: 0, x1: 0, y1: 0 };
        Self { rects: [z; MAX_DAMAGE_RECTS], len: 0 }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn set_single(&mut self, d: Damage) {
        self.rects[0] = d;
        self.len = 1;
    }

    /// Add `r`, merging it into every rect it overlaps (cascading, since a merge can grow `r` into
    /// reach of a rect it previously missed). On a full set with no overlap, fold `r` into the
    /// existing rect whose union grows the total area least — keeps the flush a tight superset.
    fn add(&mut self, mut r: Damage) {
        if r.x0 >= r.x1 || r.y0 >= r.y1 {
            return;
        }
        let mut i = 0;
        while i < self.len {
            if self.rects[i].overlaps(&r) {
                r = r.union(&self.rects[i]);
                self.len -= 1;
                self.rects[i] = self.rects[self.len];
                i = 0; // r grew; rescan from the start
            } else {
                i += 1;
            }
        }
        if self.len < MAX_DAMAGE_RECTS {
            self.rects[self.len] = r;
            self.len += 1;
        } else {
            let mut best = 0usize;
            let mut best_grow = u64::MAX;
            for k in 0..self.len {
                let grow = self.rects[k].union(&r).area() - self.rects[k].area();
                if grow < best_grow {
                    best_grow = grow;
                    best = k;
                }
            }
            self.rects[best] = self.rects[best].union(&r);
        }
    }
}

/// VUG-PAR — the maximum number of parallel flush bands (1 render core + up to 3 helper APs). The Pi
/// 4 has 4 cores; keeping the cap here means a helper scan never exceeds the frame's true parallelism
/// and the on-stack job array stays fixed-size (no per-frame heap for the jobs themselves).
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
const MAX_BANDS: usize = 4;

/// VUG-PAR — don't pay the spawn/join cost for a trivially small flush: only band-split when the
/// damaged region spans at least this many scanlines. Below it the serial path is strictly cheaper
/// (a cursor-sized dirty rect is a handful of rows), so we fall through to the byte-identical serial
/// flush. Tuned conservatively — the win this arc chases is the panel-height rotating crystal.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
const PAR_MIN_ROWS: usize = 64;

/// VUG-PAR — immutable work shared by every band of one flush. Lives on the flushing task's stack for
/// the whole `flush` call; the join barrier guarantees no worker outlives it. `front` is a `Copy`
/// `FrameBuffer` (raw base held as `usize`, so `Send`); `back_ptr`/`back_len` address the back buffer
/// for READ-ONLY row copies. Bands write DISJOINT scanline ranges of `front`, so the concurrent blits
/// never alias.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
struct FlushCommon {
    front: FrameBuffer,
    back_ptr: usize,
    back_len: usize,
    rects: [Damage; MAX_DAMAGE_RECTS],
    nrects: usize,
    stride: usize,
    bpp: usize,
}

/// VUG-PAR — one band's slice of the flush: the shared work plus the half-open scanline range
/// `[yb0, yb1)` this band owns. Bands partition the damaged y-extent into disjoint contiguous ranges,
/// so two bands never touch the same framebuffer row.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
#[derive(Clone, Copy)]
struct BandJob {
    common: *const FlushCommon,
    yb0: usize,
    yb1: usize,
}

/// VUG-PAR — blit every damaged rect's rows that fall inside this band's `[yb0, yb1)` range, then
/// clean each rect's band-local span for the non-coherent scan-out. Read-only on the back buffer,
/// write-only on disjoint front rows — safe to run concurrently with the other bands.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
fn band_run(job: &BandJob) {
    // SAFETY: `common` points at the flushing task's live stack frame; the join barrier keeps that
    // frame alive until every band has returned. The back buffer is read-only here.
    let c = unsafe { &*job.common };
    let back = unsafe { core::slice::from_raw_parts(c.back_ptr as *const u8, c.back_len) };
    for i in 0..c.nrects {
        let d = c.rects[i];
        let y0 = d.y0.max(job.yb0);
        let y1 = d.y1.min(job.yb1);
        if d.x0 >= d.x1 || y0 >= y1 {
            continue;
        }
        let seg = (d.x1 - d.x0) * c.bpp;
        for y in y0..y1 {
            let off = (y * c.stride + d.x0) * c.bpp;
            if off + seg <= c.back_len {
                c.front.blit(off, &back[off..off + seg]);
            }
        }
        let span_start = (y0 * c.stride + d.x0) * c.bpp;
        let span_end = ((y1 - 1) * c.stride + d.x1) * c.bpp;
        c.front.flush_range(span_start, span_end - span_start);
    }
}

/// VUG-PAR — the `spawn_joinable` entry (a bare `fn(usize)`): the `usize` is the address of this
/// band's `BandJob` on the flushing task's stack.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
fn band_worker(arg: usize) {
    // SAFETY: `arg` is `&BandJob as usize` from `flush_parallel`, valid until the join barrier.
    band_run(unsafe { &*(arg as *const BandJob) });
}

/// SPREAD-2 — the floor under a core's headroom weight. A core reading 100% busy still gets
/// `HEADROOM_FLOOR/100` of an equal share rather than an empty band: the blit is memory-bound, a
/// saturated core is not a stalled one, and a hard zero would let one bad window's reading park a
/// whole core's worth of rows on its neighbours.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
const HEADROOM_FLOOR: u32 = 25;

/// SPREAD-2 — frames per `[spread2]` rollup. The witness is one line per window, not per band per
/// frame; the per-spawn `SCHED: task 'vugband'` line stays the trace, this is the number.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
const SPREAD2_WINDOW: u32 = 60;

/// SPREAD-2 — widest core count the rollup counters cover (`NUM_CPUS` upper bound; `video` does not
/// depend on the arch scheduler's array shape).
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
const SPREAD2_CORES: usize = 8;

#[cfg(all(feature = "vugpar", feature = "baremetal"))]
static SPREAD2_BANDS: [core::sync::atomic::AtomicU32; SPREAD2_CORES] =
    [const { core::sync::atomic::AtomicU32::new(0) }; SPREAD2_CORES];
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
static SPREAD2_ROWS: [core::sync::atomic::AtomicU32; SPREAD2_CORES] =
    [const { core::sync::atomic::AtomicU32::new(0) }; SPREAD2_CORES];
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
static SPREAD2_FRAMES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// WEDGE-1 — candidate cores rejected by the FRESHNESS gate in [`Screen::flush_parallel`]: `tracked`
/// said the core's load number was printable, but it had not folded a dispatch span within
/// `sched::dispatch_fresh_cyc`, so it was not given a pinned band.
///
/// This is the arc's one directly-falsifiable metal number, and it is why it is a counter rather than
/// a comment. Every increment is a band that WOULD have been pinned — non-stealable, untimed join —
/// onto a core that was not dispatching. On a calm boot it must stay 0: a live core folds a span
/// every dispatch pass and cannot trip a ~30 ms bound.
///
/// What a non-zero reading means, and what it does NOT: it means cores are dropping out of the
/// dispatch loop long enough to be caught, which is a fact worth having and currently unmeasured. It
/// is NOT evidence for any particular account of the P66 wedge — that mechanism is unknown, and this
/// counter is instrumentation for the next boot rather than confirmation of a diagnosis.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
static WEDGE1_STALE_DECLINED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// SPREAD-2 — a core's share weight from its momentary busy fraction. Headroom, floored; an equal
/// set of weights reproduces the old uniform split exactly.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
fn headroom_weight(busy_pct: u32) -> u32 {
    100u32.saturating_sub(busy_pct.min(100)).max(HEADROOM_FLOOR)
}

/// SPREAD-2 — accumulate this frame's band placement and, once per `SPREAD2_WINDOW` frames, emit the
/// distribution rollup: per-core band counts, per-core rows, and the max/min row ratio in hundredths
/// over the cores that actually took rows. `ratio 100` is a perfectly even spread; the P65v2 state
/// (two cores carrying two vugs' bands, two carrying one) is what this line exists to make numeric.
#[cfg(all(feature = "vugpar", feature = "baremetal"))]
fn spread2_note(self_cpu: usize, helpers: &[usize], jobs: &[BandJob]) {
    use core::sync::atomic::Ordering;

    for (b, job) in jobs.iter().enumerate() {
        let core = if b == 0 { self_cpu } else { helpers[b - 1] };
        if core >= SPREAD2_CORES {
            continue;
        }
        SPREAD2_BANDS[core].fetch_add(1, Ordering::Relaxed);
        SPREAD2_ROWS[core].fetch_add(job.yb1.saturating_sub(job.yb0) as u32, Ordering::Relaxed);
    }

    if SPREAD2_FRAMES.fetch_add(1, Ordering::Relaxed) + 1 < SPREAD2_WINDOW {
        return;
    }
    SPREAD2_FRAMES.store(0, Ordering::Relaxed);

    let mut bands = [0u32; SPREAD2_CORES];
    let mut rows = [0u32; SPREAD2_CORES];
    // ROWS-PER-BAND in hundredths. Raw `rows` cannot be compared across cores: a core that goes
    // `tracked` late in the window (AP late-online, vug churn) takes fewer BANDS, so its row total is
    // low for a reason that has nothing to do with the split. Normalizing by that core's own band
    // count is what makes the ratio a statement about WEIGHTING rather than about participation.
    let mut rpb = [0u32; SPREAD2_CORES];
    let mut hi = 0u32;
    let mut lo = u32::MAX;
    let mut live = 0usize;
    for c in 0..SPREAD2_CORES {
        bands[c] = SPREAD2_BANDS[c].swap(0, Ordering::Relaxed);
        rows[c] = SPREAD2_ROWS[c].swap(0, Ordering::Relaxed);
        if bands[c] > 0 {
            rpb[c] = (rows[c] as u64 * 100 / bands[c] as u64) as u32;
            hi = hi.max(rpb[c]);
            lo = lo.min(rpb[c]);
            live += 1;
        }
    }
    if live == 0 {
        return;
    }
    // Ratio of the fattest to the thinnest average band, in hundredths (100 = perfectly even).
    // `lo == 0` means a core drew bands but no rows for a whole window — with `PAR_MIN_ROWS` at 64 and
    // the floor bounding the thinnest band well above zero, that is pathological, not an edge case, so
    // it reports as a sentinel that trips the spec rather than as a benign 0.
    let ratio = if lo == 0 { 9999 } else { (hi as u64 * 100 / lo as u64) as u32 };

    serial_println!(
        ":: [spread2] window {} frames cores {} bands {},{},{},{} rows {},{},{},{} rpb {},{},{},{} ratio {} stale {} ::",
        SPREAD2_WINDOW,
        live,
        bands[0],
        bands[1],
        bands[2],
        bands[3],
        rows[0],
        rows[1],
        rows[2],
        rows[3],
        rpb[0],
        rpb[1],
        rpb[2],
        rpb[3],
        ratio,
        WEDGE1_STALE_DECLINED.swap(0, core::sync::atomic::Ordering::Relaxed)
    );
}

/// FOCUS-VIS — a pending request for the NEXT desktop present to repaint the WHOLE panel.
///
/// The desktop (`Screen`) presents only its own damage, so a region the *window layer* overwrote is
/// never repainted by the desktop: nothing in `Screen` knows a window covered it. That is exactly the
/// state `wm::focus_changed(0)` creates when the operator TABs to the shell — the console's text is
/// intact in the back buffer and stale on the panel, with no damage to make it move.
///
/// A flag rather than a call because the `Screen` is OWNED by the render task (it lives in that task's
/// `TargetPal`); there is no global handle the compositor could reach it through, and inventing one
/// would give a second core a `&mut Screen`. The compositor raises this from syscall context; the
/// render task's own next present consumes it, on its own thread, under its own ownership.
static FULL_PRESENT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// FOCUS-VIS — ask the desktop layer to repaint the whole panel on its next present. Idempotent; the
/// request is consumed by the first [`Screen::flush`] that follows.
///
/// Latency, stated: the render task presents only when something marked it dirty, and the 1 Hz status
/// strip tick is the floor — so an otherwise idle desktop repaints within ~1 s of the request. The
/// caller (`wm::focus_changed`) does not rely on that for its own visible effect: it erases the
/// windows' boxes to the desktop colour immediately, and this restores the console's *text* on top of
/// that erase when the desktop next runs.
pub fn request_full_present() {
    FULL_PRESENT.store(true, core::sync::atomic::Ordering::Release);
}

/// WC-I — one step of the row-span walk that subtracts the window layer from a damage rect.
///
/// Given the occluder boxes, a scanline `y`, a cursor `xs` and the rect's right edge `x1`, returns
/// `(gap_end, next)`: the background is copied over `[xs, gap_end)` and the walk resumes at `next`.
/// `next` is always strictly greater than `xs`, which is what bounds the caller's loop.
///
/// Two cases, and no set arithmetic:
///  * some occluder COVERS `xs` — the gap is empty and the walk jumps to the furthest right edge of
///    the occluders covering it, so a pile of overlapping windows costs one step, not one per window;
///  * none does — the gap runs to the nearest occluder start to the right of `xs` on this row, or to
///    `x1` when there is none.
///
/// Deliberately not a sorted-interval merge: `MAX_WINDOWS` is 8 and this runs per scanline of a
/// damage rect, so a linear scan over at most eight boxes beats building any structure, and it needs
/// no allocation on a path that must not allocate.
fn next_visible_span(
    occ: &[(usize, usize, usize, usize)],
    y: usize,
    xs: usize,
    x1: usize,
) -> (usize, usize) {
    let mut cover_end = xs;
    for &(bx, by, bw, bh) in occ {
        if y < by || y >= by.saturating_add(bh) {
            continue;
        }
        if xs >= bx && xs < bx.saturating_add(bw) {
            cover_end = cover_end.max(bx.saturating_add(bw));
        }
    }
    if cover_end > xs {
        return (xs, cover_end.min(x1).max(xs + 1));
    }
    let mut gap_end = x1;
    for &(bx, by, bw, bh) in occ {
        if y < by || y >= by.saturating_add(bh) || bw == 0 {
            continue;
        }
        if bx > xs && bx < gap_end {
            gap_end = bx;
        }
    }
    (gap_end, gap_end.max(xs + 1))
}

pub struct Screen {
    /// The real framebuffer (flush target).
    front: FrameBuffer,
    /// Owns the back-buffer memory (cached RAM), same byte layout as `front`.
    back_store: Vec<u8>,
    /// A surface handle pointing into `back_store`. SAFETY/INVARIANT: `back_store` is allocated
    /// once at its final size in `new` and never grown/shrunk, so its heap buffer never moves —
    /// the address captured here stays valid for the life of the `Screen` (moving the `Screen`
    /// moves the `Vec` header, not the heap allocation it points at).
    back: FrameBuffer,
    info: FrameBufferInfo,
    /// Accumulated dirty rectangles since the last flush; empty when nothing changed.
    damage: DamageSet,
    /// Bytes copied by the most recent [`flush`] (VUG-FPS bandwidth witness).
    last_flush_bytes: u64,
    /// VUG-PAR — number of parallel bands the most recent [`flush`] actually used (1 = serial /
    /// fallback / feature-off). The `[vugfps]` witness reports it as `bands=N`.
    last_flush_bands: usize,
    /// VUG-FPS-2 — number of (merged) damage rectangles the most recent [`flush`] presented. Names
    /// whether the 16-rect merge cascade is collapsing the frame's disjoint regions into one big box.
    last_flush_rects: usize,
    /// VUG-FPS-2 — bounding-box dimensions (pixels) of the union of all damage rects the most recent
    /// [`flush`] presented. If this is ~panel-sized every frame, the dirty-rect union is the whole
    /// screen and bytes/frame can never drop — the number that explains the ~3.5 MB/frame plateau.
    last_union_w: usize,
    last_union_h: usize,
}

impl Screen {
    /// Build a double-buffered screen over `front`. Allocates a back buffer the same size as the
    /// framebuffer and marks the whole frame dirty so the first `flush` paints everything.
    pub fn new(front: FrameBuffer) -> Self {
        let info = front.info();
        // Single source of truth for the buffer length: the front framebuffer's reported size.
        // Sizing the back store to exactly `front.len()` guarantees the two surfaces agree, so a
        // flush can never have one bounds check pass while the other rejects (which would silently
        // drop rows). Firmware sometimes reports a size != stride*height*bpp — e.g. Apple's Retina
        // GOP — so we warn if it's *short* of the visible image (rows past the end can't be shown).
        let computed = info.stride * info.height * info.bytes_per_pixel;
        let len = front.len();
        if len < computed {
            serial_println!(
                ":: VIDEO WARNING: framebuffer_size {} < stride*height*bpp {} (firmware quirk); \
                 bottom rows may not display ::",
                len,
                computed
            );
        }
        let mut back_store = vec![0u8; len];
        let mut back = FrameBuffer::new();
        back.init(back_store.as_mut_ptr() as usize, len, info);
        Self {
            front,
            back_store,
            back,
            info,
            damage: {
                let mut ds = DamageSet::empty();
                ds.set_single(Damage { x0: 0, y0: 0, x1: info.width, y1: info.height });
                ds
            },
            last_flush_bytes: 0,
            last_flush_bands: 1,
            last_flush_rects: 0,
            last_union_w: 0,
            last_union_h: 0,
        }
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.info.width
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.info.height
    }

    /// Grow the dirty rectangle to include `[x0, x1) x [y0, y1)` (clamped to the frame).
    fn mark(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        let x1 = x1.min(self.info.width);
        let y1 = y1.min(self.info.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.damage.add(Damage { x0, y0, x1, y1 });
    }

    fn mark_full(&mut self) {
        self.damage.set_single(Damage { x0: 0, y0: 0, x1: self.info.width, y1: self.info.height });
    }

    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, color: u32) {
        self.back.put_pixel(x, y, color);
        self.mark(x, y, x + 1, y + 1);
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        self.back.fill_rect(x, y, w, h, color);
        self.mark(x, y, x + w, y + h);
    }

    pub fn fill_screen(&mut self, color: u32) {
        self.back.fill_screen(color);
        self.mark_full();
    }

    /// Draw a Bresenham line into the back buffer and mark its bounding box damaged. Endpoints are
    /// signed; the surface clips per-pixel. The `vug` wireframe primitive.
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
        self.back.draw_line(x0, y0, x1, y1, color);
        let lo_x = x0.min(x1).max(0) as usize;
        let lo_y = y0.min(y1).max(0) as usize;
        let hi_x = x0.max(x1).max(0) as usize;
        let hi_y = y0.max(y1).max(0) as usize;
        self.mark(lo_x, lo_y, hi_x + 1, hi_y + 1);
    }

    /// Fill the triangle `(a, b, c)` (pixel coordinates) with a flat colour, marking its bounding
    /// box damaged. Half-space scanline rasteriser: for each row spanning the triangle's vertical
    /// extent, fill between the two edge intersections. The `vug` solid-facet primitive; the caller
    /// does backface culling and painter's-order sorting, so this just fills.
    pub fn fill_triangle(
        &mut self,
        a: (i32, i32),
        b: (i32, i32),
        c: (i32, i32),
        color: u32,
    ) {
        // Sort vertices by y ascending: p0.y <= p1.y <= p2.y.
        let mut p = [a, b, c];
        p.sort_unstable_by_key(|v| v.1);
        let (p0, p1, p2) = (p[0], p[1], p[2]);
        let w = self.info.width as i32;
        let h = self.info.height as i32;

        // Interpolate an x at scanline `y` along the edge from `q` to `r` (in 1/65536 px).
        let edge_x = |q: (i32, i32), r: (i32, i32), y: i32| -> i32 {
            if r.1 == q.1 {
                return q.0;
            }
            q.0 + ((r.0 - q.0) as i64 * (y - q.1) as i64 / (r.1 - q.1) as i64) as i32
        };

        let y_top = p0.1.max(0);
        let y_bot = p2.1.min(h - 1);
        let mut min_x = w;
        let mut max_x = 0;
        let mut y = y_top;
        while y <= y_bot {
            // Long edge p0->p2 spans the whole height; the short edge switches at p1.y.
            let xa = edge_x(p0, p2, y);
            let xb = if y < p1.1 {
                edge_x(p0, p1, y)
            } else {
                edge_x(p1, p2, y)
            };
            let (mut xl, mut xr) = if xa <= xb { (xa, xb) } else { (xb, xa) };
            xl = xl.max(0);
            xr = xr.min(w - 1);
            if xl <= xr {
                let run = (xr - xl + 1) as usize;
                self.back.fill_rect(xl as usize, y as usize, run, 1, color);
                min_x = min_x.min(xl);
                max_x = max_x.max(xr);
            }
            y += 1;
        }
        if min_x <= max_x {
            self.mark(min_x as usize, y_top as usize, max_x as usize + 1, y_bot as usize + 1);
        }
    }

    pub fn scroll_up(&mut self, dy: usize, fill: u32) {
        self.back.scroll_up(dy, fill);
        self.mark_full();
    }

    /// Total bytes the last [`flush`] copied to the framebuffer — the VUG-FPS bandwidth witness
    /// (`[vugfps]` reads it to report flushed-bytes/frame, the number the dirty-rect win moves).
    #[inline]
    pub fn last_flush_bytes(&self) -> u64 {
        self.last_flush_bytes
    }

    /// VUG-PAR — parallel bands the last [`flush`] used (1 = serial / fallback / feature-off). The
    /// `[vugfps]` witness prints it as `bands=N` so a metal capture reads the parallel win directly.
    #[inline]
    pub fn last_flush_bands(&self) -> usize {
        self.last_flush_bands
    }

    /// VUG-FPS-2 — merged damage-rect count of the last [`flush`] (`[vugfps]` prints `rects=N`).
    #[inline]
    pub fn last_flush_rects(&self) -> usize {
        self.last_flush_rects
    }

    /// VUG-FPS-2 — union bounding-box `(w, h)` of the last [`flush`]'s damage (`[vugfps]` prints
    /// `union=WxH`). Compared against the panel size it tells P46 whether the dirty region is
    /// effectively the whole screen (why bytes/frame stays ~3.5 MB) or genuinely sub-panel.
    #[inline]
    pub fn last_union_dims(&self) -> (usize, usize) {
        (self.last_union_w, self.last_union_h)
    }

    /// Present this frame: paint the desktop (the back buffer) and then re-composite the window
    /// layer over it.
    ///
    /// ### WC-E — why the second step is not optional
    /// There are TWO writers to the one scan-out buffer, and until WC-E nothing ordered them. This
    /// `Screen` is the desktop layer: the VUG render task draws into a back buffer and flushes the
    /// damaged rectangles here, ~20 times a second, with damage unions that routinely span the whole
    /// panel. The window compositor (`video::wm`) is the window layer: it pokes window pixels
    /// DIRECTLY into the same framebuffer from the presenting task's syscall context, and it runs
    /// only when a window presents. Neither knew about the other. The consequence on a real panel is
    /// that every desktop flush silently erased whatever windows had drawn, and the next window
    /// present drew them back — the window region alternating between window content and desktop
    /// content at the render task's frame rate, in whatever partial bands the two writers' timing
    /// happened to interleave. That is the Pi 4 "window garble": not a scan-out defect, an ordering
    /// one. (It survived every witness the compositor had because those read the framebuffer back
    /// microseconds after the blit, inside the same present, long before the next desktop flush.)
    ///
    /// The fix is the layering the two paths always implied: the desktop is the background, so the
    /// window layer is restored ON TOP of it at the end of every present that painted background.
    /// Ordering — background then windows, in one call — is what makes "windows are above the
    /// desktop" true continuously instead of only until the next frame.
    ///
    /// Residual, stated honestly: the window pixels are overwritten and repainted within the same
    /// present rather than never being overwritten at all, so a window can still be caught mid-repaint
    /// by a scan-out that lands between the two steps. Eliminating that needs the flush to SKIP the
    /// rows a window owns (or a full double-buffered composite), which is a larger change than this
    /// arc; what this removes is the unbounded, every-frame erasure.
    ///
    /// ### WC-I — the residual above is now closed, and it had a period
    ///
    /// "A window can still be caught mid-repaint by a scan-out that lands between the two steps" was
    /// written as a bounded, occasional cost. On the bench it is neither: `main.rs::status_tick` posts
    /// an `Event::Timer` once a second, the render task recomposes the PI-UI-2 status strip and calls
    /// `pal.render`, and this function's `wm::repaint()` then re-blitted EVERY live window. That is the
    /// P60 report exactly — a blip in every vug window at the same instant, a little faster than once a
    /// second (Key and Button passes mark the strip dirty too), with the desktop, the console and the
    /// low-rate stat window clean because none of them is being written by two painters at once.
    ///
    /// It was also worse than one overwrite-and-restore. The repaint runs a full composite from the
    /// RENDER task's core while the vug windows present from theirs, so `wm`'s single `STAGE` back
    /// layer is contended: `try_lock` declines, and a declining window takes the pre-WC-H direct path —
    /// per-pixel writes into live scan-out, the tearing regime WC-H was built to leave. One window
    /// rarely collides; N windows collide N times per tick, which is why the symptom needed several
    /// vugs to appear.
    ///
    /// WC-I removes the overwrite instead of sequencing it: [`present_background`] subtracts the window
    /// layer's boxes from its own damage, so desktop pixels are never written where a window is. There
    /// is then nothing for a repaint to undo, and the blanket re-blit goes with it — what remains is
    /// [`wm::service_damage`](super::wm::service_damage), which composites only rows something else
    /// actually marked (chiefly `cursor::repair`, whose "marks only" contract needs a pass within a
    /// frame). `wm::repaint` is still called on the one path that could still intrude: a present that
    /// reports it did not apply the subtraction exactly.
    ///
    /// ### CURSOR-13 — one owner for the sprite, and the bracket moved INSIDE
    ///
    /// The render task used to wrap this whole call in `cursor::undraw()` … `cursor::repaint()`
    /// (the CURSOR-1 contract, on both arches). That bracket is correct for the DESKTOP half and
    /// fatal to the WINDOW half: the composite below is reached from inside it, so
    /// `cursor::sprite_plan()` saw `sp.drawn == false` on 100% of flush-path passes, and CURSOR-3's
    /// compose-through — plus CURSOR-11's deferral — was structurally unreachable there. That is
    /// P74's `[cursor12] … -> nosprite` on GR7 s48 silicon (42/42) and the same reading on x86.
    /// Two mechanisms owned the sprite and the older one starved the newer.
    ///
    /// The bracket is not deleted, it is NARROWED to the half that needs it, and it lives here
    /// rather than in the caller so no flush site has to remember it (the same argument that split
    /// [`present_background`] out in the first place):
    ///
    /// * **[`present_background`] stays bracketed.** It is a raw desktop blit into the front
    ///   framebuffer. It subtracts the WINDOW layer's boxes (WC-I) and knows nothing about the
    ///   sprite, so a live arrow standing over desktop would be overwritten and its save-under left
    ///   describing pixels that are no longer there. There is no compose-through to engage on this
    ///   path — the desktop is not a staged surface — so a bracket here costs exactly one
    ///   restore/save/draw per present and buys coherence. `cursor::note_desktop_over_sprite`
    ///   (CURSOR-6) remains what it always was: a hole detector for THIS bracket, expected 0.
    ///   FLICKER-3 — the bracket is now taken only when the present can reach the sprite; see
    ///   [`Self::bracket_needed`] for the decision and for the classes that keep it unconditionally.
    /// * **The composite below runs with the sprite ON GLASS.** `sprite_plan()` returns a real plan,
    ///   so the pass takes the machinery that already exists: a staged window composes the arrow into
    ///   its own rows (`compose_into`/`adopt_overlay`), CURSOR-11's pend class defers the handback
    ///   with its coverage-install-first settle, and the DIRECT/unstaged and sessionless arms take
    ///   their own brackets exactly as they do on the present path today
    ///   (`undraw_within_nosession`). Nothing new is invented; the path is simply no longer
    ///   pre-emptied of its subject.
    ///
    /// Ordering is load-bearing in both directions. `cursor::repaint` is called AFTER
    /// `present_background` and BEFORE the composite so the save-under is taken against a front
    /// buffer whose desktop is already final; and its `repair` tail damages every window the restore
    /// crossed, which the composite on the very next line then services within this same flush —
    /// CURSOR-9's colour-guard residual is therefore mended sooner than before, not later.
    /// `TOUCHED_SINCE_DRAW` is untouched: a present landing under a live sprite still arms the
    /// repair through `note_present_over_sprite`, and it now has a live sprite to arm it for.
    pub fn flush(&mut self) {
        // FLICKER-3 — the CURSOR-13 bracket is owed only when this present can actually REACH the
        // sprite. It used to be unconditional: every desktop present — chiefly the status strip's
        // per-core load bars, a one-line band at the panel bottom repainting about once a second —
        // took the whole sprite down and redrew it, wherever the pointer stood. That is P80's
        // "the core idle bars cause mouse to flicker when they move", and it is the same shape
        // FLICKER-2 removed from `drain_deferred`: a whole-sprite bracket paid for paint that can
        // never touch a sprite pixel. The undraw's one justification — hand a pixel back before a
        // painter in THIS operation overwrites it — applies only when some damage rect meets the
        // sprite's box, so `bracket_needed` tests exactly that and a disjoint present leaves the
        // arrow on glass entirely. The decision's snapshot can be one pointer report stale, with the
        // same degradation `drain_deferred` argues: a sprite that moved INTO the damage mid-present
        // is re-established by the mover's own `repaint`, and `present_background`'s CURSOR-6 probe
        // (`note_desktop_over_sprite`) now doubles as the detector for that race — a blit that lands
        // on a live sprite is counted there, on either path.
        let bracket = self.bracket_needed();
        if bracket {
            // CURSOR-13 — the DESKTOP bracket. Opens here, closes before the window layer is touched.
            super::cursor::undraw();
        }
        let intruded = self.present_background();
        // CURSOR-13 — bracket CLOSED. Everything below composites with the arrow on the panel, which
        // is the whole point of that arc: `sprite_plan()` must be able to answer.
        if bracket {
            super::cursor::repaint();
        }
        // Only when background pixels actually landed ON a window — which, with the subtraction in
        // place, is the fallback path only. `repaint` self-guards on there being a window layer to
        // restore (one table-lock acquisition, then out).
        if intruded {
            super::wm::repaint();
        } else {
            super::wm::service_damage();
        }
    }

    /// FLICKER-3 — does this present owe the sprite the CURSOR-13 bracket?
    ///
    /// The skip is deliberately narrow: it is taken ONLY for a sprite that is on glass, currently
    /// visible, with no whole-panel present pending and every damage rect disjoint from its box —
    /// i.e. exactly the class where the bracket restores and redraws an arrow nothing in this
    /// present can touch. Every other class keeps the bracket it has always had:
    ///
    /// * **No sprite on glass** — the undraw is a no-op, but the repaint may owe a DRAW (this is
    ///   the desktop-cadence recovery path for a sprite something took down without a tail), so it
    ///   stays.
    /// * **Visibility lapsed (CURSOR-HIDE)** — this bracket's repaint at desktop cadence is what
    ///   takes a timed-out sprite off the panel; skipping here would leave a parked arrow standing
    ///   past its 1.5 s.
    /// * **`FULL_PRESENT` pending** — the present's paint set is the whole panel; every sprite
    ///   pixel is in it. Read with `load`, not `swap`: consuming the flag is `present_background`'s
    ///   job and it must still see it.
    ///
    /// Only the live-sprite decision is counted (`[flick2] flush_undraw=`/`flush_skip=`): the
    /// legacy classes cannot blink an arrow the operator can see, and counting them would bury the
    /// discriminator this exists to put on the wire.
    fn bracket_needed(&self) -> bool {
        let Some((sx, sy, sw, sh)) = super::cursor::sprite_box() else {
            return true;
        };
        if !crate::pal::cursor::visible() {
            return true;
        }
        let taken = FULL_PRESENT.load(core::sync::atomic::Ordering::Acquire)
            || (0..self.damage.len).any(|i| {
                let d = self.damage.rects[i];
                let x1 = d.x1.min(self.info.width);
                let y1 = d.y1.min(self.info.height);
                d.x0 < x1
                    && d.y0 < y1
                    && d.x0 < sx + sw
                    && sx < x1
                    && d.y0 < sy + sh
                    && sy < y1
            });
        super::cursor::note_flush_bracket(taken);
        taken
    }

    /// Present the back buffer: copy each damaged rectangle to the framebuffer, row by row (each
    /// row a single bulk copy), then clear the damage. No-op if nothing changed. VUG-FPS: the set
    /// holds disjoint dirty regions, so a rotating crystal plus two corner widgets blit as a few
    /// tight rectangles instead of one panel-spanning box.
    ///
    /// The DESKTOP half of [`flush`] — it knows nothing about windows; see that function for the
    /// layering. Split out so both of its exits (the parallel band path and the serial fallback) are
    /// covered by the window repaint without either having to remember to call it.
    /// WC-I — returns whether this present wrote background pixels INTO the window layer (an
    /// "intrusion"). `false` is the normal answer and means the caller owes the window layer nothing;
    /// `true` means the subtraction could not be applied and WC-E's restore is still required.
    fn present_background(&mut self) -> bool {
        // FOCUS-VIS: honour a pending whole-panel repaint request BEFORE the damage set is read. This
        // is how the console comes back out from under a window layer that stopped drawing (see
        // `request_full_present`) — the back buffer already holds the right pixels; only the damage
        // that would carry them forward is missing.
        if FULL_PRESENT.swap(false, core::sync::atomic::Ordering::AcqRel) {
            self.mark_full();
        }
        let n = self.damage.len;
        self.damage.clear();
        self.last_flush_bytes = 0;
        self.last_flush_bands = 1;
        self.last_flush_rects = 0;
        self.last_union_w = 0;
        self.last_union_h = 0;
        if n == 0 || !self.front.is_ready() {
            return false;
        }
        // WC-I — the window layer's boxes, snapshotted ONCE for the whole present so every rect of
        // this frame is subtracted against the same layout (a window that moves mid-present is
        // repainted by the mover's own composite either way). Empty on a windowless desktop, which is
        // every full-screen VUG frame and every gate boot before the window fixtures run.
        let mut occ = [(0usize, 0usize, 0usize, 0usize); super::wm::MAX_WINDOWS];
        let nocc = super::wm::occluders(&mut occ);
        let occ = &occ[..nocc];
        // VUG-FPS-2 witness: the merged rect count and the union bbox of all damage this frame. The
        // rects array still holds the pre-clear data (clear() only zeroed len), so read [0..n].
        {
            let (mut ux0, mut uy0, mut ux1, mut uy1) = (usize::MAX, usize::MAX, 0usize, 0usize);
            let mut live = 0usize;
            for idx in 0..n {
                let d = self.damage.rects[idx];
                let x1 = d.x1.min(self.info.width);
                let y1 = d.y1.min(self.info.height);
                if d.x0 >= x1 || d.y0 >= y1 {
                    continue;
                }
                live += 1;
                ux0 = ux0.min(d.x0);
                uy0 = uy0.min(d.y0);
                ux1 = ux1.max(x1);
                uy1 = uy1.max(y1);
            }
            self.last_flush_rects = live;
            if ux1 > ux0 && uy1 > uy0 {
                self.last_union_w = ux1 - ux0;
                self.last_union_h = uy1 - uy0;
            }
        }
        // VUG-PAR: try to fan the row-copy work across free secondary cores. Returns true when it
        // handled the flush (>= 2 bands); false to fall through to the byte-identical serial path
        // (no free AP, or too little work to amortize the spawn/join).
        //
        // WC-I: only when the window layer is EMPTY. The band workers copy whole clipped rects and
        // know nothing about occluders; teaching them the subtraction would put the span walk on
        // three cores for no benefit, because the case that needs the subtraction (several windowed
        // apps on the panel) is also the case where the desktop's own damage is small — a status
        // strip, a console line — and the parallel path declines it as too little work anyway. The
        // full-screen VUG frame, which is what VUG-PAR was built for, has no windows and is unchanged.
        #[cfg(all(feature = "vugpar", feature = "baremetal"))]
        if occ.is_empty() {
            // CURSOR-6 — the band path copies whole clipped rects and returns without reaching the
            // serial loop's probe, so it owes the same test or the counter would be blind on exactly
            // the windowless full-screen VUG frame this path exists for. No subtraction applies here
            // (`occ` is empty), so the clipped rects ARE what lands on the panel. Taken only on the
            // branch that actually PRESENTED in bands — a declined `flush_parallel` falls through to
            // the serial loop, which runs its own probe, and counting both would double-count.
            #[cfg(feature = "witness")]
            let sbox = super::cursor::live_box_relaxed();
            #[cfg(feature = "witness")]
            let banded = {
                let mut hit = false;
                if let Some((sx, sy, sw, sh)) = sbox {
                    for idx in 0..n {
                        let d = self.damage.rects[idx];
                        let x1 = d.x1.min(self.info.width);
                        let y1 = d.y1.min(self.info.height);
                        if d.x0 < x1
                            && d.y0 < y1
                            && d.x0 < sx + sw
                            && sx < x1
                            && d.y0 < sy + sh
                            && sy < y1
                        {
                            hit = true;
                            break;
                        }
                    }
                }
                hit
            };
            if self.flush_parallel(n) {
                #[cfg(feature = "witness")]
                if banded {
                    super::cursor::note_desktop_over_sprite();
                }
                return false;
            }
        }
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let mut flushed: u64 = 0;
        // CURSOR-6 — the sprite's box, once for the whole present, read WITHOUT the sprite lock. This
        // function is bracketed by `flush` (`cursor::undraw` → here → `cursor::repaint`; CURSOR-13
        // narrowed that bracket from the whole flush to this call, and the bracket's OWNER moved from
        // the render task to `flush` itself). FLICKER-3 narrowed it once more: a present whose damage
        // is provably disjoint from a live sprite skips the bracket and runs with the arrow on glass —
        // so "a live sprite must never be seen here" became "a live sprite must never be seen UNDER A
        // DAMAGE RECT here". The per-rect overlap test below asks exactly that, on both arms, so the
        // counter keeps its meaning: a hit is a desktop blit landing on a live arrow, whether from a
        // bracket hole or from a skip decision the sprite outran. Diagnostic only: nothing below is
        // conditional on it, so a stale answer costs precision and never a pixel.
        #[cfg(feature = "witness")]
        let sprite_box = super::cursor::live_box_relaxed();
        #[cfg(feature = "witness")]
        let mut over_sprite = false;
        for idx in 0..n {
            let d = self.damage.rects[idx];
            let x1 = d.x1.min(self.info.width);
            let y1 = d.y1.min(self.info.height);
            if d.x0 >= x1 || d.y0 >= y1 {
                continue;
            }
            for y in d.y0..y1 {
                // WC-I — copy this row of the rect in the sub-spans the window layer does NOT own.
                // Row granularity is the natural unit here: the copy is already one bulk `blit` per
                // row, so subtracting a window turns one `blit` into at most `nocc + 1` shorter ones
                // and never into a per-pixel loop. `next_visible_span` merges a pile of overlapping
                // windows in one step, so no sort and no interval set is needed for eight boxes.
                let mut xs = d.x0;
                let mut guard = 0usize;
                // At most one step per occluder edge, plus the final gap — a hard bound on a loop
                // whose progress already comes from `next > xs`, so a future occluder shape can never
                // spin the render task.
                while xs < x1 && guard <= 2 * occ.len() + 2 {
                    guard += 1;
                    let (gap_end, next) = next_visible_span(occ, y, xs, x1);
                    if gap_end > xs {
                        let off = (y * stride + xs) * bpp;
                        let seg = (gap_end - xs) * bpp;
                        if off + seg <= self.back_store.len() {
                            self.front.blit(off, &self.back_store[off..off + seg]);
                            flushed += seg as u64;
                        }
                        // CURSOR-6 — did this surviving span land on the sprite? Latched, not
                        // counted per span: the unit that means something is "one desktop present
                        // erased the arrow", and a per-span count would report the arrow's height
                        // instead. Short-circuited once latched so a full-panel flush pays the test
                        // at most once.
                        #[cfg(feature = "witness")]
                        if !over_sprite {
                            if let Some((sx, sy, sw, sh)) = sprite_box {
                                if y >= sy && y < sy + sh && xs < sx + sw && sx < gap_end {
                                    over_sprite = true;
                                }
                            }
                        }
                    }
                    xs = next;
                }
            }
            // Present to a non-coherent scan-out (the Pi 4 HVS) with a single cache clean over this
            // rectangle's span — one `DC CVAC` sweep + one `DSB`, not one per scanline. The span is a
            // contiguous byte range covering every blitted row (its interior may include undamaged
            // left/right margins of middle rows, but cleaning already-clean lines is harmless). No-op
            // on cache-coherent targets (x86, and QEMU which models no caches).
            let span_start = (d.y0 * stride + d.x0) * bpp;
            let span_end = ((y1 - 1) * stride + x1) * bpp;
            self.front.flush_range(span_start, span_end - span_start);
        }
        self.last_flush_bytes = flushed;
        #[cfg(feature = "witness")]
        if over_sprite {
            super::cursor::note_desktop_over_sprite();
        }
        #[cfg(feature = "witness")]
        super::wm::note_desktop_flush(!occ.is_empty(), false);
        // WC-I — the subtraction is exact: every span this loop copied was tested against the window
        // layer, so no background pixel landed inside a window. Nothing is owed to WC-E's restore.
        // (The `true` branch is kept live by the parallel path above, which returns its own `false`
        // only because it runs exclusively when there are no occluders at all.)
        false
    }

    /// VUG-PAR — band-parallel flush. Clips the damage set, decides a band count from the currently
    /// SCHEDULED secondary cores (`sched::core_load(cpu).tracked`, minus this core), splits the
    /// damaged scanline extent into that many DISJOINT contiguous bands, dispatches all-but-one to
    /// helper APs via `sched::spawn_joinable` (this core runs band 0 inline), then joins — the
    /// correctness barrier that guarantees every band's writes land before `flush` returns.
    ///
    /// Returns `true` when it presented the frame (>= 2 bands used); `false` to fall through to the
    /// serial path (no free AP, or the damage is too small to amortize spawn/join). `last_flush_bytes`
    /// is computed here the same way the serial path counts it, so the witness is banding-independent.
    #[cfg(all(feature = "vugpar", feature = "baremetal"))]
    fn flush_parallel(&mut self, n: usize) -> bool {
        use crate::arch::sched;

        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let blen = self.back_store.len();

        // Clip the damage set once, count the bytes the flush will copy, and find the damaged
        // scanline extent [y_lo, y_hi) that the bands will partition.
        let mut rects = [Damage { x0: 0, y0: 0, x1: 0, y1: 0 }; MAX_DAMAGE_RECTS];
        let mut nr = 0usize;
        let mut flushed: u64 = 0;
        let mut y_lo = usize::MAX;
        let mut y_hi = 0usize;
        for idx in 0..n {
            let d = self.damage.rects[idx];
            let x1 = d.x1.min(self.info.width);
            let y1 = d.y1.min(self.info.height);
            if d.x0 >= x1 || d.y0 >= y1 {
                continue;
            }
            let seg = (x1 - d.x0) * bpp;
            for y in d.y0..y1 {
                let off = (y * stride + d.x0) * bpp;
                if off + seg <= blen {
                    flushed += seg as u64;
                }
            }
            rects[nr] = Damage { x0: d.x0, y0: d.y0, x1, y1 };
            nr += 1;
            y_lo = y_lo.min(d.y0);
            y_hi = y_hi.max(y1);
        }
        if nr == 0 {
            self.last_flush_bytes = 0;
            self.last_flush_bands = 1;
            return true;
        }

        // Enumerate free helper cores: scheduled (inside `run()`) and not this core. `tracked` is the
        // honest "is this core live in the scheduler" signal, so QEMU (however many cores it releases)
        // and metal (at least core 2 free) both get a deterministic, real band count.
        // SPREAD-2 — rank the candidates by MOMENTARY LOAD (`busy_pct_recent`, ascending) before the
        // `MAX_BANDS - 1` cap bites, so a capped fan-out claims the idlest cores rather than the
        // lowest-numbered ones. On a 4-core Pi the cap equals the candidate count and this ordering is
        // a no-op on the SET (it only decides which helper draws which band); it is the general-case
        // half of the fix, and the band SIZING below is the half that moves the metal numbers.
        let self_cpu = sched::meter_current_cpu();
        let ncpu = sched::meter_cpu_count();
        let mut helpers = [0usize; MAX_BANDS - 1];
        let mut hbusy = [0u32; MAX_BANDS - 1];
        let mut nh = 0usize;
        let mut c = 0usize;
        // WEDGE-1 — ELIGIBILITY IS *FRESHNESS*, NOT `tracked`. **Hardening against a latent hazard;
        // NOT a diagnosed cause of the P66 wedge, whose mechanism is unknown.**
        //
        // The hazard is real and independent of P66. A band task is spawned PINNED to the core chosen
        // here (`spawn_joinable` with a concrete cpu sets `steal_ok: false`), so no other core may ever
        // steal it, and the join below has NO TIMEOUT — its own doc says a task that never runs leaves
        // the joiner blocked forever. Hand a band to a core that has stopped going round its dispatch
        // loop and this flusher parks permanently.
        //
        // Scope it honestly: `JoinHandle::join` blocks the TASK, not the core. The flusher's own core
        // keeps dispatching other work, so this costs a vug its render task — it does not by itself
        // stall a core, and it is nowhere near sufficient to explain three cores leaving the dispatch
        // loop at once. It is worth closing because a permanently parked render task is a real defect,
        // not because it explains the bench.
        //
        // `tracked` is the wrong gate for this question and says so in its own doc: its ~2-window
        // (~500 ms) slack exists to keep a genuinely-scheduled core off the `--` DISPLAY during a slow
        // rollover. For half a second after a core stops dispatching it still reads `true`.
        // `dispatch_fresh_cyc` is the tight bound for "may I give this core work only it can run".
        //
        // This does not weaken the parallel path: a live core folds a span every dispatch pass and can
        // never trip the bound, so the calm and loaded cases alike keep their full fan-out. It only
        // declines a placement that could not have completed.
        let fresh = sched::dispatch_fresh_cyc();
        while c < ncpu {
            let load = sched::core_load(c);
            if c != self_cpu && load.tracked && load.fold_age_cyc >= fresh {
                WEDGE1_STALE_DECLINED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            } else if c != self_cpu && load.tracked {
                let busy = load.busy_pct_recent;
                if nh < MAX_BANDS - 1 {
                    // Insertion sort into the kept set: ascending `busy`.
                    let mut i = nh;
                    while i > 0 && hbusy[i - 1] > busy {
                        helpers[i] = helpers[i - 1];
                        hbusy[i] = hbusy[i - 1];
                        i -= 1;
                    }
                    helpers[i] = c;
                    hbusy[i] = busy;
                    nh += 1;
                } else if busy < hbusy[nh - 1] {
                    // Full: displace the busiest kept candidate, then re-sink into place.
                    let mut i = nh - 1;
                    while i > 0 && hbusy[i - 1] > busy {
                        helpers[i] = helpers[i - 1];
                        hbusy[i] = hbusy[i - 1];
                        i -= 1;
                    }
                    helpers[i] = c;
                    hbusy[i] = busy;
                }
            }
            c += 1;
        }

        let total_rows = y_hi.saturating_sub(y_lo);
        if nh == 0 || total_rows < PAR_MIN_ROWS {
            // No helper, or too little work: let the serial path present it (byte-identical).
            return false;
        }

        let nbands = nh + 1;
        let common = FlushCommon {
            front: self.front,
            back_ptr: self.back_store.as_ptr() as usize,
            back_len: blen,
            rects,
            nrects: nr,
            stride,
            bpp,
        };

        // SPREAD-2 — partition [y_lo, y_hi) into `nbands` contiguous slices sized by each executing
        // core's HEADROOM (`100 - busy_pct_recent`, floored), not by an equal split. Band 0 runs on
        // `self_cpu`, band b on `helpers[b - 1]`.
        //
        // Why sizing and not placement: with `MAX_BANDS == 4` on a 4-core Pi the helper set is every
        // other core, so no re-placement can change WHICH cores work — two vugs each claim all three
        // of their peers and the doubly-claimed cores saturate (P65v2: c0=99 c1=68 c2=99 c3=63).
        // Weighting the slices is what lets a core already carrying the other vug's band take less.
        //
        // Stability: `busy_pct_recent` is a ~250 ms window and frames land far inside it, so the signal
        // low-passes the per-frame feedback rather than oscillating with it. `HEADROOM_FLOOR` keeps a
        // saturated core from being handed a degenerate empty band and bounds how far the split can
        // concentrate. When every headroom is equal the prefix sums reduce to `span * b / nbands`
        // exactly — the calm/idle boot keeps its former byte-identical partition.
        let span = y_hi - y_lo;
        let mut w = [0u32; MAX_BANDS];
        w[0] = headroom_weight(sched::core_load(self_cpu).busy_pct_recent);
        for b in 1..nbands {
            w[b] = headroom_weight(sched::core_load(helpers[b - 1]).busy_pct_recent);
        }
        let mut prefix = [0u32; MAX_BANDS + 1];
        for b in 0..nbands {
            prefix[b + 1] = prefix[b] + w[b];
        }
        let total_w = prefix[nbands] as usize; // >= nbands * HEADROOM_FLOOR > 0
        let mut jobs: [BandJob; MAX_BANDS] =
            [BandJob { common: &common, yb0: 0, yb1: 0 }; MAX_BANDS];
        for b in 0..nbands {
            jobs[b] = BandJob {
                common: &common,
                yb0: y_lo + span * prefix[b] as usize / total_w,
                yb1: y_lo + span * prefix[b + 1] as usize / total_w,
            };
        }
        spread2_note(self_cpu, &helpers[..nh], &jobs[..nbands]);

        // Dispatch bands 1..nbands to helper APs; run band 0 on this core while they work; then join.
        // `jobs`/`common` stay on this frame — the joins below keep it alive until every band returns.
        let mut handles: alloc::vec::Vec<sched::JoinHandle> = alloc::vec::Vec::with_capacity(nh);
        for b in 1..nbands {
            let arg = &jobs[b] as *const BandJob as usize;
            handles.push(sched::spawn_joinable("vugband", band_worker, arg, helpers[b - 1]));
        }
        band_run(&jobs[0]);
        for h in handles {
            h.join();
        }

        self.last_flush_bytes = flushed;
        self.last_flush_bands = nbands;
        true
    }
}

/// UVUG-2 — whether the first `[uvug2]` present witness has been emitted (first present only, no
/// per-frame spam; the mini-vug presents ~300 times).
static UVUG2_WITNESSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// UVUG-2 PRESENT SEAM impl — composite an EL1-owned ARGB8888/XRGB surface `(ptr, w, h, stride)` onto
/// the real scan-out framebuffer. This is the function registered via
/// `arch::aarch64::syscall::register_fb_present_hook`; `SYS_FB_PRESENT` calls it from the presenting
/// task's syscall context (EL1). Convention: **centered** on the panel (clamped to the top-left origin
/// when the surface exceeds the panel — off-panel pixels clip via `put_pixel`'s bounds checks).
///
/// Concurrency: this writes DIRECTLY to the front framebuffer (a `Copy` handle taken from `WRITER`),
/// not through the render task's back-buffered `Screen`. That is deliberate and sound here — there is
/// no shared/global `Screen` to borrow, and while a full-screen EL0 program owns the panel the render
/// core is parked inside `dispatch_command` (SCREEN_APP_ACTIVE) and is not flushing, so the small
/// centered blit does not race an owner. The writes are per-pixel volatile stores (`FrameBuffer` is
/// `Copy`, no aliased `&mut`), followed by a `flush_range` cache-clean over the touched rows so the
/// non-coherent Pi 4 HVS scan-out sees them — the identical present discipline `Screen::flush` uses.
/// This is the "inline blit" choice from the brief (compositor-route was rejected: the render task is
/// blocked during the app run, so routing through it would present nothing).
///
/// The surface is ARGB8888 little-endian (`FB_FORMAT_ARGB8888`): each u32 is `0xAARRGGBB`, so the low
/// 24 bits are `0xRRGGBB` — exactly the `color` convention `put_pixel` expects (it re-encodes for the
/// panel's RGB/BGR layout). The alpha byte is ignored (opaque composite).
pub fn present_surface(surf: *const u8, w: u32, h: u32, stride: u32) {
    let fb = *super::WRITER.lock();
    if !fb.is_ready() || surf.is_null() || w == 0 || h == 0 {
        return;
    }
    let info = fb.info();
    let (fw, fh) = (info.width, info.height);
    let (w, h, stride) = (w as usize, h as usize, stride as usize);

    // UVUG-7: integer nearest-neighbour upscale. A 32x32 surface blitted 1:1 is ~1 cm on a 1920-wide
    // panel — invisible. Pick the largest integer factor whose scaled surface still fits the panel,
    // capped so it occupies ~40% of the panel's shorter dimension (a comfortably-visible crystal
    // rather than a screen-filling wall). Nearest-neighbour keeps it crisp and cheap (no filtering).
    let cap_factor = (fw.min(fh) * 40 / 100 / w.max(h).max(1)).max(1);
    let fit_factor = (fw / w.max(1)).min(fh / h.max(1)).max(1);
    let scale = cap_factor.min(fit_factor).max(1);
    let (dw, dh) = (w * scale, h * scale);
    // Centered, clamped to (0,0) so an over-large surface pins to the top-left and clips.
    let x0 = fw.saturating_sub(dw) / 2;
    let y0 = fh.saturating_sub(dh) / 2;

    // WC-A: hand the blit to the compositor's compat window. The geometry above is unchanged, and a
    // compat window carries no chrome and flushes exactly the rows [y0, y0+dh) this path always
    // flushed, so the panel output is byte-for-byte what the pre-WC present produced — the shim is a
    // routing change, not a rendering change.
    super::wm::compat_present(surf as usize, w, h, stride, scale, x0, y0);

    // First present only — no per-frame witness spam.
    if !UVUG2_WITNESSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!("[uvug2] present {}x{} -> blit at ({},{})", w, h, x0, y0);
        #[cfg(feature = "witness")]
        serial_println!(
            "[uvug7] surface {}x{} scaled {}x -> {}x{} at ({},{}) on {}x{} panel",
            w, h, scale, dw, dh, x0, y0, fw, fh
        );
    }
}
