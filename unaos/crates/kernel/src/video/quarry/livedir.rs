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

use super::columns::{layout_with, pending_row, sort_rows, ColState, RowMeta, COLS};
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

/// The expanded folders of the open window (relative to `cwd`; the set resets when the folder changes).
fn open_set(cwd: &str) -> Vec<String> {
    let mut x = EXP.lock();
    if x.cwd != cwd {
        x.cwd = String::from(cwd);
        x.open.clear();
    }
    x.open.clone()
}

/// QUARRYIO (B660): read the expanded folders' children — under `MODEL` only, BEFORE `COLS` is taken — for
/// [`resplice`]. A folder whose children are already listed inline is not re-read (the splice re-uses them).
/// `(folder relative to cwd, its rows)`.
pub(super) fn read_open(m: &mut Model) -> Vec<(String, Vec<DirEnt>)> {
    let open = open_set(&m.cwd);
    if open.is_empty() || toolbar::view() != View::List {
        return Vec::new();
    }
    let mut out = Vec::new();
    for rel in open {
        let pre = alloc::format!("{}/", rel);
        if m.list.iter().any(|e| e.name.starts_with(&pre)) {
            continue;
        }
        let p = join(&m.cwd, &rel);
        if let Ok((true, rows)) = m.collect_cached(&p) {
            out.push((rel, rows));
        }
    }
    out
}

/// Strip every inline row (with its facts), then splice the expanded folders' children back under their parents.
/// Called at the tail of `columns::resort` (MODEL and COLS held) — after the fresh listing's sort and after every
/// header re-sort. QUARRYIO (B660): NO READ — a folder's children are the inline rows just stripped, else the rows
/// `kids` carries ([`read_open`], read before `COLS`), else nothing; new rows get pending facts (the facts pass reads
/// them).
pub(super) fn resplice(m: &mut Model, st: &mut ColState, kids: &[(String, Vec<DirEnt>)]) {
    if st.meta.len() != m.list.len() || st.meta.iter().zip(m.list.iter()).any(|(mm, e)| mm.name != e.name) {
        let old = core::mem::take(&mut st.meta);
        st.meta = m.list.iter().map(|e| old.iter().find(|o| o.name == e.name).cloned().unwrap_or_else(|| pending_row(e))).collect();
    }
    let mut inline: Vec<(DirEnt, RowMeta)> = Vec::new();
    if m.list.iter().any(|e| e.name.contains('/')) {
        let list = core::mem::take(&mut m.list);
        let meta = core::mem::take(&mut st.meta);
        for (e, mm) in list.into_iter().zip(meta.into_iter()) {
            if e.name.contains('/') {
                inline.push((e, mm));
            } else {
                m.list.push(e);
                st.meta.push(mm);
            }
        }
    }
    let open = open_set(&m.cwd);
    if open.is_empty() || toolbar::view() != View::List {
        return;
    }
    let mut added = false;
    let mut i = 0usize;
    while i < m.list.len() {
        let e = &m.list[i];
        if is_dir(e) && open.iter().any(|o| *o == e.name) && m.list.len() < MAX_LIST {
            let rel = e.name.clone();
            let pre = alloc::format!("{}/", rel);
            let mut rows: Vec<(DirEnt, RowMeta)> =
                inline.iter().filter(|(k, _)| k.name.strip_prefix(&pre).map_or(false, |r| !r.contains('/'))).cloned().collect();
            if rows.is_empty() {
                if let Some((_, ks)) = kids.iter().find(|(p, _)| *p == rel) {
                    let mut v: Vec<DirEnt> = ks
                        .iter()
                        .cloned()
                        .map(|mut k| {
                            k.name = join(&rel, &k.name);
                            k
                        })
                        .collect();
                    dedupe_by_name(&mut v);
                    added |= !v.is_empty();
                    rows = v.into_iter().map(|k| { let mm = pending_row(&k); (k, mm) }).collect();
                }
            }
            rows.truncate(MAX_LIST - m.list.len());
            let (mut kl, mut km): (Vec<DirEnt>, Vec<RowMeta>) = rows.into_iter().unzip();
            sort_rows(&mut kl, &mut km, st.key, st.desc);
            let tail = m.list.split_off(i + 1);
            m.list.extend(kl);
            m.list.extend(tail);
            let tail = st.meta.split_off(i + 1);
            st.meta.extend(km);
            st.meta.extend(tail);
        }
        i += 1; // the spliced children are walked next, so a nested expanded folder splices too
    }
    if added {
        super::columns::FACTS_POSTED.store(true, Ordering::Release); // QUARRYIO (B660): the new rows' facts, on the service pass
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
    let x0 = {
        let st = COLS.lock();
        let c = layout_with(&g, li, lsb, &st.widths, st.trash, &attrcols::widths());
        c.name_x + depth_of(&e.name) * g.mark_w()
    };
    if sx < x0 || sx >= x0 + g.mark_w() {
        return false;
    }
    let name = e.name.clone();
    let open = toggle(&m.cwd, &name);
    let kids = read_open(m); // QUARRYIO (B660): the read, before COLS
    {
        let mut st = COLS.lock();
        resplice(m, &mut st, &kids);
    }
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

/// Re-list the open window's folder after a namespace change, reading afresh under the caller's MODEL hold (the
/// `tests unplug` door). The live pass reads off the lock instead ([`relist_now`]). Returns the rows now listed.
pub(super) fn relist(m: &mut Model) -> usize {
    m.invalidate();
    relist_cached(m)
}

/// [`relist`]'s body over the model's cache as it stands: QUARRYRELIST (B824) pre-fills it off the lock, so every
/// `collect_cached` below is a hit.
fn relist_cached(m: &mut Model) -> usize {
    let sel = m.list.get(m.list_sel).map(|e| e.name.clone());
    let (scroll, focus) = (m.list_scroll, m.focus);
    retree(m);
    let cwd = m.cwd.clone();
    m.show(&cwd);
    if m.err.is_some() && cwd != "/" && crate::fs::removable::departed_name(&cwd).is_none() { // UNPLUG (B703): a folder whose VOLUME left stays, marked `Removed: <name>` (show), until a click closes it or the stick returns
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
    let _ = nav_now(); // NAVREAD (B834): a navigation the router deferred reads off MODEL here, before the live compare
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

/// QUARRYRELIST (B824): plan under one short MODEL take, read with no Quarry lock, install under one short take —
/// never inside a paint (the change stays pending), and a read the generation outlived is dropped (retried next pass).
fn relist_now(ng: u64, now: u64) -> Option<(String, usize)> {
    if super::columns::painting() {
        return None;
    }
    LAST_MS.store(now, Ordering::Relaxed);
    let plan = relist_plan(ng)?;
    let r = relist_install(relist_read(plan))?;
    relist_done(ng, r)
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
                let kids = read_open(m); // QUARRYIO (B660): read first, COLS second
                let mut st = COLS.lock();
                resplice(m, &mut st, &kids);
                let at = m.list.iter().position(|e| e.name == "SUB");
                let ok = at.map(|i| m.list.get(i + 1).map(|e| e.name == "SUB/IN.TXT").unwrap_or(false)).unwrap_or(false);
                toggle(&m.cwd.clone(), "SUB");
                resplice(m, &mut st, &[]);
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

// ── QUARRYRELIST (rmbp-ledger B824): the relist's reads off MODEL ───────────────────────────────────────────────
//
// CHARTER: the live relist reads its folders with Quarry's MODEL released and installs them into the model's own
// cache (`Model::collect_cached`) in one short take; the existing relist body then lists from that cache. Flight 37:
// `relist_now` held MODEL 2175 ms on the render task around a directory read (LOCKREG `held_max_at=live.rs:521`).

/// What one relist reads, snapshotted under MODEL (no read): the generation the pass saw, the folder, the paths —
/// the cwd first, then the expanded inline folders (List view), then every expanded tree row; at most `MAX_CACHE`.
struct RelistPlan {
    at_gen: u64,
    cwd: String,
    paths: Vec<String>,
}

/// What the plan read, with no Quarry lock held.
struct RelistRead {
    plan: RelistPlan,
    ents: Vec<CacheEnt>,
    read_ms: u64,
    read_cycles: u64,
}

/// One relist's account (the wire's and the fixture's). `locked_reads` = directory reads the install still made under
/// MODEL (a failed read is not cacheable: a vanished cwd or tree row is re-read there once).
pub(super) struct Relisted {
    pub dir: String,
    pub rows: usize,
    pub read_ms: u64,
    pub held_us: u64,
    pub locked_reads: usize,
}

/// The install's outcome: done, or dropped because the generation or the folder moved under the read (folder, planned
/// generation, generation now).
enum Install {
    Done(Relisted),
    Dropped(String, u64, u64),
}

fn relist_plan(ng: u64) -> Option<RelistPlan> {
    let g = MODEL.lock();
    let m = g.as_ref()?;
    let cwd = m.cwd.clone();
    let mut paths: Vec<String> = alloc::vec![cwd.clone()];
    if toolbar::view() == View::List {
        for rel in open_set(&cwd) {
            paths.push(join(&cwd, &rel));
        }
    }
    for r in m.tree.iter().filter(|r| r.expanded) {
        paths.push(r.path.clone());
    }
    let mut seen: Vec<String> = Vec::new();
    paths.retain(|p| if seen.iter().any(|s| s == p) { false } else { seen.push(p.clone()); true });
    paths.truncate(MAX_CACHE);
    Some(RelistPlan { at_gen: ng, cwd, paths })
}

/// The reads — no Quarry lock is taken here. The sidebar re-derives its rows now (its own lock), so the install's
/// `toolbar::after_show` finds the generation already seen.
fn relist_read(plan: RelistPlan) -> RelistRead {
    super::sidebar::refresh(false);
    let up = if plan.cwd != "/" { Some(parent(&plan.cwd)) } else { None }; // the fallback `navigate` target
    let (ents, read_ms, read_cycles) = read_paths(&plan.paths, up);
    RelistRead { plan, ents, read_ms, read_cycles }
}

/// The ONE off-lock directory read (QUARRYRELIST's, shared by NAVREAD): every path through `collect`, no Quarry lock
/// taken; `up` is read too when the first path fails and it is not already planned. `(entries, read_ms, read_cycles)`.
fn read_paths(paths: &[String], up: Option<String>) -> (Vec<CacheEnt>, u64, u64) {
    let (t0, c0) = (crate::arch::ms(), crate::arch::now_cycles());
    let mut ents: Vec<CacheEnt> = Vec::new();
    let mut extra: Option<String> = None;
    for (k, p) in paths.iter().enumerate() {
        match collect(p) {
            Ok((is_dir, rows)) => ents.push(CacheEnt { path: String::from(p.as_str()), is_dir, rows }),
            Err(_) if k == 0 => extra = up.clone(),
            Err(_) => {}
        }
    }
    if let Some(p) = extra.filter(|p| !paths.iter().any(|q| q == p)) {
        if let Ok((is_dir, rows)) = collect(&p) {
            ents.push(CacheEnt { path: p, is_dir, rows });
        }
    }
    ents.truncate(MAX_CACHE);
    let read_cycles = crate::arch::now_cycles().saturating_sub(c0);
    (ents, crate::arch::ms().saturating_sub(t0), read_cycles)
}

/// ONE MODEL take: the read installs only into the generation and folder it was planned in.
fn relist_install(r: RelistRead) -> Option<Install> {
    let mut g = MODEL.lock();
    let c0 = crate::arch::now_cycles();
    let m = g.as_mut()?;
    let now_gen = volume_gen();
    if now_gen != r.plan.at_gen || m.cwd != r.plan.cwd {
        return Some(Install::Dropped(r.plan.cwd, r.plan.at_gen, now_gen));
    }
    m.reads += r.ents.len();
    m.read_cycles = m.read_cycles.saturating_add(r.read_cycles);
    m.cache = r.ents;
    m.cache_gen = r.plan.at_gen;
    let reads0 = m.reads;
    let rows = relist_cached(m);
    let locked_reads = m.reads.saturating_sub(reads0);
    let dir = m.cwd.clone();
    let held_us = crate::video::strip::cycles_to_us(crate::arch::now_cycles().saturating_sub(c0));
    drop(g);
    Some(Install::Done(Relisted { dir, rows, read_ms: r.read_ms, held_us, locked_reads }))
}

/// The pass's tail: the generation is seen, the wire, the repaint — or the drop line (SEEN unmoved: the next pass after
/// `SETTLE_MS` re-lists at the new generation).
fn relist_done(ng: u64, r: Install) -> Option<(String, usize)> {
    match r {
        Install::Done(d) => {
            SEEN.store(ng, Ordering::Release);
            RELISTS.fetch_add(1, Ordering::Relaxed);
            serial_println!("[quarry] live dir={} gen={} relisted={}", d.dir, ng, d.rows);
            serial_println!(
                "[quarry] relist dir={} rows={} read_ms={} held_us={} on={} locked_reads={}",
                d.dir, d.rows, d.read_ms, d.held_us, crate::arch::sched::current_name().unwrap_or("?"), d.locked_reads
            );
            let out = (d.dir.clone(), d.rows);
            *LAST.lock() = Some(d);
            repaint();
            Some(out)
        }
        Install::Dropped(dir, a, b) => {
            serial_println!("[quarry] relist dropped dir={} gen={}->{}", dir, a, b);
            None
        }
    }
}

/// The last installed relist's account (the fixture reads it; a leaf lock).
static LAST: crate::sync::Mutex<Option<Relisted>> = crate::sync::Mutex::new(None);

/// `tests quarryio`'s relist leg (columns.rs `relist_leg`).
pub(super) struct RelistProbe {
    pub paint_defer: bool,
    pub stale_dropped: bool,
    pub done: Option<Relisted>,
}

/// (a) under a PaintMark `relist_now` declines with no relist; (b) a read whose generation `bump` moves before the
/// install is dropped; (c) a relist of the open folder, read off the lock.
pub(super) fn relist_probe(bump: &dyn Fn()) -> RelistProbe {
    let r0 = RELISTS.load(Ordering::Relaxed);
    let paint_defer = {
        let _paint = super::columns::PaintMark::new();
        relist_now(volume_gen(), crate::arch::ms()).is_none() && RELISTS.load(Ordering::Relaxed) == r0
    };
    let stale_dropped = match relist_plan(volume_gen()) {
        Some(plan) => {
            let read = relist_read(plan);
            bump();
            matches!(relist_install(read), Some(Install::Dropped(..)))
        }
        None => false,
    };
    *LAST.lock() = None;
    let ok = relist_now(volume_gen(), crate::arch::ms()).is_some();
    let done = if ok { LAST.lock().take() } else { None };
    RelistProbe { paint_defer, stale_dropped, done }
}

// ── NAVREAD (rmbp-ledger B834): a navigation's read off MODEL ───────────────────────────────────────────────────
//
// CHARTER: a navigation the router makes (content_press's tree press and list double-click, key_route's Enter and
// Backspace) to a folder the model's cache does not hold is DEFERRED: recorded under the router's MODEL hold with the
// path bar saying `opening <leaf>`, read on the service pass with no Quarry lock (QUARRYRELIST's `read_paths`), and
// installed in one short take through the existing `Model::navigate` over the warm cache. The glass shows the press at
// once instead of freezing on the old folder for the length of the read. Toolbar travel (q3_press) is owed NAVTRAVEL.

impl Model {
    /// The ONE cache insert (`collect_cached`'s miss and the NAVREAD install): an entry read at volume generation `gen`.
    /// A generation the cache was not filled against starts it over; the same path is replaced; the oldest goes at
    /// `MAX_CACHE`.
    pub(super) fn cache_put(&mut self, gen: u64, ent: CacheEnt) {
        if gen != self.cache_gen {
            self.cache.clear();
            self.cache_gen = gen;
        }
        self.cache.retain(|e| e.path != ent.path);
        if self.cache.len() >= MAX_CACHE {
            self.cache.remove(0);
        }
        self.cache.push(ent);
    }
}

/// One deferred navigation: its sequence number, the folder it goes to and the folder it left.
#[derive(Clone)]
struct NavIntent {
    seq: u64,
    path: String,
    from: String,
}

/// The pending navigation (a newer one replaces it). A LEAF: taken after MODEL, never holding another lock.
static NAV: crate::sync::Mutex<Option<NavIntent>> = crate::sync::Mutex::new(None);
static NAV_SEQ: AtomicU64 = AtomicU64::new(0);
/// Up only while a router gesture runs under MODEL ([`Door`]); read only inside `Model::navigate` (MODEL held).
static DOOR: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The router's door: while it lives (inside the router's MODEL hold) a cold navigation is deferred.
pub(super) struct Door;
impl Door {
    pub(super) fn raise() -> Door {
        DOOR.store(true, Ordering::Release);
        Door
    }
}
impl Drop for Door {
    fn drop(&mut self) {
        DOOR.store(false, Ordering::Release);
    }
}

/// The path-bar status a deferred navigation shows (worded as `openers`' activation status).
fn opening(path: &str) -> String {
    alloc::format!("opening {}", leaf(path))
}

/// `Model::navigate`'s first statement. `false` (navigate synchronously) unless the Door is up AND `path` is cold;
/// a warm path also cancels any pending intent. A cold path is recorded, the status set, and `true` returned.
pub(super) fn nav_defer(m: &mut Model, path: &str) -> bool {
    if !DOOR.load(Ordering::Acquire) {
        return false;
    }
    let warm = m.cache_gen == volume_gen() && m.cache.iter().any(|e| e.path == path);
    if warm {
        if let Some(old) = NAV.lock().take() {
            if m.status.as_deref() == Some(opening(&old.path).as_str()) {
                m.status = None;
            }
        }
        return false;
    }
    let seq = NAV_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let from = m.cwd.clone();
    serial_println!("[quarry] navigate pending dir={} seq={} from={}", path, seq, from);
    m.status = Some(opening(path));
    *NAV.lock() = Some(NavIntent { seq, path: String::from(path), from });
    true
}

/// What one deferred navigation reads: the intent, and the generation it is read at.
struct NavPlan {
    intent: NavIntent,
    at_gen: u64,
}

/// What the plan read, with no Quarry lock held.
struct NavRead {
    plan: NavPlan,
    ents: Vec<CacheEnt>,
    read_ms: u64,
    read_cycles: u64,
}

/// One installed navigation's account (the wire's and the fixture's).
pub(super) struct Navigated {
    pub dir: String,
    pub rows: usize,
    pub read_ms: u64,
    pub held_us: u64,
    pub locked_reads: usize,
}

/// The install's outcome.
enum NavInstall {
    Done(Navigated),
    /// (folder, why: superseded | moved | closed)
    Dropped(String, &'static str),
    /// (folder, planned generation, generation now): the intent stays, the next pass re-reads.
    Retry(String, u64, u64),
}

/// One service pass's NAVREAD step.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NavStep {
    Idle,
    Declined,
    Done,
    Dropped,
    Retry,
}

/// The pending intent, cloned under NAV only.
fn nav_plan() -> Option<NavPlan> {
    let intent = NAV.lock().as_ref().cloned()?;
    Some(NavPlan { intent, at_gen: volume_gen() })
}

/// The read — no Quarry lock is taken here.
fn nav_read(plan: NavPlan) -> NavRead {
    let (ents, read_ms, read_cycles) = read_paths(core::slice::from_ref(&plan.intent.path), None);
    NavRead { plan, ents, read_ms, read_cycles }
}

/// ONE MODEL take, timed: the read installs only for the intent, generation and folder it was planned in.
fn nav_install(r: NavRead) -> NavInstall {
    let mut g = MODEL.lock();
    let c0 = crate::arch::now_cycles();
    let path = r.plan.intent.path.clone();
    let Some(m) = g.as_mut() else {
        *NAV.lock() = None;
        return NavInstall::Dropped(path, "closed");
    };
    {
        let mut nav = NAV.lock();
        if nav.as_ref().map(|n| n.seq) != Some(r.plan.intent.seq) {
            return NavInstall::Dropped(path, "superseded");
        }
        let now_gen = volume_gen();
        if now_gen != r.plan.at_gen {
            return NavInstall::Retry(path, r.plan.at_gen, now_gen);
        }
        *nav = None;
    }
    let status = opening(&path);
    if m.cwd != r.plan.intent.from {
        if m.status.as_deref() == Some(status.as_str()) {
            m.status = None;
        }
        return NavInstall::Dropped(path, "moved");
    }
    m.reads += r.ents.len();
    m.read_cycles = m.read_cycles.saturating_add(r.read_cycles);
    for e in r.ents {
        m.cache_put(r.plan.at_gen, e);
    }
    let reads0 = m.reads;
    m.navigate(&path); // the Door is down here: a warm, synchronous show
    if m.status.as_deref() == Some(status.as_str()) {
        m.status = None;
    }
    m.settle();
    let locked_reads = m.reads.saturating_sub(reads0);
    let (dir, rows) = (m.cwd.clone(), m.list.len());
    let held_us = crate::video::strip::cycles_to_us(crate::arch::now_cycles().saturating_sub(c0));
    drop(g);
    NavInstall::Done(Navigated { dir, rows, read_ms: r.read_ms, held_us, locked_reads })
}

/// The pass's tail, after the MODEL drop: the wire, the account, the repaint.
fn nav_done(r: NavInstall) -> NavStep {
    match r {
        NavInstall::Done(d) => {
            serial_println!(
                "[quarry] navigate dir={} rows={} read_ms={} held_us={} on={} locked_reads={}",
                d.dir, d.rows, d.read_ms, d.held_us, crate::arch::sched::current_name().unwrap_or("?"), d.locked_reads
            );
            *NAV_LAST.lock() = Some(d);
            repaint();
            NavStep::Done
        }
        NavInstall::Dropped(dir, why) => {
            serial_println!("[quarry] navigate dropped dir={} why={}", dir, why);
            if why == "moved" {
                repaint();
            }
            NavStep::Dropped
        }
        NavInstall::Retry(dir, a, b) => {
            serial_println!("[quarry] navigate retry dir={} gen={}->{}", dir, a, b);
            NavStep::Retry
        }
    }
}

/// The service pass's NAVREAD step: never inside a paint (the intent stays pending); nothing pending is one leaf lock.
pub(super) fn nav_now() -> NavStep {
    if super::columns::painting() {
        return NavStep::Declined;
    }
    let Some(plan) = nav_plan() else { return NavStep::Idle };
    nav_done(nav_install(nav_read(plan)))
}

/// The last installed navigation's account (the fixture reads it; a leaf lock).
static NAV_LAST: crate::sync::Mutex<Option<Navigated>> = crate::sync::Mutex::new(None);

/// `tests quarryio`'s navigation leg (columns.rs `nav_leg`).
pub(super) struct NavProbe {
    pub deferred: bool,
    pub paint_defer: bool,
    pub stale_dropped: bool,
    pub cwd_ok: bool,
    pub done: Option<Navigated>,
}

/// From the open window: (a) under ONE MODEL take, the cache dropped and the Door up, `navigate(to)` defers (cwd
/// unchanged, status `opening <leaf>`) and the plan is taken in the same hold; (b) in that hold, under a PaintMark,
/// `nav_now` declines with the intent still pending; (c) read that plan, then under one take a second Door navigation
/// (cache dropped again, so it is cold whichever pass ran) supersedes it and its install drops; (d) the pass installs
/// the newer intent (this task's or the render task's own), and the window shows `to`. The legs are ordered so the
/// render task's concurrent pass cannot change an outcome: (b) runs while MODEL is held (nothing can install), and
/// NAV_LAST is cleared in (c)'s hold after B is recorded (an install of A after that is superseded, so only B's sets it).
pub(super) fn nav_probe(to: &str) -> NavProbe {
    let (deferred, paint_defer, plan_a) = {
        let mut g = MODEL.lock();
        match g.as_mut() {
            Some(m) => {
                m.invalidate();
                let was = m.cwd.clone();
                let ok = {
                    let _door = Door::raise();
                    m.navigate(to);
                    m.cwd == was && m.status.as_deref() == Some(opening(to).as_str())
                };
                let plan = nav_plan();
                let paint_defer = {
                    let _paint = super::columns::PaintMark::new();
                    nav_now() == NavStep::Declined
                } && NAV.lock().is_some();
                (ok && plan.is_some(), paint_defer, plan)
            }
            None => (false, false, None),
        }
    };
    let stale_dropped = match plan_a {
        Some(a) => {
            let read = nav_read(a);
            if let Some(m) = MODEL.lock().as_mut() {
                m.invalidate();
                let _door = Door::raise();
                m.navigate(to);
                *NAV_LAST.lock() = None;
            }
            matches!(nav_install(read), NavInstall::Dropped(_, "superseded"))
        }
        None => false,
    };
    let t0 = crate::arch::ms();
    while NAV_LAST.lock().is_none() && crate::arch::ms().saturating_sub(t0) < 2_000 {
        if nav_now() == NavStep::Idle {
            crate::arch::sched::yield_now();
        }
    }
    let done = NAV_LAST.lock().take();
    let cwd_ok = MODEL.lock().as_ref().map_or(false, |m| m.cwd == to);
    NavProbe { deferred, paint_defer, stale_dropped, cwd_ok, done }
}
