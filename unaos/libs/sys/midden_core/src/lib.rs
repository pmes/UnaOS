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
    /// aarch64 build. RELICS (R26 clause 3): this fact no longer gates any VERB - the `u*`
    /// family it used to gate retired into the plain file verbs, and `top` is registered on every
    /// build. It still shapes `help` (the SMP line) and combines with `vugdemo` for the demos.
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
    // DECRUD-2 (2026-08-28): the `vugdemo` fact is GONE with the in-kernel demos it described.
    // Its BARENAME lesson outlived it and still governs this table: a verb the build does not carry
    // is not a verb, and claiming one stands between the operator and the ring-3 program of the same
    // name — verb-ness is absolute over bare-name launch. `vug` and `pulse` are now ALWAYS bare-name
    // launches of VUG.ELF / PULSE.ELF on every build and every arch, which is what the bench ran.
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
/// BASICS (orin 17): `exit`/`quit` are here and not in the ring because the
/// honest answer to them is a *sentence about what the shell is*, and that is
/// the core's own subject. There is no parent process to return to — the
/// framebuffer console IS the shell (shell_philosophy §5) — so the word must
/// neither be swallowed as "Unknown command" (a lie: it is a command everyone
/// knows) nor silently reboot the machine. It says what to type instead.
pub const CORE_VERBS: &[&str] = &["help", "echo", "ver", "version", "gneiss", "exit", "quit"];

/// When a host verb exists.
///
/// Verb-ness is **not** a global fact, and getting that wrong is the one way
/// this refactor could have broken the shell: `v3d` is a registered verb only
/// on an aarch64 `v3d` build, and a word that is not a verb here must fall
/// through to bare-name launch — which is how `vug` starts `VUG.ELF` on every
/// build now that DECRUD-2 has retired the in-kernel demo. A flat name list would have turned
/// that launch into a refusal. So each host verb carries the condition under
/// which the ring actually registers it, and those conditions mirror the
/// `#[cfg]` on the kernel's match arms one for one — **one for one is the whole
/// contract**, and PARITY §6.6a is what it costs when a condition here is
/// looser than the `#[cfg]` it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avail {
    /// Registered on every build.
    Always,
    /// aarch64 only. RELICS (R26 clause 3): NO MEMBERS. The native unafs verbs it existed for
    /// retired into the plain file verbs, which are `Always`. Kept because it is the honest
    /// availability for a verb whose ring arm genuinely does not compile off aarch64, and a
    /// future one may need it - but a new member needs a hardware reason written down beside it.
    ///
    /// ⚠ CORRECTION (SO47, 2026-09-13): the line above says "a future one may need it", and A GATE
    /// IN THIS FILE SAYS OTHERWISE. `no_verb_is_pinned_to_a_platform_without_a_capability` asserts
    /// `!matches!(a, Avail::Aarch64)` over EVERY member of `HOST_VERBS`, so this variant is empty BY
    /// TEST, not by convention - a new member reds `cargo test -p midden_core` before it ever reaches
    /// a reviewer. MEASURED, not read: registering SO47's `dns` here failed 2 of 16 tests
    /// (`lib.rs:972` and `lib.rs:940`, "`dns` must be a verb on Facts { aarch64: false, .. }").
    /// Whoever wants a member must amend that test and argue with R26 clause 3 in the same breath;
    /// the "hardware reason" above is the bar to clear. An invitation this file's own gate refuses is
    /// worse than no invitation, so the two paragraphs are read together or not at all.
    ///
    /// SO47's `dns` is the first verb since RELICS whose ring arm is genuinely aarch64-gated
    /// (`#[cfg(all(feature = "net6", target_arch = "aarch64"))]`) and it is registered `Always`
    /// anyway; the full reasoning sits beside that registration in `HOST_VERBS`. The short of it:
    /// this variant would not have been narrower (the arm is `net6` AND `aarch64`, this is only
    /// `aarch64`), and on the x86 half it does not make a verb absent - it makes it look like a typo.
    Aarch64,
    /// aarch64 + `v3d` (the GPU battery replay).
    V3d,
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
    ("time", Avail::Always), ("uptime", Avail::Always),
    // FAT + VFS surface
    ("ls", Avail::Always), ("dir", Avail::Always), ("cd", Avail::Always),
    ("pwd", Avail::Always), ("cat", Avail::Always), ("type", Avail::Always),
    ("head", Avail::Always), ("tail", Avail::Always), ("find", Avail::Always),
    ("du", Avail::Always), ("stat", Avail::Always), ("hexdump", Avail::Always),
    ("touch", Avail::Always), ("append", Avail::Always), ("rm", Avail::Always),
    ("del", Avail::Always), ("mkdir", Avail::Always), ("md", Avail::Always),
    ("rmdir", Avail::Always), ("rd", Avail::Always),
    ("cp", Avail::Always), ("copy", Avail::Always), ("mv", Avail::Always),
    ("move", Avail::Always), ("ren", Avail::Always), ("rename", Avail::Always),
    ("sync", Avail::Always), ("write", Avail::Always),
    // PRTSCR: capture the panel to `SCREEN<n>.PNG` at the volume root. Always - the VERB is one
    // word on every UnaOS (the ONE-OS law `reboot` is registered under), and the per-platform panel
    // decides what it can capture, never whether the word exists. The ring arm refuses honestly and
    // by name when there is no panel, no writable volume, or no free index.
    ("screenshot", Avail::Always),
    // WALLPAPER (rmbp-0929): `wallpaper <path.png>` / `wallpaper off` — the word exists everywhere (ONE-OS law); a kernel without the desktop backdrop simply has no arm and says so.
    ("wallpaper", Avail::Always),
    // RELICS (R26 clause 2): the native-volume file verbs ARE the plain file verbs. The `u*`
    // family (`uls` `ucat` `utouch` `uwrite` `umkdir` `urm` `umv` `urmattr`) was a second spelling
    // of `ls`/`cat`/`touch`/`write`/`mkdir`/`rm`/`mv` that differed only in which volume it
    // reached, and the plain verbs now reach that volume themselves through the one namespace
    // (`/` native, `/boot`, `/usb`) - the rule `ls`, `cat`, `run` and `bg` already followed. The
    // one member with no plain twin, `urmattr`, becomes the standard extended-attribute verb.
    ("setfattr", Avail::Always),
    // ATTRSURF (B299): the read half of the attribute surface and the query over it. `getfattr`
    // with no key IS the listing (one verb per job, R26 — no `lsattr`). Always: a FAT volume answers
    // `-ENOTSUP` from its own backend; the word exists everywhere.
    ("getfattr", Avail::Always), ("query", Avail::Always),
    // FILETYPE (B307): `file <path>` — the type and the leg that decided it; `assoc` — the type
    // registry (FILETYPES B423: attributes on /system/filetypes/*; openers are registrants). Always: a FAT root answers from the
    // registrants' compiled-in resources and says so.
    ("file", Avail::Always), ("assoc", Avail::Always),
    // RELICS (R26 clause 2): the five `usnap*` spellings collapse into ONE verb with subcommands,
    // the way `git` / `ip` / `systemctl` spell a family. `snap list|create|drop|ls|cat`.
    ("snap", Avail::Always),
    // block / device
    ("fdisk", Avail::Always), ("lsusb", Avail::Always), ("dd", Avail::Always),
    // network
    ("ifconfig", Avail::Always), ("ping", Avail::Always), ("arp", Avail::Always),
    ("nc", Avail::Always), ("curl", Avail::Always), ("fetch", Avail::Always),
    // SO47 (NETVERB, 2026-09-13): `dns` was MISSING from this group, so `dns google.com` fell to the
    // `Plan::Say(TerminalError)` at the bottom of `plan` and the shell's own `"dns"` arm
    // (`shell.rs:5594`, `#[cfg(all(feature = "net6", target_arch = "aarch64"))]`) had never been
    // reachable on ANY build. The tell on the render14 wire is that a BARE `dns` with no arguments
    // returned the same 44-byte error as `dns google.com` — a verb that parsed its argument could not
    // do that, and 44 is exactly the length of this file's "Unknown command." refusal.
    //
    // `Avail::Always`, and NOT `Avail::Aarch64`, and the reason is not a preference:
    //
    //  * `Aarch64` is not NARROWER here, it is the SAME looseness with a worse failure. The arm is
    //    `all(net6, aarch64)`; `Aarch64` is `aarch64`. On an aarch64 build with `net6` off the word
    //    is still a verb with no arm, so BOTH spellings land in the drift net either way. What
    //    `Aarch64` changes is only the x86 half — and there it does not make the verb absent, it
    //    makes it indistinguishable from a typo: `facts.exec` is true on x86, `resolve_exec("dns")`
    //    finds no DNS.ELF, and the shell answers with the very 44-byte `TerminalError` this fix
    //    exists to delete. Trading a sentence that names the verb for one that denies it is a
    //    regression wearing the shape of precision.
    //  * R26 clause 3 already ruled this exact case. `burst` and `simmer` were `Avail::Aarch64` "for
    //    no hardware reason" and were moved HERE, to `Always`, with the ring arm left to refuse
    //    honestly and by name. The `Aarch64` variant's own comment asks a new member for a HARDWARE
    //    reason, and `dns` has none: a resolver is not a device, x86 already resolves names through
    //    `smolnet::resolve` (the SNTP client's), and the missing x86 `dns` verb is an UNWRITTEN ARM,
    //    not an absent capability. Peter's words on the clause: "this is one OS not a bunch of
    //    separate ones."
    //  * The test at the bottom of this file already encodes that ruling as a rule — every standard
    //    word in the `// network` group is asserted `is_verb` on ALL THREE fact sets, x86 included.
    //    `dns` is a standard word in that group; registering it any other way would make it the one
    //    member of its own group that the rule does not hold for.
    //
    // EXACT was considered and is out of reach from this file alone: `Facts` carries no `net6` fact,
    // and inventing `Avail::Net6` + `Facts::net6` needs a PRODUCER in `midden_facts()` (shell.rs) to
    // set it. Added here without that producer the field is false on every build and the verb
    // registers NOWHERE, which is strictly worse than today. It is worth doing when someone holds
    // both files — one field, one variant, one line in `midden_facts()` — and it is overkill for any
    // smaller reason than closing the drift below.
    //
    // ⚠ WHAT THIS COSTS, stated rather than left for a reader to trip over: `shell.rs:6103-6122`
    // documents the `other =>` drift net as UNREACHABLE BY CONSTRUCTION — "that set is empty today,
    // because every `Avail` in `HOST_VERBS` mirrors the `#[cfg]` on its arm below exactly". With
    // this line that is no longer true: typing `dns` on a build without `all(net6, aarch64)` reaches
    // the net and prints "dns: not available on this build (the verb exists; this kernel does not
    // carry it)". The BEHAVIOUR is right — that sentence is exactly R26 clause 3's honest refusal —
    // but the COMMENT overstates and must be corrected in the same fold (reported, not edited here:
    // shell.rs is outside this arc's file list).
    ("dns", Avail::Always),
    // WIFI1 (rmbp-ledger B338, R79): `wifi` [status|up|scan|join <ssid>] — the BCM4331 radio verb.
    // A HOST verb (only the ring holds the radio and the bring-up ladder), `Avail::Always` on the
    // same ONE-OS precedent as `dns`/`reboot`/`install`: the word exists on every UnaOS, and the
    // platform (and the `wifi` knob) decides the answer. The kernel's dispatch arm is x86-only and
    // `wifi`-gated; on a build without it the arm falls through to the honest "not available" refusal
    // the drift net prints, exactly as `dns` does off `net6` (shell.rs's `other =>`).
    ("wifi", Avail::Always),
    // apps + scheduler + power + witnesses
    ("v3d", Avail::V3d),
    // RELICS (R26 clause 3, Peter: "you must stop trying to pin everything down and lock it in
    // this is one OS not a bunch of separate ones"). `burst` and `simmer` were `Avail::Aarch64`
    // for no hardware reason - both ring arms already carry a `#[cfg(not(aarch64))]` line that
    // refuses honestly and by name, exactly as `top` and `batmon` (registered `Always` since they
    // were written) do. The word exists on every UnaOS; the platform decides the answer.
    ("burst", Avail::Always), ("simmer", Avail::Always),
    ("tste", Avail::Always), ("selftest", Avail::Always), ("tests", Avail::Always), ("census", Avail::Always), ("prof", Avail::Always), ("src", Avail::Always), // SRCEXTRACT: arm is cfg(selfhost), registered Always like `fetch`
    ("ps", Avail::Always), ("top", Avail::Always), ("batmon", Avail::Always),
    ("dmesg", Avail::Always), ("shutdown", Avail::Always), ("off", Avail::Always),
    // ORIN-REBOOT (baton orin-6 5.1 + the cold-boot ruling 2026-08-25): the arch-neutral
    // WARM-REBOOT verb (`shutdown`/`off`, its cold sibling, was already registered above and
    // its ring arm now powers off for real). Registered here so MIDDEN-M1's single
    // interpreter routes it as Plan::Host to the ring's service arm (kernel `power::reboot` -
    // aarch64: PSCI SYSTEM_RESET via SMC; unwired platforms refuse with an honest witness).
    // Always: the VERB is one word on every UnaOS (ONE-OS law); the per-platform mechanism
    // decides what it does, never whether the word exists.
    ("reboot", Avail::Always), // HELPVERB (R75): `man` is NOT a table row — help.rs intercepts `help`/`man`/`--help` BEFORE the planner (shell.rs `help::intercept`), so a row would be a verb with no arm (GATE-VERBS, merge9 fold). (was: `man` is the help-in-a-window verb (help.rs intercepts it); the five verbs HELPVERB also registered (trash/shortcuts/play/linux/shot) live on the DONE2 row below, once (merge9 fold: the duplicate rows failed `the_table_has_no_duplicates`). LINE-NEUTRAL fold.
    // FILEVIEW `view` / TEXTEDIT `edit`: ring arms are desktop-gated (wc or desktop_firmware); registered Always on the `dns` precedent (a gated-off arm falls through to bare-name launch, never a typo refusal).
    ("view", Avail::Always), ("edit", Avail::Always), ("dialog", Avail::Always), ("activity", Avail::Always), ("settings", Avail::Always), ("trash", Avail::Always), ("shortcuts", Avail::Always), ("play", Avail::Always), ("linux", Avail::Always), ("shot", Avail::Always), ("pref", Avail::Always), ("battery", Avail::Always), ("bt", Avail::Always), // POWERMENU: `battery` (cfg-gated arm, registered Always like `src`). PREFS (B300): Principia's store, ungated arm; ACTIVITY (R75); TRASH/SHORTCUTS/PLAYWAV/LINUXABI/SHOTREGION: arms are cfg-gated, registered Always like `src`
    // processes
    ("run", Avail::Proc), ("bg", Avail::Proc), ("storm", Avail::Proc), // EXECNAME (B322, R82): the `lumen` row is GONE with its shortcut arm — `lumen` is the program LUMEN.ELF, reached by bare name like every program
    ("jobs", Avail::Proc), ("kill", Avail::Proc),
    // BASICS (orin 17, Peter: "some of the commands were 1 off tests we need basic commands
    // back"). The everyday words a prompt is unusable without. Every one is `Plan::Host` and not
    // core-answered, because each needs something only the ring has: `grep`/`wc` need the volume,
    // `df`/`mount` the block layer and the mount table, `env` the build's own facts, `history` the
    // shell's dispatch record, `sleep` the timebase, `which` the resolver over a real volume.
    // `Avail::Always` throughout: these are ONE-OS words (the law `reboot` and `screenshot` are
    // registered under) - the platform decides what the answer is, never whether the word exists.
    ("grep", Avail::Always), ("wc", Avail::Always), ("df", Avail::Always),
    ("mount", Avail::Always), ("env", Avail::Always), ("set", Avail::Always),
    ("history", Avail::Always), ("sleep", Avail::Always), ("which", Avail::Always), #[cfg(feature = "login")] ("login", Avail::Always), #[cfg(feature = "login")] ("logout", Avail::Always), #[cfg(feature = "login")] ("adduser", Avail::Always), #[cfg(feature = "login")] ("passwd", Avail::Always), #[cfg(feature = "login")] ("users", Avail::Always), #[cfg(feature = "login")] ("deluser", Avail::Always), #[cfg(feature = "login")] ("whoami", Avail::Always), #[cfg(feature = "installdemo")] ("install", Avail::Always), #[cfg(feature = "installdemo")] ("uninstall", Avail::Always), #[cfg(feature = "uvc")] ("ptp", Avail::Always), #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] ("stage", Avail::Always), // PTPCAM (B687): `ptp` — the PTP camera verb (status/view/shutter/frame), a HOST verb built where the class driver is (the `uvc` knob). LINE-NEUTRAL fold, code before this comment. // INSTALLVERB (rmbp-ledger B89 fourth rung, B91): `install` — the operator path to the partition installer. A HOST verb (only the ring holds the block registry, the GPT reader and the engine: `crates/kernel/src/shell.rs`), `Avail::Always` because it is a ONE-OS word (the law `reboot` and `screenshot` are registered under) — the platform decides what a disk censuses to, never whether the word exists. Built only where the installer engine is (this crate's `installdemo` feature, forwarded by the kernel's knob), so the `Avail` and the arm's `#[cfg]` are the same condition one for one, which is this table's whole contract. ⚠ LINE-NEUTRAL fold onto the LOGIN line for the reason recorded there. LOGIN (RULINGS R51): `login <name> <password>` / `logout`, the human-user session — host verbs (only the ring holds the record store and the principal, fs/users.rs), `Avail::Always` on every platform, built only where the LOGIN line is (this crate's `login` feature, forwarded by the kernel's knob). ⚠ LINE-NEUTRAL fold: seven added lines here moved two panic::Location bytes in the knob-off image (LOGIN M2 knoboff vs hw-jetson); the rows ride this line.
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
/// `VUG.ELF` on every build of either arch, the in-kernel sculptor verb having
/// been retired by DECRUD-2.
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
    /// `name` is the on-disk spelling to load, `args` the words that followed
    /// it on the line (RING3ABI2, B333: the program's `argv[1..]`; `argv[0]` is
    /// `typed`). The core splits the line once, so both rings pass the same words.
    Exec { typed: String, name: String, args: Vec<String> },
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
            return Plan::Exec { typed: word.to_string(), name, args: args.iter().map(|a| a.to_string()).collect() };
        }
    }
    if let Some(line) = gated_off_line(canon) {
        return Plan::Say(Message::TerminalOutput(line)); // GATED-OFF (TESTFIX4, B330): a verb of other builds, named with its knob
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
        // BASICS (orin 17). `TerminalOutput`, not `TerminalError`: the user typed a real command
        // and got a real answer, so the refusal wording and the error channel would both be
        // wrong. What is refused is the ACTION, and there is no action — there is no parent shell
        // and no session to close, because this console is the shell. Naming `shutdown` and
        // `reboot` here is the whole value of the verb: those are the two words that do what
        // someone typing `exit` at the last prompt of the day actually wants.
        "exit" | "quit" => Message::TerminalOutput(
            "this console IS the shell — there is nothing to exit to. \
             Use `shutdown` to power off, `reboot` to restart, `clear` to blank the screen."
                .to_string(),
        ),
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
    say!("COMMANDS: ver, help, clear, echo, gneiss, exit");
    // BASICS (orin 17): the shell's own state, listed with the shell's own words rather than
    // scattered among the subsystems that own the rest of the table.
    say!("SHELL:    which <word>  (verb? program? neither), history [n] [-c], sleep <ms>");
    say!("          env  (build facts + shell variables), set [NAME [VALUE]] | set -u NAME");
    say!("          (no $VAR expansion yet - the store is readable by `env`; expansion is midden M2)");
    // RELICS (R26 clause 1): the standard names. `diskinfo` was block-device GEOMETRY, which is
    // what `fdisk -l` answers, so that is the spelling; `df`/`mount` keep the capacity question.
    // `read <lba>` / `write <lba> <byte>` are `dd`, which ends the hazard of a mis-shaped file
    // write being a raw block write.
    say!("STORAGE:  df | mount  (volumes: capacity, used, free, mount point, posture)");
    say!("          fdisk -l  (block devices: vendor, block size, blocks, capacity)");
    say!("          dd if=<lba> | dd of=<lba> byte=<0xNN>  (ONE raw 512-byte block; no file is involved)");
    say!("FILES:    ls [-l] [dir], cd [dir], pwd, cat <path>");
    say!("PAGING:   head <path> [n], tail <path> [n]  (first / last n lines, default 10)");
    // BASICS (orin 17): the two text verbs. `grep`'s pattern is a FIXED STRING with ^/$ anchors -
    // said here in as many words, because a help line that says "pattern" invites a regex the
    // shell does not carry, and a silent literal match of `.*` is worse than a refusal.
    say!("TEXT:     grep [-i] [-n] [-v] [-c] <pattern> <path>  (fixed string; ^ and $ anchor)");
    say!("          wc [-l|-w|-c] <path>  (lines, words, bytes)");
    say!("WRITE:    touch <path>, write <path> <text>, append <path> <text>, rm [-r] [-f] <path>");
    say!("DIRS:     mkdir <path>, rmdir <path>  (create / remove empty directories)");
    // RELICS (R26 clause 1): the unified-namespace write surface is spelled `mount`, the same word
    // that answers "where is what attached" - one verb for the namespace, not two.
    say!("MOUNT:    mount write|append|rm|mkdir <path> [text]  (the one namespace, by prefix)");
    say!("          rm -r <dir>  (recursively delete a directory tree; root refused)");
    say!("COPY:     cp [-f] <src> <dst>  (copy a file; cp FILE DIR/ lands as DIR/<leaf>)");
    say!("          cp -r <srcdir> <dst>  (recursively copy a directory tree)");
    say!("MOVE:     mv [-f] <src...> <dst>  (move/rename a file or dir, O(1); mv SRC DIR/ lands as DIR/<leaf>)");
    say!("FLAGS:    default is no-clobber; -f = force overwrite (rm: quiet on missing), -n = no-clobber");
    say!("WILDCARD: * / ? in the last path component - ls/cat/rm/cp/mv expand it (e.g. rm *.TMP, cp *.TXT DOCS/)");
    say!("          (create/edit/delete/copy/move files & dirs anywhere in the tree; sync = write-through, durable)");
    say!("TREE:     find [root] <pattern>  (recursive glob search), du [dir]  (recursive size tally)");
    say!("          uptime  (seconds since boot; shows the wall clock when set)");
    say!("INSPECT:  stat <path>  (full on-disk detail), hexdump <path> [off] [len]  (bounded)");
    // RELICS (R26 clause 2): the `u*` family retired into the plain file verbs, which reach the
    // native volume through the one namespace. Only the two words with no plain twin are left, and
    // both wear their standard spelling: extended attributes and snapshots.
    say!("NATIVE:   the plain file verbs reach the native volume: `/` native, /boot the boot FAT, /usb stick");
    say!("          setfattr <path> <key>=<value>, setfattr -x <key> <path>  (typed attributes: 42 1.5 \"s\" b64:.. [f,f])");
    say!("          getfattr <path> [key]  (one or all attributes), query <expr>  (id path of every match)");
    say!("SNAP:     snap list, snap create <name>, snap drop <gen>  (retained roots / snapshots)");
    say!("          snap ls <gen> [path], snap cat <gen> <path>  (read a snapshot; current-ACL enforced)");
    say!("CLOCK:    date, date -s YYYY-MM-DD HH:MM[:SS]  (seeds mtime stamps; unset = honest dashes)");
    say!("          time  (shared civil clock: ISO-8601 UTC + source; unsynced until SNTP/date -s)");
    if facts.aarch64 {
        say!("SMP:      ps (per-CPU run queues), top (per-core load: busy%, ctx-switches, last task)");
    } else {
        say!("SMP:      ps (per-CPU run queues)");
    }
    if facts.v3d {
        say!("APPS:     v3d (replay the visible GPU graphics battery)");
    }
    if facts.proc_verbs {
        say!("PROC:     bg <path> / run <path> (force background / foreground), jobs (list+reap), kill <pid>");
    }
    // BARENAME (PARITY 6.6a): keyed off `exec`, the fact these two lines actually describe, not
    // off `x86`, the arch that happened to be the only one carrying it. A help line that names a
    // capability must be gated on the capability.
    if facts.exec {
        say!("          <name>      (just type it: lumen opens its window and the prompt returns; net runs, then prints its exit)");
        // BARE-NAME (M1): the elision is a resolution rule, so it is stated
        // where the launch rule is stated. `.elf` stays visible in `ls`.
        say!("          the .elf is optional (lumen finds LUMEN.ELF); the program says how it runs: a window or a server detaches, else foreground");
    }
    say!("POWER:    reboot (warm reboot), shutdown|off (power off, cold-boot-ready) - via the platform firmware");
    // RELICS (R26 clause 1): `netinfo` is `ifconfig` and not `ip`, and the OUTPUT is the reason.
    // It renders ONE interface's state - MAC, link, and this NIC's frame/IRQ counters - which is
    // ifconfig's shape. `ip` prints address OBJECTS over a set of links, and the shell has no
    // address table to print. `connect`/`udpsend` are one `nc` (TCP, `-u` for UDP); `get` is
    // `curl`, which is what an HTTP/1.0 GET over a socket is called everywhere else.
    say!("NETWORK:  ifconfig  (NIC: MAC, link, counters), ping <ip> [count], arp <ip>");
    say!("          nc [-u] <ip> <port> [message]  (TCP connect+exchange; -u = one UDP datagram)");
    say!("          curl [http://]<ip>[:port][/path]  (HTTP/1.0 GET, printed)");
    // RELICS (R26 clause 3, Peter: "most of those probes are useful still no?"). The probes STAY,
    // and they are grouped so the everyday table above reads as an everyday table. Every one of
    // them answers on every build - the ring arm says what THIS platform can do, and the word
    // exists regardless (the ONE-OS law).
    say!("BENCH:    tste (in-OS self-test suite), panic (enter the exception path deliberately)");
    say!("          run <path>  (load + run an ELF64 in the foreground)");
    say!("          screenshot  (panel -> SCREEN<n>.PNG at the volume root; n = first free 0-99)");
    say!("          the Print Screen key does the same thing (x86)");
    say!("          dmesg  (boot-milestone ring: PORTSW / FTDI console / EHCI HID / block / GUI handoff)");
    say!("          hexdump <path> [off] [len]  (bounded dump), lsusb  (USB device summary)");
    say!("          burst  (SCHED-BAL multi-hot-thread burst), simmer  (per-core load animator)");
    if facts.proc_verbs {
        say!(format!(
            "          storm [n]  (launch n vugs; n>{} is refused by the process table - serial carries the [storm] census)",
            facts.proc_rows
        ));
    }
    say!("          batmon  (battery snapshot; the platform that has one answers)");
    // BARENAME (PARITY 6.6a), post-DECRUD-2: `vug` and `pulse` are NOT advertised on any build.
    // The in-kernel demos are gone and the `vugdemo` fact with them, so the two words name only
    // the ring-3 programs `VUG.ELF` / `PULSE.ELF` and belong to the bare-name lines above.
    // Advertising them here would advertise a verb no build can dispatch: the rule `Facts` opens with.
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
            Plan::Exec { typed: "STAT.ELF".to_string(), name: "STAT.ELF".to_string(), args: Vec::new() }
        );
    }

    #[test]
    fn exec_plan_reports_both_spellings() {
        let f = Facts { exec: true, x86: true, ..Facts::bare() };
        assert_eq!(
            plan("vug", &f, &mut vol()),
            Plan::Exec { typed: "vug".to_string(), name: "VUG.ELF".to_string(), args: Vec::new() }
        );
    }

    #[test]
    fn help_tracks_the_facts() {
        // The SHIPPED Pi: v3d on, bare-name launch on (PARITY §6.6a). Post-DECRUD-2 there is no
        // build on which `vug`/`pulse` are verbs — they are ring-3 programs on every image —
        // and the bare-name lines must be present, since aarch64 now carries the capability.
        let arm = Facts {
            aarch64: true, v3d: true, proc_verbs: true, proc_rows: 6, exec: true, ..Facts::bare()
        };
        let h = help(&arm);
        // RELICS (R26): the `u*` family retired, so the class is NATIVE and it is UNCONDITIONAL -
        // the plain file verbs reach the native volume, and the sentence saying so is not gated on
        // an arch. (`UNAFS:` was the old class, and it was aarch64-only because the verbs were.)
        assert!(h.contains("NATIVE:"));
        assert!(!h.contains("pulse  (full-screen CPU monitor)"), "vugdemo-only verb advertised knob-off");
        assert!(!h.contains("vug  (in-kernel 3D sculptor)"), "vugdemo-only verb advertised knob-off");
        assert!(h.contains("APPS:     v3d ("), "the v3d verb is real knob-off and must stay listed");
        assert!(h.contains("n>6 is refused"));
        assert!(h.contains("the .elf is optional"), "bare-name help is gated on exec, not on x86");

        let x86 = Facts { x86: true, proc_verbs: true, proc_rows: 10, ..Facts::bare() };
        let h = help(&x86);
        // RELICS (R26 clause 3): NATIVE is on every build now - the plain verbs are the verbs.
        assert!(h.contains("NATIVE:"));
        // RELICS (R26 clause 1): `sched` retired; `ps` is the name.
        assert!(h.contains("SMP:      ps (per-CPU run queues)"));
        assert!(!h.contains("sched"), "the retired `sched` spelling is still in help");
        assert!(h.contains("n>10 is refused"));
        assert!(!h.contains("just type it"), "bare-name help shown on a build with exec off");
    }

    /// BARENAME (PARITY §6.6a): the regression this arc exists to prevent — a verb that the build
    /// does NOT carry standing between the operator and the program of the same name.
    #[test]
    fn vug_is_never_a_verb_and_always_reaches_the_program() {
        // DECRUD-2 made this unconditional: the in-kernel demos are gone, so on NO build and NO arch
        // are `vug`/`pulse` verbs, and the words must always reach the ring-3 program. This is the
        // strong form of what the knob-off half of this test used to assert.
        let shipped = Facts { aarch64: true, proc_verbs: true, exec: true, ..Facts::bare() };
        assert!(!is_verb("vug", &shipped));
        assert!(!is_verb("pulse", &shipped));
        assert_eq!(
            plan("vug", &shipped, &mut vol()),
            Plan::Exec { typed: "vug".to_string(), name: "VUG.ELF".to_string(), args: Vec::new() }
        );
        let x86 = Facts { x86: true, proc_verbs: true, exec: true, ..Facts::bare() };
        assert!(!is_verb("vug", &x86));
        assert!(!is_verb("pulse", &x86));
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
            Plan::Exec { typed: "VUG".to_string(), name: "VUG.ELF".to_string(), args: Vec::new() }
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

    /// BASICS (orin 17): the words Peter asked for are verbs on EVERY build, they route to the
    /// ring rather than being answered here, and they carry their arguments through untouched.
    /// A build-conditional basic would be the bug this asserts against — `wc` must not be a verb
    /// on the Pi and a program name on the rMBP.
    #[test]
    fn the_basics_are_verbs_everywhere() {
        for f in [Facts::bare(), Facts { aarch64: true, ..Facts::bare() },
                  Facts { x86: true, exec: true, ..Facts::bare() }] {
            for w in ["grep", "wc", "df", "mount", "env", "set", "history", "sleep", "which"] {
                assert!(is_verb(w, &f), "{} is not a verb on {:?}", w, f);
                assert!(
                    matches!(plan(w, &f, &mut vol()), Plan::Host { .. }),
                    "{} must route to the ring, not be answered in-core", w
                );
            }
            // `exit` is the exception, and deliberately: the core answers it in full.
            let Plan::Say(m) = plan("exit", &f, &mut vol()) else { panic!("exit") };
            assert!(m.text().contains("shutdown"), "exit must name the verb that does the thing");
            assert_eq!(m.kind(), "TerminalOutput", "a known command is not an error");
        }
        // Arguments survive the trip, including flags and a pattern with spaces around it.
        assert_eq!(
            plan("grep -i  needle  DOCS/HAY.TXT", &Facts::bare(), &mut vol()),
            Plan::Host { verb: "grep".to_string(), rest: "-i  needle  DOCS/HAY.TXT".to_string() }
        );
    }

    /// RELICS (R26 clause 1+2): the standard name REPLACED ours, it did not alias it.
    ///
    /// Both directions, and the first one is the point: a rename that leaves the old word
    /// registered is an alias, and Peter's ruling is "switch to the standard names", not "offer
    /// both". So every retired spelling must be a NON-verb on every build - at which point it is
    /// free to be a program name again, which is what a retired verb should be.
    #[test]
    fn the_retired_spellings_are_not_verbs_anywhere() {
        const RETIRED: &[&str] = &[
            // clause 1 - renamed to the standard word
            "bootlog", "vfs", "xd", "usbinfo", "netinfo", "diskinfo", "fatinfo", "setdate",
            "sched", "connect", "udpsend", "get", "read",
            // clause 2 - the duplicated unafs family, and the five usnap spellings
            "uls", "ucat", "umkdir", "umv", "urm", "urmattr", "utouch", "uwrite",
            "usnap", "usnapcat", "usnapdrop", "usnapls", "usnaps",
        ];
        let builds = [
            Facts::bare(),
            Facts { aarch64: true, v3d: true, proc_verbs: true, exec: true, ..Facts::bare() },
            Facts { x86: true, proc_verbs: true, exec: true, ..Facts::bare() },
        ];
        for f in builds {
            for w in RETIRED {
                assert!(!is_verb(w, &f), "retired spelling `{}` is still a verb on {:?}", w, f);
            }
            // ... and the standard word that replaced it is a verb on EVERY build (clause 3: no
            // verb is pinned to a platform; the ring arm decides the answer, not the word).
            // SO47: `dns` joins this list deliberately. It is the one verb here whose ring arm is
            // genuinely aarch64-gated, so it is the case that proves the rule rather than an
            // exception to it — if a later hand narrows its `Avail`, THIS assertion reds on the x86
            // fact set instead of the shell quietly answering "Unknown command" on a real build.
            for w in ["dmesg", "mount", "hexdump", "lsusb", "ifconfig", "fdisk", "dd", "ps",
                      "nc", "curl", "snap", "setfattr", "burst", "simmer", "batmon", "top", "dns"] {
                assert!(is_verb(w, &f), "`{}` must be a verb on {:?}", w, f);
                assert!(
                    matches!(plan(w, &f, &mut vol()), Plan::Host { .. }),
                    "`{}` must route to the ring", w
                );
            }
        }
        // `date` absorbed `setdate` as `date -s`: the flag rides through as `rest`, untouched.
        assert_eq!(
            plan("date -s 2026-09-06 11:22", &Facts::bare(), &mut vol()),
            Plan::Host { verb: "date".to_string(), rest: "-s 2026-09-06 11:22".to_string() }
        );
        // `mount` absorbed `vfs`: the subcommand rides through as `rest`, untouched.
        assert_eq!(
            plan("mount write /A.TXT hello", &Facts::bare(), &mut vol()),
            Plan::Host { verb: "mount".to_string(), rest: "write /A.TXT hello".to_string() }
        );
        // `snap` absorbed the five `usnap*` spellings the same way.
        assert_eq!(
            plan("snap cat 7 /K3HELLO.TXT", &Facts::bare(), &mut vol()),
            Plan::Host { verb: "snap".to_string(), rest: "cat 7 /K3HELLO.TXT".to_string() }
        );
    }

    /// RELICS (R26 clause 3): NO VERB IS PINNED TO A PLATFORM.
    ///
    /// The table's arch-conditional availabilities are now exactly three, and each is a
    /// CAPABILITY the build either compiled in or did not - never a preference about which board
    /// deserves the word. `Avail::Aarch64` has no members left at all.
    #[test]
    fn no_verb_is_pinned_to_a_platform_without_a_capability() {
        for (n, a) in HOST_VERBS {
            assert!(
                !matches!(a, Avail::Aarch64),
                "`{}` is pinned to an arch with no capability behind it (R26 clause 3)", n
            );
        }
        // The three that remain are knobs/capabilities, and they are the ones documented as such.
        for (n, a) in HOST_VERBS {
            if matches!(a, Avail::V3d) {
                assert!(["v3d", "vug", "pulse"].contains(n), "unexpected knob-gated verb `{}`", n);
            }
        }
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
        let arm = Facts { aarch64: true, v3d: true, ..Facts::bare() };
        let x86 = Facts { x86: true, exec: true, ..Facts::bare() };
        // `v3d` is a registered verb on an aarch64 `v3d` build ...
        assert!(is_verb("v3d", &arm));
        assert!(matches!(plan("v3d", &arm, &mut vol()), Plan::Host { .. }));
        // ... and on x86 it is not a verb at all. `vug` is now a verb on NEITHER (DECRUD-2), so it
        // starts VUG.ELF everywhere; a flat name list would have turned that launch into a refusal.
        assert!(!is_verb("v3d", &x86));
        assert!(!is_verb("vug", &arm));
        assert_eq!(
            plan("vug", &x86, &mut vol()),
            Plan::Exec { typed: "vug".to_string(), name: "VUG.ELF".to_string(), args: Vec::new() }
        );
        // The process verbs follow `proc_verbs`, not the arch.
        assert!(!is_verb("storm", &Facts::bare()));
        assert!(is_verb("storm", &Facts { proc_verbs: true, ..Facts::bare() }));
    }
}

// ---------------------------------------------------------------------------
// EXECNAME (rmbp-ledger B322, RULINGS R82): HOW a resolved program is launched
// ---------------------------------------------------------------------------
//
// Peter, 2026-10-04: "nobody wants to type bg." A bare name that `resolve_exec` found is launched with
// no verb, and the PROGRAM says how: an ELF note its own link emits (`una_abi::AppNote`, section
// `.note.unaos.app`). The parse lives here, in the shell core both rings link, so the x86 and aarch64
// shells (and the dock, which dispatches the same line) cannot disagree, and the host proves it.
// The three constants are the `una_abi` ones (this crate takes no dependencies); the kernel asserts the
// two agree at compile time (`arch/x86_64/elf.rs`).

/// `una_abi::APP_NOTE_NAME`: the note owner, NUL included (namesz = 6).
pub const APP_NOTE_NAME: &[u8] = b"UnaOS\0";
/// `una_abi::APP_NOTE_TYPE`.
pub const APP_NOTE_TYPE: u32 = 1;
/// `una_abi::APP_FLAG_WINDOWED`: the program opens a window.
pub const APP_FLAG_WINDOWED: u32 = 1 << 0;
/// `una_abi::APP_FLAG_RESIDENT`: the program stays running to serve (a bus fulfiller).
pub const APP_FLAG_RESIDENT: u32 = 1 << 1;

/// What a bare-name launch does with a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// The `bg` path: spawn, title the window, claim a job row, give the prompt back.
    Detach,
    /// The `run` path: run it, wait (bounded), print its exit status.
    Foreground,
}

/// The rule (R82): a program that makes a window, or that stays to serve, detaches; anything else runs
/// in the foreground. Flags 0 (no note) is the foreground fallback.
pub fn launch_mode(flags: u32) -> LaunchMode {
    if flags & (APP_FLAG_WINDOWED | APP_FLAG_RESIDENT) != 0 {
        LaunchMode::Detach
    } else {
        LaunchMode::Foreground
    }
}

fn le16(b: &[u8], o: usize) -> Option<u16> {
    let s = b.get(o..o.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}
fn le32(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn le64(b: &[u8], o: usize) -> Option<usize> {
    let s = b.get(o..o.checked_add(8)?)?;
    usize::try_from(u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])).ok()
}
fn align4(n: usize) -> Option<usize> {
    n.checked_add(3).map(|v| v & !3)
}

/// Walk one note area `[off, off+len)` of the image; the flags of the first UnaOS launch note, if any.
fn walk_notes(b: &[u8], off: usize, len: usize) -> Option<u32> {
    let area = b.get(off..off.checked_add(len)?)?;
    let mut i = 0usize;
    while i.checked_add(12)? <= area.len() {
        let namesz = le32(area, i)? as usize;
        let descsz = le32(area, i + 4)? as usize;
        let ntype = le32(area, i + 8)?;
        let name_at = i + 12;
        let desc_at = name_at.checked_add(align4(namesz)?)?;
        let next = desc_at.checked_add(align4(descsz)?)?;
        if next > area.len() {
            return None; // a record that runs off its area: malformed, stop (never index past it)
        }
        if ntype == APP_NOTE_TYPE && descsz >= 4 && area.get(name_at..name_at + namesz) == Some(APP_NOTE_NAME) {
            return le32(area, desc_at);
        }
        i = next;
    }
    None
}

/// The launch flags a program image declares in its `.note.unaos.app` note, or `None` when it carries
/// none (or is not a little-endian ELF64). UNTRUSTED bytes: every read is bounds-checked and every sum
/// is checked, so a malformed image is `None`, never a panic. Program headers (`PT_NOTE`) are walked
/// first — they survive `--strip-all` by construction — then section headers (`SHT_NOTE`) for a link
/// that kept the section but gave it no header.
pub fn app_note_flags(b: &[u8]) -> Option<u32> {
    const PT_NOTE: u32 = 4;
    const SHT_NOTE: u32 = 7;
    if b.len() < 64 || b[0..4] != [0x7F, b'E', b'L', b'F'] || b[4] != 2 || b[5] != 1 {
        return None;
    }
    let (phoff, phent, phnum) = (le64(b, 32)?, le16(b, 54)? as usize, le16(b, 56)? as usize);
    if phent == 56 {
        for k in 0..phnum {
            let ph = phoff.checked_add(k.checked_mul(56)?)?;
            if le32(b, ph) == Some(PT_NOTE) {
                if let Some(f) = walk_notes(b, le64(b, ph + 8)?, le64(b, ph + 32)?) {
                    return Some(f);
                }
            }
        }
    }
    let (shoff, shent, shnum) = (le64(b, 40)?, le16(b, 58)? as usize, le16(b, 60)? as usize);
    if shent == 64 && shoff != 0 {
        for k in 0..shnum {
            let sh = shoff.checked_add(k.checked_mul(64)?)?;
            if le32(b, sh + 4) == Some(SHT_NOTE) {
                if let Some(f) = walk_notes(b, le64(b, sh + 24)?, le64(b, sh + 32)?) {
                    return Some(f);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod execname_tests {
    use super::*;
    use alloc::vec;

    /// A minimal ELF64 LE image: header, one program header, and the note area at 0x100.
    fn image(note: &[u8], via_phdr: bool) -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        b[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        b[4] = 2;
        b[5] = 1;
        b[16..18].copy_from_slice(&2u16.to_le_bytes());
        b[18..20].copy_from_slice(&62u16.to_le_bytes());
        if via_phdr {
            b[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
            b[54..56].copy_from_slice(&56u16.to_le_bytes());
            b[56..58].copy_from_slice(&1u16.to_le_bytes());
            b[64..68].copy_from_slice(&4u32.to_le_bytes()); // PT_NOTE
            b[72..80].copy_from_slice(&0x100u64.to_le_bytes());
            b[96..104].copy_from_slice(&(note.len() as u64).to_le_bytes());
        } else {
            b[40..48].copy_from_slice(&0x180u64.to_le_bytes()); // e_shoff
            b[58..60].copy_from_slice(&64u16.to_le_bytes());
            b[60..62].copy_from_slice(&1u16.to_le_bytes());
            b[0x184..0x188].copy_from_slice(&7u32.to_le_bytes()); // SHT_NOTE
            b[0x198..0x1a0].copy_from_slice(&0x100u64.to_le_bytes());
            b[0x1a0..0x1a8].copy_from_slice(&(note.len() as u64).to_le_bytes());
        }
        b[0x100..0x100 + note.len()].copy_from_slice(note);
        b
    }

    fn note(name: &[u8], ntype: u32, flags: u32) -> Vec<u8> {
        let mut n = Vec::new();
        n.extend_from_slice(&(name.len() as u32).to_le_bytes());
        n.extend_from_slice(&4u32.to_le_bytes());
        n.extend_from_slice(&ntype.to_le_bytes());
        n.extend_from_slice(name);
        while n.len() % 4 != 0 {
            n.push(0);
        }
        n.extend_from_slice(&flags.to_le_bytes());
        n
    }

    #[test]
    fn the_note_is_read_by_program_header_and_by_section_header() {
        let w = note(APP_NOTE_NAME, APP_NOTE_TYPE, APP_FLAG_WINDOWED);
        assert_eq!(app_note_flags(&image(&w, true)), Some(APP_FLAG_WINDOWED));
        assert_eq!(app_note_flags(&image(&w, false)), Some(APP_FLAG_WINDOWED));
        // A foreign note before ours is skipped, ours is still found.
        let mut two = note(b"GNU\0", 3, 0xdead);
        two.extend_from_slice(&note(APP_NOTE_NAME, APP_NOTE_TYPE, APP_FLAG_RESIDENT));
        assert_eq!(app_note_flags(&image(&two, true)), Some(APP_FLAG_RESIDENT));
    }

    #[test]
    fn no_note_or_a_bad_one_is_the_foreground_fallback() {
        assert_eq!(app_note_flags(&image(&[], true)), None);
        assert_eq!(app_note_flags(&image(&note(b"UnaOX\0", APP_NOTE_TYPE, 1), true)), None);
        assert_eq!(app_note_flags(&image(&note(APP_NOTE_NAME, 2, 1), true)), None);
        assert_eq!(app_note_flags(b"\x7fELF"), None);
        assert_eq!(app_note_flags(&[0u8; 128]), None);
        // namesz that runs off the area and off the image: None, never a panic.
        let mut bad = note(APP_NOTE_NAME, APP_NOTE_TYPE, 1);
        bad[0..4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert_eq!(app_note_flags(&image(&bad, true)), None);
        // A note area pointing past the end of the image.
        let mut far = image(&note(APP_NOTE_NAME, APP_NOTE_TYPE, 1), true);
        far[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(app_note_flags(&far), None);
        assert_eq!(launch_mode(app_note_flags(&image(&[], true)).unwrap_or(0)), LaunchMode::Foreground);
    }

    #[test]
    fn windowed_and_resident_detach_console_runs_foreground() {
        assert_eq!(launch_mode(APP_FLAG_WINDOWED), LaunchMode::Detach);
        assert_eq!(launch_mode(APP_FLAG_RESIDENT), LaunchMode::Detach);
        assert_eq!(launch_mode(0), LaunchMode::Foreground);
        assert_eq!(launch_mode(1 << 7), LaunchMode::Foreground); // an unknown bit does not detach
    }

    #[test]
    fn the_staged_elf_names_resolve_by_bare_name_and_bin_never_did() {
        let staged = ["PREFS.ELF", "LUMEN.ELF", "NET.ELF", "BIG.ELF"];
        let mut v = NameList(&staged);
        for (w, want) in [("prefs", "PREFS.ELF"), ("lumen", "LUMEN.ELF"), ("net", "NET.ELF"), ("big", "BIG.ELF")] {
            assert_eq!(resolve_exec(w, &mut v).as_deref(), Some(want), "{w}");
        }
        // B322: the old staging name was unreachable by bare name — the reason the `lumen` arm existed.
        assert_eq!(resolve_exec("lumen", &mut NameList(&["LUMEN.BIN"])), None);
        // M3: `lumen` is no verb any more — the line plans as Exec, which is what the dock pin dispatches too.
        let f = Facts { exec: true, proc_verbs: true, x86: true, ..Facts::bare() };
        assert!(!is_verb("lumen", &f));
        assert_eq!(plan("lumen", &f, &mut NameList(&staged)), Plan::Exec { typed: "lumen".to_string(), name: "LUMEN.ELF".to_string(), args: Vec::new() });
        assert_eq!(plan("/apps/LUMEN.ELF", &f, &mut NameList(&["/apps/LUMEN.ELF"])), Plan::Exec { typed: "/apps/LUMEN.ELF".to_string(), name: "/apps/LUMEN.ELF".to_string(), args: Vec::new() });
        // RING3ABI2 (B333): the words after the program are its argv[1..], split once by the core.
        assert_eq!(plan("net  example.com  80", &f, &mut NameList(&["NET.ELF"])), Plan::Exec { typed: "net".to_string(), name: "NET.ELF".to_string(), args: vec!["example.com".to_string(), "80".to_string()] });
    }
}

// ---------------------------------------------------------------------------
// GATED-OFF (TESTFIX4, rmbp-ledger B330) — a verb this build does not carry, named with its knob
// ---------------------------------------------------------------------------
//
// Boot 20: `install` answered "Unknown command" because the verb is built only with the `installdemo`
// feature and the line did not carry `UNAOS_INSTALLDEMO=1`. The word is a verb of this OS; it is the BUILD
// that lacks it. So the core, which already decides "is this a command?", also answers "it is a command on
// other builds — this knob": one table of the cfg-gated rows of [`HOST_VERBS`] and the arroyo knob that arms
// each. A program by that name still wins (`plan` asks the resolver first), exactly as for any non-verb word.

/// Every cfg-gated [`HOST_VERBS`] row, with the build knob that carries it.
pub const GATED_VERBS: &[(&str, &str)] = &[
    ("install", "UNAOS_INSTALLDEMO=1"),
    ("login", "UNAOS_LOGIN=1"),
    ("logout", "UNAOS_LOGIN=1"),
    ("adduser", "UNAOS_LOGIN=1"),
    ("passwd", "UNAOS_LOGIN=1"),
    ("users", "UNAOS_LOGIN=1"),
    ("deluser", "UNAOS_LOGIN=1"),
    ("whoami", "UNAOS_LOGIN=1"), ("ptp", "UNAOS_UVC=1"),
];

/// The knob of a verb that exists on other builds but is NOT compiled into this one, or `None` (the verb is
/// here — whatever its platform `Avail` — or it is no gated verb at all).
pub fn gated_off(word: &str) -> Option<&'static str> {
    let w = canon_verb(word);
    let w: &str = &w;
    if HOST_VERBS.iter().any(|(n, _)| *n == w) {
        return None;
    }
    GATED_VERBS.iter().find(|(n, _)| *n == w).map(|(_, k)| *k)
}

/// The operator-facing answer for a gated-off verb: `install: verb present on UNAOS_INSTALLDEMO=1 builds only`.
pub fn gated_off_line(word: &str) -> Option<String> {
    gated_off(word).map(|k| format!("{}: verb present on {} builds only", canon_verb(word), k))
}

#[cfg(test)]
mod gated_tests {
    use super::*;

    #[test]
    fn every_gated_row_is_a_cfg_gated_host_verb() {
        // A GATED_VERBS row never names a core verb, and it is a HOST_VERBS row exactly when its feature is on.
        for (v, k) in GATED_VERBS {
            assert!(!CORE_VERBS.contains(v), "{v}");
            assert!(k.starts_with("UNAOS_") && k.ends_with("=1"), "{k}");
        }
        assert_eq!(gated_off("ls"), None);
        assert_eq!(gated_off("nosuchword"), None);
        assert_eq!(gated_off("help"), None);
    }

    #[cfg(not(feature = "installdemo"))]
    #[test]
    fn install_without_the_knob_names_the_knob() {
        let f = Facts { exec: true, proc_verbs: true, x86: true, ..Facts::bare() };
        assert!(!is_verb("install", &f));
        assert_eq!(gated_off("INSTALL"), Some("UNAOS_INSTALLDEMO=1"));
        let want = "install: verb present on UNAOS_INSTALLDEMO=1 builds only".to_string();
        assert_eq!(gated_off_line("install"), Some(want.clone()));
        assert_eq!(plan("install", &f, &mut NameList(&[])), Plan::Say(Message::TerminalOutput(want)));
        // A staged program by that name is still a program.
        assert_eq!(plan("install", &f, &mut NameList(&["INSTALL.ELF"])), Plan::Exec { typed: "install".to_string(), name: "INSTALL.ELF".to_string(), args: Vec::new() });
        // An unknown word stays unknown.
        assert!(matches!(plan("nosuchword", &f, &mut NameList(&[])), Plan::Say(Message::TerminalError(_))));
    }

    #[cfg(feature = "installdemo")]
    #[test]
    fn install_with_the_knob_is_a_host_verb() {
        let f = Facts { exec: true, proc_verbs: true, x86: true, ..Facts::bare() };
        assert_eq!(gated_off("install"), None);
        assert!(matches!(plan("install", &f, &mut NameList(&[])), Plan::Host { .. }));
    }

    #[cfg(all(feature = "installdemo", feature = "login", feature = "uvc"))]
    #[test]
    fn with_every_knob_every_gated_row_is_carried() {
        for (v, _) in GATED_VERBS {
            assert!(HOST_VERBS.iter().any(|(n, _)| n == v), "{v}");
            assert_eq!(gated_off(v), None);
        }
    }

    #[cfg(not(feature = "login"))]
    #[test]
    fn login_verbs_without_the_knob_name_the_knob() {
        assert_eq!(gated_off("whoami"), Some("UNAOS_LOGIN=1"));
    }
}

// HOLOCRON2 (rmbp-ledger B355, R82): `holocron` is a PROGRAM, not a verb — the bare name launches
// APPS/HOLOCRON.ELF with its words as argv (RING3ABI2), and its RESIDENT note detaches it.
#[cfg(test)]
mod holocron2_tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn holocron_is_a_program_with_argv() {
        let f = Facts { exec: true, proc_verbs: true, x86: true, ..Facts::bare() };
        assert!(!is_verb("holocron", &f));
        let staged = ["HOLOCRON.ELF"];
        assert_eq!(
            plan("holocron put vein claude.api_key sk-x", &f, &mut NameList(&staged)),
            Plan::Exec { typed: "holocron".to_string(), name: "HOLOCRON.ELF".to_string(), args: vec!["put".to_string(), "vein".to_string(), "claude.api_key".to_string(), "sk-x".to_string()] }
        );
        assert_eq!(launch_mode(APP_FLAG_RESIDENT), LaunchMode::Detach);
    }
}

// =============================================================================
// CONSOLEFIX M3 (rmbp-ledger B365, R65) — A PASSWORD NEVER REACHES THE WIRE
// =============================================================================
//
// Flight 22: `:: [midden] cmd="holocron init qwerty peter" -> Exec holocron.elf ::` — the kernel's
// command tracer echoed the line verbatim, and `holocron put vein claude.api_key sk-…` went out the same
// way. R65's rule (a password is never on the wire) covers the console verbs. THIS table is the one
// answer to "where does the secret start on this line?", for both rings: the kernel's tracer and its
// raw-key echoes, `handlers/midden`'s history, anything that prints a typed line.
//
// A shape is `(word, subwords, kept)`: the line's first word (canonical, path and `.elf` stripped), the
// second word it requires (`&[]` = any), and how many words stay readable. Everything after the kept
// words is the secret.

/// `(program or verb, required second word (empty = any), words kept readable)`.
pub const SECRET_SHAPES: &[(&str, &[&str], usize)] = &[
    ("holocron", &["init", "unlock", "put"], 2),
    ("login", &[], 2),
    ("adduser", &[], 2),
    ("passwd", &[], 1),
    ("su", &[], 1),
];

/// The first word as the table reads it: last path component, ASCII-lowercased, a trailing `.elf` cut.
fn secret_word(tok: &str) -> String {
    let leaf = tok.rsplit('/').next().unwrap_or(tok);
    let w = canon_verb(leaf);
    match w.strip_suffix(".elf") {
        Some(b) => b.to_string(),
        None => w,
    }
}

/// The byte offset where the secret begins on `line`, or `None` when the line is not secret-shaped (yet).
///
/// It is a PREFIX question too, which is what the raw-key echoes need while a line is being typed:
/// `"holocron init"` is `None` (the next byte may be `\r`), `"holocron init "` is `Some(14)` — every byte
/// typed from here on is secret — and `"holocron init qwerty"` is `Some(14)`.
pub fn secret_from(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let mut i = 0usize;
    let mut words: [(usize, usize); 3] = [(0, 0); 3];
    let mut nw = 0usize;
    while nw < 3 {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let s = i;
        while i < b.len() && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        words[nw] = (s, i);
        nw += 1;
    }
    if nw == 0 {
        return None;
    }
    let first = secret_word(&line[words[0].0..words[0].1]);
    let (_, subs, kept) = SECRET_SHAPES.iter().find(|(w, _, _)| *w == first)?;
    if !subs.is_empty() {
        if nw < 2 {
            return None;
        }
        let sub = canon_verb(&line[words[1].0..words[1].1]);
        if !subs.iter().any(|s| *s == sub) {
            return None;
        }
    }
    if nw < *kept {
        return None;
    }
    let end = words[*kept - 1].1;
    // The kept words must be FINISHED: whitespace after the last one, or a later word.
    let rest = &b[end..];
    if rest.first().is_some_and(|c| c.is_ascii_whitespace()) {
        let mut j = end;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        Some(j)
    } else {
        None
    }
}

/// `line` as it may be printed: the kept words and ` ***` when anything follows them; `None` = print the
/// line as typed (not secret-shaped, or nothing secret on it yet).
pub fn redact(line: &str) -> Option<String> {
    let at = secret_from(line)?;
    if line[at..].trim().is_empty() {
        return None;
    }
    Some(format!("{} ***", line[..at].trim()))
}

/// The KAT both rings run (the host in `cargo test`, the kernel in `tests pwwire`): `(line, expected)`.
/// `None` = printed as typed.
pub const REDACT_KAT: &[(&str, Option<&str>)] = &[
    ("holocron init qwerty peter", Some("holocron init ***")),
    ("holocron unlock hunter2", Some("holocron unlock ***")),
    ("holocron put vein claude.api_key sk-ant-xyz", Some("holocron put ***")),
    ("  HOLOCRON.ELF   init  pw ", Some("HOLOCRON.ELF   init ***")),
    ("/apps/holocron.elf unlock pw", Some("/apps/holocron.elf unlock ***")),
    ("holocron get vein claude.api_key", None),
    ("holocron init", None),
    ("login peter qwerty", Some("login peter ***")),
    ("login peter", None),
    ("passwd s3cret", Some("passwd ***")),
    ("adduser peter pw", Some("adduser peter ***")),
    ("ls /home/peter", None),
];

/// Run [`REDACT_KAT`] plus the prefix rule; returns `(passed, total)`.
pub fn redact_kat() -> (usize, usize) {
    let mut ok = 0usize;
    let mut n = 0usize;
    for (line, want) in REDACT_KAT {
        n += 1;
        if redact(line).as_deref() == *want {
            ok += 1;
        }
    }
    for (line, want) in [("holocron init", None), ("holocron init ", Some(14usize)), ("holocron init q", Some(14)), ("passwd", None), ("passwd ", Some(7))] {
        n += 1;
        if secret_from(line) == want {
            ok += 1;
        }
    }
    (ok, n)
}

#[cfg(test)]
mod consolefix_tests {
    use super::*;

    #[test]
    fn redact_kat_passes() {
        for (line, want) in REDACT_KAT {
            assert_eq!(redact(line).as_deref(), *want, "{line}");
        }
        let (ok, n) = redact_kat();
        assert_eq!(ok, n);
    }

    #[test]
    fn a_secret_never_survives_redaction() {
        for line in ["holocron init qwerty peter", "holocron put vein k sk-ant-flight22-bench-key", "login peter qwerty"] {
            let r = redact(line).unwrap();
            assert!(!r.contains("qwerty") && !r.contains("sk-ant"), "{r}");
        }
    }
}

// ---------------------------------------------------------------------------
// APPRES (rmbp-ledger B398, MACPARITY §16 B4 / row 38): a program's RESOURCES
// ---------------------------------------------------------------------------
//
// On BeOS an app's signature, icons and version lived in the executable's resources and the registrar
// published them, so Tracker and the Deskbar showed the icon before the app ever ran. Here the block is a
// second UnaOS note (owner "UnaOS", type [`APP_RES_NOTE_TYPE`]) in a NON-ALLOC `SHT_NOTE` section
// `.note.unaos.res` that `tools/una-res` appends after the strip — no program header, so the loader never
// maps it. The desc is [`RES_MAGIC`], a LE u32 record count, then `(u16 key_len, u32 val_len, key, value)`
// records, unpadded. The parse and the builder live here (both rings and the host tool link this crate), and
// every read is bounds-checked: the bytes are UNTRUSTED.

/// `una_abi::APP_RES_NOTE_TYPE`.
pub const APP_RES_NOTE_TYPE: u32 = 2;
/// The resource block's leading magic: format 1.
pub const RES_MAGIC: &[u8; 8] = b"UNARES\0\x01";
/// The display name.
pub const RES_KEY_NAME: &str = una_abi::attr_keys::NAME;
/// The app's signature, reverse-DNS (`org.unaos.quarry`).
pub const RES_KEY_SIGNATURE: &str = una_abi::attr_keys::SIGNATURE;
/// `semver+build`.
pub const RES_KEY_VERSION: &str = una_abi::attr_keys::VERSION;
/// `windowed` · `resident` · `console` — the launch note in words.
pub const RES_KEY_KIND: &str = una_abi::attr_keys::KIND;
/// The icon's SVG source.
pub const RES_KEY_ICON_SVG: &str = una_abi::attr_keys::ICON_SVG;
/// The icon rendered at 32, 64 and 128 px: RGBA PNGs.
pub const RES_KEY_ICON_32: &str = una_abi::attr_keys::ICON_32;
pub const RES_KEY_ICON_64: &str = una_abi::attr_keys::ICON_64;
pub const RES_KEY_ICON_128: &str = una_abi::attr_keys::ICON_128;
/// The prefix every icon key shares (`una:icon`, `una:icon.<size>`).
pub const RES_KEY_ICON_PREFIX: &str = una_abi::attr_keys::ICON;
/// The MIME types the app opens, newline-separated.
pub const RES_KEY_DOCTYPES: &str = una_abi::attr_keys::DOCTYPES;
/// DROPTYPES (B477): the MIME types the program takes as a drop, newline-separated (`type/*`, `*/*`).
pub const RES_KEY_DROPTYPES: &str = una_abi::attr_keys::DROPTYPES;
/// The rendered icon sizes, smallest first, with their keys.
pub const RES_ICON_SIZES: [(u32, &str); 3] = [(32, RES_KEY_ICON_32), (64, RES_KEY_ICON_64), (128, RES_KEY_ICON_128)];

/// The records of a resource block, in order. `None` when the block is not one (bad magic, a record that
/// runs off the end, a key that is not UTF-8, trailing bytes, or a count that disagrees with the records).
pub fn res_records(block: &[u8]) -> Option<Vec<(&str, &[u8])>> {
    if block.get(..8)? != RES_MAGIC {
        return None;
    }
    let count = le32(block, 8)? as usize;
    let mut out = Vec::with_capacity(count.min(64));
    let mut i = 12usize;
    for _ in 0..count {
        let kl = le16(block, i)? as usize;
        let vl = le32(block, i + 2)? as usize;
        let k_at = i.checked_add(6)?;
        let v_at = k_at.checked_add(kl)?;
        let end = v_at.checked_add(vl)?;
        let key = core::str::from_utf8(block.get(k_at..v_at)?).ok()?;
        if key.is_empty() {
            return None;
        }
        out.push((key, block.get(v_at..end)?));
        i = end;
    }
    if i != block.len() {
        return None;
    }
    Some(out)
}

/// One value of a resource block (the first record with `key`).
pub fn res_get<'a>(block: &'a [u8], key: &str) -> Option<&'a [u8]> {
    res_records(block)?.into_iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// One value as UTF-8 text.
pub fn res_str<'a>(block: &'a [u8], key: &str) -> Option<&'a str> {
    res_get(block, key).and_then(|v| core::str::from_utf8(v).ok())
}

/// Build a resource block from records (the host tool's writer and the tests). `None` when a key or value
/// cannot be encoded (key over u16, value over u32, empty key).
pub fn res_build(records: &[(&str, &[u8])]) -> Option<Vec<u8>> {
    let mut b = Vec::new();
    b.extend_from_slice(RES_MAGIC);
    b.extend_from_slice(&u32::try_from(records.len()).ok()?.to_le_bytes());
    for (k, v) in records {
        if k.is_empty() {
            return None;
        }
        b.extend_from_slice(&u16::try_from(k.len()).ok()?.to_le_bytes());
        b.extend_from_slice(&u32::try_from(v.len()).ok()?.to_le_bytes());
        b.extend_from_slice(k.as_bytes());
        b.extend_from_slice(v);
    }
    Some(b)
}

/// The desc of the first UnaOS note of `ntype` inside one note area, if any.
fn find_note(area: &[u8], ntype: u32) -> Option<&[u8]> {
    let mut i = 0usize;
    while i.checked_add(12)? <= area.len() {
        let namesz = le32(area, i)? as usize;
        let descsz = le32(area, i + 4)? as usize;
        let t = le32(area, i + 8)?;
        let name_at = i + 12;
        let desc_at = name_at.checked_add(align4(namesz)?)?;
        let next = desc_at.checked_add(align4(descsz)?)?;
        if desc_at.checked_add(descsz)? > area.len() {
            return None;
        }
        if t == ntype && area.get(name_at..name_at + namesz) == Some(APP_NOTE_NAME) {
            return area.get(desc_at..desc_at + descsz);
        }
        i = next;
    }
    None
}

/// The resource block inside ONE note area (a `.note.unaos.res` section's bytes), if any.
pub fn res_in_note_area(area: &[u8]) -> Option<&[u8]> {
    find_note(area, APP_RES_NOTE_TYPE)
}

/// Where an ELF64 LE image's section-header table lies: `(offset, entries)`, from the 64-byte header
/// alone. `None` for a non-ELF64-LE header or an entry size other than 64.
pub fn elf_shdr_table(ehdr: &[u8]) -> Option<(usize, usize)> {
    if ehdr.len() < 64 || ehdr[0..4] != [0x7F, b'E', b'L', b'F'] || ehdr[4] != 2 || ehdr[5] != 1 {
        return None;
    }
    let (shoff, shent, shnum) = (le64(ehdr, 40)?, le16(ehdr, 58)? as usize, le16(ehdr, 60)? as usize);
    if shent != 64 || shoff == 0 || shnum == 0 {
        return None;
    }
    Some((shoff, shnum))
}

/// The `(offset, size)` of every `SHT_NOTE` section in a section-header table's bytes. The kernel reads
/// the header, then the table, then only these areas — never the whole program.
pub fn elf_note_sections(shdrs: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for sh in shdrs.chunks_exact(64) {
        if le32(sh, 4) == Some(7) {
            if let (Some(o), Some(s)) = (le64(sh, 24), le64(sh, 32)) {
                out.push((o, s));
            }
        }
    }
    out
}

/// The resource block of a whole ELF image in memory (the launch path already holds the bytes), via its
/// section table. `None` when it carries none.
pub fn app_res(b: &[u8]) -> Option<&[u8]> {
    let (off, n) = elf_shdr_table(b)?;
    let table = b.get(off..off.checked_add(n.checked_mul(64)?)?)?;
    for (o, s) in elf_note_sections(table) {
        if let Some(area) = b.get(o..o.checked_add(s)?) {
            if let Some(r) = res_in_note_area(area) {
                return Some(r);
            }
        }
    }
    None
}

/// A note record wrapping `desc` as the UnaOS resource note (what the host tool appends as the section).
pub fn res_note(desc: &[u8]) -> Option<Vec<u8>> {
    let mut n = Vec::with_capacity(desc.len() + 24);
    n.extend_from_slice(&6u32.to_le_bytes());
    n.extend_from_slice(&u32::try_from(desc.len()).ok()?.to_le_bytes());
    n.extend_from_slice(&APP_RES_NOTE_TYPE.to_le_bytes());
    n.extend_from_slice(APP_NOTE_NAME);
    n.extend_from_slice(&[0, 0]);
    n.extend_from_slice(desc);
    while n.len() % 4 != 0 {
        n.push(0);
    }
    Some(n)
}

#[cfg(test)]
mod appres_tests {
    use super::*;
    use alloc::vec;

    fn block() -> Vec<u8> {
        res_build(&[(RES_KEY_NAME, b"Lumen"), (RES_KEY_SIGNATURE, b"org.unaos.lumen"), (RES_KEY_ICON_32, &[0x89, b'P', b'N', b'G'])]).unwrap()
    }

    /// An ELF64 LE header + one SHT_NOTE section header pointing at `area` placed at 0x100.
    fn elf_with(area: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        b[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        b[4] = 2;
        b[5] = 1;
        b.extend_from_slice(area);
        while b.len() % 8 != 0 {
            b.push(0);
        }
        let shoff = b.len();
        b.extend_from_slice(&[0u8; 64]); // SHN_UNDEF
        let mut sh = [0u8; 64];
        sh[4..8].copy_from_slice(&7u32.to_le_bytes());
        sh[24..32].copy_from_slice(&0x100u64.to_le_bytes());
        sh[32..40].copy_from_slice(&(area.len() as u64).to_le_bytes());
        b.extend_from_slice(&sh);
        b[40..48].copy_from_slice(&(shoff as u64).to_le_bytes());
        b[58..60].copy_from_slice(&64u16.to_le_bytes());
        b[60..62].copy_from_slice(&2u16.to_le_bytes());
        b
    }

    #[test]
    fn a_block_round_trips() {
        let b = block();
        let r = res_records(&b).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(res_str(&b, RES_KEY_NAME), Some("Lumen"));
        assert_eq!(res_str(&b, RES_KEY_SIGNATURE), Some("org.unaos.lumen"));
        assert_eq!(res_get(&b, RES_KEY_ICON_32), Some(&[0x89, b'P', b'N', b'G'][..]));
        assert_eq!(res_get(&b, RES_KEY_VERSION), None);
    }

    #[test]
    fn a_malformed_block_is_none_never_a_panic() {
        let b = block();
        for cut in 0..b.len() {
            assert!(res_records(&b[..cut]).is_none(), "cut {cut}");
        }
        let mut extra = b.clone();
        extra.push(0);
        assert!(res_records(&extra).is_none());
        let mut lie = b.clone();
        lie[8] = 0xFF; // the count claims more records than there are
        assert!(res_records(&lie).is_none());
        assert!(res_records(b"UNARES\0\x02\0\0\0\0").is_none());
        assert!(res_build(&[("", b"x")]).is_none());
    }

    #[test]
    fn the_block_is_found_through_the_section_table_and_not_confused_with_the_launch_note() {
        let mut area = Vec::new();
        // The launch note first (type 1), then the resource note (type 2), in one note area.
        area.extend_from_slice(&6u32.to_le_bytes());
        area.extend_from_slice(&4u32.to_le_bytes());
        area.extend_from_slice(&APP_NOTE_TYPE.to_le_bytes());
        area.extend_from_slice(b"UnaOS\0\0\0");
        area.extend_from_slice(&APP_FLAG_WINDOWED.to_le_bytes());
        area.extend_from_slice(&res_note(&block()).unwrap());
        let img = elf_with(&area);
        assert_eq!(app_res(&img), Some(&block()[..]));
        let (off, n) = elf_shdr_table(&img[..64]).unwrap();
        let secs = elf_note_sections(&img[off..off + n * 64]);
        assert_eq!(secs, vec![(0x100, area.len())]);
        // No resource note: None. A foreign owner: None.
        assert_eq!(app_res(&elf_with(&area[..24])), None);
        let mut foreign = res_note(&block()).unwrap();
        foreign[12] = b'X';
        assert_eq!(app_res(&elf_with(&foreign)), None);
        // A desc that runs off the area: None.
        let mut short = res_note(&block()).unwrap();
        short.truncate(40);
        assert_eq!(app_res(&elf_with(&short)), None);
        assert_eq!(app_res(b"\x7fELF"), None);
    }
}

/// `una_abi::APP_RES_SECTION`: the resource note's section name.
pub const APPRES_SECTION_NAME: &str = ".note.unaos.res";

// ── SMALLFIX7 (rmbp-ledger B501) — the `tests` registry's pre-heap admission, once, host-tested ───────────
// Flight 27's first two image-20 cards never booted: the stack guard registered `stackroom` at arm time, BEFORE
// the kernel heap, and SMALLFIX6's heap-grown table asked the allocator for 96 bytes it did not have. The bench's
// PREHEAP fix parked such registrations in a fixed stash; the rule lives here so the host can run it (the cloud's
// QEMU lanes build `tests-at-boot`, whose `register` never reaches the table, which is why no lane saw the panic).
pub mod fixtures {
    /// A fixed, allocation-free stash for registrations that arrive before the heap.
    pub struct Stash<T: Copy, const N: usize> {
        slots: [Option<T>; N],
    }

    /// What [`admit`] did with one registration.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Admit {
        /// No heap yet: parked in the stash (nothing allocated).
        Parked,
        /// No heap yet and the stash is full: NOT registered (the caller says so on the wire).
        Full,
        /// The heap is up: the stash drained into the table first (`drained` entries, in arrival order), then this one.
        Tabled { drained: usize },
    }

    impl<T: Copy, const N: usize> Stash<T, N> {
        pub const fn new() -> Self {
            Self { slots: [None; N] }
        }

        /// Parked entries.
        pub fn len(&self) -> usize {
            self.slots.iter().filter(|s| s.is_some()).count()
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    impl<T: Copy, const N: usize> Default for Stash<T, N> {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Admit one registration. Before the heap (`heap_up == false`) it parks — `push` is NEVER called, so nothing
    /// allocates; after it, the stash drains through `push` in arrival order and then `e` follows.
    pub fn admit<T: Copy, const N: usize>(stash: &mut Stash<T, N>, heap_up: bool, e: T, mut push: impl FnMut(T)) -> Admit {
        if !heap_up {
            return match stash.slots.iter_mut().find(|s| s.is_none()) {
                Some(slot) => {
                    *slot = Some(e);
                    Admit::Parked
                }
                None => Admit::Full,
            };
        }
        let mut drained = 0;
        for s in stash.slots.iter_mut() {
            if let Some(x) = s.take() {
                push(x);
                drained += 1;
            }
        }
        push(e);
        Admit::Tabled { drained }
    }
}

#[cfg(test)]
mod smallfix7_tests {
    extern crate std;
    use super::fixtures::{admit, Admit, Stash};
    use std::vec::Vec;

    /// The flight-27 shape: a fixture registers before the heap flag is set. The table (a growable Vec — the
    /// allocation that panicked on metal) must not be touched; the first post-heap registration drains the stash
    /// in order ahead of itself.
    #[test]
    fn registers_before_the_heap_without_touching_the_table() {
        let mut st: Stash<&str, 8> = Stash::new();
        let mut table: Vec<&str> = Vec::new();
        let mut pushes = 0;
        assert_eq!(admit(&mut st, false, "stackroom", |x| { pushes += 1; table.push(x) }), Admit::Parked);
        assert_eq!(admit(&mut st, false, "lockreg", |x| { pushes += 1; table.push(x) }), Admit::Parked);
        assert_eq!(pushes, 0, "a pre-heap registration must not allocate");
        assert!(table.is_empty());
        assert_eq!(st.len(), 2);
        assert_eq!(admit(&mut st, true, "winmemory", |x| table.push(x)), Admit::Tabled { drained: 2 });
        assert_eq!(table, ["stackroom", "lockreg", "winmemory"]);
        assert!(st.is_empty());
        assert_eq!(admit(&mut st, true, "prtscr", |x| table.push(x)), Admit::Tabled { drained: 0 });
        assert_eq!(table.len(), 4);
    }

    #[test]
    fn a_full_stash_refuses_and_keeps_what_it_holds() {
        let mut st: Stash<u8, 2> = Stash::new();
        let mut table: Vec<u8> = Vec::new();
        assert_eq!(admit(&mut st, false, 1, |x| table.push(x)), Admit::Parked);
        assert_eq!(admit(&mut st, false, 2, |x| table.push(x)), Admit::Parked);
        assert_eq!(admit(&mut st, false, 3, |x| table.push(x)), Admit::Full);
        assert_eq!(admit(&mut st, true, 4, |x| table.push(x)), Admit::Tabled { drained: 2 });
        assert_eq!(table, [1, 2, 4]);
    }
}

// ── MIDDENTWIN (LEDGER SR75, R104) — the ring-3 shell's facts, its served set and its refusal ────────────────
// `APPS/MIDDEN.ELF` (crates/user-midden) is the second ring that plans every line through THIS core: the
// kernel's shell builds its `Facts` from `cfg!` (`shell.rs` `midden_facts()`), the ring-3 program from what the
// ring-3 ABI can do, which is this constructor. The core still decides what a line MEANS; this block only
// states, once and host-tested, which `Plan::Host` verbs the ring-3 program fulfils and the text it prints for
// the rest. Appended at the file tail so no existing line (and no panic Location the kernel links) moves.

impl Facts {
    /// What a ring-3 program can do through the ring-3 ABI (una-abi's syscall list), on the arch it was built for.
    ///
    /// * `proc_verbs` / `exec` are FALSE: ring 3 has no spawn-by-path. `SYS_SPAWN` (8) loads only the fixed
    ///   staged program (HELLO.BIN), so `run`/`bg`/`jobs`/`kill`/`storm` and bare-name launch are verbs of the
    ///   kernel's shell only, and the core gives the ring-3 caller its own answer for them ("Unknown command").
    /// * `v3d` is false: the GPU battery replay is a kernel arm.
    /// * `proc_rows` is 0 beside `proc_verbs == false`, the kernel's own spelling for "no process table here".
    pub const fn ring3(x86: bool) -> Self {
        Facts { aarch64: !x86, x86, v3d: false, proc_verbs: false, proc_rows: 0, exec: false }
    }
}

/// The ring-3 shell's half of the table: what MIDDEN.ELF fulfils, what it refuses, and the witness codes.
pub mod ring3 {
    use super::{Message, CORE_VERBS, HOST_VERBS};
    use alloc::format;

    /// The `Plan::Host` verbs MIDDEN.ELF fulfils through the ring-3 ABI: `clear`/`history` are its own state,
    /// `pwd`/`cd` its cwd (`cd` checks the target with `SYS_STAT`), `ls`/`dir` read a listing with
    /// `SYS_PATH_READ` + `PATH_R_LIST`, `cat`/`type` the file with `SYS_PATH_READ`, `stat` is `SYS_STAT`.
    pub const SERVED: &[&str] = &["clear", "history", "pwd", "cd", "ls", "dir", "cat", "type", "stat"];

    /// The core verbs whose ring-3 meaning is "close this window": a window HAS a parent to return to (the
    /// desktop), which the framebuffer console the core's `exit` sentence describes does not.
    pub const CLOSES: &[&str] = &["exit", "quit"];

    /// Does MIDDEN.ELF fulfil this (canonical) `Plan::Host` verb?
    pub fn serves(verb: &str) -> bool {
        SERVED.contains(&verb)
    }

    /// The refusal for a verb the core registers on this build but the ring-3 ABI cannot fulfil. The kernel's
    /// drift net prints the same shape with "this kernel" (`shell.rs` `other =>`); ring 3 names its own carrier.
    pub fn refusal(verb: &str) -> Message {
        Message::TerminalError(format!(
            "{}: not available on this build (the verb exists; the ring-3 ABI does not carry it)",
            verb
        ))
    }

    /// The witness code of a verb: 1 + its index in `CORE_VERBS` then `HOST_VERBS`, 0 for no verb. The
    /// feature-gated `HOST_VERBS` rows sit at the table's TAIL, so every code below them is the same number in
    /// a kernel built with `login`/`installdemo` and in a program built without them.
    pub fn verb_code(verb: &str) -> u8 {
        let i = CORE_VERBS.iter().position(|v| *v == verb).or_else(|| {
            HOST_VERBS.iter().position(|(n, _)| *n == verb).map(|i| CORE_VERBS.len() + i)
        });
        match i {
            Some(i) if i < 255 => (i + 1) as u8,
            _ => 0,
        }
    }

    /// The verb a witness code names ([`verb_code`]'s inverse).
    pub fn verb_of(code: u8) -> Option<&'static str> {
        let i = (code as usize).checked_sub(1)?;
        CORE_VERBS.get(i).copied().or_else(|| HOST_VERBS.get(i - CORE_VERBS.len()).map(|(n, _)| *n))
    }

    /// The program's exit status after `exit`, the one channel the kernel's `tests midden` reads back
    /// (`bg_poll`'s `Exited(i32)`; bit 31 stays clear): `rows` scrollback rows `[9:0]`, `cmds` lines planned
    /// `[15:10]` (an `exit` that closes is not counted), the last plan's kind `[17:16]` (0 Nothing, 1 Say,
    /// 2 Host, 3 Exec), its verb code `[25:18]`, `window` `[26]`, a loaded face `[27]`.
    pub const fn status(rows: u32, cmds: u32, kind: u32, verb: u8, window: bool, face: bool) -> i32 {
        let r = if rows > 0x3FF { 0x3FF } else { rows };
        let c = if cmds > 0x3F { 0x3F } else { cmds };
        (r | (c << 10) | ((kind & 3) << 16) | ((verb as u32) << 18) | ((window as u32) << 26) | ((face as u32) << 27)) as i32
    }

    /// [`status`] read back: `(rows, cmds, kind, verb, window, face)`.
    pub const fn status_parse(s: i32) -> (u32, u32, u32, u8, bool, bool) {
        let s = s as u32;
        (s & 0x3FF, (s >> 10) & 0x3F, (s >> 16) & 3, ((s >> 18) & 0xFF) as u8, (s >> 26) & 1 != 0, (s >> 27) & 1 != 0)
    }

    /// The kind field's name.
    pub const fn kind_name(kind: u32) -> &'static str {
        match kind {
            1 => "Say",
            2 => "Host",
            3 => "Exec",
            _ => "Nothing",
        }
    }
}

// ── MIDDENTWIN: the equivalence B171 wanted — ten lines through both rings' Facts over ONE fixture Volume ─────
#[cfg(test)]
mod middentwin_tests {
    extern crate std;
    use super::*;
    use std::println;

    /// The kernel shell's facts on an x86 build, field for field from `shell.rs` `midden_facts()`: the process
    /// verbs and bare-name launch exist on x86; `proc_rows` is `proc_table_rows()` at run time (memory's limit,
    /// WINDOWCAP3) — 16 here, and it reaches only the `storm` help line.
    const KERNEL_X86: Facts = Facts { aarch64: false, x86: true, v3d: false, proc_verbs: true, proc_rows: 16, exec: true };

    /// The ten lines. Each is planned by both rings over the same Volume.
    const LINES: [&str; 10] = [
        "",
        "help",
        "echo hello twin",
        "exit",
        "LS -l",
        "cat README.TXT",
        "cd /apps",
        "vug",
        "run /apps/VUG.ELF",
        "ping 10.0.2.2",
    ];

    fn vol() -> NameList<'static> {
        NameList(&["VUG.ELF", "STAT.ELF", "README.TXT"])
    }

    /// What a ring DOES with a plan: the plan's debug text when it fulfils it, the printed text when it refuses.
    fn kernel_outcome(p: &Plan) -> std::string::String {
        std::format!("{:?}", p) // the kernel's shell fulfils every verb its own facts register
    }
    fn ring3_outcome(p: &Plan) -> std::string::String {
        match p {
            Plan::Host { verb, .. } if !ring3::serves(verb) => std::format!("REFUSED {:?}", ring3::refusal(verb)),
            Plan::Exec { .. } => std::string::String::from("REFUSED exec"), // unreachable: Facts::ring3 has exec = false
            other => std::format!("{:?}", other),
        }
    }

    #[test]
    fn ten_lines_plan_the_same_where_both_rings_can_and_refuse_in_the_cores_words_where_one_cannot() {
        let r3 = Facts::ring3(true);
        let unknown = Plan::Say(Message::TerminalError("Unknown command. Type 'help' for assistance.".into()));
        let mut differ: std::vec::Vec<&str> = std::vec::Vec::new();
        for line in LINES {
            let kp = plan(line, &KERNEL_X86, &mut vol());
            let rp = plan(line, &r3, &mut vol());
            let (ko, ro) = (kernel_outcome(&kp), ring3_outcome(&rp));
            let same = ko == ro;
            let short = |o: &str| if o.len() > 160 { std::format!("{}... ({} bytes)", &o[..48], o.len()) } else { std::string::String::from(o) };
            println!("MIDDENTWIN-KAT | {:<20} | {} | kernel={} | ring3={}", std::format!("{:?}", line), if same { "same" } else { "DIFF" }, short(&ko), short(&ro));
            if same {
                continue;
            }
            differ.push(line);
            match line {
                // Both rings answer `help` in full; the texts differ ONLY in the lines keyed on the two facts ring 3
                // does not have (proc_verbs, exec) — the PROC line, the two bare-name lines and the storm line.
                "help" => {
                    let (Plan::Say(Message::TerminalOutput(k)), Plan::Say(Message::TerminalOutput(r))) = (&kp, &rp) else {
                        panic!("help must be a core answer on both rings")
                    };
                    let only_k: std::vec::Vec<&str> = k.lines().filter(|l| !r.lines().any(|x| x == *l)).collect();
                    let only_r: std::vec::Vec<&str> = r.lines().filter(|l| !k.lines().any(|x| x == *l)).collect();
                    assert!(only_r.is_empty(), "ring 3's help must be a subset of the kernel's: {:?}", only_r);
                    assert_eq!(only_k.len(), 4, "{:?}", only_k);
                    assert!(only_k[0].starts_with("PROC:"));
                    assert!(only_k[1].contains("<name>") && only_k[2].contains(".elf is optional") && only_k[3].contains("storm"));
                    println!("MIDDENTWIN-KAT | help lines only the kernel prints: {:?}", only_k);
                }
                // A bare program name and `run`: the kernel launches; ring 3 has no spawn-by-path, so the core
                // gives it the core's own refusal (the word is not a verb on these facts, and exec is off).
                "vug" => {
                    assert!(matches!(&kp, Plan::Exec { name, .. } if name == "VUG.ELF"));
                    assert_eq!(rp, unknown);
                }
                "run /apps/VUG.ELF" => {
                    assert_eq!(kp, Plan::Host { verb: "run".into(), rest: "/apps/VUG.ELF".into() });
                    assert_eq!(rp, unknown);
                }
                // The SAME plan on both rings; only the kernel carries a NIC verb, and ring 3 prints the core's refusal.
                "ping 10.0.2.2" => {
                    assert_eq!(kp, rp, "the plan itself is identical");
                    assert_eq!(ro, std::format!("REFUSED {:?}", ring3::refusal("ping")));
                }
                other => panic!("unexpected difference on {:?}: kernel={} ring3={}", other, ko, ro),
            }
        }
        assert_eq!(differ, ["help", "vug", "run /apps/VUG.ELF", "ping 10.0.2.2"], "the documented difference set");
        println!("MIDDENTWIN-KAT | lines={} same={} differ={:?}", LINES.len(), LINES.len() - differ.len(), differ);
    }

    #[test]
    fn ring3_facts_say_what_the_abi_can_do() {
        let f = Facts::ring3(true);
        assert!(f.x86 && !f.aarch64 && !f.exec && !f.proc_verbs && f.proc_rows == 0);
        let a = Facts::ring3(false);
        assert!(a.aarch64 && !a.x86);
        // Every served verb is a verb on the ring-3 facts (no served word the core would not route to Host).
        for v in ring3::SERVED {
            assert!(HOST_VERBS.iter().any(|(n, av)| n == v && av.on(&f)), "{} is served but not a host verb", v);
        }
        for v in ring3::CLOSES {
            assert!(CORE_VERBS.contains(v));
        }
    }

    #[test]
    fn the_witness_codes_round_trip() {
        for v in CORE_VERBS.iter().chain(HOST_VERBS.iter().map(|(n, _)| n)) {
            let c = ring3::verb_code(v);
            assert!(c != 0);
            assert_eq!(ring3::verb_of(c), Some(*v));
        }
        assert_eq!(ring3::verb_code("frobnicate"), 0);
        assert_eq!(ring3::verb_of(0), None);
        let s = ring3::status(37, 2, 2, ring3::verb_code("ls"), true, false);
        assert!(s > 0);
        assert_eq!(ring3::status_parse(s), (37, 2, 2, ring3::verb_code("ls"), true, false));
        assert_eq!(ring3::status_parse(ring3::status(5000, 99, 1, 0, false, true)), (0x3FF, 0x3F, 1, 0, false, true));
    }
}

// ── MIDDEN2 (LEDGER SR120) — the person's shell: compound lines, aliases, completion, history search, filters ─────
// MIDDEN.ELF (crates/user-midden) is the shell a person uses on the image. What it needs beyond `plan` is ring-neutral
// TEXT work — where a line splits into commands, what an alias expands to, which words complete a prefix, which
// history line a search finds, what `grep`/`wc`/`head`/`tail` make of a stream of lines — so it lives here, once,
// host-tested, for any ring (the host Midden can link the same functions). ADDITIVE: appended at the file tail, no
// existing item changes, `plan` still decides what ONE simple command means; this module only cuts a line into the
// simple commands `plan` is then asked about.
pub mod shell {
    use super::{Facts, CORE_VERBS, HOST_VERBS};
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    /// How the command AFTER this one is reached: `;` (always), `&&` (only when this one succeeded), `||` (only
    /// when it failed). The last command of a line carries `Seq`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Join {
        Seq,
        And,
        Or,
    }

    /// One command of a line: a pipeline of one or more stages (`a | b | c`), each stage's text as typed (trimmed,
    /// quotes kept — `plan` sees exactly what it always saw), and the operator that followed it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Cmd {
        pub stages: Vec<String>,
        pub join: Join,
    }

    /// Why a line does not split.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SplitError {
        /// An operator with no command on one side (`| wc`, `ls |`, `&& pwd`, `ls ||`).
        Empty(&'static str),
        /// A quote opened and never closed.
        Unterminated(char),
    }

    impl SplitError {
        /// The sentence the shell prints.
        pub fn text(&self) -> String {
            match self {
                SplitError::Empty(op) => format!("syntax: `{}` needs a command on both sides", op),
                SplitError::Unterminated(q) => format!("syntax: unterminated {} quote", q),
            }
        }
        /// The witness word.
        pub fn word(&self) -> &'static str {
            match self {
                SplitError::Empty(_) => "empty-command",
                SplitError::Unterminated(_) => "unterminated-quote",
            }
        }
    }

    /// Cut `line` into commands at `;`, `&&`, `||` and stages at `|`, outside single and double quotes. A `#` that
    /// starts a word outside quotes ends the line (a comment, so an rc file can carry them). A lone `&` is an
    /// ordinary character (every ring-3 launch is already detached). An empty command between `;`s is dropped; an
    /// empty side of `|`, `&&` or `||` is a [`SplitError::Empty`]. An empty line is `Ok(vec![])`.
    pub fn split(line: &str) -> Result<Vec<Cmd>, SplitError> {
        let b = line.as_bytes();
        let mut cmds: Vec<Cmd> = Vec::new();
        let mut stages: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut quote: Option<u8> = None;
        let mut i = 0;
        // The operator that ended the previous command, for the "nothing after it" check at the end.
        let mut pending: Option<&'static str> = None;
        macro_rules! end_stage {
            ($op:expr) => {{
                let s = cur.trim();
                if s.is_empty() {
                    return Err(SplitError::Empty($op));
                }
                stages.push(s.to_string());
                cur.clear();
            }};
        }
        while i < b.len() {
            let c = b[i];
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
                cur.push(c as char);
                i += 1;
                continue;
            }
            match c {
                b'\'' | b'"' => {
                    quote = Some(c);
                    cur.push(c as char);
                    i += 1;
                }
                b'#' if cur.is_empty() || cur.ends_with(|ch: char| ch.is_ascii_whitespace()) => break,
                b'|' if b.get(i + 1) == Some(&b'|') => {
                    end_stage!("||");
                    cmds.push(Cmd { stages: core::mem::take(&mut stages), join: Join::Or });
                    pending = Some("||");
                    i += 2;
                }
                b'&' if b.get(i + 1) == Some(&b'&') => {
                    end_stage!("&&");
                    cmds.push(Cmd { stages: core::mem::take(&mut stages), join: Join::And });
                    pending = Some("&&");
                    i += 2;
                }
                b'|' => {
                    end_stage!("|");
                    pending = Some("|");
                    i += 1;
                }
                b';' => {
                    let s = cur.trim();
                    if !s.is_empty() {
                        stages.push(s.to_string());
                        cur.clear();
                    } else if !stages.is_empty() {
                        return Err(SplitError::Empty("|"));
                    }
                    if !stages.is_empty() {
                        cmds.push(Cmd { stages: core::mem::take(&mut stages), join: Join::Seq });
                    } else if let Some(op @ ("&&" | "||")) = pending {
                        return Err(SplitError::Empty(op));
                    }
                    pending = None;
                    i += 1;
                }
                _ => {
                    // Multi-byte UTF-8 is copied whole so a non-ASCII word survives the cut.
                    let len = utf8_len(c);
                    let end = (i + len).min(b.len());
                    match core::str::from_utf8(&b[i..end]) {
                        Ok(s) => cur.push_str(s),
                        Err(_) => cur.push('?'),
                    }
                    i = end;
                }
            }
        }
        if let Some(q) = quote {
            return Err(SplitError::Unterminated(q as char));
        }
        let s = cur.trim();
        if !s.is_empty() {
            stages.push(s.to_string());
        } else if let Some(op) = pending {
            return Err(SplitError::Empty(op));
        }
        if !stages.is_empty() {
            cmds.push(Cmd { stages, join: Join::Seq });
        }
        Ok(cmds)
    }

    fn utf8_len(lead: u8) -> usize {
        match lead {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        }
    }

    /// Does the command after one joined by `prev` run, given the status of the last command that RAN? The shell's
    /// rule: `a && b || c` runs `c` when `a` failed (b is skipped and the status stays a's).
    pub const fn should_run(prev: Join, last_ok: bool) -> bool {
        match prev {
            Join::Seq => true,
            Join::And => last_ok,
            Join::Or => !last_ok,
        }
    }

    /// `stage` cut into words with one level of `'…'` / `"…"` removed (a quoted word may hold spaces). The words a
    /// filter verb reads; `plan` keeps its own whitespace split.
    pub fn words(stage: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quote: Option<char> = None;
        let mut any = false;
        for c in stage.chars() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => cur.push(c),
                None if c == '\'' || c == '"' => {
                    quote = Some(c);
                    any = true;
                }
                None if c.is_whitespace() => {
                    if any || !cur.is_empty() {
                        out.push(core::mem::take(&mut cur));
                    }
                    any = false;
                }
                None => cur.push(c),
            }
        }
        if any || !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    // ---- aliases --------------------------------------------------------------------------------------------------

    /// Is `name` a word an alias may be called? Letters, digits, `_`, `-`, `.`; not empty, not starting with `-`.
    pub fn alias_name_ok(name: &str) -> bool {
        !name.is_empty() && !name.starts_with('-') && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    }

    /// `name=value` (the `alias` verb's operand, the rest of its line): the name and the value with one level of
    /// quotes removed. `None` = not a definition (no `=`, or a name [`alias_name_ok`] refuses).
    pub fn parse_alias(def: &str) -> Option<(String, String)> {
        let def = def.trim();
        let (n, v) = def.split_once('=')?;
        if !alias_name_ok(n) {
            return None;
        }
        let v = v.trim();
        let v = if v.len() >= 2 && ((v.starts_with('\'') && v.ends_with('\'')) || (v.starts_with('"') && v.ends_with('"'))) {
            &v[1..v.len() - 1]
        } else {
            v
        };
        Some((n.to_string(), v.to_string()))
    }

    /// `stage` with its FIRST word replaced by its alias, repeatedly (an alias may name another), but never expanding
    /// a name a second time — so `alias ls='ls -l'` is `ls -l` and two aliases naming each other stop. `None` = the
    /// first word is no alias. Expansion never crosses a `|`/`;` (the caller expands each stage).
    pub fn expand_alias(stage: &str, aliases: &[(String, String)]) -> Option<String> {
        let mut cur = stage.trim().to_string();
        let mut seen: Vec<String> = Vec::new();
        let mut changed = false;
        loop {
            let first = cur.split_whitespace().next().unwrap_or("");
            if first.is_empty() || seen.iter().any(|s| s == first) {
                break;
            }
            let Some((_, v)) = aliases.iter().find(|(n, _)| n == first) else {
                break;
            };
            seen.push(first.to_string());
            let rest = cur[first.len()..].to_string();
            cur = format!("{}{}", v, rest);
            changed = true;
            if seen.len() >= 16 {
                break;
            }
        }
        if changed { Some(cur) } else { None }
    }

    // ---- completion -----------------------------------------------------------------------------------------------

    /// The word the caret is in: `(start byte, command position)`. The word runs from the last whitespace or
    /// operator byte before `caret`; it is in COMMAND position when only whitespace separates it from the line start
    /// or from a `|`, `;`, `&&` or `||` (so it names a verb or a program, not a path operand).
    pub fn word_at(line: &str, caret: usize) -> (usize, bool) {
        let b = line.as_bytes();
        let caret = caret.min(b.len());
        let mut s = caret;
        while s > 0 && !matches!(b[s - 1], b' ' | b'\t' | b'|' | b';' | b'&') {
            s -= 1;
        }
        let mut j = s;
        while j > 0 && matches!(b[j - 1], b' ' | b'\t') {
            j -= 1;
        }
        (s, j == 0 || matches!(b[j - 1], b'|' | b';' | b'&'))
    }

    /// The longest prefix every candidate shares, compared ASCII case-insensitively (a FAT name answers to any case),
    /// in the FIRST candidate's spelling. Empty for no candidates.
    pub fn common_prefix(cands: &[String]) -> String {
        let Some(first) = cands.first() else {
            return String::new();
        };
        let mut n = first.len();
        for c in &cands[1..] {
            let m = first.bytes().zip(c.bytes()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            n = n.min(m);
        }
        while !first.is_char_boundary(n) {
            n -= 1;
        }
        first[..n].to_string()
    }

    /// Does `name` complete `prefix` (ASCII case-insensitive)?
    pub fn completes(name: &str, prefix: &str) -> bool {
        name.len() >= prefix.len() && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    }

    /// The command-position candidates for `prefix`: every verb registered on `facts` (the core's table, so a word the
    /// build cannot dispatch is never offered), then `extra` (the ring's own builtins, aliases and program names),
    /// sorted, without duplicates.
    pub fn verb_candidates(prefix: &str, facts: &Facts, extra: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = CORE_VERBS
            .iter()
            .copied()
            .chain(HOST_VERBS.iter().filter(|(_, a)| a.on(facts)).map(|(n, _)| *n))
            .chain(extra.iter().copied())
            .filter(|n| completes(n, prefix))
            .map(|n| n.to_string())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The listing names (`PATH_R_LIST`'s lines: a directory carries a trailing `/`) that complete `leaf`, sorted.
    pub fn path_candidates<'a>(leaf: &str, names: impl Iterator<Item = &'a str>) -> Vec<String> {
        let mut v: Vec<String> = names.filter(|n| !n.is_empty() && completes(n, leaf)).map(|n| n.to_string()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// What Tab does to the word: `Some((replacement, finished))` when the candidates extend it (`finished` = exactly
    /// one candidate, so the caller adds a space unless it is a directory), `None` when they do not (the caller lists
    /// them instead).
    pub fn complete_word(word: &str, cands: &[String]) -> Option<(String, bool)> {
        match cands.len() {
            0 => None,
            1 => Some((cands[0].clone(), true)),
            _ => {
                let p = common_prefix(cands);
                if p.len() > word.len() { Some((p, false)) } else { None }
            }
        }
    }

    // ---- history --------------------------------------------------------------------------------------------------

    /// The newest history index below `before` whose line contains `q` (Ctrl+R's step: the first search passes
    /// `hist.len()`, the next passes the last hit). `None` = no older match. An empty `q` matches nothing.
    pub fn search_back(hist: &[String], q: &str, before: usize) -> Option<usize> {
        if q.is_empty() {
            return None;
        }
        let end = before.min(hist.len());
        (0..end).rev().find(|&i| hist[i].contains(q))
    }

    /// Does a line belong in the history? Not empty, not starting with a space (the operator's "do not keep this"),
    /// and not secret-shaped ([`super::secret_from`]: a password never reaches the history file).
    pub fn keep_in_history(line: &str) -> bool {
        !line.trim().is_empty() && !line.starts_with(' ') && super::secret_from(line).is_none()
    }

    // ---- filters --------------------------------------------------------------------------------------------------

    /// The verbs that read a stream of lines (a pipeline's input, or a file the ring read): `grep`, `wc`, `head`,
    /// `tail`. Every one is a `HOST_VERBS` row registered `Always`; the ring that serves them calls [`filter`].
    pub const FILTERS: &[&str] = &["grep", "wc", "head", "tail"];

    /// The ring's own words, served before `plan` is asked (none is in `HOST_VERBS` on `Facts::ring3`): alias
    /// table, `source <file>`, the job list and the refused `kill`, and `run`.
    pub const BUILTINS: &[&str] = &["alias", "unalias", "source", "jobs", "kill", "run"];

    /// `verb args` over `input` — the lines a pipeline handed it, or the file's lines. `Err` is the usage or refusal
    /// sentence. `grep [-i] [-v] [-c] [-n] <text>` matches a SUBSTRING (no regex: said in its usage line);
    /// `wc [-l] [-w] [-c]` prints `lines words bytes` (bytes count each line plus its newline); `head`/`tail`
    /// `[-n N | -N]` keep the first/last N lines (10).
    pub fn filter(verb: &str, args: &[String], input: &[String]) -> Result<Vec<String>, String> {
        let flags: Vec<&str> = args.iter().map(|s| s.as_str()).filter(|a| a.starts_with('-') && a.len() > 1).collect();
        let ops: Vec<&str> = args.iter().map(|s| s.as_str()).filter(|a| !(a.starts_with('-') && a.len() > 1)).collect();
        match verb {
            "grep" => {
                let mut ci = false;
                let mut inv = false;
                let mut count = false;
                let mut num = false;
                for f in &flags {
                    for c in f[1..].chars() {
                        match c {
                            'i' => ci = true,
                            'v' => inv = true,
                            'c' => count = true,
                            'n' => num = true,
                            _ => return Err(format!("grep: -{}: unknown flag (usage: grep [-i] [-v] [-c] [-n] <text> [file] - a substring, not a regex)", c)),
                        }
                    }
                }
                let Some(pat) = ops.first() else {
                    return Err("usage: grep [-i] [-v] [-c] [-n] <text> [file] - a substring, not a regex".to_string());
                };
                let lp = pat.to_ascii_lowercase();
                let mut out = Vec::new();
                let mut n = 0usize;
                for (k, l) in input.iter().enumerate() {
                    let hit = if ci { l.to_ascii_lowercase().contains(&lp) } else { l.contains(pat) };
                    if hit != inv {
                        n += 1;
                        if !count {
                            out.push(if num { format!("{}:{}", k + 1, l) } else { l.clone() });
                        }
                    }
                }
                if count {
                    out.push(format!("{}", n));
                }
                Ok(out)
            }
            "wc" => {
                let l = input.len();
                let w: usize = input.iter().map(|s| s.split_whitespace().count()).sum();
                let c: usize = input.iter().map(|s| s.len() + 1).sum();
                let (mut sl, mut sw, mut sc) = (false, false, false);
                for f in &flags {
                    for ch in f[1..].chars() {
                        match ch {
                            'l' => sl = true,
                            'w' => sw = true,
                            'c' => sc = true,
                            _ => return Err(format!("wc: -{}: unknown flag (usage: wc [-l] [-w] [-c] [file])", ch)),
                        }
                    }
                }
                if !(sl || sw || sc) {
                    sl = true;
                    sw = true;
                    sc = true;
                }
                let mut parts: Vec<String> = Vec::new();
                if sl {
                    parts.push(format!("{}", l));
                }
                if sw {
                    parts.push(format!("{}", w));
                }
                if sc {
                    parts.push(format!("{}", c));
                }
                Ok(alloc::vec![parts.join(" ")])
            }
            "head" | "tail" => {
                let mut n = 10usize;
                let mut it = args.iter();
                while let Some(a) = it.next() {
                    if a == "-n" {
                        let Some(v) = it.next().and_then(|v| v.parse::<usize>().ok()) else {
                            return Err(format!("usage: {} [-n N | -N] [file]", verb));
                        };
                        n = v;
                    } else if let Some(v) = a.strip_prefix('-') {
                        match v.parse::<usize>() {
                            Ok(v) => n = v,
                            Err(_) => return Err(format!("usage: {} [-n N | -N] [file]", verb)),
                        }
                    }
                }
                let v: Vec<String> = if verb == "head" {
                    input.iter().take(n).cloned().collect()
                } else {
                    input.iter().skip(input.len().saturating_sub(n)).cloned().collect()
                };
                Ok(v)
            }
            other => Err(format!("{}: not a filter", other)),
        }
    }

    /// The file operand of a filter line (what it reads when no pipeline feeds it): the operand after the pattern for
    /// `grep`, the first operand for the others; `-n`'s value is not an operand.
    pub fn filter_file<'a>(verb: &str, args: &'a [String]) -> Option<&'a str> {
        let mut ops: Vec<&str> = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if (verb == "head" || verb == "tail") && a == "-n" {
                it.next();
                continue;
            }
            if a.starts_with('-') && a.len() > 1 {
                continue;
            }
            ops.push(a.as_str());
        }
        let skip = if verb == "grep" { 1 } else { 0 };
        ops.get(skip).copied()
    }
}

// ── MIDDEN2: the shell module's host tests ────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod midden2_tests {
    extern crate std;
    use super::shell::*;
    use super::Facts;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    fn st(c: &Cmd) -> Vec<&str> {
        c.stages.iter().map(|s| s.as_str()).collect()
    }

    #[test]
    fn a_line_cuts_into_commands_and_stages_outside_quotes() {
        let c = split("ls /apps | grep ELF && pwd ; echo 'a | b' || echo \"x;y\"").unwrap();
        assert_eq!(c.len(), 4);
        assert_eq!(st(&c[0]), ["ls /apps", "grep ELF"]);
        assert_eq!(c[0].join, Join::And);
        assert_eq!(st(&c[1]), ["pwd"]);
        assert_eq!(c[1].join, Join::Seq);
        assert_eq!(st(&c[2]), ["echo 'a | b'"]);
        assert_eq!(c[2].join, Join::Or);
        assert_eq!(st(&c[3]), ["echo \"x;y\""]);
        assert_eq!(c[3].join, Join::Seq);
        assert_eq!(split("").unwrap(), []);
        assert_eq!(split("  ;; ls ; ").unwrap().len(), 1);
        assert_eq!(split("echo hi # a comment | not a pipe").unwrap(), [Cmd { stages: ["echo hi".to_string()].into(), join: Join::Seq }]);
        assert_eq!(split("echo a#b").unwrap()[0].stages[0], "echo a#b");
    }

    #[test]
    fn an_operator_with_an_empty_side_or_an_open_quote_is_refused_by_name() {
        assert_eq!(split("| wc"), Err(SplitError::Empty("|")));
        assert_eq!(split("ls |"), Err(SplitError::Empty("|")));
        assert_eq!(split("ls && "), Err(SplitError::Empty("&&")));
        assert_eq!(split("&& ls"), Err(SplitError::Empty("&&")));
        assert_eq!(split("ls || ; pwd"), Err(SplitError::Empty("||")));
        assert_eq!(split("echo 'open"), Err(SplitError::Unterminated('\'')));
        assert!(split("ls |").unwrap_err().text().contains("`|`"));
    }

    #[test]
    fn and_or_follow_the_last_command_that_ran() {
        assert!(should_run(Join::Seq, false) && should_run(Join::Seq, true));
        assert!(should_run(Join::And, true) && !should_run(Join::And, false));
        assert!(should_run(Join::Or, false) && !should_run(Join::Or, true));
    }

    #[test]
    fn words_drop_one_level_of_quotes() {
        assert_eq!(words("grep 'a b' file"), ["grep", "a b", "file"]);
        assert_eq!(words("echo \"\" x"), ["echo", "", "x"]);
    }

    #[test]
    fn aliases_define_expand_and_stop_on_a_cycle() {
        assert_eq!(parse_alias("ll='ls -l'"), Some(("ll".into(), "ls -l".into())));
        assert_eq!(parse_alias("x=pwd"), Some(("x".into(), "pwd".into())));
        assert_eq!(parse_alias("no equals"), None);
        assert_eq!(parse_alias("-x=pwd"), None);
        let a: Vec<(String, String)> = [("ll", "ls -l"), ("ls", "ls -a"), ("p", "q"), ("q", "p")].iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        assert_eq!(expand_alias("ll /apps", &a).as_deref(), Some("ls -a -l /apps"));
        assert_eq!(expand_alias("ls", &a).as_deref(), Some("ls -a"));
        assert_eq!(expand_alias("p", &a).as_deref(), Some("p"));
        assert_eq!(expand_alias("pwd", &a), None);
    }

    #[test]
    fn completion_finds_the_word_its_position_and_the_candidates() {
        assert_eq!(word_at("histo", 5), (0, true));
        assert_eq!(word_at("ls /ap", 6), (3, false));
        assert_eq!(word_at("ls | gr", 7), (5, true));
        assert_eq!(word_at("pwd && ec", 9), (7, true));
        let f = Facts::ring3(true);
        assert_eq!(verb_candidates("histo", &f, &[]), ["history"]);
        assert_eq!(verb_candidates("al", &f, BUILTINS), ["alias"]);
        assert!(verb_candidates("r", &f, BUILTINS).contains(&"run".to_string()));
        let names = ["MIDDEN.ELF", "MAP.ELF", "docs/"];
        let c = path_candidates("mi", names.iter().copied());
        assert_eq!(c, ["MIDDEN.ELF"]);
        assert_eq!(complete_word("mi", &c), Some(("MIDDEN.ELF".into(), true)));
        let c = path_candidates("m", names.iter().copied());
        assert_eq!(complete_word("m", &c), None, "MAP/MIDDEN share only the M");
        let c: Vec<String> = ["history".into(), "historic".into()].into();
        assert_eq!(complete_word("h", &c), Some(("histor".into(), false)));
    }

    #[test]
    fn history_search_walks_older_and_secrets_are_not_kept() {
        let h: Vec<String> = ["cd /apps", "ls", "cd /home", "pwd"].iter().map(|s| s.to_string()).collect();
        assert_eq!(search_back(&h, "cd", h.len()), Some(2));
        assert_eq!(search_back(&h, "cd", 2), Some(0));
        assert_eq!(search_back(&h, "cd", 0), None);
        assert_eq!(search_back(&h, "", 4), None);
        assert!(keep_in_history("ls"));
        assert!(!keep_in_history(" ls"));
        assert!(!keep_in_history("login peter qwerty"));
        assert!(!keep_in_history("   "));
    }

    #[test]
    fn the_filters_read_a_stream_of_lines() {
        let s = |v: &[&str]| -> Vec<String> { v.iter().map(|x| x.to_string()).collect() };
        let input = s(&["alpha beta gamma", "Beta two", "three"]);
        assert_eq!(filter("wc", &[], &s(&["alpha beta gamma"])).unwrap(), ["1 3 17"]);
        assert_eq!(filter("wc", &s(&["-l"]), &input).unwrap(), ["3"]);
        assert_eq!(filter("grep", &s(&["beta"]), &input).unwrap(), ["alpha beta gamma"]);
        assert_eq!(filter("grep", &s(&["-i", "beta"]), &input).unwrap(), ["alpha beta gamma", "Beta two"]);
        assert_eq!(filter("grep", &s(&["-vc", "beta"]), &input).unwrap(), ["2"]);
        assert_eq!(filter("grep", &s(&["-n", "three"]), &input).unwrap(), ["3:three"]);
        assert!(filter("grep", &[], &input).unwrap_err().starts_with("usage: grep"));
        assert_eq!(filter("head", &s(&["-n", "1"]), &input).unwrap(), ["alpha beta gamma"]);
        assert_eq!(filter("tail", &s(&["-2"]), &input).unwrap(), ["Beta two", "three"]);
        assert_eq!(filter_file("grep", &s(&["-i", "x", "f.txt"])), Some("f.txt"));
        assert_eq!(filter_file("head", &s(&["-n", "5", "f.txt"])), Some("f.txt"));
        assert_eq!(filter_file("wc", &s(&["-l"])), None);
        for v in FILTERS {
            assert!(super::is_verb(v, &Facts::ring3(true)), "{} must be a host verb on ring 3", v);
        }
        for v in BUILTINS {
            assert!(!super::shell::FILTERS.contains(v));
        }
    }
}
