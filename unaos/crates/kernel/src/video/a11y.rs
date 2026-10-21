//! CHARTER: Kernel — wm (A11YTREE B675, BP-bar-22: the accessibility role/name tree read from the window table and the furniture's own tables, `BUS_VERB_A11Y_TREE`'s body, `tests a11ytree`)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A11YTREE (rmbp-ledger B675). BP-bar-22: *"Accessibility lives in the window table: the role/name tree,
//! focus-visible ring, reduce-motion and RTL come from the rows the compositor paints."* This module is the tree.
//!
//! Declared at `wm.rs`'s tail with `#[path]` (fbdirect's and desktop's precedent), so it is `wm::a11y`, reads the
//! window rows' own fields and `video/mod.rs` does not move. Both desktops (x86 `wc`, aarch64 `desktop_firmware`;
//! RAISEARM64 LEDGER SR175): each arch's `sys_msend` dispatcher answers the verb through [`bus_tree`].
//!
//! THE TREE (one flat list, a parent before its children, ids 1-based in that order):
//! * `1` the DESKTOP — the panel's rect;
//! * the MENU BAR (when `menubar::strip_rect` has one — none under Softy) and its title boxes from
//!   `winmenu::bar_boxes`, THE accessor its painter and its hit test read (the app menu, the published titles, the
//!   Window menu; the open one SELECTED);
//! * the DOCK (`dock::strip_rect`) and its tiles from `dock::a11y_tiles` — the settled model `press_at` routes over,
//!   laid out by the same `Layout`, each tile its own box and the painter's caption. Under Softy (the taskbar keeps
//!   its own boxes) one tile per pinned app from the dock's pin table (`DP_PINS` x `dp_is_pinned` x
//!   `installed_names`) on the strip's rect with [`A11Y_F_PARENT_BOX`];
//! * one node per live window row (no compat row, no owner-0 row), FRONT TO BACK (z descending, ties by id —
//!   the compositor's order read backwards): the role is the OWNER's — a ring-3 program's window, or kernel
//!   furniture (`is_kernel_owner`) — the name the row's title, else the owner's armed program name; the rect the
//!   outer box (chrome included); HIDDEN when parked or below the shell (`above_shell`, the compositor's own test);
//!   FOCUSED for the frontmost row of `focus_asid()` — `wl_focused`'s rule, so the tree's focus IS wm's focus;
//! * under the Settings window (the row the WINID registry's `settings` cell names), one TAB node per tab row
//!   from `settings::TAB_NAMES`, the shown tab SELECTED, the column's rect with [`A11Y_F_PARENT_BOX`].
//!
//! THE WALK is read-only: one `table()` hold, dereferenced immutably (no `DMG_GEN` bump), copying into a buffer
//! reserved BEFORE the hold — nothing allocates, prints or blocks under `TABLE` (the TableGuard rule) — and no WCSER
//! state is touched. Names and furniture are gathered after the hold drops.
//!
//! WIRE: `una_abi`'s A11YTREE block (8-byte header, 64-byte nodes, 63 per page). [`bus_tree`] is the verb's body;
//! [`selftest`] is `tests a11ytree`:
//! `:: A11YTREE: windows=<n> nodes=<n> unnamed=<n> focus=<ok|FAIL> ring_px=<n> s2=<n> stops=<rung>/<n> wire=<ok|why> -> PASS|FAIL ::`.

use alloc::vec::Vec;

use una_abi::{
    a11y_hdr_put, a11y_name_cut, a11y_node_put, a11y_u16, A11yNode, A11Y_F_FOCUSED, A11Y_F_HIDDEN, A11Y_F_PARENT_BOX,
    A11Y_F_SELECTED, A11Y_HDR_LEN, A11Y_NAME_MAX, A11Y_NODE_LEN, A11Y_PAGE_MAX, A11Y_ROLE_DESKTOP, A11Y_ROLE_DOCK,
    A11Y_ROLE_DOCK_TILE, A11Y_ROLE_FURNITURE, A11Y_ROLE_MENU, A11Y_ROLE_MENUBAR, A11Y_ROLE_TAB, A11Y_ROLE_WINDOW,
};

use super::{WinId, MAX_TITLE, WIN_NONE};

/// One window row as the walk copies it out of the table.
#[derive(Clone, Copy)]
struct Row {
    id: WinId,
    owner: u64,
    z: u32,
    hidden: bool,
    bx: (usize, usize, usize, usize),
    title: [u8; MAX_TITLE],
    len: usize,
}

/// A built node and, for a window node, the row it names (the fixture's focus check; never on the wire).
#[derive(Clone, Copy)]
struct Built {
    node: A11yNode,
    win: WinId,
}

/// The tree as it stands: its nodes and the id of the focused one (0 = none).
struct Tree {
    nodes: Vec<Built>,
    focus: u16,
}

impl Tree {
    /// Append a node under `parent`; returns its id. `name` is cut to [`A11Y_NAME_MAX`] at a character boundary.
    fn push(&mut self, parent: u16, role: u8, flags: u8, name: &[u8], bx: (usize, usize, usize, usize), win: WinId) -> u16 {
        let id = a11y_u16(self.nodes.len() + 1);
        let n = a11y_name_cut(name);
        let mut nm = [0u8; A11Y_NAME_MAX];
        nm[..n].copy_from_slice(&name[..n]);
        let node = A11yNode {
            id,
            parent,
            role,
            flags,
            name_len: n as u8,
            name: nm,
            x: a11y_u16(bx.0),
            y: a11y_u16(bx.1),
            w: a11y_u16(bx.2),
            h: a11y_u16(bx.3),
        };
        self.nodes.push(Built { node, win });
        id
    }
}

/// The WINID registry's `settings` cell: the Settings window's id, or [`WIN_NONE`].
fn settings_win() -> WinId {
    let snap = *super::HOLDERS.lock();
    snap.iter()
        .find(|h| h.tag == "settings")
        .and_then(|h| h.cell)
        .map_or(WIN_NONE, |c| c.load(core::sync::atomic::Ordering::Acquire))
}

/// THE WALK: every live, non-compat, owned row, front to back. Read-only; see the module doc.
fn rows() -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    out.reserve(super::slots() + 4);
    let shell = super::shell_z();
    {
        let t = super::table();
        let tb: &super::Table = &t;
        for r in tb.rows.iter() {
            if out.len() == out.capacity() {
                break; // never grow under the lock; a table past the high-water mark shows on `windows=` vs the count
            }
            if r.used && !r.compat && r.owner_asid != 0 {
                out.push(Row {
                    id: r.id,
                    owner: r.owner_asid,
                    z: r.z,
                    hidden: r.z == super::PARKED_Z || !super::above_shell(r, shell),
                    bx: super::outer_box(r),
                    title: r.title,
                    len: r.title_len.min(MAX_TITLE),
                });
            }
        }
    }
    out.sort_unstable_by(|a, b| (b.z, b.id).cmp(&(a.z, a.id)));
    for r in out.iter_mut() {
        if r.len == 0 {
            let mut nm = [0u8; MAX_TITLE];
            r.len = super::app_name_of(r.owner, &mut nm);
            r.title = nm;
        }
    }
    out
}

/// Build the whole tree now.
fn build() -> Tree {
    let mut t = Tree { nodes: Vec::new(), focus: 0 };
    let (pw, ph) = crate::video::panel_snapshot().filter(|f| f.is_ready()).map_or((0, 0), |f| (f.width(), f.height()));
    let root = t.push(0, A11Y_ROLE_DESKTOP, 0, b"Desktop", (0, 0, pw, ph), WIN_NONE);
    // The menu bar and its titles: the boxes its painter and its hit test read.
    if let Some(bar) = crate::video::menubar::strip_rect(pw, ph) {
        let mb = t.push(root, A11Y_ROLE_MENUBAR, 0, b"Menu bar", bar, WIN_NONE);
        let s = crate::video::winmenu::bar_boxes(pw, ph);
        for k in 0..s.n.min(crate::video::winmenu::BAR_BOXES_MAX) {
            let f = if s.open == k + 1 { A11Y_F_SELECTED } else { 0 };
            t.push(mb, A11Y_ROLE_MENU, f, s.label_of(k), (s.x[k], s.bar.1, s.w[k], s.bar.3), WIN_NONE);
        }
    }
    // The dock (the taskbar under Softy) and its pinned tiles, from the dock's pin table.
    if let Some(dr) = crate::video::dock::strip_rect(pw, ph) {
        let softy = crate::video::look::is_softy();
        let dk = t.push(root, A11Y_ROLE_DOCK, 0, if softy { b"Taskbar" as &[u8] } else { b"Dock" }, dr, WIN_NONE);
        let mut tiles = Vec::new();
        if crate::video::dock::a11y_tiles(pw, ph, &mut tiles) {
            // The tiles the painter drew and the router hits, each with its own box.
            for (cap, len, bx) in tiles.iter() {
                t.push(dk, A11Y_ROLE_DOCK_TILE, 0, &cap[..*len], *bx, WIN_NONE);
            }
        } else {
            // The taskbar (Softy): its tile boxes are its own, so the dock's pin table names the tiles on the strip's rect.
            let installed = crate::video::dock::installed_names();
            for (i, p) in crate::video::dock::DP_PINS.iter().enumerate() {
                if crate::video::dock::dp_is_pinned(i) && installed.contains(&p.name) {
                    t.push(dk, A11Y_ROLE_DOCK_TILE, A11Y_F_PARENT_BOX, p.name.as_bytes(), dr, WIN_NONE);
                }
            }
        }
    }
    // The windows, front to back; focus is wm's.
    let focus_asid = super::focus_asid();
    let sw = settings_win();
    let mut focused_done = false;
    for r in rows() {
        let role = if super::is_kernel_owner(r.owner) { A11Y_ROLE_FURNITURE } else { A11Y_ROLE_WINDOW };
        let foc = focus_asid != 0 && r.owner == focus_asid && !focused_done; // the frontmost row of the focused owner
        focused_done |= foc;
        let flags = (if foc { A11Y_F_FOCUSED } else { 0 }) | (if r.hidden { A11Y_F_HIDDEN } else { 0 });
        let id = t.push(root, role, flags, &r.title[..r.len], r.bx, r.id);
        if foc {
            t.focus = id;
        }
        if r.id == sw && sw != WIN_NONE && crate::video::settings::is_open() {
            // The tab rows: the column's rect (the window's content origin, `COL_W` logical px at the row's scale).
            let (cx, cy, cw, ch) = super::info(r.id).map_or((0, 0, 0, 0), |i| {
                (i.x, i.y, crate::video::metrics::size(crate::video::settings::COL_W) * i.scale, i.h * i.scale)
            });
            let cur = crate::video::settings::cur_tab();
            for (k, nm) in crate::video::settings::TAB_NAMES.iter().enumerate() {
                let f = A11Y_F_PARENT_BOX | if k == cur { A11Y_F_SELECTED } else { 0 };
                t.push(id, A11Y_ROLE_TAB, f, nm.as_bytes(), (cx, cy, cw, ch), WIN_NONE);
            }
        }
    }
    t
}

/// `-EINVAL`, `-EACCES`.
const EINVAL: i64 = -22;
const EACCES: i64 = -13;

/// The page of `t` starting at node `first` (1-based; 0 reads as 1) into `text`: the header, then the nodes.
fn page(t: &Tree, first: usize, text: &mut Vec<u8>) {
    let total = t.nodes.len();
    let first = first.max(1);
    let count = if first > total { 0 } else { (total - first + 1).min(A11Y_PAGE_MAX) };
    let base = text.len();
    text.resize(base + A11Y_HDR_LEN + count * A11Y_NODE_LEN, 0);
    a11y_hdr_put(a11y_u16(total), a11y_u16(first), count as u16, t.focus, &mut text[base..base + A11Y_HDR_LEN]);
    for k in 0..count {
        let o = base + A11Y_HDR_LEN + k * A11Y_NODE_LEN;
        a11y_node_put(&t.nodes[first - 1 + k].node, &mut text[o..o + A11Y_NODE_LEN]);
    }
}

/// `BUS_VERB_A11Y_TREE`: body empty or `[first lo, first hi]`; the reply text is the header and the page's nodes.
/// `in_session` is the caller's (the dispatcher's `pref_caller_in_session`): outside a session the titles are no
/// one's to read, `-EACCES`. `-EINVAL` a body of any other length.
pub fn bus_tree(in_session: bool, body: &[u8], text: &mut Vec<u8>) -> i64 {
    let first = match body.len() {
        0 => 1,
        2 => u16::from_le_bytes([body[0], body[1]]) as usize,
        _ => return EINVAL,
    };
    if !in_session {
        return EACCES;
    }
    page(&build(), first, text);
    0
}

/// `tests a11ytree`: the tree built, read back through the verb's own body page by page and decoded with una-abi's
/// decoder (`wire=`), its windows counted against a second walk, every node named (`unnamed=`), the focused node
/// the row `wl_focused` names and the only one flagged (`focus=`), and Settings' ring at every Tab stop
/// (`settings::ring_probe`: `ring_px=` the thinnest in device px against 2 logical px at the panel's scale `s2=`,
/// `stops=` rung/all; A11YTREE2 B822: every miss its own `[a11y] ring missing tab=<n>(<name>) stop=<k> row=<name>`).
#[cfg(feature = "witness")]
pub fn selftest() {
    let t = build();
    // wire: every page through the verb's body, decoded, equal to what was built; a bad body and no session refused
    let mut wire: &str = "ok";
    let mut got: Vec<A11yNode> = Vec::new();
    let mut focus_wire = u16::MAX;
    let mut first = 1usize;
    loop {
        let mut text = Vec::new();
        if bus_tree(true, &(first as u16).to_le_bytes(), &mut text) != 0 { wire = "status"; break; }
        let Some((total, f, count, focus)) = una_abi::a11y_hdr_get(&text) else { wire = "header"; break; };
        if total as usize != t.nodes.len() || f as usize != first { wire = "total"; break; }
        focus_wire = focus;
        for k in 0..count as usize {
            match una_abi::a11y_node_get(&text[A11Y_HDR_LEN + k * A11Y_NODE_LEN..]) {
                Some(n) => got.push(n),
                None => wire = "node",
            }
        }
        if count == 0 || got.len() >= total as usize || wire != "ok" { break; }
        first += count as usize;
    }
    if wire == "ok" && (got.len() != t.nodes.len() || got.iter().zip(t.nodes.iter()).any(|(a, b)| *a != b.node)) { wire = "roundtrip"; }
    if wire == "ok" && got.iter().any(|n| n.parent >= n.id || (n.parent == 0) != (n.id == 1)) { wire = "parents"; }
    if wire == "ok" && focus_wire != t.focus { wire = "focus"; }
    let mut sink = Vec::new();
    if wire == "ok" && (bus_tree(true, &[1], &mut sink) != EINVAL || bus_tree(false, &[], &mut sink) != EACCES) { wire = "refusal"; }
    // windows: the tree's window nodes against a second walk of the table
    let windows = t.nodes.iter().filter(|b| matches!(b.node.role, A11Y_ROLE_WINDOW | A11Y_ROLE_FURNITURE)).count();
    let live = rows().len();
    let unnamed = t.nodes.iter().filter(|b| b.node.name_len == 0).count();
    // focus: the flagged node is the row wm's focus names, and the only one
    let flagged = t.nodes.iter().filter(|b| b.node.flags & A11Y_F_FOCUSED != 0).count();
    let focus_ok = match super::wl_focused() {
        Some((win, _)) => flagged == 1 && t.focus != 0 && t.nodes.get(t.focus as usize - 1).is_some_and(|b| b.win == win),
        None => flagged == 0 && t.focus == 0,
    };
    let (stops, rung, ring_px, misses) = crate::video::settings::ring_probe();
    // A11YTREE2 (B822): BP-bar-22's 2 px is logical — the floor at the panel's scale (`ring_px` is device px)
    let floor = crate::video::metrics::size(2);
    let ok = wire == "ok" && windows == live && unnamed == 0 && focus_ok && stops > 0 && rung == stops && ring_px >= floor;
    for (tab, stop, row) in misses.iter() {
        let name = crate::video::settings::TAB_NAMES.get(*tab).copied().unwrap_or("-");
        serial_println!("[a11y] ring missing tab={}({}) stop={} row={}", tab, name, stop, row);
    }
    serial_println!(
        ":: A11YTREE: windows={} nodes={} unnamed={} focus={} ring_px={} s2={} stops={}/{} wire={} -> {} ::",
        windows, t.nodes.len(), unnamed, if focus_ok { "ok" } else { "FAIL" }, ring_px, crate::video::metrics::s2(), rung,
        stops, wire, if ok { "PASS" } else { "FAIL" }
    );
}
