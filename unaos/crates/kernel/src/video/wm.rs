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
//! Every user program renders into its OWN off-screen ARGB8888 surface and never touches the real
//! scan-out; the kernel owns the panel. This module is the seam between "a task has a surface" and
//! "pixels reach the HVS": a fixed table of at most [`MAX_WINDOWS`] windows (id, owner ASID,
//! geometry, z-order, surface pointer/stride, damage flag, short title) plus a back-to-front
//! composite pass that blits the damaged windows onto the framebuffer with a per-window integer
//! upscale and kernel-drawn chrome (a 1-px border and a title strip).
//!
//! **Chrome is kernel-drawn, always.** An app draws only inside its own surface; the border and the
//! title strip are painted by the compositor from the kernel's copy of the title. A user program
//! therefore cannot forge another window's frame, and the presentation-modes law ("never fake host
//! chrome") is enforced structurally rather than by convention.
//!
//! **Composite on present, no thread.** [`present`] marks a window damaged and immediately runs the
//! composite pass from the presenting task's own context (the same discipline
//! [`super::screen::present_surface`] has always used: the render task is parked while a full-screen
//! user program owns the panel, so routing through it would present nothing). There is no compositor
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
//!   mapping code, never from user-supplied dimensions; it is what bounds every source read the
//!   compositor performs.
//! - [`close_compat`] must be called from the user teardown seam next to [`close_owner`]. The compat
//!   window has no owner ASID (the `SYS_FB_PRESENT` hook signature carries none), so `close_owner`
//!   can never reap it.
//!
//! **Untrusted geometry.** `w`, `h`, `stride`, and the [`move_to`] origin may all come from an app.
//! They are validated against the slot at create time, clamped to the panel at move time, saturating
//! everywhere in between, and every composite loop is clipped to the panel intersection BEFORE it
//! runs — `put_pixel` clips writes but would still iterate a hostile extent. The kernel builds
//! without overflow checks, so wrapping arithmetic is a real failure mode here, not a theoretical
//! one.

use spin::relax::Spin as SpinRelax;
use spin::{Mutex, MutexGuard};

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
    /// kernel-visible address of the owner's ARGB8888 surface. Held as a `usize` so the table is `Send`.
    surf: usize,
    /// F1 — length in BYTES of the mapped surface slot at `surf`. The compositor never reads past
    /// `surf + surf_len`; [`create`] rejects any geometry that could. Without this the extent came
    /// from user mode and a `w=h=10000, stride=40000` window over a 4 KiB slot would have the compositor
    /// read ~400 MB of kernel memory and paint kernel bytes onto the panel (`put_pixel` clips WRITES,
    /// never the source READ).
    surf_len: usize,
    scale: usize,
    z: u32,
    damaged: bool,
    /// FBCON-DMG — the SOURCE-ROW band `[dmg_y0, dmg_y1)` of this window's surface that is known
    /// damaged, when the damage arrived through [`present_rows`]. An EMPTY band (`dmg_y1 <= dmg_y0`,
    /// which is what [`Window::empty`] and every [`Window::damage_all`] site leave behind) means THE
    /// WHOLE BOX — so a window that never takes a banded present behaves exactly as it did before.
    ///
    /// Source rows and not panel rows, because that is the coordinate the *owner* of the surface
    /// knows: the console tracks a dirty band in its own glyph grid and has no business re-deriving
    /// the compositor's placement. The conversion happens once, through the same `r.y`/`r.scale` the
    /// content blit uses (see [`damaged_box`]), so the band can never disagree with the pixels it
    /// describes.
    dmg_y0: usize,
    dmg_y1: usize,
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
            dmg_y0: 0,
            dmg_y1: 0,
            title: [0u8; MAX_TITLE],
            title_len: 0,
            compat: false,
            pinned: false,
            presented: false,
        }
    }

    /// FBCON-DMG — declare the WHOLE outer box damaged. This is what `r.damaged = true` meant before
    /// the band existed, and every damage path except [`present_rows`] still means exactly it: an
    /// empty band reads as "the whole box", so clearing the band here is what keeps a chrome repaint,
    /// a move, a raise, a focus change and a `damage_intersecting` unconditionally whole-box even
    /// when a banded present is already pending on the same row.
    fn damage_all(&mut self) {
        self.damaged = true;
        self.dmg_y0 = 0;
        self.dmg_y1 = 0;
    }

    /// FBCON-DMG — declare SOURCE rows `[y0, y1)` damaged, widening whatever band is already pending.
    ///
    /// Fail-safe in the direction that cannot lose a pixel: a band already standing for the whole box
    /// (empty, i.e. an unserviced [`Window::damage_all`]) STAYS the whole box, and an empty or
    /// inverted argument is likewise promoted to the whole box rather than silently narrowing
    /// anything.
    fn damage_rows(&mut self, y0: usize, y1: usize) {
        if y1 <= y0 {
            self.damage_all();
            return;
        }
        if self.damaged && self.dmg_y1 <= self.dmg_y0 {
            return; // a whole-box repaint is already owed; it covers these rows.
        }
        if self.damaged {
            self.dmg_y0 = self.dmg_y0.min(y0);
            self.dmg_y1 = self.dmg_y1.max(y1);
        } else {
            self.dmg_y0 = y0;
            self.dmg_y1 = y1;
        }
        self.damaged = true;
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

/// WEDGE-7 / F1 — **the sole acquisition path for [`TABLE`].** Masks IRQs for exactly the lifetime
/// of the guard.
///
/// ## The defect this closes
/// `TABLE` was acquired unmasked at 23 sites while `WINDOWS` (`arch::syscall`) is acquired MASKED at
/// all 8 of its sites, and a masked span reaches `TABLE` through `wm::present`. That asymmetry is a
/// deadlock on ONE core, with no ABBA cycle anywhere — the documented order `WINDOWS ⊃ TABLE ⊃
/// WRITER` is honoured throughout, which is why no lock-ordering discipline addresses it:
///
/// 1. `render` (pinned, `PRIO_SERVICE`) takes `TABLE` unmasked inside `service_damage`/`composite`.
/// 2. The timer PPI preempts it mid-hold — `timer_preempt` stores `STATE_READY` and switches out.
///    `render` is now descheduled **still holding TABLE**, and it is pinned, so no other core can
///    steal and finish it.
/// 3. An aged-in user mode vug on the SAME core wins the next pop and issues `SYS_WIN_PRESENT`, which
///    masks IRQs, takes `WINDOWS`, and blocks on `TABLE`.
/// 4. That core can no longer take a timer IRQ, so it never re-enters its scheduler, so `render` is
///    never re-dispatched, so `TABLE` is never released. Permanent, silent, no panic.
///
/// It propagates: the spinner holds `WINDOWS` throughout, so every other vug on every other core
/// that issues any window verb masks and blocks on `WINDOWS` too — a fleet-wide GUI freeze from one
/// vug on one core. The foreground case is not even a coincidence: `run` places a user program on
/// the launching core, which is the shell's core, which is `render`'s core.
///
/// ## Why masking is the fix, and why the alternatives were rejected
/// Masking establishes: *no core is ever preempted while holding `TABLE`*. Every critical section
/// then runs to completion once entered, so every waiter — masked or not — waits at most one
/// critical section. This is the WEDGE-4 discipline that fixed `RUN_QUEUES`, transposed to `wm`.
/// It is affordable because all 23 sections are bounded `MAX_WINDOWS`-row scans with no print, no
/// allocation, no I/O and no nested blocking lock; the two longest are the composite snapshot
/// (8 rows + damage clear, with the blit itself OUTSIDE the guard) and the focus-raise scan.
///
/// A try-lock with backoff cannot work here: the holder is on the SAME core and cannot run while we
/// back off, so a masked backoff is the same deadlock with extra steps. Moving the composite off the
/// masked span is worse than useless — `arch::syscall` documents that the `WINDOWS` hold must span
/// the composite precisely because a `CLOSE`+`CREATE` pair on other cores can recycle the id in the
/// gap and land the caller's pixels under a different process's window identity. That is a
/// protection, so dropping it to fix a deadlock is not on the table.
///
/// This ADDS masking; it removes no check, permission, checksum or page attribute. The cost is
/// worst-case IRQ latency bounded by an 8-row scan — far below the `IrqGuard` spans already present
/// in every window verb.
///
/// ## The invariant, and how to check it without booting
/// *`wm::TABLE` is never held across a preemption point.* Checkable by grep, because this is the
/// only acquisition path:
/// ```text
/// grep -n "TABLE\s*\.\s*lock" video/wm.rs   -> exactly ONE hit, inside `fn table()`
/// grep -rn "\bTABLE\b" --include=*.rs src | grep -v video/wm.rs   -> no acquisitions (TABLE is private)
/// ```
/// Standing rule for future arcs: **no `TABLE` critical section may call anything that can block,
/// print, or allocate.** True today; this guard makes it load-bearing.
struct TableGuard {
    // DECLARATION ORDER IS THE FIX. Rust drops struct fields in declaration order, so the lock
    // guard is released FIRST and the IRQ mask is restored SECOND. The reverse would unmask while
    // still holding TABLE, re-opening a preemption window in the hold's tail — which is precisely
    // the bug, in miniature, at every unlock.
    // `spin 0.12`'s guard is `<'a, T, R>` — the 2-generic form this tree requires.
    inner: MutexGuard<'static, Table, SpinRelax>,
    _irq: crate::arch::IrqMask,
}

impl core::ops::Deref for TableGuard {
    type Target = Table;
    #[inline]
    fn deref(&self) -> &Table {
        &self.inner
    }
}

impl core::ops::DerefMut for TableGuard {
    #[inline]
    fn deref_mut(&mut self) -> &mut Table {
        &mut self.inner
    }
}

/// Acquire [`TABLE`] with IRQs masked. See [`TableGuard`].
#[inline]
fn table() -> TableGuard {
    // Mask BEFORE the acquisition, not after: masking after would leave the spin itself
    // preemptible, and a holder preempted between our acquire and our mask is the same wedge.
    let _irq = crate::arch::IrqMask::new();
    let inner = TABLE.lock();
    TableGuard { inner, _irq }
}

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

// ---- VUGMIN-B: hidden-owner plumbing -----------------------------------------------------------

/// VUGMIN-B — `wm`'s SHADOW of the hidden bitmask `arch::aarch64::syscall` owns, one bit per ASID.
///
/// It exists for the WITNESS and for nothing else. The authoritative bit lives in `HIDDEN_ASIDS` over
/// in the syscall layer (that module owns the info page it is published through); this side only needs
/// to know whether a given [`vugmin_publish`] call was a TRANSITION, so `hides`/`unhides` count state
/// changes rather than the number of times focus moved. The publish itself is issued unconditionally
/// (see `vugmin_publish`), so a shadow that drifts — ASID recycle clears the real bit through
/// `boot::teardown_user_slot` without telling `wm` — can miscount a rollup but can never leave the real
/// bit wrong. The counters are the soft thing here; the mechanism is not.
#[cfg(feature = "witness")]
static VUGMIN_SHADOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// VUGMIN-B — owners this module has pushed below the shell, owners it has brought back, and presents
/// whose `composite()` was suppressed because the presenting owner was hidden. Reported by
/// [`vugmin_rollup`], beside [`wci_rollup_scoped`]'s line.
#[cfg(feature = "witness")]
static VUGMIN_HIDES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static VUGMIN_UNHIDES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static VUGMIN_SKIPPED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// VUGMIN-B — is every live, non-compat window owned by `asid` sitting BELOW `shell`?
///
/// The predicate is deliberately **per-owner and all-or-nothing**: an app may own several windows (the
/// focus ring is keyed by ASID and [`focus_changed`] raises them together), and an app with one window
/// still on the panel is not hidden however many of its others are buried. Single-window vugs are the
/// case that actually occurs; the quantifier is what keeps a two-window app from being told to idle
/// while the operator is looking at half of it.
///
/// Answers `false` for an owner with no live window at all — "hidden" is a claim about windows that
/// exist, and the empty case falls to the same safe side `set_hidden` fails to (keep rendering).
///
/// Compat rows are excluded for [`above_shell`]'s reason: a compat row carries owner ASID 0, is not a
/// focus target, and can never be raised back over a shell that overtook it. ASID 0 is never marked.
///
/// Caller holds `TABLE`. One scan of eight rows, no allocation, no nested lock.
fn owner_hidden(t: &Table, asid: u64, shell: u32) -> bool {
    if asid == 0 {
        return false;
    }
    let mut any = false;
    for r in t.rows.iter() {
        if !r.used || r.compat || r.owner_asid != asid {
            continue;
        }
        any = true;
        if above_shell(r, shell) {
            return false;
        }
    }
    any
}

/// VUGMIN-B — publish `asid`'s hidden state to the syscall layer, and count the transition.
///
/// **The seam is a DIRECT CALL, not a registered callback.** `video::wm` is not arch-neutral in
/// practice: the `[fluid3]` ledger reaches `crate::arch::aarch64::sched::fluid3_drain()` directly
/// under a plain `#[cfg(target_arch = "aarch64")]`, as do this file's EL0 fixtures, so a callback
/// table here would be a second, weaker convention for the same thing — and a seam whose whole
/// content is "one `u64`, one `bool`, no return
/// value" earns no indirection. The `cfg` is what keeps the other builds honest — `arch::aarch64::syscall`
/// is itself gated behind `baremetal`, so the call is compiled in exactly where the info page it
/// publishes through exists, and everywhere else (x86_64, the hosted aarch64 build) this is nothing.
///
/// **Never called with `TABLE` held, and it would be safe if it were.** `set_hidden` takes NO LOCK: it
/// is an `AtomicU64` `fetch_or`/`fetch_and` followed by one `write_volatile` of a `u32` into the slot's
/// info page, whose address is pure pointer arithmetic (`slot_fb_info_ptr` = base + offset, a
/// `debug_assert!` and nothing else). The page is per-slot kernel backing that lives as long as the
/// slot, so there is no allocation, no mapping change and no serial print on the path. Callers here
/// still publish OUTSIDE the table lock, because a snapshot taken under the lock and applied after it
/// is the shape every other outward call in this file already has, and it keeps the rule ("no calls
/// out from under `TABLE`") a rule rather than a case-by-case audit.
///
/// The publish is UNCONDITIONAL — `set_hidden` is idempotent, and skipping it on the strength of
/// [`VUGMIN_SHADOW`] would let one stale shadow bit (ASID recycle clears the real bit behind `wm`'s
/// back) leave a fresh tenant permanently idling. Republishing a bit that is already right costs one
/// `u32` store on a path that runs at operator speed.
fn vugmin_publish(asid: u64, hidden: bool) {
    if asid == 0 || asid >= 64 {
        return;
    }
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    crate::arch::aarch64::syscall::set_hidden(asid, hidden);
    #[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
    let _ = hidden;
    #[cfg(feature = "witness")]
    {
        use core::sync::atomic::Ordering::Relaxed;
        let bit = 1u64 << asid;
        let before = if hidden {
            VUGMIN_SHADOW.fetch_or(bit, Relaxed)
        } else {
            VUGMIN_SHADOW.fetch_and(!bit, Relaxed)
        };
        if (before & bit != 0) != hidden {
            if hidden {
                VUGMIN_HIDES.fetch_add(1, Relaxed);
            } else {
                VUGMIN_UNHIDES.fetch_add(1, Relaxed);
            }
        }
    }
}

/// VUGMIN-B — snapshot, for every live owner in the table, whether that owner is now hidden.
///
/// Returns the filled prefix length of `out`. Taken under `TABLE` by [`focus_changed`]'s **SHELL arm**
/// right after the z-order has moved, and applied through [`vugmin_publish`] once the guard is dropped.
///
/// VUGMIN-C — **this is the shell arm's scan and only the shell arm's.** It used to serve both arms, on
/// the reasoning that a predicate recomputed for every owner is a function of the table rather than of
/// the code path that reached it. That reasoning is right about the SHELL arm (`focus_changed(0)` moves
/// `SHELL_Z` above everything, so every owner's answer really does change in one step) and wrong about a
/// RAISE: raising one window changes exactly one owner's z, but the scan re-published `hidden=false` for
/// every owner still sitting above `SHELL_Z` — i.e. for every window ever raised since the last shell
/// TAB. On the P73 bench wire one `[clickroute] press hit asid=4` produced `[vugpause2] resume` with
/// `edge=unhide` for two OTHER address spaces, and a six-vug fleet stayed lit on all four cores forever.
/// The raise arm now publishes only the ARRIVING owner's unhide (see [`focus_changed`] — VUGMIN-C had
/// it publish the departing owner's hide too, which CLICK-PLAIN removed); this function keeps the
/// whole-table shape because "the shell took the top, everyone is under it" is genuinely whole-table,
/// and it is now the ONLY place a hidden bit is ever SET.
fn vugmin_scan(t: &Table, shell: u32, out: &mut [(u64, bool); MAX_WINDOWS]) -> usize {
    let mut n = 0usize;
    for r in t.rows.iter() {
        if !r.used || r.compat || r.owner_asid == 0 {
            continue;
        }
        if out[..n].iter().any(|&(a, _)| a == r.owner_asid) {
            continue;
        }
        out[n] = (r.owner_asid, owner_hidden(t, r.owner_asid, shell));
        n += 1;
    }
    n
}

// VUGMIN-C's `owner_live` lived here. It existed for ONE caller — the raise arm's hide of the previous
// focus holder — and guarded the one hazard that hide carried: a vug that exited while focused has had
// its hidden bit cleared by `boot::teardown_user_slot` on the way out, so publishing `hidden=true` for it
// afterwards would strand a set bit on a free slot and the next tenant of that ASID would come up
// permanently idle with nothing left to unhide it. CLICK-PLAIN removed the hide (see `focus_changed`),
// which removes the hazard with it: the raise arm now publishes only `hidden=false`, and a spurious
// CLEAR on a free slot is the harmless direction — it is the state `teardown_user_slot` already leaves.

/// Create a window owned by `owner_asid` over the caller's ARGB8888 surface at `surf` (kernel-visible
/// address, `surf_len` bytes long, `stride` bytes per row) with source dimensions `w` x `h` and a
/// short `title` (truncated to [`MAX_TITLE`]).
///
/// Geometry is chosen by the kernel: a fresh window is tiled into the next free column so multiple
/// apps are visible side-by-side, at the largest integer scale that keeps it legible and on-panel.
/// The caller may relocate it afterwards with [`move_to`].
///
/// # Surface-extent contract (F1) — WC-B MUST honour this
/// `surf_len` is the **real byte length of the mapped slot**, as the mapping code knows it — never a
/// value derived from user-supplied dimensions. `w`, `h` and `stride` may come straight from the
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
        None,
    )
}

/// SPAWN-PLACE — the geometry a window of `w` x `h` source pixels WILL be given, computed before any
/// row exists: `(scale, outer_w, outer_h)`, or `None` when the framebuffer is not ready.
///
/// This is the query half of [`create_at`]. A caller that wants its window at a particular place
/// (centred, corner-pinned) has to know how big the window will BE before it can say where the box
/// goes, and until this existed the only way to learn that was to create the window, read
/// [`info`], and then [`move_to`] — by which time [`create`]'s own composite had already put a frame
/// of that window on the panel at the tiler's origin. The scale comes from the same rule [`place`]
/// applies, so the answer is not an estimate of the layout: it IS the layout.
pub fn spawn_geometry(w: usize, h: usize) -> Option<(usize, usize, usize)> {
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return None;
    }
    let info = fb.info();
    let scale = place_scale(info.width, info.height, w, h);
    Some((
        scale,
        w.saturating_mul(scale).saturating_add(2 * BORDER),
        h.saturating_mul(scale)
            .saturating_add(TITLE_H + 2 * BORDER),
    ))
}

/// SPAWN-PLACE — [`create`] with the window's FINAL content origin supplied up front.
///
/// The invariant it exists to keep: **no pixel of a window is ever presented at a position it will
/// not occupy.** `create` places by the tiler and composites the new row before returning, so a
/// caller that follows it with [`move_to`] has already shown the window at the tiler's top-left
/// origin for one frame, and has left an abandoned box behind for the move to erase. Observed on the
/// metal (rMBP s41): both windows appeared at the top-left and jumped, and the vacated boxes stayed
/// on glass. Supplying the origin here removes the first frame and the vacated box together —
/// there is no move, so there is nothing to erase.
///
/// The row is PINNED (as `move_to` pins it), so the tiler leaves it where the caller put it. `(x, y)`
/// is the CONTENT origin and is clamped to the panel on both bounds exactly as [`move_to`] clamps it;
/// use [`spawn_geometry`] to size the outer box first.
#[allow(clippy::too_many_arguments)]
pub fn create_at(
    owner_asid: u64,
    surf: usize,
    surf_len: usize,
    w: u32,
    h: u32,
    stride: u32,
    title: &[u8],
    x: usize,
    y: usize,
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
        Some((x, y)),
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
    // F1 — the compat path's dimensions are NOT user-supplied: `present_surface` is reached through
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
            id = create_inner(0, surf, surf_len, w, h, stride, b"", true, None);
            if id == WIN_NONE {
                return;
            }
            COMPAT_WIN.store(id, Ordering::Relaxed);
        }
        id
    };
    {
        let mut t = table();
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
        r.damage_all();
        r.presented = true;
    }
    composite();
}

/// Whether `id` names a live row that is the compat window (F2's identity test).
fn is_compat_row(id: WinId) -> bool {
    let t = table();
    row(&t, id).map(|r| r.compat).unwrap_or(false)
}

/// CLICK-SHELL r2 — **is a FULL-SCREEN app presenting right now?** i.e. does a live compat row exist.
///
/// The compat row is the `SYS_FB_PRESENT` shim's window: at most one exists system-wide (WC-A
/// serialises its creation), it is created by the first full-screen present and closed at the
/// presenting slot's teardown (`close_compat`). It carries owner ASID 0 and is exempt from
/// [`hit_test`] and [`focus_ring`] alike, so from the router's side a full-screen app is invisible in
/// every window query — which is exactly why the router needs to be able to ask this question
/// directly. See `wc_click_route`'s miss arm: a press that hit-tests to nothing is a DESKTOP press
/// unless a full-screen app owns the panel, in which case it is that app's press.
pub fn compat_live() -> bool {
    let id = COMPAT_WIN.load(core::sync::atomic::Ordering::Relaxed);
    id != WIN_NONE && is_compat_row(id)
}

// ── CLICK-X86 seam, GRAFTED AT MERGE ASSEMBLY (r23 candidate) ────────────────────────────────
// The x86 trunk's CLICK-X86 lineage gives the kernel's own windows (panel console, desktop demo)
// clickable owner rows in a reserved band — hittable furniture that remains outside focus_ring
// and close_owner's reach. The full lineage (its hit_test/focus_changed integration and the
// fbcon/wcx registrations) re-lands as the x86 seat's own reviewed arc per the tier-3 baseline
// ruling; what is grafted HERE is only the seam `arch/x86_64/syscall.rs` already depends on:
// the band constants and the band predicate. Content taken verbatim from the x86 trunk's wm.rs
// (UnaOS-gemini f36ab3d5); doc text condensed, semantics untouched.

/// CLICK-X86: base of the reserved kernel-owner ASID band. Distinct from owner 0 ("nobody owns
/// this row" — compat rows, transient witness probes, all unclickable) so that "the kernel owns
/// it" and "nobody owns it" stop sharing one value: a kernel row is HITTABLE furniture while
/// remaining unreachable as a user focus target or teardown victim.
pub const KERNEL_OWNER_BASE: u64 = 0xFFFF_FF00;

/// CLICK-X86: the panel console's row (`fbcon::panel_console_window_open`).
pub const KERNEL_OWNER_CONSOLE: u64 = KERNEL_OWNER_BASE + 1;

/// CLICK-X86: the desktop furniture's row. **No producer since the kernel-apps eviction** — it named
/// `wcx::activate`'s kernel-drawn demo window, which is now a ring-3 process (`STAT.ELF`) owning an
/// ordinary user row. Kept as a RESERVED value rather than deleted: [`is_kernel_owner`] is a range
/// test over the whole band, the console row above still uses it, and the next piece of kernel-owned
/// desktop furniture should take this number rather than mint a third one.
pub const KERNEL_OWNER_DESKTOP: u64 = KERNEL_OWNER_BASE + 2;

/// CLICK-X86 — is `asid` in the reserved kernel-owner band? `false` for `0`, which still means
/// "nobody owns this row".
pub fn is_kernel_owner(asid: u64) -> bool {
    asid >= KERNEL_OWNER_BASE && asid <= KERNEL_OWNER_BASE + 0xFF
}

/// WC-A / F3 — close the compat window, if one exists. **WC-B must call this from the user teardown
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
///
/// ### VUGMIN-B — a hidden owner's present does not composite
/// If every window the presenting owner holds is below `SHELL_Z`, the pass would read the surface,
/// clip it, and write it nowhere the operator can see: [`composite`] draws only rows that satisfy
/// [`above_shell`], and by hypothesis none of this owner's do. The VUGMIN-A audit measured this as the
/// missing half of the mechanism — the user idle loop stops *most* presents from being issued, and this
/// makes the ones that still arrive (a vug hidden mid-frame, a program that does not poll the bit at
/// all, the frame in flight when the operator TABbed away) cost a table scan instead of a full pass.
///
/// **The row is still marked `damaged` and `presented` exactly as before**, and that is what makes the
/// suppression invisible on unhide: `focus_changed`'s raise arm re-marks the raised rows damaged and
/// ends in its own unconditional `composite()` (the call after the `<F6>` wedge mark), so the first
/// thing the panel gets back is a full repaint from the LATEST surface content, not from whatever the
/// last composited frame happened to be. Nothing is deferred here; a pass is skipped, not owed.
pub fn present(id: WinId) -> bool {
    present_banded(id, None)
}

/// FBCON-DMG — present `id`, declaring only SOURCE rows `[sy0, sy1)` of its surface damaged.
///
/// The whole of the difference from [`present`] is the extent the compositor repaints: the pass, the
/// occlusion closure, the staged-present discipline, the cursor bracket, VUGMIN-B's hidden-owner
/// suppression and every witness are the same code on the same path. A caller that knows which of
/// its rows changed can therefore stop paying for the ones that did not, without a second present
/// path existing to keep in step.
///
/// Rows are the SURFACE's, not the panel's — see [`Window::dmg_y0`]. `sy1 <= sy0` (or a band the
/// surface does not contain) degrades to the whole box rather than to nothing.
///
/// Returns `false` if `id` names no live window.
pub fn present_rows(id: WinId, sy0: usize, sy1: usize) -> bool {
    present_banded(id, Some((sy0, sy1)))
}

/// The body both present verbs share. `band` is `None` for a whole-box present, which is
/// byte-for-byte the pre-FBCON-DMG [`present`]; see that function's docs for VUGMIN-B and WC-N,
/// neither of which this arc touches.
fn present_banded(id: WinId, band: Option<(usize, usize)>) -> bool {
    // WC-G — the surface as the OWNER declared it finished. Taken here and nowhere else: this is the
    // one moment the owner is provably not writing (it is parked inside `SYS_WIN_PRESENT`), so it is
    // the only honest baseline for the `app` leg. The identity is captured under the table lock and
    // the checksum taken after it drops — a 64 KiB read is not something to hold the window table
    // across, and the surface cannot be unmapped underneath it while the owner is in the syscall.
    #[cfg(feature = "witness")]
    let mut probe: Option<(usize, usize)> = None;
    // VUGMIN-B — is the presenting owner hidden? Read under the same guard that marks the row, so the
    // answer and the mark are taken against one table state.
    let skip;
    {
        let mut t = table();
        let owner = match row_mut(&mut t, id) {
            Some(r) => {
                match band {
                    // A band the surface does not contain is not narrowed to the part that fits —
                    // it means the caller and the row disagree about the geometry, and the only
                    // answer that cannot leave a stale pixel is the whole box.
                    Some((y0, y1)) if y1 > y0 && y1 <= r.h => r.damage_rows(y0, y1),
                    _ => r.damage_all(),
                }
                r.presented = true;
                #[cfg(feature = "witness")]
                if !r.compat {
                    probe = Some((r.surf, r.surf_len));
                }
                // A compat row reports owner 0, which `owner_hidden` answers `false` for: the compat /
                // console path is never suppressed.
                if r.compat { 0 } else { r.owner_asid }
            }
            None => {
                // WC-N — a present that named no live row: the window closed under its owner. No slot
                // to charge it to, so it lands on the aggregate line.
                #[cfg(feature = "witness")]
                WCN_STALE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                return false;
            }
        };
        skip = owner_hidden(&t, owner, SHELL_Z.load(core::sync::atomic::Ordering::Acquire));
    }
    #[cfg(feature = "witness")]
    if let Some((surf, surf_len)) = probe {
        // WC-G's checksum is the OWNER's declaration of its own surface and is independent of whether
        // those pixels reach the panel, so it is taken on the suppressed path too — dropping it would
        // make the `app` leg disagree with itself the moment a window went behind the shell.
        super::wcg::on_present(id, surf, surf_len);
    }
    // WC-N — the ATTEMPT is recorded here, before the suppression branch, because an attempt is what
    // the owner did and it happened either way. `hidden` is the branch it took. Outside the table
    // lock (dropped at the block above), per this file's standing rule.
    #[cfg(feature = "witness")]
    wcn_note_present(id, skip);
    if skip {
        #[cfg(feature = "witness")]
        {
            VUGMIN_SKIPPED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // The cadence tick runs on the suppressed path too: a fleet that has just been hidden is
            // still presenting, and a rollup that went quiet the moment VUGMIN engaged would lose the
            // exact interval whose `hid=` count is the point.
            wcn_tick();
        }
        return true;
    }
    composite();
    #[cfg(feature = "witness")]
    wcn_tick();
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
    // WEDGE-1r2 `<D1>`/`<d1>` — a barrier-raising path begins, BEFORE the `WRITER`/`TABLE`
    // acquisitions that stand between here and `DrainBarrier::drain`. See the ledger block above
    // `DrainBarrier`: this is the region WEDGE-1's in-spin tripwire cannot report on, because a core
    // that dies waiting on `TABLE` here never reaches the spin the tripwire lives in. Chain-gated
    // (`mark_composite`) so it costs nothing outside a focus change — the shape every recorded wedge
    // has — and so its rate cannot bury the chain it is joining.
    crate::wedge2::mark_composite("<D1>", "<d1>");
    let fb = *super::WRITER.lock();
    // Guard intentionally dropped before the table lock: `place`/`composite` never hold the table
    // lock while touching `WRITER`, so keeping WRITER-then-TABLE strictly non-overlapping here is
    // what makes the two orders unable to interleave into a cycle.
    if !fb.is_ready() {
        return false;
    }
    let info = fb.info();

    let vacated = {
        let mut t = table();
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
                r.damage_all();
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
        // MOVE-VACATE (x86 s42 probe, `[wc-x] move-vacate … desktop=5/5 stale=0/5`): the erase
        // REACHES glass, but a desktop-layer present can later repaint the unoccluded box from
        // content that predates it — the ghost is the LATER writer, not the erase. Same cure
        // `reclaim` uses: force the next present to re-derive the whole surface.
        super::screen::request_full_present();
        // Re-open before recompositing — a composite under a raised barrier is a no-op.
        drop(barrier);
        // CURSOR-14 — CLOSE THE ERASE BRACKET BEFORE THE COMPOSITE, per CURSOR-13's single rule:
        // composite owns the sprite, and a caller-side bracket may not span a composite. `erase`
        // above took the arrow off the panel (it is a raw desktop fill and genuinely needs to), and
        // this call used to hand the restore to `composite`'s own tail — which meant the pass ran
        // with `sprite_plan() == None` and could not compose the arrow through the re-tile it is
        // about to perform. Putting it back HERE costs one restore/save/draw on a window move, and
        // the save is taken against a front buffer whose desktop is already final, exactly as
        // `Screen::flush` takes it. The tail is unaffected: `Untouched` still ends in `ensure_drawn`.
        super::cursor::repaint();
        composite();
    }
    true
}

/// Close `id`, freeing its table row. The surface itself belongs to the owner's address space and is
/// not touched here — WC-B unmaps it. Returns `false` if `id` names no live window.
pub fn close(id: WinId) -> bool {
    // PAYGO-TERM — settle this window's battery while it still HAS a surface and a row. Everything
    // below frees both, and after that there is no verdict to be had at any price. See
    // [`paygo_at_close`]; it is a no-op for a window the deferral gate never turned away, which is
    // every window on a build without the knob and most windows on one with it.
    //
    // Ahead of the WEDGE token deliberately: the composites it may run are ordinary passes and must
    // not be threaded into the death chain the token opens.
    #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
    paygo_at_close(id);
    // WEDGE-1r2 `<D1>`/`<d1>` — see `move_to`. `close` reaches its barrier through a `TABLE` critical
    // section, so the token has to precede the lock to cover the death that happens ON it.
    crate::wedge2::mark_composite("<D1>", "<d1>");
    let vacated = {
        let mut t = table();
        match row_mut(&mut t, id) {
            Some(r) => {
                let b = outer_box(r);
                *r = Window::empty();
                b
            }
            None => return false,
        }
    };
    // WC-N — the slot may be handed to a new window immediately; clear the activity clock so the next
    // tenant's first present is not measured against this one's last.
    #[cfg(feature = "witness")]
    wcn_forget(id);
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
    // CURSOR-14 — close the erase bracket before the composite. Same rule and same argument as
    // `move_to`'s; placed after BOTH reclaims, because each of them erases and the sprite must go
    // back on a panel whose desktop is finished.
    super::cursor::repaint();
    composite();
    true
}

/// Close every window owned by `owner_asid` and return how many rows were freed. Task teardown
/// (`clear_handle_row`) calls this so a dead ASID can never leave a window compositing from a
/// surface whose address space is gone.
pub fn close_owner(owner_asid: u64) -> usize {
    // WEDGE-1r2 `<D1>`/`<d1>` — see `move_to`, and note that THIS is the path that matters most: it
    // is reached from `sched::exit` → `clear_handle_row`, which has already masked interrupts, so a
    // core that blocks on the `TABLE` acquisition below is unpreemptible and silent — and is upstream
    // of the only place WEDGE-1 instrumented. It is also the hottest of the three (one call per user
    // task exit, most of them owning no window), which is why the chain gate is what makes the token
    // affordable here at all.
    crate::wedge2::mark_composite("<D1>", "<d1>");
    let mut vacated = [(0usize, 0usize, 0usize, 0usize); MAX_WINDOWS];
    let mut n = 0;
    {
        let mut t = table();
        for r in t.rows.iter_mut() {
            if r.used && r.owner_asid == owner_asid {
                vacated[n] = outer_box(r);
                // WC-N — same slot-recycle reset as `close`. Under the table lock here because the
                // row's id is only readable before the clear; `wcn_forget` is one relaxed store.
                #[cfg(feature = "witness")]
                wcn_forget(r.id);
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
    // WC-B's per-ASID surface mappings it becomes a kernel abort mid-blit.
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
    // WEDGE-2 `<D4>` — THE ERASE/RECLAIM HALF IS BEHIND US AND THE BARRIER IS DOWN; the cursor
    // bracket is next, and with it the `SPRITE` acquisition.
    //
    // Without this token a death on `SPRITE` here is MISREPORTED as a death on `TABLE`: the last
    // thing on the wire would be `<D3>`, and `<D3>` is emitted from inside `DrainBarrier::drain`,
    // which every teardown reaches — so the reader would place the wedge in the erase/reclaim run
    // and never look at the cursor at all. `<D4>` splits that region in two, and the split is the
    // whole point: `<D3>` with no `<D4>` is the reclaim, `<D4>` as the LAST token on the wire is
    // `cursor::repaint` — i.e. `SPRITE`, which is the F4 site and a different lock from F1's.
    //
    // This path is the one that matters for that distinction. `close_owner` runs on EVERY user task
    // exit (`sched::exit` → `clear_handle_row`), which has already masked interrupts, so a core that
    // blocks on `SPRITE` below is unpreemptible and silent — and the symptom is not a stalled
    // teardown but a dead panel: the sprite lock gates the cursor bracket every compositor path
    // takes, so the freeze is total (panel, cursor and input at once) with nothing on the wire.
    //
    // Unconditional `mark`, NOT `mark_composite`, and that is the same rule `<D2>`/`<D3>` follow
    // rather than a departure from `<D1>`'s: it sits past the `n == 0` early return, so its
    // population is teardowns that genuinely freed a row and genuinely raised a barrier — 31 in the
    // reference wedge2 run, not the 96 that made `<D1>` unaffordable ungated. And a wedge that eats
    // the panel is worth naming whether or not a TAB happens to be in flight.
    crate::wedge2::mark("<D4>");
    // CURSOR-14 — close the erase bracket before the composite; see `move_to`.
    super::cursor::repaint();
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
/// cap, and the same `try_lock`.
///
/// ### WC-L — why there is no direct fallback any more
///
/// WC-K kept `fill_rect` as the last resort when staging was unavailable, and reported it. The P64
/// attended boot showed what that costs: on two focus tab-cycle transitions under ~99% core load the
/// erase path could not take [`STAGE`] and wrote a 514x526 desktop fill DIRECTLY into the buffer the
/// HVS was scanning — `[wc-k] erase box=514x526 staged=no reason=lock -> DIRECT`, twice. Every other
/// fill that boot was `BUFFERED`. So the fallback did not make the discipline robust; it made it
/// conditional on nothing else wanting the lock, and it re-introduced under load exactly the last
/// direct front-buffer writer WC-K existed to remove.
///
/// The fallback is therefore GONE. When [`stage_fill`] cannot stage, the box is pushed onto
/// [`DEFER`] as deferred damage — desktop-colour repaint owed — and [`drain_deferred`], at the head
/// of the next composite pass, erases it through the staged path and re-damages the windows the
/// paint reached. This is not a second queue with its own rules: it is WC-J's `reclaim` shape
/// (erase, `damage_intersecting`, `request_full_present`) applied to a box whose erase arrived one
/// pass late. A one-frame-late desktop repaint is a cost the panel can absorb; a torn front-buffer
/// write is not.
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
///
/// CURSOR-14 — the RESTORE is no longer the following composite's tail. `move_to`, `close` and
/// `close_owner` each call `cursor::repaint()` after their last erase and BEFORE their `composite()`,
/// so the bracket this function opens is closed by its caller rather than spanning a compositor pass.
/// The undraw below is unchanged and still required: a raw desktop fill with no session is exactly
/// the class CURSOR-13 kept bracketed.
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
    for &(x, y, w, h) in boxes {
        if w == 0 || h == 0 || x >= info.width || y >= info.height {
            continue;
        }
        // F6 — clip before iterating, as in `draw_window`.
        let (w, h) = (w.min(info.width - x), h.min(info.height - y));
        // WC-L — staged or nothing. A box that cannot stage becomes deferred damage; it does NOT
        // reach the panel through this call, so the cache flush below is skipped for it too (there
        // is nothing of ours in those rows to publish, and flushing them would push whatever the
        // window left there back out as if it were fresh).
        if !stage_fill(&fb, x, y, w, h, DESKTOP_BG, false) {
            continue;
        }
        let y0 = y.min(info.height);
        let y1 = (y + h).min(info.height);
        if y1 > y0 {
            // COMPOSITE-2 — the fill's own columns, not full-width scanlines (see `draw_window`).
            fb.flush_rect(x, y0, w, y1 - y0);
        }
    }
}

/// The ASID owning `id`, or `None` if `id` names no live window. The ownership gate WC-B's verbs use:
/// a task may only present/move/close a window whose owner matches its own ASID.
pub fn owner_of(id: WinId) -> Option<u64> {
    let t = table();
    row(&t, id).map(|r| r.owner_asid)
}

/// A snapshot of `id`'s table row, or `None` if it names no live window.
pub fn info(id: WinId) -> Option<WindowInfo> {
    let t = table();
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
/// tab-cycle to pick the next `USER_INPUT_ACTIVE`; nothing in this module reads input state.
pub fn focus_ring(out: &mut [u64; MAX_WINDOWS]) -> usize {
    let t = table();
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

/// CLICK-ROUTE — **which window owns the panel pixel `(x, y)`?** The topmost VISIBLE window whose
/// outer box (chrome included) contains the point, as `(window id, owner asid, z)`; `None` for a
/// point the window layer does not own (the desktop, the console, or a window the shell hides).
///
/// ### Why this exists
/// The EL0IN-FOCUS audit established that the pointer-button path has **no window hit-test at all**:
/// focus has exactly three sources (the `run` grant, the grant's clear, and the TAB ring), and a
/// `Button` under user focus goes straight to the focused app's ring regardless of where the pointer
/// is. That is P69's bench symptom in one sentence — clicking *anywhere* click-pauses the *focused*
/// vug, because the click was never addressed to a window in the first place. This function is the
/// missing address lookup, and it is deliberately the whole of the window system's contribution:
/// the routing policy built on it lives in the arch input router, not here.
///
/// ### The visibility rule is the z-order rule
/// "Visible" is not a flag on the row — it is a POSITION. `SHELL_Z` is allocated out of the same
/// counter window z's come from, so a row with `z < SHELL_Z` is below the shell, is not composited
/// at all, and the console owns those pixels (see [`above_shell`]). Such a row is therefore **not
/// hittable**: clicking the console must reach the console, not a window buried under it. Using the
/// same predicate the compositor uses is the point — what you can click is exactly what you can see.
///
/// ### Compat rows are excluded, for the same reason [`focus_ring`] excludes them
/// A compat row is the full-screen `present_surface` shim; it carries owner ASID 0 (the hook has no
/// ASID to pass), so it is not addressable as a focus target and a "hit" on one would name nobody.
/// A full-screen app therefore reads as *no hit* here, and the router's fallback — deliver as before
/// — is what keeps its clicks working.
///
/// ### Cost and locks
/// One `TABLE` lock, a scan of eight rows, no allocation, no nested lock, no new lock order — the
/// same shape as [`focus_ring`] and [`occluders`]. A SNAPSHOT, never a handle: the caller acts on the
/// asid it gets back through the ordinary focus primitive, which re-validates for itself.
///
/// Ties in `z` break by id, matching [`composite`]'s back-to-front order, so "topmost" here and
/// "drawn last" there are the same window by construction.
pub fn hit_test(x: i32, y: i32) -> Option<(WinId, u64, u32)> {
    if x < 0 || y < 0 {
        return None;
    }
    let (px, py) = (x as usize, y as usize);
    let shell = shell_z();
    let t = table();
    let mut best: Option<(WinId, u64, u32)> = None;
    for r in t.rows.iter() {
        if !r.used || r.compat || r.owner_asid == 0 || !above_shell(r, shell) {
            continue;
        }
        let (bx, by, bw, bh) = outer_box(r);
        if bw == 0 || bh == 0 {
            continue;
        }
        // Saturating adds mirror `outer_box`'s own saturation: an absurd box clips, it never wraps
        // into a small one that would silently stop being hittable.
        if px < bx || py < by || px >= bx.saturating_add(bw) || py >= by.saturating_add(bh) {
            continue;
        }
        match best {
            Some((bid, _, bz)) if (r.z, r.id) <= (bz, bid) => {}
            _ => best = Some((r.id, r.owner_asid, r.z)),
        }
    }
    best
}

/// FOCUS-VIS — **make a focus change VISIBLE.** The one seam the focus owner calls after
/// `user_input_set_active`; `asid == 0` means the SHELL slot of the ring.
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
/// * **VUGMIN-C — a raise publishes an ARRIVAL, not a census.** The arriving owner is unhidden (the wake
///   edge that restarts a parked vug); every other owner's hidden bit is left alone. Only the shell arm
///   speaks for the whole table. Before this, a raise re-published `hidden=false` for every owner above
///   `SHELL_Z`, so focusing any one window un-minimized the entire stack and the whole fleet rendered
///   forever (P73).
/// * **CLICK-PLAIN — a focus change never STOPS anything (P75).** VUGMIN-C also hid the DEPARTING owner
///   here, so that only the focused vug ran. On top of click-to-focus that made a click appear to stop a
///   vug the operator had not clicked, one click after the fact ("stop works like absolute garbage there
///   is no reason to it"). A raise is now purely additive: it starts the window it names and leaves
///   every other window exactly as the operator last saw it. Idling the whole fleet is still one
///   gesture — focus the SHELL — which is the arm that genuinely speaks for the whole table.
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
    // WEDGE-2 `<F4>` — the focus RAISE begins, and this core claims the chain (so a composite pass on
    // any other core can report itself as concurrent until `<F9>`). Emitted before the `TABLE` lock
    // that does the z-bump: a `<F4>` with no `<F5>` puts the death on `TABLE`.
    crate::wedge2::chain_enter();
    crate::wedge2::mark("<F4>");
    let mut raised = 0usize;
    let mut first_id = WIN_NONE;
    let mut newz = 0u32;
    // Boxes of windows this call pushed BELOW the shell — the pixels the console is about to own.
    let mut hidden = [(0usize, 0usize, 0usize, 0usize); MAX_WINDOWS];
    let mut nhidden = 0usize;
    // FV-EXEMPT — of those boxes, how many belong to a row that [`above_shell`] says is STILL VISIBLE.
    // That number ought to be zero by the definition of the set, and it is not: see the shell arm.
    let mut exempt = 0usize;
    // VUGMIN-B/C — (owner asid, is it now hidden) pairs, built under the table lock below and published
    // to the syscall layer after the guard drops. The SHELL arm fills this from [`vugmin_scan`] (every
    // owner, all hidden); since CLICK-PLAIN the RAISE arm fills in exactly ONE row — `marks[0]`, the
    // arriving owner, unhidden. Assigned on both arms, so it carries no initial value to be mistaken
    // for a verdict. See [`vugmin_scan`].
    let mut marks = [(0u64, false); MAX_WINDOWS];
    let nmarks: usize;

    // FOCUS-HL: take the focus owner BEFORE the table lock, so the composite at the end of this call
    // already draws the new highlight. The window that is LOSING focus must be repainted too — the raise
    // below only damages the windows it raises, and the old holder is not among them when focus moves to
    // a different ASID (or to the shell, which raises nothing at all). Without this its chrome would keep
    // the highlight colours until something else happened to damage it.
    let prev = FOCUS_ASID.swap(asid, Ordering::Release);

    {
        let mut t = table();
        if prev != asid && prev != 0 {
            for r in t.rows.iter_mut() {
                if r.used && !r.compat && r.owner_asid == prev {
                    r.damage_all();
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
                    // FV-EXEMPT — **`r.z < z` IS NOT THE HIDING PREDICATE, AND THIS SET IS THEREFORE
                    // NOT WHAT `hidden=` HAS BEEN CALLING IT.** [`above_shell`] is what decides
                    // whether a row stops compositing, and it EXEMPTS compat rows on purpose: a
                    // compat row is the full-screen present path, it carries owner ASID 0, it is not
                    // a focus target, and it could never be raised back over a shell that overtook
                    // it — so hiding it would strand a full-screen app's output for the rest of the
                    // boot. Its z still falls below the shell here, so it is collected, erased to
                    // `DESKTOP_BG`, and then repainted by the `composite` at the end of this call,
                    // having never been hidden at all.
                    //
                    // Reachable, and on the desktop path: a BACKGROUND full-screen app (`bg` — the
                    // case BGRUN-1 exists for) is on the panel while the operator TABs to the shell.
                    // The visible cost is a whole-box desktop fill and an immediate repaint of the
                    // same pixels; the durable one is that `erase` may DEFER that fill under `STAGE`
                    // contention (WC-L), in which case the drain paints `DESKTOP_BG` over the app a
                    // pass AFTER the repaint has already put it back.
                    //
                    // COUNTED, NOT CHANGED. Narrowing the set to `!above_shell(r, z)` is a one-token
                    // edit and it is deliberately not taken here: what a bg full-screen app's pixels
                    // should do across a shell TAB is a panel question, the gate has no HID to TAB
                    // with, and this seat may not settle it from a headless run. `exempt=` puts the
                    // contradiction on the wire so the next bench sitting can read it instead of
                    // inferring it, and it is derived from `above_shell` rather than from `r.compat`
                    // so a future exemption added there is carried automatically.
                    if above_shell(r, z) {
                        exempt += 1;
                    }
                    hidden[nhidden] = outer_box(r);
                    nhidden += 1;
                    // Damaged so that a later raise repaints from the source surface rather than
                    // trusting whatever survived on the panel.
                    r.damage_all();
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
                t.rows[i].damage_all();
                if first_id == WIN_NONE {
                    first_id = t.rows[i].id;
                }
                newz = z;
                raised += 1;
            }
        }
        // VUGMIN-B/C — the z-order is now final for this focus change, so this is the one moment the
        // hidden state has a settled answer. Snapshot only; the publication is outside the guard.
        if asid == 0 {
            // SHELL arm: the shell took the top of the stack, so EVERY owner is under it and the
            // whole-table scan is the honest answer. Unchanged from VUGMIN-B.
            nmarks = vugmin_scan(&t, SHELL_Z.load(Ordering::Acquire), &mut marks);
        } else {
            // RAISE arm (VUGMIN-C, as amended by CLICK-PLAIN): publish the ARRIVAL and nothing else.
            // The arriving owner unhides — that is the wake edge VUGPAUSE-2r2 exists to deliver, and
            // it must still fire. Every OTHER owner's bit is left exactly as it was: an owner the
            // SHELL arm hid stays hidden until it is itself raised, and an owner that was running keeps
            // running.
            //
            // VUGMIN-C also HID the departing owner here, so that exactly one vug — the focused one —
            // ran at a time. P75 is the ruling against that: "stop works like absolute garbage there is
            // no reason to it". Combined with the router's click-to-focus, moving focus stopped a vug
            // the operator had not clicked, one click after they clicked somewhere else, and no visible
            // gesture explained it. **A focus change now starts things and never stops them.** The
            // fleet-idling semantic VUGMIN-A/B designed is untouched and still reachable, from the arm
            // that was always the honest place for it: focusing the SHELL hides every owner at once
            // (above), which is a whole-table statement the operator makes deliberately.
            marks[0] = (asid, false);
            nmarks = 1;
        }
    }

    // VUGMIN-B/C — tell the syscall layer who is hidden now. `TABLE` is dropped: `set_hidden` takes no
    // lock (atomics + one `u32` info-page store), so this is a convention rather than a necessity, but
    // it is the file's convention. The shell arm hides every owner at once; a raise publishes at most
    // two rows, the owner arriving and the owner leaving, and says nothing at all about the rest.
    for &(asid, hid) in marks[..nmarks].iter() {
        vugmin_publish(asid, hid);
    }

    // WEDGE-2 `<F5>` — the z-bump is done and the table guard is dropped; the immediate REPAINT half
    // (`erase` of the vacated boxes, then the desktop's full-present request) is next. Both touch the
    // framebuffer and the sprite; a `<F5>` with no `<F6>` puts the death there rather than in the
    // composite pass.
    crate::wedge2::mark("<F5>");
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
        // FV-EXEMPT — `exempt` is the part of `hidden` that was erased and immediately repainted
        // rather than hidden (see the shell arm). `hidden=N exempt=0` is the line reading it always
        // implied; any other reading is the contradiction, named.
        serial_println!(
            "[wc-fv] focus shell z={} hidden={} exempt={}",
            newz, nhidden, exempt
        );
    } else {
        serial_println!(
            "[wc-fv] focus raise asid={:#x} windows={} top_win={} z={} shell_z={}",
            asid, raised, first_id, newz, shell_z()
        );
    }
    // VUGMIN-C — and WHAT THE HIDDEN BIT DID, on the same breath as the raise it belongs to. The whole
    // defect this replaced was invisible on the wire from `wm`'s side: the only trace was N
    // `[vugpause2] resume ... edge=unhide` lines appearing in the syscall layer for ASIDs nobody had
    // focused. One line per focus change, at operator rate, that names the transition and asserts the
    // scope: exactly one unhide, at most one hide, nothing else published.
    // CLICK-PLAIN keeps the line and narrows what it can say: `hid=none` on EVERY raise, because a raise
    // no longer hides anything. Printing the field rather than deleting it is deliberate — it is the
    // standing assertion that this arm publishes exactly one bit, and a future arc that reintroduces a
    // hide here has to change the line to do it.
    #[cfg(feature = "witness")]
    if asid != 0 {
        serial_println!(
            "[vugmin] focus asid={:#x} unhid={} hid=none others=untouched",
            asid, nmarks
        );
    }
    let _ = (raised, first_id, newz, exempt);
    // WEDGE-2 `<F6>` — the `[wc-fv]` line above is the LAST thing every recorded wedge printed, so
    // this token is the one that matters most: it is emitted after that line and before the composite
    // pass. `<F6>` present with nothing after it says the chain survived the print and died inside
    // `composite`; `<F5>` with no `<F6>` says it never got out of the erase/present-request half, i.e.
    // the `[wc-fv]` print itself (which takes the serial lock) was the last step.
    crate::wedge2::mark("<F6>");
    composite();
    // WEDGE-2 — close the chain window here rather than at `<F9>`, so it is released on EVERY path out
    // of `focus_changed` (the FOCUS-VIS selftest calls this function too, and a claim left standing
    // would make every later composite in the boot report `<f7>`/`<f8>` forever). `<F9>` is then a
    // pure marker at the return site. A chain the wedge kills mid-flight leaves the claim set, which
    // costs nothing: that core is not coming back.
    crate::wedge2::chain_exit();
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
/// the full-screen present path (`screen::present_surface`), and while a full-screen user program owns
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
    // to run a compat (full-screen) user app was the blocking foreground `run`. `bg` breaks it: a
    // background compat app presents directly to the scan-out while the desktop keeps flushing —
    // exactly the two-writer collision this function exists to order. The exclusion key is therefore
    // the FOCUSED compat surface, not the compat flag: the foreground case (incl. the boot-time
    // EXEC-UVUG witness, whose 300-frame deadline the first cut blew) always holds input focus
    // (`run_user_image` sets it before its wait loop), while a bg compat app can never acquire it
    // (the TAB ring walks windows only). A compat row cannot be keyed by OWNER — `compat_present`
    // creates it with owner_asid 0 (the SYS_FB_PRESENT hook carries none) — so the key is coarser:
    // repaint compat rows only while NO user program holds input focus (`focused == 0`, the
    // bg-app-at-the-prompt state). Every foreground run — `run` verb and the boot witnesses alike —
    // sets focus before its wait loop, so the EXEC-UVUG deadline case stays excluded. Residual,
    // stated: a bg compat app still shimmers while the operator is TABbed into some OTHER app
    // (focused != 0 excludes all compat rows); cosmetic, bounded by TABbing back to the shell.
    // Focus lives in the baremetal user input router (syscall.rs is baremetal-gated); elsewhere 0
    // means every compat row repaints, which is vacuous there (compat_present is unreachable).
    #[cfg(feature = "baremetal")]
    let focused = crate::arch::syscall::user_input_active();
    #[cfg(not(feature = "baremetal"))]
    let focused: u64 = 0;
    {
        let mut t = table();
        let repaintable = |r: &Window| r.used && (!r.compat || focused == 0);
        if !t.rows.iter().any(|r| repaintable(r)) {
            return;
        }
        for r in t.rows.iter_mut() {
            if repaintable(r) {
                r.damage_all();
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
    let t = table();
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

/// GR21/WCD-OCC — a snapshot of the outer boxes of every window drawn ON TOP of one window under
/// verify, so the scan-out read-back ([`verify_window`]) and the glass read-back ([`super::wcg`])
/// can tell a pixel a HIGHER window legitimately owns from a pixel the blit got wrong.
///
/// The read-back witnesses assert *"every pixel of this window's content rect on the panel equals
/// this window's source surface"* — a claim that is false for any window with a live window over
/// it. Boot AK made this reachable as a STEADY STATE: an evicted desktop app (`STAT.ELF`) sits in
/// the tiler's second tile whenever a second window is open, and its chrome box straddles the
/// pinned console's top-left third. The panel is correct — `STAT` really is over the console — but
/// the console's own read-back charged `win=1` for 256 410 pixels `STAT` owns and reported `-> FAIL`
/// (`[wc-d]`) / `-> BLIT` (`[wc-g]`). This snapshot subtracts those pixels.
///
/// **x86 only, and the arch gate is a wire boundary, not a scoping convenience.** `wm.rs`/`wcg.rs`
/// and the tiler are SHARED with aarch64, and `scripts/specs/pi4-regression.spec` reads the
/// `[wc-d]`/`[wc-g]` line format. The `occluded=`/`occ=` fields and the counting behind them are
/// therefore gated to x86 so the aarch64 wire stays byte-identical — the same protection-boundary
/// argument WCD-TEARDOWN's interlock made (`wm.rs:3580`). The false FAIL is identical on aarch64
/// (same `wm.rs`, same tiler, same routed console); fixing it there moves another track's gate
/// wire and is the integrator's call.
///
/// A snapshot, never a handle — read under the table lock and copied out, exactly as [`occluders`]
/// is, so a window that moves or closes immediately afterwards is repaired by the mover's own
/// composite and never by us.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
#[derive(Clone, Copy)]
pub struct OccSnap {
    boxes: [(usize, usize, usize, usize); MAX_WINDOWS],
    n: usize,
}

#[cfg(all(feature = "witness", target_arch = "x86_64"))]
impl OccSnap {
    /// The empty snapshot — no window above the one under check.
    pub const fn none() -> Self {
        Self {
            boxes: [(0, 0, 0, 0); MAX_WINDOWS],
            n: 0,
        }
    }

    /// The number of occluder boxes captured — the two operands of the `occ=n0/n1` wire field.
    pub fn count(&self) -> usize {
        self.n
    }

    /// Does any occluder box cover panel pixel `(x, y)`? A mismatching pixel that a higher window
    /// covers is that window's pixel, not the blit's to answer. Saturating add mirrors [`outer_box`]:
    /// an absurd box clips rather than wrapping into one that would silently stop being hittable.
    pub fn covers(&self, x: usize, y: usize) -> bool {
        self.boxes[..self.n].iter().any(|&(bx, by, bw, bh)| {
            x >= bx && y >= by && x < bx.saturating_add(bw) && y < by.saturating_add(bh)
        })
    }
}

/// GR21/WCD-OCC — the outer boxes of every live, non-compat window stacked ABOVE `(z, id)`.
///
/// Mirrors [`occluders`]'s population (`used`, non-`compat`) but filters relative to ONE window
/// rather than to the shell: a window `rr` occludes the window under check iff it is drawn AFTER it,
/// i.e. `(rr.z, rr.id) > (z, id)` — the same back-to-front key `composite` sorts the blit order by,
/// so the set is exactly the windows whose pixels can legitimately sit over the verified rect. The
/// window under check is excluded for free (`(z, id)` is not `> (z, id)`), and so is anything at or
/// below its z, which keeps a real corruption UNDER a sibling that is not actually above it still
/// chargeable. Compat rows are excluded for the reason [`occluders`] gives — a compat row IS the
/// full-screen present path, and while it owns the panel the render task is parked and not
/// verifying anyway.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
fn occluders_above(z: u32, id: u32) -> OccSnap {
    let t = table();
    let mut snap = OccSnap::none();
    for r in t.rows.iter() {
        if !r.used || r.compat {
            continue;
        }
        if (r.z, r.id) <= (z, id) {
            continue;
        }
        let b = outer_box(r);
        if b.2 == 0 || b.3 == 0 {
            continue;
        }
        snap.boxes[snap.n] = b;
        snap.n += 1;
    }
    snap
}

/// WC-I — how did this present's occluder snapshot AGE while the desktop was copying?
///
/// [`occluders`] is a snapshot, and the sentence above ("a window that moves or closes immediately
/// afterwards is repainted by the mover's own composite") is the correctness argument for taking one.
/// It is a good argument and nothing here weakens it. What it does NOT do is make the window layer's
/// staleness observable, and that gap is why `intrusions` sat at a structural zero for two weeks: the
/// only predicate the desktop could offer after the fact was "did the subtraction I already performed
/// succeed", which is a tautology (see [`WCI_INTRUSIONS`]).
///
/// This asks the question that can still be false: **did the set change under the copy, and if it did,
/// did anything this present actually wrote land in a box that was not in the snapshot?** Both halves
/// are returned, because they answer different things and the second is worthless without the first.
///
/// * `stale` — the current occluder set differs elementwise from `snap`. [`occluders`] emits rows in
///   slot order and skips the same rows on both reads, so a plain sequence compare catches a create, a
///   close, a move, a resize and a z-change; only an A→B→A flip inside one present hides, and a flip
///   that lands back on the identical geometry has also un-done the exposure.
/// * `intruded` — some box present NOW and absent from `snap` overlaps `bbox`, the union of the spans
///   this present actually copied to glass. A box that ENTERED was never subtracted, so background
///   pixels over it are background pixels inside a live window: the original WC-I predicate, on the
///   only population where it can still be true.
///
/// **Conservative in one direction, named.** `bbox` is a union rectangle, not the span set, so an
/// entered box that overlaps the union while sitting entirely inside a span the OLD table already
/// subtracted counts as an intrusion it was not. That is over-reporting on a tripwire, which is the
/// side to err on; the exact test would mean retaining every span of the present, and the per-span
/// cost of that is not worth paying for a witness.
///
/// Takes the table lock a second time for the present. Witness builds only — the shipped desktop pays
/// nothing, and the return value drives no pixel: `present_background` still returns `false` from
/// every exit and the repair for this race is, as it always was, the mutator's own composite.
///
/// ### Cost, stated for the track that actually pays it
///
/// "Witness builds only" is not the same as "x86 only", and the second caller makes the difference
/// concrete. The probe on `present_background`'s `vugpar`+`baremetal` band exit compiles into the
/// **arm-pi bench build** — `arroyo`'s `arm-pi` leg carries `witness`, `baremetal` and `vugpar`
/// together — where that exit is the full-screen VUG present's hot path. On every such present it
/// adds a bbox loop over the damage set plus this function's second window-table lock acquisition
/// and eight-row scan. That is **unmeasured on the Pi**; this arc gated on `check` and had no Pi
/// bench time. The x86 legs do not carry `vugpar` at all, so on x86 only the serial exit is probed
/// and the added work is the same second lock against a loop that already blits.
///
/// Stated rather than assumed away: if the pi track measures a regression in VUG frame rate, this
/// call and its bbox loop are the first thing to bisect, and gating the band-exit probe behind its
/// own knob is the obvious remedy.
#[cfg(feature = "witness")]
pub(super) fn occluders_aged(
    snap: &[(usize, usize, usize, usize)],
    bbox: Option<(usize, usize, usize, usize)>,
) -> (bool, bool) {
    let mut cur = [(0usize, 0usize, 0usize, 0usize); MAX_WINDOWS];
    let ncur = occluders(&mut cur);
    let cur = &cur[..ncur];
    if cur.len() == snap.len() && cur.iter().zip(snap.iter()).all(|(a, b)| a == b) {
        return (false, false);
    }
    let Some((bx0, by0, bx1, by1)) = bbox else {
        return (true, false);
    };
    for b in cur.iter() {
        if snap.contains(b) {
            continue;
        }
        let (wx, wy, ww, wh) = *b;
        if bx0 < wx + ww && wx < bx1 && by0 < wy + wh && wy < by1 {
            return (true, true);
        }
    }
    (true, false)
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
/// Cheap when idle: one relaxed atomic load, then one table-lock acquisition and a scan of eight
/// rows, then out. The pass itself only runs when a row is genuinely damaged or an erase is owed.
///
/// ### WC-L — why the deferred queue is part of the condition
///
/// This is the erase queue's LIVENESS GUARANTEE, and without it the queue has none in the case that
/// matters most. The ordinary argument is "some window presents, which composites, which drains" —
/// but a box is deferred precisely when a window was torn down, and if it was the LAST window there
/// is nothing left to present. The desktop's own flush is then the only thing still running, and it
/// arrives here; before this check, `service_damage` would find no damaged row (there are no rows),
/// return, and the deferred box would sit on the queue for the rest of the boot with a dead window's
/// last frame on the panel — the exact P61 ghost WC-J removed, re-entering by a new route.
///
/// So the desktop's cadence, not a window present, is what bounds a deferral's latency in the
/// general case. `DEFER_N` is a relaxed load and the common value is zero, so the added cost on the
/// idle path is one atomic read ahead of a lock acquisition that was already happening.
pub fn service_damage() {
    if DEFER_N.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        let t = table();
        if !t.rows.iter().any(|r| r.used && r.damaged) {
            return;
        }
    }
    composite();
}

/// PAYGO-TERM — THE SERVICE-PASS TAKER: give a matured deferral a taker that is not a present.
///
/// ### The gap this closes, in Boot V's own numbers
///
/// The deferral's liveness argument was "the window keeps compositing, so the pass after the
/// threshold takes the sample". Boot V falsified it. Its last composite was at 13 776 ms against
/// `defer_ms=15000`; the sched demo's post-storage cycles never ran, so nothing presented again, and
/// `x86_render_service` — the compositor's own lane — blocks on `GUI_CHANNEL_X86.recv()`, so an idle
/// panel produces no passes at all. `win=1`, the console, sat at `state=waiting taken=1` for the
/// remaining 210 seconds of the boot. Four of x86-witness.spec's REQUIREs went red on a machine that
/// was working perfectly: the battery had no defect, it had no TAKER.
///
/// A deferral whose only taker is a present is not deferred, it is CONDITIONAL on a present — and
/// nothing in the policy ever said so. This restores the property the threshold implies: past
/// `defer_ms`, an owed sample is taken whether or not anyone is drawing.
///
/// ### Why here, and what it costs when there is nothing to do
///
/// Reached from `bootpace::service_dump`, which the CLOCK-X1 verdict already rides for exactly this
/// reason: it is the one call all three x86 service lanes make ungated (the BSP GUI loop, the
/// `usbdebug` loop, and the SCHED-X86 `x86_usb_pump` task), so the taker reaches the media build that
/// boots on the bench. Main-loop context in every one of them — never IRQ — which is the context
/// `composite` already runs in from `service_damage` and from `sys_win_present`, and the context the
/// read-back needs because it touches the panel.
///
/// Idle cost is one relaxed load and a `cycles_to_us`; past the rate gate but with nothing owed it is
/// one table-lock acquisition and a scan of eight rows, which is `service_damage`'s own idle cost.
///
/// ### One window per pass, and why the predicate is read before the mark
///
/// The take is not performed here — it is the ORDINARY take, reached the ordinary way: mark the row
/// damaged, composite, and `wcg::begin` / `wcd_admit` open the sample themselves because
/// `paygo_clock` has genuinely opened. There is no second copy of the verify, and a window that would
/// be declined is never marked ([`wcd_ripe`] and `wcg::paygo_ripe` read the same clock the gates
/// defer on), so the taker cannot spin against its own gate.
///
/// At most one window per pass, deliberately. A full-coverage sample is not cheap — Boot V's `prof`
/// lines put the uncached panel read-back at 1.66 us/probe, so `win=1`'s full sample is ~1.6 s of
/// read-back — and taking every owed window in one pass would land the whole battery as a single
/// stall. Spread over [`PAYGO_SVC_PERIOD_US`] the same work arrives as a sequence of passes the rest
/// of the system runs between.
///
/// ### THE PACING IS THE WORK, NOT THE GATE
///
/// [`PAYGO_SVC_PERIOD_US`] is a floor on how often a take may START, and at 250 ms against a
/// ~1.6 s full-coverage sample it is not what paces the battery — the read-back is. The gate reopens
/// roughly 1.35 s before the pass it admitted has finished, so a second lane would enter while the
/// first is still reading the panel. Two things follow, and both are deliberate:
///
///  * the take is SERIALIZED by [`PAYGO_SVC_BUSY`], claimed before the pass and released after the
///    composite returns. `wcd_admit`'s CAS already declines a second lane's wc-d verdict, but
///    `wcg::begin`'s `TAKEN.fetch_add` admits BOTH, so without this the taker would manufacture two
///    or three concurrent full-coverage read-backs of one window — at 4 Hz, against the uncached
///    panel, at exactly the moment the arc is trying to measure that panel;
///  * the rate stamp is written AFTER the work, so the 4 Hz spacing is measured from the END of a
///    take. Stamping before it would make "one pass per 250 ms" a claim about when passes are
///    admitted rather than about how much read-back this taker imposes, which is the only thing the
///    number is there to bound.
///
/// ### PREDICTION for Boot W (falsifiable; write the reading beside it)
///
/// `win=1` reaches `state=complete … -> PAID` on both wires with ZERO presents in between, and it
/// arrives ~8 s after the 15 000 ms threshold, NOT within 1–2 s of it. The arithmetic, from Boot V's
/// own `prof` figure of 1.66 us/probe against the uncached panel: `win=1` sits at `taken=1 budget=4`,
/// so it owes three `[wc-g]` samples at ~1.6 s each (~964 k probes), and its `[wc-d]` battery sits at
/// `WCD_ST_FULL`, so it owes one full verdict which walks the rect TWICE — ~3.2 s. One pass spends at
/// most one wc-g sample and one wc-d stage: pass 1 is ~4.8 s (sample + full verdict), passes 2 and 3
/// are ~1.6 s each. The `>= 250 ms` spacing between passes contributes ~0.5 s of the total.
///
/// So the readings to write beside this are: `-> PAID` on both wires at `since_entry_ms` between
/// **20 000 and 25 000** (~26–27 s uptime on Boot V's entry offset); the gaps between the three
/// `[wc-g] paygo` sample lines **1.4–1.9 s each**, not 250 ms; and the `[wc-d]` full verdict's
/// `readback_us` between **2.5e6 and 4e6**. Any of those landing near ~1–2 s total means the
/// 1.66 us/probe figure or the full-coverage assumption is wrong and must be re-derived before either
/// is quoted again — a prediction a correctly working implementation falsifies is the worst shape a
/// prediction can have, because it reads as a defect where there is none and masks one where there is.
///
/// The second consequence, stated so it is not read as a regression: the boot gets ~8 s of read-back
/// back at t ≈ 15 s. GR17 moved that cost off the boot on the argument that it would be paid on a
/// live desktop; the taker pays it on a schedule whether or not the desktop is live, so any BPACE tag
/// landing after 15 s absorbs it. `storage ~11.4 s` and the `gui ~3408` band complete before the
/// threshold and are unaffected.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
pub fn paygo_service() {
    // Rate gate first: this runs on a ~1 ms lane and the scan below takes the table lock.
    let now = crate::arch::now_cycles();
    let last = PAYGO_SVC_LAST.load(core::sync::atomic::Ordering::Relaxed);
    // `now_cycles` is an absolute `rdtsc` on x86, so a zero `last` is "never run" and not a reading
    // to subtract from — `cycles_to_us` of an absolute counter overflows its `* 1e6` at ~1.9 h and
    // the gate's answer would then be noise. The first pass is always due.
    if last != 0
        && super::wcg::cycles_to_us(now.saturating_sub(last)) < PAYGO_SVC_PERIOD_US
    {
        return;
    }
    // TAKE SERIALIZATION. The gate above is a rate limiter and NOT a mutex — a second lane that
    // arrives 250 ms into a 1.6 s read-back passes it honestly. This is the mutex, and it is the only
    // thing that makes "one full-coverage take at a time" true. Losers return; there is no queue,
    // because a take deferred to the next service pass is exactly what the next service pass is for.
    if PAYGO_SVC_BUSY
        .compare_exchange(
            false,
            true,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }
    paygo_service_pass();
    // Stamped from the END of the take, not the start — see the note on the pacing above. Written
    // before the flag is released so no lane can observe an unlocked taker with a stale stamp and
    // start a second pass back-to-back.
    PAYGO_SVC_LAST.store(crate::arch::now_cycles(), core::sync::atomic::Ordering::Release);
    PAYGO_SVC_BUSY.store(false, core::sync::atomic::Ordering::Release);
}

/// PAYGO-TERM — one service pass, run with [`PAYGO_SVC_BUSY`] held. Split out of [`paygo_service`] so
/// the flag has exactly one release site: every early return below is a return from HERE, and the
/// caller's release runs on all of them.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn paygo_service_pass() {
    // Nothing is payable before the threshold, whatever the table holds. One shared read, from the
    // same definition the two gates defer on.
    if !super::wcg::paygo_clock().2 {
        return;
    }
    let mut marked = None;
    // The STOP-NOTE gets its OWN slot. Sharing `marked` was the defect: a capped row wrote its note
    // into the slot and a later takeable row overwrote it in the same pass, and because the note was
    // armed by the equality `tries == PAYGO_SVC_MAX + 1` — a counter that keeps climbing — it could
    // never be armed again. The taker gave up on that window and said nothing, forever.
    let mut stop_note = None;
    {
        let mut t = table();
        for r in t.rows.iter_mut() {
            // `presented` and `!compat`. The two halves have DIFFERENT reasons and the first cut
            // gave them one, which was right for one wire and wrong for the other:
            //   * `!compat` mirrors both gates exactly — `verify_reference` (`r.compat`) and
            //     `wcg::begin` (`compat`) each decline a compat row, so marking one buys a repaint
            //     and no sample on either wire;
            //   * `presented` mirrors only `verify_reference`. `wcg::begin` has NO `presented` test
            //     at all (it tests `compat || surf == 0 || surf_len == 0 || i >= IDS`), and
            //     `wcg::PAYGO_PEND`'s own note says so. So this predicate is deliberately WIDER than
            //     the wc-g gate it sits in front of, and the consequence is worth stating: a window
            //     drawn by `create_inner`'s composite but never presented can open a wc-g battery,
            //     be declined, and then be invisible to this taker forever — its battery closes only
            //     via `paygo_at_close`'s UNSPENT terminal. That is the intended trade. Marking an
            //     unpresented window would buy a full wc-g read-back of a surface its owner has
            //     never published, on the taker's schedule rather than the owner's, and the sample
            //     would carry no wc-d verdict to pair with. The terminal covers the honesty half.
            // A HIDDEN window is left alone too — the composite loop's `above_shell` guard would
            // decline to draw it, and re-marking it every pass would be a 4 Hz repaint of a window
            // nobody can see.
            if !r.used || !r.presented || r.compat {
                continue;
            }
            let i = r.id as usize;
            if !(super::wcg::paygo_ripe(i) || wcd_ripe(i)) {
                continue;
            }
            // THE BOUND, and it is not decoration. Every predicate above is a property of state the
            // composite path itself moves, so the ordinary case terminates: each pass spends a sample
            // and the battery closes. If some future guard declines to draw a row this taker believes
            // is drawable, the pair would spin at `PAYGO_SVC_PERIOD_US` for the rest of the boot,
            // repainting a window forever and never taking anything. So the attempts are counted and
            // capped, and the cap SPEAKS once rather than going quiet — a taker that gave up is a
            // fact about the instrument, and this module's standing law is that an instrument which
            // stops must say so.
            let tries = PAYGO_SVC_TRIES[i].fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
            if tries > PAYGO_SVC_MAX {
                // Armed by a LATCH, not by an equality on the counter. See [`PAYGO_SVC_NOTED`]. The
                // row stays eligible to speak on every later pass until it HAS spoken, so a pass
                // that also finds a takeable row (or a second row capping in the same pass) delays
                // the note by a pass instead of consuming it. And the `continue` stands: a capped
                // row must not stop the scan, or one wedged window would starve every later one.
                if PAYGO_SVC_NOTED[i].swap(1, core::sync::atomic::Ordering::AcqRel) == 0 {
                    stop_note = Some(r.id);
                }
                continue;
            }
            r.damaged = true;
            // Whole box, not a band: the deferred pass is the FULL-coverage one by definition, and a
            // banded mark would hand `verify_reference` a band to narrow the very verdict this taker
            // exists to complete.
            r.dmg_y0 = 0;
            r.dmg_y1 = 0;
            marked = Some(r.id);
            break;
        }
    }
    // AT DETECTION — the same pass that armed it, before the take below and independent of whether
    // there is a take at all. One line lower than the `swap` that armed it only because WEDGE-7's
    // `table()` masks IRQs for the guard's lifetime and this module's standing rule is that no TABLE
    // critical section prints, blocks or allocates; the guard drops on the line above.
    //
    // `paygo-taker` and NOT `paygo`: every `[wc-d] paygo` line carries the battery key set
    // (`state=`/`emit=`/`deferred=`/`taken=`) and both x86-witness.spec and `serial-analyzer --wcg`
    // parse it positionally. A diagnostic wearing that tag with a different shape is a line that
    // reads as a battery line and is not one — the exact class of instrument this module keeps
    // convicting. A distinct tag matches no directive in any spec, which is right: there is nothing
    // here to require and nothing to forbid.
    if let Some(id) = stop_note {
        serial_println!(
            "[wc-d] paygo-taker STOP-NOTE win={} — gave up after {} attempts: the row is marked damaged and the composite declines to sample it. Battery left owed, nothing forced ::",
            id, PAYGO_SVC_MAX
        );
    }
    if marked.is_some() {
        composite();
    }
}

/// PAYGO-TERM — the MINIMUM gap between the END of one [`paygo_service`] take and the start of the
/// next. 4 Hz, as a floor.
///
/// It is a floor and not a cadence, and the difference is the whole of the pacing note on
/// `paygo_service`: one full-coverage sample of the console window is ~1.6 s of uncached read-back,
/// so this number governs the idle gap between passes and the WORK governs everything else. A matured
/// `win=1` battery therefore closes ~8 s after the threshold, of which this constant contributes
/// ~0.5 s. Slow enough that the read-backs arrive as separate passes rather than one stall, which is
/// what it is for; it was never fast enough to make the battery close in 1–2 s, and the earlier claim
/// that it was assumed the gate paced the passes.
///
/// Not tied to `wcg::CENSUS_PERIOD_US`: that one paces a PRINT, this one paces WORK, and pinning them
/// together would make either number un-tunable without moving the other.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
const PAYGO_SVC_PERIOD_US: u64 = 250_000;

/// PAYGO-TERM — the taker's own liveness bound. See the note at the `fetch_add` that reads it.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
const PAYGO_SVC_MAX: u32 = 16;

/// PAYGO-TERM — `now_cycles()` as of the END of the last taken pass; `0` = never. The rate gate.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static PAYGO_SVC_LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// PAYGO-TERM — a take is running RIGHT NOW. The taker's mutual exclusion; see the pacing note on
/// [`paygo_service`] for why the rate gate could not serve as one.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static PAYGO_SVC_BUSY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// PAYGO-TERM — per-id: marks this taker has made. Bounded by [`PAYGO_SVC_MAX`].
///
/// Reset PER TENANT, in `create_inner`, and not at close. The reset used to sit on the last line of
/// [`paygo_at_close`], behind two early returns — and the case that took the early return was the
/// SUCCESSFUL one (a window whose taker did its job owes nothing, so `paygo_pending || wcd_pending`
/// is false and the function returns before the reset). The next tenant of that slot then inherited
/// the count, and after ~16 cumulative ripe passes across the slot's life the taker was capped for
/// every future tenant of it — capped SILENTLY, because [`PAYGO_SVC_NOTED`]'s one-shot had been spent
/// by an earlier tenant. Re-armed where the id demonstrably names a new window, beside the batteries.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static PAYGO_SVC_TRIES: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// PAYGO-TERM — per-id: this tenant's taker has already said it gave up.
///
/// The STOP-NOTE's one-shot, and it is a latch rather than the equality `tries == PAYGO_SVC_MAX + 1`
/// for the reason the arming site gives: an equality on a counter that keeps climbing is armed on
/// exactly one pass, and if that pass has nowhere to put the note the note is lost forever. A latch
/// keeps the row eligible until it has actually spoken.
///
/// Per TENANT, cleared in `create_inner` with [`PAYGO_SVC_TRIES`]: the budget the note reports on is
/// re-armed there, so a note left standing would make the next tenant's own give-up silent.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static PAYGO_SVC_NOTED: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// Knob off, or not x86: there is no deferral, so there is nothing to take. Folds away entirely.
#[cfg(not(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo")))]
#[inline]
pub fn paygo_service() {}

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
    let mut t = table();
    let mut n = 0usize;
    for i in 0..MAX_WINDOWS {
        if !t.rows[i].used || t.rows[i].damaged {
            continue;
        }
        if boxes_overlap(rect, outer_box(&t.rows[i])) {
            t.rows[i].damage_all();
            n += 1;
        }
    }
    n
}

/// Number of live windows.
pub fn count() -> usize {
    let t = table();
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
/// CURSOR-7 — a FOURTH outcome, layered on the three above rather than replacing any of them. A
/// present in this pass (or a concurrent one) wrote over a live sprite that no bracket had handed
/// back, so the panel no longer holds the arrow the module believes is on it. The blit itself cannot
/// repair that — it runs inside the `BlitGuard` window, where `SPRITE` may not be taken — so the
/// repair is a whole-sprite [`super::cursor::repaint`] here, outside the guard, on exactly the footing
/// the `Repaint` tail has had since WC-I. See `cursor::PRESENT_DIRTY`.
pub fn composite() {
    // COMPOSITE-2 — the whole-pass clock. Starts before the inner pass (which self-reports its
    // sprite/wait/loop terms) and stops after the tail, so `pass_us` bounds everything a present
    // pays for its composite, including the tail's sprite work.
    //
    // FBCON-DMG — a PASS clock takes no band, and not because one was unavailable to it. A pass
    // composites every dirty window plus everything the occlusion closure dragged in, each with its
    // own `bands[i]` (and `None` for every window the closure added), so there is no single band for
    // this interval to be narrowed by. What it measures is what ran, which is already the banded cost
    // once the loop below is charging banded extents: the band moves this number by shortening the
    // work, not by dividing the reading. The same holds for all six inner clocks — only the two
    // EXTENT counters in `draw_window` had an arithmetic dependence on the band, and they are the
    // only place this widening changed arithmetic.
    #[cfg(feature = "witness")]
    let c2_t0 = crate::arch::now_cycles();
    let mut tail = composite_inner();
    #[cfg(feature = "witness")]
    let c2_t1 = crate::arch::now_cycles();
    // CURSOR-7 — read BEFORE the tail runs, so a repaint the tail is already going to do is not
    // duplicated, and a pass that would otherwise have done nothing at all is upgraded.
    let dirty = super::cursor::take_present_dirty();
    if dirty && tail == CursorTail::Untouched {
        tail = CursorTail::Repaint;
    }
    #[cfg(feature = "witness")]
    note_cursor_tail(tail);
    match tail {
        CursorTail::Adopt => {
            // `adopt_overlay` is the ONLY closer of the overlay session, so `Adopt` is never
            // downgraded — a pass that skipped it would leak the session and lock the whole overlay
            // mechanism out for the rest of the boot. The repair is appended AFTER it instead: the
            // session is closed and the sprite is then re-established from the finished front.
            super::cursor::adopt_overlay();
            if dirty {
                super::cursor::repaint();
            }
        }
        CursorTail::Settle => {
            // CURSOR-15 — the sessionless compose-through tail: the arrow never left the glass, and
            // the settle is per-pixel against the finished front. Like `Adopt`, a repair request is
            // appended rather than substituted — the settle answers only the pass's own deferrals.
            super::cursor::settle_nosession();
            if dirty {
                super::cursor::repaint();
            }
        }
        CursorTail::Repaint => super::cursor::repaint(),
        CursorTail::Untouched => super::cursor::ensure_drawn(),
    }
    // COMPOSITE-2 — close the ledger: the tail interval is sprite work, the whole interval is the
    // pass. One `now_cycles` read serves both accounts.
    #[cfg(feature = "witness")]
    {
        use core::sync::atomic::Ordering::Relaxed;
        let end = crate::arch::now_cycles();
        let pass = end.saturating_sub(c2_t0);
        C2_SPRITE_CYC.fetch_add(end.saturating_sub(c2_t1), Relaxed);
        C2_PASSES.fetch_add(1, Relaxed);
        C2_PASS_CYC.fetch_add(pass, Relaxed);
        C2_PASS_MAX_CYC.fetch_max(pass, Relaxed);
    }
}

/// What [`composite_inner`] owes the sprite when it returns.
///
/// `Untouched` and `Repaint` are WC-I's two answers, unchanged and with unchanged meanings.
/// `Adopt` is CURSOR-3's: the pass undrew the sprite AND painted it back inside a staged present, so
/// the panel is already correct and only the module's bookkeeping is outstanding. Every early exit
/// from the pass owes `Repaint` once the bracket has been taken — `Adopt` is reachable only from the
/// one path that actually composed the sprite into a back layer.
/// `Settle` is CURSOR-15's: the pass DEFERRED sessionlessly (`cursor::defer_nosession`), so the
/// arrow is still on glass and the tail owes each deferred pixel a verdict against the finished
/// front — never a bracket.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CursorTail {
    Untouched,
    Repaint,
    Settle,
    Adopt,
}

/// The composite pass proper. Private so the cursor bracket above cannot be bypassed — every caller
/// (including this module's own teardown paths) goes through [`composite`].
fn composite_inner() -> CursorTail {
    // COMPOSITE-2 — the pre-pass (drain + cursor bracket) clock opens here. Band-free for the reason
    // `composite`'s clock is: this interval runs before the table snapshot that produces `bands` at
    // all, so no band is even in scope here, let alone one this reading could be narrowed by.
    #[cfg(feature = "witness")]
    let c2_pre0 = crate::arch::now_cycles();
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
    //
    // CURSOR-4 — and this is where the PAINT SET is collected. The bracket's question was "does any
    // window meet the sprite"; the answer this pass needs is "which extents may be painted over it",
    // because the undraw is now masked to exactly those (see `cursor::undraw_within`). The set is the
    // same conservative one the boolean was derived from — every live window above the shell whose
    // outer box meets the sprite — so it can only be too large, never too small, and a pixel outside
    // it is a pixel this pass provably cannot reach.
    let mut disturbed = false;
    // CURSOR-15 — the pass composed through the sprite WITHOUT a session (`defer_nosession`): the
    // arrow is on glass and the tail owes the deferred pixels a `Settle`, never a bracket.
    let mut deferred = false;

    // WC-L — DRAIN THE DEFERRED ERASES.
    //
    // CURSOR-5 MOVED THIS AHEAD OF THE CURSOR BRACKET, and the move is a correctness fix rather than
    // a tidy-up. WC-L placed the drain between `overlay_open` and the window loop, so a pass could
    // open an overlay session, mask-undraw the sprite for it, and then — from its OWN core, inside
    // its own session — call `cursor::undraw()` through the drain. That full undraw takes the sprite
    // down and bumps its generation; the session's copy of the plan is unchanged, so every
    // `compose_into` downstream still matched, painted the arrow into a window's back layer, and
    // presented it to the panel while the sprite module believed itself off-panel. The next
    // save-under then read the front, captured the overlay's own `FILL`, and left the arrow standing
    // in the window's rect until something else damaged that window. **That is Peter's P64 "flash in
    // the vug display", and this ordering is what removes it**: with the drain first, a drain that
    // undraws leaves `sprite_plan()` empty, no session is opened at all, and the pass's `Repaint`
    // tail puts the sprite back from the finished front. A drain that paints nothing (the common
    // case: an empty queue costs one relaxed load) leaves the bracket below exactly as it was.
    //
    // `cursor::compose_into`'s generation check closes the same hole for the callers this ordering
    // cannot reach — `wm::erase` and `repaint` running on another core — so the two fixes are the
    // same fix applied to the two halves of the race, not a belt and braces.
    //
    // Everything WC-L's placement argument required is preserved. The drain still runs BEFORE the
    // dirty-set snapshot, so the windows its `damage_intersecting` reaches are repainted by THIS
    // pass; it still runs outside the F4 `BlitGuard` window, so neither `SPRITE` nor `TABLE` enters
    // the drain barrier's wait set. It reports `disturbed` when it took the sprite off the panel and
    // not when it painted, for MUST-FIX 1's reason: a drain whose boxes all re-defer has still
    // undrawn, and an `Untouched` tail there would leave the pointer missing for as long as the
    // contention lasts.
    {
        let fb = *super::WRITER.lock();
        if fb.is_ready() && drain_deferred(&fb) {
            disturbed = true;
        }
    }

    let mut plan: Option<super::cursor::Plan> = None;
    let mut session = false;
    // CURSOR-12 — the denominator. Bumped here rather than at function entry so it counts passes that
    // actually reached the bracket decision, which is the population every term is a fraction of.
    #[cfg(feature = "witness")]
    CUR12_PASSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // ONE acquisition, bound once: a second `sprite_plan()` call for the counter would take `SPRITE`
    // again and could disagree with the one the pass actually brackets on.
    let sprite_now = super::cursor::sprite_plan();
    #[cfg(feature = "witness")]
    if sprite_now.is_none() {
        CUR12_NOSPRITE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    if let Some(p) = sprite_now {
        let sbox = (p.bx, p.by, p.bw, p.bh);
        let shell = shell_z();
        let mut paint: [(usize, usize, usize, usize); MAX_WINDOWS] = [(0, 0, 0, 0); MAX_WINDOWS];
        let mut npaint = 0usize;
        #[allow(unused_mut)]
        let mut hit = {
            let t = table();
            for r in t.rows.iter() {
                if r.used && above_shell(r, shell) && boxes_overlap(sbox, outer_box(r)) {
                    paint[npaint] = outer_box(r);
                    npaint += 1;
                }
            }
            npaint > 0
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
        // CURSOR-12 — the sprite was up but nothing was under it. Counted before the `hit` branch so
        // the terms stay mutually exclusive and sum to `passes`.
        #[cfg(feature = "witness")]
        if !hit {
            CUR12_NOHIT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        if hit {
            // CURSOR-4 — the split is taken only when the overlay session is ours AND no WC-F
            // reserved box is in play (that probe paints the FRONT after this pass, so pixels a
            // layer delivered would be overwritten and the plan would describe a panel that no
            // longer holds it). Either exclusion falls back to CURSOR-3's whole-sprite bracket,
            // which is correct and exactly as good as today.
            if !reserved_hit && super::cursor::overlay_open(&p) {
                session = true;
                plan = Some(p);
                // CURSOR-11 — DEFER the handback instead of taking it. This is the P73 fix and it is
                // one call: `undraw_within` handed the paint set's sprite pixels back HERE, before a
                // single window had composed, so the panel published an arrow-less box for the whole
                // of the off-screen compose plus the row blit — once per present, which is the blink
                // Peter sees over a PRESENTING vug (together with that vug's own fps overlay, for the
                // same reason and at the same rate). `defer_within` writes no pixels at all: the arrow
                // stays on glass, `compose_into` paints it into the staged rows as it already did, and
                // the rows that land on it already contain it. The undraw's justification — hand a
                // pixel back before a painter takes it, or its save-under goes stale — is answered at
                // the TAIL instead, per pixel and against the finished front, where it is answerable
                // exactly. See `cursor::defer_within` and `cursor::settle_pending_locked`.
                //
                // The paths that still bracket are unchanged and deliberately so: the WC-F arm below,
                // the sessionless arm below it, and an `adopt_overlay` whose session came back
                // incoherent. CURSOR-9's `TOUCHED_SINCE_DRAW` repair machinery serves all three and is
                // untouched.
                super::cursor::defer_within(&paint[..npaint]);
                #[cfg(feature = "witness")]
                CUR3_PLANNED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            } else if reserved_hit {
                // WC-F's probe paints the FRONT at the tail of this pass, OUTSIDE any window box, so
                // the paint set below does not describe what it will touch and `repair` (which
                // damages windows) could not mend a sprite pixel it took. The whole sprite comes off,
                // as it has since CURSOR-3. Witness/baremetal-only, one region.
                //
                // CURSOR-9 — and the probe is one of the two front-buffer painters that do NOT reach
                // `draw_window`, so it arms the repair explicitly rather than being heard about
                // through `note_present_over_sprite`. Without this the probe's pixels inside the
                // sprite box would be restored over without the affected windows being damaged.
                super::cursor::note_sprite_touched();
                super::cursor::undraw();
                #[cfg(feature = "witness")]
                {
                    CUR3_DECL_BUDGET.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    // CURSOR-12 — attributed to the WC-F probe specifically, so `[cursor12]`'s
                    // `reserved` can be read against `budget`, which also carries the per-window
                    // `may_overlay` exclusions.
                    CUR12_RESERVED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    // CURSOR-11 — a pass whose arrow left the glass. Kept, not fixed: the probe paints
                    // the FRONT after this pass, outside every window box, so no staged present can
                    // carry the sprite through it.
                    super::cursor::note_bracketed_pass();
                }
            } else {
                // CURSOR-15 — THE SESSIONLESS PASS COMPOSES THROUGH TOO. This arm is the P82 hover
                // stutter: another pass owns the overlay session — under a presenting vug fleet the
                // steady state, several cores compositing against one session — and until this arc
                // the loser took `undraw_within_nosession` (a masked handback) plus a `Repaint` tail
                // (a whole-sprite `refresh_locked`). One erase-and-rebuild of the arrow per
                // overlapping present, ~123/s on P82, against a pointer redrawn at event cadence:
                // `[flick2] sess_undraws` climbing at present rate was those tail refreshes landing
                // inside other passes' open sessions, and `down_slow` was the intervals they cost.
                //
                // The handback's justification — hand a pixel back before a painter in THIS pass
                // overwrites it, or its save-under goes stale — is answered the same way CURSOR-11
                // answered it for the session owner: at the tail, per pixel, against the finished
                // front, where it is answerable exactly (`cursor::settle_nosession` re-saves each
                // taken pixel from the freshly-composited content BEFORE painting the arrow back —
                // the FLICKER-2/3 session-fresh discipline extended to compose-through). The arrow
                // stays on glass; nothing is handed back, so CURSOR-5's stale-stamp interleave has
                // no first move; and the one duty the old generation bump performed for a concurrent
                // owner — retiring its coverage where our blit overwrites a pixel its layer composed
                // — is carried by `cursor::overlay_uncover_any` from `draw_window`, per painted box,
                // exactly as CURSOR-4 retires the identical intra-pass hazard.
                //
                // The tail is `Settle`, not `Repaint`: no session means no coverage to install, and
                // no bracket means nothing to refresh — only the deferred verdicts are owed.
                super::cursor::defer_nosession(&paint[..npaint]);
                deferred = true;
                #[cfg(feature = "witness")]
                {
                    CUR3_DECL_LOCK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    // CURSOR-12 — the session refusal on its own, separated from `compose_into`'s
                    // contended `try_lock` inside the guard. `CUR3_DECL_LOCK` counts both and cannot
                    // answer "did the offer die before it was made, or after". Still counted: the
                    // refusal still happens; CURSOR-15 changed the RESPONSE, not the predicate.
                    CUR12_NOSESSION.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    // CURSOR-15 — no `note_masked_nosession` (no masked undraw is taken — that
                    // counter now measures a mechanism this arm no longer runs, and must read 0) and
                    // no `note_bracketed_pass` (the arrow does not leave the glass here any more;
                    // `[cursor11] passes=` picks this pass up via `defer_nosession` instead).
                }
            }
            // CURSOR-15 — `disturbed` only on the arms that actually took pixels down (`reserved`,
            // and the session arm keeps it for `tail_of`'s `Adopt`, per CURSOR-11's widened
            // reading). The deferring sessionless arm sets `deferred` instead: a `disturbed` there
            // would turn its tail into the `Repaint` bracket this arc exists to remove.
            if session || reserved_hit {
                disturbed = true;
            }
        }
    }
    #[cfg(feature = "witness")]
    {
        WCI_CURSOR_PASSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // CURSOR-15 — `deferred` counts here too: this counter has meant "the pass's tail owes the
        // sprite its pixels" since CURSOR-11 widened `disturbed`, and a deferring pass owes exactly
        // that. Keeping it out would make the WC-I ratio read as a bracket collapse it is not.
        if disturbed || deferred {
            WCI_CURSOR_BRACKETS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }
    // COMPOSITE-2 — pre-pass closed, wait clock opens: everything from here to the top of the blit
    // loop is serialisation (WRITER read, TABLE lock, damage close, ordering, guard registration).
    #[cfg(feature = "witness")]
    let c2_wait0 = {
        let t = crate::arch::now_cycles();
        C2_SPRITE_CYC.fetch_add(
            t.saturating_sub(c2_pre0),
            core::sync::atomic::Ordering::Relaxed,
        );
        t
    };

    // WEDGE-1 — the framebuffer handle is taken BEFORE the guard. **Hardening, not a fix for a
    // diagnosed defect: the P66 wedge's mechanism is UNKNOWN and this is not it.**
    //
    // The first cut of this change claimed the old ordering was the wedge, on the argument that
    // `WRITER` is ordered under `SPRITE` and so admitted the sprite lock into the drain barrier's
    // wait set. That argument is WRONG and is recorded here so it is not re-derived: every one of the
    // 35 `WRITER.lock()` sites in the tree is a single-statement `Copy` read (`*WRITER.lock()`) whose
    // guard dies at the semicolon, so no holder ever blocks on a second lock; and `SPRITE` is the
    // OUTER lock of the pair, so the implication runs the other way round anyway.
    //
    // What survives is the safe direction. `DrainBarrier::drain` spins IRQ-masked and unpreemptible
    // (reached from `sched::exit`, which masks first), and the fewer blocking acquisitions inside the
    // window it waits on, the tighter its termination argument is. The handle is `Copy` and nothing
    // between here and the old site consumed it, so this is a pure ordering change: same value, same
    // early return, one fewer blocking lock inside the guard. The `is_ready` early return now precedes
    // the registration, which is strictly better on its own — a pass that cannot draw no longer
    // registers as in-flight at all.
    // WEDGE-2 `<F7>` (owner core) / `<f7>` (any OTHER core compositing while the chain is open) — the
    // deferred-erase drain and the cursor bracket are behind us; the `WRITER` read, the `TABLE`
    // snapshot and the `BlitGuard` registration are next. This is the region WEDGE-1 hardened and the
    // region whose drain barrier WEDGE-1's silent tripwire exonerated, so a `<F7>`/`<f7>` terminus is
    // the strongest single fact this instrument can produce.
    //
    // The lowercase twin is the reason the vug storm is in the evidence at all: six vugs present
    // continuously, so several cores are inside this pass whenever a TAB lands, and a wire that ends
    // `<F7><f7><f7>` reads very differently from one that ends `<F7>` alone. Passes that run with NO
    // focus change in flight stay silent — otherwise the steady-state present rate would bury the
    // chain.
    crate::wedge2::mark_composite("<F7>", "<f7>");
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        // WC-N — a pass that produced no pixels for any window. See `WCN_ABORTED`.
        #[cfg(feature = "witness")]
        wcn_note_pass(false);
        return tail_of(disturbed, session, deferred);
    }

    // F4 — the drain barrier. Register this composite as in-flight WHILE STILL HOLDING the table
    // lock, so the registration is ordered against any teardown that takes the lock afterwards: a
    // `close_owner` that clears rows can then tell whether some other core snapshotted those rows
    // before the clear and is still blitting from their (about to be unmapped) surfaces.
    let (rows, mut dirty, mut bands, _blit) = {
        let mut t = table();
        // F4 — the barrier, observed in the SAME critical section as the registration below. A
        // teardown raises it after clearing its rows, so seeing it up means there is nothing of that
        // ASID's left to draw and the teardown will recomposite when it finishes: skipping is both
        // correct and what makes the drain terminate (`BLIT_ACTIVE` can only fall while it is up).
        // WC-I: `disturbed`, not `false` — the bracket above may already have taken the sprite off the
        // panel, and the caller's tail is what puts it back. Every early exit from here on owes the
        // same answer.
        if DRAIN_PENDING.load(core::sync::atomic::Ordering::Acquire) != 0 {
            // WC-N — aborted under the F4 barrier. The `wcn_note_pass` call is one relaxed increment
            // and takes no lock, so it is safe inside this critical section; keeping it here rather
            // than after the early return is what makes the abort count exact.
            #[cfg(feature = "witness")]
            wcn_note_pass(false);
            return tail_of(disturbed, session, deferred);
        }
        let mut dirty = [false; MAX_WINDOWS];
        // FBCON-DMG — the band travels with the dirty flag and is cleared with it, in the SAME
        // critical section, so a `present_rows` that lands after this snapshot re-damages the row
        // rather than having its rows absorbed by a pass that is no longer going to draw them.
        // Taken here and not one statement earlier or later on purpose: WC-L's drain ordering (the
        // drain runs before this snapshot and outside the `BlitGuard` window) is untouched, and this
        // loop is the one place `damaged` is already read-and-cleared under the table lock.
        let mut bands = [None; MAX_WINDOWS];
        for (i, r) in t.rows.iter_mut().enumerate() {
            dirty[i] = r.used && r.damaged;
            if dirty[i] && r.dmg_y1 > r.dmg_y0 {
                bands[i] = Some((r.dmg_y0, r.dmg_y1));
            }
            r.damaged = false;
            r.dmg_y0 = 0;
            r.dmg_y1 = 0;
        }
        let guard = BlitGuard::enter();
        // FLUID-3 — sample the in-flight depth AT registration (self included), under the same
        // table lock the guard is ordered by. Two relaxed RMWs; see the ledger above `fluid3_emit`.
        #[cfg(all(target_arch = "aarch64", feature = "witness"))]
        {
            let d = BLIT_ACTIVE.load(core::sync::atomic::Ordering::Relaxed);
            FL3_DEPTH_MAX.fetch_max(d, core::sync::atomic::Ordering::Relaxed);
            if d > 1 {
                FL3_OVERLAP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
        (t.rows, dirty, bands, guard)
    };

    // Close the damage set upwards over occlusion, to a fixed point (at most MAX_WINDOWS passes).
    //
    // FBCON-DMG — the closure now reads the DAMAGED region of `i` (its band-clipped box) rather than
    // its whole box, and the windows it drags in are promoted to a WHOLE-BOX repaint. Both halves are
    // the conservative direction: a narrower `bi` can only reach fewer windows, and every window it
    // does reach repaints at least the rows `i` is about to overwrite. A `j` that was itself banded is
    // widened here too, which is why `bands[j].is_some()` re-enters the fixed point — without it a
    // banded window could stay banded while a lower window repainted rows outside that band.
    for _ in 0..MAX_WINDOWS {
        let mut grew = false;
        for i in 0..MAX_WINDOWS {
            if !dirty[i] {
                continue;
            }
            let bi = damaged_box(&rows[i], bands[i]);
            for j in 0..MAX_WINDOWS {
                if !rows[j].used || rows[j].z <= rows[i].z {
                    continue;
                }
                if dirty[j] && bands[j].is_none() {
                    continue;
                }
                if boxes_overlap(bi, outer_box(&rows[j])) {
                    dirty[j] = true;
                    bands[j] = None;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
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

    // WEDGE-2 `<F8>` (owner core) / `<f8>` (a concurrent core) — the guard is HELD, the damage set is
    // closed and ordered, and the back-to-front BLIT LOOP is next. The pairing is what makes this
    // token worth its bytes: `<F7>` with no `<F8>` means the death is in the guard-registration
    // window (`WRITER`/`TABLE`/`BlitGuard`), while `<F8>` with no `<F9>` means it is in the blit loop
    // proper — `draw_window`, the WC-G/WC-D witnesses, or the sprite overlay they drive.
    crate::wedge2::mark_composite("<F8>", "<f8>");
    // COMPOSITE-2 — wait closed, loop clock opens. `bands` exists by here, but it is a per-window
    // array over the whole dirty set: the interval this clock covers is the SUM over that set, so
    // there is no single band to charge it against. The banded cost shows up in the reading itself,
    // because a banded window's `draw_window` copies fewer rows inside this interval.
    #[cfg(feature = "witness")]
    let c2_loop0 = {
        let t = crate::arch::now_cycles();
        C2_WAIT_CYC.fetch_add(
            t.saturating_sub(c2_wait0),
            core::sync::atomic::Ordering::Relaxed,
        );
        t
    };
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
            // WC-N — this row was damaged and this pass declined to repaint it. The compositor's end
            // of the same fact `present`'s `hid` counts from the owner's end.
            #[cfg(feature = "witness")]
            wcn_note_below(rows[i].id);
            continue;
        }
        // WC-D — the REFERENCE, taken BEFORE the blit. See `verify_reference` and the WCD-PRE section
        // of `verify_window`'s ledger for why it cannot be taken any later: the read-back's reference
        // used to be snapshotted from inside `verify_window`, which runs after `wcg::end` and
        // `stage_flush`, and both of those PRINT. With the console routed into a window,
        // `serial_println!` lands in that window's OWN surface and `route_present_banded` declines the
        // present (the compositor is already inside `composite`), so the source legitimately gains
        // pixels that are legitimately not on the panel yet — and the instrument read that mutation as
        // corruption. The reference has to be the bytes `draw_window` consumed, so it is captured here.
        //
        // Before `wcg::begin`, not between it and `draw_window`: this reads the whole content rect
        // (multiple megabytes for the console window), and inside WC-G's bracket that read would be
        // charged to `[wc-g] us=` — manufacturing a `slow=yes` tear report out of this witness's own
        // cost. WC-G's bracket must still contain the copy and nothing else.
        #[cfg(feature = "witness")]
        let wcd_ref = verify_reference(&fb, &rows[i], bands[i]);
        // WC-G — bracket the blit. `begin` must be the last thing before `draw_window` and `end` the
        // first thing after it: the `blit`/`after` checksums mean "the surface as the copy found it"
        // and "as the copy left it", and anything inserted between them widens the interval they
        // measure into something other than the copy. Budgeted per window id; `None` once spent.
        // GR21/WCD-OCC — the occluder set as of the blit is handed to `wcg::begin` on x86, so the
        // glass read-back can excuse a pixel a higher window owns exactly as `[wc-d]` does. aarch64
        // keeps the four-argument call and its byte-identical wire.
        #[cfg(all(feature = "witness", target_arch = "x86_64"))]
        let wcg_probe = super::wcg::begin(
            rows[i].id,
            rows[i].surf,
            rows[i].surf_len,
            rows[i].compat,
            occluders_above(rows[i].z, rows[i].id),
        );
        #[cfg(all(feature = "witness", not(target_arch = "x86_64")))]
        let wcg_probe = super::wcg::begin(rows[i].id, rows[i].surf, rows[i].surf_len, rows[i].compat);
        // CURSOR-3 — WHICH WINDOWS MAY CARRY THE SPRITE. WC-I's invariant "no verified pixel is ever
        // read with the sprite on the panel" is preserved here rather than weakened: this pass may
        // read this window's destination pixels back and compare them against its SOURCE surface —
        // `wcg::end`'s `fbbad` count and `verify_window`'s scan-out verdict both do exactly that — and
        // a cursor legitimately composited into those pixels would read as a blit defect. Both
        // instruments are budgeted one-shots, so declining the overlay for the handful of passes they
        // run on costs those passes WC-I's bracket and nothing else. Non-witness builds have neither
        // instrument and no condition to test.
        //
        // CURSOR-4 — the exclusion now suppresses the COMPOSE, not the plan. The plan's geometry is
        // still handed down, because a window that paints over sprite pixels without composing them
        // must CLEAR their coverage (`cursor::overlay_uncover`), or a lower window's layer save would
        // survive under pixels this window has just overwritten — and the tail would then decline to
        // repaint them. Excluding the window from the plan entirely is what would break the split.
        #[allow(unused_mut)]
        let mut may_overlay = true;
        #[cfg(feature = "witness")]
        if wcg_probe.is_some() {
            may_overlay = false;
            // CURSOR-12 — attributed, and only when there was an offer to lose. A pass with no plan
            // was never going to compose anything, so charging it here would drown the real signal.
            if plan.is_some() {
                CUR12_EXCL_PROBE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
        // WCD-PRE — `wcd_ref.is_some()` is now part of the test, and it has to be. The VERIFIED latch
        // used to be claimed AFTER this block, so on the verifying pass the `== 0` arm was still true
        // and the sprite was correctly withheld from the rows WC-D was about to read back. Moving the
        // reference (and with it the claim) ahead of the blit inverts that read: the bit is already set
        // by the time we get here. The armed reference is the same fact, asked of the pass that owns
        // the read-back rather than of the latch.
        #[cfg(feature = "witness")]
        {
            let r = &rows[i];
            if !r.compat && r.presented && r.id < 32 {
                let bit = 1u32 << r.id;
                if wcd_ref.is_some()
                    || VERIFIED.load(core::sync::atomic::Ordering::Relaxed) & bit == 0
                {
                    may_overlay = false;
                    if plan.is_some() {
                        CUR12_EXCL_UNVERIFIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        }
        #[cfg(feature = "witness")]
        if plan.is_some() && !may_overlay {
            CUR3_DECL_BUDGET.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        // FOCUS-HL: `focus == 0` is shell focus and highlights nothing — and the explicit `!= 0` also
        // keeps a compat row (owner ASID 0) from matching it by accident.
        overlaid |= draw_window(
            &fb,
            &rows[i],
            focus != 0 && focus == rows[i].owner_asid,
            plan,
            may_overlay,
            // CURSOR-7 — `disturbed`, not `plan.is_some()`: the question the present-overlap test has
            // to answer is "does this pass's TAIL owe the sprite a repaint", and both `Adopt` and
            // `Repaint` do. It is final by here (nothing below the bracket block writes it).
            // CURSOR-15 — `deferred` answers the same question yes: the `Settle` tail owes every
            // deferred pixel its verdict, so a deferring pass must not arm `PRESENT_DIRTY` either.
            disturbed || deferred,
            // FBCON-DMG — the band this window's damage was declared over. `None` for every window
            // dragged in by the occlusion closure and for every whole-box present, which is every
            // caller that predates this arc.
            bands[i],
        );
        #[cfg(feature = "witness")]
        if let Some(p) = wcg_probe {
            let r = &rows[i];
            // GR21/WCD-OCC — the read-back-time occluder set, unioned in `end` with the pre-blit set
            // carried in the probe. x86 only; aarch64 keeps the eight-argument call.
            #[cfg(target_arch = "x86_64")]
            super::wcg::end(
                p, &fb, r.x, r.y, r.w, r.h, r.stride, r.scale, occluders_above(r.z, r.id),
            );
            #[cfg(not(target_arch = "x86_64"))]
            super::wcg::end(p, &fb, r.x, r.y, r.w, r.h, r.stride, r.scale);
        }
        // WC-H — print the back-layer sample the blit above recorded, if any. Deliberately AFTER
        // `wcg::end`: this emits to the serial UART, and inside the bracket it would be charged to
        // `[wc-g] us=`. See `wcg::stage_flush`.
        #[cfg(feature = "witness")]
        super::wcg::stage_flush(rows[i].id);
        // WC-D — verify this window's blit against the scan-out, once per window id, from inside the pass
        // that drew it (the only place both the source surface and the destination rows are known).
        //
        // The eligibility test and the one-shot latch moved up to `verify_reference`; what is left here
        // is the read-back itself, which must stay AFTER `stage_flush` so `[wc-d]` keeps its place in
        // the log behind `[wc-g]`/`[wc-h]`. That those two printed into this window's surface in the
        // meantime no longer matters: the reference was frozen before the blit.
        #[cfg(feature = "witness")]
        if let Some(vr) = wcd_ref {
            verify_window(&fb, &rows[i], vr);
        }
        // WC-N — pixels on glass for this window id. The only writer of `comp`.
        #[cfg(feature = "witness")]
        wcn_note_drawn(rows[i].id);
        drawn += 1;
    }
    // COMPOSITE-2 — loop closed. The witness one-shots inside it (WC-G/WC-D/WC-C) are charged here
    // when they fire; they are budgeted per window id, so the steady state they perturb is a handful
    // of early passes and never the rollup average this line is read for.
    #[cfg(feature = "witness")]
    C2_LOOP_CYC.fetch_add(
        crate::arch::now_cycles().saturating_sub(c2_loop0),
        core::sync::atomic::Ordering::Relaxed,
    );
    // WC-N — the pass reached the blit loop (`drawn == 0` is still a pass: it means every damaged row
    // was below the shell, which is a fact about the shell and not about the pass).
    #[cfg(feature = "witness")]
    wcn_note_pass(true);

    #[cfg(feature = "witness")]
    if drawn > 0 && !COMPOSITE_WITNESSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        let live = rows.iter().filter(|r| r.used).count();
        serial_println!("[wc-a] composite windows={} drawn={}", live, drawn);
    }
    // WC-C — the SIDE-BY-SIDE witness. The arc's claim is "two user programs, two windows, both on the
    // panel at once"; a screenshot shows it to a human but proves nothing to a gate, and the per-window
    // `[wc-a] create` lines say only that rows EXISTED, never that two were composited in one pass. This
    // fires from inside the pass that actually drew them, and checksums each window's SOURCE bytes, so a
    // window that is present-but-blank (or that composited a stale/recycled surface) is distinguishable
    // from one that drew real content. FNV-1a over `surf_len` — the mapping-code length, the same bound
    // `draw_window` reads under, so the checksum can never walk past the slot.
    //
    // One-shot: this runs from present context at user-mode frame rates, and the checksum is a 64 KiB read.
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
    let _ = overlaid;
    tail_of(disturbed, session, deferred)
}

/// CURSOR-3 — what the pass owes the sprite, from the two facts the pass records.
///
/// CURSOR-4 changes the second fact from "did some window carry the sprite" to "does this pass own
/// the overlay session", and the change is forced rather than cosmetic: the undraw is now MASKED, so
/// a pass that opened a session has left sprite pixels off the panel whether or not any layer took
/// them, and only `adopt_overlay` knows how to settle them (install the covered ones, repaint the
/// rest, close the session). A `Repaint` tail there would leave the session open for the boot.
///
/// `session` implies `disturbed` by construction — the session is only ever opened on the branch
/// that undraws — and the assertion is cheap enough to keep as a debug one.
///
/// **CURSOR-11 — `disturbed` no longer means "the sprite is off the panel", and the widening is
/// deliberate.** The session arm now DEFERS the handback rather than taking it, so on that path the
/// arrow is still on glass when this returns. What `disturbed` has always been used for is unchanged
/// and is what the name should be read as: *this pass's tail owes the sprite its pixels*. Both
/// consumers stay exactly right under the wider reading — `tail_of` needs `Adopt`, and
/// `adopt_overlay` settles every deferred pixel; and `draw_window`'s `bracketed` argument asks
/// `note_present_over_sprite` "does this pass's tail owe the sprite a repaint", which a deferring
/// pass answers yes to as firmly as a bracketing one. A pass that deferred must therefore NOT arm
/// `PRESENT_DIRTY`, and with `disturbed == true` it does not.
///
/// **CURSOR-15 — `deferred` is the sessionless deferral, and `disturbed` outranks it.** A pass that
/// BOTH deferred and disturbed (the drain took a masked handback in the same pass that later
/// deferred) has `off` pixels only a whole-sprite refresh can put back; `Repaint`'s
/// `refresh_locked` resets `pend` as it goes, so the deferral is settled by the bracket rather than
/// dropped. `Settle` is taken only when the deferral is the pass's sole debt.
fn tail_of(disturbed: bool, session: bool, deferred: bool) -> CursorTail {
    debug_assert!(!session || disturbed);
    if session && disturbed {
        CursorTail::Adopt
    } else if disturbed {
        CursorTail::Repaint
    } else if deferred {
        CursorTail::Settle
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
///
/// ### WCD-LIVE — the reference must hold still (lift of x86 76c724d6)
/// Both passes derive the expected pixel from the SOURCE surface, which makes that surface the instrument's
/// reference — and that surface is user-writable. A single-threaded app is quiescent inside `present`, so the
/// reference is a constant and the comparison is sound. A multi-threaded one is not: sibling threads keep
/// painting while verify reads, and the witness compares scan-out against source bytes that were never
/// blitted. The tell is `bad_cache != bad_ram` — the two passes disagreeing with each OTHER, which no blit
/// defect can produce, since the blit is over before either pass starts — with counts that reshuffle every
/// run. The exposure is not closed by the app's own frame barrier either: verify runs from ANY pass that
/// repaints these rows (a neighbour's present growing the dirty set, a desktop flush, a cursor repaint),
/// and the one-shot latch is claimed by whichever reaches it first.
///
/// The first correction was **one source read**: the verdict used to take three independent reads of a
/// mutable surface (pass 1, post-`IVAC` pass 2, and the checksum inside the print), so the two counts could
/// differ with no cache story at all. The source rect is snapshotted ONCE and both passes compare against
/// that single `want`-stream. The two-pass design is a claim about the DESTINATION's cache state and never
/// wanted the source re-read; with the snapshot, a surviving `bad_cache != bad_ram` is again a real one.
///
/// The second was a **liveness bracket** around the snapshot. WCD-PRE below replaces it; see there for why a
/// whole-surface bracket could not survive a routed console, and what took its place.
///
/// LIVE counts as neither PASS nor FAIL; a REQUIRE asserting the LIVE line for a threaded app would be a
/// deliberate spec change neither seat has made.
///
/// The owner's-own-present gate was deliberately NOT taken: owner presents are not quiescent either with
/// free-running workers, and re-gating the one-shot VERIFIED latch would move clean verdicts.
///
/// ### WCD-PRE — the reference is taken BEFORE the blit (x86, two boots of false FAIL)
/// The snapshot above fixed the passes' disagreement with each OTHER but left the reference itself taken
/// from the wrong INSTANT. `verify_window` runs after `draw_window`, after `wcg::end` and after
/// `stage_flush`, and the last two PRINT. Once the console is routed into a window, `serial_println!` fans
/// out through `fbcon::_print` into that window's own surface; `route_present_banded` merges the glyphs into
/// the pending band and then declines the present, because the compositor is already inside `composite` and
/// the re-entry guard refuses (the hazard is written out verbatim at the head of `fbcon`'s routing). So
/// between the blit and the read-back the SOURCE legitimately gains pixels that are legitimately NOT on the
/// panel yet — and the witness, snapshotting there, adopted them as its reference and reported the
/// difference as corruption:
///
/// ```text
/// [wc-d] verify win=1 surf=1312x736 ... checked=965632 bad_cache=74544 bad_ram=74544
///        nonzero=33200 first=(788,553) got=0x000000 want=0xc0c0c0 -> FAIL
/// ```
///
/// Three things convict the instrument rather than the compositor. `0xc0c0c0` is `fbcon`'s `FG_DEFAULT` —
/// console glyph ink, not chrome. `nonzero` was byte-identical on every PASS and every FAIL across both
/// captures, so the DESTINATION never moved; only the reference did. And `[wc-g]`, two lines earlier, ran the
/// SAME comparison from a pre-blit reference and printed `fbbad=0/965632`.
///
/// The reference is therefore captured by [`verify_reference`] from the composite loop, before the blit and
/// before `wcg::begin` — the bytes `draw_window` actually consumed. The one-shot latch moves with it, since
/// whoever takes the reference is who owes the verdict.
///
/// That relocation also retires the whole-surface liveness bracket, which could not survive here: its closing
/// read would have to happen after the blit, every position after the blit is either inside WC-G's timed
/// bracket (where a multi-megabyte source read would be charged to `[wc-g] us=`) or after the prints (where
/// the console's surface has moved BY CONSTRUCTION), and a bracket that reports `-> LIVE` on every console
/// verdict is an instrument that cannot fail. Liveness is now asked PER PIXEL and only of pixels that already
/// disagree: a mismatching destination pixel whose SOURCE no longer holds the value in `want` had its
/// reference move under it and is counted `moved=` instead of being charged to the blit. A clean blit never
/// reaches that re-read at all, so the common path pays nothing for it, and the attribution is finer than the
/// whole-verdict veto it replaces — one live pixel no longer vetoes a million still ones. The residual hole is
/// named honestly: a genuine blit defect at a pixel whose source ALSO moved afterwards is attributed to
/// liveness and not charged. The old bracket had the same hole and a coarser one, since any moving pixel
/// vetoed the whole rect.
///
/// ### WCD-BAND — a banded present never promised the whole box
/// `composite` hands `draw_window` the FBCON-DMG band the damage was declared over, and the staged path then
/// repaints only those rows. Asserting whole-surface equality after such a present is unsound quite apart from
/// the print hazard: the rows outside the band hold whatever the previous present left, and the source has
/// moved on since. The verified rect is therefore CLIPPED to the band, and `band=` on the wire says which rows
/// the verdict covers.
///
/// Clipping rather than waiting for a whole-box present, deliberately. Deferring the one-shot would make it
/// hostage to a caller that may never issue one — the routed console bands every present it makes after the
/// first, so the verdict would simply never be emitted and the spec's REQUIRE would fail with no line at all
/// to say why, which is the failure mode the `-> SKIP` discipline exists to prevent. The clipped rect is
/// smaller but every pixel in it is earned: the band is the region this pass promised to repaint, and BOTH
/// paths repaint it (the direct fallback ignores the band and paints the whole box, a superset).
///
/// A band that clips empty declines the pass WITHOUT claiming the latch, unlike the geometry `-> SKIP`s. An
/// empty band is a property of one present, not of the row, so burning a one-shot on it would cost the window
/// its only verdict over a transient; a degenerate row will still be degenerate next pass, so claiming and
/// naming it is right there.
///
/// ### WCD-RAMINDEP — what `bad_ram` is worth on x86
/// The two-pass design rests on a BARE invalidate between the passes, and `aarch64` has one (`DC IVAC`).
/// x86_64 does not: `CLFLUSH`/`CLFLUSHOPT` write dirty lines back before invalidating them — the exact
/// `CIVAC` mistake this ledger already rejects, an instrument healing the defect it claims to measure — and
/// `INVD` discards every line in the cache, which is not a diagnostic, it is a crash. So the invalidate is
/// `aarch64`-only, and on x86 the second pass re-reads the same destination through the same cache state as
/// the first.
///
/// Printing two numbers that are equal by construction is what invited the misreading above, so the arch is
/// now on the wire as `ram_indep=yes|no`. The second pass is still RUN on x86 rather than faked, because
/// there it is not vacuous — it is a STABILITY re-read of the destination: `bad_cache != bad_ram` with
/// `ram_indep=no` means something wrote the panel under the verdict. What it is not, and now says it is not,
/// is an independent statement about whether the pixels reached the memory the scan-out reads.
#[cfg(feature = "witness")]
fn verify_window(fb: &super::FrameBuffer, r: &Window, vr: VerifyRef) {
    let info = fb.info();
    let VerifyRef { row0, row1, cols, banded, cksum_pre, want, step, running,
        #[cfg(target_arch = "x86_64")] seq,
        #[cfg(target_arch = "x86_64")] occ_before } = vr;
    let wi = r.id as usize;
    // GR21/WCD-OCC — the occluder set as of the READ-BACK (post-blit). Taken once, here, before the
    // multi-second glass read the passes run, so the table lock it briefly holds never overlaps that
    // read. Unioned with `occ_before` per pixel below: a mismatching pixel covered by EITHER snapshot
    // is one a higher window owns, which conservatively excuses a window that moved between the two
    // instants (the AK case, one re-tile removed). A window that enters mid-read is the same residual
    // hole WCD-PRE already names for a source that moves mid-read — those pixels are not on the glass
    // to be read. x86 only; see [`OccSnap`].
    #[cfg(target_arch = "x86_64")]
    let occ_after = occluders_above(r.z, r.id);

    // WCD-PRE — the per-pixel liveness question, asked only of pixels that already disagree. `want` was
    // frozen before the blit; if the SOURCE no longer holds that value, this pixel's reference moved
    // between the snapshot and now (a sibling app thread, or — on the routed console — `fbcon::_print`
    // fanning `[wc-g]`/`[wc-h]` into this very surface), and the disagreement is not the blit's to answer.
    let source_px = |row: usize, col: usize| -> u32 {
        // SAFETY: `verify_reference` established the same bound `draw_window` reads under —
        // `row < surf_len / stride` and `col < stride / 4`, so `row * stride + col * 4 + 4 <= surf_len`.
        let p = (r.surf as *const u8).wrapping_add(row * r.stride + col * 4) as *const u32;
        let v: u32 = unsafe { core::ptr::read_unaligned(p) };
        v & 0x00FF_FFFF
    };

    let pass = |fb: &super::FrameBuffer| {
        let mut checked = 0usize;
        let mut bad = 0usize;
        let mut moved = 0usize;
        let mut nonzero = 0usize;
        let mut first = (0usize, 0usize, 0u32, 0u32);
        // WCD-TEARDOWN — the FIRST pixel charged to a moved reference, tracked beside `first`. The
        // abort arm can now fire on `moved > 0` with both bad counts zero, and `first` is only ever
        // written by the `bad` arm — so without this the line printed `first=(0,0) got=0x00000000
        // want=0x00000000`, fabricating fbcon black: the exact colour that falsified the first model,
        // on the arm built to diagnose the next one.
        let mut first_moved = (0usize, 0usize, 0u32, 0u32);
        // GR21/WCD-OCC — pixels that mismatch AND lie under a higher window. Counted here, charged
        // to neither `bad` nor `moved`: a higher window legitimately owns the destination, so the
        // console's blit is not the writer being adjudicated. x86 only.
        #[cfg(target_arch = "x86_64")]
        let mut occluded = 0usize;
        for row in row0..row1 {
            // WC-D/PAYGO — the lattice's per-row phase. A full pass (`step == 1`) starts at column 0 and
            // advances by one, which is the `for col in 0..cols` loop this was before, exactly. A sampled
            // pass rotates its first column by one per row, so over any `step` consecutive rows every
            // column is probed exactly once and a one-pixel-wide vertical defect cannot sit in the gaps
            // for longer than that. EVERY row is visited either way, and every `scale`x`scale` upscale
            // cell of a probed column is probed whole — which is what makes a full-row band un-missable
            // at any step. `step` is never 0: `verify_reference` supplies it and collapses it to 1 below
            // the rect's own width. (Named `col0` and not `first`: this closure already has a `first`,
            // the coordinates of the earliest chargeable mismatch, and shadowing it here would silently
            // retype the thing the FAIL line prints.)
            let col0 = if step == 1 { 0 } else { row % step };
            let mut col = col0;
            while col < cols {
                let want = want[(row - row0) * cols + col];
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
                            // WCD-PRE — attribute before charging. The re-read costs nothing on a clean
                            // blit, which never gets here.
                            if source_px(row, col) != want {
                                if moved == 0 {
                                    first_moved = (dx, dy, got, want);
                                }
                                moved += 1;
                            } else {
                                // GR21/WCD-OCC — attribute to an occluder before charging `bad`, after
                                // the WCD-PRE re-read above and only on a pixel that already disagrees,
                                // so a clean blit never reaches the walk. A pixel covered by the
                                // pre-blit OR the read-back occluder set is owned by a higher window.
                                #[cfg(target_arch = "x86_64")]
                                {
                                    if occ_before.covers(dx, dy) || occ_after.covers(dx, dy) {
                                        occluded += 1;
                                    } else {
                                        if bad == 0 {
                                            first = (dx, dy, got, want);
                                        }
                                        bad += 1;
                                    }
                                }
                                #[cfg(not(target_arch = "x86_64"))]
                                {
                                    if bad == 0 {
                                        first = (dx, dy, got, want);
                                    }
                                    bad += 1;
                                }
                            }
                        }
                    }
                }
                col += step;
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            (checked, bad, moved, nonzero, first, first_moved, occluded)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            (checked, bad, moved, nonzero, first, first_moved)
        }
    };

    #[cfg(target_arch = "x86_64")]
    let (checked, bad_cache, moved_cache, nonzero, first_cache, firstmv_cache, occ_cache) =
        pass(fb);
    #[cfg(not(target_arch = "x86_64"))]
    let (checked, bad_cache, moved_cache, nonzero, first_cache, firstmv_cache) = pass(fb);

    // Discard, never clean — see the doc comment. Bare `IVAC` is what makes `bad_ram` able to fail.
    // WCD-BAND: the extent follows the verified rows, not the whole window, so a banded verdict
    // invalidates only the scanlines it is about to re-read.
    // WCD-RAMINDEP: `aarch64` only, and x86 says so on the wire rather than pretending.
    #[cfg(target_arch = "aarch64")]
    {
        let row_bytes = info.stride * info.bytes_per_pixel;
        let y0 = (r.y + row0 * r.scale).min(info.height);
        let y1 = (r.y + row1 * r.scale).min(info.height);
        if y1 > y0 {
            crate::arch::cache::invalidate_range(
                fb.base_addr() + y0 * row_bytes,
                (y1 - y0) * row_bytes,
            );
        }
    }
    let ram_indep = cfg!(target_arch = "aarch64");

    #[cfg(target_arch = "x86_64")]
    let (_, bad_ram, moved_ram, _, first_ram, firstmv_ram, occ_ram) = pass(fb);
    #[cfg(not(target_arch = "x86_64"))]
    let (_, bad_ram, moved_ram, _, first_ram, firstmv_ram) = pass(fb);
    // `cksum` is the `[wc-c]` FNV over the SOURCE slot, carried here so a verdict is content-aware: without
    // it a blank surface blitted faithfully onto a blank rect is a PASS indistinguishable from a verified
    // crystal. `nonzero` is the same question asked of the DESTINATION. `cksum_pre` is the same FNV taken at
    // reference time; the pair is now DIAGNOSTIC only — WCD-PRE explains why a whole-surface bracket cannot
    // be a verdict input on a routed console — but it stays printed on the LIVE line, where the question
    // "did the surface move at all" is exactly what the reader wants next.
    let cksum = surface_checksum(r);
    // The worse of the two passes: a pixel whose reference moved during EITHER read-back is unadjudicated,
    // and taking the max keeps a single moving pixel from being averaged out of the line.
    let moved = moved_cache.max(moved_ram);
    // GR21/WCD-OCC — the worse of the two passes, same rule as `moved`: a pixel excused as occluded
    // in EITHER read-back is unadjudicated by the console's blit, and the max keeps a single such
    // pixel visible on the line. `ok` is UNCHANGED — occluded pixels were never charged to `bad`, so
    // a verdict is CLEAN/PASS iff the non-occluded, non-moved mismatches are zero.
    #[cfg(target_arch = "x86_64")]
    let occluded = occ_cache.max(occ_ram);
    let ok = bad_cache == 0 && bad_ram == 0;
    let live = ok && moved > 0;
    let first = if bad_cache > 0 { first_cache } else { first_ram };
    // WCD-TEARDOWN — the moved-arm counterpart, from whichever pass actually charged a moved pixel.
    // x86 only: the abort arm that reads it is, and aarch64 must not grow a dead local.
    #[cfg(target_arch = "x86_64")]
    let first_moved = if moved_cache > 0 { firstmv_cache } else { firstmv_ram };
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (firstmv_cache, firstmv_ram);
    let band = BandFmt(if banded { Some((row0, row1)) } else { None });
    // WC-D/PAYGO — `checked` is the honest denominator and always was, but a denominator does not say WHY
    // it is small. This does, positionally, right beside the count it qualifies. Empty on every build but
    // an x86 `wcg-paygo` one, so the three lines below stay byte-identical elsewhere.
    let coverage = wcd_coverage_note(step);
    // WCD-TEARDOWN — close the panel-write window. Taken AFTER the last probe and before the first
    // print, so it covers exactly the interval the verdict rests on and nothing this function does
    // to the panel afterwards. x86 only; see the WCD-TEARDOWN ledger for why the arch gate is a
    // protection boundary and not a scoping convenience.
    #[cfg(target_arch = "x86_64")]
    let seq_end = panel_seq_close();
    // WCD-TEARDOWN — the rect this verdict actually read back, in PANEL coordinates, so the fill test
    // asks "did a fill land HERE" rather than "did a fill happen".
    #[cfg(target_arch = "x86_64")]
    let vrect = (
        r.x + 0,
        r.y + row0 * r.scale,
        cols * r.scale,
        (row1 - row0) * r.scale,
    );
    #[cfg(target_arch = "x86_64")]
    let stable = panel_stable(seq, seq_end, vrect);
    // WCD-TEARDOWN — THE ABORT TEST, and the `moved > 0` arm is not decoration.
    //
    // The first cut fired on `!ok` alone and had a hole big enough to swallow the very defect it was
    // written for. A foreign panel write whose pixels ALSO differ from the frozen source is attributed
    // by WCD-PRE's per-pixel re-read to `moved`, not to `bad` — so `ok` stays TRUE, the verdict prints
    // `-> LIVE`, and the repaint that produced it is invisible on a line that carried no interlock
    // field at all. `moved > 0` closes that: any disagreement this pass could not charge to the blit
    // is now adjudicated against the panel's stability, whichever bucket it landed in.
    //
    // The first cut also claimed a foreign write "cannot manufacture agreement". That is RETRACTED:
    // where `want` equals the colour being written, a desktop-colour fill over a genuinely garbled
    // pixel HEALS the mismatch and the rect clears on pixels the blit never produced. Neither arm here
    // can see it — `bad` is 0 and `moved` is 0, because the source never moved — so it lands on the
    // PASS arm, which is why the PASS line carries the interlock reading too. See there.
    #[cfg(target_arch = "x86_64")]
    if !stable && (!ok || moved > 0) {
        // Odometer first, unbudgeted, before the budget test — the counter must not stop at its cap.
        let aborts = if wi < WCD_IDS {
            WCD_ABORTS[wi].fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1
        } else {
            u32::MAX
        };
        // WCD-TEARDOWN — `retry=` says whether this window will be VERIFIED AGAIN, which is not the
        // same question as whether the abort handed the reference back. A terminal stage-1 abort keeps
        // the window at `FIRST_RUN`->`DONE`... no: it publishes nothing and seals, so nothing further
        // is owed. A terminal stage-2 abort likewise seals. Either way `retry=no` is the truth about
        // the WINDOW, which is what a reader is asking. See the note in `WCD_ABORT_MAX`.
        let retry = aborts <= WCD_ABORT_MAX;
        let fst = if bad_cache > 0 || bad_ram > 0 { first } else { first_moved };
        serial_println!(
            "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} checked={}{} bad_cache={} bad_ram={} ram_indep={} moved={} nonzero={} occluded={} occ={}/{} cksum={:#018x} first=({},{}) got={:#08x} want={:#08x} rect={}x{}+{}+{} fills={}->{} fact={}/{} desk={}->{} dact={}/{} aborts={}/{} retry={} -> SKIP (teardown)",
            r.id, r.w, r.h, band, r.scale, r.x, r.y, info.width, info.height,
            checked, coverage, bad_cache, bad_ram, yn(ram_indep), moved, nonzero,
            occluded, occ_before.count(), occ_after.count(), cksum,
            // WCD-TEARDOWN — the mismatching pixel from whichever arm actually fired. `bad` writes
            // `first`, `moved` writes `first_moved`, and printing the wrong one is how the first cut
            // would have invented a black pixel on the arm meant to diagnose black pixels.
            fst.0, fst.1, fst.2, fst.3,
            vrect.2, vrect.3, vrect.0, vrect.1,
            seq.fills, seq_end.fills, seq.fill_active, seq_end.fill_active,
            seq.desk, seq_end.desk, seq.desk_active, seq_end.desk_active,
            aborts, WCD_ABORT_MAX, yn(retry)
        );
        if retry {
            // Hand the verdict back AFTER the print, so the line and the release cannot be separated
            // by a composite on another core that re-admits and prints between them.
            wcd_release(wi, running);
        } else {
            // Budget spent: stop re-reading the glass, and close the window LOUDLY. `retry=no` above
            // says the window will not be verified again; under PAYGO the battery ALSO has to say it
            // closed, or a reader following `[wc-d] paygo` sees a window that reached `taken=2`
            // without ever emitting a terminal line — sealed and silent on the one wire that tracks
            // the battery. `-> UNPAID` and not `-> PAID`: the coverage was never bought.
            wcd_seal(wi);
            #[cfg(feature = "wcg-paygo")]
            {
                let (since_ms, clock, _) = super::wcg::paygo_clock();
                wcd_paygo_note(r.id, wi, "sealed", "UNPAID", since_ms, clock);
            }
        }
        // The repair redraw still runs: this window's pixels were overwritten under us, and putting
        // them back is exactly what it is for.
        let focus = focus_asid();
        draw_window(fb, r, focus != 0 && focus == r.owner_asid, None, false, false, None);
        return;
    }
    if live {
        // WCD-PRE — every disagreement was explained by a reference that moved, so nothing is chargeable
        // to the blit, but the rect was not fully adjudicated either. Report the fact instead, with the
        // counts kept visible so the disagreement stays readable. Distinct label, never red: a live
        // surface is a property of a multi-threaded app (or of the console printing into its own window),
        // not a defect in the compositor.
        // WCD-TEARDOWN — a LIVE verdict is the one place a foreign panel write can hide with `ok`
        // still true (WCD-PRE files such a pixel under `moved`, not `bad`), so the interlock's reading
        // rides this line even when the abort did not fire. Two cfg'd emissions rather than a shim:
        // aarch64 has no interlock and its line must stay byte-identical to the pre-interlock wire.
        #[cfg(target_arch = "x86_64")]
        serial_println!(
            "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} checked={}{} bad_cache={} bad_ram={} ram_indep={} moved={} nonzero={} occluded={} occ={}/{} cksum={:#018x} cksum_pre={:#018x} fills={}->{} fact={}/{} desk={}->{} dact={}/{} -> LIVE (unverifiable)",
            r.id, r.w, r.h, band, r.scale, r.x, r.y, info.width, info.height,
            checked, coverage, bad_cache, bad_ram, yn(ram_indep), moved, nonzero,
            occluded, occ_before.count(), occ_after.count(), cksum, cksum_pre,
            seq.fills, seq_end.fills, seq.fill_active, seq_end.fill_active,
            seq.desk, seq_end.desk, seq.desk_active, seq_end.desk_active
        );
        #[cfg(not(target_arch = "x86_64"))]
        serial_println!(
            "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} checked={}{} bad_cache={} bad_ram={} ram_indep={} moved={} nonzero={} cksum={:#018x} cksum_pre={:#018x} -> LIVE (unverifiable)",
            r.id, r.w, r.h, band, r.scale, r.x, r.y, info.width, info.height,
            checked, coverage, bad_cache, bad_ram, yn(ram_indep), moved, nonzero, cksum, cksum_pre
        );
    } else if ok {
        // WCD-TEARDOWN — the interlock's reading rides the PASS line too, on x86.
        //
        // This is the verdict shape the HEALING exposure produces, and the one the first cut left
        // mute. Where a fill's colour happens to equal `want`, it repairs a genuinely garbled pixel:
        // `bad` stays 0, `moved` stays 0 because the source never changed, `ok` is true, and the rect
        // clears on pixels the blit never wrote. Neither abort arm can see it — by the time the pass
        // looks, the evidence is the fill's own output — so the only defence is that the line says a
        // fill overlapped it and a reader can ask why a PASS needed one. A distinct terminal was the
        // alternative and was rejected: it would move a clearance out from under both gates'
        // `REQUIRE`s for a condition that is usually benign, trading a rare unreadable PASS for a
        // routine red. Printing the reading keeps the verdict where the gates expect it and makes the
        // exposure visible.
        #[cfg(target_arch = "x86_64")]
        serial_println!(
            "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} checked={}{} bad_cache=0 bad_ram=0 ram_indep={} moved={} nonzero={} occluded={} occ={}/{} cksum={:#018x} first=none fills={}->{} fact={}/{} desk={}->{} dact={}/{} stable={} -> PASS",
            r.id, r.w, r.h, band, r.scale, r.x, r.y, info.width, info.height,
            checked, coverage, yn(ram_indep), moved, nonzero,
            occluded, occ_before.count(), occ_after.count(), cksum,
            seq.fills, seq_end.fills, seq.fill_active, seq_end.fill_active,
            seq.desk, seq_end.desk, seq.desk_active, seq_end.desk_active, yn(stable)
        );
        #[cfg(not(target_arch = "x86_64"))]
        serial_println!(
            "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} checked={}{} bad_cache=0 bad_ram=0 ram_indep={} moved={} nonzero={} cksum={:#018x} first=none -> PASS",
            r.id, r.w, r.h, band, r.scale, r.x, r.y, info.width, info.height,
            checked, coverage, yn(ram_indep), moved, nonzero, cksum
        );
    } else {
        // WCD-TEARDOWN — the FAIL line carries the interlock reading too, and this arm is the reason
        // the whole mechanism exists.
        //
        // Tracing boot 8's own numbers through the finished code is what forced this: a recurrence
        // driven purely by the DESKTOP LAYER moves `desk=` and moves no fill, so `stable` stays true,
        // the abort does not fire, and the verdict lands HERE — on the one arm that, without this,
        // still printed nothing about the panel. The review asked for `desk=` on the abort and LIVE
        // lines; the acceptance test says the recurrence does not reach either. A reader now sees
        // `bad_cache=0 bad_ram=197376 ... fills=4->4 fact=0/0 desk=118->121 dact=0/1` and can convict
        // the desktop layer from the line, which is the whole point of printing a term this witness
        // has deliberately declined to abort on.
        #[cfg(target_arch = "x86_64")]
        serial_println!(
            "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} checked={}{} bad_cache={} bad_ram={} ram_indep={} moved={} nonzero={} occluded={} occ={}/{} cksum={:#018x} first=({},{}) got={:#08x} want={:#08x} fills={}->{} fact={}/{} desk={}->{} dact={}/{} -> FAIL",
            r.id, r.w, r.h, band, r.scale, r.x, r.y, info.width, info.height,
            checked, coverage, bad_cache, bad_ram, yn(ram_indep), moved, nonzero,
            occluded, occ_before.count(), occ_after.count(), cksum,
            first.0, first.1, first.2, first.3,
            seq.fills, seq_end.fills, seq.fill_active, seq_end.fill_active,
            seq.desk, seq_end.desk, seq.desk_active, seq_end.desk_active
        );
        #[cfg(not(target_arch = "x86_64"))]
        serial_println!(
            "[wc-d] verify win={} surf={}x{} band={} scale={}x at ({},{}) panel={}x{} checked={}{} bad_cache={} bad_ram={} ram_indep={} moved={} nonzero={} cksum={:#018x} first=({},{}) got={:#08x} want={:#08x} -> FAIL",
            r.id, r.w, r.h, band, r.scale, r.x, r.y, info.width, info.height,
            checked, coverage, bad_cache, bad_ram, yn(ram_indep), moved, nonzero, cksum,
            first.0, first.1, first.2, first.3
        );
    }
    // WC-D/PAYGO — the battery's terminal line, emitted right behind the verdict that closed it. `step ==
    // 1` is exactly "this pass ran at full coverage", which is the condition `wcd_commit` sealed
    // `VERIFIED_FULL` on, so the two cannot disagree about whether the window still owes a verdict. A
    // sampled pass prints nothing here — its `coverage=lattice16` marker already says what it was, and the
    // `state=waiting` census will speak from the first composite this window is declined on.
    // WC-D — the verdict is published; advance the window and set the flags the rest of the module
    // reads. After the print, so a `[wc-a]`-ordering reader sees verdict-then-transition.
    wcd_commit(wi, running, step);
    #[cfg(all(target_arch = "x86_64", feature = "wcg-paygo"))]
    if step == 1 {
        wcd_complete(r.id, wi);
    }

    // Restore what the `IVAC` may have dropped: redraw the window and re-run its flush. In a correct build
    // this is a no-op repaint; in a broken one it is what keeps the diagnostic from being destructive.
    // FOCUS-HL: ONE snapshot, as `composite_inner` takes. Reading the atomic twice in a single expression
    // could straddle a focus change and evaluate the two halves against different owners.
    let focus = focus_asid();
    // CURSOR-3: `None` — this redraw exists to put the window's OWN pixels back after the invalidate,
    // and it runs on the one pass whose read-back forbade the overlay in the first place.
    // CURSOR-7: `bracketed = false`, conservatively. This redraw does not know the enclosing pass's
    // `disturbed`, and the safe direction is to assume nothing owes the sprite a repaint: a live arrow
    // under these rows then arms the tail repair. The cost of being wrong is one spurious whole-sprite
    // repaint on the one-shot verify pass per window; the cost of the other guess is an erased arrow.
    // FBCON-DMG: `None` — the WHOLE box. This is a REPAIR of whatever the invalidate above dropped,
    // not a present of what a caller declared changed, so it has no band and must not borrow one.
    draw_window(fb, r, focus != 0 && focus == r.owner_asid, None, false, false, None);
}

/// WC-D — the reference half of the verdict: the SOURCE bytes, the rect they cover, and the surface
/// checksum at the instant they were read. Built by [`verify_reference`] before the blit and consumed by
/// [`verify_window`] after it; holding the two halves in one value is what makes it impossible to take the
/// reference from the wrong side of `draw_window` again (WCD-PRE).
#[cfg(feature = "witness")]
struct VerifyRef {
    /// First and one-past-last SOURCE row of the verified rect. `row1 - row0` is the whole visible content
    /// height for a whole-box present and the FBCON-DMG band for a banded one (WCD-BAND).
    row0: usize,
    row1: usize,
    /// Source columns per row. `want` is indexed `(row - row0) * cols + col`.
    cols: usize,
    /// Whether the present that armed this reference was banded — the `band=` field on the wire.
    banded: bool,
    /// `surface_checksum` at reference time, i.e. pre-blit. Diagnostic; see the note by `cksum` in
    /// [`verify_window`] for why it is no longer a verdict input.
    cksum_pre: u64,
    /// The snapshot itself, `0x00FF_FFFF`-masked exactly as `draw_window`'s read masks. Read at FULL width
    /// on every pass, sampled or not — WC-D/PAYGO's note explains why the lattice reduces probes and never
    /// the reference.
    want: alloc::vec::Vec<u32>,
    /// WC-D/PAYGO — the SOURCE-column stride this verdict's read-back walks at, and the one the `coverage=`
    /// marker is derived from. Always 1 (full coverage) on aarch64 and on any x86 build without the
    /// `wcg-paygo` knob, which is what makes the lattice arithmetic in [`verify_window`] fold away to the
    /// `for col in 0..cols` loop it has always been. Carried in the reference rather than recomputed at the
    /// read-back so the walk and its marker cannot disagree — the same rule `wcg` applies to `PAYGO_STEP`.
    step: usize,
    /// WC-D — the RUNNING state this reference was admitted in, so a commit or a release knows which
    /// transition it owes. See [`WCD_STATE`].
    running: u32,
    /// WCD-TEARDOWN — both panel-write detectors as of the instant the blit was about to run.
    /// Compared against a closing read after the last probe; see [`PANEL_DESK_EPOCH`]. x86 only —
    /// aarch64 has no interlock at all (the arch gate is a protection boundary; see WCD-TEARDOWN).
    #[cfg(target_arch = "x86_64")]
    seq: PanelSeq,
    /// GR21/WCD-OCC — the boxes of every window ABOVE this one at reference time (pre-blit). Unioned
    /// in [`verify_window`] with a read-back-time snapshot, so a mismatching pixel a higher window
    /// legitimately owns is counted `occluded=`, not `bad=`. x86 only; see [`OccSnap`].
    #[cfg(target_arch = "x86_64")]
    occ_before: OccSnap,
}

/// WC-D — capture the read-back's reference, from the composite loop, BEFORE `draw_window` runs.
///
/// The eligibility test and the one-shot latch live here rather than at the read-back because whoever takes
/// the reference is who owes the verdict: a second core reaching this window in the same instant must find
/// the bit already claimed and decline, or two references would be taken and one verdict emitted from the
/// wrong one. See WCD-PRE in [`verify_window`]'s ledger for why the reference cannot be taken any later, and
/// WCD-BAND for why the rect is clipped.
///
/// ### Which failures burn the latch
/// The `-> SKIP` discipline is unchanged: once the bit is claimed, EVERY path emits a line, so a gate whose
/// REQUIRE fails always has a line saying why. The ordering below is what decides which failures are worth
/// claiming for. A degenerate row (no surface, zero scale, off-panel origin, empty content rect) is a
/// property of the ROW — it will still be degenerate next pass, so claiming and naming it is the honest
/// move. An empty BAND is a property of one present, so it declines without claiming and waits for a present
/// that has visible rows; burning a window's only verdict on a transient would be the same class of mistake
/// as the false FAIL this arc exists to close.
#[cfg(feature = "witness")]
fn verify_reference(
    fb: &super::FrameBuffer,
    r: &Window,
    band: Option<(usize, usize)>,
) -> Option<VerifyRef> {
    // FOCUS-VIS — `presented`, so the one-shot is not claimed by the create-time composite of a blank
    // surface. `compat` rows have no chrome and no owner to verify against; `id < 32` is the latch width.
    if r.compat || !r.presented || r.id >= 32 {
        return None;
    }
    // WC-D — which verdict does this window still owe, and may it be taken NOW? `step` is the
    // SOURCE-column stride the admitted pass runs at (`1` is full coverage, and the only answer any
    // build but an x86 `wcg-paygo` one ever gives) and `running` is the state token the commit or the
    // release owes its transition to. `None` declines — battery closed, another core already holds
    // the ONE reference, or (PAYGO) the deferral gate is shut, in which case the decline has already
    // been counted and, on cadence, printed. See [`WCD_STATE`].
    let i = r.id as usize;
    let (step, running) = wcd_admit(r.id, i)?;
    // The reference is this core's alone from here — `wcd_admit`'s compare_exchange is the winner
    // test — so every `serial_println!` below is emitted once. The geometry `-> SKIP`s call
    // [`wcd_seal`] rather than unwinding: a degenerate row will still be degenerate at the deferred
    // stage, so it closes the window instead of handing it back.

    let info = fb.info();
    if r.surf == 0 || r.scale == 0 || r.stride < 4 || r.x >= info.width || r.y >= info.height {
        wcd_seal(i);
        serial_println!("[wc-d] verify win={} -> SKIP (degenerate row/geometry)", r.id);
        return None;
    }
    // The same bounds `draw_window` blitted under — verify exactly what was drawn, never more.
    let cols = (info.width - r.x).div_ceil(r.scale).min(r.w).min(r.stride / 4);
    let rows = (info.height - r.y).div_ceil(r.scale).min(r.h).min(r.surf_len / r.stride);
    if cols == 0 || rows == 0 {
        wcd_seal(i);
        serial_println!("[wc-d] verify win={} -> SKIP (no visible content rect)", r.id);
        return None;
    }
    // WC-D/PAYGO — the lattice COLLAPSES on a rect narrower than its own step, for the reason
    // `wcg::readback` gives at length: the per-row phase runs `row % step`, so where `cols < step` every
    // row whose phase lands at or past `cols` probes ZERO pixels, and the window would be sampled one row
    // in `step` while its line still claimed `coverage=lattice16`. Not hypothetical — the reviewed boot
    // has `win=3` at `surf=8x8 scale=8x`, i.e. `cols == 8`. Below the step there is no coverage to buy,
    // so the honest answer is to take the pass at FULL coverage and say so. The EFFECTIVE step is what
    // travels in the reference from here on, so the walk, the `coverage=` marker and the latch the pass
    // claims are all read from one cell rather than three that could disagree.
    let step = if cols < step { 1 } else { step };

    // WCD-BAND — clip to the rows this present promised to repaint. The band is in SOURCE rows, which is
    // the coordinate `rows` counts in, so the clip is a `min` and not a re-derivation of `damaged_box`'s
    // panel arithmetic — there is deliberately no second copy of that conversion to disagree with.
    let (row0, row1, banded) = match band {
        None => (0, rows, false),
        Some((sy0, sy1)) => {
            let y0 = sy0.min(rows);
            let y1 = sy1.min(rows);
            if y1 <= y0 {
                // No verdict — see the ledger above. An empty band is a property of ONE present, not of
                // the row, so burning a window's only verdict on a transient would be the same class of
                // mistake as the false FAIL WCD-PRE exists to close.
                //
                // Under the state machine this is a RELEASE and not merely a return. The reference was
                // taken by `wcd_admit`'s CAS at the top of this function — earlier than the old
                // `claim()` closure, which sat below this test and so was never reached here — so
                // returning without handing the window back would wedge it in its RUNNING state and it
                // would never be verified again. The old shape could not leak; this one can, and this
                // is where it would have.
                wcd_unwind(i, running);
                return None;
            }
            (y0, y1, true)
        }
    };

    // WC-D/PAYGO — stage-appropriate, and with the EFFECTIVE step: a sampled pass claims only the
    // first-verdict latch and leaves the terminal one for the deferred pass, while a full pass — including
    // a lattice that collapsed above — closes the battery outright. See [`wcd_admit`].
    // The reference is ours from here — `wcd_admit`'s CAS made this core the only holder — so every
    // return below prints, and every return below that does NOT publish a verdict must hand the
    // window back.
    let mut want: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    if want.try_reserve_exact((row1 - row0) * cols).is_err() {
        serial_println!(
            "[wc-d] verify win={} -> SKIP (no memory for {}x{} source snapshot)",
            r.id, cols, row1 - row0
        );
        // RECORDED FOR THE PI4 WIRE: this changes aarch64 LINE COUNT even though no format changes.
        // Before, the OOM path claimed and the SKIP was terminal — one line per window, ever. Now the
        // window is handed back, so a window that hits this transiently emits a `-> SKIP (no memory)`
        // AND, later, a real verdict. `pi4-regression.spec`'s REQUIRE wants a PASS and its FORBID
        // wants no FAIL, so neither moves; a reader counting `[wc-d]` lines per window would.
        //
        // WCD-TEARDOWN — hand the window back rather than closing it. An allocation failure is a
        // property of one INSTANT, not of the row, and a `step == 1` pass (every stage-2, and a
        // collapsed lattice such as `win=3`'s 8x8 first pass) would otherwise have closed the battery
        // outright — permanently retiring a window's verdict over a transient, which is the exact
        // outcome this function's ledger says the OOM path avoids.
        wcd_unwind(i, running);
        return None;
    }
    // Read the SOURCE exactly ONCE, into a snapshot both read-back passes share. Before WCD-LIVE a verdict
    // took three independent reads of a user-mutable surface, so `bad_cache` and `bad_ram` could differ
    // purely because they had read different bytes — no cache story required. One `want`-stream, taken at
    // one instant, on the correct side of the blit.
    {
        let surf = r.surf as *const u8;
        for row in row0..row1 {
            let row_base = row * r.stride;
            for col in 0..cols {
                // SAFETY: identical bound to `draw_window`'s read — `row < surf_len / stride` and
                // `col < stride / 4`, so `row_base + col * 4 + 4 <= surf_len`.
                want.push(
                    unsafe { core::ptr::read_unaligned(surf.add(row_base + col * 4) as *const u32) }
                        & 0x00FF_FFFF,
                );
            }
        }
    }
    let cksum_pre = surface_checksum(r);
    // WCD-TEARDOWN — open the panel-write window LAST, so it spans everything whose result the
    // verdict rests on: `draw_window`'s copy and both read-back passes. An erase landing before this
    // point is harmless by construction — the blit that follows repaints over it — and including it
    // would abandon verdicts for a race that cannot reach them. See [`PANEL_FILL_EPOCH`].
    #[cfg(target_arch = "x86_64")]
    let seq = panel_seq();
    // GR21/WCD-OCC — the occluder set as of the reference (pre-blit). Read here, alongside the `want`
    // snapshot, so it describes the same instant the source bytes were frozen at; [`verify_window`]
    // unions it with a read-back-time snapshot to excuse a window that moved between the two.
    #[cfg(target_arch = "x86_64")]
    let occ_before = occluders_above(r.z, r.id);
    Some(VerifyRef { row0, row1, cols, banded, cksum_pre, want, step, running,
        #[cfg(target_arch = "x86_64")] seq,
        #[cfg(target_arch = "x86_64")] occ_before })
}

/// WC-D — `band=` on the wire: `none` for a whole-box verdict, `y0..y1` in SOURCE rows for a banded one.
/// A `Display` shim rather than a formatted `alloc::string::String`, so the verdict costs no allocation it
/// could fail.
#[cfg(feature = "witness")]
struct BandFmt(Option<(usize, usize)>);

#[cfg(feature = "witness")]
impl core::fmt::Display for BandFmt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            None => f.write_str("none"),
            Some((y0, y1)) => write!(f, "{}..{}", y0, y1),
        }
    }
}

/// WC-D — `ram_indep=` on the wire. See WCD-RAMINDEP.
#[cfg(feature = "witness")]
fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// WC-D — window ids whose [`verify_window`] verdict has already been emitted (bit `id`), so the read-back
/// runs once per window rather than once per frame. The latch is claimed by [`verify_reference`] — BEFORE
/// the blit, since WCD-PRE moved the reference there and the claim has to travel with it — so the invariant
/// that keeps the gate diagnosable is carried by that function: EVERY path that claims the bit emits a line,
/// PASS, FAIL, LIVE, or `-> SKIP` with a reason. A future early return added without a line would burn the
/// latch silently and leave the spec's REQUIRE failing with nothing to explain why.
///
/// WC-D/PAYGO splits what this latch means on an x86 `wcg-paygo` build: it becomes the FIRST
/// verdict's latch, and [`VERIFIED_FULL`] becomes the terminal one. On every other build there is
/// one verdict and this is still it.
#[cfg(feature = "witness")]
static VERIFIED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

// ---- WCD-TEARDOWN — the panel-write interlock the read-back adjudicates against -----------------
//
// x86_64 ONLY, and the arch gate is a protection boundary rather than a scoping convenience. This
// interlock can convert a `-> FAIL` into a `-> SKIP`, and `scripts/specs/pi4-regression.spec`'s
// `FORBID \[wc-d\] verify .*-> FAIL` is another track's gate reading this witness's wire. A
// feature-gated interlock would therefore have weakened a live protection on a bench this seat does
// not own and cannot re-run. aarch64 keeps the pre-interlock behaviour verbatim: one verdict, no
// abort, no release, no new field on any line.

/// WCD-TEARDOWN — the desktop layer's blit loop: a monotone count of entries, paired with
/// [`PANEL_DESK_ACTIVE`] for the in-flight half.
///
/// ### What boot 8 showed, and two models that did not survive it
///
/// ```text
/// [  25043ms] [wc-d] verify win=3 surf=128x128 band=none scale=6x at (9,21) panel=2880x1800
///     checked=589824 bad_cache=0 bad_ram=197376 ram_indep=no moved=0 nonzero=589824
///     first=(9,21) got=0x000000 want=0x1e1e1e -> FAIL
/// ```
///
/// The instrument is convicted rather than the blit, on four counts that hold whatever wrote the
/// panel: `bad_cache=0` (a blit defect is over before either pass starts, so none can be invisible to
/// pass 1 and visible to pass 2); `bad_ram=197376` is exactly 257 full destination rows of the
/// 768-row box with `first=(9,21)` at the box origin, the shape of a rectangle paint; `ram_indep=no`
/// makes pass 2 a pure stability re-read, which this file already documents to mean *"something wrote
/// the panel under the verdict"*; and `moved=0` because WCD-PRE asks whether the SOURCE moved while
/// the writer was repainting the DESTINATION.
///
/// **Model 1 — [`erase`] — was refuted by the colour.** `erase` fills [`DESKTOP_BG`], `0x002D2B55`,
/// which the wire confirms (`[wc-x] desktop-clear panel=2880x1800 bg=002D2B55`). The failing pixels
/// read `0x000000`: `fbcon`'s `BG_DEFAULT`, console background. No erase can produce that colour.
///
/// **Model 2 — an "intrusion" counter — was refuted by the code.** It counted
/// `Screen::present_background`'s return value, on the strength of that function's own doc ("wrote
/// background pixels INTO the window layer"). But that function has exactly three exits and ALL of
/// them return `false`; there has been no `true` exit since WC-I made the subtraction exact, and the
/// comment above its last `return` is visibly confused about this. The counter could never leave
/// zero, so the stability term built on it was a TAUTOLOGY and the interlock was fill-only — which is
/// the state it had already been bounced in. It would also have been the wrong SHAPE if wired: the
/// scenario it was written for (a `request_full_present` armed at close/move, the write landing
/// later) paints the VACATED box, and the subtraction succeeds against the CURRENT table there, so
/// `intruded` stays false even when background pixels land exactly where a verdict is outstanding.
///
/// **What is counted now is the desktop layer's actual blit loop** — the `for idx in 0..n` over the
/// damage set that copies background spans to glass — bracketed at its real boundaries, so the term
/// measures the writes rather than a predicate about them.
///
/// ### It is DIAGNOSTIC, not a veto, and that is a deliberate asymmetry
///
/// `desk=` is printed on every x86 verdict and is NOT in the abort test. The loop runs on every
/// present, so "a desktop blit was in flight" is true for essentially every read-back on a live
/// desktop; putting it in the test would abandon every verdict and adjudicate nothing. Scoping it the
/// way the fill ring is scoped needs the per-rect geometry of another module's hot loop, which is not
/// this arc's to restructure. So the honest arrangement is: the FILL term decides, the DESK term is
/// on the wire in full (`desk=E0->E1 dact=A0/A1`) so that the next recurrence of boot 8 is read off
/// the line instead of re-derived from the capture. Boot 8's own signature — the abort not firing
/// while `desk=` moved — is exactly what would convict the desktop layer, and it is now printable.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
static PANEL_DESK_EPOCH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WCD-TEARDOWN — desktop blit loops in flight. With [`PANEL_DESK_EPOCH`] this is a true bracket on
/// every panel-writing path of `present_background`'s main loop: a loop already running when the
/// read-back opened and still running when it closed shows as `dact` non-zero at one end or the
/// other. Named gap, one build shape: the `vugpar`+`baremetal` parallel present copies clipped
/// rects to glass and returns ABOVE this bracket — a missed span on that leg. No shipped x86 leg
/// carries those features today (`x86-all` has neither), so the gap is unreachable, but it is a
/// property of that path, not of this counter, and it is stated rather than claimed away. **That
/// named gap is precisely the exit [`WCI_INTRUSIONS`]'s `stale=`/`intrusions=` probe instruments**
/// — deliberately, because it is also the leg that performs no occluder subtraction at all — so on
/// the band path the two terms are complementary rather than redundant: `desk=` is blind there and
/// the WC-I terms speak. (`arm-pi` DOES carry all three features, so unlike here the WC-I probe on
/// that leg is reachable and costed; see [`occluders_aged`].) The
/// epoch also counts loops that ENTER, not rows written: a fully-occluded present bumps `desk=`
/// and writes nothing, so `desk=E0->E1` reads "N loops ran", never "N loops wrote".
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
static PANEL_DESK_ACTIVE: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// WCD-TEARDOWN — the desktop layer's end of the bracket. Called from `Screen::present_background`
/// around the loop that actually copies background spans to glass. See [`PANEL_DESK_EPOCH`].
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
pub(super) struct DeskWriteGuard;

#[cfg(all(feature = "witness", target_arch = "x86_64"))]
impl DeskWriteGuard {
    pub(super) fn enter() -> Self {
        PANEL_DESK_ACTIVE.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        PANEL_DESK_EPOCH.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        DeskWriteGuard
    }
}

#[cfg(all(feature = "witness", target_arch = "x86_64"))]
impl Drop for DeskWriteGuard {
    fn drop(&mut self) {
        PANEL_DESK_ACTIVE.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
}

/// WCD-TEARDOWN — desktop-colour box fills, the writer class the abort test actually consults.
///
/// **Bracketed at [`stage_fill`], not at [`erase`].** `stage_fill(.., DESKTOP_BG, ..)` has two
/// callers, `erase` and [`drain_deferred`], and the deferred drain reaches glass without passing
/// through `erase` at all, so a guard on `erase` missed a whole writer the s73 capture shows firing
/// in bulk. Guarding the fill is what makes the claim true rather than intended.
///
/// **And below the decline tests, not above them.** `stage_fill` can decline — `defer!` when `STAGE`
/// is held or the heap will not grow, `drop_fill!` on a degenerate or over-cap box — and a declined
/// fill writes NO PIXEL. Bumping the epoch above those tests made a fill that never reached the panel
/// abandon a verdict, which is a counter reporting work that did not happen.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
static PANEL_FILL_EPOCH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WCD-TEARDOWN — desktop-colour fills in flight. [`PANEL_FILL_EPOCH`] alone catches a fill that
/// starts and one that finishes inside a read-back; this catches one already running when the
/// read-back opened and still running when it closed. Both are needed, neither suffices.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
static PANEL_FILL_ACTIVE: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// WCD-TEARDOWN — recent fill boxes, so a fill in the OPPOSITE CORNER of the panel does not abandon a
/// verdict it could not have touched. A counter alone says a fill happened; the verdict's question is
/// whether one happened HERE.
///
/// Ring of [`PANEL_FILL_RING`] entries indexed by fill sequence, with the sequence republished beside
/// each box so a reader can tell a fresh slot from a recycled one. A reader that cannot confirm a
/// slot — because the ring wrapped, or because the publishing store has not landed — treats it as an
/// overlap, which is the conservative direction: a false overlap costs one re-verification, a false
/// miss charges a blit for pixels it did not write.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
static PANEL_FILL_BOX: [core::sync::atomic::AtomicU64; PANEL_FILL_RING] =
    [const { core::sync::atomic::AtomicU64::new(0) }; PANEL_FILL_RING];

/// WCD-TEARDOWN — `seq + 1` of the fill occupying each [`PANEL_FILL_BOX`] slot; 0 means never used.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
static PANEL_FILL_SEQ: [core::sync::atomic::AtomicU64; PANEL_FILL_RING] =
    [const { core::sync::atomic::AtomicU64::new(0) }; PANEL_FILL_RING];

/// Sixteen. Fills are rare (teardown, move, deferred drain) and a read-back is one composite pass, so
/// wrapping needs sixteen box fills inside a single verdict — at which point the conservative
/// fallback (treat as overlap) is also the right answer.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
const PANEL_FILL_RING: usize = 16;

/// WCD-TEARDOWN — the fill bracket. Order as coded: `ACTIVE` up first, then `EPOCH` (whose
/// `fetch_add` yields this fill's seq), then the box and its stamp published into the ring. The
/// box is therefore NOT visible to every reader that saw the epoch — a reader can observe the new
/// epoch before the box stores land. What makes "no overlapping fill went unseen" hold is not
/// the store order but the stamp check in [`panel_fill_hit`]: a slot whose stamp is not `seq + 1`
/// is treated as an overlap, so an unpublished or recycled box can only ever fail CONSERVATIVE.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
struct PanelWriteGuard;

#[cfg(all(feature = "witness", target_arch = "x86_64"))]
impl PanelWriteGuard {
    fn enter(x: usize, y: usize, w: usize, h: usize) -> Self {
        PANEL_FILL_ACTIVE.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        let seq = PANEL_FILL_EPOCH.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        let slot = (seq as usize) % PANEL_FILL_RING;
        let pack = ((x.min(0xFFFF) as u64) << 48)
            | ((y.min(0xFFFF) as u64) << 32)
            | ((w.min(0xFFFF) as u64) << 16)
            | (h.min(0xFFFF) as u64);
        PANEL_FILL_BOX[slot].store(pack, core::sync::atomic::Ordering::Release);
        PANEL_FILL_SEQ[slot].store(seq + 1, core::sync::atomic::Ordering::Release);
        PanelWriteGuard
    }
}

#[cfg(all(feature = "witness", target_arch = "x86_64"))]
impl Drop for PanelWriteGuard {
    fn drop(&mut self) {
        PANEL_FILL_ACTIVE.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
}

/// WCD-TEARDOWN — one reading of every panel-write detector.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
#[derive(Clone, Copy)]
struct PanelSeq {
    fills: u64,
    fill_active: usize,
    desk: u64,
    desk_active: usize,
}

/// WCD-TEARDOWN — the opening read. `EPOCH` before `ACTIVE`, mirrored by [`panel_seq_close`]. With
/// the orders mirrored, if the closing `active` is 0 then a fill that overlapped had not yet raised
/// `ACTIVE` at that read, so its `EPOCH` bump — which follows `ACTIVE` in program order and is
/// ordered with it by `AcqRel` — is also after the opening `EPOCH` read, and the epochs differ.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
fn panel_seq() -> PanelSeq {
    let fills = PANEL_FILL_EPOCH.load(core::sync::atomic::Ordering::Acquire);
    let fill_active = PANEL_FILL_ACTIVE.load(core::sync::atomic::Ordering::Acquire);
    let desk = PANEL_DESK_EPOCH.load(core::sync::atomic::Ordering::Acquire);
    let desk_active = PANEL_DESK_ACTIVE.load(core::sync::atomic::Ordering::Acquire);
    PanelSeq { fills, fill_active, desk, desk_active }
}

/// WCD-TEARDOWN — the closing read, mirrored (`ACTIVE` then `EPOCH`); see [`panel_seq`].
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
fn panel_seq_close() -> PanelSeq {
    let fill_active = PANEL_FILL_ACTIVE.load(core::sync::atomic::Ordering::Acquire);
    let fills = PANEL_FILL_EPOCH.load(core::sync::atomic::Ordering::Acquire);
    let desk_active = PANEL_DESK_ACTIVE.load(core::sync::atomic::Ordering::Acquire);
    let desk = PANEL_DESK_EPOCH.load(core::sync::atomic::Ordering::Acquire);
    PanelSeq { fills, fill_active, desk, desk_active }
}

/// WCD-TEARDOWN — did any desktop-colour fill land on the panel rectangle this verdict read back?
///
/// Scans the fill sequences that could have been live during the read-back: those taken inside the
/// window, plus `fill_active` entries before it to cover fills already running when it opened.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
fn panel_fill_hit(before: PanelSeq, after: PanelSeq, rect: (usize, usize, usize, usize)) -> bool {
    let first = before.fills.saturating_sub(before.fill_active as u64);
    if after.fills <= first {
        return false;
    }
    if after.fills - first > PANEL_FILL_RING as u64 {
        // The ring wrapped inside this verdict: some overlapping fill may be unreadable. Conservative.
        return true;
    }
    let (rx, ry, rw, rh) = rect;
    let mut seq = first;
    while seq < after.fills {
        let slot = (seq as usize) % PANEL_FILL_RING;
        let stamp = PANEL_FILL_SEQ[slot].load(core::sync::atomic::Ordering::Acquire);
        let pack = PANEL_FILL_BOX[slot].load(core::sync::atomic::Ordering::Acquire);
        if stamp != seq + 1 {
            // Slot recycled, or the publishing store has not landed. Cannot confirm; assume overlap.
            return true;
        }
        // Seqlock read discipline: re-validate the stamp AFTER reading the box, or a fill sixteen
        // seqs later could overwrite the slot between the two loads above and a NON-intersecting
        // newer box would suppress a real overlap — the one unsafe direction this scan has. The
        // re-load turns that interleaving into "cannot confirm; assume overlap".
        if PANEL_FILL_SEQ[slot].load(core::sync::atomic::Ordering::Acquire) != seq + 1 {
            return true;
        }
        let fx = ((pack >> 48) & 0xFFFF) as usize;
        let fy = ((pack >> 32) & 0xFFFF) as usize;
        let fw = ((pack >> 16) & 0xFFFF) as usize;
        let fh = (pack & 0xFFFF) as usize;
        if fx < rx + rw && rx < fx + fw && fy < ry + rh && ry < fy + fh {
            return true;
        }
        seq += 1;
    }
    false
}

/// WCD-TEARDOWN — was this verdict's rectangle free of desktop-colour fills for the whole read-back?
///
/// The DESK term is deliberately absent — see [`PANEL_DESK_EPOCH`] for why it is printed rather than
/// enforced. Conservative in the only safe direction: a false `false` costs one re-verification, a
/// false `true` charges a blit for pixels it did not write.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
#[inline]
fn panel_stable(before: PanelSeq, after: PanelSeq, rect: (usize, usize, usize, usize)) -> bool {
    !panel_fill_hit(before, after, rect)
}

/// WCD-TEARDOWN — per-id: read-backs abandoned because a foreign panel write ran under them.
///
/// An ODOMETER: it keeps counting past the cap that stops the RETRIES, which is the spent-budget law
/// this module applies to every capped counter. Printed as `aborts=`.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
static WCD_ABORTS: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// WCD-TEARDOWN — retries a window may spend before an abort stops handing the verdict back.
///
/// **Six, and the figure was re-derived for the regime this actually runs in.** The first cut said
/// three "against the observed rate — boot 7 ran eleven clean verdicts and boot 8 raced once", and
/// that calibration was measured on the PRE-DEFERRAL exposure while being spent in the post-deferral
/// one. PAYGO moves the full verdict out of the boot burst and onto a live desktop, where the desktop
/// layer is presenting continuously and the window set is churning — which is exactly where an
/// intrusion is likeliest, so the budget is spent in a wider regime than the one it was sized in.
///
/// The remaining exposure is stated rather than engineered away: [`WCD_ABORTS`] is per-ID and ids are
/// recycled slot aliases (slot 3 recycles seven times in the s73 capture), so a late tenant of a hot
/// slot can start with the budget partly spent and reach `retry=no` having adjudicated nothing
/// itself. That is the deliberate trade named at the recycle site — the alternative hands a
/// recycling slot fresh retries on every cycle, and one of the writers this interlock detects is
/// reached FROM the recycle path, so the bound would stop bounding. Six doubles the headroom against
/// a boot's worst observed churn while keeping the worst case at a few full read-backs, and the wire
/// says `aborts=N/6` on every abort so a boot that is actually exhausting it says so out loud.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
const WCD_ABORT_MAX: u32 = 6;

// ---- WC-D/PAYGO — the read-back paid as the desktop uses it, not as the boot starts it ---------

/// WC-D/PAYGO — window ids whose FULL-coverage verdict has been emitted. Terminal: past this bit a
/// window owes nothing, and [`VERIFIED`] alone now means only "this window's FIRST verdict is out".
///
/// ### Why the read-back got the treatment the `[wc-g]` battery got
///
/// GR17 took the witness-armed kepler block from 17 077 ms to 2 564 ms and left `[wc-d]`'s verify as
/// the largest instrument term inside it — 1 010 ms of a 2 581 ms window, three lines (boot 7 of the
/// `rmbp-gr16-s73` capture). Nearly all of it is glass: [`verify_window`] probes every DESTINATION
/// pixel of the verified rect, TWICE (the `bad_cache` pass and the `bad_ram` re-read), and each probe
/// is a read of the WC-mapped Kepler BAR across PCIe — ~1 µs apiece even after `3c05856d` narrowed
/// `FrameBuffer::read_pixel` to one aligned `u32`. That boot's three verdicts probe 481 280
/// destination pixels per pass; `2 × 481 280 × ~1.05 µs` is the second the boot pays. The two
/// full-surface FNVs (`cksum`, `cksum_pre`) are ~6 ms each against that, and the source snapshot is
/// cacheable RAM.
///
/// So: the same policy, under the SAME knob. `UNAOS_WCG_PAYGO` arms ONE pay-as-you-go regime for the
/// whole witness battery rather than one per witness — a second env var would let the two halves be
/// armed apart, and a boot with one half armed is a boot whose `kepler=` figure describes neither
/// configuration.
///
/// | stage | coverage | when | on the wire |
/// |-------|----------|------|-------------|
/// | 1 | LATTICE — every [`WCD_LATTICE_N`]th SOURCE column per row, phase rotating one column per row | immediately, as before | `coverage=lattice16` on the verdict |
/// | 2 | FULL — every source column | once [`super::wcg::paygo_clock`] passes the shared threshold | `coverage=full`, then `[wc-d] paygo … state=complete … -> PAID` |
///
/// ### Three deviations from `[wc-g]`'s shape, each because WC-D's semantics demand it
///
/// 1. **WC-G has a four-sample battery to spread out; WC-D has a one-shot latch.** There is no budget
///    here to spend more slowly, so the LATCH is split rather than the budget. A window is verified
///    twice over a boot where it used to be verified once — once cheaply and at once, once fully and
///    late — and each verdict is a complete, independent judgement of the blit that carried it, over
///    that pass's own band, against that pass's own reference. Nothing is accumulated across the two.
/// 2. **The decline PRINTS from where it is decided.** `wcg::paygo_open` may not, because it runs
///    inside `wcg::begin` — between WC-D's frozen reference and the copy that reference describes —
///    so with the console routed into a window its own glyphs would land in the surface WC-D froze
///    (the full argument is at `wcg`'s `PAYGO_PEND`). [`verify_reference`] has no such hazard: it is
///    the site that TAKES the reference, and on the declining path it takes none. Nor is any OTHER
///    window's reference outstanding at that instant — each is taken at the top of its own composite
///    iteration and consumed by [`verify_window`] at the bottom of the same one — so the line is
///    emitted directly and needs no pending slot, no flush site and no second mechanism.
/// 3. **No delta gate on the refresh.** `wcg::paygo_flush` runs on every composite pass whether or
///    not that pass declined anything, so it needs a "has the census moved" test to keep an idle
///    window quiet. This census is refreshed FROM the decline, so it has moved by construction and
///    the test would be vacuous. The rate gate and its `compare_exchange` are kept verbatim: they
///    bound the duty cycle this instrument imposes on the composite path and let exactly one core
///    print when two decline the same window at once.
///
/// ### The snapshot stays whole, and so does every leg
///
/// [`verify_reference`]'s `want` buffer is read at full width on BOTH stages. It is cacheable RAM —
/// microseconds against the glass's milliseconds — and keeping it whole is what lets
/// `want[(row - row0) * cols + col]` and `source_px(row, col)` keep their exact indices, so the
/// `moved=` re-read still asks its question of the same reference the comparison used. Sampling
/// reduces PROBES; it does not reduce legs, and every leg survives it because the lattice changes
/// only which destination pixels are visited, never what is asked of one:
///
/// - the `bad_cache`/`bad_ram` split still runs both passes over the same probe set, so the aarch64
///   `IVAC` leg and the x86 stability re-read that `ram_indep=no` names are both intact;
/// - `moved=` still attributes a disagreeing pixel to a reference that moved under it, per pixel;
/// - `nonzero=` is still the destination's own content question, over the probes taken;
/// - `cksum` / `cksum_pre` are full-surface FNVs and the step does not reach them at all;
/// - every `scale`×`scale` UPSCALE cell of a probed source column is probed WHOLE, so nothing about
///   the upscale is narrowed — which matters here in a way it does not for `[wc-g]`, whose read-back
///   only ever probed one destination pixel per cell.
///
/// What narrows is horizontal reach. A lattice pass probes one source column in [`WCD_LATTICE_N`]
/// per row, phase rotating one column per row. It therefore CATCHES: any full-row band (every row is
/// visited at every step); any horizontal garble run of [`WCD_LATTICE_N`] source pixels or more, in
/// any row, deterministically; any stride, pitch or origin fault, which displaces the whole surface
/// and agrees with no probe in any row; and a one-source-pixel-wide vertical defect within
/// [`WCD_LATTICE_N`] consecutive rows, because the phase visits every column exactly once over that
/// span. It CANNOT catch an isolated single-pixel error at a source column that row's phase does not
/// visit. That is the whole of the narrowing, and it is why stage 2 DEFERS full coverage rather than
/// dropping it — every pixel is still adjudicated, on a live desktop instead of inside the boot.
///
/// ### A lattice verdict is never readable as a full clearance
///
/// `checked=` was always the honest denominator, but a denominator does not say WHY it is small — a
/// banded present and a lattice pass over a whole box can print the same figure. `coverage=` says
/// which, on the line, between the count and the verdict that count supports. A knob-on build marks
/// the FULL verdicts too, so the marker's absence never carries the meaning; a knob-off build inserts
/// the empty string. The `-> PASS` / `-> FAIL` / `-> LIVE` terminal and the key order around it are
/// untouched, which is what keeps `scripts/specs/pi4-regression.spec`'s
/// `REQUIRE … bad_cache=0 bad_ram=0.*-> PASS` and `FORBID … -> FAIL` reading the same line they
/// always read — and aarch64 never compiles any of this, so its wire is byte-identical.
///
/// ### CURSOR-12's exclusion fires once more per window
///
/// `may_overlay` is withheld on the pass that holds a reference, so the deferred stage costs one
/// further excluded compose per window — ~15 s in, not during the boot. The exclusion is NOT extended
/// across the deferral: the test in `composite_inner` is `wcd_ref.is_some() || VERIFIED & bit == 0`,
/// and [`VERIFIED`] is claimed by stage 1, so a window waiting for its full verdict composes the
/// sprite exactly as a verified one does today. The `FALLBACK_FIXTURE` site reads the same latch and
/// is a global one-shot, so it still fires on the first window to reach it, unmoved — with the
/// consequence, stated because it is a real narrowing, that the direct-path present it forces is now
/// verified at lattice coverage rather than in full.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static VERIFIED_FULL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// WC-D — per-id state width for every per-window array in this witness. The same 32 [`VERIFIED`]'s
/// bitmask is, and for the same reason: [`verify_reference`] declines any `id >= 32` before it
/// reaches any of this. Witness-wide rather than PAYGO- or x86-gated because [`WCD_STATE`] is: the
/// verdict progression exists on every witness build, and only the abort machinery layered over it is
/// x86-only.
#[cfg(feature = "witness")]
const WCD_IDS: usize = 32;

/// The `id >= 32` guard in [`verify_reference`] and every `[AtomicU32; WCD_IDS]` above have to be the
/// same number, and `VERIFIED`'s `1u32 << id` puts a hard ceiling on it. Pinned rather than trusted:
/// the first cut carried the window's bit SHIFTED INSIDE a packed claim token, which silently threw
/// the bit away for `id >= 30` and made a release clear nothing. The state machine retired the
/// packing, and this keeps the remaining coupling honest.
#[cfg(feature = "witness")]
const _: () = assert!(WCD_IDS == 32, "WCD_IDS must match verify_reference's `id >= 32` guard");

/// WC-D/PAYGO — the lattice's column step, in SOURCE columns. `wcg`'s sixteen, taken from `wcg`, for
/// the reason its own note gives: sixteen is a coverage decision before it is a cost one, and one
/// policy under one knob should not be two constants. Pinned to the `coverage=lattice16` literal at
/// compile time — a marker whose wire says something its code does not do is the exact class of
/// instrument this file keeps convicting.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
const WCD_LATTICE_N: usize = super::wcg::PAYGO_LATTICE_N;

#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
const _: () = assert!(
    WCD_LATTICE_N == 16,
    "the `coverage=lattice16` literal must track wcg::PAYGO_LATTICE_N"
);

/// WC-D/PAYGO — per-id: composites the deferral gate declined to verify. UNBUDGETED and taken before
/// any print test, on WC-H2's law: past a gate the LINE stops and the count must not, because a
/// counter that stops counting is an instrument that lies. This is what makes "the read-back is
/// waiting" a quantity rather than an impression.
///
/// Per ID, and there is deliberately no aggregate anywhere in this block. GR17 convicted a global
/// `any()` budget gate of a ~1.3 s/boot tax for exactly that shape — eight slots of which the bench
/// occupies three, five never-spendable ones holding the gate permanently open for every window — and
/// the fix was to close it per window. Nothing here can be wrong in that way because there is nothing
/// here that reads across ids.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static WCD_DEFERRED: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// WC-D/PAYGO — per-id: `[wc-d] paygo` lines emitted for this window so far. Printed as `emit=`,
/// one-based. The reader's rule is `wcg`'s standing one: for any `win=`, the greatest `emit=`
/// supersedes every earlier line, and these lines are never summed — they are snapshots of a monotone
/// total, not deltas.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static WCD_EMIT: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// WC-D/PAYGO — per-id: the right to print the census-opening line, taken exactly once. Two cores
/// declining the same window in the same instant must not both emit `emit=1`.
///
/// It exists as a separate latch from the cadence below because the FIRST line cannot go through the
/// rate gate: [`crate::arch::now_cycles`] is an absolute `rdtsc` on x86 and `WCD_LASTROLL` starts at
/// zero, so `cycles_to_us(now - 0)` scales an absolute counter by 1e6 and wraps somewhere north of
/// ~1.9 h of machine uptime — which could hand the gate a small reading and silently swallow the line
/// that opens the census. `wcg::paygo_flush` never meets this because its first line comes off
/// `PAYGO_PEND` rather than off the cadence; this is the same guarantee reached from the other end.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static WCD_SAID: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// WC-D/PAYGO — per-id: `now_cycles()` as of the END of the most recent `[wc-d] paygo` emission. The
/// refresh's rate gate and its mutual exclusion both, exactly as `wcg::PAYGO_LASTROLL` serves that
/// module: two cores declining the same window at once both observe this value, and only the one
/// whose `compare_exchange` succeeds prints. Re-armed after the serial write, so
/// `wcg::CENSUS_PERIOD_US` bounds the duty cycle and not merely the gap between line starts.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static WCD_LASTROLL: [core::sync::atomic::AtomicU64; WCD_IDS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; WCD_IDS];

/// WC-D — the per-id verdict progression, as ONE atomic per window.
///
/// ### Why a state machine replaced the pair of bitmasks
///
/// The first cut claimed `VERIFIED` and `VERIFIED_FULL` with two independent `fetch_or`s and let the
/// stage be inferred from them. That admits a strand this witness's own one-reference invariant
/// forbids. `BlitGuard` is a COUNT, so two cores can be inside the composite loop for the same
/// window: core A claims the first verdict and takes a reference; core B, arriving past the deferral
/// threshold, reads `VERIFIED` already set, concludes "stage 2" and claims the terminal latch through
/// its own `fetch_or`. Two references are now outstanding for one window — which
/// [`verify_reference`]'s ledger states can never happen — and if A then aborts and releases the
/// first-verdict bit, the pair lands at `VERIFIED=0, VERIFIED_FULL=1`: a window terminally closed
/// with no published first verdict, permanently `may_overlay=false` (so `CUR12_EXCL_UNVERIFIED`
/// climbs without bound), and B's terminal line reading `taken=1 -> PAID`.
///
/// One cell and one `compare_exchange` per transition removes the strand by construction: a claim
/// moves the window out of a RESTING state into the matching RUNNING state, and only the core that
/// wins the CAS holds a reference. There is no ordering of two independent RMWs left to get wrong.
///
/// [`VERIFIED`] and [`VERIFIED_FULL`] survive as PUBLISHED flags — `composite_inner`'s `may_overlay`
/// test and the `FALLBACK_FIXTURE` arm read them — and are now written only when a verdict actually
/// commits, which is also what makes `taken=` on the paygo lines recycle-safe (finding: a `-> PAID`
/// could read `taken=0` when a recycle cleared the masks between the claim and the print; the state
/// cell is the single source of truth and the recycle resets it too).
#[cfg(feature = "witness")]
static WCD_STATE: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(WCD_ST_FIRST) }; WCD_IDS];

/// Resting: owes its first verdict. The state every window starts and recycles into.
#[cfg(feature = "witness")]
const WCD_ST_FIRST: u32 = 0;
/// Running: a reference for the first verdict is outstanding. At most one core is ever here.
#[cfg(feature = "witness")]
const WCD_ST_FIRST_RUN: u32 = 1;
/// Resting: the first verdict is published and the FULL one is owed. PAYGO builds only — a
/// single-verdict build goes `FIRST_RUN` straight to `DONE`.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
const WCD_ST_FULL: u32 = 2;
/// Running: a reference for the full verdict is outstanding.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
const WCD_ST_FULL_RUN: u32 = 3;
/// Terminal: this window owes nothing.
#[cfg(feature = "witness")]
const WCD_ST_DONE: u32 = 4;

/// WC-D — how many verdicts this window has PUBLISHED, read off the one cell that knows. `taken=` on
/// the wire. Recycle-safe because the recycle resets the same cell it reads.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_taken(i: usize) -> u32 {
    match WCD_STATE[i].load(core::sync::atomic::Ordering::Relaxed) {
        WCD_ST_FIRST | WCD_ST_FIRST_RUN => 0,
        WCD_ST_DONE => 2,
        _ => 1,
    }
}

/// WC-D — may this window take a reference now, and at what probe step?
///
/// `Some((step, running))` admits the pass: `step` is the SOURCE-column stride and `running` is the
/// state token the caller carries so a commit or a release knows which transition to make. `None`
/// declines — because the battery is closed, because another core holds the only reference, or
/// (PAYGO) because the deferral gate has not opened, in which case the decline is already counted
/// and, on cadence, printed.
///
/// **The gate sits above everything the pass would spend.** A declined composite takes no reference,
/// allocates no snapshot, touches no glass and moves no state.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_admit(id: u32, i: usize) -> Option<(usize, u32)> {
    loop {
        match WCD_STATE[i].load(core::sync::atomic::Ordering::Acquire) {
            WCD_ST_FIRST => {
                if wcd_cas(i, WCD_ST_FIRST, WCD_ST_FIRST_RUN) {
                    return Some((WCD_LATTICE_N, WCD_ST_FIRST_RUN));
                }
                // Lost the CAS: another core moved this window. Re-read rather than decline, so a
                // window that just transitioned is judged on its new state and not on a stale one.
                continue;
            }
            WCD_ST_FULL => {
                let (since_ms, clock, payable) = super::wcg::paygo_clock();
                // PAYGO-TERM/PAY-AT-CLOSE — a window being torn down has no later to defer into. See
                // [`WCD_FORCE`], and `wcg::PAYGO_FORCE` for the argument in full.
                let payable =
                    payable || WCD_FORCE[i].load(core::sync::atomic::Ordering::Relaxed) != 0;
                if !payable {
                    wcd_decline(id, i, since_ms, clock);
                    return None;
                }
                if wcd_cas(i, WCD_ST_FULL, WCD_ST_FULL_RUN) {
                    return Some((1, WCD_ST_FULL_RUN));
                }
                continue;
            }
            // RUNNING (another core owns the only reference) or DONE.
            _ => return None,
        }
    }
}

/// WC-D/PAYGO-TERM — per-id: pay this window's deferred verdict NOW, whatever the deferral clock
/// says. The `wcg::PAYGO_FORCE` twin, set and cleared by [`close`]'s pay-at-close and by nothing else.
/// Kept as a separate cell rather than read out of `wcg` because the two batteries have independent
/// budgets (wc-d has two STAGES, wc-g four SAMPLES) and one can close while the other is still owed;
/// a shared latch would make the first to finish disarm the second.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static WCD_FORCE: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// WC-D/PAYGO-TERM — does this window owe a deferred verdict? `WCD_ST_FULL` is the RESTING state that
/// means "first verdict published, full one owed", which is exactly the population the deferral gate
/// turns away; `WCD_SAID` narrows it to windows the gate has actually declined, for the reason
/// `wcg::paygo_pending` gives. A `_RUN` state is another core's reference and is not ours to take.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_pending(i: usize) -> bool {
    i < WCD_IDS
        && WCD_SAID[i].load(core::sync::atomic::Ordering::Relaxed) != 0
        && WCD_STATE[i].load(core::sync::atomic::Ordering::Acquire) == WCD_ST_FULL
}

/// WC-D/PAYGO-TERM — owed AND payable now. Read from `wcg::paygo_clock`, the same one definition
/// [`wcd_admit`] defers on, so the taker cannot mark a window the admit would then decline.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_ripe(i: usize) -> bool {
    wcd_pending(i) && super::wcg::paygo_clock().2
}

/// WC-D/PAYGO-TERM — arm/disarm the pay-at-close override. Paired by [`close`].
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_force(i: usize, on: bool) {
    if i < WCD_IDS {
        WCD_FORCE[i].store(u32::from(on), core::sync::atomic::Ordering::Relaxed);
    }
}

/// PAYGO-TERM — per-TENANT: this wire's battery has spoken its closing line and is shut. Bit 0 is
/// `[wc-g]`, bit 1 is `[wc-d]`.
///
/// **Per tenant, and the first cut had it per SLOT on a premise the tree refutes.** That premise was
/// "neither battery is reset when a slot recycles". Half of it is false: `create_inner` resets
/// `WCD_STATE` and `WCD_SAID` on every recycle, with its own note saying why (a recycled id would
/// otherwise inherit its predecessor's completed battery and the new window would never be verified
/// at all). So the wc-d half of a recycled slot IS a genuinely fresh battery — it can be declined,
/// reach `WCD_ST_FULL`, and close pre-maturity exactly as its predecessor did — and a latch that
/// survived the recycle denied every tenant after the first its own terminal. Slot 3 hosts seven
/// windows in the s73 capture; six of them would have died with `state=waiting` as their last word,
/// which is precisely the defect this terminal exists to remove. The wc-g half is per-slot by design
/// (`wcg::TAKEN`/`PAYGO_SAID` have no reset writer anywhere), but "has this WINDOW said its last
/// word" is a fact about the window either way, so both bits re-arm together.
///
/// Cleared in `create_inner`, beside `WCD_STATE`/`WCD_SAID` and `wcg::paygo_recycle` — the one point
/// where the id demonstrably names something new. The census totals it does NOT travel with
/// (`WCD_DEFERRED`, `WCD_EMIT`, `wcg::PAYGO_EMIT`) stay monotone per id for the reader's
/// greatest-`emit=`-supersedes rule, which is the same split `WCD_SAID`'s own note draws.
///
/// It is also the wc-d wire's "the terminal was the last word" state: [`wcd_decline`] reads it and
/// declines to re-open the census behind a terminal, the way `wcg::PAYGO_CLOSED` does for wc-g.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
static PAYGO_CLOSE_SAID: [core::sync::atomic::AtomicU32; WCD_IDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; WCD_IDS];

/// PAYGO-TERM — how many composites [`paygo_at_close`] will run to close one window's batteries.
///
/// `wcg` budgets four SAMPLES and `wc-d` two STAGES, and each pass spends at most one of each, so a
/// window at Boot V's `taken=1` needs three passes; eight is that with headroom and a hard stop. It
/// bounds a teardown path, which is the one place in this module an unbounded loop would be worst.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
const PAYGO_CLOSE_MAX: u32 = 8;

/// PAYGO-TERM — PAY AT CLOSE, and the one case where it declines to pay.
///
/// ### The defect
///
/// Boot V closed `win=4` at 13 767 ms with `deferred=1` pending and `state=waiting` as its last word.
/// The window died, its slot recycled, and the battery it had opened was never terminated on any
/// wire — not `PAID`, not anything. The `state=sealed … -> UNPAID` note next door does not cover it:
/// that fires only on the teardown-interlock abort path (`retry=no`), never on an ordinary close. A
/// reader following the battery sees a window enter `waiting` and vanish.
///
/// ### What it does, and the case it deliberately does NOT pay
///
/// **Past the threshold: pay in full.** The deferral has matured, [`paygo_service`] would have taken
/// these samples within a pass or two anyway, and the only thing special about this moment is that it
/// is the last one where the surface still exists. The force latches (`wcg::PAYGO_FORCE`,
/// [`WCD_FORCE`]) open both gates, the row is marked and composited up to [`PAYGO_CLOSE_MAX`] times,
/// and the ordinary take path produces the ordinary terminal — `state=complete … -> PAID` — with no
/// second copy of any verify anywhere.
///
/// **Before the threshold: do not pay, and say so.** This is the conflict this arc hit and it is
/// recorded rather than smoothed over. Boot V's own `prof` lines measure the uncached panel read-back
/// at 1.66 us/probe; a full `[wc-g]` sample on the 128x128@6x demo window is ~27 ms and its one full
/// `[wc-d]` verdict walks 589 824 pixels twice — order 1.5 s. Boot V closes three such windows
/// between 13.6 s and 13.8 s, inside the boot burst. Paying them at close would put several seconds
/// straight back onto the boot that GR17 took them off, which is the deferral gate being defeated by
/// its own teardown path — the exact cost this module exists to move. So the budget stays unspent,
/// and the terminal line says `state=closed … -> UNSPENT`: a window that died inside its own deferral
/// window, by design, with nothing bought and nothing owed. See `wcg::paygo_closed` for why that
/// token is not `UNPAID`.
///
/// ### AND WITH INTERRUPTS MASKED: never composite, terminal only
///
/// `close()` IS reached from a masked teardown context, and the first cut's safety argument said it
/// was not. The trace is `sched::exit` (which disables interrupts as its first act) →
/// `user_space_release` → `memory::free_user_space_by_cr3` → `syscall::win_close_slot` →
/// `wc_shim::destroy` → `wm::close` → here; `reap_killed` is the second such path and documents its
/// own IF=0 context. What made that argument load-bearing rather than pedantic is what this function
/// added: pre-GR18 `close()` ran ONE composite from that context and ran it AFTER the row was emptied,
/// so the dying window's surface was never read. Paying a matured battery in full here runs up to
/// [`PAYGO_CLOSE_MAX`] composites with the row still live and full coverage forced — multiple seconds
/// of uncached read-back at 1.66 us/probe, held with IF=0 on the exiting task's core, with any other
/// core that raises a `DrainBarrier` spinning masked behind it.
///
/// So the pay is conditioned on `!arch::irqs_masked()`. When masked, the battery is not paid and the
/// terminal is emitted alone — the `serial_println` itself is fine at IF=0 (the panic path prints
/// from worse), it is the multi-second read-back that is not.
///
/// **The masked path therefore FORFEITS the pay, and the line says so honestly rather than hiding
/// it.** A matured battery closed masked emits `state=closed … -> UNSPENT` with a `since_entry_ms=`
/// past `defer_ms=`, which is the reading: this battery was ripe and was not taken. That is a real
/// loss of coverage and it is the rare case — a window whose owner task EXITS while the window is
/// still live, past the deferral threshold, and which [`paygo_service`] had not already taken. The
/// taker runs at 4 Hz on every service lane from the moment the clock opens, so a window reachable by
/// the ordinary path has already been paid by the time its task dies; a window the taker could not
/// reach (never presented, compat, or hidden) had no payable sample to forfeit. What is left is the
/// teardown-of-a-live-window case, and for that the honest UNSPENT line beats several seconds of
/// unpreemptible read-back inside `sched::exit`.
///
/// ### PREDICTION for Boot W
///
/// The three demo windows that close pre-maturity terminate `state=closed … -> UNSPENT` on both wires
/// at ~13.6–13.8 s and cost the boot nothing; no window closes with `state=waiting` as its last line,
/// and the count of `[wc-a] close win=N` lines for a recycling slot equals the count of
/// `paygo win=N state=closed` lines for it — one terminal per tenant, not one per slot.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn paygo_at_close(id: WinId) {
    let i = id as usize;
    if i >= WCD_IDS {
        return;
    }
    // Cheapest question first, and it is false for every window the gate never declined — which on
    // an ordinary boot is most of them. Two relaxed loads, no lock.
    if !(super::wcg::paygo_pending(i) || wcd_pending(i)) {
        return;
    }
    // A row that is gone, unpresented or compat cannot be sampled: `verify_reference` declines all
    // three, so marking it would buy repaints and no verdict. The note below still fires — the
    // battery is just as terminal either way.
    let drawable = {
        let t = table();
        matches!(row(&t, id), Some(r) if r.used && r.presented && !r.compat)
    };
    // NO COMPOSITE WITH IF=0. See the masked note above for the trace that reaches here masked and
    // for what the alternative costs.
    //
    // LOAD-BEARING PLACEMENT: this reads the CALLER's IF, so it must sit outside the `table()` block
    // above. WEDGE-7's guard masks interrupts for its own lifetime, so the same call one line higher
    // would report `masked` for every caller and disable the pay-at-close pay entirely — a gate that
    // is always true is not a gate, it is a deletion.
    let masked = crate::arch::irqs_masked();
    if drawable && !masked && super::wcg::paygo_clock().2 {
        super::wcg::paygo_force(i, true);
        wcd_force(i, true);
        for _ in 0..PAYGO_CLOSE_MAX {
            if !(super::wcg::paygo_pending(i) || wcd_pending(i)) {
                break;
            }
            let marked = {
                let mut t = table();
                match row_mut(&mut t, id) {
                    Some(r) if r.used => {
                        r.damaged = true;
                        // Whole box: the deferred pass is the FULL-coverage one by definition.
                        r.dmg_y0 = 0;
                        r.dmg_y1 = 0;
                        true
                    }
                    _ => false,
                }
            };
            if !marked {
                break;
            }
            composite();
        }
        // ALWAYS cleared, on every path out of the loop. A latch left standing would hand this
        // slot's next tenant a battery with no deferral at all — the gate silently off for a window
        // nobody armed it off for, which is this module's convicted failure shape.
        super::wcg::paygo_force(i, false);
        wcd_force(i, false);
    }
    // Whatever is still owed is owed forever now. One terminal per wire that still has a battery
    // open; a wire that closed above already printed `-> PAID` and is no longer pending.
    //
    // ONE-SHOT PER TENANT, and the QEMU run that made this arc's first cut is why the one-shot is
    // here at all: without a latch, `pending` stays true after an unspent close and every LATER close
    // on the same slot re-prints the same terminal — the first cut emitted `win=1 state=closed …
    // -> UNSPENT` seven times in one `./arroyo test`, census climbing 2 -> 7 -> 16 -> 19 -> 21 -> 26
    // behind it. A terminal that fires repeatedly is not a terminal. But the latch is re-armed by
    // `create_inner` and NOT carried across the recycle: see [`PAYGO_CLOSE_SAID`] for the premise
    // that got that wrong the first time and what it cost the slot's later tenants.
    let said = PAYGO_CLOSE_SAID[i].load(core::sync::atomic::Ordering::Relaxed);
    if super::wcg::paygo_pending(i) && said & 1 == 0 {
        // Latched BEFORE the print on both wires, so a concurrent census flush that reaches its gate
        // from here on declines and the terminal keeps the greatest `emit=`. `wcg::paygo_closed`
        // takes its own latch for the same reason and in the same order.
        PAYGO_CLOSE_SAID[i].fetch_or(1, core::sync::atomic::Ordering::Relaxed);
        super::wcg::paygo_closed(id, i);
    }
    if wcd_pending(i) && said & 2 == 0 {
        PAYGO_CLOSE_SAID[i].fetch_or(2, core::sync::atomic::Ordering::Relaxed);
        let (since_ms, clock, _) = super::wcg::paygo_clock();
        wcd_paygo_note(id, i, "closed", "UNSPENT", since_ms, clock);
    }
    // AND THE WIRE IS SHUT, on both halves, whichever way each of them got here — paid in full above
    // (`state=complete … -> PAID`), closed unspent just now, or never owed anything at all. All three
    // are the same fact for a reader: this tenant has said its last word. Setting the state only on
    // the branches that PRINTED would leave a battery that paid at close still eligible for the
    // periodic census, and a `waiting` line at a higher `emit=` behind a `PAID` supersedes it exactly
    // as it would behind an `UNSPENT`.
    PAYGO_CLOSE_SAID[i].fetch_or(3, core::sync::atomic::Ordering::Relaxed);
    super::wcg::paygo_seal_closed(i);
    // `PAYGO_SVC_TRIES` and `PAYGO_SVC_NOTED` are deliberately NOT reset here. Both are per-tenant
    // and both re-arm in `create_inner` — a close is the wrong place for them because the two early
    // returns above skip it, and the one they skip is the SUCCESSFUL case. See [`PAYGO_SVC_TRIES`].
}

/// Knob off: one verdict per window at full coverage. Same one-reference guarantee, two states.
#[cfg(all(feature = "witness", not(all(target_arch = "x86_64", feature = "wcg-paygo"))))]
#[inline]
fn wcd_admit(_id: u32, i: usize) -> Option<(usize, u32)> {
    if wcd_cas(i, WCD_ST_FIRST, WCD_ST_FIRST_RUN) {
        Some((1, WCD_ST_FIRST_RUN))
    } else {
        None
    }
}

#[cfg(feature = "witness")]
#[inline]
fn wcd_cas(i: usize, from: u32, to: u32) -> bool {
    WCD_STATE[i]
        .compare_exchange(
            from,
            to,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        )
        .is_ok()
}

/// WC-D — a verdict was published: advance the window and publish the flags the rest of the module
/// reads.
///
/// A FULL pass closes the battery whatever stage it arrived from, which is what makes a COLLAPSED
/// lattice correct rather than merely tolerable: a rect narrower than the step is walked at full
/// coverage (see [`verify_reference`]), so it earned the terminal state on its first pass and must
/// not be re-verified later to say the same thing again.
#[cfg(feature = "witness")]
fn wcd_commit(i: usize, running: u32, step: usize) {
    let bit = 1u32 << i;
    VERIFIED.fetch_or(bit, core::sync::atomic::Ordering::Relaxed);
    #[cfg(all(target_arch = "x86_64", feature = "wcg-paygo"))]
    {
        if step == 1 {
            VERIFIED_FULL.fetch_or(bit, core::sync::atomic::Ordering::Relaxed);
            WCD_STATE[i].store(WCD_ST_DONE, core::sync::atomic::Ordering::Release);
        } else {
            WCD_STATE[i].store(WCD_ST_FULL, core::sync::atomic::Ordering::Release);
        }
        let _ = running;
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wcg-paygo")))]
    {
        let _ = (running, step);
        WCD_STATE[i].store(WCD_ST_DONE, core::sync::atomic::Ordering::Release);
    }
}

/// WC-D — close this window with no further verdict owed, whatever stage asked. The `-> SKIP`
/// GEOMETRY paths use it: a degenerate row is a property of the ROW, will still be degenerate at the
/// deferred stage, and re-running it there would buy a duplicate SKIP line and nothing else.
///
/// The out-of-memory SKIP deliberately does NOT use it — an allocation failure is a property of one
/// instant, not of the row.
#[cfg(feature = "witness")]
fn wcd_seal(i: usize) {
    let bit = 1u32 << i;
    VERIFIED.fetch_or(bit, core::sync::atomic::Ordering::Relaxed);
    #[cfg(all(target_arch = "x86_64", feature = "wcg-paygo"))]
    VERIFIED_FULL.fetch_or(bit, core::sync::atomic::Ordering::Relaxed);
    WCD_STATE[i].store(WCD_ST_DONE, core::sync::atomic::Ordering::Release);
}

/// WC-D — hand the reference back with no verdict published and nothing counted.
///
/// The plain unwind, for the paths that decline a pass on a property of ONE PRESENT rather than of the
/// row or of the panel: today the empty-band clip and the out-of-memory snapshot. Distinct from
/// [`wcd_release`] only in that it is not arch-gated — every build has these paths, and only x86 has
/// an interlock.
#[cfg(feature = "witness")]
fn wcd_unwind(i: usize, running: u32) {
    #[cfg(all(target_arch = "x86_64", feature = "wcg-paygo"))]
    let resting = if running == WCD_ST_FULL_RUN { WCD_ST_FULL } else { WCD_ST_FIRST };
    #[cfg(not(all(target_arch = "x86_64", feature = "wcg-paygo")))]
    let resting = {
        let _ = running;
        WCD_ST_FIRST
    };
    WCD_STATE[i].store(resting, core::sync::atomic::Ordering::Release);
}

/// WC-D — hand this window's verdict back so a later composite can take it again.
///
/// Returns the window to the RESTING state its reference came from and publishes nothing, so a
/// stage-2 abort cannot demote a window that already has a valid first verdict back to the cheap
/// pass, and a stage-1 abort cannot release "nothing" by clearing a bit it never set. With one cell
/// there is no pair to get out of step.
#[cfg(all(feature = "witness", target_arch = "x86_64"))]
#[inline]
fn wcd_release(i: usize, running: u32) {
    wcd_unwind(i, running);
}

/// WC-D/PAYGO — record a declined composite, and keep the window's census current on the wire.
///
/// The census moves FIRST and unbudgeted, before any print test. Then the first decline speaks
/// unconditionally (see [`WCD_SAID`] for why it cannot go through the rate gate), and afterwards the
/// waiting line is RE-EMITTED on `wcg::CENSUS_PERIOD_US` cadence — so `deferred=` is a running census
/// with the moment it was taken stamped on it, not a figure frozen at the instant the deferral began.
/// A one-shot there would be `wcg`'s convicted `H_TORN` shape exactly: printed once, at first decline,
/// where the count is 1 by construction.
///
/// A window that stops compositing stops refreshing, which is not a gap: its last line describes its
/// last active state and `since_entry_ms=` says when that was.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_decline(id: u32, i: usize, since_ms: u64, clock: &'static str) {
    // THE TERMINAL IS THE LAST WORD on this wire too. Once [`paygo_at_close`] has shut this tenant's
    // wc-d half, nothing may print behind it at a higher `emit=` — the reader's supersession rule
    // would read the terminal as superseded. The census still moves (`WCD_DEFERRED` is a per-id total
    // for the whole boot and stays monotone across a recycle); only the PRINT is suppressed, and only
    // until `create_inner` re-arms the latch for the next tenant. `wcg::paygo_flush` has the twin.
    let closed = PAYGO_CLOSE_SAID[i].load(core::sync::atomic::Ordering::Acquire) & 2 != 0;
    WCD_DEFERRED[i].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if closed {
        return;
    }
    if WCD_SAID[i].swap(1, core::sync::atomic::Ordering::AcqRel) == 0 {
        wcd_paygo_note(id, i, "waiting", "DEFERRED", since_ms, clock);
        return;
    }
    let last = WCD_LASTROLL[i].load(core::sync::atomic::Ordering::Relaxed);
    let now = crate::arch::now_cycles();
    if super::wcg::cycles_to_us(now.saturating_sub(last)) < super::wcg::CENSUS_PERIOD_US {
        return;
    }
    if WCD_LASTROLL[i]
        .compare_exchange(
            last,
            now,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }
    wcd_paygo_note(id, i, "waiting", "DEFERRED", since_ms, clock);
}

/// WC-D/PAYGO — the policy's own line: what regime this window is in, how much it has declined, and
/// when the deferred half becomes payable.
///
/// A separate line rather than fields on the verdict, for the reason `wcg::paygo_note` gives: the
/// verdict's key order and terminal are matched by another platform track's gate, and a field that
/// track does not read is not worth a chance of breaking it. The key set is `[wc-g] paygo`'s exactly,
/// so one reader rule serves both — `taken=`/`budget=` count the window's STAGES, of which there are
/// two, and `taken=` counts stages CLOSED rather than lines printed, because a collapsed lattice
/// closes both in one full-coverage verdict.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_paygo_note(id: u32, i: usize, state: &str, verdict: &str, since_ms: u64, clock: &str) {
    // `taken=` off the STATE cell, not off the published bitmasks. A recycle clears those between a
    // claim and its print (slot 3 recycles seven times in the s73 capture, and a deferred stage-2
    // straddles a recycle by construction), which is how a `-> PAID` could read `taken=0`. The state
    // cell is reset by the same recycle, so it cannot disagree with itself.
    let taken = wcd_taken(i);
    let emit = WCD_EMIT[i].fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    serial_println!(
        "[wc-d] paygo win={} state={} emit={} lattice_n={} deferred={} defer_ms={} since_entry_ms={} clock={} taken={} budget=2 -> {}",
        id,
        state,
        emit,
        WCD_LATTICE_N,
        WCD_DEFERRED[i].load(core::sync::atomic::Ordering::Relaxed),
        super::wcg::PAYGO_DEFER_MS,
        since_ms,
        clock,
        taken,
        verdict
    );
    // Re-armed from AFTER the serial write, deliberately — see [`WCD_LASTROLL`].
    WCD_LASTROLL[i].store(crate::arch::now_cycles(), core::sync::atomic::Ordering::Relaxed);
}

/// WC-D/PAYGO — the battery's terminal line, emitted beside the verdict that closed it so the
/// `deferred=` census is read at the moment full coverage is claimed and not only at the moment the
/// waiting began. `deferred=0` here says the gate never declined this window at all.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
fn wcd_complete(id: u32, i: usize) {
    let (since_ms, clock, _) = super::wcg::paygo_clock();
    wcd_paygo_note(id, i, "complete", "PAID", since_ms, clock);
}

/// WC-D/PAYGO — the `coverage=` marker, inserted between `checked=` and `bad_cache=`: an INSERTION,
/// which leaves the pi4 gate's `.*` spans and its `-> PASS` / `-> FAIL` terminals matching exactly
/// what they matched before. The empty string on every build but an x86 `wcg-paygo` one, so those
/// lines stay byte-identical.
///
/// Derived from the step the walk ACTUALLY used, never from the step that was asked for — see
/// [`verify_reference`]'s collapse. A `coverage=` that misreports its own pass is worse than no
/// marker at all.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wcg-paygo"))]
#[inline]
fn wcd_coverage_note(step: usize) -> &'static str {
    if step > 1 {
        " coverage=lattice16"
    } else {
        " coverage=full"
    }
}

#[cfg(all(feature = "witness", not(all(target_arch = "x86_64", feature = "wcg-paygo"))))]
#[inline]
fn wcd_coverage_note(_step: usize) -> &'static str {
    ""
}

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

/// WEDGE-1 — spin iterations past which [`DrainBarrier::drain`] declares itself stalled. The wait it
/// is bounded against is a handful of panel-clipped `memcpy`s, microseconds each; this is order 10^8
/// spin hints, i.e. comfortably past a second of real time on the Pi even with the loop fully
/// unrolled. Deliberately far out: the tripwire's job is to name a HANG, and a threshold a merely
/// slow drain could reach would put a serial write on a hot IRQ-masked path.
const DRAIN_STALL_SPINS: u64 = 1 << 27;

/// WEDGE-1 — whether the drain-stall tripwire has already fired. Once per boot, globally: a wedge
/// takes several cores at once and each would otherwise queue its own line behind a serial lock that
/// the wedge may itself be holding.
static DRAIN_STALL_REPORTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

// ---- WEDGE-1r2 — the drain barrier's silence, made readable --------------------------------------
//
// **What this arc found, and it is a finding about an INSTRUMENT rather than about a mechanism.**
// `docs/dev/OS/08_VIDEO/engine.md` §WEDGE-2 opens by treating one inference as settled: *"The drain
// barrier is exonerated. WEDGE-1's `[wedge1] DRAIN STALLED` tripwire fires from inside the
// drain-barrier spin, and it stayed silent all three times."* That inference reads a silence as a
// refutation, and it is only sound if the tripwire could have SPOKEN in the states it is being
// cleared of. It could not, in two of them:
//
//  1. **It speaks through `serial_println!`, which takes a blocking lock.** The tripwire's own note
//     admits this ("if the wedge is IN the serial path the tripwire blocks") and then argues it is
//     "strictly no worse" than spinning — which is true about the MACHINE and false about the
//     EVIDENCE. s44's capture stopped mid-word, i.e. a core died holding the UART; on that shape the
//     tripwire's silence is structurally guaranteed and carries no information at all.
//  2. **It exists only INSIDE the spin.** Every teardown reaches the barrier through its own
//     `TABLE` critical section — `close`/`close_owner` clear their rows under the lock, `move_to`
//     rewrites its geometry under it — and a core that dies waiting on `TABLE` there never reaches
//     `drain()`. The teardown path can therefore BE the wedge site with the tripwire silent by
//     construction, because the instrument is one lock downstream of the death.
//
// Neither is fixed by moving the threshold. What both need is a voice that acquires nothing, and
// the tree already has one: WEDGE-2's `wedge2_raw_byte` — a lock-free bounded poll of the UART
// TX-ready flag and one volatile store, taking no `SERIAL_PORT`, no `FBCON`, no `WRITER`, no
// `TABLE`, no `SPRITE` and no allocator. So the teardown path gets tokens on the same terms the
// focus chain has had since WEDGE-2:
//
//   `<D1>`/`<d1>` — a teardown began, BEFORE the `TABLE`/`WRITER` critical section that precedes the
//                   barrier (`close`, `close_owner`, `move_to`). Uppercase on the core that owns the
//                   open focus chain, lowercase on any other.
//   `<D2>`        — that section is behind us; the barrier is going up and this core is about to spin.
//   `<D3>`        — the spin returned. The barrier is held and the erase/reclaim half is next.
//   `<D4>`        — the erase/reclaim half is behind us and the barrier is down; the cursor bracket
//                   (`cursor::repaint`, and with it `SPRITE`) is next. `close_owner` only.
//   `<D!>`        — the stall tripwire FIRED, emitted before its `serial_println!` so a blocked print
//                   no longer erases the fact that the threshold was crossed.
//
// Read exactly as WEDGE-2's are read — by what is MISSING after the last one on a torn wire:
// `<D1>` with no `<D2>` puts the death upstream of the barrier, in the teardown's own critical
// section (blindness 2); `<D2>` with no `<D3>` puts it in the spin; `<D3>` with no `<D4>` puts it in
// the erase/reclaim run; `<D4>` as the LAST token puts it in the cursor bracket, i.e. on `SPRITE`;
// `<D!>` with no `[wedge1] DRAIN STALLED` line after it is blindness 1 caught in the act — the
// tripwire fired and the serial lock ate it. A `<D1>` that is NOT the last thing on the wire is an
// ordinary teardown that took an early return (no such row, an unmoved `move_to`); a `<D1>` that IS
// the last thing is the finding.
//
// **`<D4>` EXISTS BECAUSE THE F4 DEATH WAS OTHERWISE MISATTRIBUTED TO F1.** Before it, the last
// token on a `close_owner` that died in `cursor::repaint` was `<D3>` — emitted from inside
// `DrainBarrier::drain`, which every teardown reaches — so a `SPRITE` wedge and a reclaim wedge were
// the same wire trace, and the audited F1 site (`TABLE`, now closed by WEDGE-7's `fn table()`) was
// the natural place to pin it. `SPRITE` is reached on EVERY user exit through this path, and a core
// that blocks on it here is IRQ-masked by `sched::exit`, so the outcome is a silent total freeze of
// panel, cursor and input. The token does not fix that; it makes the capture say which lock it was.
//
// **`<D1>` speaks only while a focus chain is open, and that budget is a measured constraint rather
// than a preference.** The first cut emitted it unconditionally at each teardown entry, and the
// wedge2 regression run priced it: 96 tokens against the whole focus chain's 17, because
// `close_owner` runs on EVERY user task exit (`sched::exit` → `clear_handle_row`) and 65 of those 96
// found no window and never raised a barrier at all. One of them interleaved into another core's
// line mid-word (`:<D1: B>GRUN-ST: slot reclaim PASS`) and took a required witness off the wire —
// the accepted cost of a lock-free token, spent on teardowns with nothing to say. So `<D1>` takes
// `mark_composite`'s existing discipline, for the reason `mark_composite` states: the steady-state
// rate would otherwise bury the chain in tokens. It is also the right AIM — all four recorded
// lockups (P66/P67v2/P68/s44) happened during TAB cycling with a focus change in flight, which is
// exactly `CHAIN_CORE != 0` — and the case split it comes with is itself evidence: `<D1>` says the
// teardown is on the core running the focus change, `<d1>` says a vug exited on some OTHER core
// while that chain was open, which is the P66 scene in one byte.
//
// `<D2>`/`<D3>`/`<D!>` stay unconditional: they fire only when a barrier is genuinely raised (31 in
// that same run), a wedge in the drain is worth naming whether or not a TAB is in flight, and the
// tripwire least of all may be gated on somebody else's state.
//
// **WHAT THE HEADLESS GATE CAN AND CANNOT WITNESS — stated here, because this whole block exists
// because a silence got banked as a refutation.** `UNAOS_WEDGE2=1 kernel8-test` emits `<D2>`/`<D3>`
// in pairs and NO `<D1>`/`<d1>` at all, and that zero is a SCENE fact rather than a wiring one: the
// chain is open only for the body of `focus_changed`, and nothing in the fixture battery tears a
// window down from inside that window (`clickplain_leg` closes before its `focus_changed(0)`;
// `closebox_leg`'s router close runs after `focus_changed` has already called `chain_exit`). The
// scene the token is aimed at — one core in a TAB while a vug exits on another — is the bench's,
// which is precisely P66's. So `<D1>` absent on QEMU proves nothing either way and MUST NOT be read
// as the mechanism being quiet; the pairing of `<D2>`/`<D3>` is what the gate does prove.
//
// One `<D2>` in that run reads as `<activD2>` on the wire — another core's line interleaved with the
// token's own bytes. That is WEDGE-2's stated and accepted cost for taking no lock, unchanged here.
//
// Knob-gated with the rest of WEDGE-2 (`UNAOS_WEDGE2=1`): with the feature off `mark` is an empty
// inline function and no token exists in the image, so no shipped media pay for this.

/// WEDGE-1r2 — how often [`DrainBarrier::drain`] ran, and how long it actually SPUN.
///
/// ### Why the tripwire alone cannot answer this
/// [`DRAIN_STALL_SPINS`] sits past ~10^8 spin hints on purpose, and its own note says why: a
/// threshold a merely slow drain could reach would put a serial write on a hot IRQ-masked path. The
/// consequence was left standing — the wire has exactly two readings, "nothing" and "wedged", with
/// the whole interesting range between them unmeasured. A teardown that spins for milliseconds is
/// not a wedge, but it IS a core held with interrupts masked (`sched::exit` masks before it reaches
/// `clear_handle_row`), and today it is indistinguishable from a drain that returned at once.
///
/// A counter costs no print, so it can measure the range the tripwire deliberately declined to.
///
/// ### Why the high-water mark is published FROM INSIDE the loop
/// A ledger written after the spin can only report drains that FINISHED — so the one drain that
/// matters most, the one still going, contributes nothing and the rollup reads clean. That is
/// WEDGE-4's W4-A defect exactly, and it is inadmissible here for the same reason: an instrument's
/// silence is evidence only if the instrument can execute in the state it reports on. [`F4W_SPIN_MAX`]
/// is therefore updated from within the spin, so a core that never leaves the loop has still
/// published how far it got, and [`F4W_IN_SPIN`] stays raised for as long as that core is in there —
/// which is what lets some OTHER core's rollup report a drain that is not coming back.
///
/// The stride is what keeps it off the hot path: one masked compare per iteration, one relaxed RMW
/// per [`DRAIN_DWELL_STEP`]. The A72 LL/SC starvation SPIN-3 padded for is a function of how many
/// cores hammer a line, and these have one writer per concurrently-draining teardown.
#[cfg(feature = "witness")]
static F4W_DRAINS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WEDGE-1r2 — drains that found `BLIT_ACTIVE` non-zero and actually entered the spin. The
/// denominator `spin_max` is meaningful against: `drains` with `spun=0` is a barrier that never had
/// to wait for anybody, which is the healthy desktop's normal answer.
#[cfg(feature = "witness")]
static F4W_SPUN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WEDGE-1r2 — the longest spin any drain reached in this rollup window, published from inside the
/// loop. See [`F4W_DRAINS`] for why that placement is a correctness property of the instrument.
#[cfg(feature = "witness")]
static F4W_SPIN_MAX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WEDGE-1r2 — drains currently inside the spin. A GAUGE, not an accumulator: it is read rather than
/// drained, and a non-zero reading taken by a core that is still running is the closest thing this
/// module has to "somebody else is stuck right now".
#[cfg(feature = "witness")]
static F4W_IN_SPIN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// WEDGE-1r2 — how often the spin publishes its progress. A power of two so the test is a mask.
#[cfg(feature = "witness")]
const DRAIN_DWELL_STEP: u64 = 1 << 12;

/// WEDGE-1r2 — spins past which a drain is worth calling a DWELL rather than a wait. Four orders of
/// magnitude under [`DRAIN_STALL_SPINS`], and far past the handful of panel-clipped `memcpy`s the
/// barrier is bounded against, so it names an interval no healthy teardown produces without
/// competing with the tripwire for the wedge itself.
#[cfg(feature = "witness")]
const DRAIN_DWELL_NOTE: u64 = 1 << 16;

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
        // WEDGE-1r2 `<D2>` — the caller's `TABLE` critical section is behind us, the barrier is going
        // up, and this core is about to spin IRQ-masked. Raw and lock-free, for the reason the block
        // above the ledger states: the `serial_println!` tripwire further down cannot speak while the
        // serial lock is what the wedge is holding, so the token that says "we got this far" must not
        // depend on it either. A `<D1>` never followed by this one puts the death upstream, in the
        // caller's own critical section — the region WEDGE-1's tripwire is blind to by construction.
        crate::wedge2::mark("<D2>");
        #[cfg(feature = "witness")]
        F4W_DRAINS.fetch_add(1, Ordering::Relaxed);
        DRAIN_PENDING.fetch_add(1, Ordering::AcqRel);
        // Ordered against the composite registration by the table lock: a composite either took the
        // lock BEFORE the clearing critical section (so it registered, and is counted here) or AFTER
        // (so it sees the raised barrier and never registers).
        //
        // WEDGE-1 — THE TRIPWIRE. P66 wedged three of four cores with nothing on the wire: no panic,
        // no exception, just `SCHED: load c0=-- c1=-- c3=--` and silence. **That wedge's mechanism is
        // UNKNOWN — this spin is not known to be the site.** It is instrumented because it is one of
        // the few places in the kernel that can consume a core silently and forever, so if the next
        // wedge does land here it will say so instead of being guessed at.
        //
        // THE GUARD-WINDOW INVARIANT, stated honestly. It is NOT "no blocking lock is acquired after
        // `BlitGuard::enter()`" — that is false and always has been. On witness builds the window
        // contains `serial_println!` (the `[wc-a]`/`[wc-c]` lines and every print in `verify_window`),
        // `wcg::begin`/`end`, and `stage_window`'s `try_reserve`/`resize` into the global allocator on
        // buffer growth. The invariant this barrier actually needs is weaker and true:
        //
        //     no lock acquired inside the window has a holder that can block UNBOUNDEDLY.
        //
        // The exceptions above are audited against exactly that and pass: `SERIAL_PORT`'s TX waits are
        // explicitly spin-bounded, `FBCON` is `try_lock` on every print path, and the allocator's
        // `allocate_first_fit` is O(free list). They are listed in docs §WEDGE-1 as bounded-hold
        // exceptions rather than removed, because removing instrumentation is not this arc's business.
        //
        // The spin is UNCHANGED — the barrier still waits out every registered blit, because
        // returning early would hand a teardown's about-to-be-unmapped surface to an in-flight blit,
        // and no diagnostic is worth that. The tripwire only makes the stall AUDIBLE: once, past a
        // threshold no bounded panel-clipped blit can reach, it names the core and the outstanding
        // count, then goes back to spinning.
        //
        // Honest about its own risk: this core is IRQ-masked, and `serial_println!` takes a blocking
        // lock. If the wedge is IN the serial path the tripwire blocks — but the alternative on that
        // path was spinning here forever anyway, so it is strictly no worse, and on every other
        // shape of wedge it converts a silent hang into a named one.
        //
        // WEDGE-1r2 CORRECTS THE SECOND HALF OF THAT SENTENCE. "Strictly no worse" is a claim about
        // the MACHINE, and it is true. It was then used as a claim about the EVIDENCE, and there it is
        // false: engine.md §WEDGE-2 banked this tripwire's silence across P66/P67v2/P68 as *"the drain
        // barrier is exonerated"*, while on the one wedge shape the wire actually points at — s44's
        // capture stopped mid-word, i.e. a core died holding the UART — the silence was structurally
        // guaranteed and said nothing whatever. A blocked print does not merely fail to report the
        // stall; it erases the fact that the threshold was crossed, which is the difference between an
        // instrument that is quiet and an instrument that cannot speak. The `<D!>` token below takes
        // no lock and is emitted BEFORE the print, so from here on this tripwire's silence means the
        // threshold was not reached rather than the report was eaten.
        let mut spins: u64 = 0;
        // WEDGE-1r2 — has this drain been charged to `spun`/`in_spin` yet? Set on the FIRST iteration
        // rather than before the loop, so a barrier that found `BLIT_ACTIVE == 0` and never waited is
        // not counted as a wait. `drains` above already counts those.
        #[cfg(feature = "witness")]
        let mut entered = false;
        while BLIT_ACTIVE.load(Ordering::Acquire) != 0 {
            #[cfg(feature = "witness")]
            if !entered {
                entered = true;
                F4W_SPUN.fetch_add(1, Ordering::Relaxed);
                // Raised HERE and lowered only after the loop, so a core that never comes out leaves
                // it raised — that standing count is the whole point (see `F4W_IN_SPIN`).
                F4W_IN_SPIN.fetch_add(1, Ordering::Relaxed);
            }
            core::hint::spin_loop();
            spins = spins.wrapping_add(1);
            // WEDGE-1r2 — publish progress FROM INSIDE the loop. A high-water mark written after the
            // spin describes only drains that finished, so the one that never does would contribute
            // nothing and the rollup would read clean over a held core — W4-A's defect, refused here.
            // The stride keeps the RMW off the hot path; the masked compare is what runs per spin.
            #[cfg(feature = "witness")]
            if spins % DRAIN_DWELL_STEP == 0 {
                F4W_SPIN_MAX.fetch_max(spins, Ordering::Relaxed);
            }
            if spins == DRAIN_STALL_SPINS && !DRAIN_STALL_REPORTED.swap(true, Ordering::Relaxed) {
                // WEDGE-1r2 `<D!>` — THE TRIPWIRE FIRED, said without taking a lock, and said BEFORE
                // the line below. The tripwire's own note concedes that a wedge in the serial path
                // blocks this print; what it did not follow through is that the blocked print then
                // destroys the evidence that the threshold was ever crossed, which is precisely how a
                // silence that could not have been broken came to be read as a refutation. This token
                // survives that: `<D!>` with no `[wedge1] DRAIN STALLED` line after it says the stall
                // is real AND that the serial path is where the core went to die.
                crate::wedge2::mark("<D!>");
                serial_println!(
                    ":: [wedge1] DRAIN STALLED core={} blit_active={} pending={} spins={} == tripwire ::",
                    crate::arch::sched::meter_current_cpu(),
                    BLIT_ACTIVE.load(Ordering::Acquire),
                    DRAIN_PENDING.load(Ordering::Acquire),
                    spins
                );
            }
        }
        #[cfg(feature = "witness")]
        if entered {
            // The exact figure, now that it is known; the in-loop publication above is the floor that
            // stands in for it while the drain is still running.
            F4W_SPIN_MAX.fetch_max(spins, Ordering::Relaxed);
            F4W_IN_SPIN.fetch_sub(1, Ordering::Relaxed);
        }
        // WEDGE-1r2 `<D3>` — the spin returned. `<D2>` with no `<D3>` puts the death in the spin
        // itself, which is the one region WEDGE-1's tripwire CAN see — and pairs with `<D!>` to say
        // whether it got far enough to try.
        crate::wedge2::mark("<D3>");
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
/// ### It was a stuck zero for two weeks, and this is the repair
///
/// From WC-I (`b72e55f4`, 2026-07-25) until now the counter's only writer passed a **literal
/// `false`** — `note_desktop_flush(!occ.is_empty(), false)` — so `intrusions=0` on every capture was
/// the constant, not the finding. WCD-TEARDOWN (`6f1225b9`) diagnosed the same thing one layer over
/// (see [`PANEL_DESK_EPOCH`], which declined to build its stability term on `present_background`'s
/// return value for exactly this reason) but left the counter standing, so a reader still had a
/// zero that looked like evidence.
///
/// The literal was not laziness: the predicate it stood in for really is a tautology. The desktop
/// subtracts the occluder snapshot from its own damage before it copies, so "did I write inside a box
/// I just subtracted" is answered by construction, and `present_background`'s three exits are
/// consequently all `false`.
///
/// **What is counted now is the only way the claim can still be false: the snapshot going stale under
/// the copy.** A window that is created, moved, resized or raised after [`occluders`] returns was
/// never subtracted, and any span this present copies over its box is a desktop pixel inside a live
/// window — the original WC-I defect, arriving by race instead of by design. [`occluders_aged`] is the
/// test; `stale`/`intrusions` below are its two terms.
///
/// This does not overlap [`PANEL_DESK_EPOCH`]'s `desk=`. That term counts blit LOOPS, unconditionally
/// and without geometry, to bracket a scan-out read-back. This one counts loops whose window layout
/// moved under them AND whose writes landed where it moved. A boot can show `desk=` climbing steadily
/// with `stale=0`, and that is the informative case: it says the desktop layer is busy and is not the
/// thing painting over windows.
///
/// The two DO meet at one place, and it is worth both notes pointing at it: [`PANEL_DESK_ACTIVE`]
/// names a gap in its own bracket — the `vugpar`+`baremetal` parallel present copies clipped rects
/// to glass and returns ABOVE the `DeskWriteGuard`, so `desk=` cannot see that leg. That leg is
/// exactly where this probe now runs, because it is also the leg with no subtraction at all. The
/// coverage is complementary rather than duplicated: on the band exit `desk=` is blind and
/// `stale=`/`intrusions=` speak; on the serial loop both do, about different things.
///
/// ### `INTRUDED` is a TRIPWIRE verdict now, not a diagnosis
///
/// For two weeks the verdict was unreachable, so every historical line said `CLEAN` or
/// `UNWITNESSED`. It is reachable again, and it is deliberately reachable with a KNOWN
/// OVER-REPORTING MODE (see [`occluders_aged`]): the geometric test is a box against the union
/// RECTANGLE of the present's spans, so an entered box that overlaps the union while sitting
/// entirely inside a span the old table already subtracted counts as an intrusion it was not. A
/// benign drag over a busy desktop can therefore print `INTRUDED` where the old line printed
/// `CLEAN`, and that is not a regression in the compositor.
///
/// **The reading rule, in order:**
///  * `stale=0` — the race never arose. `intrusions` is 0 by construction; nothing to read.
///  * `stale=N intrusions=0` — the layout moved under N presents and the writes missed every box
///    that entered. The subtraction's snapshot is aging and getting away with it.
///  * `stale=N intrusions=M` — M of those presents wrote into the union a box entered. THIS IS A
///    LEAD, NOT A CONVICTION. Confirm against the panel (the P60 blip is visible) or against
///    `[wc-d]`'s per-window read-back before calling it the WC-I defect; `M` climbing in lockstep
///    with window drags is the expected shape of the false positive, `M` climbing at the status
///    strip's ~1 Hz with a still layout is the shape of the real one.
///
/// No spec in the tree reads any `[wc-i]` line, so nothing reds on this. That makes it more
/// important, not less, that the verdict's new meaning is written where the reader is.
#[cfg(feature = "witness")]
static WCI_INTRUSIONS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-I — desktop presents whose occluder snapshot changed before the copy finished. The denominator
/// [`WCI_INTRUSIONS`] needs to mean anything: `stale=0 intrusions=0` says the race never arose, while
/// `stale=N intrusions=0` says it arose N times and the writes missed every box that entered. The two
/// readings are not the same claim, and before this term the rollup could not tell them apart.
#[cfg(feature = "witness")]
static WCI_STALE_SNAPSHOTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-I — desktop presents that ran with at least one live window on the panel. The denominator the
/// intrusion count is meaningful against: `windowed=0 intrusions=0` proves nothing, and the rollup
/// says so in its verdict rather than leaving a reader to notice.
#[cfg(feature = "witness")]
static WCI_WINDOWED_FLUSHES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-I — record one desktop present: whether it ran over a live window layer, whether its occluder
/// snapshot aged under the copy, and whether anything it wrote landed in a box that entered.
/// `Screen::present_background` is the only writer, on both of its presenting exits.
///
/// `stale` and `intruded` come from [`occluders_aged`] and are counted **outside** the `windowed`
/// gate on purpose. The race's worst case is precisely the one where the snapshot was EMPTY — the
/// windowless present, which on the `vugpar` band path performs no subtraction at all and copies whole
/// clipped rects — so gating the intrusion count on `windowed` would blind it to its own worst case.
/// `windowed_flushes` remains the denominator for the CLEAN verdict, which is a claim about the
/// desktop running over a window layer and is a different claim.
#[cfg(feature = "witness")]
pub(super) fn note_desktop_flush(windowed: bool, stale: bool, intruded: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    if stale {
        WCI_STALE_SNAPSHOTS.fetch_add(1, Relaxed);
    }
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
///    P60 blip is this number being one per status-strip tick; the fix makes it 0. Read it WITH
///    `stale`: see [`WCI_INTRUSIONS`] for why the zero it printed before this change was a constant.
///  * `stale` — presents whose occluder snapshot changed under the copy. The denominator that tells
///    `intrusions=0` (the race never arose) apart from `intrusions=0` (it arose and missed).
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

/// CURSOR-12 — the same block, on the LIVE scope, for a bench sitting rather than a boot fixture.
///
/// ### Why this entry point had to exist before any of it could be read
/// [`wci_rollup`] has exactly one caller in the tree, `arch::aarch64::syscall`'s EL0 window-verb
/// fixture, and it is a boot-time one-shot. Two consequences, both of which have cost sittings:
///
/// * **On x86 the entire cursor rollup block has never printed at all.** There is no x86 caller, so
///   `[cursor3]`/`[cursor5]`/`[cursor6]`/`[cursor8]` are absent from every rmbp capture ever taken —
///   which is why the s46 sitting shows one `[cursor3] present …` sample line (that one comes from
///   `note_cursor_tail`, a different site) and no rollup whatsoever. The x86 track has been reading
///   the cursor mechanism through a keyhole.
/// * **On both arches it fires before the operator has touched the pointer**, so even where it does
///   print, every cursor counter in it is structurally zero. `UNWITNESSED` there is not a finding.
///
/// The pointer's own motion path is the correct cadence for a pointer instrument: it runs only while
/// there is a pointer to instrument, it stops when the operator stops, and it cannot fire on a
/// headless gate at all. Paced by the caller — see `pal::cursor::rollup_tick`.
///
/// Called with no lock of `wm`'s or the sprite module's held; it takes `TABLE` internally, on the same
/// footing every other rollup does.
#[cfg(feature = "witness")]
pub fn wci_rollup_live() {
    wci_rollup_scoped("live");
}

/// WC-I — the rollup body. `scope` names WHICH evidence the line was taken on: `fixture` at the end of
/// the window-verb witness block (the counters are wired), `desktop` after the desktop layer has
/// presented over a live window layer enough times for the verdict to mean something.
#[cfg(feature = "witness")]
fn wci_rollup_scoped(scope: &str) {
    use core::sync::atomic::Ordering::Relaxed;
    let windowed = WCI_WINDOWED_FLUSHES.load(Relaxed);
    let intrusions = WCI_INTRUSIONS.load(Relaxed);
    let stale = WCI_STALE_SNAPSHOTS.load(Relaxed);
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
        "[wc-i] rollup scope={} windowed_flushes={} stale={} intrusions={} cursor_passes={} cursor_brackets={} -> {}",
        scope, windowed, stale, intrusions, passes, brackets, verdict
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
/// touched the sprite — WC-I's cheap tail, and the desktop case). CURSOR-15 adds `settle` (the
/// sessionless compose-through: the arrow never left the glass, the tail answered the deferred
/// pixels against the finished front). On a hovered fleet `settle` should absorb what `repaint` used
/// to count, at present cadence — that migration IS the fix, read straight off `[cursor3] rollup`.
#[cfg(feature = "witness")]
static CUR3_ADOPT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR3_REPAINT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR3_ENSURE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR15_SETTLE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-4 — the DECLINE BREAKDOWN. CURSOR-3's rollup reported `offers - taken` as one number, and
/// the P62 wire (`offers=4181 taken=2427`) could say only that 42% of offers fell back — not which
/// of the three documented reasons was doing it, and therefore not which one was worth fixing.
///
/// * `straddle` — offers where SOME painted sprite pixel fell outside this window's box.
///
/// ### CURSOR-6 — `straddle` was not a straddle count, and the 48% it implied was an artefact
/// The P65v2 desktop wire reads `offers=33790 taken=16111 straddle=18443`, which invites the reading
/// "half of all offers decline, and the straddle mechanism is why". It is not what those numbers say.
/// An OFFER is made to every staged window of a pass that holds a plan — `stage_window` has no
/// overlap test, deliberately, because a window that paints over sprite pixels without composing them
/// must still clear their coverage. The sprite is over ONE window; every OTHER window in the pass
/// receives an offer it misses ENTIRELY, and `missed > 0` counted that identically to a genuine
/// partial carry. With ~1.6 offers per session (`offers / planned`), the arithmetic is not a mechanism
/// declining half the time — it is one window taking the sprite and its neighbours being asked.
///
/// So the class is split, and only the second half is the mechanism:
/// * `disjoint` — `taken == 0 && missed > 0`: the sprite was nowhere in this window. Not a decline at
///   all, and not a loss; it is the shape of the offer set.
/// * `partial` — `taken > 0 && missed > 0`: a real straddle, composed in part. The measure of how
///   often the split mechanism actually runs.
/// * `lock` — a contended plan lock, at either end: a `composite` that found another pass already
///   owning the overlay session, or a `compose_into` that could not take `OVERLAY` inside the guard.
///   Both fall back whole; neither spins.
/// * `budget` — an instrument forbade the compose on this window (WC-G's live probe, an unspent
///   WC-D verification) or a WC-F reserved box overlapped the sprite. All one-shots.
#[cfg(feature = "witness")]
static CUR3_DECL_STRADDLE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-6 — the two halves of the old `straddle` class. See [`CUR3_DECL_STRADDLE`]'s comment for
/// why the single counter could not be read.
#[cfg(feature = "witness")]
static CUR6_DECL_DISJOINT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR6_DECL_PARTIAL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR3_DECL_LOCK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR3_DECL_BUDGET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-5 (lens NOTE 4) — offers declined because the plan's generation no longer described the
/// live sprite. A fourth decline class, and it must be VISIBLE here rather than only in `[cursor5]`:
/// `offers - taken` is the number a reader reconciles the breakdown against, and a class missing from
/// it reads as an unexplained gap in the mechanism instead of as the absorbed race it is.
#[cfg(feature = "witness")]
static CUR3_DECL_STALE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// ---- CURSOR-12 — WHICH PREDICATE KILLS THE OFFER --------------------------------------------
//
// P74, both seats: `[cursor3] tail=repaint offers=0 taken=0 -> BRACKETED` for whole live sittings, on
// aarch64 AND on x86, with every `[cursor11]` counter zero. Compose-through is not rare, it is
// DORMANT — CURSOR-3's mechanism has never run outside its selftest, and two tracks have spent
// sittings tuning a path the wire says never executes.
//
// `offers` is incremented in `note_cursor_overlay`, which is called from `stage_window` only when it
// actually reaches `compose_into`. Every predicate between `composite_inner`'s entry and that call is
// therefore a way to reach `offers=0`, and the existing breakdown (`straddle`/`lock`/`budget`/`stale`)
// covers only the ones DOWNSTREAM of an offer being made or a session being opened. The whole
// upstream chain — is the sprite on the panel at all, does any window meet it, did the session open —
// is invisible, and `offers=0` is exactly what an upstream death looks like from the rollup.
//
// These five name the upstream chain, one bump per composite pass, in the order the pass tests them.
// They are pass-scoped and mutually exclusive by construction, so `nosprite + nohit + reserved +
// nosession + planned` is the pass count, and whichever term dominates IS the answer.

/// CURSOR-12 — passes where `cursor::sprite_plan()` returned `None`: the sprite was not on the panel
/// when the bracket was decided, so no plan could be taken and no window could be offered one.
///
/// **This was the leading hypothesis for both arches, it was confirmed, and CURSOR-13 fixed it.**
/// `Screen::flush` ends in `wm::service_damage()` → `composite()`, and the render task used to
/// bracket its own flush with `cursor::undraw()` … `cursor::repaint()` (x86: `main.rs`'s console
/// loop; aarch64: the Pi render task, the CURSOR-1 contract). Every composite reached through the
/// desktop's flush therefore ran BETWEEN the undraw and the repaint — with `sp.drawn == false`, by
/// the caller's own design — so this counter read ≈ `passes` for structural reasons, every time, on
/// both arches, and compose-through could only ever run on a composite reached from `wm::present`.
///
/// CURSOR-13 narrowed that bracket to the desktop blit alone and moved it inside `Screen::flush`
/// (see that function). Flush-reached composites now enter with the arrow on the panel, so this
/// counter is no longer structurally pinned: post-fix it should fall to the passes where the pointer
/// is genuinely hidden (before the first report of the boot, or after CURSOR-HIDE's idle expiry) —
/// and on a QEMU boot, which has no pointer at all, it legitimately stays at `passes`.
#[cfg(feature = "witness")]
static CUR12_NOSPRITE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-12 — the sprite was on the panel, but no live window above the shell met its box. The
/// pointer is over the desktop; nothing to compose through, and WC-I's whole point.
#[cfg(feature = "witness")]
static CUR12_NOHIT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-12 — a WC-F reserved box overlapped the sprite, so the pass took CURSOR-3's whole-sprite
/// bracket deliberately. aarch64 witness+baremetal only; must be 0 on x86.
#[cfg(feature = "witness")]
static CUR12_RESERVED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-12 — `overlay_open` refused because another pass already held the session (the VUGPAR
/// steady state). Distinct from [`CUR3_DECL_LOCK`], which this pass also bumps: that counter mixes
/// this refusal with `compose_into`'s own contended `try_lock` inside the guard, and the two are
/// different questions.
#[cfg(feature = "witness")]
static CUR12_NOSESSION: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-12 — the two halves of the per-window `may_overlay` exclusion, split apart.
///
/// Both are `#[cfg(feature = "witness")]`, which is the structural problem this counter pair exists to
/// size: **every bench image either seat has ever booted is a witness build**, so if these dominate,
/// the instrument has been disabling the mechanism under observation and every compose-through
/// conclusion drawn from a metal boot is suspect. Reading the code says both are self-clearing — the
/// WC-G probe is budgeted per window id and returns `None` once spent, and `VERIFIED`'s bit is set
/// immediately after `draw_window` in the same loop body (so pass 1 excludes and pass 2 permits, and
/// the only clear is on window CREATE) — but "reading the code says" is what produced two wasted
/// sittings, so it is counted instead.
#[cfg(feature = "witness")]
static CUR12_EXCL_PROBE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static CUR12_EXCL_UNVERIFIED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-12 — total composite passes, the denominator every term above is read against.
#[cfg(feature = "witness")]
static CUR12_PASSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// CURSOR-12 — the attribution rollup: which predicate killed the offer, and how often.
///
/// Printed beside `[cursor3] rollup`, because it is the line that makes `offers=0` legible. The
/// reading is a single dominant term:
///
/// * **`nosprite` ≈ `passes`** — the render-bracket call graph above. The fix is not in the cursor
///   module at all: it is that `Screen::flush` composites from inside a bracket that has just taken
///   the sprite down. Compose-through cannot run on that path by construction.
///   **CURSOR-14 — read this term ONLY on a windowed block.** Every term is now a delta since the
///   previous rollup (`cum=` carries the boot total), and `pal::cursor::rollup_tick` no longer prints
///   on the operator's first report. Before those two changes this term was pinned at ≈ `passes` on
///   any kernel whatsoever, because a boot's worth of pre-pointer composites — during which the
///   sprite has never been drawn and `sprite_plan()` cannot answer — was permanently in the
///   numerator. A block whose `passes == cum` is that baseline and carries no verdict.
/// * **`nohit` ≈ `passes`** — the operator simply was not pointing at a window. Not a defect; check
///   the sitting, not the code.
/// * **`excl_probe` / `excl_unverified` non-trivial** — the instrument is suppressing the mechanism,
///   and the correct scoping is per-window-per-pass (exclude the ONE window the probe is bracketing on
///   the ONE pass it is spent on) rather than the general case. That is a witness-build-only defect
///   with production-only correct behaviour, which is the worst shape a defect can have.
/// * **`nosession` ≈ `passes`** — two cores compositing at once; CURSOR-5's territory.
/// * **`planned` non-zero with `[cursor3] offers=0`** — the death is BELOW the session, i.e. in
///   `stage_window`'s decline chain (`[wc-h] staged=no reason=…`), and `DECL_FIXTURE`/`DECL_LOCK` there
///   are the next things to read. A window that takes the DIRECT path never calls `compose_into` at
///   all, so it is un-composable for that pass by construction.
/// CURSOR-14 — the previous rollup's reading of every term above, so the block can report a WINDOW
/// instead of a running total. Order is the print order; see [`cursor12_rollup`] for why.
#[cfg(feature = "witness")]
static CUR12_PREV: [core::sync::atomic::AtomicU64; 8] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

/// CURSOR-14 — `now - prev`, and record `now` for the next block. Saturating, because two rollups
/// racing on different cores can read the counters in either order and a wrapped delta would print
/// as an enormous term.
#[cfg(feature = "witness")]
fn cur12_window(slot: usize, now: u64) -> u64 {
    use core::sync::atomic::Ordering::Relaxed;
    now.saturating_sub(CUR12_PREV[slot].swap(now, Relaxed))
}

#[cfg(feature = "witness")]
fn cursor12_rollup(scope: &str) {
    use core::sync::atomic::Ordering::Relaxed;
    // CURSOR-14 — EVERY TERM IS A WINDOW, NOT A RUNNING TOTAL, and the change is what makes this
    // block readable at all.
    //
    // The counters are cumulative from boot and were printed raw, which put every reading in
    // permanent debt to the pre-pointer era. Before the operator's first report the sprite has never
    // been drawn, so `sprite_plan()` is `None` on 100% of composites — a boot's worth of `nosprite`
    // that no later evidence can dilute. With a bench boot contributing ~40-70 such passes and a
    // sitting adding ~14/s, `nosprite * 2 >= passes` stays true for the first ten-odd seconds of
    // motion whatever the mechanism is actually doing, so the verdict reads `-> nosprite` on a
    // kernel where compose-through is working perfectly. Two seats spent three sittings reading that
    // number as a defect.
    //
    // A delta cannot carry that debt. `pal::cursor::rollup_tick` no longer prints on the first report
    // either, so the first block to reach the wire covers one full 5 s window of live pointer motion
    // with the sprite alive throughout — and `nosprite` in it finally means what its name says.
    // `cum=` keeps the boot total on the wire for continuity, and a block where `passes == cum` is
    // self-evidently the whole-boot baseline rather than a sample.
    let passes = cur12_window(0, CUR12_PASSES.load(Relaxed));
    let nosprite = cur12_window(1, CUR12_NOSPRITE.load(Relaxed));
    let nohit = cur12_window(2, CUR12_NOHIT.load(Relaxed));
    let reserved = cur12_window(3, CUR12_RESERVED.load(Relaxed));
    let nosession = cur12_window(4, CUR12_NOSESSION.load(Relaxed));
    let planned = cur12_window(5, CUR3_PLANNED.load(Relaxed));
    let probe = cur12_window(6, CUR12_EXCL_PROBE.load(Relaxed));
    let unver = cur12_window(7, CUR12_EXCL_UNVERIFIED.load(Relaxed));
    let cum = CUR12_PASSES.load(Relaxed);
    // The dominant term, named rather than left to arithmetic. Ties go to the earliest predicate in
    // the chain, which is the one a reader has to fix first anyway.
    let why = if passes == 0 {
        "none"
    } else if nosprite * 2 >= passes {
        "nosprite"
    } else if nohit * 2 >= passes {
        "nohit"
    } else if nosession * 2 >= passes {
        "nosession"
    } else if probe + unver > 0 && planned > 0 {
        "witness-exclusion"
    } else if planned > 0 {
        "below-session"
    } else {
        "mixed"
    };
    // CURSOR-16 (GR9 lift, 2026-07-30): the block states its own ADMISSIBILITY instead of leaving
    // it to a doc. GR9's x86 `passes=0` read as a compose-through defect for two rounds when it was
    // a SCENE fact: their presenter owns the panel with zero compositor windows, so `composite()`
    // is never entered and no offer site exists. `adm=` names which scene this block measured:
    //   empty    — composite() has never run this boot (no window layer; offers CANNOT exist)
    //   idle     — composite() has run before but not in this window (no damage; zeros are idle)
    //   baseline — first block, whole-boot totals (passes==cum)
    //   window   — live window-scene sample (the only adm under which `-> none/nosprite` indicts)
    let adm = if cum == 0 {
        "empty"
    } else if passes == 0 {
        "idle"
    } else if passes == cum {
        "baseline"
    } else {
        "window"
    };
    serial_println!(
        "[cursor12] offer scope={} adm={} passes={} nosprite={} nohit={} reserved={} nosession={} planned={} excl_probe={} excl_unverified={} cum={} -> {}",
        scope, adm, passes, nosprite, nohit, reserved, nosession, planned, probe, unver, cum, why
    );
}

/// CURSOR-3 — record one overlay offer and whether the layer took it. Called from `stage_window`,
/// which is inside the `BlitGuard` window: relaxed atomics only, no lock, no allocation, no serial.
///
/// CURSOR-4: `taken` now means "this window carried at least one sprite pixel through its present",
/// and a partial carry is counted as taken AND as a straddle — the pixels it did not carry are the
/// tail's, not a fallback to the bracket.
#[cfg(feature = "witness")]
fn note_cursor_overlay(c: &super::cursor::Composed) {
    use core::sync::atomic::Ordering::Relaxed;
    CUR3_OFFERS.fetch_add(1, Relaxed);
    if c.taken > 0 {
        CUR3_TAKEN.fetch_add(1, Relaxed);
    }
    if c.missed > 0 {
        CUR3_DECL_STRADDLE.fetch_add(1, Relaxed);
        // CURSOR-6 — and WHICH half. `taken` is the discriminator and it is already to hand, so the
        // split costs one branch on a path that was already doing four.
        if c.taken > 0 {
            CUR6_DECL_PARTIAL.fetch_add(1, Relaxed);
        } else {
            CUR6_DECL_DISJOINT.fetch_add(1, Relaxed);
        }
    }
    if c.locked {
        CUR3_DECL_LOCK.fetch_add(1, Relaxed);
    }
    if c.stale {
        CUR3_DECL_STALE.fetch_add(1, Relaxed);
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
        CursorTail::Settle => {
            CUR15_SETTLE.fetch_add(1, Relaxed);
            "settle"
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
            match tail {
                CursorTail::Adopt => "COMPOSED",
                // CURSOR-15 — the sessionless compose-through: not COMPOSED (nothing rode a
                // layer) and emphatically not BRACKETED (the arrow never left the glass).
                CursorTail::Settle => "THROUGH",
                _ => "BRACKETED",
            }
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
    let settle = CUR15_SETTLE.load(Relaxed);
    let ensure = CUR3_ENSURE.load(Relaxed);
    let straddle = CUR3_DECL_STRADDLE.load(Relaxed);
    let lock = CUR3_DECL_LOCK.load(Relaxed);
    let budget = CUR3_DECL_BUDGET.load(Relaxed);
    let stale = CUR3_DECL_STALE.load(Relaxed);
    // `taken > offers` would mean an overlay landed without being offered — a wiring defect, and the
    // only outcome here that is a defect rather than an absence of evidence.
    //
    // CURSOR-4 drops the old `adopt > taken` clause: `adopt` now counts passes that OWNED the overlay
    // session, and a pass legitimately owns one while composing nothing (every window declined, or
    // the pass exited at the drain barrier) — its tail still has the split sprite to settle. Under
    // CURSOR-3 that combination was impossible, which is why it read as incoherent then.
    let verdict = if taken > offers {
        "INCOHERENT"
    } else if offers == 0 {
        "UNWITNESSED"
    } else if taken == 0 {
        "BRACKETED"
    } else {
        "COMPOSED"
    };
    let disjoint = CUR6_DECL_DISJOINT.load(Relaxed);
    let partial = CUR6_DECL_PARTIAL.load(Relaxed);
    serial_println!(
        "[cursor3] rollup scope={} planned={} offers={} taken={} adopt={} repaint={} settle={} ensure={} straddle={} disjoint={} partial={} lock={} budget={} stale={} -> {}",
        scope, planned, offers, taken, adopt, repaint, settle, ensure, straddle, disjoint, partial,
        lock, budget, stale, verdict
    );
    // CURSOR-5 — the coherence residual, immediately after the decline breakdown so a bench capture
    // shows "how often the mechanism ran" and "what it cost when it raced" as one block.
    super::cursor::cursor5_rollup(scope);
    // CURSOR-6 — and what the PANEL got, which is the question neither of the two lines above can
    // reach: both are taken from inside the sprite's own bookkeeping, and an overwrite by a painter
    // that never consulted the module leaves that bookkeeping intact.
    super::cursor::cursor6_rollup(scope, planned);
    // CURSOR-8 — and what the repair COST, which is the question CURSOR-7 left a reader unable to ask:
    // `[cursor6] repaired=` says how often the arrow was rebuilt, and this says how often that was
    // asked for and what declined the rest. The two lines are adjacent because they share a counter.
    super::cursor::cursor8_rollup(scope);
    // CURSOR-12 — and WHY `offers=` above reads what it reads. Every other line here describes what
    // compose-through did; this one is the only line that can say whether it ran at all, and on both
    // benches the answer so far is that it did not.
    cursor12_rollup(scope);
    // FLICKER-2 — bracket latency and restore provenance beside the burst/drain pressure that can
    // cause both, one line, so the P79 flicker cadence question ("does it ride the 5 s rollup
    // burst?") is answerable from a single capture. The wm-side counters are passed in because the
    // cursor module must not reach back into this one.
    {
        use core::sync::atomic::Ordering::Relaxed;
        super::cursor::flick2_rollup(
            scope,
            F2W_DRAINS.load(Relaxed),
            F2W_DRAIN_SKIPS.load(Relaxed),
            F2W_DRAIN_MASKED.load(Relaxed),
            WCN_BURST_LAST_MS.load(Relaxed),
            WCN_BURST_MAX_MS.load(Relaxed),
        );
    }
    // VUGMIN-B — and who the window layer told to go to sleep, on the same line block for the same
    // reason: it is a fact about this compositor's passes, and the number it reports (`presents
    // skipped`) is a subtraction from `cursor_passes` above.
    vugmin_rollup(scope);
}

/// VUGMIN-B — the hidden-owner rollup: how many owners `wm` pushed below the shell, how many it brought
/// back, and how many `SYS_WIN_PRESENT` composites that cost nothing were suppressed.
///
/// **Honest scope on the gate.** The headless QEMU run has no HID, so nothing ever TABs to the shell,
/// so `focus_changed(0)` is never reached from an operator and no owner is ever hidden. All three
/// counters are 0 there and the verdict says `DORMANT` rather than `CLEAN` — the line proves the
/// counters are wired and that nothing hid by accident, which is exactly the claim a headless run can
/// support. The number that carries the arc is `hides > 0` on the bench, where a shell TAB exists.
#[cfg(feature = "witness")]
fn vugmin_rollup(scope: &str) {
    use core::sync::atomic::Ordering::Relaxed;
    let hides = VUGMIN_HIDES.load(Relaxed);
    let unhides = VUGMIN_UNHIDES.load(Relaxed);
    let skipped = VUGMIN_SKIPPED.load(Relaxed);
    let live = VUGMIN_SHADOW.load(Relaxed).count_ones();
    let verdict = if hides == 0 { "DORMANT" } else { "ENGAGED" };
    serial_println!(
        "[vugmin] wm scope={} hides={} unhides={} presents_skipped={} hidden_now={} -> {}",
        scope, hides, unhides, skipped, live, verdict
    );
    wcn_emit_forced(scope);
}

// ---- WC-N: the per-window present-rate rollup ---------------------------------------------------

/// WC-N — **"predetermined fps" as wire data.**
///
/// Two numbers already existed and neither answers the question. `[vugfps]` (user-vug) is the app's
/// own count of frames it *issued*, drawn in its corner — it cannot know whether a frame reached
/// glass. `[sched6]` counts the render task's passes and composites — fleet-wide, with no window in
/// it. So when Peter reads six vugs running at visibly different rates, nothing on the wire says
/// whether a given vug's rate is a CONSEQUENCE (it is competing for cores and the compositor) or a
/// CEILING (something is pacing it at a fixed rate regardless of load). This rollup is that fact,
/// per window, from the compositor's own side of the seam.
///
/// Four counts per live window, each a delta over the rollup window:
///  * `att` — [`present`] calls that named this row. The owner's *attempt*: `SYS_WIN_PRESENT`
///    performed its ownership check and handed us a finished surface.
///  * `comp` — times this row was actually blitted by [`composite_inner`]'s loop. Pixels on glass.
///    It can legitimately EXCEED `att`: a neighbour's present grows the dirty set upwards over
///    occlusion, and this row is repainted inside that neighbour's pass. `comp > att` is therefore a
///    reading about overlap, not an error — it is compositor work this window's owner did not ask
///    for and does not know about.
///  * `hid` — presents suppressed by the VUGMIN-B arm in [`present`]: every window this owner holds
///    was below `SHELL_Z`, so the pass would have written nowhere the operator can see.
///  * `bel` — passes in which this row was in the dirty set and then declined by [`above_shell`].
///    The same fact as `hid` seen from the compositor's end: `hid` is the owner's own present being
///    dropped, `bel` is somebody ELSE's pass declining to repaint this row.
///
/// **There is no occlusion cull, so there is no third skip class, and this rollup does not invent
/// one.** A window wholly covered by another is still blitted — the dirty-set closure exists to
/// repaint what is ON TOP, not to drop what is underneath. `comp - att` is where that cost shows.
///
/// ### The park, and why the rate is not `att / span`
///
/// VUGPAUSE-2 makes an idle vug *leave the run queues*: it blocks in the input wait and presents
/// nothing at all, for as long as the operator leaves it alone. Divided by wall-clock span, that vug
/// reads as `0.2/s` — indistinguishable from a vug that is being starved of cores at 0.2 fps, which
/// is the exact confusion this witness exists to remove. So the denominator is the window's own
/// ACTIVE time: consecutive presents closer together than [`WCN_PARK_GAP_MS`] accumulate into
/// `active`, and any longer gap accumulates into `parked` instead and is charged to neither the
/// numerator nor the denominator. A parked vug reports the rate it ran at *while it was running*,
/// with its park time stated beside it. (The window's first present after a park opens a new active
/// span rather than closing one, so `att` overstates the active interval by at most one present per
/// park — visible only at very low counts, and always in the direction of a slightly optimistic
/// rate.)
///
/// ### `gap` is the ceiling test
///
/// `gapmin`/`gapmax` are the shortest and longest ACTIVE inter-present gaps in the window, in ms. A
/// rate that is a consequence of load scatters — a contended vug's gaps run from a few ms to tens.
/// A rate that is a fixed ceiling does not: `gapmin` and `gapmax` collapse onto the same value
/// (something is pacing the loop), and the collapse is visible on one line without a second run.
/// That pair, not the rate, is what makes "predetermined fps" checkable.
#[cfg(feature = "witness")]
const WCN_ROLLUP_MS: u64 = 5000;

/// WC-N — an inter-present gap longer than this is a PARK, not a slow frame. VUGPAUSE-2's backstop
/// period is ~256 ms and a parked vug's next present waits on operator input, so anything past a
/// quarter second is provably not the render loop pacing itself. The slowest rate this can misread
/// as active is 4/s, which is already far below anything the fleet produces while rendering.
#[cfg(feature = "witness")]
const WCN_PARK_GAP_MS: u64 = 250;

/// WC-N — one window slot's accumulators. Plain atomics rather than a `Mutex`: this is written from
/// [`present`] (the owner's core, at frame rate) and from [`composite_inner`]'s blit loop (any core),
/// and a witness must not add a lock to a path whose contention it is trying to measure. Every field
/// is `Relaxed` — no other state is published through them and the rollup reads them one at a time
/// anyway, so a line that catches a counter mid-increment is off by one and never inconsistent.
#[cfg(feature = "witness")]
struct WcnRow {
    att: core::sync::atomic::AtomicU64,
    comp: core::sync::atomic::AtomicU64,
    hid: core::sync::atomic::AtomicU64,
    bel: core::sync::atomic::AtomicU64,
    /// Summed inter-present gaps at or under [`WCN_PARK_GAP_MS`] — the window's own active time.
    active_ms: core::sync::atomic::AtomicU64,
    /// Summed gaps longer than that — VUGPAUSE-2 park, excluded from the rate's denominator.
    parked_ms: core::sync::atomic::AtomicU64,
    /// Shortest / longest ACTIVE gap this rollup window. `u64::MAX` is the "no gap yet" sentinel.
    gap_min: core::sync::atomic::AtomicU64,
    gap_max: core::sync::atomic::AtomicU64,
    /// `arch::ms()` of the last present that named this row. `0` = none yet. NOT reset at rollup:
    /// the activity span is a property of the window, not of the reporting cadence, so a rollup
    /// boundary must not manufacture a park out of the gap it straddles.
    last_ms: core::sync::atomic::AtomicU64,
}

#[cfg(feature = "witness")]
impl WcnRow {
    const fn new() -> Self {
        use core::sync::atomic::AtomicU64;
        Self {
            att: AtomicU64::new(0),
            comp: AtomicU64::new(0),
            hid: AtomicU64::new(0),
            bel: AtomicU64::new(0),
            active_ms: AtomicU64::new(0),
            parked_ms: AtomicU64::new(0),
            gap_min: AtomicU64::new(u64::MAX),
            gap_max: AtomicU64::new(0),
            last_ms: AtomicU64::new(0),
        }
    }
}

/// WC-N — one slot per window id (`id - 1`). Written out rather than repeated by a `Copy` splat
/// because `WcnRow` holds atomics; the assertion below is what keeps it honest if [`MAX_WINDOWS`]
/// ever moves.
#[cfg(feature = "witness")]
static WCN: [WcnRow; 8] = [
    WcnRow::new(),
    WcnRow::new(),
    WcnRow::new(),
    WcnRow::new(),
    WcnRow::new(),
    WcnRow::new(),
    WcnRow::new(),
    WcnRow::new(),
];

#[cfg(feature = "witness")]
const _: () = assert!(WCN.len() == MAX_WINDOWS);

/// WC-N — composite passes that reached the blit loop, and passes that returned before it (the F4
/// drain barrier was up, or the framebuffer was not ready). An aborted pass is a present that cost
/// its owner a syscall and produced no pixels for ANY window, so it belongs on the aggregate line
/// rather than charged to whichever row happened to trigger it.
#[cfg(feature = "witness")]
static WCN_PASSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static WCN_ABORTED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-N — presents that named no live row at all (a window closed under its owner). Aggregate-only:
/// there is no slot to charge them to, and that is exactly what makes them worth counting.
#[cfg(feature = "witness")]
static WCN_STALE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WC-N — `arch::ms()` at the last rollup. Also the CLAIM: a core that wins the compare-exchange
/// owns the emit and every other core in the same instant falls through to its present.
#[cfg(feature = "witness")]
static WCN_LAST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// ---- COMPOSITE-2 — the per-pass cost ledger -----------------------------------------------------
//
// The compositor's measured wall is ~123 passes/s aggregate (~8 ms per pass at 1920x1200), which is
// the fps ceiling for the whole vug fleet while V3D stays walled. Before rebuilding the hot path the
// wire must say where those 8 ms go, and afterwards it must prove they moved. One [comp2] rollup
// line rides the [wcn] cadence and partitions every pass's wall time into four accounts:
//
//   * `sprite_us` — the pre-pass region (deferred-erase drain + cursor bracket) plus the tail
//     (`adopt`/`repaint`/`ensure_drawn`). Everything the sprite contract costs a pass.
//   * `wait_us`   — WRITER read, TABLE lock, damage close, ordering, guard registration: the
//     serialisation cost between the bracket and the first blit.
//   * `blit_us`   — the blit loop proper (compose + present copies), MINUS the cache term below.
//   * `cache_us`  — `draw_window`'s trailing `flush_range` (`DC CVAC` sweep + `DSB`), the
//     non-coherent scan-out's tax, measured separately because it scales with CLEANED bytes and
//     the fix for it (clean the box, not full-width scanlines) is distinct from the blit fix.
//
// Plus the denominators every claim needs: `bytes_pp` (panel bytes written per pass), `dmg_px_pp`
// (the area those bytes covered) and `box_px_pp` (the area the same passes' outer boxes would have
// covered whole). `witness`-gated and nothing else — like `wcg`, and unlike the `[fluid3]` ledger
// below, which reads aarch64's own scheduler and therefore stays there. Counters are relaxed
// increments on the hot path and are drained by the emit.
//
// ## FBCON-DMG — why the pass's two extents are now two counters
//
// This ledger was `all(target_arch = "aarch64", feature = "witness")` for as long as the only banded
// present in the tree was x86's (`fbcon::route_present_rows`, `all(x86_64, feature = "wc")`). On the
// one arch that counted, `band` was therefore provably always `None`, the outer box WAS the extent
// written, and charging `bw * bh` was exact rather than approximate.
//
// Widening the gate alone would have turned that identity into a lie without changing a line of
// arithmetic. The x86 console window bands hard — metal, this morning, a 736-row surface presenting
// a 96-row minimum span — so a whole-box charge there would over-report `bytes` and `dmg_px` by the
// box/band ratio, up to ~8x on that measurement. Every per-byte figure divided out of this line
// would then have been wrong by that factor in the flattering direction (cost per byte too LOW,
// throughput per pass too HIGH), which is the failure mode a cost ledger exists to prevent.
//
// So the two extents are charged separately: `dmg_px` is what `draw_window` actually painted and
// `box_px` is what the same call would have painted whole. Their ratio is the banding ratio, ON THE
// LINE — which is what makes a wrong widening falsifiable from the ledger by itself instead of only
// by cross-reading `[wc-h] minspan=`. Equal terms mean nothing banded, and on aarch64 they are equal
// BY CONSTRUCTION (no caller of [`present_rows`] compiles for that arch — see [`Window::dmg_y0`] and
// the counting block in [`draw_window`]), so aarch64's numbers are exactly what they were before the
// widening, including this line's new field.
#[cfg(feature = "witness")]
static C2_PASSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_PASS_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_PASS_MAX_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_SPRITE_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_WAIT_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_LOOP_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_CACHE_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static C2_DMG_PX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// FBCON-DMG — the whole-box area of the same `draw_window` calls `C2_DMG_PX` charges the painted
/// area of. Never used as a denominator; it exists so the line can be read against itself.
#[cfg(feature = "witness")]
static C2_BOX_PX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// COMPOSITE-2 — drain the ledger and print the rollup line. Called from [`wcn_emit`] so the two
/// instruments share one cadence and one `span`; all averages are per-pass, in microseconds.
/// `blit_us` subtracts the cache term because the flush is timed INSIDE the loop (it is per-window,
/// in `draw_window`), and reporting it twice would make the line un-addable.
///
/// The cycle→microsecond conversion is `wcg`'s, which already carries a reader per arch (CNTVCT at
/// `CNTFRQ_EL0` on aarch64, `rdtsc` at the rate `apic::calibrate` measured on x86); this line
/// therefore needs no arch split of its own, and the two arches' `*_us` figures mean the same thing.
///
/// FBCON-DMG — `box_px_pp` is INSERTED between `dmg_px_pp` and `rate=`, not appended: `span=` stays
/// the terminal field so any matcher anchored on the end of this line is unaffected. See the module
/// note above for what the pair is for.
#[cfg(feature = "witness")]
fn comp2_emit(span: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let passes = C2_PASSES.swap(0, Relaxed);
    let pass_cyc = C2_PASS_CYC.swap(0, Relaxed);
    let max_cyc = C2_PASS_MAX_CYC.swap(0, Relaxed);
    let sprite_cyc = C2_SPRITE_CYC.swap(0, Relaxed);
    let wait_cyc = C2_WAIT_CYC.swap(0, Relaxed);
    let loop_cyc = C2_LOOP_CYC.swap(0, Relaxed);
    let cache_cyc = C2_CACHE_CYC.swap(0, Relaxed);
    let bytes = C2_BYTES.swap(0, Relaxed);
    let dmg_px = C2_DMG_PX.swap(0, Relaxed);
    let box_px = C2_BOX_PX.swap(0, Relaxed);
    if passes == 0 {
        return;
    }
    let us = |cyc: u64| super::wcg::cycles_to_us(cyc / passes);
    serial_println!(
        "[comp2] rollup passes={} pass_us={} max_us={} sprite_us={} wait_us={} blit_us={} cache_us={} bytes_pp={} dmg_px_pp={} box_px_pp={} rate={}.{}/s span={}ms",
        passes,
        us(pass_cyc),
        super::wcg::cycles_to_us(max_cyc),
        us(sprite_cyc),
        us(wait_cyc),
        us(loop_cyc.saturating_sub(cache_cyc)),
        us(cache_cyc),
        bytes / passes,
        dmg_px / passes,
        box_px / passes,
        passes.saturating_mul(10_000) / span.max(1) / 10,
        passes.saturating_mul(10_000) / span.max(1) % 10,
        span
    );
}

// ---- FLUID-3 — the vug-side wait ledger ----------------------------------------------------------
//
// P83 (Peter, live): pointer motion grows a fleet core's idle reserve, and each vug settles to a
// characteristic fps below capacity. The load meter counts service time as busy, so the reserve is
// REAL idle — fleet tasks leaving the run queues. The present path cannot queue (a present IS its
// composite, run inline on the caller's core; there is no ack rendezvous to wait on), so the only
// parks a live vug takes are its futex parks: the frame barrier behind its workers, and the workers
// behind the next release. This line prices them, beside the present-side concurrency:
//
//   * `parks` / `park_us mean/max` / `p50/p90/p99` — completed futex parks this window and their
//     duration distribution (log2 buckets, computed in `sched::fluid3_drain`). The percentiles are
//     bucket UPPER bounds. Barrier parks behind healthy workers read tens-to-hundreds of us;
//     milliseconds here is a parent parked behind a starved worker — the invisible idle the SCHED
//     load line shows as reserve; the top bucket (>32 ms) is idle vugs on their input rings.
//   * `depth_max` — high-water of concurrently in-flight composites (`BLIT_ACTIVE` sampled at guard
//     registration). >1 proves presents do NOT serialize behind one consumer; 1 under a saturated
//     fleet would mean they do.
//   * `overlap` — passes that entered with another composite already in flight.
//
// aarch64 + witness, drained on the [wcn]/[comp2] cadence. The sched half lives in
// `arch::aarch64::sched` (`fluid3_note_park` / `fluid3_drain`).
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
static FL3_DEPTH_MAX: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
static FL3_OVERLAP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// FLUID-3 — drain both halves of the ledger and print the rollup. Percentile figures are the upper
/// bound of the log2 bucket in which the cumulative count crossed the mark, in microseconds.
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
fn fluid3_emit(span: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let (parks, mean_us, max_us, hist) = crate::arch::aarch64::sched::fluid3_drain();
    let depth = FL3_DEPTH_MAX.swap(0, Relaxed);
    let overlap = FL3_OVERLAP.swap(0, Relaxed);
    if parks == 0 && depth == 0 {
        return;
    }
    let pct = |mark: u64| -> u64 {
        // Smallest bucket upper bound covering `mark` parks cumulatively; `mark` is 1-based.
        let mut cum = 0u64;
        for (b, &h) in hist.iter().enumerate() {
            cum += h as u64;
            if cum >= mark {
                return 1u64 << b;
            }
        }
        1u64 << (hist.len() - 1)
    };
    serial_println!(
        "[fluid3] parks={} park_us mean={} max={} p50<={} p90<={} p99<={} depth_max={} overlap={} span={}ms",
        parks,
        mean_us,
        max_us,
        pct(parks.div_ceil(2)),
        pct((parks.saturating_mul(9)).div_ceil(10)),
        pct((parks.saturating_mul(99)).div_ceil(100)),
        depth,
        overlap,
        span
    );
}

// ---- FLICKER-2 — the burst/drain pressure counters ----------------------------------------------
//
// Symptom (a) of Peter's P79 sitting is a slight cursor flicker on a ~5 s "pulse" — the cadence of
// the [wcn] rollup burst above. The emission itself holds NO compositor lock (the `TABLE` snapshot
// in `wcn_emit` is dropped before the first print) and runs at the tail of `present`, OUTSIDE the
// composite pass and its cursor bracket — but on metal every line the winning core puts on the wire
// is ~13 ms of IRQ-masked 115200-baud polling, and the UART holder also drains up to `serial_ring::
// SLOTS` lines other cores staged. A timer-IRQ witness site ([pulse5]/[spread4]/[prio]) that fires
// on a core which is MID-composite therefore stretches that pass — and any cursor bracket it holds —
// by the whole burst. QEMU cannot reproduce any of this (no drawn sprite, instant UART), so the
// burst is MEASURED instead: `wcn_emit` records its own wall time here, and `[flick2]` reports it
// beside the sprite's observed down-intervals (see `cursor::flick2_rollup`) so the next bench boot
// can read whether the flicker Peter sees rides the burst.
#[cfg(feature = "witness")]
static WCN_BURST_LAST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "witness")]
static WCN_BURST_MAX_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// FLICKER-2 — drains that found queued boxes (the population the two counters below partition).
#[cfg(feature = "witness")]
static F2W_DRAINS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// FLICKER-2 — drains whose boxes never met the sprite: the sprite was left on the panel untouched.
/// Before this arc every one of these was a whole-sprite restore→repaint over the window under the
/// pointer.
#[cfg(feature = "witness")]
static F2W_DRAIN_SKIPS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// FLICKER-2 — drains that met the sprite and took the MASKED handback instead of the full undraw.
#[cfg(feature = "witness")]
static F2W_DRAIN_MASKED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WEDGE-1r2 — what the drain barrier actually cost this window, below the tripwire's threshold.
///
/// ### The admissibility clause, because this line's SILENCE is the thing that was misread
/// `drains=0` says NO TEARDOWN RAN in this window. It does not say the drain barrier is healthy, and
/// it is not evidence about any wedge: a boot with no window close and no task exit never reaches
/// that code at all. The inference WEDGE-1's watch-list drew from a quiet tripwire — "the drain
/// barrier is exonerated" — is exactly the reading this line exists to make impossible to repeat, so
/// the verdict names the SCENE and never the outcome:
///
/// * `STRADDLE` — spin evidence with zero completed drains in this window: the drain that produced
///                it was counted in a neighbouring window's `drains` (it straddled the rollup
///                boundary). Read beside those neighbours. (A straddle whose `spin_max` clears the
///                note prints `DWELL`, not `STRADDLE` — the precedence is severity-first.) A window
///                with no evidence at all does not print; there is no `NONE` verdict.
/// * `QUIET`    — drains ran, and none spun past [`DRAIN_DWELL_NOTE`]. The healthy steady state, and
///                the only verdict here that is a statement about the barrier rather than about the
///                scene.
/// * `DWELL`    — a drain spun far enough to be a stalled core, four orders of magnitude under the
///                tripwire. That is IRQ-masked time on a teardown's core, and it is the reading the
///                tripwire's own threshold note deliberately declined to produce.
/// * `INFLIGHT` — a drain was inside the spin at the instant ANOTHER core took this line. At operator
///                teardown rates against a 5 s window, one sample is a coincidence worth noting and
///                the same reading twice running is a core that is not coming out.
///
/// `tripwire=` carries [`DRAIN_STALL_REPORTED`] so the two instruments are read together: `fired`
/// beside a `DWELL` is one stall growing, and `silent` beside an `INFLIGHT` is a drain that is stuck
/// but has not yet reached [`DRAIN_STALL_SPINS`] — the state that used to produce nothing at all.
///
/// Emitted AHEAD of [`wcn_emit`]'s dirty-paced guard: a wedged teardown holds a core with interrupts
/// masked, and if the only thing keeping this block off the wire were "no window presented in the
/// last five seconds", the reading that names the wedge would be suppressed by the very condition it
/// describes. Self-silencing when there is genuinely nothing to say, so an idle desktop still prints
/// no wall of zeros.
#[cfg(feature = "witness")]
fn wedge1_dwell_emit(span: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let drains = F4W_DRAINS.swap(0, Relaxed);
    let spun = F4W_SPUN.swap(0, Relaxed);
    let spin_max = F4W_SPIN_MAX.swap(0, Relaxed);
    // A GAUGE — loaded, never drained. Draining it would report a stuck core once and then forget it,
    // which is the opposite of what a standing count is for.
    let in_spin = F4W_IN_SPIN.load(Relaxed);
    // Lens fix (s1u): the quiet-window early-out must also test the spin evidence. The swaps above
    // have already drained `spun`/`spin_max`, so a drain that straddled the previous rollup boundary
    // (counted there as `drains`, its spin published here) would have its DWELL evidence silently
    // dropped by a `drains==0` return — banking QUIET for a window that measured a stall.
    if drains == 0 && in_spin == 0 && spun == 0 && spin_max == 0 {
        return;
    }
    let verdict = if in_spin > 0 {
        "INFLIGHT"
    } else if spin_max >= DRAIN_DWELL_NOTE {
        "DWELL"
    } else if drains == 0 {
        // Reachable only via the straddle case above: spin evidence with zero completed drains in
        // THIS window — the drain that produced it was counted in a neighbouring window's `drains`.
        "STRADDLE"
    } else if spin_max > 0 {
        // WEDGE-1r3 (PA6 metal, 2026-08-01). A window that MEASURED a spin may not be called
        // QUIET, whose contract is "the healthy steady state". PA6 printed
        //   drains=20 spun=1 spin_max=6890 ... -> QUIET
        // — the first non-zero spin this track has ever recorded, banked as healthy, because the
        // only gate above was `spin_max >= DRAIN_DWELL_NOTE`. `DRAIN_DWELL_NOTE` is 1<<16 and is
        // NOT calibrated against any measurement: its stated justification is relative only ("four
        // orders under DRAIN_STALL_SPINS"), so a real dwell can sit at 65535 forever and every
        // window still reads QUIET. Lowering the constant would only move an arbitrary line; the
        // honest fix is to stop claiming health for a window that has evidence, and let the number
        // speak. SPUN is not a fault — it is "contention was observed and stayed under the note".
        "SPUN"
    } else {
        "QUIET"
    };
    serial_println!(
        "[wedge1] dwell drains={} spun={} spin_max={} note={} in_spin={} tripwire={} span={}ms -> {}",
        drains,
        spun,
        spin_max,
        DRAIN_DWELL_NOTE,
        in_spin,
        if DRAIN_STALL_REPORTED.load(Relaxed) { "fired" } else { "silent" },
        span,
        verdict
    );
}

/// WC-N — the slot for `id`, or `None` for an out-of-range id.
#[cfg(feature = "witness")]
fn wcn_slot(id: WinId) -> Option<&'static WcnRow> {
    if id == WIN_NONE {
        return None;
    }
    WCN.get(id as usize - 1)
}

/// WC-N — record one [`present`] against `id`, and fold its inter-present gap into the active/parked
/// split. Called from `present` with the table lock DROPPED (`arch::ms()` is a register read, but the
/// rule in this file is that nothing is called out from under `TABLE` and this is not the exception).
#[cfg(feature = "witness")]
fn wcn_note_present(id: WinId, hidden: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    let Some(s) = wcn_slot(id) else { return };
    s.att.fetch_add(1, Relaxed);
    if hidden {
        s.hid.fetch_add(1, Relaxed);
    }
    // `max(1)` keeps 0 as the "never presented" sentinel even if this is the very first millisecond
    // of the boot; the cost is one ms of misattribution, once, ever.
    let now = crate::arch::ms().max(1);
    let last = s.last_ms.swap(now, Relaxed);
    if last != 0 && now > last {
        let gap = now - last;
        if gap <= WCN_PARK_GAP_MS {
            s.active_ms.fetch_add(gap, Relaxed);
            s.gap_min.fetch_min(gap, Relaxed);
            s.gap_max.fetch_max(gap, Relaxed);
        } else {
            s.parked_ms.fetch_add(gap, Relaxed);
        }
    }
}

/// WC-N — record that `id`'s row was blitted by a composite pass. The only "reached glass" writer.
#[cfg(feature = "witness")]
fn wcn_note_drawn(id: WinId) {
    if let Some(s) = wcn_slot(id) {
        s.comp
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// WC-N — record that `id`'s row was in a pass's dirty set and declined for sitting below the shell.
#[cfg(feature = "witness")]
fn wcn_note_below(id: WinId) {
    if let Some(s) = wcn_slot(id) {
        s.bel.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// WC-N — one composite pass reached (`drew = true`) or did not reach (`drew = false`) the blit loop.
#[cfg(feature = "witness")]
fn wcn_note_pass(drew: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    if drew {
        WCN_PASSES.fetch_add(1, Relaxed);
    } else {
        WCN_ABORTED.fetch_add(1, Relaxed);
    }
}

/// WC-N — the dirty-paced tick, called at the tail of every [`present`].
///
/// Cadence follows `[pstrip]`/`[sched6]`: a fixed rollup period, one claim per period, and NOTHING
/// printed for a period in which no window attempted or completed a present. Two consequences worth
/// stating, because both are deliberate:
///  * the line volume is bounded by construction — at most one block (live windows + one aggregate)
///    per [`WCN_ROLLUP_MS`], however many cores are compositing;
///  * a fleet that has wholly parked goes SILENT rather than printing a wall of zeros. The witness is
///    driven by the traffic it measures, so "no `[wcn]` lines" reads as "nobody is presenting", which
///    is the same thing the absence of the lines would have meant anyway.
///
/// The period's final partial window is therefore never emitted on its own account. The forced
/// [`wcn_emit_forced`] path — fired from the fixture/desktop rollup beside `[vugmin]` — is what
/// guarantees the gate a block regardless.
#[cfg(feature = "witness")]
fn wcn_tick() {
    use core::sync::atomic::Ordering::{AcqRel, Relaxed};
    let now = crate::arch::ms();
    let last = WCN_LAST_MS.load(Relaxed);
    let span = now.wrapping_sub(last);
    if span < WCN_ROLLUP_MS {
        return;
    }
    // Claim the window. A loser does not retry: the winner is emitting the same evidence.
    if WCN_LAST_MS
        .compare_exchange(last, now, AcqRel, Relaxed)
        .is_err()
    {
        return;
    }
    wcn_emit("live", span, false);
}

/// WC-N — force a block out now, whatever the cadence says. Called from [`vugmin_rollup`] so the
/// arc's line appears on the same scoped rollup every other window witness reports on, including on
/// a headless gate whose last rollup window is still open when the fixture ends.
#[cfg(feature = "witness")]
fn wcn_emit_forced(scope: &str) {
    use core::sync::atomic::Ordering::Relaxed;
    let now = crate::arch::ms();
    let last = WCN_LAST_MS.swap(now, Relaxed);
    wcn_emit(scope, now.wrapping_sub(last), true);
}

/// WC-N — drain the accumulators and print. `span` is the wall-clock length of the window being
/// reported; `force` prints the aggregate even when nothing happened in it.
///
/// Counters are drained with `swap`, not read-then-store: a present landing on another core during
/// the emit is carried into the NEXT window rather than lost, which is what keeps the per-window
/// totals summable across a whole boot.
#[cfg(feature = "witness")]
fn wcn_emit(scope: &str, span: u64, force: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    // FLICKER-2 — the burst's own wall clock, from the identity snapshot to the last rollup byte.
    // The reading means "how long the emitting core was occupied by this block"; it is stored only
    // at the end of the function, so silent early-outs never overwrite a real burst's figure.
    let t_burst = crate::arch::ms();
    // Identity for the per-window lines, snapshotted under one acquisition so every line in a block
    // is judged against one table state and one shell position. A slot with traffic but no live row
    // is a window that closed inside this rollup window; it still gets its line (`live=no`), because
    // dropping it would silently delete the last few frames of every window that ever exits.
    let mut ident = [(0u64, false, false); MAX_WINDOWS]; // (owner asid, live, above shell)
    {
        let t = table();
        let shell = SHELL_Z.load(core::sync::atomic::Ordering::Acquire);
        for r in t.rows.iter() {
            if !r.used || r.id == WIN_NONE || r.id as usize > MAX_WINDOWS {
                continue;
            }
            ident[r.id as usize - 1] = (r.owner_asid, true, above_shell(r, shell));
        }
    }
    let (mut t_att, mut t_comp, mut t_hid, mut t_bel) = (0u64, 0u64, 0u64, 0u64);
    let mut wins = 0usize;
    let mut lines: [Option<WcnLine>; MAX_WINDOWS] = [None; MAX_WINDOWS];
    for (i, s) in WCN.iter().enumerate() {
        let att = s.att.swap(0, Relaxed);
        let comp = s.comp.swap(0, Relaxed);
        let hid = s.hid.swap(0, Relaxed);
        let bel = s.bel.swap(0, Relaxed);
        let active = s.active_ms.swap(0, Relaxed);
        let parked = s.parked_ms.swap(0, Relaxed);
        let gmin = s.gap_min.swap(u64::MAX, Relaxed);
        let gmax = s.gap_max.swap(0, Relaxed);
        t_att += att;
        t_comp += comp;
        t_hid += hid;
        t_bel += bel;
        let (asid, live, above) = ident[i];
        if live {
            wins += 1;
        }
        if att == 0 && comp == 0 && bel == 0 {
            continue;
        }
        // Tenths, for `[pstrip]`'s reason: an integer /s truncates every honest sub-1 Hz rate to 0.
        // The rate's denominator is the window's ACTIVE time (see the module note above) and falls
        // back to the wall span only when this window recorded no gap at all — a single present in
        // the whole rollup, where there is no active interval to divide by and the wall answer is
        // the honest upper bound rather than a divide by zero.
        let den = if active > 0 { active } else { span.max(1) };
        lines[i] = Some(WcnLine {
            id: (i + 1) as WinId,
            asid,
            live,
            above,
            att,
            comp,
            hid,
            bel,
            arate: att.saturating_mul(10_000) / den.max(1),
            crate_: comp.saturating_mul(10_000) / den.max(1),
            active,
            parked,
            // The "no active gap at all" sentinel reports as `0..0`, which is what a single-present
            // window honestly has: no interval to measure.
            gap_min: if gmin == u64::MAX { 0 } else { gmin },
            gap_max: gmax,
        });
    }
    // WEDGE-1r2 — the drain barrier's dwell, taken BEFORE the dirty-paced guard below. A teardown
    // that has consumed a core is a teardown that is not presenting, so gating this reading on
    // present traffic would silence it under exactly the condition it reports on. It has its own
    // self-silencing test (no drains, nobody spinning), so an idle desktop stays as quiet as before.
    wedge1_dwell_emit(span);
    if !force && t_att == 0 && t_comp == 0 {
        return; // dirty-paced: a period with no present traffic prints nothing at all.
    }
    for l in lines.iter().flatten() {
        serial_println!(
            "[wcn] win={} asid={:#x} live={} above={} att={} comp={} hid={} bel={} rate={}.{}/s comp_rate={}.{}/s active={}ms parked={}ms gap={}..{}ms",
            l.id,
            l.asid,
            if l.live { "yes" } else { "no" },
            if l.above { "yes" } else { "no" },
            l.att,
            l.comp,
            l.hid,
            l.bel,
            l.arate / 10,
            l.arate % 10,
            l.crate_ / 10,
            l.crate_ % 10,
            l.active,
            l.parked,
            l.gap_min,
            l.gap_max
        );
    }
    let passes = WCN_PASSES.swap(0, Relaxed);
    let aborted = WCN_ABORTED.swap(0, Relaxed);
    let stale = WCN_STALE.swap(0, Relaxed);
    let att_rate = t_att.saturating_mul(10_000) / span.max(1);
    let comp_rate = t_comp.saturating_mul(10_000) / span.max(1);
    // The aggregate's denominator IS wall-clock, and deliberately so: "what did the fleet cost the
    // panel over these five seconds" is a wall-clock question, and a fleet in which one vug is parked
    // and five are running genuinely did present less per second of panel time. The per-window lines
    // above are where the park is factored out; conflating the two denominators on one line would
    // make the aggregate un-addable from its own rows.
    let verdict = if t_att == 0 {
        "IDLE"
    } else if t_comp == 0 {
        "STARVED"
    } else {
        "LIVE"
    };
    serial_println!(
        "[wcn] rollup scope={} wins={} att={} comp={} hid={} bel={} stale={} passes={} aborted={} att_rate={}.{}/s comp_rate={}.{}/s span={}ms -> {}",
        scope,
        wins,
        t_att,
        t_comp,
        t_hid,
        t_bel,
        stale,
        passes,
        aborted,
        att_rate / 10,
        att_rate % 10,
        comp_rate / 10,
        comp_rate % 10,
        span,
        verdict
    );
    // COMPOSITE-2 — the cost ledger rides the same cadence and the same span, on BOTH arches now:
    // the counters and the emit are `witness`-gated and nothing else, and this block already is.
    comp2_emit(span);
    // FLUID-3 — the wait ledger rides the same cadence and the same span.
    #[cfg(target_arch = "aarch64")]
    fluid3_emit(span);
    // FLICKER-2 — stored only when a block actually went on the wire, so `burst_last` names the most
    // recent REAL burst rather than the last silent early-out. On metal this is dominated by the
    // IRQ-masked UART time of the lines above (plus any staged backlog the winning core drained);
    // in QEMU it reads ~0, which is itself the point — the stall being measured does not exist there.
    let dt = crate::arch::ms().wrapping_sub(t_burst);
    WCN_BURST_LAST_MS.store(dt, Relaxed);
    if dt > WCN_BURST_MAX_MS.load(Relaxed) {
        WCN_BURST_MAX_MS.store(dt, Relaxed);
    }
}

/// WC-N — one drained per-window line, held on the stack between the drain loop and the print loop.
/// The two are separate passes because the DECISION to print at all is a property of the whole block
/// (`force || any traffic`), and the drain must happen either way: a rollup that read the counters,
/// decided to stay silent, and left them standing would fold two windows' traffic into the next line
/// and misreport every rate in it.
#[cfg(feature = "witness")]
#[derive(Clone, Copy)]
struct WcnLine {
    id: WinId,
    asid: u64,
    live: bool,
    above: bool,
    att: u64,
    comp: u64,
    hid: u64,
    bel: u64,
    /// Attempt / completion rates in TENTHS of a present per second, over the active denominator.
    arate: u64,
    crate_: u64,
    active: u64,
    parked: u64,
    gap_min: u64,
    gap_max: u64,
}

/// WC-N — forget a slot's accumulators when its row is freed, so a recycled window id cannot inherit
/// the previous tenant's gap history and report a park that never happened. The counts themselves are
/// left to the next rollup to drain (they are real presents that really happened, and the line's
/// `live=no` is what says the window is gone); only `last_ms` — the one field that spans rollups — is
/// cleared, because it is the only one whose meaning depends on the row still being the same window.
#[cfg(feature = "witness")]
fn wcn_forget(id: WinId) {
    if let Some(s) = wcn_slot(id) {
        s.last_ms
            .store(0, core::sync::atomic::Ordering::Relaxed);
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

/// FOCUS-HL — the same three chrome colours, brightened, for the window that currently holds focus.
/// The border carries most of the signal (it frames the whole window, so it reads at a glance from
/// across the bench) and the title strip lifts with it so the two do not disagree.
///
/// Chosen to stay in the same flat, un-host-like family as the resting colours — this marks focus, it
/// does not imitate anyone's title bar — while clearing a wide enough gap to be unambiguous on the
/// bench panel rather than only on a screenshot.
const CHROME_BORDER_FOCUS: u32 = 0x008C_8CB4;
const CHROME_TITLE_BG_FOCUS: u32 = 0x003A_3A5A;

/// CLOSE-BOX (P79) — the close box's chrome colours. Red-tinted deliberately: the box is the ONE
/// piece of chrome a click ACTS on (see [`close_box`]), and it must read as such from across the
/// bench, so it does not share the title strip's blue family. The focused window's box brightens
/// with the rest of its chrome so the two never disagree about who has focus; the glyph reuses
/// [`CHROME_TITLE_FG`] so the X is exactly as legible as the title beside it.
const CHROME_CLOSE_BG: u32 = 0x0046_262C;
const CHROME_CLOSE_BG_FOCUS: u32 = 0x00A0_3C46;

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

/// FBCON-DMG — the part of `r`'s outer box a banded present will actually repaint.
///
/// `band` is in SOURCE rows; the conversion to panel rows is `r.y + sy * r.scale`, the same mapping
/// [`paint_window`] blits the content through, so the rectangle returned here is exactly the one the
/// pass writes. `None` — every window that has not taken a [`present_rows`] — returns [`outer_box`]
/// unchanged, which is why every existing caller and every existing window is unaffected.
///
/// The result is intersected with the outer box, so a band cannot extend a window's damage past its
/// own chrome however wrong the caller's rows are.
fn damaged_box(r: &Window, band: Option<(usize, usize)>) -> (usize, usize, usize, usize) {
    let (bx, by, bw, bh) = outer_box(r);
    let Some((sy0, sy1)) = band else {
        return (bx, by, bw, bh);
    };
    if sy1 <= sy0 {
        return (bx, by, bw, bh);
    }
    // F5 discipline: saturating throughout, so a nonsense band degrades to a large box the panel
    // clip bounds rather than to a small one that under-damages.
    let y0 = r.y.saturating_add(sy0.saturating_mul(r.scale)).max(by);
    let y1 = r
        .y
        .saturating_add(sy1.saturating_mul(r.scale))
        .min(by.saturating_add(bh));
    if y1 <= y0 {
        return (bx, by, bw, 0);
    }
    (bx, y0, bw, y1 - y0)
}

/// CLOSE-BOX (P79, bench: "put a close button in the upper right of the windows to exit") — the
/// close box's panel rect as `(x, y, side)`, or `None` for a row that has no close box.
///
/// ### Geometry — one function, two consumers
/// A [`TITLE_H`]-sided square at the RIGHT end of the title strip, flush against the inner edge of
/// the border: `x = bx + bw - BORDER - side`, `y = by + BORDER`. Derived from the outer box so the
/// drawn box and the hit-tested box are the same rect BY CONSTRUCTION — [`paint_window`] draws this
/// rect and [`close_box_hit`] tests it, and there is deliberately no second copy of the arithmetic
/// for the two to disagree over.
///
/// ### Who gets one
/// * **Compat rows: no.** The `present_surface` shim has no chrome at all — there is no strip to
///   put a box in, and the full-screen app's own click-to-exit already owns its clicks.
/// * **Owner-0 rows: no.** Kernel furniture (the CLICK-SHELL distinction: shell/desktop rows carry
///   `owner_asid == 0`, which is also why [`hit_test`] never names them) is not an app the operator
///   can exit; a close box on the console would be an invitation to kill the shell.
/// * **A strip too narrow to hold the box plus one title glyph: no.** Degenerate geometry declines
///   the control rather than drawing an unhittable sliver.
fn close_box(r: &Window) -> Option<(usize, usize, usize)> {
    if r.compat || r.owner_asid == 0 {
        return None;
    }
    let (bx, by, bw, _bh) = outer_box(r);
    let side = TITLE_H;
    if bw < 2 * BORDER + 2 * side {
        return None;
    }
    Some((bx + bw - BORDER - side, by + BORDER, side))
}

/// CLOSE-BOX — does panel point `(x, y)` land in window `id`'s close box? The router's second
/// question, asked only AFTER [`hit_test`] has named `id` as the owner of the point — so this is a
/// rect test against one row, not a scan, and the z-order question is already settled by the time
/// it runs. `false` for a dead id and for every row [`close_box`] declines.
pub fn close_box_hit(id: WinId, x: i32, y: i32) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let t = table();
    match row(&t, id).and_then(close_box) {
        Some((cx, cy, s)) => px >= cx && px < cx + s && py >= cy && py < cy + s,
        None => false,
    }
}

fn boxes_overlap(a: (usize, usize, usize, usize), b: (usize, usize, usize, usize)) -> bool {
    a.0 < b.0.saturating_add(b.2)
        && b.0 < a.0.saturating_add(a.2)
        && a.1 < b.1.saturating_add(b.3)
        && b.1 < a.1.saturating_add(a.3)
}

// ---- WC-L: the deferred-erase queue ------------------------------------------------------------

/// Deferred erase boxes held at once. [`MAX_WINDOWS`] because the deferred set is bounded by the
/// same thing every other erase set in this module is: the boxes a re-tile or a teardown can vacate
/// in one operation is at most the live window count. Overflow is not dropped — it coalesces (see
/// [`defer_erase`]) — so this is a fidelity bound, not a correctness one.
const MAX_DEFER: usize = MAX_WINDOWS;

/// Why a fill could not stage. These mirror `wcg`'s `DECL_*`, and the mirror is not duplication for
/// its own sake: `wcg` is compiled only under `witness`, while WC-L's decision about what to
/// do with a fill that cannot stage — defer it or drop it — is a CORRECTNESS decision that every
/// build makes, witness or not. The reason therefore has to exist outside the witness. The `const`
/// assertions below tie the two vocabularies together at compile time on the builds that have both,
/// so a renumbering in `wcg` cannot silently mislabel these lines.
const DEFER_GEOM: u32 = 1;
const DEFER_CAP: u32 = 2;
const DEFER_LOCK: u32 = 3;
const DEFER_ALLOC: u32 = 4;
#[cfg(feature = "witness")]
const _: () = assert!(
    DEFER_GEOM == super::wcg::DECL_GEOM
        && DEFER_CAP == super::wcg::DECL_CAP
        && DEFER_LOCK == super::wcg::DECL_LOCK
        && DEFER_ALLOC == super::wcg::DECL_ALLOC
);

/// WC-L — panel boxes owed a desktop-colour repaint that could not be staged when they were vacated.
///
/// Drained by [`drain_deferred`] at the head of every composite pass.
///
/// **Lock discipline.** This is a LEAF: it is acquired only by [`defer_erase`] and
/// [`drain_deferred`], it is held for an array copy and nothing else, and no other lock of this
/// module — `TABLE`, `STAGE`, `WRITER`, the cursor module's `SPRITE` — is ever taken while it is
/// held. That is what lets it use a blocking `lock()` where [`STAGE`] uses `try_lock`: a deferral
/// has nowhere left to fall back to, so losing a box here would lose the repaint outright, and a
/// leaf held for a bounded array write cannot deadlock. The converse order is the one that matters
/// and it is the one the code takes: `STAGE` (released) → `DEFER`, and `DEFER` (released) →
/// `STAGE`/`TABLE` in the drain. No new inversion is introduced because `DEFER` is never held across
/// any acquisition at all.
static DEFER: Mutex<([(usize, usize, usize, usize); MAX_DEFER], usize)> =
    Mutex::new(([(0, 0, 0, 0); MAX_DEFER], 0));

/// Cheap "is anything owed" flag, mirroring `DEFER.1` and maintained under the lock. The drain runs
/// at the head of every composite — which is every window present, on every core — so the common
/// case must cost one relaxed load and no lock traffic at all.
static DEFER_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// WC-L — one-shot latch for the deferral fixture in [`stage_fill`]. Witness builds only.
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
static DEFER_FIXTURE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// WC-L — queue a clipped panel box for a desktop repaint the staged path could not deliver now.
///
/// `requeued` distinguishes a box deferred at its original erase from one the drain retried and
/// could still not stage; both are honest, but a boot doing the second is telling the operator the
/// staging lock is contended for longer than a composite interval.
///
/// On a full queue the box is UNIONED into an existing entry rather than dropped. The union is sound
/// for the same reason WC-J's `reclaim` is: the drain re-damages every window the painted box
/// intersects, so enlarging the box costs repaint work and can never leave a window with a bite
/// taken out of it. Dropping, by contrast, would leave a dead window's last frame on the panel for
/// the rest of the boot — the P61 ghost.
///
/// The victim is the entry whose union with the new box adds the LEAST AREA, not slot 0. Always
/// unioning into slot 0 is what the first cut did, and it grows without bound in the obvious way: a
/// full queue plus a stream of scattered boxes drags slot 0's corners outward until it is most of
/// the panel, and every subsequent drain then repaints most of the panel and re-damages every window
/// on it. Choosing by added area keeps a union local — a box adjacent to an existing entry costs
/// almost nothing, and a box far from all of them lands on whichever entry it hurts least. The scan
/// is `MAX_DEFER` (8) comparisons on a path that only runs when the queue is already full.
fn defer_erase(x: usize, y: usize, w: usize, h: usize, reason: u32, requeued: bool) {
    if w == 0 || h == 0 {
        return;
    }
    {
        let mut q = DEFER.lock();
        let (boxes, n) = &mut *q;
        if *n < MAX_DEFER {
            boxes[*n] = (x, y, w, h);
            *n += 1;
        } else {
            // Pick the entry whose union with this box adds the least area. `usize` throughout: the
            // union of two panel-clipped boxes is itself panel-bounded, so no product can overflow.
            let union = |b: (usize, usize, usize, usize)| {
                let x0 = b.0.min(x);
                let y0 = b.1.min(y);
                let x1 = (b.0 + b.2).max(x + w);
                let y1 = (b.1 + b.3).max(y + h);
                (x0, y0, x1 - x0, y1 - y0)
            };
            let mut best = 0usize;
            let mut best_growth = usize::MAX;
            for (i, b) in boxes.iter().enumerate() {
                let u = union(*b);
                let growth = (u.2 * u.3) - (b.2 * b.3);
                if growth < best_growth {
                    best_growth = growth;
                    best = i;
                }
            }
            boxes[best] = union(boxes[best]);
            #[cfg(feature = "witness")]
            super::wcg::erase_coalesce();
        }
        DEFER_N.store(*n, core::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(feature = "witness")]
    super::wcg::erase_defer(w, h, reason, requeued);
    #[cfg(not(feature = "witness"))]
    let _ = (reason, requeued);
}

/// WC-L — erase the boxes owed from earlier passes, through the staged path, and re-damage what the
/// paint reached.
///
/// Returns whether the caller owes the SPRITE a repaint — which is `true` whenever this function
/// took the sprite off the panel, NOT whenever it painted a box. The distinction is the whole of
/// MUST-FIX 1 from WC-L's lens review, and getting it wrong is worse than the bug the arc fixes: if
/// the drain undraws and then every box re-defers, returning "nothing painted" leaves
/// `composite_inner` with `disturbed = false`, the tail is `Untouched`, and the sprite is removed
/// and never restored — every pass, for as long as contention lasts, which is exactly the situation
/// this arc exists for. A cursor that vanishes under load is not a lesser failure than a torn erase.
///
/// The undraw is nevertheless LAZY, because doing it unconditionally on every drain would re-create
/// WC-I's spotty sprite: an undraw/repaint per composite, on every core, is what WC-I removed. The
/// `STAGE` probe below decides. If the staging lock is unavailable — the dominant contention case,
/// and the one the P64 capture caught — nothing can be painted this pass, so the queue is restored
/// untouched and the sprite is never disturbed at all.
///
/// Called from [`composite_inner`] BEFORE the dirty-set snapshot, so the windows this re-damages are
/// repainted by the very pass that did the erasing rather than by some later one. It runs outside
/// the F4 `BlitGuard` window on purpose: it acquires the cursor module's `SPRITE` (via `undraw`) and
/// `TABLE`, and the drain barrier's termination argument requires that neither be in its wait set.
///
/// The queue is emptied into a local snapshot before any staging is attempted, so `DEFER` is not
/// held across `STAGE` or `TABLE`; a box that still cannot stage goes back on through
/// [`defer_erase`] after the lock has been released and retaken.
fn drain_deferred(fb: &super::FrameBuffer) -> bool {
    if DEFER_N.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        return false;
    }
    // Probe the staging lock BEFORE emptying the queue or touching the sprite. A `None` here means
    // every box in the queue is about to re-defer, so the cheapest and least disruptive thing this
    // pass can do is nothing at all: leave the queue as it is (no requeue churn, no `redefers`
    // inflation from a pass that never really tried) and leave the sprite on the panel.
    //
    // The probe is advisory, not a reservation — the guard is dropped immediately and `stage_fill`
    // takes the lock itself. Another core can win it in between, in which case the boxes re-defer
    // normally and the only cost is one wasted pass with the sprite bracket taken. That race is
    // benign in the direction that matters: it can cost a repaint, never skip one.
    if STAGE.try_lock().is_none() {
        return false;
    }
    let (boxes, n) = {
        let mut q = DEFER.lock();
        let snap = *q;
        q.1 = 0;
        DEFER_N.store(0, core::sync::atomic::Ordering::Relaxed);
        snap
    };
    if n == 0 {
        return false;
    }
    // FLICKER-2 — the sprite bracket is owed only where a fill can actually REACH the sprite.
    //
    // The previous shape took the FULL sprite down before the first fill byte landed, whatever the
    // queue held — so a deferred erase anywhere on the panel cost a whole-sprite restore→repaint over
    // whatever window the pointer was resting on, once per drain. Every such restore is also an
    // opportunity for the colour guard's documented residual (a stale `saved` restored over a live
    // window's fresh frame), which is exactly the "window under the cursor flickers occasionally"
    // symptom: the fills were typically nowhere near the pointer, and the sprite paid anyway.
    //
    // The undraw's one justification — hand a pixel back before a painter in THIS operation
    // overwrites it — applies only to sprite pixels inside the boxes about to be filled. So the test
    // is the same one `composite_inner`'s bracket makes: if no queued box meets the sprite's box, the
    // sprite is left entirely alone (and `false` is returned — the tail owes it nothing); if one
    // does, the handback is MASKED to the queued boxes via `undraw_within_nosession`, whose
    // generation bump is precisely what protects a concurrent core's open overlay session (see
    // CURSOR-5's interleave analysis on that function). `sprite_box` is a snapshot and can be one
    // pointer report stale; that degrades to an unnecessary masked handback or to the pre-FLICKER-2
    // behaviour for one pass, never to a missed one — a sprite that moved INTO a fill box mid-drain
    // is re-established by the mover's own `repaint`, exactly as it always was under the full undraw
    // (which had the identical window: the snapshot was simply taken inside `undraw` itself).
    //
    // CURSOR-5 — the undraw must still never land inside an open overlay session on THIS core (the
    // P64 flash). The ordering keeps that structurally impossible; the probe below still catches a
    // future reorder, on the arm that actually disturbs the sprite.
    #[cfg(feature = "witness")]
    F2W_DRAINS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let sprite_hit = super::cursor::sprite_box()
        .map(|sb| boxes[..n].iter().any(|&b| b.2 != 0 && b.3 != 0 && boxes_overlap(sb, b)))
        .unwrap_or(false);
    if sprite_hit {
        #[cfg(feature = "witness")]
        {
            super::cursor::note_drain_undraw();
            F2W_DRAIN_MASKED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        super::cursor::undraw_within_nosession(&boxes[..n]);
    } else {
        #[cfg(feature = "witness")]
        F2W_DRAIN_SKIPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    let info = fb.info();
    let mut painted = false;
    for &(x, y, w, h) in boxes[..n].iter() {
        // Re-clip: a coalesced union is a synthesised box, and the panel geometry it was clipped
        // against belonged to the original erase.
        if w == 0 || h == 0 || x >= info.width || y >= info.height {
            continue;
        }
        let (w, h) = (w.min(info.width - x), h.min(info.height - y));
        if !stage_fill(fb, x, y, w, h, DESKTOP_BG, true) {
            // `stage_fill` re-queued it; leave the panel alone and try again next pass.
            continue;
        }
        let y0 = y.min(info.height);
        let y1 = (y + h).min(info.height);
        if y1 > y0 {
            // COMPOSITE-2 — the fill's own columns, not full-width scanlines (see `draw_window`).
            fb.flush_rect(x, y0, w, y1 - y0);
        }
        damage_intersecting(x, y, w, h);
        painted = true;
    }
    if painted {
        // WC-J's third step: only the desktop can put its own content (console text, status strip)
        // back under a departed window; the erase above can only paint `DESKTOP_BG`.
        super::screen::request_full_present();
    }
    // FLICKER-2 — the caller owes the sprite a repaint exactly when this drain disturbed it, which
    // is the `sprite_hit` arm and NOT `painted`: a masked handback whose boxes all re-deferred has
    // still taken pixels down (MUST-FIX 1's argument, unchanged), while a drain whose fills never met
    // the sprite has provably touched none of its pixels and must not cost a restore→repaint cycle
    // over the window under the pointer.
    sprite_hit
}

// ---- WC-H: the window back-layer ---------------------------------------------------------------

/// WC-H — hard ceiling on the staging buffer, in bytes. 4 MiB covers a 1024x1024 box at 4
/// bytes/pixel — comfortably past the bench's 128x128@4x window (~514x526, 1.08 MiB) and far short
/// of a panel-sized allocation.
///
/// ### WC-M — this is a ceiling on the BUFFER, no longer a ceiling on the PRESENT
///
/// WC-H made an over-cap box decline to the DIRECT path — the pre-WC-H, tearing regime — which was
/// defensible while the only windows were small. It stops being defensible the moment the console
/// becomes a window: the bench panel is 1920x1200 ARGB, ~8.8 MiB for one full-panel box, so the
/// largest and most visible present in the system was the one guaranteed to tear.
///
/// [`stage_window`] now stages such a box in row-bands that each fit under this cap, so the cap
/// bounds the compositor's memory *and nothing else*. The only geometry that still declines on it is
/// one whose SINGLE ROW does not fit, which is unreachable on any panel this kernel can address (4
/// MiB is a 1 048 576-pixel row). See [`stage_window`] for the banding and for the visibility window
/// it costs.
const MAX_STAGE_BYTES: usize = 4 * 1024 * 1024;

/// WC-H — the window back-layer's memory: one buffer, allocated on first use at the size the largest
/// window needs and reused by every composite thereafter. WC-M: "what the window needs" is capped at
/// [`MAX_STAGE_BYTES`], because a box past the cap is staged one band at a time through this same
/// buffer rather than declining — the buffer never grows past the cap, whatever the panel's size.
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
/// scan-out read-back covers the fallback as well as the staged present. `witness`-gated and nothing
/// else — latching a bool and taking the direct path is arch-neutral; no flashable medium carries it.
#[cfg(feature = "witness")]
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
/// while a foreground full-screen user program owns the panel the render task is parked, so the
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
///
/// ### FBCON-DMG — `band`, and what it may and may not skip
///
/// `band` is the SOURCE-ROW range this present was declared over (`None` = the whole window, which is
/// every window that has not taken a [`present_rows`] and therefore every pre-FBCON-DMG caller). It
/// narrows the STAGED path's row range and nothing else: the box geometry handed to [`paint_window`]
/// is still the whole box, so the chrome keeps its true position across the seam exactly as a
/// WC-M-banded present does — this reuses that machinery rather than adding a second kind of band.
///
/// The DIRECT fallback ignores it and paints the whole box. That is deliberate and it is the
/// fail-safe direction: the fallback is the path taken when the back layer is unavailable, it is
/// already the expensive regime, and a whole-box repaint there can only over-paint, never leave a
/// stale glyph. The cache clean at the tail follows whichever extent actually ran.
#[allow(clippy::too_many_arguments)]
fn draw_window(
    fb: &super::FrameBuffer,
    r: &Window,
    focused: bool,
    cur: Option<super::cursor::Plan>,
    may_overlay: bool,
    bracketed: bool,
    band: Option<(usize, usize)>,
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
    let offer = if may_overlay { cur } else { None };
    // FBCON-DMG — the band, as BOX-RELATIVE rows. `damaged_box` does the source→panel conversion and
    // the clip to the box; subtracting `by` puts it in the coordinate `stage_window`'s loop counts in.
    // An empty result means this pass's damage falls entirely outside the panel-clipped box, which is
    // nothing to draw rather than everything.
    let (dy0, dy1) = match band {
        None => (0, bh),
        Some(_) => {
            let (_, dby, _, dbh) = damaged_box(r, band);
            let y0 = dby.max(by).saturating_sub(by);
            let y1 = (dby.saturating_add(dbh)).min(by + bh).saturating_sub(by);
            if y1 <= y0 {
                return false;
            }
            (y0, y1)
        }
    };
    let staged =
        stage_window(fb, r, bx, by, bw, bh, dy0, dy1, pw, ph, focused, offer, &mut overlaid);
    if !staged {
        paint_window(fb, r, 0, 0, bx, by, bw, bh, pw, ph, focused, false);
    }
    // CURSOR-4 — this window has just painted its clipped outer box. If it did NOT compose the
    // sprite into that box (direct path, instrument exclusion, compat row, contended plan lock, or
    // an unreadable layer) then every sprite pixel inside it is the window's now, and the coverage a
    // LOWER window may have claimed for those pixels is stale — its layer save describes content
    // these pixels no longer hold. Clearing here, in back-to-front draw order, makes the topmost
    // painter of each pixel the one whose verdict the tail acts on.
    if let (Some(p), false) = (cur, overlaid) {
        // CURSOR-6 — a clear that could not be applied is not a shrug. `overlay_uncover` now reports
        // it, and the session is marked untrustworthy so the tail takes the whole-sprite refresh
        // instead of installing coverage this window has just painted over. See
        // `cursor::UNCOVER_LOST` for the interleave and for both symptoms it produces.
        if !super::cursor::overlay_uncover(&p, bx, by, bw, bh) {
            super::cursor::note_uncover_lost();
        }
    } else if cur.is_none() {
        // CURSOR-15 — the SESSIONLESS pass owes a concurrent session owner the same duty, cross-
        // pass. This pass composes through the sprite (`defer_nosession`) and hands nothing back,
        // so the generation bump that used to retire the owner's session when we disturbed one of
        // its pixels is gone; if this blit just overwrote a pixel the owner's layer COVERED, its
        // tail would install a layer save the panel no longer holds and claim a pixel that is now
        // ours. Clearing the coverage inside our painted box makes its tail settle those pixels
        // against the finished front instead — CURSOR-4's back-to-front verdict rule, applied
        // across passes. Gated on the sprite's advisory box: two relaxed atomics, and a one-report-
        // stale answer degrades to a needless clear (bits the owner would have settled anyway) or
        // to no clear over a box no live sprite met — never to a missed hazard, since a sprite that
        // ARRIVED after the publish did so via a draw whose generation bump retires the session on
        // its own. A contended clear invalidates the session wholesale, exactly as above.
        if let Some(sb) = super::cursor::live_box_relaxed() {
            if boxes_overlap(sb, (bx, by, bw, bh))
                && !super::cursor::overlay_uncover_any(bx, by, bw, bh)
            {
                super::cursor::note_uncover_lost();
            }
        }
    }
    // CURSOR-6 — THE DIRECT MEASUREMENT, and CURSOR-7 — THE REPAIR IT ARMS. This window's pixels are
    // on the panel now; did they land on a LIVE arrow, and was the arrow protected when they did?
    //
    // `bracketed` is the pass's `disturbed`: it took the sprite down — fully, or masked to its paint
    // set — and its tail is therefore `Adopt` or `Repaint`, both of which put the arrow back. Those
    // pixels are owed a repaint by construction: healthy, and counted as the denominator.
    //
    // CURSOR-6 asked the narrower question `cur.is_some()`, and so charged the SESSIONLESS masked
    // undraw — the `overlay_open` refusal that IS the VUGPAR steady state — as an unbracketed
    // overwrite, although that path hands the pixels back (`undraw_within_nosession`) and takes a
    // `Repaint` tail. Part of P67v2's `present_over=9/s` was that misclassification. The rest was real,
    // and is what follows.
    //
    // `!bracketed` with a live sprite over this box is the real defect: the pass took no plan and no
    // bracket — the sprite was not on the panel when `sprite_plan()` was called, or no window met it
    // then, and it has arrived or moved since — so this blit is writing window content over an arrow
    // nothing handed back, no path tells the sprite module, and the module's own bookkeeping stays
    // perfectly self-consistent throughout. That is why every CURSOR-5 counter can read `COHERENT`
    // while the panel is spotty. Before CURSOR-7 the pass's tail was then `Untouched` — `ensure_drawn`,
    // a no-op while `sp.drawn` — and NOTHING repainted the arrow until the pointer moved again.
    //
    // Nothing here can repair it: this runs inside the `BlitGuard` window, where taking `SPRITE` would
    // put a blocking wait into F4's drain barrier (§WEDGE-1). So the call ARMS a relaxed flag, and
    // `composite` consumes it after the guard is gone and turns the pass's tail into a whole-sprite
    // repaint. See `cursor::PRESENT_DIRTY` for why the tail is the only admissible place.
    //
    // No longer `#[cfg(feature = "witness")]`: the arming is MECHANISM now, not instrumentation, and a
    // production build that skipped it would keep the defect. `live_box_relaxed` is two relaxed atomics
    // and no lock, which is exactly what makes it admissible here. The box may be one pointer report
    // stale and is deliberately over-count-biased, so its worst outcome is a spurious tail repaint.
    if let Some(sb) = super::cursor::live_box_relaxed() {
        if boxes_overlap(sb, (bx, by, bw, bh)) {
            super::cursor::note_present_over_sprite(bracketed);
        }
    }

    // Clean the touched pixels for the non-coherent scan-out — one `DC CVAC` sweep per window with
    // one `DSB`, over the BOX's own columns. COMPOSITE-2 narrowed this from full-width scanlines
    // (`flush_range` over `[y0, y1)`): every write this pass made — chrome, content, staged rows,
    // `compose_into`'s sprite pixels — lands inside the clipped outer box by construction, so the
    // box (rounded out to cache lines by the arch sweep) is still a superset of the dirty bytes,
    // and the margins of a 514-wide box on a 1920-wide panel stop being cleaned for nothing.
    // No-op on coherent targets. Unchanged by WC-H: staged or direct, the same panel pixels were
    // written by the time we get here.
    //
    // FBCON-DMG: the span follows the extent that actually ran — the band on a staged banded present,
    // the whole box on the direct fallback (which ignores the band) and on every unbanded present.
    // Cleaning rows nothing wrote would be harmless; cleaning fewer than were written would not, so
    // this is derived from `staged` rather than from `band`.
    let (fy0, fy1) = if staged { (by + dy0, by + dy1) } else { (by, by + bh) };
    let y0 = fy0.min(info.height);
    let y1 = fy1.min(info.height);
    // COMPOSITE-2 — the cache term and the pass's denominators. `bytes` is what reached the panel,
    // `dmg_px` its area; both are the numbers `[comp2]`'s per-byte claims divide by, and `box_px` is
    // the whole-box area they would have been charged as before this arc.
    //
    // ### FBCON-DMG — the extent is `y1 - y0`, and it is deliberately the FLUSH's own expression
    //
    // While this block was `target_arch = "aarch64"` it charged `bw * bh`, the whole box, on the
    // argument (correct at the time, and load-bearing) that `band` is provably `None` on that arch —
    // the only producer of a band is the routed console window, `all(x86_64, feature = "wc")`, and
    // `wm::present_rows` has no other caller that compiles for aarch64. Whole-box WAS the extent that
    // ran, so the numbers were honest and narrowing them would have been unobservable.
    //
    // That argument does not survive the widening. On x86 the console bands, so the ledger has to
    // charge the rows the pass actually wrote, and the rows it actually wrote are exactly the ones
    // the cache clean two lines down is about to sweep: `[y0, y1)`, derived above from `staged`
    // rather than from `band` precisely because the direct fallback IGNORES the band and repaints
    // whole. Reusing that expression rather than re-deriving it from `dy0`/`dy1` is the point — the
    // ledger and the flush cannot drift apart into disagreeing about what this call painted, because
    // there is only one derivation and both read it.
    //
    // ### Why aarch64's numbers cannot move
    //
    // `band == None` forces `(dy0, dy1) == (0, bh)` at the top of this function. Then BOTH arms of
    // the `staged` conditional above produce `(fy0, fy1) == (by, by + bh)` — the staged arm because
    // `by + dy0 == by` and `by + dy1 == by + bh`, the direct arm literally. F6 has already clipped
    // `bh` to `ph - by`, so `by < ph` and `by + bh <= ph == info.height`, and the two `.min` clamps
    // are no-ops: `y0 == by`, `y1 == by + bh`, hence `y1 - y0 == bh`. The new `bw * (y1 - y0)` is
    // therefore not merely equivalent to the old `bw * bh` on aarch64 — it evaluates the same
    // integer, on every pass, and `box_px` charges that same integer a second time. Nothing on that
    // arch reads differently by one.
    #[cfg(feature = "witness")]
    let c2_f0 = {
        use core::sync::atomic::Ordering::Relaxed;
        let wrote = bw * (y1 - y0);
        C2_BYTES.fetch_add((wrote * info.bytes_per_pixel) as u64, Relaxed);
        C2_DMG_PX.fetch_add(wrote as u64, Relaxed);
        C2_BOX_PX.fetch_add((bw * bh) as u64, Relaxed);
        crate::arch::now_cycles()
    };
    if y1 > y0 {
        fb.flush_rect(bx, y0, bw, y1 - y0);
    }
    // COMPOSITE-2 — the cache term is band-correct without arithmetic of its own: it brackets a
    // `flush_rect` over `[y0, y1)`, the same rows charged above, so a banded present pays a banded
    // sweep and the interval measures it.
    //
    // On x86 that call is one `SFENCE` and no sweep at all — `flush_rect` takes the
    // `not(target_arch = "aarch64")` arm, where the drain is range-independent — so `cache_us` there
    // reads 0 once the per-pass division and the cycles→us truncation have run. That zero is a
    // measurement, not a hole: it says the scan-out is coherent and the extent cost nothing to
    // publish, and it is why `blit_us` and `loop_us` coincide on that arch.
    #[cfg(feature = "witness")]
    C2_CACHE_CYC.fetch_add(
        crate::arch::now_cycles().saturating_sub(c2_f0),
        core::sync::atomic::Ordering::Relaxed,
    );
    // CURSOR-3: the cache clean above covers the sprite's pixels for free — they are inside these
    // rows, which is the whole point of composing them into the layer rather than poking them into
    // the front afterwards. Nothing extra is written to the front buffer on this path.
    overlaid
}

/// WC-M — [`super::FrameBuffer::fill_rect`] with a vertical origin that may lie ABOVE the
/// destination's row 0. The rows above are dropped and the remainder is filled at its true position;
/// everything else clips inside `fill_rect` exactly as before. For `y >= 0` this IS
/// `fill_rect(x, y as usize, w, h, color)` — the direct path and every single-band stage take that
/// branch, so neither sees any change at all.
fn fill_rect_v(dst: &super::FrameBuffer, x: usize, y: isize, w: usize, h: usize, color: u32) {
    if y >= 0 {
        dst.fill_rect(x, y as usize, w, h, color);
        return;
    }
    let skip = (-y) as usize;
    if skip >= h {
        return;
    }
    dst.fill_rect(x, 0, w, h - skip, color);
}

/// WC-H — paint the window's chrome and upscaled content into `dst`, whose origin sits at panel
/// coordinate `(ox, oy)`. The two callers differ only in that origin: the direct path passes the
/// front framebuffer with `(0, 0)`, the staged path passes the back layer with the outer box's
/// top-left. Every clip bound is still derived from the PANEL (`pw`/`ph`), so both paths draw the
/// identical pixel set — the back layer is exactly `bw x bh` of it, addressed from a different zero.
///
/// ### WC-M — `oy` may now sit BELOW the box's top edge
///
/// A chunked stage paints the same window once per row-band, with `dst` covering only that band and
/// `oy` at the band's first panel row. Every band after the first therefore has the box's top border,
/// its title strip and its first source rows ABOVE its own row 0, so the destination-local vertical
/// origin is SIGNED. `bx`/`by`/`bw`/`bh` still describe the whole box (the chrome geometry must not
/// move between bands) and `dst`'s own height is what bounds the band.
///
/// The horizontal axis is untouched: bands split on full rows only, so `ox` never exceeds `bx`.
///
/// When `oy <= by` — the direct path, and any stage that fits the cap in one band — every value
/// below is non-negative and every call is the pre-WC-M call with the pre-WC-M arguments.
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
    let lbx = bx.saturating_sub(ox);
    // WC-M — signed: a band below the first has the box's top edge above its own row 0.
    let lby = by as isize - oy as isize;
    // COMPOSITE-2 — the content extent, hoisted above the chrome so the border fill can subtract
    // it. Identical derivation to the block below the chrome (which keeps its own copy for the
    // early-return path's clarity): how many source cols/rows land on the panel AND inside the
    // mapped slot, hence exactly which destination pixels the content loop is GUARANTEED to write.
    let (c2_cols, c2_rows) = if r.x < pw && r.y < ph {
        (
            (pw - r.x).div_ceil(r.scale).min(r.w).min(r.stride / 4),
            (ph - r.y).div_ceil(r.scale).min(r.h).min(r.surf_len / r.stride),
        )
    } else {
        (0, 0)
    };
    if !r.compat {
        // Frame: fill the outer box in the border colour, then lay the title strip and the content
        // over it. The only pixels that survive are the 1-px frame itself.
        //
        // WC-H relies on this fill being DENSE over the whole clipped box: it is what guarantees the
        // back layer carries no residue from the window it staged last. Every pixel the present
        // copies was written by this pass.
        //
        // COMPOSITE-2 — dense, but no longer DOUBLE. The old fill painted the whole box and the
        // content loop then rewrote ~97% of it (a 514x526 bench box is 270k px of fill under 262k px
        // of content), so the biggest single population of the compose was pixels written twice.
        // The fill is now the box MINUS the content extent — four rects: the title band above it,
        // the strip below it, and the side borders beside it. The invariant is preserved exactly,
        // because the subtracted region is only what the content loop provably writes this same
        // call: both painters clip with the same bounds at the same coordinates (destination
        // width/height/length), so any subtracted pixel the clip would have denied the content is a
        // pixel it denies the fill too. A window whose content is short (`cols == 0`/`rows == 0` —
        // nothing visible or nothing mapped) keeps the full dense fill.
        // FOCUS-HL: the ONLY difference the focused window's chrome carries is these two colours. The
        // geometry is identical either way, so focus never moves a pixel — it just repaints the frame
        // and strip that were already going to be painted, at no extra cost per present.
        let (border, title_bg) = if focused {
            (CHROME_BORDER_FOCUS, CHROME_TITLE_BG_FOCUS)
        } else {
            (CHROME_BORDER, CHROME_TITLE_BG)
        };
        if c2_cols > 0 && c2_rows > 0 {
            let cx0 = r.x.saturating_sub(ox);
            let cy0 = r.y as isize - oy as isize;
            let cx1 = cx0 + c2_cols * r.scale;
            let cy1 = cy0 + (c2_rows * r.scale) as isize;
            let box_x1 = lbx + bw;
            let box_y1 = lby + bh as isize;
            // Above the content (border + title band) and below it (bottom border + any clip gap).
            fill_rect_v(dst, lbx, lby, bw, (cy0 - lby).max(0) as usize, border);
            if box_y1 > cy1 {
                fill_rect_v(dst, lbx, cy1, bw, (box_y1 - cy1) as usize, border);
            }
            // Beside it, only over the content's own rows.
            let mid_h = (cy1 - cy0).max(0) as usize;
            fill_rect_v(dst, lbx, cy0, cx0.saturating_sub(lbx), mid_h, border);
            if box_x1 > cx1 {
                fill_rect_v(dst, cx1, cy0, box_x1 - cx1, mid_h, border);
            }
        } else {
            fill_rect_v(dst, lbx, lby, bw, bh, border);
        }
        fill_rect_v(
            dst,
            lbx + BORDER,
            lby + BORDER as isize,
            bw.saturating_sub(2 * BORDER),
            TITLE_H,
            title_bg,
        );
        // CLOSE-BOX — the close control, over the strip's right end. The rect comes from
        // `close_box` (the SAME function the router hit-tests, so what is drawn is what is
        // clickable), converted to this destination's origin exactly as the strip above was; the
        // title's width budget below excludes it so a long title truncates BESIDE the box rather
        // than running under it. Signed-`y` discipline matches `fill_rect_v`/`draw_title`: a
        // chunked stage's later bands see the box above their row 0 and skip those lines.
        let close_w = match close_box(r) {
            Some((cbx, cby, s)) => {
                let clx = cbx.saturating_sub(ox);
                let cly = cby as isize - oy as isize;
                let close_bg = if focused { CHROME_CLOSE_BG_FOCUS } else { CHROME_CLOSE_BG };
                fill_rect_v(dst, clx, cly, s, s, close_bg);
                // The X glyph: two 2-px diagonals, inset so the strokes never touch the box edge.
                let g = 3;
                let n = s.saturating_sub(2 * g);
                for i in 0..n {
                    let dy = cly + (g + i) as isize;
                    if dy < 0 {
                        continue;
                    }
                    dst.put_pixel(clx + g + i, dy as usize, CHROME_TITLE_FG);
                    dst.put_pixel(clx + g + i + 1, dy as usize, CHROME_TITLE_FG);
                    dst.put_pixel(clx + g + (n - 1 - i), dy as usize, CHROME_TITLE_FG);
                    dst.put_pixel(clx + g + (n - 1 - i) + 1, dy as usize, CHROME_TITLE_FG);
                }
                s + 2
            }
            None => 0,
        };
        draw_title(
            dst,
            r,
            lbx + BORDER + 2,
            lby + BORDER as isize + 2,
            bw.saturating_sub(2 * BORDER + 4 + close_w),
        );
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

    let cx = r.x.saturating_sub(ox);
    // WC-M — signed, for the same reason `lby` is: the content's first rows are above a later band.
    let cy = r.y as isize - oy as isize;
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
    // COMPOSITE-2 — the content upscale's WORD path. The measured wall was here: at 1920x1200 the
    // per-pixel `put_pixel` compose was ~11 ms of a ~15 ms pass ([comp2] blit_us=12952 against
    // present_us=896 for the same box), because every destination pixel paid a function call, four
    // bounds checks, a format match and three byte stores. The hoist is exactly `encode4`'s: decide
    // the swizzle ONCE per compose, encode each source pixel once, and write its `scale`-wide run
    // as one clipped span of 4-byte words (64-bit pairs inside). `fill_span4` clips to the
    // destination's width and mapped length — the same bounds `put_pixel` enforced per pixel — so
    // the pixel set is identical; the only byte that differs is the pad, written 0 where
    // `put_pixel` skipped it, which no reader decodes (scan-out ignores it, `read_pixel` reads 3
    // bytes, and the staged layer's pads are already 0 by construction).
    let fast = dst.word4()
        && matches!(
            dinfo.pixel_format,
            unaos_boot_info::PixelFormat::Rgb | unaos_boot_info::PixelFormat::Bgr
        );
    // In LE memory Bgr stores `b, g, r` — the 0x00RRGGBB word unchanged — while Rgb stores
    // `r, g, b`, the same word with R and B exchanged. Mirrors `FrameBuffer::encode4` exactly.
    let swap = matches!(dinfo.pixel_format, unaos_boot_info::PixelFormat::Rgb);
    let surf = r.surf as *const u8;
    for row in 0..rows {
        // WC-M — where this source row's `scale` destination lines start, in the DESTINATION's own
        // coordinates. Negative means the row begins above `dst`'s row 0, which only a chunked
        // stage's second-or-later band can produce.
        let dy0 = cy + (row * r.scale) as isize;
        if dy0 >= dh as isize {
            // Rows only descend from here, so nothing further can land in `dst`. Behaviourally
            // identical to the pre-WC-M loop, every one of whose remaining writes `put_pixel`
            // rejected on the same bound — it just stops paying for them, which is what keeps a
            // banded present the cost of ONE compose rather than one per band.
            break;
        }
        // The first line of this source row that lands inside `dst`. Zero everywhere except at a
        // band's top edge, where a source row can straddle the boundary: the lines above it were
        // composed and presented by the previous band, and the ones from here are this band's.
        let sy_first = if dy0 < 0 { (-dy0) as usize } else { 0 };
        if sy_first >= r.scale {
            continue;
        }
        let row_base = row * r.stride;
        // `dup` composes exactly one line per source row and replicates it; without it every line is
        // written per-pixel. Either way the run starts at the first line this destination can hold.
        let sy_end = if dup { sy_first + 1 } else { r.scale };
        // COMPOSITE-2 — the SINGLE-LINE row compose, flattened. When the row composes exactly one
        // destination line (every staged `dup` row, and any `scale == 1` row) the whole line is one
        // contiguous, clipped, word-aligned run — so the bounds work is done ONCE here and the
        // column loop degenerates to "encode, store `scale` words, advance". This is where the
        // remaining compose milliseconds lived after the span writer landed: at 128 source columns
        // per row, even `fill_span4`'s once-per-span checks were ~half the per-column work. The
        // span clamps are exactly the writer's: the destination's visible width and its mapped
        // length, so the pixel set is identical to the general path below.
        let fastline = if fast && sy_end == sy_first + 1 {
            let dy = (dy0 + sy_first as isize) as usize;
            let row_off = (dy * dinfo.stride + cx) * 4;
            if dy < dh && cx < dw && row_off + 4 <= dst.len() {
                let span = (cols * r.scale).min(dw - cx).min((dst.len() - row_off) / 4);
                if span > 0 {
                    Some((dst.base_addr() + row_off, span))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some((line0, span)) = fastline {
            // SAFETY: `line0 + span * 4 <= dst.base + dst.len()` by the clamp above; `line0` is
            // 4-aligned (`word4` demands a word-aligned base and every term of `row_off` is a
            // multiple of 4); the source reads carry `paint_window`'s own bound (`row < surf_len /
            // stride`, `col < stride / 4`).
            let mut p = line0 as *mut u32;
            let mut rem = span;
            let mut col = 0usize;
            while rem > 0 && col < cols {
                let px =
                    unsafe { core::ptr::read_unaligned(surf.add(row_base + col * 4) as *const u32) }
                        & 0x00FF_FFFF;
                let raw = if swap {
                    ((px & 0xFF) << 16) | (px & 0xFF00) | (px >> 16)
                } else {
                    px
                };
                let n = r.scale.min(rem);
                for _ in 0..n {
                    unsafe {
                        p.write(raw);
                        p = p.add(1);
                    }
                }
                rem -= n;
                col += 1;
            }
        } else if fast && sy_end == sy_first + 1 {
            // The line is wholly clipped (off the destination's height/width/length): the general
            // path below would have written nothing for it either. Skip straight to replication,
            // whose own clips decide what, if anything, lands.
        } else {
        for col in 0..cols {
            // Unaligned-safe read of the ARGB8888 pixel; low 24 bits are RRGGBB (alpha ignored —
            // this arc composites opaquely). In bounds by construction: `row < surf_len / stride`
            // and `col < stride / 4`, so `row_base + col * 4 + 4 <= surf_len`.
            let px = unsafe { core::ptr::read_unaligned(surf.add(row_base + col * 4) as *const u32) }
                & 0x00FF_FFFF;
            if fast {
                let raw = if swap {
                    ((px & 0xFF) << 16) | (px & 0xFF00) | (px >> 16)
                } else {
                    px
                };
                for sy in sy_first..sy_end {
                    // `sy >= sy_first` makes `dy0 + sy` non-negative by construction.
                    let dy = (dy0 + sy as isize) as usize;
                    dst.fill_span4(cx + col * r.scale, dy, r.scale, raw);
                }
                continue;
            }
            for sy in sy_first..sy_end {
                // `sy >= sy_first` makes `dy0 + sy` non-negative by construction.
                let dy = (dy0 + sy as isize) as usize;
                for sx in 0..r.scale {
                    dst.put_pixel(cx + col * r.scale + sx, dy, px);
                }
            }
        }
        }
        if !dup {
            continue;
        }
        // Replicate the composed line over the remaining `scale - 1` lines of this source row. The
        // segment is exactly the span `put_pixel` accepted above: the clip is the destination's own
        // width, the same bound that decided which pokes landed.
        // WC-M — the line that was actually composed above, which is the row's first line inside
        // `dst` and not necessarily its first line overall. Replicating from any other one would
        // read rows this band does not hold.
        let y0 = (dy0 + sy_first as isize) as usize;
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
        for sy in (sy_first + 1)..r.scale {
            let y = (dy0 + sy as isize) as usize;
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
/// whose single ROW is over [`MAX_STAGE_BYTES`], a lock another core holds, and an allocator that
/// cannot grow the buffer. None of them can lose a window.
///
/// ### WC-M — over-cap boxes are BANDED, not declined
///
/// WC-H sized the buffer to the whole box and declined anything larger. The bench panel is 1920x1200
/// ARGB — ~8.8 MiB for one full-panel box, over twice the cap — so a console-as-a-window, the single
/// largest and most conspicuous present the compositor will ever run, was exactly the present that
/// fell back into the tearing regime WC-H exists to remove.
///
/// The buffer now holds a BAND: `chunk_rows = MAX_STAGE_BYTES / row_bytes` full rows of the box. The
/// pass composes band 0 and presents its rows, then composes band 1 into the same buffer and presents
/// those, and so on. Each band's present is the WC-H present unchanged — one bulk
/// `copy_nonoverlapping` per row, at the panel's row stride — and `paint_window` draws the whole
/// window each time with the band's origin as its destination zero, so the chrome and the upscale
/// keep their true geometry across the seam. A band costs only its own rows: `paint_window` starts at
/// the first source row that lands in the band and breaks at the first that lands past it, so a
/// banded present is one compose's worth of per-pixel work, not one per band.
///
/// **A single band is the pre-WC-M path exactly.** When the box fits the cap, `chunk_rows == bh`, the
/// loop runs once with origin `(bx, by)`, `paint_window` sees a non-negative vertical origin, the
/// buffer is the same size WC-H allocated, and the sprite offer is unconditional as before. Every
/// window on the bench today and every window in the QEMU regression takes that branch.
///
/// ### The visibility window, stated honestly
///
/// **A banded present is NOT atomic, and this arc does not claim it is.** The rows of band 0 are on
/// the panel while band 1 is still being composed, so for the length of one band's compose the panel
/// can hold the new top of the window over the old bottom of it. What the arc DOES guarantee:
///
/// * **Every seam is a full ROW boundary.** A band is a whole number of complete rows and each row
///   still reaches the panel in one bulk copy, so no scanline is ever half-old and half-new — which
///   is the artifact WC-G convicted and WC-H removed. The horizontal tear WC-H was built against
///   cannot come back through this path.
/// * **The seam is bounded and known**: at most `ceil(bh / chunk_rows) - 1` of them, at the row
///   offsets the banding picks, for one compose each.
/// * **Nothing is lost or duplicated.** Every row of the box is composed by exactly one band and
///   presented exactly once, and `paint_window`'s dense border fill covers each band completely, so
///   the "every pixel the present copies was written by this pass" invariant holds per band.
///
/// The alternative that WOULD be atomic — one buffer the size of the whole box — is the panel-sized
/// allocation the cap exists to refuse. What this replaces is not an atomic present; it is the DIRECT
/// path, whose entire scattered per-pixel upscale was visible to the scan-out from the first poke to
/// the last. A bounded number of row-aligned seams is strictly better than that, and is the whole of
/// the trade.
///
/// One thing that does NOT change: [`draw_window`]'s single `flush_range` still runs once, after all
/// bands, over the box's whole row span. On the non-coherent Pi 4 the bands therefore tend to become
/// visible together rather than one at a time — a mitigation worth having and NOT a guarantee (a
/// cache line may be evicted at any point), which is why the seams above are described as real.
///
/// ### FBCON-DMG — `dy0`/`dy1`: which of the box's rows this present owes
///
/// Box-relative row range, `0..bh` for every present that did not declare a band — which is every
/// present that existed before FBCON-DMG, and the branch every aarch64 window takes. When it IS
/// narrowed, the banding loop below simply starts and stops there: each band it runs is the same
/// band, composed the same way and presented by the same bulk row copies, and `paint_window` already
/// takes the box geometry separately from the destination origin (that is WC-M's whole contract), so
/// the chrome and the upscale keep their true geometry with nothing new to reason about. A one-line
/// console change therefore costs one band of a few rows instead of `bh` rows of full-box compose.
#[allow(clippy::too_many_arguments)]
fn stage_window(
    fb: &super::FrameBuffer,
    r: &Window,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    dy0: usize,
    dy1: usize,
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
            #[cfg(feature = "witness")]
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
    #[cfg(feature = "witness")]
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
    // FBCON-DMG — the row range this present owes, clamped to the box. A caller that hands in a band
    // outside the box gets the whole box: over-painting is free of consequence, under-painting is not.
    let (dy0, dy1) = if dy1 > dy0 && dy1 <= bh { (dy0, dy1) } else { (0, bh) };
    let span = dy1 - dy0;
    // `bw <= pw` and `bh <= ph` (both clipped by the caller), so neither product can wrap.
    let row_bytes = bw * bpp;
    // WC-M — the cap sizes a BAND, not the present. `chunk_rows` is how many whole rows of this box
    // fit under it; `bh` of them when the box fits outright, which is the pre-WC-M allocation and the
    // pre-WC-M single-pass present. Zero means one ROW does not fit — the only cap decline left, and
    // unreachable at any panel width this kernel can address.
    //
    // FBCON-DMG: `span`, not `bh` — the buffer only ever has to hold the rows this present writes,
    // so a banded present of a box that would have needed several WC-M bands takes ONE.
    let chunk_rows = if row_bytes == 0 { 0 } else { (MAX_STAGE_BYTES / row_bytes).min(span) };
    if chunk_rows == 0 {
        decline!(super::wcg::DECL_CAP);
    }
    let need = row_bytes * chunk_rows;
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

    // WC-M — the two halves are now accumulated ACROSS bands rather than read off one pair of
    // timestamps, so `[wc-h]` keeps reporting the same two quantities for the same present: all of
    // this window's compose, and all of its panel-facing copy. `stage_note` takes them as
    // `(t_end, t0, t1)` differences, so a zero base and two running totals feed it unchanged — no
    // witness signature moves for this arc, which is what lets the x86 tree take this diff as-is.
    #[cfg(feature = "witness")]
    let (mut compose_cyc, mut present_cyc) = (0u64, 0u64);

    let fb_row = info.stride * bpp;
    // WC-M — one band per turn. `band` is the box-relative row the buffer currently holds.
    // FBCON-DMG — it starts at the damaged range's first row and stops at its last, instead of
    // walking the whole box. `dy0 == 0 && dy1 == bh` is the unbanded present and is the pre-FBCON-DMG
    // loop verbatim.
    let mut band = dy0;
    while band < dy1 {
        let rows = chunk_rows.min(dy1 - band);

        #[cfg(feature = "witness")]
        let b0 = crate::arch::now_cycles();

        // The back layer: same pixel format and bytes/pixel as the panel (so the composed bytes ARE
        // the panel's bytes and the present is a straight copy), but its own stride — the box width,
        // with no panel margin — which is what makes each row a single contiguous run. WC-M: its
        // HEIGHT is the band's, and its length the band's bytes, so every clip `put_pixel` and `blit`
        // already performed now also fences the compose to the rows this buffer holds.
        let mut layer = super::FrameBuffer::new();
        layer.init(
            stage.as_mut_ptr() as usize,
            rows * row_bytes,
            unaos_boot_info::FrameBufferInfo {
                width: bw,
                height: rows,
                stride: bw,
                bytes_per_pixel: bpp,
                pixel_format: info.pixel_format,
            },
        );
        // The BOX is still `(bx, by, bw, bh)` — chrome geometry must not move between bands — while
        // the destination ORIGIN is the band's first panel row. That difference is the whole of the
        // banding as `paint_window` sees it.
        paint_window(&layer, r, bx, by + band, bx, by, bw, bh, pw, ph, focused, true);

        // CURSOR-3 — the sprite, LAST into the layer and therefore on top of the window's own
        // content, and still before a single byte reaches the panel. `compose_into` declines (leaving
        // the layer exactly as `paint_window` left it) unless the sprite's box is wholly inside this
        // box; it takes no lock of the sprite module's, only the plan's own `try_lock`, so nothing
        // here enters F4's drain wait set. Counted either way, so a boot that never manages the
        // overlay says so.
        //
        // WC-M — the offer is made AT MOST ONCE per pass, never once per band. A single-band present
        // offers unconditionally, exactly as before this arc. A BANDED present offers only to a band
        // whose rows contain the sprite's box outright: composing a sprite across two bands would
        // open two overlay sessions inside one composite and publish two plans for one `adopt`, and
        // no part of CURSOR-3's provenance argument covers that. A sprite straddling a seam is
        // therefore taken by nobody, `overlaid` stays false, and the caller's WC-I repaint tail puts
        // it back from the finished front — the same fallback every other decline already uses.
        //
        // FBCON-DMG leaves this contract exactly as it found it. The offer is still "this band holds
        // the sprite's box outright"; the only thing that changed is which rows the band covers, and a
        // band that does not contain the sprite declines here as it always did. `chunk_rows >= span`
        // (not `>= bh`) is the same "single band" test restated against the rows this present owes —
        // a banded present that runs in one turn is as much a single band as an unbanded one is.
        if let Some(plan) = cur {
            let whole = chunk_rows >= span
                || (plan.by >= by + band && plan.by + plan.bh <= by + band + rows);
            if whole {
                let c = super::cursor::compose_into(&layer, bx, by + band, plan);
                *overlaid |= c.taken > 0;
                #[cfg(feature = "witness")]
                note_cursor_overlay(&c);
            }
        }

        #[cfg(feature = "witness")]
        let b1 = crate::arch::now_cycles();

        // Present: one bulk copy per row. This is the whole of what the scan-out can catch
        // mid-flight, and it is the same primitive and the same shape as
        // `Screen::present_background`'s damage-rect flush.
        for y in 0..rows {
            let src = y * row_bytes;
            fb.blit((by + band + y) * fb_row + bx * bpp, &stage[src..src + row_bytes]);
        }

        #[cfg(feature = "witness")]
        {
            compose_cyc += b1.saturating_sub(b0);
            present_cyc += crate::arch::now_cycles().saturating_sub(b1);
        }
        band += rows;
    }

    // `bytes` is what REACHED THE PANEL, not what the buffer held — on a WC-M-banded present that is
    // the whole box, and it is the number that says banding happened at all (a `-> BUFFERED` line
    // whose `bytes=` exceeds `MAX_STAGE_BYTES` could not have been staged before WC-M).
    //
    // FBCON-DMG — `span`, not `bh`, and for exactly the same reason: the rows this present copied are
    // the rows it owed. `[wc-h] bytes=N` therefore keeps meaning "what this present put on the
    // glass", so a damage-limited console line reports its true cost instead of the cost of the box
    // it lives in. Unbanded presents have `span == bh` and their `[wc-h]` numbers do not move — which
    // on aarch64, where no band is ever produced, is every present there is.
    //
    // BOTH heights go to the witness, and that is the FBCON-DMG instrument fix. `span` alone told it
    // how much was written but never how much COULD have been, so it could not tell a banded present
    // of a tall box from a whole-box present of a short one — and with the two sharing one sample
    // budget, the four samples were spent on creation-time whole-box presents before the console had
    // banded once. `stage_note` classifies on `span < bh` and budgets the two classes apart.
    #[cfg(feature = "witness")]
    super::wcg::stage_note(
        r.id,
        bw,
        bh,
        span,
        row_bytes * span,
        compose_cyc + present_cyc,
        0,
        compose_cyc,
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
#[allow(clippy::too_many_arguments)]
fn stage_fill(
    fb: &super::FrameBuffer,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
    requeued: bool,
) -> bool {
    // WC-L — the four decline reasons split by whether RETRYING can ever succeed, and the split is
    // load-bearing rather than tidy.
    //
    // `lock` and `alloc` are TRANSIENT: another core holds `STAGE` right now, or the heap could not
    // grow right now. Both are answered by trying again a pass later, so the box is DEFERRED — the
    // fill does not reach the panel in this call, and `drain_deferred` delivers it through this same
    // staged path next composite. There is no direct `fill_rect` behind this function any more.
    macro_rules! defer {
        ($reason:expr) => {{
            defer_erase(x, y, w, h, $reason, requeued);
            return false;
        }};
    }
    // `geom` and `cap` are PERMANENT for a given box: a degenerate rect and an over-cap row are the
    // same next pass as this one, so deferring them would put a box on the queue that can never come
    // off it, and the drain would re-defer it forever. They are DROPPED instead, and counted as
    // declines — which is what they are, a fill the panel never received — so the rollup's
    // `-> UNSTAGED` FORBID catches them. Neither is reachable on any panel this kernel drives (a
    // single row is at most `width * 4` bytes, three orders of magnitude under `MAX_STAGE_BYTES`);
    // the branch exists so that if one ever becomes reachable it is a loud red and not a silent
    // spin.
    macro_rules! drop_fill {
        ($reason:expr) => {{
            // Named unconditionally: the reason is part of the CORRECTNESS decision (drop, not
            // defer) on every build, and only its reporting is witness-gated.
            let _reason: u32 = $reason;
            #[cfg(feature = "witness")]
            super::wcg::erase_drop(w, h, _reason);
            return false;
        }};
    }
    // WC-L DEFERRAL FIXTURE (witness builds only) — force exactly one deferral per boot, on the
    // first erase that is not already a drain retry.
    //
    // QEMU never reaches this path on its own, and that is precisely why WC-K shipped with a direct
    // fallback nobody had seen fire: the condition needs a second core wanting `STAGE` at the moment
    // a window is torn down, which took a 1920x1200 bench panel at ~99% load to produce, twice, in
    // one attended boot. A path whose only witness is a hardware boot is a path that regresses
    // between hardware boots. The latch is the WC-H fallback fixture's shape, for the same reason and
    // with the same one-shot discipline: `swap(true)` so it can fire at most once, and gated on
    // `!requeued` so the drain's retry is guaranteed to take the real staged path and the queued box
    // is provably delivered rather than cycling.
    //
    // What it proves in `kernel8-test`: the `-> DEFERRED` line, the queue round trip, the drain's
    // `damage_intersecting` re-damage, and the `BUFFERED` erase that follows one pass later. What it
    // does NOT prove is behaviour under genuine lock contention — for that the proof still rides a
    // metal boot, and the rollup's `redefers=` is where it will show.
    #[cfg(all(target_arch = "aarch64", feature = "witness"))]
    {
        if !requeued && !DEFER_FIXTURE.swap(true, core::sync::atomic::Ordering::Relaxed) {
            defer!(DEFER_LOCK);
        }
    }
    let info = fb.info();
    let bpp = info.bytes_per_pixel;
    if w == 0 || h == 0 || bpp == 0 {
        drop_fill!(DEFER_GEOM);
    }
    // Caller clipped `(x, y, w, h)` to the panel, so neither product can wrap.
    let row_bytes = w * bpp;
    let fb_row = info.stride * bpp;
    if row_bytes == 0 || row_bytes > MAX_STAGE_BYTES {
        drop_fill!(DEFER_CAP);
    }
    let mut stage = match STAGE.try_lock() {
        Some(g) => g,
        None => defer!(DEFER_LOCK),
    };
    if stage.len() < row_bytes {
        let add = row_bytes - stage.len();
        // Same `try_reserve` + `resize` contract as `stage_window`: an exhausted heap declines here
        // rather than panicking from a close path.
        if stage.try_reserve(add).is_err() {
            // Drop the guard before queueing: `DEFER` must never be taken while `STAGE` is held.
            drop(stage);
            defer!(DEFER_ALLOC);
        }
        stage.resize(row_bytes, 0);
    }

    // WCD-TEARDOWN — the fill bracket opens HERE, past every `defer!`/`drop_fill!` exit, because a
    // declined fill writes NO PIXEL. Opened above them (as the first cut did) it counted work that did
    // not happen, and a `DEFER_LOCK` on a busy panel could abandon a verdict no fill ever touched. The
    // box travels with it so a fill in the opposite corner cannot abandon a verdict either — see
    // [`PANEL_FILL_BOX`]. From here every path reaches the panel.
    #[cfg(all(feature = "witness", target_arch = "x86_64"))]
    let _panel = PanelWriteGuard::enter(x, y, w, h);
    #[cfg(feature = "witness")]
    let t0 = crate::arch::now_cycles();

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

    #[cfg(feature = "witness")]
    let t1 = crate::arch::now_cycles();

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

    #[cfg(feature = "witness")]
    super::wcg::erase_note(
        w,
        h,
        row_bytes,
        contig,
        crate::arch::now_cycles(),
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
/// WC-M — `y` is SIGNED because a chunked stage paints the box into one row-band at a time and the
/// title sits in the box's first rows, above every band but the first. Glyph rows landing above the
/// destination are dropped; the rest are drawn at their true position, and `put_pixel` clips the
/// bottom as it always did. For `y >= 0` — the direct path, and any stage that fits in one band —
/// this is byte-for-byte the pre-WC-M loop.
fn draw_title(fb: &super::FrameBuffer, r: &Window, x: usize, y: isize, max_w: usize) {
    let cols = max_w / 8;
    for (i, &b) in r.title[..r.title_len].iter().enumerate() {
        if i >= cols {
            break;
        }
        let ch = if (0x20..0x7f).contains(&b) { b } else { b' ' };
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch as usize];
        for (ry, byte) in bitmap.iter().enumerate() {
            let dy = y + ry as isize;
            if dy < 0 {
                continue;
            }
            for rx in 0..8 {
                if byte & (1 << rx) != 0 {
                    fb.put_pixel(x + i * 8 + rx, dy as usize, CHROME_TITLE_FG);
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

    // VUGMIN-C — one more raise, purely to EXERCISE the transition the four legs above cannot reach.
    // Every focus change in this fixture so far arrives from the shell (`prev == 0`), so every
    // `[vugmin] focus` line it prints says `hid=none`. This one goes window→window with both owners
    // live, which is the shape the arc is about, and puts `hid=asid=<b>` on the headless wire. It
    // asserts no pixel — the panel claim is the four legs' — and it is deliberately AFTER the verdict
    // line so it can perturb none of them. Both windows are closed on the next two lines, so the extra
    // raise leaves nothing behind. (`vugmin_publish` no-ops for these synthetic ASIDs: 0xF0A/0xF0B are
    // outside the 64-wide hidden mask, so this moves no real process's bit, only the witness.)
    focus_changed(ASID_A);

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
/// pinned — nothing in user mode moves a window, so every real window is laid out by the TILER — and a
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
    let t = table();
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
    at: Option<(usize, usize)>,
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
    // SPAWN-PLACE — resolve the caller's origin BEFORE the table lock (WRITER is never held across
    // it, which is what keeps the WRITER/TABLE order acyclic here as it is in `move_to` and `place`).
    // A framebuffer that is not ready leaves the row to the tiler, exactly as `move_to` declines.
    let placed = at.and_then(|(x, y)| {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            return None;
        }
        let info = fb.info();
        let scale = place_scale(info.width, info.height, w, h);
        // The same clamp `move_to` applies, for the same reason (F5: the kernel builds without
        // overflow checks, so an unclamped origin would wrap in the geometry arithmetic).
        let cw = w.saturating_mul(scale);
        let ch = h.saturating_mul(scale);
        let max_x = info.width.saturating_sub(cw + BORDER).max(BORDER);
        let max_y = info
            .height
            .saturating_sub(ch + BORDER)
            .max(TITLE_H + BORDER);
        Some((
            x.clamp(BORDER, max_x),
            y.clamp(TITLE_H + BORDER, max_y),
            scale,
        ))
    });
    let mut t = table();
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
    row.damage_all();
    row.compat = compat;
    row.title_len = title.len().min(MAX_TITLE);
    row.title[..row.title_len].copy_from_slice(&title[..row.title_len]);
    // SPAWN-PLACE — the row is born at its final geometry and PINNED, so the `place` below skips it
    // and the `composite` below paints it exactly once, where it stays.
    if let Some((x, y, scale)) = placed {
        row.x = x;
        row.y = y;
        row.scale = scale;
        row.pinned = true;
    }
    t.rows[slot] = row;
    drop(t);
    // WC-D: ids are recycled slot aliases, so a fresh window in a used slot is a DIFFERENT window and
    // deserves its own verdict — clear the one-shot latch here rather than at close, which is the point
    // where the id demonstrably names something new.
    #[cfg(feature = "witness")]
    if id < 32 {
        VERIFIED.fetch_and(!(1u32 << id), core::sync::atomic::Ordering::Relaxed);
        // WC-D/PAYGO — the terminal latch travels with the first-verdict one, or a recycled id would
        // inherit its predecessor's completed battery and the new window would never be verified at all.
        // The `deferred=`/`emit=` census deliberately does NOT reset: it is a per-ID total for the whole
        // boot, and `emit=` has to stay monotone or the reader's "greatest `emit=` per `win=` supersedes"
        // rule breaks across a recycle.
        #[cfg(all(target_arch = "x86_64", feature = "wcg-paygo"))]
        VERIFIED_FULL.fetch_and(!(1u32 << id), core::sync::atomic::Ordering::Relaxed);
        // WC-D — the STATE cell is the one that actually re-arms the window; the two bitmasks above
        // are published flags derived from it. Reset last, so no core can observe a cleared mask
        // against a stale state.
        WCD_STATE[id as usize].store(WCD_ST_FIRST, core::sync::atomic::Ordering::Release);
        // WC-D/PAYGO — `WCD_SAID` travels with them. It is the "this window's census-opening line has
        // been spoken" latch, and leaving it set would let a NEW tenant's first decline fall through
        // to the 2 s cadence gate — silently breaking the guarantee `WCD_SAID`'s own note makes, that
        // the first decline always speaks. The census totals (`WCD_DEFERRED`, `WCD_EMIT`) are NOT
        // reset: `emit=` must stay monotone per id or the reader's "greatest `emit=` supersedes" rule
        // breaks across a recycle.
        #[cfg(all(target_arch = "x86_64", feature = "wcg-paygo"))]
        WCD_SAID[id as usize].store(0, core::sync::atomic::Ordering::Relaxed);
        // PAYGO-TERM — and the TERMINAL latches travel with them, on both wires. `PAYGO_CLOSE_SAID`
        // and `wcg::PAYGO_CLOSED` mean "this window has spoken its closing line", which is a fact
        // about the WINDOW and not about the slot: the batteries the terminal reports on re-arm two
        // lines above (wc-d) or are per-slot by design (wc-g), but either way the row this id names
        // is a different window that will live its own life and die its own death. Left set, they
        // would deny every tenant after the first its terminal — the silent `state=waiting` close
        // this arc exists to remove, reintroduced for six of the seven windows slot 3 hosts in the
        // s73 capture. That is the same argument the `WCD_SAID` note makes one line up, and the
        // opposite of the `WCD_ABORTS` argument one line down: the budget is per boot, the verdict
        // is per tenant — and so is the last word.
        #[cfg(all(target_arch = "x86_64", feature = "wcg-paygo"))]
        {
            PAYGO_CLOSE_SAID[id as usize].store(0, core::sync::atomic::Ordering::Relaxed);
            super::wcg::paygo_recycle(id as usize);
            // PAYGO-TERM — and so is the TAKER's budget. `PAYGO_SVC_TRIES` bounds how many times the
            // service-pass taker will mark THIS window before giving up on it, and `PAYGO_SVC_NOTED`
            // is the one-shot that makes the giving-up audible. A count inherited from a predecessor
            // starves the new tenant's taker, and an inherited note makes that starvation silent —
            // the counter's cap is an equality the earlier tenant already consumed. Reset here rather
            // than at close because `paygo_at_close` returns early on exactly the tenant that spent
            // the most budget: the one that owed nothing by the time it died.
            PAYGO_SVC_TRIES[id as usize].store(0, core::sync::atomic::Ordering::Relaxed);
            PAYGO_SVC_NOTED[id as usize].store(0, core::sync::atomic::Ordering::Relaxed);
        }
        // WCD-TEARDOWN — `WCD_ABORTS` is deliberately NOT cleared here. Re-arming the latches hands
        // the new tenant a fresh verdict, which is right; re-arming the abort budget with it would
        // hand it fresh RETRIES, and this very function is one of the two unbarriered `erase` sites
        // (`reclaim` below, no drain barrier — see [`PANEL_FILL_EPOCH`]). A slot that recycles under load
        // would then be able to abandon a verdict on every cycle and never exhaust anything, which is
        // the interlock defeating its own bound. The budget is per boot; the verdict is per tenant.
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
/// The tiler's SCALE RULE, factored out so [`spawn_geometry`] answers with the layout's own value
/// rather than a second copy of it: the largest integer factor whose scaled surface fits half the
/// panel width and half the layout area's height, capped by [`legibility_cap`], never 0. `usable_h`
/// is derived here exactly as [`place`] derives it (PULSE-2's bottom-chrome reservation).
fn place_scale(pw: usize, ph: usize, w: usize, h: usize) -> usize {
    let usable_h = ph.saturating_sub(crate::ui_status::chrome_h(ph)).max(1);
    (pw / 2 / w.max(1))
        .min(usable_h / 2 / h.max(1))
        .min(legibility_cap(ph))
        .max(1)
}

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

    // PULSE-2 — the bottom chrome's reservation. `ui_status` owns the last `chrome_h(ph)` rows of the
    // panel: the CPU pulse instrument and the host/ip/clock status line. The tiler lays out against
    // `usable_h` instead of `ph` so a window box is never *placed* into those rows in the first place.
    //
    // This is a vertical budget rather than a `wcf::reserved`-style box list because the two answer
    // different questions: WC-F's list lets the compositor refuse to paint a probe over a window that
    // is already there, while this must stop the window arriving. It also does not touch `occluders`
    // (WC-I): occlusion decides who wins where regions overlap, and after the reservation they do not.
    //
    // Note the scale rule reads `usable_h` but `legibility_cap` still reads `ph` — the cap is a
    // function of panel DENSITY (how big a font pixel wants to be), not of the layout area.
    let usable_h = ph
        .saturating_sub(crate::ui_status::chrome_h(ph))
        .max(1);

    let mut t = table();
    let mut cx = GAP;
    let mut cy = GAP + TITLE_H + BORDER;
    let mut row_h = 0usize;
    for i in 0..MAX_WINDOWS {
        let r = &t.rows[i];
        if !r.used || r.compat || r.pinned {
            continue;
        }
        let (w, h) = (r.w.max(1), r.h.max(1));
        let scale = place_scale(pw, ph, w, h);
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
        // PULSE-2 — the last-resort clamp. The scale rule already keeps a single row inside
        // `usable_h`, but rows STACK: enough windows and `cy` walks off the bottom. When that happens
        // the window is pulled back up so its box ends at the reservation. Two windows overlapping
        // each other is a legibility problem the operator can fix by closing one; a window over the
        // instrument panel silently breaks the one surface that is supposed to be always readable.
        r.y = cy.min(usable_h.saturating_sub(bh));
        r.damage_all();
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

// ---- CLICK-ROUTE witness -----------------------------------------------------------------------

/// CLICK-ROUTE witness surface: one solid 8x8 ARGB8888 patch in kernel rodata, on the same terms as
/// the FOCUS-VIS surfaces (static so the selftest allocates nothing; read-only because the compositor
/// only ever reads a window's surface). Both probe windows share it — this witness reads the TABLE,
/// not the panel, so the colours carry no verdict and there is no reason to have two.
#[cfg(feature = "witness")]
static HT_SURF: [u32; 64] = [0x0020_C080; 64];

/// CLICK-ROUTE — the HIT-TEST witness: does [`hit_test`] name the window an operator would say they
/// clicked on?
///
/// ### Why this one is a TABLE test and not a read-back
/// FOCUS-VIS reads the scan-out back because its question ("is the focused window frontmost?") is a
/// question about PIXELS, and kernel state had already been observed lying about it. This question is
/// different in kind: `hit_test` is a pure function of the window table and the z-order, its answer is
/// an ASID rather than a colour, and the property that matters is that it agrees with the compositor's
/// own visibility predicate. Asserting that directly is both stronger and drivable on a headless gate
/// with no pointer at all — which matters, because the pointer is exactly what QEMU raspi4b does not
/// have, and the routing this feeds is otherwise metal-only.
///
/// ### The five legs, over two windows placed at ONE origin
/// 1. **inside** — a point inside the shared content area hits SOMETHING (the stack is addressable).
/// 2. **topmost** — that something is B, the later-created window, which is the one in front. A
///    hit-test that returned the first matching row instead of the frontmost would fail here and
///    nowhere else, and it would mis-route every click on an overlap.
/// 3. **raise** — after `focus_changed(A)` the same point hits A. Raising changes who owns the pixel,
///    so it must change who owns the click; this is the leg that ties the two orders together.
/// 4. **outside** — a point clear of both boxes hits nothing. Misses must be misses: the router's
///    desktop arm consumes the click on the strength of a `None`, so a false hit here would hand the
///    console's clicks to an app. The point is FOUND on the live window table (the panel's furniture
///    owns most of it since CLICK-X86), and the leg reports `skip` rather than a false verdict if the
///    panel leaves nothing unowned — see the search itself for why that is not circular.
/// 5. **hidden** — after `focus_changed(0)` the shell is above both windows, they stop compositing,
///    and the same inside point hits nothing. Visibility is a POSITION in the shared z-order, and what
///    you can click has to be what you can see: clicking a console that covers a window must reach the
///    console. This is [`above_shell`] holding on the input side as well as the drawing side.
/// 6. **shell** (CLICK-SHELL, P71) — the ROUTER leg, and the only one that leaves this file. With A
///    raised and focused, a press that hit-tests to nothing must (a) be consumed and (b) hand focus to
///    the SHELL. Legs 1-5 assert the address lookup; this one asserts the POLICY built on it, because
///    P71's finding was not a bad hit-test — `hit_test` answered `None` correctly and the router threw
///    the answer away. Driven through the real `wc_click_route` (a synthetic `Button` press edge, then
///    a release edge to leave the router's mask tracker as it was found), against the REAL cursor
///    position, so it exercises the shipped path rather than a re-implementation of it. The leg reports
///    SKIP rather than FAIL when the cursor happens to sit over a live window (nothing has moved the
///    pointer on a headless gate, so it is parked at panel centre — but that is a fact about the
///    fixture set, not something this witness may assume), and DORMANT on builds where the router does
///    not exist (x86_64 and the hosted aarch64 build).
/// 7. **bare** (CLICK-SHELL r2, P72) — leg 6's press again, from a focus that owns NO window and no
///    compat row. Leg 6 asserts the policy for a WINDOWED focus, which is the case CLICK-SHELL got
///    right; leg 7 asserts it for the focus that owns nothing on the panel, which is the case it got
///    wrong and which this suite reported PASS on for the whole time the bench could not click into
///    the shell. See [`clickshell_windowless_leg`].
/// 8. **hit / deliver / wake** (CLICK-PLAIN, P75) — the HIT arm's policy, which legs 6 and 7 leave
///    entirely unasserted: a press on an UNFOCUSED window must move focus and then reach that window's
///    ring WHOLE (press and release both), the next press on it must be delivered too, and neither may
///    cost — or precede — the VUGPAUSE-2r2 restore chain. Three fields rather than one because they
///    fail in three different directions, and the last one is what pins the ORDER the fix depends on.
///    See [`clickplain_leg`].
/// 9. **close** (CLOSE-BOX, P79) — the one ACTION click in the grammar: a press in a window's close
///    box is consumed by the router and the row goes away. See [`closebox_leg`].
/// 10. **closereal** (CLOSE-FIX, P82) — the same close arm against a row the battery created through
///    the ordinary path (`wa`), asserting the named row is the reaped row and the settle read-back
///    is the `noproc-selftest` discriminator. See [`closebox_real_leg`].
///
/// Self-cleaning on the FOCUS-VIS pattern: both windows are closed, `SHELL_Z` and `FOCUS_ASID` are
/// restored (this calls `focus_changed` with SYNTHETIC asids, which must not be left naming an address
/// space that does not exist), and the live set is repainted — and, since CLOSE-FIX, a teardown
/// witness guard sweeps the table for any row still owned by a battery ASID and reports a reap as a
/// FAIL-shaped line (a silent fixture leak polluted a whole bench boot's hit-tests in P82).
/// `witness`-gated, one-shot, and ordered after every one-shot per-window latch so it cannot burn
/// one with its own rows.
/// CLICK-SHELL (P71) — leg 6 of [`hittest_selftest`], factored out because it is the one leg that
/// leaves this file: it drives the ARCH router (`wc_click_route`), which exists only on the baremetal
/// aarch64 build, through the same `#[cfg]` seam [`vugmin_publish`] already uses for `set_hidden`.
///
/// The fixture, in order: raise `asid` back above the shell (leg 5 buried everything) and give it
/// focus; read the REAL cursor; bail out (`None` — asserted nothing) if the pointer happens to be over
/// a live window, since then the press is a HIT and this leg has no fixture; otherwise drive one PRESS
/// edge and check the three things P71 asks for — the press is CONSUMED, focus is now the SHELL
/// (`user_input_active() == 0`), and the previously focused window is BELOW the shell, which is what
/// makes the shell focus the same state VUGMIN idles the fleet from.
///
/// One RELEASE edge follows, so the router's press/release tracker is left exactly as it was found: a
/// witness that left a phantom press outstanding would make the operator's next real release drop.
#[cfg(all(feature = "witness", target_arch = "aarch64", feature = "baremetal"))]
fn clickshell_leg(asid: u64, ix: i32, iy: i32, w: i32, h: i32) -> Option<bool> {
    use crate::arch::aarch64::syscall as sc;
    focus_changed(asid);
    let (cx, cy) = crate::pal::cursor::pos(w, h);
    if hit_test(cx, cy).is_some() {
        return None; // pointer parked over a window: that is the HIT arm, not the desktop arm
    }
    sc::user_input_set_active(asid);
    let consumed = sc::wc_click_route(crate::pal::Event::Button(1));
    let refocused = sc::user_input_active() == 0;
    let buried = hit_test(ix, iy).is_none();
    let _ = sc::wc_click_route(crate::pal::Event::Button(0));
    Some(consumed && refocused && buried)
}

/// CLICK-SHELL, DORMANT half: no arch router in this build, so leg 6 asserts nothing.
#[cfg(all(feature = "witness", not(all(target_arch = "aarch64", feature = "baremetal"))))]
fn clickshell_leg(asid: u64, _ix: i32, _iy: i32, _w: i32, _h: i32) -> Option<bool> {
    focus_changed(asid);
    None
}

/// CLICK-SHELL r2 (P72) — leg 7 of [`hittest_selftest`], and the leg that would have caught the metal
/// defect leg 6 slept through.
///
/// Leg 6 drives the miss arm with a focus that owns a window, which is the case CLICK-SHELL got right;
/// it therefore passed on the gate for the whole time the bench could not click into the shell. The
/// state that failed is the one leg 6 has no fixture for: a focus that owns **no window and no compat
/// row** — a `run` program before its first present, a batch program that never presents, a windowed
/// app that closed its last window. On the shipped predicate
/// (`click_owner_is_windowed`) that focus took the DELIVER arm, so the press went to an app that owns
/// nothing on the panel and the shell was reachable only by cycling the entire TAB ring.
///
/// `asid` is a synthetic owner of nothing. The fixture is `focus_changed(asid)` (which raises no
/// window — it only publishes `FOCUS_ASID`) plus `user_input_set_active(asid)`, so BOTH halves of the
/// focus primitive name a windowless owner; the leg then drives one PRESS edge and requires the press
/// to be CONSUMED and both halves to have moved to the shell (`user_input_active() == 0` **and**
/// `FOCUS_ASID == 0`), which is precisely the state a TAB-to-shell leaves. Checking `FOCUS_ASID` as
/// well as the router's active ASID is the point: "focus moved" means the routing half and the VISIBLE
/// half both moved, and a fix that did only the first would read as focused and look unfocused.
///
/// SKIPs (`None`) when a compat row is live, since that is the one state whose press is legitimately
/// delivered rather than consumed — the exemption this leg must not assert against. A release edge
/// follows for leg 6's reason: the router's press tracker is left as it was found.
#[cfg(all(feature = "witness", target_arch = "aarch64", feature = "baremetal"))]
fn clickshell_windowless_leg(asid: u64) -> Option<bool> {
    use crate::arch::aarch64::syscall as sc;
    if compat_live() {
        return None; // a full-screen app owns the panel: the deliver arm, not this leg's fixture
    }
    focus_changed(asid); // no window to raise — this only names the windowless owner
    sc::user_input_set_active(asid);
    let consumed = sc::wc_click_route(crate::pal::Event::Button(1));
    let refocused = sc::user_input_active() == 0;
    let visible = FOCUS_ASID.load(core::sync::atomic::Ordering::Acquire) == 0;
    let _ = sc::wc_click_route(crate::pal::Event::Button(0));
    Some(consumed && refocused && visible)
}

/// CLICK-SHELL r2, DORMANT half: no arch router in this build, so leg 7 asserts nothing.
#[cfg(all(feature = "witness", not(all(target_arch = "aarch64", feature = "baremetal"))))]
fn clickshell_windowless_leg(_asid: u64) -> Option<bool> {
    None
}

/// CLICK-PLAIN (P75) — leg 8 of [`hittest_selftest`]: **a press goes to the window under the cursor,
/// and if that window was not focused, the focus goes there FIRST.**
///
/// Legs 6 and 7 assert the MISS arm's policy. This one asserts the HIT arm's. It was written for
/// CLICK-SWALLOW, which consumed the focus-changing press so an app could never see a click it had not
/// owned the focus for; P75 retired that rule (the withheld press made a click's visible effect a
/// function of invisible state) and the leg's assertions invert with it. The three checks are the three
/// the router has to get right at once, and the ORDER between the first and third is the whole content:
///  * **hit** — a press on an UNFOCUSED window moves focus AND is delivered WHOLE to the raised owner's
///    ring: depth 1 after the press, depth 2 after the release. The ring is the only honest place to ask
///    that question ([`user_input_depth`]); the router's return value says what the ROUTER decided, not
///    what the app got, and the two are what the whole seam sits between. Checking the release as well
///    as the press is what pins [`CLICK_PRESS_TARGET`] to the RAISED owner rather than the sentinel — a
///    press delivered with a dropped release is the half-click that tracker exists to prevent, in the
///    other direction.
///  * **deliver** — the very next press, now on the SAME (and now focused) window, is delivered too:
///    depth 3. The refocusing click must cost the operator nothing and must not consume the next one.
///  * **wake** — the delivered press still runs the VUGPAUSE-2r2 restore chain, and runs it BEFORE the
///    push. A router that queued the press first and moved focus after would pass the first two checks
///    and strand a PARKED vug: the click would land in the ring of the app that was focused a moment
///    ago. `user_input_wake_edges` counts the NAMED edges the seam was asked to run, so this reads 2
///    (focus arrival, then unhide) regardless of whether anything was parked — which matters, since on
///    a headless gate nothing ever is.
///
/// Drives [`crate::arch::aarch64::syscall::user_input_enqueue`] rather than `wc_click_route`, because
/// the claim is about DELIVERY and the push lives on the far side of the router. The window is placed
/// UNDER the real cursor (the router hit-tests the live pointer, so the fixture moves the window, not
/// the pointer); `None` = not asserted, when the pointer sits somewhere this leg cannot build that
/// fixture, when a compat row owns the panel, or when `owner` is not free to borrow.
///
/// Self-cleaning: the window is closed, the owner's ring is reset (via a focus arrival, the one
/// primitive that clears it) and focus is dropped to the shell, so the slot is left exactly as found.
#[cfg(all(feature = "witness", target_arch = "aarch64", feature = "baremetal"))]
fn clickplain_leg(owner: u64, other: u64, surf: usize, len: usize) -> Option<(bool, bool, bool)> {
    use crate::arch::aarch64::syscall as sc;
    use crate::pal::Event::Button;
    if compat_live() {
        return None; // the deliver-as-before exemption owns the panel: not this leg's fixture
    }
    // `owner` is a REAL private-slot ASID (it must be, or it would have no ring at all), so unlike
    // ASID_A/B it could in principle name a LIVE app. Borrow it only if nothing is using it: it holds
    // no input focus and owns no window in the table.
    if sc::user_input_active() == owner {
        return None;
    }
    let mut ring = [0u64; MAX_WINDOWS];
    let n = focus_ring(&mut ring);
    if ring[..n].contains(&owner) {
        return None;
    }

    let (pw, ph) = {
        let info = super::WRITER.lock().info();
        (info.width as i32, info.height as i32)
    };
    let (cx, cy) = crate::pal::cursor::pos(pw, ph);
    // Room for the chrome above/left of the content origin and for the box below/right of it.
    if cx < 64 || cy < 64 || cx + 64 >= pw || cy + 64 >= ph {
        return None;
    }

    let w = create(owner, surf, len, 8, 8, 32, b"ht-s");
    if w == WIN_NONE {
        return None;
    }
    // Content origin two pixels up-left of the pointer, so the pointer is inside the content area —
    // the same relation leg 1's probe point has to `ox`/`oy`, built the other way round.
    move_to(w, (cx - 2) as usize, (cy - 2) as usize);
    focus_changed(owner); // raise it above the fixture rows leg 5 left buried and leg 6 re-raised
    if hit_test(cx, cy).map(|(_, a, _)| a) != Some(owner) {
        close(w); // the pointer does not address this window after all: no fixture, assert nothing
        focus_changed(0);
        return None;
    }

    // --- hit: an UNFOCUSED hit. A focus arrival resets the ring, so `owner` starts empty; focus then
    // moves to `other` (a synthetic ASID outside the slot range — it clears no ring and runs no wake
    // edge of its own, so the baseline below is clean).
    sc::user_input_set_active(owner);
    sc::user_input_set_active(other);
    let edges0 = sc::user_input_wake_edges();
    let queued = sc::user_input_enqueue(Button(1));
    let depth_press = sc::user_input_depth(owner);
    let refocused = sc::user_input_active() == owner;
    let edges1 = sc::user_input_wake_edges();
    let _ = sc::user_input_enqueue(Button(0));
    let depth_release = sc::user_input_depth(owner);
    let hit = queued && refocused && depth_press == 1 && depth_release == 2;
    let woke = edges1.wrapping_sub(edges0) >= 2;

    // --- deliver: the SAME window, now focused (the press above left focus here). Ordinary app input,
    // and it must stack on the pair already in the ring rather than replace it.
    let queued2 = sc::user_input_enqueue(Button(1));
    let delivered = queued2 && sc::user_input_depth(owner) == 3;
    let _ = sc::user_input_enqueue(Button(0));

    close(w);
    sc::user_input_set_active(owner); // the one primitive that resets a ring — leave the slot clean
    sc::user_input_set_active(0);
    focus_changed(0);
    Some((hit, delivered, woke))
}

/// CLICK-PLAIN, DORMANT half: no arch router (and no user input rings) in this build, so leg 8
/// asserts nothing.
#[cfg(all(feature = "witness", not(all(target_arch = "aarch64", feature = "baremetal"))))]
fn clickplain_leg(_owner: u64, _other: u64, _surf: usize, _len: usize) -> Option<(bool, bool, bool)> {
    None
}

/// CLOSE-BOX (P79) — leg 9 of [`hittest_selftest`]: **a press in a window's close box routes to
/// CLOSE, and the row goes away.**
///
/// Legs 6-8 assert where an ordinary press GOES. This one asserts the single exception to
/// CLICK-SELECT's "a click only selects" grammar: the close box is window FURNITURE, and a press
/// that lands in it is an action — the router consumes it, closes the owner's windows, and kills
/// the owner. The leg drives the shipped router (`wc_click_route`, real cursor position — the
/// CLICK-PLAIN fixture discipline: move the window under the pointer, never the pointer), with the
/// window placed so its CLOSE BOX contains the cursor rather than its content area.
///
/// `owner` is synthetic and outside the private-slot range, deliberately: the router's kill arm
/// skips an ASID with no process behind it, so the leg asserts the two things the WINDOW layer owes
/// — the press is CONSUMED (it neither selects nor reaches any ring) and the row is GONE
/// (`info(w)` empty) — without arming a real kill on the gate. The kill half of the arm is the
/// SKILL-1 primitive `bg`/`run` already gate elsewhere; re-proving it here would add a process
/// launch to a window-layer witness.
///
/// `None` = not asserted: a compat row owns the panel, the pointer parks where the fixture cannot
/// straddle it with a close box (too near an edge for the chrome), the table is full, or the
/// placement lands under some other window. A release edge follows the press either way, leaving
/// the router's mask tracker as it was found (the release follows a DROPPED press, so it is
/// consumed too — asserting nothing, restoring everything).
#[cfg(all(feature = "witness", target_arch = "aarch64", feature = "baremetal"))]
fn closebox_leg(owner: u64, surf: usize, len: usize) -> Option<bool> {
    use crate::arch::aarch64::syscall as sc;
    if compat_live() {
        return None; // the deliver-as-before exemption owns the panel: not this leg's fixture
    }
    let (pw, ph) = {
        let info = super::WRITER.lock().info();
        (info.width as i32, info.height as i32)
    };
    let (cx, cy) = crate::pal::cursor::pos(pw, ph);
    // Room for a whole window left/below the pointer and its chrome above it.
    if cx < 96 || cy < 96 || cx + 96 >= pw || cy + 96 >= ph {
        return None;
    }
    let w = create(owner, surf, len, 8, 8, 32, b"ht-x");
    if w == WIN_NONE {
        return None;
    }
    // Place the CONTENT origin so the close box contains the pointer. From `close_box`'s own
    // arithmetic (BORDER=1 cancels): the box spans x in [r.x + w*scale - side, r.x + w*scale) and
    // y in [r.y - TITLE_H, r.y - TITLE_H + side); aim the pointer at the box's centre. The scale
    // is read back from the row `create` actually minted, not re-derived.
    let side = TITLE_H as i32;
    let ws = match info(w) {
        Some(wi) => (wi.w * wi.scale) as i32,
        None => 0,
    };
    let (x, y) = (cx + side / 2 - ws, cy + TITLE_H as i32 - side / 2);
    if ws == 0 || x < 0 || y < (TITLE_H + BORDER) as i32 {
        close(w);
        return None;
    }
    move_to(w, x as usize, y as usize);
    focus_changed(owner); // raise it above the fixture rows the earlier legs left behind
    sc::user_input_set_active(owner); // the closed owner also holds focus: the arm must hand it back
    // Fixture validity: the pointer must address THIS window, in its CLOSE BOX.
    if hit_test(cx, cy).map(|(i, a, _)| (i, a)) != Some((w, owner)) || !close_box_hit(w, cx, cy) {
        close(w);
        sc::user_input_set_active(0);
        focus_changed(0);
        return None;
    }
    let consumed = sc::wc_click_route(crate::pal::Event::Button(1));
    let gone = info(w).is_none();
    let refocused = sc::user_input_active() == 0;
    let _ = sc::wc_click_route(crate::pal::Event::Button(0));
    // The router's close arm already dropped focus to the shell (asserted above); nothing to clean
    // beyond making sure a FAILED close does not leak the row.
    if !gone {
        close(w);
        sc::user_input_set_active(0);
        focus_changed(0);
    }
    Some(consumed && gone && refocused)
}

/// CLOSE-BOX, DORMANT half: no arch router in this build, so leg 9 asserts nothing.
#[cfg(all(feature = "witness", not(all(target_arch = "aarch64", feature = "baremetal"))))]
fn closebox_leg(_owner: u64, _surf: usize, _len: usize) -> Option<bool> {
    None
}

/// CLOSE-FIX (P82) — leg 10 of [`hittest_selftest`]: **a routed close click on a row the battery
/// created through the ordinary path reaps THAT row, and the settle tag is the selftest tag.**
///
/// Leg 9 proves the close arm's mechanics on a probe row it builds for the purpose. What it cannot
/// prove — and what P82 fell through — is that a close click resolves to the row the operator is
/// actually looking at and that the wire's settle tag is honest about who (if anyone) was killed:
/// the leg EXPECTED `noproc`, so a boot in which every real close also settled `noproc` read as
/// green. This leg closes `wa` — a window the battery minted at the top of the run exactly as an
/// app would — through the shipped router, and asserts three things leg 9 does not:
///  * the ROW THE LEG NAMED is the one reaped (`info(wa)` empty afterwards — a router that
///    threaded a constant, or resolved some other row first, fails here);
///  * the settle READ-BACK ([`crate::arch::aarch64::syscall::wc_close_last_settle`]) is
///    `noproc-selftest` and nothing else — `wa`'s owner is synthetic, so any OTHER tag means the
///    close acted on a process it had no business finding, and plain `noproc` means the
///    discriminator regressed;
///  * the press was consumed and focus fell to the shell, as in leg 9.
///
/// Fixture discipline is leg 9's exactly (move the window's close box under the real pointer, never
/// the pointer; validate with the same `hit_test` + `close_box_hit` pair the router uses; `None`
/// when the fixture cannot be built). On the success path the row is gone through the router — the
/// battery's own `close(wa)` tail then no-ops harmlessly; on every bail-out `wa` is left exactly
/// where the battery can still close it.
#[cfg(all(feature = "witness", target_arch = "aarch64", feature = "baremetal"))]
fn closebox_real_leg(w: WinId, owner: u64) -> Option<bool> {
    use crate::arch::aarch64::syscall as sc;
    if compat_live() {
        return None; // the deliver-as-before exemption owns the panel: not this leg's fixture
    }
    let (pw, ph) = {
        let info = super::WRITER.lock().info();
        (info.width as i32, info.height as i32)
    };
    let (cx, cy) = crate::pal::cursor::pos(pw, ph);
    if cx < 96 || cy < 96 || cx + 96 >= pw || cy + 96 >= ph {
        return None;
    }
    let side = TITLE_H as i32;
    let ws = match info(w) {
        Some(wi) => (wi.w * wi.scale) as i32,
        None => 0, // `wa` already gone (leg 9's retry fell through onto it): no fixture
    };
    let (x, y) = (cx + side / 2 - ws, cy + TITLE_H as i32 - side / 2);
    if ws == 0 || x < 0 || y < (TITLE_H + BORDER) as i32 {
        return None; // no row was minted here: `wa` stays for the battery's own close
    }
    move_to(w, x as usize, y as usize);
    focus_changed(owner);
    sc::user_input_set_active(owner);
    if hit_test(cx, cy).map(|(i, a, _)| (i, a)) != Some((w, owner)) || !close_box_hit(w, cx, cy) {
        sc::user_input_set_active(0);
        focus_changed(0);
        return None; // fixture invalid; the battery's `close(wa)` still owns the row
    }
    let consumed = sc::wc_click_route(crate::pal::Event::Button(1));
    let gone = info(w).is_none();
    let settle_ok = sc::wc_close_last_settle() == sc::CLOSE_SETTLE_NOPROC_SELFTEST;
    let refocused = sc::user_input_active() == 0;
    let _ = sc::wc_click_route(crate::pal::Event::Button(0));
    if !gone {
        // A FAILED close must not change who owns the cleanup: hand focus back and leave the row
        // to the battery's tail.
        sc::user_input_set_active(0);
        focus_changed(0);
    }
    Some(consumed && gone && settle_ok && refocused)
}

/// CLOSE-FIX, DORMANT half: no arch router in this build, so leg 10 asserts nothing.
#[cfg(all(feature = "witness", not(all(target_arch = "aarch64", feature = "baremetal"))))]
fn closebox_real_leg(_w: WinId, _owner: u64) -> Option<bool> {
    None
}

#[cfg(feature = "witness")]
pub fn hittest_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        serial_println!("[clickroute] hit-test -> SKIP (framebuffer not ready)");
        return;
    }
    let info = fb.info();
    if info.width < 128 || info.height < 128 {
        serial_println!(
            "[clickroute] hit-test -> SKIP (panel {}x{} too small)",
            info.width, info.height
        );
        return;
    }

    const ASID_A: u64 = 0xC0A;
    const ASID_B: u64 = 0xC0B;
    /// CLICK-SHELL r2 leg 7's owner: synthetic, and deliberately never passed to `create` — the leg
    /// needs a focus that owns NOTHING.
    const ASID_C: u64 = 0xC0C;
    /// CLICK-PLAIN leg 8's owner, and the one ASID here that is NOT synthetic: the leg asserts what
    /// an app's input RING received, and rings exist only for private slots (`1..=USER_SLOTS`). The
    /// HIGHEST slot, because slots are handed out from the bottom, so it is the last one a live app
    /// would be holding — and the leg checks that it is free before borrowing it, and resets it after.
    const PLAIN_ASID: u64 = 8;
    /// CLOSE-BOX leg 9's owner: synthetic (outside the slot range) so the router's close arm has a
    /// window row to remove and provably no process to kill — see [`closebox_leg`].
    const ASID_X: u64 = 0xC0D;

    // One origin for both (so exactly one can own the probe point), upper-middle, clear of WC-F's
    // reserved boxes at the bottom edge. `move_to` pins the rows against the tiler below.
    let ox = info.width / 3;
    let oy = info.height / 4 + TITLE_H + BORDER;
    // The outer box the probe rows WILL be given, from the tiler's own scale rule — asked before any
    // row exists, so the miss-point search below can run against the panel as it stands.
    let (scale, bw, bh) = match spawn_geometry(8, 8) {
        Some(g) => g,
        None => {
            serial_println!("[clickroute] hit-test -> SKIP (geometry unavailable)");
            return;
        }
    };
    let (bx, by) = (ox.saturating_sub(BORDER), oy.saturating_sub(TITLE_H + BORDER));

    // CLICK-X86 fallout — the MISS point is FOUND on the live table, never derived from a constant.
    // Until kernel furniture had an owner, `hit_test` skipped every owner-0 row, so everything but an
    // app window read as a miss and a point offset from this fixture's own origin was unowned BY
    // CONSTRUCTION. CLICK-X86 gave the console and the desktop demo owners in the reserved band and
    // made them hittable, and that invariant died with it — on the rMBP's 2880x1800 panel the console
    // window is 1314x750 centred at (783,444) and swallows the whole upper-middle quadrant this
    // fixture probes, which is why the s50 bench boot read `outside=false` while the QEMU gate (no
    // Kepler takeover, so `wcx::activate` never runs and neither furniture row exists) kept reading
    // `outside=true`. The sibling `clickroute_selftest` was written IN that arc and already finds its
    // desktop point this way; this witness predates it, which is the whole of the difference.
    //
    // Panel corners after the historical diagonal point, so a bare panel still probes the same pixel
    // it always did. Candidates inside the probe box are rejected arithmetically rather than by
    // asking `hit_test`, and the search runs BEFORE the probe rows exist — so the only thing it can
    // reject is a point a REAL window owns, and the leg's own claim (still a miss with both probes
    // up) stays a claim about the probe rows rather than a restatement of the search.
    let diag = (8 * scale + BORDER + 4) as i32;
    let (pw, ph) = (info.width as i32, info.height as i32);
    let in_probe_box = |x: i32, y: i32| {
        x >= bx as i32 && y >= by as i32 && x < (bx + bw) as i32 && y < (by + bh) as i32
    };
    let miss_pt = [
        (ox as i32 + diag, oy as i32 + diag),
        (2, 2),
        (pw - 3, 2),
        (2, ph - 3),
        (pw - 3, ph - 3),
    ]
    .into_iter()
    .find(|&(x, y)| !in_probe_box(x, y) && hit_test(x, y).is_none());

    let s = &raw const HT_SURF as usize;
    let len = core::mem::size_of_val(&HT_SURF);
    // 8x8 ARGB8888, stride 32 BYTES (= 8 px) — the FOCUS-VIS surface geometry exactly. The compositor
    // picks the upscale itself (`place_scale`), which is the scale `spawn_geometry` answered with.
    let wa = create(ASID_A, s, len, 8, 8, 32, b"ht-a");
    let wb = create(ASID_B, s, len, 8, 8, 32, b"ht-b");
    if wa == WIN_NONE || wb == WIN_NONE {
        serial_println!("[clickroute] hit-test -> SKIP (window table full: a={} b={})", wa, wb);
        close(wa);
        close(wb);
        return;
    }
    move_to(wa, ox, oy);
    move_to(wb, ox, oy);

    // Inside the shared content area.
    let (ix, iy) = ((ox + 2) as i32, (oy + 2) as i32);

    let inside = hit_test(ix, iy);
    let inside_ok = inside.is_some();
    let topmost_ok = inside.map(|(_, a, _)| a) == Some(ASID_B);
    focus_changed(ASID_A);
    let raise_ok = hit_test(ix, iy).map(|(_, a, _)| a) == Some(ASID_A);
    // Raising A cannot lower anything, so a point unowned before the probes existed is unowned now
    // unless a probe row claims it — which is exactly the leg. A panel with no unowned point left
    // reports `skip` (the sibling's discipline) rather than a verdict it has no fixture for.
    let outside_ok: Option<bool> = miss_pt.map(|(x, y)| hit_test(x, y).is_none());
    focus_changed(0);
    let hidden_ok = hit_test(ix, iy).is_none();

    // Leg 6 — CLICK-SHELL. Re-raise A (leg 5 left every window under the shell) and give it focus,
    // then drive one PRESS edge through the router with the pointer wherever it actually is. The
    // fixture is only valid if that point hits nothing: `shell` is the verdict, `None` = not asserted.
    let shell: Option<bool> = clickshell_leg(ASID_A, ix, iy, info.width as i32, info.height as i32);

    // Leg 7 — CLICK-SHELL r2 (P72). The same press, from a focus that owns NO window and no compat
    // row. Leg 6 covers the windowed focus and passed on this gate throughout the bench defect; this
    // is the leg that fails without the predicate fix. ASID_C owns nothing by construction.
    let bare: Option<bool> = clickshell_windowless_leg(ASID_C);

    // Leg 8 — CLICK-PLAIN (P75). The HIT arm's policy, which legs 6 and 7 do not touch: a press on an
    // UNFOCUSED window moves focus and is DELIVERED WHOLE to the raised owner, the next press on it is
    // delivered too, and the wake edges run BEFORE the push. Needs a real
    // private-slot ASID — the assertion is about a RING, and only slot ASIDs have one — so it runs
    // last, borrows the highest slot, and hands it back reset. `ASID_A` stands in as the app that
    // held focus before the click.
    let plain: Option<(bool, bool, bool)> = clickplain_leg(PLAIN_ASID, ASID_A, s, len);

    // Leg 9 — CLOSE-BOX (P79). The one action click in the grammar: a press in a window's close
    // box is CONSUMED by the router, and the row is closed. Runs last — it is the only leg that
    // REMOVES a row through the router — and self-cleans through the close itself (the row is the
    // thing being asserted gone). ASID_X owns nothing but the leg's own window, so the router's
    // kill arm is a witnessed no-op.
    let closebox: Option<bool> = closebox_leg(ASID_X, s, len);

    // Leg 10 — CLOSE-FIX (P82). The same close arm, driven against a row the battery created
    // through the ordinary path (`wa`), asserting the ROW NAMED is the row reaped and the settle
    // read-back is the SELFTEST tag — the discriminator that keeps this battery's wire lines
    // distinguishable from a real operator close. Runs after leg 9 (so the probe row is gone) and
    // consumes `wa` through the router on success; the tail's `close(wa)` then no-ops.
    let closereal: Option<bool> = closebox_real_leg(wa, ASID_A);

    let ok = inside_ok
        && topmost_ok
        && raise_ok
        && outside_ok.unwrap_or(true)
        && hidden_ok
        && shell != Some(false)
        && bare != Some(false)
        && plain.map(|(a, b, c)| a && b && c) != Some(false)
        && closebox != Some(false)
        && closereal != Some(false);
    let verdict3 = |v: Option<(bool, bool, bool)>, pick: fn((bool, bool, bool)) -> bool| match v {
        Some(t) => {
            if pick(t) {
                "true"
            } else {
                "false"
            }
        }
        None => "skip",
    };
    let (mx, my) = miss_pt.unwrap_or((-1, -1));
    serial_println!(
        "[clickroute] hit-test at ({},{}) inside={} topmost={} raise={} outside={} miss=({},{}) hidden={} shell={} bare={} hit={} deliver={} wake={} close={} closereal={} -> {}",
        ix, iy, inside_ok, topmost_ok, raise_ok,
        match outside_ok {
            Some(true) => "true",
            Some(false) => "false",
            None => "skip",
        },
        mx, my, hidden_ok,
        match shell { Some(true) => "true", Some(false) => "false", None => "skip" },
        match bare { Some(true) => "true", Some(false) => "false", None => "skip" },
        verdict3(plain, |t| t.0),
        verdict3(plain, |t| t.1),
        verdict3(plain, |t| t.2),
        match closebox { Some(true) => "true", Some(false) => "false", None => "skip" },
        match closereal { Some(true) => "true", Some(false) => "false", None => "skip" },
        if ok { "PASS" } else { "FAIL" }
    );

    close(wa);
    close(wb);
    // CLOSE-FIX (P82) — the teardown WITNESS GUARD: no synthetic row may outlive the battery,
    // and a leak may not be silent. The bench cost of one leaked probe row is a whole boot of
    // polluted hit-tests — a real click resolving to a fixture ASID, an ASID-scoped kill finding
    // nobody, undead `jobs` rows — so the guard is a sweep, not an assertion: every row still owned
    // by one of the battery's synthetic ASIDs is reaped HERE, and the reap prints a FAIL-shaped
    // line the regression spec forbids. Zero rows swept is free (`close_owner` returns before any
    // panel work); the guard's cost exists only in the state it exists to kill.
    let mut leaked = 0usize;
    for a in [ASID_A, ASID_B, ASID_C, ASID_X] {
        leaked += close_owner(a);
    }
    if leaked > 0 {
        serial_println!(
            "[clickroute] hit-test teardown LEAK — {} synthetic row(s) reaped -> FAIL",
            leaked
        );
    }
    // Same restore FOCUS-VIS owes and for the same reasons: drop the shell back to the bottom of the
    // z-order, un-name the synthetic focus owner, and repaint the live set (this selftest's
    // `focus_changed(0)` leg pushed EVERY live window below the shell and consumed its damage flag).
    SHELL_Z.store(0, Ordering::Release);
    FOCUS_ASID.store(0, Ordering::Release);
    repaint();
}

/// CLICK-X86 — the restore every selftest that drives [`focus_changed`] with SYNTHETIC owners owes:
/// drop the shell back to the bottom of the z-order, un-name the focus owner (it must not be left
/// naming an address space that does not exist), and repaint the live set (a `focus_changed(0)` leg
/// pushes every live window below the shell and consumes its damage flag).
///
/// Deliberately the LAST item in this file. It is `witness`-only, so with the knob off it does not
/// compile — but a definition inserted higher up would still renumber every `core::panic::Location`
/// below it, and those records live in the loadable image, not only in DWARF. Appending keeps the
/// knob-off artifact byte-identical on both targets, which is the property `wcg`'s module note
/// claims for the whole witness.
#[cfg(feature = "witness")]
pub fn focus_reset() {
    use core::sync::atomic::Ordering;
    SHELL_Z.store(0, Ordering::Release);
    FOCUS_ASID.store(0, Ordering::Release);
    repaint();
}
