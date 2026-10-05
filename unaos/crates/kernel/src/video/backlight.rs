// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver
//!
//! BRIGHTFLOOR (rmbp-ledger B312, FLIGHT 19 §2) — THE ONE BACKLIGHT WRITER. Both the Settings slider and
//! the brightness keys (BRIGHTKEYS) set the panel through [`set_level`], which clamps to the floor,
//! scales to the panel's measured range, writes the register, READS IT BACK and says so:
//!
//! `[backlight] level=<l> reg=<raw> readback=<rb|-> max=<m> on=<0|1> driver=<gmux|sim> via=<who>`
//!
//! # What went dark on flight 19
//!
//! The register is the Apple gmux backlight (`GMUX_PORT_BRIGHTNESS`, index 0x74; the Kepler drives the
//! panel, the gmux owns its backlight — no Intel `BLC_PWM`, no `pp_control`). The old writer put the
//! value bytes at 0x7C0 instead of the gmux data port 0x7C2, so the register got 0 for EVERY level, and
//! it scaled 16 to 0xFFFF against a panel range of 0x3FF. The gmux's 0 is "backlight off". Settings
//! persisted the level, the login re-applied it, the next session came up dark.
//!
//! # The floor
//!
//! Levels are `FLOOR..=STEPS` (1..16). 0 is never written by this function: OFF is the idle blank's
//! (DIMIDLE paints the surface black and never touches the backlight). `raw = max * level / STEPS`, `max`
//! read once from gmux index 0x70 (0x3FF on the bench rMBP; [`FALLBACK_MAX`] when the read fails), so
//! level 1 is 1/16 of the panel's range, lit.
//!
//! # Boards without the gmux
//!
//! No `gmux_igd` (the Pi desktops, QEMU, a build without the knob): a simulated register with the gmux's
//! semantics (32-bit, 0 = off) stands in, so the level, the print and the fixture behave the same and
//! the line says `driver=sim`.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// Levels 0..=STEPS exist in the step rule; the lit range is `FLOOR..=STEPS`.
pub const STEPS: u8 = prefs_core::display::BRIGHTNESS_MAX as u8;
/// The lowest level [`set_level`] writes: 1/16 of the panel's range.
pub const FLOOR: u8 = prefs_core::display::BRIGHTNESS_MIN as u8;
/// The level assumed before the first write (the firmware's level is not read back at boot).
pub const DEFAULT_LEVEL: u8 = prefs_core::display::BRIGHTNESS_DEFAULT as u8;
/// The panel range used when the gmux does not answer index 0x70 (the bench rMBP's value).
pub const FALLBACK_MAX: u32 = 0x3FF;

static LEVEL: AtomicU8 = AtomicU8::new(DEFAULT_LEVEL);
/// The measured panel range (0 = not read yet).
static MAX: AtomicU32 = AtomicU32::new(0);
/// The simulated register (boards with no gmux, and the fixture's `sim` leg).
static SIM_REG: AtomicU32 = AtomicU32::new(0);

/// Clamp a level into the lit range. Pure.
pub const fn clamp(l: u8) -> u8 {
    if l < FLOOR { FLOOR } else if l > STEPS { STEPS } else { l }
}

/// Register value for a level over a panel range of `max`. Never 0 for a nonzero `max`. Pure.
pub const fn raw_for(level: u8, max: u32) -> u32 {
    let r = (max as u64 * clamp(level) as u64 / STEPS as u64) as u32;
    if r == 0 && max != 0 { 1 } else { r }
}

/// Percent shown beside the slider. Pure.
pub const fn percent(level: u8) -> u32 {
    clamp(level) as u32 * 100 / STEPS as u32
}

/// The current level (`FLOOR..=STEPS`).
pub fn level() -> u8 {
    LEVEL.load(Ordering::Relaxed)
}

/// Record a level WITHOUT touching the hardware (the key decoder's context: atomics only); the desktop
/// pass then calls [`set_level_via`]. Returns the clamped level.
pub fn stage(l: u8) -> u8 {
    let lv = clamp(l);
    LEVEL.store(lv, Ordering::Relaxed);
    HW_RAW.store(raw_for(lv, panel_range()), Ordering::Relaxed); // GLASSLAG M3: what the register WILL hold once the pass writes it
    lv
}

/// What one write did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Applied {
    pub level: u8,
    pub reg: u32,
    pub readback: Option<u32>,
    pub max: u32,
    pub on: bool,
    pub hw: bool,
}

/// The pure core: clamp, scale, write, read back. `on` = the readback is the value written and nonzero
/// (a write the hardware did not keep, or a 0, is NOT on).
pub fn apply_with(l: u8, max: u32, write: &mut dyn FnMut(u32) -> bool, read: &mut dyn FnMut() -> Option<u32>, hw: bool) -> Applied {
    let level = clamp(l);
    let reg = raw_for(level, max);
    let wrote = write(reg);
    let readback = if wrote { read() } else { None };
    let on = matches!(readback, Some(rb) if rb == reg && rb != 0);
    Applied { level, reg, readback, max, on, hw }
}

#[cfg(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb"))]
fn panel_max() -> (u32, bool) {
    let m = MAX.load(Ordering::Relaxed);
    if m != 0 { return (m, true); }
    let m = crate::drivers::gpu::igpu::gmux_max_brightness().unwrap_or(FALLBACK_MAX);
    MAX.store(m, Ordering::Relaxed);
    (m, true)
}
#[cfg(not(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb")))]
fn panel_max() -> (u32, bool) {
    MAX.store(FALLBACK_MAX, Ordering::Relaxed);
    (FALLBACK_MAX, false)
}

fn drive(l: u8) -> Applied {
    let (max, hw) = panel_max();
    #[cfg(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb"))]
    if hw {
        return apply_with(
            l,
            max,
            &mut |r| crate::drivers::gpu::igpu::gmux_set_brightness(r),
            &mut || crate::drivers::gpu::igpu::gmux_get_brightness(),
            true,
        );
    }
    let _ = hw;
    apply_with(l, max, &mut |r| { SIM_REG.store(r, Ordering::Relaxed); true }, &mut || Some(SIM_REG.load(Ordering::Relaxed)), false)
}

/// THE writer, named by the arc: clamp, write, read back, print. `via` = who asked (slider, keys,
/// login, fixture).
pub fn set_level_via(l: u8, via: &str) -> Applied {
    let a = drive(l);
    LEVEL.store(a.level, Ordering::Relaxed);
    HW_RAW.store(a.readback.unwrap_or(a.reg), Ordering::Relaxed); // GLASSLAG M3: the register's value is now known
    let mut rb = [0u8; 12];
    serial_println!(
        "[backlight] level={} reg={} readback={} max={} on={} driver={} via={}",
        a.level, a.reg, fmt_opt(a.readback, &mut rb), a.max, a.on as u8, if a.hw { "gmux" } else { "sim" }, via
    );
    a
}

/// [`set_level_via`] with no caller tag.
pub fn set_level(l: u8) -> Applied {
    set_level_via(l, "api")
}

fn fmt_opt(v: Option<u32>, buf: &mut [u8; 12]) -> &str {
    let Some(mut x) = v else { return "-" };
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 { break; }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("?")
}

/// Fixture `tests brightfloor` (R80: registered, never at boot). Set 0 → the floor and on=1; set 16 →
/// the full range; a stored 0 reloads clamped (an in-memory tree in Principia's format — the operator's
/// file is not touched); the safe-mode reset on a tree → defaults; the BRIGHTKEYS key path (moved here
/// from the boot). The prior level is restored.
pub fn selftest() {
    use prefs_core::{display, PrefTree, PrefValue};
    let prev = level();
    let a0 = set_level_via(0, "fixture");
    let set0_on = a0.level == FLOOR && a0.on && a0.reg != 0;
    let a16 = set_level_via(16, "fixture");
    let full = a16.level == STEPS && a16.on && a16.reg == a16.max;
    let _ = set_level_via(prev, "fixture-restore");
    // The pure rule over every level: never 0, monotone, top = max.
    let mut mono = true;
    let mut last = 0u32;
    for l in 0..=STEPS {
        let r = raw_for(l, FALLBACK_MAX);
        if r == 0 || r < last { mono = false; }
        last = r;
    }
    mono &= raw_for(STEPS, FALLBACK_MAX) == FALLBACK_MAX && clamp(0) == FLOOR;
    // A stored 0 → reload → clamped.
    let load_clamped = match PrefTree::parse("[system]\ndisplay.brightness = 0\n") {
        Ok(t) => {
            let stored = t.get("system", display::BRIGHTNESS_KEY).and_then(|v| v.as_int());
            stored == Some(0) && stored.map(display::clamp_brightness) == Some(FLOOR as i64)
                && crate::video::settings::load_brightness(stored) == (FLOOR, true)
        }
        Err(_) => false,
    };
    // The reset path, simulated on a tree.
    let reset_ok = match PrefTree::parse("[system]\ndisplay.brightness = 0\ndisplay.idle_min = 0\n") {
        Ok(mut t) => display::reset(&mut t) == 3
            && t.get("system", display::BRIGHTNESS_KEY) == Some(&PrefValue::Int(display::BRIGHTNESS_DEFAULT)),
        Err(_) => false,
    };
    crate::video::brightkeys::selftest();
    step_kat();
    let ok = set0_on && full && mono && load_clamped && reset_ok;
    serial_println!(
        ":: BRIGHTFLOOR: floor={} set0_on={} load_clamped={} reset_ok={} full={} mono={} driver={} -> {} ::",
        FLOOR, set0_on as u8, load_clamped as u8, reset_ok as u8, full as u8, mono as u8,
        if a0.hw { "gmux" } else { "sim" }, if ok { "PASS" } else { "FAIL" }
    );
}

// ── GLASSLAG M3 (rmbp-ledger B370) — THE FIRST STEP MOVES FROM THE PANEL'S OWN LEVEL ─────────────────────────────
// Flight 22, Peter in Settings: "lowering brightness at first made it more brite". [`LEVEL`] starts at
// [`DEFAULT_LEVEL`] (12/16) — "the firmware's level is not read back at boot" — and every step was taken from
// that ASSUMED level: with the firmware panel below 11/16, the first Down wrote 11/16, brighter than the
// glass. The floor/clamp order made it worse at the bottom: Down at the floor re-wrote the floor even when
// the panel sat BELOW it. Now:
//
// * the first desktop pass reads the register back ([`seed_from_hw`], gmux only — a board without the gmux
//   keeps the simulated default) and seeds [`LEVEL`] with the highest level whose register value does not
//   exceed the panel's ([`level_at_or_below`]), so the slider and the keys start where the glass is;
// * every step is judged against the register's KNOWN value ([`next_level`]): Down must write a strictly
//   lower value, Up a strictly higher one, or nothing is written (Down at a panel already at or below the
//   floor stays put — it never brightens).
//
// `[backlight] seed readback=<raw> max=<m> level=<l>` once; `tests brightfloor` adds the KAT line
// `:: BRIGHTSTEP: raws=<n> down_ok=<0|1> up_ok=<0|1> flight22=<0|1> -> PASS|FAIL ::` over every register value.

/// The register's value as last read, written or staged to be written (`u32::MAX` = not known yet).
static HW_RAW: AtomicU32 = AtomicU32::new(u32::MAX);
/// The one-shot seed has run.
static SEEDED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The highest lit level whose register value is `<= raw` over a panel range of `max`; [`FLOOR`] when even
/// the floor is above `raw`. Pure.
pub const fn level_at_or_below(raw: u32, max: u32) -> u8 {
    let mut l = STEPS;
    while l > FLOOR {
        if raw_for(l, max) <= raw { return l; }
        l -= 1;
    }
    FLOOR
}

/// One notch from `level` against a register that holds `cur_raw`: `Some(level to write)` only when it moves
/// the panel the asked way (Down strictly darker, Up strictly brighter); `None` = write nothing. Pure.
pub const fn next_level(level: u8, cur_raw: u32, max: u32, up: bool) -> Option<u8> {
    let notch = if up { if level >= STEPS { STEPS } else { level + 1 } } else if level <= FLOOR { FLOOR } else { level - 1 };
    let cand = clamp(notch);
    let r = raw_for(cand, max);
    if up { if r > cur_raw { Some(cand) } else { None } } else if r < cur_raw { Some(cand) } else { None }
}

/// The register's value as best known: the last readback/write, else the current level's value.
pub fn cur_raw() -> u32 {
    let r = HW_RAW.load(Ordering::Relaxed);
    if r != u32::MAX { return r; }
    raw_for(level(), panel_range())
}

/// The panel range as measured, or [`FALLBACK_MAX`] before the first read. No I/O.
pub fn panel_range() -> u32 {
    let m = MAX.load(Ordering::Relaxed);
    if m == 0 { FALLBACK_MAX } else { m }
}

/// The key path's step: `Some(level)` staged (no I/O — the decoder's context), `None` when the step would
/// move the panel the wrong way or not at all.
pub fn stage_step(up: bool) -> Option<u8> {
    next_level(level(), cur_raw(), panel_range(), up).map(stage)
}

/// The first desktop pass: read the panel's own level back, once, before anything has been written (gmux
/// boards only; the port I/O belongs on this pass, never in the key decoder).
pub fn seed_from_hw() {
    if SEEDED.swap(true, Ordering::AcqRel) || HW_RAW.load(Ordering::Relaxed) != u32::MAX { return; }
    #[cfg(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb"))]
    {
        let (max, hw) = panel_max();
        if !hw { return; }
        let Some(rb) = crate::drivers::gpu::igpu::gmux_get_brightness() else {
            serial_println!("[backlight] seed readback=- max={} level={} (gmux did not answer; steps from the default)", max, level());
            return;
        };
        if HW_RAW.compare_exchange(u32::MAX, rb, Ordering::AcqRel, Ordering::Relaxed).is_err() { return; }
        let l = level_at_or_below(rb, max);
        LEVEL.store(l, Ordering::Relaxed);
        serial_println!("[backlight] seed readback={} max={} level={} (GLASSLAG: the first step moves from the panel's own level)", rb, max, l);
    }
}

/// The KAT on the step table: over every register value `0..=max`, seeded at [`level_at_or_below`], Down
/// writes strictly lower or nothing, Up strictly higher or nothing, and nothing is refused that could move.
/// Plus flight 22's case: the panel at 6/16, the assumed level 12 — the OLD rule (`step(12)` = 11) brightened.
fn step_kat() {
    let max = FALLBACK_MAX;
    let (mut down_ok, mut up_ok) = (true, true);
    let mut raw = 0u32;
    while raw <= max {
        let l = level_at_or_below(raw, max);
        match next_level(l, raw, max, false) {
            Some(n) => down_ok &= raw_for(n, max) < raw,
            None => down_ok &= raw <= raw_for(FLOOR, max),
        }
        match next_level(l, raw, max, true) {
            Some(n) => up_ok &= raw_for(n, max) > raw,
            None => up_ok &= raw >= raw_for(STEPS, max),
        }
        raw += 1;
    }
    let fw = raw_for(6, max);
    let old_inverted = raw_for(DEFAULT_LEVEL - 1, max) > fw;
    let new_down = next_level(level_at_or_below(fw, max), fw, max, false).map(|n| raw_for(n, max) < fw) == Some(true);
    let flight22 = old_inverted && new_down;
    let ok = down_ok && up_ok && flight22;
    serial_println!(
        ":: BRIGHTSTEP: raws={} down_ok={} up_ok={} flight22={} seeded={} -> {} ::",
        max + 1, down_ok as u8, up_ok as u8, flight22 as u8, (HW_RAW.load(Ordering::Relaxed) != u32::MAX) as u8,
        if ok { "PASS" } else { "FAIL" }
    );
}
