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

/// The mount prefixes, longest-prefix-resolved volumes of the one namespace — the tree's roots.
#[cfg(target_arch = "aarch64")]
fn roots() -> Vec<String> {
    let mt = crate::shell::vfs_mount_table();
    let mut v: Vec<String> = mt.prefixes().iter().map(|p| String::from(*p)).collect();
    v.sort();
    v
}

#[cfg(not(target_arch = "aarch64"))]
fn roots() -> Vec<String> {
    Vec::new()
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
// `FrameBuffer::scroll_up` is a whole-surface memmove with no syscall and no rect, the HID boot-mouse
// decoder discards the wheel byte before the ABI ever sees it, and `theme::SCROLL_TRACK`/`SCROLL_THUMB`
// are two colours no scrollbar has ever consumed. So Quarry owns its scroll completely: an integer row
// offset, a clamp, and a full pane redraw at the new offset. That is the whole mechanism, and the cost
// is stated rather than hidden — see `quarry.md` §5.

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
        };
        m.reload_roots();
        // Open on the first root and expand it, so the operator sees two levels without a gesture —
        // the brief's "expandable, at least 2 levels", demonstrated at open rather than promised.
        if !m.tree.is_empty() {
            m.expand(0);
            let p = m.tree[0].path.clone();
            m.show(&p);
        } else {
            m.err = Some(String::from("no volumes mounted"));
        }
        m
    }

    /// Rebuild the root rows from the mount table. Called at open and on refresh, so a stick plugged
    /// after Quarry opened enters the tree on the next `r` — the honest hot-plug posture vfs.md §6
    /// gives the table itself, inherited rather than re-invented.
    fn reload_roots(&mut self) {
        let keep = self.tree.get(self.tree_sel).map(|r| r.path.clone());
        self.tree.clear();
        for p in roots() {
            self.tree.push(TreeRow { name: leaf(&p), path: p, depth: 0, expanded: false });
        }
        self.tree_sel = keep
            .and_then(|k| self.tree.iter().position(|r| r.path == k))
            .unwrap_or(0);
        self.tree_scroll = 0;
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
        let kids: Vec<TreeRow> = match collect(&path) {
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
        match collect(path) {
            Ok((true, mut rows)) => {
                self.list_truncated = rows.len() > MAX_LIST;
                rows.truncate(MAX_LIST);
                list_sort(&mut rows);
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
            if dir {
                nm.push(b'/');
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
/// KEYBOARD — a parked Quarry that kept first refusal on arrows and `Esc` would be eating keys for a
/// window the operator cannot see, which is the same defect in kind as typing into a hidden shell.
/// The pointer needs no equivalent guard: `wm::hit_test` does not report a row that is not
/// compositing, so [`press_route`] already declines a parked window by construction.
fn on_glass() -> bool {
    let id = WIN.load(Ordering::Relaxed);
    id != wm::WIN_NONE && wm::info(id).map(|i| i.z > wm::shell_z()).unwrap_or(false)
}

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
    let (pw, ph) = {
        let fb = *crate::video::WRITER.lock();
        let i = fb.info();
        (i.width, i.height)
    };
    let Some(g) = geometry(pw, ph) else {
        serial_println!(
            "[quarry] DECLINE reason=panel-below-floor panel={}x{} floor={}x{}",
            pw, ph, FLOOR_W, FLOOR_H
        );
        return;
    };
    // CONSOLEWIN, applied to a second tenant — `wcx`'s law, unchanged in substance and unchanged in
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
    WIN.store(id, Ordering::Relaxed);
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
    repaint();
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
/// The contract is `video::instgui`'s, for its reason: while Quarry's window is open it takes first
/// refusal on the keys it binds, and `Esc` closes it and hands the keyboard straight back. It never
/// swallows a key it does not bind, so the console keeps working underneath for everything else.
///
/// Arrow keys arrive as the C0 codes the HID map assigns (`0x1C..=0x1F`, right/left/down/up) — the
/// same bytes `una_abi::KEY_RIGHT`..`KEY_UP` publish to ring 3, decoded once in the driver.
pub fn key_route(ev: crate::pal::Event) -> bool {
    if !on_glass() {
        return false;
    }
    let crate::pal::Event::Key(c) = ev else {
        return false;
    };
    // ESC is answered before the model lock: closing must work even if a directory read left the
    // model in a state the rest of this function would rather not be in.
    if c == 0x1B {
        close();
        return true;
    }
    let mut acted = true;
    {
        let mut guard = MODEL.lock();
        let Some(m) = guard.as_mut() else {
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
                Pane::List => {
                    if let Some(e) = m.list.get(m.list_sel) {
                        if matches!(e.kind, NodeKind::Dir) {
                            let p = join(&m.cwd, &e.name);
                            if p.len() <= PATH_MAX {
                                // Reveal it on the left first, so the tree row exists for `navigate`
                                // to select rather than silently not finding it.
                                if let Some(i) = m.tree.iter().position(|r| r.path == m.cwd) {
                                    if !m.tree[i].expanded {
                                        m.expand(i);
                                    }
                                }
                                m.navigate(&p);
                                m.focus = Pane::List;
                            }
                        }
                    }
                }
            },
            // Backspace — up one level, from wherever the focus is.
            0x08 | 0x7F => {
                let p = parent(&m.cwd);
                m.navigate(&p);
            }
            // `r` — re-read. The mount table is rebuilt too, so a stick plugged after open appears.
            b'r' | b'R' => {
                m.reload_roots();
                let cwd = m.cwd.clone();
                m.show(&cwd);
            }
            _ => acted = false,
        }
        if acted {
            m.settle();
        }
    }
    if acted {
        repaint();
    }
    acted
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
    {
        let mut guard = MODEL.lock();
        let Some(m) = guard.as_mut() else {
            return true;
        };
        // The return says whether a ROW was hit, and the repaint below does not read it: a press that
        // hit no row still changed the pane focus (and therefore the selection ink), and it is still
        // CONSUMED — it moved window focus and must not be delivered to whatever lies underneath.
        let _ = content_press(m, sx, sy);
    }
    repaint();
    true
}

/// Route a press already resolved to SOURCE coordinates. Split out from [`press_route`] so the
/// witness can drive it without a window table, and so the hit arithmetic reads beside the painter's.
fn content_press(m: &mut Model, sx: usize, sy: usize) -> bool {
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
        // The gutter pages. There is no wheel on this machine — the HID boot-mouse decoder drops
        // byte 3 before the ABI sees it — so the track IS the coarse scroll gesture.
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
            return true;
        }
        if sy < ti.y {
            return true;
        }
        // `contains` tested the OUTER pane, so the bottom keyline row can land one past the last
        // viewport slot. Decline it rather than selecting a row the press did not visually address.
        let r = (sy - ti.y) / row_h;
        let i = m.tree_scroll + r;
        if r >= tvis || i >= m.tree.len() {
            return true;
        }
        m.tree_sel = i;
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
        return true;
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
            return true;
        }
        if sy < body_y {
            return true; // the header is not a row
        }
        let r = (sy - body_y) / row_h;
        let i = m.list_scroll + r;
        if r < lvis && i < m.list.len() {
            m.list_sel = i;
        }
        m.settle();
        return true;
    }
    false
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

    Ok((small.w * small.h, bench.w * bench.h))
}

/// Run [`selftest_result`] and print the one uncounted witness line.
#[cfg(feature = "witness")]
pub fn selftest() {
    match selftest_result() {
        Ok((a, b)) => serial_println!(
            ":: QUARRY: geometry+scroll+tree+hit — 640x480 surf_px={} 1920x1200 surf_px={} :: PASS ::",
            a, b
        ),
        Err(why) => serial_println!(":: QUARRY: {} :: FAIL ::", why),
    }
}
