// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! LAUNCHER (rmbp-ledger B417, MACPARITY row 36) — **Cmd-Space: one field over programs, files, settings
//! and math.** Our name (the Launcher), our glyph; only the chord and the gesture are the Mac's.
//!
//! * THE ROW — a chromeless overlay (`wm::overlay_open`, the SHORTCUTS/DIALOG pattern), pinned topmost,
//!   centred in the upper third: the field, then up to four groups — Programs, Files, Settings, Math.
//! * THE DOORS — [`key_door`] is the FIRST door of x86's `wc_route_event` (Cmd-Space toggles; while up every
//!   key is the launcher's: type, Backspace, Up/Down, Return, Esc); [`press_at`] is the click router's first
//!   arm (a row picks it, anywhere else closes). The doors only edit the query and latch: the ranking, the
//!   file walk, the picks and the recency file run in [`service`] (the desktop pass, chained from
//!   `settings::service`), never in the input router (R88).
//! * THE SOURCES — Programs: the dock's table apps (`dock::installed_names`, launched by the pin's own post)
//!   and every `/apps/*.ELF` (the shell line EXECNAME resolves, `dock::post_line_launch`, a glass launch);
//!   names and icons are APPRES's. Files: `fs::search` (the name trees, one bounded walk per open).
//!   Settings: `prefs_core::schema::SCHEMA`'s `system.*` keys and docs; a pick opens Settings on that row
//!   (`settings::request_open_at`). Math: `+ - * / ( )` evaluated inline; Return copies the value.
//! * RECENCY — a pick moves its token to the front of a 16-entry LRU kept in ONE file,
//!   `<home>/settings/launcher` (SETTINGSFILES' shape: Principia's TOML codec, namespace `launcher`).
//!   Within a group, recent picks rank first, then match quality.
//!
//! Wire: `[launcher] open win=<id>`, `[launcher] snapshot programs=<n> files=<n> truncated=<0|1> ms=<n>`,
//! `[launcher] query=<q> hits=p<n>/f<n>/s<n>[/m1] ms=<n>`, `[launcher] pick kind=<k> name=<n> -> <how>`,
//! `[launcher] close`, `[launcher] saved <path> recent=<n>`; `tests launcher` →
//! `:: LAUNCHER: programs=<n> files=<n> settings=<n> math=ok open=ok ms=<n> -> PASS ::`.
//! Design: `docs/dev/evidence/rmbp-1005/launcher.md`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::{menubar, theme, wm};
use crate::fs::search;

// ── The pure core: match quality, settings labels, math ─────────────────────────────────────────

fn contains_ci(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    !n.is_empty() && h.len() >= n.len() && (0..=h.len() - n.len()).any(|i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// How well `q` names `name` (higher is better; `None` = no match): a prefix of the name, a prefix of a
/// word in it, a substring, then a fuzzy subsequence (two letters or more, fewer gaps better).
pub fn quality(name: &str, q: &str) -> Option<i32> {
    if q.is_empty() || name.is_empty() {
        return None;
    }
    let len = name.len().min(200) as i32;
    match search::name_match(name, q) {
        Some(2) => return Some(1000 - len),
        Some(_) => return Some(700 - len),
        None => {}
    }
    if contains_ci(name, q) {
        return Some(500 - len);
    }
    let qb = q.as_bytes();
    if qb.len() < 2 {
        return None;
    }
    let (mut j, mut gaps, mut last) = (0usize, 0i32, None::<usize>);
    for (i, c) in name.bytes().enumerate() {
        if j < qb.len() && c.eq_ignore_ascii_case(&qb[j]) {
            if let Some(l) = last {
                gaps += (i - l - 1) as i32;
            }
            last = Some(i);
            j += 1;
        }
    }
    (j == qb.len()).then(|| (300 - gaps * 10 - len).max(1))
}

/// `display.font_size` → `Display > Font Size` (the kernel text path draws bytes, so `>`, not `›`).
pub fn setting_label(key: &str) -> String {
    let mut out = String::new();
    for (i, seg) in key.split('.').enumerate() {
        if i > 0 {
            out.push_str(" > ");
        }
        let mut up = true;
        for c in seg.chars() {
            if c == '_' {
                out.push(' ');
                up = true;
            } else if up {
                out.push(c.to_ascii_uppercase());
                up = false;
            } else {
                out.push(c);
            }
        }
    }
    out
}

/// Settings the query names: `(key, label, doc, quality)`, best first.
pub fn settings_matches(q: &str) -> Vec<(&'static str, String, &'static str, i32)> {
    let mut v = Vec::new();
    if q.is_empty() {
        return v;
    }
    for k in prefs_core::schema::SCHEMA.iter().filter(|k| k.ns == "system") {
        let label = setting_label(k.key);
        let best = [quality(&label, q), quality(k.key, q).map(|s| s - 50)].into_iter().flatten().max();
        let best = best.or_else(|| (q.len() >= 3 && contains_ci(k.doc, q)).then_some(100));
        if let Some(s) = best {
            v.push((k.key, label, k.doc, s));
        }
    }
    v.sort_by(|a, b| b.3.cmp(&a.3));
    v
}

struct Calc<'a> {
    b: &'a [u8],
    i: usize,
    depth: u32,
}

impl Calc<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i] == b' ' {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.b.get(self.i).copied()
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(b'+') => { self.i += 1; v += self.term()?; }
                Some(b'-') => { self.i += 1; v -= self.term()?; }
                _ => return Some(v),
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.factor()?;
        loop {
            match self.peek() {
                Some(b'*') => { self.i += 1; v *= self.factor()?; }
                Some(b'/') => {
                    self.i += 1;
                    let d = self.factor()?;
                    if d == 0.0 {
                        return None;
                    }
                    v /= d;
                }
                _ => return Some(v),
            }
        }
    }
    fn factor(&mut self) -> Option<f64> {
        self.depth += 1;
        if self.depth > 64 {
            return None;
        }
        let r = match self.peek()? {
            b'-' => { self.i += 1; self.factor().map(|v| -v) }
            b'+' => { self.i += 1; self.factor() }
            b'(' => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(b')') {
                    return None;
                }
                self.i += 1;
                Some(v)
            }
            c if c.is_ascii_digit() || c == b'.' => {
                let (mut v, mut scale, mut frac, mut digits) = (0f64, 1f64, false, 0);
                while let Some(&c) = self.b.get(self.i) {
                    if c.is_ascii_digit() {
                        digits += 1;
                        if frac {
                            scale /= 10.0;
                            v += (c - b'0') as f64 * scale;
                        } else {
                            v = v * 10.0 + (c - b'0') as f64;
                        }
                    } else if c == b'.' && !frac {
                        frac = true;
                    } else {
                        break;
                    }
                    self.i += 1;
                }
                (digits > 0).then_some(v)
            }
            _ => None,
        };
        self.depth -= 1;
        r
    }
}

/// Evaluate `q` when it is a sum (digits and at least one operator, nothing else): the value as text.
pub fn math(q: &str) -> Option<String> {
    let b = q.trim().as_bytes();
    if b.is_empty() || !b.iter().all(|c| b"0123456789.+-*/() ".contains(c)) || !b.iter().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // A lone (signed) number is not a sum.
    if !b.iter().enumerate().any(|(i, c)| b"*/()".contains(c) || (i > 0 && b"+-".contains(c))) {
        return None;
    }
    let mut c = Calc { b, i: 0, depth: 0 };
    let v = c.expr()?;
    if c.peek().is_some() || !v.is_finite() {
        return None;
    }
    if v.abs() < 1e15 && (v as i64) as f64 == v {
        return Some(alloc::format!("{}", v as i64));
    }
    let mut s = alloc::format!("{:.6}", v);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    Some(s)
}

// ── Sources ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Launch {
    /// A dock table app (`dock::launch_named`).
    Table(&'static str),
    /// A program path posted as the shell line (`dock::post_line_launch`).
    Line(String),
}

#[derive(Clone)]
struct Prog {
    key: String,
    name: String,
    how: Launch,
}

fn title_case(s: &str) -> String {
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() {
        o.push(if i == 0 { c.to_ascii_uppercase() } else { c });
    }
    o
}

/// The programs the Launcher offers: the dock's table apps, then every `/apps/*.ELF` the table does not name.
fn programs() -> Vec<Prog> {
    let mut v: Vec<Prog> = Vec::new();
    for n in super::dock::installed_names() {
        let name = crate::fs::appres::builtin_app(n).map(|a| a.name).filter(|s| !s.is_empty()).unwrap_or_else(|| title_case(n));
        v.push(Prog { key: String::from(n), name, how: Launch::Table(n) });
    }
    let mt = crate::shell::vfs_mount_table();
    if let Ok(ents) = mt.read_dir("/apps") {
        for e in ents {
            let up = e.name.to_ascii_uppercase();
            if !matches!(e.kind, crate::fs::vfs::NodeKind::File) || !up.ends_with(".ELF") {
                continue;
            }
            let path = alloc::format!("/apps/{}", e.name);
            let key = crate::fs::appres::key_of_path(&path);
            if v.iter().any(|p| p.key == key) {
                continue;
            }
            let name = crate::fs::appres::app_at(&path).map(|a| a.name).filter(|s| !s.is_empty()).unwrap_or_else(|| title_case(&key));
            v.push(Prog { key, name, how: Launch::Line(path) });
        }
    }
    v
}

// ── Results ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Program,
    File,
    Setting,
    Math,
}

impl Kind {
    pub const fn word(self) -> &'static str {
        match self {
            Kind::Program => "program",
            Kind::File => "file",
            Kind::Setting => "setting",
            Kind::Math => "math",
        }
    }
    const fn group(self) -> &'static str {
        match self {
            Kind::Program => "Programs",
            Kind::File => "Files",
            Kind::Setting => "Settings",
            Kind::Math => "Math",
        }
    }
}

#[derive(Clone)]
pub struct Item {
    pub kind: Kind,
    pub label: String,
    pub detail: String,
    /// What a pick acts on: a program key, a file path, a setting key, a math value.
    pub target: String,
    /// The LRU token (`program:<key>`, `file:<path>`, `setting:<key>`); empty for math.
    pub token: String,
    /// The icon key (programs).
    pub icon: String,
    dir: bool,
    launch: Option<Launch>,
}

const P_MAX: usize = 5;
const F_MAX: usize = 6;
const S_MAX: usize = 4;

/// Rank `q` over the sources: Programs, Files, Settings, Math — each group recent-first, then by quality.
/// Returns the items and `(programs, files, settings, math)` counts.
fn rank(q: &str, progs: &[Prog], files: &[search::Hit], lru: &[String]) -> (Vec<Item>, [usize; 4]) {
    let pos = |t: &str| lru.iter().position(|x| x == t).unwrap_or(usize::MAX);
    let mut out: Vec<Item> = Vec::new();
    let mut n = [0usize; 4];
    if q.is_empty() {
        return (out, n);
    }
    let mut ps: Vec<(usize, i32, Item)> = Vec::new();
    for p in progs {
        let Some(s) = [quality(&p.name, q), quality(&p.key, q)].into_iter().flatten().max() else { continue };
        let token = alloc::format!("program:{}", p.key);
        let detail = match &p.how {
            Launch::Table(_) => String::from("program"),
            Launch::Line(path) => path.clone(),
        };
        ps.push((pos(&token), s, Item { kind: Kind::Program, label: p.name.clone(), detail, target: p.key.clone(), token, icon: p.key.clone(), dir: false, launch: Some(p.how.clone()) }));
    }
    ps.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    n[0] = ps.len();
    out.extend(ps.into_iter().take(P_MAX).map(|x| x.2));

    let mut fs: Vec<(usize, usize, Item)> = Vec::new();
    for (i, h) in search::filter(files, q, 64).into_iter().enumerate() {
        let token = alloc::format!("file:{}", h.path);
        let label = String::from(search::leaf(&h.path));
        let detail = h.path.clone();
        fs.push((pos(&token), i, Item { kind: Kind::File, label, detail, target: h.path, token, icon: String::new(), dir: h.dir, launch: None }));
    }
    fs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    n[1] = fs.len();
    out.extend(fs.into_iter().take(F_MAX).map(|x| x.2));

    let mut ss: Vec<(usize, i32, Item)> = Vec::new();
    for (key, label, doc, s) in settings_matches(q) {
        let token = alloc::format!("setting:{}", key);
        ss.push((pos(&token), s, Item { kind: Kind::Setting, label, detail: String::from(doc), target: String::from(key), token, icon: String::new(), dir: false, launch: None }));
    }
    ss.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    n[2] = ss.len();
    out.extend(ss.into_iter().take(S_MAX).map(|x| x.2));

    if let Some(v) = math(q) {
        n[3] = 1;
        out.push(Item { kind: Kind::Math, label: alloc::format!("= {}", v), detail: alloc::format!("{}   (Return copies)", q.trim()), target: v, token: String::new(), icon: String::new(), dir: false, launch: None });
    }
    (out, n)
}

// ── State ───────────────────────────────────────────────────────────────────────────────────────

const QUERY_MAX: usize = 64;
const LINES_MAX: usize = 1 + P_MAX + 1 + F_MAX + 1 + S_MAX + 1 + 1;
const LRU_MAX: usize = 16;

struct St {
    query: String,
    sel: usize,
    items: Vec<Item>,
    /// Panel placement and size of the row.
    ox: usize,
    oy: usize,
    w: usize,
    h: usize,
    /// `(y0, y1, item)` of each drawn result row, row-relative.
    rows: Vec<(usize, usize, usize)>,
    buf: Vec<u32>,
}

static WIN: AtomicU32 = AtomicU32::new(0);
static ST: spin::Mutex<Option<St>> = spin::Mutex::new(None);
static PROGS: spin::Mutex<Vec<Prog>> = spin::Mutex::new(Vec::new());
static FILES: spin::Mutex<Vec<search::Hit>> = spin::Mutex::new(Vec::new());
static SNAP_OWED: AtomicBool = AtomicBool::new(false);
static RANK_OWED: AtomicBool = AtomicBool::new(false);
static PAINT_OWED: AtomicBool = AtomicBool::new(false);
static PICK: spin::Mutex<Option<Item>> = spin::Mutex::new(None);
static LRU: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());
static LRU_FOR: spin::Mutex<String> = spin::Mutex::new(String::new());
static LRU_OWED: AtomicBool = AtomicBool::new(false);

/// Is the Launcher up?
pub fn is_open() -> bool {
    WIN.load(Ordering::Acquire) != 0
}

// ── Paint ───────────────────────────────────────────────────────────────────────────────────────

fn fill(buf: &mut [u32], w: usize, h: usize, x: usize, y: usize, rw: usize, rh: usize, c: u32) {
    for yy in y..(y + rh).min(h) {
        for xx in x..(x + rw).min(w) {
            buf[yy * w + xx] = c;
        }
    }
}

fn text(buf: &mut [u32], w: usize, h: usize, s: &str, x: usize, top: usize, ink: u32, bold: bool) {
    let ch = menubar::BAR_CELL_H();
    let cw = menubar::BAR_CELL_W().max(1);
    let room = w.saturating_sub(x + 8) / cw;
    let b = s.as_bytes();
    let b = &b[..b.len().min(room)];
    for sy in 0..ch {
        let y = top + sy;
        if y < h {
            super::text::draw_row(&mut buf[y * w..(y + 1) * w], w, b, x, sy, ink, bold, menubar::BAR_FACE);
        }
    }
}

/// Our glyph: an accent tile with a light ring and a stroke out of it — the Launcher's mark.
fn glyph(buf: &mut [u32], w: usize, h: usize, x: usize, y: usize, s: usize) {
    fill(buf, w, h, x, y, s, s, theme::ACCENT);
    let r = (s as i32) / 4;
    let (cx, cy) = (x as i32 + s as i32 * 2 / 5, y as i32 + s as i32 * 2 / 5);
    for yy in 0..s as i32 {
        for xx in 0..s as i32 {
            let (px, py) = (x as i32 + xx, y as i32 + yy);
            let d2 = (px - cx) * (px - cx) + (py - cy) * (py - cy);
            let ring = d2 <= r * r && d2 >= (r - 2).max(0) * (r - 2).max(0);
            let stroke = px - cx == py - cy && px > cx + r / 2 && px < x as i32 + s as i32 - 2;
            if ring || stroke {
                buf[py as usize * w + px as usize] = theme::BEVEL_LIGHT;
            }
        }
    }
}

fn paint(st: &mut St) {
    let (w, h) = (st.w, st.h);
    let ch = menubar::BAR_CELL_H();
    let lh = ch + 10;
    let pad = 12usize;
    let buf = &mut st.buf;
    fill(buf, w, h, 0, 0, w, h, theme::CHROME_FACE);
    fill(buf, w, h, 0, 0, w, 1, theme::FRAME_LINE);
    fill(buf, w, h, 0, h - 1, w, 1, theme::FRAME_LINE);
    fill(buf, w, h, 0, 0, 1, h, theme::FRAME_LINE);
    fill(buf, w, h, w - 1, 0, 1, h, theme::FRAME_LINE);
    // The field.
    let fy = pad;
    let fh = lh + 8;
    fill(buf, w, h, pad, fy, w - 2 * pad, fh, theme::BEVEL_LIGHT);
    fill(buf, w, h, pad, fy + fh - 1, w - 2 * pad, 1, theme::FRAME_LINE);
    glyph(buf, w, h, pad + 6, fy + (fh - ch) / 2, ch);
    let tx = pad + 12 + ch;
    let ty = fy + (fh - ch) / 2;
    if st.query.is_empty() {
        text(buf, w, h, "Launcher - programs, files, settings, or a sum", tx, ty, theme::TITLE_TEXT_INACTIVE, false);
    } else {
        text(buf, w, h, &st.query, tx, ty, theme::CONTENT_TEXT, false);
        let cx = tx + st.query.len() * menubar::BAR_CELL_W() + 1;
        fill(buf, w, h, cx, ty, 2, ch, theme::ACCENT);
    }
    // The groups.
    st.rows.clear();
    let mut y = fy + fh + 6;
    let mut last: Option<Kind> = None;
    if !st.query.is_empty() && st.items.is_empty() {
        text(buf, w, h, "No results", pad + 8, y + 4, theme::TITLE_TEXT_INACTIVE, false);
    }
    for (i, it) in st.items.iter().enumerate() {
        if y + lh > h {
            break;
        }
        if last != Some(it.kind) {
            text(buf, w, h, it.kind.group(), pad + 4, y + 4, theme::TITLE_TEXT_INACTIVE, true);
            y += lh;
            last = Some(it.kind);
            if y + lh > h {
                break;
            }
        }
        let on = i == st.sel;
        let (ink, dim) = if on { (theme::BEVEL_LIGHT, theme::BEVEL_LIGHT) } else { (theme::CONTENT_TEXT, theme::TITLE_TEXT_INACTIVE) };
        if on {
            fill(buf, w, h, pad, y, w - 2 * pad, lh, theme::ACCENT);
        }
        let ix = pad + 8;
        let iy = y + (lh - ch) / 2;
        let drew = it.kind == Kind::Program && crate::fs::appres::blit_key_icon(buf, w, h, ix, iy, ch, &it.icon);
        if !drew {
            let mark = match it.kind {
                Kind::Program => "P",
                Kind::File if it.dir => "D",
                Kind::File => "F",
                Kind::Setting => "S",
                Kind::Math => "=",
            };
            fill(buf, w, h, ix, iy, ch, ch, if on { theme::BEVEL_LIGHT } else { theme::FRAME_LINE });
            text(buf, w, h, mark, ix + ch / 4, iy, if on { theme::ACCENT } else { theme::BEVEL_LIGHT }, true);
        }
        let lx = ix + ch + 10;
        text(buf, w, h, &it.label, lx, iy, ink, on);
        let dx = lx + (it.label.len() + 3) * menubar::BAR_CELL_W();
        if dx + 8 * menubar::BAR_CELL_W() < w {
            text(buf, w, h, &it.detail, dx, iy, dim, false);
        }
        st.rows.push((y, y + lh, i));
        y += lh;
    }
}

fn repaint() {
    let id = WIN.load(Ordering::Acquire);
    if id == 0 {
        return;
    }
    {
        let mut g = ST.lock();
        let Some(st) = g.as_mut() else { return };
        paint(st);
    }
    let _ = wm::present(id);
}

// ── Open / close ────────────────────────────────────────────────────────────────────────────────

/// Open the Launcher (idempotent). `false` on refusal (no panel, allocation, window table).
pub fn open() -> bool {
    if is_open() {
        return true;
    }
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            return false;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    let (cw, ch) = (menubar::BAR_CELL_W(), menubar::BAR_CELL_H());
    let lh = ch + 10;
    let w = (cw * 72 + 24).min(pw.saturating_sub(40)).max(64);
    let h = (24 + lh + 8 + 6 + LINES_MAX * lh).min(ph.saturating_sub(40)).max(64);
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(w * h).is_err() {
        return false;
    }
    buf.resize(w * h, theme::CHROME_FACE);
    let ox = pw.saturating_sub(w) / 2;
    let oy = (ph / 3).saturating_sub(h / 3).max(ph / 10);
    let mut st = St { query: String::new(), sel: 0, items: Vec::new(), ox, oy, w, h, rows: Vec::new(), buf };
    paint(&mut st);
    let (addr, len) = (st.buf.as_ptr() as usize, st.buf.len() * 4);
    *ST.lock() = Some(st);
    let id = wm::overlay_open(addr, len, w, h, ox, oy);
    if id == wm::WIN_NONE {
        *ST.lock() = None;
        serial_println!("[launcher] open REFUSED reason=overlay");
        return false;
    }
    wm::set_modal_top(id);
    WIN.store(id, Ordering::Release);
    SNAP_OWED.store(true, Ordering::Release);
    serial_println!("[launcher] open win={} {}x{} at={},{}", id, w, h, ox, oy);
    true
}

/// Close the Launcher (idempotent).
pub fn close() -> bool {
    let id = WIN.swap(0, Ordering::AcqRel);
    if id == 0 {
        return false;
    }
    wm::clear_modal_top(id);
    wm::close(id);
    *ST.lock() = None; // after the row is gone
    PROGS.lock().clear();
    FILES.lock().clear();
    serial_println!("[launcher] close");
    true
}

// ── The doors (input router: edit and latch only) ───────────────────────────────────────────────

fn take_selected() -> Option<Item> {
    let g = ST.lock();
    let st = g.as_ref()?;
    st.items.get(st.sel).cloned()
}

fn owe_pick(it: Item) {
    *PICK.lock() = Some(it);
    close();
    #[cfg(not(feature = "quarry"))]
    service();
}

fn owe_rank() {
    RANK_OWED.store(true, Ordering::Release);
    #[cfg(not(feature = "quarry"))]
    service();
}

fn owe_paint() {
    PAINT_OWED.store(true, Ordering::Release);
    #[cfg(not(feature = "quarry"))]
    service();
}

/// **The key door** — asked FIRST by x86's `wc_route_event`. Cmd-Space toggles the Launcher; while it is up
/// every key and chord is its own (consumed). `true` when consumed.
pub fn key_door(ev: crate::pal::Event) -> bool {
    use crate::pal::Event;
    if let Event::Action(super::keymap::Action::Launcher) = ev {
        if is_open() {
            close();
        } else {
            let _ = open();
        }
        return true;
    }
    if !is_open() {
        return false;
    }
    match ev {
        Event::Key(c) => {
            match c {
                0x1B => {
                    close();
                }
                b'\r' | b'\n' => {
                    if let Some(it) = take_selected() {
                        owe_pick(it);
                    }
                }
                0x1F | 0x1E => {
                    if let Some(st) = ST.lock().as_mut() {
                        let n = st.items.len();
                        if n > 0 {
                            st.sel = if c == 0x1F { st.sel.saturating_sub(1) } else { (st.sel + 1).min(n - 1) };
                        }
                    }
                    owe_paint();
                }
                0x08 | 0x7F => {
                    if let Some(st) = ST.lock().as_mut() {
                        st.query.pop();
                    }
                    owe_rank();
                }
                0x20..=0x7E => {
                    if let Some(st) = ST.lock().as_mut() {
                        if st.query.len() < QUERY_MAX {
                            st.query.push(c as char);
                        }
                    }
                    owe_rank();
                }
                _ => {}
            }
            true
        }
        Event::KeyUp(_) | Event::Action(_) => true,
        _ => false,
    }
}

/// **The click door** — the click router's first arm. While the Launcher is up a primary press on a result
/// row picks it; any other press closes the Launcher. Consumed (`true`) whenever it was up.
pub fn press_at(x: i32, y: i32, mask: u8) -> bool {
    if !is_open() {
        return false;
    }
    let hit = {
        let g = ST.lock();
        g.as_ref().and_then(|st| {
            if mask & 0x01 == 0 || x < st.ox as i32 || y < st.oy as i32 {
                return None;
            }
            let (rx, ry) = (x as usize - st.ox, y as usize - st.oy);
            if rx >= st.w {
                return None;
            }
            st.rows.iter().find(|r| ry >= r.0 && ry < r.1).and_then(|r| st.items.get(r.2).cloned())
        })
    };
    match hit {
        Some(it) => owe_pick(it),
        None => {
            close();
        }
    }
    true
}

// ── The pass (off the router) ───────────────────────────────────────────────────────────────────

fn lru_path() -> Option<String> {
    crate::prefs::home().filter(|h| !h.is_empty()).map(|h| alloc::format!("{}/settings/launcher", h))
}

/// Load the recency file when the session's home changed since the last load.
fn lru_load() {
    let Some(p) = lru_path() else { return };
    if *LRU_FOR.lock() == p {
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    let mut v: Vec<String> = Vec::new();
    if let Ok(st) = mt.stat(&p) {
        if st.size > 0 && st.size < 16 * 1024 {
            if let Ok(b) = mt.read(&p, 0, st.size as usize) {
                if let Some(t) = core::str::from_utf8(&b).ok().and_then(|s| prefs_core::PrefTree::parse(s).ok()) {
                    for i in 0..LRU_MAX {
                        if let Some(prefs_core::PrefValue::Str(s)) = t.get("launcher", &alloc::format!("r{:02}", i)) {
                            v.push(s.clone());
                        }
                    }
                } else {
                    serial_println!("[launcher] recency file {} refused (not Principia TOML) -> empty", p);
                }
            }
        }
    }
    serial_println!("[launcher] recency loaded {} recent={}", p, v.len());
    *LRU.lock() = v;
    *LRU_FOR.lock() = p;
}

fn lru_note(token: &str) {
    if token.is_empty() {
        return;
    }
    let mut l = LRU.lock();
    l.retain(|t| t != token);
    l.insert(0, String::from(token));
    l.truncate(LRU_MAX);
    LRU_OWED.store(true, Ordering::Release);
}

/// Write the recency file: `<home>/settings/launcher`, Principia's TOML, namespace `launcher`, keys `r00..`.
fn lru_save() -> bool {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as K};
    let Some(p) = lru_path() else { return false };
    let mut t = prefs_core::PrefTree::new();
    let l = LRU.lock().clone();
    for (i, s) in l.iter().enumerate() {
        let _ = t.set("launcher", &alloc::format!("r{:02}", i), prefs_core::PrefValue::Str(s.clone()));
    }
    let body = t.to_toml();
    let mt = crate::shell::vfs_mount_table();
    let dir = p.trim_end_matches("/launcher");
    if mt.stat(dir).is_err() {
        let _ = mt.create(dir, NodeKind::Dir, K);
    }
    let _ = mt.unlink(&p, K);
    let ok = mt.create(&p, NodeKind::File, K).is_ok() && {
        let b = body.as_bytes();
        let mut off = 0usize;
        while off < b.len() {
            match mt.write(&p, off as u64, &b[off..], K) {
                Ok(n) if n > 0 => off += n,
                _ => break,
            }
        }
        off == b.len()
    };
    serial_println!("[launcher] saved {} recent={} ok={}", p, l.len(), ok as u8);
    ok
}

/// Act on a pick: a program launches, a file opens by its opener, a setting opens Settings on its row, a sum
/// is copied. Returns the wire's `how`.
fn act(it: &Item) -> String {
    let how = match it.kind {
        Kind::Program => match &it.launch {
            Some(Launch::Table(n)) => match super::dock::launch_named(n) {
                Some(h) => alloc::format!("{} origin=glass", h),
                None => String::from("refused (not in this build)"),
            },
            Some(Launch::Line(path)) => {
                if super::dock::post_line_launch(path) { alloc::format!("line-posted path={} origin=glass", path) } else { String::from("refused (line slot busy)") }
            }
            None => String::from("refused (no launch)"),
        },
        Kind::File => open_file(&it.target, it.dir),
        Kind::Setting => {
            let (tab, ctrl) = super::settings::request_open_at(&it.target);
            alloc::format!("settings tab={} control={} key=system.{}", tab, ctrl, it.target)
        }
        Kind::Math => {
            let ok = super::clipboard::set(it.target.as_bytes());
            alloc::format!("clipboard value={} copied={}", it.target, ok as u8)
        }
    };
    serial_println!("[launcher] pick kind={} name={} -> {}", it.kind.word(), it.label, how);
    lru_note(&it.token);
    how
}

#[cfg(feature = "quarry")]
fn open_file(path: &str, dir: bool) -> String {
    if dir {
        super::quarry::request_open();
        return alloc::format!("quarry (a folder; Quarry opens at its own start) path={}", path);
    }
    let (mime, _) = crate::fs::filetype::type_of(path);
    let (opener, _) = crate::fs::assoc::opener_for(path, &mime);
    let eff = super::quarry::live::openers::effective(&opener, path);
    if eff == "none" {
        return alloc::format!("no opener type={} path={}", mime, path);
    }
    let line = super::quarry::live::openers::open(&opener, path, &mime);
    alloc::format!("open type={} opener={} path={} ({})", mime, eff, path, line)
}

#[cfg(not(feature = "quarry"))]
fn open_file(path: &str, _dir: bool) -> String {
    alloc::format!("no opener dispatch in this build (UNAOS_QUARRY arms it) path={}", path)
}

/// Re-rank the query and repaint; the query line on the wire.
fn rerank() {
    let t0 = crate::arch::ms();
    let q = match ST.lock().as_ref() {
        Some(st) => st.query.clone(),
        None => return,
    };
    let lru = LRU.lock().clone();
    let (items, n) = {
        let p = PROGS.lock();
        let f = FILES.lock();
        rank(&q, &p, &f, &lru)
    };
    if let Some(st) = ST.lock().as_mut() {
        if st.query == q {
            st.items = items;
            st.sel = 0;
        } else {
            RANK_OWED.store(true, Ordering::Release); // typed again meanwhile: once more next pass
        }
    }
    repaint();
    if !q.is_empty() {
        serial_println!("[launcher] query={} hits=p{}/f{}/s{}{} ms={}", q, n[0], n[1], n[2], if n[3] != 0 { "/m1" } else { "" }, crate::arch::ms().saturating_sub(t0));
    }
}

/// **The pass** — chained from `settings::service` (the desktop's service tick). Idle: four atomic loads.
pub fn service() {
    if let Some(it) = PICK.lock().take() {
        lru_load();
        let _ = act(&it);
    }
    if LRU_OWED.swap(false, Ordering::AcqRel) {
        let _ = lru_save();
    }
    if !is_open() {
        return;
    }
    if SNAP_OWED.swap(false, Ordering::AcqRel) {
        let t0 = crate::arch::ms();
        lru_load();
        let p = programs();
        let s = search::snapshot(search::SNAP_BUDGET);
        serial_println!("[launcher] snapshot programs={} files={} truncated={} ms={}", p.len(), s.hits.len(), s.truncated as u8, crate::arch::ms().saturating_sub(t0));
        *PROGS.lock() = p;
        *FILES.lock() = s.hits;
        RANK_OWED.store(true, Ordering::Release);
    }
    if RANK_OWED.swap(false, Ordering::AcqRel) {
        PAINT_OWED.store(false, Ordering::Release);
        rerank();
    } else if PAINT_OWED.swap(false, Ordering::AcqRel) {
        repaint();
    }
}

// ── The fixture ─────────────────────────────────────────────────────────────────────────────────

/// `tests launcher` (R80: typed, never at boot). Programs for `set` (the Settings table app is in every
/// build), files for `test` (TESTF's staged `/system/test-f`; zero is a pass only when that folder is absent),
/// settings for `bright` (must offer `Display > Brightness`), math `2*(3+4)` = 14 (and `1/0` refused), and —
/// with a panel — the row opened by the door's own Cmd-Space, typed into, ranked by the pass, closed by Esc.
pub fn selftest() {
    let t0 = crate::arch::ms();
    let progs = programs();
    let snap = search::snapshot(search::SNAP_BUDGET);
    let (items_p, n_p) = rank("set", &progs, &[], &[]);
    let (_, n_f) = rank("test", &[], &snap.hits, &[]);
    let sm = settings_matches("bright");
    let bright = sm.first().map(|s| s.0 == "display.brightness" && s.1 == "Display > Brightness").unwrap_or(false);
    let math_ok = math("2*(3+4)").as_deref() == Some("14") && math("1/0").is_none() && math("42").is_none() && math("1.5*2").as_deref() == Some("3");
    let testf = crate::shell::vfs_mount_table().stat("/system/test-f").is_ok();
    let programs_n = n_p[0];
    let files_n = n_f[1];
    let settings_n = sm.len();
    let set_first = items_p.first().map(|i| i.target == "settings").unwrap_or(false);
    // The door, end to end.
    let open_word = if WIN.load(Ordering::Acquire) != 0 {
        "busy"
    } else {
        let opened = key_door(crate::pal::Event::Action(super::keymap::Action::Launcher));
        if !is_open() {
            let _ = opened;
            "skip-headless"
        } else {
            for c in b"bright" {
                key_door(crate::pal::Event::Key(*c));
            }
            service();
            let ranked = ST.lock().as_ref().map(|st| st.query == "bright" && st.items.iter().any(|i| i.kind == Kind::Setting && i.target == "display.brightness")).unwrap_or(false);
            key_door(crate::pal::Event::Key(0x08));
            service();
            let back = ST.lock().as_ref().map(|st| st.query == "brigh").unwrap_or(false);
            key_door(crate::pal::Event::Key(0x1B));
            let closed = !is_open();
            serial_println!("[launcher] fixture door ranked={} backspace={} esc_closed={}", ranked, back, closed);
            if ranked && back && closed { "ok" } else { "FAIL" }
        }
    };
    let ms = crate::arch::ms().saturating_sub(t0);
    serial_println!("[launcher] fixture set_first={} bright={} testf={} walk={} truncated={}", set_first, bright, testf, snap.hits.len(), snap.truncated as u8);
    let ok = programs_n >= 1 && set_first && (files_n >= 1 || !testf) && settings_n >= 1 && bright && math_ok && open_word != "FAIL";
    serial_println!(
        ":: LAUNCHER: programs={} files={} settings={} math={} open={} ms={} -> {} ::",
        programs_n, files_n, settings_n, if math_ok { "ok" } else { "FAIL" }, open_word, ms, if ok { "PASS" } else { "FAIL" }
    );
}

/// `tests launcher` registration, once (rides `appres::ensure_tests`).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("launcher", selftest);
    }
}

