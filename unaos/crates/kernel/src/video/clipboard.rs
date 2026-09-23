// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! APPCLIP — **the clipboard, and the delivery of KEYMAP's edit actions to something that acts on
//! them.**
//!
//! # Why this file exists
//!
//! R61: *"i prefer command-c and friends (alt-c on pc) then there's no special case for the command
//! line to resolve that usability question."* KEYMAP built the first half of that sentence — the
//! binding table resolves `⌘C`/`⌘V`/`⌘X`/`⌘A` to [`Action::Copy`]/[`Action::Paste`]/[`Action::Cut`]/
//! [`Action::SelectAll`] and stops at the decoder, with no delivery path and no consumer
//! (`docs/dev/OS/08_VIDEO/keymap.md` §6 named the seam rather than half-building it). This file is
//! the other half: one buffer, one consumer, and the three seams between them.
//!
//! # The three parts, in the order an event travels them
//!
//! 1. **DELIVERY.** [`crate::pal::Event::Action`] is a THIRD KIND of input event — neither a key
//!    nor a pointer report — pushed by the two HID decoders on the NON-capture arm of the chord
//!    they already resolve. It travels the same ring every keystroke travels, and both arches' EL0
//!    routers pack it for ring 3 as `una_abi::INPUT_EV_ACTION` with [`action_code`] in the payload,
//!    so a ring-3 program receives a `⌘V` exactly the way it receives a key.
//!
//! 2. **THE CLIPBOARD** — this module's [`set`]/[`get`]. ONE buffer, [`CLIP_CAP`] bytes, TEXT ONLY,
//!    and **owned by the session**: every [`set`] stamps the live session epoch, and a [`get`]
//!    whose stamp is not the live epoch clears the buffer and returns nothing. A later user cannot
//!    paste the previous user's text, and that is enforced on the READ rather than on a log-out
//!    hook — see *Ownership* below for why, and for what would change if a hook existed.
//!
//! 3. **THE FIRST CONSUMER: the terminal.** [`terminal_action`] is what the shell's line editor
//!    does with an action. `Paste` types the clipboard into the current line **through the same
//!    path a typed byte takes** — it pushes each byte back onto the ring as
//!    [`crate::pal::Event::Key`], so the line edit, the echo and the wire are identical to the
//!    operator having typed it and no second entry into the editor exists to keep in step. `Copy`
//!    takes the whole current line, because **there is no selection model in this tree** — there is
//!    nothing on any surface that records "these characters are selected", so a line is the largest
//!    honest unit. `Cut` and `SelectAll` are therefore ACCEPTED AND WITNESSED as `unsupported`
//!    (`[clip] unsupported action=…`), never silently dropped: a selection model is the next arc and
//!    an operator pressing `⌘X` today must be able to see on the wire that the chord arrived and
//!    the desktop declined it.
//!
//! **No `Ctrl-C` special case exists anywhere in this file or in the path it completes** — that is
//! R61's whole point. The table never claims `Ctrl-C` (`keymap.md` §4), `hid_key_ascii` still hands
//! the shell `0x03`, and nothing here tests a modifier, a focus or a window kind.
//!
//! # Ownership — why the epoch is checked on READ and not cleared at log-out
//!
//! The brief's first choice was a clear folded into `fs::users::logout`. That file belongs to
//! SECLOGIN this round and is read-only to this arc, so the clear lives on the OTHER side of the
//! same fact: [`set`] stamps `session_epoch()` and [`fresh_len`] compares it, so the FIRST read
//! after a session boundary clears the buffer and reports `stale=yes`. The two are equivalent for
//! the property that matters (no read of another session's text can succeed) and the read-side form
//! is strictly harder to get wrong: it cannot be bypassed by a session that ends some other way
//! than through `logout`. What it does NOT do is shrink the window in which the bytes still sit in
//! kernel RAM; if a hook is added to `users.rs` later, it calls [`clear`] and this check stays as
//! the belt.
//!
//! `session_epoch()` is `0` on an image with no `login` feature and no EL0 regime. That does not
//! weaken the check: the stamp and the comparison move together, and the fixture proves the
//! mechanism by AGEING a stamp rather than by opening a session, so it measures the same code on
//! every build (see [`selftest`]).
//!
//! # What is NOT here
//!
//! **A ring-3 clipboard API.** Ring 3 receives the `⌘C` (delivery, part 1) but cannot read or write
//! the buffer: that needs TWO syscalls this arc does not add — a `SYS_CLIP_SET(ptr, len)` and a
//! `SYS_CLIP_GET(ptr, cap) -> len`, each with the same text-only and capacity refusals [`set`]
//! makes, and each owing an ownership question this kernel has not answered (may a background
//! program overwrite the clipboard of a foreground one?). Named here rather than half-built.
//!
//! **A selection model**, and with it a real `Cut` and `SelectAll` — see part 3 above.

use crate::video::keymap::Action;
use spin::Mutex;

/// The clipboard's capacity in bytes. A [`set`] longer than this is REFUSED with a witness rather
/// than truncated: a silently shortened paste is a corrupted one, and the operator has no way to
/// see it happened. 4 KiB because the consumer that exists is a shell line editor and the buffer is
/// static kernel `.bss` shared by the whole machine — this is a clipboard, not a transfer buffer.
pub const CLIP_CAP: usize = 4096;

/// The one buffer. `stamped` is separate from `len`: an EMPTY clipboard that was set this session
/// and a never-set one are the same length and must not be the same thing to the staleness check,
/// or the first `get` on a fresh boot would report `stale=yes` off an epoch that never moved.
struct Clip {
    buf: [u8; CLIP_CAP],
    len: usize,
    /// The session epoch this content was [`set`] under.
    epoch: u64, // u64 since SECLOGIN M5 widened the session epoch (fold 9081402e); the seat widened this copy at the fold seam 2026-09-23 — the epoch is never narrowed
    /// Has anything ever been set? `false` means "empty by construction", not "cleared".
    stamped: bool,
}

static CLIP: Mutex<Clip> = Mutex::new(Clip {
    buf: [0; CLIP_CAP],
    len: 0,
    epoch: 0,
    stamped: false,
});

/// The LIVE session epoch, read exactly the way `fs::users::arch_session_epoch` reads it (that
/// function is private to a file this arc does not edit, so the shape is duplicated and not the
/// value). `0` where there is no `login` feature or no EL0 regime to carry a session — see the
/// module header on why that does not weaken the ownership check.
fn session_epoch() -> u64 {
    #[cfg(all(feature = "login", any(target_arch = "x86_64", feature = "aarch64_el0")))]
    {
        return crate::arch::syscall::session_epoch();
    }
    #[cfg(not(all(feature = "login", any(target_arch = "x86_64", feature = "aarch64_el0"))))]
    0
}

/// TEXT ONLY, and the definition is on the wire rather than in prose: printable ASCII plus the two
/// whitespace bytes a line editor can actually carry. A control byte in the clipboard is how a
/// paste turns into an unintended `\r` dispatch of a half-typed command, which is why this refuses
/// rather than filters — a filtered paste is a paste the operator did not ask for.
const fn is_text(b: u8) -> bool {
    (b >= 0x20 && b <= 0x7E) || b == b'\n' || b == b'\t'
}

/// Put `bytes` on the clipboard, stamped with the live session epoch. Returns `false` and changes
/// nothing on a refusal; both refusals carry their reason on the wire.
pub fn set(bytes: &[u8]) -> bool {
    if bytes.len() > CLIP_CAP {
        serial_println!(
            "[clip] refuse reason=too-large len={} cap={}",
            bytes.len(),
            CLIP_CAP
        );
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        if !is_text(bytes[i]) {
            serial_println!(
                "[clip] refuse reason=non-text off={} byte={:#04x}",
                i,
                bytes[i]
            );
            return false;
        }
        i += 1;
    }
    let epoch = session_epoch();
    {
        let mut c = CLIP.lock();
        c.buf[..bytes.len()].copy_from_slice(bytes);
        c.len = bytes.len();
        c.epoch = epoch;
        c.stamped = true;
    }
    serial_println!("[clip] set len={} epoch={}", bytes.len(), epoch);
    true
}

/// Drop the contents. The ONE place the buffer is emptied, so a future `users::logout` hook and the
/// staleness check below cannot diverge on what "cleared" means.
pub fn clear(reason: &str) {
    let had = {
        let mut c = CLIP.lock();
        let had = c.len;
        c.len = 0;
        c.stamped = false;
        had
    };
    serial_println!("[clip] clear len={} reason={}", had, reason);
}

/// **THE OWNERSHIP GATE.** The readable length of the clipboard right now — and the point at which a
/// buffer belonging to a CLOSED session is destroyed. Emits the `[clip] get` witness, so every read
/// of the clipboard is on the wire with the epoch it was judged against.
pub fn fresh_len() -> usize {
    let live = session_epoch();
    let (len, epoch, stale) = {
        let mut c = CLIP.lock();
        let stale = c.stamped && c.epoch != live;
        if stale {
            c.len = 0;
            c.stamped = false;
        }
        (c.len, c.epoch, stale)
    };
    serial_println!(
        "[clip] get len={} epoch={} stale={}",
        len,
        if stale { epoch } else { live },
        if stale { "yes" } else { "no" }
    );
    len
}

/// Copy out of the buffer WITHOUT the epoch check or the witness — the raw half of [`get`], split
/// out so [`paste_into_ring`] can walk a long clipboard in small chunks and never hold this lock
/// across a `pal::push_event` (which takes the ring lock with interrupts off). Returns how many
/// bytes were copied.
fn copy_out(off: usize, out: &mut [u8]) -> usize {
    let c = CLIP.lock();
    if off >= c.len {
        return 0;
    }
    let n = core::cmp::min(out.len(), c.len - off);
    out[..n].copy_from_slice(&c.buf[off..off + n]);
    n
}

/// Read the clipboard into `out`, epoch-checked. Returns the number of bytes written — `0` for an
/// empty clipboard AND for one whose session has closed, which are the same thing to a reader.
pub fn get(out: &mut [u8]) -> usize {
    let n = core::cmp::min(fresh_len(), out.len());
    copy_out(0, &mut out[..n])
}

/// PASTE — type the clipboard into whatever is reading the keyboard, one byte at a time, **through
/// the ring**. Not a call into the line editor: `Event::Key` is what the editor already consumes,
/// so the bytes are indistinguishable from typed ones at every stage below this line — the echo,
/// the backspace accounting, the `\n` dispatch and the `[uvug10]` key census all behave exactly as
/// they do for an operator's hands. Returns the number of bytes pushed.
///
/// Chunked at 64 bytes: the CLIP lock is released before any `push_event`, so the two locks are
/// never held at once, and no 4 KiB copy ever lands on a kernel stack (U7STK).
pub fn paste_into_ring() -> usize {
    let total = fresh_len();
    let mut off = 0usize;
    while off < total {
        let mut chunk = [0u8; 64];
        let took = copy_out(off, &mut chunk);
        if took == 0 {
            break;
        }
        let mut i = 0;
        while i < took {
            crate::pal::push_event(crate::pal::Event::Key(chunk[i]));
            i += 1;
        }
        off += took;
    }
    serial_println!("[clip] paste bytes={} (pushed as Event::Key — the typed path)", off);
    off
}

/// **THE TERMINAL'S CONSUMER.** What the shell's line editor does with one resolved [`Action`].
/// `line` is the editor's CURRENT INPUT LINE (`console::Console::current_input` at the call site;
/// the fixture passes its own buffer). Returns the field this action contributes to the witness —
/// `ok`, `refused`, `unsupported` or `ignored` — so the caller never has to restate the outcome.
///
/// Every arm is terminal and none of them is silent, which is the rule this seam exists to keep: an
/// action the desktop cannot honour must be visibly declined, or the operator learns that `⌘X` does
/// nothing and stops reporting it.
pub fn terminal_action(act: Action, line: &str) -> &'static str {
    match act {
        // There is NO SELECTION MODEL — nothing anywhere records which characters are selected — so
        // the largest honest unit is the line the editor holds. Stated here and in the module header
        // because a reader who assumes a selection will read this as a bug.
        Action::Copy => {
            if set(line.as_bytes()) {
                serial_println!("[clip] copy unit=line len={}", line.len());
                "ok"
            } else {
                "refused"
            }
        }
        Action::Paste => {
            paste_into_ring();
            "ok"
        }
        Action::Cut | Action::SelectAll => {
            serial_println!(
                "[clip] unsupported action={} reason=no-selection-model",
                act.name()
            );
            "unsupported"
        }
        // Not this consumer's business: the capture actions are delivered and acted on at the
        // decoder (`Action::is_capture`), and `LogOut` is KEYMAP's slot, bound by nobody.
        Action::Screenshot | Action::ScreenshotRegion | Action::LogOut => "ignored",
    }
}

/// The RING-3 WIRE VALUE of an action — the payload of a `una_abi::INPUT_EV_ACTION` event, packed by
/// both arches' `pack_input`. Lives here and not in `keymap.rs` because it is an ABI fact about the
/// input ring, not a property of the binding table; KEYMAP owns what a chord MEANS, this owns how
/// that meaning crosses the ring.
///
/// The numbering is EXPLICIT and starts at 1, so it survives a reordering of the enum and so `0` is
/// never a valid action. Adding a variant appends; changing a value here is an ABI break.
pub const fn action_code(a: Action) -> u64 {
    match a {
        Action::Screenshot => 1,
        Action::ScreenshotRegion => 2,
        Action::Copy => 3,
        Action::Cut => 4,
        Action::Paste => 5,
        Action::SelectAll => 6,
        Action::LogOut => 7,
    }
}

// --- fixture ---------------------------------------------------------------------------------

/// APPCLIP — the arc's gate, chained from `drivers::ehci::parser_selftest` BESIDE
/// `video::keymap::selftest` (not inside it: KEYMAP's verdict must stay exactly what it was, and a
/// field added to somebody else's line is a field its spec row cannot see).
///
/// It drives the WHOLE path and not a piece of it: the actions go through `pal::push_event` into the
/// REAL ring and come back out of `pal::next_event`, the consumer is the shipped
/// [`terminal_action`], and the paste arrives as `Event::Key` events on that same ring which this
/// fixture then applies with the line editor's own rule (printable ASCII extends the line —
/// `main::handle_key`). That is what makes `line_match=` a comparison and not a restatement: the
/// bytes are compared after a round trip through the ring, so a delivery that never happened, a
/// clipboard that stored nothing and a paste that pushed nothing each read `false` here.
///
/// WHAT EACH FIELD MEASURES:
///
///  * `delivered=` — how many `Event::Action`s came back OUT of the ring. Four go in; a variant the
///    ring cannot carry, or a push the classification refuses, lowers it.
///  * `copy=` / `paste=` — [`terminal_action`]'s own return for those two arms.
///  * `len=` — bytes the paste pushed, and `line_match=` — the rebuilt line against the source.
///  * `cut=` / `selectall=` — `unsupported`, ASSERTED AS A VALUE: a future selection arc that makes
///    them work must change this fixture, which is the point.
///  * `epoch_clear=` — the ownership gate, measured by AGEING the stamp (the buffer is set, its
///    stamp is walked back one epoch exactly as a log-out would leave it, and [`fresh_len`] must
///    then read 0 with `stale=yes`). It measures the same code on a build with no `login` feature,
///    which is what the gate lane runs.
///
/// Go-red: delete the `Action::Paste` arm's `paste_into_ring()` call in [`terminal_action`] — no
/// `Event::Key` reaches the ring, the line stays empty and the verdict reads
/// `paste=ok len=0 line_match=false -> FAIL`.
pub fn selftest() {
    use crate::pal::{self, Event};

    // The ring must start from a known state or `delivered=` would count somebody else's traffic.
    // At `ehci::init` time no HID is enumerated yet, so this is expected to discard NOTHING — it is
    // printed rather than assumed, because a nonzero reading would mean this fixture ate a real
    // event and the count is the only way anyone would ever know.
    let mut pre = 0usize;
    while pal::next_event().is_some() {
        pre += 1;
    }
    serial_println!("[clip] fixture pre-drain discarded={}", pre);

    const SRC: &str = "unaos clip";
    let mut line = [0u8; 64];
    let mut ll = 0usize;
    let mut delivered = 0usize;
    let mut copy = "no";
    let mut paste = "no";
    let mut cut = "no";
    let mut selectall = "no";
    let mut pasted = 0usize;

    // One action per round trip, dispatched exactly as the terminal's key consumer dispatches it.
    // The Paste round trip is the interesting one: `terminal_action` pushes the clipboard back onto
    // the ring as `Event::Key`, and those keys are drained BY THIS SAME LOOP and applied with the
    // line editor's rule — so the paste is proved end to end without a second code path.
    for act in [Action::Copy, Action::Paste, Action::Cut, Action::SelectAll] {
        // The line the editor holds when the chord arrives: the source text for the copy, and empty
        // for the paste, so `line_match=` compares a line the paste BUILT and not one it found.
        if act == Action::Paste {
            ll = 0;
        }
        pal::push_event(Event::Action(act));
        while let Some(ev) = pal::next_event() {
            match ev {
                Event::Action(a) => {
                    delivered += 1;
                    let line_now = if a == Action::Copy {
                        SRC
                    } else {
                        core::str::from_utf8(&line[..ll]).unwrap_or("")
                    };
                    let verdict = terminal_action(a, line_now);
                    match a {
                        Action::Copy => copy = verdict,
                        Action::Paste => paste = verdict,
                        Action::Cut => cut = verdict,
                        Action::SelectAll => selectall = verdict,
                        _ => {}
                    }
                }
                // `main::handle_key`'s printable-ASCII rule, and nothing else: this is the line
                // editor standing where the console stands on a live boot.
                Event::Key(b) if b >= 32 && b <= 126 => {
                    if ll < line.len() {
                        line[ll] = b;
                        ll += 1;
                    }
                    pasted += 1;
                }
                _ => {}
            }
        }
    }
    let line_match = &line[..ll] == SRC.as_bytes();

    // THE OWNERSHIP GATE, measured. Age the stamp by one epoch — the state a log-out leaves behind —
    // and the next read must destroy the buffer instead of serving it.
    set(b"previous session");
    {
        let mut c = CLIP.lock();
        c.epoch = c.epoch.wrapping_sub(1);
    }
    let epoch_clear = fresh_len() == 0;

    let pass = delivered == 4
        && copy == "ok"
        && paste == "ok"
        && line_match
        && cut == "unsupported"
        && selectall == "unsupported"
        && epoch_clear;
    serial_println!(
        ":: APPCLIP: delivered={} copy={} paste={} len={} line_match={} cut={} selectall={} epoch_clear={} -> {} ::",
        delivered,
        copy,
        paste,
        pasted,
        line_match,
        cut,
        selectall,
        if epoch_clear { "ok" } else { "no" },
        if pass { "PASS" } else { "FAIL" }
    );
}
