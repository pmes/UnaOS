// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ACTIVITY (R75) — the wire, ON the glass. Peter, flight 15: *"smp is still weird though. seems like
//! it should spread the load better"* — and the serial census disagreed with his eye twice. This window
//! repaints once a second from the SAME census the `:: SMPLOAD:` witness reads: per-CPU load bars
//! (`sched::core_load`, run-queue depth, migrations/s), a process table (the x86 `PROCS` rows: name,
//! asid, state, CPU), the heap (`allocator::heap_census`), the compositor (band workers, windows, the
//! hottest window's presents/s), uptime, the session user and the last boot stage.
//!
//! Built on FILEVIEW's pattern: a cached-RAM surface allocated ONCE at [`open`], painted in place with
//! `font::draw_text` and filled rects, so the once-a-second paint allocates nothing (the census is a set
//! of fixed arrays on the stack; the heap census's own probe allocs are transient and outside the
//! surface). Keys (focus-gated, via `quarry::live::key_route`): `q` closes, Up/Down select a process,
//! `k` kills the selected one through `wc_close_click`'s kill arm (root, or the owner of an
//! operator-launched process — the ACL rule lives in `syscall::act_kill`).
//!
//! aarch64 shows whatever `arch::sched` exposes there: per-CPU bars from its `core_load`, migrations
//! `n/a`, and NO process table (the Pi's `PROCS` is not exported) — the window says so.
//!
//! Witness: `:: ACTIVITY: cpus=<n> procs=<n> heap_used=<KiB> repaints=<n> -> PASS ::` on open and from
//! the `tests activity` fixture ([`selftest`]: open, two repaints, close).

use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::video::{theme, wm};

/// Kernel-furniture owner slot (`+ 7`, after TEXTEDIT's `+ 6`).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 7;
const _: () = assert!(OWNER != wm::KERNEL_OWNER_CONSOLE && OWNER != wm::KERNEL_OWNER_DESKTOP);
const _: () = assert!(OWNER != super::fileview::OWNER && OWNER != super::textedit::OWNER);

const WIN_W: usize = 560;
const WIN_H: usize = 480;
const PAD: usize = 8;
const PERIOD_MS: u64 = 1000;
const MAXC: usize = 8;
const MAXP: usize = 12;
const NPRES: usize = 16;
const BAR_W: usize = 220;

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static STATE: spin::Mutex<Option<State>> = spin::Mutex::new(None);
/// Per-window present counts, fed by `sys_win_present` (one relaxed add). ACTIVITY diffs them.
static PRES: [AtomicU32; NPRES] = [const { AtomicU32::new(0) }; NPRES];

/// Count one present of window `id` (called from the x86 present syscalls).
#[inline]
pub fn note_present(id: usize) {
    PRES[id % NPRES].fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
struct ProcRow { pid: u64, slot: u64, running: bool, cpu: i8, name: [u8; 16], nlen: u8 }

/// One census: fixed arrays only (no allocation).
#[derive(Clone, Copy)]
struct Census {
    n_cpu: usize,
    busy: [i16; MAXC], // -1 = untracked
    runq: [u8; MAXC],
    migr_total: u64,
    n_proc: usize,
    procs: [ProcRow; MAXP],
    procs_known: bool,
    heap_used: usize,
    heap_free: usize,
    wins: usize,
    workers: usize,
    hot_win: usize,
    hot_pps: u32,
    tot_pps: u32,
    up_s: u64,
    user: [u8; 32],
    ulen: usize,
    stage: &'static str,
}

const ZROW: ProcRow = ProcRow { pid: 0, slot: 0, running: false, cpu: -1, name: [0; 16], nlen: 0 };

struct State {
    w: usize,
    h: usize,
    surf: Vec<u32>,
    sel: usize,
    repaints: u32,
    last_ms: u64,
    last_migr: u64,
    last_pres: [u32; NPRES],
    verdict: &'static str,
    cen: Census,
}

/// A stack line buffer implementing `fmt::Write` (truncating).
struct Buf { b: [u8; 120], n: usize }
impl Buf {
    fn new() -> Self { Buf { b: [0; 120], n: 0 } }
    fn bytes(&self) -> &[u8] { &self.b[..self.n] }
}
impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            if self.n < self.b.len() { self.b[self.n] = c; self.n += 1; }
        }
        Ok(())
    }
}

pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

// ── The census ──────────────────────────────────────────────────────────────────────────────────

fn cpu_count() -> usize {
    #[cfg(target_arch = "x86_64")]
    { crate::arch::sched::meter_cpu_count().min(MAXC) }
    #[cfg(target_arch = "aarch64")]
    { crate::arch::percpu::NUM_CPUS.min(MAXC) }
}

fn take_census(prev_pres: &mut [u32; NPRES], dt_ms: u64) -> Census {
    let mut c = Census {
        n_cpu: cpu_count(), busy: [-1; MAXC], runq: [0; MAXC], migr_total: 0, n_proc: 0, procs: [ZROW; MAXP],
        procs_known: false, heap_used: 0, heap_free: 0, wins: 0, workers: 0, hot_win: 0, hot_pps: 0, tot_pps: 0,
        up_s: crate::arch::ms() / 1000, user: [0; 32], ulen: 0, stage: "-",
    };
    for i in 0..c.n_cpu {
        let ld = crate::arch::sched::core_load(i);
        c.busy[i] = if ld.tracked { ld.busy_pct_recent.min(100) as i16 } else { -1 };
        c.runq[i] = crate::arch::sched::run_queue_len(i).min(255) as u8;
    }
    #[cfg(target_arch = "x86_64")]
    {
        c.migr_total = crate::arch::sched::migrations_total();
        let mut rows = [crate::arch::syscall::ActProc { pid: 0, slot: 0, running: false, bg: false }; MAXP];
        let n = crate::arch::syscall::act_proc_rows(&mut rows);
        c.procs_known = true;
        for r in rows.iter().take(n) {
            let p = &mut c.procs[c.n_proc];
            p.pid = r.pid;
            p.slot = r.slot;
            p.running = r.running;
            let mut nm = [0u8; wm::MAX_TITLE];
            let l = wm::app_name_of(r.slot, &mut nm);
            if l > 0 {
                let l = l.min(16);
                p.name[..l].copy_from_slice(&nm[..l]);
                p.nlen = l as u8;
            } else {
                let tag: &[u8] = if r.bg { b"(program)" } else { b"(system)" };
                p.name[..tag.len()].copy_from_slice(tag);
                p.nlen = tag.len() as u8;
            }
            p.cpu = -1;
            for cpu in 0..c.n_cpu {
                if crate::arch::sched::current_task_id(cpu) == Some(r.pid) { p.cpu = cpu as i8; break; }
            }
            c.n_proc += 1;
        }
    }
    let h = crate::allocator::heap_census(4096);
    c.heap_used = h.used;
    c.heap_free = h.free;
    c.wins = wm::live_window_count();
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    { c.workers = super::wcpar::workers(); }
    let mut total = 0u32;
    for i in 0..NPRES {
        let now = PRES[i].load(Ordering::Relaxed);
        let d = now.wrapping_sub(prev_pres[i]);
        prev_pres[i] = now;
        total += d;
        if d > c.hot_pps { c.hot_pps = d; c.hot_win = i; }
    }
    let per = |d: u32| -> u32 { if dt_ms == 0 { 0 } else { (d as u64 * 1000 / dt_ms) as u32 } };
    c.hot_pps = per(c.hot_pps);
    c.tot_pps = per(total);
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        if let Some(n) = crate::fs::users::whoami(&mut nm) {
            let n = n.min(32);
            c.user[..n].copy_from_slice(&nm[..n]);
            c.ulen = n;
        }
    }
    if let Some((_, tag)) = crate::bootpace::last_stamp() { c.stage = tag; }
    c
}

// ── The painter ─────────────────────────────────────────────────────────────────────────────────

fn rect(surf: &mut [u32], stride: usize, h: usize, x: usize, y: usize, w: usize, rh: usize, c: u32) {
    for yy in y..(y + rh).min(h) {
        let row = yy * stride;
        for xx in x..(x + w).min(stride) {
            surf[row + xx] = c;
        }
    }
}

fn bar_color(pct: i16) -> u32 {
    if pct >= 85 { 0x00C8_4B3C } else if pct >= 50 { 0x00D9_A22E } else { 0x0043_A05A }
}

fn paint(st: &mut State) {
    let face = super::text::Face::Body;
    let ch = face.cell_h() + 2;
    let cw = face.cell_w();
    let (w, h) = (st.w, st.h);
    let c = st.cen;
    for p in st.surf.iter_mut() { *p = theme::CONTENT_FILL; }
    let ink = theme::CONTENT_TEXT;
    let mut y = PAD;
    let line = |surf: &mut [u32], y: &mut usize, b: &Buf, color: u32| {
        super::text::draw_text(surf, w, w, h, PAD, *y, b.bytes(), color, false, face);
        *y += ch;
    };
    // Header.
    let mut b = Buf::new();
    let user = core::str::from_utf8(&c.user[..c.ulen]).unwrap_or("?");
    let _ = write!(b, "ACTIVITY  up {}h{:02}m{:02}s  user={}  stage={}", c.up_s / 3600, (c.up_s / 60) % 60, c.up_s % 60, if c.ulen == 0 { "-" } else { user }, c.stage);
    line(&mut st.surf, &mut y, &b, ink);
    // CPU bars.
    let mut b = Buf::new();
    #[cfg(target_arch = "x86_64")]
    { let _ = write!(b, "CPU load   cpus={}  migrations={}/s", c.n_cpu, c.migr_total.saturating_sub(st.last_migr) * 1000 / PERIOD_MS.max(1)); }
    #[cfg(target_arch = "aarch64")]
    { let _ = write!(b, "CPU load   cpus={}  migrations n/a on this scheduler", c.n_cpu); }
    line(&mut st.surf, &mut y, &b, ink);
    let bx = PAD + 6 * cw;
    for i in 0..c.n_cpu {
        let mut b = Buf::new();
        let _ = write!(b, "cpu{}", i);
        super::text::draw_text(&mut st.surf, w, w, h, PAD, y, b.bytes(), ink, false, face);
        rect(&mut st.surf, w, h, bx, y + 1, BAR_W, ch - 4, theme::SCROLL_TRACK);
        let pct = c.busy[i];
        let mut t = Buf::new();
        if pct >= 0 {
            rect(&mut st.surf, w, h, bx, y + 1, BAR_W * pct as usize / 100, ch - 4, bar_color(pct));
            let _ = write!(t, "{:>3}%  q={}", pct, c.runq[i]);
        } else {
            let _ = write!(t, " --   q={}", c.runq[i]);
        }
        super::text::draw_text(&mut st.surf, w, w, h, bx + BAR_W + 8, y, t.bytes(), ink, false, face);
        y += ch;
    }
    y += 4;
    // Heap.
    let total = (c.heap_used + c.heap_free).max(1);
    let mut b = Buf::new();
    let _ = write!(b, "heap  used={} KiB  free={} KiB", c.heap_used / 1024, c.heap_free / 1024);
    line(&mut st.surf, &mut y, &b, ink);
    rect(&mut st.surf, w, h, PAD, y, BAR_W + 6 * cw, ch - 6, theme::SCROLL_TRACK);
    rect(&mut st.surf, w, h, PAD, y, (BAR_W + 6 * cw) * c.heap_used / total, ch - 6, theme::ACCENT);
    y += ch;
    // Compositor.
    let mut b = Buf::new();
    let _ = write!(b, "compositor  band-workers={}  windows={}  presents={}/s  hottest win{}={}/s", c.workers, c.wins, c.tot_pps, c.hot_win, c.hot_pps);
    line(&mut st.surf, &mut y, &b, ink);
    y += 4;
    // Process table.
    let mut b = Buf::new();
    if c.procs_known {
        let _ = write!(b, "{:<4} {:<16} {:>4} {:<7} {}", "pid", "name", "asid", "state", "cpu");
    } else {
        let _ = write!(b, "process table: x86 only (this scheduler exports no PROCS rows)");
    }
    line(&mut st.surf, &mut y, &b, ink);
    for i in 0..c.n_proc {
        let p = c.procs[i];
        let mut b = Buf::new();
        let nm = core::str::from_utf8(&p.name[..p.nlen as usize]).unwrap_or("?");
        let _ = write!(b, "{:<4} {:<16} {:>4} {:<7} ", p.pid, nm, p.slot, if p.running { "run" } else { "exited" });
        if p.cpu >= 0 { let _ = write!(b, "{}", p.cpu); } else { let _ = write!(b, "-"); }
        if i == st.sel {
            rect(&mut st.surf, w, h, 0, y, w, ch, theme::ACCENT);
            line(&mut st.surf, &mut y, &b, theme::CONTENT_FILL);
        } else {
            line(&mut st.surf, &mut y, &b, ink);
        }
    }
    // Footer.
    let fy = h.saturating_sub(ch + 2);
    let mut b = Buf::new();
    let _ = write!(b, "q close   up/down select   k kill{}{}", if st.verdict.is_empty() { "" } else { "   -> " }, st.verdict);
    super::text::draw_text(&mut st.surf, w, w, h, PAD, fy, b.bytes(), ink, false, face);
}

/// Take a census and repaint (in place). Does NOT present.
fn repaint(st: &mut State, now: u64) {
    let dt = now.saturating_sub(st.last_ms).max(1);
    let mut pp = st.last_pres;
    let cen = take_census(&mut pp, dt);
    st.last_pres = pp;
    st.cen = cen;
    if st.sel >= cen.n_proc { st.sel = cen.n_proc.saturating_sub(1); }
    paint(st);
    st.last_migr = cen.migr_total;
    st.last_ms = now;
    st.repaints += 1;
}

fn witness(st: &State, tag_ok: bool) -> bool {
    let c = &st.cen;
    let ok = tag_ok && c.n_cpu > 0 && c.heap_used > 0 && st.repaints >= 1;
    serial_println!(
        ":: ACTIVITY: cpus={} procs={} heap_used={} repaints={} -> {} ::",
        c.n_cpu, c.n_proc, c.heap_used / 1024, st.repaints, if ok { "PASS" } else { "FAIL" }
    );
    ok
}

/// Open the window (replacing a previous one) and paint the first census.
pub fn open() -> Result<(), &'static str> {
    if is_open() {
        close();
    }
    let pi = crate::video::panel_info_nonblocking().ok_or("panel busy")?;
    let (pw, ph) = (pi.width, pi.height);
    let w = WIN_W.min(pw.saturating_sub(2 * wm::BORDER()).max(1));
    let h = WIN_H.min(ph.saturating_sub(wm::TITLE_H() + 2 * wm::BORDER()).max(1));
    if w < 200 || h < 120 {
        return Err("window below floor");
    }
    let len = w * h;
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(len).is_err() {
        return Err("out of memory");
    }
    surf.resize(len, theme::CONTENT_FILL);
    let (_s, ow, oh) = wm::spawn_geometry(w, h).ok_or("geometry unavailable")?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    let mut st = State {
        w, h, surf, sel: 0, repaints: 0, last_ms: crate::arch::ms().saturating_sub(PERIOD_MS), last_migr: 0,
        last_pres: [0; NPRES], verdict: "",
        cen: Census {
            n_cpu: 0, busy: [-1; MAXC], runq: [0; MAXC], migr_total: 0, n_proc: 0, procs: [ZROW; MAXP], procs_known: false,
            heap_used: 0, heap_free: 0, wins: 0, workers: 0, hot_win: 0, hot_pps: 0, tot_pps: 0, up_s: 0, user: [0; 32], ulen: 0, stage: "-",
        },
    };
    // Prime the present counters so the first fps reads a delta, not the boot total.
    for i in 0..NPRES { st.last_pres[i] = PRES[i].load(Ordering::Relaxed); }
    #[cfg(target_arch = "x86_64")]
    { st.last_migr = crate::arch::sched::migrations_total(); }
    repaint(&mut st, crate::arch::ms());
    let base = st.surf.as_ptr() as usize;
    let id = wm::create_at(OWNER, base, len * 4, w as u32, h as u32, (w * 4) as u32, b"Activity", ox + wm::BORDER(), oy + wm::TITLE_H() + wm::BORDER());
    if id == wm::WIN_NONE {
        return Err("window create failed");
    }
    witness(&st, true);
    *STATE.lock() = Some(st);
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "activity");
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    serial_println!("[activity] open win={}", id);
    Ok(())
}

/// Close the window; the surface is freed after the row stops naming it.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    wm::close(id);
    *STATE.lock() = None;
    serial_println!("[activity] closed win={}", id);
}

/// The once-a-second repaint. Chained from `quarry::live::service`; a quiet pass is one atomic load.
pub fn service() {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    let now = crate::arch::ms();
    let Some(mut g) = STATE.try_lock() else { return };
    let Some(st) = g.as_mut() else { return };
    if now.saturating_sub(st.last_ms) < PERIOD_MS {
        return;
    }
    repaint(st, now);
    drop(g);
    let _ = wm::present(id);
}

/// Force one repaint + present now (the fixture's door and the post-kill refresh).
fn repaint_now() -> bool {
    let id = WIN.load(Ordering::Relaxed);
    let mut g = STATE.lock();
    let Some(st) = g.as_mut() else { return false };
    repaint(st, crate::arch::ms());
    drop(g);
    let _ = wm::present(id);
    true
}

/// Keys, only while this window holds focus. `true` when consumed.
pub fn key_route(ev: crate::pal::Event) -> bool {
    if !is_open() || wm::focus_asid() != OWNER {
        return false;
    }
    let crate::pal::Event::Key(c) = ev else { return false };
    match c {
        b'q' | b'Q' => { close(); true }
        0x1F | 0x1E => {
            if let Some(st) = STATE.lock().as_mut() {
                if c == 0x1F { st.sel = st.sel.saturating_sub(1); } else if st.sel + 1 < st.cen.n_proc { st.sel += 1; }
                paint(st);
            }
            let _ = wm::present(WIN.load(Ordering::Relaxed));
            true
        }
        b'k' | b'K' => { kill_selected(); true }
        _ => false,
    }
}

fn kill_selected() {
    #[cfg(target_arch = "x86_64")]
    {
        let pid = {
            let g = STATE.lock();
            g.as_ref().and_then(|s| if s.sel < s.cen.n_proc { Some(s.cen.procs[s.sel].pid) } else { None })
        };
        let Some(pid) = pid else { return };
        // The kill yields while the scheduler confirms the reap: never hold STATE across it.
        let v = crate::arch::syscall::act_kill(pid);
        serial_println!("[activity] kill pid={} -> {}", pid, v);
        if let Some(st) = STATE.lock().as_mut() { st.verdict = v; }
        repaint_now();
    }
    #[cfg(target_arch = "aarch64")]
    {
        if let Some(st) = STATE.lock().as_mut() { st.verdict = "kill: no process table on this scheduler"; }
        repaint_now();
    }
}

/// Pointer: close box, and raise on a press in the content. `true` when consumed.
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
        close();
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    if x < info.x as i32 || y < info.y as i32 {
        return false;
    }
    let sc = info.scale.max(1);
    if (x as usize - info.x) / sc >= info.w || (y as usize - info.y) / sc >= info.h {
        return false;
    }
    wm::focus_changed(OWNER);
    true
}

/// ACTIVITY — open, two forced repaints, close; the witness carries the census the window drew.
#[cfg(feature = "witness")]
pub fn selftest() {
    let opened = open().is_ok();
    let r2 = opened && repaint_now();
    let r3 = r2 && repaint_now();
    let snap = STATE.lock().as_ref().map(|s| (s.cen.n_cpu, s.cen.n_proc, s.cen.heap_used, s.repaints));
    close();
    let closed = !is_open();
    match snap {
        Some((cpus, procs, heap, rep)) => {
            let ok = r3 && closed && cpus > 0 && heap > 0 && rep >= 3;
            serial_println!(":: ACTIVITY: cpus={} procs={} heap_used={} repaints={} -> {} ::", cpus, procs, heap / 1024, rep, if ok { "PASS" } else { "FAIL" });
        }
        None => serial_println!(":: ACTIVITY: cpus=0 procs=0 heap_used=0 repaints=0 reason=open-refused -> FAIL ::"),
    }
}

/// KERNELFONT2 (B363) M4: the faces loaded or were restyled — repaint the open window once.
pub fn font_repaint() {
    let _ = repaint_now();
}
