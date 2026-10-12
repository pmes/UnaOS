#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// MIDDENTWIN (LEDGER SR75, R104): `APPS/MIDDEN.ELF` — Midden, the Shell (CODEX §2), as a ring-3 program: the
// Code layout's bottom pane (CODEX §4) on the glass. CHARTER: Midden — shared-core midden_core (the kernel's
// shell links the same core, `crates/kernel/src/shell.rs`, and so does the host Midden); the LUMENAPP shape.
//
// WHAT IT IS. One compositor window: a scrollback ring over a prompt line with a caret. Every typed line goes
// through `midden_core::plan(line, &facts, &mut vol)`:
//   * `facts` = `Facts::ring3(x86)` — what the ring-3 ABI can do. There is no spawn-by-path syscall
//     (`SYS_SPAWN` loads only the fixed staged program), so the process verbs and bare-name launch are OFF and
//     the core answers them with its own refusal; `help` is the core's text for those facts.
//   * `vol` = a `Volume` over `SYS_STAT` on the cwd-resolved path (a regular file = kind 0).
//   * the plan is executed through the syscalls it names: `Say` renders the core's text; `Host` verbs in
//     `midden_core::ring3::SERVED` are fulfilled here (`ls`/`dir` = SYS_PATH_READ + PATH_R_LIST, `cat`/`type` =
//     SYS_PATH_READ, `cd` = SYS_STAT kind 1, `stat` = SYS_STAT, `pwd`/`clear`/`history` are the window's own
//     state); every other verb prints `midden_core::ring3::refusal(verb)` — the core's text, never a stub.
//     Each refusal from the kernel names its errno and the reason (e.g. a listing outside the caller's home is
//     -EACCES: PATH_R_LIST is home-scoped; a kernel without `selfdiag` answers SYS_PATH_* with -ENOSYS).
//
// KEYS. Printable ASCII inserts at the caret; Backspace deletes; Left/Right move the caret; Up/Down walk the
// history; Enter runs the line; Ctrl+C clears the line (`^C`); `exit`/`quit` (the core's CLOSES) close the
// window and exit; a window close request does the same.
//
// WIRE. `:: MIDDEN: start window=<id> font=<face|font8x8 font_why=<w>> facts=ring3 cwd=<p> ::` once, then per
// line `:: MIDDEN: plan=<Nothing|Say|Host|Exec> verb=<v> served=<1|0> rows=<n> ::`, and on exit
// `:: MIDDEN: exit status=<s> rows=<n> cmds=<n> ::`. The exit status packs the same facts
// (`midden_core::ring3::status`) — the channel `tests midden` (kernel `middenapp.rs`) reads back.

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use midden_core::{plan, ring3, Facts, Message, Plan, Volume};
use una_abi::{
    input_ev_payload, input_ev_type, INPUT_EV_CLOSE_REQ, INPUT_EV_KEY_DOWN, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP, SYS_EXIT,
    SYS_INPUT_POLL, SYS_STAT, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};
use vein_ring3::files;
use vein_ring3::sys::{sleep_ms, sys, write};

mod txt;

/// LUMEN.ELF's size-class heap over SYS_SBRK: plans, scrollback lines and the face bytes are heap objects.
#[global_allocator]
static HEAP: vein_ring3::heap::Heap = vein_ring3::heap::Heap::sbrk();

const W: i32 = 288; // the x86 compositor's per-window maximum (FB_WIN_MAX_W/H)
const H: i32 = 288;
const STRIDE: usize = W as usize * 4;
/// The scrollback ring's byte budget: lines are dropped from the top once their text exceeds it.
const SCROLL_BYTES: usize = 8 * 1024;
const INP_CAP: usize = 240;
const HIST_CAP: usize = 32;
/// The most a `cat` reads (the window shows the tail; the ring keeps 8 KB of it).
const CAT_MAX: usize = 64 * 1024;

const BG: u32 = 0xFF12_141A;
const INK: u32 = 0xFFE6_E3EC;
const ERR_INK: u32 = 0xFFFF_8A80;
const ECHO_INK: u32 = 0xFF8F_B8FF;
const CARET: u32 = 0xFFB0_8CFF;
const RULE: u32 = 0xFF3A_3550;

static FONT: [[u8; 8]; 128] = font8x8::legacy::BASIC_LEGACY;

fn exit(code: i32) -> ! {
    sys(SYS_EXIT, code as u32 as u64, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

// ---- wire lines -----------------------------------------------------------------------------------------
struct Line(Vec<u8>);
impl Line {
    fn new(s: &[u8]) -> Line {
        Line(s.to_vec())
    }
    fn put(&mut self, s: &[u8]) -> &mut Self {
        self.0.extend_from_slice(s);
        self
    }
    fn dec(&mut self, v: i64) -> &mut Self {
        self.0.extend_from_slice(alloc::format!("{}", v).as_bytes());
        self
    }
    fn wire(&mut self) {
        self.0.push(b'\n');
        write(&self.0);
    }
}

// ---- the Volume: SYS_STAT on the cwd-resolved path --------------------------------------------------------
struct Stat {
    kind: u32,
    size: u64,
    id: u64,
    flags: u32,
}

fn stat(path: &str) -> Result<Stat, i64> {
    let mut st = [0u8; una_abi::USER_STAT_LEN];
    let r = sys(SYS_STAT, path.as_ptr() as u64, path.len() as u64, st.as_mut_ptr() as u64, 0);
    if r < 0 {
        return Err(r);
    }
    let u32_at = |o: usize| u32::from_le_bytes([st[o], st[o + 1], st[o + 2], st[o + 3]]);
    let u64_at = |o: usize| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&st[o..o + 8]);
        u64::from_le_bytes(b)
    };
    Ok(Stat { kind: u32_at(0), flags: u32_at(4), size: u64_at(8), id: u64_at(16) })
}

/// `p` resolved against `cwd`: absolute, `.`/`..`/empty components folded (`..` stops at `/`).
fn resolve(cwd: &str, p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let joined;
    let full = if p.starts_with('/') {
        p
    } else {
        joined = alloc::format!("{}/{}", cwd, p);
        &joined
    };
    for c in full.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = String::new();
    for c in parts {
        out.push('/');
        out.push_str(c);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

struct R3Vol<'a> {
    cwd: &'a str,
}
impl Volume for R3Vol<'_> {
    fn is_file(&mut self, name: &str) -> bool {
        matches!(stat(&resolve(self.cwd, name)), Ok(s) if s.kind == 0)
    }
}

/// An errno's name and the ring-3 reason it most often carries here.
fn errno_text(e: i64) -> &'static str {
    match e {
        una_abi::ENOSYS => "ENOSYS (this kernel offers ring 3 no SYS_PATH_* — built without `selfdiag`)",
        una_abi::EACCES => "EACCES (ring 3 lists and reads only what its principal may: PATH_R_LIST is home-scoped)",
        una_abi::ENOENT => "ENOENT (no such file or directory)",
        una_abi::EISDIR => "EISDIR (a directory: use ls)",
        una_abi::ENOTDIR => "ENOTDIR (not a directory)",
        una_abi::EINVAL => "EINVAL (the path is not one the surface accepts)",
        _ => "error",
    }
}

// ---- the app ----------------------------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Out,
    Err,
    Echo,
}

struct App {
    surf: *mut u8,
    win: i64,
    face: Option<txt::Txt>,
    rows: VecDeque<(Tone, String)>,
    bytes: usize,
    inp: Vec<u8>,
    caret: usize,
    hist: VecDeque<String>,
    hist_at: Option<usize>,
    cwd: String,
    facts: Facts,
    cmds: u32,
    last_kind: u32,
    last_verb: u8,
    dirty: bool,
}

impl App {
    fn lh(&self) -> i32 {
        if self.face.is_some() { txt::LH_TT } else { txt::LH_BITMAP }
    }
    fn cell(&self) -> f32 {
        self.face.as_ref().map_or(8.0, |t| t.cell)
    }
    fn cols(&self) -> usize {
        (((W - 8) as f32) / self.cell()) as usize
    }

    /// Push text into the scrollback: split on newlines, wrapped at the window's columns, made printable.
    fn say(&mut self, tone: Tone, text: &str) {
        let cols = self.cols().max(8);
        for l in text.split('\n') {
            let clean: String = l.chars().map(|c| if (' '..='~').contains(&c) { c } else if c == '\t' { ' ' } else { '?' }).collect();
            let mut rest: &str = &clean;
            loop {
                let take = rest.len().min(cols);
                self.push_row(tone, &rest[..take]);
                rest = &rest[take..];
                if rest.is_empty() {
                    break;
                }
            }
        }
        self.dirty = true;
    }
    fn push_row(&mut self, tone: Tone, s: &str) {
        self.bytes += s.len() + 1;
        self.rows.push_back((tone, String::from(s)));
        while self.bytes > SCROLL_BYTES {
            match self.rows.pop_front() {
                Some((_, r)) => self.bytes -= r.len() + 1,
                None => break,
            }
        }
    }
    fn msg(&mut self, m: &Message) {
        match m {
            Message::NoOp => {}
            Message::TerminalOutput(s) | Message::FileSystemEvent(s) => self.say(Tone::Out, s),
            Message::TerminalError(s) => self.say(Tone::Err, s),
        }
    }

    // ---- the plan, executed through the syscalls it names ----
    fn run_line(&mut self, line: &str) {
        let prompt = alloc::format!("{}> {}", self.cwd, line);
        self.say(Tone::Echo, &prompt);
        if !line.trim().is_empty() {
            if self.hist.back().map(|s| s.as_str()) != Some(line) {
                self.hist.push_back(String::from(line));
                if self.hist.len() > HIST_CAP {
                    self.hist.pop_front();
                }
            }
        }
        let facts = self.facts;
        let p = {
            let mut vol = R3Vol { cwd: &self.cwd };
            plan(line, &facts, &mut vol)
        };
        let word = midden_core::canon_verb(line.split_whitespace().next().unwrap_or(""));
        if let Plan::Say(_) = p {
            if ring3::CLOSES.contains(&word.as_str()) {
                self.close();
            }
        }
        let (kind, verb, served) = match &p {
            Plan::Nothing => (0, String::new(), true),
            Plan::Say(m) => {
                self.msg(m);
                if word == "help" {
                    self.say(Tone::Out, &alloc::format!("MIDDEN.ELF serves: {} (+ the core's own words above)", ring3::SERVED.join(" ")));
                }
                (1, word.clone(), true)
            }
            Plan::Host { verb, rest } => {
                let served = ring3::serves(verb);
                if served {
                    self.host(verb, rest);
                } else {
                    let r = ring3::refusal(verb);
                    self.msg(&r);
                }
                (2, verb.clone(), served)
            }
            // `Facts::ring3` has `exec == false`, so the core never plans a launch here; the refusal is the core's.
            Plan::Exec { typed, .. } => {
                let r = ring3::refusal(typed);
                self.msg(&r);
                (3, typed.clone(), false)
            }
        };
        if kind != 0 {
            self.cmds += 1;
            self.last_kind = kind;
            self.last_verb = ring3::verb_code(&verb);
        }
        let mut l = Line::new(b":: MIDDEN: plan=");
        l.put(ring3::kind_name(kind).as_bytes()).put(b" verb=").put(if verb.is_empty() { b"-" } else { verb.as_bytes() });
        l.put(b" served=").dec(served as i64).put(b" rows=").dec(self.rows.len() as i64).put(b" ::");
        l.wire();
    }

    fn host(&mut self, verb: &str, rest: &str) {
        let args: Vec<&str> = rest.split_whitespace().collect();
        let operand = args.iter().copied().find(|a| !a.starts_with('-'));
        match verb {
            "clear" => {
                self.rows.clear();
                self.bytes = 0;
                self.dirty = true;
            }
            "pwd" => {
                let c = self.cwd.clone();
                self.say(Tone::Out, &c);
            }
            "history" => {
                let lines: Vec<String> = self.hist.iter().enumerate().map(|(i, h)| alloc::format!("{:>3}  {}", i + 1, h)).collect();
                for l in lines {
                    self.say(Tone::Out, &l);
                }
            }
            "cd" => {
                let target = resolve(&self.cwd, operand.unwrap_or("/"));
                match stat(&target) {
                    Ok(s) if s.kind == 1 => self.cwd = target,
                    Ok(_) => self.say(Tone::Err, &alloc::format!("cd: {}: {}", target, errno_text(una_abi::ENOTDIR))),
                    Err(e) => self.say(Tone::Err, &alloc::format!("cd: {}: {} {}", target, e, errno_text(e))),
                }
            }
            "stat" => {
                let Some(o) = operand else {
                    self.say(Tone::Err, "usage: stat <path>");
                    return;
                };
                let target = resolve(&self.cwd, o);
                match stat(&target) {
                    Ok(s) => {
                        let line = alloc::format!(
                            "{}: {} size={} id={} backend={}",
                            target,
                            if s.kind == 1 { "dir" } else { "file" },
                            s.size,
                            s.id,
                            if s.flags & una_abi::STAT_HAS_ID != 0 { "unafs" } else { "fat" }
                        );
                        self.say(Tone::Out, &line);
                    }
                    Err(e) => self.say(Tone::Err, &alloc::format!("stat: {}: {} {}", target, e, errno_text(e))),
                }
            }
            "ls" | "dir" => {
                let target = resolve(&self.cwd, operand.unwrap_or("."));
                match list(&target) {
                    Ok(names) => {
                        if names.is_empty() {
                            self.say(Tone::Out, "(empty)");
                        }
                        for n in names {
                            self.say(Tone::Out, &n);
                        }
                    }
                    Err(e) => {
                        // A file operand lists as itself (what `ls FILE` does everywhere).
                        if let Ok(s) = stat(&target) {
                            if s.kind == 0 {
                                self.say(Tone::Out, &alloc::format!("{}  {}", target, s.size));
                                return;
                            }
                        }
                        self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, target, e, errno_text(e)));
                    }
                }
            }
            "cat" | "type" => {
                let Some(o) = operand else {
                    self.say(Tone::Err, &alloc::format!("usage: {} <path>", verb));
                    return;
                };
                let target = resolve(&self.cwd, o);
                match read_file(&target) {
                    Ok(b) => {
                        let text = String::from_utf8_lossy(&b).into_owned();
                        self.say(Tone::Out, text.trim_end_matches('\n'));
                    }
                    Err(e) => self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, target, e, errno_text(e))),
                }
            }
            // `ring3::SERVED` is the list above; a word added there without an arm is a visible refusal.
            other => {
                let r = ring3::refusal(other);
                self.msg(&r);
            }
        }
    }

    fn close(&mut self) -> ! {
        let st = ring3::status(self.rows.len() as u32, self.cmds, self.last_kind, self.last_verb, self.win >= 0, self.face.is_some());
        let mut l = Line::new(b":: MIDDEN: exit status=");
        l.dec(st as i64).put(b" rows=").dec(self.rows.len() as i64).put(b" cmds=").dec(self.cmds as i64).put(b" ::");
        l.wire();
        exit(st)
    }

    // ---- keys ----
    fn key(&mut self, k: u8) {
        match k {
            b'\n' | b'\r' => {
                let line = String::from_utf8_lossy(&self.inp).into_owned();
                self.inp.clear();
                self.caret = 0;
                self.hist_at = None;
                self.run_line(&line);
            }
            0x03 => {
                // Ctrl+C: the line is abandoned, as a terminal does.
                let line = alloc::format!("{}> {}^C", self.cwd, String::from_utf8_lossy(&self.inp));
                self.say(Tone::Echo, &line);
                self.inp.clear();
                self.caret = 0;
                self.hist_at = None;
            }
            0x08 | 0x7f => {
                if self.caret > 0 {
                    self.caret -= 1;
                    self.inp.remove(self.caret);
                }
            }
            KEY_LEFT => self.caret = self.caret.saturating_sub(1),
            KEY_RIGHT => self.caret = (self.caret + 1).min(self.inp.len()),
            KEY_UP | KEY_DOWN => {
                if self.hist.is_empty() {
                    return;
                }
                let n = self.hist.len();
                let at = match (self.hist_at, k == KEY_UP) {
                    (None, true) => Some(n - 1),
                    (None, false) => None,
                    (Some(i), true) => Some(i.saturating_sub(1)),
                    (Some(i), false) => if i + 1 < n { Some(i + 1) } else { None },
                };
                self.hist_at = at;
                self.inp = at.map_or(Vec::new(), |i| self.hist[i].as_bytes().to_vec());
                self.caret = self.inp.len();
            }
            0x20..=0x7e => {
                if self.inp.len() < INP_CAP {
                    self.inp.insert(self.caret, k);
                    self.caret += 1;
                }
            }
            _ => return,
        }
        self.dirty = true;
    }

    // ---- paint ----
    fn fill(&self, x0: i32, y0: i32, w: i32, h: i32, c: u32) {
        let (xs, ys) = (x0.max(0), y0.max(0));
        let (xe, ye) = ((x0 + w).min(W), (y0 + h).min(H));
        for y in ys..ye {
            let row = unsafe { self.surf.add(y as usize * STRIDE) as *mut u32 };
            for x in xs..xe {
                unsafe { row.add(x as usize).write_volatile(c) };
            }
        }
    }
    fn text(&mut self, x: i32, y: i32, s: &[u8], c: u32) {
        let surf = self.surf;
        if let Some(t) = self.face.as_mut() {
            t.draw(x as f32, y, s, c, &mut |gx, gy, cov| {
                if gx >= 0 && gy >= 0 && gx < W && gy < H {
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
                    if bits & (1 << col) != 0 {
                        self.fill(gx + col, y + 1 + r as i32, 1, 1, c);
                    }
                }
            }
        }
    }
    fn paint(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let lh = self.lh();
        self.fill(0, 0, W, H, BG);
        // The prompt line at the bottom, under a rule; the scrollback fills upward from it.
        let py = H - lh - 4;
        self.fill(0, py - 3, W, 1, RULE);
        let prompt = alloc::format!("{}> ", self.cwd);
        let cols = self.cols();
        let mut shown: Vec<u8> = prompt.as_bytes().to_vec();
        let start = shown.len();
        shown.extend_from_slice(&self.inp);
        // Keep the caret visible: show the tail of a line wider than the window.
        let caret_at = start + self.caret;
        let skip = (caret_at + 1).saturating_sub(cols);
        let vis: Vec<u8> = shown[skip..].iter().copied().take(cols).collect();
        self.text(4, py, &vis, INK);
        let cx = 4 + ((caret_at - skip) as f32 * self.cell()) as i32;
        self.fill(cx, py, 2, lh - 1, CARET);
        let fit = ((py - 6) / lh).max(0) as usize;
        let n = self.rows.len();
        let first = n.saturating_sub(fit);
        let rows: Vec<(Tone, String)> = self.rows.iter().skip(first).cloned().collect();
        let mut y = py - 6 - (rows.len() as i32) * lh;
        for (tone, s) in rows {
            let ink = match tone {
                Tone::Out => INK,
                Tone::Err => ERR_INK,
                Tone::Echo => ECHO_INK,
            };
            self.text(4, y, s.as_bytes(), ink);
            y += lh;
        }
        sys(SYS_WIN_PRESENT, self.win as u64, 0, 0, 0);
    }
}

// ---- the file surface: SYS_PATH_READ (whole file) and SYS_PATH_READ + PATH_R_LIST (a directory) ----------
fn path_req(path: &str, flags: u16, offset: u64) -> Option<Vec<u8>> {
    let p = path.as_bytes();
    if p.is_empty() || p.len() > una_abi::PATH_IO_PATH_MAX {
        return None;
    }
    let mut r = alloc::vec![0u8; una_abi::PATH_IO_HDR_LEN + p.len()];
    r[0..2].copy_from_slice(&(p.len() as u16).to_le_bytes());
    r[2..4].copy_from_slice(&flags.to_le_bytes());
    r[8..16].copy_from_slice(&offset.to_le_bytes());
    r[16..].copy_from_slice(p);
    Some(r)
}

fn list(path: &str) -> Result<Vec<String>, i64> {
    let mut all = Vec::new();
    let mut buf = alloc::vec![0u8; 4096];
    loop {
        let req = path_req(path, una_abi::PATH_R_LIST, all.len() as u64).ok_or(una_abi::EINVAL)?;
        let r = sys(una_abi::SYS_PATH_READ, req.as_ptr() as u64, req.len() as u64, buf.as_mut_ptr() as u64, buf.len() as u64);
        if r < 0 {
            return Err(r);
        }
        if r == 0 {
            break;
        }
        all.extend_from_slice(&buf[..r as usize]);
        if all.len() > CAT_MAX {
            break;
        }
    }
    let text = String::from_utf8_lossy(&all).into_owned();
    Ok(text.split('\n').filter(|l| !l.is_empty()).map(String::from).collect())
}

fn read_file(path: &str) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    let mut buf = alloc::vec![0u8; 4096];
    loop {
        let r = files::path_read(path.as_bytes(), out.len() as u64, &mut buf);
        if r < 0 {
            return Err(r);
        }
        if r == 0 {
            break;
        }
        out.extend_from_slice(&buf[..r as usize]);
        if out.len() >= CAT_MAX {
            break;
        }
    }
    Ok(out)
}

/// The start cwd: the session home (SYS_WHOAMI) when a user is logged in, else the root.
fn home() -> String {
    let mut rec = [0u8; una_abi::WHOAMI_MAX];
    let r = sys(una_abi::SYS_WHOAMI, rec.as_mut_ptr() as u64, rec.len() as u64, 0, 0);
    if r > 0 {
        if let Some(w) = una_abi::whoami_parse(&rec[..(r as usize).min(rec.len())]) {
            if let Ok(h) = core::str::from_utf8(w.home) {
                let h = h.trim_end_matches('/');
                if h.starts_with('/') {
                    return String::from(h);
                }
            }
        }
    }
    String::from("/")
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
            Line::new(b":: MIDDEN: no args page (window base unknown) ::").wire();
            exit(1)
        }
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    let base = landmark_base();
    let win = sys(SYS_WIN_CREATE, W as u64, H as u64, 0, 0);
    if win < 0 {
        let mut l = Line::new(b":: MIDDEN: SYS_WIN_CREATE refused code=");
        l.dec(win).put(b" ::");
        l.wire();
        exit(1);
    }
    let face = txt::load();
    let (face, why) = match face {
        Ok(t) => (Some(t), None),
        Err(w) => (None, Some(w)),
    };
    let mut app = App {
        surf: (base + 0x5000) as *mut u8,
        win,
        face,
        rows: VecDeque::new(),
        bytes: 0,
        inp: Vec::new(),
        caret: 0,
        hist: VecDeque::new(),
        hist_at: None,
        cwd: home(),
        facts: Facts::ring3(cfg!(target_arch = "x86_64")),
        cmds: 0,
        last_kind: 0,
        last_verb: 0,
        dirty: true,
    };
    let mut l = Line::new(b":: MIDDEN: start window=");
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
    l.put(b" facts=ring3 cwd=").put(app.cwd.as_bytes()).put(b" ::");
    l.wire();
    app.say(Tone::Out, "Midden on UnaOS - the shell core, in a window. Type help.");
    app.paint();

    loop {
        let mut k = 0;
        while k < 32 {
            let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
            if ev < 0 {
                break;
            }
            let ev = ev as u64;
            match input_ev_type(ev) {
                INPUT_EV_KEY_DOWN => app.key(input_ev_payload(ev) as u8),
                INPUT_EV_CLOSE_REQ => app.close(),
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
    write(b":: MIDDEN: panic ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — it opens a window, so a bare launch detaches.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_WINDOWED);
