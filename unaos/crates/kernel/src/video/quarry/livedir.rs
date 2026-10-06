// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! QUARRYLIVE (rmbp-ledger B494, flight 26 boot 1) — Peter: "quarry should be live updated and double click item in
//! tree should expand, no? i forget how mac does it". The Mac: a Finder window reflects its folder's changes as they
//! happen, and the list view's disclosure triangle expands a folder IN PLACE.
//!
//! * **LIVE** — no new mechanism: the namespace generation (`fs::NS_GEN`, NSGEN/SR3: one bump per successful
//!   create/write/truncate/unlink/rename/rmdir in `MountTable`) plus the block publish epoch is Quarry's
//!   `volume_gen()`. [`service`] (on Quarry's service pass) compares ONE atomic against the generation it last
//!   listed at — the listing is never polled — and on a move re-lists: the cache is dropped, the tree rebuilt with
//!   every expanded path and the selection kept, the cwd re-shown with the selected row (by name) and the scroll
//!   kept. A vanished cwd falls back to its parent. Bursts coalesce ([`SETTLE_MS`]); a running search defers.
//!   Witness `[quarry] live dir=<d> gen=<n> relisted=<n>`.
//! * **DISCLOSE** — List view only (Icons and Columns unchanged): every folder row carries a triangle; a press on it
//!   splices the folder's children under it (sorted by the folder's key, nested expansion kept). An inline row's
//!   NAME is its path relative to the cwd (`SUB/LEAF`), so every `join(cwd, name)` in Quarry (open, Quick Look,
//!   delete, drag) addresses the right file with no second path model; the painter draws the leaf, indented.
//!   [`resplice`] runs at the tail of every sort, so a re-sort or a relist never scrambles the children.
//!   Witness `[quarry] disclose row=<r> open=<0|1> rows=<n>`.
//! * **Tree double-click** — Peter's literal words: a double-click on a tree row toggles its expansion.
//!
//! Lock order: `MODEL` → `COLS` → [`EXP`] (a leaf). Fixture: `tests quarrylive` ([`selftest`]).
//! Design: `docs/dev/evidence/rmbp-1005/quarrylive.md`.

use super::columns::{compute_meta, layout_with, sort_rows, ColState, COLS};
use super::toolbar::{self, View};
use super::*;
use core::sync::atomic::AtomicU64;

/// One relist per this many ms at most: a copy's chunked writes each move the generation.
const SETTLE_MS: u64 = 250;

/// The generation the window last listed at (`u64::MAX` = not yet seen).
static SEEN: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_MS: AtomicU64 = AtomicU64::new(0);
/// Relists this boot (the fixture's and the census's count).
static RELISTS: AtomicU64 = AtomicU64::new(0);

/// The list view's expanded folders, as paths relative to `cwd`.
struct Exp {
    cwd: String,
    open: Vec<String>,
}
static EXP: crate::sync::Mutex<Exp> = crate::sync::Mutex::new(Exp { cwd: String::new(), open: Vec::new() });

/// The display leaf of a (possibly inline, `SUB/LEAF`) row name.
pub(super) fn leaf_of(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn depth_of(name: &str) -> usize {
    name.bytes().filter(|&b| b == b'/').count()
}

fn is_dir(e: &DirEnt) -> bool {
    matches!(e.kind, NodeKind::Dir)
}

/// Where the name text starts on a list row whose name column starts at `x0`: one triangle column per level.
pub(super) fn name_x(g: &Geom, x0: usize, e: &DirEnt) -> usize {
    x0 + (depth_of(&e.name) + 1) * g.mark_w() + g.ts
}

/// Paint the row's disclosure triangle (folders only) and return where the name text starts.
pub(super) fn mark(px: &mut [u32], g: &Geom, x0: usize, y: usize, row_h: usize, e: &DirEnt, ink: u32) -> usize {
    if is_dir(e) {
        let open = { let x = EXP.lock(); x.open.iter().any(|o| *o == e.name) };
        disclosure(px, g, x0 + depth_of(&e.name) * g.mark_w(), y + row_h / 2 - 4 * g.ts, open, ink);
    }
    name_x(g, x0, e)
}

/// Strip every inline row, then splice the expanded folders' children back under their parents. Called at the tail
/// of `columns::resort` (MODEL and COLS held) — after the fresh listing's sort and after every header re-sort.
pub(super) fn resplice(m: &mut Model, st: &mut ColState) {
    if m.list.iter().any(|e| e.name.contains('/')) {
        let aligned = st.meta.len() == m.list.len();
        let keep: Vec<bool> = m.list.iter().map(|e| !e.name.contains('/')).collect();
        let mut k = keep.iter();
        m.list.retain(|_| *k.next().unwrap_or(&true));
        if aligned {
            let mut k = keep.iter();
            st.meta.retain(|_| *k.next().unwrap_or(&true));
        }
    }
    let open = {
        let mut x = EXP.lock();
        if x.cwd != m.cwd {
            x.cwd = m.cwd.clone();
            x.open.clear();
        }
        x.open.clone()
    };
    if open.is_empty() || toolbar::view() != View::List {
        return;
    }
    let aligned = st.meta.len() == m.list.len();
    let mt = crate::shell::vfs_mount_table();
    let mut i = 0usize;
    while i < m.list.len() {
        let e = &m.list[i];
        if is_dir(e) && open.iter().any(|o| *o == e.name) && m.list.len() < MAX_LIST {
            let rel = e.name.clone();
            let p = join(&m.cwd, &rel);
            if let Ok((true, rows)) = m.collect_cached(&p) {
                let mut kids: Vec<DirEnt> = rows
                    .into_iter()
                    .map(|mut k| {
                        k.name = join(&rel, &k.name);
                        k
                    })
                    .collect();
                dedupe_by_name(&mut kids);
                kids.truncate(MAX_LIST - m.list.len());
                let mut km = compute_meta(&mt, &m.cwd, &kids, &[]);
                sort_rows(&mut kids, &mut km, st.key, st.desc);
                let n = kids.len();
                let tail = m.list.split_off(i + 1);
                m.list.extend(kids);
                m.list.extend(tail);
                if aligned && km.len() == n {
                    let tail = st.meta.split_off(i + 1);
                    st.meta.extend(km);
                    st.meta.extend(tail);
                }
            }
        }
        i += 1; // the spliced children are walked next, so a nested expanded folder splices too
    }
}

/// A list-pane press on row `i` at source x `sx`: on a folder's triangle it toggles the folder in place. `true` when
/// consumed. List view only.
pub(super) fn press(m: &mut Model, sx: usize, i: usize) -> bool {
    if toolbar::view() != View::List {
        return false;
    }
    let Some(e) = m.list.get(i) else { return false };
    if !is_dir(e) {
        return false;
    }
    let g = m.geom;
    let li = g.list_pane().inner();
    let lsb = if m.list.len() > m.list_visible() { SBW() } else { 0 };
    let mut st = COLS.lock();
    let c = layout_with(&g, li, lsb, &st.widths, st.trash, &attrcols::widths());
    let x0 = c.name_x + depth_of(&e.name) * g.mark_w();
    if sx < x0 || sx >= x0 + g.mark_w() {
        return false;
    }
    let name = e.name.clone();
    let open = toggle(&m.cwd, &name);
    resplice(m, &mut st);
    drop(st);
    if let Some(k) = m.list.iter().position(|r| r.name == name) {
        m.list_sel = k;
    }
    serial_println!("[quarry] disclose row={} open={} rows={}", name, open as u8, m.list.len());
    true
}

/// Flip `name`'s expansion in `cwd`'s set (closing a folder closes its expanded descendants). Returns the new state.
fn toggle(cwd: &str, name: &str) -> bool {
    let mut x = EXP.lock();
    if x.cwd != cwd {
        x.cwd = String::from(cwd);
        x.open.clear();
    }
    if x.open.iter().any(|o| o == name) {
        let pre = alloc::format!("{}/", name);
        x.open.retain(|o| o != name && !o.starts_with(&pre));
        false
    } else {
        x.open.push(String::from(name));
        true
    }
}

/// A tree-pane row press that completed a double-click toggles the row's expansion (Peter, flight 26).
pub(super) fn tree_double(m: &mut Model, i: usize, prev_ms: u64, prev_row: usize, prev_tree: bool) {
    if i >= m.tree.len() || !is_double(prev_ms, crate::arch::ms(), prev_row, i, prev_tree) {
        return;
    }
    if m.tree[i].expanded {
        m.collapse(i);
    } else {
        m.expand(i);
    }
    m.click_ms = 0; // consumed: a third press is a fresh single
    serial_println!("[quarry] tree double row={} expanded={}", m.tree[i].path, m.tree[i].expanded as u8);
}

/// Rebuild the tree from its roots, re-expanding every path that was expanded and keeping the selected path.
fn retree(m: &mut Model) {
    let exp: Vec<String> = m.tree.iter().filter(|r| r.expanded).map(|r| r.path.clone()).collect();
    let sel = m.tree.get(m.tree_sel).map(|r| r.path.clone());
    m.tree.retain(|r| r.depth == 0);
    for r in m.tree.iter_mut() {
        r.expanded = false;
    }
    let mut i = 0usize;
    while i < m.tree.len() {
        if exp.iter().any(|p| *p == m.tree[i].path) {
            m.expand(i);
        }
        i += 1;
    }
    m.tree_sel = sel.and_then(|p| m.tree.iter().position(|r| r.path == p)).unwrap_or(0);
}

/// Re-list the open window's folder after a namespace change. Returns the rows now listed.
fn relist(m: &mut Model) -> usize {
    let sel = m.list.get(m.list_sel).map(|e| e.name.clone());
    let (scroll, focus) = (m.list_scroll, m.focus);
    m.invalidate();
    retree(m);
    let cwd = m.cwd.clone();
    m.show(&cwd);
    if m.err.is_some() && cwd != "/" {
        m.navigate(&parent(&cwd)); // the folder itself went away: its parent, as the Finder does
    } else {
        m.list_scroll = scroll;
        if let Some(k) = sel.and_then(|n| m.list.iter().position(|e| e.name == n)) {
            m.list_sel = k;
        }
    }
    m.focus = focus;
    m.settle();
    m.list.len()
}

/// The service pass (rides `live::service`): one atomic compare when nothing changed.
pub fn service() {
    let ng = volume_gen();
    let seen = SEEN.load(Ordering::Acquire);
    if ng == seen || !is_open() {
        if !is_open() {
            SEEN.store(ng, Ordering::Release);
        }
        return;
    }
    let now = crate::arch::ms();
    if seen != u64::MAX && now.saturating_sub(LAST_MS.load(Ordering::Relaxed)) < SETTLE_MS {
        return; // coalesce; `seen` is unchanged, so the change is picked up next pass
    }
    if toolbar::search_active() {
        return; // the list shows search hits; relisting would end the search
    }
    relist_now(ng, now);
}

fn relist_now(ng: u64, now: u64) -> Option<(String, usize)> {
    SEEN.store(ng, Ordering::Release);
    LAST_MS.store(now, Ordering::Relaxed);
    let out = {
        let mut g = MODEL.lock();
        let m = g.as_mut()?;
        let n = relist(m);
        (m.cwd.clone(), n)
    };
    RELISTS.fetch_add(1, Ordering::Relaxed);
    serial_println!("[quarry] live dir={} gen={} relisted={}", out.0, ng, out.1);
    repaint();
    Some(out)
}

// ── The fixture ─────────────────────────────────────────────────────────────────────────────────

/// `tests quarrylive` registration (rides `quarry3_tests`, no tests.rs line).
pub fn tests() {
    crate::tests::register("quarrylive", selftest);
}

fn listed(name: &str) -> bool {
    MODEL.lock().as_ref().map(|m| m.list.iter().any(|e| e.name == name)).unwrap_or(false)
}

/// `tests quarrylive` — through the LIVE window and the live pass: open Quarry on a scratch folder under the home,
/// create a file through the VFS, run the pass, see it; delete it, see it leave; expand a sub-folder in place.
/// `:: QUARRYLIVE: changed=<0|1> relisted=<0|1> expand=<ok|no> -> PASS|FAIL ::`.
pub fn selftest() {
    use crate::fs::vfs::KERNEL_PRINCIPAL as P;
    let t = crate::shell::vfs_mount_table();
    let base = ops::home_dir();
    let scratch = join(&base, "QLIVETMP");
    let sub = join(&scratch, "SUB");
    let _ = ops::op_delete(&scratch);
    if ops::op_mkdir(&scratch).and_then(|_| ops::op_mkdir(&sub)).is_err() {
        serial_println!(":: QUARRYLIVE: base={} reason=mkdir -> SKIP ::", base);
        return;
    }
    let _ = t.create(&join(&sub, "IN.TXT"), NodeKind::File, P);
    let was_open = is_open();
    if !open_at(&scratch) {
        let _ = ops::op_delete(&scratch);
        serial_println!(":: QUARRYLIVE: reason=no-window -> SKIP ::");
        return;
    }
    let pass = |want: bool, name: &str| -> bool {
        let r0 = RELISTS.load(Ordering::Relaxed);
        let _ = relist_now(volume_gen(), crate::arch::ms());
        RELISTS.load(Ordering::Relaxed) > r0 && listed(name) == want
    };
    // changed: the generation moves on a create (the seam this arc listens to)
    let g0 = volume_gen();
    let f = join(&scratch, "NEW.TXT");
    let made = t.create(&f, NodeKind::File, P).is_ok();
    let changed = made && volume_gen() != g0;
    let seen_new = pass(true, "NEW.TXT");
    let gone = t.unlink(&f, P).is_ok() && pass(false, "NEW.TXT");
    let relisted = seen_new && gone;
    // expand: the triangle's toggle on SUB splices `SUB/IN.TXT` under it, in List view
    let expand = {
        let mut g = MODEL.lock();
        match g.as_mut() {
            Some(m) if toolbar::view() == View::List => {
                toggle(&m.cwd.clone(), "SUB");
                let mut st = COLS.lock();
                resplice(m, &mut st);
                let at = m.list.iter().position(|e| e.name == "SUB");
                let ok = at.map(|i| m.list.get(i + 1).map(|e| e.name == "SUB/IN.TXT").unwrap_or(false)).unwrap_or(false);
                toggle(&m.cwd.clone(), "SUB");
                resplice(m, &mut st);
                ok && !m.list.iter().any(|e| e.name.contains('/'))
            }
            _ => false,
        }
    };
    let _ = ops::op_delete(&scratch);
    if was_open {
        let _ = open_at(&base);
    } else {
        close();
    }
    let ok = changed && relisted && expand;
    serial_println!(
        ":: QUARRYLIVE: changed={} relisted={} expand={} -> {} :: dir={} view={} ::",
        changed as u8, relisted as u8, if expand { "ok" } else { "no" }, if ok { "PASS" } else { "FAIL" },
        scratch, if toolbar::view() == View::List { "list" } else { "other" }
    );
}
