#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// MIDDENTWIN (LEDGER SR75, R104): `APPS/MIDDEN.ELF` — Midden, the Shell (CODEX §2), as a ring-3 program: the
// Code layout's bottom pane (CODEX §4) on the glass. CHARTER: Midden — shared-core midden_core (the kernel's
// shell links the same core, `crates/kernel/src/shell.rs`, and so does the host Midden); the LUMENAPP shape.
//
// WHAT IT IS. One compositor window: a scrollback ring over a prompt line with a caret. Every simple command goes
// through `midden_core::plan(stage, &facts, &mut vol)`:
//   * `facts` = `Facts::ring3(x86)` with `exec` ON where the kernel carries `SYS_SPAWN_PATH` (RING3EXEC,
//     rmbp-ledger B521: x86) — a bare name the core's `resolve_exec` finds (in the cwd, then under /apps) is
//     launched by path, and `run <path>` is served here through the same syscall (the kernel applies the shell's
//     own checks under this program's principal and answers a pid or an errno).
//   * `vol` = a `Volume` over `SYS_STAT` on the cwd-resolved path (a regular file = kind 0), then `/apps/<name>`.
//   * the plan is executed through the syscalls it names: `Say` renders the core's text; `Host` verbs in
//     `midden_core::ring3::SERVED` and `shell::FILTERS` are fulfilled here (`ls`/`dir` = SYS_PATH_READ +
//     PATH_R_LIST, `cat`/`type` = SYS_PATH_READ, `cd` = SYS_STAT kind 1, `stat` = SYS_STAT, `grep`/`wc`/`head`/
//     `tail` = `shell::filter` over a pipeline's lines or the file's, `pwd`/`clear`/`history` are the window's own
//     state); every other verb prints `midden_core::ring3::refusal(verb)` — the core's text, never a stub.
//
// MIDDEN2 (LEDGER SR120) — THE SHELL A PERSON USES. Over the core's `shell` module (midden_core lib.rs tail):
//   * LINES. `shell::split` cuts a line at `;` `&&` `||` and `|`; `shell::should_run` decides each next command.
//     A pipeline's stages run in order, each stage's Out rows captured as the next stage's input (Err rows still
//     reach the window, as stderr does). A PROGRAM may be any stage: it is launched with SPAWN_PIPE, its output
//     collected until EOF, then the rest of the pipeline and of the line runs. SHELLABI (rmbp-ledger B612): a program
//     in a LATER stage is handed the previous stage's lines through `SYS_PIPE_WRITE(pid)` (una-abi 71: each byte a
//     KEY_DOWN on the child's input ring, then one KEY_EOT) — `feed`, once per frame beside `pump`, resends what a
//     full ring did not take. A one-shot `MIDDEN.ELF -c <filter>` with no file operand reads that input (`stdin`).
//   * HISTORY. Up/Down walk it (the line being typed comes back past the newest); Ctrl+R searches backward
//     (`shell::search_back`; Ctrl+R again = older, Enter runs, Esc/Ctrl+G cancels, any motion key accepts);
//     `history [n] | -c`. Persisted to `<home>/.midden_history` through SYS_PATH_WRITE (appended per line; the
//     tail re-read at start; rewritten when it outgrows HIST_FILE_MAX) — only lines `shell::keep_in_history`
//     keeps (no leading-space line, no secret-shaped line: R65). No session home = no file, said at start.
//   * COMPLETION. Tab (byte 0x09) and Ctrl+Tab (una_abi::KEY_CTRL_TAB — on the glass a bare Tab is the window
//     system's focus key, x86 `wc_focus_key`, so Ctrl+Tab is the key that reaches this program there): a word in
//     command position completes over the core's verbs on these facts + the builtins + the aliases + the program
//     names under /apps; any other word over the listing of its directory (PATH_R_LIST). One candidate completes
//     (a directory keeps its `/`), several extend to their common prefix or are listed.
//   * LINE EDITING through edit_core's Buffer (Tabula's core, SR73): Ctrl+A/E line start/end (and Home/End, the
//     keymap's CursorLineStart/End actions), Ctrl+B/F word left/right (Ctrl+arrows are the window system's nudge
//     chords and Alt is suppressed by the HID fold, so the word moves ride B/F), Ctrl+K/U kill to end/start,
//     Ctrl+W kill the word before the caret, Ctrl+Y yank the last kill, Ctrl+D/Delete delete forward, Ctrl+Z undo,
//     Ctrl+L clear, Copy/Paste (the keymap's actions) through SYS_CLIP_SET/GET.
//   * ALIASES and the RC. `alias [name[=value]]`, `unalias name | -a`; `source <file>`; `<home>/.middenrc` is
//     sourced once at start. The rc is a SCRIPT the operator writes (aliases, a `cd`), not a preference: every
//     setting the system owns is Principia's (its store over the PREFS bus — theme.rs reads the look there).
//   * CWD. One per window (this program owns one window): `cd [dir | - | ~]`, `pwd`; every path operand resolves
//     against it, `~` against the session home.
//   * JOBS. `jobs` lists the launched programs whose pipe is open. SHELLABI (B612): `kill <pid | %n>` kills one of
//     them through `SYS_KILL` (una-abi 72 — the kernel's gate is parenthood: only this program's own SPAWN_PIPE
//     children), and Ctrl+C on an empty line kills the newest job the same way and cancels the rest of its line.
//   * SCROLLBACK. The wheel (INPUT_EV_WHEEL, 3 rows a detent), PageUp/PageDown — the key bytes una-abi KEY_PGUP /
//     KEY_PGDN (SHELLABI: the HID table's 0x4B/0x4E rows), which Shift+PgUp/PgDn also press, so the keymap's
//     ScrollPageUp/Down actions that ride beside them are NOT bound here (one page per press) — Ctrl+Home/End
//     (ScrollTop/Bottom). Typing returns to the live bottom.
//
// THEME. UNAKEYS (rmbp-ledger B552): every colour is a field of `theme::Theme` (theme.rs: Principia's
// `system.appearance.*` over the PREFS bus, the kernel palette's Light/Dark rows), read once at start and again when
// the store moves (`theme::due`, PrefChanged's ring-3 stand-in); `:: MIDDEN: theme mode=<light|dark> accent=<name>
// bus=<0|-errno> ::` at start and on every change.
//
// WIRE. `:: MIDDEN: history file=<p|none> loaded=<n> err=<e> ::` and `:: MIDDEN: rc path=<p|none> lines=<n> err=<e>
// ::` before `:: MIDDEN: start window=<id> font=<face|font8x8 font_why=<w>> facts=ring3 cwd=<p> ::`, then per simple
// command `:: MIDDEN: plan=<Nothing|Say|Host> verb=<v> served=<1|0> rows=<n> ::` — a launch (`run <path>` or a bare
// name) is `:: MIDDEN: plan=Exec verb=<run|typed> served=1 pid=<n> ::`, or `served=0 err=<errno>`; a builtin is
// `:: MIDDEN: builtin=<w> ok=<0|1> rows=<n> ::`; a pipeline `:: MIDDEN: pipeline stages=<n> last=<verb> rows=<n>
// ok=<0|1> ::`; a compound line `:: MIDDEN: line cmds=<n> ok=<0|1> ::`; `:: MIDDEN: cd ok=<0|1> cwd=<p> ::`;
// `:: MIDDEN: complete ok=<0|1> word=<w> got=<w|-> cands=<n> ::`; `:: MIDDEN: recall ok=<0|1> via=<up|down|search>
// line=<l|-> …::`; and on exit `:: MIDDEN: exit status=<s> rows=<n> cmds=<n> ::`. The exit status packs the same
// facts (`midden_core::ring3::status`) — the channel `tests midden` (kernel `middenapp.rs`) reads back.
//
// RING3PIPE (rmbp-ledger B609). `run` and a bare-name launch ask SYS_SPAWN_PATH for a pipe (`una_abi::SPAWN_PIPE`): the
// program still runs detached, and what it writes to its console comes back through `SYS_PIPE_READ(pid)` — `pump`,
// once per frame, drains every open pipe into the scrollback as the bytes arrive (one row per line) and closes it at
// EOF (the child exited, the ring is empty). A kernel that does not know the flag (E2BIG to the high argc bits) gets
// the plain detached launch. Wire: `:: MIDDEN: pipe pid=<p> bytes=<n> lines=<l> ::` per chunk, `:: MIDDEN: pipe eof
// ok=<0|1> pid=<p> lines=<n> bytes=<b> dropped=<d> rows=<r> ::` at EOF (ok = a line reached the scrollback, none dropped).
// ONE-SHOT: `MIDDEN.ELF -c <words…>` opens no window — it plans the words as one line, writes every Out/Err row to its
// console (SYS_WRITE handle 1, so a launcher's pipe carries it), says `:: MIDDEN: oneshot rows=<n> cmds=<n> status=<s>
// ::` and exits with the packed status: the shell's `sh -c`, and the program `tests midden` / `tests una` run through
// a pipe. A one-shot reads no rc and writes no history.
// SHELLABI (rmbp-ledger B612). `:: MIDDEN: pipe feed pid=<p> bytes=<n> ok=<0|1> [err=<e>] ::` when a later stage's
// input is delivered (its KEY_EOT taken) or refused; `:: MIDDEN: kill ok=<0|1> via=<builtin|ctrl-c> pid=<p> [err=<e>]
// [cancelled=<0|1>] ::` (ok first: `tests midden` watches a fixed prefix); a one-shot reading its input says `:: MIDDEN:
// stdin bytes=<n> eot=<0|1> ::`. A pipeline whose LAST stage is a program says its `pipeline …` line at that
// program's EOF, `last=` the program as typed. The one-shot's own `:: MIDDEN: ` lines ride its pipe too; a pipeline
// stage's collection drops them (they are this protocol's wire, not the program's data).

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use edit_core::{Buffer, Direction, Motion, Unit};
use fsio::{list, read_file, read_from, resolve, stat, write_at};
use midden_core::shell::{self, Cmd, Join};
use midden_core::{plan, ring3, Facts, Message, Plan, Volume};
use una_abi::{
    input_ev_payload, input_ev_type, INPUT_EV_ACTION, INPUT_EV_CLOSE_REQ, INPUT_EV_KEY_DOWN, INPUT_EV_WHEEL, KEY_CTRL_TAB,
    KEY_DOWN, KEY_EOT, KEY_ESC, KEY_LEFT, KEY_PGDN, KEY_PGUP, KEY_RIGHT, KEY_UP, SYS_EXIT, SYS_INPUT_POLL, SYS_WIN_CREATE,
    SYS_WIN_PRESENT,
};
use vein_ring3::sys::{now_ms, sleep_ms, sys, write};

mod fsio;
mod theme;
mod txt;

/// LUMEN.ELF's size-class heap over SYS_SBRK: plans, scrollback lines and the face bytes are heap objects.
#[global_allocator]
static HEAP: vein_ring3::heap::Heap = vein_ring3::heap::Heap::sbrk();

const W: i32 = 288; // the x86 compositor's per-window maximum (FB_WIN_MAX_W/H)
const H: i32 = 288;
const STRIDE: usize = W as usize * 4;
/// The scrollback ring's byte budget: lines are dropped from the top once their text exceeds it.
const SCROLL_BYTES: usize = 64 * 1024;
const INP_CAP: usize = 240;
/// MIDDEN2: the history kept in memory, the tail of the file read at start, and the size past which the file is
/// rewritten to its kept tail.
const HIST_CAP: usize = 500;
const HIST_READ: usize = 48 * 1024;
const HIST_FILE_MAX: u64 = 96 * 1024;
const HIST_NAME: &str = ".midden_history";
const RC_NAME: &str = ".middenrc";
/// MIDDEN2: how deep `source` may nest (an rc that sources itself stops here, said by name).
const SOURCE_DEPTH: u32 = 8;
/// MIDDEN2: rows one wheel detent scrolls.
const WHEEL_ROWS: usize = 3;
/// RING3PIPE: one SYS_PIPE_READ's buffer, and the reads one pipe gets per frame.
const PIPE_CHUNK: usize = 4096;
const PIPE_READS: usize = 4;
/// RING3PIPE: a line longer than this with no newline is pushed as it stands (the window wraps it).
const PIPE_LINE_MAX: usize = 1024;
/// MIDDEN2: the keymap's actions this shell acts on (una-abi INPUT_EV_ACTION, `video::clipboard::action_code`).
const ACT_COPY: u64 = 3;
const ACT_PASTE: u64 = 5;
const ACT_LINE_START: u64 = 15;
const ACT_LINE_END: u64 = 16;
const ACT_PAGE_UP: u64 = 22;
const ACT_PAGE_DOWN: u64 = 23;
/// SHELLABI: the SYS_PIPE_WRITE calls one pipe's feed gets per frame, and how long a one-shot waits for a byte of its
/// input before it takes the silence as the end.
const FEED_WRITES: usize = 8;
const STDIN_IDLE_MS: u64 = 10_000;
/// SHELLABI: the prefix of this program's own wire lines (a one-shot child's ride its pipe; not pipeline data).
const WIRE_PREFIX: &str = ":: MIDDEN: ";
const ACT_TOP: u64 = 24;
const ACT_BOTTOM: u64 = 25; const ACT_UNDO: u64 = 51; // RING3UNDO (rmbp-ledger B771): ⌘Z, the keymap's Undo row (LINE-NEUTRAL append)

/// MIDDEN2: what is left of a line when its pipeline's first stage is a program: the later stages (run over the
/// program's collected output at EOF), the commands after it, and the operator that joins them.
struct Cont {
    rest: Vec<String>,
    stages: usize,
    q: VecDeque<Cmd>,
    join: Join,
    got: Vec<String>,
    /// SHELLABI: the program as typed — the pipeline line's `last=` when it is the last stage.
    last: String,
}

/// RING3PIPE: a launched program whose output this shell is reading.
struct Piped {
    pid: i64,
    cmd: String,
    part: Vec<u8>,
    lines: u32,
    bytes: u64,
    dropped: u64,
    /// MIDDEN2: Some = a pipeline's stage — its lines are collected, not shown.
    cont: Option<Cont>,
    /// SHELLABI: the input still owed to the child (the previous stage's lines), and the KEY_EOT after it.
    feed: Vec<u8>,
    feed_eof: bool,
    fed: u64,
}

/// MIDDEN2: Ctrl+R's state — the query, the hit, and the line that was being typed (Esc puts it back).
struct Search {
    q: String,
    at: Option<usize>,
    draft: String,
}

/// MIDDEN2: how a pipeline ended — done with a status, or parked on a program's pipe (the rest runs at its EOF).
enum Step {
    Done(bool),
    Parked,
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

// ---- the Volume: SYS_STAT on the cwd-resolved path, then /apps ---------------------------------------------
struct R3Vol<'a> {
    cwd: &'a str,
}
impl Volume for R3Vol<'_> {
    fn is_file(&mut self, name: &str) -> bool {
        fsio::is_file(&resolve(self.cwd, name)) || (!name.contains('/') && fsio::is_file(&apps_path(name)))
    }
}

/// MIDDEN2: `/apps/<name>` — where a bare program name is found when the cwd does not hold it.
fn apps_path(name: &str) -> String {
    alloc::format!("/apps/{}", name)
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
        una_abi::ESRCH => "ESRCH (no such child: not a program this shell launched, or it already exited)",
        una_abi::EPIPE => "EPIPE (the program exited before it took its input)",
        _ => "error",
    }
}

/// SHELLABI: `%n` (the n-th job, as `jobs` numbers them) or a pid.
fn kill_target(op: &str, pipes: &[Piped]) -> Option<i64> {
    match op.strip_prefix('%') {
        Some(n) => n.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|k| pipes.get(k)).map(|p| p.pid),
        None => op.parse::<i64>().ok().filter(|&p| p > 0),
    }
}

/// SHELLABI: the program a stage launches, as typed — the word after `run`, else the bare name.
fn program_word(stage: &str) -> String {
    let mut w = stage.split_whitespace();
    let first = w.next().unwrap_or("");
    let name = if midden_core::canon_verb(first) == "run" { w.next().unwrap_or(first) } else { first };
    String::from(name)
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
    /// MIDDEN2: the prompt line, an edit_core buffer (one line: no newline is ever inserted).
    ed: Buffer,
    kill: String,
    hist: VecDeque<String>,
    hist_at: Option<usize>,
    draft: String,
    search: Option<Search>,
    hist_path: Option<String>,
    hist_size: u64,
    hist_err: i64,
    home: String,
    cwd: String,
    oldpwd: String,
    aliases: Vec<(String, String)>,
    apps: Option<Vec<String>>,
    facts: Facts,
    cmds: u32,
    last_kind: u32,
    last_verb: u8,
    dirty: bool,
    /// MIDDEN2: a pipeline stage's captured Out rows, the stage's input, and "an Err row was said".
    cap: Option<Vec<String>>,
    stdin: Option<Vec<String>>,
    failed: bool,
    out_rows: u64,
    sourcing: u32,
    /// MIDDEN2: rows the view is scrolled up from the live bottom.
    scroll: usize,
    /// RING3PIPE: the launched programs whose output is still arriving.
    pipes: Vec<Piped>,
    /// ONE-SHOT (`-c`): no window; every Out/Err line is written to the console as it is said.
    oneshot: bool,
}

impl App {
    fn new(surf: *mut u8, win: i64, face: Option<txt::Txt>, oneshot: bool) -> App {
        let home = home();
        App {
            surf,
            win,
            face,
            rows: VecDeque::new(),
            bytes: 0,
            ed: Buffer::new(""),
            kill: String::new(),
            hist: VecDeque::new(),
            hist_at: None,
            draft: String::new(),
            search: None,
            hist_path: None,
            hist_size: 0,
            hist_err: 0,
            cwd: home.clone(),
            oldpwd: home.clone(),
            home,
            aliases: Vec::new(),
            apps: None,
            facts: Facts { exec: true, ..Facts::ring3(cfg!(target_arch = "x86_64")) }, // RING3EXEC (B521) + ARMPIPE (A108): SYS_SPAWN_PATH on both arches
            cmds: 0,
            last_kind: 0,
            last_verb: 0,
            dirty: !oneshot,
            cap: None,
            stdin: None,
            failed: false,
            out_rows: 0,
            sourcing: 0,
            scroll: 0,
            pipes: Vec::new(),
            oneshot,
        }
    }

    fn lh(&self) -> i32 {
        if self.face.is_some() { txt::LH_TT } else { txt::LH_BITMAP }
    }
    fn cell(&self) -> f32 {
        self.face.as_ref().map_or(8.0, |t| t.cell)
    }
    fn cols(&self) -> usize {
        (((W - 8) as f32) / self.cell()) as usize
    }
    /// Scrollback rows that fit above the prompt.
    fn fit(&self) -> usize {
        let py = H - self.lh() - 4;
        ((py - 6) / self.lh()).max(1) as usize
    }
    fn line_text(&self) -> String {
        self.ed.text().to_string()
    }
    /// A path operand: `~` against the session home, anything else against the cwd.
    fn path(&self, p: &str) -> String {
        if p == "~" {
            return self.home.clone();
        }
        if let Some(r) = p.strip_prefix("~/") {
            return resolve(&self.home, r);
        }
        resolve(&self.cwd, p)
    }

    /// Push text into the scrollback: split on newlines, wrapped at the window's columns, made printable. Inside a
    /// pipeline stage the Out lines become the next stage's input instead.
    fn say(&mut self, tone: Tone, text: &str) {
        if tone == Tone::Err {
            self.failed = true;
        }
        if tone == Tone::Out {
            if let Some(cap) = self.cap.as_mut() {
                for l in text.split('\n') {
                    cap.push(String::from(l));
                }
                return;
            }
        }
        let cols = self.cols().max(8);
        for l in text.split('\n') {
            if tone == Tone::Out {
                self.out_rows += 1;
            }
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
        if self.scroll > 0 {
            self.scroll += 1; // MIDDEN2: a scrolled view stays on the rows it shows while output arrives
        }
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

    // ---- history ----
    /// MIDDEN2: the history file under the session home — its tail read into memory, rewritten if it outgrew
    /// HIST_FILE_MAX. No home (no session) = no file.
    fn hist_load(&mut self) {
        if self.home == "/" {
            Line::new(b":: MIDDEN: history file=none reason=no-session-home loaded=0 ::").wire();
            return;
        }
        let path = alloc::format!("{}/{}", self.home, HIST_NAME);
        let mut loaded = 0usize;
        let mut err = 0i64;
        match stat(&path) {
            Ok(s) => {
                self.hist_size = s.size;
                let off = s.size.saturating_sub(HIST_READ as u64);
                match read_from(&path, off, HIST_READ) {
                    Ok(b) => {
                        let text = String::from_utf8_lossy(&b).into_owned();
                        let mut lines: Vec<&str> = text.split('\n').filter(|l| !l.is_empty()).collect();
                        if off > 0 && !lines.is_empty() {
                            lines.remove(0); // the first line of a tail window is a fragment
                        }
                        let skip = lines.len().saturating_sub(HIST_CAP);
                        for l in &lines[skip..] {
                            self.hist.push_back(String::from(*l));
                        }
                        loaded = self.hist.len();
                        if s.size > HIST_FILE_MAX {
                            let mut kept = String::new();
                            for l in self.hist.iter() {
                                kept.push_str(l);
                                kept.push('\n');
                            }
                            let r = write_at(&path, 0, true, kept.as_bytes());
                            if r < 0 {
                                err = r;
                            } else {
                                self.hist_size = kept.len() as u64;
                            }
                        }
                    }
                    Err(e) => err = e,
                }
            }
            Err(e) => {
                if e != una_abi::ENOENT {
                    err = e;
                }
                self.hist_size = 0;
            }
        }
        let mut l = Line::new(b":: MIDDEN: history file=");
        l.put(path.as_bytes()).put(b" loaded=").dec(loaded as i64).put(b" err=").dec(err).put(b" ::");
        l.wire();
        if err == 0 {
            self.hist_path = Some(path);
        } else {
            self.hist_err = err;
        }
    }

    /// Keep `line` in the history (memory and file) when `shell::keep_in_history` says so.
    fn remember(&mut self, line: &str) {
        if self.oneshot || !shell::keep_in_history(line) {
            return;
        }
        if self.hist.back().map(|s| s.as_str()) == Some(line) {
            return;
        }
        self.hist.push_back(String::from(line));
        if self.hist.len() > HIST_CAP {
            self.hist.pop_front();
        }
        let Some(path) = self.hist_path.clone() else {
            return;
        };
        let mut data = Vec::with_capacity(line.len() + 1);
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
        let r = write_at(&path, self.hist_size, self.hist_size == 0, &data);
        if r < 0 {
            // Said once, then the file is left alone: the history still works in memory.
            self.hist_err = r;
            self.hist_path = None;
            let mut l = Line::new(b":: MIDDEN: history save err=");
            l.dec(r).put(b" file=").put(path.as_bytes()).put(b" ::");
            l.wire();
            self.say(Tone::Err, &alloc::format!("history: {}: not saved - {} {} (kept in this window)", path, r, errno_text(r)));
        } else {
            self.hist_size += r as u64;
        }
    }

    fn recall_wire(&self, via: &str, line: Option<&str>, extra: &str) {
        let mut l = Line::new(b":: MIDDEN: recall ok=");
        l.dec(line.is_some() as i64).put(b" via=").put(via.as_bytes()).put(b" line=").put(line.unwrap_or("-").as_bytes());
        l.put(b" ").put(extra.as_bytes()).put(b" ::");
        l.wire();
    }

    fn set_line(&mut self, s: &str) {
        self.ed = Buffer::new(s);
        self.ed.set_cursor(s.len());
    }

    // ---- the line, executed command by command ----
    /// One typed (or sourced) line: echoed (redacted when secret-shaped), kept in the history, split, run.
    fn run_line(&mut self, line: &str) -> bool {
        if self.sourcing == 0 {
            let shown = midden_core::redact(line).unwrap_or_else(|| String::from(line));
            let prompt = alloc::format!("{}> {}", self.cwd, shown);
            self.say(Tone::Echo, &prompt);
            self.remember(line);
        }
        let cmds = match shell::split(line) {
            Ok(c) => c,
            Err(e) => {
                let t = e.text();
                self.say(Tone::Err, &t);
                let mut l = Line::new(b":: MIDDEN: line refused=");
                l.put(e.word().as_bytes()).put(b" ::");
                l.wire();
                return false;
            }
        };
        let n = cmds.len();
        if n == 0 {
            return self.run_simple(""); // the empty line: `plan` answers Nothing, the wire says so as it always did
        }
        let ok = self.run_cmds(cmds.into_iter().collect(), true, None);
        if n > 1 {
            let mut l = Line::new(b":: MIDDEN: line cmds=");
            l.dec(n as i64).put(b" ok=").dec(ok as i64).put(b" ::");
            l.wire();
        }
        ok
    }

    /// Run commands in order under the `;` / `&&` / `||` rule. A pipeline parked on a program's pipe takes the rest
    /// of the queue with it (resumed at the program's EOF, `pump`).
    fn run_cmds(&mut self, mut q: VecDeque<Cmd>, mut ok: bool, mut prev: Option<Join>) -> bool {
        while let Some(c) = q.pop_front() {
            if let Some(j) = prev {
                if !shell::should_run(j, ok) {
                    prev = Some(c.join);
                    continue;
                }
            }
            prev = Some(c.join);
            let n = c.stages.len();
            match self.run_stages(&c.stages, 0, n, None) {
                Step::Done(r) => ok = r,
                Step::Parked => {
                    if let Some(cont) = self.pipes.last_mut().and_then(|p| p.cont.as_mut()) {
                        cont.q = q;
                        cont.join = c.join;
                    }
                    return true;
                }
            }
        }
        ok
    }

    /// Run `stages` (global stage numbers `first..first+len` of a pipeline of `total`), feeding each stage's Out rows
    /// to the next; `input` is the first stage's input (a program's collected output on a resume).
    fn run_stages(&mut self, stages: &[String], first: usize, total: usize, mut input: Option<Vec<String>>) -> Step {
        let rows0 = self.out_rows;
        let mut ok = true;
        let mut last_verb = String::new();
        for (k, raw) in stages.iter().enumerate() {
            let i = first + k;
            let stage = self.expand(raw);
            let last = k + 1 == stages.len();
            last_verb = midden_core::canon_verb(stage.split_whitespace().next().unwrap_or(""));
            if total > 1 && self.is_program_stage(&stage) {
                if self.oneshot {
                    self.say(Tone::Err, "pipeline: a program stage needs the window's pipe pump (not under -c)");
                    self.pipeline_wire(total, &last_verb, 0, false, Some("oneshot"));
                    return Step::Done(false);
                }
                let cont = Cont { rest: stages[k + 1..].to_vec(), stages: total, q: VecDeque::new(), join: Join::Seq, got: Vec::new(), last: program_word(&stage) };
                let before = self.pipes.len();
                self.run_simple_cont(&stage, Some(cont));
                if self.pipes.len() <= before {
                    return Step::Done(false);
                }
                // SHELLABI (B612): a later stage — the previous stage's lines are its input, through SYS_PIPE_WRITE.
                if i > 0 {
                    let mut b: Vec<u8> = Vec::new();
                    for l in input.take().unwrap_or_default() {
                        b.extend_from_slice(l.as_bytes());
                        b.push(b'\n');
                    }
                    if let Some(p) = self.pipes.last_mut() {
                        p.feed = b;
                        p.feed_eof = true;
                    }
                }
                return Step::Parked;
            }
            if !last {
                self.cap = Some(Vec::new());
            }
            self.stdin = input.take();
            ok = self.run_simple(&stage);
            self.stdin = None;
            input = self.cap.take();
        }
        if total > 1 {
            let rows = self.out_rows - rows0;
            self.pipeline_wire(total, &last_verb, rows, ok, None);
        }
        Step::Done(ok)
    }

    fn pipeline_wire(&self, stages: usize, last: &str, rows: u64, ok: bool, refused: Option<&str>) {
        let mut l = Line::new(b":: MIDDEN: pipeline stages=");
        l.dec(stages as i64).put(b" last=").put(if last.is_empty() { b"-" } else { last.as_bytes() });
        l.put(b" rows=").dec(rows as i64).put(b" ok=").dec(ok as i64);
        if let Some(r) = refused {
            l.put(b" refused=").put(r.as_bytes());
        }
        l.put(b" ::");
        l.wire();
    }

    /// The stage with its first word's alias expanded (`shell::expand_alias`).
    fn expand(&self, stage: &str) -> String {
        shell::expand_alias(stage, &self.aliases).unwrap_or_else(|| String::from(stage.trim()))
    }

    /// Does this stage launch a program (`run …`, or a bare name that is no verb or builtin and resolves)?
    fn is_program_stage(&mut self, stage: &str) -> bool {
        let w = stage.split_whitespace().next().unwrap_or("");
        let canon = midden_core::canon_verb(w);
        if canon == "run" {
            return self.facts.exec;
        }
        if shell::BUILTINS.contains(&canon.as_str()) || midden_core::is_verb(w, &self.facts) || !self.facts.exec {
            return false;
        }
        midden_core::resolve_exec(w, &mut R3Vol { cwd: &self.cwd }).is_some()
    }

    fn account(&mut self, kind: u32, verb: &str) {
        self.cmds += 1;
        self.last_kind = kind;
        self.last_verb = ring3::verb_code(verb);
    }

    fn run_simple(&mut self, stage: &str) -> bool {
        self.run_simple_cont(stage, None)
    }

    /// ONE simple command — the pre-MIDDEN2 `run_line` body: the builtins, `run`, then the core's plan, executed.
    /// `cont` rides into a launch (a pipeline's first stage). The status: no Err row was said (a launch: a pid).
    fn run_simple_cont(&mut self, stage: &str, cont: Option<Cont>) -> bool {
        self.failed = false;
        let facts = self.facts;
        let word = midden_core::canon_verb(stage.split_whitespace().next().unwrap_or(""));
        if word != "run" && shell::BUILTINS.contains(&word.as_str()) {
            let t = stage.trim();
            let rest = String::from(t[t.split_whitespace().next().unwrap_or("").len()..].trim_start());
            self.builtin(&word, &rest);
            let ok = !self.failed;
            self.account(2, &word);
            let mut l = Line::new(b":: MIDDEN: builtin=");
            l.put(word.as_bytes()).put(b" ok=").dec(ok as i64).put(b" rows=").dec(self.rows.len() as i64).put(b" ::");
            l.wire();
            return ok;
        }
        if word == "run" && facts.exec {
            // RING3EXEC (B521): `run <path> [args]` — not a verb on `Facts::ring3` (no job table), served here: the
            // core's resolver finds the program, SYS_SPAWN_PATH launches it with argv = the words after `run`.
            let words: Vec<&str> = stage.split_whitespace().skip(1).collect();
            let r = match words.first() {
                None => {
                    self.say(Tone::Err, "usage: run <path> [args]");
                    Err(una_abi::EINVAL)
                }
                Some(&prog) => {
                    let found = midden_core::resolve_exec(prog, &mut R3Vol { cwd: &self.cwd });
                    self.launch("run", prog, found.as_deref(), &words, stage, cont)
                }
            };
            let ok = r.is_ok();
            self.exec_done("run", r);
            return ok;
        }
        let p = {
            let mut vol = R3Vol { cwd: &self.cwd };
            plan(stage, &facts, &mut vol)
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
                    self.say(Tone::Out, &alloc::format!("MIDDEN.ELF serves: {} {} (+ the core's own words above)", ring3::SERVED.join(" "), shell::FILTERS.join(" ")));
                    self.say(Tone::Out, &alloc::format!("MIDDEN.ELF builtins: {}; lines: a | b, a && b, a || b, a ; b", shell::BUILTINS.join(" ")));
                    self.say(Tone::Out, "MIDDEN.ELF keys: Up/Down history, Ctrl+R search, Tab or Ctrl+Tab complete, Ctrl+A/E/B/F/K/U/W/Y/D/Z/L, wheel or PgUp/PgDn scroll, Ctrl+C on an empty line kills the newest job");
                    if facts.exec {
                        self.say(Tone::Out, "MIDDEN.ELF launches: run <path> [args], and bare names (cwd, then /apps; detached, as this program's user; their output arrives here; at any pipeline stage; `kill <pid|%n>`)");
                    }
                }
                (1, word.clone(), true)
            }
            Plan::Host { verb, rest } => {
                let served = ring3::serves(verb) || shell::FILTERS.contains(&verb.as_str());
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
                let r = self.launch(typed, typed, Some(name.as_str()), &argv, stage, cont);
                let ok = r.is_ok();
                self.exec_done(typed, r);
                return ok;
            }
        };
        if kind != 0 {
            self.account(kind, &verb);
        }
        let mut l = Line::new(b":: MIDDEN: plan=");
        l.put(ring3::kind_name(kind).as_bytes()).put(b" verb=").put(if verb.is_empty() { b"-" } else { verb.as_bytes() });
        l.put(b" served=").dec(served as i64).put(b" rows=").dec(self.rows.len() as i64).put(b" ::");
        l.wire();
        served && !self.failed
    }

    /// RING3EXEC (B521): launch `found` (the core's resolution of `typed`: cwd-relative, else under /apps) with `argv`
    /// through SYS_SPAWN_PATH. The pid, or the errno the kernel answered (its refusal line is on the wire beside ours).
    fn launch(&mut self, verb: &str, typed: &str, found: Option<&str>, argv: &[&str], cmd: &str, cont: Option<Cont>) -> Result<i64, i64> {
        let Some(name) = found else {
            self.say(Tone::Err, &alloc::format!("{}: {}: {}", verb, typed, errno_text(una_abi::ENOENT)));
            return Err(una_abi::ENOENT);
        };
        let mut path = self.path(name);
        if !fsio::is_file(&path) && !name.contains('/') && fsio::is_file(&apps_path(name)) {
            path = apps_path(name); // MIDDEN2: a bare program name under /apps
        }
        // RING3PIPE: ask for a pipe; a kernel without the flag answers E2BIG to the high argc bits — launch plainly.
        let mut piped = !self.oneshot;
        let mut r = if piped { fsio::spawn_piped(&path, argv) } else { fsio::spawn_path(&path, argv) };
        if piped && r == una_abi::E2BIG {
            piped = false;
            r = fsio::spawn_path(&path, argv);
        }
        if r > 0 {
            self.say(Tone::Out, &alloc::format!("{}: {} started - pid {}", verb, path, r));
            if piped {
                self.pipes.push(Piped { pid: r, cmd: String::from(cmd), part: Vec::new(), lines: 0, bytes: 0, dropped: 0, cont, feed: Vec::new(), feed_eof: false, fed: 0 });
            } else if cont.is_some() {
                self.say(Tone::Err, "pipeline: this kernel launched it without a pipe (no SPAWN_PIPE), so its output cannot feed the next stage");
                return Err(una_abi::E2BIG);
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
            self.feed(i); // SHELLABI (B612): a later stage's input first, then what the program wrote
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
                        match p.cont.as_mut() {
                            Some(c) => c.got.push(tail),
                            None => self.say(Tone::Out, &tail),
                        }
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
                    // MIDDEN2: a pipeline's first stage ended — its lines feed the rest, then the line goes on.
                    if let Some(c) = p.cont.take() {
                        let st = if c.rest.is_empty() {
                            // SHELLABI (B612): the program was the LAST stage — its lines are the pipeline's output.
                            for l in &c.got {
                                self.say(Tone::Out, l);
                            }
                            self.pipeline_wire(c.stages, &c.last, c.got.len() as u64, r == 0, None);
                            Step::Done(r == 0)
                        } else {
                            let first = c.stages - c.rest.len();
                            self.run_stages(&c.rest, first, c.stages, Some(c.got))
                        };
                        let ok = match st {
                            Step::Done(ok) => ok,
                            Step::Parked => true,
                        };
                        self.run_cmds(c.q, ok, Some(c.join));
                    }
                }
            }
        }
    }

    /// SHELLABI (B612): hand pipe `i`'s owed input to the child — SYS_PIPE_WRITE takes up to one input ring per call
    /// (-EAGAIN = full, or the child holds the focus: the rest waits for the next frame), then one zero-length write
    /// delivers the KEY_EOT. -EPIPE (the child exited without taking it) or any refusal drops the rest, said once.
    fn feed(&mut self, i: usize) {
        let pid = self.pipes[i].pid;
        for _ in 0..FEED_WRITES {
            let p = &mut self.pipes[i];
            if p.feed.is_empty() && !p.feed_eof {
                return;
            }
            let r = if p.feed.is_empty() {
                sys(una_abi::SYS_PIPE_WRITE, pid as u64, 0, 0, 0)
            } else {
                sys(una_abi::SYS_PIPE_WRITE, pid as u64, p.feed.as_ptr() as u64, p.feed.len() as u64, 0)
            };
            if r == una_abi::EAGAIN {
                return;
            }
            if r < 0 {
                let fed = p.fed;
                p.feed.clear();
                p.feed_eof = false;
                if r != una_abi::EPIPE {
                    self.say(Tone::Err, &alloc::format!("pipeline: pid {}: its input was refused - {} {}", pid, r, errno_text(r)));
                }
                let mut l = Line::new(b":: MIDDEN: pipe feed pid=");
                l.dec(pid).put(b" bytes=").dec(fed as i64).put(b" ok=0 err=").dec(r).put(b" ::");
                l.wire();
                return;
            }
            if p.feed.is_empty() {
                p.feed_eof = false;
                let mut l = Line::new(b":: MIDDEN: pipe feed pid=");
                l.dec(pid).put(b" bytes=").dec(p.fed as i64).put(b" ok=1 ::");
                l.wire();
                return;
            }
            let n = (r as usize).min(p.feed.len());
            p.feed.drain(..n);
            p.fed += n as u64;
        }
    }

    /// SHELLABI (B612): a one-shot's standard input — the KEY_DOWN bytes on its own input ring (a launcher's
    /// SYS_PIPE_WRITE) up to KEY_EOT, or STDIN_IDLE_MS with no byte (said: `eot=0`). Lines, the last one unterminated
    /// or not.
    fn ring_stdin(&mut self) -> Vec<String> {
        let mut b: Vec<u8> = Vec::new();
        let mut last = now_ms();
        let mut eot = false;
        loop {
            let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
            if ev >= 0 {
                let ev = ev as u64;
                if input_ev_type(ev) == INPUT_EV_KEY_DOWN {
                    let c = input_ev_payload(ev) as u8;
                    if c == KEY_EOT {
                        eot = true;
                        break;
                    }
                    b.push(c);
                    last = now_ms();
                }
                continue;
            }
            if now_ms().saturating_sub(last) >= STDIN_IDLE_MS {
                break;
            }
            sleep_ms(2);
        }
        let mut l = Line::new(b":: MIDDEN: stdin bytes=");
        l.dec(b.len() as i64).put(b" eot=").dec(eot as i64).put(b" ::");
        l.wire();
        let text = String::from_utf8_lossy(&b).into_owned();
        let mut v: Vec<String> = text.split('\n').map(String::from).collect();
        if v.last().is_some_and(|l| l.is_empty()) {
            v.pop();
        }
        v
    }

    /// SHELLABI (B612): `SYS_KILL(pid)` on one of this shell's jobs; a kill cancels the rest of that job's line.
    fn kill_pid(&mut self, pid: i64, via: &str) -> bool {
        let r = sys(una_abi::SYS_KILL, pid as u64, 0, 0, 0);
        let mut cancelled = false;
        if r == 0 {
            if let Some(p) = self.pipes.iter_mut().find(|p| p.pid == pid) {
                cancelled = p.cont.take().is_some();
                p.feed.clear();
                p.feed_eof = false;
            }
            self.say(Tone::Out, &alloc::format!("kill: pid {} killed{}", pid, if cancelled { " (the rest of its line is cancelled)" } else { "" }));
        } else {
            self.say(Tone::Err, &alloc::format!("kill: pid {}: {} {}", pid, r, errno_text(r)));
        }
        let mut l = Line::new(b":: MIDDEN: kill ok=");
        l.dec((r == 0) as i64).put(b" via=").put(via.as_bytes()).put(b" pid=").dec(pid);
        if r != 0 {
            l.put(b" err=").dec(r);
        }
        l.put(b" cancelled=").dec(cancelled as i64).put(b" ::");
        l.wire();
        r == 0
    }

    /// RING3PIPE: `b` from pipe `i` — complete lines into the scrollback (or a pipeline's collection), the rest held
    /// for the next chunk.
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
            let line = String::from(line.trim_end_matches('\r'));
            match self.pipes[i].cont.as_mut() {
                Some(_) if line.starts_with(WIRE_PREFIX) => {} // SHELLABI: a one-shot child's wire, not its data
                Some(c) => c.got.push(line),
                None => self.say(Tone::Out, &line),
            }
            n += 1;
        }
        self.pipes[i].lines += n;
        n
    }

    /// RING3EXEC (B521): account a launch line and put it on the wire —
    /// `:: MIDDEN: plan=Exec verb=<v> served=1 pid=<n> ::` or `served=0 err=<errno>`.
    fn exec_done(&mut self, verb: &str, r: Result<i64, i64>) {
        self.account(3, verb);
        let mut l = Line::new(b":: MIDDEN: plan=Exec verb=");
        l.put(verb.as_bytes());
        match r {
            Ok(pid) => l.put(b" served=1 pid=").dec(pid),
            Err(e) => l.put(b" served=0 err=").dec(e),
        };
        l.put(b" ::");
        l.wire();
    }

    // ---- the builtins ----
    fn builtin(&mut self, word: &str, rest: &str) {
        match word {
            "alias" => {
                if rest.is_empty() {
                    if self.aliases.is_empty() {
                        self.say(Tone::Out, "(no aliases)");
                    }
                    let lines: Vec<String> = self.aliases.iter().map(|(n, v)| alloc::format!("alias {}='{}'", n, v)).collect();
                    for l in lines {
                        self.say(Tone::Out, &l);
                    }
                } else if rest.contains('=') {
                    match shell::parse_alias(rest) {
                        Some((n, v)) => match self.aliases.iter_mut().find(|(a, _)| *a == n) {
                            Some(slot) => slot.1 = v,
                            None => self.aliases.push((n, v)),
                        },
                        None => self.say(Tone::Err, "usage: alias name=value  (a name is letters, digits, _ - .)"),
                    }
                } else {
                    for n in rest.split_whitespace() {
                        let found = self.aliases.iter().find(|(a, _)| a == n).map(|(a, v)| alloc::format!("alias {}='{}'", a, v));
                        match found {
                            Some(l) => self.say(Tone::Out, &l),
                            None => self.say(Tone::Err, &alloc::format!("alias: {}: not found", n)),
                        }
                    }
                }
            }
            "unalias" => {
                if rest.trim() == "-a" {
                    self.aliases.clear();
                    return;
                }
                if rest.trim().is_empty() {
                    self.say(Tone::Err, "usage: unalias name... | -a");
                }
                for n in rest.split_whitespace() {
                    let before = self.aliases.len();
                    self.aliases.retain(|(a, _)| a != n);
                    if self.aliases.len() == before {
                        self.say(Tone::Err, &alloc::format!("unalias: {}: not found", n));
                    }
                }
            }
            "source" => {
                let Some(f) = rest.split_whitespace().next() else {
                    self.say(Tone::Err, "usage: source <file>");
                    return;
                };
                let p = self.path(f);
                if let Err(e) = self.source(&p) {
                    self.say(Tone::Err, &alloc::format!("source: {}: {} {}", p, e, errno_text(e)));
                }
            }
            "jobs" => {
                if self.pipes.is_empty() {
                    self.say(Tone::Out, "(no jobs: no launched program's pipe is open)");
                }
                let lines: Vec<String> = self
                    .pipes
                    .iter()
                    .enumerate()
                    .map(|(k, p)| alloc::format!("[{}] pid {}  running  {}  ({} lines{})", k + 1, p.pid, p.cmd, p.lines, if p.cont.is_some() { ", feeding a pipeline" } else { "" }))
                    .collect();
                for l in lines {
                    self.say(Tone::Out, &l);
                }
            }
            "kill" => {
                let ops: Vec<String> = rest.split_whitespace().map(String::from).collect();
                if ops.is_empty() {
                    self.say(Tone::Err, "usage: kill <pid | %job>");
                }
                for op in ops {
                    // SHELLABI (B612): SYS_KILL — the kernel kills only this program's own SPAWN_PIPE children.
                    match kill_target(&op, &self.pipes) {
                        Some(pid) => {
                            self.kill_pid(pid, "builtin");
                        }
                        None => {
                            self.say(Tone::Err, &alloc::format!("kill: {}: no such job (give a pid, or %n from `jobs`)", op));
                            let mut l = Line::new(b":: MIDDEN: kill ok=0 via=builtin target=");
                            l.put(op.as_bytes()).put(b" err=").dec(una_abi::ESRCH).put(b" ::");
                            l.wire();
                        }
                    }
                }
            }
            other => {
                let r = ring3::refusal(other);
                self.msg(&r);
            }
        }
    }

    /// `source <file>` and the rc: every line run as typed, none echoed or kept in the history. Lines run.
    fn source(&mut self, path: &str) -> Result<usize, i64> {
        if self.sourcing >= SOURCE_DEPTH {
            self.say(Tone::Err, &alloc::format!("source: {}: refused - nested {} deep", path, SOURCE_DEPTH));
            return Ok(0);
        }
        let b = read_file(path)?;
        let text = String::from_utf8_lossy(&b).into_owned();
        self.sourcing += 1;
        let mut n = 0;
        for l in text.split('\n') {
            let l = l.trim_end_matches('\r');
            if l.trim().is_empty() || l.trim_start().starts_with('#') {
                continue;
            }
            self.run_line(l);
            n += 1;
        }
        self.sourcing -= 1;
        Ok(n)
    }

    /// MIDDEN2: `<home>/.middenrc`, sourced once at start (a script, not a preference). Its lines are not counted in
    /// the exit status, which reports what was TYPED.
    fn rc(&mut self) {
        if self.home == "/" {
            Line::new(b":: MIDDEN: rc path=none reason=no-session-home lines=0 ::").wire();
            return;
        }
        let path = alloc::format!("{}/{}", self.home, RC_NAME);
        let saved = (self.cmds, self.last_kind, self.last_verb);
        let r = self.source(&path);
        (self.cmds, self.last_kind, self.last_verb) = saved;
        let (n, err) = match r {
            Ok(n) => (n, 0),
            Err(e) => (0, e),
        };
        if err != 0 && err != una_abi::ENOENT {
            self.say(Tone::Err, &alloc::format!("{}: {} {}", path, err, errno_text(err)));
        }
        let mut l = Line::new(b":: MIDDEN: rc path=");
        l.put(path.as_bytes()).put(b" lines=").dec(n as i64).put(b" err=").dec(err).put(b" ::");
        l.wire();
    }

    /// The lines a filter reads: the pipeline's, else its file operand's. `None` = said why not.
    fn filter_input(&mut self, verb: &str, w: &[String]) -> Option<Vec<String>> {
        if let Some(i) = self.stdin.take() {
            return Some(i);
        }
        let Some(f) = shell::filter_file(verb, w) else {
            if self.oneshot {
                return Some(self.ring_stdin()); // SHELLABI (B612): a one-shot's input is its launcher's SYS_PIPE_WRITE
            }
            self.say(Tone::Err, &alloc::format!("{}: no input - give it a file, or a pipeline (`ls | {}`)", verb, verb));
            return None;
        };
        let target = self.path(f);
        match read_file(&target) {
            Ok(b) => {
                let text = String::from_utf8_lossy(&b).into_owned();
                let mut v: Vec<String> = text.split('\n').map(String::from).collect();
                if v.last().is_some_and(|l| l.is_empty()) {
                    v.pop();
                }
                Some(v)
            }
            Err(e) => {
                self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, target, e, errno_text(e)));
                None
            }
        }
    }

    fn cd_wire(&self, ok: bool) {
        let mut l = Line::new(b":: MIDDEN: cd ok=");
        l.dec(ok as i64).put(b" cwd=").put(self.cwd.as_bytes()).put(b" ::");
        l.wire();
    }

    fn host(&mut self, verb: &str, rest: &str) {
        let args: Vec<&str> = rest.split_whitespace().collect();
        let operand = args.iter().copied().find(|a| !a.starts_with('-'));
        match verb {
            "clear" => {
                self.rows.clear();
                self.bytes = 0;
                self.scroll = 0;
                self.dirty = true;
            }
            "pwd" => {
                let c = self.cwd.clone();
                self.say(Tone::Out, &c);
            }
            "history" => {
                if args.first() == Some(&"-c") {
                    self.hist.clear();
                    self.hist_at = None;
                    if let Some(p) = self.hist_path.clone() {
                        let r = write_at(&p, 0, true, &[]);
                        if r < 0 {
                            self.say(Tone::Err, &alloc::format!("history: {}: {} {}", p, r, errno_text(r)));
                        } else {
                            self.hist_size = 0;
                        }
                    }
                    return;
                }
                let n = operand.and_then(|o| o.parse::<usize>().ok()).unwrap_or(self.hist.len());
                let skip = self.hist.len().saturating_sub(n);
                let lines: Vec<String> = self.hist.iter().enumerate().skip(skip).map(|(i, h)| alloc::format!("{:>3}  {}", i + 1, h)).collect();
                for l in lines {
                    self.say(Tone::Out, &l);
                }
                if self.hist_err != 0 {
                    let m = alloc::format!("(the history file is not being written: {} {})", self.hist_err, errno_text(self.hist_err));
                    self.say(Tone::Echo, &m);
                }
            }
            "cd" => {
                let target = match operand {
                    None => self.home.clone(),
                    Some("-") => self.oldpwd.clone(),
                    Some(o) => self.path(o),
                };
                match stat(&target) {
                    Ok(s) if s.kind == 1 => {
                        self.oldpwd = core::mem::replace(&mut self.cwd, target);
                        self.cd_wire(true);
                    }
                    Ok(_) => {
                        self.say(Tone::Err, &alloc::format!("cd: {}: {}", target, errno_text(una_abi::ENOTDIR)));
                        self.cd_wire(false);
                    }
                    Err(e) => {
                        self.say(Tone::Err, &alloc::format!("cd: {}: {} {}", target, e, errno_text(e)));
                        self.cd_wire(false);
                    }
                }
            }
            "stat" => {
                let Some(o) = operand else {
                    self.say(Tone::Err, "usage: stat <path>");
                    return;
                };
                let target = self.path(o);
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
                let target = self.path(operand.unwrap_or("."));
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
                    // MIDDEN2: no operand inside a pipeline passes the input through, as `cat` does.
                    match self.stdin.take() {
                        Some(lines) => {
                            for l in lines {
                                self.say(Tone::Out, &l);
                            }
                        }
                        // SHELLABI (B612): a one-shot reads its launcher's SYS_PIPE_WRITE input, as `cat` reads stdin.
                        None if self.oneshot => {
                            for l in self.ring_stdin() {
                                self.say(Tone::Out, &l);
                            }
                        }
                        None => self.say(Tone::Err, &alloc::format!("usage: {} <path>", verb)),
                    }
                    return;
                };
                let target = self.path(o);
                match read_file(&target) {
                    Ok(b) => {
                        let text = String::from_utf8_lossy(&b).into_owned();
                        self.say(Tone::Out, text.trim_end_matches('\n'));
                    }
                    Err(e) => self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, target, e, errno_text(e))),
                }
            }
            // MIDDEN2: the filters, over a pipeline's lines or the file operand's.
            "grep" | "wc" | "head" | "tail" => {
                let w = shell::words(rest);
                let Some(input) = self.filter_input(verb, &w) else {
                    return;
                };
                match shell::filter(verb, &w, &input) {
                    Ok(out) => {
                        // grep that matched nothing fails, as everywhere (so `grep x f && …` reads right).
                        let none = out.is_empty();
                        for l in out {
                            self.say(Tone::Out, &l);
                        }
                        if verb == "grep" && none {
                            self.failed = true;
                        }
                    }
                    Err(e) => self.say(Tone::Err, &e),
                }
            }
            // `ring3::SERVED` + `shell::FILTERS` are the lists above; a word added there without an arm is a visible refusal.
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

    // ---- completion ----
    /// MIDDEN2: Tab / Ctrl+Tab — complete the word before the caret.
    fn complete(&mut self) {
        let line = self.line_text();
        let caret = self.ed.cursor().min(line.len());
        let (s, cmdpos) = shell::word_at(&line, caret);
        let word = String::from(&line[s..caret]);
        let (base, cands) = if cmdpos && !word.contains('/') {
            let apps = self.app_names();
            let mut extra: Vec<&str> = shell::BUILTINS.to_vec();
            extra.extend(self.aliases.iter().map(|(n, _)| n.as_str()));
            extra.extend(apps.iter().map(|s| s.as_str()));
            (String::new(), shell::verb_candidates(&word, &self.facts, &extra))
        } else {
            let (dir, leaf) = match word.rfind('/') {
                Some(i) => (String::from(&word[..=i]), String::from(&word[i + 1..])),
                None => (String::new(), word.clone()),
            };
            let target = if dir.is_empty() { self.cwd.clone() } else { self.path(&dir) };
            let names = list(&target).unwrap_or_default();
            (dir, shell::path_candidates(&leaf, names.iter().map(|n| n.as_str())))
        };
        let leaf = &word[base.len()..];
        let mut got: Option<String> = None;
        match shell::complete_word(leaf, &cands) {
            Some((rep, finished)) => {
                let mut new = alloc::format!("{}{}", base, rep);
                if finished && !rep.ends_with('/') {
                    new.push(' ');
                }
                if line.len() - (caret - s) + new.len() <= INP_CAP {
                    self.ed.replace(s..caret, &new);
                    self.ed.set_cursor(s + new.len());
                    got = Some(String::from(new.trim_end()));
                }
            }
            None if cands.len() > 1 => {
                let shown = cands.join("  ");
                self.say(Tone::Echo, &shown);
            }
            None => {}
        }
        let mut l = Line::new(b":: MIDDEN: complete ok=");
        l.dec(got.is_some() as i64).put(b" word=").put(if word.is_empty() { b"-" } else { word.as_bytes() });
        l.put(b" got=").put(got.as_deref().unwrap_or("-").as_bytes()).put(b" cands=").dec(cands.len() as i64).put(b" ::");
        l.wire();
    }

    /// The program names under /apps, as a bare word launches them (`MIDDEN.ELF` -> `midden`), listed once.
    fn app_names(&mut self) -> Vec<String> {
        if self.apps.is_none() {
            let names = list("/apps").unwrap_or_default();
            let v: Vec<String> = names
                .iter()
                .filter_map(|n| {
                    let lower = n.to_ascii_lowercase();
                    lower.strip_suffix(".elf").map(String::from)
                })
                .collect();
            self.apps = Some(v);
        }
        self.apps.clone().unwrap_or_default()
    }

    // ---- keys ----
    /// MIDDEN2: a key while Ctrl+R searches. `true` = consumed; `false` = the search ended and the key is processed
    /// as usual on the accepted line.
    fn search_key(&mut self, k: u8) -> bool {
        let Some(mut s) = self.search.take() else {
            return false;
        };
        let hv: Vec<String> = self.hist.iter().cloned().collect();
        match k {
            0x12 => {
                let from = s.at.unwrap_or(hv.len());
                if let Some(i) = shell::search_back(&hv, &s.q, from) {
                    s.at = Some(i);
                }
                let line = s.at.map(|i| hv[i].clone());
                self.recall_wire("search", line.as_deref(), &alloc::format!("q={}", s.q));
            }
            0x08 | 0x7f => {
                s.q.pop();
                s.at = shell::search_back(&hv, &s.q, hv.len());
                let line = s.at.map(|i| hv[i].clone());
                self.recall_wire("search", line.as_deref(), &alloc::format!("q={}", s.q));
            }
            0x20..=0x7e => {
                s.q.push(k as char);
                s.at = shell::search_back(&hv, &s.q, hv.len());
                let line = s.at.map(|i| hv[i].clone());
                self.recall_wire("search", line.as_deref(), &alloc::format!("q={}", s.q));
            }
            KEY_ESC | 0x07 | 0x03 => {
                let d = s.draft.clone();
                self.set_line(&d);
                return true;
            }
            _ => {
                // Enter runs the hit; any other key accepts it onto the line and acts on it.
                let line = match s.at {
                    Some(i) => hv[i].clone(),
                    None => s.draft.clone(),
                };
                self.set_line(&line);
                return false;
            }
        }
        self.search = Some(s);
        true
    }

    fn key(&mut self, k: u8) {
        if self.search.is_some() && self.search_key(k) {
            self.dirty = true;
            return;
        }
        let at = self.ed.cursor();
        match k {
            b'\n' | b'\r' => {
                let line = self.line_text();
                self.ed = Buffer::new("");
                self.hist_at = None;
                self.scroll = 0;
                self.run_line(&line);
            }
            0x03 => {
                // Ctrl+C: the line is abandoned, as a terminal does; on an empty line, the newest job is named.
                let text = self.line_text();
                if text.is_empty() && !self.pipes.is_empty() {
                    // SHELLABI (B612): Ctrl+C on an empty line kills the newest job (SYS_KILL) and cancels its line.
                    let pid = self.pipes[self.pipes.len() - 1].pid;
                    let echo = alloc::format!("{}> ^C", self.cwd);
                    self.say(Tone::Echo, &echo);
                    self.kill_pid(pid, "ctrl-c");
                } else {
                    let shown = midden_core::redact(&text).unwrap_or(text);
                    let line = alloc::format!("{}> {}^C", self.cwd, shown);
                    self.say(Tone::Echo, &line);
                }
                self.ed = Buffer::new("");
                self.hist_at = None;
            }
            0x08 => {
                self.ed.delete(Unit::Grapheme, Direction::Backward);
            }
            0x7f | 0x04 => {
                self.ed.delete(Unit::Grapheme, Direction::Forward);
            }
            0x01 => self.ed.move_cursors(Motion::LineStart, false),
            0x05 => self.ed.move_cursors(Motion::LineEnd, false),
            0x02 => self.ed.move_cursors(Motion::WordLeft, false),
            0x06 => self.ed.move_cursors(Motion::WordRight, false),
            0x0B => {
                self.kill = self.ed.slice(at..self.ed.len());
                self.ed.delete(Unit::Line, Direction::Forward);
            }
            0x15 => {
                self.kill = self.ed.slice(0..at);
                self.ed.delete(Unit::Line, Direction::Backward);
            }
            0x17 => {
                let from = self.ed.word_left(at);
                self.kill = self.ed.slice(from..at);
                self.ed.delete(Unit::Word, Direction::Backward);
            }
            0x19 => {
                if !self.kill.is_empty() && self.ed.len() + self.kill.len() <= INP_CAP {
                    let y = self.kill.clone();
                    self.ed.insert(&y);
                }
            }
            0x1A => {
                self.ed.undo();
            }
            0x0C => {
                self.rows.clear();
                self.bytes = 0;
                self.scroll = 0;
            }
            0x12 => {
                let draft = self.line_text();
                self.search = Some(Search { q: String::new(), at: None, draft });
            }
            0x09 | KEY_CTRL_TAB => self.complete(),
            KEY_PGUP | KEY_PGDN => {
                // SHELLABI (B612): the key bytes (Shift+PgUp/PgDn press them too; the actions beside are unbound).
                let fit = self.fit();
                let max = self.rows.len().saturating_sub(fit);
                let page = fit.saturating_sub(1).max(1);
                self.scroll = if k == KEY_PGUP { (self.scroll + page).min(max) } else { self.scroll.saturating_sub(page) };
            }
            KEY_LEFT => self.ed.move_cursors(Motion::Left, false),
            KEY_RIGHT => self.ed.move_cursors(Motion::Right, false),
            KEY_UP | KEY_DOWN => {
                if self.hist.is_empty() {
                    return;
                }
                let n = self.hist.len();
                if self.hist_at.is_none() {
                    self.draft = self.line_text();
                }
                let at = match (self.hist_at, k == KEY_UP) {
                    (None, true) => Some(n - 1),
                    (None, false) => None,
                    (Some(i), true) => Some(i.saturating_sub(1)),
                    (Some(i), false) => if i + 1 < n { Some(i + 1) } else { None },
                };
                self.hist_at = at;
                let line = match at {
                    Some(i) => self.hist[i].clone(),
                    None => self.draft.clone(),
                };
                self.set_line(&line);
                let via = if k == KEY_UP { "up" } else { "down" };
                let shown = at.map(|_| line.as_str());
                self.recall_wire(via, shown, &alloc::format!("at={}", at.map_or(-1, |i| i as i64)));
            }
            0x20..=0x7e => {
                if self.ed.len() < INP_CAP {
                    let mut b = [0u8; 4];
                    let s = (k as char).encode_utf8(&mut b);
                    self.ed.insert(s);
                }
                self.scroll = 0;
            }
            _ => return,
        }
        self.dirty = true;
    }

    /// MIDDEN2: one keymap action (INPUT_EV_ACTION's discriminant).
    fn action(&mut self, a: u64) {
        let fit = self.fit();
        let max = self.rows.len().saturating_sub(fit);
        match a {
            ACT_LINE_START => self.ed.move_cursors(Motion::LineStart, false),
            ACT_LINE_END => self.ed.move_cursors(Motion::LineEnd, false),
            ACT_PAGE_UP | ACT_PAGE_DOWN => return, // SHELLABI (B612): the KEY_PGUP/KEY_PGDN bytes pressed beside these scroll (`key`)
            ACT_TOP => self.scroll = max,
            ACT_BOTTOM => self.scroll = 0,
            ACT_COPY => {
                let t = self.line_text();
                if !t.is_empty() {
                    sys(una_abi::SYS_CLIP_SET, t.as_ptr() as u64, t.len() as u64, 0, 0);
                }
            }
            ACT_PASTE => {
                let mut b = alloc::vec![0u8; una_abi::CLIP_CAP];
                let r = sys(una_abi::SYS_CLIP_GET, b.as_mut_ptr() as u64, b.len() as u64, 0, 0);
                if r > 0 {
                    let s: String = b[..r as usize].iter().map(|&c| if (0x20..=0x7e).contains(&c) { c as char } else { ' ' }).collect();
                    let room = INP_CAP.saturating_sub(self.ed.len());
                    let s = &s[..s.len().min(room)];
                    self.ed.insert(s);
                }
            }
            ACT_UNDO => {
                self.ed.undo();
            }
            _ => return,
        }
        self.dirty = true;
    }

    /// MIDDEN2: one wheel report — positive is away from the user (older output).
    fn wheel(&mut self, d: i8) {
        let max = self.rows.len().saturating_sub(self.fit());
        if d > 0 {
            self.scroll = (self.scroll + d as usize * WHEEL_ROWS).min(max);
        } else {
            self.scroll = self.scroll.saturating_sub((-(d as i32)) as usize * WHEEL_ROWS);
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
        let line = self.line_text();
        let (prompt, body, caret) = match &self.search {
            Some(s) => {
                let hit = s.at.and_then(|i| self.hist.get(i)).cloned().unwrap_or_default();
                let c = hit.len();
                (alloc::format!("(search '{}') ", s.q), hit, c)
            }
            None => (alloc::format!("{}> ", self.cwd), line, self.ed.cursor()),
        };
        let cols = self.cols();
        let mut shown: Vec<u8> = prompt.as_bytes().to_vec();
        let start = shown.len();
        shown.extend_from_slice(body.as_bytes());
        // Keep the caret visible: show the tail of a line wider than the window.
        let caret_at = start + caret.min(body.len());
        let skip = (caret_at + 1).saturating_sub(cols);
        let vis: Vec<u8> = shown[skip..].iter().copied().take(cols).collect();
        self.text(4, py, &vis, th.ink);
        let cx = 4 + ((caret_at - skip) as f32 * self.cell()) as i32;
        self.fill(cx, py, 2, lh - 1, th.caret);
        let fit = self.fit();
        let n = self.rows.len();
        self.scroll = self.scroll.min(n.saturating_sub(fit));
        let end = n - self.scroll;
        let first = end.saturating_sub(fit);
        let rows: Vec<(Tone, String)> = self.rows.iter().skip(first).take(end - first).cloned().collect();
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
        if self.scroll > 0 {
            // MIDDEN2: the view is above the live bottom — say how far, just over the rule.
            let tag = alloc::format!("[+{} below]", self.scroll);
            let tx = W - 4 - (tag.len() as f32 * self.cell()) as i32;
            self.fill(tx - 2, py - 3 - lh, W - tx + 2, lh, th.bg);
            self.text(tx, py - 3 - lh, tag.as_bytes(), th.echo_ink);
        }
        sys(SYS_WIN_PRESENT, self.win as u64, 0, 0, 0);
    }
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
    let mut app = App::new(core::ptr::null_mut(), -1, None, true);
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
        oneshot(&line); // ONE-SHOT (RING3PIPE, B609): `-c <words…>` — no window
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
    let mut app = App::new((base + 0x5000) as *mut u8, win, face, false);
    app.hist_load(); // MIDDEN2: the history file's tail, before the first key
    app.rc(); // MIDDEN2: <home>/.middenrc — before the start line, so a fixture's first watched line is ours
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
                INPUT_EV_ACTION => app.action(input_ev_payload(ev) & 0xFF),
                INPUT_EV_WHEEL => app.wheel(input_ev_payload(ev) as u8 as i8),
                INPUT_EV_CLOSE_REQ => app.close(),
                _ => {}
            }
            k += 1;
        }
        if theme::due(now_ms()) && theme::load() {
            app.dirty = true; // UNAKEYS (B552): the store moved — repaint in the new look
            theme_wire();
        }
        app.pump(); // RING3PIPE (B609): what the launched programs wrote since the last frame
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
