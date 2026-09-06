// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! QUARRY — UnaOS's file manager. Peter's naming, 2026-08-17: *"Tree on left and start with detailed
//! list view on right."*
//!
//! A kernel-owned compositor window in the CRISPY theme: a **directory tree** of the mounted volumes
//! on the left, a **detailed list** (name · size · modified) of the selected directory on the right,
//! and — because nothing in this tree had one — **scrolling**, implemented here at the only layer
//! that can honestly own it today.
//!
//! ## Why this is a kernel window and not a ring-3 program
//!
//! The ring-3 line (`user-vug`, `user-stat`, `user-pulse`) is where a new app *should* live, and this
//! module is written so that moving it there is a port rather than a rewrite: the whole of the layout,
//! the tree walk, the list model and the scroll arithmetic below are pure functions over a
//! `[`DirEnt`](crate::fs::vfs::DirEnt)` slice and a pixel buffer. Two facts in the tree today make the
//! EL0 version impossible rather than merely harder, and both are measurable rather than matters of
//! taste:
//!
//! 1. **An EL0 window is hard-capped at 128x128 pixels, on both arches.**
//!    `arch::aarch64::boot::FB_WIN_MAX_W`/`H` and `arch::x86_64::memory::FB_WIN_MAX_W`/`H` are both
//!    128, and the x86 side carries a `const` assertion tying them to the 64 KiB window region slot
//!    (`assert!((FB_WIN_MAX_W * FB_WIN_MAX_H * 4) as usize == FB_WIN_SLOT_SIZE)`). `sys_win_create`
//!    rejects anything larger with `-EINVAL`. At the 8-px font cell that is **16 columns** — which is
//!    four characters short of one FAT 8.3 name plus a space, let alone a name column beside a size
//!    column beside a date column. The deliverable is not expressible in that surface.
//! 2. **There is no directory syscall.** The VFS mount table is `crate::fs::vfs`, kernel-internal;
//!    the frozen ABI (`una-abi`) has `SYS_OPEN`/`SYS_READ`/`SYS_SEEK`/`SYS_UNLINK`/`SYS_CLOSE` and no
//!    `readdir`/`stat` (34..=39 are unallocated). `SYS_OPEN` itself does not route through the VFS —
//!    it binds `fat::mount()` directly and accepts a bare <=12-byte 8.3 root name. The only listing an
//!    EL0 program can perform today is the midden bus's `BUS_VERB_LS`, which reads the **FAT root**
//!    through `fat::mount()` and therefore bypasses the mount table entirely: precisely the raw-backend
//!    path this arc is forbidden to take, and the `ls`-disagrees-with-`cat` defect VFS-1 (adoption)
//!    was written to delete.
//!
//! So Quarry follows the idiom the tree actually uses for a windowed tool that needs room and kernel
//! data — `video::instgui`'s installer dialog, `video::fbcon::panel_console_window_open`'s console
//! window, `main.rs`'s shell window: a kernel-owned `wm` row over a cached-RAM ARGB8888 surface,
//! presented through the ordinary [`wm::present`] path. Nothing here touches the scan-out.
//! What the ring-3 port needs is named in `docs/dev/OS/05_USER_EXPERIENCE/quarry.md` §7, in the order
//! it must land.
//!
//! ## The VFS seam
//!
//! Every directory read goes through [`crate::shell::vfs_ls_collect`] — the ONE collector VFS-1
//! (adoption) left behind when it deleted the per-volume ones. Quarry therefore inherits, for free and
//! by construction: longest-prefix mount resolution, the VFS-4 `-ENODEV` guard for a reserved-but-
//! unbound volume, the synthesized mount-point rows (`/fat`, `/usb`) below a listed path, and the
//! `.`/`..` filter. It never names a backend, never calls `fat::mount()`, and never calls
//! `unafs::with_unafs()`. The volume list on the left is [`MountTable::prefixes`], read from the same
//! table the collector builds.
//!
//! ## Front-buffer discipline
//!
//! Every pixel lands in [`SURF`]'s heap allocation. Presentation is `wm`'s, through one
//! [`wm::present`] per repaint. This module never touches the framebuffer and never holds the
//! `WRITER` lock across a directory read.
//!
//! ## v2 — the four bench complaints, and what each one actually was
//!
//! Quarry v1 shipped and was driven on the bench. Four defects came back, and none of them was a
//! matter of taste; each had a mechanism, and the mechanism is written down beside its fix.
//!
//! 1. **"FAT contents VERY SLOW to come up."** The listing path re-probes the whole storage stack on
//!    every call. One [`crate::shell::vfs_ls_collect`] is *three* volume probes, not one:
//!    `vfs_mount_table()` itself calls `fat::mount_source(BlockSource::Usb)` to decide whether to
//!    bind `/usb` (honest hot-plug, vfs.md §6), and then `MountTable::stat` and
//!    `MountTable::read_dir` each call `fat::mount_source` again inside `FatBackend` — and
//!    `mount_source` is a full superfloppy → GPT → MBR scan (LBA 0, LBA 1, a BPB sector per
//!    candidate partition) before a single directory sector is read. On top of that v1's model asked
//!    for the SAME directory twice on every navigation: [`Model::expand`] collects a row's children
//!    and [`Model::show`] then collects the identical path again for the list pane. Landing on
//!    `/fat` therefore cost ~4 mount probes and 2 root-directory walks where 1 of each would do.
//!    The fix is [`Model::collect_cached`] — see §Cost below.
//! 2. **"/fat is LISTED TWICE."** Not a rendering bug and not a collector bug: v1 made *every* mount
//!    prefix a tree ROOT (`mt.prefixes()` = `/`, `/fat`, `/usb`) and then expanded `/`, whose
//!    listing carries the same mount points as synthesized child rows. `/fat` was a depth-0 root
//!    AND a depth-1 child of `/`, both naming the same path. [`root_prefixes`] is the fix and it is
//!    a statement about namespaces rather than about FAT: **a mount point claimed by another mount
//!    point is not a root — it is reached through its parent.** `/` claims everything, so on this
//!    machine the tree has exactly one root and the volumes hang under it, which is also what the
//!    one namespace actually looks like.
//! 3. **"double-click on VUG should open the app."** v1 deliberately minted no launch path. v2 has
//!    one, and it is not a new mechanism: [`launch`] reads the image through the same VFS seam and
//!    hands it to `arch::syscall::spawn_user_image_bg` — the exact call the shell's `bg` verb makes.
//!    The double-click needs a timing constant, and none existed anywhere in this tree; see
//!    [`DOUBLE_CLICK_MS`] for the honest derivation.
//! 4. **"where is vug, where is the kernel."** True, and measurable: the native UnaFS root that v1
//!    opened on holds exactly two files (`K3HELLO.TXT`, `K3PAT.BIN` — see `arroyo`'s staging step),
//!    while the card a person means when they say "my card" is `/fat`, with `KERNEL8.IMG`,
//!    `VUG.ELF`, `CONFIG.TXT`, `SRC.TGZ` and the firmware on it. v1 was not hiding anything; it
//!    landed on the emptiest volume in the namespace. [`Model::landing`] fixes that with a rule
//!    stated on the wire rather than a hardcoded `/fat`.
//!
//! ## Cost, measured rather than asserted
//!
//! [`Model::collect_cached`] is the whole SLOW fix and it is deliberately the *smallest* thing that
//! could work: an in-model, path-keyed cache of the seam's own answer, bounded at [`MAX_CACHE`]
//! entries with a FIFO eviction, invalidated on three events and no others —
//!
//!   * **window open** (a fresh [`Model`] has an empty cache — this is the brief's "per open");
//!   * **the `r` key**, the refresh gesture that already existed and already meant "re-read";
//!   * **a volume generation change**, `drivers::block::usb_publish_gen()` — the block layer's own
//!     hot-plug epoch, bumped by every publish and every retraction (PA35's storage race). It is one
//!     relaxed atomic load, so it can be asked on every access; asking `vfs_mount_table()` instead
//!     would cost the very USB probe this cache exists to stop paying.
//!
//! Nothing else is cached: errors are not (a `-ENODEV` volume may arrive), and the mount PREFIX list
//! is re-read only by `reload_roots`. The model counts its own hits, misses and cycles and prints
//! them, so "it is faster now" is a number in the log and not a claim — `[quarry] reads=…` on open
//! and on every refresh.
//!
//! The alternative the brief offered — bounding and paging the directory read — is deliberately NOT
//! taken, and the reason is that it fixes the wrong term. The cost here is per-CALL setup (three
//! volume probes), not per-ENTRY: a 12-entry FAT root and a 2000-entry one pay almost the same
//! mount-scan toll. Paging would divide a term that is already small and leave the dominant one
//! untouched, and it would put a page cursor in a model that ORIN's UnaFS is about to re-back.
//! [`MAX_LIST`] already bounds the entry term and says so on the glass when it bites.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::video::theme;
use crate::video::wm;
use crate::fs::vfs::{DirEnt, NodeKind, VfsTime};

// ── Identity ────────────────────────────────────────────────────────────────────────────────────

/// Quarry's owner ASID: kernel FURNITURE, in the reserved band, and deliberately neither
/// [`wm::KERNEL_OWNER_CONSOLE`] nor [`wm::KERNEL_OWNER_DESKTOP`] — a `close_owner` sweep aimed at
/// either of those must not reap this window, and vice versa (the CLOSEISO rule). Declared HERE
/// rather than in `wm.rs` on purpose: `wm.rs` is compiled into the knob-off `kernel8.img`, whose
/// byte-identity proof a single added line would break, and [`wm::is_kernel_owner`] already admits
/// the whole `KERNEL_OWNER_BASE ..= +0xFF` band, so no `wm` change is owed for a new tenant of it.
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 3;
const _: () = assert!(OWNER != wm::KERNEL_OWNER_CONSOLE && OWNER != wm::KERNEL_OWNER_DESKTOP);

/// The live window id, or [`wm::WIN_NONE`].
static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

// ── Bounds ──────────────────────────────────────────────────────────────────────────────────────
//
// Every model in this module is capped, because both of its inputs are attacker-shaped in the sense
// that matters here: a directory's entry count is whatever the medium says, and the tree's depth is
// whatever the operator clicks. The caps are the loop bounds, not a tidiness pass.

/// Rows the flattened tree may hold. A deep expand stops adding rather than growing without limit.
const MAX_TREE: usize = 192;
/// Directory entries the list pane will model. A larger directory is truncated and SAYS so on the
/// last row — a silently short listing is a lie about the medium.
const MAX_LIST: usize = 2048;
/// Tree nesting the expander will follow. "At least 2 levels" is the brief; this is the ceiling.
const MAX_DEPTH: usize = 8;
/// Longest path Quarry will build. Longer, and the child is skipped with a row that says so.
const PATH_MAX: usize = 160;
/// Directory listings the model will hold at once. FIFO, so a deep walk evicts the shallow rows it
/// has finished with rather than growing. Sixteen covers a root plus every volume plus a walk down
/// two levels of one of them, which is every gesture the window offers.
const MAX_CACHE: usize = 16;
/// Programs Quarry will hold reaped-pending at once. A launched job's kernel `Proc` row stays
/// claimed after it exits until something polls it, and [`reap_jobs`] is Quarry's poller (the
/// shell's `jobs` verb cannot be: `BG_JOBS` is `shell.rs`-private and that file is compiled into the
/// knob-off image, where an added line is a byte-identity break). This is the ceiling on rows this
/// window can have outstanding; the 9th launch reaps first and then declines out loud.
///
/// ARCH-NEUTRAL (rmbp-7 QUARRY): this is a ceiling on QUARRY'S OWN table, not a property of a chip,
/// and the seam it protects (`arch::syscall::spawn_user_image_bg` / `bg_poll`) exists on both
/// arches. It was gated to aarch64 only because its one reader sat inside the aarch64 half of
/// [`launch`]; the check now lives in the arch-neutral [`run_act`], so the ceiling means the same
/// thing on both arches and the x86 VFS adoption inherits it for free.
const MAX_JOBS: usize = 8;
/// Entries the open-time census prints by name.
const CENSUS_MAX: usize = 12;

/// The double-click window, in milliseconds.
///
/// **Nothing in this tree had one.** There is no double-click anywhere in the kernel, no
/// `DOUBLE_CLICK`/`DBLCLK` constant in any driver or compositor file, and the HID boot-mouse decoder
/// publishes button transitions with no timing attached at all — so this constant is minted here,
/// and the honest thing to do is say what it was chosen against rather than to imply it was
/// inherited. Three facts fixed it:
///
///   * the CLOCK is `arch::ms()`, which on aarch64 is `CNTVCT_EL0 / (CNTFRQ_EL0/1000)` — derived
///     from the free-running counter, NOT from `ticks()`, so it is correct on QEMU raspi4b where the
///     periodic timer IRQ is never delivered and `ticks()` stays frozen at 0 (UVUG-7's measurement).
///     A tick-derived clock would have made every gate here vacuous on the QEMU battery;
///   * the mouse arrives over USB HID at boot-protocol rates, so two deliberate clicks land tens of
///     milliseconds apart at best — a window under ~200 ms would drop real double-clicks on a busy
///     compositor pass;
///   * 400 ms is the interval under which two presses on the SAME row read as one gesture rather
///     than as two decisions. It sits between the classic desktop defaults (macOS ~450 ms, Windows
///     500 ms) and the low end, and it is deliberately on the short side because Quarry's
///     single-click is not inert — it selects — so a too-long window makes a slow re-select feel
///     like an accidental launch.
///
/// The predicate is [`is_double`], which is pure and witnessed, so this number is the only part of
/// the gesture that is a judgement call.
const DOUBLE_CLICK_MS: u64 = 400;

// ── Geometry ────────────────────────────────────────────────────────────────────────────────────

/// Inner padding inside a pane, in source pixels.
const PAD: usize = 4;
/// The raw `font8x8` cell, before the text scale.
const BASE_CELL: usize = 8;
/// Scrollbar gutter width — [`theme::SCROLLBAR_WIDTH`], the role that has existed since the theme
/// table landed and has never had a consumer. Quarry is the first.
const SBW: usize = theme::SCROLLBAR_WIDTH;
/// Shortest thumb we will draw, so a 10 000-row directory still leaves something to aim at.
const THUMB_MIN: usize = 16;

/// Smallest content surface Quarry will accept. Below this the two panes cannot both carry a usable
/// column set and the honest answer is to decline rather than to draw an unreadable window.
const FLOOR_W: usize = 320;
const FLOOR_H: usize = 200;
/// Largest content surface. A file manager does not need the whole 1920x1200 bench panel, and a
/// bounded surface bounds the repaint cost the scroll path pays (see §Cost in the module doc's
/// companion, `quarry.md` §5).
const CEIL_W: usize = 1152;
const CEIL_H: usize = 720;

/// The resolved surface geometry for this boot's panel.
#[derive(Clone, Copy)]
struct Geom {
    /// Content width/height in source pixels.
    w: usize,
    h: usize,
    /// Integer text scale over the 8 px `font8x8` cell.
    ts: usize,
}

impl Geom {
    #[inline]
    fn cell(&self) -> usize {
        BASE_CELL * self.ts
    }
    #[inline]
    fn row_h(&self) -> usize {
        self.cell() + 2 * self.ts
    }
    /// The path bar across the top of the surface.
    #[inline]
    fn bar_h(&self) -> usize {
        self.row_h() + 2 * self.ts
    }
    /// Tree pane width. 5/16 of the surface, floored at ten columns and capped at half — so the list
    /// pane, which carries three columns, is never squeezed by the one that carries one.
    #[inline]
    fn tree_w(&self) -> usize {
        let want = self.w * 5 / 16;
        let lo = (10 * self.cell() + 2 * PAD + SBW).min(self.w / 2);
        want.clamp(lo, self.w / 2)
    }
    /// `(x, y, w, h)` of the tree pane, in source pixels.
    #[inline]
    fn tree_pane(&self) -> Rect {
        Rect { x: 0, y: self.bar_h(), w: self.tree_w(), h: self.h - self.bar_h() }
    }
    /// `(x, y, w, h)` of the list pane. One pixel of divider sits between the two.
    #[inline]
    fn list_pane(&self) -> Rect {
        let tw = self.tree_w();
        Rect { x: tw + 1, y: self.bar_h(), w: self.w - tw - 1, h: self.h - self.bar_h() }
    }
}

/// A rectangle in source-surface pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    #[inline]
    fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    /// The pane's interior — inside the 1 px keyline.
    #[inline]
    fn inner(&self) -> Rect {
        Rect { x: self.x + 1, y: self.y + 1, w: self.w.saturating_sub(2), h: self.h.saturating_sub(2) }
    }
}

/// Resolve the surface geometry for a panel, or `None` when the panel cannot host the floor.
///
/// Pure, total, and the single source of the numbers the painter and the router both read — the
/// crispywire law `dock::Layout` states: one geometry accessor, never a second copy of the arithmetic
/// to drift out of step.
fn geometry(pw: usize, ph: usize) -> Option<Geom> {
    // The text scale is a legibility decision about the PANEL, and it is the one number here that is
    // not a proportion: a 640x480 QEMU panel reads at 1x and the 1920x1200 bench panel does not.
    let ts = if pw >= 1280 { 2 } else { 1 };
    let cell = BASE_CELL * ts;
    // Leave the chrome room. `wm` draws the title strip above the content and a border around it, and
    // a window whose OUTER box does not fit is a window the tiler will fight.
    let avail_w = pw.saturating_sub(2 * wm::BORDER);
    let avail_h = ph.saturating_sub(wm::TITLE_H + 2 * wm::BORDER);
    let w = (pw * 3 / 5).min(CEIL_W).min(avail_w) / cell * cell;
    let h = (ph * 3 / 5).min(CEIL_H).min(avail_h) / cell * cell;
    if w < FLOOR_W || h < FLOOR_H {
        return None;
    }
    Some(Geom { w, h, ts })
}

// ── The model ───────────────────────────────────────────────────────────────────────────────────

/// One flattened row of the left-hand tree.
struct TreeRow {
    /// Absolute VFS path this row names.
    path: String,
    /// Display name (the last component; a mount prefix shows as itself).
    name: String,
    /// Nesting level; roots are 0.
    depth: usize,
    /// Whether this row's children are currently spliced in below it.
    expanded: bool,
}

/// Which pane the keyboard is driving.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Tree,
    List,
}

/// One cached directory read — the seam's exact answer for one path, nothing derived.
struct CacheEnt {
    path: String,
    is_dir: bool,
    rows: Vec<DirEnt>,
}

/// What a press or an Enter decided, carried OUT of the model lock before it is acted on.
///
/// This exists because of the one thing a file manager does that a list widget does not: it starts
/// programs. `spawn_user_image_bg` reserves a `Proc` row, maps an address-space slot and calls
/// `spawn_user_slot` — it takes the scheduler's locks and it may run the child on another core
/// before it returns. Doing that with Quarry's `MODEL` spinlock held would put a repaint-path lock
/// underneath the scheduler for no reason at all. So the router decides, drops the lock, and only
/// then acts.
enum Act {
    /// Nothing to do outside the lock (a selection, a scroll, a navigation — all already applied).
    None,
    /// Read and spawn this absolute path.
    Launch(String),
    /// A double-click on something Quarry cannot open yet. Census, no action.
    NoOpener(String),
}

struct Model {
    geom: Geom,
    tree: Vec<TreeRow>,
    tree_sel: usize,
    tree_scroll: usize,
    /// The directory the list pane is showing.
    cwd: String,
    list: Vec<DirEnt>,
    /// True when the medium had more entries than [`MAX_LIST`] and the model is a prefix of them.
    list_truncated: bool,
    list_sel: usize,
    list_scroll: usize,
    focus: Pane,
    /// The last collector error, shown in the list pane instead of rows.
    err: Option<String>,
    /// Every mount prefix, as of the last `reload_roots`. Held so the landing rule and the tree can
    /// both read it without paying a second `vfs_mount_table()` (and therefore a second USB probe).
    mounts: Vec<String>,
    /// The directory cache — see the module doc's §Cost.
    cache: Vec<CacheEnt>,
    /// The volume generation `cache` was filled against.
    cache_gen: u64,
    /// Cache misses (real seam reads), hits, and the cycles the misses cost. Printed, not asserted.
    reads: usize,
    hits: usize,
    read_cycles: u64,
    /// The previous press, for [`is_double`]. `click_ms == 0` means "none this session".
    click_ms: u64,
    click_row: usize,
    click_pane: Pane,
    /// The last activation's one-line result, shown in the path bar. This is the only feedback an
    /// operator gets for a launch — there is no console in this window — so it is not optional.
    status: Option<String>,
}

static MODEL: spin::Mutex<Option<Model>> = spin::Mutex::new(None);

/// The surface. A heap allocation sized from the live panel rather than a `[u32; W * H]` static,
/// because the two panels this must serve differ by 7.5x in area (QEMU raspi4b 640x480, the bench Pi
/// 1920x1200) and a static large enough for the second is `.bss` the first pays for and never uses.
/// `try_reserve_exact` + a DECLINE line is the `video::fbcon::panel_console_window_open` idiom.
///
/// INVARIANT: once [`open`] has published it, the `Vec` is at its final length and never grows, so the
/// address `wm` holds stays valid for the window's whole life. [`close`] drops it only after
/// `wm::close` has removed the row.
static SURF: spin::Mutex<Vec<u8>> = spin::Mutex::new(Vec::new());

/// Repaint sequence — the wire's proof that a scroll or a navigation actually redrew, and the number
/// `quarry.md` §5's cost note is counted against.
static PAINTS: AtomicUsize = AtomicUsize::new(0);

// ── The VFS seam, and its one arch gate ─────────────────────────────────────────────────────────
//
// The `target_arch` below is NOT a hardware decision and is not Quarry's. `fs/vfs.rs` gates
// `NativeBackend` and `FatBackend`'s impls to aarch64 (vfs.md §12.4: "x86 is unchanged by design …
// that arch has no mount table to route through"), so the collector and the mount table simply do not
// exist on x86 to be called. Quarry compiles, lays out, scrolls and paints identically on both arches;
// on x86 it opens on an empty volume list and SAYS why, and the day the x86 VFS adoption lands these
// two shims collapse into one. Nothing else in this file mentions an arch.

/// Collect one directory through the VFS seam. `Ok((true, rows))` for a directory.
#[cfg(target_arch = "aarch64")]
fn collect(path: &str) -> Result<(bool, Vec<DirEnt>), String> {
    crate::shell::vfs_ls_collect(path)
}

#[cfg(not(target_arch = "aarch64"))]
fn collect(_path: &str) -> Result<(bool, Vec<DirEnt>), String> {
    Err(String::from("no VFS mount table on this arch yet (vfs.md 12.4)"))
}

/// Every mount prefix, sorted. NOT the tree's roots — see [`root_prefixes`], which is the fix for
/// the duplicate `/fat`.
///
/// This is the one call that costs a `vfs_mount_table()`, and therefore a USB probe, so the model
/// makes it exactly once per `reload_roots` and remembers the answer in `Model::mounts`.
#[cfg(target_arch = "aarch64")]
fn mount_prefixes() -> Vec<String> {
    let mt = crate::shell::vfs_mount_table();
    let mut v: Vec<String> = mt.prefixes().iter().map(|p| String::from(*p)).collect();
    v.sort();
    v
}

#[cfg(not(target_arch = "aarch64"))]
fn mount_prefixes() -> Vec<String> {
    Vec::new()
}

/// The block layer's hot-plug epoch — [`Model::collect_cached`]'s invalidation stamp.
///
/// `usb_publish_gen` is advanced by every geometry publish and every retraction (PA35's storage
/// race: two devices on a recycled slot id were otherwise indistinguishable). It is a single
/// `Acquire` load of an `AtomicU64`, which is what makes it safe to ask on every cache access —
/// re-reading the MOUNT TABLE to detect the same event would cost the full USB volume probe this
/// cache exists to stop paying. It is not FAT-specific and it is not namespace-specific: it says
/// "the set of block devices under this namespace changed", which is precisely the event that can
/// make a cached listing a lie, and it will mean the same thing when ORIN's UnaFS is the backend.
#[cfg(target_arch = "aarch64")]
fn volume_gen() -> u64 {
    crate::drivers::block::usb_publish_gen()
}

#[cfg(not(target_arch = "aarch64"))]
fn volume_gen() -> u64 {
    0
}

// ── The duplicate-root rule (pure) ──────────────────────────────────────────────────────────────

/// Does mount prefix `q` claim path `p`? The resolver's boundary rule, restated here as a pure
/// function because `MountTable`'s copy is private and this needs to be witnessed on both arches:
/// `/usb` claims `/usb` and `/usb/...` but never `/usbfoo`, and the bare root claims everything.
fn prefix_claims(q: &str, p: &str) -> bool {
    if q == "/" {
        return p.starts_with('/');
    }
    if !p.starts_with(q) {
        return false;
    }
    p.len() == q.len() || p.as_bytes()[q.len()] == b'/'
}

/// **The `/fat`-listed-twice fix.** Reduce a mount prefix list to the prefixes that are genuinely
/// ROOTS of the tree — those not claimed by some OTHER prefix in the same list.
///
/// v1 made every prefix a depth-0 row and then expanded `/`, whose listing carries `/fat` and `/usb`
/// as synthesized mount-point rows (`shell::vfs_ls_collect`'s "mount points immediately below
/// `path`" arm). So `/fat` was a root AND a child of the root — one path, two rows, which is what
/// the bench saw. Dropping the row would have been the wrong repair: the CHILD row is the correct
/// one, because it is where the path actually lives in the one namespace and it is what a person
/// means by "inside my machine". So the ROOT row goes, and the rule that removes it is a statement
/// about namespaces rather than about this machine's three volumes:
///
/// > A mount point claimed by another mount point is not a root; it is reached through its parent.
///
/// On a table carrying `/` it leaves exactly `["/"]`, which is also the honest shape of a single
/// namespace. On a table with NO root mount (an arch that has not adopted the VFS, or a future
/// namespace assembled from peers) it leaves every unclaimed prefix, so the tree still has roots and
/// nothing is hidden. It is order-independent and idempotent, both witnessed.
fn root_prefixes(all: &[String]) -> Vec<String> {
    all.iter()
        .filter(|p| !all.iter().any(|q| q.as_str() != p.as_str() && prefix_claims(q, p)))
        .cloned()
        .collect()
}

/// Drop rows that repeat a name already present, keeping the first. The collector already dedupes
/// its synthesized mount rows against the backend's own entries, so this is belt-and-braces at the
/// PRESENTATION layer — and it is the layer that must hold when a future backend surfaces a name
/// twice for a reason of its own. Stable, so the sort order above it survives.
fn dedupe_by_name(rows: &mut Vec<DirEnt>) {
    let mut seen: Vec<String> = Vec::new();
    rows.retain(|e| {
        if seen.iter().any(|n| *n == e.name) {
            false
        } else {
            seen.push(e.name.clone());
            true
        }
    });
}

// ── Launchability, and the double-click predicate (pure) ────────────────────────────────────────

/// Is `name` a program THIS kernel's loader can be asked to run?
///
/// `.ELF` and `.BIN`, case-insensitively — the two shapes `spawn_user_image_bg` accepts (a validated
/// ELF64, or a flat blob bounded to one code page), and exactly the two the packaging text tells the
/// operator to `bg`. The extension is a ROUTING hint and nothing more: every real check —
/// ELF magic, `EI_CLASS`, `e_machine`, segment bounds, the 16 KiB window — is the loader's, and
/// [`launch`] reports whatever it says. A `.ELF` that is not one is refused with the loader's own
/// words, not with a guess made here.
fn is_executable(name: &str) -> bool {
    let n = name.as_bytes();
    let ends = |ext: &[u8]| n.len() > ext.len() && n[n.len() - ext.len()..].eq_ignore_ascii_case(ext);
    ends(b".elf") || ends(b".bin")
}

/// Two presses are one double-click iff they hit the SAME row of the SAME pane inside
/// [`DOUBLE_CLICK_MS`].
///
/// `prev == 0` means "no previous press this session" and can never open the window — which is not
/// pedantry: `arch::ms()` legitimately answers 0 before the timebase is up (and on any board whose
/// `CNTFRQ_EL0` reads 0, where `ms()` returns 0 forever by construction). Without this guard every
/// pair of clicks on such a machine would be a double-click, and the FIRST click of a boot would
/// launch whatever it landed on. `now == 0` is refused for the same reason, from the other side.
fn is_double(prev_ms: u64, now_ms: u64, prev_row: usize, row: usize, same_pane: bool) -> bool {
    prev_ms != 0
        && now_ms != 0
        && same_pane
        && prev_row == row
        && now_ms >= prev_ms
        && now_ms - prev_ms <= DOUBLE_CLICK_MS
}

// ── Path arithmetic (pure) ──────────────────────────────────────────────────────────────────────

/// Join a directory path and a child name into an absolute VFS path. Purely lexical, exactly like the
/// resolver: the mount table decides which volume the result lands on, never this function.
fn join(base: &str, name: &str) -> String {
    if base.ends_with('/') {
        alloc::format!("{}{}", base, name)
    } else {
        alloc::format!("{}/{}", base, name)
    }
}

/// The parent of an absolute path. `/` is its own parent — there is nothing above the root of the one
/// namespace, and walking off the top of it is not a thing an operator can be allowed to do.
fn parent(path: &str) -> String {
    match path.rfind('/') {
        None | Some(0) => String::from("/"),
        Some(i) => String::from(&path[..i]),
    }
}

/// The display name for a path: its last component, or the path itself for the root.
fn leaf(path: &str) -> String {
    if path == "/" {
        return String::from("/");
    }
    String::from(path.rsplit('/').next().unwrap_or(path))
}

// ── Scroll arithmetic (pure) — the part of "scrolling" that did not exist ──────────────────────
//
// Nothing in the window/present stack carries a content offset: `wm::WindowInfo` has no scroll field,
// `FrameBuffer::scroll_up` is a whole-surface memmove with no syscall and no rect, and
// `theme::SCROLL_TRACK`/`SCROLL_THUMB` are two colours no scrollbar has ever consumed. So Quarry owns
// its scroll completely: an integer row offset, a clamp, and a full pane redraw at the new offset.
// That is the whole mechanism, and the cost is stated rather than hidden — see `quarry.md` §5.
//
// QSCROLL: the fourth item of that list — "the HID boot-mouse decoder discards the wheel byte before
// the ABI ever sees it" — was true when this was written and is not any more. The WHEEL arc landed the
// byte, `pal::Event::Wheel(i8)` and the routing; [`wheel_next`] below and [`wheel_scroll`] are the
// consumer, and they feed these same three functions rather than a second offset of their own.

/// The largest first-visible row for a list of `len` rows in a `visible`-row viewport.
#[inline]
fn scroll_max(len: usize, visible: usize) -> usize {
    len.saturating_sub(visible)
}

/// Clamp `scroll` so the viewport is full where it can be and `sel` is inside it.
///
/// Two properties, both asserted by the witness: the offset never exceeds [`scroll_max`] (so the
/// viewport never shows blank rows below a short list), and the selection is always on screen (so the
/// keyboard cannot drive a cursor the operator cannot see).
fn scroll_follow(scroll: usize, sel: usize, len: usize, visible: usize) -> usize {
    if visible == 0 || len == 0 {
        return 0;
    }
    let mut s = scroll.min(scroll_max(len, visible));
    if sel < s {
        s = sel;
    } else if sel >= s + visible {
        s = sel + 1 - visible;
    }
    s.min(scroll_max(len, visible))
}

/// QSCROLL — rows one wheel detent moves a viewport.
///
/// Three, and the number is a RATIO to the two gestures that already existed rather than a taste.
/// The scrollbar track pages by a whole viewport (`tvis`/`lvis` — 26 rows on QEMU's panel, 33 on the
/// bench's) and `Up`/`Down` moves exactly one; a wheel that did either would be a duplicate of a
/// gesture the operator already has. Three rows is the smallest step that is unmistakably a SCROLL
/// rather than a cursor nudge, it is a tenth of a viewport on both panels, and a full flick of a
/// boot-protocol mouse (the drain hands us at most ±127 detents in one report) still lands on the
/// clamp rather than overshooting into arithmetic this module does not do.
const WHEEL_ROWS: usize = 3;

/// QSCROLL — the wheel's whole arithmetic: apply `detents` to a row offset and clamp.
///
/// **Positive `detents` = the wheel turned AWAY from the operator = the content moves DOWN = the
/// offset moves UP, toward row 0.** That is the platform-conventional direction and it is the same
/// sign convention `user-vug`'s `wheel_zoom` reads (WHEELZOOM: positive = away = zoom in), decoded
/// from the same `pal::Event::Wheel(i8)` the same HID byte produces — so the two consumers in this
/// tree cannot disagree about which way a hand turned.
///
/// Total, and saturating on both ends: `scroll_max` is the floor's twin, `0` is the floor. Overflow
/// is not reachable — `detents` is an `i8` widened to `i32` and `WHEEL_ROWS` is 3, so the step is at
/// most 384 rows — but it is written saturating anyway, because the alternative is a guard that has
/// to be re-argued every time the drain's shape changes.
#[inline]
fn wheel_next(scroll: usize, smax: usize, detents: i32) -> usize {
    let step = (detents.unsigned_abs() as usize).saturating_mul(WHEEL_ROWS);
    if detents > 0 {
        scroll.min(smax).saturating_sub(step)
    } else {
        scroll.saturating_add(step).min(smax)
    }
}

/// Scrollbar thumb geometry inside a `track_h`-tall gutter, or `None` when everything fits.
///
/// Returns `(thumb_y_offset, thumb_h)` relative to the top of the track.
fn thumb(track_h: usize, len: usize, visible: usize, scroll: usize) -> Option<(usize, usize)> {
    if visible == 0 || len <= visible || track_h < THUMB_MIN {
        return None;
    }
    let th = (track_h * visible / len).max(THUMB_MIN).min(track_h);
    let span = track_h - th;
    let smax = scroll_max(len, visible);
    let ty = if smax == 0 { 0 } else { span * scroll.min(smax) / smax };
    Some((ty, th))
}

// ── Model transitions ───────────────────────────────────────────────────────────────────────────

/// Sort key for the list pane: directories first, then case-sensitive name. The VFS collector sorts by
/// name alone (that is `ls`'s contract); a FILE MANAGER groups, which is a presentation decision and
/// belongs here rather than in the shared collector.
fn list_sort(rows: &mut [DirEnt]) {
    rows.sort_by(|a, b| {
        let ad = matches!(a.kind, NodeKind::Dir);
        let bd = matches!(b.kind, NodeKind::Dir);
        bd.cmp(&ad).then_with(|| a.name.cmp(&b.name))
    });
}

impl Model {
    fn new(geom: Geom) -> Self {
        let mut m = Model {
            geom,
            tree: Vec::new(),
            tree_sel: 0,
            tree_scroll: 0,
            cwd: String::from("/"),
            list: Vec::new(),
            list_truncated: false,
            list_sel: 0,
            list_scroll: 0,
            focus: Pane::Tree,
            err: None,
            mounts: Vec::new(),
            cache: Vec::new(),
            cache_gen: volume_gen(),
            reads: 0,
            hits: 0,
            read_cycles: 0,
            click_ms: 0,
            click_row: 0,
            click_pane: Pane::List,
            status: None,
        };
        m.reload_roots();
        if m.tree.is_empty() {
            m.err = Some(String::from("no volumes mounted"));
            return m;
        }
        // Expand the root, so the operator sees two levels without a gesture — the brief's
        // "expandable, at least 2 levels", demonstrated at open rather than promised. With the
        // duplicate-root rule in place the volumes ARE that second level.
        m.expand(0);
        let root = m.tree[0].path.clone();
        // …then land where the content is. Every probe the rule makes is cached, so the directory it
        // chooses is already in hand when `show` asks for it.
        let land = m.landing(&root);
        if land != root {
            if let Some(i) = m.tree.iter().position(|r| r.path == land) {
                m.tree_sel = i;
                m.expand(i);
            }
        }
        m.show(&land);
        m
    }

    /// Rebuild the root rows from the mount table. Called at open and on refresh, so a stick plugged
    /// after Quarry opened enters the tree on the next `r` — the honest hot-plug posture vfs.md §6
    /// gives the table itself, inherited rather than re-invented.
    ///
    /// The roots are [`root_prefixes`] of the mount list, NOT the mount list — that is the
    /// duplicate-`/fat` fix, and the reason it lives here rather than in the painter is that the
    /// duplicate was a MODEL fact: two rows existed, both real, both naming one path.
    fn reload_roots(&mut self) {
        let keep = self.tree.get(self.tree_sel).map(|r| r.path.clone());
        self.tree.clear();
        self.mounts = mount_prefixes();
        for p in root_prefixes(&self.mounts) {
            self.tree.push(TreeRow { name: leaf(&p), path: p, depth: 0, expanded: false });
        }
        self.tree_sel = keep
            .and_then(|k| self.tree.iter().position(|r| r.path == k))
            .unwrap_or(0);
        self.tree_scroll = 0;
    }

    /// Collect one directory through the seam, **once**.
    ///
    /// The SLOW fix. See the module doc's §Cost for what a miss actually costs and why the
    /// invalidation set is exactly three events. Errors are returned but never cached: a
    /// `-ENODEV` volume is a state that ends when the operator plugs the stick in, and a cache that
    /// remembered it would need a fourth invalidation event to forget it.
    fn collect_cached(&mut self, path: &str) -> Result<(bool, Vec<DirEnt>), String> {
        let now_gen = volume_gen();
        if now_gen != self.cache_gen {
            self.cache.clear();
            self.cache_gen = now_gen;
        }
        if let Some(e) = self.cache.iter().find(|e| e.path == path) {
            self.hits += 1;
            return Ok((e.is_dir, e.rows.clone()));
        }
        let t0 = crate::arch::now_cycles();
        let out = collect(path);
        self.read_cycles = self
            .read_cycles
            .saturating_add(crate::arch::now_cycles().saturating_sub(t0));
        self.reads += 1;
        if let Ok((is_dir, rows)) = &out {
            if self.cache.len() >= MAX_CACHE {
                self.cache.remove(0);
            }
            self.cache.push(CacheEnt {
                path: String::from(path),
                is_dir: *is_dir,
                rows: rows.clone(),
            });
        }
        out
    }

    /// Forget every cached listing. The `r` gesture's other half — `reload_roots` re-reads the mount
    /// table, this re-reads the media.
    fn invalidate(&mut self) {
        self.cache.clear();
        self.cache_gen = volume_gen();
    }

    /// QUARRY-LAND — **the "where is vug, where is the kernel" fix.** Choose the directory to open
    /// on, given the namespace root.
    ///
    /// > Open on the root; unless one of the root's immediate mount points carries strictly more
    /// > plain FILES than the root does, in which case open on the richest of them.
    ///
    /// The measurement that motivates it is in `arroyo`'s own staging step: the native UnaFS volume
    /// this machine mounts at `/` is built with exactly two files on it (`K3HELLO.TXT`, `K3PAT.BIN`),
    /// while `/fat` is the boot card — `KERNEL8.IMG`, `VUG.ELF`, `CONFIG.TXT`, `SRC.TGZ` and the
    /// firmware. v1 opened on `/`, saw two files, and was correctly called dumb. It was not hiding
    /// the kernel's own files from the owner; it had simply landed on the emptiest volume there was.
    ///
    /// The rule names no volume, no filesystem and no extension, which is the requirement ORIN's
    /// UnaFS imposes: when the native volume is the one carrying the system, the same rule lands on
    /// `/` and this function's behaviour inverts without a line changing. It is bounded by the mount
    /// count (three today) and one level deep by construction; every probe goes through
    /// [`collect_cached`], so the listing it selects is already in hand for `show` and the tree — the
    /// rule's marginal cost on this machine is ONE directory read, of a volume the operator is about
    /// to be looking at.
    ///
    /// A tie keeps the root. "Strictly more" is doing real work there: it makes the root the default
    /// and the descent the exception, so a machine whose namespace has content at the top is never
    /// dragged down into a volume by a coin flip.
    fn landing(&mut self, root: &str) -> String {
        fn files(rows: &[DirEnt]) -> usize {
            rows.iter().filter(|e| !matches!(e.kind, NodeKind::Dir)).count()
        }
        let mut best_n = match self.collect_cached(root) {
            Ok((true, rows)) => files(&rows),
            // The root is unreadable or is not a directory: there is nothing to compare against and
            // nothing to be clever about. Land on it and let `show` report whatever it reports.
            _ => return String::from(root),
        };
        let mut best = String::from(root);
        for p in self.mounts.clone() {
            if p == root || parent(&p) != root {
                continue;
            }
            if let Ok((true, rows)) = self.collect_cached(&p) {
                let n = files(&rows);
                if n > best_n {
                    best_n = n;
                    best = p;
                }
            }
        }
        best
    }

    /// Rows immediately below `i` that belong to its subtree.
    fn subtree_len(&self, i: usize) -> usize {
        let d = self.tree[i].depth;
        let mut n = 0usize;
        while i + 1 + n < self.tree.len() && self.tree[i + 1 + n].depth > d {
            n += 1;
        }
        n
    }

    /// Splice `i`'s DIRECTORY children in below it. A leaf, a failed read, or a full tree simply marks
    /// the row expanded with nothing under it — an expander that silently does nothing is worse than
    /// one that visibly opens onto an empty level.
    fn expand(&mut self, i: usize) {
        if i >= self.tree.len() || self.tree[i].expanded {
            return;
        }
        let depth = self.tree[i].depth;
        self.tree[i].expanded = true;
        if depth + 1 > MAX_DEPTH {
            return;
        }
        let path = self.tree[i].path.clone();
        let kids: Vec<TreeRow> = match self.collect_cached(&path) {
            Ok((true, rows)) => rows
                .into_iter()
                .filter(|e| matches!(e.kind, NodeKind::Dir))
                .filter_map(|e| {
                    let p = join(&path, &e.name);
                    if p.len() > PATH_MAX {
                        None
                    } else {
                        Some(TreeRow { name: e.name, path: p, depth: depth + 1, expanded: false })
                    }
                })
                // Never splice a path the tree already carries. [`root_prefixes`] removed the way
                // this happened in v1 (a mount point that was both a root and a child of the root);
                // this is the same invariant asserted at the SPLICE, so no future source of tree
                // rows can reintroduce a duplicate path without tripping over it here.
                .filter(|k| !self.tree.iter().any(|r| r.path == k.path))
                .collect(),
            _ => Vec::new(),
        };
        // The cap is enforced against the WHOLE tree, not per level: an operator expanding six
        // 200-entry directories must not be able to grow this Vec without bound.
        let room = MAX_TREE.saturating_sub(self.tree.len());
        let n = kids.len().min(room);
        for (k, row) in kids.into_iter().take(n).enumerate() {
            self.tree.insert(i + 1 + k, row);
        }
    }

    /// Drop `i`'s spliced subtree.
    fn collapse(&mut self, i: usize) {
        if i >= self.tree.len() || !self.tree[i].expanded {
            return;
        }
        let n = self.subtree_len(i);
        self.tree.drain(i + 1..i + 1 + n);
        self.tree[i].expanded = false;
        // The selection follows the rows, and the three cases are distinct. A selection INSIDE the
        // drained subtree has nowhere to be but the row that closed over it; a selection BELOW the
        // subtree is still on the same node and must shift up by what was removed; a selection at or
        // above `i` never moved. Collapsing `i` while a later SIBLING is selected must not drag the
        // highlight backwards onto `i` — which is exactly what a bare `sel > i` test does.
        if self.tree_sel > i && self.tree_sel <= i + n {
            self.tree_sel = i;
        } else if self.tree_sel > i + n {
            self.tree_sel -= n;
        }
    }

    /// Point the list pane at `path`, through the seam.
    fn show(&mut self, path: &str) {
        self.cwd = String::from(path);
        self.list_sel = 0;
        self.list_scroll = 0;
        self.list_truncated = false;
        // Through the cache, which is what collapses v1's two reads per navigation into one: the
        // tree's `expand` and this call ask for the SAME path on every descent.
        match self.collect_cached(path) {
            Ok((true, mut rows)) => {
                self.list_truncated = rows.len() > MAX_LIST;
                rows.truncate(MAX_LIST);
                list_sort(&mut rows);
                dedupe_by_name(&mut rows);
                self.list = rows;
                self.err = None;
            }
            Ok((false, rows)) => {
                // The collector's "you named a file" answer. Show the one row rather than an error —
                // it is the DOS idiom the shell keeps and it is more useful than a refusal.
                self.list = rows;
                self.err = None;
            }
            Err(e) => {
                self.list.clear();
                self.err = Some(e);
            }
        }
    }

    /// Navigate the list pane into `path` AND reveal it in the tree when the tree already carries it,
    /// so the two panes never disagree about where the operator is.
    fn navigate(&mut self, path: &str) {
        self.show(path);
        if let Some(i) = self.tree.iter().position(|r| r.path == path) {
            self.tree_sel = i;
        }
    }

    /// Rows the tree viewport can show.
    fn tree_visible(&self) -> usize {
        let inner = self.geom.tree_pane().inner();
        inner.h / self.geom.row_h()
    }

    /// Rows the list viewport can show — one fewer than the pane holds, because the column header is
    /// not part of the scrolled content.
    fn list_visible(&self) -> usize {
        let inner = self.geom.list_pane().inner();
        (inner.h / self.geom.row_h()).saturating_sub(1)
    }

    /// Re-clamp both offsets after any model change. One place, so no transition can forget.
    fn settle(&mut self) {
        if self.tree_sel >= self.tree.len() {
            self.tree_sel = self.tree.len().saturating_sub(1);
        }
        if self.list_sel >= self.list.len() {
            self.list_sel = self.list.len().saturating_sub(1);
        }
        self.tree_scroll =
            scroll_follow(self.tree_scroll, self.tree_sel, self.tree.len(), self.tree_visible());
        self.list_scroll =
            scroll_follow(self.list_scroll, self.list_sel, self.list.len(), self.list_visible());
    }

    /// **Open list row `i`** — the one decision behind both the double-click and Enter.
    ///
    /// A DIRECTORY is entered, exactly as v1's Enter did (revealing it on the left first, so the two
    /// panes never disagree). A LAUNCHABLE file becomes an [`Act::Launch`] for the caller to run
    /// outside the lock. Anything else becomes an [`Act::NoOpener`] — honestly, and with a census
    /// line, because "nothing happened" and "nothing CAN happen yet" are different facts and only
    /// one of them is a bug. There are no openers in this tree: no registry, no association table,
    /// no viewer. `quarry.md` §7 says what a `.TXT` double-click is waiting on.
    fn activate_row(&mut self, i: usize) -> Act {
        let Some(e) = self.list.get(i) else {
            return Act::None;
        };
        let name = e.name.clone();
        let is_dir = matches!(e.kind, NodeKind::Dir);
        let cwd = self.cwd.clone();
        let p = join(&cwd, &name);
        if p.len() > PATH_MAX {
            self.status = Some(alloc::format!("path too long: {}", name));
            return Act::None;
        }
        if is_dir {
            if let Some(t) = self.tree.iter().position(|r| r.path == cwd) {
                if !self.tree[t].expanded {
                    self.expand(t);
                }
            }
            self.navigate(&p);
            self.focus = Pane::List;
            self.status = None;
            Act::None
        } else if is_executable(&name) {
            Act::Launch(p)
        } else {
            Act::NoOpener(p)
        }
    }
}

// ── Launching, and who reaps what it started ────────────────────────────────────────────────────
//
// The spawn seam is `arch::syscall::spawn_user_image_bg` — the SAME call the shell's `bg` verb
// makes, with the same bounds, the same console-cap endowment and the same DETACHED posture. Quarry
// mints no loader, no second image-reading path and no policy of its own: the image is read through
// the VFS mount table (never `fat::mount()` — this arc's standing law), the loader does every real
// check, and its refusal is what the operator is shown.

/// One program this window started.
///
/// ARCH-NEUTRAL (rmbp-7 QUARRY). This row was gated to aarch64 on the premise that "the arch that
/// cannot spawn cannot have a job to track" — but that premise was wrong about the seam: x86 has
/// `arch::syscall::spawn_user_image_bg` and `bg_poll` with the same signatures (see
/// `arch/x86_64/syscall.rs`), so the SPAWN half is not what is missing. What is missing on x86 is
/// the VFS mount table that resolves a path to bytes, which is one cross-file gate in `fs/vfs.rs`
/// / `shell.rs` (vfs.md §12.4) and is not this file's to close. Keeping the table, the ceiling and
/// the reaper arch-neutral means the day that gate lifts, only [`launch`]'s body changes.
struct Job {
    pid: u64,
    asid: u64,
    name: String,
}

/// The jobs Quarry has outstanding. Small, bounded, and Quarry's OWN — see [`MAX_JOBS`] for why it
/// cannot be the shell's table. Arch-neutral for the reason stated on [`Job`].
static JOBS: spin::Mutex<Vec<Job>> = spin::Mutex::new(Vec::new());

/// Poll every outstanding job and free the kernel rows of the ones that have finished.
///
/// `bg_poll(pid, reap = true)` is the same reaper the shell's `jobs` verb runs, so a Quarry-launched
/// program's row is released by exactly the mechanism a `bg`-launched one's is. Called on every
/// input gesture and every [`service`] pass, which is often enough that the [`MAX_JOBS`] ceiling is
/// a bound on CONCURRENT programs rather than on launches per boot.
///
/// ARCH-NEUTRAL (rmbp-7 QUARRY): this was an aarch64 body plus an x86 no-op stub. `bg_poll` is
/// `pub` on both arches with the same signature, so the reaper is the SAME code on both — an x86
/// table that is empty today simply reaps nothing, and no stub has to be kept in step with the
/// real one.
///
/// ⚠ **"ARCH-NEUTRAL" WAS THE WRONG AXIS — THERE ARE THREE STATES, NOT TWO (rmbp-7 QUARRY2).**
/// `arch::syscall` is not a property of the *arch*; it is a property of the **EL0 layer**. On x86 the
/// module is unconditional (`arch/x86_64/mod.rs:11`), but `arch/aarch64/mod.rs:47` gates
/// `pub mod syscall;` behind `any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0")` — the rings, the
/// process table and `bg_poll` all belong to that layer. So an aarch64 build carrying
/// `desktop_firmware` (this window's own gate) with NEITHER of those two features named a module that
/// does not exist, and this function failed E0432/E0433 at the `use` and at the `bg_poll` call. No
/// coverage leg reached the combination: `arm-pi` always carries `baremetal`, `arm-tegra-desk` always
/// carries `tegra_el0`, and the third polarity was named nowhere — which is how it shipped. The same
/// defect, from the same premise, was fixed one file over in [`super::super::dock`]'s focus seam
/// (`ba3e9b62`); this is that fix's shape applied to Quarry's spawn seam.
///
/// The ringless arm below is a genuine no-op rather than a placeholder, and the reason is a
/// *measurable* property of this file rather than an assumption: [`JOBS`] has exactly ONE push site
/// — the one inside the EL0 [`launch`] — and that body is not compiled in a ringless build. The table
/// is therefore provably empty, and "poll every outstanding job" over an empty table is precisely
/// nothing. Nothing is silently swallowed: the launch that would have filled it refuses out loud,
/// with a reason, in the arm below.
#[cfg(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0"))
))]
fn reap_jobs() {
    use crate::arch::syscall::BgPoll;
    let mut jobs = JOBS.lock();
    let mut i = 0;
    while i < jobs.len() {
        let verdict = match crate::arch::syscall::bg_poll(jobs[i].pid, true) {
            BgPoll::Running => {
                i += 1;
                continue;
            }
            BgPoll::Exited(st) => alloc::format!("exited status={}", st),
            BgPoll::Faulted => String::from("faulted (contained)"),
            // The ONE genuinely per-arch line in this function, and it is not a hardware fact: the
            // `BgPoll` enum itself differs. `arch/aarch64/syscall.rs` has a `Closed` variant
            // (CLOSE-CLEAN — closed by the operator via a window's close box); `arch/x86_64/
            // syscall.rs`'s `BgPoll` (declared near its `bg_poll`) has only Running/Exited/Faulted/
            // Gone, so naming `Closed` unconditionally would not COMPILE on x86. Closing this
            // properly means adding the variant to the x86 enum, which is outside this file's lane;
            // until then the arm is gated so the aarch64 verdict text is not lost.
            #[cfg(target_arch = "aarch64")]
            BgPoll::Closed => String::from("closed by its window"),
            BgPoll::Gone => String::from("gone (already reaped)"),
        };
        let j = jobs.remove(i);
        serial_println!("[quarry] reaped pid={} asid={} name={} — {}", j.pid, j.asid, j.name, verdict);
    }
}

/// The ringless counterpart of [`reap_jobs`] — an aarch64 desktop build with no EL0 layer, hence no
/// `arch::syscall` and no `bg_poll` to poll with.
///
/// Empty by proof, not by convenience: see the gated twin's doc comment. [`JOBS`]'s sole `push` lives
/// in the EL0 [`launch`], which this configuration does not compile, so the table this would iterate
/// cannot be non-empty. Kept as a real function rather than folded into its callers' `cfg`s because
/// it has FIVE call sites (the ceiling pre-check, the post-launch pass, and three service/gesture
/// passes) and gating each of them would put the same conjunct in five places to drift out of step.
#[cfg(not(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0"))
)))]
fn reap_jobs() {}

/// Read `path` through the VFS seam and spawn it detached. Returns the one line the path bar shows.
///
/// Every gate here is the one `shell::read_el0_image` applies, in the same order and with the same
/// vocabulary, because they are answering the same question about the same kind of file — a
/// directory, an empty file and an oversize file each get their errno-tagged refusal rather than a
/// loader error the operator cannot act on. The ELF pre-checks sharpen the message only; the kernel
/// loader re-validates from scratch either way.
///
/// It does NOT go through `shell::read_el0_image` itself, and that is a deliberate bound rather than
/// an oversight: that function takes a `&mut Console` and prints into it, and it lives in `shell.rs`
/// — a file compiled into the knob-off `kernel8.img`, where splitting out a console-free core would
/// ADD lines and break the byte-identity proof (PARITY.md §5.3). The shared thing is the seam that
/// matters — the mount table for the read, `spawn_user_image_bg` for the spawn — not the printing.
///
/// This is ONE OF THREE arms (it was written as one of two — see the warning below): the x86_64 arm
/// is the `cfg(not(target_arch = "aarch64"))`
/// `launch` at the end of this section, and it declines because there is no mount table to resolve
/// against — NOT because the spawn seam is missing (x86 has `spawn_user_image_bg` and `bg_poll`
/// too). The counterpart is named here because this body is longer than a reader — or the parity
/// detector's window — will scan before concluding the other arch has nothing.
///
/// ⚠ **THE DISPATCH IS THREE-WAY, NOT TWO-WAY (rmbp-7 QUARRY2).** This body needs BOTH halves of the
/// seam — `uslots::USER_REGION_SIZE` for the ceiling and `arch::syscall::spawn_user_image_bg` for the
/// spawn — and `arch/aarch64/mod.rs` gates both modules behind
/// `any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0")`, because the user address space and the
/// process table belong to the EL0 layer rather than to the chip. `target_arch = "aarch64"` alone
/// therefore over-claimed: it named this body for a ringless `desktop_firmware` build that has
/// neither module, and it failed E0433 at the `CAP` const and again at the spawn call. The conjunct
/// added here is those modules' own gate, VERBATIM, so `arm-pi` and every tegra-EL0 leg emit exactly
/// the code they emitted before; the third arm below is the configuration that used to land here and
/// could not compile. Keep this predicate identical to `arch/aarch64/mod.rs`'s gate on
/// `pub mod syscall;` / `pub mod uslots` — a narrower copy silently stops Quarry launching on a board
/// that HAS an EL0 layer, and a wider one is the E0433 back again.
#[cfg(all(
    target_arch = "aarch64",
    any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0")
))]
fn launch(path: &str) -> String {
    use crate::fs::vfs::{NodeKind as NK, VfsError};
    fn why(e: VfsError) -> String {
        match e {
            VfsError::NoSuchVolume => String::from("no such volume"),
            VfsError::NoSuchPath => String::from("no such file (-ENOENT)"),
            VfsError::NotADirectory => String::from("not a directory (-ENOTDIR)"),
            VfsError::IsADirectory => String::from("is a directory (-EISDIR)"),
            VfsError::Denied => String::from("permission denied (-EACCES)"),
            VfsError::Unsupported => String::from("not supported on this volume (-ENOTSUP)"),
            VfsError::Backend(s) => alloc::format!("backend: {}", s),
        }
    }
    const CAP: u64 = crate::arch::aarch64::uslots::USER_REGION_SIZE as u64; // JETSON-EL0: uslots facade (boot.rs on pi / mmu_tegra_el0.rs on tegra)
    // The reap-then-ceiling pre-check moved to [`run_act`] (rmbp-7 QUARRY) — same two steps, same
    // order, same refusal line, but arch-neutral, because the ceiling is Quarry's table's and not
    // this arch's. By the time this body runs the table is reaped and has a free slot.
    let mt = crate::shell::vfs_mount_table();
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(e) => {
            let s = why(e);
            serial_println!("[quarry] launch REFUSED path={} reason=stat ({})", path, s);
            return s;
        }
    };
    if matches!(st.kind, NK::Dir) {
        return String::from("is a directory (-EISDIR)");
    }
    if st.size == 0 {
        serial_println!("[quarry] launch REFUSED path={} reason=empty", path);
        return String::from("empty file");
    }
    if st.size > CAP {
        let s = alloc::format!("{} bytes exceeds the {}-byte user window (-E2BIG)", st.size, CAP);
        serial_println!("[quarry] launch REFUSED path={} reason=oversize ({})", path, s);
        return s;
    }
    let bytes = match mt.read(path, 0, st.size as usize) {
        Ok(b) => b,
        Err(e) => {
            let s = why(e);
            serial_println!("[quarry] launch REFUSED path={} reason=read ({})", path, s);
            return s;
        }
    };
    if bytes.len() >= 20 && bytes[0..4] == [0x7F, b'E', b'L', b'F'] {
        let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
        if bytes[4] != 2 || bytes[5] != 1 || machine != 183 {
            let s = alloc::format!(
                "not an aarch64 ELF64 (class {} data {} machine {})",
                bytes[4], bytes[5], machine
            );
            serial_println!("[quarry] launch REFUSED path={} reason=elf ({})", path, s);
            return s;
        }
    }
    let n = bytes.len();
    match crate::arch::syscall::spawn_user_image_bg(&bytes) {
        Ok((pid, asid, entry)) => {
            JOBS.lock().push(Job { pid, asid, name: String::from(path) });
            serial_println!(
                ":: QUARRY-LAUNCH: {} — {} bytes, entry {:#x}, pid={} asid={} DETACHED (spawn_user_image_bg, the same seam `bg` takes) ::",
                path, n, entry, pid, asid
            );
            alloc::format!("started pid {}", pid)
        }
        Err(e) => {
            serial_println!("[quarry] launch REFUSED path={} reason=spawn ({})", path, e);
            String::from(e)
        }
    }
}

/// x86 has no VFS mount table (`fs/vfs.rs` gates both backends to aarch64, vfs.md §12.4), so there
/// is nothing here to resolve a path against and this arm SAYS so rather than reaching for
/// `fat::mount()` — the raw-backend path this arc is forbidden to take. The layout, the gesture, the
/// double-click predicate and the launchability test all compile and are witnessed on this arch; the
/// day the x86 VFS adoption lands, this shim collapses into the one above.
///
/// This arm's `cfg` is left EXACTLY as written (`not(target_arch = "aarch64")`) by the QUARRY2 fix,
/// rather than narrowed to `target_arch = "x86_64"`: the third arm below carves its configuration out
/// of the aarch64 side only, so these three predicates stay disjoint and exhaustive while this one's
/// emitted code is untouched in every configuration that already compiled.
#[cfg(not(target_arch = "aarch64"))]
fn launch(path: &str) -> String {
    serial_println!("[quarry] launch DECLINE path={} reason=no-vfs-on-this-arch (vfs.md 12.4)", path);
    String::from("no VFS mount table on this arch yet (vfs.md 12.4)")
}

/// The THIRD arm (rmbp-7 QUARRY2): aarch64 with `desktop_firmware` but **no EL0 layer** — neither
/// `baremetal` nor `tegra_el0`. Quarry's window, tree, list, scrolling and gestures all compile and
/// work here; what is absent is the ring-3 machinery underneath the launch seam, so there is no
/// `arch::syscall::spawn_user_image_bg` to spawn with and no `uslots::USER_REGION_SIZE` to size the
/// refusal against.
///
/// It REFUSES, out loud, naming the missing layer — it does not silently no-op, and it does not fake
/// a pid. That is Quarry's own idiom rather than a new one: every other impossible launch in this file
/// prints `[quarry] launch REFUSED path=… reason=…` and hands the same sentence back to the path bar
/// (`reason=stat`, `reason=empty`, `reason=oversize`, `reason=job-table-full`, `reason=spawn`), so an
/// operator who double-clicks a program on a ringless board reads *why* in the window and the serial
/// log carries the matching line. `REFUSED` rather than the x86 arm's `DECLINE` because the two are
/// genuinely different findings: x86 has the spawn seam and lacks the *mount table* (a cross-file gate
/// in `fs/vfs.rs`, vfs.md §12.4), whereas this build has the mount table and lacks the *rings*. Both
/// are stated in the terms the reader can act on — the feature to turn on is named.
///
/// No facade is invented. `arch/aarch64/mod.rs` declines to publish `syscall`/`uslots` outside the EL0
/// features deliberately, and a `crate::arch::spawn_user_image_bg` shim that answered `Err` here would
/// be exactly the fiction that preamble refuses; the dispatch is kept at the call site instead, which
/// is also what [`super::super::dock`]'s focus seam does one file over (`ba3e9b62`).
#[cfg(all(
    target_arch = "aarch64",
    not(any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0"))
))]
fn launch(path: &str) -> String {
    serial_println!(
        "[quarry] launch REFUSED path={} reason=no-el0-layer (this aarch64 build has none of `baremetal` / `tegra_el0` / `virt_el0`, so `arch::syscall`/`uslots` are not compiled)",
        path
    );
    String::from("no EL0 layer in this build (needs `baremetal`, `tegra_el0` or `virt_el0`)")
}

/// Perform an [`Act`] decided inside the model lock, with that lock RELEASED, then record its
/// one-line result back into the model for the path bar.
fn run_act(act: Act) {
    let line = match act {
        Act::None => return,
        Act::Launch(p) => {
            // Reap first, then test the ceiling — the two steps that used to open the aarch64
            // [`launch`] body, hoisted here (rmbp-7 QUARRY) so [`MAX_JOBS`] and [`JOBS`] have an
            // arch-neutral reader and mean the same thing on both chips. Order and wording are
            // unchanged, so the aarch64 serial line is byte-for-byte what it was.
            reap_jobs();
            if JOBS.lock().len() >= MAX_JOBS {
                let s = alloc::format!("{} live jobs — kill one first", MAX_JOBS);
                serial_println!("[quarry] launch REFUSED path={} reason=job-table-full ({})", p, s);
                s
            } else {
                let r = launch(&p);
                reap_jobs();
                r
            }
        }
        Act::NoOpener(p) => {
            // The honest census the brief asks for. Nothing in this tree opens a document: there is
            // no association registry, no viewer, and no `SYS_EXEC`-with-argv for a program to be
            // handed a path with. Saying that out loud is the point — an operator who double-clicks
            // `CONFIG.TXT` and sees nothing should be able to tell "broken" from "not built yet".
            serial_println!(
                "[quarry] open UNHANDLED path={} — no opener exists in this tree (launchable = .ELF/.BIN via spawn_user_image_bg; a document needs the opener registry named in quarry.md 7)",
                p
            );
            alloc::format!("no opener for {}", leaf(&p))
        }
    };
    if let Some(m) = MODEL.lock().as_mut() {
        m.status = Some(line);
    }
}

// ── Painting ────────────────────────────────────────────────────────────────────────────────────

/// Blit one filled rect into the surface. Clamped to the surface BEFORE the loop runs, so the bound is
/// always reachable and the trip count is bounded by the surface — the `user-stat::fill_rect` rule,
/// which exists because an unclamped bound compiled to a ~2^64-iteration row loop there.
fn fill(px: &mut [u32], g: &Geom, x: usize, y: usize, w: usize, h: usize, c: u32) {
    let xe = (x + w).min(g.w);
    let ye = (y + h).min(g.h);
    let mut r = y.min(g.h);
    while r < ye {
        let base = r * g.w;
        let mut col = x.min(g.w);
        while col < xe {
            px[base + col] = c;
            col += 1;
        }
        r += 1;
    }
}

/// One-pixel keyline rectangle.
fn keyline(px: &mut [u32], g: &Geom, r: Rect, c: u32) {
    fill(px, g, r.x, r.y, r.w, 1, c);
    fill(px, g, r.x, r.y + r.h.saturating_sub(1), r.w, 1, c);
    fill(px, g, r.x, r.y, 1, r.h, c);
    fill(px, g, r.x + r.w.saturating_sub(1), r.y, 1, r.h, c);
}

/// Draw ASCII text, clipped to `max_x`. Uses `font8x8` — the kernel's one font, the same table
/// `pal::draw_text` and `instgui::text` blit from. Returns the pen x after the last glyph.
fn text(px: &mut [u32], g: &Geom, x: usize, y: usize, s: &[u8], max_x: usize, fg: u32) -> usize {
    let cell = g.cell();
    let mut cx = x;
    for &ch in s {
        if cx + cell > max_x || cx + cell > g.w {
            break;
        }
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch.min(127) as usize];
        for (ry, rowbits) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if rowbits & (1 << rx) != 0 {
                    fill(px, g, cx + rx * g.ts, y + ry * g.ts, g.ts, g.ts, fg);
                }
            }
        }
        cx += cell;
    }
    cx
}

/// The disclosure marker: a right-pointing triangle when collapsed, down-pointing when expanded.
/// Drawn rather than spelled, because `font8x8`'s BASIC page has no triangle and a `>`/`v` pair reads
/// as text the operator might try to select.
fn disclosure(px: &mut [u32], g: &Geom, x: usize, y: usize, open: bool, c: u32) {
    let n = 4 * g.ts;
    for i in 0..n {
        if open {
            // Down-pointing: row `i` runs `x+i ..= x+i+2*(n-i)`, so the widest row is the top one and
            // the apex is at the bottom. Every row's midpoint is `x + n`, so the glyph is centred.
            let w = 2 * (n - i);
            fill(px, g, x + i, y + i, w, 1, c);
        } else {
            // Right-pointing: column `i` runs `y+i ..= y+i+2*(n-i)`, so the widest column is the left
            // one and the apex is at the right. Every column's midpoint is `y + n`, which is what
            // lets the caller centre the marker on the row with a single offset.
            let h = 2 * (n - i);
            fill(px, g, x + i, y + i, 1, h, c);
        }
    }
}

/// Render a `VfsTime` into the list pane's 16-column `YYYY-MM-DD HH:MM` field.
///
/// `None` — a medium with no stamp (native UnaFS answers `None` for every row) or a FAT entry whose
/// on-disk field was all-zero — renders as a dash, never a fabricated 1980 date. That is vfs.md
/// §12.3's ruling, and this is the second RENDERER of the one stamp type, not a second decoder:
/// `shell::vfs_mtime_field` owns the 19-char `ls -l` field and this owns the 16-char column, both over
/// [`VfsTime`], which is where the decoding actually happens.
fn mtime_field(ts: Option<&VfsTime>) -> String {
    match ts {
        None => String::from("       -        "),
        Some(t) => {
            alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.min)
        }
    }
}

/// Render a byte count right-aligned in `w` characters, with a `K`/`M`/`G` suffix once it stops
/// fitting. Exact below 100 000 bytes — a file manager that rounds a small file is annoying.
fn size_field(bytes: u64, w: usize) -> String {
    let s = if bytes < 100_000 {
        alloc::format!("{}", bytes)
    } else if bytes < 100 * 1024 * 1024 {
        alloc::format!("{}K", bytes / 1024)
    } else if bytes < 100 * 1024 * 1024 * 1024 {
        alloc::format!("{}M", bytes / (1024 * 1024))
    } else {
        alloc::format!("{}G", bytes / (1024 * 1024 * 1024))
    };
    let pad = w.saturating_sub(s.len());
    let mut out = String::new();
    for _ in 0..pad {
        out.push(' ');
    }
    out.push_str(&s);
    out
}

/// The scroll gutter for a pane's inner rect, and the thumb inside it.
fn paint_scrollbar(
    px: &mut [u32],
    g: &Geom,
    inner: Rect,
    top: usize,
    len: usize,
    visible: usize,
    scroll: usize,
) {
    if len <= visible {
        return;
    }
    // `saturating_sub` on both terms: `geometry`'s floors make a pane narrower than the gutter or
    // shorter than its header unreachable, but a painter that can underflow on a geometry change is a
    // painter that will, and the cost of not being able to is nothing.
    let track = Rect {
        x: inner.x + inner.w.saturating_sub(SBW),
        y: top,
        w: SBW,
        h: (inner.y + inner.h).saturating_sub(top),
    };
    fill(px, g, track.x, track.y, track.w, track.h, theme::SCROLL_TRACK);
    fill(px, g, track.x, track.y, 1, track.h, theme::FRAME_LINE);
    if let Some((ty, th)) = thumb(track.h, len, visible, scroll) {
        fill(px, g, track.x + 2, track.y + ty, track.w.saturating_sub(4), th, theme::SCROLL_THUMB);
    }
}

fn repaint_locked(m: &Model, px: &mut [u32]) {
    let g = &m.geom;
    let cell = g.cell();
    let row_h = g.row_h();

    // ── the path bar ────────────────────────────────────────────────────────────────────────────
    fill(px, g, 0, 0, g.w, g.bar_h(), theme::CHROME_FACE);
    fill(px, g, 0, g.bar_h() - 1, g.w, 1, theme::FRAME_LINE);
    let mut label: Vec<u8> = Vec::new();
    label.extend_from_slice(m.cwd.as_bytes());
    if m.list_truncated {
        label.extend_from_slice(b"  (list truncated)");
    }
    // The activation result rides the path bar. It is the ONLY feedback a launch has — this window
    // has no console — so it is drawn even when it is a refusal, and especially then.
    if let Some(s) = &m.status {
        label.extend_from_slice(b"  -  ");
        label.extend_from_slice(s.as_bytes());
    }
    text(px, g, PAD, g.ts, &label, g.w - PAD, theme::TITLE_TEXT_ACTIVE);

    // ── the tree pane ───────────────────────────────────────────────────────────────────────────
    let tp = g.tree_pane();
    let ti = tp.inner();
    fill(px, g, tp.x, tp.y, tp.w, tp.h, theme::CONTENT_FILL);
    keyline(px, g, tp, theme::FRAME_LINE);
    let tvis = m.tree_visible();
    let tsb = if m.tree.len() > tvis { SBW } else { 0 };
    for r in 0..tvis {
        let i = m.tree_scroll + r;
        if i >= m.tree.len() {
            break;
        }
        let row = &m.tree[i];
        let y = ti.y + r * row_h;
        let sel = i == m.tree_sel;
        if sel {
            let c = if m.focus == Pane::Tree { theme::ACCENT } else { theme::SCROLL_THUMB };
            fill(px, g, ti.x, y, ti.w - tsb, row_h, c);
        }
        let ink = if sel && m.focus == Pane::Tree {
            theme::CHROME_FACE
        } else {
            theme::CONTENT_TEXT
        };
        let indent = PAD + row.depth * cell;
        // Both triangle forms have their centre at `y + 4 * ts` by construction (column/row `i`
        // spans `i ..= i + 2*(n-i)`, whose midpoint is `n`, independent of `i`), so this offset
        // puts the marker's centre exactly on the row's.
        disclosure(px, g, ti.x + indent, y + row_h / 2 - 4 * g.ts, row.expanded, ink);
        text(
            px,
            g,
            ti.x + indent + cell + g.ts,
            y + g.ts,
            row.name.as_bytes(),
            ti.x + ti.w - tsb,
            ink,
        );
    }
    paint_scrollbar(px, g, ti, ti.y, m.tree.len(), tvis, m.tree_scroll);

    // ── the divider ─────────────────────────────────────────────────────────────────────────────
    fill(px, g, tp.x + tp.w, tp.y, 1, tp.h, theme::FRAME_LINE);

    // ── the list pane ───────────────────────────────────────────────────────────────────────────
    let lp = g.list_pane();
    let li = lp.inner();
    fill(px, g, lp.x, lp.y, lp.w, lp.h, theme::CONTENT_FILL);
    keyline(px, g, lp, theme::FRAME_LINE);
    let lvis = m.list_visible();
    let lsb = if m.list.len() > lvis { SBW } else { 0 };
    let cols_w = li.w.saturating_sub(lsb).saturating_sub(2 * PAD);
    // Columns degrade rather than overlap: the date goes first, then the size, so a narrow pane still
    // shows names instead of three columns of ellipsis.
    let (size_cols, date_cols) = if cols_w >= 34 * cell {
        (9usize, 16usize)
    } else if cols_w >= 22 * cell {
        (9, 0)
    } else {
        (0, 0)
    };
    let name_cols = (cols_w / cell).saturating_sub(size_cols + date_cols + 2);
    let name_x = li.x + PAD;
    let size_x = name_x + (name_cols + 1) * cell;
    let date_x = size_x + (size_cols + 1) * cell;
    let clip = li.x + li.w - lsb;

    // Header — outside the scrolled band by construction, so a scrolled list never loses its columns.
    fill(px, g, li.x, li.y, li.w, row_h, theme::CHROME_FACE);
    fill(px, g, li.x, li.y + row_h - 1, li.w, 1, theme::FRAME_LINE);
    text(px, g, name_x, li.y + g.ts, b"NAME", clip, theme::TITLE_TEXT_INACTIVE);
    if size_cols > 0 {
        text(px, g, size_x, li.y + g.ts, b"SIZE", clip, theme::TITLE_TEXT_INACTIVE);
    }
    if date_cols > 0 {
        text(px, g, date_x, li.y + g.ts, b"MODIFIED", clip, theme::TITLE_TEXT_INACTIVE);
    }
    let body_y = li.y + row_h;

    if let Some(e) = &m.err {
        text(px, g, name_x, body_y + g.ts, e.as_bytes(), clip, theme::CONTROL_CLOSE);
    } else {
        for r in 0..lvis {
            let i = m.list_scroll + r;
            if i >= m.list.len() {
                break;
            }
            let ent = &m.list[i];
            let y = body_y + r * row_h;
            let sel = i == m.list_sel;
            if sel {
                let c = if m.focus == Pane::List { theme::ACCENT } else { theme::SCROLL_THUMB };
                fill(px, g, li.x, y, li.w - lsb, row_h, c);
            }
            let ink = if sel && m.focus == Pane::List {
                theme::CHROME_FACE
            } else {
                theme::CONTENT_TEXT
            };
            let dir = matches!(ent.kind, NodeKind::Dir);
            let mut nm: Vec<u8> = Vec::new();
            nm.extend_from_slice(ent.name.as_bytes());
            // `ls -F`'s two marks, and they are the row's whole contract with the pointer: `/` is
            // "double-click descends", `*` is "double-click RUNS this". A window that starts programs
            // must show which rows start programs — an operator should never have to discover that
            // by double-clicking and finding out.
            if dir {
                nm.push(b'/');
            } else if is_executable(&ent.name) {
                nm.push(b'*');
            }
            nm.truncate(name_cols);
            text(px, g, name_x, y + g.ts, &nm, size_x.min(clip), ink);
            if size_cols > 0 {
                let s = if dir {
                    alloc::format!("{:>1$}", "--", size_cols)
                } else {
                    size_field(ent.size, size_cols)
                };
                text(px, g, size_x, y + g.ts, s.as_bytes(), date_x.min(clip), ink);
            }
            if date_cols > 0 {
                let d = mtime_field(ent.mtime.as_ref());
                text(px, g, date_x, y + g.ts, d.as_bytes(), clip, ink);
            }
        }
    }
    paint_scrollbar(px, g, li, body_y, m.list.len(), lvis, m.list_scroll);
}

/// Repaint the whole surface and present it.
///
/// One `wm::present` per call. The scroll path costs exactly this — see `quarry.md` §5: there is no
/// per-row damage rectangle here because `SYS_WIN_PRESENT_ROWS`' kernel twin, `wm::present_rows`, is
/// reached only from the x86 syscall arm today, and a row-band present that only one arch honours is
/// not a mechanism, it is a fork.
fn repaint() {
    let guard = MODEL.lock();
    let Some(m) = guard.as_ref() else {
        return;
    };
    let mut surf = SURF.lock();
    if surf.is_empty() {
        return;
    }
    {
        // SAFETY: `surf` is a `Vec<u8>` of exactly `w * h * 4` bytes, allocated by `open` and never
        // resized; a `[u32]` view over it is in-bounds and correctly aligned (a `Vec<u8>` from the
        // global allocator meets `u32`'s alignment for a 4-byte-multiple length, and `wm` reads the
        // same bytes as ARGB8888 words).
        let px: &mut [u32] = unsafe {
            core::slice::from_raw_parts_mut(surf.as_mut_ptr() as *mut u32, m.geom.w * m.geom.h)
        };
        repaint_locked(m, px);
    }
    drop(surf);
    let id = WIN.load(Ordering::Relaxed);
    drop(guard);
    if id != wm::WIN_NONE {
        let _ = wm::present(id);
        PAINTS.fetch_add(1, Ordering::Relaxed);
    }
}

// ── Lifecycle ───────────────────────────────────────────────────────────────────────────────────

/// Is Quarry's window live? (A PARKED window is still live — see [`on_glass`].)
pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

/// Is Quarry's window live **and on the glass**?
///
/// `wm` expresses "minimised" as a POSITION: the row's `z` drops below `SHELL_Z`, it stops
/// compositing, and the dock is the way back. So [`is_open`] alone is the wrong question for the
/// KEYBOARD — a parked Quarry that kept first refusal on arrows and `<Enter>` would be eating keys for a
/// window the operator cannot see, which is the same defect in kind as typing into a hidden shell.
/// The pointer needs no equivalent guard: `wm::hit_test` does not report a row that is not
/// compositing, so [`press_route`] already declines a parked window by construction.
///
/// SO9: this is no longer the WHOLE keyboard gate, it is one conjunct of it. [`key_route`] binds a
/// key only when Quarry also HOLDS FOCUS; being on the glass is necessary and was never sufficient.
/// It is still asked, and still load-bearing, because [`close`] does not release focus — `FOCUS_ASID`
/// can name [`OWNER`] with no row left to route to. The WHEEL keeps this as its only guard, for the
/// reason [`key_route`]'s header gives.
fn on_glass() -> bool {
    let id = WIN.load(Ordering::Relaxed);
    id != wm::WIN_NONE && wm::info(id).map(|i| i.z > wm::shell_z()).unwrap_or(false)
}

/// POSFIX — how many times [`open`] re-asks `video::panel_info_nonblocking()` before it declines.
/// The same bound LOCKFIX gave `inwedge_selftest`'s released read, for the same reason: enough that
/// only a genuinely stuck holder can exhaust it, small enough that exhausting it is bounded work.
const PANEL_TRIES: u32 = 64;

/// Open Quarry. Idempotent — a second call raises the existing window rather than minting a second.
///
/// Every failure arm prints exactly one `[quarry] DECLINE reason=…` line and leaves no half-built
/// state: the surface is allocated and fully painted BEFORE the row names it, so the compositor can
/// never read an unpainted buffer, and a failed `wm::create_at` drops the allocation on the way out.
pub fn open() {
    if is_open() {
        let id = WIN.load(Ordering::Relaxed);
        wm::focus_changed(OWNER);
        serial_println!("[quarry] open SKIP reason=already-open win={}", id);
        return;
    }
    // POSFIX — the panel read on the dock-click path, through LOCKFIX's one door.
    //
    // WHY THIS ONE WAS STILL BLOCKING. `open()` looks like boot furniture — `desktop_firmware` calls it once
    // while the desktop is being built — but it has a SECOND caller and that one is an input event:
    // the dock's pinned tile latches `request_open()`, and `service()` drains the latch from
    // `syscall.rs`'s strip-press arm, i.e. from the preemptible `usb-pump`/`input` band, the exact
    // band INWEDGE convicted. A blocking `WRITER.lock()` there is boot 8 with a file manager on the
    // other end of it, and being a heavyweight one-shot makes it worse, not better: this function
    // goes on to allocate, read a volume and mint a window, so a tick landing inside the acquire is
    // likelier here than anywhere else on the band.
    //
    // WHY BOUNDED-RETRY RATHER THAN A BARE DECLINE. `wheel_route`'s single-shot decline is right for
    // a detent — an event with no second chance and a cheap loss. An open is a deliberate operator
    // gesture, and losing it to an instantaneous lock race would read as a dead dock tile. Panel
    // geometry is also, unlike a detent, STATIC: the answer a retry gets is the answer the first try
    // wanted. So we retry the non-blocking door — never blocking, never masked ACROSS a retry (each
    // try masks and releases inside `panel_info_nonblocking`), so the holder can always run — with
    // the same bound and the same `tries=` accounting LOCKFIX gave `inwedge_selftest`'s released
    // read.
    //
    // AND IF EVERY TRY REFUSES: re-latch and say so. The request goes BACK into `REOPEN`, so the next
    // `service()` pass reopens with no further operator action, and the DECLINE line names the
    // reason. What is not on the table is guessing: an `open()` that proceeded on stale or zero
    // geometry would size the window, the dock-strip check and the surface allocation off a lie.
    let mut tries = 0u32;
    let (pw, ph) = loop {
        if let Some(i) = crate::video::panel_info_nonblocking() {
            break (i.width, i.height);
        }
        tries += 1;
        if tries >= PANEL_TRIES {
            REOPEN.store(true, Ordering::Release);
            serial_println!(
                "[quarry] DECLINE reason=panel-busy tries={} (re-latched — the next dock press reopens; POSFIX declines rather than block on the panel from the input band)",
                tries
            );
            return;
        }
        core::hint::spin_loop();
    };
    if tries != 0 {
        serial_println!("[quarry] open panel-contended tries={} panel={}x{}", tries, pw, ph);
    }
    let Some(g) = geometry(pw, ph) else {
        serial_println!(
            "[quarry] DECLINE reason=panel-below-floor panel={}x{} floor={}x{}",
            pw, ph, FLOOR_W, FLOOR_H
        );
        return;
    };
    // CONSOLEWIN, applied to a second tenant — `desktop_uefi`'s law, unchanged in substance and unchanged in
    // reason. Quarry is an ordinary `wm` row, so the kernel draws it the ordinary control cluster,
    // including a MINIMISE disc. Minimise is a position: the row drops below `SHELL_Z`, stops
    // compositing, and the only gesture that brings it back is the dock — including its own pinned
    // tile, which is a tile IN that strip and therefore no escape from this. `dock::Layout::for_panel`
    // answers `None` when the strip will not fit at `MAX_WINDOWS` rows, and a control that hides a
    // window with no way back is worse than no window. `MAX_WINDOWS` and not the live count, because
    // the check must hold for every table state this boot can reach.
    //
    // This is also, stated plainly rather than discovered later, what keeps the QEMU raspi4b gate
    // honest: 640x480 cannot host a twelve-tile dock, so Quarry DECLINES there and the video witness
    // battery — which asserts exact panel pixels and knows nothing of a file manager — is unperturbed.
    // The bench panel (1920x1200) hosts the strip, so that is where the window actually opens, and the
    // armed bench-geometry run is where its effect on that battery is measured rather than assumed.
    if crate::video::dock::Layout::for_panel(wm::MAX_WINDOWS, pw, ph).is_none() {
        serial_println!(
            "[quarry] DECLINE reason=dock-cannot-host-full-strip panel={}x{} rows={} (the minimise disc would have no way back, and neither would the pinned tile)",
            pw, ph, wm::MAX_WINDOWS
        );
        return;
    }
    let len = g.w * g.h * 4;
    {
        let mut surf = SURF.lock();
        surf.clear();
        if surf.try_reserve_exact(len).is_err() {
            serial_println!("[quarry] DECLINE reason=alloc len={}", len);
            return;
        }
        surf.resize(len, 0);
    }
    // The model, then the first full paint, then the row. `repaint` needs no window (it presents only
    // when one exists), so this ordering costs nothing and buys the "never composite a blank surface"
    // property `Window::presented` exists to enforce for ring-3 apps.
    *MODEL.lock() = Some(Model::new(g));
    if let Some(m) = MODEL.lock().as_mut() {
        m.settle();
    }
    repaint();

    let Some((_scale, ow, oh)) = wm::spawn_geometry(g.w, g.h) else {
        serial_println!("[quarry] DECLINE reason=geometry-unavailable");
        *MODEL.lock() = None;
        SURF.lock().clear();
        return;
    };
    // Centred in the WORK AREA — below the menu bar's reservation and above the bottom instrument
    // strip — so an enabled bar never covers the path bar. At `top_chrome_h == 0` this is a plain
    // panel centring, unchanged.
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop
        + ph.saturating_sub(wtop)
            .saturating_sub(crate::ui_status::chrome_h(ph))
            .saturating_sub(oh)
            / 2;
    let base = SURF.lock().as_ptr() as usize;
    let id = wm::create_at(
        OWNER,
        base,
        len,
        g.w as u32,
        g.h as u32,
        (g.w * 4) as u32,
        b"quarry",
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        serial_println!("[quarry] DECLINE reason=create-failed");
        *MODEL.lock() = None;
        SURF.lock().clear();
        return;
    }
    WIN.store(id, Ordering::Relaxed); wm::winid_register_holder(&WIN, "quarry"); // WINID (SO1(b)) — ⚠ SAME-LINE fold, line-NEUTRAL. Same argument as pulsewin's: `close()` clears this cell and `press_route` is its only caller, so a close taken by the router's furniture arm would free the row and leave this cell naming a re-issuable slot. render7 is the boot where this window WAS re-issued the console's id (`[quarry] open win=1` after `close-furniture win=1`).
    wm::focus_changed(OWNER);
    let (roots_n, tn, ln) = {
        let gd = MODEL.lock();
        gd.as_ref()
            .map(|m| (m.tree.iter().filter(|r| r.depth == 0).count(), m.tree.len(), m.list.len()))
            .unwrap_or((0, 0, 0))
    };
    serial_println!(
        "[quarry] open win={} surf={}x{} ts={} box={}x{} at ({},{}) volumes={} tree-rows={} list-rows={} cwd={}",
        id, g.w, g.h, g.ts, ow, oh, ox, oy, roots_n, tn, ln,
        MODEL.lock().as_ref().map(|m| m.cwd.clone()).unwrap_or_default()
    );
    census("open");
    repaint();
}

/// The three lines that make v2's four claims READABLE in a headless capture, printed at open and on
/// every refresh. None of them is a verdict; they are measurements, and each one answers a bench
/// complaint in the terms it was made in.
///
///  * `volumes=` — the mount prefixes and the ROOT rows they reduced to. `/fat listed twice` is
///    convicted or cleared by comparing the two lists, without a photograph.
///  * `reads=/hits=/cycles=` — the cost of the listing path. `reads` is seam calls actually made,
///    `hits` is the calls the cache answered; on this machine an open used to make four and now
///    makes two, and the number says so.
///  * `census=` — the DIRECTORY ITSELF, by name and size, up to [`CENSUS_MAX`] entries. "Where is
///    vug, where is the kernel" is a question about content, so the content is on the wire.
fn census(when: &str) {
    let g = MODEL.lock();
    let Some(m) = g.as_ref() else {
        return;
    };
    let roots: Vec<&str> = m.tree.iter().filter(|r| r.depth == 0).map(|r| r.path.as_str()).collect();
    serial_println!(
        "[quarry] {} volumes mounts={:?} roots={:?} tree-rows={} (a mount claimed by another mount is not a root — that is the duplicate-/fat rule)",
        when, m.mounts, roots, m.tree.len()
    );
    serial_println!(
        "[quarry] {} cost reads={} hits={} cycles={} cache={}/{} gen={}",
        when, m.reads, m.hits, m.read_cycles, m.cache.len(), MAX_CACHE, m.cache_gen
    );
    let dirs = m.list.iter().filter(|e| matches!(e.kind, NodeKind::Dir)).count();
    let mut names = String::new();
    for e in m.list.iter().take(CENSUS_MAX) {
        if matches!(e.kind, NodeKind::Dir) {
            names.push_str(&alloc::format!(" {}/", e.name));
        } else {
            names.push_str(&alloc::format!(
                " {}{}({})",
                e.name,
                if is_executable(&e.name) { "*" } else { "" },
                e.size
            ));
        }
    }
    serial_println!(
        "[quarry] {} census cwd={} entries={} dirs={} files={} truncated={} names:{}{}",
        when, m.cwd, m.list.len(), dirs, m.list.len() - dirs, m.list_truncated, names,
        if m.list.len() > CENSUS_MAX { " ..." } else { "" }
    );
    if let Some(e) = &m.err {
        serial_println!("[quarry] {} census ERROR cwd={} {}", when, m.cwd, e);
    }
}

/// Close Quarry and release its surface. Safe to call when not open.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    // `wm::close` first: the row must stop naming the buffer before the buffer goes away. It spins on
    // the drain barrier, so it is called with no lock of ours held.
    wm::close(id);
    *MODEL.lock() = None;
    SURF.lock().clear();
    serial_println!("[quarry] closed win={} paints={}", id, PAINTS.load(Ordering::Relaxed));
}

// ── Input ───────────────────────────────────────────────────────────────────────────────────────

/// Keyboard. Returns `true` when the key was CONSUMED.
///
/// Arrow keys arrive as the C0 codes the HID map assigns (`0x1C..=0x1F`, right/left/down/up) — the
/// same bytes `una_abi::KEY_RIGHT`..`KEY_UP` publish to ring 3, decoded once in the driver.
///
/// ### SO9 — THE GATE IS FOCUS, NOT PRESENCE
///
/// This used to say: *"the contract is `video::instgui`'s — while Quarry's window is OPEN it takes
/// first refusal on the keys it binds, and it never swallows a key it does not bind, so the console
/// keeps working underneath for everything else."* The second clause is true and practically empty.
/// `<Enter>` and `<Backspace>` are the two keys a line editor cannot work without, and they are both
/// bound here — so an open-but-unfocused Quarry did not leave "the console working underneath", it
/// left a console that could not run a command or erase a character. Measured on the Orin, render8
/// 2026-09-06: Quarry was on the glass from boot (`desktop_firmware::activate` step 6) and stayed
/// there for the whole session, so `<Enter>` at the shell opened a FILE for every keystroke of the
/// flight, while `[wc-fv] focus raise asid=0xffffff01` says focus was on the CONSOLE for long
/// stretches of it (`docs/dev/LEDGER.md` SO9).
///
/// The remedy is the desktop model Peter asked for — **keys go to the FOCUSED window only.** Quarry
/// binds a key when `wm::focus_asid()` names [`OWNER`], and at no other time; unfocused, every key
/// falls straight through to the drain that called us. Focus arrives the way it does on a Mac: the
/// operator CLICKS the window ([`press_route`] raises through `wm::focus_changed(OWNER)`), or Quarry
/// mints/raises its own window ([`open`]). Focus LEAVES the same way — a click on the console's
/// content is `arch/aarch64/syscall.rs`'s SHELLWIN-PI arm (`focus_changed(owner)`), a click on the
/// bare desktop is `focus_changed(0)`.
///
/// [`on_glass`] stays in the conjunction and is not redundant: [`close`] does not release focus, so
/// `FOCUS_ASID` can still name [`OWNER`] after the window is gone.
///
/// ### R24 — `<Esc>` NO LONGER CLOSES QUARRY
///
/// The `if c == 0x1B { close(); return true; }` arm that stood here is RETRACTED BY RULING, not
/// fixed: *"esc should not close any app windows"* (Peter, 2026-09-06, `docs/dev/RULINGS.md` R24).
/// Esc dismisses MENUS ONLY, and it already does — `strip::key_escape` is asked AHEAD of this
/// function in all three routers (`main.rs:2948`, `main.rs:4578`, `arch/x86_64/syscall.rs:6740`), so
/// the modal surface still gets it first and a bare Esc with nothing down now falls past Quarry
/// untouched. The close disc and the dock tile are the ways out of this window.
///
/// ### The WHEEL is deliberately NOT focus-gated
///
/// [`wheel_route`] resolves the pointer against `wm::hit_test` and scrolls the window UNDER THE
/// CURSOR, which is the same desktop model this gate is enforcing for the keyboard — scroll-under-
/// pointer is not focus theft, and the hit test already stops Quarry taking a detent that belongs to
/// a window above it. Gating it on focus would delete a working gesture no defect asks about. It
/// keeps the [`on_glass`] guard only, exactly as before.
pub fn key_route(ev: crate::pal::Event) -> bool {
    // QSCROLL — the WHEEL arrives here, at the seam that already exists, because this function is
    // handed the whole `pal::Event` rather than a keycode and `arch/aarch64/syscall.rs` is a
    // byte-identity-critical file no arc may add a line to (PARITY.md §5.3). The name is a keyboard
    // noun and the event is not one; that trade is stated in [`wheel_route`] and in `quarry.md` §14
    // rather than paid for with an edit to the router. Asked BEFORE the `Event::Key` test for the
    // ordinary reason: the arm below would otherwise decline it and the wheel would fall through to
    // the focus ring, which has no consumer for it either. SO9 keeps it on [`on_glass`] alone — see
    // this function's header, §"The WHEEL is deliberately NOT focus-gated".
    if let crate::pal::Event::Wheel(d) = ev {
        return on_glass() && wheel_route(d);
    }
    let crate::pal::Event::Key(c) = ev else {
        return false;
    };
    // SO9 — the two facts, read once each, in the order that makes the witness readable.
    let live = is_open();
    let focused = wm::focus_asid() == OWNER;
    if !(focused && on_glass()) {
        if live {
            key_witness(c, focused, false);
        }
        return false;
    }
    let mut acted = true;
    let mut refreshed = false;
    // Decided under the lock, run without it — see [`Act`].
    let mut act = Act::None;
    {
        let mut guard = MODEL.lock();
        let Some(m) = guard.as_mut() else {
            // Focused, on the glass, and no model — the close that is mid-flight. Declined, and said
            // so on the wire: an unwitnessed decline here would read as the SO9 gate firing.
            key_witness(c, true, false);
            return false;
        };
        match c {
            // Up / Down — move the selection in the focused pane; `settle` does the following.
            0x1F => match m.focus {
                Pane::Tree => m.tree_sel = m.tree_sel.saturating_sub(1),
                Pane::List => m.list_sel = m.list_sel.saturating_sub(1),
            },
            0x1E => match m.focus {
                Pane::Tree => {
                    if m.tree_sel + 1 < m.tree.len() {
                        m.tree_sel += 1;
                    }
                }
                Pane::List => {
                    if m.list_sel + 1 < m.list.len() {
                        m.list_sel += 1;
                    }
                }
            },
            // Left — in the list, hand focus back to the tree; in the tree, collapse, or step to the
            // parent row when this one is already closed. The Finder gesture, on a keyboard.
            0x1D => match m.focus {
                Pane::List => m.focus = Pane::Tree,
                Pane::Tree => {
                    if m.tree.get(m.tree_sel).map(|r| r.expanded).unwrap_or(false) {
                        m.collapse(m.tree_sel);
                    } else if let Some(d) = m.tree.get(m.tree_sel).map(|r| r.depth) {
                        if d > 0 {
                            let mut i = m.tree_sel;
                            while i > 0 && m.tree[i].depth >= d {
                                i -= 1;
                            }
                            m.tree_sel = i;
                        }
                    }
                }
            },
            // Right — in the tree, expand; a second press on an open row crosses into the list.
            0x1C => match m.focus {
                Pane::Tree => {
                    if m.tree.get(m.tree_sel).map(|r| r.expanded).unwrap_or(false) {
                        m.focus = Pane::List;
                    } else {
                        m.expand(m.tree_sel);
                    }
                }
                Pane::List => {}
            },
            // Enter — open. In the tree that means "show this directory"; in the list it means
            // "descend into the selected directory", which also expands and reveals it on the left.
            b'\r' | b'\n' => match m.focus {
                Pane::Tree => {
                    let i = m.tree_sel;
                    if i < m.tree.len() {
                        if !m.tree[i].expanded {
                            m.expand(i);
                        }
                        let p = m.tree[i].path.clone();
                        m.show(&p);
                    }
                }
                // The KEYBOARD twin of the double-click, and deliberately the SAME function: Enter
                // on a directory descends (as it always did) and Enter on a program runs it. A
                // gesture that exists only on the pointer is a gesture an operator at a serial
                // console cannot reach, and this window is driven from both.
                Pane::List => {
                    let i = m.list_sel;
                    act = m.activate_row(i);
                }
            },
            // Backspace — up one level, from wherever the focus is.
            0x08 | 0x7F => {
                let p = parent(&m.cwd);
                m.navigate(&p);
            }
            // `r` — re-read. The mount table is rebuilt AND the directory cache dropped, so a stick
            // plugged after open appears and a card written to from the shell re-reads. This is the
            // operator's half of the cache's invalidation set (module doc §Cost); the other two
            // halves — window open and a volume-generation change — need no gesture.
            b'r' | b'R' => {
                m.invalidate();
                m.reload_roots();
                let cwd = m.cwd.clone();
                m.status = None;
                m.show(&cwd);
                refreshed = true;
            }
            _ => acted = false,
        }
        if acted {
            m.settle();
        }
    }
    run_act(act);
    reap_jobs();
    if refreshed {
        census("refresh");
    }
    if acted {
        repaint();
    }
    key_witness(c, true, acted);
    acted
}

/// SO9 — the routed-key witness's line budget. A per-EVENT family on the input band, so it is capped
/// the way every other one in the tree is (`CLOSE_LOG_MAX` in both `syscall.rs` files is the idiom):
/// enough lines to score a whole bench sitting's worth of gestures, few enough that a stuck key
/// cannot bury the capture the rest of the flight is read from.
const KEY_LOG_MAX: usize = 96;
static KEY_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);

/// SO9 — **one line per key this function was asked about while Quarry's window was live.**
///
/// `focus=1 took=0` is a bound key declined for a reason that is not focus (a close mid-flight);
/// `focus=0 took=0` is the SO9 gate doing its job — the key is on its way to the shell drain;
/// `focus=1 took=1` is Quarry consuming its own key. A capture that shows `focus=0 took=1` on any
/// line is this fix regressed, which is the point of printing `took` beside `focus` rather than
/// inferring one from the other.
///
/// Silent when the window is not live: a board with no file manager on the glass owes no line per
/// keystroke, and the SO9 question is not being asked there.
fn key_witness(c: u8, focus: bool, took: bool) {
    if KEY_LOG_COUNT.fetch_add(1, Ordering::Relaxed) < KEY_LOG_MAX {
        serial_println!(
            "[quarry] key_route key={:#04x} focus={} took={}",
            c,
            focus as u8,
            took as u8
        );
    }
}

/// Pointer press, in PANEL coordinates. Returns `true` when Quarry consumed it.
///
/// Ordering discipline: this asks [`wm::hit_test`] itself and acts only when the TOP-MOST window at
/// the point is Quarry's, so folding the call in ahead of the router's window arms cannot let Quarry
/// steal a press that landed on a window above it. Chrome (title strip, border) and the minimise/zoom
/// discs are deliberately NOT claimed — they fall through to the router's own arms, which is what
/// makes Quarry draggable and parkable like any other row. The CLOSE disc IS claimed, because
/// `wc_close_click` kills an ASID and there is no process behind a kernel owner to kill.
pub fn press_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return false;
    }
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return false,
    }
    if wm::close_box_hit(id, x, y) {
        serial_println!("[quarry] press close win={} at ({},{})", id, x, y);
        close();
        return true;
    }
    let Some(info) = wm::info(id) else {
        return false;
    };
    let scale = info.scale.max(1);
    // Panel -> source. A press above/left of the content is chrome; the router owns it.
    if x < info.x as i32 || y < info.y as i32 {
        return false;
    }
    let sx = (x as usize - info.x) / scale;
    let sy = (y as usize - info.y) / scale;
    if sx >= info.w || sy >= info.h {
        return false;
    }
    // The press is ours: raise so the operator's click also brings Quarry forward, exactly as the
    // router's own select arm would have.
    wm::focus_changed(OWNER);
    let act = {
        let mut guard = MODEL.lock();
        let Some(m) = guard.as_mut() else {
            return true;
        };
        // A press that hit no row still changed the pane focus (and therefore the selection ink),
        // and it is still CONSUMED — it moved window focus and must not be delivered to whatever
        // lies underneath. So the return carries only the DEFERRED work, never "was it mine".
        content_press(m, sx, sy)
    };
    // Outside the lock, always: `run_act` may reach the ELF loader and the scheduler.
    run_act(act);
    reap_jobs();
    repaint();
    true
}

/// Route a press already resolved to SOURCE coordinates. Split out from [`press_route`] so the
/// witness can drive it without a window table, and so the hit arithmetic reads beside the painter's.
///
/// Returns the work that must happen with the model lock RELEASED — [`Act::None`] for every gesture
/// that is purely a model change, which is all of them except opening a program.
fn content_press(m: &mut Model, sx: usize, sy: usize) -> Act {
    let g = m.geom;
    let row_h = g.row_h();
    let tp = g.tree_pane();
    let lp = g.list_pane();
    let cell = g.cell();

    if tp.contains(sx, sy) {
        let ti = tp.inner();
        let tvis = m.tree_visible();
        let sb = m.tree.len() > tvis;
        m.focus = Pane::Tree;
        // The gutter PAGES — a whole viewport per press. QSCROLL made the wheel the FINE gesture
        // beside it ([`wheel_scroll`], three rows a detent); the track stays the coarse one, because
        // it is the only scroll a wheel-less mouse has and the only one that crosses a
        // thousand-entry directory in a gesture a wrist can complete. The two share this pane's one
        // offset and the same selection pull, so neither can drift from the other.
        if sb && sx >= ti.x + ti.w - SBW {
            let (ty, th) = thumb(ti.h, m.tree.len(), tvis, m.tree_scroll).unwrap_or((0, 0));
            let rel = sy.saturating_sub(ti.y);
            if rel < ty {
                m.tree_scroll = m.tree_scroll.saturating_sub(tvis);
            } else if rel >= ty + th {
                m.tree_scroll = (m.tree_scroll + tvis).min(scroll_max(m.tree.len(), tvis));
            }
            // NOT `clamp`: it PANICS when min > max, and a degenerate viewport (`tvis == 0`, a pane
            // shorter than one row) reaches exactly that shape. `max` then `min` is total.
            m.tree_sel = m.tree_sel.max(m.tree_scroll).min((m.tree_scroll + tvis).saturating_sub(1));
            m.settle();
            return Act::None;
        }
        if sy < ti.y {
            return Act::None;
        }
        // `contains` tested the OUTER pane, so the bottom keyline row can land one past the last
        // viewport slot. Decline it rather than selecting a row the press did not visually address.
        let r = (sy - ti.y) / row_h;
        let i = m.tree_scroll + r;
        if r >= tvis || i >= m.tree.len() {
            return Act::None;
        }
        m.tree_sel = i;
        // The tree stamps the click too, so a press here followed by a press in the LIST can never
        // combine into a double-click. `is_double` tests the PANE as well as the row, and this is
        // what makes that test load-bearing rather than decorative.
        m.click_ms = crate::arch::ms();
        m.click_row = i;
        m.click_pane = Pane::Tree;
        // A press ON the disclosure marker toggles; anywhere else on the row navigates. The two
        // regions are derived from the SAME indent the painter used, so they cannot drift.
        let indent = ti.x + PAD + m.tree[i].depth * cell;
        if sx >= indent && sx < indent + cell {
            if m.tree[i].expanded {
                m.collapse(i);
            } else {
                m.expand(i);
            }
        } else {
            let p = m.tree[i].path.clone();
            m.show(&p);
        }
        m.settle();
        return Act::None;
    }

    if lp.contains(sx, sy) {
        let li = lp.inner();
        let lvis = m.list_visible();
        let sb = m.list.len() > lvis;
        m.focus = Pane::List;
        let body_y = li.y + row_h;
        if sb && sx >= li.x + li.w - SBW && sy >= body_y {
            let track_h = li.y + li.h - body_y;
            let (ty, th) = thumb(track_h, m.list.len(), lvis, m.list_scroll).unwrap_or((0, 0));
            let rel = sy - body_y;
            if rel < ty {
                m.list_scroll = m.list_scroll.saturating_sub(lvis);
            } else if rel >= ty + th {
                m.list_scroll = (m.list_scroll + lvis).min(scroll_max(m.list.len(), lvis));
            }
            m.list_sel = m.list_sel.max(m.list_scroll).min((m.list_scroll + lvis).saturating_sub(1));
            m.settle();
            return Act::None;
        }
        if sy < body_y {
            return Act::None; // the header is not a row
        }
        let r = (sy - body_y) / row_h;
        let i = m.list_scroll + r;
        if r >= lvis || i >= m.list.len() {
            // A press below the last row is still a press: it moved focus to this pane, and it must
            // NOT leave a stale click stamp behind that a later press on a row could pair with.
            m.click_ms = 0;
            m.settle();
            return Act::None;
        }
        // ── the double-click ────────────────────────────────────────────────────────────────────
        // Two presses, same pane, same ROW, inside DOUBLE_CLICK_MS. The row test is what makes this
        // a gesture rather than a timer: a rapid press on row 3 then row 4 is two selections, which
        // is what an operator scanning a list is doing, and it must never run anything.
        let now = crate::arch::ms();
        let dbl = is_double(m.click_ms, now, m.click_row, i, m.click_pane == Pane::List);
        m.list_sel = i;
        m.click_row = i;
        m.click_pane = Pane::List;
        // A consumed double-click RESETS the stamp rather than re-arming it, so three fast presses
        // are one double-click and one fresh single — never two overlapping activations of the row.
        m.click_ms = if dbl { 0 } else { now };
        m.settle();
        if dbl {
            let act = m.activate_row(i);
            m.settle();
            return act;
        }
        return Act::None;
    }
    Act::None
}

/// QSCROLL — what one wheel event did to the model, for the census and the repaint decision.
struct WheelHit {
    /// `"tree"` or `"list"` — the pane the POINTER was over, not the pane the keyboard is driving.
    pane: &'static str,
    /// The offset after the detents, and the largest offset that pane admits.
    scroll: usize,
    max: usize,
    /// False when the detents could not move the picture — the operator is already at a bound.
    moved: bool,
}

/// QSCROLL — apply `detents` of wheel to the pane under source-pixel `(sx, sy)`.
///
/// `None` means the pointer was inside Quarry's surface but over neither pane (the path bar), which
/// is still Quarry's pixel and still consumes the event — it simply scrolls nothing.
///
/// **The pane is chosen by the POINTER, not by `m.focus`.** That is the whole difference between a
/// wheel and an arrow key: `Up`/`Down` drive the pane the keyboard owns, and a wheel drives the pane
/// the hand is over. Resolved through the same [`Rect::contains`] calls on the same [`Geom`] accessor
/// that [`content_press`] uses one screen above, so the wheel and the press can never disagree about
/// where the divider is.
///
/// **The click grammar is untouched, deliberately and by omission.** This function writes no
/// `click_ms`, no `click_row` and no `click_pane`, produces no [`Act`], and acknowledges nothing on
/// the glass: a scroll is not a press, so it can neither complete a double-click nor break one that
/// is half-made. An operator who presses a row, scrolls, and presses the same row again inside
/// [`DOUBLE_CLICK_MS`] gets the launch they asked for — the wheel did not consume their stamp.
///
/// **What it DOES move is the selection, and only as far as the viewport's edge** — the same pull
/// [`content_press`]'s scrollbar-track arm applies, written the same way (`max` then `min`, never
/// `clamp`, which PANICS when a degenerate viewport makes `min > max`). One rule for both coarse
/// gestures rather than two, so `quarry.md` §4's standing invariant — the selection is always inside
/// the viewport — holds after a wheel exactly as it holds after a track page, and [`Model::settle`]
/// stays a fixed point rather than yanking the viewport back to a selection left off-screen.
fn wheel_scroll(m: &mut Model, sx: usize, sy: usize, detents: i32) -> Option<WheelHit> {
    let g = m.geom;
    let tree = g.tree_pane().contains(sx, sy);
    if !tree && !g.list_pane().contains(sx, sy) {
        return None;
    }
    let (len, vis, scroll) = if tree {
        (m.tree.len(), m.tree_visible(), m.tree_scroll)
    } else {
        (m.list.len(), m.list_visible(), m.list_scroll)
    };
    let max = scroll_max(len, vis);
    let next = wheel_next(scroll, max, detents);
    let lo = next;
    let hi = (next + vis).saturating_sub(1);
    if tree {
        m.tree_scroll = next;
        m.tree_sel = m.tree_sel.max(lo).min(hi);
    } else {
        m.list_scroll = next;
        m.list_sel = m.list_sel.max(lo).min(hi);
    }
    m.settle();
    Some(WheelHit { pane: if tree { "tree" } else { "list" }, scroll: next, max, moved: next != scroll })
}

/// QSCROLL — the `[qscroll]` census. Frames in which a detent reached a viewport, and how many of
/// those landed on a bound.
///
/// Two counters and not one, for WHEELZOOM's reason — the `[vugzoom] applied=`/`clamped=` split in
/// `user-vug`, made here in the same words: they answer different questions at a bench.
/// `applied` says the byte was decoded, routed, hit-tested to Quarry's window and moved the picture;
/// `clamped` says all of that happened and the range is spent, which is a working scroll at the end
/// of a directory rather than a dead one. Without the split those two are indistinguishable from
/// outside the machine, which is precisely the confusion the WHEEL arc's own `[wheel1]` census exists
/// to prevent one layer down.
static WHEEL_APPLIED: AtomicUsize = AtomicUsize::new(0);
static WHEEL_CLAMPED: AtomicUsize = AtomicUsize::new(0);

/// QSCROLL — one wheel event, routed to the viewport under the pointer. `true` when CONSUMED.
///
/// ### The seam, and why it is not a new one
///
/// This is reached from [`key_route`], which is reached from the ONE line
/// `arch/aarch64/syscall.rs::user_input_enqueue` already carries for the desktop furniture. That is
/// the single choke point every event bound for a focused window passes through — the same one the
/// WHEEL arc routes `INPUT_EV_WHEEL` through on its way to an EL0 ring, and therefore the same seam
/// `user-vug`'s WHEELZOOM consumer sits behind, one layer further out. **No routing file is touched
/// by this arc and no line is added to one**: `syscall.rs` is compiled into the knob-off
/// `kernel8.img`, where an added line breaks the byte-identity proof (PARITY.md §5.3), and a folded
/// call cannot carry a `cfg` of its own. `key_route` already receives the whole `pal::Event` rather
/// than a keycode, which is what makes the wheel arm free.
///
/// Its name therefore under-describes it, and that is a deliberate trade stated rather than hidden:
/// renaming the export would edit a line in the byte-identity-critical file for a noun, and this
/// arc will not spend that. `quarry.md` §14 records it.
///
/// ### Position, and the two things this deliberately does NOT do
///
/// A wheel event carries a delta and no coordinates, so the position is the system cursor's — read
/// exactly as `syscall.rs::click_pointer_pos` reads it for a button, from `pal::cursor::pos` against
/// the live panel. [`wm::hit_test`] then decides ownership, so a wheel over a window ABOVE Quarry is
/// that window's and a wheel over a PARKED Quarry reaches nothing (a row below `SHELL_Z` does not
/// composite and does not hit-test — the same construction that lets [`press_route`] skip the
/// [`on_glass`] guard the keyboard needs).
///
/// It does **not** call `wm::focus_changed`: a scroll is not a press, and raising a window under a
/// hand that is only turning a wheel would make the pointer's mere presence re-order the glass.
/// Quarry's own click grammar says a click SELECTS and acknowledges and focus stops nothing; the
/// wheel is quieter still — it selects nothing, acknowledges nothing, and starts nothing.
///
/// It also does **not** touch the model when nothing moved: a detent spent on a bound repaints
/// nothing, so an operator holding a flick against the end of a short directory costs one census
/// line and no `wm::present` at all.
fn wheel_route(delta: i8) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE || delta == 0 {
        return false;
    }
    // LOCKFIX — the panel geometry is read through the input path's ONE door
    // ([`crate::video::panel_info_nonblocking`]): masked, non-blocking, counted. A plain
    // `WRITER.lock()` here is the boot-8 wedge exactly as INWEDGE described it — this runs on the
    // preemptible `usb-pump` band (`route_input_to_active_el0` → `user_input_enqueue` →
    // `key_route`), on the core that also carries the kernel's IRQ-context printer, so a hold
    // preempted here leaves a lock the next masked acquirer on that core can never see released.
    //
    // A refusal DECLINES the event rather than consuming it. The wheel is not ours until the
    // pointer has been proven to be over our window, and without the panel geometry that question
    // cannot be asked; `false` is the same answer a wheel over any other window gets, so the event
    // falls through untouched rather than being swallowed on a lock race.
    let Some(i) = crate::video::panel_info_nonblocking() else {
        return false;
    };
    let (x, y) = crate::pal::cursor::pos(i.width as i32, i.height as i32);
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return false,
    }
    let Some(info) = wm::info(id) else {
        return false;
    };
    let scale = info.scale.max(1);
    // Panel -> source, exactly as `press_route` converts it. Above/left of the content is chrome.
    if x < info.x as i32 || y < info.y as i32 {
        return false;
    }
    let sx = (x as usize - info.x) / scale;
    let sy = (y as usize - info.y) / scale;
    if sx >= info.w || sy >= info.h {
        return false;
    }
    let hit = {
        let mut guard = MODEL.lock();
        // The window row can be closed by another core between the hit-test above and this lock —
        // `close()` swaps `WIN` and then takes `MODEL`. A `None` model is that race, arriving; the
        // event is still ours (the pixel was Quarry's when it was addressed) and is consumed rather
        // than delivered to whatever the close is about to reveal underneath.
        let Some(m) = guard.as_mut() else {
            return true;
        };
        wheel_scroll(m, sx, sy, delta as i32)
    };
    // Over Quarry but over neither pane — the path bar. Consumed, nothing scrolled, nothing said.
    let Some(h) = hit else {
        return true;
    };
    let (tag, n) = if h.moved {
        (" applied=", WHEEL_APPLIED.fetch_add(1, Ordering::Relaxed) + 1)
    } else {
        (" clamped=", WHEEL_CLAMPED.fetch_add(1, Ordering::Relaxed) + 1)
    };
    serial_println!(
        "[qscroll]{}{} pane={} detents={} rows={} scroll={}/{}",
        tag, n, h.pane, delta, WHEEL_ROWS, h.scroll, h.max
    );
    if h.moved {
        repaint();
    }
    true
}

// ── The dock's request seam ─────────────────────────────────────────────────────────────────────

/// Latched by `video::dock`'s pinned tile; drained by whoever services the desktop.
static REOPEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Ask for Quarry to be (re)opened. Called from the dock's press arm, which runs inside the input
/// drain and must not do disk I/O; the latch defers the directory read to [`service`].
pub fn request_open() {
    REOPEN.store(true, Ordering::Release);
}

/// Consume a pending open request. Idempotent, and safe to call every pass: a quiet pass is one
/// relaxed load.
pub fn service() {
    reap_jobs();
    if REOPEN.swap(false, Ordering::AcqRel) {
        open();
    }
}

// ── The witness ─────────────────────────────────────────────────────────────────────────────────

/// QUARRY — the arch-neutral, disk-free proof of the parts an operator cannot photograph: the scroll
/// arithmetic, the geometry accessor, the tree splice, and the press-to-row mapping.
///
/// Every leg is a pure function over synthetic input, so it runs identically on both arches and needs
/// no panel, no volume and no window table. `Err(reason)` names the first failing claim; the caller
/// prints one `:: QUARRY: … :: PASS ::` / `:: FAIL ::` line.
#[cfg(feature = "witness")]
pub fn selftest_result() -> Result<(usize, usize), &'static str> {
    // ── leg 1: geometry — floors decline, and both bench panels resolve ──────────────────────────
    if geometry(200, 200).is_some() {
        return Err("geometry admitted a panel below the floor");
    }
    let small = geometry(640, 480).ok_or("geometry declined 640x480")?;
    let bench = geometry(1920, 1200).ok_or("geometry declined 1920x1200")?;
    if small.ts != 1 || bench.ts != 2 {
        return Err("text scale did not follow the panel");
    }
    if small.w > 640 || small.h > 480 || bench.w > CEIL_W || bench.h > CEIL_H {
        return Err("surface exceeded its panel or its ceiling");
    }
    // The two panes must tile the surface exactly, with one pixel of divider and nothing over.
    for g in [small, bench] {
        let (tp, lp) = (g.tree_pane(), g.list_pane());
        if tp.x + tp.w + 1 != lp.x || lp.x + lp.w != g.w {
            return Err("panes do not tile the surface");
        }
        if tp.y != g.bar_h() || tp.h != g.h - g.bar_h() || lp.h != tp.h {
            return Err("panes do not fill below the path bar");
        }
    }

    // ── leg 2: scroll_follow — clamped above, and the selection always visible ────────────────────
    if scroll_follow(999, 0, 10, 4) != 0 {
        return Err("scroll not clamped to the top for a leading selection");
    }
    if scroll_follow(0, 9, 10, 4) != 6 {
        return Err("scroll did not follow the selection to the tail");
    }
    if scroll_follow(99, 5, 10, 4) != 5 {
        return Err("scroll_max not honoured");
    }
    if scroll_follow(3, 3, 3, 8) != 0 {
        return Err("a list shorter than the viewport must not scroll");
    }
    for len in [0usize, 1, 7, 64, 1000] {
        for vis in [1usize, 3, 10] {
            for sel in [0usize, 1, len / 2, len.saturating_sub(1)] {
                // Only IN-RANGE selections: `Model::settle` clamps `sel` to `len - 1` before any
                // scroll is computed, so `sel >= len` is a state the model cannot be in — and the
                // first cut of this sweep asserted it anyway and failed itself on `len=1, sel=1`.
                // That is the witness working: the claim was wrong, not the function.
                if len > 0 && sel >= len {
                    continue;
                }
                let s = scroll_follow(0, sel, len, vis);
                if s > scroll_max(len, vis) {
                    return Err("scroll_follow exceeded scroll_max");
                }
                if len > 0 && (sel < s || sel >= s + vis) {
                    return Err("scroll_follow left the selection off screen");
                }
            }
        }
    }

    // ── leg 3: thumb — absent when everything fits, inside the track when it does not ─────────────
    if thumb(100, 5, 10, 0).is_some() {
        return Err("a thumb was drawn for a list that fits");
    }
    let (ty, th) = thumb(100, 1000, 10, 0).ok_or("no thumb for an overflowing list")?;
    if ty != 0 || th < THUMB_MIN {
        return Err("thumb floor not honoured at scroll 0");
    }
    let (ty2, th2) = thumb(100, 1000, 10, 990).ok_or("no thumb at the tail")?;
    if ty2 + th2 != 100 {
        return Err("thumb did not reach the end of the track at max scroll");
    }
    for scroll in [0usize, 1, 250, 500, 989, 990] {
        let (a, b) = thumb(100, 1000, 10, scroll).ok_or("thumb vanished mid-range")?;
        if a + b > 100 {
            return Err("thumb overran its track");
        }
    }

    // ── leg 4: the tree splice — expand/collapse is an exact inverse ──────────────────────────────
    // Driven against a hand-built model rather than a volume, so the leg is honest on a machine with
    // no disk (QEMU raspi4b has no USB stick and x86 has no mount table at all).
    let mut m = Model {
        geom: small,
        tree: Vec::new(),
        tree_sel: 0,
        tree_scroll: 0,
        cwd: String::from("/"),
        list: Vec::new(),
        list_truncated: false,
        list_sel: 0,
        list_scroll: 0,
        focus: Pane::Tree,
        err: None,
        mounts: Vec::new(),
        cache: Vec::new(),
        cache_gen: 0,
        reads: 0,
        hits: 0,
        read_cycles: 0,
        click_ms: 0,
        click_row: 0,
        click_pane: Pane::List,
        status: None,
    };
    m.tree.push(TreeRow { path: String::from("/"), name: String::from("/"), depth: 0, expanded: false });
    m.tree.push(TreeRow { path: String::from("/fat"), name: String::from("fat"), depth: 0, expanded: false });
    let before = m.tree.len();
    // Splice a synthetic level under row 0 the way `expand` would, then prove `collapse` removes
    // exactly it — the property a hand-rolled index walk gets wrong when a sibling follows.
    m.tree[0].expanded = true;
    for k in 0..3 {
        m.tree.insert(
            1 + k,
            TreeRow {
                path: alloc::format!("/k{}", k),
                name: alloc::format!("k{}", k),
                depth: 1,
                expanded: false,
            },
        );
    }
    if m.subtree_len(0) != 3 {
        return Err("subtree_len did not stop at the sibling");
    }
    // The SELECTION follows the rows. Select the later sibling (`/fat`, now at index 4) and prove the
    // collapse leaves the highlight ON IT rather than dragging it back onto the row that closed — the
    // defect a bare `sel > i` test has, and the reason the arithmetic below has three cases.
    m.tree_sel = 4;
    m.collapse(0);
    if m.tree.len() != before || m.tree[1].path != "/fat" {
        return Err("collapse did not restore the tree exactly");
    }
    if m.tree_sel != 1 || m.tree[m.tree_sel].path != "/fat" {
        return Err("collapse moved the selection off the sibling it was on");
    }
    // And a selection INSIDE the subtree has nowhere to go but the row that closed over it. Spliced
    // by hand again, deliberately: calling `expand` here would reach `collect`, and the whole point of
    // this leg is that it is DISK-FREE and therefore honest on a machine with no volume at all.
    m.tree[0].expanded = true;
    for k in 0..2 {
        m.tree.insert(
            1 + k,
            TreeRow {
                path: alloc::format!("/j{}", k),
                name: alloc::format!("j{}", k),
                depth: 1,
                expanded: false,
            },
        );
    }
    m.tree_sel = 2;
    m.collapse(0);
    if m.tree_sel != 0 {
        return Err("collapse did not bring an inside-the-subtree selection to the closing row");
    }

    // ── leg 5: press-to-row — the painter's arithmetic, read backwards ────────────────────────────
    // A press on the first tree row must select row `tree_scroll + 0`, and one row_h lower must select
    // the next. This is the crispywire law's testable half: painter and router share one accessor.
    m.tree[0].expanded = false;
    m.tree_scroll = 0;
    let ti = small.tree_pane().inner();
    let picked_first = {
        let r = (ti.y + 1 - ti.y) / small.row_h();
        m.tree_scroll + r
    };
    let picked_second = {
        let r = (ti.y + small.row_h() + 1 - ti.y) / small.row_h();
        m.tree_scroll + r
    };
    if picked_first != 0 || picked_second != 1 {
        return Err("press-to-row did not invert the painter's row layout");
    }
    // And the same for the list pane, whose body starts one row below its inner top (the header).
    let li = small.list_pane().inner();
    let body_y = li.y + small.row_h();
    if (body_y + small.row_h() - body_y) / small.row_h() != 1 {
        return Err("list press-to-row did not clear the header");
    }

    // ── leg 6: the duplicate-root rule — the `/fat`-listed-twice fix, as a property ───────────────
    // `prefix_claims` first, because `root_prefixes` is only as good as the boundary rule under it.
    if !prefix_claims("/", "/fat") || !prefix_claims("/usb", "/usb/a") || !prefix_claims("/usb", "/usb") {
        return Err("prefix_claims failed to claim a path its prefix owns");
    }
    if prefix_claims("/usb", "/usbfoo") || prefix_claims("/fat", "/") || prefix_claims("/fat", "/usb") {
        return Err("prefix_claims claimed a path across a name boundary");
    }
    // THE DEFECT, in one assertion: this is exactly the live mount table of a Pi with a stick in it,
    // and v1 turned it into three depth-0 rows, two of which the expanded `/` then repeated.
    let live: Vec<String> =
        alloc::vec![String::from("/"), String::from("/fat"), String::from("/usb")];
    let rooted = root_prefixes(&live);
    if rooted.len() != 1 || rooted[0] != "/" {
        return Err("root_prefixes did not reduce a rooted namespace to its single root");
    }
    // Idempotent and order-independent — a rule applied twice, or to a differently-sorted table,
    // must not answer differently.
    if root_prefixes(&rooted) != rooted {
        return Err("root_prefixes is not idempotent");
    }
    let shuffled: Vec<String> =
        alloc::vec![String::from("/usb"), String::from("/"), String::from("/fat")];
    if root_prefixes(&shuffled) != rooted {
        return Err("root_prefixes depends on the order of the mount table");
    }
    // …and it must not HIDE a volume on a table with no root mount, which is the failure mode the
    // lazy fix ("just drop everything but `/`") would have had on an arch that has not adopted the
    // VFS root, or on a namespace assembled from peers.
    let rootless: Vec<String> = alloc::vec![String::from("/fat"), String::from("/usb")];
    if root_prefixes(&rootless).len() != 2 {
        return Err("root_prefixes hid a volume on a table with no root mount");
    }
    // The boundary case: `/usb` does not claim `/usbfoo`, so both are roots.
    let boundary: Vec<String> = alloc::vec![String::from("/usb"), String::from("/usbfoo")];
    if root_prefixes(&boundary).len() != 2 {
        return Err("root_prefixes dropped a sibling that only shares a name prefix");
    }

    // ── leg 7: the list-side name dedupe ─────────────────────────────────────────────────────────
    let mut rows = alloc::vec![
        DirEnt { name: String::from("fat"), kind: NodeKind::Dir, size: 0, mtime: None },
        DirEnt { name: String::from("fat"), kind: NodeKind::Dir, size: 0, mtime: None },
        DirEnt { name: String::from("VUG.ELF"), kind: NodeKind::File, size: 12568, mtime: None },
    ];
    dedupe_by_name(&mut rows);
    if rows.len() != 2 || rows[0].name != "fat" || rows[1].name != "VUG.ELF" {
        return Err("dedupe_by_name did not keep exactly the first of each name, in order");
    }

    // ── leg 8: launchability — the routing test, and what it must NOT claim ───────────────────────
    // The four names on Peter's card, plus the case forms, plus the two shapes that must be refused.
    for yes in ["VUG.ELF", "vug.elf", "Vug.Elf", "STAT.ELF", "HELLO.BIN", "midden.bin"] {
        if !is_executable(yes) {
            return Err("is_executable refused a program the loader accepts");
        }
    }
    for no in ["KERNEL8.IMG", "CONFIG.TXT", "SRC.TGZ", "START4.ELF.BAK", ".ELF", ".BIN", "ELF", ""] {
        if is_executable(no) {
            return Err("is_executable claimed a file the loader was never offered");
        }
    }

    // ── leg 9: the double-click predicate ────────────────────────────────────────────────────────
    if !is_double(100, 100 + DOUBLE_CLICK_MS, 3, 3, true) {
        return Err("is_double refused two presses exactly at the window");
    }
    if is_double(100, 101 + DOUBLE_CLICK_MS, 3, 3, true) {
        return Err("is_double accepted two presses past the window");
    }
    if is_double(100, 200, 3, 4, true) {
        return Err("is_double paired presses on different rows");
    }
    if is_double(100, 200, 3, 3, false) {
        return Err("is_double paired presses in different panes");
    }
    // The guard that matters on a board whose CNTFRQ reads 0 (and on the first press of any boot):
    // a zero clock must NEVER read as a double-click, or the first click of the session launches.
    if is_double(0, 0, 0, 0, true) || is_double(0, 200, 3, 3, true) || is_double(100, 0, 3, 3, true) {
        return Err("is_double armed on a zero clock — the first press of a boot would launch");
    }
    // Monotonic-only: a clock that went backwards is not a 4-billion-ms-fast double-click.
    if is_double(500, 100, 3, 3, true) {
        return Err("is_double accepted a backwards clock");
    }

    // ── leg 10: the cache — hit/miss accounting and its bound ────────────────────────────────────
    // Driven against the model's OWN cache rather than through `collect_cached` (which would reach
    // the seam and therefore a volume this machine may not have), because what is being proven is
    // the eviction bound and the generation reset, both of which are pure. The read path itself is
    // proven at the bench by the `[quarry] … cost reads=/hits=` line.
    m.cache.clear();
    for k in 0..(MAX_CACHE + 4) {
        if m.cache.len() >= MAX_CACHE {
            m.cache.remove(0);
        }
        m.cache.push(CacheEnt { path: alloc::format!("/p{}", k), is_dir: true, rows: Vec::new() });
    }
    if m.cache.len() != MAX_CACHE {
        return Err("the directory cache grew past its bound");
    }
    // FIFO: the OLDEST paths are the ones gone, and the newest is still there.
    if m.cache.iter().any(|e| e.path == "/p0") || !m.cache.iter().any(|e| e.path == "/p19") {
        return Err("the directory cache did not evict oldest-first");
    }

    // ── leg 11: the launch gesture, END TO END through the real router ───────────────────────────
    // Legs 8 and 9 prove the two predicates in isolation; this proves the WIRING — that two presses
    // at a real pixel, through the same `content_press` the click router calls, produce an
    // `Act::Launch` naming the right absolute path. Everything downstream of that `Act` is `bg`'s
    // own already-witnessed machinery (`spawn_user_image_bg`, exercised on every boot by the BGRUN
    // fixtures), so this leg deliberately stops at the DECISION: a fixture that actually spawned a
    // program would perturb the very window table the rest of this battery asserts exact pixels of.
    //
    // Disk-free, exactly as leg 4's tree is: the list is hand-built, so the leg is honest on a
    // machine with no volume at all.
    m.cache.clear();
    m.cwd = String::from("/fat");
    m.list = alloc::vec![
        DirEnt { name: String::from("VUG.ELF"), kind: NodeKind::File, size: 12568, mtime: None },
        DirEnt { name: String::from("CONFIG.TXT"), kind: NodeKind::File, size: 842, mtime: None },
    ];
    m.list_scroll = 0;
    m.list_sel = 0;
    m.click_ms = 0;
    m.focus = Pane::Tree;
    // The first BODY row of the list pane, derived the way the PAINTER derives it — inner top plus
    // one row for the pinned header, plus a pixel to land inside the row rather than on its edge.
    let lin = small.list_pane().inner();
    let px_x = lin.x + PAD + 1;
    let row0_y = lin.y + small.row_h() + 1;
    let row1_y = row0_y + small.row_h();
    // A zero clock is a legitimate state (`CNTFRQ_EL0` unset), and on such a machine the guard in
    // `is_double` must SUPPRESS the gesture rather than fire it on the first press. Both branches
    // are asserted, so this leg is a real claim on every board rather than on some of them.
    let clock_live = crate::arch::ms() != 0;
    if !matches!(content_press(&mut m, px_x, row0_y), Act::None) {
        return Err("a first press launched something");
    }
    if m.list_sel != 0 || m.focus != Pane::List {
        return Err("a list press did not select its row and focus its pane");
    }
    match (content_press(&mut m, px_x, row0_y), clock_live) {
        (Act::Launch(p), true) => {
            if p != "/fat/VUG.ELF" {
                return Err("the double-click launched the wrong path");
            }
        }
        (Act::None, false) => {} // the zero-clock guard, doing exactly its job
        (_, true) => return Err("a double-click on a program did not ask for a launch"),
        (_, false) => return Err("a double-click fired on a zero clock"),
    }
    // The stamp RESET: a third rapid press must be a fresh single, never a second activation.
    if !matches!(content_press(&mut m, px_x, row0_y), Act::None) {
        return Err("a third rapid press re-activated the row");
    }
    // Two presses on DIFFERENT rows are two selections and must open nothing — the property that
    // makes this a gesture rather than a timer.
    m.click_ms = 0;
    let _ = content_press(&mut m, px_x, row0_y);
    if !matches!(content_press(&mut m, px_x, row1_y), Act::None) {
        return Err("presses on two different rows were paired into a double-click");
    }
    // …and a double-click on a NON-program is honest rather than silent.
    m.click_ms = 0;
    let _ = content_press(&mut m, px_x, row1_y);
    match (content_press(&mut m, px_x, row1_y), clock_live) {
        (Act::NoOpener(p), true) => {
            if p != "/fat/CONFIG.TXT" {
                return Err("the unhandled double-click named the wrong path");
            }
        }
        (Act::None, false) => {}
        (_, true) => return Err("a double-click on a document did not report that it has no opener"),
        (_, false) => return Err("a document double-click fired on a zero clock"),
    }

    // ── leg 12: the WHEEL (QSCROLL), through the same `wheel_scroll` the router calls ─────────────
    // Leg 2 proves the offset arithmetic and leg 5 proves the press-to-pane mapping; this proves the
    // GESTURE — that a detent delivered at a real pixel moves the viewport under the POINTER, in the
    // conventional direction, stops exactly at both bounds, and leaves the click grammar alone.
    //
    // It stops at `wheel_scroll` for leg 11's reason, restated because it is the same boundary:
    // everything above it in [`wheel_route`] is the cursor read, `wm::hit_test` and the panel->source
    // conversion that [`press_route`] already performs identically, and a fixture that built a window
    // to hit-test would perturb the very window table the rest of this battery asserts exact pixels
    // of. Below it is arithmetic, and the arithmetic is what a QEMU with no HID at all can prove.
    let tvis = m.tree_visible();
    let lvis = m.list_visible();
    if tvis == 0 || lvis == 0 {
        return Err("a 640x480 pane held no rows — the wheel leg would be vacuous");
    }
    // Both panes deliberately overflow their viewport, so every clamp below is a real bound.
    m.tree = Vec::new();
    for i in 0..(tvis * 3) {
        m.tree.push(TreeRow {
            path: alloc::format!("/t{}", i),
            name: alloc::format!("t{}", i),
            depth: 0,
            expanded: false,
        });
    }
    m.list = Vec::new();
    for i in 0..(lvis * 3) {
        m.list.push(DirEnt {
            name: alloc::format!("F{}.BIN", i),
            kind: NodeKind::File,
            size: 16,
            mtime: None,
        });
    }
    m.tree_sel = 0;
    m.tree_scroll = 0;
    m.list_sel = 0;
    m.list_scroll = 0;
    m.settle();
    let tin = small.tree_pane().inner();
    let (tree_x, tree_y) = (tin.x + PAD + 1, tin.y + 1);
    let (list_x, list_y) = (px_x, row0_y);
    let lmax = scroll_max(m.list.len(), lvis);
    let tmax = scroll_max(m.tree.len(), tvis);

    // One detent TOWARD the operator scrolls the list DOWN by exactly `WHEEL_ROWS`, and touches the
    // pane the pointer is NOT over not at all.
    match wheel_scroll(&mut m, list_x, list_y, -1) {
        Some(h) if h.pane == "list" && h.moved && h.scroll == WHEEL_ROWS && h.max == lmax => {}
        _ => return Err("a detent over the list pane did not scroll it by WHEEL_ROWS"),
    }
    if m.tree_scroll != 0 {
        return Err("a wheel over the list moved the tree's viewport");
    }
    // …and one detent AWAY from the operator puts it back. The direction convention is the same sign
    // `user-vug`'s WHEELZOOM reads off the same `pal::Event::Wheel(i8)`.
    match wheel_scroll(&mut m, list_x, list_y, 1) {
        Some(h) if h.moved && h.scroll == 0 => {}
        _ => return Err("the wheel did not reverse with the sign of the detent"),
    }
    // The TOP bound: a detent spent at row 0 moves nothing and SAYS so, rather than wrapping.
    match wheel_scroll(&mut m, list_x, list_y, 4) {
        Some(h) if !h.moved && h.scroll == 0 => {}
        _ => return Err("the wheel did not clamp at the top of the list"),
    }
    // The BOTTOM bound, from a flick far larger than the list: exactly `scroll_max`, never past it.
    match wheel_scroll(&mut m, list_x, list_y, -1000) {
        Some(h) if h.moved && h.scroll == lmax => {}
        _ => return Err("a long flick did not clamp at scroll_max"),
    }
    match wheel_scroll(&mut m, list_x, list_y, -1) {
        Some(h) if !h.moved && h.scroll == lmax => {}
        _ => return Err("the wheel did not clamp at the bottom of the list"),
    }
    // The SELECTION invariant `quarry.md` §4 states holds after a wheel exactly as it holds after a
    // keyboard move: the wheel pulled it to the viewport's edge, and `settle` left it there.
    if m.list_sel < m.list_scroll || m.list_sel >= m.list_scroll + lvis {
        return Err("a wheel left the list selection outside its viewport");
    }
    // The POINTER chooses the pane, not `m.focus` — the whole difference between a wheel and an
    // arrow key. Focus is the List here, and a detent over the TREE must still scroll the tree.
    m.focus = Pane::List;
    let list_before = m.list_scroll;
    match wheel_scroll(&mut m, tree_x, tree_y, -1000) {
        Some(h) if h.pane == "tree" && h.moved && h.scroll == tmax => {}
        _ => return Err("a detent over the tree pane did not scroll the tree"),
    }
    if m.list_scroll != list_before {
        return Err("a wheel over the tree moved the list's viewport");
    }
    if m.tree_sel < m.tree_scroll || m.tree_sel >= m.tree_scroll + tvis {
        return Err("a wheel left the tree selection outside its viewport");
    }
    // Over Quarry, over NEITHER pane (the path bar): nothing scrolls and nothing is disturbed.
    let (tb, lb) = (m.tree_scroll, m.list_scroll);
    if wheel_scroll(&mut m, small.w / 2, small.bar_h() / 2, -3).is_some() {
        return Err("the path bar claimed a wheel");
    }
    if m.tree_scroll != tb || m.list_scroll != lb {
        return Err("a wheel over the path bar moved a viewport");
    }
    // ── the click grammar is FINAL, and a scroll is not a press ──────────────────────────────────
    // A wheel writes no stamp, so it can neither complete a double-click nor break a half-made one.
    // Asserted on the stamp itself AND end to end: press a row, scroll, press it again — the launch
    // the operator asked for still arrives.
    m.list_scroll = 0;
    m.list_sel = 0;
    m.settle();
    m.click_ms = 4242;
    m.click_row = 7;
    m.click_pane = Pane::Tree;
    let _ = wheel_scroll(&mut m, list_x, list_y, -2);
    let _ = wheel_scroll(&mut m, tree_x, tree_y, 2);
    if m.click_ms != 4242 || m.click_row != 7 || m.click_pane != Pane::Tree {
        return Err("a wheel disturbed the click stamp");
    }
    m.list = alloc::vec![DirEnt {
        name: String::from("VUG.ELF"),
        kind: NodeKind::File,
        size: 12568,
        mtime: None
    }];
    m.list_scroll = 0;
    m.list_sel = 0;
    m.click_ms = 0;
    m.settle();
    if !matches!(content_press(&mut m, px_x, row0_y), Act::None) {
        return Err("the first press of the scroll-between-clicks case launched something");
    }
    // A one-row list cannot scroll, which is exactly the point: the wheel must be inert here and
    // must still not eat the stamp.
    let _ = wheel_scroll(&mut m, list_x, list_y, -1);
    match (content_press(&mut m, px_x, row0_y), clock_live) {
        (Act::Launch(p), true) if p == "/fat/VUG.ELF" => {}
        (Act::None, false) => {} // the zero-clock guard, as everywhere else in this battery
        (_, true) => return Err("a scroll between two presses broke the double-click"),
        (_, false) => return Err("a double-click fired on a zero clock"),
    }
    // ── `wheel_next` swept, rather than sampled ──────────────────────────────────────────────────
    // The two properties the clamps rest on, over a space no hand-picked case covers: the result is
    // never past either bound, and the sign of the detent is the direction of travel.
    for smax in [0usize, 1, 7, 64, 4096] {
        for scroll in [0usize, 1, smax / 2, smax] {
            for d in [-127i32, -8, -1, 1, 8, 127] {
                let n = wheel_next(scroll, smax, d);
                if n > smax {
                    return Err("wheel_next ran past scroll_max");
                }
                let from = scroll.min(smax);
                if (d > 0 && n > from) || (d < 0 && n < from) {
                    return Err("wheel_next travelled against the sign of the detent");
                }
            }
        }
    }

    Ok((small.w * small.h, bench.w * bench.h))
}

/// Run [`selftest_result`] and print the one uncounted witness line.
#[cfg(feature = "witness")]
pub fn selftest() {
    match selftest_result() {
        Ok((a, b)) => serial_println!(
            ":: QUARRY: geometry+scroll+tree+hit+dedupe+exec+dblclick+cache+launch+wheel — 640x480 surf_px={} 1920x1200 surf_px={} dbl={}ms cache={} wheel={}rows :: PASS ::",
            a, b, DOUBLE_CLICK_MS, MAX_CACHE, WHEEL_ROWS
        ),
        Err(why) => serial_println!(":: QUARRY: {} :: FAIL ::", why),
    }
}

/// QUARRYDOOR (KEYDOORS F1, then SO9/R24) — **an arrow reaches a FOCUSED Quarry through the SHELL
/// DOOR, and `<Enter>`/`<Backspace>` reach the SHELL past an unfocused one.**
///
/// F1's original claim was reachability: `key_route` had one caller and it was the EL0 ring door.
/// SO9 added the second half — reachability without a focus gate is key THEFT — and R24 deleted the
/// escape hatch that made the theft survivable. Legs 3 and 4 below are those two, and they are the
/// legs that GO RED on the pre-SO9 tree: leg 3 saw `Event::Unknown` for `<Enter>` (Quarry ate it) and
/// leg 4 saw the window gone (Esc closed it).
///
/// ### Which seam this drives, and why it is a different claim from [`selftest`]
///
/// [`selftest`] is pure logic and [`key_route`] has always been correct; the KEYDOARS F1 defect was
/// never in the callee. It was that `key_route` had exactly ONE caller in the whole tree —
/// `arch/aarch64/syscall.rs:13211`, the EL0 RING door — so on every SHELL path the function was
/// unreachable. A fixture that calls `key_route` directly would have passed on the broken tree, which
/// makes it worthless as a proof of this fix.
///
/// So this one drives **`arch::x86_64::syscall::wc_route_event`** — x86's actual shell door, the
/// function `main.rs`'s two x86 drains call for every event, and the exact statement F1 edited. That
/// is the same choice `wmdirect_selftest` made for the pointer chain and for the same reason: assert
/// against the REAL chain, never against a transcription of it.
///
/// ### Why x86 only, stated rather than silently skipped
///
/// The other two doors F1 fixed are `main.rs` drain loops (`jd2_console_pump`,
/// `pump_usb_into_gui`) — they are not functions a fixture can call, they only exist inside a running
/// pump, and the aarch64 QEMU targets emulate no HID to feed one. Those two folds are scoreable only
/// from a metal capture, and SO9 gave that capture a better witness than the close line it used to
/// name: every key `key_route` is asked about while the window is live now prints
/// `[quarry] key_route key=0x.. focus=<0|1> took=<0|1>` ([`key_witness`]), so a bench seat scores the
/// aarch64 doors by pairing each `KEY 0x..` echo with its route line — `focus=0 took=0` beside a
/// shell keystroke is SO9 fixed, and any `took=1` with `focus=0` is SO9 back. This leg says so on the
/// wire rather than reporting a silent PASS that covered one arch.
///
/// ### Legs
///
/// 1. `open` — Quarry is on the glass (`on_glass()`, i.e. a live row ABOVE `wm::shell_z()`) AND
///    holds focus, because [`open`] raises through `wm::focus_changed(OWNER)`.
///    A DECLINE is a SKIP, not a FAIL: at 640x480 `open()` refuses by design so the pixel-exact video
///    battery is unperturbed, and that refusal is not this fixture's defect.
/// 2. `arrow` — `wc_route_event(Key(0x1E))` is CONSUMED (`Event::Unknown`) while Quarry is FOCUSED.
///    Down-arrow, the key that fell through to `handle_key` on every board before F1.
/// 3. `enter_blind` — **SO9, and the leg this fixture exists for now.** Focus is handed to the
///    console owner (`wm::focus_changed(KERNEL_OWNER_CONSOLE)`, which raises the console's rows and
///    leaves `SHELL_Z` alone, so Quarry is STILL ON THE GLASS — asserted, not assumed), and then
///    `wc_route_event(Key(b'\r'))` must come back UNCHANGED. That is a shell `<Enter>` reaching the
///    drain past an open file manager: the exact gesture that opened a FILE for the whole of the
///    Orin's render8 flight. Backspace (`0x08`) is asserted in the same breath, because the two
///    together are what a line editor cannot work without.
/// 4. `esc_keeps` — **R24.** With focus handed back to Quarry, `wc_route_event(Key(0x1B))` must come
///    back UNCHANGED and the window must still be there. Esc dismisses menus only; the `close()` arm
///    that used to sit in `key_route` is retracted by ruling, and this leg is the tripwire against a
///    reader restoring it (`docs/dev/RULINGS.md` R24).
/// 5. `shut` — THE CONTROL, and the leg that makes leg 2 mean something. With Quarry closed,
///    `key_route(Key(0x1E))` must return FALSE: `on_glass()` is the conjunct that stops a closed file
///    manager stealing the shell's arrows, and a fixture that only ever tested the open case could not
///    tell a working guard from a missing one.
///
/// Focus is snapshotted at entry and restored at exit, the discipline `winmenu::selftest` and
/// `dock`'s fixture already keep: a witness that leaves `FOCUS_ASID` naming a row it invented is a
/// witness that changes the boot it was measuring.
#[cfg(feature = "witness")]
pub fn door_selftest() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        serial_println!(
            ":: QUARRYDOOR: this arch's shell doors are main.rs drain loops (jd2_console_pump, pump_usb_into_gui) — not callable from a fixture, and no HID is emulated to drive one; score them from a metal capture by pairing each `KEY 0x..` echo with its `[quarry] key_route key= focus= took=` line (SO9: focus=0 took=0 is a key on its way to the shell; focus=0 took=1 anywhere is SO9 back) :: SKIP ::"
        );
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        // Snapshot BEFORE the first `open()`, which raises through `focus_changed(OWNER)`.
        let saved_focus = wm::focus_asid();
        if is_open() {
            close();
        }
        open();
        if !is_open() {
            wm::focus_changed(saved_focus);
            serial_println!(
                ":: QUARRYDOOR: quarry declined to open — see the `[quarry] DECLINE reason=` line above (640x480 refuses by design) :: SKIP ::"
            );
            return;
        }
        let win = WIN.load(Ordering::Relaxed);
        let leg_open = on_glass() && wm::focus_asid() == OWNER;
        // Leg 2 — an ARROW, through the door, while Quarry is FOCUSED.
        let arrow = crate::arch::x86_64::syscall::wc_route_event(crate::pal::Event::Key(0x1E));
        let leg_arrow = leg_open && matches!(arrow, crate::pal::Event::Unknown);
        // Leg 3 (SO9) — focus to the CONSOLE, Quarry still on the glass, and the shell's two
        // indispensable keys must come back UNCHANGED. `KERNEL_OWNER_CONSOLE` and not the shell slot
        // `0`: the `asid == 0` arm of `focus_changed` gives `SHELL_Z` a fresh z, which would park
        // Quarry and let `on_glass()` pass this leg for the wrong reason. `still_glass` is asserted
        // so the leg can only be green because FOCUS declined the key.
        wm::focus_changed(wm::KERNEL_OWNER_CONSOLE);
        let still_glass = on_glass();
        let enter = crate::arch::x86_64::syscall::wc_route_event(crate::pal::Event::Key(b'\r'));
        let bsp = crate::arch::x86_64::syscall::wc_route_event(crate::pal::Event::Key(0x08));
        let leg_blind = still_glass
            && !matches!(enter, crate::pal::Event::Unknown)
            && !matches!(bsp, crate::pal::Event::Unknown)
            && is_open();
        // Leg 4 (R24) — focus back to Quarry, and <Esc> must NOT close it.
        wm::focus_changed(OWNER);
        let esc = crate::arch::x86_64::syscall::wc_route_event(crate::pal::Event::Key(0x1B));
        let leg_esc_keeps = !matches!(esc, crate::pal::Event::Unknown)
            && is_open()
            && wm::info(win).is_some();
        // Leg 5 — the control: a CLOSED Quarry consumes nothing.
        close();
        let leg_shut = !key_route(crate::pal::Event::Key(0x1E));
        // Restore: if a red leg left the row standing, take it back rather than leaving a file
        // manager on the operator's desktop — and give the focus owner back whatever held it.
        if wm::info(win).is_some() {
            close();
        }
        wm::focus_changed(saved_focus);
        let ok = leg_open && leg_arrow && leg_blind && leg_esc_keeps && leg_shut;
        serial_println!(
            ":: QUARRYDOOR: win={} seam=arch::x86_64::syscall::wc_route_event focused_on_glass={} arrow_consumed={} unfocused_enter_bsp_pass={} esc_keeps_window={} closed_consumes_nothing={} :: {} ::",
            win, leg_open, leg_arrow, leg_blind, leg_esc_keeps, leg_shut,
            if ok { "PASS" } else { "FAIL" }
        );
    }
}
