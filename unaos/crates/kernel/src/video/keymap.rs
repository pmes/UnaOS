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
//!    needs no suppression rule and did not gain one. **A row with no `CMD` role is the exception
//!    and says so:** `Shift+←`/`Shift+→` and `Esc` (TERMSEL) still type their byte (`0x1D`/`0x1C`,
//!    `0x1B`), pushed by the decoder just AHEAD of the action, because neither Shift nor no modifier
//!    suppresses a key. That is deliberate: a menu that closes on `0x1B` still sees it, and the
//!    shell's line editor ignores both bytes. TERMSEL2's bare `←`/`→` rows (the caret) are the same
//!    exception: the arrow bytes `0x1D`/`0x1C` are still pushed ahead of `CursorLeft`/`CursorRight`,
//!    so Quarry and `user-vug`, which read the bytes, see exactly what they saw before; `Home`/`End`
//!    type nothing (their ascii is 0) and `⌘←`/`⌘→` are suppressed by the `CMD` rule.
//!
//! # What is NOT here
//!
//! **The consumer of `Copy`/`Cut`/`Paste`/`SelectAll` is not here, and neither is the clipboard.**
//! Both exist since APPCLIP (rmbp-ledger B176): the decoders push a resolved non-capture action
//! onto the input ring as `pal::Event::Action`, `video/clipboard.rs` holds the one session-owned
//! text buffer, and the terminal is its first consumer (`clipboard::terminal_action`) — `Paste`
//! types the clipboard into the shell line; since TERMSEL (`video/termsel.rs`) `SelectAll` and the
//! selection actions (`SelectLeft` .. `Deselect`) move a selection over the editable line, `Copy` copies that
//! selection (or the whole line when none is live), and `Cut` removes it from the line (or is
//! declined on the wire, `[clip] cut refused reason=no-selection`, when none is live). This file
//! still only RESOLVES; what an action does is `clipboard.md`'s to say, not this file's.
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
    /// SHOTREGION M2: window capture (`⌘⇧5`). Acted on by the x86 router (`shotsel`); not a decoder capture.
    ScreenshotWindow,
    /// R61's first row. Consumed by the terminal (`clipboard::terminal_action`): the selection,
    /// or the whole line when none is live.
    Copy,
    /// R61. Consumed by the terminal: cuts the SELECTION from the editable line (TERMSEL); with
    /// no selection it is declined on the wire.
    Cut,
    /// R61. Consumed by the terminal: types the clipboard through the ring.
    Paste,
    /// R61. Consumed by the terminal: selects the whole editable line (TERMSEL).
    SelectAll,
    /// The SLOT. LOGINFLOW may bind it; nothing else may, and nothing acts on it today.
    LogOut,
    /// TERMSEL: move the selection's HEAD one cell left (`Shift+←` on both tables).
    SelectLeft,
    /// TERMSEL: move the selection's HEAD one cell right (`Shift+→` on both tables).
    SelectRight,
    /// TERMSEL: move the selection's HEAD to the line start (`⌘⇧←` and `Shift+Home` on CRISPY,
    /// `Shift+Home` on PC).
    SelectLineStart,
    /// TERMSEL: move the selection's HEAD to the line end (`⌘⇧→` and `Shift+End` on CRISPY,
    /// `Shift+End` on PC).
    SelectLineEnd,
    /// TERMSEL: drop the selection (`Esc`, a row with no roles — the `0x1B` key is still typed).
    Deselect,
    /// TERMSEL2: the CARET one cell left (`←` on both tables; the `0x1D` byte is still typed). With
    /// a selection on the editable line it collapses to the selection's start, as on a Mac.
    CursorLeft,
    /// TERMSEL2: the caret one cell right (`→`; the `0x1C` byte is still typed); with a selection on
    /// the editable line, to its end.
    CursorRight,
    /// TERMSEL2: the caret to the line start (`⌘←` and `Home` on CRISPY, `Home` on PC).
    CursorLineStart,
    /// TERMSEL2: the caret to the line end (`⌘→` and `End` on CRISPY, `End` on PC).
    CursorLineEnd,
    /// BRIGHTKEYS: panel backlight one step down (`F1`, the Apple keyboard's brightness-down key).
    /// Consumed at `pal::push_event` by `video::brightkeys`; never reaches a ring consumer.
    BrightnessDown,
    /// BRIGHTKEYS: panel backlight one step up (`F2`).
    BrightnessUp,
    /// WINCYCLE: focus the least-recently-raised app window (`Alt+Tab`; `⌘Tab` on CRISPY). Acted on by
    /// the window system's router, never by the terminal.
    CycleWindow,
    /// SCREENLOCK: lock the session without ending it (`⌘L`, `Ctrl+Alt+L`). Acted on by the router (`login::lock`).
    LockScreen,
    /// SHORTCUTS: open the keyboard-shortcut help overlay (`⌘/`). Acted on by the router.
    ShowShortcuts,
    /// SCROLLBACK: the shell view one page toward older output (`Shift+PgUp`). Consumed by `Console::act`.
    ScrollPageUp,
    /// SCROLLBACK: one page toward the live bottom (`Shift+PgDn`).
    ScrollPageDown,
    /// SCROLLBACK: the oldest page (`Cmd+Home` / `Ctrl+Home`).
    ScrollTop,
    /// SCROLLBACK: back to the live bottom (`Cmd+End` / `Ctrl+End`).
    ScrollBottom,
    /// WINSNAP: snap the focused window to the left half (`⌘⌥←`, `Ctrl+Alt+←`). Acted on by the router.
    SnapLeft,
    /// WINSNAP: right half (`⌘⌥→`, `Ctrl+Alt+→`).
    SnapRight,
    /// WINSNAP: zoom (`⌘⌥↑`, `Ctrl+Alt+↑`).
    SnapZoom,
    /// WINSNAP: restore the pre-snap placement (`⌘⌥↓`, `Ctrl+Alt+↓`).
    SnapRestore,
    /// WINRESIZE M3: Ctrl+arrow nudges the focused window 16 px (L/R/U/D), Ctrl+Shift+arrow resizes it.
    WinNudgeLeft,
    WinNudgeRight,
    WinNudgeUp,
    WinNudgeDown,
    WinSizeLeft,
    WinSizeRight,
    WinSizeUp,
    WinSizeDown, Minimize /* WINDOWLIST: ⌘M, code 39 */, CycleApp /* WINDOWLIST: ⌘` — next window of the SAME app, code 40 */, QuitApp /* APPMENU2: ⌘Q, code 42 */, CloseWindow /* APPMENU2: ⌘W, code 43 */, HideApp /* APPMENU2: ⌘H, code 44 */, OpenSettings /* APPMENU2: ⌘, code 45 */, ForceQuit /* APPMENU2: ⌘⌥Esc, code 46 */, ClearView /* LUMENBIN: ⌘K — clear the focused view (LUMEN.ELF's transcript), code 41 */, GetInfo /* ATTRCOLUMNS (B402): ⌘I — Quarry's Get Info, code 47 */, Launcher /* LAUNCHER (B417): ⌘Space, code 48 (the arc wrote 47; renumbered at merge17) */, Interrupt /* SHELLTASK2 (B474): ⌘. — abort the shell window's running command (Ctrl-C's twin), code 49 */, Start /* WINKEY (B665): the Windows key ALONE — RING-3 DELIVERY ONLY, code 50; no row binds it, no decoder emits it yet, and it is NOT Start's trigger (the tap counter below is: `tap_track` → `startmenu::key_service`) */,
}

impl Action {
    /// The witness token. Stable: it is what `action=` carries on the wire and what a spec pins.
    pub const fn name(self) -> &'static str {
        match self {
            Action::Screenshot => "screenshot",
            Action::ScreenshotRegion => "screenshot-region",
            Action::ScreenshotWindow => "screenshot-window",
            Action::Copy => "copy",
            Action::Cut => "cut",
            Action::Paste => "paste",
            Action::SelectAll => "select-all",
            Action::LogOut => "log-out",
            Action::SelectLeft => "select-left",
            Action::SelectRight => "select-right",
            Action::SelectLineStart => "select-line-start",
            Action::SelectLineEnd => "select-line-end",
            Action::Deselect => "deselect",
            Action::CursorLeft => "cursor-left",
            Action::CursorRight => "cursor-right",
            Action::CursorLineStart => "cursor-line-start",
            Action::CursorLineEnd => "cursor-line-end",
            Action::BrightnessDown => "brightness-down",
            Action::BrightnessUp => "brightness-up",
            Action::CycleWindow => "cycle-window",
            Action::LockScreen => "lock-screen",
            Action::ShowShortcuts => "show-shortcuts",
            Action::ScrollPageUp => "scroll-page-up",
            Action::ScrollPageDown => "scroll-page-down",
            Action::ScrollTop => "scroll-top",
            Action::ScrollBottom => "scroll-bottom",
            Action::SnapLeft => "snap-left",
            Action::SnapRight => "snap-right",
            Action::SnapZoom => "snap-zoom",
            Action::SnapRestore => "snap-restore",
            Action::WinNudgeLeft => "win-nudge-left",
            Action::WinNudgeRight => "win-nudge-right",
            Action::WinNudgeUp => "win-nudge-up",
            Action::WinNudgeDown => "win-nudge-down",
            Action::WinSizeLeft => "win-size-left",
            Action::WinSizeRight => "win-size-right",
            Action::WinSizeUp => "win-size-up",
            Action::WinSizeDown => "win-size-down", Action::Minimize => "minimize", Action::CycleApp => "cycle-app", Action::ClearView => "clear-view", Action::QuitApp => "quit-app", Action::CloseWindow => "close-window", Action::HideApp => "hide-app", Action::OpenSettings => "open-settings", Action::ForceQuit => "force-quit", Action::GetInfo => "get-info", Action::Launcher => "launcher", Action::Interrupt => "interrupt", Action::Start => "start", // WINKEY (B665) · WINDOWLIST · LUMENBIN · APPMENU2 · ATTRCOLUMNS
        }
    }

    /// BRIGHTKEYS: is this a backlight step (handled by `video::brightkeys`, not delivered)?
    pub const fn is_brightness(self) -> bool {
        matches!(self, Action::BrightnessDown | Action::BrightnessUp)
    }

    /// Does this action arm the screen capture? The ONE question the HID decoders ask of an
    /// [`Action`] — everything else they resolve is delivered, not acted on.
    pub const fn is_capture(self) -> bool {
        // SHOTREGION: on x86 `wc` the region chord is the ROUTER's (selection mode, `shotsel`), so the decoder no longer arms a whole-panel capture for it; elsewhere it still does.
        matches!(self, Action::Screenshot) || (cfg!(not(all(target_arch = "x86_64", feature = "wc"))) && matches!(self, Action::ScreenshotRegion))
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
        if roles & ALT != 0 && modifiers & self.option_role() == 0 { // SOFTY2 (B641): the Option role's physical key — HID_MOD_ALT, or HID_MOD_GUI on a MIRRORED table (Boot Camp's swap)
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
    win_l(modifiers, usage_edge) // LOCK2 (B728): Win+L, asked after every row declined, here in the ONE walk so `resolve` and `resolve_edge` (and every caller of either) answer it alike (file tail). LINE-NEUTRAL fold, code first.
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
    note_keys(cur_keys, prev_keys); #[cfg(all(target_arch = "x86_64", feature = "wc"))] zoom_edge(cur_keys, prev_keys, modifiers); let mut i = 0; // ZOOM (B713): Win+= / Win+- / Win+0 through `ZOOM_ROWS` (file tail). SOFTY2 (B641): a key edge spoils a Windows-key tap; an Esc edge is counted for the flyouts. SAME-LINE fold, code first.
    while i < cur_keys.len() {
        let u = cur_keys[i];
        if u != 0 && !prev_keys.contains(&u) {
            if let Some(hit) = resolve_chord(table, modifiers, u) {
                return Some(hit);
            }
        }
        i += 1;
    }
    None // AUTOLOCK (B712) -> LOCK2 (B728): Win+L is answered inside `resolve_chord` (the one walk) for every edge above, so this fallback is gone and the chord is listed once (file tail). LINE-NEUTRAL fold.
}

/// The table the desktop is running. ONE call site for the selection, so a knob that picks
/// [`super::theme::PC_BINDINGS`] is a change to this function and to nothing else. Such a knob is
/// NOT this arc — the PC table exists to prove the seam and is selected by nothing.
pub fn active() -> &'static Table {
    softy2_table(PC_ROLE.load(core::sync::atomic::Ordering::Relaxed), softy_look()) // SOFTY2 (B641, R126): the look AND the keyboard pick the table — Una+Mac CRISPY, Una+PC UNA_PC (mirrored), Softy+Mac SOFTY_MAC (Boot Camp: Option = Win, Command = Alt), Softy+PC PC. STATUSTRAY (B426): the input item's one toggle says the keyboard is a PC's (R61's Command-role swap)
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

// WINRESIZE (R75) — TAIL-APPENDED.
impl Action {
    /// WINRESIZE M3: `(dx, dy, resize)` for the eight window-key actions (`-1/0/1`; `resize` = Ctrl+Shift),
    /// `None` for every other action.
    pub const fn win_key(self) -> Option<(i32, i32, bool)> {
        match self {
            Action::WinNudgeLeft => Some((-1, 0, false)),
            Action::WinNudgeRight => Some((1, 0, false)),
            Action::WinNudgeUp => Some((0, -1, false)),
            Action::WinNudgeDown => Some((0, 1, false)),
            Action::WinSizeLeft => Some((-1, 0, true)),
            Action::WinSizeRight => Some((1, 0, true)),
            Action::WinSizeUp => Some((0, -1, true)),
            Action::WinSizeDown => Some((0, 1, true)),
            _ => None,
        }
    }
}

/// WINRESIZE M2 — the modifier byte of the latest HID key report (written by the xhci chord resolver).
static HID_MODS_LAST: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// WINRESIZE — record the latest HID modifier byte.
pub fn note_mods(m: u8) {
    tap_track(m); HID_MODS_LAST.store(m, core::sync::atomic::Ordering::Relaxed); // SOFTY2 (B641): the Windows-key tap detector reads the edge BEFORE the byte is overwritten. SAME-LINE fold, code first.
}
/// WINRESIZE M2 — is Shift down per the latest HID report (either side)?
pub fn shift_held() -> bool {
    HID_MODS_LAST.load(core::sync::atomic::Ordering::Relaxed) & HID_MOD_SHIFT != 0
}

// ── STATUSTRAY (B426) — the input item's toggle: the R61 Command-role swap ────────────────────────────
//
// `active()` is the ONE selection site; this flag is what it reads. Off = CRISPY (Command = the GUI key on
// the Apple keyboard); on = PC (Command's role is played by Alt). Boot-static, not persisted: a preference
// is Principia's (R79), and the Settings row that stores it is owed to SETTINGS, not invented here.
static PC_ROLE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Select the PC table (`true`) or the Mac table (`false`). Returns the table now active.
pub fn set_pc(on: bool) -> &'static Table {
    PC_ROLE.store(on, core::sync::atomic::Ordering::Relaxed);
    let t = active();
    serial_println!("[keymap] table={} cmd_role={} via=statustray", t.name, if t.cmd_role == HID_MOD_GUI { "gui" } else if t.cmd_role == HID_MOD_ALT { "alt" } else { "other" });
    t
}

/// True while the PC table is selected.
pub fn pc_selected() -> bool {
    PC_ROLE.load(core::sync::atomic::Ordering::Relaxed)
}

/// DRAGDROP (B440) — is Option down per the latest HID report (either side)? Option is physically `HID_MOD_ALT`
/// on both shipped tables; a drag reads it at the drop (Option forces a copy).
pub fn option_held() -> bool {
    HID_MODS_LAST.load(core::sync::atomic::Ordering::Relaxed) & active().option_role() != 0 // SOFTY2 (B641): the Option ROLE's physical key on the live table (the PC Win key under Una on a PC keyboard)
}

/// DRAGDROP2 (B470) — is the Command ROLE down per the latest HID report (the active table's `cmd_role`: the GUI
/// key on the Mac table, Alt on the PC one)? Quarry's Cmd-press toggles a row into the selection.
pub fn command_held() -> bool {
    HID_MODS_LAST.load(core::sync::atomic::Ordering::Relaxed) & active().cmd_role != 0
}

// ══════════════ SOFTY2 (rmbp-ledger B641, R126) — THE BOOT CAMP KEY TABLE — TAIL-APPENDED ══════════════
//
// Peter, 2026-10-14 (R126): *"mac has that way to run windows whatever they did we do? i mean, to me its obvious mac
// option is windows win key and mac command is windows alt."* — and the same table the other way for people on a PC
// keyboard under the Una look.
//
// The two keyboards put the same three keys in the same three places: a Mac's `Ctrl Option Command` and a PC's
// `Ctrl Win Alt`. Boot Camp maps them BY POSITION — Option plays the Windows key, Command plays Alt — and that is the
// whole of this block: a MIRRORED table reads the two physical modifiers swapped. The look (Una | Softy) and the
// keyboard (the STATUSTRAY input item's PC flag) pick one of four tables, and no row is re-typed:
//
// | look  | keyboard | table       | Command role (`cmd_role`) | Option role (`option_role`) | the Windows key (Start) |
// |-------|----------|-------------|---------------------------|-----------------------------|-------------------------|
// | Una   | Mac      | `crispy`    | Command (GUI)             | Option (ALT)                | —                       |
// | Una   | PC       | `una-pc`    | Alt (ALT)                 | Win (GUI)                   | —                       |
// | Softy | Mac      | `softy-mac` | Command (GUI) = Win's Alt | Command (GUI): Ctrl+Command | Option (ALT), a tap     |
// | Softy | PC       | `pc`        | Alt (ALT)                 | Alt (ALT): Ctrl+Alt         | Win (GUI), a tap        |
//
// `softy-mac` is the PC rows (`theme::PC_ROWS`) read mirrored: every `CMD` (Windows' Alt chords — Alt+Tab, Alt+C)
// is the Mac's Command, every `ALT` (Ctrl+Alt+arrows, Ctrl+Alt+L) is Ctrl+Command, and Option is free to be the
// Windows key. `una-pc` is the Mac rows (`theme::CRISPY_ROWS`) read mirrored: ⌘C is PC Alt+C, ⌘⌥← is Alt+Win+←.
//
// THE WINDOWS KEY is a bare TAP of the key that plays it (down and up with no other modifier and no key between):
// [`tap_track`] (from `note_mods`, every HID report) and [`note_keys`] (from `resolve_edge`) count it, and the look's
// service pass (`softy::startmenu::key_service`) toggles Start — never the decoder, which runs where a window may
// not open. A key edge while it is held spoils the tap (Win+E is a chord, not Start). WINKEY (B665, seat's path (b)):
// `Action::Start` (ring-3 code 50) now exists for RING-3 DELIVERY ONLY — the decoders do not emit it (a tap completes on a
// modifier-only report, where no key edged and the non-capture arm never runs); THIS COUNTER stays Start's one trigger.

/// Softy is the look (`video::look`, a desktop image's module; Una everywhere else).
#[inline]
fn softy_look() -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        super::look::is_softy()
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        false
    }
}

/// SOFTY on an Apple keyboard — Boot Camp's: the PC rows, mirrored (Command = Alt; Option = the Windows key).
pub static SOFTY_MAC_BINDINGS: &Table = &Table {
    name: "softy-mac",
    cmd_role: HID_MOD_GUI,
    rows: super::theme::PC_ROWS,
};

/// UNA on a PC keyboard — the same swap the other way: the Mac rows, mirrored (Alt = Command; Win = Option).
pub static UNA_PC_BINDINGS: &Table = &Table {
    name: "una-pc",
    cmd_role: HID_MOD_ALT,
    rows: super::theme::CRISPY_ROWS,
};

/// The four tables by (PC keyboard, Softy look) — `active()`'s one body.
#[inline]
pub fn softy2_table(pc: bool, softy: bool) -> &'static Table {
    match (softy, pc) {
        (false, false) => super::theme::CRISPY_BINDINGS,
        (false, true) => UNA_PC_BINDINGS,
        (true, false) => SOFTY_MAC_BINDINGS,
        (true, true) => super::theme::PC_BINDINGS,
    }
}

impl Table {
    /// A MIRRORED table reads the GUI and ALT keys swapped (Boot Camp's position mapping).
    #[inline]
    pub fn mirrored(&self) -> bool {
        core::ptr::eq(self, SOFTY_MAC_BINDINGS) || core::ptr::eq(self, UNA_PC_BINDINGS)
    }
    /// The physical HID modifier that plays the [`ALT`] (Option) role on this table.
    #[inline]
    pub fn option_role(&self) -> u8 {
        if self.mirrored() { HID_MOD_GUI } else { HID_MOD_ALT }
    }
}

/// The physical HID modifier that plays the WINDOWS key on the live look and keyboard: Option (`HID_MOD_ALT`) on a
/// Mac keyboard under Softy, the Win key (`HID_MOD_GUI`) on a PC keyboard under Softy, none (0) under Una.
pub fn start_role() -> u8 {
    if !softy_look() {
        0
    } else if PC_ROLE.load(core::sync::atomic::Ordering::Relaxed) {
        HID_MOD_GUI
    } else {
        HID_MOD_ALT
    }
}

/// The Windows key's name on this keyboard (the wire's `role=`).
pub fn start_role_name() -> &'static str {
    match start_role() {
        0 => "none",
        r if r == HID_MOD_GUI => "win",
        _ => "option",
    }
}

/// The tap detector's state: 0 idle, 1 armed (the key went down alone), 2 spoilt (something joined it).
static TAP: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Taps counted and not yet taken by the look's service pass.
static START_TAPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Esc press edges not yet taken (the flyouts close on one).
static ESC_EDGES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// From `note_mods` with the NEW modifier byte, before it replaces the last one: the Windows key's edges.
fn tap_track(m: u8) {
    use core::sync::atomic::Ordering::Relaxed;
    let role = start_role();
    if role == 0 {
        TAP.store(0, Relaxed);
        return;
    }
    let prev = HID_MODS_LAST.load(Relaxed);
    let (was, now) = (prev & role != 0, m & role != 0);
    let others = m & !role;
    match (was, now) {
        (false, true) => TAP.store(if others == 0 && prev & !role == 0 { 1 } else { 2 }, Relaxed),
        (true, true) if others != 0 => TAP.store(2, Relaxed),
        (true, false) => {
            if TAP.swap(0, Relaxed) == 1 {
                START_TAPS.fetch_add(1, Relaxed);
            }
        }
        _ => {}
    }
}

/// From `resolve_edge` with the report pair: any key press edge spoils an armed tap; an Esc edge is counted.
fn note_keys(cur: &[u8; 6], prev: &[u8; 6]) {
    use core::sync::atomic::Ordering::Relaxed;
    for &u in cur.iter() {
        if u != 0 && !prev.contains(&u) {
            if TAP.load(Relaxed) == 1 {
                TAP.store(2, Relaxed);
            }
            if u == 0x29 {
                ESC_EDGES.fetch_add(1, Relaxed);
            }
        }
    }
}

/// The Windows-key taps since the last take (the look's service pass toggles Start on an odd count).
pub fn take_start_taps() -> u32 {
    START_TAPS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

/// The Esc press edges since the last take.
pub fn take_esc_edges() -> u32 {
    ESC_EDGES.swap(0, core::sync::atomic::Ordering::Relaxed)
}

/// `[keymap] table=<name> cmd_role=<gui|alt> option=<gui|alt> start=<option|win|none> via=<who>` — the live table, named.
pub fn announce(via: &str) {
    let t = active();
    let nm = |r: u8| if r == HID_MOD_GUI { "gui" } else if r == HID_MOD_ALT { "alt" } else { "other" };
    serial_println!("[keymap] table={} cmd_role={} option={} start={} via={}", t.name, nm(t.cmd_role), nm(t.option_role()), start_role_name(), via);
}

/// SOFTY2's fixture leg (`tests softy` prints it in its line): the two mirrored tables resolved from SYNTHETIC
/// modifier bytes, and the tap detector driven through `note_mods` / `resolve_edge` exactly as a decoder drives them.
/// Restores the detector's state and the last modifier byte. Returns `(softy_mac_ok, una_pc_ok, tap_ok, spoil_ok)`:
///
/// * softy-mac: physical Command+C (`HID_MOD_GUI`) is Copy, Ctrl+Command+L is LockScreen (Windows' Ctrl+Alt+L);
///   physical Option+C (`HID_MOD_ALT`) is NOTHING (Option is the Windows key there).
/// * una-pc: PC Alt+C is Copy (the Mac's ⌘C), Alt+Win+← is SnapLeft (⌘⌥←); Win+C alone is nothing.
/// * tap: the Windows key down then up alone counts ONE tap (Softy only — the caller has switched the look).
/// * spoil: down, a key edge (`e`), up counts NONE.
pub fn softy2_selftest() -> (bool, bool, bool, bool) {
    use core::sync::atomic::Ordering::Relaxed;
    let sm = SOFTY_MAC_BINDINGS;
    let up = UNA_PC_BINDINGS;
    let softy_mac_ok = resolve(sm, HID_MOD_GUI, 0x06) == Some(Action::Copy)
        && resolve(sm, HID_MOD_CTRL | HID_MOD_GUI, 0x0F) == Some(Action::LockScreen)
        && resolve(sm, HID_MOD_ALT, 0x06).is_none()
        && sm.option_role() == HID_MOD_GUI;
    let una_pc_ok = resolve(up, HID_MOD_ALT, 0x06) == Some(Action::Copy)
        && resolve(up, HID_MOD_ALT | HID_MOD_GUI, 0x50) == Some(Action::SnapLeft)
        && resolve(up, HID_MOD_GUI, 0x06).is_none()
        && up.option_role() == HID_MOD_GUI;
    let mods0 = HID_MODS_LAST.load(Relaxed);
    let taps0 = START_TAPS.swap(0, Relaxed);
    let role = start_role();
    let none = [0u8; 6];
    let mut e = [0u8; 6];
    e[0] = 0x08; // `e`
    note_mods(0);
    note_mods(role);
    note_mods(0);
    let tap_ok = role != 0 && START_TAPS.swap(0, Relaxed) == 1;
    note_mods(role);
    let _ = resolve_edge(active(), &e, &none, role);
    note_mods(0);
    let spoil_ok = role != 0 && START_TAPS.swap(0, Relaxed) == 0;
    ESC_EDGES.store(0, Relaxed);
    TAP.store(0, Relaxed);
    HID_MODS_LAST.store(mods0, Relaxed);
    START_TAPS.store(taps0, Relaxed);
    (softy_mac_ok, una_pc_ok, tap_ok, spoil_ok)
}

// ══════════════ WINKEY (rmbp-ledger B665) — the fixture's driver — TAIL-APPENDED ══════════════

/// WINKEY's fixture driver: ONE press of the key that plays the Windows role on the live look and keyboard
/// ([`start_role`]), driven through [`note_mods`] (and, for `chord`, [`resolve_edge`] with an `e` edge between the down
/// and the up) exactly as a decoder drives them. The counted tap (if any) is LEFT for the caller's service pass; the
/// last modifier byte and the detector's state are restored. Under the Una look the role is 0 and nothing is counted.
pub fn winkey_drive(chord: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    let mods0 = HID_MODS_LAST.load(Relaxed);
    let role = start_role();
    let none = [0u8; 6];
    let mut e = [0u8; 6];
    e[0] = 0x08; // `e`
    note_mods(0);
    note_mods(role);
    if chord {
        let _ = resolve_edge(active(), &e, &none, role);
    }
    note_mods(0);
    TAP.store(0, Relaxed);
    HID_MODS_LAST.store(mods0, Relaxed);
}

// ══════════════ KEYMAPS (rmbp-ledger B704) — the LAYOUT seam — TAIL-APPENDED ══════════════
//
// The chord table above judges what a chord MEANS; the layout tables (`input/layouts.rs`, declared here so the crate
// root is untouched) say what a key TYPES on the operator's keyboard: us gb de fr es it se ch, dead keys, AltGr, the
// Compose tap. ONE call per HID boot report is the whole seam, beside `resolve_edge`, which a decoder already makes.

#[path = "../input/layouts.rs"]
pub mod layouts;

/// KEYMAPS: THE LAYOUT SEAM — one HID boot report's PRESS edges typed on the live layout (`system.keyboard.layout`),
/// through the dead-key and Compose state machine; returns how many chars were written to `out`. A key the layout
/// does not claim (a chord, a Ctrl fold, a non-printing key) writes nothing and stays on `hid_key_ascii`'s path.
pub fn layout_report(cur_keys: &[u8; 6], prev_keys: &[u8; 6], modifiers: u8, caps: bool, out: &mut [char; 12]) -> usize {
    layouts::report_live(cur_keys, prev_keys, modifiers, caps, out)
}

// ══════════════ ZOOM (rmbp-ledger B713) — the magnifier's chord rows — TAIL-APPENDED ══════════════
//
// The lens (`video/zoom.rs`) steps on `Win+=` (in: off -> x2 -> x3 -> x4), `Win+-` (out: x4 -> x3 -> x2 -> off) and
// clears on `Win+0`. The modifier is the PHYSICAL key in the Windows key's position, `HID_MOD_GUI` (Win on a PC
// keyboard, Command on an Apple one), on every table and both looks: the magnifier is a property of the glass, not
// of the look's chord dialect, and no shipped table binds GUI with `=`, `-` or `0`. GUI held suppresses the typed
// character (`hid_key_ascii`), so the chord types nothing; it resolves to no `Action` (nothing is delivered to a
// window) and costs a Softy Windows-key tap nothing it did not already cost (`note_keys` spoils a tap on any edge).
// The rows are a TABLE here (BP-input-2), judged at the one seam both HID decoders reach ([`resolve_edge`]).

/// `(HID usage, step)`: `=` steps in, `-` steps out, `0` clears (`zoom::stepped`).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub const ZOOM_ROWS: [(u8, i8); 3] = [(0x2E, 1), (0x2D, -1), (0x27, 0)];

/// From [`resolve_edge`]: each press edge of a `ZOOM_ROWS` usage with `HID_MOD_GUI` held is one step (atomics only).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn zoom_edge(cur: &[u8; 6], prev: &[u8; 6], modifiers: u8) {
    if modifiers & HID_MOD_GUI == 0 {
        return;
    }
    for &u in cur.iter() {
        if u != 0 && !prev.contains(&u) {
            if let Some(&(_, step)) = ZOOM_ROWS.iter().find(|r| r.0 == u) {
                super::zoom::chord(step);
            }
        }
    }
}

// AUTOLOCK (rmbp-ledger B712) — TAIL-APPENDED, so no line above moves. THE Win+L ENTRY: the key that plays the Windows
// role on the live look and keyboard ([`start_role`]: Option on a Mac keyboard under Softy, Win on a PC keyboard under
// Softy) or, under Una (no Start role), the GUI key (Win on a PC keyboard; ⌘ on a Mac, where ⌘L's row has already
// answered with the same action). A role-space row cannot say it: no role maps the Win key on the PC table, and the
// mirrored tables put it under Option. Asked by [`resolve_chord`] (LOCK2, B728; was `resolve_edge` alone) only after every row
// declined, so no row's token moves.

/// Win+L: L (0x0F) going down with the Windows key held: `LockScreen`, token `win-l`. Pure over its arguments and the
/// live look/keyboard. LOCK2 (B728): asked by [`resolve_chord`] after its rows (so `resolve`'s callers and
/// `resolve_edge`'s per-edge walk both reach it); the edge itself is the caller's, as for every row.
fn win_l(modifiers: u8, usage_edge: u8) -> Option<(Action, &'static str)> {
    let role = start_role();
    let win = if role != 0 { role } else { HID_MOD_GUI };
    if modifiers & win != 0 && usage_edge == 0x0F {
        Some((Action::LockScreen, "win-l"))
    } else {
        None
    }
}
