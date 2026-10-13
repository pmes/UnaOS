// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAMETAL (LEDGER SR90): the bottom pane — MIDDEN.ELF's loop (crates/user-midden/src/main.rs, MIDDENTWIN SR75 +
// RING3EXEC B521), copied into this module, the crate not linked. CHARTER: Midden — shared-core midden_core.
//
// Every typed line goes through `midden_core::plan(line, &facts, &mut vol)` with MIDDEN.ELF's facts
// (`Facts::ring3(x86)` with `exec` on where SYS_SPAWN_PATH exists) and its Volume (SYS_STAT kind 0 on the
// cwd-resolved path). `Say` renders the core's text; `Host` verbs in `midden_core::ring3::SERVED` are fulfilled
// here through fs.rs; every other verb prints `ring3::refusal(verb)`; `run <path>` and a bare name launch through
// SYS_SPAWN_PATH. What differs from MIDDEN.ELF is only what a pane inside an IDE must differ in: the scrollback
// wraps at the pane's columns (set by the painter), the cwd starts at the workspace root, `exit`/`quit` ask the
// IDE to quit (returned as `Act::Quit`, the window is the IDE's), and the wire tag is `:: UNA: plan=`.
//
// UNAIDE2 (LEDGER SR111): `run` is the door the IDE's Run verb (Ctrl+R, main.rs) uses — a `.run` line or the typed
// line goes through the same plan. RING3PIPE (rmbp-ledger B562): a launch asks SYS_SPAWN_PATH for a pipe
// (`una_abi::SPAWN_PIPE`); the program still runs detached, and its console output comes back through
// `SYS_PIPE_READ(pid)` — `pump` (called once per frame by main.rs) drains every open pipe into this scrollback as the
// bytes arrive, one row per line, and closes it at EOF (the child exited and the ring is empty). A kernel that does
// not know the flag (it answers E2BIG to the high argc bits) gets the plain detached launch and the old note.
// Wire: `:: UNA: pipe pid=<p> bytes=<n> lines=<l> ::` per chunk read, `:: UNA: pipe eof ok=<0|1> pid=<p> lines=<n>
// bytes=<b> dropped=<d> rows=<r> pane=console ::` at EOF (ok = at least one line reached the pane and none dropped).
// `git` (`git status`, `git diff [path]`) is served here, read-only, through git.rs over git_core (SR59) at the
// workspace root; any other git word is refused by name. Wire `:: UNA: git verb=<status|diff> … ::`.

use crate::fs::{errno_text, list, read_file, resolve, spawn_path, stat, CAT_MAX};
use crate::Line;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use midden_core::{plan, ring3, Facts, Message, Plan, Volume};
use una_abi::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};
use vein_ring3::sys::sys;

/// The scrollback ring's byte budget: lines are dropped from the top once their text exceeds it.
const SCROLL_BYTES: usize = 8 * 1024;
const INP_CAP: usize = 240;
const HIST_CAP: usize = 32;
/// RING3PIPE: one SYS_PIPE_READ's buffer, and the reads one pipe gets per frame (a burst lands over a few frames).
const PIPE_CHUNK: usize = 4096;
const PIPE_READS: usize = 4;
/// RING3PIPE: a line longer than this with no newline is pushed as it stands (the pane wraps it).
const PIPE_LINE_MAX: usize = 1024;

/// RING3PIPE: a launched program whose output this pane is reading.
struct Piped {
    pid: i64,
    part: Vec<u8>,
    lines: u32,
    bytes: u64,
    dropped: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Out,
    Err,
    Echo,
}

/// What a key asks of the IDE.
pub enum Act {
    None,
    Quit,
}

struct R3Vol<'a> {
    cwd: &'a str,
}
impl Volume for R3Vol<'_> {
    fn is_file(&mut self, name: &str) -> bool {
        matches!(stat(&resolve(self.cwd, name)), Ok(s) if s.kind == 0)
    }
}

pub struct Console {
    pub rows: VecDeque<(Tone, String)>,
    bytes: usize,
    pub inp: Vec<u8>,
    pub caret: usize,
    hist: VecDeque<String>,
    hist_at: Option<usize>,
    pub cwd: String,
    /// UNAIDE2: the workspace root (where `git` looks for `.git`).
    root: String,
    facts: Facts,
    pub cmds: u32,
    pub last_kind: u32,
    pub last_verb: u8,
    /// The pane's text columns, set by the painter before any text is pushed.
    pub cols: usize,
    pub dirty: bool,
    /// RING3PIPE: the launched programs whose output is still arriving.
    pipes: Vec<Piped>,
}

impl Console {
    pub fn new(cwd: String, cols: usize) -> Console {
        Console {
            rows: VecDeque::new(),
            bytes: 0,
            inp: Vec::new(),
            caret: 0,
            hist: VecDeque::new(),
            hist_at: None,
            root: cwd.clone(),
            cwd,
            facts: Facts { exec: cfg!(target_arch = "x86_64"), ..Facts::ring3(cfg!(target_arch = "x86_64")) }, // RING3EXEC (B521): SYS_SPAWN_PATH is x86's
            cmds: 0,
            last_kind: 0,
            last_verb: 0,
            cols,
            dirty: true,
            pipes: Vec::new(),
        }
    }

    /// Push text into the scrollback: split on newlines, wrapped at the pane's columns, made printable.
    pub fn say(&mut self, tone: Tone, text: &str) {
        let cols = self.cols.max(8);
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

    /// UNAIDE2: run `line` as if typed (the IDE's Run verb).
    pub fn run(&mut self, line: &str) -> Act {
        self.dirty = true;
        self.run_line(line)
    }

    /// UNAIDE2: the typed line, taken out of the input (Ctrl+R runs it); `None` when nothing is typed.
    pub fn take_input(&mut self) -> Option<String> {
        if self.inp.iter().all(|b| *b == b' ') {
            return None;
        }
        let line = String::from_utf8_lossy(&self.inp).into_owned();
        self.inp.clear();
        self.caret = 0;
        self.hist_at = None;
        Some(line)
    }

    // ---- the plan, executed through the syscalls it names ----
    fn run_line(&mut self, line: &str) -> Act {
        let prompt = alloc::format!("{}> {}", self.cwd, line);
        self.say(Tone::Echo, &prompt);
        if !line.trim().is_empty() && self.hist.back().map(|s| s.as_str()) != Some(line) {
            self.hist.push_back(String::from(line));
            if self.hist.len() > HIST_CAP {
                self.hist.pop_front();
            }
        }
        let facts = self.facts;
        let word = midden_core::canon_verb(line.split_whitespace().next().unwrap_or(""));
        if word == "git" {
            // UNAIDE2: read-only git through git_core, served here.
            let rest: Vec<String> = line.split_whitespace().skip(1).map(String::from).collect();
            self.git(&rest);
            return Act::None;
        }
        if word == "run" && facts.exec {
            // RING3EXEC (B521): `run <path> [args]` — not a verb on `Facts::ring3` (no job table), served here.
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
            return Act::None;
        }
        let p = {
            let mut vol = R3Vol { cwd: &self.cwd };
            plan(line, &facts, &mut vol)
        };
        if let Plan::Say(_) = p {
            if ring3::CLOSES.contains(&word.as_str()) {
                return Act::Quit;
            }
        }
        let (kind, verb, served) = match &p {
            Plan::Nothing => (0, String::new(), true),
            Plan::Say(m) => {
                self.msg(m);
                if word == "help" {
                    self.say(Tone::Out, &alloc::format!("UNA.ELF console serves: {} (+ the core's own words above)", ring3::SERVED.join(" ")));
                    if facts.exec {
                        self.say(Tone::Out, "UNA.ELF console launches: run <path> [args], and bare names (detached; their output arrives here)");
                    }
                    self.say(Tone::Out, "keys: Ctrl+1 tree, Ctrl+2 editor, Ctrl+3 console (Esc cycles), Ctrl+O open, Ctrl+S save, Ctrl+Q quit");
                    self.say(Tone::Out, "tabs: Ctrl+T next, Ctrl+4..9 tab 1..6, Ctrl+W close; Ctrl+F find, Ctrl+L replace (Tab swaps, Ctrl+W whole-word, Ctrl+A all), Ctrl+G line");
                    self.say(Tone::Out, "tree: n new file (Ctrl+N anywhere), f new folder, r rename, Delete trash, g git diff; Ctrl+R runs the .run line or the typed line");
                    self.say(Tone::Out, "UNA.ELF console serves git (read-only): git status, git diff [path]");
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
                return Act::None;
            }
        };
        if kind != 0 {
            self.cmds += 1;
            self.last_kind = kind;
            self.last_verb = ring3::verb_code(&verb);
        }
        let mut l = Line::new(b":: UNA: plan=");
        l.put(ring3::kind_name(kind).as_bytes()).put(b" verb=").put(if verb.is_empty() { b"-" } else { verb.as_bytes() });
        l.put(b" served=").dec(served as i64).put(b" rows=").dec(self.rows.len() as i64).put(b" pane=console ::");
        l.wire();
        Act::None
    }

    fn launch(&mut self, verb: &str, typed: &str, found: Option<&str>, argv: &[&str]) -> Result<i64, i64> {
        let Some(name) = found else {
            self.say(Tone::Err, &alloc::format!("{}: {}: {}", verb, typed, errno_text(una_abi::ENOENT)));
            return Err(una_abi::ENOENT);
        };
        let path = resolve(&self.cwd, name);
        // RING3PIPE: ask for a pipe; a kernel without the flag answers E2BIG to the high argc bits — launch plainly.
        let mut piped = true;
        let mut r = spawn_piped(&path, argv);
        if r == una_abi::E2BIG {
            piped = false;
            r = spawn_path(&path, argv);
        }
        if r > 0 {
            self.say(Tone::Out, &alloc::format!("{}: {} started - pid {}", verb, path, r));
            if piped {
                self.pipes.push(Piped { pid: r, part: Vec::new(), lines: 0, bytes: 0, dropped: 0 });
            } else {
                self.say(Tone::Out, "(detached: this kernel has no pipe for its output)");
            }
            Ok(r)
        } else {
            self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, path, r, errno_text(r)));
            Err(r)
        }
    }

    /// RING3PIPE: drain every open pipe into the scrollback (called once per frame). True when a row was added.
    pub fn pump(&mut self) -> bool {
        if self.pipes.is_empty() {
            return false;
        }
        let mut added = false;
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
                    added |= lines > 0;
                    let mut l = Line::new(b":: UNA: pipe pid=");
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
                    added = true;
                    let ok = r == 0 && p.lines > 0 && p.dropped == 0;
                    let mut l = Line::new(b":: UNA: pipe eof ok=");
                    l.dec(ok as i64).put(b" pid=").dec(pid).put(b" lines=").dec(p.lines as i64).put(b" bytes=").dec(p.bytes as i64);
                    l.put(b" dropped=").dec(p.dropped as i64).put(b" rows=").dec(self.rows.len() as i64);
                    if r != 0 {
                        l.put(b" err=").dec(r);
                    }
                    l.put(b" pane=console ::");
                    l.wire();
                }
            }
        }
        added
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

    /// `:: UNA: plan=Exec verb=<v> served=1 pid=<n> ::` or `served=0 err=<errno>`.
    fn exec_done(&mut self, verb: &str, r: Result<i64, i64>) {
        self.cmds += 1;
        self.last_kind = 3;
        self.last_verb = ring3::verb_code(verb);
        let mut l = Line::new(b":: UNA: plan=Exec verb=");
        l.put(verb.as_bytes());
        match r {
            Ok(pid) => l.put(b" served=1 pid=").dec(pid),
            Err(e) => l.put(b" served=0 err=").dec(e),
        };
        l.put(b" pane=console ::");
        l.wire();
    }

    /// UNAIDE2: `git [status]` / `git diff [path]` — read-only.
    fn git(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            None | Some("status") => self.git_status(),
            Some("diff") => match args.get(1) {
                Some(p) => {
                    let p = resolve(&self.cwd, p);
                    self.git_diff(&p);
                }
                None => {
                    let Some(repo) = self.repo() else { return };
                    let st = repo.status();
                    if st.modified.is_empty() {
                        self.say(Tone::Out, "git diff: no modified files");
                    }
                    for rel in st.modified.iter().take(20) {
                        let p = if repo.root == "/" { alloc::format!("/{}", rel) } else { alloc::format!("{}/{}", repo.root, rel) };
                        self.diff_rows(&repo, &p);
                    }
                }
            },
            Some(other) => {
                let m = alloc::format!("git {}: refused - UNA.ELF's git is read-only (status, diff)", other);
                self.say(Tone::Err, &m);
            }
        }
    }

    fn repo(&mut self) -> Option<crate::git::Repo> {
        match crate::git::open(&self.root) {
            Ok(Some(r)) => Some(r),
            Ok(None) => {
                let m = alloc::format!("git: no .git under {}", self.root);
                self.say(Tone::Err, &m);
                None
            }
            Err(e) => {
                let m = alloc::format!("git: {}", e);
                self.say(Tone::Err, &m);
                None
            }
        }
    }

    fn git_status(&mut self) {
        let Some(repo) = self.repo() else { return };
        let st = repo.status();
        let head = alloc::format!("git {} at {} ({} index entries){}", repo.hk_name(), repo.gitdir, repo.entries(), if st.truncated { " - walk truncated" } else { "" });
        self.say(Tone::Out, &head);
        for (tag, tone, v) in [("modified:  ", Tone::Echo, &st.modified), ("deleted:   ", Tone::Err, &st.deleted), ("untracked: ", Tone::Out, &st.untracked)] {
            for p in v.iter() {
                let row = alloc::format!("  {}{}", tag, p);
                self.say(tone, &row);
            }
        }
        if st.modified.is_empty() && st.deleted.is_empty() && st.untracked.is_empty() {
            self.say(Tone::Out, "nothing to commit, working tree clean");
        }
        let mut l = Line::new(b":: UNA: git verb=status modified=");
        l.dec(st.modified.len() as i64).put(b" untracked=").dec(st.untracked.len() as i64).put(b" deleted=").dec(st.deleted.len() as i64);
        l.put(b" truncated=").dec(st.truncated as i64).put(b" rc=0 ::");
        l.wire();
    }

    /// UNAIDE2: `path`'s diff against the index, into the scrollback (the tree's `g`).
    pub fn git_diff(&mut self, path: &str) {
        let Some(repo) = self.repo() else { return };
        self.diff_rows(&repo, path);
    }

    fn diff_rows(&mut self, repo: &crate::git::Repo, path: &str) {
        let (rc, lines) = match repo.diff(path) {
            Ok(d) if d.is_empty() => {
                let m = alloc::format!("git diff {}: no changes", path);
                self.say(Tone::Out, &m);
                (0, 0)
            }
            Ok(d) => {
                let text = String::from_utf8_lossy(&d).into_owned();
                let mut n = 0;
                for l in text.lines() {
                    let tone = if l.starts_with('+') && !l.starts_with("+++") {
                        Tone::Echo
                    } else if l.starts_with('-') && !l.starts_with("---") {
                        Tone::Err
                    } else {
                        Tone::Out
                    };
                    self.say(tone, l);
                    n += 1;
                }
                (0, n)
            }
            Err(e) => {
                let m = alloc::format!("git diff {}: {}", path, e);
                self.say(Tone::Err, &m);
                (-1, 0)
            }
        };
        let mut l = Line::new(b":: UNA: git verb=diff path=");
        l.put(path.as_bytes()).put(b" lines=").dec(lines as i64).put(b" rc=").dec(rc).put(b" ::");
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
                match read_file(&target, CAT_MAX, false) {
                    Ok(b) => {
                        let text = String::from_utf8_lossy(&b).into_owned();
                        self.say(Tone::Out, text.trim_end_matches('\n'));
                    }
                    Err(e) => self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, target, e, errno_text(e))),
                }
            }
            other => {
                let r = ring3::refusal(other);
                self.msg(&r);
            }
        }
    }

    // ---- keys (the pane has focus) ----
    pub fn key(&mut self, k: u8) -> Act {
        let mut act = Act::None;
        match k {
            b'\n' | b'\r' => {
                let line = String::from_utf8_lossy(&self.inp).into_owned();
                self.inp.clear();
                self.caret = 0;
                self.hist_at = None;
                act = self.run_line(&line);
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
                    return act;
                }
                let n = self.hist.len();
                let at = match (self.hist_at, k == KEY_UP) {
                    (None, true) => Some(n - 1),
                    (None, false) => None,
                    (Some(i), true) => Some(i.saturating_sub(1)),
                    (Some(i), false) => {
                        if i + 1 < n {
                            Some(i + 1)
                        } else {
                            None
                        }
                    }
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
            _ => return act,
        }
        self.dirty = true;
        act
    }
}

/// RING3PIPE (B562): `SYS_SPAWN_PATH` with `SPAWN_PIPE` on argc's high half — the pid names the child's output pipe.
fn spawn_piped(path: &str, argv: &[&str]) -> i64 {
    let descs: Vec<una_abi::SpawnArg> = argv.iter().map(|a| una_abi::SpawnArg { ptr: a.as_ptr() as u64, len: a.len() as u64 }).collect();
    sys(una_abi::SYS_SPAWN_PATH, path.as_ptr() as u64, path.len() as u64, descs.as_ptr() as u64, descs.len() as u64 | una_abi::SPAWN_PIPE)
}
