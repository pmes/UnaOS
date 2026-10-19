//! CHARTER: Kernel — driver (the HID input path's own data: scancode -> character per keyboard layout; the layout and the Compose switch are Principia's `system.keyboard.*`, rmbp-ledger B704)
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

//! KEYMAPS (rmbp-ledger B704) — keyboard LAYOUTS as data, the dead-key and Compose state machine.
//!
//! Research C §10 (BP-bar-17): a keyboard layout and dead keys are the first thing a non-US user needs and the
//! first thing that breaks. Before this file a key became a character in ONE place, `drivers/xhci`'s
//! `hid_key_ascii`, and that place knew one layout (US) and one byte per key.
//!
//! # The shape
//!
//! A [`Layout`] is a name and a const table of [`K`] rows: a HID usage and the character it types BARE, with
//! SHIFT and with ALTGR (Right Alt). `'\0'` is "no character". [`US`] is the whole printable block; every other
//! layout lists FULL rows for the keys that differ and is looked up first, with US behind it. A DEAD key is a row
//! whose cell is a COMBINING mark ([`D_ACUTE`] .. [`D_TILDE`]) — the table stays pure data and the state machine
//! knows a dead key by its character, never by a flag beside it.
//!
//! # The rules ([`step`], [`report`])
//!
//!  * Ctrl, GUI or LEFT Alt held: not the layout's key (`None`) — chords and the Ctrl folds stay on the existing
//!    path, so R61's `Ctrl-C` = 0x03 is untouched.
//!  * Right Alt held: the AltGr column; a key with no AltGr cell is not claimed (US keeps today's suppression).
//!  * Caps Lock flips Shift on LETTERS only: the bare cell lowercase and the shifted cell uppercase (ü/Ü yes,
//!    ß/? and é/2 no).
//!  * A dead key arms; the same dead key again types its spacing mark; dead + Space types the mark; dead + a base
//!    in [`DEAD`] types the precomposed character; dead + anything else types the mark and then the character.
//!  * Compose (when [`compose_on`]) is a TAP of Right Alt — down and up with no other modifier and no key edge
//!    between (the Windows-key tap shape `keymap` uses) — so AltGr still works while Compose is on. The next two
//!    characters go through [`PAIRS`], or a mark character plus a base through [`DEAD`] (either order); no match
//!    types both. A second tap cancels.
//!
//! Wire: `[kbd] layout=<name> dead=<n> compose=<0|1> via=<who>` on every change ([`announce`]).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

/// No character in this cell.
pub const N: char = '\0';
/// The dead keys, as the combining marks they put on the next letter.
pub const D_ACUTE: char = '\u{301}';
pub const D_GRAVE: char = '\u{300}';
pub const D_CIRC: char = '\u{302}';
pub const D_DIAER: char = '\u{308}';
pub const D_TILDE: char = '\u{303}';

/// HID modifier bits (HUT 1.12 §8): the right Alt alone is AltGr / Compose; Left Alt, Ctrl and GUI make a chord.
pub const RALT: u8 = 0x40;
const LALT: u8 = 0x04;
const CTRL: u8 = 0x11;
const GUI: u8 = 0x88;
const SHIFT: u8 = 0x22;

/// One key: its HID usage and the characters it types bare, with Shift, with AltGr.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct K {
    pub usage: u8,
    pub base: char,
    pub shift: char,
    pub altgr: char,
}

const fn k(usage: u8, base: char, shift: char, altgr: char) -> K {
    K { usage, base, shift, altgr }
}

/// A keyboard layout: the Principia spelling (`name`), the Settings segment (`label`), the long name (`title`).
pub struct Layout {
    pub name: &'static str,
    pub label: &'static str,
    pub title: &'static str,
    pub keys: &'static [K],
}

// ── The tables ─────────────────────────────────────────────────────────────────────────────────────────────────

/// US: the whole printable block (the US bytes of `drivers::xhci::HID_SCANCODE_TO_ASCII`), behind every layout.
pub static US: Layout = Layout { name: "us", label: "US", title: "U.S.", keys: &[
    k(0x04, 'a', 'A', N), k(0x05, 'b', 'B', N), k(0x06, 'c', 'C', N), k(0x07, 'd', 'D', N), k(0x08, 'e', 'E', N),
    k(0x09, 'f', 'F', N), k(0x0A, 'g', 'G', N), k(0x0B, 'h', 'H', N), k(0x0C, 'i', 'I', N), k(0x0D, 'j', 'J', N),
    k(0x0E, 'k', 'K', N), k(0x0F, 'l', 'L', N), k(0x10, 'm', 'M', N), k(0x11, 'n', 'N', N), k(0x12, 'o', 'O', N),
    k(0x13, 'p', 'P', N), k(0x14, 'q', 'Q', N), k(0x15, 'r', 'R', N), k(0x16, 's', 'S', N), k(0x17, 't', 'T', N),
    k(0x18, 'u', 'U', N), k(0x19, 'v', 'V', N), k(0x1A, 'w', 'W', N), k(0x1B, 'x', 'X', N), k(0x1C, 'y', 'Y', N),
    k(0x1D, 'z', 'Z', N),
    k(0x1E, '1', '!', N), k(0x1F, '2', '@', N), k(0x20, '3', '#', N), k(0x21, '4', '$', N), k(0x22, '5', '%', N),
    k(0x23, '6', '^', N), k(0x24, '7', '&', N), k(0x25, '8', '*', N), k(0x26, '9', '(', N), k(0x27, '0', ')', N),
    k(0x2C, ' ', ' ', N), k(0x2D, '-', '_', N), k(0x2E, '=', '+', N), k(0x2F, '[', '{', N), k(0x30, ']', '}', N),
    k(0x31, '\\', '|', N), k(0x32, '#', '~', N), k(0x33, ';', ':', N), k(0x34, '\'', '"', N), k(0x35, '`', '~', N),
    k(0x36, ',', '<', N), k(0x37, '.', '>', N), k(0x38, '/', '?', N), k(0x64, '\\', '|', N),
] };

/// UK (ISO): £ on 3, @ and " swapped, # ~ beside Return, ` ¬ ¦; AltGr+4 = €.
pub static GB: Layout = Layout { name: "gb", label: "UK", title: "British", keys: &[
    k(0x1F, '2', '"', N), k(0x20, '3', '£', N), k(0x21, '4', '$', '€'), k(0x34, '\'', '@', N), k(0x31, '#', '~', N),
    k(0x32, '#', '~', N), k(0x35, '`', '¬', '¦'), k(0x64, '\\', '|', N),
] };

/// German (QWERTZ): ü ö ä ß, dead ´ ` ^, AltGr @ € µ { [ ] } \ ~ |.
pub static DE: Layout = Layout { name: "de", label: "DE", title: "German", keys: &[
    k(0x1C, 'z', 'Z', N), k(0x1D, 'y', 'Y', N), k(0x14, 'q', 'Q', '@'), k(0x08, 'e', 'E', '€'), k(0x10, 'm', 'M', 'µ'),
    k(0x1E, '1', '!', N), k(0x1F, '2', '"', '²'), k(0x20, '3', '§', '³'), k(0x21, '4', '$', N), k(0x22, '5', '%', N),
    k(0x23, '6', '&', N), k(0x24, '7', '/', '{'), k(0x25, '8', '(', '['), k(0x26, '9', ')', ']'), k(0x27, '0', '=', '}'),
    k(0x2D, 'ß', '?', '\\'), k(0x2E, D_ACUTE, D_GRAVE, N), k(0x2F, 'ü', 'Ü', N), k(0x30, '+', '*', '~'),
    k(0x31, '#', '\'', N), k(0x32, '#', '\'', N), k(0x33, 'ö', 'Ö', N), k(0x34, 'ä', 'Ä', N), k(0x35, D_CIRC, '°', N),
    k(0x36, ',', ';', N), k(0x37, '.', ':', N), k(0x38, '-', '_', N), k(0x64, '<', '>', '|'),
] };

/// French (AZERTY): é è ç à ù on the top rows, dead ^ ¨ and AltGr dead ~ `, AltGr # { [ | \ ^ @ ] } ¤ €.
pub static FR: Layout = Layout { name: "fr", label: "FR", title: "French", keys: &[
    k(0x04, 'q', 'Q', N), k(0x14, 'a', 'A', N), k(0x1A, 'z', 'Z', N), k(0x1D, 'w', 'W', N), k(0x33, 'm', 'M', N),
    k(0x10, ',', '?', N), k(0x08, 'e', 'E', '€'),
    k(0x1E, '&', '1', N), k(0x1F, 'é', '2', D_TILDE), k(0x20, '"', '3', '#'), k(0x21, '\'', '4', '{'),
    k(0x22, '(', '5', '['), k(0x23, '-', '6', '|'), k(0x24, 'è', '7', D_GRAVE), k(0x25, '_', '8', '\\'),
    k(0x26, 'ç', '9', '^'), k(0x27, 'à', '0', '@'),
    k(0x2D, ')', '°', ']'), k(0x2E, '=', '+', '}'), k(0x2F, D_CIRC, D_DIAER, N), k(0x30, '$', '£', '¤'),
    k(0x31, '*', 'µ', N), k(0x32, '*', 'µ', N), k(0x34, 'ù', '%', N), k(0x35, '²', N, N), k(0x36, ';', '.', N),
    k(0x37, ':', '/', N), k(0x38, '!', '§', N), k(0x64, '<', '>', N),
] };

/// Spanish: ñ ç ¡ ¿ º ª, dead ´ ¨ ` ^, AltGr | @ # ~ ¬ [ ] { } \ €.
pub static ES: Layout = Layout { name: "es", label: "ES", title: "Spanish", keys: &[
    k(0x08, 'e', 'E', '€'),
    k(0x1E, '1', '!', '|'), k(0x1F, '2', '"', '@'), k(0x20, '3', '·', '#'), k(0x21, '4', '$', '~'), k(0x22, '5', '%', N),
    k(0x23, '6', '&', '¬'), k(0x24, '7', '/', N), k(0x25, '8', '(', N), k(0x26, '9', ')', N), k(0x27, '0', '=', N),
    k(0x2D, '\'', '?', N), k(0x2E, '¡', '¿', N), k(0x2F, D_GRAVE, D_CIRC, '['), k(0x30, '+', '*', ']'),
    k(0x31, 'ç', 'Ç', '}'), k(0x32, 'ç', 'Ç', '}'), k(0x33, 'ñ', 'Ñ', N), k(0x34, D_ACUTE, D_DIAER, '{'),
    k(0x35, 'º', 'ª', '\\'), k(0x36, ',', ';', N), k(0x37, '.', ':', N), k(0x38, '-', '_', N), k(0x64, '<', '>', N),
] };

/// Italian: ò à è ì ù é ç ° § £, AltGr @ # [ ] €; no dead keys.
pub static IT: Layout = Layout { name: "it", label: "IT", title: "Italian", keys: &[
    k(0x08, 'e', 'E', '€'),
    k(0x1E, '1', '!', N), k(0x1F, '2', '"', N), k(0x20, '3', '£', N), k(0x21, '4', '$', N), k(0x22, '5', '%', '€'),
    k(0x23, '6', '&', N), k(0x24, '7', '/', N), k(0x25, '8', '(', N), k(0x26, '9', ')', N), k(0x27, '0', '=', N),
    k(0x2D, '\'', '?', N), k(0x2E, 'ì', '^', N), k(0x2F, 'è', 'é', '['), k(0x30, '+', '*', ']'), k(0x31, 'ù', '§', N),
    k(0x32, 'ù', '§', N), k(0x33, 'ò', 'ç', '@'), k(0x34, 'à', '°', '#'), k(0x35, '\\', '|', N), k(0x36, ',', ';', N),
    k(0x37, '.', ':', N), k(0x38, '-', '_', N), k(0x64, '<', '>', N),
] };

/// Swedish / Finnish: å ä ö, dead ´ ` ¨ ^ and AltGr dead ~, AltGr @ £ $ € { [ ] } \ |.
pub static SE: Layout = Layout { name: "se", label: "SE", title: "Swedish / Finnish", keys: &[
    k(0x08, 'e', 'E', '€'),
    k(0x1E, '1', '!', N), k(0x1F, '2', '"', '@'), k(0x20, '3', '#', '£'), k(0x21, '4', '¤', '$'), k(0x22, '5', '%', '€'),
    k(0x23, '6', '&', N), k(0x24, '7', '/', '{'), k(0x25, '8', '(', '['), k(0x26, '9', ')', ']'), k(0x27, '0', '=', '}'),
    k(0x2D, '+', '?', '\\'), k(0x2E, D_ACUTE, D_GRAVE, N), k(0x2F, 'å', 'Å', N), k(0x30, D_DIAER, D_CIRC, D_TILDE),
    k(0x31, '\'', '*', N), k(0x32, '\'', '*', N), k(0x33, 'ö', 'Ö', N), k(0x34, 'ä', 'Ä', N), k(0x35, '§', '½', N),
    k(0x36, ',', ';', N), k(0x37, '.', ':', N), k(0x38, '-', '_', N), k(0x64, '<', '>', '|'),
] };

/// Swiss German (QWERTZ): ü/è ö/é ä/à, dead ¨ ^ ` and AltGr dead ´ ~, AltGr ¦ @ # ¬ | ¢ [ ] { } \ €.
pub static CH: Layout = Layout { name: "ch", label: "CH", title: "Swiss German", keys: &[
    k(0x1C, 'z', 'Z', N), k(0x1D, 'y', 'Y', N), k(0x08, 'e', 'E', '€'),
    k(0x1E, '1', '+', '¦'), k(0x1F, '2', '"', '@'), k(0x20, '3', '*', '#'), k(0x21, '4', 'ç', N), k(0x22, '5', '%', N),
    k(0x23, '6', '&', '¬'), k(0x24, '7', '/', '|'), k(0x25, '8', '(', '¢'), k(0x26, '9', ')', N), k(0x27, '0', '=', N),
    k(0x2D, '\'', '?', D_ACUTE), k(0x2E, D_CIRC, D_GRAVE, D_TILDE), k(0x2F, 'ü', 'è', '['), k(0x30, D_DIAER, '!', ']'),
    k(0x31, '$', '£', '}'), k(0x32, '$', '£', '}'), k(0x33, 'ö', 'é', N), k(0x34, 'ä', 'à', '{'), k(0x35, '§', '°', N),
    k(0x36, ',', ';', N), k(0x37, '.', ':', N), k(0x38, '-', '_', N), k(0x64, '<', '>', '\\'),
] };

/// Every layout, in the Settings segment order. Index = the live `ACTIVE` value.
pub static LAYOUTS: [&Layout; 8] = [&US, &GB, &DE, &FR, &ES, &IT, &SE, &CH];

/// The Principia spellings, in [`LAYOUTS`] order (the schema's enum is this list).
pub const NAMES: [&str; 8] = ["us", "gb", "de", "fr", "es", "it", "se", "ch"];

/// A dead mark + a base -> the precomposed character.
pub static DEAD: &[(char, char, char)] = &[
    (D_ACUTE, 'a', 'á'), (D_ACUTE, 'e', 'é'), (D_ACUTE, 'i', 'í'), (D_ACUTE, 'o', 'ó'), (D_ACUTE, 'u', 'ú'), (D_ACUTE, 'y', 'ý'),
    (D_ACUTE, 'A', 'Á'), (D_ACUTE, 'E', 'É'), (D_ACUTE, 'I', 'Í'), (D_ACUTE, 'O', 'Ó'), (D_ACUTE, 'U', 'Ú'), (D_ACUTE, 'Y', 'Ý'),
    (D_GRAVE, 'a', 'à'), (D_GRAVE, 'e', 'è'), (D_GRAVE, 'i', 'ì'), (D_GRAVE, 'o', 'ò'), (D_GRAVE, 'u', 'ù'),
    (D_GRAVE, 'A', 'À'), (D_GRAVE, 'E', 'È'), (D_GRAVE, 'I', 'Ì'), (D_GRAVE, 'O', 'Ò'), (D_GRAVE, 'U', 'Ù'),
    (D_CIRC, 'a', 'â'), (D_CIRC, 'e', 'ê'), (D_CIRC, 'i', 'î'), (D_CIRC, 'o', 'ô'), (D_CIRC, 'u', 'û'),
    (D_CIRC, 'A', 'Â'), (D_CIRC, 'E', 'Ê'), (D_CIRC, 'I', 'Î'), (D_CIRC, 'O', 'Ô'), (D_CIRC, 'U', 'Û'),
    (D_DIAER, 'a', 'ä'), (D_DIAER, 'e', 'ë'), (D_DIAER, 'i', 'ï'), (D_DIAER, 'o', 'ö'), (D_DIAER, 'u', 'ü'), (D_DIAER, 'y', 'ÿ'),
    (D_DIAER, 'A', 'Ä'), (D_DIAER, 'E', 'Ë'), (D_DIAER, 'I', 'Ï'), (D_DIAER, 'O', 'Ö'), (D_DIAER, 'U', 'Ü'),
    (D_TILDE, 'a', 'ã'), (D_TILDE, 'n', 'ñ'), (D_TILDE, 'o', 'õ'), (D_TILDE, 'A', 'Ã'), (D_TILDE, 'N', 'Ñ'), (D_TILDE, 'O', 'Õ'),
];

/// Compose pairs that are not a mark on a letter (the X11 Compose spellings). Order matters (`s s`, `= e` / `e =`).
pub static PAIRS: &[(char, char, char)] = &[
    ('s', 's', 'ß'), ('=', 'e', '€'), ('e', '=', '€'), ('=', 'C', '€'), ('C', '=', '€'), ('o', 'c', '©'), ('o', 'r', '®'),
    ('<', '<', '«'), ('>', '>', '»'), ('?', '?', '¿'), ('!', '!', '¡'), ('a', 'e', 'æ'), ('A', 'E', 'Æ'), ('o', '/', 'ø'),
    ('/', 'o', 'ø'), ('o', 'a', 'å'), ('o', 'o', '°'), ('1', '2', '½'), ('L', '-', '£'), ('-', 'L', '£'), ('Y', '=', '¥'),
    ('A', 'T', '@'),
];

// ── The pure machine ───────────────────────────────────────────────────────────────────────────────────────────

/// Is `c` a dead key (a combining mark from the tables)?
#[inline]
pub const fn is_dead(c: char) -> bool {
    matches!(c, D_ACUTE | D_GRAVE | D_CIRC | D_DIAER | D_TILDE)
}

/// A dead mark's spacing form (what a second press, or dead + Space, types).
pub const fn spacing(m: char) -> char {
    match m {
        D_ACUTE => '´',
        D_GRAVE => '`',
        D_CIRC => '^',
        D_DIAER => '¨',
        D_TILDE => '~',
        c => c,
    }
}

/// The mark a typed character names inside a Compose sequence (`'` or ´ acute, ` grave, ^ circumflex, " or ¨
/// diaeresis, ~ tilde).
const fn mark_of(c: char) -> Option<char> {
    match c {
        '\'' | '´' => Some(D_ACUTE),
        '`' => Some(D_GRAVE),
        '^' => Some(D_CIRC),
        '"' | '¨' => Some(D_DIAER),
        '~' => Some(D_TILDE),
        _ => None,
    }
}

const DEAD_IDX: [char; 5] = [D_ACUTE, D_GRAVE, D_CIRC, D_DIAER, D_TILDE];

/// The precomposed character for mark `m` on `base`.
pub fn compose_dead(m: char, base: char) -> Option<char> {
    DEAD.iter().find(|(a, b, _)| *a == m && *b == base).map(|t| t.2)
}

/// The Compose key's result for the pair `(a, b)`: [`PAIRS`], else a mark character with a base (either order).
pub fn compose_pair(a: char, b: char) -> Option<char> {
    if let Some(t) = PAIRS.iter().find(|(x, y, _)| *x == a && *y == b) {
        return Some(t.2);
    }
    if let Some(m) = mark_of(a) {
        if let Some(c) = compose_dead(m, b) {
            return Some(c);
        }
    }
    if let Some(m) = mark_of(b) {
        return compose_dead(m, a);
    }
    None
}

/// The row for `usage` on `l` (the layout's own row first, US behind it).
pub fn entry(l: &Layout, usage: u8) -> Option<K> {
    l.keys.iter().find(|r| r.usage == usage).or_else(|| US.keys.iter().find(|r| r.usage == usage)).copied()
}

/// How many dead keys `l` carries (cells across all three columns that are combining marks).
pub fn dead_count(l: &Layout) -> usize {
    l.keys.iter().map(|r| is_dead(r.base) as usize + is_dead(r.shift) as usize + is_dead(r.altgr) as usize).sum()
}

/// The typing state between key presses: a pending dead mark, the Compose stage and its first character, and the
/// Right Alt tap detector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KbdState {
    /// The pending dead mark (`N` = none).
    pub dead: char,
    /// 0 idle, 1 Compose armed (waiting for the first char), 2 the first char held in `first`.
    pub stage: u8,
    pub first: char,
    /// Right Alt down in the last report.
    pub ralt: bool,
    /// Right Alt went down alone and nothing has joined it yet (a tap in progress).
    pub armed: bool,
}

impl KbdState {
    pub const fn new() -> Self {
        KbdState { dead: N, stage: 0, first: N, ralt: false, armed: false }
    }
    /// Packed into one word: dead idx [0..3), stage [3..5), ralt 5, armed 6, first [8..29).
    pub fn pack(&self) -> u32 {
        let d = DEAD_IDX.iter().position(|m| *m == self.dead).map_or(0, |i| i as u32 + 1);
        d | ((self.stage as u32 & 3) << 3) | ((self.ralt as u32) << 5) | ((self.armed as u32) << 6) | ((self.first as u32 & 0x1F_FFFF) << 8)
    }
    pub fn unpack(w: u32) -> Self {
        let d = (w & 7) as usize;
        KbdState {
            dead: if d >= 1 && d <= DEAD_IDX.len() { DEAD_IDX[d - 1] } else { N },
            stage: ((w >> 3) & 3) as u8,
            first: char::from_u32((w >> 8) & 0x1F_FFFF).unwrap_or(N),
            ralt: (w >> 5) & 1 != 0,
            armed: (w >> 6) & 1 != 0,
        }
    }
}

/// What one press typed: 0, 1 or 2 characters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Out {
    pub c: [char; 2],
    pub n: usize,
}

impl Out {
    pub const NONE: Out = Out { c: [N, N], n: 0 };
    const fn one(a: char) -> Out {
        Out { c: [a, N], n: 1 }
    }
    const fn two(a: char, b: char) -> Out {
        Out { c: [a, b], n: 2 }
    }
    pub fn chars(&self) -> &[char] {
        &self.c[..self.n]
    }
}

/// ONE press of `usage` under `mods` / `caps` on layout `l`. `None` = not the layout's key (a chord, the Ctrl
/// fold, a non-printing key, AltGr on a key with no AltGr cell): the caller keeps its existing path.
/// `Some(Out::NONE)` = consumed with nothing typed yet (a dead key armed, a Compose char held).
pub fn step(l: &Layout, st: &mut KbdState, usage: u8, mods: u8, caps: bool) -> Option<Out> {
    if mods & (CTRL | GUI | LALT) != 0 {
        return None;
    }
    let r = entry(l, usage)?;
    let c = if mods & RALT != 0 {
        if r.altgr == N {
            return None;
        }
        r.altgr
    } else {
        let letter = r.base.is_lowercase() && r.shift.is_uppercase();
        if (mods & SHIFT != 0) ^ (caps && letter) { r.shift } else { r.base }
    };
    if c == N {
        return Some(Out::NONE);
    }
    // Compose: the two characters after the tap (a dead key counts as its spacing mark here).
    if st.stage == 1 {
        st.first = spacing(c);
        st.stage = 2;
        return Some(Out::NONE);
    }
    if st.stage == 2 {
        let a = st.first;
        let b = spacing(c);
        st.stage = 0;
        st.first = N;
        return Some(match compose_pair(a, b) {
            Some(x) => Out::one(x),
            None => Out::two(a, b),
        });
    }
    if is_dead(c) {
        let m = st.dead;
        if m == N {
            st.dead = c;
            return Some(Out::NONE);
        }
        if m == c {
            st.dead = N;
            return Some(Out::one(spacing(m)));
        }
        st.dead = c;
        return Some(Out::one(spacing(m)));
    }
    if st.dead != N {
        let m = st.dead;
        st.dead = N;
        if c == ' ' {
            return Some(Out::one(spacing(m)));
        }
        return Some(match compose_dead(m, c) {
            Some(x) => Out::one(x),
            None => Out::two(spacing(m), c),
        });
    }
    Some(Out::one(c))
}

/// ONE HID boot report: the Right Alt tap (Compose, when `compose`) and every PRESS edge (`cur` keys absent from
/// `prev`) through [`step`]. Returns how many characters were written to `out` (at most 12: six edges x two).
pub fn report(l: &Layout, st: &mut KbdState, compose: bool, cur: &[u8; 6], prev: &[u8; 6], mods: u8, caps: bool, out: &mut [char; 12]) -> usize {
    let now = mods & RALT != 0;
    let edged = cur.iter().any(|&u| u > 1 && !prev.contains(&u));
    if now && !st.ralt {
        st.armed = compose && mods & !RALT == 0 && !edged;
    } else if now && (edged || mods & !RALT != 0) {
        st.armed = false;
    }
    let mut n = 0usize;
    for &u in cur.iter() {
        if u > 1 && !prev.contains(&u) {
            if let Some(o) = step(l, st, u, mods, caps) {
                for &c in o.chars() {
                    if n < out.len() {
                        out[n] = c;
                        n += 1;
                    }
                }
            }
        }
    }
    if !now && st.ralt && st.armed {
        st.armed = false;
        if st.stage == 0 {
            st.stage = 1;
            st.dead = N;
        } else {
            st.stage = 0;
            st.first = N;
        }
    }
    st.ralt = now;
    n
}

// ── The live layout (Principia's `system.keyboard.*`, set by Settings) ─────────────────────────────────────────

static ACTIVE: AtomicU8 = AtomicU8::new(0);
static COMPOSE: AtomicBool = AtomicBool::new(false);
static STATE: AtomicU32 = AtomicU32::new(0);

/// The live layout.
pub fn active() -> &'static Layout {
    LAYOUTS[(ACTIVE.load(Ordering::Relaxed) as usize).min(LAYOUTS.len() - 1)]
}

/// Is the Compose key (a Right Alt tap) on?
pub fn compose_on() -> bool {
    COMPOSE.load(Ordering::Relaxed)
}

/// The index of a Principia spelling.
pub fn index_of(name: &str) -> Option<usize> {
    NAMES.iter().position(|n| *n == name)
}

/// `[kbd] layout=<name> dead=<n> compose=<0|1> via=<who>` — the live layout, named.
pub fn announce(via: &str) {
    let l = active();
    serial_println!("[kbd] layout={} dead={} compose={} via={}", l.name, dead_count(l), compose_on() as u8, via);
}

/// Select layout `i` (clamped) and the Compose switch. A change to either drops any pending dead key or Compose
/// sequence. Announces once.
pub fn set(i: usize, compose: bool, via: &str) {
    let i = i.min(LAYOUTS.len() - 1) as u8;
    let moved = ACTIVE.swap(i, Ordering::Relaxed) != i;
    let cm = COMPOSE.swap(compose, Ordering::Relaxed) != compose;
    if moved || cm {
        STATE.store(KbdState::new().pack(), Ordering::Relaxed);
    }
    announce(via);
}

/// Select by Principia spelling, the Compose switch unchanged; an unknown spelling is refused (`false`).
pub fn set_layout_named(name: &str, via: &str) -> bool {
    match index_of(name) {
        Some(i) => {
            set(i, compose_on(), via);
            true
        }
        None => false,
    }
}

/// [`report`] on the live layout and the live state (the decoders' one call, through `keymap::layout_report`).
pub fn report_live(cur: &[u8; 6], prev: &[u8; 6], mods: u8, caps: bool, out: &mut [char; 12]) -> usize {
    let mut st = KbdState::unpack(STATE.load(Ordering::Relaxed));
    let n = report(active(), &mut st, compose_on(), cur, prev, mods, caps, out);
    STATE.store(st.pack(), Ordering::Relaxed);
    n
}

/// The fixture's swap: install `(layout, compose, state)` silently and return the previous triple, so a test
/// drives the LIVE seam and puts everything back without a `[kbd]` line.
pub fn fixture_swap(layout: usize, compose: bool, state: u32) -> (usize, bool, u32) {
    let l = ACTIVE.swap(layout.min(LAYOUTS.len() - 1) as u8, Ordering::Relaxed) as usize;
    let c = COMPOSE.swap(compose, Ordering::Relaxed);
    let s = STATE.swap(state, Ordering::Relaxed);
    (l, c, s)
}
