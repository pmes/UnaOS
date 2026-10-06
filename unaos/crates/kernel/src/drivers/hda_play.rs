//! PLAYWAV (R75) — a streaming PCM path on the HDA output stream, and a WAV player on top of it.
//! Declared from the tail of `hda.rs` as `#[path = "hda_play.rs"] pub mod play;` so it is a CHILD of `hda`
//! and reaches the controller's private registers/`Rings`/`Path` without widening any of them.
//!
//! Shape: `start(rate, ch, bits)` re-runs the tone's own bring-up (`hda_tone_test`'s probe) with `REQ_RATE`
//! set; `run_tone` — after it has done the power/amp/GPIO/EAPD/converter-format/stream-bind work itself —
//! calls `gate()` (one same-line fold before the SDnCTL reset) which swaps the 192 KB / 2-entry tone buffer for
//! a 4 x 32 KiB IOC ring and returns early. The stream is NOT run until `feed()` has primed the ring; the
//! service tick (`service()`, folded at the top of `probe_after_root`) polls `LPIB` (INTCTL is never written, the
//! BCIS latch reaches no CPU — HDASIE is a latch experiment) and refills each entry as it completes.
//! Internally the stream is ALWAYS 16-bit stereo: `feed()` widens 8-bit and duplicates mono, and resamples by
//! nearest-sample to 48 kHz when the codec's PCM-rates word (or the format readback) refuses the rate.
use super::tone::*;
use super::*;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const ENTRIES: usize = 4;
const ENTRY_BYTES: usize = 32 * 1024;
const RING_BYTES: usize = ENTRIES * ENTRY_BYTES;
// AUDIO7 (B313) M3: was 128 KiB = RING_BYTES — with feed()'s 2x frame estimate the FIFO topped out at 96 KiB
// (90312 bytes at 44.1 kHz) and never reached the RING_BYTES prime threshold: armed, never RUN, silent (PLAY2).
const FIFO_CAP: usize = 2 * RING_BYTES;

static REQ_RATE: AtomicU32 = AtomicU32::new(0);
static RING_ADDR: AtomicU64 = AtomicU64::new(0);
static BDL_ADDR: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false); // cheap idle test for `service()`

struct St {
    armed: bool, running: bool, ended: bool, done: bool,
    base: u64, sd: u64, eff_rate: u32,
    src_rate: u32, src_ch: usize, src_bits: usize,
    fifo: VecDeque<u8>, carry: Vec<u8>,
    prev_cur: usize, silent: u32, underruns: u32, completed: u64, err: u32,
    in_frames: u64, out_n: u64, resampled: bool,
    // AUDIO7 (B313) M3 — the run witness: RUN readback, LPIB at RUN and as it moves, the first buffer's level.
    run_bit: bool, run_ms: u64, lpib0: u32, lpib_last: u32, lpib_moved: bool, moved_ms: u64, next_print: u64,
    level: u32, witnessed: bool, tag: u32, armed_ms: u64,
}
impl St {
    const fn new() -> St {
        St { armed: false, running: false, ended: false, done: false, base: 0, sd: 0, eff_rate: 48_000,
             src_rate: 48_000, src_ch: 2, src_bits: 16, fifo: VecDeque::new(), carry: Vec::new(),
             prev_cur: 0, silent: 0, underruns: 0, completed: 0, err: 0, in_frames: 0, out_n: 0, resampled: false,
             run_bit: false, run_ms: 0, lpib0: 0, lpib_last: 0, lpib_moved: false, moved_ms: 0, next_print: 0, level: 0, witnessed: false, tag: 0, armed_ms: 0 }
    }
}
static ST: spin::Mutex<St> = spin::Mutex::new(St::new());

struct Wav { path: String, rate: u32, ch: usize, bits: usize, data_off: u64, data_len: u64, pos: u64, chunk: usize, fed_end: bool, start: u64 }
static WAV: spin::Mutex<Option<Wav>> = spin::Mutex::new(None);
static PENDING: spin::Mutex<Option<String>> = spin::Mutex::new(None);

/// Converter-format rate bits (BASE/MULT/DIV) for an exact rate, or None. [HDA-SPEC §3.3.41]
fn rate_bits(rate: u32) -> Option<u16> {
    for (base, bb) in [(48_000u32, 0u16), (44_100u32, 0x4000u16)] {
        for mult in 1..=2u32 {
            for div in 1..=8u32 {
                if base * mult / div == rate && base * mult % div == 0 {
                    return Some(bb | (((mult - 1) as u16) << 11) | (((div - 1) as u16) << 8));
                }
            }
        }
    }
    None
}
/// `PARAM_SUPPORTED_PCM` bit for the rates a converter reports in its low half. [HDA-SPEC §7.3.4.7]
fn rate_cap_bit(rate: u32) -> Option<u32> {
    [8000u32, 11025, 16000, 22050, 32000, 44100, 48000].iter().position(|&r| r == rate).map(|i| i as u32)
}

/// The seam `run_tone` calls (same-line fold, before the SDnCTL reset). The tone has already powered the
/// path, unmuted the amps, driven GPIO/EAPD and bound the converters to `STREAM_TAG`; this sets the converter
/// format for the requested rate, builds the 4-entry ring and programs (but does not RUN) the stream.
pub(super) fn gate(base: u64, sd: u64, iss: u8, rings: &mut Rings, cad: u8, paths: &[Path], chan: &[u8], a: &mut Audit) -> bool {
    let want = REQ_RATE.swap(0, Ordering::AcqRel);
    if want == 0 || paths.is_empty() {
        return false;
    }
    let caps = rings.cmd(cad, paths[0].dac, VERB_GET_PARAMETER, PARAM_SUPPORTED_PCM, a).unwrap_or(0);
    let cap_ok = rate_cap_bit(want).map_or(false, |b| caps & (1 << b) != 0);
    let mut eff = want;
    let mut fmt = match (cap_ok, rate_bits(want)) { (true, Some(b)) => b | FMT_48K_16_STEREO, _ => { eff = 48_000; FMT_48K_16_STEREO } };
    let mut back = 0u32;
    for pass in 0..2 {
        for p in paths {
            let _ = rings.cmd16(cad, p.dac, VERB_SET_CONVERTER_FORMAT, fmt as u32, a);
        }
        back = rings.cmd(cad, paths[0].dac, VERB_GET_CONVERTER_FORMAT, 0, a).unwrap_or(0) & 0xFFFF;
        if back == fmt as u32 || fmt == FMT_48K_16_STEREO || pass == 1 {
            break;
        }
        eff = 48_000; // the codec rejected it: fall back to the always-supported 48 kHz and resample
        fmt = FMT_48K_16_STEREO;
    }
    if RING_ADDR.load(Ordering::Relaxed) == 0 {
        RING_ADDR.store(dma_alloc(RING_BYTES, 128), Ordering::Relaxed);
        BDL_ADDR.store(dma_alloc(ENTRIES * 16, BDL_ALIGN), Ordering::Relaxed);
    }
    let (ring, bdl) = (RING_ADDR.load(Ordering::Relaxed), BDL_ADDR.load(Ordering::Relaxed));
    if ring == 0 || bdl == 0 {
        serial_println!("[play] REFUSED reason=dma-alloc");
        return true;
    }
    for i in 0..ENTRIES {
        let e = bdl + (i as u64) * 16;
        let addr = bus_addr(ring) + (i * ENTRY_BYTES) as u64;
        unsafe {
            core::ptr::write_volatile(e as *mut u32, (addr & 0xFFFF_FFFF) as u32);
            core::ptr::write_volatile((e + 4) as *mut u32, (addr >> 32) as u32);
            core::ptr::write_volatile((e + 8) as *mut u32, ENTRY_BYTES as u32);
            core::ptr::write_volatile((e + 12) as *mut u32, 1); // IOC
        }
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    // AUDIO7 (B313) M2: the ONE reset the tone also uses — stop, SRST, STS, flush, snoop, descriptor, codec re-bind
    // with the converter format = SDxFMT, and the M1 `run=<n> play pre:/post:/diff` lines.
    let rr = super::stream::rearm(rings, &super::stream::Params { base, sd, cad, bdl, bdl_bytes: ENTRIES * 16, buf: ring, buf_bytes: RING_BYTES,
        cbl: RING_BYTES as u32, lvi: (ENTRIES - 1) as u16, fmt, tag: STREAM_TAG, paths, chan }, "play", a);
    let mut s = ST.lock();
    s.armed = true; s.base = base; s.sd = sd; s.eff_rate = eff; s.resampled = eff != want; s.tag = STREAM_TAG; s.armed_ms = crate::arch::ms();
    // AUDIO8 (B329): the stream is the player's now (the amp stays held while it is open); the DAC out amp is muted
    // until RUN, then ramped (ring_pump), and ramped down before the stop (hw_stop).
    super::amp::to_play();
    super::amp::ramp_prep(rings, cad, paths, a);
    serial_println!("[play] arm sd={} iss={} want_rate={} eff_rate={} fmt={:#06x}(readback {:#06x}) pcmcaps={:#010x} ring={:#x} entries={}x{} cbl={}(readback {}) lvi={}(readback {}) ioc=1 poll=lpib",
        0, iss, want, eff, fmt, back, caps, ring, ENTRIES, ENTRY_BYTES, RING_BYTES, r32(base, sd + SD_CBL), ENTRIES - 1, r16(base, sd + SD_LVI));
    serial_println!("[play] rearm run={} dac_fmt_match={} tag_match={} sdfmt_rd={:#06x} stable={}", rr.run, rr.fmt_match as u8, rr.tag_match as u8, rr.sdfmt_rd, rr.stable as u8);
    true
}

/// Begin a stream of `rate` Hz / `channels` / `bits` source samples. Returns the EFFECTIVE codec rate
/// (equal to `rate` unless it is being resampled to 48 kHz). The stream starts once `feed()` primes the ring.
pub fn start(rate: u32, channels: u8, bits: u8) -> Result<u32, &'static str> {
    if !(8_000..=48_000).contains(&rate) { return Err("rate outside 8000..48000"); }
    if channels != 1 && channels != 2 { return Err("channels must be 1 or 2"); }
    if bits != 8 && bits != 16 { return Err("bits must be 8 or 16"); }
    stop();
    *ST.lock() = { let mut s = St::new(); s.src_rate = rate; s.src_ch = channels as usize; s.src_bits = bits as usize; s };
    REQ_RATE.store(rate, Ordering::Release);
    TONE_NOW.store(true, Ordering::Relaxed);
    super::probe();
    TONE_NOW.store(false, Ordering::Relaxed);
    REQ_RATE.store(0, Ordering::Release);
    let s = ST.lock();
    if !s.armed {
        return Err("no speaker path / stream not armed");
    }
    ACTIVE.store(true, Ordering::Release);
    Ok(s.eff_rate)
}

/// Queue source-format PCM bytes. Returns the bytes accepted: all of them, or 0 when the ring's FIFO has no
/// room yet (retry on the next tick). Format conversion and nearest-sample resampling happen here.
pub fn feed(data: &[u8]) -> usize {
    let mut s = ST.lock();
    if !s.armed || s.ended || data.is_empty() { return 0; }
    let fin = s.src_ch * s.src_bits / 8;
    let nframes = (s.carry.len() + data.len()) / fin;
    let ratio = ((s.eff_rate + s.src_rate.max(1) - 1) / s.src_rate.max(1)) as usize; // AUDIO7 M3: ceil, not +1 (a 2x over-estimate at equal rates)
    if s.fifo.len() + nframes * ratio * 4 > FIFO_CAP { return 0; }
    let mut buf = core::mem::take(&mut s.carry);
    buf.extend_from_slice(data);
    let (src, eff) = (s.src_rate as u64, s.eff_rate as u64);
    let mut i = 0;
    while i + fin <= buf.len() {
        let f = &buf[i..i + fin];
        let (l, r) = match (s.src_bits, s.src_ch) {
            (8, 1) => { let v = ((f[0] as i16) - 128) << 8; (v, v) }
            (8, _) => (((f[0] as i16) - 128) << 8, ((f[1] as i16) - 128) << 8),
            (_, 1) => { let v = i16::from_le_bytes([f[0], f[1]]); (v, v) }
            _ => (i16::from_le_bytes([f[0], f[1]]), i16::from_le_bytes([f[2], f[3]])),
        };
        let gi = s.in_frames; // global input frame index
        s.in_frames += 1;
        while s.out_n * src / eff <= gi {
            let (lb, rb) = (l.to_le_bytes(), r.to_le_bytes());
            s.fifo.extend([lb[0], lb[1], rb[0], rb[1]]);
            s.out_n += 1;
        }
        i += fin;
    }
    s.carry = buf[i..].to_vec();
    data.len()
}

/// The producer has no more bytes: the ring drains to silence and the stream stops itself.
pub fn finish() { ST.lock().ended = true; }

/// Stop the stream now (idempotent).
pub fn stop() {
    {
        let mut s = ST.lock();
        if s.armed { hw_stop(&mut s); }
        s.armed = false; s.running = false; s.done = true;
    }
    PAUSED.store(false, Ordering::Release); super::amp::amp_release(super::amp::PLAY); // AUDIO8 (B329): idempotent — a stop with nothing armed still closes the owner bit
    ACTIVE.store(false, Ordering::Release);
    *WAV.lock() = None; *CODED.lock() = None; // after the ST guard drops: wav_pump takes WAV then ST, so never the other order
}

fn hw_stop(s: &mut St) {
    let (b, sd) = (s.base, s.sd);
    super::amp::ramp_svc(false); // AUDIO8 (B329) M2: down and muted before RUN clears
    w8(b, sd + SD_CTL, 0);
    wait_us(10_000, || r8(b, sd + SD_CTL) & SDCTL_RUN as u8 == 0);
    w8(b, sd + SD_CTL, SDCTL_SRST as u8);
    wait_us(10_000, || r8(b, sd + SD_CTL) & SDCTL_SRST as u8 != 0);
    w8(b, sd + SD_CTL, 0);
    wait_us(10_000, || r8(b, sd + SD_CTL) & SDCTL_SRST as u8 == 0);
    w8(b, sd + SD_STS, SDSTS_BCIS | SDSTS_FIFOE | SDSTS_DESE);
    super::amp::amp_release(super::amp::PLAY); // AUDIO8 (B329) M1: the last close starts the amp's idle hold-off
}

fn refill(s: &mut St, i: usize, prefill: bool) {
    let ring = RING_ADDR.load(Ordering::Relaxed);
    let dst = (ring + (i * ENTRY_BYTES) as u64) as *mut u8;
    let n = s.fifo.len().min(ENTRY_BYTES) & !3;
    for k in 0..n { unsafe { core::ptr::write_volatile(dst.add(k), s.fifo.pop_front().unwrap_or(0)); } }
    unsafe { core::ptr::write_bytes(dst.add(n), 0, ENTRY_BYTES - n); }
    core::sync::atomic::fence(Ordering::SeqCst);
    super::stream::flush(dst as u64, ENTRY_BYTES); // AUDIO7 M2: the entry reaches memory before the DMA can fetch it
    if n == 0 && s.ended { s.silent += 1; } else if n < ENTRY_BYTES && !s.ended && !prefill { s.underruns += 1; }
}

fn ring_pump(s: &mut St) {
    if !s.armed || s.done || PAUSED.load(Ordering::Acquire) { return; } // PLAYER (B419): a paused stream is left alone (RUN is clear)
    let (b, sd) = (s.base, s.sd);
    if !s.running {
        // AUDIO7 M3: start once a whole ring is queued, OR the producer is done, OR another chunk could not fit.
        if s.fifo.len() < RING_BYTES && !s.ended && s.fifo.len() + ENTRY_BYTES <= FIFO_CAP { return; }
        for i in 0..ENTRIES { refill(s, i, true); }
        s.level = rms(RING_ADDR.load(Ordering::Relaxed), ENTRY_BYTES);
        s.prev_cur = 0;
        s.lpib0 = r32(b, sd + SD_LPIB);
        s.lpib_last = s.lpib0;
        let (ctl, on) = super::stream::run(b, sd, s.tag);
        super::amp::ramp_svc(true); // AUDIO8 (B329) M2
        s.run_bit = on; s.run_ms = crate::arch::ms(); s.moved_ms = s.run_ms; s.next_print = s.run_ms + 100;
        serial_println!("[play] run ctl={:#08x} run_bit={} lpib0={} level={} fifo={}", ctl, on as u8, s.lpib0, s.level, s.fifo.len());
        s.running = true;
        return;
    }
    track(s);
    let sts = r8(b, sd + SD_STS);
    if sts & (SDSTS_BCIS | SDSTS_FIFOE | SDSTS_DESE) != 0 {
        if sts & (SDSTS_FIFOE | SDSTS_DESE) != 0 { s.err += 1; }
        w8(b, sd + SD_STS, SDSTS_BCIS | SDSTS_FIFOE | SDSTS_DESE);
    }
    let cur = (r32(b, sd + SD_LPIB) as usize / ENTRY_BYTES) % ENTRIES;
    let mut guard = 0;
    while s.prev_cur != cur && guard < ENTRIES {
        let j = s.prev_cur;
        refill(s, j, false);
        s.prev_cur = (j + 1) % ENTRIES;
        s.completed += 1;
        guard += 1;
    }
    if s.silent >= ENTRIES as u32 {
        hw_stop(s);
        s.running = false; s.done = true;
    }
}

// ── WAV ──────────────────────────────────────────────────────────────────────────────────────────

fn le16(b: &[u8], o: usize) -> u32 { u16::from_le_bytes([b[o], b[o + 1]]) as u32 }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }

fn parse(path: &str) -> Result<Wav, String> {
    let mt = crate::shell::vfs_mount_table();
    let size = mt.stat(path).map_err(|e| alloc::format!("stat: {:?}", e))?.size;
    let h = mt.read(path, 0, 12).map_err(|e| alloc::format!("read: {:?}", e))?;
    if h.len() < 12 || &h[0..4] != b"RIFF" || &h[8..12] != b"WAVE" { return Err(String::from("not a RIFF/WAVE file")); }
    let (mut off, mut fmt): (u64, Option<(u32, u32, u32, u32)>) = (12, None);
    while off + 8 <= size {
        let c = mt.read(path, off, 8).map_err(|e| alloc::format!("read: {:?}", e))?;
        if c.len() < 8 { break; }
        let len = le32(&c, 4) as u64;
        if &c[0..4] == b"fmt " {
            let f = mt.read(path, off + 8, 16).map_err(|e| alloc::format!("read: {:?}", e))?;
            if f.len() < 16 { return Err(String::from("short fmt chunk")); }
            fmt = Some((le16(&f, 0), le16(&f, 2), le32(&f, 4), le16(&f, 14)));
        } else if &c[0..4] == b"data" {
            let (tag, ch, rate, bits) = fmt.ok_or_else(|| String::from("data before fmt"))?;
            if tag != 1 { return Err(alloc::format!("not PCM (format tag {})", tag)); }
            if bits != 8 && bits != 16 { return Err(alloc::format!("{}-bit PCM unsupported (8/16 only)", bits)); }
            if ch != 1 && ch != 2 { return Err(alloc::format!("{} channels unsupported (1/2 only)", ch)); }
            if !(8_000..=48_000).contains(&rate) { return Err(alloc::format!("{} Hz unsupported (8000..48000)", rate)); }
            let data_off = off + 8;
            let data_len = len.min(size.saturating_sub(data_off));
            let fin = (ch * bits / 8) as u64;
            let frames = (8192 * rate as u64 / 48_000).max(64);
            return Ok(Wav { path: String::from(path), rate, ch: ch as usize, bits: bits as usize, data_off,
                            data_len: data_len / fin * fin, pos: 0, chunk: (frames * fin) as usize, fed_end: false, start: 0 });
        }
        off += 8 + len + (len & 1);
    }
    Err(String::from("no data chunk"))
}

/// Parse `path`, start the stream and arm the tick-driven file pump. Prints the witness arm itself on refusal.
pub fn open_wav(path: &str) -> Result<(), String> {
    let w = match parse(path) {
        Ok(w) => w,
        Err(r) => { return open_coded(path, r); }
    };
    let eff = match start(w.rate, w.ch as u8, w.bits as u8) {
        Ok(e) => e,
        Err(r) => { serial_println!(":: PLAYWAV: path={} reason={} -> REFUSED ::", path, r); return Err(String::from(r)); }
    };
    serial_println!("[play] open path={} rate={} ch={} bits={} bytes={} chunk={} eff_rate={} resampled={}", path, w.rate, w.ch, w.bits, w.data_len, w.chunk, eff, (eff != w.rate) as u8);
    *WAV.lock() = Some(w);
    ACTIVE.store(true, Ordering::Release);
    Ok(())
}

fn wav_pump() {
    for _ in 0..4 {
        let mut g = WAV.lock();
        let Some(w) = g.as_mut() else { return };
        if w.fed_end { return; }
        if w.pos >= w.data_len { w.fed_end = true; drop(g); finish(); return; }
        let n = (w.data_len - w.pos).min(w.chunk as u64) as usize;
        let data = crate::shell::vfs_mount_table().read(&w.path, w.data_off + w.pos, n);
        match data {
            Ok(d) if !d.is_empty() => {
                if feed(&d) == 0 { return; } // no room: retry next tick
                w.pos += d.len() as u64;
            }
            _ => { serial_println!("[play] read error at pos={} — ending the stream", w.pos); w.pos = w.data_len; }
        }
    }
}

fn report() {
    let w = WAV.lock().take();
    let Some(w) = w else { return };
    let s = ST.lock();
    let frames = s.in_frames;
    let want = (w.data_len - w.start) / (w.ch * w.bits / 8) as u64; // PLAYER (B419): a seek starts the stream at `start`
    let ms = frames * 1000 / w.rate as u64;
    let ok = frames == want && s.underruns == 0 && s.err == 0 && s.completed >= 1 && s.lpib_moved && s.done;
    serial_println!("[play] done resampled={} eff_rate={} entries={} fifoe_dese={} run_bit={} lpib_moved={} level={}", s.resampled as u8, s.eff_rate, s.completed, s.err, s.run_bit as u8, s.lpib_moved as u8, s.level);
    // AUDIO7 M4: `done=` is the ring's own drain (LPIB walked past the last data entry and every entry refilled with silence).
    serial_println!(":: PLAYWAV: path={} rate={} frames={} lpib_moved={} done={} -> {} :: ch={} bits={} secs={}.{} under={} ::", w.path, w.rate, frames, s.lpib_moved as u8, s.done as u8, if ok { "PASS" } else { "FAIL" }, w.ch, w.bits, ms / 1000, (ms % 1000) / 100, s.underruns);
}

/// The service tick (folded at the top of `probe_after_root`): latched opens, file pump, ring refill.
pub fn service() {
    super::amp::tick(); // AUDIO8 (B329) M1: the amp's idle hold-off (idle cost: one atomic load)
    if ALERT.load(Ordering::Acquire) { alert_play(); } // NOTIFYPANE (B435): a latched alert sound (idle cost: one atomic load)
    if TQ_LIVE.load(Ordering::Acquire) { tq_tick(); } // DECJOBHANG M3: `tests play`'s queue (idle cost: one atomic load)
    if !ACTIVE.load(Ordering::Acquire) { return; }
    if let Some(p) = PENDING.try_lock().and_then(|mut g| g.take()) { let _ = open_wav(&p); }
    wav_pump(); coded_pump();
    let fin = { let Some(mut s) = ST.try_lock() else { return }; ring_pump(&mut s); s.done && s.armed };
    if fin {
        report(); coded_report();
        let mut s = ST.lock();
        s.armed = false;
        ACTIVE.store(false, Ordering::Release);
    }
}

/// Quarry: latch a `.WAV` open for the next tick (click-router stack depth).
pub fn request_open(path: &str) { *PENDING.lock() = Some(String::from(path)); ACTIVE.store(true, Ordering::Release); }

/// `play <path.wav>` / `play stop`. `path` is the cwd-resolved argument.
pub fn shell_verb(args: &[&str], path: &str, console: &mut crate::console::Console) {
    match args.first().copied() {
        None => console.println("usage: play <file: wav flac ogg opus mp3 aac m4a aiff> | play stop"),
        Some("stop") => { let q = { let mut t = TQ.lock(); let n = t.len(); t.clear(); n }; stop(); dec_stop(); console.println(&alloc::format!("play: stopped (tests queue dropped {})", q)); } // DECJOBHANG M3: stop ends the `tests play` queue too
        Some(_) => match open_wav(path) {
            Ok(()) => console.println(&alloc::format!("play: {} — streaming (watch the serial for :: PLAYWAV ::)", path)),
            Err(r) => console.println(&alloc::format!("play: {}: {}", path, r)),
        },
    }
}

// ── `tests playwav` — a generated 2 s 440 Hz stereo WAV, played, then unlinked ────────────────────

fn synth_wav() -> Vec<u8> {
    const N: usize = 96_000;
    let mut v: Vec<u8> = Vec::with_capacity(44 + N * 4);
    v.extend_from_slice(b"RIFF"); v.extend_from_slice(&((36 + N * 4) as u32).to_le_bytes()); v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes()); v.extend_from_slice(&1u16.to_le_bytes()); v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&48_000u32.to_le_bytes()); v.extend_from_slice(&192_000u32.to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes()); v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data"); v.extend_from_slice(&((N * 4) as u32).to_le_bytes());
    const H: i64 = 32_768; // half a cycle, Bhaskara sine: 16u/(5H^2-4u), u = h(H-h)
    for n in 0..N {
        let ph = ((n as u64 * 440 * 65_536 / 48_000) % 65_536) as i64;
        let (h, neg) = if ph < H { (ph, false) } else { (ph - H, true) };
        let u = h * (H - h);
        let mut s = 16 * u * 8192 / (5 * H * H - 4 * u);
        let edge = n.min(N - 1 - n) as i64; // 20 ms linear fade at both ends
        if edge < 960 { s = s * edge / 960; }
        let sv: i64 = if neg { -s } else { s };
        let b = (sv as i16).to_le_bytes();
        v.extend_from_slice(&[b[0], b[1], b[0], b[1]]);
    }
    v
}

/// `tests playwav`.
pub fn selftest() {
    use crate::fs::vfs::NodeKind;
    let mut cands: Vec<String> = Vec::new();
    #[cfg(feature = "login")]
    {
        let mut b = [0u8; crate::fs::users::NAME_MAX];
        if let Some(n) = crate::fs::users::whoami(&mut b) {
            if let Ok(name) = core::str::from_utf8(&b[..n]) { cands.push(alloc::format!("/home/{}/TEST.WAV", name)); }
        }
    }
    cands.push(String::from("/home/TEST.WAV"));
    cands.push(String::from("/TEST.WAV"));
    let wav = synth_wav();
    let mt = crate::shell::vfs_mount_table();
    let p = crate::fs::vfs::KERNEL_PRINCIPAL;
    let mut written: Option<&String> = None;
    for c in cands.iter() {
        let _ = mt.unlink(c, p);
        if mt.create(c, NodeKind::File, p).is_err() { continue; }
        let (mut off, mut ok) = (0usize, true);
        while off < wav.len() {
            let n = (wav.len() - off).min(4096);
            match mt.write(c, off as u64, &wav[off..off + n], p) { Ok(w) if w > 0 => off += w, _ => { ok = false; break; } }
        }
        if ok { written = Some(c); break; }
        let _ = mt.unlink(c, p);
    }
    let Some(path) = written else {
        serial_println!(":: PLAYWAV: path=- reason=no-writable-path -> FAIL ::");
        return;
    };
    // AUDIO7 M4: the completion is the ring's drain (`report()` from `service()`), not a wall clock. The loop
    // ends on that, or on a STALL (LPIB still for 1 s while running, or no RUN 2 s after the arm). No codec = SKIP.
    match open_wav(path) {
        Ok(()) => {
            let mut stalled = false;
            while ACTIVE.load(Ordering::Acquire) {
                service();
                let (_done, _moved, st) = progress();
                if st { stalled = true; break; }
                delay_us(2_000);
            }
            if stalled {
                let (lp, run_bit, fifo) = { let s = ST.lock(); (s.lpib_last, s.run_bit as u8, s.fifo.len()) };
                stop();
                serial_println!(":: PLAYWAV: path={} reason=stalled lpib={} run_bit={} fifo={} -> FAIL ::", path, lp, run_bit, fifo);
            }
        }
        Err(r) => serial_println!(":: PLAYWAV: path={} reason=no-codec ({}) -> SKIP ::", path, r),
    }
    let _ = mt.unlink(path, p);
}

// ── AUDIO7 (B313) M3 — the play witness ──────────────────────────────────────────────────────────────

/// RMS of the interleaved 16-bit samples in `[p, p + bytes)`, CPU view (integer square root).
fn rms(p: u64, bytes: usize) -> u32 {
    if p == 0 { return 0; }
    let n = bytes / 2;
    let mut acc: u64 = 0;
    for i in 0..n {
        let v = unsafe { core::ptr::read_volatile((p + (i * 2) as u64) as *const i16) } as i64;
        acc += (v * v) as u64;
    }
    let mean = if n == 0 { 0 } else { acc / n as u64 };
    let (mut x, mut y) = (mean, (mean + 1) / 2);
    if mean < 2 { return mean as u32; }
    while y < x { x = y; y = (x + mean / x) / 2; }
    x as u32
}

/// LPIB while running: printed every 100 ms for the first second, then every second; the one-shot witness
/// line once 300 ms have passed. `moved_ms` is the last time LPIB changed (the stall guard reads it).
fn track(s: &mut St) {
    let (b, sd) = (s.base, s.sd);
    let now = crate::arch::ms();
    let l = r32(b, sd + SD_LPIB);
    if l != s.lpib_last { s.lpib_moved = true; s.lpib_last = l; s.moved_ms = now; }
    if now >= s.next_print {
        let el = now.saturating_sub(s.run_ms);
        serial_println!("[play] t={} lpib={} sts={:#04x} ctl={:#04x} completed={} fifo={} under={}", el, l, r8(b, sd + SD_STS), r8(b, sd + SD_CTL), s.completed, s.fifo.len(), s.underruns);
        s.next_print = now + if el < 1000 { 100 } else { 1000 };
    }
    if !s.witnessed && now.saturating_sub(s.run_ms) >= 300 {
        s.witnessed = true;
        let r = super::stream::last();
        serial_println!("[play] run={} lpib={} lpib0={} lpib_now={} tag={} dac_fmt={} level={}", s.run_bit as u8, s.lpib_moved as u8, s.lpib0, l, r.tag_match as u8, r.fmt_match as u8, s.level);
    }
}

/// `tests playwav` completion state: (done by the ring's own drain, LPIB moved, stalled).
fn progress() -> (bool, bool, bool) {
    let Some(s) = ST.try_lock() else { return (false, false, false) };
    let now = crate::arch::ms();
    let stalled = s.armed && !s.done && ((s.running && now.saturating_sub(s.moved_ms) > 1000)
        || (!s.running && s.armed_ms != 0 && now.saturating_sub(s.armed_ms) > 2000));
    (s.done, s.lpib_moved, stalled)
}

// ── AUDIOCODEC (SR30) M5 — `play <any file>`: audio_core sniffs and decodes, `feed()` takes the 16-bit PCM ──────
// Reached from `open_wav` when the WAV parse refuses a file (any non-RIFF file, and WAVs it does not take: 24/32-bit,
// float, EXTENSIBLE, > 2 channels). Integer only on this side: the decoder hands out left-justified i32, the pump
// downmixes to <= 2 channels with Q15 weights and decimates by an integer step to <= 48 kHz (88.2/96 kHz -> 2:1).
// FLAC, WAV/AIFF and Opus (the fixed-point decoder) are integer inside too; MP3, AAC and Vorbis run on the
// kernel target's soft-float (x86_64-unaos.json: +soft-float) — measured realtime on metal is owed.

/// audio_core's byte source over the VFS: forward reads of at most 32 KiB.
struct VfsSrc { path: String, off: u64 }
impl audio_core::Read for VfsSrc {
    fn read(&mut self, buf: &mut [u8]) -> audio_core::Result<usize> {
        let want = buf.len().min(32 * 1024);
        match crate::shell::vfs_mount_table().read(&self.path, self.off, want) {
            Ok(d) => { let n = d.len().min(want); buf[..n].copy_from_slice(&d[..n]); self.off += n as u64; Ok(n) }
            Err(_) => Err(audio_core::Error::Invalid("vfs read")),
        }
    }
}

// MP3HANG (rmbp B373): the coded player's state, its decode and its verdict live at the file TAIL (`dec_*`):
// the decoder runs on its own `play-dec` task, never on the caller of `open_wav`/`service()` (flight 23: the
// render task, 32 KiB, run off its stack by a 92.5 KiB `Decoder::open` frame). These three keep their names.
fn open_coded(path: &str, wav_reason: String) -> Result<(), String> { dec_open(path, wav_reason) }
fn coded_pump() { dec_pump() }
fn coded_report() { dec_report() }

/// `tests play [fmt]` (and `tests playflac` … through `crate::tests::arg()`): play `TEST.<EXT>` from the user's home,
/// /home or / for each format; a format with no such file SKIPs. The completion is the ring's drain, as in `tests playwav`.
pub fn selftest_codecs() {
    const FMTS: [(&str, &str); 8] = [("wav", "WAV"), ("flac", "FLAC"), ("opus", "OPUS"), ("vorbis", "OGG"), ("mp3", "MP3"), ("aac", "AAC"), ("m4a", "M4A"), ("aiff", "AIF")];
    let t0 = tms(); // DECJOBHANG M3: the shell's time inside this verb (DECJOB shell_blocked_ms)
    let want = crate::tests::arg();
    let mt = crate::shell::vfs_mount_table();
    for (fmt, ext) in FMTS {
        if let Some(w) = want.as_deref() { if w != fmt && !w.eq_ignore_ascii_case(ext) { continue; } }
        let mut cands: Vec<String> = Vec::new();
        #[cfg(feature = "login")]
        {
            let mut b = [0u8; crate::fs::users::NAME_MAX];
            if let Some(n) = crate::fs::users::whoami(&mut b) {
                if let Ok(name) = core::str::from_utf8(&b[..n]) { cands.push(alloc::format!("/home/{}/TEST.{}", name, ext)); }
            }
        }
        cands.push(alloc::format!("/home/TEST.{}", ext));
        if let Some(p) = crate::fs::volumes::testf_find(&mt, &alloc::format!("TEST.{}", ext)) { cands.push(p); } // VOLUMES (B366) M3: system/test-f after /home
        cands.push(alloc::format!("/TEST.{}", ext));
        let Some(path) = cands.into_iter().find(|c| mt.stat(c).is_ok()) else {
            serial_println!(":: PLAYCODEC: fmt={} path=- reason=no-file (put TEST.{} in /home, /system/test-f or /) -> SKIP ::", fmt, ext);
            continue;
        };
        TQ.lock().push_back((fmt, path)); // DECJOBHANG M3: queued; the service tick plays them, the shell returns now
    }
    tq_arm(t0);
}

// ── MP3HANG (rmbp-ledger B373) — the coded player's decode on its OWN task ──────────────────────────────────────
// Flight 23 (image 16): `tests play mp3` → `:: TESTS: run play ::` → nothing on the wire for 18 minutes, keys dead,
// the glass alive. Read and measured (docs/dev/evidence/rmbp-1005/mp3hang.md): `tests` dispatches on the x86 RENDER
// task (32 KiB, `RENDER_PATH_STACK_SIZE`), and `audio_core::Decoder::open` was a 92504-byte frame on this target —
// its inline stack probes zeroed a qword every 4 KiB through the guard into the heap below, and the MP3 arm then
// wrote a 16 KiB decoder there. audio_core's frames are bounded at the source now (M1, the shared core); this is the
// kernel half: NO caller of `open_wav`/`service()` — the render task, a Quarry click, the shell — ever runs a codec
// again. The decoder lives on `play-dec`, a task with a sized stack, and hands 16-bit PCM through a BOUNDED queue
// that `service()` drains into `feed()`. `service()` never waits on it: it reads the decoder's heartbeat, and a
// silence longer than `DEC_STALL_MS` is named on the wire (`[play] mp3 stall stage=… frame=… ms=…`) and ends the play.

/// `play-dec`'s usable stack. Measured, not guessed (`-Z emit-stack-sizes`, x86_64-unaos.json, after M1): the
/// deepest decoder chain is Opus's open (`OggOpus::new` 9352 + `ChannelState::new` 4232 + the open/arm frames,
/// under 16 KiB); MP3's is under 8 KiB (`decode_frame` 5384). The VFS read under `VfsSrc` runs the same chain the
/// 32 KiB render task runs today (RENDSTACK measured that task at 15600 high). 64 KiB = the two plus margin;
/// `[play] dec stack high=` reports the real high-water of every play, so the next flight checks the number.
const DEC_STACK: usize = 160 * 1024; // AUDIOCORE (B396) measured every audio_core open/new under 8 KiB (worst 6040 B) and derived 64 KiB; the SEAT keeps DECJOBHANG's 160 KiB for THIS flight (the kernel's own share of the chain is an estimate) — the flight's `[play] dec exit … stack high=` re-derives it, one line.
/// A decoder that has not advanced in this long while it holds the CPU-side stages (demux/frame/synth) is stalled.
const DEC_STALL_MS: u64 = 2_000;
/// The PCM queue between `play-dec` and `feed()`: at most this many bytes decoded ahead (~0.7 s at 48 kHz stereo).
const DEC_QUEUE: usize = 128 * 1024;
/// `[play] <codec> frames=` every this many decoder calls (each call is up to 4096 source frames).
const DEC_PRINT_EVERY: u32 = 4;
/// `play-dec` gives up waiting for a consumer that stopped draining (a `play stop`, a stalled ring) after this long.
const DEC_ORPHAN_MS: u64 = 10_000;

const STAGE_IDLE: u8 = 0;
const STAGE_DEMUX: u8 = 1; // `Decoder::open`: container + header parse
const STAGE_ARM: u8 = 2; // opened; waiting for `service()` to arm the stream and print `[play] open`
const STAGE_FRAME: u8 = 3; // inside `next_i32` (the codec's frame decode, synthesis included)
const STAGE_SYNTH: u8 = 4; // our side: downmix / decimate / pack to 16-bit
const STAGE_DMA: u8 = 5; // the queue is full: waiting on the ring to drain (not a decoder stall)
const STAGE_DONE: u8 = 6;
const STAGE_QUEUED: u8 = 7; // DECJOBHANG (B386): spawned, its first instruction not yet run

fn stage_name(s: u8) -> &'static str {
    match s { STAGE_QUEUED => "queued", STAGE_DEMUX => "demux", STAGE_ARM => "arm", STAGE_FRAME => "frame", STAGE_SYNTH => "synth", STAGE_DMA => "dma", STAGE_DONE => "done", _ => "idle" }
}

static DEC_STAGE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(STAGE_IDLE);
static DEC_BEAT_MS: AtomicU64 = AtomicU64::new(0);
static DEC_CALLS: AtomicU64 = AtomicU64::new(0);
static DEC_LIVE: AtomicBool = AtomicBool::new(false);
static DEC_ABORT: AtomicBool = AtomicBool::new(false);
static DEC_ARMED: AtomicBool = AtomicBool::new(false);
static DEC_GEN: AtomicU32 = AtomicU32::new(0);
/// The last coded play's guard facts, for `tests play`'s MP3GUARD line: (stall stage or 0, heartbeat lines, stack high).
static DEC_STALL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static DEC_BEATS: AtomicU32 = AtomicU32::new(0);
static DEC_HIGH: AtomicU64 = AtomicU64::new(0);

/// What `play-dec` hands across (under one lock, held only for a copy).
struct DecOut {
    jid: u32,
    path: String,
    info: Option<audio_core::Info>,
    open_err: Option<String>,
    wav_reason: String,
    out_ch: usize, step: usize,
    pcm: VecDeque<u8>,
    decoded: u64, fed: u64, eos: bool, err: Option<audio_core::Error>,
    /// `dec_pump` has armed (or refused) this play — it is never armed twice.
    consumed: bool,
}
static DEC_OUT: spin::Mutex<Option<DecOut>> = spin::Mutex::new(None);

/// The service side's view of the current coded play.
struct Coded { path: String, info: audio_core::Info, step: usize, armed: bool, fed_end: bool, open_ms: u64 }
static CODED: spin::Mutex<Option<Coded>> = spin::Mutex::new(None);

/// `open_coded`: hand `path` to a fresh `play-dec` task. Returns at once; the stream is armed by `dec_pump` once the
/// decoder has read the headers. One decoder at a time: a live (or wedged) one refuses the next open, by name.
fn dec_open(path: &str, wav_reason: String) -> Result<(), String> {
    if DEC_LIVE.load(Ordering::Acquire) {
        let st = stage_name(DEC_STAGE.load(Ordering::Acquire));
        serial_println!(":: PLAYWAV: path={} reason=decoder busy (stage={}) -> REFUSED ::", path, st);
        return Err(alloc::format!("decoder busy (stage={})", st));
    }
    stop();
    let jid = DEC_GEN.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    *DEC_OUT.lock() = Some(DecOut { jid, path: String::from(path), info: None, open_err: None, wav_reason, out_ch: 1, step: 1,
                                    pcm: VecDeque::new(), decoded: 0, fed: 0, eos: false, err: None, consumed: false });
    *CODED.lock() = None;
    DEC_ABORT.store(false, Ordering::Release);
    DEC_ARMED.store(false, Ordering::Release);
    DEC_CALLS.store(0, Ordering::Release);
    DEC_STALL.store(0, Ordering::Release);
    DEC_BEATS.store(0, Ordering::Release);
    DEC_HIGH.store(0, Ordering::Release);
    DEC_STAGE.store(STAGE_QUEUED, Ordering::Release); // DECJOBHANG M2: the guard's clock starts at the task's first beat
    DEC_BEAT_MS.store(tms(), Ordering::Release);
    DEC_SPAWN_MS.store(tms(), Ordering::Release);
    DEC_LIVE.store(true, Ordering::Release);
    let (cpu, on) = dec_cpu();
    dj_spawned(on == "worker", jid); DEC_CPU.store(if cpu == crate::arch::sched::CPU_AUTO { u32::MAX } else { cpu as u32 }, Ordering::Release);
    serial_println!("[play] dec spawn path={} jid={} stack={} cpu={} on={}", path, jid, DEC_STACK, if cpu == crate::arch::sched::CPU_AUTO { -1 } else { cpu as i64 }, on);
    crate::arch::sched::spawn_stack("play-dec", dec_task, jid as usize, cpu, crate::arch::sched::PRIO_NORMAL, DEC_STACK);
    ACTIVE.store(true, Ordering::Release);
    Ok(())
}

fn dec_beat(stage: u8) {
    DEC_STAGE.store(stage, Ordering::Release);
    DEC_BEAT_MS.store(tms(), Ordering::Release);
}

/// The `play-dec` task. Everything that can run long or deep — the container parse, every frame decode, the
/// downmix — runs here, in bounded steps (one `next_i32` call of at most 4096 frames), with a heartbeat between.
fn dec_task(arg: usize) {
    use audio_core::AudioDecoder;
    let jid = arg as u32;
    let mine = || DEC_GEN.load(Ordering::Acquire) == jid && !DEC_ABORT.load(Ordering::Acquire);
    let paint = dec_stack_paint();
    let path = match DEC_OUT.lock().as_ref() { Some(o) if o.jid == jid => o.path.clone(), _ => { dec_exit(jid, paint, "superseded"); return; } };
    dec_ran(jid);
    dec_beat(STAGE_DEMUX);
    let opened = audio_core::Decoder::open(alloc::boxed::Box::new(VfsSrc { path: path.clone(), off: 0 }));
    let mut dec = match opened {
        Ok(d) => d,
        Err(e) => {
            if let Some(o) = DEC_OUT.lock().as_mut().filter(|o| o.jid == jid) { o.open_err = Some(alloc::format!("{}; audio_core: {:?}", o.wav_reason, e)); }
            dec_exit(jid, paint, "open-refused");
            return;
        }
    };
    let info = dec.info();
    let step = (info.rate as usize).div_ceil(48_000).max(1);
    let out_ch = if info.channels >= 2 { 2 } else { 1 };
    if let Some(o) = DEC_OUT.lock().as_mut().filter(|o| o.jid == jid) { o.info = Some(info); o.step = step; o.out_ch = out_ch; }
    // `[play] open` is printed by `dec_pump` when it arms the stream: no frame is decoded before it is on the wire.
    dec_beat(STAGE_ARM);
    while mine() && !DEC_ARMED.load(Ordering::Acquire) {
        if tms().saturating_sub(DEC_BEAT_MS.load(Ordering::Acquire)) > DEC_ORPHAN_MS { dec_exit(jid, paint, "never-armed"); return; }
        crate::arch::sched::sleep_ms(2);
    }
    let ch = info.channels as usize;
    let mut buf: Vec<i32> = alloc::vec![0; 4096 * step * ch.max(1)];
    let mut phase = 0usize;
    let codec = alloc::format!("{:?}", info.codec).to_ascii_lowercase();
    let t0 = crate::arch::ms();
    let mut calls = 0u32;
    let mut waited_ms = 0u64;
    let mut skip = seek_skip(jid, info.rate); // PLAYER (B419): a seek's decode-skip (0 for every other open)
    loop {
        if !mine() { dec_exit(jid, paint, "aborted"); return; }
        // the bounded queue: decode ahead no further than DEC_QUEUE; a consumer gone for DEC_ORPHAN_MS ends the task
        let queued = DEC_OUT.lock().as_ref().map(|o| o.pcm.len()).unwrap_or(0);
        if queued >= DEC_QUEUE {
            dec_beat(STAGE_DMA);
            crate::arch::sched::sleep_ms(2);
            waited_ms += 2;
            if waited_ms > DEC_ORPHAN_MS && !PAUSED.load(Ordering::Acquire) { dec_exit(jid, paint, "consumer-gone"); return; } // PLAYER (B419): a paused consumer is not gone
            continue;
        }
        waited_ms = 0;
        dec_beat(STAGE_FRAME);
        let (n, err) = match dec.next_i32(&mut buf) { Ok(n) => (n, None), Err(e) => (0, Some(e)) };
        calls += 1;
        DEC_CALLS.store(calls as u64, Ordering::Release);
        dec_beat(STAGE_SYNTH);
        let mut pend: Vec<u8> = Vec::with_capacity(n / step * out_ch * 2 + 4);
        let mut fed = 0u64;
        // downmix: the first two channels carry, every further one adds half weight to both; scaled so a full
        // scale input cannot clip (Q15) — the arithmetic `coded_pump` ran before MP3HANG, unchanged
        let w0: i64 = if ch <= 2 { 32_768 } else { 65_536 / ch as i64 };
        let s0 = (skip.min(n as u64)) as usize; skip -= s0 as u64; // PLAYER (B419): frames before the seek target are decoded and dropped
        for i in s0..n {
            let ph = phase; phase = (phase + 1) % step;
            if ph != 0 { continue; }
            let f = &buf[i * ch..i * ch + ch];
            let v = |k: usize| (f[k] >> 16) as i64;
            let (l, r) = if ch == 1 { (v(0), v(0)) } else if ch == 2 { (v(0), v(1)) } else {
                let rest: i64 = (2..ch).map(v).sum();
                ((v(0) * w0 + rest * w0 / 2) >> 15, (v(1) * w0 + rest * w0 / 2) >> 15)
            };
            let (l, r) = (l.clamp(-32_768, 32_767) as i16, r.clamp(-32_768, 32_767) as i16);
            pend.extend_from_slice(&l.to_le_bytes());
            if out_ch == 2 { pend.extend_from_slice(&r.to_le_bytes()); }
            fed += 1;
        }
        let (decoded, eos) = {
            let mut g = DEC_OUT.lock();
            let Some(o) = g.as_mut().filter(|o| o.jid == jid) else { drop(g); dec_exit(jid, paint, "superseded"); return; };
            o.pcm.extend(pend.iter().copied());
            o.decoded += n as u64;
            o.fed += fed;
            if err.is_some() { o.err = err; }
            if n == 0 { o.eos = true; }
            (o.decoded, o.eos)
        };
        if eos || calls % DEC_PRINT_EVERY == 0 {
            DEC_BEATS.fetch_add(1, Ordering::AcqRel);
            serial_println!("[play] {} frames={} calls={} ms={}{}", codec, decoded, calls, crate::arch::ms().saturating_sub(t0), if eos { " eos=1" } else { "" });
        }
        if eos { dec_exit(jid, paint, "eos"); return; }
        crate::arch::sched::yield_now();
    }
}

/// `play-dec`'s end, every path: the stack high-water on the wire, the task's liveness released.
fn dec_exit(jid: u32, paint: Option<(u64, u64)>, why: &str) {
    let high = dec_stack_high(paint);
    DEC_HIGH.store(high, Ordering::Release);
    serial_println!("[play] dec exit jid={} why={} stage={} calls={} stack high={} of {}", jid, why, stage_name(DEC_STAGE.load(Ordering::Acquire)), DEC_CALLS.load(Ordering::Acquire), high, DEC_STACK);
    if DEC_GEN.load(Ordering::Acquire) == jid { DEC_STAGE.store(STAGE_DONE, Ordering::Release); DEC_LIVE.store(false, Ordering::Release); } // DECJOBHANG M1: a straggler of an older jid never releases the current one
}

const DEC_PAINT: u8 = 0xC3;

/// Paint this task's own unused stack (from its low bound, past the guard, up to 1 KiB under the live frame) so
/// `dec_stack_high` can read the deepest point the decode reached — the measurement `DEC_STACK` is checked by.
fn dec_stack_paint() -> Option<(u64, u64)> {
    let (low, top) = crate::arch::sched::current_stack_bounds()?;
    let marker = 0u8;
    let here = &marker as *const u8 as u64;
    if here <= low + 2048 || here > top { return None; }
    let end = here - 1024;
    let mut a = low;
    while a < end {
        // SAFETY: [low, here - 1 KiB) is this task's own stack below its live frame (the bounds are the scheduler's
        // for the task on this core); nothing lives there yet, and an interrupt frame that lands later overwrites
        // the paint, which is what the reading counts.
        unsafe { core::ptr::write_volatile(a as *mut u8, DEC_PAINT); }
        a += 1;
    }
    Some((low, top))
}

fn dec_stack_high(paint: Option<(u64, u64)>) -> u64 {
    let Some((low, top)) = paint else { return 0 };
    let mut a = low;
    // SAFETY: the same span `dec_stack_paint` wrote, on this task's own stack, read below the live frame.
    while a < top && unsafe { core::ptr::read_volatile(a as *const u8) } == DEC_PAINT { a += 1; }
    top - a
}

/// `coded_pump` (the service tick, whatever task runs it): arm the stream once the decoder has the headers, move
/// decoded PCM into `feed()`, and watch the heartbeat. Never calls into a codec; every lock here is held for a copy.
fn dec_pump() {
    let jid = DEC_GEN.load(Ordering::Acquire);
    // 1) the decoder's open finished: arm the HDA stream here, on the service side, and say so BEFORE any frame
    let need_arm = CODED.lock().is_none();
    if need_arm {
        let (path, info, open_err, step, out_ch) = {
            let mut g = DEC_OUT.lock();
            let Some(o) = g.as_mut().filter(|o| o.jid == jid && !o.consumed) else { return };
            if o.open_err.is_none() && o.info.is_none() { drop(g); dec_watch(jid); return; } // still in the demux
            o.consumed = true;
            (o.path.clone(), o.info, o.open_err.clone(), o.step, o.out_ch)
        };
        if let Some(r) = open_err {
            serial_println!(":: PLAYWAV: path={} reason={} -> REFUSED ::", path, r);
            dec_end_play();
            return;
        }
        if let Some(info) = info {
            let rate = info.rate / step as u32;
            match start(rate, out_ch as u8, 16) {
                Ok(eff) => {
                    serial_println!("[play] open path={} format={} codec={:?} rate={} ch={} bits={} frames={:?} decim={} out_ch={} eff_rate={} resampled={}",
                        path, info.format.name(), info.codec, info.rate, info.channels, info.bits, info.frames, step, out_ch, eff, (eff != rate) as u8);
                    *CODED.lock() = Some(Coded { path, info, step, armed: true, fed_end: false, open_ms: crate::arch::ms() });
                    ACTIVE.store(true, Ordering::Release);
                    DEC_ARMED.store(true, Ordering::Release);
                }
                Err(r) => {
                    serial_println!(":: PLAYWAV: path={} reason={} ({} Hz / {} ch from {}) -> REFUSED ::", path, r, rate, out_ch, info.format.name());
                    dec_end_play();
                    return;
                }
            }
        }
    }
    // 2) the watchdog
    if dec_watch(jid) { return; }
    // 3) PCM across: whole chunks of at most 16 KiB, all-or-nothing as `feed()` takes them
    let armed = CODED.lock().as_ref().map(|c| c.armed && !c.fed_end).unwrap_or(false);
    if !armed { return; }
    dec_feed(jid);
}

/// The watchdog: a decoder silent in a CPU stage (demux/frame/synth) for `DEC_STALL_MS` is named on the wire and
/// the play ends. Returns true when it ended the play.
fn dec_watch(jid: u32) -> bool {
    let stage = DEC_STAGE.load(Ordering::Acquire);
    if DEC_LIVE.load(Ordering::Acquire) && stage == STAGE_QUEUED && dec_queued_watch(jid) { return true; }
    if DEC_LIVE.load(Ordering::Acquire) && matches!(stage, STAGE_DEMUX | STAGE_FRAME | STAGE_SYNTH) {
        let silent = tms().saturating_sub(DEC_BEAT_MS.load(Ordering::Acquire));
        if silent > DEC_STALL_MS {
            let (codec, frames) = {
                let g = DEC_OUT.lock();
                let o = g.as_ref().filter(|o| o.jid == jid);
                (o.and_then(|o| o.info).map(|i| alloc::format!("{:?}", i.codec).to_ascii_lowercase()).unwrap_or_else(|| String::from("coded")), o.map(|o| o.decoded).unwrap_or(0))
            };
            serial_println!("[play] {} stall stage={} frame={} calls={} ms={} -> ABORT", codec, stage_name(stage), frames, DEC_CALLS.load(Ordering::Acquire), silent);
            DEC_STALL.store(stage, Ordering::Release);
            DEC_ABORT.store(true, Ordering::Release);
            DEC_LIVE.store(false, Ordering::Release); // DECJOBHANG M1: a job the guard ended (a halted task never reaches `dec_exit`) frees the decoder for the next open
            stop();
            dec_end_play();
            return true;
        }
    }
    false
}

/// Decoded PCM into `feed()`: whole chunks of at most 16 KiB, all-or-nothing as `feed()` takes them.
fn dec_feed(jid: u32) {
    for _ in 0..8 {
        let (chunk, eos_empty) = {
            let mut g = DEC_OUT.lock();
            let Some(o) = g.as_mut().filter(|o| o.jid == jid) else { return };
            let n = o.pcm.len().min(16 * 1024) & !3usize;
            let n = if n == 0 && o.eos { o.pcm.len() } else { n };
            (o.pcm.iter().take(n).copied().collect::<Vec<u8>>(), o.eos && o.pcm.len() == n)
        };
        if chunk.is_empty() {
            if eos_empty { if let Some(c) = CODED.lock().as_mut() { c.fed_end = true; } finish(); }
            return;
        }
        if feed(&chunk) == 0 { return; } // no room: retry next tick
        let mut g = DEC_OUT.lock();
        if let Some(o) = g.as_mut().filter(|o| o.jid == jid) { o.pcm.drain(..chunk.len()); }
        drop(g);
        if eos_empty { if let Some(c) = CODED.lock().as_mut() { c.fed_end = true; } finish(); return; }
    }
}

/// A coded play that ends without a verdict from the ring (refused open, a stall): the player goes idle.
fn dec_end_play() {
    DEC_ABORT.store(true, Ordering::Release);
    *CODED.lock() = None;
    ACTIVE.store(false, Ordering::Release);
}

/// `coded_report`: the coded player's verdict, once the ring has drained (`service()`, after `report()`).
fn dec_report() {
    let c = CODED.lock().take();
    let Some(c) = c else { return };
    let (decoded, fed, err) = {
        let g = DEC_OUT.lock();
        match g.as_ref() { Some(o) => (o.decoded, o.fed, o.err.clone()), None => (0, 0, None) }
    };
    let s = ST.lock();
    let rate = c.info.rate / c.step as u32;
    let stated_ok = c.info.frames.map(|f| f == decoded).unwrap_or(true);
    let ok = s.in_frames == fed && err.is_none() && stated_ok && s.underruns == 0 && s.err == 0 && s.completed >= 1 && s.lpib_moved && s.done;
    let ms = fed * 1000 / rate.max(1) as u64;
    serial_println!("[play] done resampled={} eff_rate={} entries={} fifoe_dese={} run_bit={} lpib_moved={} level={} play_ms={}", s.resampled as u8, s.eff_rate, s.completed, s.err, s.run_bit as u8, s.lpib_moved as u8, s.level, crate::arch::ms().saturating_sub(c.open_ms));
    serial_println!(":: PLAYCODEC: path={} format={} codec={:?} rate={} frames={} lpib_moved={} done={} -> {} :: ch={} decoded={} stated={:?} decim={} err={:?} secs={}.{} under={} ::",
        c.path, c.info.format.name(), c.info.codec, c.info.rate, s.in_frames, s.lpib_moved as u8, s.done as u8, if ok { "PASS" } else { "FAIL" },
        c.info.channels, decoded, c.info.frames, c.step, err, ms / 1000, (ms % 1000) / 100, s.underruns);
    drop(s);
    DEC_ABORT.store(true, Ordering::Release); // the task has exited at eos; a straggler stops at its next step
}

/// `play stop`: end a coded play's decoder too.
fn dec_stop() { if DEC_LIVE.load(Ordering::Acquire) { DEC_ABORT.store(true, Ordering::Release); } *CODED.lock() = None; }

/// `tests play`: the guard's verdict for one coded play. `gap_ms` is the longest the WAITING task (the shell's own,
/// the one that drains the keys and owns the console) went between two passes of its loop — the decoder never ran
/// on it, so a decoder fault cannot hold it; `sink_alive` is the decoder's own heartbeat lines reaching the wire.
fn dec_guard(fmt: &str, path: &str, gap_ms: u64) {
    let codec = DEC_OUT.lock().as_ref().and_then(|o| o.info).map(|i| i.codec);
    let stall = DEC_STALL.load(Ordering::Acquire);
    let beats = DEC_BEATS.load(Ordering::Acquire);
    let keys_alive = gap_ms <= 250;
    let sink_alive = beats >= 1 || stall != 0;
    let ok = stall == 0 && keys_alive && sink_alive;
    let st = if stall == 0 { "none" } else { stage_name(stall) };
    if stall != 0 { serial_println!(":: PLAYCODEC: fmt={} path={} reason=decoder-stall stage={} -> FAIL ::", fmt, path, st); }
    if matches!(codec, Some(audio_core::Codec::Mp3)) || fmt == "mp3" {
        serial_println!(":: MP3GUARD: stall={} keys_alive={} sink_alive={} -> {} :: gap_ms={} beats={} dec_stack_high={} of {} task=play-dec ::",
            st, keys_alive as u8, sink_alive as u8, if ok { "PASS" } else { "FAIL" }, gap_ms, beats, DEC_HIGH.load(Ordering::Acquire), DEC_STACK);
    } else {
        serial_println!("[play] guard fmt={} stall={} keys_alive={} sink_alive={} gap_ms={} beats={} dec_stack_high={} of {}",
            fmt, st, keys_alive as u8, sink_alive as u8, gap_ms, beats, DEC_HIGH.load(Ordering::Acquire), DEC_STACK);
    }
}

// ── DECJOBHANG (rmbp-ledger B386) — the decoder job's placement, its own clock, and a `tests play` that never ──────
// blocks the shell. Flights 24/25: `dec spawn … cpu=0` — `other_dispatching_cpu()` is the lowest core that is not the
// caller's, i.e. always the BSP, the one core that advances `arch::ms()`; every UnaFS read runs masked
// (`with_unafs`), so the decoder's reads put masked spans on the clock every guard here was measured with, while
// the shell spun in `tests play` waiting on that clock. Flight 24's reboot: the job ran and overflowed 64 KiB in
// the AAC constructor, never reached `dec_exit`, and held `DEC_LIVE` for the rest of the boot (every later open
// "decoder busy"). Design and measurements: docs/dev/evidence/rmbp-1005/decjobhang.md.

/// Milliseconds from the TSC — not the BSP's tick, so a masked or wedged clock core cannot blind the guard.
fn tms() -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    if hz >= 1000 { crate::arch::now_cycles() / (hz / 1000) } else { crate::arch::ms() }
}

/// `play-dec`'s core: the first worker-pool core (`smp::worker_cpu` — neither the render nor the service core)
/// that is not the caller's and not the BSP; `CPU_AUTO` (named `on=auto`) only when the pool has none.
fn dec_cpu() -> (usize, &'static str) {
    let here = crate::arch::percpu::this_cpu().cpu_index as usize;
    for n in 0..crate::arch::gdt::MAX_CPUS {
        match crate::arch::smp::worker_cpu(n) {
            Some(c) if c != here && c != 0 => return (c, "worker"),
            Some(_) => continue,
            None => break,
        }
    }
    (crate::arch::sched::CPU_AUTO, "auto")
}

static DEC_SPAWN_MS: AtomicU64 = AtomicU64::new(0);
/// The jid `[play] dec not-scheduled` was printed for (once per job).
static DEC_NS_SAID: AtomicU32 = AtomicU32::new(0);
/// `DECJOB` counters since the last `tests play`: spawned, ran, spawned on a worker core.
static DJ_SPAWNED: AtomicU32 = AtomicU32::new(0);
static DJ_RAN: AtomicU32 = AtomicU32::new(0);
static DJ_WORKER: AtomicU32 = AtomicU32::new(0);

fn dj_spawned(worker: bool, _jid: u32) {
    DJ_SPAWNED.fetch_add(1, Ordering::AcqRel);
    if worker { DJ_WORKER.fetch_add(1, Ordering::AcqRel); }
}

/// The task's first act: one line — it ran, where, and how long after its spawn.
fn dec_ran(jid: u32) {
    DJ_RAN.fetch_add(1, Ordering::AcqRel);
    let cpu = crate::arch::percpu::this_cpu().cpu_index;
    serial_println!("[play] dec run jid={} cpu={} wait_ms={}", jid, cpu, tms().saturating_sub(DEC_SPAWN_MS.load(Ordering::Acquire)));
}

/// A job still queued: named once at 500 ms (a scheduling fault, told apart from a decode fault), ended at
/// `DEC_ORPHAN_MS`. Returns true when it ended the play.
fn dec_queued_watch(jid: u32) -> bool {
    let waited = tms().saturating_sub(DEC_SPAWN_MS.load(Ordering::Acquire));
    if waited > 500 && DEC_NS_SAID.swap(jid, Ordering::AcqRel) != jid {
        let cpu = match DEC_CPU.load(Ordering::Acquire) { u32::MAX => -1, c => c as i64 };
        serial_println!("[play] dec not-scheduled jid={} cpu={} ms={}", jid, cpu, waited);
    }
    if waited > DEC_ORPHAN_MS {
        serial_println!("[play] coded stall stage=queued frame=0 calls=0 ms={} -> ABORT", waited);
        DEC_STALL.store(STAGE_QUEUED, Ordering::Release);
        DEC_ABORT.store(true, Ordering::Release);
        DEC_LIVE.store(false, Ordering::Release);
        stop();
        dec_end_play();
        return true;
    }
    false
}
static DEC_CPU: AtomicU32 = AtomicU32::new(u32::MAX);

/// `tests play`'s queue: (fmt, path) to play in order, the one now playing (fmt, path, decoder generation before
/// it), and the shell's own time in the verb. The service tick drives it; the shell is never in it.
static TQ: spin::Mutex<VecDeque<(&'static str, String)>> = spin::Mutex::new(VecDeque::new());
static TQ_CUR: spin::Mutex<Option<(&'static str, String, u32)>> = spin::Mutex::new(None);
static TQ_LIVE: AtomicBool = AtomicBool::new(false);
static TQ_IN: AtomicBool = AtomicBool::new(false);
static TQ_SHELL_MS: AtomicU64 = AtomicU64::new(0);

/// End of `tests play`: reset the counters, arm the first entry, and return to the shell.
fn tq_arm(t0: u64) {
    DJ_SPAWNED.store(0, Ordering::Release);
    DJ_RAN.store(0, Ordering::Release);
    DJ_WORKER.store(0, Ordering::Release);
    let n = TQ.lock().len();
    TQ_LIVE.store(n > 0, Ordering::Release);
    TQ_SHELL_MS.store(tms().saturating_sub(t0), Ordering::Release);
    serial_println!("[play] tests queued n={} shell_ms={} (the plays run on the service tick; `play stop` ends one)", n, TQ_SHELL_MS.load(Ordering::Acquire));
}

/// One service tick of the queue: close the entry that finished (its guard verdict), then open the next; when the
/// queue is empty, the `DECJOB` verdict. Re-entry (`start()` → `probe()` → `service()`) is skipped.
fn tq_tick() {
    if TQ_IN.swap(true, Ordering::AcqRel) { return; }
    let cur = TQ_CUR.lock().clone();
    if let Some((fmt, path, jid0)) = cur {
        let (_done, _moved, st) = progress();
        if st {
            let (lp, run_bit, fifo) = { let s = ST.lock(); (s.lpib_last, s.run_bit as u8, s.fifo.len()) };
            stop(); dec_stop();
            serial_println!(":: PLAYCODEC: fmt={} path={} reason=stalled lpib={} run_bit={} fifo={} -> FAIL ::", fmt, path, lp, run_bit, fifo);
        }
        if ACTIVE.load(Ordering::Acquire) { TQ_IN.store(false, Ordering::Release); return; }
        if DEC_GEN.load(Ordering::Acquire) != jid0 { dec_guard(fmt, &path, TQ_SHELL_MS.load(Ordering::Acquire)); }
        *TQ_CUR.lock() = None;
    }
    let next = TQ.lock().pop_front();
    match next {
        Some((fmt, path)) => {
            let jid0 = DEC_GEN.load(Ordering::Acquire);
            *TQ_CUR.lock() = Some((fmt, path.clone(), jid0));
            if let Err(r) = open_wav(&path) {
                serial_println!(":: PLAYCODEC: fmt={} path={} reason={} -> SKIP ::", fmt, path, r);
                *TQ_CUR.lock() = None;
            }
        }
        None => {
            TQ_LIVE.store(false, Ordering::Release);
            let (sp, ran, wk) = (DJ_SPAWNED.load(Ordering::Acquire), DJ_RAN.load(Ordering::Acquire), DJ_WORKER.load(Ordering::Acquire));
            let shell = TQ_SHELL_MS.load(Ordering::Acquire);
            let on = if sp == 0 { "-" } else if wk == sp { "worker" } else { "auto" };
            let v = if sp == 0 { "SKIP" } else if ran == sp && wk == sp && shell <= 250 { "PASS" } else { "FAIL" };
            serial_println!(":: DECJOB: spawned={} ran={} on={} shell_blocked_ms={} -> {} ::", sp, ran, on, shell, v);
        }
    }
    TQ_IN.store(false, Ordering::Release);
}

// ── PLAYER (rmbp-ledger B419, MACPARITY row 30) — the transport the Player window drives: pause, seek, position ──────
// The window is `video/player.rs`; the play stays here. Pause clears the stream's RUN bit (the amp ramped down first)
// and `ring_pump` leaves a paused stream alone; the decoder waits instead of calling its consumer gone. Seek: a WAV
// re-arms the stream at the target frame (`method=pcm-exact`); a coded file has NO seek table in audio_core (and
// `demux_core::Demuxer::seek` is the video container's keyframe seek — audio_core's MP4 does not route through it),
// so a superseding `play-dec` job decodes from the start and drops the frames before the target (`method=decode-skip
// table=none`): exact, at the cost of decode time. Design: docs/dev/evidence/rmbp-1005/player.md.

static PAUSED: AtomicBool = AtomicBool::new(false);
/// The job a seek's skip belongs to (the jid `dec_open` is about to mint) and the target in ms.
static SEEK_JID: AtomicU32 = AtomicU32::new(0);
static SEEK_MS: AtomicU64 = AtomicU64::new(0);

/// `play-dec`'s skip for job `jid` at the source `rate`: frames to drop, and the seek line on the wire.
fn seek_skip(jid: u32, rate: u32) -> u64 {
    if SEEK_JID.load(Ordering::Acquire) != jid || jid == 0 { return 0; }
    let ms = SEEK_MS.load(Ordering::Acquire);
    let frames = ms * rate as u64 / 1000;
    let landed = if rate == 0 { 0 } else { frames * 1000 / rate as u64 };
    serial_println!("[play] seek to_ms={} landed_ms={} method=decode-skip table=none frames={} jid={} (audio_core has no seek table)", ms, landed, frames, jid);
    frames
}

/// Pause (`on`) or resume the stream. `Some(run_bit readback)` while a stream is armed; `None` when nothing is.
pub fn pause(on: bool) -> Option<bool> {
    PAUSED.store(on, Ordering::Release);
    let mut s = ST.lock();
    if !s.armed || s.done { return None; }
    if !s.running {
        serial_println!("[play] pause on={} run_bit=0 (stream not yet running)", on as u8);
        return Some(false);
    }
    let (b, sd) = (s.base, s.sd);
    if on {
        super::amp::ramp_svc(false);
        w8(b, sd + SD_CTL, r8(b, sd + SD_CTL) & !(SDCTL_RUN as u8));
        wait_us(10_000, || r8(b, sd + SD_CTL) & SDCTL_RUN as u8 == 0);
    } else {
        w8(b, sd + SD_CTL, r8(b, sd + SD_CTL) | SDCTL_RUN as u8);
        super::amp::ramp_svc(true);
        s.moved_ms = crate::arch::ms();
    }
    let rb = r8(b, sd + SD_CTL) & SDCTL_RUN as u8 != 0;
    serial_println!("[play] pause on={} run_bit={}", on as u8, rb as u8);
    Some(rb)
}

/// Is the stream paused?
pub fn paused() -> bool { PAUSED.load(Ordering::Acquire) }

/// Is a play in flight (decoder opening, stream armed or draining)?
pub fn busy() -> bool { ACTIVE.load(Ordering::Acquire) }

/// The stream's RUN bit, read back (false when nothing is armed).
pub fn run_bit() -> bool {
    let s = ST.lock();
    s.armed && !s.done && s.running && r8(s.base, s.sd + SD_CTL) & SDCTL_RUN as u8 != 0
}

/// Milliseconds the ring has played since this stream's RUN (entries completed + LPIB within the current one).
pub fn position_ms() -> u64 {
    let Some(s) = ST.try_lock() else { return u64::MAX };
    if !s.running { return 0; }
    let bytes = s.completed * ENTRY_BYTES as u64 + (s.lpib_last as u64 % ENTRY_BYTES as u64);
    bytes / 4 * 1000 / s.eff_rate.max(1) as u64
}

/// What the file is, from the player's own parse: a WAV's `(rate, ch, bits, frames)` header facts, or for a coded
/// file the decoder's `Info` once `play-dec` has opened it (`None` until then, or for another path).
pub fn facts(path: &str) -> Option<(u32, u16, u16, Option<u64>, &'static str)> {
    if let Ok(w) = parse(path) {
        return Some((w.rate, w.ch as u16, w.bits as u16, Some(w.data_len / (w.ch * w.bits / 8) as u64), "pcm"));
    }
    let g = DEC_OUT.lock();
    let o = g.as_ref().filter(|o| o.path == path)?;
    let i = o.info?;
    let codec = match i.codec {
        audio_core::Codec::Pcm => "pcm", audio_core::Codec::Flac => "flac", audio_core::Codec::Opus => "opus",
        audio_core::Codec::Vorbis => "vorbis", audio_core::Codec::Mp3 => "mp3", audio_core::Codec::Aac => "aac",
    };
    Some((i.rate, i.channels, i.bits, i.frames, codec))
}

/// Seek `path` to `ms`: re-arm the play there. `Ok((landed_ms, method))`. Whatever was playing stops first.
pub fn seek_to(path: &str, ms: u64) -> Result<(u64, &'static str), String> {
    if let Ok(mut w) = parse(path) {
        let fin = (w.ch * w.bits / 8) as u64;
        let total = w.data_len / fin;
        let frame = (ms * w.rate as u64 / 1000).min(total);
        let landed = frame * 1000 / w.rate.max(1) as u64;
        if DEC_LIVE.load(Ordering::Acquire) { dec_stop(); }
        let eff = start(w.rate, w.ch as u8, w.bits as u8).map_err(String::from)?;
        w.pos = frame * fin;
        w.start = w.pos;
        serial_println!("[play] seek to_ms={} landed_ms={} method=pcm-exact table=pcm frame={} of {} eff_rate={} path={}", ms, landed, frame, total, eff, path);
        *WAV.lock() = Some(w);
        ACTIVE.store(true, Ordering::Release);
        return Ok((landed, "pcm-exact"));
    }
    // coded: supersede the live job (its `mine()` goes false at its next step; its exit leaves the new job's liveness)
    DEC_LIVE.store(false, Ordering::Release);
    SEEK_MS.store(ms, Ordering::Release);
    SEEK_JID.store(DEC_GEN.load(Ordering::Acquire).wrapping_add(1), Ordering::Release);
    dec_open(path, String::from("seek: not a PCM WAV"))?;
    Ok((ms, "decode-skip"))
}

/// The Player's open: whatever plays stops (a live decoder is aborted and superseded, never "busy"), then the
/// ordinary `open_wav` (WAV pump, else the coded `play-dec` job).
pub fn open_player(path: &str) -> Result<(), String> {
    dec_stop();
    DEC_LIVE.store(false, Ordering::Release);
    open_wav(path)
}

/// The Player's close: the stream and any decoder end now.
pub fn stop_all() {
    stop();
    dec_stop();
}

/// Drive the service tick from a typed fixture until `until()` holds or `ms` pass (`tests player`).
pub fn pump_until(ms: u64, until: impl Fn() -> bool) -> bool {
    let t0 = tms();
    loop {
        service();
        if until() { return true; }
        if tms().saturating_sub(t0) > ms { return false; }
        delay_us(2_000);
    }
}

/// The PLAYWAV fixture's 2.0 s, 48 kHz stereo body (for `tests player` when no TEST.WAV is staged).
pub fn fixture_wav() -> Vec<u8> { synth_wav() }

// ── NOTIFYPANE (rmbp-ledger B435, MACPARITY row 26) — the alert sound ───────────────────────────────────────────
// Our own chime (not a copied system sound): 880 Hz + 1320 Hz (a fifth), 180 ms, a fast attack and a squared
// decay, mono 16-bit at 48 kHz through this file's own `start` / `feed` / `finish`. NOTIFY asks with
// [`request_alert`] (an atomic latch, safe from its pass); the device-service tick plays it. Never while the
// output is already sounding (a player, a WAV, a decoder, another alert): refused `busy`, nothing stacked.

static ALERT: AtomicBool = AtomicBool::new(false);
const ALERT_RATE: u32 = 48_000;
const ALERT_MS: usize = 180;

/// Ask for one alert sound on the next service tick. `Err("busy")` while the output is sounding or one is latched.
pub fn request_alert() -> Result<(), &'static str> {
    if busy() || DEC_LIVE.load(Ordering::Acquire) || ALERT.load(Ordering::Acquire) {
        return Err("busy");
    }
    ALERT.store(true, Ordering::Release);
    Ok(())
}

/// One partial of the chime at sample `n` (Bhaskara's sine, as `synth_wav`), amplitude `amp`.
fn alert_partial(n: usize, hz: u64, amp: i64) -> i64 {
    const H: i64 = 32_768;
    let ph = ((n as u64 * hz * 65_536 / ALERT_RATE as u64) % 65_536) as i64;
    let (h, neg) = if ph < H { (ph, false) } else { (ph - H, true) };
    let u = h * (H - h);
    let s = 16 * u * amp / (5 * H * H - 4 * u);
    if neg { -s } else { s }
}

/// The chime's PCM (mono, 16-bit LE).
fn alert_pcm() -> Vec<u8> {
    let total = ALERT_RATE as usize * ALERT_MS / 1000;
    let attack = ALERT_RATE as usize / 500; // 2 ms
    let mut v = Vec::with_capacity(total * 2);
    for n in 0..total {
        let rest = (total - n) as i64;
        let mut x = alert_partial(n, 880, 6000) + alert_partial(n, 1320, 3000);
        x = x * rest * rest / (total as i64 * total as i64); // squared decay to silence
        if n < attack { x = x * n as i64 / attack as i64; }
        v.extend_from_slice(&(x.clamp(-32_767, 32_767) as i16).to_le_bytes());
    }
    v
}

fn alert_play() {
    if !ALERT.swap(false, Ordering::AcqRel) {
        return;
    }
    if busy() {
        serial_println!("[play] alert -> busy (the output is sounding)");
        return;
    }
    let pcm = alert_pcm();
    match start(ALERT_RATE, 1, 16) {
        Ok(eff) => {
            let took = feed(&pcm);
            finish();
            serial_println!("[play] alert hz=880+1320 ms={} bytes={} fed={} eff_rate={} -> started", ALERT_MS, pcm.len(), took, eff);
        }
        Err(r) => serial_println!("[play] alert -> REFUSED reason={}", r),
    }
}
