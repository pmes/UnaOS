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
///
/// TERMSEL2 grew it three ways and replaced nothing: a ROW beside each offset (`anchor_row`,
/// `head_row` — an absolute scrollback line number, or [`EDIT_ROW`] for the editable line, so the
/// pair is a start CELL and an end CELL); a CARET on the editable line; and the pointer's press
/// state ([`Ptr`]). A selection whose two rows are both [`EDIT_ROW`] is exactly TERMSEL's, and
/// [`LineSel::range`] still answers only for that case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineSel {
    anchor: usize,
    head: usize,
    live: bool,
    /// TERMSEL2 — the row of `anchor` / `head`: [`EDIT_ROW`] or an absolute scrollback line.
    anchor_row: u64,
    head_row: u64,
    /// TERMSEL2 — the caret, a cell BOUNDARY on the editable line; [`CARET_END`] follows the line
    /// end, which is where TERMSEL's editor always inserted.
    caret: usize,
    /// TERMSEL2 — the pointer's press state.
    ptr: Ptr,
}

/// The row of the EDITABLE line in a cell address. Scrollback rows are ABSOLUTE line numbers
/// (`Console::hist_base` + the index into `history`), so a line keeps its number while newer output
/// pushes it up the screen, and the editable line sorts after every one of them.
pub const EDIT_ROW: u64 = u64::MAX;

/// The caret value meaning "at the end of the line, however long it gets" — TERMSEL's only
/// insertion point, and still the caret's resting place until something moves it.
const CARET_END: usize = usize::MAX;

/// A double-click is a second DOWN on the SAME cell within this many milliseconds of the first.
/// 500 ms is the macOS default ("Double-click speed", the middle of the slider).
pub const DBL_MS: u64 = 500;

/// A cell address: `(row, col)`. Ordered by row, then column, which is reading order.
pub type Cell = (u64, usize);

/// The pointer's press state over the terminal's text (TERMSEL2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ptr {
    /// A primary press on the text area is DOWN.
    held: bool,
    /// The press was a double-click: its word is selected and drags are ignored until the release.
    word: bool,
    /// The press cell — the ANCHOR cell of a drag.
    cell: Cell,
    /// The last drag cell, so motion inside one cell prints and repaints nothing.
    drag: Cell,
    /// The last DOWN, for the double-click test: `(ms, cell)`; `last_ok` false when there is none.
    last_ms: u64,
    last_cell: Cell,
    last_ok: bool,
}

impl Ptr {
    const fn new() -> Self {
        Self {
            held: false,
            word: false,
            cell: (EDIT_ROW, 0),
            drag: (EDIT_ROW, 0),
            last_ms: 0,
            last_cell: (EDIT_ROW, 0),
            last_ok: false,
        }
    }
}

impl LineSel {
    /// No selection.
    pub const fn new() -> Self {
        Self {
            anchor: 0,
            head: 0,
            live: false,
            anchor_row: EDIT_ROW,
            head_row: EDIT_ROW,
            caret: CARET_END,
            ptr: Ptr::new(),
        }
    }

    /// The selected cells as `[lo, hi)`, CLAMPED to a line of `len` cells, or `None`. The ONE
    /// reading of the state: the painter, `Copy`, `Cut` and the witness all ask this, so the band on
    /// the glass and the bytes on the clipboard cannot disagree.
    ///
    /// TERMSEL2: this answers ONLY for a selection that lies wholly on the editable line (both rows
    /// [`EDIT_ROW`]); one that reaches the scrollback is read with [`LineSel::span`].
    pub fn range(&self, len: usize) -> Option<(usize, usize)> {
        if !self.live || self.anchor_row != EDIT_ROW || self.head_row != EDIT_ROW {
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
        let before = self.span(len);
        match act {
            Action::SelectAll => {
                self.anchor = 0;
                self.head = len;
                self.live = true;
                self.anchor_row = EDIT_ROW;
                self.head_row = EDIT_ROW;
            }
            Action::Deselect => self.live = false,
            Action::SelectLeft | Action::SelectRight | Action::SelectLineStart | Action::SelectLineEnd => {
                // TERMSEL2: with no selection ON THE EDITABLE LINE (none at all, or one in the
                // scrollback) a head motion starts a fresh one at the CARET — TERMSEL's "at the line
                // end" is the caret's resting place, so a caret nobody moved reads exactly as before.
                if self.range(len).is_none() {
                    let k = self.caret_col(len);
                    self.anchor = k;
                    self.head = k;
                    self.anchor_row = EDIT_ROW;
                    self.head_row = EDIT_ROW;
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
        if self.span(len).is_none() {
            self.live = false;
        }
        // TERMSEL2: the caret rides the HEAD of an editable-line selection, as a Mac text field's
        // insertion point does, so `Shift+←` then `→` continues from where the head stopped.
        if self.range(len).is_some() {
            self.set_caret(self.head, len);
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
        // TERMSEL2: a dispatched line leaves an empty one, and its caret at the (new) end.
        if c == b'\n' || c == b'\r' {
            self.caret = CARET_END;
        }
        let before = self.span(len);
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

    /// The ONE witness site: prints iff the selection differs from `before`. A selection wholly on
    /// the editable line prints TERMSEL's `sel=` line, byte for byte; one that reaches the scrollback
    /// prints `span=(c,r)..(c,r)` — its start and end CELLS, the editable line's row written `e`.
    fn note(&self, before: Option<(Cell, Cell)>, len: usize, by: &str) -> bool {
        let after = self.span(len);
        if after == before {
            return false;
        }
        match after {
            Some(((r0, c0), (r1, c1))) if r0 != EDIT_ROW || r1 != EDIT_ROW => serial_println!(
                "[termsel] span=({},{})..({},{}) line={} by={}",
                c0,
                RowFmt(r0),
                c1,
                RowFmt(r1),
                len,
                by
            ),
            Some(((_, lo), (_, hi))) => serial_println!(
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

    // --- TERMSEL2 ------------------------------------------------------------------------------

    /// The selection as its start and end CELLS in reading order, or `None` (no selection, or a
    /// collapsed one). Editable-line columns are clamped to a line of `len` cells; scrollback
    /// columns are not — the reader clamps them to the row it holds ([`LineSel::cols_on`]).
    pub fn span(&self, len: usize) -> Option<(Cell, Cell)> {
        if !self.live {
            return None;
        }
        let clamp = |row: u64, col: usize| if row == EDIT_ROW { core::cmp::min(col, len) } else { col };
        let a = (self.anchor_row, clamp(self.anchor_row, self.anchor));
        let h = (self.head_row, clamp(self.head_row, self.head));
        let (s, e) = if a <= h { (a, h) } else { (h, a) };
        if s == e {
            None
        } else {
            Some((s, e))
        }
    }

    /// The caret's column on a line of `len` cells.
    pub fn caret_col(&self, len: usize) -> usize {
        core::cmp::min(self.caret, len)
    }

    /// Put the caret at boundary `pos` of a line of `len` cells; at or past the end it FOLLOWS the
    /// end ([`CARET_END`]), so typing at the end of the line keeps it there.
    fn set_caret(&mut self, pos: usize, len: usize) {
        self.caret = if pos >= len { CARET_END } else { pos };
    }

    /// TERMSEL2 — one POINTER press on the terminal's text, already turned into a cell by the
    /// console (`Console::pointer`, which owns the layout): `row` is an absolute scrollback line or
    /// [`EDIT_ROW`], `col` the cell under the pointer (a column past the row's last character is
    /// allowed and means "past the end"), `text` that row's text, `len` the editable line's length.
    ///
    /// A DOWN on the same cell as the previous DOWN within [`DBL_MS`] is a double-click, `kind=dbl`;
    /// a drag prints only when it enters a NEW cell. Every press is on the wire as
    /// `[termsel] press cell=(c,r) kind=down|drag|up|dbl`. Returns the repaint the console owes: 0
    /// none, 1 the input line, 2 the whole terminal.
    pub fn pointer(&mut self, kind: PressKind, ms: u64, row: u64, col: usize, text: &str, len: usize) -> u8 {
        let _ = (text, len);
        let cell = (row, col);
        match kind {
            PressKind::Down => {
                let dbl = self.ptr.last_ok
                    && self.ptr.last_cell == cell
                    && ms.wrapping_sub(self.ptr.last_ms) <= DBL_MS;
                // A double-click consumes the pair: a third press is a fresh single click.
                self.ptr.last_ok = !dbl;
                self.ptr.last_ms = ms;
                self.ptr.last_cell = cell;
                self.ptr.held = true;
                self.ptr.word = dbl;
                self.ptr.cell = cell;
                self.ptr.drag = cell;
                serial_println!(
                    "[termsel] press cell=({},{}) kind={}",
                    col,
                    RowFmt(row),
                    if dbl { "dbl" } else { "down" }
                );
                0
            }
            PressKind::Drag => {
                if !self.ptr.held || self.ptr.word || cell == self.ptr.drag {
                    return 0;
                }
                self.ptr.drag = cell;
                serial_println!("[termsel] press cell=({},{}) kind=drag", col, RowFmt(row));
                0
            }
            PressKind::Up => {
                if !self.ptr.held {
                    return 0;
                }
                self.ptr.held = false;
                serial_println!("[termsel] press cell=({},{}) kind=up", col, RowFmt(row));
                0
            }
        }
    }
}

/// A row as the wire writes it: the absolute scrollback line number, or `e` for the editable line.
struct RowFmt(u64);

impl core::fmt::Display for RowFmt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == EDIT_ROW {
            f.write_str("e")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

// --- TERMSEL2: the pointer's way in ----------------------------------------------------------------
//
// The click router (`arch::x86_64::syscall::wc_click_route_at`) CONSUMES a press on the shell
// window — it is kernel furniture, so the press is never an `Event::Button` the render service
// sees — and it runs on the same input path as every other press. So the router NOTES the press
// here, in PANEL pixels turned into the window's SURFACE pixels, and the render service takes the
// notes for its own window after each routed event and hands them to its `Console`, which owns the
// text layout and turns a pixel into a cell. The router knows windows and not text; the console
// knows text and not windows; this queue is the whole seam between them.

/// What the pointer did. A double-click is not a kind the router can see — it is two DOWNs, and the
/// model ([`LineSel::pointer`]) classifies the second one from its time and its CELL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressKind {
    Down,
    Drag,
    Up,
}

/// One pointer note: which window, where on its SURFACE (source pixels, before the compositor's
/// upscale; negative or past the edge while a drag leaves the window), what, and when (`arch::ms`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    pub win: u32,
    pub lx: i32,
    pub ly: i32,
    pub kind: PressKind,
    pub ms: u64,
}

const PRESSQ_CAP: usize = 16;

struct PressQ {
    q: [Press; PRESSQ_CAP],
    n: usize,
    dropped: u64,
}

static PRESSQ: spin::Mutex<PressQ> = spin::Mutex::new(PressQ {
    q: [Press { win: 0, lx: 0, ly: 0, kind: PressKind::Up, ms: 0 }; PRESSQ_CAP],
    n: 0,
    dropped: 0,
});

/// The window a primary press is HELD on (`wm::WIN_NONE` = 0 when none): a drag and the release
/// follow the press to it, never to whatever the pointer has since moved over — the router's own
/// rule for a release (`CLICK_PRESS_TARGET`).
static HELD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Queue one note for `win` at PANEL `(x, y)`. A drag directly behind another drag for the same
/// window REPLACES it (the pointer's latest cell is the only one that matters, and a sweep would
/// otherwise fill the queue); a full queue drops its OLDEST note and says so on the wire.
fn note(win: u32, x: i32, y: i32, kind: PressKind) {
    let Some(i) = super::wm::info(win) else {
        return;
    };
    let sc = core::cmp::max(i.scale, 1) as i32;
    let p = Press {
        win,
        lx: (x - i.x as i32).div_euclid(sc),
        ly: (y - i.y as i32).div_euclid(sc),
        kind,
        ms: crate::arch::ms(),
    };
    let mut q = PRESSQ.lock();
    let n = q.n;
    if kind == PressKind::Drag && n > 0 && q.q[n - 1].win == win && q.q[n - 1].kind == PressKind::Drag {
        q.q[n - 1] = p;
        return;
    }
    if n == PRESSQ_CAP {
        q.q.copy_within(1.., 0);
        q.dropped += 1;
        let d = q.dropped;
        q.q[PRESSQ_CAP - 1] = p;
        drop(q);
        serial_println!("[termsel] press queue full dropped={} (oldest)", d);
        return;
    }
    q.q[n] = p;
    q.n = n + 1;
}

/// The ROUTER's press door: a primary press landed on the content of the shell window `win`.
pub fn pointer_press(win: u32, x: i32, y: i32) {
    HELD.store(win, core::sync::atomic::Ordering::Release);
    note(win, x, y, PressKind::Down);
}

/// Is a press held on the shell's text? The router asks this before it reads the cursor for
/// [`pointer_motion`], so a boot nobody pressed the shell on pays one atomic load per report.
pub fn pointer_held() -> bool {
    HELD.load(core::sync::atomic::Ordering::Acquire) != 0
}

/// The router's MOTION door, called for every pointer report with the live cursor; one atomic load
/// when no press is held, which is every report of a boot nobody pressed the shell on.
pub fn pointer_motion(x: i32, y: i32) {
    let win = HELD.load(core::sync::atomic::Ordering::Acquire);
    if win != 0 {
        note(win, x, y, PressKind::Drag);
    }
}

/// The router's RELEASE door, called on every primary release edge. Only a release that ends a
/// press HELD here becomes a note.
pub fn pointer_release(x: i32, y: i32) {
    let win = HELD.swap(0, core::sync::atomic::Ordering::AcqRel);
    if win != 0 {
        note(win, x, y, PressKind::Up);
    }
}

/// Take the OLDEST note for `win`, or `None`. Notes for another window stay queued for their own
/// taker, so the render service and a fixture's probe window cannot eat each other's presses.
pub fn take_press(win: u32) -> Option<Press> {
    let mut q = PRESSQ.lock();
    let n = q.n;
    let i = q.q[..n].iter().position(|p| p.win == win)?;
    let p = q.q[i];
    q.q.copy_within(i + 1..n, i);
    q.n = n - 1;
    Some(p)
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

// --- TERMSEL2 fixture ----------------------------------------------------------------------------

/// TERMSEL2 — the pointer on the shell's text, end to end on the path a hand takes: a probe window
/// owned by `wm::KERNEL_OWNER_DESKTOP` (the shell window's owner), presses and releases driven
/// through the LIVE click router `wc_click_route_at` exactly as `clickroute_selftest` drives it,
/// motion through [`pointer_motion`] (what `wc_route_tail` calls with the live cursor), the notes
/// taken with [`take_press`] as the render service takes them, and a real `Console` — the shipped
/// layout, `Console::pointer_at` — turning each into a cell. QEMU delivers no pointer; the position is
/// the parameter, as in every click fixture on this path.
///
/// The probe surface is 288x72 at scale 1 (`Metrics::for_height(72)`: 8-px cells, 12-px lines, a
/// 12-px margin): two scrollback rows `alpha beta` and `gamma delta`, and the editable line
/// `unaos select` after the `architect@unaos:~$ ` prompt. Each press point is the centre of a cell,
/// computed here from the metrics and NOT from the console's lookup, so the two derivations meet.
///
/// LEGS, one bit each in `legs=<got>/<want>`:
///  * `hit` (0x1) — every probe point hit-tests to the probe row (else SKIP: the panel is not ours).
///  * `route` (0x2) — a primary press on scrollback cell (2,0) is CONSUMED by the router and arrives
///    as ONE `down` note, which the console reads as cell `(2,0)`.
///  * `drag` (0x4) — motion to cell (3,1) arrives as a `drag` note read as `(3,1)`.
///  * `up` (0x8) — the release is consumed, arrives as an `up` note at `(3,1)`, and nothing is held.
///  * `dbl` (0x10) — two presses on editable cell (8,e) inside [`DBL_MS`]: the second is `dbl`.
#[cfg(all(target_arch = "x86_64", feature = "witness"))]
pub fn pointer_selftest() {
    use super::wm;
    use crate::arch::x86_64::syscall::{user_input_active, user_input_set_active, wc_click_route_at};
    use crate::pal::Event;
    use alloc::string::String;

    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            serial_println!(":: TERMSEL2: SKIP (framebuffer not ready) ::");
            return;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    const W: usize = 288;
    const H: usize = 72;
    if pw < 2 * W || ph < 4 * H {
        serial_println!(":: TERMSEL2: SKIP (panel {}x{} too small) ::", pw, ph);
        return;
    }
    let store = alloc::vec![0u32; W * H];
    let (ox, oy) = (pw / 4, ph / 3);
    let win = wm::create_at(
        wm::KERNEL_OWNER_DESKTOP,
        store.as_ptr() as usize,
        W * H * 4,
        W as u32,
        H as u32,
        (W * 4) as u32,
        b"ts2",
        ox,
        oy,
    );
    if win == wm::WIN_NONE {
        serial_println!(":: TERMSEL2: SKIP (window table full) ::");
        return;
    }
    let Some(info) = wm::info(win) else {
        wm::close(win);
        serial_println!(":: TERMSEL2: SKIP (probe row vanished) ::");
        return;
    };
    let saved_focus = user_input_active();
    // A stale note for this id (an earlier holder of the slot) must not be read as ours.
    while take_press(win).is_some() {}

    let m = crate::ui::Metrics::for_height(H);
    let mut con = crate::console::Console::new();
    con.mark_in_window();
    con.place_for_fixture("alpha beta");
    con.place_for_fixture("gamma delta");
    con.current_input = String::from("unaos select");
    let prompt = con.prompt_cells();
    let sc = core::cmp::max(info.scale, 1);
    // The PANEL point at the centre of cell `col` of shown row `vrow` (the editable line is shown
    // row 2 here: two scrollback rows above it).
    let at = |vrow: usize, col: usize| -> (i32, i32) {
        let x0 = m.margin + if vrow == 2 { m.text_w(prompt) } else { 0 };
        let lx = x0 + col * m.cell_w + m.cell_w / 2;
        let ly = m.margin + vrow * m.line_h + m.cell_h / 2;
        ((info.x + lx * sc) as i32, (info.y + ly * sc) as i32)
    };
    // Take every note queued for the probe row and hand it to the console; return the last as
    // `(kind, row, col)` and how many there were.
    let mut take = |con: &mut crate::console::Console| -> (Option<(PressKind, u64, usize)>, usize) {
        let mut last = None;
        let mut n = 0usize;
        while let Some(p) = take_press(win) {
            n += 1;
            let (row, col, _) = con.cell_at(m, W, H, p.lx, p.ly);
            con.pointer_at(&p, m, W, H);
            last = Some((p.kind, row, col));
        }
        (last, n)
    };

    let p_a = at(0, 2);
    let p_b = at(1, 3);
    let p_w = at(2, 8);
    let hit = [p_a, p_b, p_w]
        .iter()
        .all(|&(x, y)| wm::hit_test(x, y).map(|(w, _, _)| w) == Some(win));
    let mut got = 0u32;
    let (mut route, mut drag, mut up, mut dbl) = (false, false, false, false);
    if hit {
        got |= 0x1;
        // route — a press on the scrollback is consumed and noted for THIS window.
        let c = wc_click_route_at(Event::Button(1), p_a.0, p_a.1);
        route = c && take(&mut con) == (Some((PressKind::Down, 0, 2)), 1);
        // drag — motion while held.
        pointer_motion(p_b.0, p_b.1);
        drag = take(&mut con) == (Some((PressKind::Drag, 1, 3)), 1);
        // up — the release is consumed and ends the hold.
        let c = wc_click_route_at(Event::Button(0), p_b.0, p_b.1);
        up = c && take(&mut con) == (Some((PressKind::Up, 1, 3)), 1) && !pointer_held();
        // dbl — two presses on one editable cell, well inside DBL_MS.
        let mut kinds = 0usize;
        for _ in 0..2 {
            wc_click_route_at(Event::Button(1), p_w.0, p_w.1);
            if take(&mut con).0 == Some((PressKind::Down, EDIT_ROW, 8)) {
                kinds += 1;
            }
            wc_click_route_at(Event::Button(0), p_w.0, p_w.1);
            let _ = take(&mut con);
        }
        dbl = kinds == 2 && con.sel.ptr.word == true && !con.sel.ptr.held;
    }
    for (bit, ok) in [(0x2u32, route), (0x4, drag), (0x8, up), (0x10, dbl)] {
        if ok {
            got |= bit;
        }
    }
    const WANT: u32 = 0x1f;
    serial_println!(
        ":: TERMSEL2: legs={:#x}/{:#x} hit={} route={} drag={} up={} dbl={} -> {} ::",
        got,
        WANT,
        yn(hit),
        yn(route),
        yn(drag),
        yn(up),
        yn(dbl),
        if got == WANT { "PASS" } else { "FAIL" }
    );

    wm::close(win);
    while take_press(win).is_some() {}
    drop(store);
    user_input_set_active(saved_focus);
    wm::focus_reset();
}
