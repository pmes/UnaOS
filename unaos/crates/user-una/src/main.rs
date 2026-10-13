#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAMETAL (LEDGER SR90, R104): `APPS/UNA.ELF` — the Una IDE's UnaOS-native face: the Code layout (CODEX §4) as one
// ring-3 program. CHARTER: Una is the vessel; the panes are their handlers' cores — Tabula (edit_core, shared-core,
// SR73) in the centre, Midden (midden_core, shared-core, SR75) at the bottom, the Matrix's file view (the ring-3
// file surface) in the sidebar. The host Una (vessels/una, GTK) stays as the other-OS face (R104).
//
// WHAT IT IS. One 288x288 compositor window (the x86 per-window maximum, FB_WIN_MAX) split three ways:
//
//   +--------+---------------------------+
//   | tree   | editor (edit_core)        |   sidebar  x 0..SIDE_W         y 0..CON_Y
//   |        |                           |   editor   x SIDE_W+1..W       y 0..CON_Y, its status line at the foot
//   |        |---------------------------|
//   |        | path* L:C  message        |
//   +--------+---------------------------+
//   | console (midden_core)              |   console  x 0..W              y CON_Y+1..H
//   +------------------------------------+
//
// The workspace root is argv[1] (the args page; `una /home/peter/proj`), or the session home (SYS_WHOAMI), or `/`.
// An argv[1] that names a FILE opens it in the editor and roots the tree at its directory.
//
// FOCUS. Ctrl+1 / Ctrl+2 / Ctrl+3 = tree / editor / console, and a click on a pane. The ring carries one byte and
// no modifier bits, and the HID decoder SUPPRESSES the press of Ctrl+<non-letter> (xhci `hid_key_ascii`: its fold
// would collide with the arrow bytes), but it still delivers the RELEASE as the bare digit (ALLKEYS: a release is
// never suppressed). So a KEY_UP of `1`/`2`/`3` with no KEY_DOWN of it before is exactly a modified digit — Ctrl
// (or Alt/Cmd) + 1/2/3 — and that is the chord this program reads. Esc cycles the focus too (one key, no chord).
// A click focuses the pane under the pointer; the pointer is this program's own estimate, moved by MOUSE_REL deltas
// and clamped to the window (x86 gives ring 3 no window origin to map MOUSE_ABS through — owed, named here).
//
// GLOBAL KEYS. Ctrl+S save the editor's file; Ctrl+O open the tree's selected file; Ctrl+Q quit (and a window
// close request). Everything else goes to the focused pane (tree.rs, editor.rs, console.rs say their keys).
//
// WIRE. `:: UNA: start window=<id> font=<face|font8x8 font_why=<w>> panes=3 root=<p> entries=<n> cwd=<p> ::` once;
// the console's `:: UNA: plan=<Nothing|Say|Host|Exec> verb=<v> served=<0|1> rows=<n> pane=console ::` per line; the
// editor's `:: UNA: open path=<p> bytes=<n> lines=<n> lang=<id|none> spans=<n> rc=<r> ::` (UNAMETAL2: the syntax
// core's grammar and token count) and `:: UNA: save path=<p> bytes=<n> rc=<r> ::`;
// on exit `:: UNA: exit status=<s> panes=3 lines=<n> cmds=<n> focus=<tree|editor|console> ::`. The exit status packs
// the facts `tests una` (kernel unaapp.rs) reads back — see `status`.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use una_abi::{
    input_ev_payload, input_ev_type, INPUT_EV_ACTION, INPUT_EV_BUTTON, INPUT_EV_CLOSE_REQ, INPUT_EV_KEY_DOWN, INPUT_EV_KEY_UP,
    INPUT_EV_MOUSE_REL, KEY_ESC, SYS_EXIT, SYS_INPUT_POLL, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};
use vein_ring3::sys::{sleep_ms, sys, write};

mod console;
mod editor;
mod fs;
mod tree;
mod txt;

/// MIDDEN.ELF's size-class heap over SYS_SBRK: the document, the tree, plans, scrollback and the face bytes.
#[global_allocator]
static HEAP: vein_ring3::heap::Heap = vein_ring3::heap::Heap::sbrk();

const W: i32 = 288; // the x86 compositor's per-window maximum (FB_WIN_MAX_W/H)
const H: i32 = 288;
const STRIDE: usize = W as usize * 4;
/// The sidebar's width and the console's top edge (px).
const SIDE_W: i32 = 86;
const CON_Y: i32 = 190;
const PANES: u32 = 3;

const BG: u32 = 0xFF12_141A;
const SIDE_BG: u32 = 0xFF16_1922;
const ED_BG: u32 = 0xFF1A_1E28;
const STATUS_BG: u32 = 0xFF24_2A38;
const INK: u32 = 0xFFE6_E3EC;
const DIM_INK: u32 = 0xFF8C_8A99;
const DIR_INK: u32 = 0xFF9F_E6B0;
const ERR_INK: u32 = 0xFFFF_8A80;
const ECHO_INK: u32 = 0xFF8F_B8FF;
const CARET: u32 = 0xFFB0_8CFF;
const RULE: u32 = 0xFF3A_3550;
const SEL_BG: u32 = 0xFF3A_3F66;
const ROW_SEL: u32 = 0xFF2E_3348;

static FONT: [[u8; 8]; 128] = font8x8::legacy::BASIC_LEGACY;

fn exit(code: i32) -> ! {
    sys(SYS_EXIT, code as u32 as u64, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

// ---- wire lines -----------------------------------------------------------------------------------------
pub struct Line(Vec<u8>);
impl Line {
    pub fn new(s: &[u8]) -> Line {
        Line(s.to_vec())
    }
    pub fn put(&mut self, s: &[u8]) -> &mut Self {
        self.0.extend_from_slice(s);
        self
    }
    pub fn dec(&mut self, v: i64) -> &mut Self {
        self.0.extend_from_slice(alloc::format!("{}", v).as_bytes());
        self
    }
    pub fn wire(&mut self) {
        self.0.push(b'\n');
        write(&self.0);
    }
}

/// The exit status `tests una` reads back (`bg_poll`'s `Exited(i32)`; bit 31 stays clear). UNAMETAL2 (LEDGER SR98)
/// repacked it to carry the save and the highlight: the console's scrollback rows `[9:0]`, its lines planned
/// `[11:10]` (cap 3), the last plan's kind `[13:12]` (0 Nothing, 1 Say, 2 Host, 3 Exec), its verb code `[21:14]`
/// (`midden_core::ring3::verb_code`), the pane count `[23:22]`, a window `[24]`, a loaded face `[25]`, SAVED `[26]`
/// (the last Ctrl+S answered rc=0), HIGHLIGHT `[30:27]` (the document's syntax_core token count, cap 15). The kernel
/// decodes it in `unaapp.rs` `status_parse` — the two are kept in step by hand.
#[allow(clippy::too_many_arguments)]
const fn status(rows: u32, cmds: u32, kind: u32, verb: u8, panes: u32, window: bool, face: bool, saved: bool, spans: u32) -> i32 {
    let r = if rows > 0x3FF { 0x3FF } else { rows };
    let c = if cmds > 3 { 3 } else { cmds };
    let h = if spans > 0xF { 0xF } else { spans };
    (r | (c << 10)
        | ((kind & 3) << 12)
        | ((verb as u32) << 14)
        | ((panes & 3) << 22)
        | ((window as u32) << 24)
        | ((face as u32) << 25)
        | ((saved as u32) << 26)
        | (h << 27)) as i32
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Tree,
    Editor,
    Console,
}
impl Pane {
    fn name(self) -> &'static [u8] {
        match self {
            Pane::Tree => b"tree",
            Pane::Editor => b"editor",
            Pane::Console => b"console",
        }
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}
impl Rect {
    fn has(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}
const R_TREE: Rect = Rect { x: 0, y: 0, w: SIDE_W, h: CON_Y };
const R_ED: Rect = Rect { x: SIDE_W + 1, y: 0, w: W - SIDE_W - 1, h: CON_Y };
const R_CON: Rect = Rect { x: 0, y: CON_Y + 1, w: W, h: H - CON_Y - 1 };

struct App {
    surf: *mut u8,
    win: i64,
    face: Option<txt::Txt>,
    focus: Pane,
    tree: tree::Tree,
    ed: editor::Editor,
    con: console::Console,
    /// Bytes whose KEY_DOWN this program saw and whose KEY_UP it has not (the modified-digit chord test).
    down: [bool; 128],
    ptr: (i32, i32),
    buttons: u8,
    dirty: bool,
}

impl App {
    fn lh(&self) -> i32 {
        if self.face.is_some() { txt::LH_TT } else { txt::LH_BITMAP }
    }
    fn cell(&self) -> f32 {
        self.face.as_ref().map_or(8.0, |t| t.cell)
    }
    fn cols_in(&self, w: i32) -> usize {
        (((w - 8).max(0) as f32) / self.cell()) as usize
    }
    /// The editor's status line top and its visible text rows.
    fn ed_geom(&self) -> (i32, usize) {
        let lh = self.lh();
        let sy = R_ED.y + R_ED.h - lh - 2;
        (sy, ((sy - 3) / lh).max(1) as usize)
    }
    fn tree_rows(&self) -> usize {
        ((R_TREE.h - 4) / self.lh() - 1).max(1) as usize
    }

    fn set_focus(&mut self, p: Pane) {
        if self.focus != p {
            self.focus = p;
            self.dirty = true;
        }
    }
    fn cycle_focus(&mut self) {
        let n = match self.focus {
            Pane::Tree => Pane::Editor,
            Pane::Editor => Pane::Console,
            Pane::Console => Pane::Tree,
        };
        self.set_focus(n);
    }

    fn open_selected(&mut self) {
        match self.tree.selected_file().map(String::from) {
            Some(p) => {
                self.ed.open(&p);
                self.set_focus(Pane::Editor);
            }
            None => {
                self.ed.msg = String::from("select a file in the tree (Ctrl+1)");
                self.ed.repaint = true;
            }
        }
    }

    fn quit(&mut self) -> ! {
        let rows = self.con.rows.len() as u32;
        let spans = self.ed.highlight.as_ref().map_or(0, |h| h.spans()) as u32;
        let st = status(rows, self.con.cmds, self.con.last_kind, self.con.last_verb, PANES, self.win >= 0, self.face.is_some(), self.ed.saved, spans);
        let mut l = Line::new(b":: UNA: exit status=");
        l.dec(st as i64).put(b" panes=").dec(PANES as i64).put(b" lines=").dec(rows as i64);
        l.put(b" saved=").dec(self.ed.saved as i64).put(b" spans=").dec(spans as i64);
        l.put(b" cmds=").dec(self.con.cmds as i64).put(b" focus=").put(self.focus.name()).put(b" ::");
        l.wire();
        exit(st)
    }

    // ---- input ----
    fn key_down(&mut self, k: u8) {
        if (k as usize) < self.down.len() {
            self.down[k as usize] = true;
        }
        match k {
            0x11 => self.quit(),           // Ctrl+Q
            0x13 => return self.ed.save(), // Ctrl+S
            0x0F => return self.open_selected(), // Ctrl+O
            KEY_ESC => {
                if !(self.focus == Pane::Editor && self.ed.escape()) {
                    self.cycle_focus();
                }
                return;
            }
            _ => {}
        }
        match self.focus {
            Pane::Tree => {
                if let tree::Act::Open(p) = self.tree.key(k) {
                    self.ed.open(&p);
                    self.set_focus(Pane::Editor);
                }
            }
            Pane::Editor => self.ed.key(k),
            Pane::Console => {
                if let console::Act::Quit = self.con.key(k) {
                    self.quit();
                }
            }
        }
    }
    fn key_up(&mut self, k: u8) {
        let i = k as usize;
        if i >= self.down.len() {
            return;
        }
        if self.down[i] {
            self.down[i] = false;
            return;
        }
        // A release whose press was suppressed: Ctrl/Alt/Cmd + digit (see FOCUS above).
        match k {
            b'1' => self.set_focus(Pane::Tree),
            b'2' => self.set_focus(Pane::Editor),
            b'3' => self.set_focus(Pane::Console),
            _ => {}
        }
    }
    fn button(&mut self, mask: u8) {
        let pressed = mask & 1 != 0 && self.buttons & 1 == 0;
        self.buttons = mask;
        if !pressed {
            return;
        }
        let (x, y) = self.ptr;
        if R_TREE.has(x, y) {
            self.set_focus(Pane::Tree);
            let r = (y - R_TREE.y - 2) / self.lh() - 1;
            if r >= 0 {
                self.tree.select_row(r as usize);
            }
        } else if R_ED.has(x, y) {
            self.set_focus(Pane::Editor);
        } else if R_CON.has(x, y) {
            self.set_focus(Pane::Console);
        }
    }

    // ---- paint ----
    fn fill(&self, r: Rect, c: u32) {
        let (xs, ys) = (r.x.max(0), r.y.max(0));
        let (xe, ye) = ((r.x + r.w).min(W), (r.y + r.h).min(H));
        for y in ys..ye {
            let row = unsafe { self.surf.add(y as usize * STRIDE) as *mut u32 };
            for x in xs..xe {
                unsafe { row.add(x as usize).write_volatile(c) };
            }
        }
    }
    /// Draw printable ASCII `s` with its line box's top at `y`, clipped to `clip`.
    fn text(&mut self, x: i32, y: i32, s: &[u8], c: u32, clip: Rect) {
        let surf = self.surf;
        if let Some(t) = self.face.as_mut() {
            t.draw(x as f32, y, s, c, &mut |gx, gy, cov| {
                if clip.has(gx, gy) && gx >= 0 && gy >= 0 && gx < W && gy < H {
                    let p = unsafe { (surf.add(gy as usize * STRIDE) as *mut u32).add(gx as usize) };
                    let bg = unsafe { p.read_volatile() };
                    let px = 0xFF00_0000 | font_core::ui::blend(bg & 0x00FF_FFFF, c & 0x00FF_FFFF, cov);
                    unsafe { p.write_volatile(px) };
                }
            });
            return;
        }
        for (n, &ch) in s.iter().enumerate() {
            let g = &FONT[(ch & 0x7f) as usize];
            let gx = x + n as i32 * 8;
            for (r, &bits) in g.iter().enumerate() {
                for col in 0..8 {
                    let (px, py) = (gx + col, y + 1 + r as i32);
                    if bits & (1 << col) != 0 && clip.has(px, py) {
                        self.fill(Rect { x: px, y: py, w: 1, h: 1 }, c);
                    }
                }
            }
        }
    }
    fn cx(&self, x0: i32, col: usize) -> i32 {
        x0 + (col as f32 * self.cell()) as i32
    }

    fn paint_tree(&mut self) {
        let lh = self.lh();
        let r = R_TREE;
        self.fill(r, SIDE_BG);
        let cols = self.cols_in(r.w);
        let title = self.tree.root.rsplit('/').find(|s| !s.is_empty()).unwrap_or("/");
        let title: Vec<u8> = alloc::format!("{}/", title).bytes().map(|b| if (0x20..=0x7e).contains(&b) { b } else { b'?' }).take(cols).collect();
        self.text(4, r.y + 2, &title, DIM_INK, r);
        let rows = self.tree_rows();
        self.tree.scroll_to_sel(rows);
        let focused = self.focus == Pane::Tree;
        let n = self.tree.nodes.len();
        for row in 0..rows {
            let i = self.tree.top + row;
            if i >= n {
                break;
            }
            let y = r.y + 2 + (row as i32 + 1) * lh;
            if i == self.tree.sel {
                self.fill(Rect { x: r.x, y, w: r.w, h: lh }, if focused { SEL_BG } else { ROW_SEL });
            }
            let (mark, ink) = {
                let nd = &self.tree.nodes[i];
                if nd.err {
                    (b' ', ERR_INK)
                } else if nd.dir {
                    (if nd.open { b'-' } else { b'+' }, DIR_INK)
                } else {
                    (b' ', INK)
                }
            };
            let ind = self.tree.indent(i);
            let mut s: Vec<u8> = Vec::new();
            s.extend(core::iter::repeat(b' ').take(ind));
            s.push(mark);
            s.extend(self.tree.nodes[i].name.bytes().map(|b| if (0x20..=0x7e).contains(&b) { b } else { b'?' }));
            s.truncate(cols);
            self.text(2, y, &s, ink, r);
        }
        if n == 0 {
            self.text(4, r.y + 2 + lh, b"(empty)", DIM_INK, r);
        }
    }

    fn paint_editor(&mut self) {
        let lh = self.lh();
        let r = R_ED;
        self.fill(r, ED_BG);
        let (sy, rows) = self.ed_geom();
        let cols = self.cols_in(r.w);
        self.ed.view(rows, cols);
        let x0 = r.x + 4;
        let text_clip = Rect { x: r.x, y: r.y, w: r.w, h: sy - r.y };
        let sels: Vec<(usize, usize)> = self.ed.buf.selections().iter().filter(|s| !s.is_empty()).map(|s| (s.start(), s.end())).collect();
        let nlines = self.ed.buf.line_count();
        let (left, top) = (self.ed.left, self.ed.top);
        for row in 0..rows {
            let l = top + row;
            if l >= nlines {
                break;
            }
            let y = r.y + 2 + row as i32 * lh;
            let line = self.ed.buf.line(l);
            let ls = self.ed.buf.pos_to_offset(edit_core::Pos::new(l, 0));
            let idx: Vec<(usize, char)> = line.char_indices().skip(left).take(cols).collect();
            // The selection behind the text.
            for &(a, b) in &sels {
                for (c, &(bi, ch)) in idx.iter().enumerate() {
                    let off = ls + bi;
                    if off >= a && off + ch.len_utf8() <= b {
                        let xa = self.cx(x0, c);
                        let xb = self.cx(x0, c + 1);
                        self.fill(Rect { x: xa, y, w: xb - xa, h: lh }, SEL_BG);
                    }
                }
            }
            let glyphs: Vec<u8> = idx.iter().map(|&(_, ch)| if (' '..='~').contains(&ch) { ch as u8 } else if ch == '\t' { b' ' } else { b'?' }).collect();
            // UNAMETAL2 (SR98): each char's ink is its syntax_core token's appearance token; runs of one ink draw once.
            let inks: Option<Vec<u32>> = self.ed.highlight.as_ref().map(|h| idx.iter().map(|&(bi, _)| h.ink_at(l, bi)).collect());
            match inks {
                Some(inks) => {
                    let mut a = 0;
                    while a < inks.len() {
                        let mut b = a + 1;
                        while b < inks.len() && inks[b] == inks[a] {
                            b += 1;
                        }
                        let x = self.cx(x0, a);
                        self.text(x, y, &glyphs[a..b], inks[a], text_clip);
                        a = b;
                    }
                }
                None => self.text(x0, y, &glyphs, INK, text_clip),
            }
        }
        // The caret (full on focus, dim otherwise).
        let (cl, cc) = self.ed.caret_lc();
        if cl >= top && cl < top + rows && cc >= left {
            let x = self.cx(x0, cc - left);
            let y = r.y + 2 + (cl - top) as i32 * lh;
            self.fill(Rect { x, y, w: 2, h: lh - 1 }, if self.focus == Pane::Editor { CARET } else { RULE });
        }
        // The status line: the find prompt, or path / dirty / line:col / message.
        let sr = Rect { x: r.x, y: sy, w: r.w, h: r.y + r.h - sy };
        self.fill(sr, STATUS_BG);
        let s = match &self.ed.search {
            Some(q) => alloc::format!("find: {}_", q),
            None => {
                let name = self.ed.path.as_deref().map_or("untitled", |p| p.rsplit('/').next().unwrap_or(p));
                alloc::format!("{}{} {}:{} {}", name, if self.ed.dirty { "*" } else { "" }, cl + 1, cc + 1, self.ed.msg)
            }
        };
        let s: Vec<u8> = s.bytes().map(|b| if (0x20..=0x7e).contains(&b) { b } else { b'?' }).take(cols).collect();
        self.text(x0, sy + 1, &s, if self.ed.search.is_some() { ECHO_INK } else { DIM_INK }, sr);
    }

    fn paint_console(&mut self) {
        let lh = self.lh();
        let r = R_CON;
        self.fill(r, BG);
        let cols = self.cols_in(r.w);
        self.con.cols = cols;
        // The prompt line at the bottom; the scrollback fills upward from it (MIDDEN.ELF's paint).
        let py = r.y + r.h - lh - 2;
        let prompt = alloc::format!("{}> ", self.con.cwd);
        let mut shown: Vec<u8> = prompt.as_bytes().to_vec();
        let start = shown.len();
        shown.extend_from_slice(&self.con.inp);
        let caret_at = start + self.con.caret;
        let skip = (caret_at + 1).saturating_sub(cols);
        let vis: Vec<u8> = shown[skip..].iter().copied().take(cols).collect();
        self.text(4, py, &vis, INK, r);
        let cx = self.cx(4, caret_at - skip);
        self.fill(Rect { x: cx, y: py, w: 2, h: lh - 1 }, if self.focus == Pane::Console { CARET } else { RULE });
        let fit = ((py - r.y - 2) / lh).max(0) as usize;
        let n = self.con.rows.len();
        let first = n.saturating_sub(fit);
        let rows: Vec<(console::Tone, String)> = self.con.rows.iter().skip(first).cloned().collect();
        let mut y = py - (rows.len() as i32) * lh;
        for (tone, s) in rows {
            let ink = match tone {
                console::Tone::Out => INK,
                console::Tone::Err => ERR_INK,
                console::Tone::Echo => ECHO_INK,
            };
            self.text(4, y, s.as_bytes(), ink, r);
            y += lh;
        }
    }

    fn paint(&mut self) {
        let any = self.dirty || self.tree.dirty || self.ed.repaint || self.con.dirty;
        if !any {
            return;
        }
        self.paint_tree();
        self.paint_editor();
        self.paint_console();
        // The rules between the panes, and the focused pane's accent bar along its top edge.
        self.fill(Rect { x: SIDE_W, y: 0, w: 1, h: CON_Y }, RULE);
        self.fill(Rect { x: 0, y: CON_Y, w: W, h: 1 }, RULE);
        let f = match self.focus {
            Pane::Tree => R_TREE,
            Pane::Editor => R_ED,
            Pane::Console => R_CON,
        };
        self.fill(Rect { x: f.x, y: f.y, w: f.w, h: 2 }, CARET);
        self.dirty = false;
        self.tree.dirty = false;
        self.ed.repaint = false;
        self.con.dirty = false;
        sys(SYS_WIN_PRESENT, self.win as u64, 0, 0, 0);
    }
}

#[cfg(target_arch = "x86_64")]
fn landmark_base() -> u64 {
    una_abi::USER_BASE_X86
}
#[cfg(not(target_arch = "x86_64"))]
fn landmark_base() -> u64 {
    match una_abi::args() {
        Some(a) if a.window_base() != 0 => a.window_base(),
        _ => {
            Line::new(b":: UNA: no args page (window base unknown) ::").wire();
            exit(1)
        }
    }
}

/// The workspace: argv[1] resolved from `/`, else the session home. `(root, file to open)`.
fn workspace() -> (String, Option<String>) {
    let arg = una_abi::args().and_then(|a| a.get(1)).and_then(|b| core::str::from_utf8(b).ok()).map(|s| fs::resolve("/", s));
    let Some(p) = arg else {
        return (fs::home(), None);
    };
    match fs::stat(&p) {
        Ok(s) if s.kind == 0 => {
            let dir = match p.rfind('/') {
                Some(0) | None => String::from("/"),
                Some(i) => String::from(&p[..i]),
            };
            (dir, Some(p))
        }
        _ => (p, None),
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    let base = landmark_base();
    let win = sys(SYS_WIN_CREATE, W as u64, H as u64, 0, 0);
    if win < 0 {
        let mut l = Line::new(b":: UNA: SYS_WIN_CREATE refused code=");
        l.dec(win).put(b" ::");
        l.wire();
        exit(1);
    }
    let (face, why) = match txt::load() {
        Ok(t) => (Some(t), None),
        Err(w) => (None, Some(w)),
    };
    let (root, file) = workspace();
    let cell = face.as_ref().map_or(8.0, |t| t.cell);
    let con_cols = (((W - 8) as f32) / cell) as usize;
    let mut app = App {
        surf: (base + 0x5000) as *mut u8,
        win,
        face,
        focus: Pane::Editor,
        tree: tree::Tree::new(root.clone()),
        ed: editor::Editor::new(),
        con: console::Console::new(root.clone(), con_cols),
        down: [false; 128],
        ptr: (W / 2, H / 2),
        buttons: 0,
        dirty: true,
    };
    let mut l = Line::new(b":: UNA: start window=");
    l.dec(win).put(b" font=");
    match (&app.face, &why) {
        (Some(t), _) => {
            l.put(t.name.as_bytes()).put(b" font_kib=").dec((t.bytes / 1024) as i64);
        }
        (None, Some(w)) => {
            let (word, code) = w.word();
            l.put(b"font8x8 font_why=").put(word);
            if let Some(c) = code {
                l.put(b" font_code=").dec(c);
            }
        }
        (None, None) => {
            l.put(b"font8x8");
        }
    }
    l.put(b" panes=").dec(PANES as i64).put(b" root=").put(root.as_bytes()).put(b" entries=").dec(app.tree.entries() as i64);
    l.put(b" cwd=").put(app.con.cwd.as_bytes()).put(b" ::");
    l.wire();
    app.con.say(console::Tone::Out, "Una on UnaOS - the console is Midden. Type help.");
    if let Some(f) = file {
        app.ed.open(&f);
    }
    app.paint();

    loop {
        let mut k = 0;
        while k < 32 {
            let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
            if ev < 0 {
                break;
            }
            let ev = ev as u64;
            let p = input_ev_payload(ev);
            match input_ev_type(ev) {
                INPUT_EV_KEY_DOWN => app.key_down(p as u8),
                INPUT_EV_KEY_UP => app.key_up(p as u8),
                INPUT_EV_ACTION => {
                    if app.focus == Pane::Editor {
                        app.ed.action(p as u8);
                    }
                }
                INPUT_EV_MOUSE_REL => {
                    let (dx, dy) = ((p >> 16) as u16 as i16 as i32, p as u16 as i16 as i32);
                    app.ptr = ((app.ptr.0 + dx).clamp(0, W - 1), (app.ptr.1 + dy).clamp(0, H - 1));
                }
                INPUT_EV_BUTTON => app.button(p as u8),
                INPUT_EV_CLOSE_REQ => app.quit(),
                _ => {}
            }
            k += 1;
        }
        app.paint();
        sleep_ms(16);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: UNA: panic ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — it opens a window, so a bare launch detaches.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_WINDOWED);
