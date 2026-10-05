// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! GLASSEYES (rmbp-ledger B343, SR23 EYES) — `shot <state>`: a DETERMINISTIC picture of a named state of the
//! desktop, with a mask of what is allowed to differ, so the bench can pull it off the card and EYES can score it
//! against a golden (`tools/eyes/run.sh metal --from <dir>`).
//!
//! ONE VERB, FIVE STATES: `login | desktop | quarry | settings <general|users|display|about> | lumen`.
//!
//!  1. COMPOSE — put the state on the glass: the lock screen over the session (SCREENLOCK's `lock`, released after
//!     the capture), the kernel windows closed, Quarry / Settings opened, Lumen spawned (and killed after).
//!  2. SETTLE — the wm keeps no global "dirty region" counter, so the composed frame itself is the signal: the panel
//!     sampled on a [`GRID`]-px lattice, OUTSIDE the mask, every [`SAMPLE_MS`]; settled = [`SETTLE_N`] consecutive
//!     identical samples (no change between N presents), bounded by [`SETTLE_MAX_MS`]. A timeout still captures and
//!     says `settle=timeout` — a moving state is a finding, not a reason to write nothing.
//!  3. CAPTURE — through the EXISTING screenshot job (`prtscr::capture_named`): same panel door, same streaming
//!     encoder, same mount-table write and SHOTZIP/SHOTMOUNT witness, into `/home/<u>/Shots/<STEM>.PNG`, replacing
//!     the last shot of that state.
//!  4. MASK — `/home/<u>/Shots/<STEM>.MSK`, a PNG of the panel's size (white = masked, black = scored), from the
//!     per-state table [`mask_table`]: every state masks the menu-bar CLOCK, the CURSOR sprite and the status
//!     GLYPHS (battery item, brightness transient, anything else left of the clock); a state adds what it names
//!     volatile (Quarry's size/date columns).
//!
//! NAMES. FAT's create path writes 8.3 only (prtscr's documented rule), so `<state>.png` / `<state>.mask.png` are
//! spelled `<STEM>.PNG` / `<STEM>.MSK`; the runner reads both spellings. Stems: [`stem`].
//!
//! WITNESS. `:: SHOT: state=<s> file=<path> settle=<ok|timeout> samples=<n> settle_ms=<ms> mask=<kind>@x,y,w,h;…
//! mask_file=<path> -> OK|FAIL ::`, and from `tests shot`
//! `:: GLASSEYES: states=5 shot=desktop png=ok mask=clock,cursor,glyphs -> PASS ::`. Nothing runs at boot.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::png::PngEncoder;
use crate::console::Console;

/// The state names `shot` takes (the `settings` state takes a tab after it).
pub const STATES: [&str; 5] = ["login", "desktop", "quarry", "settings", "lumen"];
/// The leaf under the session's home the shots land in. 5 characters: a legal 8.3 directory name as written.
pub const SHOTS_DIR: &str = "Shots";
/// Settings tabs, in strip order, and their 8.3 stems.
const TABS: [(&str, &str); 4] = [("general", "SETGEN"), ("users", "SETUSR"), ("display", "SETDSP"), ("about", "SETABT")];

/// SETTLE: consecutive identical samples that make a settled frame.
const SETTLE_N: u32 = 3;
/// SETTLE: the sample period.
const SAMPLE_MS: u64 = 100;
/// SETTLE: the bound — a state that has not settled by then is captured as it is, `settle=timeout`.
const SETTLE_MAX_MS: u64 = 4_000;
/// SETTLE: the sampling lattice pitch, panel pixels.
const GRID: usize = 16;
/// COMPOSE: how long a latched open may take to reach the glass.
const OPEN_MAX_MS: u64 = 2_000;
/// MASK: slack around the cursor sprite's box (a one-report-stale box, and the move between mask and capture).
const CURSOR_PAD: usize = 8;

/// A state, parsed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Login,
    Desktop,
    Quarry,
    Settings(usize),
    Lumen,
}

/// One masked rectangle, panel pixels, with the kind the witness names.
#[derive(Clone, Copy)]
pub struct MaskRect {
    pub kind: &'static str,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl MaskRect {
    fn covers(&self, x: usize, y: usize) -> bool {
        x >= self.x && y >= self.y && x < self.x.saturating_add(self.w) && y < self.y.saturating_add(self.h)
    }
}

/// The 8.3 stem a state's files carry.
pub fn stem(s: State) -> &'static str {
    match s {
        State::Login => "LOGIN",
        State::Desktop => "DESKTOP",
        State::Quarry => "QUARRY",
        State::Settings(t) => TABS.get(t).map(|p| p.1).unwrap_or("SETGEN"),
        State::Lumen => "LUMEN",
    }
}

/// The state's name as the bench (and the golden set) spells it: `settings-display`, `desktop`, …
pub fn label(s: State) -> String {
    match s {
        State::Login => String::from("login"),
        State::Desktop => String::from("desktop"),
        State::Quarry => String::from("quarry"),
        State::Settings(t) => format!("settings-{}", TABS.get(t).map(|p| p.0).unwrap_or("general")),
        State::Lumen => String::from("lumen"),
    }
}

/// Is `w` one of the state words (the x86 `shot` arm asks before its `region|window` usage line)?
pub fn is_state(w: &str) -> bool {
    STATES.contains(&w)
}

/// `shot <state> [tab]` -> the state, or the usage sentence.
pub fn parse(args: &[&str]) -> Result<State, String> {
    match args.first().copied().unwrap_or("") {
        "login" => Ok(State::Login),
        "desktop" => Ok(State::Desktop),
        "quarry" => Ok(State::Quarry),
        "lumen" => Ok(State::Lumen),
        "settings" => {
            let t = args.get(1).copied().unwrap_or("general");
            match TABS.iter().position(|p| p.0.eq_ignore_ascii_case(t)) {
                Some(i) => Ok(State::Settings(i)),
                None => Err(format!("shot settings: no tab `{}` (general | users | display | about)", t)),
            }
        }
        _ => Err(String::from("usage: shot <login | desktop | quarry | settings <general|users|display|about> | lumen>")),
    }
}

// ── MASK ─────────────────────────────────────────────────────────────────────────────────────────────

/// THE PER-STATE MASK TABLE. Common to every state: the clock, the status glyphs, the cursor. Per state: what
/// that state names volatile. Absent furniture (the bar off, the sprite hidden) is simply not in the list.
pub fn mask_table(s: State, pw: usize, ph: usize) -> Vec<MaskRect> {
    let mut m: Vec<MaskRect> = Vec::new();
    let mut push = |kind: &'static str, r: (usize, usize, usize, usize)| {
        let (x, y) = (r.0.min(pw), r.1.min(ph));
        let (w, h) = (r.2.min(pw - x), r.3.min(ph - y));
        if w != 0 && h != 0 {
            m.push(MaskRect { kind, x, y, w, h });
        }
    };
    let (clock, glyphs) = super::menubar::volatile_rects(pw, ph);
    if let Some(r) = clock {
        push("clock", r);
    }
    if let Some(r) = glyphs {
        push("glyphs", r);
    }
    if let Some((x, y, w, h)) = super::cursor::live_box_relaxed() {
        push("cursor", (x.saturating_sub(CURSOR_PAD), y.saturating_sub(CURSOR_PAD), w + 2 * CURSOR_PAD, h + 2 * CURSOR_PAD));
    }
    match s {
        // Quarry lists the home folder: sizes and dates move with every shot written into it. The right 45 % of
        // the content (the size and date columns) is masked; the names, the chrome and the layout are scored.
        #[cfg(feature = "quarry")]
        State::Quarry => {
            if let Some((x, y, w, h)) = super::wm::frame_of(super::quarry::live::win_id()) {
                let top = super::wm::TITLE_H.min(h);
                push("columns", (x + w * 55 / 100, y + top, w - w * 55 / 100, h - top));
            }
        }
        _ => {}
    }
    m
}

fn masked(m: &[MaskRect], x: usize, y: usize) -> bool {
    m.iter().any(|r| r.covers(x, y))
}

/// `clock@x,y,w,h;cursor@…` for the witness.
fn mask_wire(m: &[MaskRect]) -> String {
    let mut s = String::new();
    for r in m {
        if !s.is_empty() {
            s.push(';');
        }
        s.push_str(&format!("{}@{},{},{},{}", r.kind, r.x, r.y, r.w, r.h));
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

/// The mask's kinds, de-duplicated, in table order: `clock,glyphs,cursor`.
fn mask_kinds(m: &[MaskRect]) -> String {
    let mut s = String::new();
    for (i, r) in m.iter().enumerate() {
        if m[..i].iter().any(|p| p.kind == r.kind) {
            continue;
        }
        if !s.is_empty() {
            s.push(',');
        }
        s.push_str(r.kind);
    }
    s
}

// ── SETTLE ───────────────────────────────────────────────────────────────────────────────────────────

/// One sample of the composed frame: FNV-1a over the lattice pixels outside the mask, read through the panel
/// handle exactly as the capture reads them (no panel lock held across the reads).
fn sample(m: &[MaskRect]) -> Option<u64> {
    let fb = super::panel_snapshot()?;
    if !fb.is_ready() {
        return None;
    }
    let info = fb.info();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut y = GRID / 2;
    while y < info.height {
        let mut x = GRID / 2;
        while x < info.width {
            if !masked(m, x, y) {
                let p = fb.read_pixel(x, y).unwrap_or(0);
                for b in p.to_le_bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            x += GRID;
        }
        y += GRID;
    }
    Some(h)
}

fn sleep_ms(ms: u64) {
    let t0 = crate::arch::ms();
    while crate::arch::ms().saturating_sub(t0) < ms {
        crate::arch::sched::yield_now();
    }
}

/// Wait for [`SETTLE_N`] identical samples in a row, bounded. `(settled, samples, ms)`.
fn settle(m: &[MaskRect]) -> (bool, u32, u64) {
    let t0 = crate::arch::ms();
    let (mut last, mut same, mut n) = (None, 0u32, 0u32);
    loop {
        let s = sample(m);
        n += 1;
        if s.is_some() && s == last {
            same += 1;
            if same + 1 >= SETTLE_N {
                return (true, n, crate::arch::ms().saturating_sub(t0));
            }
        } else {
            same = 0;
        }
        last = s;
        if crate::arch::ms().saturating_sub(t0) >= SETTLE_MAX_MS {
            return (false, n, crate::arch::ms().saturating_sub(t0));
        }
        sleep_ms(SAMPLE_MS);
    }
}

fn wait_until(f: impl Fn() -> bool) -> bool {
    let t0 = crate::arch::ms();
    while !f() {
        if crate::arch::ms().saturating_sub(t0) >= OPEN_MAX_MS {
            return false;
        }
        sleep_ms(10);
    }
    true
}

// ── COMPOSE ──────────────────────────────────────────────────────────────────────────────────────────

/// What to undo after the capture.
enum Undo {
    None,
    #[cfg(feature = "login")]
    Unlock,
    #[cfg(all(target_arch = "x86_64", feature = "lumen"))]
    Kill(u64, u64),
}

/// The kernel's own windows a state shot closes first, so every state starts from the same desktop.
fn close_kernel_windows() {
    super::settings::close();
    #[cfg(feature = "quarry")]
    super::quarry::close();
    #[cfg(feature = "facet")]
    super::facet::close();
}

fn compose(s: State) -> Result<Undo, String> {
    #[cfg(feature = "login")]
    if super::crystal::login::is_open() {
        return Err(String::from("the login screen is already up — log in (or unlock) first"));
    }
    close_kernel_windows();
    match s {
        State::Desktop => Ok(Undo::None),
        State::Settings(t) => {
            if !super::settings::set_tab_named(TABS[t].0) {
                return Err(format!("settings has no tab `{}`", TABS[t].0));
            }
            super::settings::open().map_err(|e| format!("settings: {}", e))?;
            Ok(Undo::None)
        }
        #[cfg(feature = "quarry")]
        State::Quarry => {
            super::quarry::request_open();
            if !wait_until(super::quarry::is_open) {
                return Err(format!("quarry did not open within {} ms", OPEN_MAX_MS));
            }
            Ok(Undo::None)
        }
        #[cfg(not(feature = "quarry"))]
        State::Quarry => Err(String::from("quarry is not built (UNAOS_QUARRY=1)")),
        #[cfg(feature = "login")]
        State::Login => {
            if !super::crystal::login::shot_lock() {
                return Err(String::from("the lock screen refused (no session, or the session user has no password)"));
            }
            Ok(Undo::Unlock)
        }
        #[cfg(not(feature = "login"))]
        State::Login => Err(String::from("login is not built (UNAOS_LOGIN=1)")),
        #[cfg(all(target_arch = "x86_64", feature = "lumen"))]
        State::Lumen => {
            let fs = crate::fs::fat::mount_program_source().map_err(|_| String::from("no program volume"))?;
            let cap = crate::arch::syscall::user_image_cap();
            let de = fs.find_app("LUMEN.ELF").map_err(|_| String::from("LUMEN.ELF is not on the volume"))?;
            if de.size == 0 || de.size as usize > cap {
                return Err(String::from("LUMEN.ELF size out of range"));
            }
            let mut img = Vec::new();
            fs.read_file(&de, &mut img, cap).map_err(|_| String::from("LUMEN.ELF read failed"))?;
            let (pid, slot, _entry) = crate::arch::syscall::spawn_user_image_bg(&img).map_err(|e| format!("lumen spawn refused ({})", e))?;
            sleep_ms(500); // the window is the program's to mint; settle measures the rest
            Ok(Undo::Kill(pid, slot))
        }
        #[cfg(not(all(target_arch = "x86_64", feature = "lumen")))]
        State::Lumen => Err(String::from("lumen is not built on this arch (x86_64 + UNAOS_LUMEN)")),
    }
}

fn undo(u: Undo) {
    match u {
        Undo::None => {}
        #[cfg(feature = "login")]
        Undo::Unlock => {
            let _ = super::crystal::login::shot_release();
        }
        #[cfg(all(target_arch = "x86_64", feature = "lumen"))]
        Undo::Kill(pid, slot) => {
            let _ = crate::arch::syscall::bg_kill(pid, slot);
        }
    }
}

// ── THE MASK FILE ────────────────────────────────────────────────────────────────────────────────────

/// Encode the mask (white masked, black scored) at the panel's size and write it through the mount table.
/// Returns the bytes written.
fn write_mask(path: &str, dir: &str, m: &[MaskRect], pw: usize, ph: usize) -> Result<usize, String> {
    let mt = crate::shell::vfs_mount_table();
    let p = crate::fs::vfs::KERNEL_PRINCIPAL;
    let _ = mt.create(dir, crate::fs::vfs::NodeKind::Dir, p);
    let _ = mt.unlink(path, p);
    mt.create(path, crate::fs::vfs::NodeKind::File, p).map_err(|e| format!("create {}: {:?}", path, e))?;
    let mut enc = PngEncoder::new(pw as u32, ph as u32).map_err(|e| format!("encoder {:?}", e))?;
    let mut row: Vec<u8> = Vec::new();
    if row.try_reserve_exact(pw * 3).is_err() {
        return Err(String::from("out of memory"));
    }
    let mut piece: Vec<u8> = Vec::new();
    let mut at = 0usize;
    let flush = |enc: &mut PngEncoder, piece: &mut Vec<u8>, at: &mut usize| -> Result<(), String> {
        while enc.next_piece(piece) {
            let n = mt.write(path, *at as u64, piece, p).map_err(|e| format!("write {}: {:?}", path, e))?;
            if n != piece.len() {
                return Err(format!("short write {} of {}", n, piece.len()));
            }
            *at += n;
        }
        Ok(())
    };
    for y in 0..ph {
        row.clear();
        for x in 0..pw {
            let v = if masked(m, x, y) { 0xFF } else { 0x00 };
            row.extend_from_slice(&[v, v, v]);
        }
        enc.push_row(&row).map_err(|e| format!("encode {:?}", e))?;
        flush(&mut enc, &mut piece, &mut at)?;
    }
    enc.finish().map_err(|e| format!("finish {:?}", e))?;
    flush(&mut enc, &mut piece, &mut at)?;
    if !enc.verified() {
        return Err(String::from("mask encode did not verify"));
    }
    let _ = crate::fs::filetype::stamp_as_in(&mt, path, crate::fs::filetype::IMAGE_PNG);
    Ok(at)
}

// ── THE SHOT ─────────────────────────────────────────────────────────────────────────────────────────

/// What one state shot produced.
pub struct Report {
    pub state: String,
    pub file: String,
    pub mask_file: String,
    pub mask: Vec<MaskRect>,
    pub pw: usize,
    pub ph: usize,
    pub settled: bool,
}

/// Compose, settle, capture, mask, undo. Prints the `:: SHOT:` witness on every outcome.
pub fn run(s: State) -> Result<Report, String> {
    let name = label(s);
    let r = run_inner(s, &name);
    match &r {
        Ok(_) => {}
        Err(e) => serial_println!(":: SHOT: state={} reason={} -> FAIL ::", name, e),
    }
    r
}

fn run_inner(s: State, name: &str) -> Result<Report, String> {
    // The refusal a capture would give, before anything is opened: no session, no shot.
    let home = super::prtscr::session_home().map_err(|r| r.sentence())?;
    let info = super::panel_info_nonblocking().ok_or_else(|| String::from("panel busy or absent"))?;
    let (pw, ph) = (info.width, info.height);
    let u = compose(s)?;
    let pre = mask_table(s, pw, ph);
    let (settled, samples, settle_ms) = settle(&pre);
    // Re-read the table at the capture (the cursor may have moved, a window placed itself): the union is masked.
    let mut mask = mask_table(s, pw, ph);
    for r in pre {
        if !mask.iter().any(|q| q.kind == r.kind && q.x == r.x && q.y == r.y && q.w == r.w && q.h == r.h) {
            mask.push(r);
        }
    }
    let st = stem(s);
    let shot = super::prtscr::capture_named(SHOTS_DIR, &format!("{}.PNG", st));
    undo(u);
    let shot = shot.map_err(|r| {
        r.report();
        r.sentence()
    })?;
    shot.report_ok();
    let dir = format!("{}/{}", home, SHOTS_DIR);
    let file = format!("{}/{}.PNG", dir, st);
    let mask_file = format!("{}/{}.MSK", dir, st);
    let mbytes = write_mask(&mask_file, &dir, &mask, pw, ph);
    serial_println!(
        ":: SHOT: state={} file={} bytes={} settle={} samples={} settle_ms={} mask={} mask_file={} mask_bytes={} -> {} ::",
        name,
        file,
        shot.bytes,
        if settled { "ok" } else { "timeout" },
        samples,
        settle_ms,
        mask_wire(&mask),
        mask_file,
        match &mbytes { Ok(n) => format!("{}", n), Err(e) => format!("0 ({})", e) },
        if mbytes.is_ok() { "OK" } else { "FAIL" }
    );
    mbytes?;
    Ok(Report { state: String::from(name), file, mask_file, mask, pw, ph, settled })
}

/// The `shot <state>` verb body (both arches' `shot` arms reach it).
pub fn verb(args: &[&str], console: &mut Console) {
    let s = match parse(args) {
        Ok(s) => s,
        Err(e) => {
            console.println_styled(super::theme::TERM_RED, &e);
            return;
        }
    };
    console.println(&format!("shot: composing {} (settle <= {} ms), then capturing…", label(s), SETTLE_MAX_MS));
    match run(s) {
        Ok(r) => console.println(&format!(
            "shot: {} -> {} + {} ({}; masked: {})",
            r.state,
            r.file,
            r.mask_file,
            if r.settled { "settled" } else { "did NOT settle" },
            mask_kinds(&r.mask)
        )),
        Err(e) => console.println_styled(super::theme::TERM_RED, &format!("shot: {}: {}", label(s), e)),
    }
}

// ── `tests shot` ─────────────────────────────────────────────────────────────────────────────────────

/// Register `tests shot` once (called from `tests::shell_verb`).
pub fn ensure_tests() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("shot", selftest);
    }
}

/// `tests shot` — shoot `desktop`, decode the PNG back with the kernel's own decoder (`facet::decode_file`), decode
/// the mask and check it covers the clock's rectangle and leaves the panel's middle scored.
pub fn selftest() {
    let r = match run(State::Desktop) {
        Ok(r) => r,
        Err(e) => {
            let skip = e.contains("refused") || e.contains("nobody is logged in") || e.contains("READ-ONLY");
            serial_println!(":: GLASSEYES: reason={} ::", e);
            serial_println!(":: GLASSEYES: states={} shot=desktop png=none mask=none -> {} ::", STATES.len(), if skip { "SKIP" } else { "FAIL" });
            return;
        }
    };
    let kinds = mask_kinds(&r.mask);
    let clock = r.mask.iter().find(|m| m.kind == "clock").copied();
    let (png, cover) = verify(&r, clock);
    let pass = png == "ok" && cover && clock.is_some();
    if !pass {
        serial_println!(":: GLASSEYES: reason=png={} clock_rect={} mask_covers_clock={} ::", png, clock.is_some(), cover);
    }
    serial_println!(
        ":: GLASSEYES: states={} shot=desktop png={} mask={} -> {} ::",
        STATES.len(),
        png,
        kinds,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Decode both files; `(png verdict, mask covers the clock)`.
#[cfg(feature = "facet")]
fn verify(r: &Report, clock: Option<MaskRect>) -> (&'static str, bool) {
    // A quarter-size decode (box-reduced by facet's integer `k`): every filter, the zlib stream and the colour path
    // run over the whole file, at a sixteenth of the memory of a native decode.
    let (bw, bh) = ((r.pw / 4).max(1), (r.ph / 4).max(1));
    let png = match super::facet::decode_file(&r.file, u64::MAX, bw, bh) {
        Ok((_, _, _, w, h)) if w as usize == r.pw && h as usize == r.ph => "ok",
        Ok(_) => "geometry",
        Err(_) => "decode-refused",
    };
    let Some(c) = clock else { return (png, false) };
    let cover = match super::facet::decode_file(&r.mask_file, u64::MAX, bw, bh) {
        Ok((px, ow, oh, w, _)) => {
            let k = (w as usize / ow.max(1)).max(1);
            let at = |x: usize, y: usize| -> Option<u32> { px.get((y / k).min(oh - 1) * ow + (x / k).min(ow - 1)).copied() };
            let white = |v: Option<u32>| v.is_some_and(|p| (p >> 16) & 0xFF >= 0x80);
            // The clock's centre and its four inset corners are white; the panel's middle (outside every mask) is black.
            let (cx, cy) = (c.x + c.w / 2, c.y + c.h / 2);
            let inset = |a: usize, l: usize| a + (l / 4).min(l.saturating_sub(1));
            let corners = [(inset(c.x, c.w), inset(c.y, c.h)), (c.x + c.w - 1 - (c.w / 4).min(c.w - 1), inset(c.y, c.h)), (inset(c.x, c.w), c.y + c.h - 1 - (c.h / 4).min(c.h - 1)), (cx, cy)];
            let mid = (r.pw / 2, r.ph / 2);
            corners.iter().all(|&(x, y)| white(at(x, y))) && (masked(&r.mask, mid.0, mid.1) || !white(at(mid.0, mid.1)))
        }
        Err(_) => false,
    };
    (png, cover)
}

/// Without the decoder (`facet` off) the PNG is checked by its header only, and the mask's coverage is not
/// decodable — the verdict says `png=hdr`, never `ok`.
#[cfg(not(feature = "facet"))]
fn verify(r: &Report, _clock: Option<MaskRect>) -> (&'static str, bool) {
    let mt = crate::shell::vfs_mount_table();
    let ok = mt.read(&r.file, 0, 24).is_ok_and(|h| {
        h.len() == 24
            && h[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
            && &h[12..16] == b"IHDR"
            && u32::from_be_bytes([h[16], h[17], h[18], h[19]]) as usize == r.pw
            && u32::from_be_bytes([h[20], h[21], h[22], h[23]]) as usize == r.ph
    });
    (if ok { "hdr" } else { "bad-header" }, false)
}
