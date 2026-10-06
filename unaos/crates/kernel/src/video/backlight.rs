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
    step_raw(cur_raw(), panel_range(), up).map(stage) // BRIGHTSLIDER M1 (B377): the keys move along the slider's own linear scale, from the register's value
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

// ── BRIGHTSLIDER M1 (rmbp-ledger B377, R89) — ONE LINEAR SCALE, THE REGISTER IS THE TRUTH ────────────────────────
// Flight 23: the settings slider was quantised to 16 steps and painted from its own copy of the level, and the
// login wrote the stored step over the panel's own (`seed readback=160` then `reg=127 via=login`). Now the slider
// maps its track LINEARLY onto the register range ([`raw_for_pos`] / [`pos_for_raw`]), a press writes that register
// value through [`set_raw_via`] (floor-clamped, read back, the same `[backlight] level=` line), the keys step to the
// next 1/16 grid point strictly above/below the register ([`step_raw`]) — the same scale — and the login only READS
// ([`login_keep`]). `LEVEL` stays the level at or below the register (Principia's 1..16 key, the bar's indicator).

/// A register value clamped into the lit range `raw_for(FLOOR)..=max`. Pure.
pub const fn clamp_raw(raw: u32, max: u32) -> u32 {
    let f = raw_for(FLOOR, max);
    if raw < f { f } else if raw > max { max } else { raw }
}

/// Register value for slider position `pos` of `span` (linear, rounded, floor-clamped). Pure.
pub const fn raw_for_pos(pos: usize, span: usize, max: u32) -> u32 {
    if span == 0 { return max; }
    let p = if pos > span { span } else { pos };
    clamp_raw(((max as u64 * p as u64 + span as u64 / 2) / span as u64) as u32, max)
}

/// Slider position (of `span`) that shows register value `raw` (linear, rounded). Pure.
pub const fn pos_for_raw(raw: u32, max: u32, span: usize) -> usize {
    if max == 0 { return 0; }
    let r = if raw > max { max } else { raw };
    ((r as u64 * span as u64 + max as u64 / 2) / max as u64) as usize
}

/// Percent of the panel's range a register value is. Pure.
pub const fn pct_of(raw: u32, max: u32) -> u32 {
    pos_for_raw(raw, max, 100) as u32
}

/// One key notch from a register holding `cur`: the lowest grid level strictly above it (Up) or the highest
/// strictly below it (Down); `None` = nothing to move to (at the top, or at/below the floor). Pure.
pub const fn step_raw(cur: u32, max: u32, up: bool) -> Option<u8> {
    if up {
        let mut l = FLOOR;
        while l <= STEPS {
            if raw_for(l, max) > cur { return Some(l); }
            l += 1;
        }
        None
    } else {
        let mut l = STEPS;
        loop {
            if raw_for(l, max) < cur { return Some(l); }
            if l <= FLOOR { return None; }
            l -= 1;
        }
    }
}

/// The panel range, measured on first use (gmux index 0x70) — the I/O belongs on a desktop pass.
pub fn max_now() -> u32 {
    panel_max().0
}

fn drive_raw(raw: u32) -> Applied {
    let (max, hw) = panel_max();
    let reg = clamp_raw(raw, max);
    let level = level_at_or_below(reg, max);
    #[cfg(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb"))]
    if hw {
        let wrote = crate::drivers::gpu::igpu::gmux_set_brightness(reg);
        let readback = if wrote { crate::drivers::gpu::igpu::gmux_get_brightness() } else { None };
        let on = matches!(readback, Some(rb) if rb == reg && rb != 0);
        return Applied { level, reg, readback, max, on, hw: true };
    }
    let _ = hw;
    SIM_REG.store(reg, Ordering::Relaxed);
    let rb = SIM_REG.load(Ordering::Relaxed);
    Applied { level, reg, readback: Some(rb), max, on: rb == reg && rb != 0, hw: false }
}

/// THE writer for a register value (the slider's linear scale): clamp to the floor, write, read back, print the
/// same `[backlight] level=` line as [`set_level_via`]. `LEVEL` = the level at or below what the register holds.
pub fn set_raw_via(raw: u32, via: &str) -> Applied {
    let a = drive_raw(raw);
    let held = a.readback.unwrap_or(a.reg);
    LEVEL.store(level_at_or_below(held, a.max), Ordering::Relaxed);
    HW_RAW.store(held, Ordering::Relaxed);
    let mut rb = [0u8; 12];
    serial_println!(
        "[backlight] level={} reg={} readback={} max={} on={} driver={} via={}",
        a.level, a.reg, fmt_opt(a.readback, &mut rb), a.max, a.on as u8, if a.hw { "gmux" } else { "sim" }, via
    );
    a
}

/// Read the register now (gmux boards; a simulated register answers only once something wrote it). Updates the
/// known value and `LEVEL`. `None` = no answer.
pub fn readback_now() -> Option<u32> {
    let (max, hw) = panel_max();
    let rb = {
        #[cfg(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb"))]
        { if hw { crate::drivers::gpu::igpu::gmux_get_brightness() } else { None } }
        #[cfg(not(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb")))]
        { let _ = hw; match SIM_REG.load(Ordering::Relaxed) { 0 => None, r => Some(r) } }
    }?;
    HW_RAW.store(rb, Ordering::Relaxed);
    LEVEL.store(level_at_or_below(rb, max), Ordering::Relaxed);
    Some(rb)
}

/// BRIGHTSLIDER M3 — the login: read the panel, seed the slider from it, WRITE NOTHING. The one exception is a
/// panel the register says is below the floor (dark): it gets the floor, and the line says `wrote=1`.
/// `[backlight] login keep readback=<n|-> max=<m> slider=<pct>% stored=<l|-> wrote=<0|1>`. Returns `LEVEL`.
pub fn login_keep(stored: Option<u8>) -> u8 {
    let max = max_now();
    let rb = readback_now();
    let wrote = matches!(rb, Some(r) if r < raw_for(FLOOR, max));
    if wrote { let _ = set_raw_via(raw_for(FLOOR, max), "login-floor"); }
    let (mut b1, mut b2) = ([0u8; 12], [0u8; 12]);
    serial_println!(
        "[backlight] login keep readback={} max={} slider={}% stored={} wrote={}",
        fmt_opt(rb, &mut b1), max, pct_of(cur_raw(), max), fmt_opt(stored.map(|s| s as u32), &mut b2), wrote as u8
    );
    level()
}

/// BRIGHTSLIDER M4 — fixture `tests brightstep` (R80: registered, never at boot): five slider positions
/// (10/30/50/70/90 % of the track) written through [`set_raw_via`], each read back, the settings knob checked
/// against the readback (`settings::slider_sync`), the prior register restored; plus the key scale's KAT over
/// every register value (Up = the lowest grid point strictly above, Down = the highest strictly below).
/// `:: BRIGHTSTEP: steps=5 readback_ok=<n> slider_sync=<ok|bad> keys=<ok|bad> driver=<gmux|sim> -> PASS|FAIL ::`
pub fn brightstep() {
    let max = max_now();
    let prev = cur_raw();
    let (mut rb_ok, mut sync_ok, mut hw) = (0u32, 0u32, false);
    const POS: [usize; 5] = [10, 30, 50, 70, 90];
    for p in POS {
        let a = set_raw_via(raw_for_pos(p, 100, max), "brightstep");
        hw = a.hw;
        if a.readback == Some(a.reg) { rb_ok += 1; }
        if a.readback.is_some_and(crate::video::settings::slider_sync) { sync_ok += 1; }
    }
    let _ = set_raw_via(prev, "brightstep-restore");
    let _ = crate::video::settings::slider_sync(cur_raw());
    let mut keys = true;
    let mut raw = 0u32;
    while raw <= max {
        keys &= match step_raw(raw, max, true) {
            Some(l) => raw_for(l, max) > raw && (l == FLOOR || raw_for(l - 1, max) <= raw),
            None => raw >= max,
        };
        keys &= match step_raw(raw, max, false) {
            Some(l) => raw_for(l, max) < raw && (l == STEPS || raw_for(l + 1, max) >= raw),
            None => raw <= raw_for(FLOOR, max),
        };
        raw += 1;
    }
    let n = POS.len() as u32;
    let ok = rb_ok == n && sync_ok == n && keys;
    serial_println!(
        ":: BRIGHTSTEP: steps={} readback_ok={} slider_sync={} keys={} driver={} -> {} ::",
        n, rb_ok, if sync_ok == n { "ok" } else { "bad" }, if keys { "ok" } else { "bad" },
        if hw { "gmux" } else { "sim" }, if ok { "PASS" } else { "FAIL" }
    );
}

/// LIDSLEEP (B431) — the backlight OFF: gmux index 0x74 := 0 (upstream apple-gmux: 0 = off; BRIGHTFLOOR flight 19 saw
/// it dark), read back. The one writer of a 0, and only for a sleep: `LEVEL` and the known register value are left as
/// they were so the wake restores them through [`set_raw_via`]. Returns `(wrote, readback, hw)`.
/// `[backlight] off reg=0 readback=<rb|-> driver=<gmux|sim> via=<who>`
pub fn off_via(via: &str) -> (bool, Option<u32>, bool) {
    let (_, hw) = panel_max();
    #[allow(unused_mut)]
    let mut r = (false, None, false);
    #[cfg(all(target_arch = "x86_64", feature = "gmux_igd", feature = "intel-ivb"))]
    if hw {
        let wrote = crate::drivers::gpu::igpu::gmux_set_brightness(0);
        r = (wrote, if wrote { crate::drivers::gpu::igpu::gmux_get_brightness() } else { None }, true);
    }
    if !r.2 {
        let _ = hw;
        SIM_REG.store(0, Ordering::Relaxed);
        r = (true, Some(0), false);
    }
    let mut rb = [0u8; 12];
    serial_println!("[backlight] off reg=0 readback={} driver={} via={}", fmt_opt(r.1, &mut rb), if r.2 { "gmux" } else { "sim" }, via);
    r
}
