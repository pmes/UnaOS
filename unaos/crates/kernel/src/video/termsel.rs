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

//! TERMSEL — **the terminal's selection**: which cells of the shell's editable line are selected,
//! so `⌘C` copies them instead of the whole line and `⌘X` has something to cut.
//!
//! # The model, in one paragraph
//!
//! The terminal's text is a read-only scrollback plus ONE editable line
//! (`console::Console::current_input`), and the editor has no caret: `main::handle_key` appends a
//! printable byte, pops on Backspace and dispatches on Return, and nothing else changes the line.
//! Every byte in it is printable ASCII, so a byte offset IS a cell column. A selection here is an
//! ANCHOR and a HEAD over that one line — the anchor where the selection started, the head the end
//! that moves. With no caret, a selection that starts from nothing starts at the LINE END, where
//! the block cursor is painted. The scrollback is not selectable in this arc (see
//! `docs/dev/OS/08_VIDEO/clipboard.md` §7.2 and §7.7).
//!
//! # Where each part lives
//!
//!  * **The chords** — rows in `video/theme.rs`'s tables, resolved by `video::keymap` at the HID
//!    decoder and delivered as `pal::Event::Action`. Nothing here tests a modifier or a usage; the
//!    terminal only ever sees an [`Action`] by name (R60, R61).
//!  * **The model** — [`LineSel`], this file. It is the ONE place selection state changes, and the
//!    only place the `[termsel]` witness is printed, once per STATE CHANGE and never per repaint.
//!  * **The consumer** — `video::clipboard::terminal_action`, which routes the selection actions
//!    here and reads [`LineSel::range`] for `Copy` and `Cut`.
//!  * **The edit rule** — `main::handle_key` calls [`LineSel::on_edit`] before every edit, so a
//!    typed byte, a Backspace, a Return and every byte of a paste drop the selection first.
//!  * **The painter** — `console::Console::draw_prompt_line` paints [`LineSel::range`] as an
//!    inverse-video band. It reads the model and never writes it.

use crate::video::keymap::Action;

/// A selection over the editable line: an ANCHOR, a HEAD, and whether one is live. Offsets are
/// byte offsets into `Console::current_input`, which are cell columns (every byte is printable
/// ASCII). `live` is separate from the offsets so "no selection" is a state and not a coincidence
/// of two numbers, and [`LineSel::range`] folds a collapsed pair (anchor == head) into "none".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineSel {
    anchor: usize,
    head: usize,
    live: bool,
}

impl LineSel {
    /// No selection.
    pub const fn new() -> Self {
        Self { anchor: 0, head: 0, live: false }
    }

    /// The selected cells as `[lo, hi)`, CLAMPED to a line of `len` cells, or `None`. The ONE
    /// reading of the state: the painter, `Copy`, `Cut` and the witness all ask this, so the band on
    /// the glass and the bytes on the clipboard cannot disagree.
    pub fn range(&self, len: usize) -> Option<(usize, usize)> {
        if !self.live {
            return None;
        }
        let a = core::cmp::min(self.anchor, len);
        let h = core::cmp::min(self.head, len);
        let (lo, hi) = if a <= h { (a, h) } else { (h, a) };
        if lo == hi {
            None
        } else {
            Some((lo, hi))
        }
    }

    /// Is this one of the actions [`LineSel::apply`] acts on?
    pub const fn is_selection_action(act: Action) -> bool {
        matches!(
            act,
            Action::SelectAll
                | Action::SelectLeft
                | Action::SelectRight
                | Action::SelectLineStart
                | Action::SelectLineEnd
                | Action::Deselect
        )
    }

    /// Apply one selection action to a line of `len` cells. Returns `true` iff the selection
    /// CHANGED — and then, and only then, the `[termsel]` witness has been printed and the caller
    /// owes a repaint of the input line.
    ///
    ///  * `SelectAll` — anchor 0, head the line end.
    ///  * `SelectLeft`/`SelectRight` — the head one cell left/right, stopping at 0 / the line end.
    ///  * `SelectLineStart`/`SelectLineEnd` — the head to 0 / the line end.
    ///  * `Deselect` — no selection.
    ///
    /// A head motion with no selection live starts one at the LINE END (anchor = head = `len`),
    /// because that is where the editor's only insertion point is. A pair that meets collapses to no
    /// selection.
    pub fn apply(&mut self, act: Action, len: usize) -> bool {
        let before = self.range(len);
        match act {
            Action::SelectAll => {
                self.anchor = 0;
                self.head = len;
                self.live = true;
            }
            Action::Deselect => self.live = false,
            Action::SelectLeft | Action::SelectRight | Action::SelectLineStart | Action::SelectLineEnd => {
                if before.is_none() {
                    self.anchor = len;
                    self.head = len;
                }
                let head = core::cmp::min(self.head, len);
                self.anchor = core::cmp::min(self.anchor, len);
                self.head = match act {
                    Action::SelectLeft => head.saturating_sub(1),
                    Action::SelectRight => core::cmp::min(head + 1, len),
                    Action::SelectLineStart => 0,
                    _ => len,
                };
                self.live = true;
            }
            _ => return false,
        }
        if self.range(len).is_none() {
            self.live = false;
        }
        self.note(before, len, act.name())
    }

    /// THE EDIT RULE. Called by the line editor BEFORE it applies byte `c` to a line of `len`
    /// cells: a byte that edits the line (printable, BS/DEL, CR/LF) drops a live selection. The
    /// editor has no caret, so it cannot replace a selection with what is typed (a Mac text field
    /// would); keeping the selection across the edit would paint the band over cells that moved.
    /// A byte that edits nothing — Esc, an arrow's `0x1C..0x1F` — leaves it alone, which matters:
    /// the decoders push `Shift+←`'s arrow byte onto the ring JUST AHEAD of its `SelectLeft`.
    /// Returns `true` iff the selection changed.
    pub fn on_edit(&mut self, c: u8, len: usize) -> bool {
        let edits = c == b'\n' || c == b'\r' || c == 8 || c == 0x7F || (c >= 32 && c <= 126);
        if !edits {
            return false;
        }
        let before = self.range(len);
        self.live = false;
        self.note(before, len, "edit")
    }

    /// Drop the selection WITHOUT a witness and hand back what it covered — for `Cut`, which
    /// changes the line and the selection in one step and prints ONE line after both, with the
    /// line's new length ([`LineSel::witness_cleared`]).
    pub fn take(&mut self, len: usize) -> Option<(usize, usize)> {
        let r = self.range(len);
        self.live = false;
        r
    }

    /// The witness for a selection dropped by [`LineSel::take`], once the line is `len` cells.
    pub fn witness_cleared(len: usize, by: &str) {
        serial_println!("[termsel] none line={} by={}", len, by);
    }

    /// The ONE witness site: prints iff the selection differs from `before`.
    fn note(&self, before: Option<(usize, usize)>, len: usize, by: &str) -> bool {
        let after = self.range(len);
        if after == before {
            return false;
        }
        match after {
            Some((lo, hi)) => serial_println!(
                "[termsel] sel={}..{} cells={} line={} by={}",
                lo,
                hi,
                hi - lo,
                len,
                by
            ),
            None => serial_println!("[termsel] none line={} by={}", len, by),
        }
        true
    }
}

// --- fixture ---------------------------------------------------------------------------------

/// TERMSEL — the arc's gate, chained from `drivers::ehci::parser_selftest` AFTER
/// `video::clipboard::selftest` (APPCLIP's verdict stays on its own line).
///
/// It drives each CHORD, not each action: every step is a synthetic HID report pair resolved
/// through the LIVE table (`keymap::resolve_edge(keymap::active(), …)`), the resolved action is
/// pushed through `pal::push_event` into the REAL ring, taken back out of `pal::next_event`, and
/// handed to the SHIPPED consumer `clipboard::terminal_action` with a line and a [`LineSel`] that
/// stand where the console's stand. QEMU cannot press a key; this is the part of the path it can
/// run, and it is the same path from the resolver down.
///
/// WHAT EACH FIELD MEASURES:
///
///  * `resolved=` — steps whose chord resolved to the action it must, through the live table.
///  * `delivered=` — `Event::Action`s that came back OUT of the ring. Equal to `resolved=` or the
///    ring lost one.
///  * `left=` — `Shift+←` x3 then `Shift+→` on `unaos select`: the selection must read `10..12`.
///  * `copy_sel=` — `⌘C` with that selection: the clipboard must hold exactly `ct` (read back with
///    `clipboard::get`, i.e. through the epoch gate), not the line.
///  * `home=` — `⌘⇧←` (the Mac chord for the line start): `0..12`.
///  * `esc=` — `Esc`: no selection.
///  * `copy_line=` — `⌘C` with NO selection must still copy the whole line (APPCLIP preserved).
///  * `cut=` — `⌘A`, `Shift+←`, `⌘X`: the clipboard holds `unaos selec`, the line is `t`, and the
///    selection is gone.
///  * `cut_empty=` — `⌘X` with nothing selected is DECLINED (`empty`) and the line is untouched.
///  * `edit=` — `⌘A`, then one typed byte through [`LineSel::on_edit`]: the selection drops.
///  * `pc=` — the PC table resolves `Shift+Home`, `Shift+End`, `Shift+←`, `Shift+→` and `Esc` to the
///    same five actions (the Mac column's chords are the ones driven through the ring above).
///
/// Go-red: make `terminal_action`'s `Copy` arm ignore the selection (copy `line` whole) — the
/// clipboard reads `unaos select`, `copy_sel=no`, and the verdict is `-> FAIL`.
pub fn selftest() {
    use crate::drivers::xhci::{HID_MOD_GUI, HID_MOD_SHIFT};
    use crate::pal::{self, Event};
    use crate::video::keymap;
    use alloc::string::String;

    let mut pre = 0usize;
    while pal::next_event().is_some() {
        pre += 1;
    }
    serial_println!("[termsel] fixture pre-drain discarded={}", pre);

    const U_LEFT: u8 = 0x50;
    const U_RIGHT: u8 = 0x4F;
    const U_HOME: u8 = 0x4A;
    const U_END: u8 = 0x4D;
    const U_ESC: u8 = 0x29;
    const U_A: u8 = 0x04;
    const U_C: u8 = 0x06;
    const U_X: u8 = 0x1B;

    let mut line = String::from("unaos select");
    let mut sel = LineSel::new();
    let mut resolved = 0usize;
    let mut delivered = 0usize;
    let mut steps = 0usize;

    // One chord, end to end: resolve through the live table, push, drain, consume. Returns the
    // consumer's verdict field ("-" when nothing came out of the ring).
    let mut chord = |usage: u8, mods: u8, want: Action, line: &mut String, sel: &mut LineSel| -> &'static str {
        steps += 1;
        let none: [u8; 6] = [0; 6];
        let cur: [u8; 6] = [usage, 0, 0, 0, 0, 0];
        let hit = keymap::resolve_edge(keymap::active(), &cur, &none, mods);
        let act = match hit {
            Some((a, _)) if a == want => {
                resolved += 1;
                a
            }
            _ => return "-",
        };
        pal::push_event(Event::Action(act));
        let mut out = "-";
        while let Some(ev) = pal::next_event() {
            if let Event::Action(a) = ev {
                delivered += 1;
                out = crate::video::clipboard::terminal_action(a, line, sel).0;
            }
        }
        out
    };
    let clip_is = |want: &str| -> bool {
        let mut buf = [0u8; 64];
        let n = crate::video::clipboard::get(&mut buf);
        &buf[..n] == want.as_bytes()
    };

    // left= — Shift+Left x3 from nothing selects the last three cells; Shift+Right gives one back.
    for _ in 0..3 {
        chord(U_LEFT, HID_MOD_SHIFT, Action::SelectLeft, &mut line, &mut sel);
    }
    chord(U_RIGHT, HID_MOD_SHIFT, Action::SelectRight, &mut line, &mut sel);
    let left = sel.range(line.len()) == Some((10, 12));

    // copy_sel= — the selection, not the line.
    let c1 = chord(U_C, HID_MOD_GUI, Action::Copy, &mut line, &mut sel);
    let copy_sel = c1 == "ok" && clip_is("ct") && sel.range(line.len()) == Some((10, 12));

    // home= — the Mac chord for the line start, from the live selection's anchor.
    chord(U_LEFT, HID_MOD_GUI | HID_MOD_SHIFT, Action::SelectLineStart, &mut line, &mut sel);
    let home = sel.range(line.len()) == Some((0, 12));

    // esc= — Deselect.
    chord(U_ESC, 0, Action::Deselect, &mut line, &mut sel);
    let esc = sel.range(line.len()).is_none();

    // copy_line= — no selection: APPCLIP's whole-line copy, unchanged.
    let c2 = chord(U_C, HID_MOD_GUI, Action::Copy, &mut line, &mut sel);
    let copy_line = c2 == "ok" && clip_is("unaos select");

    // cut= — select all, give back the last cell, cut.
    chord(U_A, HID_MOD_GUI, Action::SelectAll, &mut line, &mut sel);
    chord(U_LEFT, HID_MOD_SHIFT, Action::SelectLeft, &mut line, &mut sel);
    let x1 = chord(U_X, HID_MOD_GUI, Action::Cut, &mut line, &mut sel);
    let cut = x1 == "ok" && clip_is("unaos selec") && line == "t" && sel.range(line.len()).is_none();

    // cut_empty= — nothing selected: declined, line untouched.
    let x2 = chord(U_X, HID_MOD_GUI, Action::Cut, &mut line, &mut sel);
    let cut_empty = x2 == "empty" && line == "t";

    // edit= — a typed byte drops a live selection (the editor's rule, the shipped method).
    chord(U_A, HID_MOD_GUI, Action::SelectAll, &mut line, &mut sel);
    let had = sel.range(line.len()).is_some();
    sel.on_edit(b'!', line.len());
    line.push('!');
    let edit = had && sel.range(line.len()).is_none();

    // pc= — the PC column resolves the same five actions (Shift+Home/End are its line-end chords).
    let pc = keymap::resolve(super::theme::PC_BINDINGS, HID_MOD_SHIFT, U_HOME) == Some(Action::SelectLineStart)
        && keymap::resolve(super::theme::PC_BINDINGS, HID_MOD_SHIFT, U_END) == Some(Action::SelectLineEnd)
        && keymap::resolve(super::theme::PC_BINDINGS, HID_MOD_SHIFT, U_LEFT) == Some(Action::SelectLeft)
        && keymap::resolve(super::theme::PC_BINDINGS, HID_MOD_SHIFT, U_RIGHT) == Some(Action::SelectRight)
        && keymap::resolve(super::theme::PC_BINDINGS, 0, U_ESC) == Some(Action::Deselect);

    let pass = resolved == steps
        && delivered == steps
        && left
        && copy_sel
        && home
        && esc
        && copy_line
        && cut
        && cut_empty
        && edit
        && pc;
    serial_println!(
        ":: TERMSEL: resolved={}/{} delivered={} left={} copy_sel={} home={} esc={} copy_line={} cut={} cut_empty={} edit={} pc={} -> {} ::",
        resolved,
        steps,
        delivered,
        yn(left),
        yn(copy_sel),
        yn(home),
        yn(esc),
        yn(copy_line),
        yn(cut),
        yn(cut_empty),
        yn(edit),
        yn(pc),
        if pass { "PASS" } else { "FAIL" }
    );
}

/// `ok`/`no` — the fixture's field alphabet, as KEYMAP's.
const fn yn(v: bool) -> &'static str {
    if v {
        "ok"
    } else {
        "no"
    }
}
