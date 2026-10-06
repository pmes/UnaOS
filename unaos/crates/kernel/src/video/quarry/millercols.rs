// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! COLUMNSVIEW (rmbp-ledger B436, MACPARITY row 27) — Quarry's COLUMNS view (Miller columns), the Mac's third
//! view. Quarry is the Finder by ruling (R50). NOT `columns.rs`: that is the LIST view's attribute columns
//! (QUARRY2 B336 / ATTRCOLUMNS B402); this module is the view in which each pane is a directory.
//!
//! ONE MODEL (R79: no second store). The columns are a view of Quarry's model, not a copy of it:
//!
//! * the FOCUS pane is the list — `Model::cwd`, `list`, `list_sel` — so Quick Look (Space), the path bar,
//!   Back / Forward, Enter and the double press (`activate_row`) and FOLDERVIEW's per-folder mode work unchanged;
//! * the panes to its LEFT are its ancestors from `/`, each selecting the child on the path (read through
//!   `collect`, the shell's `ls` seam, behind a small listing cache that the volume generation clears);
//! * the pane to its RIGHT is the selection: a folder's listing, or for a file Quick Look's card
//!   (`quicklook::render_into` — the viewer the type opens with; no second renderer). The decode is I/O, so the
//!   painter only LATCHES the request and draws the icon; the card renders on Quarry's service pass.
//!
//! Panes are narrow lists; the preview takes the rest of the width. When they overflow, the view scrolls by whole
//! panes so the newest stays on the glass, and a strip along the bottom shows the offset. Left / Right move
//! between panes (to the parent selecting the folder left, into the selected folder), Up / Down within; a press
//! on an ancestor's or the next pane's row goes there and selects it; a double press on the focus pane opens.
//!
//! Wire: `[quarry] view=columns panes=<n> path=<p>` whenever the panes change; `tests columnsview`.

use alloc::string::String;
use alloc::vec::Vec;

use super::{fill, text, theme, Act, Geom, Model, NodeKind, Pane, Rect, MODEL, PAD};

/// What a pane shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    /// An ancestor of the focus pane; its selection is the child on the path.
    Up,
    /// The list (`Model::cwd`).
    Focus,
    /// The selected folder's listing.
    Next,
    /// The selected file's Quick Look card.
    Preview,
}

/// One pane: the directory it lists (or the file it previews), its rows `(name, is_dir)` and its selection.
pub(super) struct Col {
    pub kind: Kind,
    pub path: String,
    pub rows: Vec<(String, bool)>,
    pub sel: Option<usize>,
}

/// Listings held for the ancestor and next panes (FIFO), keyed by path, cleared on a new volume generation.
const LCACHE_MAX: usize = 12;
static LCACHE: spin::Mutex<(u64, Vec<(String, Vec<(String, bool)>)>)> = spin::Mutex::new((0, Vec::new()));

/// The rendered preview card, and the request the painter latched for the service pass.
struct Prev {
    path: String,
    w: usize,
    h: usize,
    surf: Vec<u32>,
}
static PREV: spin::Mutex<Option<Prev>> = spin::Mutex::new(None);
static WANT: spin::Mutex<Option<(String, usize, usize)>> = spin::Mutex::new(None);
/// The last `(panes, path)` the glass witness printed.
static SAID: spin::Mutex<(usize, String)> = spin::Mutex::new((0, String::new()));

/// A directory's rows, sorted and de-duplicated as the list pane sorts them by default.
fn listing(path: &str) -> Vec<(String, bool)> {
    let vgen = super::volume_gen();
    {
        let mut c = LCACHE.lock();
        if c.0 != vgen {
            c.0 = vgen;
            c.1.clear();
        }
        if let Some((_, r)) = c.1.iter().find(|(p, _)| p == path) {
            return r.clone();
        }
    }
    let rows: Vec<(String, bool)> = match super::collect(path) {
        Ok((true, mut rows)) => {
            rows.truncate(super::MAX_LIST);
            super::list_sort(&mut rows);
            super::dedupe_by_name(&mut rows);
            rows.into_iter().map(|e| { let d = matches!(e.kind, NodeKind::Dir); (e.name, d) }).collect()
        }
        _ => Vec::new(),
    };
    let mut c = LCACHE.lock();
    if c.1.len() >= LCACHE_MAX {
        c.1.remove(0);
    }
    c.1.push((String::from(path), rows.clone()));
    rows
}

/// The directories from `/` down to `cwd`, inclusive.
pub(super) fn chain(cwd: &str) -> Vec<String> {
    let mut out: Vec<String> = alloc::vec![String::from("/")];
    let mut acc = String::new();
    for part in cwd.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(part);
        out.push(acc.clone());
    }
    out
}

/// The panes for the model, left to right.
pub(super) fn plan(m: &Model) -> Vec<Col> {
    let ch = chain(&m.cwd);
    let mut cols: Vec<Col> = Vec::new();
    for k in 0..ch.len().saturating_sub(1) {
        let rows = listing(&ch[k]);
        let want = super::leaf(&ch[k + 1]);
        let sel = rows.iter().position(|(n, _)| *n == want);
        cols.push(Col { kind: Kind::Up, path: ch[k].clone(), rows, sel });
    }
    let rows: Vec<(String, bool)> = m.list.iter().map(|e| (e.name.clone(), matches!(e.kind, NodeKind::Dir))).collect();
    let sel = (!rows.is_empty()).then(|| m.list_sel.min(rows.len() - 1));
    let next = sel.map(|i| (super::join(&m.cwd, &rows[i].0), rows[i].1));
    cols.push(Col { kind: Kind::Focus, path: m.cwd.clone(), rows, sel });
    match next {
        Some((p, true)) => {
            let rows = listing(&p);
            cols.push(Col { kind: Kind::Next, path: p, rows, sel: None });
        }
        Some((p, false)) => cols.push(Col { kind: Kind::Preview, path: p, rows: Vec::new(), sel: None }),
        None => {}
    }
    cols
}

/// A list pane's width, physical px.
pub(super) fn col_w(g: &Geom) -> usize {
    (20 * g.cell_w()).max(crate::ui::px(160))
}

fn strip_h() -> usize {
    crate::ui::px(6)
}

/// Where the panes sit inside the list pane's interior `li`: a rect per pane (`None` = scrolled off), the first
/// pane shown, and the whole row's width and the offset scrolled past (for the strip).
pub(super) struct Lay {
    pub rects: Vec<Option<Rect>>,
    pub first: usize,
    pub total: usize,
    pub off: usize,
}

pub(super) fn lay(g: &Geom, li: Rect, cols: &[Col]) -> Lay {
    let cw = col_w(g).min(li.w);
    let ws: Vec<usize> = cols.iter().map(|c| if c.kind == Kind::Preview { (li.w / 2).max(cw) } else { cw }).collect();
    let n = ws.len();
    // The newest panes stay on the glass: the first shown is the smallest whose tail fits.
    let (mut first, mut sum) = (n, 0usize);
    while first > 0 && sum + ws[first - 1] <= li.w {
        first -= 1;
        sum += ws[first];
    }
    // The focus pane always shows (a too-wide preview is cut at the right edge instead).
    if let Some(f) = cols.iter().position(|c| c.kind == Kind::Focus) {
        first = first.min(f);
    }
    let total: usize = ws.iter().sum();
    let off: usize = ws[..first].iter().sum();
    let h = li.h.saturating_sub(if first > 0 { strip_h() } else { 0 });
    let mut rects: Vec<Option<Rect>> = alloc::vec![None; n];
    let mut x = li.x;
    for i in first..n {
        let room = (li.x + li.w).saturating_sub(x);
        // The preview, last, takes the rest of the width.
        let w = if cols[i].kind == Kind::Preview { room } else { ws[i].min(room) };
        if w < 2 {
            break;
        }
        rects[i] = Some(Rect { x, y: li.y, w, h });
        x += w;
    }
    Lay { rects, first, total, off }
}

/// The first row a `vis`-row pane shows: the selection kept on the glass.
fn top(sel: Option<usize>, len: usize, vis: usize) -> usize {
    match sel {
        Some(s) if s >= vis => (s + 1 - vis).min(len.saturating_sub(vis)),
        _ => 0,
    }
}

/// Paint the panes over the list pane's interior `li` (after the list painted, when the view is Columns).
/// Returns the panes on the glass.
pub(super) fn paint(m: &Model, px: &mut [u32], li: Rect) -> usize {
    let g = &m.geom;
    fill(px, g, li.x, li.y, li.w, li.h, theme::content_fill());
    let cols = plan(m);
    let l = lay(g, li, &cols);
    let rh = g.row_h();
    let live = m.focus == Pane::List;
    let mut shown = 0usize;
    for (i, c) in cols.iter().enumerate() {
        let Some(r) = l.rects[i] else { continue };
        shown += 1;
        if r.x + r.w < li.x + li.w {
            fill(px, g, r.x + r.w - 1, r.y, 1, r.h, theme::frame_line());
        }
        if c.kind == Kind::Preview {
            paint_preview(g, px, r, &c.path);
            continue;
        }
        if c.kind == Kind::Focus {
            if let Some(e) = &m.err {
                text(px, g, r.x + PAD(), r.y + PAD(), e.as_bytes(), r.x + r.w - 1, theme::content_text());
                continue;
            }
        }
        let vis = (r.h / rh).max(1);
        let t = top(c.sel, c.rows.len(), vis);
        let chev = 2 * g.cell_w();
        for (j, (name, dir)) in c.rows.iter().enumerate().skip(t).take(vis) {
            let y = r.y + (j - t) * rh;
            let sel = c.sel == Some(j);
            let hot = sel && live && c.kind == Kind::Focus;
            if sel {
                fill(px, g, r.x, y, r.w - 1, rh, if hot { theme::accent() } else { theme::scroll_thumb() });
            }
            let ink = if hot { theme::chrome_face() } else { theme::content_text() };
            text(px, g, r.x + PAD(), y + g.ts, name.as_bytes(), (r.x + r.w).saturating_sub(chev), ink);
            if *dir {
                text(px, g, (r.x + r.w).saturating_sub(chev) + g.ts, y + g.ts, b">", r.x + r.w - 1, ink);
            }
        }
    }
    if l.first > 0 && l.total > 0 {
        let y = li.y + li.h.saturating_sub(strip_h());
        fill(px, g, li.x, y, li.w, strip_h(), theme::scroll_track());
        let tw = (li.w * li.w / l.total).max(strip_h());
        fill(px, g, li.x + li.w * l.off / l.total, y, tw.min(li.w), strip_h(), theme::scroll_thumb());
    }
    let deepest = cols.last().map(|c| c.path.clone()).unwrap_or_default();
    let mut s = SAID.lock();
    if s.0 != cols.len() || s.1 != deepest {
        serial_println!("[quarry] view=columns panes={} path={} shown={} first={}", cols.len(), deepest, shown, l.first);
        *s = (cols.len(), deepest);
    }
    shown
}

/// The preview pane: the rendered card when it is in hand, else the icon and the name (and a latched request).
fn paint_preview(g: &Geom, px: &mut [u32], r: Rect, path: &str) {
    let (w, h) = (r.w - 1, r.h);
    {
        let pv = PREV.lock();
        if let Some(p) = pv.as_ref().filter(|p| p.path == path && p.w == w && p.h == h) {
            for row in 0..h.min(g.h.saturating_sub(r.y)) {
                let n = w.min(g.w.saturating_sub(r.x));
                let d = (r.y + row) * g.w + r.x;
                px[d..d + n].copy_from_slice(&p.surf[row * w..row * w + n]);
            }
            return;
        }
    }
    let s = crate::ui::px(64).min(h / 2).min(w / 2);
    super::iconview::draw_icon(px, g.w, g.h, r.x + (w - s) / 2, r.y + 4 * PAD(), s, path, false, g.face);
    let leaf = path.rsplit('/').next().unwrap_or(path).as_bytes();
    let tw = leaf.len() * g.cell_w();
    text(px, g, r.x + w.saturating_sub(tw) / 2, r.y + 6 * PAD() + s, leaf, r.x + w, theme::content_text());
    *WANT.lock() = Some((String::from(path), w, h));
}

/// Quarry's service pass: render the latched preview card (Quick Look's renderer; I/O, never in the router), then
/// repaint so the card shows.
pub(super) fn service() {
    let Some((path, w, h)) = WANT.try_lock().and_then(|mut g| g.take()) else { return };
    if w == 0 || h == 0 {
        return;
    }
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(w * h).is_err() {
        serial_println!("[quarry] columns preview DECLINE reason=oom bytes={}", w * h * 4);
        return;
    }
    surf.resize(w * h, theme::content_fill());
    super::quicklook::render_into(&path, &mut surf, w, h);
    *PREV.lock() = Some(Prev { path, w, h, surf });
    super::repaint();
}

/// Select `name` in the list (the folder a step left came out of, the row a press chose).
fn select_name(m: &mut Model, name: &str) {
    m.list_sel = m.list.iter().position(|e| e.name == name).unwrap_or(0);
}

/// A press in the list pane while the view is Columns. `None` = not ours.
pub(super) fn press(m: &mut Model, sx: usize, sy: usize) -> Option<Act> {
    let li = m.geom.list_pane().inner();
    if !li.contains(sx, sy) {
        return None;
    }
    m.focus = Pane::List;
    let cols = plan(m);
    let l = lay(&m.geom, li, &cols);
    let Some(i) = (0..cols.len()).find(|&i| l.rects[i].map(|r| r.contains(sx, sy)).unwrap_or(false)) else {
        m.click_ms = 0;
        return Some(Act::None);
    };
    let (r, c) = (l.rects[i].unwrap_or(li), &cols[i]);
    if c.kind == Kind::Preview {
        return Some(Act::None);
    }
    let rh = m.geom.row_h();
    let j = top(c.sel, c.rows.len(), (r.h / rh).max(1)) + (sy - r.y) / rh;
    let Some((name, _)) = c.rows.get(j) else {
        m.click_ms = 0;
        return Some(Act::None);
    };
    let now = crate::arch::ms();
    if c.kind == Kind::Focus {
        let dbl = super::is_double(m.click_ms, now, m.click_row, j, m.click_pane == Pane::List);
        m.list_sel = j;
        m.click_row = j;
        m.click_pane = Pane::List;
        m.click_ms = if dbl { 0 } else { now };
        if dbl {
            return Some(m.activate_row(j));
        }
    } else {
        // An ancestor's row, or the next pane's: go to that pane's folder and select the row.
        let (p, name) = (c.path.clone(), name.clone());
        m.navigate(&p);
        select_name(m, &name);
        m.click_row = m.list_sel;
        m.click_pane = Pane::List;
        m.click_ms = now;
    }
    m.settle();
    Some(Act::None)
}

/// One arrow on the model: Up / Down within the focus pane, Left to the parent (the folder left selected), Right
/// into the selected folder. `true` = consumed.
pub(super) fn step(m: &mut Model, c: u8) -> bool {
    if !matches!(c, 0x1C..=0x1F) || m.focus != Pane::List {
        return false;
    }
    match c {
        0x1F => m.list_sel = m.list_sel.saturating_sub(1),
        0x1E => {
            if m.list_sel + 1 < m.list.len() {
                m.list_sel += 1;
            }
        }
        0x1D => {
            if m.cwd != "/" {
                let (up, child) = (super::parent(&m.cwd), super::leaf(&m.cwd));
                m.navigate(&up);
                select_name(m, &child);
            }
        }
        _ => {
            let into = m.list.get(m.list_sel).filter(|e| matches!(e.kind, NodeKind::Dir)).map(|e| super::join(&m.cwd, &e.name));
            if let Some(p) = into.filter(|p| p.len() <= super::PATH_MAX) {
                m.navigate(&p);
            }
        }
    }
    m.settle();
    true
}

/// The arrows while the view is Columns and the list has the keyboard. `true` = consumed.
pub(super) fn key(c: u8) -> bool {
    let mut g = MODEL.lock();
    let Some(m) = g.as_mut() else { return false };
    step(m, c)
}

/// `tests columnsview` registration (from `quarry3_tests`, no tests.rs line).
pub(super) fn register() {
    crate::tests::register("columnsview", selftest);
}

/// M3 — `tests columnsview`, an off-glass model on the test-f directory with its first file selected:
///
/// `:: COLUMNSVIEW: panes=<n> depth=<n> preview=<ok> keys=<ok> -> PASS :: …`
///
/// * panes — one per ancestor (each selecting the child on the path), the focus pane, and the preview: depth + 2;
/// * preview — the file's card renders through Quick Look's renderer into the preview pane's size, with ink;
/// * keys — Down, Up, Left (the parent, the folder selected), Right (back in);
/// * hit — a press on the root pane's selected row goes to `/` with that folder selected.
pub fn selftest() {
    let ok = |b: bool| if b { "ok" } else { "FAIL" };
    let mt = crate::shell::vfs_mount_table();
    let Some(dir) = crate::fs::volumes::TESTF_DIRS.iter().copied().find(|d| mt.stat(d).is_ok()) else {
        serial_println!(":: COLUMNSVIEW: panes=0 depth=0 preview=skip keys=skip -> SKIP :: reason=no-test-f ::");
        return;
    };
    let Some(g) = super::geometry(crate::ui::px(1920), crate::ui::px(1200)) else {
        serial_println!(":: COLUMNSVIEW: panes=0 depth=0 preview=FAIL keys=FAIL -> FAIL :: reason=no-geometry ::");
        return;
    };
    let mut m = Model::new(g);
    m.navigate(dir);
    m.focus = Pane::List;
    if let Some(i) = m.list.iter().position(|e| !matches!(e.kind, NodeKind::Dir)) {
        m.list_sel = i;
    }
    let depth = chain(&m.cwd).len() - 1;
    let cols = plan(&m);
    let panes = cols.len();
    let shape_ok = panes == depth + 2 && cols.last().map(|c| c.kind == Kind::Preview).unwrap_or(false) && cols[..depth].iter().all(|c| c.sel.is_some());
    let li = g.list_pane().inner();
    let mut px: Vec<u32> = alloc::vec![0; g.w * g.h];
    let shown = paint(&m, &mut px, li);
    // The preview card, as the service pass renders it.
    let l = lay(&g, li, &cols);
    let (via, ink) = match l.rects[panes - 1] {
        Some(r) if cols[panes - 1].kind == Kind::Preview => {
            let (w, h) = (r.w - 1, r.h);
            let mut s: Vec<u32> = alloc::vec![0; w * h];
            let (via, _) = super::quicklook::render_into(&cols[panes - 1].path, &mut s, w, h);
            (via, s.iter().filter(|&&p| p != theme::content_fill()).count())
        }
        _ => ("none", 0),
    };
    let preview_ok = ink > 0;
    // The keys.
    let (sel0, len) = (m.list_sel, m.list.len());
    step(&mut m, 0x1E);
    let down_ok = m.list_sel == if sel0 + 1 < len { sel0 + 1 } else { sel0 };
    step(&mut m, 0x1F);
    let up_ok = m.list_sel == sel0;
    step(&mut m, 0x1D);
    let left_ok = m.cwd == super::parent(dir) && m.list.get(m.list_sel).map(|e| e.name == super::leaf(dir)).unwrap_or(false);
    step(&mut m, 0x1C);
    let right_ok = m.cwd == dir;
    let keys_ok = down_ok && up_ok && left_ok && right_ok;
    // A press on the root pane's selected row.
    let cols = plan(&m);
    let l = lay(&g, li, &cols);
    let hit = match (l.rects[0], cols[0].sel, cols.len() > 1) {
        (Some(r), Some(s), true) if s < (r.h / g.row_h()).max(1) => {
            let want = cols[1].path.clone();
            let _ = press(&mut m, r.x + r.w / 2, r.y + s * g.row_h() + g.row_h() / 2);
            ok(m.cwd == "/" && m.list.get(m.list_sel).map(|e| e.name == super::leaf(&want)).unwrap_or(false))
        }
        _ => "skip",
    };
    let pass = shape_ok && preview_ok && keys_ok && hit != "FAIL";
    serial_println!(
        ":: COLUMNSVIEW: panes={} depth={} preview={} keys={} -> {} :: path={} shape={} shown={} via={} ink={} down={} up={} left={} right={} hit={} ::",
        panes,
        depth,
        ok(preview_ok),
        ok(keys_ok),
        if pass { "PASS" } else { "FAIL" },
        dir,
        ok(shape_ok),
        shown,
        via,
        ink,
        ok(down_ok),
        ok(up_ok),
        ok(left_ok),
        ok(right_ok),
        hit
    );
}

/// SMALLFIX4 (COLUMNSVIEW owed): which pane and row of the Columns view is under `(sx, sy)` —
/// `Some((pane index, row))`, row `None` over a pane's empty tail or the preview. `None` = outside the panes.
fn pane_row_at(m: &Model, cols: &[Col], sx: usize, sy: usize) -> Option<(usize, Option<usize>)> {
    let li = m.geom.list_pane().inner();
    if !li.contains(sx, sy) {
        return None;
    }
    let l = lay(&m.geom, li, cols);
    let i = (0..cols.len()).find(|&i| l.rects[i].map(|r| r.contains(sx, sy)).unwrap_or(false))?;
    let (r, c) = (l.rects[i].unwrap_or(li), &cols[i]);
    if c.kind == Kind::Preview {
        return Some((i, None));
    }
    let rh = m.geom.row_h();
    let j = top(c.sel, c.rows.len(), (r.h / rh).max(1)) + (sy - r.y) / rh;
    Some((i, (j < c.rows.len()).then_some(j)))
}

/// SMALLFIX4 (COLUMNSVIEW owed): a RIGHT press in the Columns view targets the PANE under the pointer, not
/// the list geometry: a focus-pane row is selected, an ancestor's or the next pane's row is navigated to and
/// selected (as a primary press does, without the double-click grammar), and the file menu then acts on it.
/// `true` = the press was over the panes. Witness: `[quarry] columns right-press pane=<i> kind=<k> row=<r|->`.
pub(super) fn right_select(m: &mut Model, sx: usize, sy: usize) -> bool {
    let cols = plan(m);
    let Some((i, row)) = pane_row_at(m, &cols, sx, sy) else { return false };
    let c = &cols[i];
    let kind = match c.kind { Kind::Up => "up", Kind::Focus => "focus", Kind::Next => "next", Kind::Preview => "preview" };
    if let Some(j) = row {
        if c.kind == Kind::Focus {
            m.list_sel = j;
        } else {
            let (p, name) = (c.path.clone(), c.rows[j].0.clone());
            m.navigate(&p);
            select_name(m, &name);
        }
        m.focus = Pane::List;
        m.settle();
    }
    serial_println!("[quarry] columns right-press pane={} kind={} row={}", i, kind, row.map(|j| alloc::format!("{}", j)).unwrap_or_else(|| String::from("-")));
    true
}

/// SMALLFIX4 (COLUMNSVIEW owed): the wheel over the Columns view moves the PANE under the pointer. A pane
/// shows the rows around its selection (`top`), so the focus pane's wheel steps the selection by QSCROLL's
/// `wheel_next` (three rows a detent, positive = away = toward row 0) and the pane follows; an ancestor's selection IS the
/// path and the next pane has none, so their wheel scrolls nothing (consumed, `moved=false`).
pub(super) fn wheel(m: &mut Model, sx: usize, sy: usize, detents: i32) -> Option<super::WheelHit> {
    let cols = plan(m);
    let (i, _) = pane_row_at(m, &cols, sx, sy)?;
    let len = m.list.len();
    if cols[i].kind != Kind::Focus || len == 0 {
        return Some(super::WheelHit { pane: "columns", scroll: 0, max: 0, moved: false });
    }
    let was = m.list_sel.min(len - 1);
    let next = super::wheel_next(was, len - 1, detents.signum());
    m.list_sel = next;
    m.settle();
    Some(super::WheelHit { pane: "columns", scroll: next, max: len - 1, moved: next != was })
}
