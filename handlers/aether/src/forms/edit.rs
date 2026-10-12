//! The text-editing model shared by `<input>` and `<textarea>` (AETHERINPUT, LEDGER SR68).
//!
//! One caret model for every text control: a caret and an anchor, both BYTE offsets into the control's
//! value that always sit on grapheme-cluster boundaries (UAX #29 through `font_core::grapheme` — the
//! boundaries `text_host::line::Line::caret_x`/`hit` place a caret on). The selection is the range
//! between them; it is empty when they meet.
//!
//! Key behaviour is Chromium's on Linux (`EditingBehavior` for `kEditingUnixBehavior`):
//! - Left/Right move one grapheme, collapsing a selection to its start/end first;
//!   Ctrl or Alt + Left/Right move by word (to a word's start going left, to its END going right —
//!   Unix does not skip the following space, Windows does).
//! - Home/End go to the start/end of the line (the value, in a single-line field; the logical line in a
//!   textarea), Ctrl+Home/End to the start/end of the value; Up/Down change logical line in a textarea.
//! - Shift with any move extends the selection from the anchor; Ctrl+A selects the whole value.
//! - Backspace deletes backward by Blink's BackspaceStateMachine rule: one CODE POINT, except that an
//!   emoji sequence (ZWJ, modifier, flag, keycap, variation selector) goes whole; Delete removes the
//!   next whole grapheme cluster. Ctrl+Backspace/Delete remove a word. A selection is deleted first.
//! - Return inserts a line feed in a textarea and submits a single-line field.
//!
//! Pure: no DOM, no fonts — the engine feeds the value in and writes it back (the dirty value).

use font_core::grapheme;

/// The caret and the selection anchor of the focused text control, as byte offsets into its value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditState {
    pub caret: usize,
    pub anchor: usize,
}

/// What a key did to the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not an editing key: the engine may use it (page scrolling never runs while a field holds focus).
    Unhandled,
    /// Consumed with no visible change (Up in a single-line field).
    Consumed,
    /// The caret or selection moved; the value is unchanged.
    Moved,
    /// The value changed.
    Edited,
    /// Return in a single-line field: implicit submission (HTML §4.10.21.2).
    Submit,
}

/// Modifier state of a key string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

/// The keys the editing model and the page scroller know, by GDK keysym or DOM `KeyboardEvent.key` name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Return,
    Space,
    Tab,
    Escape,
    /// A single printable character pressed with a modifier (`Ctrl+a`).
    Char(char),
}

/// Splits a key string from the shell into its modifiers and key. The shell sends GDK keysym names
/// (`Left`, `BackSpace`, `Page_Down`, `space`) — or DOM key names (`ArrowLeft`, `Backspace`, `PageDown`,
/// `" "`) — prefixed by `Ctrl+`, `Alt+`, `Shift+`, `Meta+` in any order (`Ctrl+Shift+Left`).
pub fn parse_key(s: &str) -> (Mods, Option<Key>) {
    let mut mods = Mods::default();
    let mut rest = s;
    loop {
        let Some(i) = rest.find('+') else { break };
        if i == 0 || i + 1 >= rest.len() {
            break; // "+" itself, or a trailing '+': the key is the plus sign
        }
        match rest[..i].to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "primary" => mods.ctrl = true,
            "alt" | "option" | "mod1" => mods.alt = true,
            "shift" => mods.shift = true,
            "meta" | "super" | "cmd" | "command" => mods.meta = true,
            _ => break,
        }
        rest = &rest[i + 1..];
    }
    let key = match rest {
        "Left" | "ArrowLeft" | "KP_Left" => Key::Left,
        "Right" | "ArrowRight" | "KP_Right" => Key::Right,
        "Up" | "ArrowUp" | "KP_Up" => Key::Up,
        "Down" | "ArrowDown" | "KP_Down" => Key::Down,
        "Home" | "KP_Home" => Key::Home,
        "End" | "KP_End" => Key::End,
        "Page_Up" | "PageUp" | "Prior" | "KP_Page_Up" | "KP_Prior" => Key::PageUp,
        "Page_Down" | "PageDown" | "Next" | "KP_Page_Down" | "KP_Next" => Key::PageDown,
        "BackSpace" | "Backspace" => Key::Backspace,
        "Delete" | "KP_Delete" | "Del" => Key::Delete,
        "Return" | "Enter" | "KP_Enter" | "ISO_Enter" => Key::Return,
        "space" | "Space" | " " | "KP_Space" => Key::Space,
        "Tab" | "ISO_Left_Tab" | "KP_Tab" => Key::Tab,
        "Escape" | "Esc" => Key::Escape,
        _ => {
            let mut it = rest.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Key::Char(c),
                _ => return (mods, None),
            }
        }
    };
    (mods, Some(key))
}

/// The grapheme boundary before byte `i` (0 at the start).
pub fn prev_boundary(text: &str, i: usize) -> usize {
    let i = i.min(text.len());
    grapheme::clusters(text).into_iter().map(|(a, _)| a).filter(|&a| a < i).last().unwrap_or(0)
}

/// The grapheme boundary after byte `i` (the end at the end).
pub fn next_boundary(text: &str, i: usize) -> usize {
    grapheme::clusters(text).into_iter().map(|(_, b)| b).find(|&b| b > i).unwrap_or(text.len())
}

/// `i` snapped down onto a grapheme boundary.
pub fn snap(text: &str, i: usize) -> usize {
    let i = i.min(text.len());
    if i == text.len() {
        return i;
    }
    grapheme::clusters(text).into_iter().map(|(a, _)| a).take_while(|&a| a <= i).last().unwrap_or(0)
}

/// A word character for caret movement: letters, digits, combining marks and the connector `_`
/// (UAX #29's ALetter/Numeric/Extend/ExtendNumLet, approximated by Unicode general category).
fn is_word_char(c: char) -> bool {
    use font_core::ucd::Gcb;
    c.is_alphanumeric() || c == '_' || matches!(grapheme::grapheme_break_property(c), Gcb::Extend | Gcb::SpacingMark | Gcb::ZWJ)
}

/// The start of the word at or before `i` (Ctrl+Left): skip separators backward, then the word.
pub fn word_left(text: &str, i: usize) -> usize {
    let mut j = i.min(text.len());
    let before = |j: usize| text[..j].chars().next_back();
    while let Some(c) = before(j) {
        if is_word_char(c) {
            break;
        }
        j -= c.len_utf8();
    }
    while let Some(c) = before(j) {
        if !is_word_char(c) {
            break;
        }
        j -= c.len_utf8();
    }
    snap(text, j)
}

/// The end of the word at or after `i` (Ctrl+Right on Unix): skip separators forward, then the word.
pub fn word_right(text: &str, i: usize) -> usize {
    let mut j = i.min(text.len());
    let after = |j: usize| text[j..].chars().next();
    while let Some(c) = after(j) {
        if is_word_char(c) {
            break;
        }
        j += c.len_utf8();
    }
    while let Some(c) = after(j) {
        if !is_word_char(c) {
            break;
        }
        j += c.len_utf8();
    }
    let s = snap(text, j);
    if s < j { next_boundary(text, s) } else { s }
}

/// The start of the logical line holding byte `i`.
pub fn line_start(text: &str, i: usize) -> usize {
    text[..i.min(text.len())].rfind('\n').map(|p| p + 1).unwrap_or(0)
}

/// The end (before its line feed) of the logical line holding byte `i`.
pub fn line_end(text: &str, i: usize) -> usize {
    let i = i.min(text.len());
    text[i..].find('\n').map(|p| i + p).unwrap_or(text.len())
}

/// The byte range Backspace deletes before `i`: Blink's BackspaceStateMachine — one code point, but an
/// emoji sequence, flag, keycap or variation sequence (the clusters a single code point would break)
/// goes as a whole grapheme cluster.
pub fn backspace_range(text: &str, i: usize) -> (usize, usize) {
    let i = i.min(text.len());
    let start = prev_boundary(text, i);
    let cluster = &text[start..i];
    let whole = cluster.chars().any(|c| {
        grapheme::is_extended_pictographic(c)
            || ('\u{1F1E6}'..='\u{1F1FF}').contains(&c) // regional indicators (flags)
            || c == '\u{20E3}' // combining enclosing keycap
            || ('\u{FE00}'..='\u{FE0F}').contains(&c) // variation selectors
            || ('\u{E0100}'..='\u{E01EF}').contains(&c)
            || c == '\u{200D}'
            || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c) // emoji modifiers
            || ('\u{E0020}'..='\u{E007F}').contains(&c) // tag sequences
    }) || cluster == "\r\n";
    if whole {
        return (start, i);
    }
    let last = cluster.chars().next_back().map(char::len_utf8).unwrap_or(0);
    (i - last, i)
}

/// Grapheme column of byte `i` within its logical line.
fn column(text: &str, i: usize) -> usize {
    let s = line_start(text, i);
    grapheme::clusters(&text[s..i]).len()
}

/// The byte at grapheme column `col` of the logical line starting at `s` (clamped to its end).
fn at_column(text: &str, s: usize, col: usize) -> usize {
    let e = line_end(text, s);
    grapheme::clusters(&text[s..e]).get(col).map(|&(a, _)| s + a).unwrap_or(e)
}

impl EditState {
    /// A collapsed caret at byte `i`.
    pub fn at(i: usize) -> Self {
        EditState { caret: i, anchor: i }
    }

    /// The selection as an ordered (start, end) byte range.
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    /// Re-seats caret and anchor on boundaries of `text` (a script may have rewritten the value).
    pub fn clamp(&mut self, text: &str) {
        self.caret = snap(text, self.caret);
        self.anchor = snap(text, self.anchor);
    }

    /// Replaces the selection (or inserts at the caret) with `s`; the caret lands after it.
    pub fn insert(&mut self, text: &mut String, s: &str) {
        self.clamp(text);
        let (a, b) = self.selection();
        text.replace_range(a..b, s);
        *self = EditState::at(a + s.len());
    }

    /// Deletes the selection. False when there was none.
    fn delete_selection(&mut self, text: &mut String) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (a, b) = self.selection();
        text.replace_range(a..b, "");
        *self = EditState::at(a);
        true
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
    }

    /// Applies one key to the field. `multiline` is a textarea.
    pub fn key(&mut self, text: &mut String, key: &str, multiline: bool) -> Outcome {
        let (m, Some(k)) = parse_key(key) else { return Outcome::Unhandled };
        self.clamp(text);
        let word = m.ctrl || m.alt;
        let before = *self;
        match k {
            Key::Left | Key::Right => {
                let left = k == Key::Left;
                let to = if self.has_selection() && !m.shift && !word {
                    let (a, b) = self.selection();
                    if left { a } else { b }
                } else if word {
                    if left { word_left(text, self.caret) } else { word_right(text, self.caret) }
                } else if left {
                    prev_boundary(text, self.caret)
                } else {
                    next_boundary(text, self.caret)
                };
                self.move_to(to, m.shift);
            }
            Key::Home | Key::End => {
                let home = k == Key::Home;
                let to = match (multiline && !m.ctrl, home) {
                    (true, true) => line_start(text, self.caret),
                    (true, false) => line_end(text, self.caret),
                    (false, true) => 0,
                    (false, false) => text.len(),
                };
                self.move_to(to, m.shift);
            }
            Key::Up | Key::Down if multiline => {
                let col = column(text, self.caret);
                let s = line_start(text, self.caret);
                let to = if k == Key::Up {
                    if s == 0 { 0 } else { at_column(text, line_start(text, s - 1), col) }
                } else {
                    let e = line_end(text, self.caret);
                    if e == text.len() { text.len() } else { at_column(text, e + 1, col) }
                };
                self.move_to(to, m.shift);
            }
            Key::Up | Key::Down => return Outcome::Consumed,
            Key::Char('a') | Key::Char('A') if m.ctrl || m.meta => {
                self.anchor = 0;
                self.caret = text.len();
            }
            Key::Backspace => {
                if !self.delete_selection(text) {
                    if self.caret == 0 {
                        return Outcome::Consumed;
                    }
                    let (a, b) = if word { (word_left(text, self.caret), self.caret) } else { backspace_range(text, self.caret) };
                    text.replace_range(a..b, "");
                    *self = EditState::at(a);
                }
                return Outcome::Edited;
            }
            Key::Delete => {
                if !self.delete_selection(text) {
                    if self.caret >= text.len() {
                        return Outcome::Consumed;
                    }
                    let b = if word { word_right(text, self.caret) } else { next_boundary(text, self.caret) };
                    text.replace_range(self.caret..b, "");
                }
                return Outcome::Edited;
            }
            Key::Return if multiline => {
                self.insert(text, "\n");
                return Outcome::Edited;
            }
            Key::Return => return Outcome::Submit,
            // Printable keys arrive as Text; inside a field Space and Page keys are the field's
            // (they never scroll the page under a focused field).
            Key::Space | Key::PageUp | Key::PageDown | Key::Char(_) => return Outcome::Consumed,
            Key::Tab | Key::Escape => return Outcome::Unhandled,
        }
        if *self == before { Outcome::Consumed } else { Outcome::Moved }
    }
}

/// The value a typed or pasted string inserts: a single-line field strips line breaks (HTML §4.10.5.1.2
/// "strip newlines"); a textarea normalizes CRLF and CR to LF (its API value, §4.10.11).
pub fn sanitize_insert(s: &str, multiline: bool) -> String {
    if multiline {
        s.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        s.chars().filter(|&c| c != '\n' && c != '\r').collect()
    }
}
