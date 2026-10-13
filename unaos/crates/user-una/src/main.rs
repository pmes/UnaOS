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
// FOCUS. Ctrl+1 / Ctrl+2 / Ctrl+3 = tree / editor / console, and a click on a pane. UNAKEYS (rmbp-ledger B552): the
// ring carries one byte and no modifier field, so the modifier state rides the KEY_DOWN payload itself — since
// SMALLFIX9 (B533) the shared HID fold (xhci `hid_key_ascii`) presses Ctrl + a main-row digit as its own byte,
// `una_abi::KEY_CTRL_DIGIT | d` (Ctrl+1 = 0x81), a range no other key produces; that PRESS is the chord. Its RELEASE
// still arrives as the bare digit (ALLKEYS: a release cannot know the press's modifiers), so the press marks that
// digit's release as owed and `key_up` swallows it. The older reading stays as the fallback for a press the decoder
// still suppresses (Alt/Cmd + digit, a pre-SMALLFIX9 kernel): a KEY_UP of `1`/`2`/`3` with no KEY_DOWN of it before
// is a modified digit. Esc cycles the focus too (one key, no chord).
// A click focuses the pane under the pointer; the pointer is the one the kernel delivers (AETHERPTR, rmbp-ledger
// B543): every INPUT_EV_MOUSE_ABS carries the system pointer in this window's pixels, so nothing here estimates it.
// CURSORSHAPE (rmbp-ledger B550): each one also asks the compositor for the pointer's shape (`SYS_CURSOR_SHAPE`) —
// the text beam over the editor pane, the arrow over the tree, the console and outside the window.
//
// GLOBAL KEYS. Ctrl+S save the active tab's file; Ctrl+O open the tree's selected file; Ctrl+Q quit (and a window
// close request). Everything else goes to the focused pane (tree.rs, editor.rs, console.rs say their keys).
//
// TABS — UNAIDE2 (LEDGER SR111). The editor pane holds `tabs` (one `editor::Editor` — one edit_core Buffer — per
// document) under a tab strip along its top: each label is the file's leaf, `*` when dirty, the active one on the
// editor ground under the accent bar. Opening a file already open switches to its tab; a blank active tab is loaded
// in place; anything else opens a new tab (TAB_MAX). Ctrl+T next tab (Ctrl+Tab never reaches ring 3: the HID fold
// suppresses Ctrl with a non-letter other than a main-row digit, drivers/xhci/mod.rs:231-232), Ctrl+4..Ctrl+9 tab
// 1..6 (KEY_CTRL_DIGIT presses), a click on a label; Ctrl+W closes the active tab (asking once over unsaved edits);
// Ctrl+F / Ctrl+L / Ctrl+G open the editor's find / replace / line prompt from any pane (editor.rs THE PROMPT).
// Wire: `:: UNA: tab active=<i> tabs=<n> path=<p|untitled> dirty=<0|1> ::` on every switch, open and close, and
// `:: UNA: close path=<p> dirty=<0|1> tabs=<n> ::`. A press in the editor's text places the caret under the pointer.
//
// FILES AND RUN — UNAIDE2 (LEDGER SR111). The tree's operations (tree.rs FILE OPERATIONS: `n`/Ctrl+N new file, `f`
// new folder, `r` rename, Delete trash — RING3RENAME B561: a real rename and trash_core's Trash) ask their name in the
// editor's status line (`ask`; Enter commits, Esc drops it); a new file opens in a tab, a renamed file's tab follows
// it (a renamed folder's open files too). Ctrl+R (`run`) runs the first
// line of `.run` in the active file's folder, else the workspace root's (`$FILE` = the active file), else the line
// typed in the console, through the console's midden_core plan; `:: UNA: run src=<path|typed> line=<l> ::`.
// POINTER (B543/B550) in every pane: the hand over the tab strip, the beam over the editor's text and the console,
// the arrow over the tree and the status line; a press on the tree's selected row opens it, as Enter does.
//
// GIT — UNAIDE2 (SR111), read-only through git_core (git.rs): the tree's rows carry `M` / `?` at their right edge;
// `g` on a file, or the console's `git diff [path]`, shows its diff against the index in the console; `git status`
// lists the workspace's state. `:: UNA: git root=<p> gitdir=<p> hk=<k> entries=<n> M=<n> untracked=<n> ::` at start
// when `.git` exists (`err=<why>` when it cannot be read).
//
// THEME. Every colour is a field of `theme::Theme` (theme.rs: Principia's `system.appearance.*` and Tabula's
// `tabula.theme.<kind>` over the PREFS bus, the kernel palette's Light/Dark rows) — read once at start and again when
// the store moves (`theme::due`, PrefChanged's ring-3 stand-in); `:: UNA: theme mode=<light|dark> accent=<name>
// bus=<0|-errno> ::` at start and on every change.
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
    INPUT_EV_MOUSE_ABS, KEY_CTRL_DIGIT, KEY_ESC, SYS_EXIT, SYS_INPUT_POLL, SYS_WIN_CREATE, SYS_WIN_PRESENT, CURSOR_ARROW, CURSOR_HAND, CURSOR_TEXT,
    SYS_CURSOR_SHAPE,
};
use vein_ring3::sys::{now_ms, sleep_ms, sys, write};

mod console;
mod editor;
mod fs;
mod git;
mod theme;
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
/// UNAIDE2: the most tabs open at once (a further open is refused by name).
const TAB_MAX: usize = 12;

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

/// The exit status `tests una` reads back (`bg_poll`'s `Exited(i32)`; bit 31 stays clear). UNAIDE2 (SR111): SAVED is
/// the IDE's last Ctrl+S on any tab, HIGHLIGHT the largest span count over the open tabs; the layout is unchanged.
/// UNAMETAL2 (LEDGER SR98)
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

/// UNAIDE2: a tree operation's name prompt — the op, the folder (or, for a rename, the path) it acts on, the text.
struct Ask {
    op: tree::Op,
    base: String,
    text: String,
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
    /// UNAIDE2: the open documents and the active one (never empty).
    tabs: Vec<editor::Editor>,
    cur: usize,
    /// UNAIDE2: each visible tab label's rect, for a click (filled by the painter).
    tab_hits: Vec<(Rect, usize)>,
    /// UNAIDE2: the last Ctrl+S on any tab answered rc=0 (the exit status's SAVED bit).
    saved: bool,
    /// UNAIDE2: a tree operation waiting for its name (the status-line prompt).
    ask: Option<Ask>,
    con: console::Console,
    /// Bytes whose KEY_DOWN this program saw and whose KEY_UP it has not (the modified-digit chord test).
    down: [bool; 128],
    /// The delivered pointer (INPUT_EV_MOUSE_ABS, window px, unclamped); `None` until the first one.
    ptr: Option<(i32, i32)>,
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
    /// UNAIDE2: the tab strip's height (the top of the editor pane).
    fn tab_h(&self) -> i32 {
        self.lh() + 3
    }
    /// The editor's status line top and its visible text rows (below the tab strip).
    fn ed_geom(&self) -> (i32, usize) {
        let lh = self.lh();
        let sy = R_ED.y + R_ED.h - lh - 2;
        (sy, ((sy - self.tab_h() - 3) / lh).max(1) as usize)
    }
    /// The first text row's top.
    fn text_y0(&self) -> i32 {
        R_ED.y + self.tab_h() + 2
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

    /// The active tab.
    fn ed(&mut self) -> &mut editor::Editor {
        &mut self.tabs[self.cur]
    }
    /// A status-line message on the active tab.
    fn say(&mut self, m: String) {
        let e = self.ed();
        e.msg = m;
        e.repaint = true;
    }

    /// `:: UNA: tab active=<i> tabs=<n> path=<p|untitled> dirty=<0|1> ::` (1-based).
    fn tab_wire(&self) {
        let e = &self.tabs[self.cur];
        let mut l = Line::new(b":: UNA: tab active=");
        l.dec(self.cur as i64 + 1).put(b" tabs=").dec(self.tabs.len() as i64).put(b" path=");
        l.put(e.path.as_deref().unwrap_or("untitled").as_bytes()).put(b" dirty=").dec(e.dirty as i64).put(b" ::");
        l.wire();
    }
    fn switch_to(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.cur = i;
            self.tabs[i].repaint = true;
            self.dirty = true;
            self.tab_wire();
        }
    }

    /// UNAIDE2: open `p` — its tab if one has it, else the blank active tab, else a new tab.
    fn open_path(&mut self, p: &str) {
        if let Some(i) = self.tabs.iter().position(|e| e.path.as_deref() == Some(p)) {
            self.switch_to(i);
        } else if self.tabs[self.cur].blank() {
            let c = self.cur;
            self.tabs[c].open(p);
            self.dirty = true;
            self.tab_wire();
        } else if self.tabs.len() >= TAB_MAX {
            self.say(alloc::format!("{} tabs open: close one (Ctrl+W)", TAB_MAX));
        } else {
            let mut e = editor::Editor::new();
            e.map = self.tabs[self.cur].map.clone();
            if e.open(p) {
                self.tabs.push(e);
                self.cur = self.tabs.len() - 1;
                self.dirty = true;
                self.tab_wire();
            } else {
                let m = e.msg.clone();
                self.say(m);
            }
        }
        self.set_focus(Pane::Editor);
    }

    /// UNAIDE2: Ctrl+W — close the active tab, asking once over unsaved edits; the last one leaves a blank tab.
    fn close_tab(&mut self) {
        let e = &mut self.tabs[self.cur];
        if e.dirty && !e.close_armed {
            e.close_armed = true;
            e.msg = String::from("unsaved: Ctrl+S, or Ctrl+W again discards");
            e.repaint = true;
            return;
        }
        let mut l = Line::new(b":: UNA: close path=");
        l.put(e.path.as_deref().unwrap_or("untitled").as_bytes()).put(b" dirty=").dec(e.dirty as i64);
        let map = e.map.clone();
        self.tabs.remove(self.cur);
        if self.tabs.is_empty() {
            let mut b = editor::Editor::new();
            b.map = map;
            self.tabs.push(b);
        }
        l.put(b" tabs=").dec(self.tabs.len() as i64).put(b" ::");
        l.wire();
        let n = self.cur.min(self.tabs.len() - 1);
        self.switch_to(n);
    }

    /// UNAIDE2: start a tree operation's name prompt.
    fn begin_ask(&mut self, op: tree::Op) {
        let (base, text) = match op {
            tree::Op::Rename => match self.tree.selected() {
                Some((p, _)) => {
                    let leaf = String::from(p.rsplit('/').next().unwrap_or(&p));
                    (p, leaf)
                }
                None => return self.say(String::from("rename: select a file or folder in the tree")),
            },
            _ => (self.tree.target_dir(), String::new()),
        };
        self.ask = Some(Ask { op, base, text });
        self.dirty = true;
    }

    /// UNAIDE2: Enter on the name prompt — the op through tree.rs, its outcome in the status line.
    fn commit_ask(&mut self) {
        let Some(a) = self.ask.take() else { return };
        self.dirty = true;
        let name = String::from(a.text.trim());
        let m = match a.op {
            tree::Op::NewFile => match self.tree.new_file(&a.base, &name) {
                Ok(p) => {
                    self.open_path(&p);
                    alloc::format!("created {}", name)
                }
                Err(e) => alloc::format!("new file: {} {}", fs::errno_word(e), e),
            },
            tree::Op::NewDir => match self.tree.new_dir(&a.base, &name) {
                Ok(_) => alloc::format!("folder {} made", name),
                Err(e) => alloc::format!("new folder: {} {}", fs::errno_word(e), e),
            },
            tree::Op::Rename => match self.tree.rename(&a.base, &name) {
                Ok(to) => {
                    // RING3RENAME (B561): every tab on the renamed path, or under it when it was a folder, follows it.
                    let under = alloc::format!("{}/", a.base);
                    for e in self.tabs.iter_mut() {
                        let moved = match e.path.as_deref() {
                            Some(p) if p == a.base => Some(to.clone()),
                            Some(p) if p.starts_with(&under) => Some(alloc::format!("{}/{}", to, &p[under.len()..])),
                            _ => None,
                        };
                        if let Some(np) = moved {
                            e.path = Some(np);
                            e.repaint = true;
                        }
                    }
                    alloc::format!("renamed to {}", name)
                }
                Err(e) => {
                    self.tree.rebuild(None);
                    alloc::format!("rename: {} {}", fs::errno_word(e), e)
                }
            },
        };
        self.say(m);
    }

    /// The name prompt's keys; `false` = not one it takes.
    fn ask_key(&mut self, k: u8) -> bool {
        let Some(a) = self.ask.as_mut() else { return false };
        match k {
            b'\n' | b'\r' => self.commit_ask(),
            KEY_ESC => self.ask = None,
            0x08 | 0x7f => {
                a.text.pop();
            }
            0x20..=0x7e => {
                if a.text.len() < 255 {
                    a.text.push(k as char)
                }
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    /// UNAIDE2: Ctrl+R — the `.run` line (active file's folder, then the workspace root), else the typed line.
    fn run(&mut self) {
        self.set_focus(Pane::Console);
        let file = self.tabs[self.cur].path.clone();
        let root = fs::resolve("/", &self.tree.root);
        let mut dirs: Vec<String> = Vec::new();
        if let Some(f) = &file {
            dirs.push(match f.rfind('/') {
                Some(0) | None => String::from("/"),
                Some(i) => String::from(&f[..i]),
            });
        }
        if !dirs.contains(&root) {
            dirs.push(root);
        }
        let mut found: Option<(String, String)> = None;
        for d in &dirs {
            let p = if d == "/" { String::from("/.run") } else { alloc::format!("{}/.run", d) };
            if let Ok(b) = fs::read_file(&p, 4096, false) {
                let text = String::from_utf8_lossy(&b).into_owned();
                if let Some(l) = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')) {
                    found = Some((p, l.replace("$FILE", file.as_deref().unwrap_or(""))));
                    break;
                }
            }
        }
        let (src, line) = match found {
            Some(f) => f,
            None => match self.con.take_input() {
                Some(l) => (String::from("typed"), l),
                None => {
                    let looked: Vec<String> = dirs.iter().map(|d| alloc::format!("{}/.run", d.trim_end_matches('/'))).collect();
                    let m = alloc::format!("Ctrl+R: no .run line in {} - type a command here, then Ctrl+R", looked.join(" or "));
                    self.con.say(console::Tone::Err, &m);
                    return;
                }
            },
        };
        let mut w = Line::new(b":: UNA: run src=");
        w.put(src.as_bytes()).put(b" line=").put(line.as_bytes()).put(b" ::");
        w.wire();
        if let console::Act::Quit = self.con.run(&line) {
            self.quit();
        }
    }

    fn open_selected(&mut self) {
        match self.tree.selected_file().map(String::from) {
            Some(p) => self.open_path(&p),
            None => self.say(String::from("select a file in the tree (Ctrl+1)")),
        }
    }

    fn quit(&mut self) -> ! {
        let rows = self.con.rows.len() as u32;
        let spans = self.tabs.iter().map(|e| e.highlight.as_ref().map_or(0, |h| h.spans())).max().unwrap_or(0) as u32;
        let st = status(rows, self.con.cmds, self.con.last_kind, self.con.last_verb, PANES, self.win >= 0, self.face.is_some(), self.saved, spans);
        let mut l = Line::new(b":: UNA: exit status=");
        l.dec(st as i64).put(b" panes=").dec(PANES as i64).put(b" lines=").dec(rows as i64);
        l.put(b" saved=").dec(self.saved as i64).put(b" spans=").dec(spans as i64).put(b" tabs=").dec(self.tabs.len() as i64);
        l.put(b" cmds=").dec(self.con.cmds as i64).put(b" focus=").put(self.focus.name()).put(b" ::");
        l.wire();
        exit(st)
    }

    // ---- input ----
    fn key_down(&mut self, k: u8) {
        // UNAKEYS (B552): Ctrl + main-row digit, read from the press's own byte (see FOCUS above).
        if (KEY_CTRL_DIGIT..=KEY_CTRL_DIGIT + 9).contains(&k) {
            let d = k - KEY_CTRL_DIGIT;
            self.down[(b'0' + d) as usize] = true; // its release arrives as the bare digit: owed, swallowed by key_up
            match d {
                1 => self.set_focus(Pane::Tree),
                2 => self.set_focus(Pane::Editor),
                3 => self.set_focus(Pane::Console),
                4..=9 => self.switch_to((d - 4) as usize), // UNAIDE2: Ctrl+4..Ctrl+9 = tab 1..6
                _ => {}
            }
            return;
        }
        if (k as usize) < self.down.len() {
            self.down[k as usize] = true;
        }
        // UNAIDE2: a tree operation's name prompt, then the editor's open prompt, take their own keys first.
        if self.ask.is_some() && self.ask_key(k) {
            return;
        }
        if self.focus == Pane::Editor && self.tabs[self.cur].wants(k) {
            return self.ed().key(k);
        }
        match k {
            0x11 => self.quit(), // Ctrl+Q
            0x13 => {
                // Ctrl+S
                self.ed().save();
                self.saved = self.tabs[self.cur].saved && !self.tabs[self.cur].dirty;
                if let Some(p) = self.tabs[self.cur].path.clone() {
                    self.tree.remark(&p); // UNAIDE2: the saved file's git mark
                }
                self.dirty = true;
                return;
            }
            0x0F => return self.open_selected(), // Ctrl+O
            0x14 => {
                // UNAIDE2: Ctrl+T — the next tab, wrapping
                let n = (self.cur + 1) % self.tabs.len();
                return self.switch_to(n);
            }
            0x17 => return self.close_tab(), // UNAIDE2: Ctrl+W
            0x0E => return self.begin_ask(tree::Op::NewFile), // UNAIDE2: Ctrl+N — a new file, from any pane
            0x12 => return self.run(),                        // UNAIDE2: Ctrl+R — the Run verb
            0x06 | 0x0C | 0x07 => {
                // UNAIDE2: Ctrl+F / Ctrl+L / Ctrl+G — the editor's find / replace / line prompt, from any pane
                self.set_focus(Pane::Editor);
                return self.ed().key(k);
            }
            KEY_ESC => {
                if !(self.focus == Pane::Editor && self.ed().escape()) {
                    self.cycle_focus();
                }
                return;
            }
            _ => {}
        }
        match self.focus {
            Pane::Tree => match self.tree.key(k) {
                tree::Act::Open(p) => self.open_path(&p),
                tree::Act::Ask(op) => self.begin_ask(op),
                tree::Act::Trash => {
                    // RING3RENAME (B561): trash_core's rule over the ring-3 rename door.
                    let m = match self.tree.trash() {
                        Ok((p, to)) => alloc::format!("trashed {} -> {}", p.rsplit('/').next().unwrap_or(&p), to),
                        Err((p, why)) => alloc::format!("trash {}: {}", p.rsplit('/').next().unwrap_or(&p), why),
                    };
                    self.say(m);
                }
                tree::Act::Diff(p) => {
                    self.set_focus(Pane::Console);
                    self.con.git_diff(&p);
                }
                tree::Act::None => {}
            },
            Pane::Editor => self.ed().key(k),
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
        // A release whose press was suppressed: Alt/Cmd + digit, or Ctrl + digit on a pre-SMALLFIX9 kernel (FOCUS).
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
        let Some((x, y)) = self.ptr else { return };
        if R_TREE.has(x, y) {
            self.set_focus(Pane::Tree);
            let r = (y - R_TREE.y - 2) / self.lh() - 1;
            if r >= 0 {
                // UNAIDE2: a press on the row already selected opens it (Enter's act).
                let i = self.tree.top + r as usize;
                let again = i == self.tree.sel && i < self.tree.nodes.len();
                self.tree.select_row(r as usize);
                if again {
                    if let tree::Act::Open(p) = self.tree.key(b'\n') {
                        self.open_path(&p);
                    }
                }
            }
        } else if R_ED.has(x, y) {
            self.set_focus(Pane::Editor);
            if y < R_ED.y + self.tab_h() {
                // UNAIDE2: a press on a tab label switches to it.
                if let Some(&(_, i)) = self.tab_hits.iter().find(|(r, _)| r.has(x, y)) {
                    self.switch_to(i);
                }
            } else {
                // UNAIDE2: a press in the text places the caret under the delivered pointer (B543).
                let (sy, _) = self.ed_geom();
                let y0 = self.text_y0();
                if y >= y0 && y < sy {
                    let row = ((y - y0) / self.lh()) as usize;
                    let col = (((x - R_ED.x - 4).max(0) as f32) / self.cell() + 0.5) as usize;
                    let e = &mut self.tabs[self.cur];
                    let (l, c) = (e.top + row, e.left + col);
                    e.click(l, c);
                }
            }
        } else if R_CON.has(x, y) {
            self.set_focus(Pane::Console);
        }
    }

    /// UNAIDE2: the pointer shape over (x, y): the hand over the tab strip, the beam over the editor's text and the
    /// console, the arrow over the tree, the editor's status line and outside the window.
    fn shape_at(&self, x: i32, y: i32) -> u64 {
        if R_ED.has(x, y) {
            let (sy, _) = self.ed_geom();
            if y < R_ED.y + self.tab_h() {
                CURSOR_HAND
            } else if y < sy {
                CURSOR_TEXT
            } else {
                CURSOR_ARROW
            }
        } else if R_CON.has(x, y) {
            CURSOR_TEXT
        } else {
            CURSOR_ARROW
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
        let th = theme::cur();
        let lh = self.lh();
        let r = R_TREE;
        self.fill(r, th.side_bg);
        let cols = self.cols_in(r.w);
        let title = self.tree.root.rsplit('/').find(|s| !s.is_empty()).unwrap_or("/");
        let title: Vec<u8> = alloc::format!("{}/", title).bytes().map(|b| if (0x20..=0x7e).contains(&b) { b } else { b'?' }).take(cols).collect();
        self.text(4, r.y + 2, &title, th.dim_ink, r);
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
                self.fill(Rect { x: r.x, y, w: r.w, h: lh }, if focused { th.sel_bg } else { th.row_sel });
            }
            let (mark, ink) = {
                let nd = &self.tree.nodes[i];
                if nd.err {
                    (b' ', th.err_ink)
                } else if nd.dir {
                    (if nd.open { b'-' } else { b'+' }, th.dir_ink)
                } else {
                    (b' ', th.ink)
                }
            };
            let ind = self.tree.indent(i);
            let gm = self.tree.nodes[i].mark;
            let mut s: Vec<u8> = Vec::new();
            s.extend(core::iter::repeat(b' ').take(ind));
            s.push(mark);
            s.extend(self.tree.nodes[i].name.bytes().map(|b| if (0x20..=0x7e).contains(&b) { b } else { b'?' }));
            s.truncate(if gm == b' ' { cols } else { cols.saturating_sub(1) });
            self.text(2, y, &s, ink, r);
            if gm != b' ' {
                // UNAIDE2: the git mark at the row's right edge — modified in the accent ink, untracked dim.
                let x = r.x + r.w - self.cell() as i32 - 3;
                self.text(x, y, &[gm], if gm == b'M' { th.caret } else { th.dim_ink }, r);
            }
        }
        if n == 0 {
            self.text(4, r.y + 2 + lh, b"(empty)", th.dim_ink, r);
        }
    }

    /// UNAIDE2: the tab strip — each label the file's leaf (`*` when dirty), the active one on the editor ground under
    /// the accent bar; when the labels overflow, the first one shown slides right until the active one fits.
    fn paint_tabs(&mut self) {
        let th = theme::cur();
        let r = Rect { x: R_ED.x, y: R_ED.y, w: R_ED.w, h: self.tab_h() };
        self.fill(r, th.status_bg);
        let cell = self.cell();
        let labels: Vec<Vec<u8>> = self
            .tabs
            .iter()
            .map(|e| {
                let mut s: Vec<u8> = e.name().bytes().map(|b| if (0x20..=0x7e).contains(&b) { b } else { b'?' }).take(14).collect();
                if e.dirty {
                    s.push(b'*');
                }
                s
            })
            .collect();
        let width = |s: &Vec<u8>| ((s.len() + 2) as f32 * cell) as i32 + 1;
        let mut first = 0;
        loop {
            let used: i32 = labels[first..=self.cur].iter().map(width).sum();
            if used <= r.w || first == self.cur {
                break;
            }
            first += 1;
        }
        self.tab_hits.clear();
        let mut x = r.x;
        for (i, s) in labels.iter().enumerate().skip(first) {
            if x >= r.x + r.w {
                break;
            }
            let w = width(s);
            let tr = Rect { x, y: r.y, w: (w - 1).min(r.x + r.w - x), h: r.h };
            let active = i == self.cur;
            if active {
                self.fill(tr, th.ed_bg);
                self.fill(Rect { x, y: r.y, w: tr.w, h: 2 }, th.caret);
            }
            self.text(x + cell as i32, r.y + 2, s, if active { th.ink } else { th.dim_ink }, tr);
            self.fill(Rect { x: x + w - 1, y: r.y + 3, w: 1, h: r.h - 6 }, th.rule);
            self.tab_hits.push((tr, i));
            x += w;
        }
        self.fill(Rect { x: r.x, y: r.y + r.h - 1, w: r.w, h: 1 }, th.rule);
    }

    fn paint_editor(&mut self) {
        let th = theme::cur();
        let lh = self.lh();
        let r = R_ED;
        self.fill(r, th.ed_bg);
        self.paint_tabs();
        let (sy, rows) = self.ed_geom();
        let cols = self.cols_in(r.w);
        let cur = self.cur;
        self.tabs[cur].view(rows, cols);
        let x0 = r.x + 4;
        let y0 = self.text_y0();
        let text_clip = Rect { x: r.x, y: y0, w: r.w, h: sy - y0 };
        let sels: Vec<(usize, usize)> = self.tabs[cur].buf.selections().iter().filter(|s| !s.is_empty()).map(|s| (s.start(), s.end())).collect();
        let nlines = self.tabs[cur].buf.line_count();
        let (left, top) = (self.tabs[cur].left, self.tabs[cur].top);
        for row in 0..rows {
            let l = top + row;
            if l >= nlines {
                break;
            }
            let y = y0 + row as i32 * lh;
            let line = self.tabs[cur].buf.line(l);
            let ls = self.tabs[cur].buf.pos_to_offset(edit_core::Pos::new(l, 0));
            let idx: Vec<(usize, char)> = line.char_indices().skip(left).take(cols).collect();
            // The selection behind the text.
            for &(a, b) in &sels {
                for (c, &(bi, ch)) in idx.iter().enumerate() {
                    let off = ls + bi;
                    if off >= a && off + ch.len_utf8() <= b {
                        let xa = self.cx(x0, c);
                        let xb = self.cx(x0, c + 1);
                        self.fill(Rect { x: xa, y, w: xb - xa, h: lh }, th.sel_bg);
                    }
                }
            }
            let glyphs: Vec<u8> = idx.iter().map(|&(_, ch)| if (' '..='~').contains(&ch) { ch as u8 } else if ch == '\t' { b' ' } else { b'?' }).collect();
            // UNAMETAL2 (SR98): each char's ink is its syntax_core token's appearance token; runs of one ink draw once.
            // UNAKEYS (B552): kind -> token through Principia's map, token -> ink through the live look.
            let map = &self.tabs[cur].map;
            let inks: Option<Vec<u32>> = self.tabs[cur].highlight.as_ref().map(|h| idx.iter().map(|&(bi, _)| th.token_ink(map.token(h.kind_at(l, bi)))).collect());
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
                None => self.text(x0, y, &glyphs, th.ink, text_clip),
            }
        }
        // The caret (full on focus, dim otherwise).
        let (cl, cc) = self.tabs[cur].caret_lc();
        if cl >= top && cl < top + rows && cc >= left {
            let x = self.cx(x0, cc - left);
            let y = y0 + (cl - top) as i32 * lh;
            self.fill(Rect { x, y, w: 2, h: lh - 1 }, if self.focus == Pane::Editor { th.caret } else { th.rule });
        }
        // The status line: the find prompt, or path / dirty / line:col / message.
        let sr = Rect { x: r.x, y: sy, w: r.w, h: r.y + r.h - sy };
        self.fill(sr, th.status_bg);
        let e = &self.tabs[cur];
        let prompt = match &self.ask {
            // UNAIDE2: a tree operation's name prompt has the line first.
            Some(a) => Some(match a.op {
                tree::Op::Rename => alloc::format!("rename {} to: {}_", a.base.rsplit('/').next().unwrap_or(&a.base), a.text),
                tree::Op::NewFile => alloc::format!("new file in {}: {}_", a.base, a.text),
                tree::Op::NewDir => alloc::format!("new folder in {}: {}_", a.base, a.text),
            }),
            None => e.prompt_text(),
        };
        let asking = prompt.is_some();
        let s = match prompt {
            Some(q) => q,
            None => alloc::format!("{}{} {}:{} {}", e.name(), if e.dirty { "*" } else { "" }, cl + 1, cc + 1, e.msg),
        };
        let s: Vec<u8> = s.bytes().map(|b| if (0x20..=0x7e).contains(&b) { b } else { b'?' }).take(cols).collect();
        self.text(x0, sy + 1, &s, if asking { th.echo_ink } else { th.dim_ink }, sr);
    }

    fn paint_console(&mut self) {
        let th = theme::cur();
        let lh = self.lh();
        let r = R_CON;
        self.fill(r, th.con_bg);
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
        self.text(4, py, &vis, th.ink, r);
        let cx = self.cx(4, caret_at - skip);
        self.fill(Rect { x: cx, y: py, w: 2, h: lh - 1 }, if self.focus == Pane::Console { th.caret } else { th.rule });
        let fit = ((py - r.y - 2) / lh).max(0) as usize;
        let n = self.con.rows.len();
        let first = n.saturating_sub(fit);
        let rows: Vec<(console::Tone, String)> = self.con.rows.iter().skip(first).cloned().collect();
        let mut y = py - (rows.len() as i32) * lh;
        for (tone, s) in rows {
            let ink = match tone {
                console::Tone::Out => th.ink,
                console::Tone::Err => th.err_ink,
                console::Tone::Echo => th.echo_ink,
            };
            self.text(4, y, s.as_bytes(), ink, r);
            y += lh;
        }
    }

    fn paint(&mut self) {
        let any = self.dirty || self.tree.dirty || self.tabs[self.cur].repaint || self.con.dirty;
        if !any {
            return;
        }
        self.paint_tree();
        self.paint_editor();
        self.paint_console();
        // The rules between the panes, and the focused pane's accent bar along its top edge.
        let th = theme::cur();
        self.fill(Rect { x: SIDE_W, y: 0, w: 1, h: CON_Y }, th.rule);
        self.fill(Rect { x: 0, y: CON_Y, w: W, h: 1 }, th.rule);
        let f = match self.focus {
            Pane::Tree => R_TREE,
            Pane::Editor => R_ED,
            Pane::Console => R_CON,
        };
        self.fill(Rect { x: f.x, y: f.y, w: f.w, h: 2 }, th.caret);
        self.dirty = false;
        self.tree.dirty = false;
        self.tabs[self.cur].repaint = false;
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
    // UNAKEYS (B552): the look and Tabula's theme map from Principia, before the first paint.
    theme::load();
    let map = theme::syntax_map();
    theme_wire();
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
        tabs: alloc::vec![editor::Editor::new()],
        cur: 0,
        tab_hits: Vec::new(),
        saved: false,
        ask: None,
        con: console::Console::new(root.clone(), con_cols),
        down: [false; 128],
        ptr: None,
        buttons: 0,
        dirty: true,
    };
    app.tabs[0].map = map;
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
    // UNAIDE2: the workspace's repository, when `.git` is there.
    match (&app.tree.git, &app.tree.git_err) {
        (Some(g), _) => {
            let (m, u) = app.tree.mark_counts();
            let mut w = Line::new(b":: UNA: git root=");
            w.put(g.root.as_bytes()).put(b" gitdir=").put(g.gitdir.as_bytes()).put(b" hk=").put(g.hk_name().as_bytes());
            w.put(b" entries=").dec(g.entries() as i64).put(b" M=").dec(m as i64).put(b" untracked=").dec(u as i64).put(b" ::");
            w.wire();
        }
        (None, Some(e)) => {
            let mut w = Line::new(b":: UNA: git root=");
            w.put(app.tree.root.as_bytes()).put(b" err=").put(e.as_bytes()).put(b" ::");
            w.wire();
        }
        _ => {}
    }
    app.con.say(console::Tone::Out, "Una on UnaOS - the console is Midden. Type help.");
    if let Some(f) = file {
        app.open_path(&f);
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
                        app.ed().action(p as u8);
                    }
                }
                INPUT_EV_MOUSE_ABS => {
                    let (x, y) = una_abi::input_ev_xy(p);
                    app.ptr = Some((x, y));
                    // CURSORSHAPE (B550): asked on every pointer event, uncached (a focus change resets the shape
                    // behind this program's back); the answer is ignored — a kernel without the verb leaves the
                    // arrow. UNAIDE2: every pane says its shape (`App::shape_at`).
                    let shape = app.shape_at(x, y);
                    let _ = sys(SYS_CURSOR_SHAPE, shape, 0, 0, 0);
                }
                INPUT_EV_BUTTON => app.button(p as u8),
                INPUT_EV_CLOSE_REQ => app.quit(),
                _ => {}
            }
            k += 1;
        }
        // UNAKEYS (B552): the store moved (PrefChanged's ring-3 stand-in, theme.rs) -> every pane repaints.
        if theme::due(now_ms()) {
            let moved = theme::load();
            let map = theme::syntax_map();
            if moved || map != app.tabs[0].map {
                for e in app.tabs.iter_mut() {
                    e.map = map.clone();
                }
                app.dirty = true;
                theme_wire();
            }
        }
        app.paint();
        sleep_ms(16);
    }
}

/// `:: UNA: theme mode=<light|dark> accent=<name> bus=<0|-errno> ::` — the look in force.
fn theme_wire() {
    let l = theme::cur().look;
    let mut w = Line::new(b":: UNA: theme mode=");
    w.put(l.mode_name().as_bytes()).put(b" accent=").put(l.accent_name().as_bytes()).put(b" bus=").dec(theme::bus_err()).put(b" ::");
    w.wire();
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
