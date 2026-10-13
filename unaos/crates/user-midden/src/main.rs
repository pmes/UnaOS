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
//   * `facts` = `Facts::ring3(x86)` with `exec` ON where the kernel carries `SYS_SPAWN_PATH` (RING3EXEC,
//     rmbp-ledger B521: x86) — a bare name the core's `resolve_exec` finds is launched by path, and `run <path>`
//     is served here through the same syscall (the kernel applies the shell's own checks under this program's
//     principal and answers a pid or an errno). `bg`/`jobs`/`kill`/`storm` stay off (`proc_verbs`): ring 3
//     holds no job table; a launched program is detached, the kernel's `bg` body.
//   * `vol` = a `Volume` over `SYS_STAT` on the cwd-resolved path (a regular file = kind 0).
//   * the plan is executed through the syscalls it names: `Say` renders the core's text; `Host` verbs in
//     `midden_core::ring3::SERVED` are fulfilled here (`ls`/`dir` = SYS_PATH_READ + PATH_R_LIST, `cat`/`type` =
//     SYS_PATH_READ, `cd` = SYS_STAT kind 1, `stat` = SYS_STAT, `pwd`/`clear`/`history` are the window's own
//     state); every other verb prints `midden_core::ring3::refusal(verb)` — the core's text, never a stub.
//     Each refusal from the kernel names its errno and the reason (RING3EXEC: a listing is governed by the
//     volume's ownership ACL — a directory this program's principal may not read is -EACCES by the ACL's word;
//     SYS_PATH_READ is on the default x86 kernel, SYS_PATH_WRITE only under `selfdiag`).
//
// KEYS. Printable ASCII inserts at the caret; Backspace deletes; Left/Right move the caret; Up/Down walk the
// history; Enter runs the line; Ctrl+C clears the line (`^C`); `exit`/`quit` (the core's CLOSES) close the
// window and exit; a window close request does the same.
//
// THEME. UNAKEYS (rmbp-ledger B552): every colour is a field of `theme::Theme` (theme.rs: Principia's
// `system.appearance.*` over the PREFS bus, the kernel palette's Light/Dark rows), read once at start and again when
// the store moves (`theme::due`, PrefChanged's ring-3 stand-in); `:: MIDDEN: theme mode=<light|dark> accent=<name>
// bus=<0|-errno> ::` at start and on every change.
//
// WIRE. `:: MIDDEN: start window=<id> font=<face|font8x8 font_why=<w>> facts=ring3 cwd=<p> ::` once, then per
// line `:: MIDDEN: plan=<Nothing|Say|Host> verb=<v> served=<1|0> rows=<n> ::` — a launch (`run <path>` or a bare
// name) is `:: MIDDEN: plan=Exec verb=<run|typed> served=1 pid=<n> ::`, or `served=0 err=<errno>` — and on exit
// `:: MIDDEN: exit status=<s> rows=<n> cmds=<n> ::`. The exit status packs the same facts
// (`midden_core::ring3::status`) — the channel `tests midden` (kernel `middenapp.rs`) reads back.
//
// RING3PIPE (rmbp-ledger B562). `run` and a bare-name launch ask SYS_SPAWN_PATH for a pipe (`una_abi::SPAWN_PIPE`): the
// program still runs detached, and what it writes to its console comes back through `SYS_PIPE_READ(pid)` — `pump`,
// once per frame, drains every open pipe into the scrollback as the bytes arrive (one row per line) and closes it at
// EOF (the child exited, the ring is empty). A kernel that does not know the flag (E2BIG to the high argc bits) gets
// the plain detached launch. Wire: `:: MIDDEN: pipe pid=<p> bytes=<n> lines=<l> ::` per chunk, `:: MIDDEN: pipe eof
// ok=<0|1> pid=<p> lines=<n> bytes=<b> dropped=<d> rows=<r> ::` at EOF (ok = a line reached the scrollback, none dropped).
// ONE-SHOT: `MIDDEN.ELF -c <words…>` opens no window — it plans the words as one line, writes every Out/Err row to its
// console (SYS_WRITE handle 1, so a launcher's pipe carries it), says `:: MIDDEN: oneshot rows=<n> cmds=<n> status=<s>
// ::` and exits with the packed status: the shell's `sh -c`, and the program `tests midden` / `tests una` run through
// a pipe.

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
use vein_ring3::sys::{now_ms, sleep_ms, sys, write};

mod theme;
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
/// RING3PIPE: one SYS_PIPE_READ's buffer, and the reads one pipe gets per frame.
const PIPE_CHUNK: usize = 4096;
const PIPE_READS: usize = 4;
/// RING3PIPE: a line longer than this with no newline is pushed as it stands (the window wraps it).
const PIPE_LINE_MAX: usize = 1024;

/// RING3PIPE: a launched program whose output this shell is reading.
struct Piped {
    pid: i64,
    part: Vec<u8>,
    lines: u32,
    bytes: u64,
    dropped: u64,
}

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
        una_abi::ENOSYS => "ENOSYS (this kernel does not carry that syscall for ring 3)",
        una_abi::EACCES => "EACCES (refused by the ownership ACL for this program's principal, or APPTRUST: answer the Open question first)",
        una_abi::E2BIG => "E2BIG (the image exceeds the user window, or the words exceed the args page)",
        una_abi::ENOEXEC => "ENOEXEC (not an ELF64 program for this machine)",
        una_abi::EAGAIN => "EAGAIN (the loader refused: process table or memory's limit)",
        una_abi::EIO => "EIO (short read)",
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
    /// RING3PIPE: the launched programs whose output is still arriving.
    pipes: Vec<Piped>,
    /// ONE-SHOT (`-c`): no window; every Out/Err line is written to the console as it is said.
    oneshot: bool,
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
            if self.oneshot && tone != Tone::Echo {
                let mut o = Vec::with_capacity(l.len() + 1);
                o.extend_from_slice(l.as_bytes());
                o.push(b'\n');
                write(&o);
            }
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
        let word = midden_core::canon_verb(line.split_whitespace().next().unwrap_or(""));
        if word == "run" && facts.exec {
            // RING3EXEC (B521): `run <path> [args]` — not a verb on `Facts::ring3` (no job table), served here: the
            // core's resolver finds the program, SYS_SPAWN_PATH launches it with argv = the words after `run`.
            let words: Vec<&str> = line.split_whitespace().skip(1).collect();
            let r = match words.first() {
                None => {
                    self.say(Tone::Err, "usage: run <path> [args]");
                    Err(una_abi::EINVAL)
                }
                Some(&prog) => {
                    let found = midden_core::resolve_exec(prog, &mut R3Vol { cwd: &self.cwd });
                    self.launch("run", prog, found.as_deref(), &words)
                }
            };
            self.exec_done("run", r);
            return;
        }
        let p = {
            let mut vol = R3Vol { cwd: &self.cwd };
            plan(line, &facts, &mut vol)
        };
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
                    if facts.exec {
                        self.say(Tone::Out, "MIDDEN.ELF launches: run <path> [args], and bare names (detached, as this program's user; their output arrives here)");
                    }
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
            // RING3EXEC (B521): a bare name the core resolved — launched by path, argv[0] = the word typed.
            Plan::Exec { typed, name, args } => {
                let mut argv: Vec<&str> = alloc::vec![typed.as_str()];
                argv.extend(args.iter().map(|a| a.as_str()));
                let r = self.launch(typed, typed, Some(name.as_str()), &argv);
                self.exec_done(typed, r);
                return;
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

    /// RING3EXEC (B521): launch `found` (the core's resolution of `typed`, cwd-relative) with `argv` through
    /// SYS_SPAWN_PATH. The pid, or the errno the kernel answered (its refusal line is on the wire beside ours).
    fn launch(&mut self, verb: &str, typed: &str, found: Option<&str>, argv: &[&str]) -> Result<i64, i64> {
        let Some(name) = found else {
            self.say(Tone::Err, &alloc::format!("{}: {}: {}", verb, typed, errno_text(una_abi::ENOENT)));
            return Err(una_abi::ENOENT);
        };
        let path = resolve(&self.cwd, name);
        // RING3PIPE: ask for a pipe; a kernel without the flag answers E2BIG to the high argc bits — launch plainly.
        let mut piped = !self.oneshot;
        let mut r = if piped { spawn_piped(&path, argv) } else { spawn_path(&path, argv) };
        if piped && r == una_abi::E2BIG {
            piped = false;
            r = spawn_path(&path, argv);
        }
        if r > 0 {
            self.say(Tone::Out, &alloc::format!("{}: {} started - pid {}", verb, path, r));
            if piped {
                self.pipes.push(Piped { pid: r, part: Vec::new(), lines: 0, bytes: 0, dropped: 0 });
            }
            Ok(r)
        } else {
            self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, path, r, errno_text(r)));
            Err(r)
        }
    }

    /// RING3PIPE: drain every open pipe into the scrollback (called once per frame).
    fn pump(&mut self) {
        if self.pipes.is_empty() {
            return;
        }
        let mut buf = alloc::vec![0u8; PIPE_CHUNK];
        let mut i = 0;
        while i < self.pipes.len() {
            let pid = self.pipes[i].pid;
            let mut closed: Option<i64> = None;
            for _ in 0..PIPE_READS {
                let r = sys(una_abi::SYS_PIPE_READ, pid as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0);
                if r > 0 {
                    let n = r as usize;
                    let lines = self.pipe_bytes(i, &buf[..n]);
                    let mut l = Line::new(b":: MIDDEN: pipe pid=");
                    l.dec(pid).put(b" bytes=").dec(n as i64).put(b" lines=").dec(lines as i64).put(b" ::");
                    l.wire();
                    if n < buf.len() {
                        let d = sys(una_abi::SYS_PIPE_READ, pid as u64, 0, 0, 0);
                        if d >= 0 {
                            self.pipes[i].dropped = d as u64;
                        }
                        break;
                    }
                } else if r == una_abi::EAGAIN {
                    break;
                } else {
                    closed = Some(r);
                    break;
                }
            }
            match closed {
                None => i += 1,
                Some(r) => {
                    let mut p = self.pipes.remove(i);
                    if !p.part.is_empty() {
                        let tail = String::from_utf8_lossy(&p.part).into_owned();
                        self.say(Tone::Out, &tail);
                        p.lines += 1;
                        p.part.clear();
                    }
                    if r == 0 {
                        let m = alloc::format!("run: pid {} exited - {} lines{}", pid, p.lines, if p.dropped > 0 { alloc::format!(", {} bytes dropped", p.dropped) } else { String::new() });
                        self.say(Tone::Echo, &m);
                    } else {
                        let m = alloc::format!("run: pid {}: output lost - {} {}", pid, r, errno_text(r));
                        self.say(Tone::Err, &m);
                    }
                    let ok = r == 0 && p.lines > 0 && p.dropped == 0;
                    let mut l = Line::new(b":: MIDDEN: pipe eof ok=");
                    l.dec(ok as i64).put(b" pid=").dec(pid).put(b" lines=").dec(p.lines as i64).put(b" bytes=").dec(p.bytes as i64);
                    l.put(b" dropped=").dec(p.dropped as i64).put(b" rows=").dec(self.rows.len() as i64);
                    if r != 0 {
                        l.put(b" err=").dec(r);
                    }
                    l.put(b" ::");
                    l.wire();
                }
            }
        }
    }

    /// RING3PIPE: `b` from pipe `i` — complete lines into the scrollback, the rest held for the next chunk.
    fn pipe_bytes(&mut self, i: usize, b: &[u8]) -> u32 {
        self.pipes[i].bytes += b.len() as u64;
        self.pipes[i].part.extend_from_slice(b);
        let mut n = 0;
        loop {
            let part = &self.pipes[i].part;
            let cut = match part.iter().position(|&c| c == b'\n') {
                Some(k) => k,
                None if part.len() >= PIPE_LINE_MAX => part.len(),
                None => break,
            };
            let line = String::from_utf8_lossy(&part[..cut]).into_owned();
            let drop_to = (cut + 1).min(part.len());
            self.pipes[i].part.drain(..drop_to);
            self.say(Tone::Out, line.trim_end_matches('\r'));
            n += 1;
        }
        self.pipes[i].lines += n;
        n
    }

    /// RING3EXEC (B521): account a launch line and put it on the wire —
    /// `:: MIDDEN: plan=Exec verb=<v> served=1 pid=<n> ::` or `served=0 err=<errno>`.
    fn exec_done(&mut self, verb: &str, r: Result<i64, i64>) {
        self.cmds += 1;
        self.last_kind = 3;
        self.last_verb = ring3::verb_code(verb);
        let mut l = Line::new(b":: MIDDEN: plan=Exec verb=");
        l.put(verb.as_bytes());
        match r {
            Ok(pid) => l.put(b" served=1 pid=").dec(pid),
            Err(e) => l.put(b" served=0 err=").dec(e),
        };
        l.put(b" ::");
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
        if !self.dirty || self.win < 0 {
            return;
        }
        self.dirty = false;
        let th = theme::cur();
        let lh = self.lh();
        self.fill(0, 0, W, H, th.bg);
        // The prompt line at the bottom, under a rule; the scrollback fills upward from it.
        let py = H - lh - 4;
        self.fill(0, py - 3, W, 1, th.rule);
        let prompt = alloc::format!("{}> ", self.cwd);
        let cols = self.cols();
        let mut shown: Vec<u8> = prompt.as_bytes().to_vec();
        let start = shown.len();
        shown.extend_from_slice(&self.inp);
        // Keep the caret visible: show the tail of a line wider than the window.
        let caret_at = start + self.caret;
        let skip = (caret_at + 1).saturating_sub(cols);
        let vis: Vec<u8> = shown[skip..].iter().copied().take(cols).collect();
        self.text(4, py, &vis, th.ink);
        let cx = 4 + ((caret_at - skip) as f32 * self.cell()) as i32;
        self.fill(cx, py, 2, lh - 1, th.caret);
        let fit = ((py - 6) / lh).max(0) as usize;
        let n = self.rows.len();
        let first = n.saturating_sub(fit);
        let rows: Vec<(Tone, String)> = self.rows.iter().skip(first).cloned().collect();
        let mut y = py - 6 - (rows.len() as i32) * lh;
        for (tone, s) in rows {
            let ink = match tone {
                Tone::Out => th.ink,
                Tone::Err => th.err_ink,
                Tone::Echo => th.echo_ink,
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

/// RING3EXEC (B521): `SYS_SPAWN_PATH(path, len, argv, argc)` — the argv descriptors point at the words in place.
fn spawn_path(path: &str, argv: &[&str]) -> i64 {
    let descs: Vec<una_abi::SpawnArg> = argv.iter().map(|a| una_abi::SpawnArg { ptr: a.as_ptr() as u64, len: a.len() as u64 }).collect();
    sys(una_abi::SYS_SPAWN_PATH, path.as_ptr() as u64, path.len() as u64, descs.as_ptr() as u64, descs.len() as u64)
}

/// RING3PIPE (B562): `SYS_SPAWN_PATH` with `SPAWN_PIPE` on argc's high half — the pid names the child's output pipe.
fn spawn_piped(path: &str, argv: &[&str]) -> i64 {
    let descs: Vec<una_abi::SpawnArg> = argv.iter().map(|a| una_abi::SpawnArg { ptr: a.as_ptr() as u64, len: a.len() as u64 }).collect();
    sys(una_abi::SYS_SPAWN_PATH, path.as_ptr() as u64, path.len() as u64, descs.as_ptr() as u64, descs.len() as u64 | una_abi::SPAWN_PIPE)
}

/// ONE-SHOT: `-c <words…>` — the words after `-c`, joined by one space; `None` without `-c`.
fn oneshot_line() -> Option<String> {
    let a = una_abi::args()?;
    if a.get(1)? != b"-c" {
        return None;
    }
    let mut line = String::new();
    for i in 2..a.argc() {
        if let Some(w) = a.get(i).and_then(|b| core::str::from_utf8(b).ok()) {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(w);
        }
    }
    Some(line)
}

/// ONE-SHOT: plan `line` with no window, its rows written to the console, then exit with the packed status.
fn oneshot(line: &str) -> ! {
    let mut app = App {
        surf: core::ptr::null_mut(),
        win: -1,
        face: None,
        rows: VecDeque::new(),
        bytes: 0,
        inp: Vec::new(),
        caret: 0,
        hist: VecDeque::new(),
        hist_at: None,
        cwd: home(),
        facts: Facts { exec: cfg!(target_arch = "x86_64"), ..Facts::ring3(cfg!(target_arch = "x86_64")) },
        cmds: 0,
        last_kind: 0,
        last_verb: 0,
        dirty: false,
        pipes: Vec::new(),
        oneshot: true,
    };
    app.run_line(line);
    let st = ring3::status(app.rows.len() as u32, app.cmds, app.last_kind, app.last_verb, false, false);
    let mut l = Line::new(b":: MIDDEN: oneshot rows=");
    l.dec(app.rows.len() as i64).put(b" cmds=").dec(app.cmds as i64).put(b" status=").dec(st as i64).put(b" ::");
    l.wire();
    exit(st)
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
    if let Some(line) = oneshot_line() {
        oneshot(&line); // ONE-SHOT (RING3PIPE, B562): `-c <words…>` — no window
    }
    let base = landmark_base();
    let win = sys(SYS_WIN_CREATE, W as u64, H as u64, 0, 0);
    if win < 0 {
        let mut l = Line::new(b":: MIDDEN: SYS_WIN_CREATE refused code=");
        l.dec(win).put(b" ::");
        l.wire();
        exit(1);
    }
    theme::load(); // UNAKEYS (B552): the look from Principia, before the first paint
    theme_wire();
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
        facts: Facts { exec: cfg!(target_arch = "x86_64"), ..Facts::ring3(cfg!(target_arch = "x86_64")) }, // RING3EXEC (B521): SYS_SPAWN_PATH is x86's
        cmds: 0,
        last_kind: 0,
        last_verb: 0,
        dirty: true,
        pipes: Vec::new(),
        oneshot: false,
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
        if theme::due(now_ms()) && theme::load() {
            app.dirty = true; // UNAKEYS (B552): the store moved — repaint in the new look
            theme_wire();
        }
        app.pump(); // RING3PIPE (B562): what the launched programs wrote since the last frame
        app.paint();
        sleep_ms(16);
    }
}

/// `:: MIDDEN: theme mode=<light|dark> accent=<name> bus=<0|-errno> ::` — the look in force.
fn theme_wire() {
    let l = theme::cur().look;
    let mut w = Line::new(b":: MIDDEN: theme mode=");
    w.put(l.mode_name().as_bytes()).put(b" accent=").put(l.accent_name().as_bytes()).put(b" bus=").dec(theme::bus_err()).put(b" ::");
    w.wire();
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
