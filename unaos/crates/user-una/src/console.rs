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

use crate::fs::{errno_text, list, read_file, resolve, spawn_path, stat, CAT_MAX};
use crate::Line;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use midden_core::{plan, ring3, Facts, Message, Plan, Volume};
use una_abi::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};

/// The scrollback ring's byte budget: lines are dropped from the top once their text exceeds it.
const SCROLL_BYTES: usize = 8 * 1024;
const INP_CAP: usize = 240;
const HIST_CAP: usize = 32;

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
    facts: Facts,
    pub cmds: u32,
    pub last_kind: u32,
    pub last_verb: u8,
    /// The pane's text columns, set by the painter before any text is pushed.
    pub cols: usize,
    pub dirty: bool,
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
            cwd,
            facts: Facts { exec: cfg!(target_arch = "x86_64"), ..Facts::ring3(cfg!(target_arch = "x86_64")) }, // RING3EXEC (B521): SYS_SPAWN_PATH is x86's
            cmds: 0,
            last_kind: 0,
            last_verb: 0,
            cols,
            dirty: true,
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
                        self.say(Tone::Out, "UNA.ELF console launches: run <path> [args], and bare names (detached)");
                    }
                    self.say(Tone::Out, "keys: Ctrl+1 tree, Ctrl+2 editor, Ctrl+3 console (Esc cycles), Ctrl+O open, Ctrl+S save, Ctrl+Q quit");
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
        let r = spawn_path(&path, argv);
        if r > 0 {
            self.say(Tone::Out, &alloc::format!("{}: {} started - pid {}", verb, path, r));
            Ok(r)
        } else {
            self.say(Tone::Err, &alloc::format!("{}: {}: {} {}", verb, path, r, errno_text(r)));
            Err(r)
        }
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
