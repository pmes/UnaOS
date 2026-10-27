//! The per-boot half of WIFI10's pass: the PHY, MAC and DMA one-shots that run ONCE per boot.
//!
//! CHARTER: Kernel — driver (the d11's HT-PHY TX side, its MAC and its TX/RX DMA engines; no CODEX §2 handler owns a
//! radio register).
//!
//! PHYONCEFN (rmbp-ledger B950) — the steps `bringup::wifi10()` ran inline, moved here verbatim and in their order:
//! the MAC re-suspended (`rx::suspend`), the HT-PHY's TX-side sequencing (`htphyinit::tx_prep`), WIFI11's TX power
//! control (`htphyinit::tx_pwr_ctl` from the SPROM's PA block), the channel cookie (`rx::chan_cookie`), the TX ring
//! and its SHM / filter words (`tx::prep`), and S6's receiver armed (`rx::arm`). These are the one-shots STATIONREJOIN
//! (B891) never re-runs: the per-association half is `join::associate`, and `wifi10()`'s `RAN` latch keeps this fn
//! to one call per boot. Every line it prints is the one `wifi10()` printed, in the same place on the wire.

use super::{htphyinit, ident, rx, tx, w32, D11_SHM_CONTROL};

/// What the per-boot one-shots leave for the rest of `wifi10()`'s pass: the probe, `join::run`/`link`/`keep` and the end line.
pub(super) struct PhyState {
    pub(super) suspended: bool,
    pub(super) h: htphyinit::Ht,
    pub(super) tp: htphyinit::TxPrep,
    pub(super) pctl: Option<htphyinit::TxPctl>,
    pub(super) pctl_on: bool,
    pub(super) cookie_pre: u16,
    pub(super) prep: tx::Prep,
    pub(super) rxr: rx::Rx,
    pub(super) w: rx::Writes,
}

/// From `wifi10()`, inside its once-per-boot latch, after its gates and the SHM_CONTROL pre-image `shm_pre`. A failure
/// prints its own `:: wifi10: tx NOT ATTEMPTED …` line, records `set_wifi10(3, 0)` and restores `shm_pre` before it
/// returns; the `Err` names the step (`tx-prep`, `rx-arm`).
pub(super) fn once(bar0: u64, bus: u8, dev: u8, func: u8, mac: [u8; 6], ch: u16, shm_pre: u32) -> Result<PhyState, &'static str> {
    let mut w = rx::Writes::default();
    // 1. The MAC suspended (wifi9's end left it so; re-asserted, main.c:2987-3017), then the PHY's TX side.
    let suspended = rx::suspend(bar0, &mut w, "wifi10");
    let mut h = htphyinit::Ht::new(bar0);
    let tp = htphyinit::tx_prep(&mut h);
    // WIFI11 (B653): HT-PHY TX power control from the SPROM's per-core PA coefficients — the half WIFI10
    // designed and did not build (phy_ht.c:1001-1004, after tx_prep's :999-1000). Refused, witnessed, on an
    // unread / implausible / blank SPROM block: the frame then leaves at WIFI10's fixed index.
    let pctl = match ident::pwr_2g() {
        Ok((rev, p)) => {
            ident::pwr_line(rev, &p);
            match p.iter().position(ident::pwr_blank) {
                Some(c) => {
                    serial_println!(
                        ":: wifi11: txpctl on=0 target_idx=- sprom=blank(core{} rev={}) -> NOT-SET reason=unprogrammed-pa-block — b43 would drive control from it; the frame leaves at txpctl_cmd=[{:#06x},{:#06x},{:#06x}] ::",
                        c, rev, tp.txpctl[0], tp.txpctl[1], tp.txpctl[2]
                    );
                    None
                }
                None => {
                    let pin = [0usize, 1, 2].map(|c| htphyinit::PwrIn {
                        target: p[c].maxpwr_2g, a1: p[c].pa_2g[0] as i16, b0: p[c].pa_2g[1] as i16, b1: p[c].pa_2g[2] as i16,
                    });
                    let r = htphyinit::tx_pwr_ctl(&mut h, &pin);
                    serial_println!(
                        ":: wifi11: txpctl on={} target_idx={:#06x} status=[{:#06x},{:#06x},{:#06x}] targ_pwr=[{},{},{}]dBm/4 idle_tssi=[{},{},{}] tone={} sprom=ok(rev={}) -> {} ::",
                        r.on as u8, r.status[0], r.status[0], r.status[1], r.status[2], pin[0].target, pin[1].target, pin[2].target,
                        r.idle[0], r.idle[1], r.idle[2], if r.tone_ok { "ran" } else { "TIMEOUT" }, rev,
                        if r.on { "SET" } else { "NOT-SET reason=cmd-en-bits-unset" }
                    );
                    Some(r)
                }
            }
        }
        Err(why) => {
            serial_println!(":: wifi11: txpctl on=0 target_idx=- sprom={} -> NOT-SET reason=sprom-{} — the frame leaves at txpctl_cmd=[{:#06x},{:#06x},{:#06x}] ::", why, why, tp.txpctl[0], tp.txpctl[1], tp.txpctl[2]);
            None
        }
    };
    let pctl_on = pctl.as_ref().map_or(false, |r| r.on);
    let cookie_pre = rx::chan_cookie(bar0, ch, &mut w);
    // 2. The TX ring, the rate memory, the PHY-TX-control words, the station MAC into the filter.
    let prep = match tx::prep(bar0, mac, &mut w) {
        Ok(p) => p,
        Err(why) => {
            super::super::status::set_wifi10(3, 0);
            unsafe { w32(bar0, D11_SHM_CONTROL, shm_pre) };
            serial_println!(":: wifi10: tx NOT ATTEMPTED reason={} ::", why);
            return Err("tx-prep");
        }
    };
    // 3. S6's receiver armed — the MAC enabled, the ucode awake.
    let rxr = match rx::arm(bar0, bus, dev, func, &mut w, "wifi10") {
        Ok(r) => r,
        Err(why) => {
            super::super::status::set_wifi10(3, 0);
            let _ = tx::stop(bar0, &mut w);
            unsafe { w32(bar0, D11_SHM_CONTROL, shm_pre) };
            serial_println!(":: wifi10: tx NOT ATTEMPTED reason=rx-{} ::", why);
            return Err("rx-arm");
        }
    };
    Ok(PhyState { suspended, h, tp, pctl, pctl_on, cookie_pre, prep, rxr, w })
}

// ── PHYONCE-TESTS (rmbp-ledger B954): `tests phy` parks `once`'s PhyState here; `wifi10()` takes it ──────────────────────

/// The PhyState `tests phy` left, with the SHM_CONTROL pre-image `wifi10()` restores at its end. Filled at most once per
/// boot (`bringup::PHY_RAN` keeps `once` to one call), emptied by `take`. The lock is held for the put, the take and the
/// check only; the suspend, the drain and the resume (they poll and settle) run outside it.
static PARK: crate::sync::Mutex<Option<(PhyState, u32)>> = crate::sync::Mutex::new(None);

/// From `bringup::phy_tests` after `once` returned Ok: the MAC suspended again (`rx::suspend` — an enabled ucode ACKs unicast
/// to the station MAC that `tx::prep` put in MACFILTER_SELF, and the 64-slot ring fills with no reader), the frames the armed
/// ring took meanwhile drained into a scratch `Seen` (wifi10's probe window counts only its own), SHM_CONTROL restored and
/// read back, the state parked. `(mac_suspended, drained_frames, shm_restored)`.
pub(super) fn park(bar0: u64, mut p: PhyState, ch: u16, shm_pre: u32) -> (bool, u32, bool) {
    let mac_off = rx::suspend(bar0, &mut p.w, "wifi10-phy");
    let mut scratch = rx::Seen::default();
    rx::drain(bar0, &mut p.rxr, &mut scratch, ch, &mut p.w);
    unsafe { w32(bar0, D11_SHM_CONTROL, shm_pre) };
    let shm_ok = unsafe { super::r32(bar0, D11_SHM_CONTROL) } == shm_pre;
    *PARK.lock() = Some((p, shm_pre));
    (mac_off, scratch.frames, shm_ok)
}

/// `wifi10()`'s latch check: a parked PhyState lets its pass run once more, from the probe.
pub(super) fn parked() -> bool {
    PARK.lock().is_some()
}

/// From `wifi10()` in place of `once`: the parked state and its pre-image, the MAC enabled again (`rx::resume`, arm's
/// enable as one fn) right before the probe, as `once` left it. `None` when `tests phy` parked nothing.
pub(super) fn take(bar0: u64) -> Option<(PhyState, u32)> {
    let mut got = PARK.lock().take()?;
    rx::resume(bar0, &mut got.0.w);
    serial_println!("[wifi10] phy taken from tests phy (PHYONCE-TESTS B954) -> mac resumed; the pass goes on from the probe");
    Some(got)
}
