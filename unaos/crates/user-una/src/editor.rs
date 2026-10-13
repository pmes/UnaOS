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
// `Buffer` text -> SYS_PATH_WRITE (truncate, then chunks) — UNAMETAL2 (LEDGER SR98): on the default x86 kernel too,
// as this program's principal under the volume's write ACL (a refusal is EACCES in the status). Opening over unsaved
// edits is refused once ("Ctrl+O again discards").
//
// KEYS (focused). Printable ASCII inserts; Enter = `newline` (the core's auto-indent); Backspace = delete one
// grapheme back; Tab = four spaces; arrows = `move_cursors`; Ctrl+A / Ctrl+E line start / end; Ctrl+K delete the
// line; Ctrl+Z undo, Ctrl+Y redo; Ctrl+F a find prompt in the status line (Enter finds forward from the caret,
// wrapping, and selects the match; Esc leaves it), Ctrl+G finds the next match. Selection rides the desktop's
// resolved actions (INPUT_EV_ACTION, APPCLIP/TERMSEL): select-all, select-left/right, select-line-start/end,
// deselect, and TERMSEL2's caret moves — the ring carries no modifier bits, so Shift+arrow arrives that way.
//
// HIGHLIGHT — UNAMETAL2 (LEDGER SR98) fills SYNTAXCORE's (SR89) seam with the core itself: `highlight` holds the
// grammar (`syntax_core::language_for_id`, chosen from the file's extension by Tabula's table), the incremental
// `Highlighter` and the `Theme`. Open lexes the document through a `Source` over edit_core's Buffer (one line at a
// time, never the document as one String); every edit's `Change`s are spliced and re-lexed (Tabula's
// `TabulaDocument::sync_syntax` shape), so a keystroke re-lexes one line in the common case. A character's ink is the
// APPEARANCE TOKEN the theme map names for its kind (`Theme::token`), resolved by [`token_ink`] through the painter's
// DARK set (kernel `video/theme.rs`; this program paints a dark ground) — no colour belongs to a kind. Owed: ring 3
// reads neither the live theme (Light/Dark, the chosen accent) nor Principia's `tabula.theme.<kind>`; the default
// theme and the default accent stand until a syscall carries them. A file with no grammar draws in the one ink.

use crate::fs::{errno_word, read_file, write_file, EDIT_MAX};
use crate::Line;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use edit_core::{Buffer, Change, Direction, Motion, SearchOpts, Selection, Unit};
use syntax_core::{HighlightKind, Highlighter, Language, Source, Theme};
use una_abi::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};

/// The language id for a path's extension — `TabulaDocument::language_for` (handlers/tabula/src/document.rs), the
/// ids syntax_core has grammars for; anything else is plain text.
fn language_for(path: &str) -> &'static str {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    match leaf.rsplit_once('.').map(|(_, e)| e) {
        Some("rs") => "rust",
        Some("toml") => "toml",
        Some("md") => "markdown",
        Some("c") | Some("h") | Some("cpp") => "c",
        Some("sh") | Some("bash") => "sh",
        _ => "txt",
    }
}

/// An appearance token's ink (`0xAARRGGBB`): the DARK rows of kernel `video/theme.rs` for the tokens
/// `syntax_core::APPEARANCE_TOKENS` may name, `Accent` = the default accent (`ACCENTS[0]`). An unknown name is the
/// content ink.
pub fn token_ink(token: &str) -> u32 {
    0xFF00_0000
        | match token {
            "SyntaxCode" => 0x006C_C88A,
            "SyntaxNum" => 0x00E3_9A55,
            "SyntaxLit" => 0x00C3_8BE0,
            "ErrorText" => 0x00E5_7373,
            "TitleTextInactive" => 0x008E_8E94,
            "Accent" => 0x004A_73AA,
            _ => 0x00E6_E4DF, // ContentText
        }
}

/// edit_core's Buffer as syntax_core reads text: lines, never the whole document.
struct BufSource<'a>(&'a Buffer);
impl Source for BufSource<'_> {
    fn line_count(&self) -> usize {
        self.0.line_count()
    }
    fn line_start(&self, line: usize) -> usize {
        self.0.text().line_start(line)
    }
    fn read_line(&self, line: usize, out: &mut String) {
        out.push_str(&self.0.line(line));
    }
}

/// The document's grammar, its per-line checkpoints and tokens, and the theme map.
pub struct Syntax {
    lang: Box<dyn Language>,
    hl: Highlighter,
    theme: Theme,
}

impl Syntax {
    /// The grammar for `path`'s language, with `buf` lexed whole; `None` = no grammar (plain text).
    fn for_path(path: &str, buf: &Buffer) -> Option<Syntax> {
        let lang = syntax_core::language_for_id(language_for(path))?;
        let mut hl = Highlighter::new();
        hl.reset(lang.as_ref(), &BufSource(buf));
        Some(Syntax { lang, hl, theme: Theme::default() })
    }
    /// Splice an edit's changed line ranges into the checkpoints and re-lex from the first touched line.
    fn sync(&mut self, buf: &Buffer, changes: &[Change]) {
        for c in changes {
            self.hl.splice(c.first_line, c.old_last_line, c.new_last_line);
        }
        self.hl.relex(self.lang.as_ref(), &BufSource(buf));
    }
    pub fn id(&self) -> &'static str {
        self.lang.id()
    }
    /// Every token of the document, as last lexed (the witness's `spans`).
    pub fn spans(&self) -> usize {
        (0..self.hl.line_count()).map(|l| self.hl.line_tokens(l).len()).sum()
    }
    /// The ink of byte `byte` of `line`: its token's kind through the theme map, else the content ink.
    pub fn ink_at(&self, line: usize, byte: usize) -> u32 {
        let k = self.hl.line_tokens(line).iter().find(|t| byte >= t.start as usize && byte < t.end as usize).map(|t| t.kind);
        token_ink(self.theme.token(k.unwrap_or(HighlightKind::Identifier)))
    }
}

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
    /// UNAMETAL2: the open document's syntax core (`None` = a language with no grammar, or no document).
    pub highlight: Option<Syntax>,
    /// UNAMETAL2: the last Ctrl+S answered rc=0 (the exit status's SAVED bit).
    pub saved: bool,
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
            saved: false,
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
                self.highlight = Syntax::for_path(path, &self.buf);
                self.path = Some(String::from(path));
                self.dirty = false;
                self.top = 0;
                self.left = 0;
                self.search = None;
                self.msg = if lossy { String::from("opened (not UTF-8: shown lossy)") } else { String::from("opened") };
                l.put(b" bytes=").dec(bytes.len() as i64).put(b" lines=").dec(self.buf.line_count() as i64);
                l.put(b" lang=").put(self.highlight.as_ref().map_or("none", |h| h.id()).as_bytes());
                l.put(b" spans=").dec(self.highlight.as_ref().map_or(0, |h| h.spans()) as i64).put(b" rc=0 ::");
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
                self.saved = true;
                self.msg = String::from("saved");
                l.put(b" rc=0 ::");
            }
            Err(e) => {
                self.saved = false;
                self.msg = alloc::format!("save: {} {}", errno_word(e), e);
                l.put(b" rc=").dec(e).put(b" ::");
            }
        }
        l.wire();
        self.repaint = true;
    }

    /// An edit's changes: dirty the document and re-lex what they touched.
    fn edited(&mut self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        self.dirty = true;
        self.discard_armed = false;
        if let Some(h) = self.highlight.as_mut() {
            h.sync(&self.buf, &changes);
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
                let c = self.buf.newline();
                self.edited(c);
            }
            0x08 | 0x7f => {
                let c = self.buf.delete(Unit::Grapheme, Direction::Backward);
                self.edited(c);
            }
            0x09 => {
                let c = self.buf.insert("    ");
                self.edited(c);
            }
            KEY_LEFT => self.buf.move_cursors(Motion::Left, false),
            KEY_RIGHT => self.buf.move_cursors(Motion::Right, false),
            KEY_UP => self.buf.move_cursors(Motion::Up, false),
            KEY_DOWN => self.buf.move_cursors(Motion::Down, false),
            0x01 => self.buf.move_cursors(Motion::LineStart, false),
            0x05 => self.buf.move_cursors(Motion::LineEnd, false),
            0x0B => {
                let c = self.buf.delete_lines();
                self.edited(c);
            }
            0x1A => match self.buf.undo() {
                Some(c) => {
                    self.edited(c);
                    self.dirty = true;
                    self.msg = String::from("undo");
                }
                None => self.msg = String::from("nothing to undo"),
            },
            0x19 => match self.buf.redo() {
                Some(c) => {
                    self.edited(c);
                    self.dirty = true;
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
                let c = self.buf.insert((k as char).encode_utf8(&mut s));
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
