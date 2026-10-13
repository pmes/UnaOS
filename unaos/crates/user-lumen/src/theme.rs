// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAKEYS (rmbp-ledger B552): RING-3 THEME READS. CHARTER: Principia owns the choice (`system.appearance.mode` /
// `.accent` / `.highlight`, prefs_core::schema); the
// painter's palette is the kernel's (`video/theme.rs` LIGHT / DARK / ACCENTS). Seam: a bus CLIENT — the PREFS verbs
// (una-abi BUS_VERB_PREF_GET 16, fulfilled in-kernel by `prefs::bus_fulfil`, the same store the
// kernel's own `prefs_client` reads, SETTINGSBUS B337). Nothing here decides a preference.
//
// THE RULE (the appearance gate's, `scripts/appearance-check.py`, applied to ring 3): every colour this program
// paints is a field of [`Theme`]; this file is the only one in the crate that spells a colour. The palette rows are
// the kernel set's values for the tokens this program reads — copied, because ring 3 links no kernel file (owed: a
// shared no_std palette core both rings link, so the copy cannot drift).
//
// WHEN. [`load`] once at start; [`due`] from the frame loop, then [`load`] again. Ring-3 delivery of PrefChanged (verb 19) is OWED
// (kernel `prefs_client.rs` header: "Ring-3 delivery of verb 19 … is OWED"), and SYS_MRECV blocks on an empty
// mailbox, so a program cannot wait for it beside its input ring: [`due`] answers every [`POLL_MS`] instead, and a
// PrefChanged frame met in the mailbox while a reply is awaited (kind REPLY, verb 19, corr 0) re-reads at once. A
// re-read that moves the look answers `true` and the caller repaints everything.

use una_abi::{
    BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC, BUS_VERB_PREF_CHANGED, BUS_VERB_PREF_GET,
    BUS_VERSION, SYS_MRECV, SYS_MSEND, SYS_TIME,
};
use vein_ring3::sys::sys;

/// How often [`due`] asks for a re-read of the store (ms) while PrefChanged has no ring-3 delivery.
pub const POLL_MS: u64 = 2000;

// ---- the palette (kernel `video/theme.rs`) ------------------------------------------------------------------

/// The kernel tokens the four ring-3 programs read (the same rows in each program's theme.rs; a program uses those
/// its roles name).
#[allow(dead_code)]
#[derive(Clone, Copy)]
#[repr(usize)]
enum Tok {
    WindowBg,
    FrameLine,
    TitleActiveBottom,
    TitleTextInactive,
    ContentFill,
    ContentText,
    FieldBg,
    ErrorText,
    SyntaxCode,
    SyntaxNum,
    SyntaxLit,
    MeterLow,
}
const N: usize = 12;
/// LIGHT rows: CHROME_FACE, FRAME_LINE, TITLE_ACTIVE_BOTTOM, TITLE_TEXT_INACTIVE, CONTENT_FILL, CONTENT_TEXT, FieldBg,
/// ErrorText, SyntaxCode / Num / Lit, MeterLow.
const LIGHT: [u32; N] = [
    0x00EC_ECEE, 0x00B4_B4B9, 0x00E3_E3E8, 0x007A_7A7E, 0x00F5_F2EA, 0x0021_201E, 0x00FF_FFFF, 0x00A0_2020, 0x0020_8040, 0x00B0_5000, 0x0080_30A0,
    0x0043_A05A,
];
/// DARK rows, the same tokens.
const DARK: [u32; N] = [
    0x002A_2A2D, 0x004A_4A50, 0x0034_3438, 0x008E_8E94, 0x001E_1E20, 0x00E6_E4DF, 0x002C_2C2F, 0x00E5_7373, 0x006C_C88A, 0x00E3_9A55, 0x00C3_8BE0,
    0x0052_B86A,
];
/// The eight accents, `prefs_core::appearance::ACCENTS` order (index 0 the kit's own).
const ACCENTS: [u32; 8] = [0x004A_73AA, 0x002E_8C8C, 0x005B_8C3A, 0x00C0_8A1E, 0x00C0_603A, 0x00B0_4A6A, 0x007A_5AB0, 0x006E_6E73];
/// Their names (`prefs_core::appearance::ACCENTS`, spelled here: this crate links no prefs_core).
const ACCENT_NAMES: [&str; 8] = ["crispy", "teal", "moss", "amber", "clay", "rose", "violet", "slate"];

/// The appearance as Principia stores it, resolved: the set, the accent, the highlight.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    pub accent: u8,
    pub hl: u8,
}
#[allow(dead_code)]
impl Look {
    /// The store's defaults (`appearance.mode = light`, `accent = crispy`, `highlight = accent`).
    pub const DEFAULT: Look = Look { dark: false, accent: 0, hl: 0 };
    const fn tok(self, t: Tok) -> u32 {
        if self.dark { DARK[t as usize] } else { LIGHT[t as usize] }
    }
    const fn accent(self) -> u32 {
        ACCENTS[self.accent as usize % 8]
    }
    /// The kernel's `selection()`: the highlight mixed toward the content ground (5/8 Light, 3/8 Dark).
    const fn selection(self) -> u32 {
        let (num, den) = if self.dark { (3, 8) } else { (5, 8) };
        mix(ACCENTS[self.hl as usize % 8], self.tok(Tok::ContentFill), num, den)
    }
    pub fn mode_name(self) -> &'static str {
        if self.dark { "dark" } else { "light" }
    }
    pub fn accent_name(self) -> &'static str {
        ACCENT_NAMES[self.accent as usize % 8]
    }
}

/// `a` and `b` mixed `num/den` of the way to `b`, per channel (the kernel's `theme::mix`).
const fn mix(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let mut out = 0;
    let mut s = 0;
    while s < 24 {
        let (x, y) = ((a >> s) & 0xFF, (b >> s) & 0xFF);
        out |= ((x * (den - num) + y * num) / den) << s;
        s += 8;
    }
    out
}
const fn opaque(c: u32) -> u32 {
    0xFF00_0000 | c
}

// ---- this program's roles ----------------------------------------------------------------------------------

/// Every colour LUMEN.ELF paints (`0xAARRGGBB`, opaque).
#[derive(Clone, Copy)]
pub struct Theme {
    pub look: Look,
    pub bg: u32,
    /// The rules (footer, input, a markdown rule) and the scrollbar track.
    pub rule: u32,
    pub ink: u32,
    /// The user's turns and the input's `>`.
    pub user_ink: u32,
    /// Notes and the second status row.
    pub note_ink: u32,
    /// The caret, the busy / flash status, the scrollbar thumb.
    pub caret: u32,
    /// The first status row (the provider line).
    pub ok_ink: u32,
    /// Markdown: a heading, code, a quote, a link.
    pub head_ink: u32,
    pub code_ink: u32,
    pub code_bg: u32,
    pub quote_ink: u32,
    pub link_ink: u32,
    /// The selected reply's bar.
    pub sel_bar: u32,
}
impl Theme {
    pub const fn of(l: Look) -> Theme {
        Theme {
            look: l,
            bg: opaque(l.tok(Tok::ContentFill)),
            rule: opaque(l.tok(Tok::FrameLine)),
            ink: opaque(l.tok(Tok::ContentText)),
            user_ink: opaque(l.accent()),
            note_ink: opaque(l.tok(Tok::TitleTextInactive)),
            caret: opaque(l.accent()),
            ok_ink: opaque(l.tok(Tok::MeterLow)),
            head_ink: opaque(l.tok(Tok::SyntaxNum)),
            code_ink: opaque(l.tok(Tok::SyntaxCode)),
            code_bg: opaque(l.tok(Tok::FieldBg)),
            quote_ink: opaque(l.tok(Tok::SyntaxLit)),
            link_ink: opaque(l.accent()),
            sel_bar: opaque(l.selection()),
        }
    }
}

// ---- the bus client ----------------------------------------------------------------------------------------

static mut TX: [u8; BUS_HDR_LEN + 64] = [0; BUS_HDR_LEN + 64];
static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
static mut CORR: u32 = 0x5448_0000;
/// A PrefChanged frame was met in the mailbox: the next [`due`] answers at once.
static mut HINT: bool = false;

/// One PREFS request; the reply body is copied into `out`. `Err` = the bus refused (or `-ENOENT`: unset).
fn call(verb: u8, body: &[u8], out: &mut [u8]) -> Result<usize, i64> {
    let (tx, rx) = unsafe { (&mut *core::ptr::addr_of_mut!(TX), &mut *core::ptr::addr_of_mut!(RX)) };
    if body.len() > tx.len() - BUS_HDR_LEN {
        return Err(una_abi::EINVAL);
    }
    let corr = unsafe {
        CORR = CORR.wrapping_add(1);
        CORR
    };
    tx[..BUS_HDR_LEN].fill(0);
    tx[0..4].copy_from_slice(&BUS_MAGIC);
    tx[4] = BUS_VERSION;
    tx[5] = BUS_KIND_REQUEST;
    tx[6] = verb;
    tx[8..12].copy_from_slice(&corr.to_le_bytes());
    tx[48..52].copy_from_slice(&(body.len() as u32).to_le_bytes());
    tx[BUS_HDR_LEN..BUS_HDR_LEN + body.len()].copy_from_slice(body);
    let s = sys(SYS_MSEND, tx.as_ptr() as u64, (BUS_HDR_LEN + body.len()) as u64, 0, 0);
    if s != 0 {
        return Err(s);
    }
    for _ in 0..8 {
        let n = sys(SYS_MRECV, rx.as_mut_ptr() as u64, rx.len() as u64, 0, 0);
        if n < 0 {
            return Err(n);
        }
        let n = n as usize;
        if n < BUS_HDR_LEN || rx[0..4] != BUS_MAGIC || rx[5] != BUS_KIND_REPLY {
            continue;
        }
        if rx[6] == BUS_VERB_PREF_CHANGED && rx[8..12] == [0; 4] {
            unsafe { HINT = true };
            continue;
        }
        if rx[8..12] != corr.to_le_bytes() {
            continue; // a stale frame: not ours
        }
        let status = i32::from_le_bytes([rx[12], rx[13], rx[14], rx[15]]) as i64;
        if status != 0 {
            return Err(status);
        }
        let blen = (u32::from_le_bytes([rx[48], rx[49], rx[50], rx[51]]) as usize).min(n - BUS_HDR_LEN);
        let m = blen.min(out.len());
        out[..m].copy_from_slice(&rx[BUS_HDR_LEN..BUS_HDR_LEN + m]);
        return Ok(m);
    }
    Err(una_abi::EIO)
}

/// A TOML scalar literal without its quotes and blanks.
fn unquote(b: &[u8]) -> &[u8] {
    let mut s = b;
    while let [c, rest @ ..] = s {
        if c.is_ascii_whitespace() { s = rest } else { break }
    }
    while let [rest @ .., c] = s {
        if c.is_ascii_whitespace() { s = rest } else { break }
    }
    match s {
        [b'"', mid @ .., b'"'] | [b'\'', mid @ .., b'\''] => mid,
        _ => s,
    }
}

/// `system.<key>`'s word (unquoted, at most 16 bytes) into `buf`; `None` unset or refused (the first refusal other
/// than unset lands in `err`).
fn word<'a>(key: &str, buf: &'a mut [u8; 16], err: &mut i64) -> Option<&'a [u8]> {
    let mut raw = [0u8; 32];
    match call(BUS_VERB_PREF_GET, key.as_bytes(), &mut raw) {
        Ok(n) => {
            let w = unquote(&raw[..n]);
            let m = w.len().min(16);
            buf[..m].copy_from_slice(&w[..m]);
            Some(&buf[..m])
        }
        Err(e) => {
            if e != una_abi::ENOENT && *err == 0 {
                *err = e;
            }
            None
        }
    }
}

fn accent_ix(w: Option<&[u8]>) -> Option<u8> {
    let w = w?;
    ACCENT_NAMES.iter().position(|n| n.as_bytes().eq_ignore_ascii_case(w)).map(|i| i as u8)
}

/// Read the look from the store: `(look, the first bus refusal or 0)`. `auto` is dark 19:00-07:00 by the kernel's
/// civil clock (SYS_TIME — the clock `video::appearance::local_hour` reads); no clock = light (prefs_core's rule).
pub fn read_look() -> (Look, i64) {
    let mut err = 0;
    let mut b = [0u8; 16];
    let dark = match word("system.appearance.mode", &mut b, &mut err) {
        Some(w) if w.eq_ignore_ascii_case(b"dark") => true,
        Some(w) if w.eq_ignore_ascii_case(b"auto") => {
            let t = sys(SYS_TIME, 0, 0, 0, 0);
            t >= 0 && {
                let h = (t / 3600) % 24;
                !(7..19).contains(&h)
            }
        }
        _ => false,
    };
    let accent = accent_ix(word("system.appearance.accent", &mut b, &mut err)).unwrap_or(0);
    let hl = accent_ix(word("system.appearance.highlight", &mut b, &mut err)).unwrap_or(accent);
    (Look { dark, accent, hl }, err)
}

// ---- the live theme ----------------------------------------------------------------------------------------

static mut CUR: Theme = Theme::of(Look::DEFAULT);
static mut NEXT: u64 = 0;
static mut BUS_ERR: i64 = 0;

/// The theme every painter reads.
pub fn cur() -> Theme {
    unsafe { *core::ptr::addr_of!(CUR) }
}
/// The first bus refusal of the last read (0 = Principia answered).
pub fn bus_err() -> i64 {
    unsafe { BUS_ERR }
}
/// Read the store and install the look; `true` when it moved.
pub fn load() -> bool {
    let (l, e) = read_look();
    unsafe {
        BUS_ERR = e;
        let moved = (*core::ptr::addr_of!(CUR)).look != l;
        CUR = Theme::of(l);
        moved
    }
}
/// The frame loop's clock: `true` once every [`POLL_MS`] (or at once after a PrefChanged frame) — time to re-read.
pub fn due(now_ms: u64) -> bool {
    unsafe {
        if !HINT && now_ms < NEXT {
            return false;
        }
        HINT = false;
        NEXT = now_ms + POLL_MS;
    }
    true
}
