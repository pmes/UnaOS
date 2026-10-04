//! AUDIO8 (rmbp-ledger B329) — the speaker amp is a HELD state, not a per-run bracket.
//! CHARTER: Stria — owed B289 (the HDA driver's output half a Stria fulfiller would call; no store of its own).
//!
//! Declared from the tail of `hda.rs` as `#[path = "hda_amp.rs"] pub mod amp;` (a CHILD of `hda`, so it reaches
//! `Rings`/`Path`/the register helpers without widening them). `hda-tone` only.
//!
//! FLIGHT20 (boot 20, docs/dev/evidence/rmbp-1005/AUDIO8.md): sound works and it POPS — every `tests hda` and
//! every `play` pulsed `GCTL.CRST` (a link reset is a codec reset: GPIO, pin controls and power fall back to
//! reset values), drove the Cirrus speaker-enable GPIO up (`[hda] gpio … data=0x08 speaker_bit=1`) and
//! restored it after the stream stopped (`[hda] gpio … restored data=0x00`). The class-D amp was switched
//! around every run.
//!
//! * M1 — the amp is raised by the FIRST run that plays (tone or stream) and held while any stream is open
//!   (`acquire`/`release`, one bit per owner) and for an idle hold-off after the last one closes
//!   ([`AMP_HOLDOFF_MS`], or Principia's `system`/`audio.amp_holdoff_ms`), then dropped ONCE from the
//!   device-service tick ([`tick`]). While held, `reset()` takes the warm path ([`warm`]: no CRST), the GPIO
//!   drive and the end-of-run codec restore are skipped. The firmware's GPIO trio and the member pins'
//!   pin control + EAPD are recorded at first touch and put back at shutdown/reboot ([`shutdown`]).
//!   One line per transition: `[hda] amp=<up|down> why=<first-play|idle|shutdown> …`; nothing per run.
//! * M2 — the DAC's own output amplifier is RAMPED over ~10 ms at stream start and stop ([`ramp_prep`],
//!   [`ramp`], [`ramp_svc`]) instead of stepping. The CS4206 DACs declare an out amp (`caps=0x000d041d`).
//! * M4 — [`witness`]: `:: AUDIO8: runs= plays= amp_up= amp_down= pops_bracketed= -> PASS|PENDING|FAIL ::`.
//!
//! Single service loop: `tests`, `play::service` and this module's tick run on the same kernel loop (the
//! tone is a blocking TSC spin inside it), so the tick never interleaves with a probe's rings.
use super::tone::*;
use super::*;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// The idle hold-off after the last stream closes, when Principia's store has no `audio.amp_holdoff_ms`.
pub const AMP_HOLDOFF_MS: u64 = 5000;
/// Principia key (namespace `system`, the store's audio keys' namespace: `audio.volume`, `audio.mute`).
pub const PREF_KEY: &str = "audio.amp_holdoff_ms";

/// Stream owners (bits of [`OPEN`]).
pub const TONE: u32 = 1 << 0;
pub const PLAY: u32 = 1 << 1;

static HELD: AtomicBool = AtomicBool::new(false);
static OPEN: AtomicU32 = AtomicU32::new(0);
static IDLE_SINCE: AtomicU64 = AtomicU64::new(0);
static UP_AT: AtomicU64 = AtomicU64::new(0);
static BASE: AtomicU64 = AtomicU64::new(0);
static CAD: AtomicU8 = AtomicU8::new(0);
/// The GPIO data word while the amp is up (the HDA-TONE witness reads it on held runs).
static GPIO_UP: AtomicU8 = AtomicU8::new(0);
/// Counters. `W_*` = the witness window (since the last `amp=up`), `B_*` = since boot.
static W_RUNS: AtomicU32 = AtomicU32::new(0);
static W_PLAYS: AtomicU32 = AtomicU32::new(0);
static W_UPS: AtomicU32 = AtomicU32::new(0);
static W_DOWNS: AtomicU32 = AtomicU32::new(0);
static W_BRACKET: AtomicU32 = AtomicU32::new(0);
static B_UPS: AtomicU32 = AtomicU32::new(0);
static B_DOWNS: AtomicU32 = AtomicU32::new(0);
static B_WARM: AtomicU32 = AtomicU32::new(0);
static RAMPS_UP: AtomicU32 = AtomicU32::new(0);
static RAMPS_DOWN: AtomicU32 = AtomicU32::new(0);

/// (afg, mask, data, dir, enable) as the FIRMWARE left them — recorded at the first raise, restored at drop.
static FW_GPIO: spin::Mutex<Option<(u8, u8, u8, u8, u8)>> = spin::Mutex::new(None);
const MAX_PINS: usize = 4;
/// Member pins touched this boot: (pin nid, firmware pin control, firmware EAPD word; 0xFFFF = unread).
static PINS: spin::Mutex<[(u8, u8, u16); MAX_PINS]> = spin::Mutex::new([(0, 0, 0xFFFF); MAX_PINS]);
/// M2 — the DACs the current stream ramps: (dac nid, target gain, has out amp). Set by [`ramp_prep`].
static RAMP: spin::Mutex<[(u8, u8, bool); PAIR_MAX]> = spin::Mutex::new([(0, 0, false); PAIR_MAX]);
/// DACs whose ramp capability line has been printed (bit = nid & 31).
static RAMP_SAID: AtomicU32 = AtomicU32::new(0);
/// The amp module's own CORB/RIRB pair (allocated once, re-pointed on every out-of-probe use).
static SVC: spin::Mutex<Option<(u64, u64)>> = spin::Mutex::new(None);

pub fn held() -> bool { HELD.load(Ordering::Acquire) }
pub(super) fn gpio_now() -> u8 { GPIO_UP.load(Ordering::Relaxed) }

fn holdoff_ms() -> u64 {
    crate::prefs::peek_int(PREF_KEY, 0, 600_000).map(|v| v as u64).unwrap_or(AMP_HOLDOFF_MS)
}

/// `reset()`'s first statement: while the amp is held on THIS controller and it is out of reset, the link is
/// NOT reset (a CRST is a codec reset — the pop). Returns the codec mask the cold reset found.
pub(super) fn warm(base: u64) -> Option<u16> {
    if !held() || BASE.load(Ordering::Relaxed) != base || r32(base, REG_GCTL) & GCTL_CRST == 0 {
        return None;
    }
    B_WARM.fetch_add(1, Ordering::Relaxed);
    Some(1u16 << (CAD.load(Ordering::Relaxed) & 0x0F))
}

/// `run_tone`, after the GPIO block and before any codec register is saved: the tone owns a stream now.
/// `gpio` is the block's pre-drive (afg, mask, data, dir, enable) when it drove the pins this run (taken, so
/// the end-of-run restore never sees it); `gpio_data` its readback after the drive.
pub(super) fn note(rings: &mut Rings, base: u64, cad: u8, paths: &[Path], gpio: Option<(u8, u8, u8, u8, u8)>, gpio_data: u8, a: &mut Audit) {
    // The member pins' firmware state, read before this run programs them (first touch only).
    {
        let mut pins = PINS.lock();
        for p in paths {
            if pins.iter().any(|e| e.0 == p.pin && e.0 != 0) { continue; }
            let Some(slot) = pins.iter_mut().find(|e| e.0 == 0) else { break };
            let pc = rings.cmd(cad, p.pin, VERB_GET_PIN_CONTROL, 0, a).map(|v| v as u8).unwrap_or(0);
            let ep = rings.cmd(cad, p.pin, VERB_GET_EAPD, 0, a).map(|v| (v & 0xFF) as u16).unwrap_or(0xFFFF);
            a.verbs_get += 2;
            *slot = (p.pin, pc, ep);
        }
    }
    OPEN.fetch_or(TONE, Ordering::AcqRel);
    if held() {
        if gpio.is_some() { W_BRACKET.fetch_add(1, Ordering::Relaxed); } // a GPIO write inside a held window
        W_RUNS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // The raise. The GPIO block above has driven the pins; record what the firmware had.
    let mut fw = FW_GPIO.lock();
    if fw.is_none() { *fw = gpio; }
    let was = fw.map(|g| g.2).unwrap_or(0);
    drop(fw);
    BASE.store(base, Ordering::Relaxed);
    CAD.store(cad, Ordering::Relaxed);
    GPIO_UP.store(gpio_data, Ordering::Relaxed);
    UP_AT.store(crate::arch::ms(), Ordering::Relaxed);
    for c in [&W_RUNS, &W_PLAYS, &W_DOWNS, &W_BRACKET] { c.store(0, Ordering::Relaxed); }
    W_UPS.store(1, Ordering::Relaxed);
    W_RUNS.store(1, Ordering::Relaxed);
    B_UPS.fetch_add(1, Ordering::Relaxed);
    HELD.store(true, Ordering::Release);
    let npins = PINS.lock().iter().filter(|e| e.0 != 0).count();
    serial_println!("[hda] amp=up why=first-play gpio={:#04x}->{:#04x} holdoff_ms={} pins={}", was, gpio_data, holdoff_ms(), npins);
}

/// `play::gate` took the stream `run_tone` opened: the owner becomes PLAY (the tone run returns early).
pub(super) fn to_play() {
    OPEN.fetch_or(PLAY, Ordering::AcqRel);
    OPEN.fetch_and(!TONE, Ordering::AcqRel);
    W_PLAYS.fetch_add(1, Ordering::Relaxed);
    W_RUNS.store(W_RUNS.load(Ordering::Relaxed).saturating_sub(1), Ordering::Relaxed); // the run was a play, not a tone
}

/// A stream closed. The last close starts the idle hold-off.
pub(super) fn amp_release(who: u32) {
    let prev = OPEN.fetch_and(!who, Ordering::AcqRel);
    if prev & !who == 0 && prev & who != 0 {
        IDLE_SINCE.store(crate::arch::ms(), Ordering::Relaxed);
    }
}

/// The device-service tick (`play::service`): drop a held amp once the hold-off has passed with no stream.
/// Idle cost: one atomic load.
pub fn tick() {
    if !held() || OPEN.load(Ordering::Acquire) != 0 { return; }
    let idle = crate::arch::ms().saturating_sub(IDLE_SINCE.load(Ordering::Relaxed));
    if idle < holdoff_ms() { return; }
    drop_amp("idle");
}

/// GPIO back to the firmware trio in the mirror order of the drive (data, direction, enable), one line.
fn drop_amp(why: &str) {
    let base = BASE.load(Ordering::Relaxed);
    let cad = CAD.load(Ordering::Relaxed);
    let mut a = Audit::default();
    let fw = *FW_GPIO.lock();
    let mut rd = 0xFFu8;
    if let (Some((fg, _mask, d0, dir0, en0)), Some(mut r)) = (fw, svc_rings(base, &mut a)) {
        let _ = r.cmd(cad, fg, VERB_SET_GPIO_DATA, d0 as u32, &mut a);
        let _ = r.cmd(cad, fg, VERB_SET_GPIO_DIRECTION, dir0 as u32, &mut a);
        let _ = r.cmd(cad, fg, VERB_SET_GPIO_ENABLE, en0 as u32, &mut a);
        rd = r.cmd(cad, fg, VERB_GET_GPIO_DATA, 0, &mut a).map(|v| v as u8).unwrap_or(0xFF);
        r.stop(&mut a);
    }
    HELD.store(false, Ordering::Release);
    W_DOWNS.fetch_add(1, Ordering::Relaxed);
    B_DOWNS.fetch_add(1, Ordering::Relaxed);
    let held_ms = crate::arch::ms().saturating_sub(UP_AT.load(Ordering::Relaxed));
    serial_println!("[hda] amp=down why={} held_ms={} gpio={:#04x}->{:#04x} readback={:#04x}",
        why, held_ms, GPIO_UP.load(Ordering::Relaxed), fw.map(|g| g.2).unwrap_or(0), rd);
}

/// Shutdown / reboot (`powerdown::stop`, before the streams are stopped): drop a held amp, then put the member
/// pins' pin control and EAPD back to what the firmware had. The ONLY place the widget path is restored.
pub fn shutdown() {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 { return; }
    if held() { drop_amp("shutdown"); }
    let cad = CAD.load(Ordering::Relaxed);
    let pins = *PINS.lock();
    let n = pins.iter().filter(|e| e.0 != 0).count();
    if n == 0 { return; }
    let mut a = Audit::default();
    let Some(mut r) = svc_rings(base, &mut a) else { return };
    for (pin, pc, ep) in pins.iter().copied().filter(|e| e.0 != 0) {
        if ep != 0xFFFF { let _ = r.cmd(cad, pin, VERB_SET_EAPD, ep as u32, &mut a); }
        let _ = r.cmd(cad, pin, VERB_SET_PIN_CONTROL, pc as u32, &mut a);
    }
    r.stop(&mut a);
    serial_println!("[hda] path restored why=shutdown pins={}", n);
}

// ── M2 — the DAC out-amp ramp ──────────────────────────────────────────────────────────────────────────
const RAMP_STEPS: u32 = 10;
const RAMP_STEP_US: u64 = 1000;

/// Before RUN: record each member DAC's current out-amp gain (what `stream::rearm` set) as the ramp target, then
/// set it to gain 0 muted. The converter is idle, so this write is inaudible. A DAC without an out amp
/// (PARAM_WIDGET_CAPS bit 2) is left alone and the GPIO hold is the whole cure for it.
pub(super) fn ramp_prep(rings: &mut Rings, cad: u8, paths: &[Path], a: &mut Audit) {
    let mut t = RAMP.lock();
    *t = [(0, 0, false); PAIR_MAX];
    for (m, p) in paths.iter().enumerate().take(PAIR_MAX) {
        let caps = rings.cmd(cad, p.dac, VERB_GET_PARAMETER, PARAM_WIDGET_CAPS, a).unwrap_or(0);
        let has = caps & WCAP_OUT_AMP != 0;
        let cur = rings.cmd16(cad, p.dac, VERB_GET_AMP_GAIN_MUTE, 0xA000, a).unwrap_or(0) as u8;
        a.verbs_get += 2;
        let target = cur & 0x7F;
        t[m] = (p.dac, target, has);
        let bit = 1u32 << (p.dac & 31);
        if RAMP_SAID.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
            let ac = rings.cmd(cad, p.dac, VERB_GET_PARAMETER, PARAM_OUT_AMP_CAPS, a).unwrap_or(0);
            serial_println!("[hda] ramp dac=0x{:02x} out_amp={} steps={} target={} step_us={} n={}{}", p.dac, has as u8,
                (ac >> 8) & 0x7F, target, RAMP_STEP_US, RAMP_STEPS, if has { "" } else { " (no out amp: the held GPIO is the cure)" });
        }
        if has && rings.cmd16(cad, p.dac, VERB_SET_AMP_GAIN_MUTE, amp_payload(true, 0, true, 0), a).is_some() {
            a.verbs_set += 1;
        }
    }
}

/// Ramp the prepared DACs up (0 -> target, unmuted) or down (target -> 0, then mute) in RAMP_STEPS steps. Down
/// also UNBINDS each prepared converter (stream/channel 0, what Linux `snd_hda_codec_cleanup_stream` does on
/// every close): with no per-run restore while the amp is held, a converter left on the tag would play in the
/// next run of another shape (`tests hda2` after `tests hda` would sound both DACs).
pub(super) fn ramp(rings: &mut Rings, up: bool, a: &mut Audit) {
    let cad = CAD.load(Ordering::Relaxed);
    let t = *RAMP.lock();
    if !t.iter().any(|e| e.2) { if !up { unbind(rings, cad, &t, a); } return; }
    for k in 1..=RAMP_STEPS {
        for &(dac, target, has) in t.iter() {
            if !has { continue; }
            let num = if up { k } else { RAMP_STEPS - k };
            let g = ((target as u32) * num / RAMP_STEPS) as u8;
            let mute = !up && k == RAMP_STEPS;
            if rings.cmd16(cad, dac, VERB_SET_AMP_GAIN_MUTE, amp_payload(true, 0, mute, g), a).is_some() { a.verbs_set += 1; }
        }
        delay_us(RAMP_STEP_US);
    }
    if up { RAMPS_UP.fetch_add(1, Ordering::Relaxed); } else { RAMPS_DOWN.fetch_add(1, Ordering::Relaxed); unbind(rings, cad, &t, a); }
}

fn unbind(rings: &mut Rings, cad: u8, t: &[(u8, u8, bool); PAIR_MAX], a: &mut Audit) {
    for &(dac, _, _) in t.iter().filter(|e| e.0 != 0) {
        if rings.cmd(cad, dac, VERB_SET_STREAM_CHANNEL, 0, a).is_some() { a.verbs_set += 1; }
    }
}

/// [`ramp`] from outside a probe (the player's RUN and STOP run on the service tick, with the probe's rings
/// stopped): through the amp module's own CORB/RIRB pair, stopped again afterwards.
pub(super) fn ramp_svc(up: bool) {
    if !RAMP.lock().iter().any(|e| e.0 != 0) { return; }
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 { return; }
    let mut a = Audit::default();
    if let Some(mut r) = svc_rings(base, &mut a) {
        ramp(&mut r, up, &mut a);
        r.stop(&mut a);
    }
}

/// The amp module's command rings: allocated by `Rings::init` on first use, then RE-POINTED at the same two
/// buffers ([HDA-SPEC §4.4.1.3, §4.4.2.2], the same sequence `Rings::init` writes, no allocation).
fn svc_rings(base: u64, a: &mut Audit) -> Option<Rings> {
    let mut g = SVC.lock();
    let Some((corb, rirb)) = *g else {
        let r = Rings::init(base, a)?;
        *g = Some((r.corb, r.rirb));
        return Some(r);
    };
    w8(base, REG_CORBCTL, 0);
    w8(base, REG_RIRBCTL, 0);
    if !wait_us(1000, || r8(base, REG_CORBCTL) & CORBCTL_RUN == 0) { return None; }
    w32(base, REG_CORBLBASE, (bus_addr(corb) & 0xFFFF_FFFF) as u32);
    w32(base, REG_CORBUBASE, (bus_addr(corb) >> 32) as u32);
    w16(base, REG_CORBRP, CORBRP_RST);
    let _ = wait_us(5000, || r16(base, REG_CORBRP) & CORBRP_RST != 0);
    w16(base, REG_CORBRP, 0);
    if !wait_us(5000, || r16(base, REG_CORBRP) & CORBRP_RST == 0) { return None; }
    w16(base, REG_CORBWP, 0);
    w32(base, REG_RIRBLBASE, (bus_addr(rirb) & 0xFFFF_FFFF) as u32);
    w32(base, REG_RIRBUBASE, (bus_addr(rirb) >> 32) as u32);
    w16(base, REG_RIRBWP, RIRBWP_RST);
    w16(base, REG_RINTCNT, 1);
    w8(base, REG_CORBCTL, CORBCTL_RUN);
    w8(base, REG_RIRBCTL, RIRBCTL_DMAEN | RIRBCTL_RINTCTL);
    a.ctrl += 12;
    if !wait_us(1000, || r8(base, REG_CORBCTL) & CORBCTL_RUN != 0) { return None; }
    Some(Rings { base, corb, rirb, corb_wp: 0, rirb_rp: 0, traced: true })
}

/// M4 — printed at the end of every tone run. The window is "since the last `amp=up`": three tone runs in it
/// with one up, no down and no GPIO write/restore inside it is the claim (no pop between runs).
pub(super) fn witness() {
    let (runs, plays) = (W_RUNS.load(Ordering::Relaxed), W_PLAYS.load(Ordering::Relaxed));
    let (ups, downs, br) = (W_UPS.load(Ordering::Relaxed), W_DOWNS.load(Ordering::Relaxed), W_BRACKET.load(Ordering::Relaxed));
    let clean = ups == 1 && downs == 0 && br == 0 && held();
    let v = if !clean { "FAIL" } else if runs >= 3 { "PASS" } else { "PENDING" };
    serial_println!(
        ":: AUDIO8: runs={} plays={} amp_up={} amp_down={} pops_bracketed={} -> {} :: boot_up={} boot_down={} warm={} holdoff_ms={} ramps={}/{} ::",
        runs, plays, ups, downs, br, v, B_UPS.load(Ordering::Relaxed), B_DOWNS.load(Ordering::Relaxed),
        B_WARM.load(Ordering::Relaxed), holdoff_ms(), RAMPS_UP.load(Ordering::Relaxed), RAMPS_DOWN.load(Ordering::Relaxed)
    );
}
