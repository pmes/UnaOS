// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAMETAL (LEDGER SR90): the centre pane — Tabula's editor over `edit_core::Buffer`, the buffer of record the host
// Tabula links (EDITCORE, SR73; CHARTER: Tabula owns Text, seam shared-core). This file owns no text state of its
// own: the text, the cursor and the selections, grouped undo/redo and search are the core's; the pane keeps only
// what a view keeps — the file's path, a dirty flag, the scroll origin, the search prompt and one status message.
//
// FILES. Open = SYS_PATH_READ of the whole file (EDIT_MAX bound, refused past it rather than truncated — a
// truncated document saved back would destroy the tail), lossy UTF-8 (the status says so), `Buffer::new`. Save =
// `Buffer` text -> SYS_PATH_WRITE (truncate, then chunks), present on a `selfdiag` kernel; -ENOSYS otherwise, named
// in the status. Opening over unsaved edits is refused once ("Ctrl+O again discards").
//
// KEYS (focused). Printable ASCII inserts; Enter = `newline` (the core's auto-indent); Backspace = delete one
// grapheme back; Tab = four spaces; arrows = `move_cursors`; Ctrl+A / Ctrl+E line start / end; Ctrl+K delete the
// line; Ctrl+Z undo, Ctrl+Y redo; Ctrl+F a find prompt in the status line (Enter finds forward from the caret,
// wrapping, and selects the match; Esc leaves it), Ctrl+G finds the next match. Selection rides the desktop's
// resolved actions (INPUT_EV_ACTION, APPCLIP/TERMSEL): select-all, select-left/right, select-line-start/end,
// deselect, and TERMSEL2's caret moves — the ring carries no modifier bits, so Shift+arrow arrives that way.
//
// SYNTAXCORE (LEDGER SR89) runs this wave: `highlight` is the seam it fills — a per-line span painter the pane
// asks before drawing a line. Nothing is linked; `None` draws every line in the one ink.

use crate::fs::{errno_word, read_file, write_file, EDIT_MAX};
use crate::Line;
use alloc::string::String;
use alloc::vec::Vec;
use edit_core::{Buffer, Direction, Motion, SearchOpts, Selection, Unit};
use una_abi::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};

/// SYNTAXCORE (SR89) seam: fill `spans` with `(byte_start, byte_end, argb)` runs over one line's text. The pane
/// draws a byte covered by no span in the default ink.
pub type Highlight = fn(line: &str, spans: &mut Vec<(usize, usize, u32)>);

pub struct Editor {
    pub buf: Buffer,
    pub path: Option<String>,
    pub dirty: bool,
    /// First visible line and first visible column (cells).
    pub top: usize,
    pub left: usize,
    /// The find prompt while it is open.
    pub search: Option<String>,
    needle: String,
    pub msg: String,
    discard_armed: bool,
    /// SYNTAXCORE (LEDGER SR89): the highlighter seam — `None` until that core lands; nothing is linked here.
    pub highlight: Option<Highlight>,
    pub repaint: bool,
}

impl Editor {
    pub fn new() -> Editor {
        Editor {
            buf: Buffer::new(""),
            path: None,
            dirty: false,
            top: 0,
            left: 0,
            search: None,
            needle: String::new(),
            msg: String::from("Ctrl+1 tree, Enter opens a file"),
            discard_armed: false,
            highlight: None,
            repaint: true,
        }
    }

    /// (line, column in chars) of the primary caret.
    pub fn caret_lc(&self) -> (usize, usize) {
        let p = self.buf.offset_to_pos(self.buf.cursor());
        let l = self.buf.line(p.line);
        let col = l.get(..p.byte.min(l.len())).map_or(0, |s| s.chars().count());
        (p.line, col)
    }

    /// Scroll so the caret is inside `rows` x `cols`.
    pub fn view(&mut self, rows: usize, cols: usize) {
        let (l, c) = self.caret_lc();
        let (rows, cols) = (rows.max(1), cols.max(1));
        if l < self.top {
            self.top = l;
        } else if l >= self.top + rows {
            self.top = l + 1 - rows;
        }
        if c < self.left {
            self.left = c;
        } else if c >= self.left + cols {
            self.left = c + 1 - cols;
        }
    }

    pub fn open(&mut self, path: &str) {
        if self.dirty && !self.discard_armed {
            self.discard_armed = true;
            self.msg = String::from("unsaved: Ctrl+S, or Ctrl+O again discards");
            self.repaint = true;
            return;
        }
        self.discard_armed = false;
        let mut l = Line::new(b":: UNA: open path=");
        l.put(path.as_bytes());
        match read_file(path, EDIT_MAX, true) {
            Ok(bytes) => {
                let lossy = core::str::from_utf8(&bytes).is_err();
                let text = String::from_utf8_lossy(&bytes);
                self.buf = Buffer::new(&text);
                self.path = Some(String::from(path));
                self.dirty = false;
                self.top = 0;
                self.left = 0;
                self.search = None;
                self.msg = if lossy { String::from("opened (not UTF-8: shown lossy)") } else { String::from("opened") };
                l.put(b" bytes=").dec(bytes.len() as i64).put(b" lines=").dec(self.buf.line_count() as i64).put(b" rc=0 ::");
            }
            Err(e) => {
                self.msg = alloc::format!("open: {} {}", errno_word(e), e);
                l.put(b" rc=").dec(e).put(b" ::");
            }
        }
        l.wire();
        self.repaint = true;
    }

    pub fn save(&mut self) {
        let Some(path) = self.path.clone() else {
            self.msg = String::from("no file: open one from the tree (Ctrl+O)");
            self.repaint = true;
            return;
        };
        let text = self.buf.text().to_string_lossless();
        let mut l = Line::new(b":: UNA: save path=");
        l.put(path.as_bytes()).put(b" bytes=").dec(text.len() as i64);
        match write_file(&path, text.as_bytes()) {
            Ok(_) => {
                self.dirty = false;
                self.discard_armed = false;
                self.msg = String::from("saved");
                l.put(b" rc=0 ::");
            }
            Err(e) => {
                self.msg = alloc::format!("save: {} {}", errno_word(e), e);
                l.put(b" rc=").dec(e).put(b" ::");
            }
        }
        l.wire();
        self.repaint = true;
    }

    fn edited(&mut self, changed: bool) {
        if changed {
            self.dirty = true;
            self.discard_armed = false;
        }
    }

    fn find_next(&mut self) {
        if self.needle.is_empty() {
            self.msg = String::from("Ctrl+F to find");
            return;
        }
        let from = self.buf.selections().first().map_or(0, |s| s.end());
        match self.buf.find(&self.needle, SearchOpts::default(), from, Direction::Forward, true) {
            Some(r) => {
                self.buf.set_selections(&[Selection::new(r.start, r.end)]);
                let n = self.buf.find_all(&self.needle, SearchOpts::default()).len();
                self.msg = alloc::format!("found ({} in file)", n);
            }
            None => self.msg = alloc::format!("not found: {}", self.needle),
        }
    }

    /// A resolved desktop action (INPUT_EV_ACTION's discriminant, `video::clipboard::action_code`).
    pub fn action(&mut self, a: u8) {
        match a {
            6 => {
                let n = self.buf.len();
                self.buf.set_selections(&[Selection::new(0, n)]);
            }
            8 => self.buf.move_cursors(Motion::Left, true),
            9 => self.buf.move_cursors(Motion::Right, true),
            10 => self.buf.move_cursors(Motion::LineStart, true),
            11 => self.buf.move_cursors(Motion::LineEnd, true),
            12 => {
                let c = self.buf.cursor();
                self.buf.set_cursor(c);
            }
            13 => self.buf.move_cursors(Motion::Left, false),
            14 => self.buf.move_cursors(Motion::Right, false),
            15 => self.buf.move_cursors(Motion::LineStart, false),
            16 => self.buf.move_cursors(Motion::LineEnd, false),
            _ => return,
        }
        self.repaint = true;
    }

    /// Esc: close the find prompt. `false` = no prompt was open (the IDE then cycles focus).
    pub fn escape(&mut self) -> bool {
        if self.search.take().is_some() {
            self.msg = String::new();
            self.repaint = true;
            return true;
        }
        false
    }

    pub fn key(&mut self, k: u8) {
        self.repaint = true;
        if let Some(q) = self.search.as_mut() {
            match k {
                b'\n' | b'\r' => {
                    self.needle = core::mem::take(q);
                    self.search = None;
                    self.find_next();
                }
                0x08 | 0x7f => {
                    q.pop();
                }
                0x20..=0x7e => q.push(k as char),
                _ => {}
            }
            return;
        }
        match k {
            b'\n' | b'\r' => {
                let c = !self.buf.newline().is_empty();
                self.edited(c);
            }
            0x08 | 0x7f => {
                let c = !self.buf.delete(Unit::Grapheme, Direction::Backward).is_empty();
                self.edited(c);
            }
            0x09 => {
                let c = !self.buf.insert("    ").is_empty();
                self.edited(c);
            }
            KEY_LEFT => self.buf.move_cursors(Motion::Left, false),
            KEY_RIGHT => self.buf.move_cursors(Motion::Right, false),
            KEY_UP => self.buf.move_cursors(Motion::Up, false),
            KEY_DOWN => self.buf.move_cursors(Motion::Down, false),
            0x01 => self.buf.move_cursors(Motion::LineStart, false),
            0x05 => self.buf.move_cursors(Motion::LineEnd, false),
            0x0B => {
                let c = !self.buf.delete_lines().is_empty();
                self.edited(c);
            }
            0x1A => match self.buf.undo() {
                Some(_) => {
                    self.edited(true);
                    self.msg = String::from("undo");
                }
                None => self.msg = String::from("nothing to undo"),
            },
            0x19 => match self.buf.redo() {
                Some(_) => {
                    self.edited(true);
                    self.msg = String::from("redo");
                }
                None => self.msg = String::from("nothing to redo"),
            },
            0x06 => {
                self.search = Some(String::new());
                self.msg = String::new();
            }
            0x07 => self.find_next(),
            0x20..=0x7e => {
                let mut s = [0u8; 4];
                let c = !self.buf.insert((k as char).encode_utf8(&mut s)).is_empty();
                self.edited(c);
            }
            _ => self.repaint = false,
        }
    }
}

/// The document's text as one `String` (edit_core's `Text` is `Display`).
trait Whole {
    fn to_string_lossless(&self) -> String;
}
impl Whole for edit_core::Text {
    fn to_string_lossless(&self) -> String {
        let mut s = String::with_capacity(self.len());
        for c in self.chunks() {
            s.push_str(c);
        }
        s
    }
}
