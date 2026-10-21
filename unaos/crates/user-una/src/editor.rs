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
// as this program's principal under the volume's write ACL (a refusal is EACCES in the status). UNAIDE2 (LEDGER
// SR111): one Editor is one TAB (main.rs `tabs`); a document is never opened over another, so the old "Ctrl+O again
// discards" guard is gone and `close_armed` asks once before Ctrl+W drops unsaved edits.
//
// KEYS (focused). Printable ASCII inserts; Enter = `newline` (the core's auto-indent); Backspace = delete one
// grapheme back; Tab = four spaces; arrows = `move_cursors`; Ctrl+A / Ctrl+E line start / end; Ctrl+K delete the
// line; Ctrl+Z undo, Ctrl+Y redo; ⌘Z / ⌘⇧Z (INPUT_EV_ACTION 51/52). Selection rides the desktop's
// resolved actions (INPUT_EV_ACTION, APPCLIP/TERMSEL): select-all, select-left/right, select-line-start/end,
// deselect, and TERMSEL2's caret moves — the ring carries no modifier bits, so Shift+arrow arrives that way.
//
// THE PROMPT — UNAIDE2 (LEDGER SR111). Ctrl+F opens it on the FIND field, Ctrl+L on the REPLACE field (Ctrl+H, the
// usual chord, never reaches ring 3: the HID fold carves Ctrl-H/I/J/M out, drivers/xhci/mod.rs:237-238, because 0x08
// is Backspace), Ctrl+G on the LINE field. The needle and the replacement live on the tab, so Ctrl+F then Enter is
// "find next". In the prompt: printable edits the field, Backspace, Tab swaps find/replace, Ctrl+W toggles
// whole-word (edit_core's word-edge rule), Esc closes it. Regex is off — edit_core's search is literal and
// case-sensitive. Enter on FIND selects the next match (wrapping); Enter on REPLACE replaces the selection when it
// IS a match (one undo group) and selects the next; Ctrl+A replaces all (one `set_text` edit, one undo group);
// Enter on LINE goes to `L` or `L:C` (1-based, clamped) and closes. Keys the prompt does not take fall through to
// the IDE (`wants`). Wire: `:: UNA: find needle=<s> word=<0|1> hits=<k> at=<l>:<c> ::`, `:: UNA: replace
// needle=<s> with=<s> word=<0|1> n=<k> ::`, `:: UNA: goto line=<l> of=<n> ::`.
//
// HIGHLIGHT — UNAMETAL2 (LEDGER SR98) fills SYNTAXCORE's (SR89) seam with the core itself: `highlight` holds the
// grammar (`syntax_core::language_for_id`, chosen from the file's extension by Tabula's table) and the incremental
// `Highlighter`. Open lexes the document through a `Source` over edit_core's Buffer (one line at a time, never the
// document as one String); every edit's `Change`s are spliced and re-lexed (Tabula's `TabulaDocument::sync_syntax`
// shape), so a keystroke re-lexes one line in the common case. A character's KIND ([`Syntax::kind_at`]) goes through
// the theme map (`map`, `syntax_core::Theme` — UNAKEYS (rmbp-ledger B552): Principia's `tabula.theme.<kind>` applied,
// read by `theme::syntax_map`) to an APPEARANCE TOKEN, and the painter resolves the token through the live look
// (`theme::Theme::token_ink`: Light/Dark and the chosen accent) — no colour belongs to a kind, and none is spelled
// here. A file with no grammar draws in the one ink.

use crate::fs::{errno_word, read_file, write_file, EDIT_MAX};
use crate::Line;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use edit_core::{Buffer, Change, Direction, Motion, Pos, SearchOpts, Selection, Unit};
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

/// The document's grammar and its per-line checkpoints and tokens.
pub struct Syntax {
    lang: Box<dyn Language>,
    hl: Highlighter,
}

impl Syntax {
    /// The grammar for `path`'s language, with `buf` lexed whole; `None` = no grammar (plain text).
    fn for_path(path: &str, buf: &Buffer) -> Option<Syntax> {
        let lang = syntax_core::language_for_id(language_for(path))?;
        let mut hl = Highlighter::new();
        hl.reset(lang.as_ref(), &BufSource(buf));
        Some(Syntax { lang, hl })
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
    /// The kind of byte `byte` of `line`: its token's, else `Identifier` (the content ink through the shipped map).
    pub fn kind_at(&self, line: usize, byte: usize) -> HighlightKind {
        let k = self.hl.line_tokens(line).iter().find(|t| byte >= t.start as usize && byte < t.end as usize).map(|t| t.kind);
        k.unwrap_or(HighlightKind::Identifier)
    }
}

/// UNAIDE2: the prompt's fields.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Find,
    Repl,
    Line,
}

pub struct Editor {
    pub buf: Buffer,
    pub path: Option<String>,
    pub dirty: bool,
    /// First visible line and first visible column (cells).
    pub top: usize,
    pub left: usize,
    /// UNAIDE2: the prompt's open field (`None` = closed), the needle, the replacement, whole-word, the line field.
    pub prompt: Option<Field>,
    pub needle: String,
    pub repl: String,
    pub whole: bool,
    goto: String,
    pub msg: String,
    /// UNAIDE2: Ctrl+W met unsaved edits once; the next Ctrl+W discards (cleared by any edit).
    pub close_armed: bool,
    /// UNAMETAL2: the open document's syntax core (`None` = a language with no grammar, or no document).
    pub highlight: Option<Syntax>,
    /// UNAKEYS (B552): kind -> appearance token, Principia's `tabula.theme.<kind>` applied (`theme::syntax_map`).
    pub map: Theme,
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
            prompt: None,
            needle: String::new(),
            repl: String::new(),
            whole: false,
            goto: String::new(),
            msg: String::from("Ctrl+1 tree, Enter opens a file"),
            close_armed: false,
            highlight: None,
            map: Theme::default(),
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

    /// Load `path` into this tab (the IDE opens into a blank or a new tab). `true` = loaded.
    pub fn open(&mut self, path: &str) -> bool {
        let mut ok = false;
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
                self.prompt = None;
                self.close_armed = false;
                ok = true;
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
        ok
    }

    /// True for a tab holding nothing: no file, no edits, no text (the IDE loads into it in place).
    pub fn blank(&self) -> bool {
        self.path.is_none() && !self.dirty && self.buf.len() == 0
    }

    /// The tab's label: the file's leaf, or `untitled`.
    pub fn name(&self) -> &str {
        self.path.as_deref().map_or("untitled", |p| p.rsplit('/').next().unwrap_or(p))
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
                self.close_armed = false;
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
        self.close_armed = false;
        if let Some(h) = self.highlight.as_mut() {
            h.sync(&self.buf, &changes);
        }
    }

    fn opts(&self) -> SearchOpts {
        SearchOpts { case_insensitive: false, whole_word: self.whole }
    }

    /// (line, column in chars), 1-based, of byte offset `off`.
    fn lc1(&self, off: usize) -> (usize, usize) {
        let p = self.buf.offset_to_pos(off);
        let l = self.buf.line(p.line);
        (p.line + 1, l.get(..p.byte.min(l.len())).map_or(0, |s| s.chars().count()) + 1)
    }

    /// The next match after the selection (wrapping), selected. `:: UNA: find … ::`.
    fn find_next(&mut self) {
        if self.needle.is_empty() {
            self.msg = String::from("type what to find");
            return;
        }
        let opts = self.opts();
        let from = self.buf.selections().first().map_or(0, |s| s.end());
        let n = self.buf.find_all(&self.needle, opts).len();
        let hit = self.buf.find(&self.needle, opts, from, Direction::Forward, true);
        let (al, ac) = match &hit {
            Some(r) => {
                self.buf.set_selections(&[Selection::new(r.start, r.end)]);
                self.msg = alloc::format!("found ({} in file)", n);
                self.lc1(r.start)
            }
            None => {
                self.msg = alloc::format!("not found: {}", self.needle);
                (0, 0)
            }
        };
        let mut l = Line::new(b":: UNA: find needle=");
        l.put(self.needle.as_bytes()).put(b" word=").dec(self.whole as i64).put(b" hits=").dec(n as i64);
        l.put(b" at=").dec(al as i64).put(b":").dec(ac as i64).put(b" ::");
        l.wire();
    }

    fn replace_wire(&self, n: usize) {
        let mut l = Line::new(b":: UNA: replace needle=");
        l.put(self.needle.as_bytes()).put(b" with=").put(self.repl.as_bytes()).put(b" word=").dec(self.whole as i64);
        l.put(b" n=").dec(n as i64).put(b" ::");
        l.wire();
    }

    /// Enter on the replace field: the selection, when it IS a match, becomes the replacement (one undo group);
    /// then the next match is selected.
    fn replace_one(&mut self) {
        if self.needle.is_empty() {
            self.msg = String::from("type what to find (Tab)");
            return;
        }
        let sel = self.buf.selections().first().map(|s| s.range());
        let all = self.buf.find_all(&self.needle, self.opts());
        let mut n = 0;
        if let Some(r) = sel.filter(|r| all.contains(r)) {
            let at = r.start;
            let c = self.buf.replace(r, &self.repl);
            self.edited(c);
            self.buf.set_cursor(at + self.repl.len());
            n = 1;
        }
        self.replace_wire(n);
        self.find_next();
        if n == 1 {
            self.msg = alloc::format!("replaced 1; {}", self.msg);
        }
    }

    /// Ctrl+A on the prompt: every match replaced in ONE edit (`set_text`, one undo group).
    fn replace_all(&mut self) {
        if self.needle.is_empty() {
            self.msg = String::from("type what to find");
            return;
        }
        let all = self.buf.find_all(&self.needle, self.opts());
        if !all.is_empty() {
            let old = self.buf.text().to_string_lossless();
            let mut new = String::with_capacity(old.len());
            let mut at = 0;
            for r in &all {
                new.push_str(&old[at..r.start]);
                new.push_str(&self.repl);
                at = r.end;
            }
            new.push_str(&old[at..]);
            let caret = self.buf.cursor().min(new.len());
            let c = self.buf.set_text(&new);
            self.edited(c);
            let caret = if new.is_char_boundary(caret) { caret } else { 0 };
            self.buf.set_cursor(caret);
        }
        self.msg = alloc::format!("replaced {}", all.len());
        self.replace_wire(all.len());
    }

    /// Enter on the line field: the caret to `L` or `L:C` (1-based, clamped). `:: UNA: goto … ::`.
    fn goto_line(&mut self) {
        let mut it = self.goto.split(':');
        let l: usize = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let c: usize = it.next().and_then(|v| v.parse().ok()).unwrap_or(1);
        let n = self.buf.line_count();
        if l == 0 {
            self.msg = String::from("line: a number, or line:col");
            return;
        }
        let l = l.min(n.max(1));
        self.click(l - 1, c.max(1) - 1);
        self.msg = alloc::format!("line {} of {}", l, n);
        let mut w = Line::new(b":: UNA: goto line=");
        w.dec(l as i64).put(b" of=").dec(n as i64).put(b" ::");
        w.wire();
    }

    /// The caret to `line`, `col` chars in (0-based, clamped) — Ctrl+G and a press in the text (B543's pointer).
    pub fn click(&mut self, line: usize, col: usize) {
        let n = self.buf.line_count().max(1);
        let line = line.min(n - 1);
        let text = self.buf.line(line);
        let byte = text.char_indices().nth(col).map_or(text.len(), |(b, _)| b);
        let off = self.buf.pos_to_offset(Pos::new(line, byte));
        self.buf.set_cursor(off);
        self.repaint = true;
    }

    /// Open the prompt on `f` (Ctrl+F / Ctrl+L / Ctrl+G).
    pub fn ask(&mut self, f: Field) {
        if f == Field::Line {
            self.goto.clear();
        }
        self.prompt = Some(f);
        self.msg = String::new();
        self.repaint = true;
    }

    /// Does the open prompt take key `k`? (Everything else falls through to the IDE's keys.)
    pub fn wants(&self, k: u8) -> bool {
        self.prompt.is_some() && matches!(k, 0x20..=0x7e | 0x08 | 0x7f | b'\n' | b'\r' | 0x09 | 0x17 | 0x01)
    }

    /// The status line's prompt text while it is open.
    pub fn prompt_text(&self) -> Option<String> {
        let w = if self.whole { "[w]" } else { "" };
        Some(match self.prompt? {
            Field::Find => alloc::format!("find{}: {}_  repl: {}", w, self.needle, self.repl),
            Field::Repl => alloc::format!("repl{}: {} -> {}_", w, self.needle, self.repl),
            Field::Line => alloc::format!("line: {}_", self.goto),
        })
    }

    fn prompt_key(&mut self, f: Field, k: u8) {
        match k {
            b'\n' | b'\r' => match f {
                Field::Find => self.find_next(),
                Field::Repl => self.replace_one(),
                Field::Line => {
                    self.goto_line();
                    self.prompt = None;
                }
            },
            0x09 => {
                self.prompt = Some(match f {
                    Field::Find => Field::Repl,
                    Field::Repl => Field::Find,
                    Field::Line => Field::Line,
                })
            }
            0x17 if f != Field::Line => self.whole = !self.whole,
            0x01 if f != Field::Line => self.replace_all(),
            0x08 | 0x7f => {
                match f {
                    Field::Find => self.needle.pop(),
                    Field::Repl => self.repl.pop(),
                    Field::Line => self.goto.pop(),
                };
            }
            0x20..=0x7e => match f {
                Field::Find => self.needle.push(k as char),
                Field::Repl => self.repl.push(k as char),
                Field::Line => {
                    if k.is_ascii_digit() || k == b':' {
                        self.goto.push(k as char)
                    }
                }
            },
            _ => {}
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
            51 => self.history(false, 51),
            52 => self.history(true, 52),
            _ => return,
        }
        self.repaint = true;
    }

    /// Esc: close the prompt. `false` = no prompt was open (the IDE then cycles focus).
    pub fn escape(&mut self) -> bool {
        if self.prompt.take().is_some() {
            self.msg = String::new();
            self.repaint = true;
            return true;
        }
        false
    }

    pub fn key(&mut self, k: u8) {
        self.repaint = true;
        if let Some(f) = self.prompt {
            if self.wants(k) {
                return self.prompt_key(f, k);
            }
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
            0x1A => self.history(false, 0x1A),
            0x19 => self.history(true, 0x19),
            0x06 => self.ask(Field::Find),
            0x0C => self.ask(Field::Repl),
            0x07 => self.ask(Field::Line),
            0x20..=0x7e => {
                let mut s = [0u8; 4];
                let c = self.buf.insert((k as char).encode_utf8(&mut s));
                self.edited(c);
            }
            _ => self.repaint = false,
        }
    }

    /// CHARTER: RING3UNDO (rmbp-ledger B771) — one undo/redo step of the core's journal, from a Ctrl byte or a keymap
    /// action; status, dirty and one wire line: `:: UNA: <undo|redo> code=<code> -> <ok|empty> ::`.
    fn history(&mut self, redo: bool, code: u8) {
        let (step, word) = if redo { (self.buf.redo(), "redo") } else { (self.buf.undo(), "undo") };
        let hit = step.is_some();
        match step {
            Some(c) => {
                self.edited(c);
                self.dirty = true;
                self.msg = String::from(word);
            }
            None => self.msg = alloc::format!("nothing to {}", word),
        }
        let mut l = Line::new(b":: UNA: ");
        l.put(word.as_bytes()).put(b" code=").dec(code as i64).put(b" -> ").put(if hit { b"ok" as &[u8] } else { b"empty" }).put(b" ::");
        l.wire();
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
