//! CHARTER: Stria — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! AUDIO (UAC1, rmbp-ledger B706) — THE ONE AUDIO SINK SEAM. Whatever the kernel plays goes to the sink the person
//! chose in Settings > General (the Sound box): `HDA internal` or `USB <vid:pid>`.
//!
//! Declared from `lib.rs` as `#[cfg(any(feature = "uvc", feature = "hda-tone"))] pub mod audio;` on the existing
//! `pub mod drivers;` line (LINE-NEUTRAL), so an image with neither audio path never lexes it.
//!
//! * [`sink`] chooses by name (`hda` / `internal` | `usb` / `usb:<vid:pid>`) and keeps the choice in Principia's store
//!   as `system.audio.output` (a preference is Principia's, never a dotfile); [`current`] answers it, USB only while
//!   a USB sink is registered (a speaker that left plays nothing — the internal one takes over).
//! * [`tone_on`] / [`tone`]: the HDA sink is reached by ONE call, `drivers::hda::hda_tone_test_default` (its own
//!   build-time 440 Hz / 1 s and its own `:: HDA-TONE ::` verdict); the USB sink is fed here, an integer sine written
//!   into the class driver's ring (`drivers::usb::uac::write`) at the rate the isochronous stream drains it.
//! * The USB sink registers itself when its stream starts ([`usb_arrived`]) and leaves with the device ([`usb_gone`]).
//! * UACMIC (rmbp-ledger B723) — THE ONE CAPTURE SEAM: [`source`]`(app)` opens the USB microphone (`uac` feature;
//!   `drivers::usb::uac`'s capture ring) and hands back a [`Source`] whose drop closes it. The kernel's recorder
//!   fixture names [`KERNEL_RECORDER`]; any other app is a GRANT — Principia's kept `system.decisions.microphone.<app>`
//!   (`fs::apptrust::MIC_DOMAIN`, REASK's store) or ONE ask, the camera's shape (`<app> wants the microphone`: Not now
//!   · Allow; Allow is kept, Not now is not) — and gets `Err("asked")` until the answer is kept. While any source is
//!   open the driver's capture pipe runs and the menu bar's mic dot is on (`video::menubar::micdot`).
//! * AUDIOMIX (rmbp-ledger B731): [`mixer`] is the sink's one producer — N sources (a ring-3 program's
//!   `SYS_AUDIO_*` stream, a kernel source), each PCM16/2/48k with its own gain, mixed per 10 ms period with the
//!   master volume and mute into the current sink (the USB ring, or the HDA output stream through `hda::play`'s door).

use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

pub mod mixer;

/// Where sound goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sink {
    Hda,
    Usb,
}

/// Principia key (namespace `system`): `"hda"` or `"usb"`.
pub const PREF_KEY: &str = crate::prefs::key::AUDIO_OUTPUT;

static USB_PRESENT: AtomicBool = AtomicBool::new(false);
static USB_ID: AtomicU32 = AtomicU32::new(0);
/// 0 = not read yet, 1 = HDA, 2 = USB.
static CHOSEN: AtomicU8 = AtomicU8::new(0);

/// The HDA tone path is in this image.
pub fn hda_built() -> bool {
    cfg!(all(target_arch = "x86_64", feature = "hda-tone"))
}

/// A USB audio stream started (`uac::probe`).
pub fn usb_arrived(vid: u16, pid: u16) {
    USB_ID.store((vid as u32) << 16 | pid as u32, Ordering::Relaxed);
    USB_PRESENT.store(true, Ordering::Release);
    serial_println!("[audio] sink usb:{:04x}:{:04x} registered current={}", vid, pid, label(current()));
}

/// The USB device left.
pub fn usb_gone() {
    USB_PRESENT.store(false, Ordering::Release);
    serial_println!("[audio] sink usb gone current={}", label(current()));
}

fn chosen() -> u8 {
    match CHOSEN.load(Ordering::Acquire) {
        0 => {
            let v = match crate::prefs::text(PREF_KEY) {
                Some(s) if s.starts_with("usb") => 2,
                Some(_) => 1, None => return 1, // CRAWLC: nothing read (the store not loaded yet, or the key unset) is answered, never cached — a kept `usb` read later still counts (BP-audio-2)
            };
            CHOSEN.store(v, Ordering::Release);
            v
        }
        v => v,
    }
}

/// The sink sound goes to now.
pub fn current() -> Sink {
    let usb = USB_PRESENT.load(Ordering::Acquire);
    if usb && (chosen() == 2 || !hda_built()) {
        Sink::Usb
    } else {
        Sink::Hda
    }
}

/// The sink's name as Settings shows it.
pub fn label(s: Sink) -> String {
    match s {
        Sink::Hda => String::from("HDA internal"),
        Sink::Usb => {
            if USB_PRESENT.load(Ordering::Acquire) {
                let v = USB_ID.load(Ordering::Relaxed);
                alloc::format!("USB {:04x}:{:04x}", v >> 16, v & 0xFFFF)
            } else {
                String::from("USB (none)")
            }
        }
    }
}

/// Choose the sink by name and keep the choice in Principia's store.
pub fn sink(name: &str) -> Result<Sink, &'static str> {
    let s = match name {
        "hda" | "internal" => Sink::Hda,
        n if n == "usb" || n.starts_with("usb:") => Sink::Usb,
        _ => return Err("unknown-sink"),
    };
    if s == Sink::Hda && !hda_built() {
        serial_println!("[audio] sink={} REFUSED reason=hda-tone-not-built", name);
        return Err("hda-tone-not-built");
    }
    if s == Sink::Usb && !USB_PRESENT.load(Ordering::Acquire) {
        serial_println!("[audio] sink={} REFUSED reason=no-usb-audio-device", name);
        return Err("no-usb-audio-device");
    }
    CHOSEN.store(if s == Sink::Usb { 2 } else { 1 }, Ordering::Release);
    crate::prefs::set_sys(PREF_KEY, crate::prefs::PrefValue::Str(String::from(if s == Sink::Usb { "usb" } else { "hda" })));
    serial_println!("[audio] sink={} chosen", label(s));
    Ok(s)
}

/// Settings > General's picker: (left label, right label, the selected segment).
pub fn picker() -> (String, String, usize) {
    (label(Sink::Hda), label(Sink::Usb), (current() == Sink::Usb) as usize)
}

/// A tone on the current sink.
pub fn tone(hz: u32, ms: u32) -> Result<(u64, u32), &'static str> {
    tone_on(current(), hz, ms)
}

/// A tone on `s`: Ok((frames the USB stream played, underruns)); the HDA sink answers (0, 0) and prints its own verdict.
pub fn tone_on(s: Sink, hz: u32, ms: u32) -> Result<(u64, u32), &'static str> {
    serial_println!("[audio] tone sink={} hz={} ms={}", label(s), hz, ms);
    match s {
        Sink::Hda => hda_tone(),
        Sink::Usb => usb_tone(hz, ms),
    }
}

#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn hda_tone() -> Result<(u64, u32), &'static str> {
    crate::drivers::hda::hda_tone_test_default(); // THE ONE CALL: the HDA sink is the HDA tone path, unchanged
    Ok((0, 0))
}

#[cfg(not(all(target_arch = "x86_64", feature = "hda-tone")))]
fn hda_tone() -> Result<(u64, u32), &'static str> {
    Err("hda-tone-not-built")
}

#[cfg(not(feature = "uac"))]
fn usb_tone(_hz: u32, _ms: u32) -> Result<(u64, u32), &'static str> {
    Err("usb-audio-not-built")
}

/// Q15 sine of `phase` in 1/1024 turns: a 5th-order odd polynomial on the first quadrant, mirrored (no float).
/// AUDIOMIX (B731): also the mixer's test tones (`mixer::selftest`), so it is built with either path.
fn sin_q15(phase: u64) -> i32 {
    const Q: u64 = 256;
    let p = phase % (4 * Q);
    let (quad, off) = (p / Q, p % Q);
    let x = if quad == 0 || quad == 2 { off } else { Q - off };
    let t = (x as i64 * 32768) / Q as i64;
    let t2 = (t * t) >> 15;
    let t3 = (t2 * t) >> 15;
    let t5 = (t3 * t2) >> 15;
    let s = ((51472 * t - 21166 * t3 + 2608 * t5) >> 15).clamp(-32768, 32767) as i32;
    if quad >= 2 { -s } else { s }
}

#[cfg(feature = "uac")]
pub(crate) fn nap(ms: u64) {
    #[cfg(target_arch = "x86_64")]
    crate::arch::sched::sleep_ms(ms);
    #[cfg(not(target_arch = "x86_64"))]
    {
        let t0 = crate::arch::ms();
        while crate::arch::ms().wrapping_sub(t0) < ms {
            core::hint::spin_loop();
        }
    }
}

/// The USB sink: `ms` of an `hz` sine at -18 dBFS (4096 of 32767, the HDA flown level) with 20 ms fades, written into
/// the class driver's ring as it drains, then the drain awaited.
#[cfg(feature = "uac")]
fn usb_tone(hz: u32, ms: u32) -> Result<(u64, u32), &'static str> {
    use crate::drivers::usb::uac;
    const AMP: i32 = 4096;
    const CHUNK: usize = 240; // frames = 5 ms
    let rate = uac::RATE as u64;
    let total = (rate * ms as u64 / 1000) as usize;
    let fade = (rate / 50) as usize;
    let (f0, u0) = uac::counters();
    uac::begin()?;
    let mut chunk = [0u8; CHUNK * 4];
    let (mut f, mut last) = (0usize, crate::arch::ms());
    while f < total {
        let n = (total - f).min(CHUNK);
        for i in 0..n {
            let fr = f + i;
            let env = if fr < fade { (fr * 32768 / fade) as i32 } else if fr + fade >= total { ((total - 1 - fr) * 32768 / fade) as i32 } else { 32768 };
            let s = ((((sin_q15(fr as u64 * hz as u64 * 1024 / rate) * AMP) >> 15) * env) >> 15) as i16;
            let b = s.to_le_bytes();
            chunk[i * 4..i * 4 + 4].copy_from_slice(&[b[0], b[1], b[0], b[1]]);
        }
        let mut off = 0usize;
        while off < n * 4 {
            let w = uac::write(&chunk[off..n * 4]);
            off += w;
            if w != 0 {
                last = crate::arch::ms();
            } else if !uac::streaming() || crate::arch::ms().wrapping_sub(last) > 500 {
                uac::finish();
                serial_println!("[audio] tone sink=usb ABORT reason=stream-stalled written_frames={} of {}", f + off / 4, total);
                return Err("stream-stalled");
            } else {
                nap(2);
            }
        }
        f += n;
    }
    uac::finish();
    let t0 = crate::arch::ms();
    while uac::queued() > 0 && crate::arch::ms().wrapping_sub(t0) < 2000 {
        nap(2);
    }
    let (f1, u1) = uac::counters();
    Ok((f1.wrapping_sub(f0), u1.wrapping_sub(u0)))
}

// ── UACMIC (rmbp-ledger B723): THE ONE CAPTURE SEAM ──────────────────────────────────────────────────────────────────

/// The kernel's own recorder (the `tests uacmic` fixture): the only caller that is not asked.
#[cfg(feature = "uac")]
pub const KERNEL_RECORDER: &str = "kernel";

/// An open capture source: PCM 16-bit little-endian, `format()` channels at `format()` Hz. Dropping it closes it.
#[cfg(feature = "uac")]
pub struct Source {
    app: String,
    ch: u8,
    rate: u32,
}

#[cfg(feature = "uac")]
impl Source {
    /// Whole frames captured since the last read (0 = none yet: the pipe opens on the next service pass).
    pub fn read(&mut self, dst: &mut [u8]) -> usize {
        crate::drivers::usb::uac::read_capture(dst)
    }

    /// `(channels, rate)`.
    pub fn format(&self) -> (u8, u32) {
        (self.ch, self.rate)
    }
}

#[cfg(feature = "uac")]
impl Drop for Source {
    fn drop(&mut self) {
        crate::drivers::usb::uac::capture_close();
        serial_println!("[audio] source app={} closed", self.app);
    }
}

/// The microphone grant for `app`: `"kernel"` (the recorder), `"kept"` (Principia holds Allow) or `"asked"` (no kept
/// answer: the ask is raised when `ask`, else only said — the fixture's quiet leg).
#[cfg(feature = "uac")]
pub fn mic_grant(app: &str, ask: bool) -> &'static str {
    use crate::fs::apptrust::MIC_DOMAIN;
    if app == KERNEL_RECORDER {
        return "kernel";
    }
    if crate::prefs::decided(MIC_DOMAIN, app, "-").as_deref() == Some(MIC_ALLOW) {
        return "kept";
    }
    let posted = if ask { mic_ask(app) } else { false };
    serial_println!("[mic] ask app={} domain={} posted={}{}", app, MIC_DOMAIN, posted as u8, if ask { "" } else { " (quiet)" });
    "asked"
}

/// THE capture seam: open the microphone for `app` (see the module head). `Err("asked")` = no kept grant yet.
#[cfg(feature = "uac")]
pub fn source(app: &str) -> Result<Source, &'static str> {
    open(app, true)
}

/// [`source`] with the ask raised or only said (`ask = false`: the fixture's grant leg).
#[cfg(feature = "uac")]
pub(crate) fn open(app: &str, ask: bool) -> Result<Source, &'static str> {
    let g = mic_grant(app, ask);
    if g == "asked" {
        return Err("asked");
    }
    let (ch, rate) = crate::drivers::usb::uac::capture_open()?;
    serial_println!("[audio] source app={} grant={} fmt=pcm16/{}/{} opened", app, g, ch, rate);
    Ok(Source { app: String::from(app), ch, rate })
}

/// The kept answer's word.
#[cfg(feature = "uac")]
const MIC_ALLOW: &str = "allow";

#[cfg(all(feature = "uac", target_arch = "x86_64", feature = "wc"))]
static MIC_ASKING: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None);

/// The ask on the glass — the camera's (`<app> wants the microphone`, Not now · Allow; Allow the default, rightmost;
/// Escape and the close box are Not now). QUEUE-ONLY (`dialog::post`); one ask at a time; never before the desktop.
#[cfg(all(feature = "uac", target_arch = "x86_64", feature = "wc"))]
fn mic_ask(app: &str) -> bool {
    use crate::video::dialog::{self, Act, Dlg, Icon};
    if crate::boot::phase() != crate::boot::Phase::Desktop {
        return false;
    }
    let mut g = MIC_ASKING.lock();
    if g.is_some() {
        return false; // one ask up already; the app asks again after it is answered
    }
    let msg = alloc::format!("{} wants the microphone", app);
    let mut d = Dlg::new(Icon::Caution, b"Microphone", msg.as_bytes(), b"The microphone dot in the menu bar shows while it is on.
Settings > Privacy revokes it.", &[b"Not now", b"Allow"]);
    d.act = Act::Hook(mic_answer as fn(bool) as usize);
    let posted = dialog::post(d);
    if posted {
        *g = Some(String::from(app));
    }
    posted
}

#[cfg(all(feature = "uac", not(all(target_arch = "x86_64", feature = "wc"))))]
fn mic_ask(_app: &str) -> bool {
    false // no glass to ask on: the app stays refused
}

/// The dialog's hook (press/key route; queue-only): Allow is KEPT in Principia (`prefs::decide`); Not now is not.
#[cfg(all(feature = "uac", target_arch = "x86_64", feature = "wc"))]
fn mic_answer(ok: bool) {
    let Some(app) = MIC_ASKING.lock().take() else { return };
    let allow = ok && crate::video::dialog::last_button() == 1;
    if allow {
        crate::prefs::decide(crate::fs::apptrust::MIC_DOMAIN, &app, "-", MIC_ALLOW);
    }
    serial_println!("[mic] answer={} app={}", if allow { MIC_ALLOW } else { "not-now" }, app);
}
