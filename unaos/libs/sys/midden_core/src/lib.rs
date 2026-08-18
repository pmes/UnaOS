// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! # midden_core — the one interpreter
//!
//! UnaOS had **two** shell interpreters: `handlers/midden` (Ring 3, host, std +
//! GTK) and `unaos/crates/kernel/src/shell.rs`'s `dispatch_command` (Ring 0, a
//! 1300-line `match`). They shared no code, no command table, and no notion of
//! what a word means. This crate is the seam that ends that: **the command
//! table, the parser, the bare-name resolver and the text of the verbs the
//! shell itself owns live here, once, `no_std`.**
//!
//! The convention is the tree's own (`docs/dev/USERLAND/ARCHITECTURE.md` §1):
//! Ring-0-embeddable cores live under `unaos/libs/` — `no_std`,
//! `forbid(unsafe_code)`, members of the root host workspace, pulled by the
//! kernel with `default-features = false`. `fs/unafs` is the format core both
//! rings share; `sys/helm` is the interlock core; `sys/midden_core` is the
//! **shell** core. `sys/` rather than a device-class dir for the same reason
//! helm is there: it is system authority, not a device class.
//!
//! ## What the core decides, and what the ring does
//!
//! The core never touches hardware, a filesystem, or a screen. It answers one
//! question — *what does this line mean?* — and returns a [`Plan`]:
//!
//! | Plan | Meaning | Who acts |
//! | --- | --- | --- |
//! | [`Plan::Nothing`] | empty line | nobody |
//! | [`Plan::Say`] | the core answered it in full | the ring renders the [`Message`] |
//! | [`Plan::Exec`] | a bare name resolved to a program on disk | the ring loads and runs it |
//! | [`Plan::Host`] | a verb the core knows but only the ring can service | the ring's service layer |
//!
//! `Plan::Host` is the honest statement of the M1 boundary: the kernel's
//! `match` still *performs* `ls`/`cat`/`ping`/`storm`, but it no longer
//! *decides* — a word is a verb because [`is_verb`] says so, and a word that is
//! not a verb becomes a program or a refusal by [`resolve_exec`], never by a
//! `_ =>` arm inventing policy. There is exactly one command table and it is
//! midden's.
//!
//! ## The message shape
//!
//! [`Message`] mirrors, variant for variant, the terminal arm of
//! `bandy::SMessage` (`NoOp` / `TerminalOutput` / `TerminalError` /
//! `FileSystemEvent`). It is a separate type only because `bandy` is `std` +
//! Tokio and cannot be linked into Ring 0; a host caller converts in four
//! lines. When userspace converges (see
//! `docs/dev/USERLAND/MIDDEN_CONVERGENCE.md` §4) this type is the thing that
//! collapses into `SMessage` itself.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// The ring-neutral twin of `bandy::SMessage`'s terminal arm.
///
/// Same four variants, same meanings, no `std` and no Tokio. A host caller maps
/// it onto `SMessage` directly; Ring 0 renders it to the framebuffer `Console`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Nothing to say (an empty line).
    NoOp,
    /// Ordinary terminal output.
    TerminalOutput(String),
    /// Terminal output that reports a failure (a refusal, a usage line).
    TerminalError(String),
    /// A filesystem intent was expressed (`touch`), for the bus to observe.
    FileSystemEvent(String),
}

impl Message {
    /// The variant name, for witnesses and logs. Stable text — a bench capture
    /// greps it.
    pub fn kind(&self) -> &'static str {
        match self {
            Message::NoOp => "NoOp",
            Message::TerminalOutput(_) => "TerminalOutput",
            Message::TerminalError(_) => "TerminalError",
            Message::FileSystemEvent(_) => "FileSystemEvent",
        }
    }

    /// The carried text (empty for [`Message::NoOp`]).
    pub fn text(&self) -> &str {
        match self {
            Message::NoOp => "",
            Message::TerminalOutput(s) | Message::TerminalError(s) | Message::FileSystemEvent(s) => s,
        }
    }

    /// Length in bytes of the carried text — the witness's `len=` field.
    pub fn len(&self) -> usize {
        self.text().len()
    }

    /// `true` when there is nothing to render.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Build facts
// ---------------------------------------------------------------------------

/// What the *hosting build* can actually do.
///
/// The core is compiled once and must not carry `#[cfg(target_arch)]` — a shell
/// core that only tells the truth on one arch is not a shared core. So the
/// facts a command table needs are passed IN, and the ring builds them with
/// `cfg!()` at its single call site. A help line that advertises a verb this
/// build cannot dispatch is worse than no help line at all (the rule
/// `shell.rs` already followed for `pulse`), and this struct is how that rule
/// survives the move out of the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// aarch64 build (the Pi / Orin verbs: `uls`, `top`, `pulse`, `vug`).
    pub aarch64: bool,
    /// x86_64 build.
    pub x86: bool,
    /// aarch64 + the `v3d` feature (the `APPS:` line).
    pub v3d: bool,
    /// The process verbs (`run`/`bg`/`jobs`/`kill`/`storm`) are dispatchable:
    /// aarch64-baremetal or x86.
    pub proc_verbs: bool,
    /// `MAX_PROCS` for this build — the storm cap, read from the process table
    /// rather than written down (HEADROOM split it 10/6 per arch).
    pub proc_rows: usize,
    /// Bare-name program launch is wired on this build. When false the core
    /// never returns [`Plan::Exec`] and never probes the volume, so a build
    /// without a loader behaves exactly as it did before this crate existed.
    pub exec: bool,
    /// The in-kernel demo verbs (`vug`, `pulse`) are compiled into this build.
    ///
    /// BARENAME (PARITY §6.6a): a KNOB, and it has to be one. Those two `match`
    /// arms carry `feature = "vugdemo"`, **DEFAULT OFF** (DECRUD-1), while this
    /// table advertised them on every aarch64 build. So on the image users
    /// actually flash, `vug` was claimed as a verb, routed to a `match` with no
    /// such arm, and answered "the verb exists; this kernel does not carry it".
    /// Worse: verb-ness is absolute over bare-name launch, so that phantom verb
    /// was ALSO what stood between the operator and `VUG.ELF` — turning on
    /// [`Facts::exec`] alone would have changed nothing for the one word Peter
    /// typed. A verb the build does not carry is not a verb.
    pub vugdemo: bool,
}

impl Facts {
    /// A conservative all-off fact set: no arch verbs, no process verbs, no
    /// exec. Used by tests and by any caller that only wants the parser.
    pub const fn bare() -> Self {
        Facts {
            aarch64: false,
            x86: false,
            v3d: false,
            proc_verbs: false,
            proc_rows: 0,
            exec: false,
            vugdemo: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The command table
// ---------------------------------------------------------------------------

/// Verbs the core answers **in full** — it needs nothing from the ring.
///
/// These are the shell's own words: what it is, what it knows, and echoing back
/// what it was told. Everything else needs a filesystem, a NIC, a scheduler or
/// a screen, and is therefore [`Plan::Host`].
pub const CORE_VERBS: &[&str] = &["help", "echo", "ver", "version", "gneiss"];

/// When a host verb exists.
///
/// Verb-ness is **not** a global fact, and getting that wrong is the one way
/// this refactor could have broken the shell: `vug` is a registered verb only
/// on a `vugdemo` build, and everywhere else the same word must fall through to
/// bare-name launch and start `VUG.ELF`. A flat name list would have turned
/// that launch into a refusal. So each host verb carries the condition under
/// which the ring actually registers it, and those conditions mirror the
/// `#[cfg]` on the kernel's match arms one for one — **one for one is the whole
/// contract**, and PARITY §6.6a is what it costs when a condition here is
/// looser than the `#[cfg]` it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avail {
    /// Registered on every build.
    Always,
    /// aarch64 only (the native unafs verbs).
    Aarch64,
    /// aarch64 + `v3d` (the GPU battery replay).
    V3d,
    /// aarch64 + `vugdemo` (the in-kernel `vug` / `pulse` demos, DEFAULT OFF).
    ///
    /// BARENAME (PARITY §6.6a): these two were `Aarch64`, which over-claimed
    /// them on every Pi build — see [`Facts::vugdemo`] for what that cost. The
    /// arch half stays because the demos draw through `crate::vug`, which is
    /// aarch64-only source; the knob half is the part that was missing.
    VugDemo,
    /// The process verbs: aarch64-baremetal or x86.
    Proc,
}

impl Avail {
    /// Is a verb with this availability registered on the described build?
    pub const fn on(self, facts: &Facts) -> bool {
        match self {
            Avail::Always => true,
            Avail::Aarch64 => facts.aarch64,
            Avail::V3d => facts.v3d,
            Avail::VugDemo => facts.aarch64 && facts.vugdemo,
            Avail::Proc => facts.proc_verbs,
        }
    }
}

/// Verbs the core recognises but only the ring can service, each with the
/// condition under which the ring registers it.
///
/// This is the kernel's historical `match` arm list, lifted with its `#[cfg]`s
/// intact, so the table is the *same* table. Membership here (as filtered by
/// [`Avail::on`]) is what makes a word a verb; `dispatch_command` no longer
/// decides that.
pub const HOST_VERBS: &[(&str, Avail)] = &[
    // shell / clock
    ("clear", Avail::Always), ("panic", Avail::Always), ("date", Avail::Always),
    ("setdate", Avail::Always), ("time", Avail::Always), ("uptime", Avail::Always),
    // FAT + VFS surface
    ("ls", Avail::Always), ("dir", Avail::Always), ("cd", Avail::Always),
    ("pwd", Avail::Always), ("cat", Avail::Always), ("type", Avail::Always),
    ("head", Avail::Always), ("tail", Avail::Always), ("find", Avail::Always),
    ("du", Avail::Always), ("stat", Avail::Always), ("xd", Avail::Always),
    ("touch", Avail::Always), ("append", Avail::Always), ("rm", Avail::Always),
    ("del", Avail::Always), ("mkdir", Avail::Always), ("md", Avail::Always),
    ("rmdir", Avail::Always), ("rd", Avail::Always), ("vfs", Avail::Always),
    ("cp", Avail::Always), ("copy", Avail::Always), ("mv", Avail::Always),
    ("move", Avail::Always), ("ren", Avail::Always), ("rename", Avail::Always),
    ("sync", Avail::Always), ("write", Avail::Always),
    // native unafs — aarch64 only
    ("uls", Avail::Aarch64), ("ucat", Avail::Aarch64), ("utouch", Avail::Aarch64),
    ("uwrite", Avail::Aarch64), ("umkdir", Avail::Aarch64), ("urm", Avail::Aarch64),
    ("usnaps", Avail::Aarch64), ("usnap", Avail::Aarch64), ("usnapdrop", Avail::Aarch64),
    ("usnapls", Avail::Aarch64), ("usnapcat", Avail::Aarch64),
    // block / device
    ("diskinfo", Avail::Always), ("usbinfo", Avail::Always), ("fatinfo", Avail::Always),
    ("read", Avail::Always),
    // network
    ("netinfo", Avail::Always), ("ping", Avail::Always), ("arp", Avail::Always),
    ("connect", Avail::Always), ("udpsend", Avail::Always), ("get", Avail::Always),
    // apps + scheduler + power + witnesses
    ("vug", Avail::VugDemo), ("pulse", Avail::VugDemo), ("v3d", Avail::V3d),
    ("tste", Avail::Always), ("selftest", Avail::Always), ("sched", Avail::Always),
    ("ps", Avail::Always), ("top", Avail::Always), ("batmon", Avail::Always),
    ("bootlog", Avail::Always), ("shutdown", Avail::Always), ("off", Avail::Always),
    // processes
    ("run", Avail::Proc), ("bg", Avail::Proc), ("storm", Avail::Proc),
    ("jobs", Avail::Proc), ("kill", Avail::Proc),
];

/// The canonical (lower-case) spelling of a typed word.
///
/// **Verb lookup is case-INSENSITIVE, and that is a security property, not a
/// convenience.** The resolver underneath it always was: the kernel's FAT walk
/// matches components with `eq_ignore_ascii_case`, so `resolve_exec("LS")`
/// probes `LS.elf` and the volume happily answers with `LS.ELF`. If `is_verb`
/// had stayed case-SENSITIVE, then typing `LS` — with caps lock on, or simply
/// mirroring the 8.3 spelling `ls` itself prints — would have missed the verb
/// table and **launched a dropped `LS.ELF` instead of listing the directory**.
/// The same held for `RM`, `Cat`, `Kill`, `Write` and the caps variant of every
/// other verb. §5's rule ("a shell where a dropped file can shadow `ls` is a
/// security problem") is only true if it holds for every spelling of the word,
/// so the table is consulted through this function and never against the raw
/// token.
///
/// ASCII only, deliberately: `to_ascii_lowercase` cannot change a token's
/// length or invent characters the user did not type (Unicode case folding can
/// — `İ` lower-cases to two chars). Verbs are ASCII words; a non-ASCII token is
/// left exactly as typed and is therefore not a verb, which is correct.
pub fn canon_verb(word: &str) -> String {
    word.to_ascii_lowercase()
}

/// `true` iff `word` is a shell verb **on this build** — core-answered or
/// ring-serviced.
///
/// The single authority on "is this a command?". `handlers/midden`, the kernel,
/// a completion engine and a help renderer all ask this one function. A word
/// that is a verb somewhere else but not here is honestly NOT a verb here, and
/// is free to become a program name — which is exactly how `vug` starts
/// `VUG.ELF` on the shipped build of either arch while staying the in-kernel 3D
/// sculptor verb on a `vugdemo` one.
///
/// Case-insensitive via [`canon_verb`]: `LS`, `Ls` and `lS` are all the verb
/// `ls`.
pub fn is_verb(word: &str, facts: &Facts) -> bool {
    let w = canon_verb(word);
    let w: &str = &w;
    CORE_VERBS.contains(&w) || HOST_VERBS.iter().any(|(n, a)| *n == w && a.on(facts))
}

// ---------------------------------------------------------------------------
// Bare-name resolution (the `.elf` question)
// ---------------------------------------------------------------------------

/// Extensions a bare name may be wearing without the user typing them.
///
/// Peter, 2026-08-08: *"we should also do like other OSes and not make people
/// type `.elf` … but i do like the `.elf` and extensions help distinguish
/// listings in the shell."* Both halves are honoured by making the elision a
/// property of **resolution**, never of storage: the file on disk is and stays
/// `STAT.ELF`, `ls` prints `STAT.ELF`, and only the *lookup* is permitted to
/// try the suffix. Nothing renames, nothing hides. See
/// `docs/dev/USERLAND/MIDDEN_CONVERGENCE.md` §5.
pub const EXEC_EXTS: &[&str] = &["elf"];

/// The ring's view of the volume, reduced to the one question resolution asks.
///
/// Deliberately not a filesystem trait: the core must not learn about
/// directories, handles, mounts or errors. `is_file(name)` is true iff `name`
/// names a regular (non-directory) file reachable from the shell's cwd. The
/// kernel implements it over FAT; a host implements it over `std::fs`; a test
/// implements it over a slice of names.
pub trait Volume {
    /// Does `name` resolve to a regular file right now?
    fn is_file(&mut self, name: &str) -> bool;
}

/// A [`Volume`] over a fixed list of names — the test/fixture implementation.
///
/// Present in the core (not behind `cfg(test)`) because the kernel's boot
/// witness uses it: the fixture must prove the *same* resolver the live path
/// uses, on an arch whose live path does not exist yet.
pub struct NameList<'a>(pub &'a [&'a str]);

impl Volume for NameList<'_> {
    fn is_file(&mut self, name: &str) -> bool {
        self.0.iter().any(|n| *n == name)
    }
}

/// Resolve a bare token to the on-disk spelling of a program, or `None`.
///
/// Order — first hit wins, and the order is the whole design:
///
/// 1. **Exact match.** `VUG.ELF` typed in full is `VUG.ELF`. A user who typed a
///    real name always gets that name; elision can never steal from an exact
///    hit, so no file becomes unreachable by its own spelling.
/// 2. **Extension-elided, as typed.** `vug` → `vug.elf`.
/// 3. **Extension-elided, upper-cased.** `vug` → `VUG.ELF`. This arm is **dead
///    on the kernel today and is not the one that fires on the boot volume** —
///    the x86 `Volume` impl walks FAT with `eq_ignore_ascii_case`, so arm 2's
///    probe for `vug.elf` already MATCHES the on-disk `VUG.ELF` and returns the
///    string `"vug.elf"`. Arm 3 exists for two callers that are not that one:
///    the boot fixture's `NameList`, which compares names exactly, and any
///    future case-SENSITIVE `Volume` (a UnaFS mount, a host `std::fs` backend).
///    Keeping it costs one probe and removes a silent behaviour difference
///    between those backends; it is documented as latent rather than live so a
///    gate is never written against a spelling the kernel does not produce.
///
/// Only the LAST path component is elided; a token containing `/` keeps its
/// directory part (`DOCS/vug` → `DOCS/vug.elf`, never `DOCS.ELF/vug`).
///
/// **Verbs are not consulted here.** `resolve_exec` is reached only after
/// [`is_verb`] said no, which preserves the standing precedence rule (a verb
/// always wins) and its consequence: a program whose stem collides with a verb
/// — `STAT.ELF` vs. the `stat` verb — is reachable as `STAT.ELF`, `stat.elf`,
/// or `./STAT.ELF`, but a bare `stat` stays the verb. That collision is real
/// and is written up rather than papered over; see MIDDEN_CONVERGENCE §5.
pub fn resolve_exec(token: &str, vol: &mut dyn Volume) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    if vol.is_file(token) {
        return Some(token.to_string());
    }
    // Split off the directory part so elision only ever touches the leaf.
    let (dir, leaf) = match token.rfind('/') {
        Some(i) => (&token[..=i], &token[i + 1..]),
        None => ("", token),
    };
    if leaf.is_empty() || leaf.contains('.') {
        // A leaf that already carries an extension is not a bare name; if the
        // exact probe above missed it, it simply does not exist. This is what
        // keeps `vug.txt` from silently launching `VUG.ELF`.
        return None;
    }
    for ext in EXEC_EXTS {
        let as_typed = format!("{}{}.{}", dir, leaf, ext);
        if vol.is_file(&as_typed) {
            return Some(as_typed);
        }
        // ASCII case only. `str::to_uppercase` is UNICODE: `ﬁ` becomes `FI`,
        // `ß` becomes `SS`, `ı` becomes `I` — each of them a name of a
        // different LENGTH than the one the user typed, probed against a
        // volume that never contained it. A filename transform must not invent
        // characters, so this is the ASCII twin.
        let upper = format!("{}{}.{}", dir, leaf.to_ascii_uppercase(), ext.to_ascii_uppercase());
        if upper != as_typed && vol.is_file(&upper) {
            return Some(upper);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// What the shell decided a line means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The line was empty (or whitespace). Nothing happens, nothing is printed.
    Nothing,
    /// The core answered in full; the ring renders this message and is done.
    Say(Message),
    /// A bare name resolved to a program. `typed` is what the user wrote,
    /// `name` is the on-disk spelling to load.
    Exec { typed: String, name: String },
    /// A verb only the ring can service. `verb` is the first word; `rest` is
    /// the remainder of the line, untouched, for the ring to re-split exactly
    /// as it always has.
    Host { verb: String, rest: String },
}

/// Parse and decide. **The** entry point — one line in, one [`Plan`] out.
///
/// `vol` is consulted only when `facts.exec` is set and the word was not a
/// verb, so a build without a program loader performs no filesystem work at
/// all and behaves byte-identically to the pre-core shell.
pub fn plan(line: &str, facts: &Facts, vol: &mut dyn Volume) -> Plan {
    let trimmed = line.trim();
    let mut it = trimmed.split_whitespace();
    let Some(word) = it.next() else {
        return Plan::Nothing;
    };
    let rest = trimmed[word.len()..].trim_start();
    let args: Vec<&str> = rest.split_whitespace().collect();

    // CASE. The table is consulted through `canon_verb`, never against the raw
    // token — see that function for why this is a security property and not a
    // nicety. The CANONICAL spelling is what leaves in `Plan::Host`, because the
    // ring's service `match` arms are lower-case literals: handing it back a raw
    // `LS` would miss every arm and land in `other =>`, printing "the verb
    // exists; this kernel does not carry it" about a verb this kernel carries.
    // `Plan::Exec` is unaffected — `typed` and the resolver both keep the user's
    // own spelling, because a FILE name is not a verb name.
    let canon = canon_verb(word);
    let canon: &str = &canon;

    if CORE_VERBS.contains(&canon) {
        return Plan::Say(answer(canon, &args, facts));
    }
    if HOST_VERBS.iter().any(|(n, a)| *n == canon && a.on(facts)) {
        return Plan::Host { verb: canon.to_string(), rest: rest.to_string() };
    }
    if facts.exec {
        if let Some(name) = resolve_exec(word, vol) {
            return Plan::Exec { typed: word.to_string(), name };
        }
    }
    Plan::Say(Message::TerminalError(
        "Unknown command. Type 'help' for assistance.".to_string(),
    ))
}

/// The verbs the core owns outright.
fn answer(word: &str, args: &[&str], facts: &Facts) -> Message {
    match word {
        "ver" | "version" => Message::TerminalOutput(
            "unaOS v0.1.0 (Kernel: Jules 1 / Cortex: Jules 6)".to_string(),
        ),
        "gneiss" => Message::TerminalOutput("Gneiss is Home.".to_string()),
        "echo" => Message::TerminalOutput(args.join(" ")),
        "help" => Message::TerminalOutput(help(facts)),
        // `plan` only routes CORE_VERBS here, so this arm is unreachable; it
        // exists so adding a name to CORE_VERBS without an arm is a visible
        // refusal rather than a silent nothing.
        other => Message::TerminalError(format!("{}: recognised but unanswered", other)),
    }
}

/// The help text: the command table describing itself.
///
/// Returned as ONE `\n`-joined string (the ring splits and prints line by line,
/// which is exactly what the kernel's `Console::println` loop did). Every
/// conditional line keys off [`Facts`], never off `cfg!` — the core is compiled
/// once for both rings and both arches.
pub fn help(facts: &Facts) -> String {
    let mut l: Vec<String> = Vec::new();
    macro_rules! say { ($s:expr) => { l.push(($s).to_string()) }; }
    say!("COMMANDS: ver, help, clear, echo, panic, gneiss");
    say!("STORAGE:  diskinfo, usbinfo, read <lba>, write <lba> <byte>");
    say!("FILES:    fatinfo (FAT geometry), ls [-l] [dir], cd [dir], pwd, cat <path>");
    say!("PAGING:   head <path> [n], tail <path> [n]  (first / last n lines, default 10)");
    say!("WRITE:    touch <path>, write <path> <text>, append <path> <text>, rm [-r] [-f] <path>");
    say!("DIRS:     mkdir <path>, rmdir <path>  (create / remove empty directories)");
    say!("VFS:      vfs write|append|rm|mkdir <path> [text]  (unified namespace: / native, /fat FAT)");
    say!("          rm -r <dir>  (recursively delete a directory tree; root refused)");
    say!("COPY:     cp [-f] <src> <dst>  (copy a file; cp FILE DIR/ lands as DIR/<leaf>)");
    say!("          cp -r <srcdir> <dst>  (recursively copy a directory tree)");
    say!("MOVE:     mv [-f] <src...> <dst>  (move/rename a file or dir, O(1); mv SRC DIR/ lands as DIR/<leaf>)");
    say!("FLAGS:    default is no-clobber; -f = force overwrite (rm: quiet on missing), -n = no-clobber");
    say!("WILDCARD: * / ? in the last path component — ls/cat/rm/cp/mv expand it (e.g. rm *.TMP, cp *.TXT DOCS/)");
    say!("          (create/edit/delete/copy/move files & dirs anywhere in the tree; sync = write-through, durable)");
    say!("TREE:     find [root] <pattern>  (recursive glob search), du [dir]  (recursive size tally)");
    say!("          uptime  (seconds since boot; shows the wall clock when set)");
    say!("INSPECT:  stat <path>  (full on-disk detail), xd <path> [off] [len]  (bounded hexdump)");
    if facts.aarch64 {
        say!("UNAFS:    uls [path], ucat <path>  (native unafs volume, absolute paths)");
        say!("          utouch <path>, uwrite <path> <text>, umkdir <path>, urm <path>  (write-through)");
    }
    say!("          usnaps, usnap <name>, usnapdrop <gen>  (retained roots / snapshots)");
    if facts.aarch64 {
        say!("          usnapls <gen> [path], usnapcat <gen> <path>  (read a snapshot; current-ACL enforced)");
    }
    say!("CLOCK:    date, setdate YYYY-MM-DD HH:MM[:SS]  (seeds mtime stamps; unset = honest dashes)");
    say!("          time  (shared civil clock: ISO-8601 UTC + source; unsynced until SNTP/setdate)");
    // BARENAME (PARITY §6.6a): `vug` and `pulse` are the in-kernel DEMOS, and they are verbs only
    // on a `vugdemo` build. Off the knob — which is every image users flash — the same two words
    // name the ring-3 programs `VUG.ELF` / `PULSE.ELF` and belong to the bare-name lines below, so
    // advertising them here would be advertising a verb this build cannot dispatch: the exact rule
    // the `Facts` doc opens with.
    if facts.aarch64 {
        if facts.vugdemo {
            say!("SMP:      sched (per-CPU run queues), pulse (full-screen CPU monitor)");
        } else {
            say!("SMP:      sched (per-CPU run queues)");
        }
        say!("          top  (per-core load: recent busy%, ctx-switches, last task)");
    } else {
        say!("SMP:      sched (per-CPU run queues)");
    }
    if facts.v3d && facts.vugdemo {
        say!("APPS:     vug (3D sculptor), v3d (replay the visible GPU graphics battery)");
    } else if facts.v3d {
        say!("APPS:     v3d (replay the visible GPU graphics battery)");
    }
    if facts.proc_verbs {
        say!("PROC:     run <path> (foreground), bg <path> (background), jobs (list+reap), kill <pid>");
    }
    // BARENAME (PARITY §6.6a): keyed off `exec`, the fact these two lines actually describe, not
    // off `x86`, the arch that happened to be the only one carrying it. A help line that names a
    // capability must be gated on the capability.
    if facts.exec {
        say!("          <name.elf>  (just type it: vug.elf runs VUG.ELF in a window, prompt returns)");
        // BARE-NAME (M1): the elision is a resolution rule, so it is stated
        // where the launch rule is stated. `.elf` stays visible in `ls`.
        say!("          <name>      (the .elf is optional: vug finds VUG.ELF; ls still shows VUG.ELF)");
    }
    if facts.proc_verbs {
        say!(format!(
            "          storm [n]  (launch n vugs; n>{} is refused by the process table — serial carries the [storm] census)",
            facts.proc_rows
        ));
    }
    say!("POWER:    batmon (SMC battery snapshot; x86 UNAOS_SMC=1 only)");
    say!("WITNESS:  bootlog (boot-milestone ring: PORTSW / FTDI console / EHCI HID / block / GUI handoff)");
    say!("TEST:     tste (in-OS self-test suite: boot-replay + live checks)");
    say!("NETWORK:  netinfo, ping <ip> [count], arp <ip>");
    say!("          connect <ip> <port> [message], udpsend <ip> <port> [message]");
    say!("          get <ip> [port] [path]  (HTTP/1.0 GET)");
    l.join("\n")
}

// ---------------------------------------------------------------------------
// Tests (host `cargo test -p midden_core`; the kernel runs the same assertions
// through `shell::midden_witness`, which is why the fixture Volume is public).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn vol() -> NameList<'static> {
        NameList(&["VUG.ELF", "STAT.ELF", "README.TXT", "lower.elf"])
    }

    #[test]
    fn empty_line_is_nothing() {
        assert_eq!(plan("   ", &Facts::bare(), &mut vol()), Plan::Nothing);
    }

    #[test]
    fn core_verbs_are_answered_in_core() {
        let f = Facts::bare();
        let Plan::Say(m) = plan("echo hello world", &f, &mut vol()) else {
            panic!("echo must be core-answered");
        };
        assert_eq!(m, Message::TerminalOutput("hello world".to_string()));
        let Plan::Say(m) = plan("help", &f, &mut vol()) else { panic!("help") };
        assert!(m.text().starts_with("COMMANDS: ver, help"));
    }

    #[test]
    fn host_verbs_carry_the_rest_untouched() {
        let p = plan("cp -f  A.TXT  B.TXT", &Facts::bare(), &mut vol());
        assert_eq!(
            p,
            Plan::Host { verb: "cp".to_string(), rest: "-f  A.TXT  B.TXT".to_string() }
        );
    }

    #[test]
    fn unknown_word_refuses_when_exec_is_off() {
        let p = plan("nosuchthing", &Facts::bare(), &mut vol());
        assert_eq!(
            p,
            Plan::Say(Message::TerminalError(
                "Unknown command. Type 'help' for assistance.".to_string()
            ))
        );
    }

    #[test]
    fn extension_is_elided_only_for_bare_leaves() {
        let mut v = vol();
        assert_eq!(resolve_exec("VUG.ELF", &mut v).as_deref(), Some("VUG.ELF"));
        assert_eq!(resolve_exec("vug", &mut v).as_deref(), Some("VUG.ELF"));
        assert_eq!(resolve_exec("lower", &mut v).as_deref(), Some("lower.elf"));
        // Already-extensioned, and absent: no second guess.
        assert_eq!(resolve_exec("vug.txt", &mut v), None);
        assert_eq!(resolve_exec("README.TXT", &mut v).as_deref(), Some("README.TXT"));
        assert_eq!(resolve_exec("nope", &mut v), None);
    }

    #[test]
    fn a_verb_always_wins_over_a_program_of_the_same_stem() {
        let f = Facts { exec: true, ..Facts::bare() };
        // STAT.ELF is on the volume, but `stat` is a verb: the verb wins.
        assert!(matches!(plan("stat X", &f, &mut vol()), Plan::Host { .. }));
        // Its full name still launches it.
        assert_eq!(
            plan("STAT.ELF", &f, &mut vol()),
            Plan::Exec { typed: "STAT.ELF".to_string(), name: "STAT.ELF".to_string() }
        );
    }

    #[test]
    fn exec_plan_reports_both_spellings() {
        let f = Facts { exec: true, x86: true, ..Facts::bare() };
        assert_eq!(
            plan("vug", &f, &mut vol()),
            Plan::Exec { typed: "vug".to_string(), name: "VUG.ELF".to_string() }
        );
    }

    #[test]
    fn help_tracks_the_facts() {
        // The SHIPPED Pi: v3d on, `vugdemo` off (DECRUD-1's default), bare-name launch on
        // (PARITY §6.6a). `vug`/`pulse` must NOT be advertised as verbs — they are programs here —
        // and the bare-name lines must be present, since aarch64 now carries the capability.
        let arm = Facts {
            aarch64: true, v3d: true, proc_verbs: true, proc_rows: 6, exec: true, ..Facts::bare()
        };
        let h = help(&arm);
        assert!(h.contains("UNAFS:"));
        assert!(!h.contains("pulse (full-screen CPU monitor)"), "vugdemo-only verb advertised knob-off");
        assert!(!h.contains("vug (3D sculptor)"), "vugdemo-only verb advertised knob-off");
        assert!(h.contains("APPS:     v3d ("), "the v3d verb is real knob-off and must stay listed");
        assert!(h.contains("n>6 is refused"));
        assert!(h.contains("the .elf is optional"), "bare-name help is gated on exec, not on x86");

        // The same Pi with the demo knob ON: the two verbs exist, so they are advertised again.
        let armdemo = Facts { vugdemo: true, ..arm };
        let h = help(&armdemo);
        assert!(h.contains("pulse (full-screen CPU monitor)"));
        assert!(h.contains("APPS:     vug (3D sculptor), v3d ("));

        let x86 = Facts { x86: true, proc_verbs: true, proc_rows: 10, ..Facts::bare() };
        let h = help(&x86);
        assert!(!h.contains("UNAFS:"));
        assert!(h.contains("SMP:      sched (per-CPU run queues)"));
        assert!(h.contains("n>10 is refused"));
        assert!(!h.contains("just type it"), "bare-name help shown on a build with exec off");
    }

    /// BARENAME (PARITY §6.6a): the regression this arc exists to prevent — a verb that the build
    /// does NOT carry standing between the operator and the program of the same name.
    #[test]
    fn vug_is_a_verb_only_on_a_vugdemo_build() {
        let shipped = Facts { aarch64: true, proc_verbs: true, exec: true, ..Facts::bare() };
        assert!(!is_verb("vug", &shipped));
        assert!(!is_verb("pulse", &shipped));
        assert_eq!(
            plan("vug", &shipped, &mut vol()),
            Plan::Exec { typed: "vug".to_string(), name: "VUG.ELF".to_string() }
        );
        let demo = Facts { vugdemo: true, ..shipped };
        assert!(is_verb("vug", &demo));
        assert!(matches!(plan("vug", &demo, &mut vol()), Plan::Host { .. }));
        // The knob is not a licence to leak onto x86: `crate::vug` is aarch64 source.
        let x86demo = Facts { x86: true, vugdemo: true, exec: true, ..Facts::bare() };
        assert!(!is_verb("vug", &x86demo));
    }

    #[test]
    fn a_verb_wins_in_any_case_and_arrives_canonical() {
        // The volume carries LS.ELF — a dropped program named exactly like the
        // 8.3 spelling `ls` itself prints. Every casing of the verb must still
        // be the VERB, or a dropped file shadows `ls` for anyone with caps lock
        // on. (Before the case fold, `is_verb("LS")` was false and the FAT
        // walk's `eq_ignore_ascii_case` then matched LS.ELF: a launch.)
        let mut v = NameList(&["LS.ELF", "VUG.ELF"]);
        let f = Facts { exec: true, x86: true, ..Facts::bare() };
        for typed in ["LS", "Ls", "lS", "ls"] {
            assert!(is_verb(typed, &f), "{typed} must be the verb");
            assert_eq!(
                plan(typed, &f, &mut v),
                // The verb leaves CANONICAL: the ring's match arms are
                // lower-case, so a raw `LS` would fall to `other =>`.
                Plan::Host { verb: "ls".to_string(), rest: String::new() },
                "{typed} must dispatch as the verb `ls`, not launch LS.ELF"
            );
        }
        // Args survive the fold untouched — only the VERB is canonicalised.
        assert_eq!(
            plan("CAT DoCs/ReadMe.TXT", &f, &mut v),
            Plan::Host { verb: "cat".to_string(), rest: "DoCs/ReadMe.TXT".to_string() }
        );
        // And a core verb folds too, answering rather than refusing.
        let Plan::Say(m) = plan("ECHO Hi There", &f, &mut v) else { panic!("ECHO") };
        assert_eq!(m, Message::TerminalOutput("Hi There".to_string()));
        // A non-verb is still a program, and its own spelling is preserved.
        assert_eq!(
            plan("VUG", &f, &mut v),
            Plan::Exec { typed: "VUG".to_string(), name: "VUG.ELF".to_string() }
        );
    }

    #[test]
    fn upper_casing_a_leaf_is_ascii_only() {
        // `str::to_uppercase` would turn `ß` into `SS` and probe a name one
        // byte longer than anything the user typed. The resolver must not
        // invent characters: no ASCII-uppercase form of these exists on the
        // volume, so both simply miss.
        let mut v = NameList(&["SS.ELF", "FI.ELF"]);
        assert_eq!(resolve_exec("ß", &mut v), None);
        assert_eq!(resolve_exec("\u{fb01}", &mut v), None);
    }

    #[test]
    fn the_table_has_no_duplicates() {
        for v in CORE_VERBS {
            assert!(
                !HOST_VERBS.iter().any(|(n, _)| n == v),
                "{} is in both halves of the table",
                v
            );
        }
        for (i, (n, _)) in HOST_VERBS.iter().enumerate() {
            assert!(
                !HOST_VERBS[i + 1..].iter().any(|(m, _)| m == n),
                "{} listed twice",
                n
            );
        }
    }

    #[test]
    fn verb_ness_follows_the_build() {
        let arm = Facts { aarch64: true, vugdemo: true, ..Facts::bare() };
        let x86 = Facts { x86: true, exec: true, ..Facts::bare() };
        // `vug` is the 3D sculptor verb on a `vugdemo` Pi build ...
        assert!(is_verb("vug", &arm));
        assert!(matches!(plan("vug", &arm, &mut vol()), Plan::Host { .. }));
        // ... and on x86 it is not a verb at all, so it starts VUG.ELF. A flat
        // name list would have turned this launch into a refusal.
        assert!(!is_verb("vug", &x86));
        assert_eq!(
            plan("vug", &x86, &mut vol()),
            Plan::Exec { typed: "vug".to_string(), name: "VUG.ELF".to_string() }
        );
        // The process verbs follow `proc_verbs`, not the arch.
        assert!(!is_verb("storm", &Facts::bare()));
        assert!(is_verb("storm", &Facts { proc_verbs: true, ..Facts::bare() }));
    }
}
