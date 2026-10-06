// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver
//!
//! LIDSLEEP (rmbp-ledger B431, MACPARITY row 34) — the lid read and the backlight-only sleep. Design and rung
//! ledger: `docs/dev/evidence/rmbp-1005/lidsleep.md` (DRIVERS-METHOD §6).
//!
//! * **Rung 0 (the premise, READ-ONLY):** with `lidsleep` (UNAOS_LIDSLEEP=1, implies `smc`, x86) the SMC key
//!   `MSLD` is read once at the first desktop pass and every 1 s after, through `smc::read_key` (READ_CMD only —
//!   nothing is written to the SMC). Printed at boot and on change: `[smc] lid=<open|closed> raw=<byte> …`.
//!   `nonzero = closed` is the HYPOTHESIS rung 0 tests; the raw byte is always printed beside it.
//! * **The ladder, written and UNAPPLIED:** lid closed → backlight off (gmux 0x74 := 0, the register BRIGHTSLIDER
//!   drives), the Kepler's display engine left running, input ignored; lid open → restore the prior raw.
//!   [`LADDER_ARMED`] is false until rung 0 is confirmed on metal; each lid change prints what it WOULD do.
//! * **[`sleep_request`]** (LOGINWINDOW's Sleep button): blanks the backlight only and says so; the next key or
//!   pointer event restores it ([`service`] watches `dimidle`'s activity clock). S3 is NOT this arc.
//! * **`tests lidsleep`** (R80: registered, never at boot): `:: LIDSLEEP: msld=… flips=… backlight_off=… ::`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Rung 2 (lid → blank) is applied only after rung 0 is confirmed on metal. Reopens: a flight's `[smc] lid=` flips.
pub const LADDER_ARMED: bool = false;
/// Rung 0's poll period (ms).
pub const POLL_MS: u64 = 1000;

/// MSLD's last raw byte (`u32::MAX` = never read).
static LID_RAW: AtomicU32 = AtomicU32::new(u32::MAX);
/// Lid state changes seen since boot.
static FLIPS: AtomicU32 = AtomicU32::new(0);
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(0);
/// 0 = polling, 1 = key absent (stop), 2 = stuck seen once (keep polling, print once).
static POLL_STATE: AtomicU32 = AtomicU32::new(0);
/// A [`sleep_request`] blank is in force.
static SLEPT: AtomicBool = AtomicBool::new(false);
static SLEPT_AT_MS: AtomicU64 = AtomicU64::new(0);
/// The register's value before the blank (restored on wake).
static PRIOR_RAW: AtomicU32 = AtomicU32::new(0);

/// `nonzero = closed` — rung 0's hypothesis (polarity unpinned, decided by the glass). Pure.
pub const fn lid_closed(raw: u8) -> bool { raw != 0 }

fn lid_word(raw: u8) -> &'static str { if lid_closed(raw) { "closed" } else { "open" } }

/// What one MSLD read answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Msld {
    Ok(u8, usize),
    NoKey,
    Stuck(u8),
    Unbuilt,
}

impl Msld {
    pub fn word(self) -> &'static str {
        match self { Msld::Ok(..) => "ok", Msld::NoKey => "no-key", Msld::Stuck(_) => "stuck", Msld::Unbuilt => "unbuilt" }
    }
}

/// One read-only MSLD read (1 byte: the first value byte of the key, whatever its length).
pub fn read_msld() -> Msld {
    #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))]
    {
        use crate::drivers::smc;
        let mut b = [0u8; 1];
        return match smc::read_key(b"MSLD", &mut b) {
            Ok(n) => Msld::Ok(b[0], n),
            Err(smc::SmcError::Absent) => Msld::NoKey,
            Err(smc::SmcError::Stuck(s)) => Msld::Stuck(s),
        };
    }
    #[allow(unreachable_code)]
    Msld::Unbuilt
}

/// Lid changes seen since boot.
pub fn flips() -> u32 { FLIPS.load(Ordering::Relaxed) }

/// A sleep blank is in force.
pub fn asleep() -> bool { SLEPT.load(Ordering::Relaxed) }

/// One service pass (called from `dimidle::service`): rung 0's 1 s MSLD poll, and the wake from a [`sleep_request`].
pub fn service() {
    let now = crate::arch::ms();
    if SLEPT.load(Ordering::Acquire) {
        let act = crate::video::dimidle::last_activity_ms();
        if act > SLEPT_AT_MS.load(Ordering::Relaxed) { wake("input"); }
    }
    if POLL_STATE.load(Ordering::Relaxed) == 1 { return; }
    let last = LAST_POLL_MS.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) < POLL_MS { return; }
    LAST_POLL_MS.store(now.max(1), Ordering::Relaxed);
    poll(now);
}

fn poll(now: u64) {
    match read_msld() {
        Msld::Ok(raw, n) => {
            let prev = LID_RAW.swap(raw as u32, Ordering::AcqRel);
            if prev == u32::MAX {
                serial_println!("[smc] lid={} raw={} len={} at=boot key=MSLD (rung 0: nonzero=closed is the hypothesis)", lid_word(raw), raw, n);
            } else if prev != raw as u32 {
                let f = FLIPS.fetch_add(1, Ordering::Relaxed) + 1;
                serial_println!("[smc] lid={} raw={} was={} flips={} ms={}", lid_word(raw), raw, prev, f, now);
                if (prev != 0) != (raw != 0) {
                    let would = if lid_closed(raw) { "backlight-off" } else { "restore" };
                    if LADDER_ARMED {
                        if lid_closed(raw) { let _ = sleep_request("lid"); } else if asleep() { wake("lid"); }
                    } else {
                        serial_println!("[lidsleep] ladder unapplied lid={} would={} armed=0 (rung 0 open)", lid_word(raw), would);
                    }
                }
            }
        }
        Msld::NoKey => {
            POLL_STATE.store(1, Ordering::Relaxed);
            serial_println!("[smc] lid=no-key key=MSLD (rung 0 refuted for MSLD; poll stopped)");
        }
        Msld::Stuck(s) => {
            if POLL_STATE.swap(2, Ordering::Relaxed) != 2 {
                serial_println!("[smc] lid=stuck key=MSLD step={} (bounded; poll continues, printed once)", s);
            }
        }
        Msld::Unbuilt => POLL_STATE.store(1, Ordering::Relaxed),
    }
}

/// What a blank did: `ok` (hardware readback 0), `unarmed` (no gmux on this build), `fail` (readback not 0).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Blank {
    pub prior: Option<u32>,
    pub readback: Option<u32>,
    pub hw: bool,
}

impl Blank {
    pub fn word(&self) -> &'static str {
        if !self.hw { "unarmed" } else if self.readback == Some(0) { "ok" } else { "fail" }
    }
}

fn opt(v: Option<u32>) -> alloc::string::String {
    match v { Some(x) => alloc::format!("{}", x), None => alloc::string::String::from("-") }
}

/// LOGINWINDOW's Sleep (and the lid ladder once armed): the backlight goes off, nothing else. The display engine keeps
/// running; the next key or pointer event restores the prior level. S3 is not this arc, and the line says so.
pub fn sleep_request(via: &str) -> Blank {
    let prior = super::backlight::readback_now().or(Some(super::backlight::cur_raw()));
    let (_, readback, hw) = super::backlight::off_via(via);
    let b = Blank { prior, readback, hw };
    if !SLEPT.load(Ordering::Relaxed) { PRIOR_RAW.store(prior.unwrap_or(0), Ordering::Relaxed); }
    SLEPT_AT_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
    SLEPT.store(true, Ordering::Release);
    serial_println!(
        "[lidsleep] sleep_request via={} backlight_off={} prior={} readback={} engine=running s3=not-this-arc wake=input",
        via, b.word(), opt(prior), opt(readback)
    );
    b
}

/// Restore the level the blank took. Returns the readback.
pub fn wake(via: &str) -> Option<u32> {
    if !SLEPT.swap(false, Ordering::AcqRel) { return None; }
    let prior = PRIOR_RAW.load(Ordering::Relaxed);
    let a = super::backlight::set_raw_via(prior, "lidsleep-wake");
    serial_println!("[lidsleep] wake via={} restored={} readback={}", via, prior, opt(a.readback));
    a.readback
}

/// `tests lidsleep` (R80: registered, never at boot). One MSLD read; the blank and its restore (the panel blinks dark).
/// Expected shape (the capture and the gmux's own semantics, not this build's output): msld=ok (MSLD is in this SMC's
/// key index, f13 idx 255), backlight_off=ok (index 0x74 reads back 0 after the 0) and the restore reads back the prior.
pub fn selftest() {
    let m = read_msld();
    let raw = match m { Msld::Ok(r, _) => alloc::format!("{}", r), _ => alloc::string::String::from("-") };
    let b = sleep_request("tests");
    let restore_rb = wake("tests");
    let restored = match (b.hw, b.prior) { (true, Some(p)) => restore_rb == Some(p), _ => true };
    let verdict = match (m, b.word()) {
        (Msld::Ok(..), "ok") if restored => "PASS",
        (Msld::Ok(..) | Msld::Unbuilt, "unarmed") => "SKIP",
        _ => "FAIL",
    };
    serial_println!(
        ":: LIDSLEEP: msld={} raw={} flips={} backlight_off={} prior={} off_rb={} restore_rb={} armed={} -> {} ::",
        m.word(), raw, flips(), b.word(), opt(b.prior), opt(b.readback), opt(restore_rb), LADDER_ARMED as u8, verdict
    );
}
