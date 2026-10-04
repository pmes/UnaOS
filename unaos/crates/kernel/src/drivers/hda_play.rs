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

struct Wav { path: String, rate: u32, ch: usize, bits: usize, data_off: u64, data_len: u64, pos: u64, chunk: usize, fed_end: bool }
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
    super::amp::release(super::amp::PLAY); // AUDIO8 (B329): idempotent — a stop with nothing armed still closes the owner bit
    ACTIVE.store(false, Ordering::Release);
    *WAV.lock() = None; // after the ST guard drops: wav_pump takes WAV then ST, so never the other order
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
    super::amp::release(super::amp::PLAY); // AUDIO8 (B329) M1: the last close starts the amp's idle hold-off
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
    if !s.armed || s.done { return; }
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
                            data_len: data_len / fin * fin, pos: 0, chunk: (frames * fin) as usize, fed_end: false });
        }
        off += 8 + len + (len & 1);
    }
    Err(String::from("no data chunk"))
}

/// Parse `path`, start the stream and arm the tick-driven file pump. Prints the witness arm itself on refusal.
pub fn open_wav(path: &str) -> Result<(), String> {
    let w = match parse(path) {
        Ok(w) => w,
        Err(r) => { serial_println!(":: PLAYWAV: path={} reason={} -> REFUSED ::", path, r); return Err(r); }
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
    let want = w.data_len / (w.ch * w.bits / 8) as u64;
    let ms = frames * 1000 / w.rate as u64;
    let ok = frames == want && s.underruns == 0 && s.err == 0 && s.completed >= 1 && s.lpib_moved && s.done;
    serial_println!("[play] done resampled={} eff_rate={} entries={} fifoe_dese={} run_bit={} lpib_moved={} level={}", s.resampled as u8, s.eff_rate, s.completed, s.err, s.run_bit as u8, s.lpib_moved as u8, s.level);
    // AUDIO7 M4: `done=` is the ring's own drain (LPIB walked past the last data entry and every entry refilled with silence).
    serial_println!(":: PLAYWAV: path={} rate={} frames={} lpib_moved={} done={} -> {} :: ch={} bits={} secs={}.{} under={} ::", w.path, w.rate, frames, s.lpib_moved as u8, s.done as u8, if ok { "PASS" } else { "FAIL" }, w.ch, w.bits, ms / 1000, (ms % 1000) / 100, s.underruns);
}

/// The service tick (folded at the top of `probe_after_root`): latched opens, file pump, ring refill.
pub fn service() {
    super::amp::tick(); // AUDIO8 (B329) M1: the amp's idle hold-off (idle cost: one atomic load)
    if !ACTIVE.load(Ordering::Acquire) { return; }
    if let Some(p) = PENDING.try_lock().and_then(|mut g| g.take()) { let _ = open_wav(&p); }
    wav_pump();
    let fin = { let Some(mut s) = ST.try_lock() else { return }; ring_pump(&mut s); s.done && s.armed };
    if fin {
        report();
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
        None => console.println("usage: play <path.wav> | play stop"),
        Some("stop") => { stop(); console.println("play: stopped"); }
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
