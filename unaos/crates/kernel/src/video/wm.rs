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

//! WC-A — the window table and the compositor.
//!
//! Every EL0 program renders into its OWN off-screen ARGB8888 surface and never touches the real
//! scan-out; the kernel owns the panel. This module is the seam between "a task has a surface" and
//! "pixels reach the HVS": a fixed table of at most [`MAX_WINDOWS`] windows (id, owner ASID,
//! geometry, z-order, surface pointer/stride, damage flag, short title) plus a back-to-front
//! composite pass that blits the damaged windows onto the framebuffer with a per-window integer
//! upscale and kernel-drawn chrome (a 1-px border and a title strip).
//!
//! **Chrome is kernel-drawn, always.** An app draws only inside its own surface; the border and the
//! title strip are painted by the compositor from the kernel's copy of the title. An EL0 program
//! therefore cannot forge another window's frame, and the presentation-modes law ("never fake host
//! chrome") is enforced structurally rather than by convention.
//!
//! **Composite on present, no thread.** [`present`] marks a window damaged and immediately runs the
//! composite pass from the presenting task's own context (the same discipline
//! [`super::screen::present_surface`] has always used: the render task is parked while a full-screen
//! EL0 program owns the panel, so routing through it would present nothing). There is no compositor
//! thread in this arc.
//!
//! **Non-coherent scan-out.** Each composited region is cleaned with `FrameBuffer::flush_range` over
//! the rows it touched, exactly as `Screen::flush` does, so the Pi 4 HVS sees the new pixels.
//!
//! ### Seam for WC-B (the syscall side)
//! `SYS_WIN_CREATE` / `SYS_WIN_PRESENT` / `SYS_WIN_MOVE` / `SYS_WIN_CLOSE` are thin, fail-closed
//! wrappers over [`create`], [`present`], [`move_to`] and [`close`]; per-ASID ownership is checked
//! with [`owner_of`], and task teardown calls [`close_owner`] **and** [`close_compat`]. Nothing in
//! this module reads task state or touches the syscall layer — the ASID is passed in as an opaque
//! tag.
//!
//! Two obligations WC-B cannot skip:
//! - [`create`] takes `surf_len`, the **real byte length of the mapped slot**. It must come from the
//!   mapping code, never from EL0-supplied dimensions; it is what bounds every source read the
//!   compositor performs.
//! - [`close_compat`] must be called from the EL0 teardown seam next to [`close_owner`]. The compat
//!   window has no owner ASID (the `SYS_FB_PRESENT` hook signature carries none), so `close_owner`
//!   can never reap it.
//!
//! **Untrusted geometry.** `w`, `h`, `stride`, and the [`move_to`] origin may all come from an app.
//! They are validated against the slot at create time, clamped to the panel at move time, saturating
//! everywhere in between, and every composite loop is clipped to the panel intersection BEFORE it
//! runs — `put_pixel` clips writes but would still iterate a hostile extent. The kernel builds
//! without overflow checks, so wrapping arithmetic is a real failure mode here, not a theoretical
//! one.

use spin::Mutex;

/// WC-A — the window table is fixed-size and statically allocated: the compositor runs from syscall
/// context on a non-coherent scan-out path where a heap allocation (or a growable table) would be
/// both a latency and a failure mode we do not want. Eight windows is far past what a 1920-wide panel
/// can usefully tile at a legible integer scale.
pub const MAX_WINDOWS: usize = 8;

/// WC-A — maximum stored title length in bytes. Titles are kernel-owned byte strings (ASCII, not
/// NUL-terminated); anything longer is truncated at [`create`] time, so a hostile length can never
/// reach the compositor.
pub const MAX_TITLE: usize = 16;

/// WC-A — height in panel pixels of the kernel-drawn title strip above each window's content.
pub const TITLE_H: usize = 12;

/// WC-A — width in panel pixels of the kernel-drawn window border.
pub const BORDER: usize = 1;

/// A window identifier. Ids are `1..=MAX_WINDOWS`; `0` is never a valid window and is the
/// fail-closed return for every operation that could not be satisfied.
pub type WinId = u32;

/// The reserved "no window" id.
pub const WIN_NONE: WinId = 0;

/// An immutable snapshot of one window's table row, for callers that need to report or check state
/// without holding the table lock (the syscall layer's ownership gate, witnesses, future WC-C focus
/// logic). Never a live handle — re-read it after any mutating call.
#[derive(Clone, Copy)]
pub struct WindowInfo {
    /// Window id (`1..=MAX_WINDOWS`).
    pub id: WinId,
    /// Owning address-space id, as passed to [`create`]. Opaque to this module.
    pub owner_asid: u64,
    /// Top-left of the window's CONTENT area on the panel, in panel pixels (chrome is drawn
    /// immediately above and around it).
    pub x: usize,
    pub y: usize,
    /// Surface dimensions in SOURCE pixels (before the integer upscale).
    pub w: usize,
    pub h: usize,
    /// Integer upscale factor applied when compositing (>= 1, nearest-neighbour).
    pub scale: usize,
    /// Z-order; higher composites later (in front). Ties break by id.
    pub z: u32,
    /// Whether the window has unpresented content.
    pub damaged: bool,
}

/// One row of the window table. `None`-ness is carried by `used` so the table stays a plain array.
#[derive(Clone, Copy)]
struct Window {
    used: bool,
    id: WinId,
    owner_asid: u64,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    stride: usize,
    /// EL1-visible address of the owner's ARGB8888 surface. Held as a `usize` so the table is `Send`.
    surf: usize,
    /// F1 — length in BYTES of the mapped surface slot at `surf`. The compositor never reads past
    /// `surf + surf_len`; [`create`] rejects any geometry that could. Without this the extent came
    /// from EL0 and a `w=h=10000, stride=40000` window over a 4 KiB slot would have the compositor
    /// read ~400 MB of EL1 memory and paint kernel bytes onto the panel (`put_pixel` clips WRITES,
    /// never the source READ).
    surf_len: usize,
    scale: usize,
    z: u32,
    damaged: bool,
    title: [u8; MAX_TITLE],
    title_len: usize,
    /// Compat shim marker: window created implicitly by [`super::screen::present_surface`]. Such a
    /// window is centered with the legacy scale rule and gets NO chrome, so the pre-WC UVUG present
    /// stays byte-for-byte identical on the panel.
    compat: bool,
    /// Set by [`move_to`]: the caller placed this window explicitly, so the automatic tiling in
    /// [`place`] leaves it where it is.
    pinned: bool,
}

impl Window {
    const fn empty() -> Self {
        Self {
            used: false,
            id: WIN_NONE,
            owner_asid: 0,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            stride: 0,
            surf: 0,
            surf_len: 0,
            scale: 1,
            z: 0,
            damaged: false,
            title: [0u8; MAX_TITLE],
            title_len: 0,
            compat: false,
            pinned: false,
        }
    }
}

/// The window table. One global instance ([`TABLE`]); the compositor snapshots it and releases the
/// lock before touching the framebuffer, so a present can never hold this lock across a scan-out
/// cache clean.
struct Table {
    rows: [Window; MAX_WINDOWS],
    /// Monotonic z allocator: each create/raise takes the next value, so "created later is in front".
    next_z: u32,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    rows: [Window::empty(); MAX_WINDOWS],
    next_z: 1,
});

/// Create a window owned by `owner_asid` over the caller's ARGB8888 surface at `surf` (EL1-visible
/// address, `surf_len` bytes long, `stride` bytes per row) with source dimensions `w` x `h` and a
/// short `title` (truncated to [`MAX_TITLE`]).
///
/// Geometry is chosen by the kernel: a fresh window is tiled into the next free column so multiple
/// apps are visible side-by-side, at the largest integer scale that keeps it legible and on-panel.
/// The caller may relocate it afterwards with [`move_to`].
///
/// # Surface-extent contract (F1) — WC-B MUST honour this
/// `surf_len` is the **real byte length of the mapped slot**, as the mapping code knows it — never a
/// value derived from EL0-supplied dimensions. `w`, `h` and `stride` may come straight from the
/// caller; this function rejects any combination that would read outside the slot
/// (`w * 4 > stride`, or `h * stride > surf_len`), so the compositor's source reads are bounded by
/// construction rather than by trusting the app.
///
/// Returns the new [`WinId`], or [`WIN_NONE`] when the table is full, the arguments are degenerate
/// (null surface, zero extent) or the geometry does not fit the slot — fail-closed, never a panic,
/// so the syscall wrapper maps a single error case.
pub fn create(
    owner_asid: u64,
    surf: usize,
    surf_len: usize,
    w: u32,
    h: u32,
    stride: u32,
    title: &[u8],
) -> WinId {
    create_inner(
        owner_asid,
        surf,
        surf_len,
        w as usize,
        h as usize,
        stride as usize,
        title,
        false,
    )
}

/// The compat window's id (`WIN_NONE` until the first `present_surface` call), so the shim reuses
/// one table row instead of leaking one per frame.
static COMPAT_WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(WIN_NONE);

/// Serialises the compat window's check-and-create so concurrent presents cannot each create one
/// (the loser's row would be ownerless and unreachable — an immortal window). Never held while the
/// table lock is held.
static COMPAT_CREATE: Mutex<()> = Mutex::new(());

/// WC-A — the compat path for [`super::screen::present_surface`] (`SYS_FB_PRESENT`, pre-window
/// apps). Creates the auto window on first call and thereafter updates it in place, then composites.
///
/// The caller supplies the geometry it has always computed (the legacy centered placement and
/// integer scale), and the compat row is marked `compat` so the compositor draws NO chrome and
/// flushes exactly the rows the legacy blit did — the panel output stays byte-for-byte what the
/// pre-WC UVUG run produced.
pub(super) fn compat_present(
    surf: usize,
    w: usize,
    h: usize,
    stride: usize,
    scale: usize,
    x: usize,
    y: usize,
) {
    use core::sync::atomic::Ordering;
    // F1 — the compat path's dimensions are NOT EL0-supplied: `present_surface` is reached through
    // the `SYS_FB_PRESENT` hook, which passes the kernel's own `boot::FB_SURFACE_*` constants. The
    // hook signature carries no slot length, so the tightest bound available here is the extent those
    // constants describe. That makes `surf_len` a restatement of `h * stride` rather than an
    // independent bound — it is what lets `draw_window`'s single `surf_len`-derived clamp cover both
    // paths, not a validation of this path. The real check for the compat row is `w * 4 <= stride`
    // below; the slot-length check is the one that has teeth on `create`, where the length comes from
    // the mapping code.
    let surf_len = h.saturating_mul(stride);

    // F2 — ids are slot aliases with no generation, so "is this still MY window?" cannot be answered
    // by liveness alone: after the compat row is closed and slot 0 is recycled to a real app, the row
    // is live again under a new owner and a liveness test would have the shim overwrite that app's
    // surface pointer and geometry while `owner_of` still reported the app. Test the `compat` FLAG
    // instead — the one property no recycled row can accidentally have (only `compat_present` ever
    // sets it). Chosen over widening `WinId` with a generation counter because the flag is exact for
    // this hazard and keeps the id a plain slot number for WC-B's syscall ABI.
    //
    // The check-and-create is serialised: `COMPAT_WIN` alone would be check-then-act, and two
    // `SYS_FB_PRESENT`s landing on different cores could each see no compat row and each create one.
    // The loser's row would then be an ownerless, unreferenced window that nothing can ever close —
    // re-opening F3 through a race. `COMPAT_CREATE` is taken ONLY here and never while holding the
    // table lock, so it sits strictly outside the WRITER/TABLE ordering.
    let id = {
        let _claim = COMPAT_CREATE.lock();
        let mut id = COMPAT_WIN.load(Ordering::Relaxed);
        if id == WIN_NONE || !is_compat_row(id) {
            id = create_inner(0, surf, surf_len, w, h, stride, b"", true);
            if id == WIN_NONE {
                return;
            }
            COMPAT_WIN.store(id, Ordering::Relaxed);
        }
        id
    };
    {
        let mut t = TABLE.lock();
        let r = match row_mut(&mut t, id) {
            // F2 — re-check under the lock: the row could have been closed and recycled between the
            // guard above and here.
            Some(r) if r.compat => r,
            _ => return,
        };
        // Rows-fit-slot is tautological here (see above); pixels-fit-stride is the real constraint.
        if w.saturating_mul(4) > stride {
            return;
        }
        r.surf = surf;
        r.surf_len = surf_len;
        r.w = w;
        r.h = h;
        r.stride = stride;
        r.scale = scale;
        r.x = x;
        r.y = y;
        r.damaged = true;
    }
    composite();
}

/// Whether `id` names a live row that is the compat window (F2's identity test).
fn is_compat_row(id: WinId) -> bool {
    let t = TABLE.lock();
    row(&t, id).map(|r| r.compat).unwrap_or(false)
}

/// WC-A / F3 — close the compat window, if one exists. **WC-B must call this from the EL0 teardown
/// seam** — the same place in `clear_handle_row` that calls [`close_owner`] — because the compat row
/// has no real owner ASID (`present_surface` is reached through the `SYS_FB_PRESENT` hook, whose
/// signature carries no ASID, so the shim cannot learn who is presenting) and therefore
/// [`close_owner`] can never reap it.
///
/// Without that call the compat row is immortal: after the app exits, every later composite re-blits
/// whatever now lives in the dead surface buffer, and the panel keeps showing a window whose owner is
/// gone. Returns `true` if a compat window was closed.
pub fn close_compat() -> bool {
    let id = COMPAT_WIN.swap(WIN_NONE, core::sync::atomic::Ordering::Relaxed);
    if id == WIN_NONE || !is_compat_row(id) {
        return false;
    }
    close(id)
}

/// Mark `id` damaged and composite. This is the whole present path: WC-B's `SYS_WIN_PRESENT`
/// performs its ownership check, its checksum and its focused-present accounting, then calls this.
///
/// Returns `false` if `id` names no live window.
pub fn present(id: WinId) -> bool {
    {
        let mut t = TABLE.lock();
        match row_mut(&mut t, id) {
            Some(r) => r.damaged = true,
            None => return false,
        }
    }
    composite();
    true
}

/// Move `id`'s content origin to `(x, y)` on the panel, clamped on BOTH bounds so the window (with
/// its chrome) stays on the panel. Marks it damaged; the next [`present`] or [`composite`] repaints
/// it.
///
/// F5 — the upper clamp is not cosmetic: an unclamped `usize::MAX` would feed the geometry
/// arithmetic in `outer_box`/`draw_window`, and the kernel builds without overflow checks, so the
/// additions would wrap in release rather than trap.
///
/// Returns `false` if `id` names no live window, or if the framebuffer is not ready (nothing sane to
/// clamp against).
pub fn move_to(id: WinId, x: usize, y: usize) -> bool {
    let fb = *super::WRITER.lock();
    // Guard intentionally dropped before the table lock: `place`/`composite` never hold the table
    // lock while touching `WRITER`, so keeping WRITER-then-TABLE strictly non-overlapping here is
    // what makes the two orders unable to interleave into a cycle.
    if !fb.is_ready() {
        return false;
    }
    let info = fb.info();

    let mut t = TABLE.lock();
    match row_mut(&mut t, id) {
        Some(r) => {
            // Largest origin that keeps the whole window (content + chrome) on the panel; falls back
            // to the minimum when the window is wider/taller than the panel.
            let cw = r.w.saturating_mul(r.scale);
            let ch = r.h.saturating_mul(r.scale);
            let max_x = info.width.saturating_sub(cw + BORDER).max(BORDER);
            let max_y = info
                .height
                .saturating_sub(ch + BORDER)
                .max(TITLE_H + BORDER);
            r.x = x.clamp(BORDER, max_x);
            r.y = y.clamp(TITLE_H + BORDER, max_y);
            r.pinned = true;
            r.damaged = true;
            true
        }
        None => false,
    }
}

/// Close `id`, freeing its table row. The surface itself belongs to the owner's address space and is
/// not touched here — WC-B unmaps it. Returns `false` if `id` names no live window.
pub fn close(id: WinId) -> bool {
    let vacated = {
        let mut t = TABLE.lock();
        match row_mut(&mut t, id) {
            Some(r) => {
                let b = outer_box(r);
                *r = Window::empty();
                b
            }
            None => return false,
        }
    };
    // F4 — same barrier as `close_owner`: this row's surface may be under an in-flight blit.
    let barrier = DrainBarrier::drain();
    #[cfg(feature = "witness")]
    serial_println!("[wc-a] close win={}", id);
    erase(&[vacated]);
    place(WIN_NONE);
    // Re-open the barrier before recompositing — a composite under a raised barrier is a no-op.
    drop(barrier);
    composite();
    true
}

/// Close every window owned by `owner_asid` and return how many rows were freed. Task teardown
/// (`clear_handle_row`) calls this so a dead ASID can never leave a window compositing from a
/// surface whose address space is gone.
pub fn close_owner(owner_asid: u64) -> usize {
    let mut vacated = [(0usize, 0usize, 0usize, 0usize); MAX_WINDOWS];
    let mut n = 0;
    {
        let mut t = TABLE.lock();
        for r in t.rows.iter_mut() {
            if r.used && r.owner_asid == owner_asid {
                vacated[n] = outer_box(r);
                *r = Window::empty();
                n += 1;
            }
        }
    }
    if n == 0 {
        return 0;
    }
    // F4 — the rows are gone, but a composite on another core may have snapshotted them a moment ago
    // and still be reading those surfaces. Raise the phase barrier and drain before returning: the
    // caller is about to unmap the ASID's memory, and today that would be a stale read, but under
    // WC-B's per-ASID surface mappings it becomes an EL1 abort mid-blit.
    let barrier = DrainBarrier::drain();
    #[cfg(feature = "witness")]
    serial_println!("[wc-a] close_owner asid={:#x} closed={}", owner_asid, n);
    erase(&vacated[..n]);
    place(WIN_NONE);
    // Re-open the barrier before recompositing — a composite under a raised barrier is a no-op.
    drop(barrier);
    composite();
    n
}

/// WC-C — the DESKTOP background colour, painted wherever no window is. Flat and theme-y today (the
/// crispy theme will supply real desktop data later); the point of the constant is that "no window
/// here" is a POSITIVE state the compositor paints, not the absence of paint.
///
/// Erasing to black (the WC-A behaviour) was wrong in a way only the panel showed: a closed window left
/// a black rectangle over whatever the console had drawn, and the kernel chrome's close-box left a black
/// hole in the middle of the desktop. Black is not "nothing" on a panel — it is a colour, and it read as
/// damage. Repainting the desktop colour makes a vacated box indistinguishable from panel that never had
/// a window on it, which is the invariant the compositor actually wants.
///
/// The value is the console's Moonstone (`console.rs`'s private `Console::BG`), because that is what the
/// desktop under these windows IS today. Deliberately RESTATED rather than imported: reaching into the
/// console from the compositor would make the compositor's notion of "desktop" mean "whatever the text
/// console happens to paint". The crispy theme will hand the compositor real desktop data; until then
/// this is the compositor's own theme value that happens to agree — and any drift between the two shows
/// up instantly, as a visible rectangle where a window used to be.
pub const DESKTOP_BG: u32 = 0x002D_2B55;

/// Paint the given outer boxes with the desktop background and clean them for the scan-out — the panel
/// area a closed window vacated. Windows that overlapped it are repainted by the [`composite`] that
/// follows (closing damages the whole live set through [`place`]).
fn erase(boxes: &[(usize, usize, usize, usize)]) {
    let fb = *super::WRITER.lock();
    // Guard intentionally dropped here: this function never takes the table lock, so it cannot
    // participate in a WRITER/TABLE cycle.
    if !fb.is_ready() {
        return;
    }
    let info = fb.info();
    let row_bytes = info.stride * info.bytes_per_pixel;
    for &(x, y, w, h) in boxes {
        if w == 0 || h == 0 || x >= info.width || y >= info.height {
            continue;
        }
        // F6 — clip before iterating, as in `draw_window`.
        let (w, h) = (w.min(info.width - x), h.min(info.height - y));
        fb.fill_rect(x, y, w, h, DESKTOP_BG);
        let y0 = y.min(info.height);
        let y1 = (y + h).min(info.height);
        if y1 > y0 {
            fb.flush_range(y0 * row_bytes, (y1 - y0) * row_bytes);
        }
    }
}

/// The ASID owning `id`, or `None` if `id` names no live window. The ownership gate WC-B's verbs use:
/// a task may only present/move/close a window whose owner matches its own ASID.
pub fn owner_of(id: WinId) -> Option<u64> {
    let t = TABLE.lock();
    row(&t, id).map(|r| r.owner_asid)
}

/// A snapshot of `id`'s table row, or `None` if it names no live window.
pub fn info(id: WinId) -> Option<WindowInfo> {
    let t = TABLE.lock();
    row(&t, id).map(|r| WindowInfo {
        id: r.id,
        owner_asid: r.owner_asid,
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        scale: r.scale,
        z: r.z,
        damaged: r.damaged,
    })
}

/// WC-C — the FOCUS RING: the distinct owner ASIDs of the live, non-compat windows, in window-id order,
/// written into `out` and returned by count. Deterministic (id order, not z-order) so the tab-cycle is a
/// stable rotation an operator can predict rather than a walk over a stack that reorders under them.
///
/// Compat windows are excluded deliberately: their row carries owner ASID 0 (the `SYS_FB_PRESENT` hook
/// signature has no ASID to pass), so they are not addressable as a focus target at all. An app that
/// still uses the compat path keeps whatever focus the launcher gave it.
///
/// A snapshot, never a handle — the caller re-validates before acting on it. Used by the syscall layer's
/// tab-cycle to pick the next `EL0_INPUT_ACTIVE`; nothing in this module reads input state.
pub fn focus_ring(out: &mut [u64; MAX_WINDOWS]) -> usize {
    let t = TABLE.lock();
    let mut n = 0usize;
    let mut ids = [(0u32, 0u64); MAX_WINDOWS];
    let mut m = 0usize;
    for r in t.rows.iter() {
        if r.used && !r.compat && r.owner_asid != 0 {
            ids[m] = (r.id, r.owner_asid);
            m += 1;
        }
    }
    ids[..m].sort_unstable_by_key(|&(id, _)| id);
    for &(_, asid) in ids[..m].iter() {
        if !out[..n].contains(&asid) {
            out[n] = asid;
            n += 1;
        }
    }
    n
}

/// Number of live windows.
pub fn count() -> usize {
    let t = TABLE.lock();
    t.rows.iter().filter(|r| r.used).count()
}

/// Composite every damaged window back-to-front onto the real framebuffer, clearing each window's
/// damage flag, and clean the touched rows for the non-coherent scan-out. A no-op when nothing is
/// damaged or the framebuffer is not ready.
///
/// Occlusion: repainting a damaged window would erase any window stacked ON TOP of it, so the damage
/// set is closed upwards first — every higher-z window whose outer box overlaps a damaged one is
/// repainted too (transitively). Back-to-front order then yields the correct stack.
///
/// The table lock is taken only to snapshot the rows and clear their damage flags; every framebuffer
/// write and cache clean happens after it is released, so a present never holds the window lock
/// across a scan-out flush.
pub fn composite() {
    // F4 — the drain barrier. Register this composite as in-flight WHILE STILL HOLDING the table
    // lock, so the registration is ordered against any teardown that takes the lock afterwards: a
    // `close_owner` that clears rows can then tell whether some other core snapshotted those rows
    // before the clear and is still blitting from their (about to be unmapped) surfaces.
    let (rows, mut dirty, _blit) = {
        let mut t = TABLE.lock();
        // F4 — the barrier, observed in the SAME critical section as the registration below. A
        // teardown raises it after clearing its rows, so seeing it up means there is nothing of that
        // ASID's left to draw and the teardown will recomposite when it finishes: skipping is both
        // correct and what makes the drain terminate (`BLIT_ACTIVE` can only fall while it is up).
        if DRAIN_PENDING.load(core::sync::atomic::Ordering::Acquire) != 0 {
            return;
        }
        let mut dirty = [false; MAX_WINDOWS];
        for (i, r) in t.rows.iter_mut().enumerate() {
            dirty[i] = r.used && r.damaged;
            r.damaged = false;
        }
        (t.rows, dirty, BlitGuard::enter())
    };

    // Close the damage set upwards over occlusion, to a fixed point (at most MAX_WINDOWS passes).
    for _ in 0..MAX_WINDOWS {
        let mut grew = false;
        for i in 0..MAX_WINDOWS {
            if !dirty[i] {
                continue;
            }
            let bi = outer_box(&rows[i]);
            for j in 0..MAX_WINDOWS {
                if dirty[j] || !rows[j].used || rows[j].z <= rows[i].z {
                    continue;
                }
                if boxes_overlap(bi, outer_box(&rows[j])) {
                    dirty[j] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }

    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return;
    }

    // Back-to-front: ascending z, ties by id (creation order).
    let mut order = [0usize; MAX_WINDOWS];
    for (i, slot) in order.iter_mut().enumerate() {
        *slot = i;
    }
    order.sort_unstable_by_key(|&i| (rows[i].z, rows[i].id));

    let mut drawn = 0usize;
    for &i in order.iter() {
        if !rows[i].used || !dirty[i] {
            continue;
        }
        draw_window(&fb, &rows[i]);
        // WC-D — verify this window's blit against the scan-out, once per window id, from inside the pass
        // that drew it (the only place both the source surface and the destination rows are known).
        #[cfg(feature = "witness")]
        {
            let r = &rows[i];
            if !r.compat && r.id < 32 {
                let bit = 1u32 << r.id;
                if VERIFIED.fetch_or(bit, core::sync::atomic::Ordering::Relaxed) & bit == 0 {
                    verify_window(&fb, r);
                }
            }
        }
        drawn += 1;
    }

    #[cfg(feature = "witness")]
    if drawn > 0 && !COMPOSITE_WITNESSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        let live = rows.iter().filter(|r| r.used).count();
        serial_println!("[wc-a] composite windows={} drawn={}", live, drawn);
    }
    // WC-C — the SIDE-BY-SIDE witness. The arc's claim is "two EL0 programs, two windows, both on the
    // panel at once"; a screenshot shows it to a human but proves nothing to a gate, and the per-window
    // `[wc-a] create` lines say only that rows EXISTED, never that two were composited in one pass. This
    // fires from inside the pass that actually drew them, and checksums each window's SOURCE bytes, so a
    // window that is present-but-blank (or that composited a stale/recycled surface) is distinguishable
    // from one that drew real content. FNV-1a over `surf_len` — the mapping-code length, the same bound
    // `draw_window` reads under, so the checksum can never walk past the slot.
    //
    // One-shot: this runs from present context at EL0 frame rates, and the checksum is a 64 KiB read.
    #[cfg(feature = "witness")]
    if drawn > 0 {
        let real = rows.iter().filter(|r| r.used && !r.compat).count();
        if real >= 2 && !SIDEBYSIDE_WITNESSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
            serial_println!("[wc-c] side-by-side windows={} drawn={}", real, drawn);
            for &i in order.iter() {
                let r = &rows[i];
                if !r.used || r.compat {
                    continue;
                }
                serial_println!(
                    "[wc-c] win={} asid={:#x} surf={}x{} scale={}x at ({},{}) z={} cksum={:#018x}",
                    r.id, r.owner_asid, r.w, r.h, r.scale, r.x, r.y, r.z, surface_checksum(r)
                );
            }
        }
    }
    let _ = drawn;
}

/// WC-D — the SCALED-BLIT / SCAN-OUT VERDICT. One line per window, once, from inside the composite that
/// drew it: re-derive every destination pixel of the window's content rect from the SOURCE surface and
/// compare it against what is actually in the scan-out buffer.
///
/// Why a read-back and not another checksum. `[wc-c]`'s checksum hashes the surface — the bytes the app
/// wrote — so it answers "did the app draw?" and nothing else. Every failure mode between that surface and
/// the panel is invisible to it: a stride or index error in the upscale, a `put_pixel` colour-order
/// mismatch, a clip that drops columns. Those show up as garble on a panel and as nothing at all in a
/// serial log, and the compositor is the only place that knows both sides of the comparison.
///
/// ### The two passes, and what each one is ALLOWED to conclude
/// * **`bad_cache`** — read straight after the blit, so it sees the CPU's own (possibly still dirty) lines.
///   This is the honest verdict on the BLIT: stride/pitch arithmetic, upscale indexing, colour encoding,
///   clipping. Everything WC-D actually earns about the compositor's arithmetic is earned here.
/// * **`bad_ram`** — read after a bare **`DC IVAC`** (invalidate, NO write-back) over the same rows, so
///   every line is re-fetched from the memory the HVS scans. This is the verdict on whether the pixels
///   REACHED that memory.
///
/// **The invalidate must not clean.** The first cut of this witness used `DC CIVAC`, and it was wrong in
/// the exact way that mattered: `CIVAC` writes dirty lines back before invalidating them, so if
/// `draw_window`'s trailing `flush_range` were missing or short, the witness would clean those very lines
/// to RAM and then read the repaired result — the instrument would heal the defect it claims to measure,
/// and print `bad_ram=0` for a panel that garbles. The falsifier is concrete: delete the `flush_range`
/// call, and the CIVAC form still passes. A bare `IVAC` DISCARDS un-cleaned lines instead, so a short
/// flush surfaces as stale RAM and `bad_ram > 0` — which is the whole point.
///
/// **Consequence, accepted deliberately.** `IVAC` over these rows discards anything in them that was
/// dirty-and-unclean, which in a correct build is nothing (`draw_window` just cleaned a strict superset of
/// them). In a BROKEN build it can drop pixels — and the invalidated extent is FULL-WIDTH panel scanlines,
/// not this window's columns, so the concrete exposure is `fbcon`'s deferred dirty band (`mark_rows` →
/// `flush_dirty`): console glyphs written on another core but not yet flushed lose their pixels, and the
/// redraw below restores only THIS window's rect, not theirs. That is why this function REDRAWS the window
/// afterwards: the window's rect is restored and re-flushed before returning; any co-resident console
/// residue is transient and repaired by the console's next flush. The residue is bounded, `witness`-gated, and strictly preferable to an
/// instrument that lies. See the hazard note in `engine.md` §WC-D: a witness build can therefore look
/// DIFFERENT from a default build in the presence of a flush defect, in both directions.
///
/// One-shot per window id and `witness`-gated: this walks the content rect twice (a 128x128 surface at 4x
/// is 262144 comparisons per pass) from present context — a cost nothing should pay per frame.
///
/// A guarded-out window still EMITS — a `-> SKIP` line naming the reason — so the one-shot latch is never
/// burned silently. A gate whose REQUIRE fails then has a line saying why, instead of nothing at all.
#[cfg(feature = "witness")]
fn verify_window(fb: &super::FrameBuffer, r: &Window) {
    let info = fb.info();
    if r.surf == 0 || r.scale == 0 || r.stride < 4 || r.x >= info.width || r.y >= info.height {
        serial_println!("[wc-d] verify win={} -> SKIP (degenerate row/geometry)", r.id);
        return;
    }
    // The same bounds `draw_window` blitted under — verify exactly what was drawn, never more.
    let cols = (info.width - r.x).div_ceil(r.scale).min(r.w).min(r.stride / 4);
    let rows = (info.height - r.y).div_ceil(r.scale).min(r.h).min(r.surf_len / r.stride);
    if cols == 0 || rows == 0 {
        serial_println!("[wc-d] verify win={} -> SKIP (no visible content rect)", r.id);
        return;
    }

    let pass = |fb: &super::FrameBuffer| {
        let surf = r.surf as *const u8;
        let mut checked = 0usize;
        let mut bad = 0usize;
        let mut nonzero = 0usize;
        let mut first = (0usize, 0usize, 0u32, 0u32);
        for row in 0..rows {
            let row_base = row * r.stride;
            for col in 0..cols {
                // SAFETY: identical bound to `draw_window`'s read — `row < surf_len / stride` and
                // `col < stride / 4`, so `row_base + col * 4 + 4 <= surf_len`.
                let want =
                    unsafe { core::ptr::read_unaligned(surf.add(row_base + col * 4) as *const u32) }
                        & 0x00FF_FFFF;
                for sy in 0..r.scale {
                    let dy = r.y + row * r.scale + sy;
                    for sx in 0..r.scale {
                        let dx = r.x + col * r.scale + sx;
                        // Off-panel destinations were clipped by `put_pixel`, not lost — not a defect.
                        let got = match fb.read_pixel(dx, dy) {
                            Some(g) => g,
                            None => continue,
                        };
                        checked += 1;
                        if got != 0 {
                            nonzero += 1;
                        }
                        if got != want {
                            if bad == 0 {
                                first = (dx, dy, got, want);
                            }
                            bad += 1;
                        }
                    }
                }
            }
        }
        (checked, bad, nonzero, first)
    };

    let (checked, bad_cache, nonzero, first_cache) = pass(fb);

    // Discard, never clean — see the doc comment. Bare `IVAC` is what makes `bad_ram` able to fail.
    #[cfg(target_arch = "aarch64")]
    {
        let row_bytes = info.stride * info.bytes_per_pixel;
        let y0 = r.y;
        let y1 = (r.y + rows * r.scale).min(info.height);
        if y1 > y0 {
            crate::arch::cache::invalidate_range(
                fb.base_addr() + y0 * row_bytes,
                (y1 - y0) * row_bytes,
            );
        }
    }

    let (_, bad_ram, _, first_ram) = pass(fb);
    let ok = bad_cache == 0 && bad_ram == 0;
    let first = if bad_cache > 0 { first_cache } else { first_ram };
    // `cksum` is the `[wc-c]` FNV over the SOURCE slot, carried here so a verdict is content-aware: without
    // it a blank surface blitted faithfully onto a blank rect is a PASS indistinguishable from a verified
    // crystal. `nonzero` is the same question asked of the DESTINATION.
    if ok {
        serial_println!(
            "[wc-d] verify win={} surf={}x{} scale={}x at ({},{}) panel={}x{} checked={} bad_cache=0 bad_ram=0 nonzero={} cksum={:#018x} first=none -> PASS",
            r.id, r.w, r.h, r.scale, r.x, r.y, info.width, info.height,
            checked, nonzero, surface_checksum(r)
        );
    } else {
        serial_println!(
            "[wc-d] verify win={} surf={}x{} scale={}x at ({},{}) panel={}x{} checked={} bad_cache={} bad_ram={} nonzero={} cksum={:#018x} first=({},{}) got={:#08x} want={:#08x} -> FAIL",
            r.id, r.w, r.h, r.scale, r.x, r.y, info.width, info.height,
            checked, bad_cache, bad_ram, nonzero, surface_checksum(r),
            first.0, first.1, first.2, first.3
        );
    }

    // Restore what the `IVAC` may have dropped: redraw the window and re-run its flush. In a correct build
    // this is a no-op repaint; in a broken one it is what keeps the diagnostic from being destructive.
    draw_window(fb, r);
}

/// WC-D — window ids whose [`verify_window`] verdict has already been emitted (bit `id`), so the read-back
/// runs once per window rather than once per frame. The latch is set BEFORE `verify_window` runs (the
/// `fetch_or` in `composite` claims the bit), so the invariant that keeps the gate diagnosable is carried
/// by `verify_window` itself: EVERY path through it emits a line — PASS, FAIL, or `-> SKIP` with a reason.
/// A future early return added without a line would burn the latch silently and leave the spec's REQUIRE
/// failing with nothing to explain why.
#[cfg(feature = "witness")]
static VERIFIED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// WC-C — FNV-1a 64 over a window's mapped surface slot, for the side-by-side witness. Bounded by
/// `surf_len` (the length the MAPPING code supplied), so it shares `draw_window`'s F1 read bound; a null
/// or zero-length surface hashes to the FNV offset basis, which is a value no drawn surface produces.
#[cfg(feature = "witness")]
fn surface_checksum(r: &Window) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    if r.surf == 0 {
        return h;
    }
    let p = r.surf as *const u8;
    let mut i = 0usize;
    while i < r.surf_len {
        // SAFETY: `i < surf_len`, and `surf_len` is the real byte length of the mapped slot.
        h ^= unsafe { core::ptr::read_volatile(p.add(i)) } as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// WC-C — whether the one-shot side-by-side witness has fired.
#[cfg(feature = "witness")]
static SIDEBYSIDE_WITNESSED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// F4 — number of composites that have snapshotted the table and may still be blitting from the
/// surfaces they snapshotted. Teardown drains this to zero before it returns.
static BLIT_ACTIVE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// F4 — RAII registration for an in-flight composite. Constructed under the table lock (so it is
/// ordered against teardown's own lock acquisition) and dropped when the blit is done, on every exit
/// path including the early `!fb.is_ready()` return.
struct BlitGuard;

impl BlitGuard {
    fn enter() -> Self {
        BLIT_ACTIVE.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        BlitGuard
    }
}

impl Drop for BlitGuard {
    fn drop(&mut self) {
        BLIT_ACTIVE.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
}

/// F4 — number of teardowns currently draining. Non-zero closes the barrier: a composite that sees
/// it (under the table lock, where teardown also raises it) does not register and does not blit.
static DRAIN_PENDING: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// F4 — a teardown's phase barrier, RAII so the barrier always re-opens. Raised by
/// [`close`]/[`close_owner`] and dropped once the drain has completed.
struct DrainBarrier;

impl DrainBarrier {
    /// Raise the barrier and wait out the composites that snapshotted before it went up.
    ///
    /// The caller must already have cleared its rows and released the table lock. Any composite that
    /// takes the lock from here on observes `DRAIN_PENDING != 0` and skips entirely (it would have
    /// nothing of the caller's to draw anyway — the rows are gone, and teardown recomposites when it
    /// is done). So `BLIT_ACTIVE` can only fall.
    ///
    /// **Termination.** This is a phase barrier, not a "wait for idle" loop. The earlier form spun
    /// on `BLIT_ACTIVE == 0` with nothing stopping new composites from re-raising it, so a stream of
    /// presents could keep it above zero forever — and the teardown path (`sched::exit` →
    /// `clear_handle_row` → `close_owner`) spins IRQ-MASKED and unpreemptible on its core, so that
    /// livelock would have been a dead core rather than a slow one. With the barrier up the wait set
    /// is fixed at entry, finite, and every member is running a bounded panel-clipped blit.
    fn drain() -> Self {
        use core::sync::atomic::Ordering;
        DRAIN_PENDING.fetch_add(1, Ordering::AcqRel);
        // Ordered against the composite registration by the table lock: a composite either took the
        // lock BEFORE the clearing critical section (so it registered, and is counted here) or AFTER
        // (so it sees the raised barrier and never registers).
        while BLIT_ACTIVE.load(Ordering::Acquire) != 0 {
            core::hint::spin_loop();
        }
        DrainBarrier
    }
}

impl Drop for DrainBarrier {
    fn drop(&mut self) {
        DRAIN_PENDING.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
}

/// WC-A — whether the first `[wc-a] composite` witness has been emitted (first composite only; a
/// mini-vug run presents ~300 frames and per-frame witness spam would drown the serial log).
#[cfg(feature = "witness")]
static COMPOSITE_WITNESSED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// WC-A — kernel chrome colours (0x00RRGGBB, the `put_pixel` convention; the panel's RGB/BGR layout
/// is applied by `FrameBuffer`). Deliberately flat and un-host-like: this is UnaOS chrome, not an
/// imitation of anyone's title bar.
const CHROME_BORDER: u32 = 0x003A_3A46;
const CHROME_TITLE_BG: u32 = 0x001E_1E28;
const CHROME_TITLE_FG: u32 = 0x00C8_C8D8;

/// The outer box `(x, y, w, h)` a window occupies on the panel, chrome included. A compat window
/// (the `present_surface` shim) has no chrome, so its outer box is exactly its content.
/// F5 — every product and sum here saturates. The kernel builds with overflow checks off, so a
/// wrapping `w * scale` would silently produce a SMALL box that then fails to damage the region it
/// actually paints; saturation degrades to "absurdly large box", which the panel clip then bounds.
fn outer_box(r: &Window) -> (usize, usize, usize, usize) {
    let cw = r.w.saturating_mul(r.scale);
    let ch = r.h.saturating_mul(r.scale);
    if r.compat {
        (r.x, r.y, cw, ch)
    } else {
        (
            r.x.saturating_sub(BORDER),
            r.y.saturating_sub(TITLE_H + BORDER),
            cw.saturating_add(2 * BORDER),
            ch.saturating_add(TITLE_H + 2 * BORDER),
        )
    }
}

fn boxes_overlap(a: (usize, usize, usize, usize), b: (usize, usize, usize, usize)) -> bool {
    a.0 < b.0.saturating_add(b.2)
        && b.0 < a.0.saturating_add(a.2)
        && a.1 < b.1.saturating_add(b.3)
        && b.1 < a.1.saturating_add(a.3)
}

/// Paint one window: chrome (border frame + title strip + title text), then the surface content
/// nearest-neighbour upscaled by `scale`, then one cache clean over the rows it touched so the
/// non-coherent Pi 4 HVS scan-out sees the new pixels. All writes are `put_pixel`/`fill_rect` on a
/// `Copy` `FrameBuffer` handle (volatile stores, no aliased `&mut`), the same discipline
/// `Screen::flush` and the legacy `present_surface` use.
fn draw_window(fb: &super::FrameBuffer, r: &Window) {
    // `stride`/`scale` are divisors below and `surf_len` bounds the reads, so all four are checked
    // here rather than trusted from the row.
    if r.surf == 0 || r.w == 0 || r.h == 0 || r.scale == 0 || r.stride < 4 || r.surf_len == 0 {
        return;
    }
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    let (bx, by, bw, bh) = outer_box(r);
    if bx >= pw || by >= ph {
        return;
    }
    // F6 — clip the outer box to the panel BEFORE any loop runs over it. `put_pixel`/`fill_rect`
    // clip per pixel, which keeps the writes safe but still ITERATES the full extent: a window
    // claiming 10000x10000 would spin ~1e8 clipped pokes per present, from syscall context.
    let bw = bw.min(pw - bx);
    let bh = bh.min(ph - by);

    if !r.compat {
        // Frame: fill the whole outer box in the border colour, then lay the title strip and the
        // content over it. The only pixels that survive are the 1-px frame itself.
        fb.fill_rect(bx, by, bw, bh, CHROME_BORDER);
        fb.fill_rect(bx + BORDER, by + BORDER, bw.saturating_sub(2 * BORDER), TITLE_H, CHROME_TITLE_BG);
        draw_title(fb, r, bx + BORDER + 2, by + BORDER + 2, bw.saturating_sub(2 * BORDER + 4));
    }

    if r.x >= pw || r.y >= ph {
        return;
    }
    // F6 — how many source rows/columns can actually land on the panel: each source pixel occupies
    // `scale` destination pixels, so `ceil(remaining_panel / scale)` source units are visible.
    let vis_cols = (pw - r.x).div_ceil(r.scale).min(r.w);
    let vis_rows = (ph - r.y).div_ceil(r.scale).min(r.h);
    // F1 — and never read outside the mapped slot, whatever the row says. `create` already rejects
    // geometry that would (and `compat_present` re-checks), so this is the belt to that braces: the
    // read bound is derived from `surf_len`, the length the MAPPING code supplied, not from the
    // app's dimensions.
    let cols = vis_cols.min(r.stride / 4);
    let rows = vis_rows.min(r.surf_len / r.stride);

    let surf = r.surf as *const u8;
    for row in 0..rows {
        let row_base = row * r.stride;
        for col in 0..cols {
            // Unaligned-safe read of the ARGB8888 pixel; low 24 bits are RRGGBB (alpha ignored —
            // this arc composites opaquely). In bounds by construction: `row < surf_len / stride`
            // and `col < stride / 4`, so `row_base + col * 4 + 4 <= surf_len`.
            let px = unsafe { core::ptr::read_unaligned(surf.add(row_base + col * 4) as *const u32) }
                & 0x00FF_FFFF;
            for sy in 0..r.scale {
                let dy = r.y + row * r.scale + sy;
                for sx in 0..r.scale {
                    fb.put_pixel(r.x + col * r.scale + sx, dy, px);
                }
            }
        }
    }

    // Clean the touched rows (superset: whole scanlines of the outer box) for the non-coherent
    // scan-out — one `DC CVAC` sweep per window, not one per scanline. No-op on coherent targets.
    let row_bytes = info.stride * info.bytes_per_pixel;
    let y0 = by.min(info.height);
    let y1 = (by + bh).min(info.height);
    if y1 > y0 {
        fb.flush_range(y0 * row_bytes, (y1 - y0) * row_bytes);
    }
}

/// Draw the kernel's copy of the window title into the title strip, 8x8 glyphs, clipped to `max_w`
/// pixels. Non-printable bytes render as a space, so a hostile title can only ever paint blanks.
fn draw_title(fb: &super::FrameBuffer, r: &Window, x: usize, y: usize, max_w: usize) {
    let cols = max_w / 8;
    for (i, &b) in r.title[..r.title_len].iter().enumerate() {
        if i >= cols {
            break;
        }
        let ch = if (0x20..0x7f).contains(&b) { b } else { b' ' };
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch as usize];
        for (ry, byte) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if byte & (1 << rx) != 0 {
                    fb.put_pixel(x + i * 8 + rx, y + ry, CHROME_TITLE_FG);
                }
            }
        }
    }
}

// ---- internals -------------------------------------------------------------------------------

fn row(t: &Table, id: WinId) -> Option<&Window> {
    t.rows.iter().find(|r| r.used && r.id == id)
}

fn row_mut(t: &mut Table, id: WinId) -> Option<&mut Window> {
    t.rows.iter_mut().find(|r| r.used && r.id == id)
}

/// Shared body of [`create`] and the compat shim's implicit window.
fn create_inner(
    owner_asid: u64,
    surf: usize,
    surf_len: usize,
    w: usize,
    h: usize,
    stride: usize,
    title: &[u8],
    compat: bool,
) -> WinId {
    if surf == 0 || w == 0 || h == 0 || stride == 0 || surf_len == 0 {
        return WIN_NONE;
    }
    // F1 — the surface-extent contract. A row must fit its stride and the rows must fit the slot;
    // saturating arithmetic so a hostile `h`/`stride` overflows into a rejection, not a wrap (the
    // kernel builds with overflow checks off, so `h * stride` alone could wrap to something small).
    if w.saturating_mul(4) > stride || h.saturating_mul(stride) > surf_len {
        return WIN_NONE;
    }
    let mut t = TABLE.lock();
    let slot = match t.rows.iter().position(|r| !r.used) {
        Some(s) => s,
        None => return WIN_NONE,
    };
    let z = t.next_z;
    t.next_z = t.next_z.wrapping_add(1).max(1);
    let id = (slot + 1) as WinId;
    let mut row = Window::empty();
    row.used = true;
    row.id = id;
    row.owner_asid = owner_asid;
    row.w = w;
    row.h = h;
    row.stride = stride;
    row.surf = surf;
    row.surf_len = surf_len;
    row.z = z;
    row.damaged = true;
    row.compat = compat;
    row.title_len = title.len().min(MAX_TITLE);
    row.title[..row.title_len].copy_from_slice(&title[..row.title_len]);
    t.rows[slot] = row;
    drop(t);
    // WC-D: ids are recycled slot aliases, so a fresh window in a used slot is a DIFFERENT window and
    // deserves its own verdict — clear the one-shot latch here rather than at close, which is the point
    // where the id demonstrably names something new.
    #[cfg(feature = "witness")]
    if id < 32 {
        VERIFIED.fetch_and(!(1u32 << id), core::sync::atomic::Ordering::Relaxed);
    }
    place(id);
    #[cfg(feature = "witness")]
    if !compat {
        if let Some(i) = info(id) {
            serial_println!(
                "[wc-a] create win={} asid={:#x} surf={}x{} stride={} scale={}x at ({},{}) z={}",
                i.id, i.owner_asid, i.w, i.h, stride, i.scale, i.x, i.y, i.z
            );
        }
    }
    id
}

/// Choose `id`'s scale and on-panel origin. WC-A2 tiles windows left-to-right; the compat window is
/// placed by the legacy centering rule at composite time and is skipped here.
/// WC-A — gap in panel pixels between tiled windows (and from the panel edge).
const GAP: usize = 8;

/// Lay out every non-compat, non-pinned window: pick each one's integer scale, then pack the outer
/// boxes left-to-right in id order, wrapping to a new row when the next box would run off the panel.
/// Called whenever the window set changes, so the tiling stays deterministic (it depends only on the
/// live set, not on the order of creates and closes). A window the caller has explicitly [`move_to`]d
/// is pinned and keeps its position.
///
/// Scale rule: the largest integer factor whose scaled surface fits half the panel width and half its
/// height — big enough that a 32x32 surface is legible on a 1920-wide panel, small enough that two
/// windows sit side-by-side. Never 0.
fn place(_created: WinId) {
    // Read the panel geometry BEFORE taking the table lock: `composite` takes the table lock and
    // releases it before touching `WRITER`, so no path ever holds both — no lock-order inversion.
    // The WRITER guard is intentionally dropped at the end of this statement (`FrameBuffer` is
    // `Copy`); the table lock below is therefore never nested inside it.
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return;
    }
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);

    let mut t = TABLE.lock();
    let mut cx = GAP;
    let mut cy = GAP + TITLE_H + BORDER;
    let mut row_h = 0usize;
    for i in 0..MAX_WINDOWS {
        let r = &t.rows[i];
        if !r.used || r.compat || r.pinned {
            continue;
        }
        let (w, h) = (r.w.max(1), r.h.max(1));
        let scale = ((pw / 2 / w).min(ph / 2 / h)).max(1);
        // F5 — saturating throughout; `w`/`h` come from the caller via `create`.
        let bw = w.saturating_mul(scale).saturating_add(2 * BORDER);
        let bh = h
            .saturating_mul(scale)
            .saturating_add(TITLE_H + 2 * BORDER);
        if cx.saturating_add(bw) > pw && cx > GAP {
            cx = GAP;
            cy = cy.saturating_add(row_h).saturating_add(GAP);
            row_h = 0;
        }
        let r = &mut t.rows[i];
        r.scale = scale;
        r.x = cx + BORDER;
        r.y = cy;
        r.damaged = true;
        cx = cx.saturating_add(bw).saturating_add(GAP);
        row_h = row_h.max(bh);
    }
}
