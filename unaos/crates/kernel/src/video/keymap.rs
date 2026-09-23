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

//! KEYMAP — the desktop's key bindings as a TABLE beside the theme, and the ONE place a chord is
//! judged.
//!
//! # Why this file exists
//!
//! R60: *"we will be implementing a windows-esque them at some point so key-bindings shouldn't be
//! hard coded."* Before this module every chord in the system was a LITERAL in a HID decoder —
//! `drivers/xhci/mod.rs`'s `hid_screenshot_chord_edge` tested `HID_MOD_GUI` and `HID_MOD_SHIFT`
//! and usage `0x20`/`0x21` inline, the EHCI decoder called the same function, and Print Screen was
//! a second literal (`0x46`) beside it. A second, Windows-shaped theme could not be added without
//! editing the USB drivers, which is exactly the coupling the ruling forbids.
//!
//! R61: *"i prefer command-c and friends (alt-c on pc) then there's no special case for the
//! command line to resolve that usability question."* The edit chords are the COMMAND key on every
//! window INCLUDING the terminal, so `Ctrl-C` never has to be argued about: it keeps its byte-0x03
//! meaning in the shell and no terminal special case exists anywhere in this tree. That is a
//! PROPERTY OF THE TABLE, not of a guard — `⌘C` is a row here and `Ctrl-C` is not, so the resolver
//! declines `Ctrl-C` and `hid_key_ascii` hands the shell `0x03` exactly as it did before KEYMAP.
//! [`selftest`] measures both halves of that sentence on every boot that reaches it.
//!
//! # The shape
//!
//! A [`Binding`] is a ROLE MASK plus a HID usage plus the [`Action`] it means. A [`Table`] is a
//! named list of them plus ONE more thing: `cmd_role`, the PHYSICAL HID modifier bits that play
//! the abstract *Command* role on that table. That is the whole seam. `⌘C` is written once, as
//! `CMD | usage 0x06`, and the TABLE decides whether the operator's hand is on the GUI key (Apple,
//! [`super::theme::CRISPY_BINDINGS`]) or the Alt key (PC, [`super::theme::PC_BINDINGS`]). No row
//! anywhere names a physical modifier.
//!
//! [`resolve`] is the only place a chord is judged. Both HID decoders ask it; neither tests a
//! modifier bit or a usage of its own any more.
//!
//! # Rules the table keeps from the code it replaces
//!
//!  * **Extra modifiers held do not disqualify.** On macOS `⌃⌘⇧3` is still a screenshot, and the
//!    predicate this module replaced said so in its own doc. A row requires the roles it names to
//!    be DOWN; it says nothing about the ones it does not name.
//!  * **Left and right of a modifier count alike.** `HID_MOD_*` are two-bit masks (left half in
//!    bits 0..3, right half in bits 4..7) and the test is `!= 0`, never `== mask`.
//!  * **The edge, not the level.** A boot report carries the set of keys HELD, so a chord held for
//!    half a second must arm once. [`resolve_edge`] diffs against the previous report exactly as
//!    the lock keys and `0x46` do.
//!  * **A chord types nothing.** `hid_key_ascii` returns 0 for any usage while a GUI **or** an Alt
//!    bit is held, so both `cmd_role` spellings suppress the character on their own — the table
//!    needs no suppression rule and did not gain one.
//!
//! # What is NOT here
//!
//! **No consumer of `Copy`/`Cut`/`Paste`/`SelectAll` exists.** There is no clipboard in this tree
//! (measured: `grep -r -i clipboard video/` finds prose only), so those rows RESOLVE and go no
//! further. The delivery seam is named in `docs/dev/OS/08_VIDEO/keymap.md` §Delivery and is one
//! `pal::Event` variant away; `pal.rs` was not in KEYMAP's brief, so the arc stops at the resolver
//! and says so rather than inventing a second event path beside `pal::push_event`.
//!
//! **`LogOut` is a SLOT.** `⌘⇧Q` resolves to [`Action::LogOut`] and nothing in this tree acts on
//! it. LOGINFLOW may bind it; nothing else may.

use crate::drivers::xhci::{HID_MOD_ALT, HID_MOD_CTRL, HID_MOD_GUI, HID_MOD_SHIFT};

/// What a chord MEANS. Named for the desktop's intent, never for a key — that is the entire point
/// of the indirection: a Windows-shaped table binds a different chord to the same variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Whole-screen capture. `video::prtscr::request()`; the destination is the THEME's (R60).
    Screenshot,
    /// Region capture. Honoured as a whole-screen capture until a pointer selector exists — the
    /// same reservation `hid_screenshot_chord_edge` carried for `⌘⇧4`, moved to the table.
    ScreenshotRegion,
    /// R61's first row. No consumer: there is no clipboard.
    Copy,
    /// R61. No consumer.
    Cut,
    /// R61. No consumer.
    Paste,
    /// R61. No consumer.
    SelectAll,
    /// The SLOT. LOGINFLOW may bind it; nothing else may, and nothing acts on it today.
    LogOut,
}

impl Action {
    /// The witness token. Stable: it is what `action=` carries on the wire and what a spec pins.
    pub const fn name(self) -> &'static str {
        match self {
            Action::Screenshot => "screenshot",
            Action::ScreenshotRegion => "screenshot-region",
            Action::Copy => "copy",
            Action::Cut => "cut",
            Action::Paste => "paste",
            Action::SelectAll => "select-all",
            Action::LogOut => "log-out",
        }
    }

    /// Does this action arm the screen capture? The ONE question the HID decoders ask of an
    /// [`Action`] — everything else they resolve is delivered, not acted on.
    pub const fn is_capture(self) -> bool {
        matches!(self, Action::Screenshot | Action::ScreenshotRegion)
    }
}

/// ROLE BITS — the abstract modifiers a [`Binding`] is written in. These are NOT HID bits: the
/// table maps them to HID bits, which is what makes one row serve `⌘C` and `Alt+C`.
pub const CMD: u8 = 0x01;
/// The Shift role. Physically `HID_MOD_SHIFT` on every table — Shift is Shift on both keyboards.
pub const SHIFT: u8 = 0x02;
/// The Control role. Physically `HID_MOD_CTRL` on every table. NO ROW IN THIS TREE USES IT, and
/// that absence is R61 (`Ctrl-C` belongs to the command line).
pub const CTRL: u8 = 0x04;
/// The Option/Alt role. Physically `HID_MOD_ALT`. Unused by both shipped tables; on the PC table
/// Alt is already spoken for as `cmd_role`, so a row naming both would be a contradiction.
pub const ALT: u8 = 0x08;

/// One binding. `roles` is a mask of [`CMD`]/[`SHIFT`]/[`CTRL`]/[`ALT`]; `usage` is a HID
/// Keyboard/Keypad page usage id (HUT 1.12 §10).
pub struct Binding {
    /// Which abstract modifiers must be DOWN. Extra modifiers do not disqualify.
    pub roles: u8,
    /// The HID usage whose PRESS EDGE this row watches.
    pub usage: u8,
    /// What it means.
    pub action: Action,
    /// The witness token for this row — what `chord=` carries on the wire. Written here rather
    /// than derived, so the two chords that already have metal-proven witnesses
    /// (`cmd-shift-3`, `cmd-shift-4`, flight 11) keep the exact bytes a capture can be grepped for.
    pub token: &'static str,
}

/// A named binding table. ONE per theme.
pub struct Table {
    /// The theme's own name. `table=` on the wire.
    pub name: &'static str,
    /// THE SEAM: the PHYSICAL HID modifier mask that plays the [`CMD`] role here. `HID_MOD_GUI` on
    /// an Apple-shaped table, `HID_MOD_ALT` on a PC-shaped one.
    pub cmd_role: u8,
    /// The rows, in PRECEDENCE ORDER — the first row whose usage edged and whose roles are held
    /// wins, so a more specific chord is written above a less specific one.
    pub rows: &'static [Binding],
}

impl Table {
    /// Are the abstract `roles` held, given this report's HID modifier byte? Per-role `!= 0`, so
    /// left and right of each modifier count alike; extra modifiers are ignored by construction.
    pub fn held(&self, roles: u8, modifiers: u8) -> bool {
        if roles & CMD != 0 && modifiers & self.cmd_role == 0 {
            return false;
        }
        if roles & SHIFT != 0 && modifiers & HID_MOD_SHIFT == 0 {
            return false;
        }
        if roles & CTRL != 0 && modifiers & HID_MOD_CTRL == 0 {
            return false;
        }
        if roles & ALT != 0 && modifiers & HID_MOD_ALT == 0 {
            return false;
        }
        true
    }
}

/// PRECEDENCE, CHECKED AT COMPILE TIME. Row `i` SHADOWS a later row `j` when they watch the same
/// usage and everything `i` requires is also required by `j` — whenever `j` would match, `i`
/// already did, so `j` is unreachable. The hazard is real and cheap to write by accident: a bare
/// `PrtSc -> Screenshot` row (`roles: 0`) placed above `Shift+PrtSc -> ScreenshotRegion` disarms
/// the region chord silently, with every gate green, because nothing on the wire says a row was
/// never consulted. Each shipped table asserts this at the foot of `theme.rs`.
///
/// Returns `true` when no row shadows a later one.
pub const fn no_shadow(rows: &[Binding]) -> bool {
    let mut i = 0;
    while i < rows.len() {
        let mut j = i + 1;
        while j < rows.len() {
            if rows[i].usage == rows[j].usage && rows[i].roles & rows[j].roles == rows[i].roles {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

/// **THE RESOLVER.** One modifier byte plus one usage that just went DOWN becomes an [`Action`],
/// or nothing. This is the only place in the tree that judges a chord; the HID decoders, which
/// used to test `HID_MOD_GUI`/`HID_MOD_SHIFT`/`0x20`/`0x21` inline, now ask this and nothing else.
///
/// `Ctrl-C` returns `None` on every shipped table and that is R61 on purpose, not by omission.
pub fn resolve(table: &Table, modifiers: u8, usage_edge: u8) -> Option<Action> {
    resolve_chord(table, modifiers, usage_edge).map(|(action, _)| action)
}

/// [`resolve`]'s witness face — the same single walk, returning the row's `token` as well so a
/// caller can name which chord fired. `resolve` is `.map`ped off this; there is ONE walk.
pub fn resolve_chord(table: &Table, modifiers: u8, usage_edge: u8) -> Option<(Action, &'static str)> {
    let mut i = 0;
    while i < table.rows.len() {
        let b = &table.rows[i];
        if b.usage == usage_edge && table.held(b.roles, modifiers) {
            return Some((b.action, b.token));
        }
        i += 1;
    }
    None
}

/// The EDGE face, for a HID boot report pair. Walks the usages that are present NOW and absent
/// from the PREVIOUS report — one press each — and asks [`resolve_chord`] about each in turn.
///
/// A key held across reports edges once, which is the rule the predicate this replaced carried
/// (without the diff a chord held for half a second would fill a volume with screenshots).
pub fn resolve_edge(
    table: &Table,
    cur_keys: &[u8; 6],
    prev_keys: &[u8; 6],
    modifiers: u8,
) -> Option<(Action, &'static str)> {
    let mut i = 0;
    while i < cur_keys.len() {
        let u = cur_keys[i];
        if u != 0 && !prev_keys.contains(&u) {
            if let Some(hit) = resolve_chord(table, modifiers, u) {
                return Some(hit);
            }
        }
        i += 1;
    }
    None
}

/// The table the desktop is running. ONE call site for the selection, so a knob that picks
/// [`super::theme::PC_BINDINGS`] is a change to this function and to nothing else. Such a knob is
/// NOT this arc — the PC table exists to prove the seam and is selected by nothing.
pub fn active() -> &'static Table {
    super::theme::CRISPY_BINDINGS
}

// --- fixture ---------------------------------------------------------------------------------

/// KEYMAP — the resolver's own gate, driven by SYNTHETIC boot reports the way
/// `drivers/ehci`'s `[tp] dispatch self-test` drives the trackpad dispatcher, and for the same
/// reason: the decision is the only part of this path QEMU can exercise at all, since QEMU has no
/// operator's hands. Chained from `drivers::ehci::parser_selftest`, which the x86 `wc` lane runs
/// (`:: EHCI-HID: report-parser self-test:` is on every capture of that lane).
///
/// WHAT EACH FIELD MEASURES, because a legible field that compares nothing is the trap:
///
///  * `resolved=` — how many of the legs below resolved to the action they must. It is a COUNT of
///    agreements, not of rows, so removing a row LOWERS it.
///  * `screenshot=` / `region=` — `⌘⇧3` and `⌘⇧4` under a REPORT PAIR, so the edge is in the test
///    and not just the mask; these are the two chords flight 11 proved on metal.
///  * `copy=` / `paste=` / `cut=` / `selectall=` — R61's four, each through a report pair.
///  * `ctrl_c_ascii=` — `hid_key_ascii(0x06, HID_MOD_CTRL, false)`, the SHELL's byte, MEASURED and
///    not asserted. Paired with `ctrl_c_action=`, which must be `none`: together they are R61's
///    whole claim — the command line keeps `Ctrl-C` because the table never claimed it.
///  * `pc_table_alt_c=` — the SEAM. The same `CMD | 0x06` row read through a table whose
///    `cmd_role` is `HID_MOD_ALT`, resolved from an ALT-held modifier byte. If the role
///    indirection were fake this field would read `none` while every field above stayed `ok`.
///  * `extramods=` — `⌃⌘⇧3` still resolves to `Screenshot` (the macOS rule the predicate carried).
///  * `logout=` — the `⌘⇧Q` SLOT resolves; nothing consumes it.
///
/// Go-red: delete the `Cmd+C` row from `theme::CRISPY_ROWS` — `copy=no`, `resolved=` drops by one
/// and the verdict flips to `FAIL`.
pub fn selftest() {
    let crispy = super::theme::CRISPY_BINDINGS;
    let pc = super::theme::PC_BINDINGS;
    let none: [u8; 6] = [0; 6];
    let mut ok = 0usize;
    // One press each, as a REPORT PAIR: the usage present NOW and absent from the previous report,
    // so the EDGE is under test and not merely the mask. Compared on the ACTION — the token is the
    // row's own and is asserted separately by the witness the decoders print.
    let leg = |usage: u8, modifiers: u8, want: Action| -> bool {
        let cur: [u8; 6] = [usage, 0, 0, 0, 0, 0];
        matches!(resolve_edge(crispy, &cur, &none, modifiers), Some((a, _)) if a == want)
    };
    let cmd = HID_MOD_GUI;
    let shot = leg(0x20, cmd | HID_MOD_SHIFT, Action::Screenshot);
    let region = leg(0x21, cmd | HID_MOD_SHIFT, Action::ScreenshotRegion);
    let copy = leg(0x06, cmd, Action::Copy);
    let paste = leg(0x19, cmd, Action::Paste);
    let cut = leg(0x1B, cmd, Action::Cut);
    let selectall = leg(0x04, cmd, Action::SelectAll);
    let logout = leg(0x14, cmd | HID_MOD_SHIFT, Action::LogOut);
    // R61, both halves. The shell's byte is MEASURED through the same decoder the shell reads.
    let ctrl_c_ascii = crate::drivers::xhci::hid_key_ascii(0x06, HID_MOD_CTRL, false);
    let ctrl_c_action = resolve(crispy, HID_MOD_CTRL, 0x06);
    let ctrl_c_ok = ctrl_c_ascii == 0x03 && ctrl_c_action.is_none();
    // Print Screen resolves through the table like everything else.
    let prtsc = matches!(resolve(crispy, 0, 0x46), Some(Action::Screenshot));
    // The macOS rule the predicate carried: extra modifiers do not disqualify.
    let extramods = leg(0x20, cmd | HID_MOD_SHIFT | HID_MOD_CTRL, Action::Screenshot);
    // THE SEAM: one row, the other table, the other physical modifier.
    let pc_alt_c = resolve(pc, HID_MOD_ALT, 0x06);
    let pc_ok = matches!(pc_alt_c, Some(Action::Copy));
    for hit in [
        shot, region, copy, paste, cut, selectall, logout, ctrl_c_ok, prtsc, extramods, pc_ok,
    ] {
        if hit {
            ok += 1;
        }
    }
    let pass = ok == 11;
    serial_println!(
        ":: KEYMAP: table={} resolved={} screenshot={} region={} copy={} paste={} cut={} selectall={} ctrl_c_ascii={:#04x} ctrl_c_action={} pc_table_alt_c={} prtsc={} extramods={} logout={} rows={}/{} -> {} ::",
        crispy.name,
        ok,
        yn(shot),
        yn(region),
        yn(copy),
        yn(paste),
        yn(cut),
        yn(selectall),
        ctrl_c_ascii,
        ctrl_c_action.map(Action::name).unwrap_or("none"),
        pc_alt_c.map(Action::name).unwrap_or("none"),
        yn(prtsc),
        yn(extramods),
        yn(logout),
        crispy.rows.len(),
        pc.rows.len(),
        if pass { "PASS" } else { "FAIL" }
    );
}

/// `ok`/`no` — the fixture's field alphabet. Two values, so a field that stops being computed
/// cannot read as a third thing.
const fn yn(v: bool) -> &'static str {
    if v {
        "ok"
    } else {
        "no"
    }
}
