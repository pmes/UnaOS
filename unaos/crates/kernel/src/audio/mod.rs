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
    Usb, #[cfg(all(target_arch = "x86_64", feature = "btc"))] Bt, // A2DP (rmbp-ledger B739): the paired headset, `drivers/bt/avdtp.rs`'s sink (LINE-NEUTRAL fold, code first)
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
                Some(s) if s.starts_with("usb") => 2, #[cfg(all(target_arch = "x86_64", feature = "btc"))] Some(s) if s.starts_with("bt") => 3, // A2DP (B739): `bt` (LINE-NEUTRAL fold, code first)
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
    let usb = USB_PRESENT.load(Ordering::Acquire); #[cfg(all(target_arch = "x86_64", feature = "btc"))] if chosen() == 3 && bt_ready() { return Sink::Bt; } // A2DP (B739): a `bt` choice holds while a stream runs (LINE-NEUTRAL fold, code first)
    if usb && (chosen() == 2 || !hda_built()) {
        Sink::Usb
    } else {
        Sink::Hda
    }
}

/// The sink's name as Settings shows it.
pub fn label(s: Sink) -> String {
    match s {
        Sink::Hda => String::from("HDA internal"), #[cfg(all(target_arch = "x86_64", feature = "btc"))] Sink::Bt => bt_label(), // A2DP (B739) (LINE-NEUTRAL fold, code first)
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
        n if n == "usb" || n.starts_with("usb:") => Sink::Usb, #[cfg(all(target_arch = "x86_64", feature = "btc"))] n if n == "bt" || n.starts_with("bt:") => return bt_choose(n), // A2DP (B739): `bt` / `bt:<addr>` (LINE-NEUTRAL fold, code first)
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
        Sink::Usb => usb_tone(hz, ms), #[cfg(all(target_arch = "x86_64", feature = "btc"))] Sink::Bt => bt_tone(hz, ms), // A2DP (B739) (LINE-NEUTRAL fold, code first)
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

#[cfg(any(feature = "uac", all(target_arch = "x86_64", feature = "btc")))]
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
        crate::drivers::usb::uac::capture_close(); ksrc_closed(); // SOFTYMIC (B749): the kernel source leaves `mic_holder`'s count (file tail). LINE-NEUTRAL fold, code first.
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
    let (g, ch, rate) = grant_open(app, ask)?;
    serial_println!("[audio] source app={} grant={} fmt=pcm16/{}/{} opened", app, g, ch, rate);
    ksrc_opened(app); Ok(Source { app: String::from(app), ch, rate }) // SOFTYMIC (B749): the kernel source's count and name for `mic_holder` (file tail). LINE-NEUTRAL fold, code first.
}

/// The grant, then the driver's open: `Ok((grant, channels, rate))` with the wanted count raised (the caller owes
/// one `uac::capture_close`). Shared by [`open`] (a `Source`) and the ring-3 table ([`bus_source`], MICRING3).
#[cfg(feature = "uac")]
fn grant_open(app: &str, ask: bool) -> Result<(&'static str, u8, u32), &'static str> {
    let g = mic_grant(app, ask);
    if g == "asked" {
        return Err("asked");
    }
    let (ch, rate) = crate::drivers::usb::uac::capture_open()?;
    Ok((g, ch, rate))
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

// ── MICRING3 (rmbp-ledger B736, BP-audio-12): the ring-3 way in — one source per program over the one seam ─────────────
// The bus verbs `una_abi::BUS_VERB_AUDIO_SOURCE_{OPEN,READ,CLOSE}` (40..42) reach [`bus_source`] from the x86 bus
// dispatch arm. The program is named by the launcher (`wm::app_name_of(slot + 1)`, else `argv[0]` on the slot's
// kernel-written args page — NETGRANT's rule), never by the caller; its grant is [`mic_grant`]'s, the camera's ask. The
// open source is ONE per-slot word, [`PROG_SRC`] (the handle, 0 = none): every close — the CLOSE verb, Settings >
// Privacy's Revoke ([`source_revoked`], through `fs::apptrust::mic_revoked`), the program's end ([`slot_gone`], from
// `sched::term_release_edge`) — is a swap of that word to 0 followed by ONE `uac::capture_close`, so exactly one of
// them closes it. The driver's service pass then sees the wanted count fall and closes the pipe (the dot goes out).
// The slot tables are TYPE-bounded (`procslot::SLOT_ID_MAX`, the APPFAULT shape), not a pool; nothing here allocates
// on the terminate path (IF=0).

/// Slots a ring-3 source can belong to (the slot's type bound).
#[cfg(feature = "uac")]
const PROG_SLOTS: usize = crate::procslot::SLOT_ID_MAX;
/// Per slot: the open source's handle (0 = none). Swapped to 0 by whichever close comes first.
#[cfg(feature = "uac")]
static PROG_SRC: [AtomicU32; PROG_SLOTS] = [const { AtomicU32::new(0) }; PROG_SLOTS];
/// Per slot: the handle a revoke closed — READ answers -EPERM on it until CLOSE or the next OPEN.
#[cfg(feature = "uac")]
static PROG_REVOKED: [AtomicU32; PROG_SLOTS] = [const { AtomicU32::new(0) }; PROG_SLOTS];
/// `(slot, program)` for each slot that opened a source (the revoke's match and the wire). A row whose source closed
/// stays until that slot's next open replaces it: it names nothing open ([`PROG_SRC`] is the fact).
#[cfg(feature = "uac")]
static PROG_NAME: crate::sync::Mutex<alloc::vec::Vec<(usize, String)>> = crate::sync::Mutex::new(alloc::vec::Vec::new());
/// The last handle given (handles are never 0).
#[cfg(feature = "uac")]
static PROG_HSEQ: AtomicU32 = AtomicU32::new(0);
/// `tests micring3`'s counts: asks posted, opens, PCM bytes read, closes by revoke / by terminate / by the verb,
/// opens refused for want of a microphone.
#[cfg(feature = "uac")]
static R3_COUNTS: [core::sync::atomic::AtomicU64; 7] = [const { core::sync::atomic::AtomicU64::new(0) }; 7];
#[cfg(feature = "uac")]
const R3_ASKED: usize = 0;
#[cfg(feature = "uac")]
const R3_OPENED: usize = 1;
#[cfg(feature = "uac")]
const R3_READ: usize = 2;
#[cfg(feature = "uac")]
const R3_REVOKED: usize = 3;
#[cfg(feature = "uac")]
const R3_TERM: usize = 4;
#[cfg(feature = "uac")]
const R3_CLOSED: usize = 5;
#[cfg(feature = "uac")]
const R3_NODEV: usize = 6;

#[cfg(feature = "uac")]
fn r3_add(i: usize, n: u64) {
    R3_COUNTS[i].fetch_add(n, Ordering::Relaxed);
}

/// `[asked, opened, read_bytes, revoked, terminated, closed, nodev]` since boot (`tests micring3` reads deltas).
#[cfg(feature = "uac")]
pub fn ring3_counts() -> [u64; 7] {
    core::array::from_fn(|i| R3_COUNTS[i].load(Ordering::Relaxed))
}

/// Slot `s`'s open handle (0 = none).
#[cfg(feature = "uac")]
pub fn ring3_open(s: usize) -> u32 {
    if s < PROG_SLOTS { PROG_SRC[s].load(Ordering::Acquire) } else { 0 }
}

/// THE close: slot `s`'s word to 0, then the driver's count down once. Returns the handle closed (0 = nothing open).
#[cfg(feature = "uac")]
fn prog_close(s: usize) -> u32 {
    let h = PROG_SRC[s].swap(0, Ordering::AcqRel);
    if h != 0 {
        crate::drivers::usb::uac::capture_close();
    }
    h
}

/// Is `app`'s microphone ask on the glass now? (Its OPEN polls quietly until it is answered.)
#[cfg(feature = "uac")]
fn mic_asking(app: &str) -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        return MIC_ASKING.lock().as_deref() == Some(app);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        let _ = app;
        false
    }
}

/// The program slot `row` runs, by the launcher's name; `None` = a kernel fixture blob (nobody launched it by name).
#[cfg(all(feature = "uac", target_arch = "x86_64"))]
fn prog_name(row: usize) -> Option<String> {
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    let n = crate::video::wm::app_name_of(row as u64 + 1, &mut nb);
    if n > 0 {
        return core::str::from_utf8(&nb[..n]).ok().map(String::from);
    }
    // The args page is read only under the caller's own CR3 (a ring-3 SYS_MSEND): `USER_ARGS_VA` is mapped for every
    // slot (`memory::args_map`, ring-3 RO + NX) and written only by the launcher.
    if crate::arch::memory::current_slot() != Some(row) {
        return None;
    }
    let va = una_abi::USER_ARGS_VA;
    // SAFETY: the live CR3 is slot `row`'s (checked above), which maps its 4 KiB args page at `USER_ARGS_VA`.
    let page = unsafe { core::slice::from_raw_parts(va as *const u8, 4096) };
    let a = una_abi::Args::parse(page, va)?;
    let w = core::str::from_utf8(a.get(0)?).ok()?;
    let p = crate::video::wm::program_name(w);
    if p.is_empty() { None } else { Some(String::from(p)) }
}

/// The program on slot `s` as the table last named it (`-` when unnamed or the table is busy: never spins).
#[cfg(feature = "uac")]
fn say_close(s: usize, by: &str) {
    let g = PROG_NAME.try_lock();
    let app = match &g {
        Some(t) => t.iter().find(|e| e.0 == s).map(|e| e.1.as_str()).unwrap_or("-"),
        None => "-",
    };
    serial_println!("[audio] source app={} slot={} closed by={}", app, s, by);
}

/// THE ring-3 verbs (x86 bus dispatch, the caller's slot `row`): see una-abi's MICRING3 block for the wire.
#[cfg(all(feature = "uac", target_arch = "x86_64"))]
pub fn bus_source(row: usize, verb: u8, body: &[u8], text: &mut alloc::vec::Vec<u8>) -> i64 {
    use una_abi::{EAGAIN, EBADF, EBUSY, EINVAL, ENODEV, EPERM};
    if row >= PROG_SLOTS {
        return EPERM; // the shared row: a kernel task, no program
    }
    match verb {
        una_abi::BUS_VERB_AUDIO_SOURCE_OPEN => {
            if !body.is_empty() {
                return EINVAL;
            }
            let Some(app) = prog_name(row) else {
                serial_println!("[audio] source slot={} refused=unnamed (no launcher name: not asked)", row);
                return EPERM;
            };
            if app == KERNEL_RECORDER {
                serial_println!("[audio] source app={} slot={} refused=reserved-name", app, row);
                return EPERM; // the recorder's grant is the kernel's, never a program's
            }
            if PROG_SRC[row].load(Ordering::Acquire) != 0 {
                return EBUSY;
            }
            if mic_asking(&app) {
                return EAGAIN; // its ask is up: poll quietly until the user answers
            }
            let (g, ch, rate) = match grant_open(&app, true) {
                Ok(x) => x,
                Err("asked") => {
                    if mic_asking(&app) {
                        r3_add(R3_ASKED, 1);
                    }
                    return EAGAIN;
                }
                Err(e) => {
                    r3_add(R3_NODEV, 1);
                    serial_println!("[audio] source app={} slot={} refused={}", app, row, e);
                    return ENODEV;
                }
            };
            let mut h = PROG_HSEQ.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            if h == 0 {
                h = PROG_HSEQ.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            }
            {
                let mut t = PROG_NAME.lock();
                t.retain(|e| e.0 != row);
                t.push((row, app.clone()));
            }
            PROG_REVOKED[row].store(0, Ordering::Release);
            PROG_SRC[row].store(h, Ordering::Release);
            r3_add(R3_OPENED, 1);
            serial_println!("[audio] source app={} grant={} fmt=pcm16/{}/{} opened slot={} handle={}", app, g, ch, rate, row, h);
            text.extend_from_slice(&una_abi::audio_source_open_reply(ch, rate, h));
            0
        }
        una_abi::BUS_VERB_AUDIO_SOURCE_READ => {
            let Some(h) = una_abi::audio_source_handle_parse(body) else { return EINVAL };
            if PROG_SRC[row].load(Ordering::Acquire) != h {
                return if PROG_REVOKED[row].load(Ordering::Acquire) == h { EPERM } else { EBADF };
            }
            text.resize(una_abi::AUDIO_SOURCE_READ_MAX, 0);
            let n = crate::drivers::usb::uac::read_capture(&mut text[..]);
            text.truncate(n);
            r3_add(R3_READ, n as u64);
            0
        }
        una_abi::BUS_VERB_AUDIO_SOURCE_CLOSE => {
            let Some(h) = una_abi::audio_source_handle_parse(body) else { return EINVAL };
            if PROG_REVOKED[row].compare_exchange(h, 0, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return 0; // the revoke closed it already
            }
            if PROG_SRC[row].compare_exchange(h, 0, Ordering::AcqRel, Ordering::Acquire).is_err() {
                return EBADF;
            }
            crate::drivers::usb::uac::capture_close();
            r3_add(R3_CLOSED, 1);
            say_close(row, "verb");
            0
        }
        _ => EINVAL,
    }
}

/// MICRING3 M2 — Settings > Privacy revoked decision `key` (called through `fs::apptrust::mic_revoked`, BEFORE the
/// store write — the camera's order): every slot whose program that microphone decision names has its source closed
/// now; the driver's next service pass closes the pipe and the dot goes out. Service context (it allocates).
#[cfg(feature = "uac")]
pub fn source_revoked(key: &str) {
    let t = PROG_NAME.lock();
    let mut n = 0u32;
    for (s, app) in t.iter() {
        if crate::prefs::decision_key(crate::fs::apptrust::MIC_DOMAIN, app) != key {
            continue;
        }
        let h = prog_close(*s);
        if h == 0 {
            continue;
        }
        PROG_REVOKED[*s].store(h, Ordering::Release);
        r3_add(R3_REVOKED, 1);
        n += 1;
        serial_println!("[audio] source app={} revoked -> closed", app);
    }
    if n == 0 {
        serial_println!("[mic] revoke key={} open=0 (no program source open; the next open asks)", key);
    }
}

/// MICRING3 — slot `s` lost its last holder (`sched::term_release_edge`, IF=0): its source, if any, closes. Atomics and
/// a `try_lock` only; allocates nothing.
#[cfg(feature = "uac")]
pub fn slot_gone(s: usize) {
    if s >= PROG_SLOTS {
        return;
    }
    PROG_REVOKED[s].store(0, Ordering::Release);
    if prog_close(s) == 0 {
        return;
    }
    r3_add(R3_TERM, 1);
    say_close(s, "terminate");
}
// ── A2DP (rmbp-ledger B739): THE BLUETOOTH SINK — TAIL-APPENDED ───────────────────────────────────────────────────────
// A third sink behind the same seam (BP-audio-1): `sink("bt")` / `sink("bt:<addr>")` chooses it while an A2DP stream
// runs (`drivers/ehci/bthid::avdtp`, the paired headset); `tone_on(Sink::Bt, ..)` writes the sine into that module's
// PCM ring (44.1 kHz stereo, the stream's rate) exactly as the USB sink is fed, and the media clock frames it into SBC.
// The choice is Principia's `system.audio.output` = `bt` (BP-audio-2). The mixer (AUDIOMIX) is not on this tip: this is
// `audio::sink`'s existing shape, one more arm. Settings > General's Sound picker reads [`outputs`] and presses
// [`choose`] (a third segment while the device is on a link).

#[cfg(all(target_arch = "x86_64", feature = "btc"))]
use crate::drivers::ehci::bthid::avdtp as a2dp;

#[cfg(all(target_arch = "x86_64", feature = "btc"))]
fn bt_ready() -> bool {
    a2dp::streaming()
}

#[cfg(all(target_arch = "x86_64", feature = "btc"))]
fn bt_label() -> String {
    a2dp::device().unwrap_or_else(|| String::from("BT (none)"))
}

#[cfg(all(target_arch = "x86_64", feature = "btc"))]
fn bt_choose(name: &str) -> Result<Sink, &'static str> {
    if !bt_ready() {
        serial_println!("[audio] sink={} REFUSED reason=no-a2dp-stream", name);
        return Err("no-a2dp-stream");
    }
    CHOSEN.store(3, Ordering::Release);
    crate::prefs::set_sys(PREF_KEY, crate::prefs::PrefValue::Str(String::from("bt")));
    serial_println!("[audio] sink={} chosen", label(Sink::Bt));
    Ok(Sink::Bt)
}

/// Settings > General's Sound picker: the labels (HDA, USB, and the A2DP device while one is on a link) and the
/// selected segment.
#[cfg(all(target_arch = "x86_64", feature = "btc"))]
pub fn outputs() -> (alloc::vec::Vec<String>, usize) {
    let mut v = alloc::vec![label(Sink::Hda), label(Sink::Usb)];
    if let Some(d) = a2dp::device() {
        v.push(d);
    }
    let sel = match current() {
        Sink::Hda => 0,
        Sink::Usb => 1,
        Sink::Bt => 2,
    };
    let last = v.len() - 1;
    (v, sel.min(last))
}

/// The picker's segment `i` pressed.
#[cfg(all(target_arch = "x86_64", feature = "btc"))]
pub fn choose(i: usize) -> Result<Sink, &'static str> {
    sink(match i {
        0 => "hda",
        1 => "usb",
        _ => "bt",
    })
}

/// The Bluetooth sink: `ms` of an `hz` sine at -18 dBFS with 20 ms fades into the A2DP ring at 44.1 kHz, then the
/// drain awaited — `usb_tone`'s shape.
#[cfg(all(target_arch = "x86_64", feature = "btc"))]
fn bt_tone(hz: u32, ms: u32) -> Result<(u64, u32), &'static str> {
    const AMP: i32 = 4096;
    const CHUNK: usize = 245; // frames = 5.6 ms
    let rate = a2dp::RATE as u64;
    let total = (rate * ms as u64 / 1000) as usize;
    let fade = (rate / 50) as usize;
    let (f0, u0) = a2dp::counters();
    a2dp::begin()?;
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
            let w = a2dp::write(&chunk[off..n * 4]);
            off += w;
            if w != 0 {
                last = crate::arch::ms();
            } else if !a2dp::streaming() || crate::arch::ms().wrapping_sub(last) > 500 {
                a2dp::finish();
                serial_println!("[audio] tone sink=bt ABORT reason=stream-stalled written_frames={} of {}", f + off / 4, total);
                return Err("stream-stalled");
            } else {
                nap(2);
            }
        }
        f += n;
    }
    a2dp::finish();
    let t0 = crate::arch::ms();
    while a2dp::queued() > 0 && crate::arch::ms().wrapping_sub(t0) < 2000 {
        nap(2);
    }
    let (f1, u1) = a2dp::counters();
    Ok((f1.wrapping_sub(f0), u1.wrapping_sub(u0)))
}


// ── SOFTYMIC (rmbp-ledger B749, BP-video-24, BP-audio-12) — TAIL-APPENDED: who holds the microphone ──────────────────────
// The in-use light paints from the driver's fact (`uac::capture_armed`, BP-audio-10); the NAME beside it (the light's
// wire, Settings > Privacy's `in use by <app>`) comes from here: the live kernel `Source`s ([`KSRC`], raised in `open`,
// lowered in `Drop`) and the ring-3 slots whose `PROG_SRC` word is non-zero (MICRING3). Readers run in paint context
// (the menu bar's witness is masked in the composite chain): atomics and `try_lock` only, no allocation.

/// Live kernel `Source`s (the recorder, or a kernel app holding a grant).
#[cfg(feature = "uac")]
static KSRC: AtomicU32 = AtomicU32::new(0);
/// The last kernel opener's app name (written in task context at the open; read with `try_lock`).
#[cfg(feature = "uac")]
static KSRC_NAME: crate::sync::Mutex<String> = crate::sync::Mutex::new(String::new());

#[cfg(feature = "uac")]
fn ksrc_opened(app: &str) {
    {
        let mut g = KSRC_NAME.lock();
        g.clear();
        g.push_str(app);
    }
    KSRC.fetch_add(1, Ordering::AcqRel);
}

#[cfg(feature = "uac")]
fn ksrc_closed() {
    let _ = KSRC.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
}

#[cfg(feature = "uac")]
fn name_into(out: &mut [u8], b: &[u8]) -> usize {
    let n = b.len().min(out.len());
    out[..n].copy_from_slice(&b[..n]);
    n
}

/// SOFTYMIC — THE one read of who holds the microphone: `(open sources, bytes of the holder's name written to out)`.
/// Open sources = live kernel `Source`s + ring-3 slots with an open source. The name is the first open ring-3 slot's
/// launcher name, else the last kernel opener's; 0 bytes = nobody open, or the name table busy this instant (never
/// spins). Paint-context safe: atomics and `try_lock`, no allocation.
#[cfg(feature = "uac")]
pub fn mic_holder(out: &mut [u8]) -> (u32, usize) {
    let k = KSRC.load(Ordering::Acquire);
    let mut n = k;
    let mut first: Option<usize> = None;
    for (s, w) in PROG_SRC.iter().enumerate() {
        if w.load(Ordering::Acquire) != 0 {
            n += 1;
            if first.is_none() {
                first = Some(s);
            }
        }
    }
    let len = match first {
        Some(s) => PROG_NAME.try_lock().and_then(|t| t.iter().find(|e| e.0 == s).map(|e| name_into(out, e.1.as_bytes()))).unwrap_or(0),
        None if k != 0 => KSRC_NAME.try_lock().map(|g| name_into(out, g.as_bytes())).unwrap_or(0),
        None => 0,
    };
    (n, len)
}
