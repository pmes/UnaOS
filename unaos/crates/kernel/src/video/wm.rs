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
    /// FOCUS-VIS — has the OWNER ever presented this window? False between [`create`] and the first
    /// [`present`], when the surface still holds whatever the mapping code left there (zeros).
    ///
    /// Only WC-D reads it, and it matters because FOCUS-VIS made `create` composite. WC-D's read-back
    /// verdict is one-shot per window id, so without this gate the latch would be claimed by that
    /// create-time composite and would verify a BLANK surface — a vacuous `-> PASS` that satisfies the
    /// spec's REQUIRE while the app's real content is never checked at all. The verdict waits for
    /// content the app actually put there.
    presented: bool,
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
            presented: false,
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

/// FOCUS-VIS — **the SHELL's position in the z-order.** The desktop/console is a member of the stack the
/// TAB ring already cycles through, not a backdrop the stack is painted onto.
///
/// Until FOCUS-VIS the shell had no z at all, and the consequence was the two halves of the P59 bench
/// report. Focus provably cycled (`[wc-c] focus tab-cycle`) and the panel never changed, because focus
/// was a pure *input-routing* fact: nothing raised the newly focused window, so a window covered by
/// another stayed covered. And when focus reached the shell slot the windows kept compositing over the
/// console, so the prompt and its output were unreadable — the operator could type but not read.
///
/// One value fixes both, because both are the same missing fact. `SHELL_Z` is allocated out of the SAME
/// monotonic `next_z` counter every window raise uses, so "the shell is in front of window W" and
/// "window W is in front of the shell" are the ordinary z comparison, evaluated the ordinary way:
///  * a window with `z > SHELL_Z` is ABOVE the shell and composites normally;
///  * a window with `z < SHELL_Z` is BELOW it and is not drawn at all — the console owns those pixels.
///
/// `0` is the initial value and means "the shell is at the very bottom", which is exactly the pre-
/// FOCUS-VIS behaviour: every window (z >= 1) is above it. Nothing changes until a focus change
/// actually happens, so a boot that never TABs — every QEMU gate run, since raspi4b has no HID —
/// composites byte-identically to before.
///
/// Read outside the table lock deliberately: it is a single monotonic value with no invariant tying it
/// to any row, and the composite pass snapshots it once per pass so the whole pass judges every window
/// against one shell position.
static SHELL_Z: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// FOCUS-HL — the ASID that currently holds focus, or `0` for the SHELL. The compositor draws the
/// chrome of a window whose `owner_asid` matches this in the highlight colours, and every other window
/// in the resting colours; shell focus (`0`) therefore highlights nothing, which is the honest reading
/// — no app has the keyboard.
///
/// Set only by [`focus_changed`], and read only by the composite pass, which snapshots it once so a
/// single pass judges every window against one focus owner. `0` is also the initial value, so a boot
/// that never changes focus draws exactly the resting chrome it always did.
static FOCUS_ASID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// FOCUS-HL — the ASID that currently holds focus (`0` = the shell). See [`FOCUS_ASID`].
pub fn focus_asid() -> u64 {
    FOCUS_ASID.load(core::sync::atomic::Ordering::Acquire)
}

/// FOCUS-VIS — the shell's current z. See [`SHELL_Z`].
pub fn shell_z() -> u32 {
    SHELL_Z.load(core::sync::atomic::Ordering::Acquire)
}

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
        r.presented = true;
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
    // WC-G — the surface as the OWNER declared it finished. Taken here and nowhere else: this is the
    // one moment the owner is provably not writing (it is parked inside `SYS_WIN_PRESENT`), so it is
    // the only honest baseline for the `app` leg. The identity is captured under the table lock and
    // the checksum taken after it drops — a 64 KiB read is not something to hold the window table
    // across, and the surface cannot be unmapped underneath it while the owner is in the syscall.
    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    let mut probe: Option<(usize, usize)> = None;
    {
        let mut t = TABLE.lock();
        match row_mut(&mut t, id) {
            Some(r) => {
                r.damaged = true;
                r.presented = true;
                #[cfg(all(target_arch = "aarch64", feature = "witness"))]
                if !r.compat {
                    probe = Some((r.surf, r.surf_len));
                }
            }
            None => return false,
        }
    }
    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    if let Some((surf, surf_len)) = probe {
        super::wcg::on_present(id, surf, surf_len);
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

    let vacated = {
        let mut t = TABLE.lock();
        match row_mut(&mut t, id) {
            Some(r) => {
                let before = outer_box(r);
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
                if outer_box(r) == before { None } else { Some(before) }
            }
            None => return false,
        }
    };
    // FOCUS-VIS — a move VACATES its old box, and nothing was repainting it. The compositor draws
    // windows and never the desktop, so before this the window's previous position kept its last frame
    // on the panel forever: an app that moved its window left a full copy of itself behind, and a
    // kernel-drawn title strip and border to go with it. Same treatment `close` gives a vacated box —
    // desktop colour, then re-damage whatever the erase reached so the surviving windows repaint over
    // it — because it is the same event: those pixels stopped belonging to this window.
    if let Some(b) = vacated {
        // F4 — the same phase barrier `close`/`close_owner` raise, and for the same reason. A composite
        // on another core may have snapshotted this row at its OLD geometry a moment ago and still be
        // blitting it; without the barrier that in-flight blit lands AFTER the erase below and paints
        // the ghost straight back. The row is live (unlike the teardown paths) so nothing is being
        // unmapped and the stale frame would self-heal at WC-E's next flush — but "self-heals within a
        // frame" is exactly the standard the ghost fails, and the barrier is the mechanism this module
        // already has for "a snapshot of this row is in flight and its pixels are no longer wanted".
        //
        // Raised AFTER the table lock is released, as `drain`'s contract requires: a composite that
        // takes the lock from here on sees the barrier and skips, so `BLIT_ACTIVE` can only fall.
        let barrier = DrainBarrier::drain();
        erase(&[b]);
        damage_intersecting(b.0, b.1, b.2, b.3);
        // Re-open before recompositing — a composite under a raised barrier is a no-op.
        drop(barrier);
        composite();
    }
    true
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
    // WC-J — reclaim the closed window's own box AND every box the re-tile takes from a survivor.
    // `erase` above only covers the first; without the second, closing one of several windows leaves a
    // full copy of each survivor standing at its previous tile for the rest of the boot.
    let (nv, moved) = place(WIN_NONE);
    reclaim(&[vacated]);
    reclaim(&moved[..nv]);
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
    // WC-J — the EXIT-TEARDOWN twin of `close`'s reclaim, and the path P61 actually took: a
    // backgrounded app reaches its exit with a window open, `clear_handle_row` lands here, and every
    // OTHER app's window is re-tiled by the `place` below. Same two sets, same treatment.
    let (nv, moved) = place(WIN_NONE);
    reclaim(&vacated[..n]);
    reclaim(&moved[..nv]);
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
///
/// ### WC-K — why this fill is staged too
///
/// WC-H convicted the *shape* of a write, not the identity of its writer: per-pixel `put_pixel` into
/// the live front framebuffer, unsynchronised against the beam, is structurally overtaken by the
/// scan-out and latches part-old/part-new. `fill_rect` is exactly that shape — `w * h`
/// bounds-checked pokes into memory the HVS is scanning right now — and until this arc `erase` was
/// the last writer in the window lifecycle still doing it. WC-I's standing note named the debt
/// ("`erase` is still unstaged and still fills the desktop directly"); WC-J made it heavier by
/// calling `erase` on three more paths through [`reclaim`], so every close and every re-tile ran a
/// direct fill over boxes as large as a whole tile.
///
/// [`stage_fill`] gives it the WC-H discipline, and deliberately not a third one: compose in cached
/// RAM, present as contiguous full-width row copies out of the same [`STAGE`] buffer, under the same
/// cap, the same `try_lock`, and the same four fall-back declines. The fallback direct fill remains
/// as the last resort, and — per WC-H's `stage_decline` precedent — a decline is a SAMPLE that says
/// so on the wire, because a silent fallback here is precisely the tearing regime the arc removes.
///
/// ### Cursor coherence
///
/// Unchanged, and unchanged on purpose. The `undraw()` below takes the sprite off the panel before
/// the FIRST byte of any fill lands (staged or direct — staging moves where the composing writes go,
/// never when the panel writes happen relative to this bracket), and the `composite()` that every
/// `erase` caller runs next puts it back. The staged fill does NOT take CURSOR-3's overlay: that
/// path exists for a window whose staged box wholly contains the sprite and whose compositor pass
/// handed `draw_window` a `cursor::Plan`. `erase` has no plan — it is not a compositor pass, it
/// holds no claim on the sprite's state machine, and inventing one here would mean a second,
/// unsynchronised writer of the save-under. So the overlay decision this path takes is the same one
/// `stage_window` takes when `cur` is `None`: compose no sprite, and leave the repaint to the
/// following composite. That is CURSOR-3's own fallback, not a new rule.
fn erase(boxes: &[(usize, usize, usize, usize)]) {
    // CURSOR-1: take the sprite off the panel before repainting desktop under it. Without this the
    // fills below would overwrite the sprite, and the save-under would later restore pre-erase
    // pixels over freshly-painted desktop — a stale patch the following composite would not repaint
    // (composite repaints windows, not the desktop). The `composite()` that every erase caller runs
    // next puts the sprite back.
    super::cursor::undraw();
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
        // WC-K — staged first; the direct fill is the fallback, and `stage_fill` has already said on
        // the wire which one this was.
        if !stage_fill(&fb, x, y, w, h, DESKTOP_BG) {
            fb.fill_rect(x, y, w, h, DESKTOP_BG);
        }
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

/// FOCUS-VIS — **make a focus change VISIBLE.** The one seam the focus owner calls after
/// `el0_input_set_active`; `asid == 0` means the SHELL slot of the ring.
///
/// ### Why this exists (P59, bench)
/// WC-C shipped the tab-cycle as an input-routing change and nothing else: `[wc-c] focus tab-cycle`
/// fired on every press, and the panel did not move. Two windows up, the covered one stayed covered
/// after being focused, and the shell stayed buried under both. Focus that cannot be SEEN is not focus
/// — the operator has no way to know where their keystrokes are going, which is the whole content of
/// the concept.
///
/// ### What it does
/// * **A focused window is raised.** Every live, non-compat window owned by `asid` takes a fresh z off
///   the same allocator `create` uses, so it lands above every other window *and* above the shell. All
///   of the owner's windows are raised, not just one: the focus ring is keyed by ASID (an app may own
///   several windows and they focus together), so raising a subset would leave an app half in front.
/// * **The shell is raised the same way.** `asid == 0` gives [`SHELL_Z`] the fresh z, which puts every
///   existing window BELOW the shell; those windows stop compositing, their boxes are erased to the
///   desktop colour immediately, and the desktop is asked for a whole-panel present so the console's
///   text comes back over the erase. That is the "TAB to the shell, then read your command's output"
///   case, and it is a z-order fact rather than a special case.
/// * **Both ends repaint.** The raise marks the affected windows damaged and composites, so the change
///   is on the panel before this returns rather than at the focused app's next present — which, for a
///   window whose app is idle or blocked in a read, might be never.
///
/// Ordering note: the erase and the composite both bracket the system cursor (`erase` undraws, and
/// `composite` undraws-then-repaints), so the sprite is off the panel while these pixels move and back
/// on top of them afterwards. The cursor stays top-most across a focus change for the same reason it
/// does across any other composite.
///
/// Cheap and idempotent: focusing an ASID that owns no window still raises nothing and composites only
/// what was already damaged. Safe to call on every focus change, including no-op ones.
pub fn focus_changed(asid: u64) {
    use core::sync::atomic::Ordering;
    let mut raised = 0usize;
    let mut first_id = WIN_NONE;
    let mut newz = 0u32;
    // Boxes of windows this call pushed BELOW the shell — the pixels the console is about to own.
    let mut hidden = [(0usize, 0usize, 0usize, 0usize); MAX_WINDOWS];
    let mut nhidden = 0usize;

    // FOCUS-HL: take the focus owner BEFORE the table lock, so the composite at the end of this call
    // already draws the new highlight. The window that is LOSING focus must be repainted too — the raise
    // below only damages the windows it raises, and the old holder is not among them when focus moves to
    // a different ASID (or to the shell, which raises nothing at all). Without this its chrome would keep
    // the highlight colours until something else happened to damage it.
    let prev = FOCUS_ASID.swap(asid, Ordering::Release);

    {
        let mut t = TABLE.lock();
        if prev != asid && prev != 0 {
            for r in t.rows.iter_mut() {
                if r.used && !r.compat && r.owner_asid == prev {
                    r.damaged = true;
                }
            }
        }
        if asid == 0 {
            // The SHELL takes the top of the stack. Every window is now below it; collect the boxes so
            // the panel stops showing them at once rather than at the desktop's next flush.
            let z = t.next_z;
            t.next_z = t.next_z.wrapping_add(1).max(1);
            SHELL_Z.store(z, Ordering::Release);
            newz = z;
            for r in t.rows.iter_mut() {
                if r.used && r.z < z {
                    hidden[nhidden] = outer_box(r);
                    nhidden += 1;
                    // Damaged so that a later raise repaints from the source surface rather than
                    // trusting whatever survived on the panel.
                    r.damaged = true;
                }
            }
        } else {
            for i in 0..MAX_WINDOWS {
                if !t.rows[i].used || t.rows[i].compat || t.rows[i].owner_asid != asid {
                    continue;
                }
                let z = t.next_z;
                t.next_z = t.next_z.wrapping_add(1).max(1);
                t.rows[i].z = z;
                t.rows[i].damaged = true;
                if first_id == WIN_NONE {
                    first_id = t.rows[i].id;
                }
                newz = z;
                raised += 1;
            }
        }
    }

    if nhidden > 0 {
        // Desktop colour under the vacated boxes NOW (visible immediately), console TEXT over it at the
        // desktop's next present. Splitting it that way is what keeps the response instant without this
        // path needing a `&mut Screen` it has no right to.
        erase(&hidden[..nhidden]);
    }
    if asid == 0 {
        super::screen::request_full_present();
    }
    #[cfg(feature = "witness")]
    if asid == 0 {
        serial_println!("[wc-fv] focus shell z={} hidden={}", newz, nhidden);
    } else {
        serial_println!(
            "[wc-fv] focus raise asid={:#x} windows={} top_win={} z={} shell_z={}",
            asid, raised, first_id, newz, shell_z()
        );
    }
    let _ = (raised, first_id, newz);
    composite();
}

/// FOCUS-VIS — whether `r` composites at all, i.e. whether it sits ABOVE the shell in the one z-order
/// windows and the shell share. `shell` is the pass's single snapshot of [`SHELL_Z`].
///
/// Compat rows are exempt: a compat window IS the full-screen present path, it carries owner ASID 0 and
/// is not addressable as a focus target at all (see [`focus_ring`]), so it can never be raised back
/// above a shell that overtook it — hiding it would strand a full-screen app's output permanently.
fn above_shell(r: &Window, shell: u32) -> bool {
    r.compat || r.z > shell
}

/// WC-E — restore the whole window layer over a background that was just repainted underneath it.
///
/// The desktop (`video::screen::Screen`, driven by the VUG render task) presents by copying its back
/// buffer's damaged rectangles straight into the scan-out framebuffer. That back buffer contains no
/// windows — the compositor writes windows to the framebuffer directly, from the presenting task's
/// syscall context — so every desktop present erases the window pixels inside the rectangles it
/// copied. `Screen::flush` calls this immediately afterwards to put them back; see the layering note
/// there for the panel symptom this ordering removes.
///
/// Marks the affected windows damaged rather than intersecting against the flushed rectangles: the
/// desktop's damage unions on the Pi routinely span most of the panel, so the intersection test would
/// nearly always answer "yes" while costing a rect-set walk to say so. Repainting is idempotent, and
/// [`composite`] already closes the damage set upwards over occlusion, so the restored stack is
/// correct back-to-front.
///
/// **COMPAT windows are excluded, and that exclusion is what makes this affordable.** A compat row is
/// the full-screen present path (`screen::present_surface`), and while a full-screen EL0 program owns
/// the panel the render task is parked inside `dispatch_command` and is not flushing at all — there is
/// no second writer to order against, so a repaint there would be pure cost with nothing to fix. It is
/// not a small cost either: the first cut of this function repainted compat rows too, and re-blitting
/// UVUG's 32x32 surface at 15x on every frame was enough to push its 300-frame run past the
/// `EXEC-UVUG` deadline and fail the gate. The collision this function exists for is specifically the
/// WC-C case — a *windowed* app drawn alongside a live desktop.
///
/// Cheap when there is nothing to do: one table-lock acquisition, no live real window, return without
/// touching the framebuffer. That is the per-frame cost on an ordinary windowless desktop.
pub fn repaint() {
    // BGRUN-1 COMPOSITION FIX (WC-E lens should-fix 2): the compat exclusion's premise — "compat
    // implies the render task is parked, so there is no second writer" — was true when the ONLY way
    // to run a compat (full-screen) EL0 app was the blocking foreground `run`. `bg` breaks it: a
    // background compat app presents directly to the scan-out while the desktop keeps flushing —
    // exactly the two-writer collision this function exists to order. The exclusion key is therefore
    // the FOCUSED compat surface, not the compat flag: the foreground case (incl. the boot-time
    // EXEC-UVUG witness, whose 300-frame deadline the first cut blew) always holds input focus
    // (`run_user_image` sets it before its wait loop), while a bg compat app can never acquire it
    // (the TAB ring walks windows only). A compat row cannot be keyed by OWNER — `compat_present`
    // creates it with owner_asid 0 (the SYS_FB_PRESENT hook carries none) — so the key is coarser:
    // repaint compat rows only while NO EL0 program holds input focus (`focused == 0`, the
    // bg-app-at-the-prompt state). Every foreground run — `run` verb and the boot witnesses alike —
    // sets focus before its wait loop, so the EXEC-UVUG deadline case stays excluded. Residual,
    // stated: a bg compat app still shimmers while the operator is TABbed into some OTHER app
    // (focused != 0 excludes all compat rows); cosmetic, bounded by TABbing back to the shell.
    // Focus lives in the baremetal EL0 input router (syscall.rs is baremetal-gated); elsewhere 0
    // means every compat row repaints, which is vacuous there (compat_present is unreachable).
    #[cfg(feature = "baremetal")]
    let focused = crate::arch::syscall::el0_input_active();
    #[cfg(not(feature = "baremetal"))]
    let focused: u64 = 0;
    {
        let mut t = TABLE.lock();
        let repaintable = |r: &Window| r.used && (!r.compat || focused == 0);
        if !t.rows.iter().any(|r| repaintable(r)) {
            return;
        }
        for r in t.rows.iter_mut() {
            if repaintable(r) {
                r.damaged = true;
            }
        }
    }
    // Lock released FIRST: `composite` takes the table lock itself, and the lock is not reentrant.
    composite();
}

/// WC-I — the panel boxes the WINDOW LAYER currently owns, written into `out` and returned by count.
///
/// ### Why the desktop needs this (P60: the ~1 Hz synchronized blip)
/// WC-E ordered the two writers by making the desktop's present be FOLLOWED by a window repaint, and
/// named its own residual (`Screen::flush`): the window pixels are overwritten and then repainted
/// rather than never being overwritten at all, so a scan-out landing between the two steps catches
/// the window mid-restore. On the bench that residual has a period and a trigger — the PI-UI-2 status
/// strip's 1 Hz tick (`main.rs::status_tick` → `Event::Timer` → `ui_status::draw` → `pal.render`).
/// One tick repaints EVERY window on the panel, which is exactly the reported fingerprint: a blip in
/// every vug window at the same instant, slightly faster than once a second (the tick's own period
/// plus the Key/Button passes that also mark the strip dirty), while the desktop and the console —
/// single-writer surfaces — stay clean. A window that presents rarely (the stat window) is almost
/// never mid-present when the repaint lands and reads clean too; the high-rate vug windows are hit
/// every time, and each hit ALSO puts a second compositor on a second core, so `STAGE`'s `try_lock`
/// declines and those windows fall back to the pre-WC-H direct, tearing path.
///
/// The fix is to stop the overwrite happening at all: the desktop subtracts these boxes from its own
/// damage before it copies, so a window's pixels are never desktop pixels for any interval, however
/// short. `Screen::present_background` is the only caller.
///
/// **Compat rows are excluded.** A compat row IS the full-screen present path: its box is the panel,
/// subtracting it would suppress the desktop entirely, and while a foreground full-screen program
/// owns the panel the render task is parked and not flushing anyway. That is the same population
/// `stage_window` and `wcg::begin` already scope themselves to, so the staged set, the instrumented
/// set and the occluding set are one set.
///
/// **Rows the shell hides are excluded**, because the console owns those pixels — subtracting them
/// would leave a permanently stale rectangle where a hidden window used to be, which is the opposite
/// of what FOCUS-VIS's erase established.
///
/// A snapshot, never a handle. It is read without the desktop holding anything of ours, and a window
/// that moves or closes immediately afterwards is repainted by the mover/closer's own composite.
pub fn occluders(out: &mut [(usize, usize, usize, usize); MAX_WINDOWS]) -> usize {
    let shell = shell_z();
    let t = TABLE.lock();
    let mut n = 0usize;
    for r in t.rows.iter() {
        if !r.used || r.compat || !above_shell(r, shell) {
            continue;
        }
        let b = outer_box(r);
        if b.2 == 0 || b.3 == 0 {
            continue;
        }
        out[n] = b;
        n += 1;
    }
    n
}

/// WC-I — composite whatever is ALREADY marked damaged, and nothing else.
///
/// The desktop's post-present call once WC-I's subtraction is in place. [`repaint`] exists to undo an
/// overwrite the desktop performed, so it damages the whole live set; with the overwrite gone there
/// is nothing to undo, and re-blitting every window once per desktop frame would keep the exact cost
/// and the exact multi-core contention the blip is made of. What still has to be serviced on the
/// desktop's cadence is damage some OTHER path recorded — chiefly `cursor::repair`, whose contract
/// ("marks only; never composites") depends on someone else running the pass within a frame.
///
/// Cheap when idle: one table-lock acquisition and a scan of eight rows, then out. The pass itself
/// only runs when a row is genuinely damaged.
pub fn service_damage() {
    {
        let t = TABLE.lock();
        if !t.rows.iter().any(|r| r.used && r.damaged) {
            return;
        }
    }
    composite();
}

/// CURSOR-1 — mark every window whose outer box overlaps `(x, y, w, h)` damaged, so the next
/// composite redraws it from its source surface. Returns the number marked.
///
/// The system cursor's save-under restore writes into the scan-out from outside the compositor. Its
/// colour guard means a pixel another painter has taken is left alone, but a painter whose content
/// happens to equal the sprite's own colour is indistinguishable from the sprite — so a restore CAN,
/// narrowly, put stale pixels inside a window's rect. This is how that is repaired: the affected
/// windows are damaged, and the next composite overwrites the lot from the app's surface.
///
/// **Marks only; never composites.** [`composite`] brackets itself with `video::cursor`, so a
/// composite from the cursor path would recurse. WC-I keeps the "within a frame" guarantee that
/// depends on: the desktop no longer runs a blanket [`repaint`], but [`service_damage`] on the same
/// cadence composites exactly the rows this function marked. Without a desktop, at the next present.
///
/// Compat rows are included: they are a full-screen present whose rect covers the panel, so a stale
/// patch there is exactly as visible as anywhere else.
pub fn damage_intersecting(x: usize, y: usize, w: usize, h: usize) -> usize {
    if w == 0 || h == 0 {
        return 0;
    }
    let rect = (x, y, w, h);
    let mut t = TABLE.lock();
    let mut n = 0usize;
    for i in 0..MAX_WINDOWS {
        if !t.rows[i].used || t.rows[i].damaged {
            continue;
        }
        if boxes_overlap(rect, outer_box(&t.rows[i])) {
            t.rows[i].damaged = true;
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
/// CURSOR-1 — the composite pass, bracketed by the system cursor.
///
/// Whenever the sprite could be over a pixel this pass writes, it is taken OFF the panel before any
/// window pixel is drawn or read, and put back only after the last `draw_window` / `verify_window`
/// has returned. That ordering is what makes the cursor free of consequences for the witnesses:
/// `[wc-d]`'s scan-out read-back never sees a sprite pixel in a window's rect, and `[wc-c]`'s
/// checksum reads the source surface, which the cursor never touches. (The second, independent
/// guarantee is that the sprite is drawn only after a real pointer report — see `video::cursor`.)
///
/// Every early return in the pass body (the drain barrier, a framebuffer that is not ready) reports
/// the same `disturbed` answer the full pass does, so there is no path that can paint or verify with
/// the sprite still on the panel, and none that returns having left it off.
///
/// WC-I — the bracket is now CONDITIONAL, and the tail is the cheap form. See
/// [`super::cursor::sprite_box`] for the panel symptom: an unconditional restore→save→draw per
/// present, at several windows' present rate across several cores, is what made the sprite spotty.
/// [`composite_inner`] undraws only when the sprite's box actually intersects a window this pass is
/// going to paint, and reports whether it did; the tail then either restores the sprite properly
/// (we moved its pixels) or merely makes sure it is on the panel (we did not).
///
/// `ensure_drawn` and not "nothing" on the false branch, because `erase` takes the sprite down and
/// leaves the composite that follows to put it back — that contract predates WC-I and is unchanged.
///
/// CURSOR-3 — a third tail. When the pass carried the sprite through a staged present (see
/// [`super::cursor::compose_into`]) the panel already has it, and the tail's job is to tell the
/// sprite module so rather than to paint anything: [`super::cursor::adopt_overlay`] installs the
/// plan and, in the common case, writes no pixels at all.
pub fn composite() {
    let tail = composite_inner();
    #[cfg(feature = "witness")]
    note_cursor_tail(tail);
    match tail {
        CursorTail::Adopt => super::cursor::adopt_overlay(),
        CursorTail::Repaint => super::cursor::repaint(),
        CursorTail::Untouched => super::cursor::ensure_drawn(),
    }
}

/// What [`composite_inner`] owes the sprite when it returns.
///
/// `Untouched` and `Repaint` are WC-I's two answers, unchanged and with unchanged meanings.
/// `Adopt` is CURSOR-3's: the pass undrew the sprite AND painted it back inside a staged present, so
/// the panel is already correct and only the module's bookkeeping is outstanding. Every early exit
/// from the pass owes `Repaint` once the bracket has been taken — `Adopt` is reachable only from the
/// one path that actually composed the sprite into a back layer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CursorTail {
    Untouched,
    Repaint,
    Adopt,
}

/// The composite pass proper. Private so the cursor bracket above cannot be bypassed — every caller
/// (including this module's own teardown paths) goes through [`composite`].
fn composite_inner() -> CursorTail {
    // WC-I — THE CURSOR BRACKET, decided BEFORE anything is registered as an in-flight blit.
    //
    // Before WC-I `composite` undrew the sprite unconditionally on every call — and `composite` runs
    // once per window present, from the presenting task's own core. With several high-rate windows the
    // sprite spent its life mid restore→save→draw on one core or another, and `cursor::undraw_locked`'s
    // colour guard DECLINES to restore a pixel another painter has taken, which under that contention
    // is most of them: the sprite reads as spotty and flickering, which is the second P60 symptom. The
    // cost was paid on every present regardless of where the pointer actually was.
    //
    // **Placement is a correctness constraint, not a preference.** `undraw` takes the SPRITE lock, and
    // `F4`'s drain barrier is a teardown spinning IRQ-MASKED and unpreemptible until `BLIT_ACTIVE`
    // reaches zero. Acquiring SPRITE from inside the `BlitGuard` window would put a second lock into
    // that wait set, so a core preempted while holding SPRITE would stall the draining core forever
    // rather than for the length of a bounded blit. Deciding here — before the snapshot that registers
    // the guard — keeps the drain's wait set exactly what its termination argument assumes.
    //
    // The test is therefore against every live window ABOVE THE SHELL rather than only the damaged
    // ones: the dirty set is closed upwards over occlusion inside the pass, which this pre-pass cannot
    // see, and a conservative answer here degrades to the pre-WC-I behaviour for one pass while a
    // narrow one would leave sprite-coloured pixels inside a window's rect. The sprite's BOX is used
    // for the same reason (it is a snapshot taken without the sprite lock held, so it must be the
    // conservative extent). There is no false negative: every pixel the sprite paints lies inside the
    // box it reports.
    //
    // Lock order: `SPRITE` → `TABLE` (stated in `cursor::repair`). The table access below is a
    // statement of its own whose guard is dropped before `undraw` is called, so nothing of ours is
    // held across it.
    //
    // CURSOR-3 — this is also where the OVERLAY PLAN is taken, and for the same reason. The plan is
    // the sprite's geometry, snapshotted under the same acquisition as the box we test with; the
    // staged present later paints the sprite from it, with no lock of the sprite module's in the
    // guard's wait set at all (`compose_into` touches only `OVERLAY`, and only with `try_lock`).
    let mut disturbed = false;
    let mut plan: Option<super::cursor::Plan> = None;
    if let Some(p) = super::cursor::sprite_plan() {
        let sbox = (p.bx, p.by, p.bw, p.bh);
        let shell = shell_z();
        #[allow(unused_mut)]
        let mut hit = {
            let t = TABLE.lock();
            t.rows
                .iter()
                .any(|r| r.used && above_shell(r, shell) && boxes_overlap(sbox, outer_box(r)))
        };
        // WC-F's ground-truth probe paints at the TAIL of this pass and is outside the window layer,
        // so `repair` (which damages WINDOWS) can never mend a sprite pixel it took. Treat its
        // reserved boxes as painted extents too. Witness/baremetal-only, like the probe itself.
        //
        // CURSOR-3: a sprite over a reserved box also gives up the overlay. The probe paints into the
        // FRONT buffer after this pass, so pixels the overlay would have delivered inside a staged
        // present are pixels the probe overwrites — the sprite module's plan would then describe a
        // panel that no longer holds it, and the colour guard would decline the restore. The bracket
        // handles this case as it did before; the probe is witness/baremetal-only and one region.
        #[allow(unused_mut)]
        let mut reserved_hit = false;
        #[cfg(all(target_arch = "aarch64", feature = "witness", feature = "baremetal"))]
        {
            let fb = *super::WRITER.lock();
            if fb.is_ready() {
                let (pw, ph) = (fb.info().width, fb.info().height);
                if let Some(boxes) = super::wcf::reserved(pw, ph) {
                    reserved_hit = boxes.iter().any(|b| boxes_overlap(sbox, *b));
                }
            }
            hit |= reserved_hit;
        }
        if hit {
            super::cursor::undraw();
            disturbed = true;
            if !reserved_hit {
                plan = Some(p);
                #[cfg(feature = "witness")]
                CUR3_PLANNED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    #[cfg(feature = "witness")]
    {
        WCI_CURSOR_PASSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if disturbed {
            WCI_CURSOR_BRACKETS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

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
        // WC-I: `disturbed`, not `false` — the bracket above may already have taken the sprite off the
        // panel, and the caller's tail is what puts it back. Every early exit from here on owes the
        // same answer.
        if DRAIN_PENDING.load(core::sync::atomic::Ordering::Acquire) != 0 {
            return tail_of(disturbed, false);
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
        return tail_of(disturbed, false);
    }

    // Back-to-front: ascending z, ties by id (creation order).
    let mut order = [0usize; MAX_WINDOWS];
    for (i, slot) in order.iter_mut().enumerate() {
        *slot = i;
    }
    order.sort_unstable_by_key(|&i| (rows[i].z, rows[i].id));

    // FOCUS-VIS — one snapshot of the shell's z for the whole pass, so every window in it is judged
    // against the same shell position (a concurrent focus change either lands wholly before this pass
    // or is serviced by the composite that change performs itself).
    let shell = shell_z();
    // FOCUS-HL: one snapshot per pass, for the same reason `shell` is one snapshot per pass — every
    // window in this pass is judged against a single focus owner, so no pass can draw two highlights.
    let focus = focus_asid();

    let mut drawn = 0usize;
    // CURSOR-3 — did any window in this pass carry the sprite through its staged present? Back-to-
    // front order means the LAST window to take the overlay is the topmost one that fully contains
    // the sprite, and its plan is the one `OVERLAY` ends up holding — which is the window the
    // operator is pointing at.
    let mut overlaid = false;
    for &i in order.iter() {
        if !rows[i].used || !dirty[i] {
            continue;
        }
        // FOCUS-VIS: below the shell, so the console owns these pixels — do not draw. The damage flag
        // was already cleared above, which is right: `focus_changed` re-damages every window it raises,
        // so a window coming back up repaints from its source surface rather than inheriting a stale
        // flag from while it was hidden. Outermost of the per-window guards: WC-G's sampler below
        // must never bracket a window this skip declines to draw.
        if !above_shell(&rows[i], shell) {
            continue;
        }
        // WC-G — bracket the blit. `begin` must be the last thing before `draw_window` and `end` the
        // first thing after it: the `blit`/`after` checksums mean "the surface as the copy found it"
        // and "as the copy left it", and anything inserted between them widens the interval they
        // measure into something other than the copy. Budgeted per window id; `None` once spent.
        #[cfg(all(target_arch = "aarch64", feature = "witness"))]
        let wcg_probe = super::wcg::begin(rows[i].id, rows[i].surf, rows[i].surf_len, rows[i].compat);
        // CURSOR-3 — WHICH WINDOWS MAY CARRY THE SPRITE. WC-I's invariant "no verified pixel is ever
        // read with the sprite on the panel" is preserved here rather than weakened: this pass may
        // read this window's destination pixels back and compare them against its SOURCE surface —
        // `wcg::end`'s `fbbad` count and `verify_window`'s scan-out verdict both do exactly that — and
        // a cursor legitimately composited into those pixels would read as a blit defect. Both
        // instruments are budgeted one-shots, so declining the overlay for the handful of passes they
        // run on costs those passes WC-I's bracket and nothing else. Non-witness builds have neither
        // instrument and no condition to test.
        #[allow(unused_mut)]
        let mut window_plan = plan;
        #[cfg(all(target_arch = "aarch64", feature = "witness"))]
        if wcg_probe.is_some() {
            window_plan = None;
        }
        #[cfg(feature = "witness")]
        {
            let r = &rows[i];
            if !r.compat && r.presented && r.id < 32 {
                let bit = 1u32 << r.id;
                if VERIFIED.load(core::sync::atomic::Ordering::Relaxed) & bit == 0 {
                    window_plan = None;
                }
            }
        }
        // FOCUS-HL: `focus == 0` is shell focus and highlights nothing — and the explicit `!= 0` also
        // keeps a compat row (owner ASID 0) from matching it by accident.
        overlaid |= draw_window(
            &fb,
            &rows[i],
            focus != 0 && focus == rows[i].owner_asid,
            window_plan,
        );
        #[cfg(all(target_arch = "aarch64", feature = "witness"))]
        if let Some(p) = wcg_probe {
            let r = &rows[i];
            super::wcg::end(p, &fb, r.x, r.y, r.w, r.h, r.stride, r.scale);
        }
        // WC-H — print the back-layer sample the blit above recorded, if any. Deliberately AFTER
        // `wcg::end`: this emits to the serial UART, and inside the bracket it would be charged to
        // `[wc-g] us=`. See `wcg::stage_flush`.
        #[cfg(all(target_arch = "aarch64", feature = "witness"))]
        super::wcg::stage_flush(rows[i].id);
        // WC-D — verify this window's blit against the scan-out, once per window id, from inside the pass
        // that drew it (the only place both the source surface and the destination rows are known).
        #[cfg(feature = "witness")]
        {
            let r = &rows[i];
            if !r.compat && r.presented && r.id < 32 {
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
    // WC-F — the scan-out ground-truth probe, at the TAIL of the pass so its marks are the last thing
    // written and no compositor writer can erase them mid-frame. Repainted so the panel still carries
    // them when the bench operator photographs it; prints once. See `video::wcf` and
    // docs/dev/OS/08_VIDEO/engine.md §WC-F.
    //
    // Two conditions, both here rather than in `wcf` because this is where the table snapshot lives.
    // `drawn > 0`: the probe writes ~12 K pixels and cleans its rows, and it is instrumenting the very
    // path it runs in — charging that to every idle repaint would perturb what it measures. Passes
    // that already drew and flushed are the ones where its cost disappears into work being done
    // anyway. The overlap test: the probe paints LAST, so anything it covers it wins, and a window
    // under its region would silently show WC-F's pattern instead of the app's content.
    #[cfg(all(target_arch = "aarch64", feature = "witness", feature = "baremetal"))]
    if drawn > 0 {
        let (pw, ph) = (fb.info().width, fb.info().height);
        let clear = match super::wcf::reserved(pw, ph) {
            None => false,
            Some(boxes) => !rows
                .iter()
                .any(|r| r.used && boxes.iter().any(|b| boxes_overlap(*b, outer_box(r)))),
        };
        super::wcf::run(&fb, clear);
    }
    let _ = drawn;
    tail_of(disturbed, overlaid)
}

/// CURSOR-3 — what the pass owes the sprite, from the two facts the pass records.
///
/// `overlaid` implies `disturbed` by construction (the plan is only ever handed down on the branch
/// that undrew), and the assertion is cheap enough to keep as a debug one: an `Adopt` returned by a
/// pass that never took the sprite off the panel would install a plan describing pixels some other
/// painter owns.
fn tail_of(disturbed: bool, overlaid: bool) -> CursorTail {
    debug_assert!(!overlaid || disturbed);
    if overlaid && disturbed {
        CursorTail::Adopt
    } else if disturbed {
        CursorTail::Repaint
    } else {
        CursorTail::Untouched
    }
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
    // FOCUS-HL: ONE snapshot, as `composite_inner` takes. Reading the atomic twice in a single expression
    // could straddle a focus change and evaluate the two halves against different owners.
    let focus = focus_asid();
    // CURSOR-3: `None` — this redraw exists to put the window's OWN pixels back after the invalidate,
    // and it runs on the one pass whose read-back forbade the overlay in the first place.
    draw_window(fb, r, focus != 0 && focus == r.owner_asid, None);
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

// ---- WC-I witnesses ----------------------------------------------------------------------------

/// WC-I — composite passes that ran, and the subset that had to take the sprite off the panel.
///
/// The pair is the whole claim of the cursor half of this arc: before WC-I `brackets == passes` by
/// construction, because `composite` undrew unconditionally. Reported by [`wci_rollup`].
///
/// **Honest scope on the gate.** QEMU raspi4b delivers no HID pointer report, so the sprite is never
/// drawn there and `brackets` is trivially 0 — the QEMU line proves the counter is wired and that the
/// window path did not start bracketing for some other reason, and nothing more. The number that
/// carries the fix is `brackets < passes` on the bench, where a pointer exists.
#[cfg(feature = "witness")]
static WCI_CURSOR_PASSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static WCI_CURSOR_BRACKETS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-I — desktop presents that wrote background pixels INTO a live window's box. This is the
/// ~1 Hz blip, counted: every one of these is a window that was erased and then restored, and the
/// arc's claim is that after the subtraction in `Screen::present_background` the count is zero.
///
/// Bumped by [`note_desktop_flush`] from the desktop's own present path, so a future change that
/// reintroduces the overwrite (a new flush path, a parallel band that forgets the occluders) shows up
/// as a non-zero rollup rather than as a panel artefact nobody can reproduce in QEMU.
#[cfg(feature = "witness")]
static WCI_INTRUSIONS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-I — desktop presents that ran with at least one live window on the panel. The denominator the
/// intrusion count is meaningful against: `windowed=0 intrusions=0` proves nothing, and the rollup
/// says so in its verdict rather than leaving a reader to notice.
#[cfg(feature = "witness")]
static WCI_WINDOWED_FLUSHES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-I — record one desktop present that ran over a live window layer, and whether it intruded on
/// it. Called by `Screen::present_background`, the only writer.
#[cfg(feature = "witness")]
pub(super) fn note_desktop_flush(windowed: bool, intruded: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    if intruded {
        WCI_INTRUSIONS.fetch_add(1, Relaxed);
    }
    if !windowed {
        return;
    }
    let n = WCI_WINDOWED_FLUSHES.fetch_add(1, Relaxed) + 1;
    // Fire the desktop-scoped rollup once, at the point the evidence exists. The fixture-time call in
    // the WC-B witness block proves the counters are WIRED; only this one can say the desktop ran over
    // a live window layer and did not intrude, because only here has that actually happened.
    //
    // `WCI_EVIDENCE` samples is the threshold and it is deliberately well past one: the blip is a
    // PERIODIC event at the status strip's ~1 Hz, so a verdict taken after a couple of flushes could
    // miss it by luck. At the bench's ~20 fps desktop that is a few seconds of panel time and several
    // strip ticks; on the QEMU gate — where `status_tick` is not spawned (no Group-1 IRQ) and there is
    // no input to flush on — it never fires at all, which is the honest outcome and why the
    // fixture-time line reports `UNWITNESSED` there rather than a vacuous `CLEAN`.
    if n == WCI_EVIDENCE && !WCI_DESKTOP_ROLLED.swap(true, Relaxed) {
        wci_rollup_scoped("desktop");
    }
}

/// WC-I — windowed desktop presents required before the desktop-scoped rollup is worth printing.
#[cfg(feature = "witness")]
const WCI_EVIDENCE: u64 = 64;

#[cfg(feature = "witness")]
static WCI_DESKTOP_ROLLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// WC-I — the arc's rollup, printed once per boot from the witness harness.
///
/// Two independent claims on one line, because they are two faces of one defect — the periodic
/// full-desktop repaint that overwrote every window at once, and the unconditional cursor bracket
/// that every one of those repaints then ran on top:
///  * `intrusions` — desktop presents that wrote background pixels inside a live window's box. The
///    P60 blip is this number being one per status-strip tick; the fix makes it 0.
///  * `brackets`/`passes` — composite passes that had to take the sprite off the panel. See
///    [`WCI_CURSOR_PASSES`] for why the gate can only witness the wiring of this half.
///
/// The verdict is `CLEAN` only when the desktop ran over a window layer at least once AND never
/// intruded; a boot that never had a window on the panel while the desktop flushed reports
/// `UNWITNESSED`, so an empty run can never be read as a pass.
#[cfg(feature = "witness")]
pub fn wci_rollup() {
    wci_rollup_scoped("fixture");
}

/// WC-I — the rollup body. `scope` names WHICH evidence the line was taken on: `fixture` at the end of
/// the window-verb witness block (the counters are wired), `desktop` after the desktop layer has
/// presented over a live window layer enough times for the verdict to mean something.
#[cfg(feature = "witness")]
fn wci_rollup_scoped(scope: &str) {
    use core::sync::atomic::Ordering::Relaxed;
    let windowed = WCI_WINDOWED_FLUSHES.load(Relaxed);
    let intrusions = WCI_INTRUSIONS.load(Relaxed);
    let passes = WCI_CURSOR_PASSES.load(Relaxed);
    let brackets = WCI_CURSOR_BRACKETS.load(Relaxed);
    let verdict = if intrusions > 0 {
        "INTRUDED"
    } else if windowed == 0 {
        "UNWITNESSED"
    } else {
        "CLEAN"
    };
    serial_println!(
        "[wc-i] rollup scope={} windowed_flushes={} intrusions={} cursor_passes={} cursor_brackets={} -> {}",
        scope, windowed, intrusions, passes, brackets, verdict
    );
    cursor3_rollup(scope);
}

// ---- CURSOR-3 witnesses ------------------------------------------------------------------------

/// CURSOR-3 — composite passes that took WC-I's bracket AND still had an eligible overlay plan to
/// hand down. The denominator for `taken`: a bracket with no plan is a pass the overlay was never
/// offered on (the sprite is over a reserved probe box, or over a window an instrument is reading),
/// and counting it against the mechanism would understate it.
#[cfg(feature = "witness")]
static CUR3_PLANNED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-3 — per-window overlay offers and the subset that landed. `offers - taken` is the
/// straddling sprite, the contended plan lock, and the unreadable layer — every one of which falls
/// back to WC-I's bracket, so the difference is a MISSED IMPROVEMENT and never a defect.
#[cfg(feature = "witness")]
static CUR3_OFFERS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR3_TAKEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-3 — how each pass's tail was settled: `adopt` (the panel already carried the sprite, the
/// module only had to agree), `repaint` (WC-I's bracket ran to completion), `ensure` (the pass never
/// touched the sprite — WC-I's cheap tail, and the desktop case).
#[cfg(feature = "witness")]
static CUR3_ADOPT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR3_REPAINT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR3_ENSURE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-3 — record one overlay offer and whether the layer took it. Called from `stage_window`,
/// which is inside the `BlitGuard` window: relaxed atomics only, no lock, no allocation, no serial.
#[cfg(feature = "witness")]
fn note_cursor_overlay(took: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    CUR3_OFFERS.fetch_add(1, Relaxed);
    if took {
        CUR3_TAKEN.fetch_add(1, Relaxed);
    }
}

/// CURSOR-3 — per-present samples printed before the budget is spent. The mechanism is a per-present
/// decision, so the rollup alone would leave a bench operator unable to see WHICH presents took which
/// tail while the pointer was where; a handful of lines makes the sequence readable and then stops,
/// because a present path at ~60 Hz per window cannot afford a line per frame.
///
/// Only passes that actually touched the sprite print — a pass with no sprite anywhere near a window
/// is the overwhelming majority and says nothing. On QEMU that is EVERY pass, so this prints nothing
/// at all there and default-quiet boot is unchanged.
#[cfg(feature = "witness")]
const CUR3_SAMPLES: u64 = 8;

#[cfg(feature = "witness")]
static CUR3_SAMPLED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-3 — record which tail a pass took, from [`composite`] and nowhere else.
///
/// Called with no lock of this module's or the sprite module's held, and outside the `BlitGuard`
/// window — which is what makes the sampled `serial_println!` admissible here at all.
#[cfg(feature = "witness")]
fn note_cursor_tail(tail: CursorTail) {
    use core::sync::atomic::Ordering::Relaxed;
    let name = match tail {
        CursorTail::Adopt => {
            CUR3_ADOPT.fetch_add(1, Relaxed);
            "adopt"
        }
        CursorTail::Repaint => {
            CUR3_REPAINT.fetch_add(1, Relaxed);
            "repaint"
        }
        CursorTail::Untouched => {
            CUR3_ENSURE.fetch_add(1, Relaxed);
            return;
        }
    };
    if CUR3_SAMPLED.fetch_add(1, Relaxed) < CUR3_SAMPLES {
        serial_println!(
            "[cursor3] present tail={} offers={} taken={} -> {}",
            name,
            CUR3_OFFERS.load(Relaxed),
            CUR3_TAKEN.load(Relaxed),
            if tail == CursorTail::Adopt { "COMPOSED" } else { "BRACKETED" }
        );
    }
}

/// CURSOR-3 — the arc's rollup, printed alongside `[wc-i]`'s (same scopes, same harness, one arc's
/// worth of cursor evidence in two lines that can be read together).
///
/// **What the QEMU gate can and cannot witness, stated rather than implied.** QEMU raspi4b delivers
/// no HID pointer report, so `pal::cursor::visible()` is false for the whole boot, the sprite is
/// never drawn, `sprite_plan()` is always `None`, and every counter below is 0. The gate therefore
/// proves NO-REGRESSION only: the window path still runs its passes, still settles every one of them
/// through a tail, and never took an overlay it was never offered. The verdict says `UNWITNESSED` in
/// exactly that case, so a run with no pointer can never be read as evidence for the mechanism. The
/// number that carries the fix is `taken > 0` with `offers == taken` on the bench, where a pointer
/// exists and the operator can hold it over a presenting window.
#[cfg(feature = "witness")]
fn cursor3_rollup(scope: &str) {
    use core::sync::atomic::Ordering::Relaxed;
    let planned = CUR3_PLANNED.load(Relaxed);
    let offers = CUR3_OFFERS.load(Relaxed);
    let taken = CUR3_TAKEN.load(Relaxed);
    let adopt = CUR3_ADOPT.load(Relaxed);
    let repaint = CUR3_REPAINT.load(Relaxed);
    let ensure = CUR3_ENSURE.load(Relaxed);
    // `taken > offers` would mean an overlay landed without being offered — a wiring defect, and the
    // only outcome here that is a defect rather than an absence of evidence.
    let verdict = if taken > offers || adopt > taken {
        "INCOHERENT"
    } else if offers == 0 {
        "UNWITNESSED"
    } else if taken == 0 {
        "BRACKETED"
    } else {
        "COMPOSED"
    };
    serial_println!(
        "[cursor3] rollup scope={} planned={} offers={} taken={} adopt={} repaint={} ensure={} -> {}",
        scope, planned, offers, taken, adopt, repaint, ensure, verdict
    );
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

/// FOCUS-HL — the same three chrome colours, brightened, for the window that currently holds focus.
/// The border carries most of the signal (it frames the whole window, so it reads at a glance from
/// across the bench) and the title strip lifts with it so the two do not disagree.
///
/// Chosen to stay in the same flat, un-host-like family as the resting colours — this marks focus, it
/// does not imitate anyone's title bar — while clearing a wide enough gap to be unambiguous on the
/// bench panel rather than only on a screenshot.
const CHROME_BORDER_FOCUS: u32 = 0x008C_8CB4;
const CHROME_TITLE_BG_FOCUS: u32 = 0x003A_3A5A;

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

// ---- WC-H: the window back-layer ---------------------------------------------------------------

/// WC-H — hard ceiling on the staging buffer, in bytes. A window whose clipped outer box needs more
/// than this composes on the DIRECT path instead (the pre-WC-H behaviour): the fallback is always
/// available, so the cap bounds the compositor's memory rather than its correctness. 4 MiB covers a
/// 1024x1024 box at 4 bytes/pixel — comfortably past the bench's 128x128@4x window (~514x526, 1.08
/// MiB) and far short of a panel-sized allocation.
const MAX_STAGE_BYTES: usize = 4 * 1024 * 1024;

/// WC-H — the window back-layer's memory: one buffer, allocated on first use at the size the largest
/// window needs and reused by every composite thereafter.
///
/// This is the one heap allocation this module makes, and the module header's "no heap in the
/// compositor" note is narrowed rather than contradicted: the allocation happens on GROWTH only (the
/// steady state is a lock, a paint and a copy, with no allocator involvement), it uses `try_reserve`
/// so exhaustion returns an error instead of panicking from present context, and every failure —
/// exhaustion, over-cap geometry, or another core holding the lock — falls back to the direct path.
/// A window can therefore never fail to be drawn because of the back-layer.
///
/// `try_lock`, never `lock`: two cores can composite at once (a present on one, a desktop-flush
/// repaint on the other). The loser takes the direct path rather than blocking a present behind
/// another core's copy, which keeps the buffer single-writer without adding a wait to the path.
///
/// The buffer is zero-initialised and only ever written through `put_pixel`, which writes the 3
/// colour bytes of a 4-byte pixel and never the pad byte — so pad bytes stay 0 for the life of the
/// buffer, and the rows copied to the front carry the same zero pad `Screen`'s back buffer has
/// always presented.
static STAGE: Mutex<alloc::vec::Vec<u8>> = Mutex::new(alloc::vec::Vec::new());

/// WC-H — whether the one-shot fallback fixture has been spent. See the fixture block in
/// [`stage_window`]: it forces exactly one non-compat composite onto the direct path so WC-D's
/// scan-out read-back covers the fallback as well as the staged present. `witness`-gated and
/// aarch64-only, so the flashable media are unaffected.
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
static FALLBACK_FIXTURE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Paint one window: chrome (border frame + title strip + title text), then the surface content
/// nearest-neighbour upscaled by `scale`, then one cache clean over the rows it touched so the
/// non-coherent Pi 4 HVS scan-out sees the new pixels. All writes are `put_pixel`/`fill_rect` on a
/// `Copy` `FrameBuffer` handle (volatile stores, no aliased `&mut`), the same discipline
/// `Screen::flush` and the legacy `present_surface` use.
///
/// ### WC-H — why the paint no longer lands in the scan-out
///
/// WC-G localized the garble to the one suspect no checksum can reach. Every byte was correct at
/// every moment (`coher=0 race=0 blit=0`) and the copy still took ~15 ms while the beam crosses the
/// window's own rows in ~7 ms: the copy is structurally overtaken by the scan-out, and what the
/// panel latches is part-old and part-new, split at whatever scanline the beam held — horizontal
/// tearing, and exactly the shape in the bench photograph.
///
/// The cause is not the copy's *content* but its *destination*. A window's pixels were poked one at
/// a time into the live front framebuffer, upscaled — at 4x, each source pixel is 16 separate
/// bounds-checked `put_pixel` calls into memory the HVS is scanning right now — with no vblank
/// synchronisation anywhere. The desktop has never had this problem because it does not write the
/// front at all: it draws into `Screen`'s cached back buffer and reaches the panel through a
/// contiguous per-row damage-rect flush.
///
/// WC-H gives the window layer the same treatment, in the only form available to it:
///
/// * **Compose** the whole window — chrome, title, upscaled content — into a cached-RAM back layer
///   ([`STAGE`]) sized to the window's clipped outer box. Every expensive, scattered, per-pixel
///   write happens here, where no scan-out can observe a partial result.
/// * **Present** it as contiguous full-box rows: one `blit` (a bulk `copy_nonoverlapping`) per row,
///   then the same single `flush_range` over the touched scanlines. This is byte-for-byte the
///   discipline `Screen::present_background` uses, and it is the ONLY part of the operation the
///   panel can catch mid-flight.
///
/// The exposure window therefore shrinks from "the entire upscale" to "`bh` bulk row copies" —
/// the identical mechanism, and the identical bound, that makes the desktop path clean.
///
/// ### The trade, stated: why not route windows through `Screen` itself
///
/// The obvious "do it right" answer is to hand the window layer to the same `Screen` the desktop
/// flushes, and it is structurally blocked, not merely inconvenient. There is no global `Screen`:
/// each render task constructs its OWN (`main.rs` builds four, `witness.rs` a fifth) over the shared
/// `WRITER` framebuffer, and none is reachable from here. The compositor runs from the presenting
/// task's syscall context on an arbitrary core, while the render task that owns a `Screen` may be
/// parked inside `dispatch_command` (the compat/full-screen case) and flushing nothing at all —
/// which is precisely why `present_surface` never routed through it either. Reaching a `Screen`
/// would mean promoting one to a global, giving it a lock taken from syscall context, and making
/// every window present wait on the desktop's frame cadence. This arc takes the second option from
/// the brief instead — a dedicated window back-layer with the same present discipline — which buys
/// the same tear-free property with no ownership change and no new cross-subsystem lock.
///
/// **Cost.** One extra full copy of the window's box per composite (compose to RAM, then RAM to
/// panel). The panel-facing half gets cheaper, not dearer: bulk row copies replace ~`scale²`
/// bounds-checked pokes per source pixel. `[wc-h]` reports both halves separately so the trade is on
/// the wire rather than asserted.
///
/// **Compat rows keep the direct path, deliberately.** A compat window is the full-screen
/// `present_surface` shim: its box is the whole panel, so staging it would cost a panel-sized
/// allocation and a full extra panel copy per present — enough, on the evidence of the `repaint`
/// exclusion that preceded it, to blow the `EXEC-UVUG` frame deadline. It also does not need it:
/// while a foreground full-screen EL0 program owns the panel the render task is parked, so the
/// contention this arc exists to remove is not present. `wcg::begin` already returns `None` for
/// compat rows, so the instrumented population and the staged population are the same set.
///
/// ### CURSOR-3 — the sprite rides the present
///
/// `cur` is the compositor's overlay plan (`None` when the pointer is elsewhere, when the pass did
/// not take the sprite off the panel, or when an instrument on this window forbids it). When the
/// staged path runs and the sprite's box lies wholly inside this window's clipped outer box, the
/// sprite is painted into the BACK LAYER after the window is composed and before the rows are
/// copied — so it reaches the panel inside the same contiguous row copies, and the window's pixels
/// are never on the panel without it. Returns whether that happened; the direct fallback path never
/// takes the overlay (it has no back layer to save from), and reports `false` so the caller's tail
/// falls back to WC-I's repaint.
fn draw_window(
    fb: &super::FrameBuffer,
    r: &Window,
    focused: bool,
    cur: Option<super::cursor::Plan>,
) -> bool {
    // `stride`/`scale` are divisors below and `surf_len` bounds the reads, so all four are checked
    // here rather than trusted from the row.
    if r.surf == 0 || r.w == 0 || r.h == 0 || r.scale == 0 || r.stride < 4 || r.surf_len == 0 {
        return false;
    }
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    let (bx, by, bw, bh) = outer_box(r);
    if bx >= pw || by >= ph {
        return false;
    }
    // F6 — clip the outer box to the panel BEFORE any loop runs over it. `put_pixel`/`fill_rect`
    // clip per pixel, which keeps the writes safe but still ITERATES the full extent: a window
    // claiming 10000x10000 would spin ~1e8 clipped pokes per present, from syscall context.
    let bw = bw.min(pw - bx);
    let bh = bh.min(ph - by);

    // WC-H — compose off-screen and present the box as contiguous rows. Returns false when the
    // back-layer is unavailable (compat row, over-cap geometry, another core holding it, or the
    // allocator declining), in which case the direct path below runs exactly as it always has.
    let mut overlaid = false;
    if !stage_window(fb, r, bx, by, bw, bh, pw, ph, focused, cur, &mut overlaid) {
        paint_window(fb, r, 0, 0, bx, by, bw, bh, pw, ph, focused, false);
    }

    // Clean the touched rows (superset: whole scanlines of the outer box) for the non-coherent
    // scan-out — one `DC CVAC` sweep per window, not one per scanline. No-op on coherent targets.
    // Unchanged by WC-H: staged or direct, the same panel rows were written by the time we get here.
    let row_bytes = info.stride * info.bytes_per_pixel;
    let y0 = by.min(info.height);
    let y1 = (by + bh).min(info.height);
    if y1 > y0 {
        fb.flush_range(y0 * row_bytes, (y1 - y0) * row_bytes);
    }
    // CURSOR-3: the cache clean above covers the sprite's pixels for free — they are inside these
    // rows, which is the whole point of composing them into the layer rather than poking them into
    // the front afterwards. Nothing extra is written to the front buffer on this path.
    overlaid
}

/// WC-H — paint the window's chrome and upscaled content into `dst`, whose origin sits at panel
/// coordinate `(ox, oy)`. The two callers differ only in that origin: the direct path passes the
/// front framebuffer with `(0, 0)`, the staged path passes the back layer with the outer box's
/// top-left. Every clip bound is still derived from the PANEL (`pw`/`ph`), so both paths draw the
/// identical pixel set — the back layer is exactly `bw x bh` of it, addressed from a different zero.
#[allow(clippy::too_many_arguments)]
fn paint_window(
    dst: &super::FrameBuffer,
    r: &Window,
    ox: usize,
    oy: usize,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    pw: usize,
    ph: usize,
    focused: bool,
    dup: bool,
) {
    let (lbx, lby) = (bx.saturating_sub(ox), by.saturating_sub(oy));
    if !r.compat {
        // Frame: fill the whole outer box in the border colour, then lay the title strip and the
        // content over it. The only pixels that survive are the 1-px frame itself.
        //
        // WC-H relies on this fill being DENSE over the whole clipped box: it is what guarantees the
        // back layer carries no residue from the window it staged last. Every pixel the present
        // copies was written by this pass.
        // FOCUS-HL: the ONLY difference the focused window's chrome carries is these two colours. The
        // geometry is identical either way, so focus never moves a pixel — it just repaints the frame
        // and strip that were already going to be painted, at no extra cost per present.
        let (border, title_bg) = if focused {
            (CHROME_BORDER_FOCUS, CHROME_TITLE_BG_FOCUS)
        } else {
            (CHROME_BORDER, CHROME_TITLE_BG)
        };
        dst.fill_rect(lbx, lby, bw, bh, border);
        dst.fill_rect(lbx + BORDER, lby + BORDER, bw.saturating_sub(2 * BORDER), TITLE_H, title_bg);
        draw_title(dst, r, lbx + BORDER + 2, lby + BORDER + 2, bw.saturating_sub(2 * BORDER + 4));
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

    let (cx, cy) = (r.x.saturating_sub(ox), r.y.saturating_sub(oy));
    let dinfo = dst.info();
    let (dw, dh, dbpp) = (dinfo.width, dinfo.height, dinfo.bytes_per_pixel);
    // WC-H — the upscale's vertical runs, written once and replicated. A nearest-neighbour upscale
    // emits `scale` IDENTICAL destination lines per source row; encoding and storing each of them
    // pixel-by-pixel does the same work `scale` times over. On the back layer the destination lines
    // are contiguous byte runs at a known stride, so the first line can be composed per-pixel and the
    // rest produced with one bulk copy each — the same `blit` the present uses.
    //
    // `dup` is the caller's, not inferred, and only the staged caller sets it. On the DIRECT path the
    // replication would copy the front buffer's existing PAD bytes from the first line onto the
    // others (`put_pixel` writes 3 of 4 bytes), so the fallback stays a pure per-pixel write and
    // remains byte-for-byte the pre-WC-H path. On the back layer every pad byte is 0 by construction,
    // so the copy reproduces exactly what the per-pixel loop would have left.
    let dup = dup && r.scale > 1 && dbpp != 0 && dinfo.stride == dw;
    let surf = r.surf as *const u8;
    for row in 0..rows {
        let row_base = row * r.stride;
        let lines = if dup { 1 } else { r.scale };
        for col in 0..cols {
            // Unaligned-safe read of the ARGB8888 pixel; low 24 bits are RRGGBB (alpha ignored —
            // this arc composites opaquely). In bounds by construction: `row < surf_len / stride`
            // and `col < stride / 4`, so `row_base + col * 4 + 4 <= surf_len`.
            let px = unsafe { core::ptr::read_unaligned(surf.add(row_base + col * 4) as *const u32) }
                & 0x00FF_FFFF;
            for sy in 0..lines {
                let dy = cy + row * r.scale + sy;
                for sx in 0..r.scale {
                    dst.put_pixel(cx + col * r.scale + sx, dy, px);
                }
            }
        }
        if !dup {
            continue;
        }
        // Replicate the composed line over the remaining `scale - 1` lines of this source row. The
        // segment is exactly the span `put_pixel` accepted above: the clip is the destination's own
        // width, the same bound that decided which pokes landed.
        let y0 = cy + row * r.scale;
        if y0 >= dh || cx >= dw {
            continue;
        }
        let seg = (cols * r.scale).min(dw - cx) * dbpp;
        let src_off = (y0 * dw + cx) * dbpp;
        if seg == 0 || src_off + seg > dst.len() {
            continue;
        }
        // SAFETY: `src_off + seg <= dst.len()`, and `dst` is a plain byte surface over the back
        // layer — the same memory `blit` writes, read here as the source of the copy. `blit` itself
        // bounds-checks the destination and uses `copy_nonoverlapping`; the ranges are disjoint
        // because `y` is strictly greater than `y0`.
        let line = unsafe { core::slice::from_raw_parts((dst.base_addr() + src_off) as *const u8, seg) };
        for sy in 1..r.scale {
            let y = y0 + sy;
            if y >= dh {
                break;
            }
            dst.blit((y * dw + cx) * dbpp, line);
        }
    }
}

/// WC-H — the back-layer path: compose the window into cached RAM, then present its clipped outer
/// box to the front framebuffer as `bh` contiguous row copies. Returns `false` when the back layer
/// is unavailable, leaving the front untouched so the caller can run the direct path.
///
/// The four declines are all "fall back", never "fail": a compat row (see [`draw_window`]), a box
/// over [`MAX_STAGE_BYTES`], a lock another core holds, and an allocator that cannot grow the
/// buffer. None of them can lose a window.
#[allow(clippy::too_many_arguments)]
fn stage_window(
    fb: &super::FrameBuffer,
    r: &Window,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    pw: usize,
    ph: usize,
    focused: bool,
    cur: Option<super::cursor::Plan>,
    overlaid: &mut bool,
) -> bool {
    if r.compat {
        // Not a decline: compat rows are out of scope by design (see `draw_window`), and counting
        // them would make the back layer look like it was failing when it was never asked.
        return false;
    }
    // Every exit below is a DECLINE: this window reaches the panel through the pre-WC-H direct
    // path — the tearing regime — and the witness must say so, or a boot that declines
    // continuously prints `TEAR-FREE` from whatever handful of composites did get staged. See
    // `wcg::stage_decline`.
    macro_rules! decline {
        ($reason:expr) => {{
            #[cfg(all(target_arch = "aarch64", feature = "witness"))]
            super::wcg::stage_decline(r.id, $reason);
            return false;
        }};
    }
    // WC-H FALLBACK FIXTURE (witness builds only) — take the direct path exactly once per boot, so
    // WC-D's scan-out read-back verifies the FALLBACK as well as the staged path. Before WC-H every
    // `[wc-d] verify` read a directly-drawn window; afterwards every one of them read a staged
    // present, and that coverage would have been traded away silently. The latch is global and
    // one-shot, and the gate runs two windows, so exactly one window is verified on each path.
    //
    // Armed on the composite WC-D is about to verify — the same predicate `composite_inner` tests
    // afterwards — so the fixture and the verification cannot land in different passes.
    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    {
        if r.presented && r.id < 32 {
            let bit = 1u32 << r.id;
            if VERIFIED.load(core::sync::atomic::Ordering::Relaxed) & bit == 0
                && !FALLBACK_FIXTURE.swap(true, core::sync::atomic::Ordering::Relaxed)
            {
                super::wcg::stage_decline(r.id, super::wcg::DECL_FIXTURE);
                return false;
            }
        }
    }
    let info = fb.info();
    let bpp = info.bytes_per_pixel;
    if bw == 0 || bh == 0 || bpp == 0 {
        decline!(super::wcg::DECL_GEOM);
    }
    // `bw <= pw` and `bh <= ph` (both clipped by the caller), so neither product can wrap.
    let row_bytes = bw * bpp;
    let need = row_bytes * bh;
    if need == 0 || need > MAX_STAGE_BYTES {
        decline!(super::wcg::DECL_CAP);
    }
    let mut stage = match STAGE.try_lock() {
        Some(g) => g,
        None => decline!(super::wcg::DECL_LOCK),
    };
    if stage.len() < need {
        let add = need - stage.len();
        // `try_reserve` + `resize`: an exhausted heap returns here instead of panicking from present
        // context. The buffer only ever grows, so a steady window size allocates exactly once.
        if stage.try_reserve(add).is_err() {
            decline!(super::wcg::DECL_ALLOC);
        }
        stage.resize(need, 0);
    }

    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    let t0 = crate::arch::aarch64::now_cycles();

    // The back layer: same pixel format and bytes/pixel as the panel (so the composed bytes ARE the
    // panel's bytes and the present is a straight copy), but its own stride — the box width, with no
    // panel margin — which is what makes each row a single contiguous run.
    let mut layer = super::FrameBuffer::new();
    layer.init(
        stage.as_mut_ptr() as usize,
        need,
        unaos_boot_info::FrameBufferInfo {
            width: bw,
            height: bh,
            stride: bw,
            bytes_per_pixel: bpp,
            pixel_format: info.pixel_format,
        },
    );
    paint_window(&layer, r, bx, by, bx, by, bw, bh, pw, ph, focused, true);

    // CURSOR-3 — the sprite, LAST into the layer and therefore on top of the window's own content,
    // and still before a single byte reaches the panel. `compose_into` declines (leaving the layer
    // exactly as `paint_window` left it) unless the sprite's box is wholly inside this box; it takes
    // no lock of the sprite module's, only the plan's own `try_lock`, so nothing here enters F4's
    // drain wait set. Counted either way, so a boot that never manages the overlay says so.
    if let Some(plan) = cur {
        let took = super::cursor::compose_into(&layer, bx, by, plan);
        *overlaid |= took;
        #[cfg(feature = "witness")]
        note_cursor_overlay(took);
    }

    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    let t1 = crate::arch::aarch64::now_cycles();

    // Present: one bulk copy per row. This is the whole of what the scan-out can catch mid-flight,
    // and it is the same primitive and the same shape as `Screen::present_background`'s damage-rect
    // flush.
    let fb_row = info.stride * bpp;
    for y in 0..bh {
        let src = y * row_bytes;
        fb.blit((by + y) * fb_row + bx * bpp, &stage[src..src + row_bytes]);
    }

    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    super::wcg::stage_note(
        r.id,
        bw,
        bh,
        need,
        crate::arch::aarch64::now_cycles(),
        t0,
        t1,
        ph,
    );
    true
}

/// WC-K — the back-layer path for a SOLID fill: compose one row of `color` in cached RAM, then
/// present the box to the front framebuffer as `h` contiguous row copies. Returns `false` when the
/// back layer is unavailable, leaving the front untouched so [`erase`] can run the direct fill.
///
/// ### Why one row and not the whole box
///
/// [`stage_window`] stages `bw x bh` because every one of those rows is different. A desktop fill's
/// rows are *identical by construction*, so the composed artifact that the present copies is a
/// single row — and the present is then byte-for-byte the same operation `stage_window` performs:
/// `h` bulk `copy_nonoverlapping` calls of `w * bpp` bytes at the panel's row stride, which is the
/// only part of the operation the scan-out can catch mid-flight. This is the same discipline, not a
/// third one: same buffer, same lock, same cap, same decline set, same present primitive. What it is
/// not is a full-box staging, and refusing to allocate `w * h` bytes to hold `h` copies of one row
/// is not a shortcut — it is what makes a full-panel erase (7.6 MB at the bench's 1920x1200) fit
/// under [`MAX_STAGE_BYTES`] at all instead of declining on the cap and falling straight back into
/// the tearing regime for the largest boxes, which are exactly the ones that tear worst.
///
/// The composed row is ZEROED before it is filled. `put_pixel` writes 3 of 4 bytes, so without this
/// the pad byte of the previous tenant of [`STAGE`] — a window's staged pixels — would be presented
/// into the desktop's pad. Invisible on this panel (the pad is not scanned), but the back layer's
/// "every pixel the present copies was written by this pass" invariant is load-bearing and cheap to
/// keep honest for one row.
fn stage_fill(fb: &super::FrameBuffer, x: usize, y: usize, w: usize, h: usize, color: u32) -> bool {
    // Every exit below is a DECLINE: this fill reaches the panel through the pre-WC-K direct
    // `fill_rect` — the tearing regime — and the witness must say so.
    macro_rules! decline {
        ($reason:expr) => {{
            #[cfg(all(target_arch = "aarch64", feature = "witness"))]
            super::wcg::erase_decline(w, h, $reason);
            return false;
        }};
    }
    let info = fb.info();
    let bpp = info.bytes_per_pixel;
    if w == 0 || h == 0 || bpp == 0 {
        decline!(super::wcg::DECL_GEOM);
    }
    // Caller clipped `(x, y, w, h)` to the panel, so neither product can wrap.
    let row_bytes = w * bpp;
    let fb_row = info.stride * bpp;
    if row_bytes == 0 || row_bytes > MAX_STAGE_BYTES {
        decline!(super::wcg::DECL_CAP);
    }
    let mut stage = match STAGE.try_lock() {
        Some(g) => g,
        None => decline!(super::wcg::DECL_LOCK),
    };
    if stage.len() < row_bytes {
        let add = row_bytes - stage.len();
        // Same `try_reserve` + `resize` contract as `stage_window`: an exhausted heap declines here
        // rather than panicking from a close path.
        if stage.try_reserve(add).is_err() {
            decline!(super::wcg::DECL_ALLOC);
        }
        stage.resize(row_bytes, 0);
    }

    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    let t0 = crate::arch::aarch64::now_cycles();

    // Compose: one row, in the panel's own pixel format so the present is a straight copy.
    for b in stage[..row_bytes].iter_mut() {
        *b = 0;
    }
    let mut layer = super::FrameBuffer::new();
    layer.init(
        stage.as_mut_ptr() as usize,
        row_bytes,
        unaos_boot_info::FrameBufferInfo {
            width: w,
            height: 1,
            stride: w,
            bytes_per_pixel: bpp,
            pixel_format: info.pixel_format,
        },
    );
    layer.fill_rect(0, 0, w, 1, color);

    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    let t1 = crate::arch::aarch64::now_cycles();

    // Present: one bulk copy per row. ROW-CONTIGUITY is checked here rather than asserted in a
    // comment — each destination run must be exactly `row_bytes` long, must fit inside its scanline
    // (`x * bpp + row_bytes <= fb_row`, so no run wraps into the next row), and consecutive runs must
    // step by exactly one panel row. A `contig=no` would mean the present is no longer the shape
    // WC-H's tear-free argument rests on, whatever the timings say.
    let mut contig = x * bpp + row_bytes <= fb_row;
    let mut prev = usize::MAX;
    for r in 0..h {
        let off = (y + r) * fb_row + x * bpp;
        if prev != usize::MAX && off != prev + fb_row {
            contig = false;
        }
        prev = off;
        fb.blit(off, &stage[..row_bytes]);
    }

    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    super::wcg::erase_note(
        w,
        h,
        row_bytes,
        contig,
        crate::arch::aarch64::now_cycles(),
        t0,
        t1,
        info.height,
    );
    // Read on every build so the contiguity check is not compiled out of the non-witness kernels: the
    // check is cheap and its absence would make the two builds structurally different here.
    let _ = contig;
    true
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

// ---- FOCUS-VIS witness -------------------------------------------------------------------------

/// FOCUS-VIS witness surfaces: two solid 8x8 ARGB8888 patches in kernel rodata. Static rather than
/// heaped so the selftest allocates nothing on a path that runs from a boot task, and READ-ONLY because
/// the compositor only ever reads a window's surface.
///
/// The colours are chosen to be distinguishable from each other AND from [`DESKTOP_BG`], because
/// telling those three apart at one pixel is the entire verdict.
#[cfg(feature = "witness")]
static FV_SURF_A: [u32; 64] = [0x00FF_2020; 64];
#[cfg(feature = "witness")]
static FV_SURF_B: [u32; 64] = [0x0020_FF20; 64];

/// FOCUS-VIS — the RAISE-IS-VISIBLE witness. Four legs, each a scan-out READ-BACK at one pixel: the
/// content origin of two windows placed at the SAME point, so exactly one of them can own that pixel and
/// the panel itself says which.
///
/// ### Why a read-back, and why at that pixel
/// Every existing focus witness is a statement about kernel state — `[wc-c] focus tab-cycle` prints the
/// ring transition, and it printed correctly on the bench for a panel that never changed. That is the
/// precise failure this witness is built to catch: it never asks the table who is in front, it asks the
/// FRAMEBUFFER what colour is actually there. Two windows at one origin turn "is the focused window
/// frontmost?" into a single equality, with no tolerance and nothing to interpret.
///
/// ### The four legs
/// 1. **stack** — B created after A, so B is in front: the pixel is B's colour. (Baseline; if this fails
///    the other three prove nothing about focus.)
/// 2. **raise** — `focus_changed(A)`: the pixel becomes A's colour. This is defect 1 of the arc.
/// 3. **shell** — `focus_changed(0)`: the pixel is neither window's colour. The shell took the top of
///    the z-order, both windows dropped below it and their boxes were erased. This is the "TAB to the
///    shell and READ your output" case, reduced to the one thing a headless gate can check — that the
///    window layer stopped owning those pixels.
/// 4. **reraise** — `focus_changed(B)`: B comes back from under the shell. Proves the shell is a
///    POSITION in the rotation and not a terminus for the window layer.
///
/// Self-cleaning: both windows are closed at the end, which erases their boxes to the desktop colour and
/// recomposites, so the panel is left as it was found. Placement is explicit (`move_to`, which pins the
/// rows against the tiler) at a point clear of WC-F's reserved probe boxes — those sit at the BOTTOM of
/// the panel, so an upper-middle origin cannot collide with them at either gate or bench geometry.
///
/// `witness`-gated and one-shot. Runs on the real panel, so it must be invoked AFTER the one-shot
/// `[wc-c]`/`[wc-d]` window witnesses have fired, or it would burn their latches with its own rows.
#[cfg(feature = "witness")]
pub fn focusvis_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        serial_println!("[wc-fv] focus-vis -> SKIP (framebuffer not ready)");
        return;
    }
    let info = fb.info();
    if info.width < 128 || info.height < 128 {
        serial_println!(
            "[wc-fv] focus-vis -> SKIP (panel {}x{} too small)",
            info.width, info.height
        );
        return;
    }

    const ASID_A: u64 = 0xF0A;
    const ASID_B: u64 = 0xF0B;
    let a_col = FV_SURF_A[0];
    let b_col = FV_SURF_B[0];
    let sa = &raw const FV_SURF_A as usize;
    let sb = &raw const FV_SURF_B as usize;
    let len = core::mem::size_of_val(&FV_SURF_A);

    let wa = create(ASID_A, sa, len, 8, 8, 32, b"fv-a");
    let wb = create(ASID_B, sb, len, 8, 8, 32, b"fv-b");
    if wa == WIN_NONE || wb == WIN_NONE {
        serial_println!("[wc-fv] focus-vis -> SKIP (window table full: a={} b={})", wa, wb);
        close(wa);
        close(wb);
        return;
    }

    // One origin for both, upper-middle: WC-F's reserved boxes hug the bottom edge, and the tiler is
    // disarmed for these rows by `move_to`'s pin.
    let ox = info.width / 3;
    let oy = info.height / 4 + TITLE_H + BORDER;
    move_to(wa, ox, oy);
    move_to(wb, ox, oy);
    // The rows carry content from creation (static surfaces), so a present is the honest way to say
    // "this content is the owner's" — it is also what arms WC-D for these ids.
    present(wa);
    present(wb);

    // The probe pixel: one scaled block inside the content origin, clear of the 1-px chrome border.
    let (px, py) = (ox + 1, oy + 1);
    let read = || super::WRITER.lock().read_pixel(px, py).unwrap_or(0);

    let got_stack = read();
    focus_changed(ASID_A);
    let got_raise = read();
    focus_changed(0);
    let got_shell = read();
    focus_changed(ASID_B);
    let got_reraise = read();

    let stack_ok = got_stack == b_col;
    let raise_ok = got_raise == a_col;
    let shell_ok = got_shell != a_col && got_shell != b_col;
    let reraise_ok = got_reraise == b_col;
    let ok = stack_ok && raise_ok && shell_ok && reraise_ok;
    serial_println!(
        "[wc-fv] focus-vis at ({},{}) a={:#08x} b={:#08x} stack={:#08x}/{} raise={:#08x}/{} shell={:#08x}/{} reraise={:#08x}/{} -> {}",
        px, py, a_col, b_col,
        got_stack, stack_ok, got_raise, raise_ok, got_shell, shell_ok, got_reraise, reraise_ok,
        if ok { "PASS" } else { "FAIL" }
    );

    close(wa);
    close(wb);
    // Leave the shell where the operator's boot left it: at the bottom, so ordinary windows composite.
    // (Not a strict necessity — `next_z` only moves forward, so any window created after this is above
    // it either way; this keeps the invariant readable rather than relying on that.)
    SHELL_Z.store(0, Ordering::Release);
    // FOCUS-HL: restore the focus owner for the same reason, and in the same breath. This selftest calls
    // `focus_changed` with SYNTHETIC ASIDs (0xF0A/0xF0B), so it leaves `FOCUS_ASID` naming an address
    // space that does not exist and never will. It is inert in practice — no real ASID can collide with
    // those values, so no window is wrongly highlighted — but it makes the documented invariant ("0 means
    // the shell holds focus") false for the rest of the boot, and the `repaint` immediately below would
    // composite the whole live set against a focus owner that is pure fiction. Restoring it costs one
    // store and keeps the selftest's footprint at zero, which is what a selftest owes.
    FOCUS_ASID.store(0, Ordering::Release);
    // ...and REPAINT, because the store alone leaves a hole. This selftest's `focus_changed(0)` leg
    // pushed EVERY live window below the shell, not only its own two: a window belonging to a real
    // backgrounded app was erased to the desktop colour, and `composite_inner` consumed its damage flag
    // on the way past (damage is cleared under the table lock, BEFORE the `above_shell` skip). So
    // dropping the shell back to 0 makes those windows drawable again while nothing is left marked
    // damaged to actually draw them — a blank rectangle on the panel until WC-E's next desktop flush
    // happened to heal it. `repaint` re-damages the whole live set and composites, which is exactly the
    // restore this owes; it is a no-op on the gate, where the table is empty by now.
    repaint();
}

/// WC-I — the CLOSE→REOPEN witness: does a surviving window still reach the panel after a SIBLING
/// window is closed and its table slot is recycled?
///
/// ### Why this exists
/// The P60 bench report has a third face: after killing a vug and relaunching it, the relaunched
/// window shows EMPTY and no further vug displays. One hypothesis put to this arc was that WC-H's
/// back layer carries per-window state that teardown fails to rebind, so a recycled slot presents
/// into a dead layer. This is the falsifier for the compositor half of that hypothesis, and it is
/// drivable in QEMU (unlike the 1 Hz blip, which needs the bench's timing, or the cursor, which needs
/// HID): close a window, recycle its slot with a new one, and then read the SCAN-OUT back at the
/// content origin of the window that survived and of the window that took the freed slot.
///
/// Read-back at a pixel, for the same reason `focusvis_selftest` uses one: every other instrument
/// here reports kernel state, and kernel state was correct in the bench report — `[wc-d] verify`
/// printed `PASS` with `nonzero=262144` for a window the operator saw as empty. Only the framebuffer
/// can contradict that, so only the framebuffer is asked.
///
/// ### The three legs
/// 1. **both** — A and B live and presented: each origin holds its own colour. (Baseline.)
/// 2. **reopen** — close A, create C into the freed slot, present it: C's origin holds C's colour.
///    This is the "relaunched window is empty" case at the compositor layer.
/// 3. **survivor** — B, which was never touched, still holds B's colour after the close and the
///    recycle. This is the "no further vug displays" case.
///
/// Self-cleaning and one-shot, on the same terms as `focusvis_selftest`: explicit placement clear of
/// WC-F's reserved boxes, both survivors closed at the end, and the live set repainted.
#[cfg(feature = "witness")]
pub fn reopen_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        serial_println!("[wc-i] reopen -> SKIP (framebuffer not ready)");
        return;
    }
    let info = fb.info();
    if info.width < 256 || info.height < 128 {
        serial_println!("[wc-i] reopen -> SKIP (panel {}x{} too small)", info.width, info.height);
        return;
    }

    const ASID_A: u64 = 0xE0A;
    const ASID_B: u64 = 0xE0B;
    const ASID_C: u64 = 0xE0C;
    let sa = &raw const FV_SURF_A as usize;
    let sb = &raw const FV_SURF_B as usize;
    let len = core::mem::size_of_val(&FV_SURF_A);
    let (a_col, b_col) = (FV_SURF_A[0], FV_SURF_B[0]);

    let wa = create(ASID_A, sa, len, 8, 8, 32, b"re-a");
    let wb = create(ASID_B, sb, len, 8, 8, 32, b"re-b");
    if wa == WIN_NONE || wb == WIN_NONE {
        serial_println!("[wc-i] reopen -> SKIP (window table full: a={} b={})", wa, wb);
        close(wa);
        close(wb);
        return;
    }
    // Two distinct origins, upper-middle, clear of WC-F's bottom-edge probe boxes and of each other.
    let oy = info.height / 4 + TITLE_H + BORDER;
    let (ax, bx) = (info.width / 4, info.width / 2);
    move_to(wa, ax, oy);
    move_to(wb, bx, oy);
    present(wa);
    present(wb);
    let read = |x: usize, y: usize| super::WRITER.lock().read_pixel(x, y).unwrap_or(0);
    let both_ok = read(ax + 1, oy + 1) == a_col && read(bx + 1, oy + 1) == b_col;

    // Recycle A's slot. `close` frees the row and `create` takes the lowest free one, so C lands on
    // exactly the slot A vacated — which is the id-aliasing the bench report's `close win=1` /
    // `create win=1` pair shows, reproduced deliberately.
    close(wa);
    let wc = create(ASID_C, sa, len, 8, 8, 32, b"re-c");
    if wc == WIN_NONE {
        serial_println!("[wc-i] reopen -> SKIP (no row for the reopened window)");
        close(wb);
        return;
    }
    move_to(wc, ax, oy);
    present(wc);
    let reopen_ok = read(ax + 1, oy + 1) == a_col;
    let survivor_ok = read(bx + 1, oy + 1) == b_col;

    let ok = both_ok && reopen_ok && survivor_ok;
    serial_println!(
        "[wc-i] reopen closed={} reopened={} survivor={} both={} reopen={} survivor_px={} -> {}",
        wa, wc, wb, both_ok, reopen_ok, survivor_ok,
        if ok { "PASS" } else { "FAIL" }
    );

    close(wb);
    close(wc);
    repaint();

    // WC-J: the VACATE read-back, invoked here (rather than from the arch selftest driver) because it
    // has exactly this witness's preconditions — every one-shot per-window latch already spent, the
    // shell z restored, the live set repainted — and because a close/damage question belongs to the
    // window layer, not to the syscall layer that happens to sequence the video selftests.
    vacate_selftest();
}

/// WC-J — the CLOSE→VACATE witness: do the rows a closed window occupied come back as DESKTOP?
///
/// ### Why this exists (P61, bench, attended)
/// Several background vugs were launched and some killed. The operator reported one crash, two
/// FROZEN windows and one still running; `jobs` then showed all four pids `exited 0 (reaped)`. The
/// process story was clean, so the "frozen" windows were not frozen processes — they were panel
/// pixels belonging to windows whose owners had already exited and been reaped. A GHOST is a window
/// layer defect by construction: nothing alive was drawing those pixels.
///
/// Before WC-I the desktop ran a blanket `wm::repaint()` every tick, which re-blitted the whole live
/// set and, on the desktop's own present, re-copied the background — so a vacated box healed within a
/// second whether or not the close path had actually reclaimed it. WC-I removed that blanket pass in
/// favour of `service_damage` plus occlusion subtraction, which makes the reclaim load-bearing: if a
/// close frees the row without restoring the pixels, nothing else ever will.
///
/// ### The legs (both closes that a dying app can take)
/// 1. **close** — the explicit `SYS_WIN_CLOSE` path (`wc_shim::destroy` → [`close`]).
/// 2. **owner** — the EXIT-TEARDOWN path (`clear_handle_row` → [`close_owner`]), which is the exact
///    P61 scenario: a backgrounded app reaches its exit with a window still open.
///
/// Each leg presents a window, proves the panel took its colour (a vacate check over a box that was
/// never painted proves nothing), closes it, and then reads the vacated box back at five points — the
/// content origin, the two content diagonals and both chrome bands (title strip and lower border),
/// since chrome is kernel-drawn and leaks exactly as visibly as content. Every point must equal
/// [`DESKTOP_BG`], byte-for-byte, with no tolerance.
#[cfg(feature = "witness")]
pub fn vacate_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        serial_println!("[wc-j] vacate -> SKIP (framebuffer not ready)");
        return;
    }
    let info = fb.info();
    if info.width < 256 || info.height < 128 {
        serial_println!("[wc-j] vacate -> SKIP (panel {}x{} too small)", info.width, info.height);
        return;
    }

    const ASID_J: u64 = 0xE0D;
    let sa = &raw const FV_SURF_A as usize;
    let len = core::mem::size_of_val(&FV_SURF_A);
    let a_col = FV_SURF_A[0];
    let read = |x: usize, y: usize| super::WRITER.lock().read_pixel(x, y).unwrap_or(0);

    // Upper-middle, clear of WC-F's bottom-edge reserved probe boxes, as the sibling witnesses are.
    let oy = info.height / 4 + TITLE_H + BORDER;
    let ox = info.width / 4;

    // One leg: place a window at (ox, oy), prove it owns its box, close it by `f`, and count how many
    // of the five sample points came back as desktop.
    let leg = |f: &dyn Fn(WinId, u64), name: &str| -> (bool, bool, usize) {
        let w = create(ASID_J, sa, len, 8, 8, 32, b"vc-a");
        if w == WIN_NONE {
            return (false, false, 0);
        }
        move_to(w, ox, oy);
        present(w);
        // The outer box, recomputed from the row so the sample points track the real chrome geometry.
        let b = match info_box(w) {
            Some(b) => b,
            None => {
                close(w);
                return (false, false, 0);
            }
        };
        let painted = read(ox + 1, oy + 1) == a_col;
        f(w, ASID_J);
        // Five points: content origin, both content diagonals, the title strip and the lower border.
        let pts = [
            (ox + 1, oy + 1),
            (ox + 2, oy + 2),
            (ox + 5, oy + 5),
            (b.0 + b.2 / 2, b.1 + TITLE_H / 2),
            (b.0 + b.2 / 2, b.1 + b.3 - 1),
        ];
        let mut clean = 0usize;
        for &(x, y) in pts.iter() {
            if read(x, y) == DESKTOP_BG {
                clean += 1;
            }
        }
        let _ = name;
        (painted, clean == pts.len(), clean)
    };

    let (p1, v1, n1) = leg(&|w, _| { close(w); }, "close");
    let (p2, v2, n2) = leg(&|_, asid| { close_owner(asid); }, "owner");

    let ok = p1 && v1 && p2 && v2;
    serial_println!(
        "[wc-j] vacate close_painted={} close_desktop={} ({}/5) owner_painted={} owner_desktop={} ({}/5) -> {}",
        p1, v1, n1, p2, v2, n2,
        if ok { "PASS" } else { "FAIL" }
    );
    retile_selftest();
    repaint();
}

/// WC-J — the RETILE-GHOST witness: when one window closes, do the SURVIVORS the tiler moves leave
/// their previous frame behind?
///
/// ### Why this is the P61 shape and the single-window vacate is not
/// [`vacate_selftest`]'s two legs place their window explicitly (`move_to`, which PINS the row) and
/// close it alone, so the only box in play is the one the closer erases. A real app's window is never
/// pinned — nothing in EL0 moves a window, so every real window is laid out by the TILER — and a
/// tiled window's position is a function of HOW MANY windows exist. `close` calls `place(WIN_NONE)`
/// after freeing the row, which re-tiles every surviving unpinned window into the compacted layout:
/// the survivors MOVE, and the closer erases only the box the CLOSED window vacated, never the boxes
/// the survivors vacated by moving.
///
/// [`move_to`] already knows this (FOCUS-VIS gave it an erase of its old box); the tiler does not.
/// Before WC-I the desktop's per-tick blanket present covered the difference within a second. With
/// WC-I the desktop presents only its own damage and nothing marks those rows, so an abandoned tile
/// is permanent — a full copy of a window standing where the window no longer is. That is what the
/// operator reads as a FROZEN window, and it is why P61 saw several of them the moment one bg vug of
/// four was killed while all four processes were provably clean.
///
/// The leg: two tiled windows, both presented; note where B is; close A; B must have MOVED (otherwise
/// the leg proves nothing about the tiler), B's NEW box must hold B's colour, and B's OLD box must be
/// back to [`DESKTOP_BG`].
#[cfg(feature = "witness")]
fn retile_selftest() {
    const ASID_A: u64 = 0xE1A;
    const ASID_B: u64 = 0xE1B;
    let sa = &raw const FV_SURF_A as usize;
    let sb = &raw const FV_SURF_B as usize;
    let len = core::mem::size_of_val(&FV_SURF_A);
    let b_col = FV_SURF_B[0];
    let read = |x: usize, y: usize| super::WRITER.lock().read_pixel(x, y).unwrap_or(0);

    let wa = create(ASID_A, sa, len, 8, 8, 32, b"rt-a");
    let wb = create(ASID_B, sb, len, 8, 8, 32, b"rt-b");
    if wa == WIN_NONE || wb == WIN_NONE {
        serial_println!("[wc-j] retile -> SKIP (window table full: a={} b={})", wa, wb);
        close(wa);
        close(wb);
        return;
    }
    present(wa);
    present(wb);
    let before = match info_box(wb) {
        Some(b) => b,
        None => {
            serial_println!("[wc-j] retile -> SKIP (no row for the survivor)");
            close(wa);
            close(wb);
            return;
        }
    };
    // Sample the survivor's CONTENT origin, not its chrome: chrome is redrawn identically at the new
    // position, so a chrome pixel cannot distinguish "moved" from "still there".
    let (bx0, by0) = (before.0 + BORDER, before.1 + TITLE_H);
    let painted = read(bx0 + 1, by0 + 1) == b_col;

    close(wa);

    let after = info_box(wb).unwrap_or(before);
    let moved = after != before;
    // The survivor still reaches the panel at its NEW box...
    let live_ok = read(after.0 + BORDER + 1, after.1 + TITLE_H + 1) == b_col;
    // ...and its OLD box is desktop again. Three points inside the abandoned content area.
    let mut clean = 0usize;
    let pts = [(bx0 + 1, by0 + 1), (bx0 + 2, by0 + 2), (bx0 + 5, by0 + 5)];
    for &(x, y) in pts.iter() {
        if read(x, y) == DESKTOP_BG {
            clean += 1;
        }
    }
    let ghost_free = !moved || clean == pts.len();
    let ok = painted && live_ok && ghost_free;
    serial_println!(
        "[wc-j] retile survivor={} moved={} painted={} live={} old_desktop={} ({}/3) -> {}",
        wb, moved, painted, live_ok, ghost_free, clean,
        if ok { "PASS" } else { "FAIL" }
    );
    close(wb);
}

/// The outer (chrome-inclusive) panel box of `id`, or `None` if `id` names no live window.
#[cfg(feature = "witness")]
fn info_box(id: WinId) -> Option<(usize, usize, usize, usize)> {
    let t = TABLE.lock();
    row(&t, id).map(outer_box)
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
    // WC-J — a CREATE re-tiles the existing windows exactly as a close does (the layout is a function
    // of the live set), so it abandons their old boxes in the same way and they are reclaimed the same
    // way. No drain barrier here: no row is being freed and no surface unmapped, so there is nothing an
    // in-flight blit could be reading that is about to disappear — the composite below re-establishes
    // every moved window over the erase.
    let (nv, moved) = place(id);
    reclaim(&moved[..nv]);
    #[cfg(feature = "witness")]
    if !compat {
        if let Some(i) = info(id) {
            serial_println!(
                "[wc-a] create win={} asid={:#x} surf={}x{} stride={} scale={}x at ({},{}) z={}",
                i.id, i.owner_asid, i.w, i.h, stride, i.scale, i.x, i.y, i.z
            );
        }
    }
    // FOCUS-VIS / CURSOR — composite the creation, instead of leaving the new row to the owner's first
    // present. Two things follow, and the second is the one the P59 bench report is about:
    //  * the window's kernel chrome is on the panel the moment the row exists, so `create` is visible
    //    even for an app that maps a surface and then blocks before presenting;
    //  * the create runs INSIDE the cursor bracket (`composite` = undraw -> paint -> repaint). Before
    //    this, `create` -> `place` mutated the layout and the FIRST thing to touch the panel afterwards
    //    was the owner's `draw_window`, on another core, with the sprite still down and its save-under
    //    holding pre-window pixels. The sprite came back only if the pointer happened to move again.
    //    Now every window creation ends with the cursor re-saved and re-drawn on top.
    // Called with the table lock released (`place` released it) — `composite` takes it itself.
    //
    // NOT on the compat path: `create_inner` is only the first half of `compat_present`, which sets the
    // row's real scale and centred origin in a SECOND critical section and composites itself. A
    // composite here would blit that surface once at the row's defaults (1x at the origin) — a visible
    // flash of mis-scaled content, for a row that is about to be composited correctly anyway.
    if !compat {
        composite();
    }
    id
}

/// Choose `id`'s scale and on-panel origin. WC-A2 tiles windows left-to-right; the compat window is
/// placed by the legacy centering rule at composite time and is skipped here.
/// WC-A — gap in panel pixels between tiled windows (and from the panel edge).
const GAP: usize = 8;

/// WC-SCALE — the LEGIBILITY CEILING on a window's integer upscale, for a panel `ph` pixels tall.
///
/// The fit rule alone ("largest factor fitting half the panel") is a function of the *surface*, so the
/// smaller the surface the larger the magnification: midden's 24x16 status readout landed at `scale=37x`
/// on a 1920x1200 panel — a handful of 3x5 glyphs blown up to ~100 px per font pixel, which reads as an
/// abstract pattern rather than as digits. Past a point, magnification stops adding legibility and starts
/// removing it, and the surface's own content (bitmap glyphs, 1-px rules) is what sets that point.
///
/// THE METRICS RULE: no absolute pixel constant here. [`ui::SCALE_MAX`] is the kernel's existing answer to
/// exactly this question for text — *"beyond 4x legibility gains nothing and glyph blocks get blocky-huge"* —
/// and [`ui::Metrics::for_height`] is how every other UI dimension tracks the panel. So the ceiling is
/// `SCALE_MAX` font-scale steps: `SCALE_MAX * metrics.scale`, i.e. 4x on a ≤1799-row panel and 8x on an
/// 1800p-class one, growing with panel density the same way the console's type does.
///
/// This is the same *kind* of cap `screen::present_surface` applies on the compat path (there: "~40% of the
/// panel's shorter dimension", a window-size ceiling); that path keeps its own rule, because a compat
/// surface is a full-screen app's canvas and wants to be as big as it can comfortably be, whereas a tiled
/// window is one of several and wants to be as *readable* as it can be.
///
/// Effect on the existing witness geometry (nothing here perturbs a checksum — `cksum` is FNV over the
/// SOURCE `surf_len`, which no scale change touches): on the 640x480 gate panel the 128x128 window stays
/// 1x and the 64x64 stays 3x, both already under the cap; on the 1920x1200 bench panel the 128x128 window
/// stays 4x, the 64x64 comes down 9x → 4x, and the 24x16 comes down 37x → 4x.
fn legibility_cap(ph: usize) -> usize {
    crate::ui::SCALE_MAX
        .saturating_mul(crate::ui::Metrics::for_height(ph).scale)
        .max(1)
}

/// Lay out every non-compat, non-pinned window: pick each one's integer scale, then pack the outer
/// boxes left-to-right in id order, wrapping to a new row when the next box would run off the panel.
/// Called whenever the window set changes, so the tiling stays deterministic (it depends only on the
/// live set, not on the order of creates and closes). A window the caller has explicitly [`move_to`]d
/// is pinned and keeps its position.
///
/// Scale rule: the largest integer factor whose scaled surface fits half the panel width and half its
/// height — big enough that a 32x32 surface is legible on a 1920-wide panel, small enough that two
/// windows sit side-by-side — **capped by [`legibility_cap`]**. Never 0.
fn place(_created: WinId) -> (usize, [(usize, usize, usize, usize); MAX_WINDOWS]) {
    let mut vacated = [(0usize, 0usize, 0usize, 0usize); MAX_WINDOWS];
    let mut nv = 0usize;
    // Read the panel geometry BEFORE taking the table lock: `composite` takes the table lock and
    // releases it before touching `WRITER`, so no path ever holds both — no lock-order inversion.
    // The WRITER guard is intentionally dropped at the end of this statement (`FrameBuffer` is
    // `Copy`); the table lock below is therefore never nested inside it.
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return (0, vacated);
    }
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);

    let cap = legibility_cap(ph);

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
        let scale = ((pw / 2 / w).min(ph / 2 / h)).min(cap).max(1);
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
        // WC-J — the box this row occupied BEFORE the tiler moved it. A tiled window's position is a
        // function of how many windows exist, so every create and every close re-tiles the survivors;
        // the pixels they leave behind belong to nobody afterwards, and nothing else in this module
        // knows they were abandoned. Recorded here (the one place that can see both geometries) and
        // reclaimed by `reclaim` at every `place` call site.
        let before = outer_box(r);
        r.scale = scale;
        r.x = cx + BORDER;
        r.y = cy;
        r.damaged = true;
        if outer_box(r) != before && before.2 != 0 && before.3 != 0 {
            vacated[nv] = before;
            nv += 1;
        }
        cx = cx.saturating_add(bw).saturating_add(GAP);
        row_h = row_h.max(bh);
    }
    drop(t);
    (nv, vacated)
}

/// WC-J — RECLAIM panel rows the window layer has stopped owning: paint them desktop, re-damage the
/// windows the paint reached, and ask the desktop for its own content back.
///
/// ### Why this is a separate step and not "the closer erases its box"
/// A close reclaims two disjoint things. The box the CLOSED window occupied — which [`close`] and
/// [`close_owner`] already erase — and the boxes the SURVIVING windows vacate when [`place`] re-tiles
/// them into the compacted layout, which nothing erased. The second set is invisible to every caller:
/// only the tiler sees both the old and the new geometry, so only the tiler can report it.
///
/// ### Why it became permanent (P61)
/// Before WC-I the desktop presented its whole damage set every tick and `wm::repaint` re-blitted the
/// whole live set on top, so an abandoned tile was overwritten within about a second whether or not
/// anything had reclaimed it. WC-I subtracts the window layer from the desktop's damage and drops the
/// blanket re-blit — correctly, that is what removed the 1 Hz blip — and with it the accident that was
/// covering this. The abandoned tile now belongs to nobody: not to the window (which moved), not to
/// the desktop (whose damage for those rows was discarded while a window sat there). It stays on the
/// panel for the rest of the boot, showing the last frame of a window that is no longer there — which
/// is exactly what the P61 operator read as a FROZEN vug while `jobs` showed every pid already exited
/// and reaped.
///
/// ### The three steps, and why all three
/// * **erase** — desktop colour on the panel NOW, so the ghost is gone within this call rather than at
///   the desktop's next tick. Same immediate-response argument `focus_changed` makes for its hidden
///   boxes.
/// * **damage_intersecting** — a survivor whose box OVERLAPS a reclaimed one just had a bite taken out
///   of it by the erase; it must be repainted by the composite the caller runs next.
/// * **request_full_present** — the desktop's own content (the console's text, the status strip) is
///   what actually belongs under a departed window, and only the desktop can put it there: the erase
///   above can only paint [`DESKTOP_BG`]. The flag is the mechanism FOCUS-VIS already built for
///   precisely this hand-off, and it is consumed by the render task's next flush, on its own thread.
///   Not raised when there is nothing to reclaim, so an ordinary windowless boot never asks for one.
///
/// Callers run this INSIDE their drain barrier (the pixels must not race an in-flight blit of the old
/// geometry) and composite after dropping it.
fn reclaim(vacated: &[(usize, usize, usize, usize)]) {
    if vacated.is_empty() {
        return;
    }
    erase(vacated);
    for &(x, y, w, h) in vacated.iter() {
        damage_intersecting(x, y, w, h);
    }
    super::screen::request_full_present();
}
