// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UNAKEYS (rmbp-ledger B552): RING-3 THEME READS. CHARTER: Principia owns the choice (`system.appearance.mode` /
// `.accent` / `.highlight`, prefs_core::schema); the
// painter's palette is `palette_core` (PALETTECORE B559: the LIGHT / DARK rows, the ACCENTS, `Look`, the role struct
// `palette_core::Theme` — the one no_std table the kernel's `video/theme.rs` re-exports too). Seam: a bus CLIENT — the PREFS verbs
// (una-abi BUS_VERB_PREF_GET 16, fulfilled in-kernel by `prefs::bus_fulfil`, the same store the
// kernel's own `prefs_client` reads, SETTINGSBUS B337). Nothing here decides a preference.
//
// THE RULE (the appearance gate's, `scripts/appearance-check.py`, applied to ring 3): every colour this program
// paints is a field of [`Theme`], mapped over `palette_core` — no file of this crate spells a colour, this one
// included: the rows are the core's, linked by both rings (shared-core seam), so ring 3 cannot drift from the kernel.
// The gate scans this file for zero beside the kernel's `video/theme.rs`.
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

// ---- the palette (`palette_core`, PALETTECORE B559 — the table the kernel re-exports) --------------------------

/// The appearance as Principia stores it, resolved (the set, the accent, the highlight) — the core's.
pub use palette_core::Look;
use palette_core::{opaque, Tok};

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
        let p = palette_core::Theme::of(l);
        Theme {
            look: l,
            bg: opaque(p.tok(Tok::SurfaceContent)),
            rule: opaque(p.tok(Tok::LineSubtle)),
            ink: opaque(p.tok(Tok::TextPrimary)),
            user_ink: opaque(p.accent),
            note_ink: opaque(p.tok(Tok::TextSecondary)),
            caret: opaque(p.accent),
            ok_ink: opaque(p.tok(Tok::InkPositive)),
            head_ink: opaque(p.tok(Tok::SyntaxNum)),
            code_ink: opaque(p.tok(Tok::SyntaxCode)),
            code_bg: opaque(p.tok(Tok::SurfaceSunken)),
            quote_ink: opaque(p.tok(Tok::SyntaxLit)),
            link_ink: opaque(p.accent),
            sel_bar: opaque(p.selection),
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
    palette_core::accent_by_name(w?)
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
/// PREFCHANGED (rmbp-ledger B558): the frame loop's question — `true` when the store moved, time to re-read. The first
/// call subscribes this program to PrefChanged (verb 19 as a REQUEST, one per prefix below) and answers `true` once (a
/// write may land between [`load`] and the subscription); every call then drains the mailbox WITHOUT blocking
/// (`SYS_MRECV(…, MRECV_NOWAIT)`) and answers `true` iff a delivered PrefChanged frame was met. The [`POLL_MS`]
/// re-read survives only as the fallback for a kernel that refuses the subscription; otherwise every [`POLL_MS`] the
/// civil hour alone is checked (an `auto` look turns on the hour without a write).
pub fn due(now_ms: u64) -> bool {
    const SUBSCRIBE: &[&str] = &["system.appearance."];
    static mut SUB: i64 = 1; // 1 = not yet asked; 0 = subscribed; < 0 = refused (the poll fallback)
    static mut HOUR: i64 = -1; // the civil hour at the last clock check
    unsafe {
        if SUB == 1 {
            SUB = 0;
            for p in SUBSCRIBE {
                if let Err(e) = call(BUS_VERB_PREF_CHANGED, p.as_bytes(), &mut []) {
                    SUB = e;
                }
            }
            HINT = true;
        }
        let rx = &mut *core::ptr::addr_of_mut!(RX);
        while SUB == 0 {
            let n = sys(SYS_MRECV, rx.as_mut_ptr() as u64, rx.len() as u64, una_abi::MRECV_NOWAIT, 0);
            if n < BUS_HDR_LEN as i64 {
                break; // -EAGAIN: the mailbox is empty
            }
            if rx[0..4] == BUS_MAGIC && rx[5] == BUS_KIND_REPLY && rx[6] == BUS_VERB_PREF_CHANGED && rx[8..12] == [0; 4] {
                HINT = true;
            }
        }
        if now_ms >= NEXT {
            // No store re-read here: the civil hour is checked, because `auto` turns dark/light on it with no write.
            NEXT = now_ms + POLL_MS;
            let t = sys(SYS_TIME, 0, 0, 0, 0);
            let h = if t >= 0 { (t / 3600) % 24 } else { -1 };
            if SUB < 0 || (HOUR >= 0 && h != HOUR) {
                HINT = true;
            }
            HOUR = h;
        }
        if !HINT {
            return false;
        }
        HINT = false;
    }
    true
}
