//! AUDIO7 (rmbp-ledger B313) — ONE stream discipline for the tone and the player.
//! CHARTER: Stria — owed B289 (the kernel player stands in for Stria, the A/V handler; this is the HDA
//! driver's stream half that a Stria fulfiller would call once BANDY3 lets ring 3 own `play`).
//!
//! Declared from the tail of `hda.rs` as `#[path = "hda_stream.rs"] pub mod stream;` (a CHILD of `hda`, so
//! it reaches `Rings`/`Path`/the register helpers without widening them). Two entry points:
//!
//! * [`rearm`] — the full reset BOTH `run_tone` and `play::gate` call before a stream is used: stop (clear
//!   RUN, wait for it), SRST pulse with both waits, SDxSTS W1C, flush the buffer and the BDL out of the CPU
//!   cache, allow snooping (PCIe DevCtl "Enable No Snoop" cleared, TCSEL to TC0), rewrite BDPL/U, CBL, LVI,
//!   FMT and the tag, then on the codec: D0 on every path node, connection select, stream/channel, the
//!   converter format = the SAME word as SDxFMT, out amps unmuted at the declared 0 dB step, pin OUT enable,
//!   EAPD where capable. It does NOT run the stream; [`run`] does.
//! * M1 — the register truth: one `[hda] run=<n> <who> pre:` line before the reset, one `post:` line after,
//!   and a `diff` line against the previous run of the same kind. Identical runs that sound different are
//!   answered by the fields this diff names (or by its silence, which points away from register state).
//!
//! FLIGHT19 §2 read (docs/dev/evidence/rmbp-1004/AUDIO7.md): the stream registers were byte-identical on
//! all 18 tone arms; the one thing new every run was the heap buffer the DMA reads, written through a
//! write-back cache and never flushed. The flush and the snoop bit are this file's answer to that.
use super::tone::*;
use super::*;
use alloc::string::String;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const PARAM_CONN_LEN: u32 = 0x0E; // [HDA-SPEC §7.3.4.11]
const WT_SHIFT: u32 = 20; // widget type, PARAM_WIDGET_CAPS bits 23:20 [HDA-SPEC §7.3.4.6]

/// What a caller hands [`rearm`]. `paths[i]` is bound at starting channel `chan[i]`.
pub(super) struct Params<'a> {
    pub base: u64,
    pub sd: u64,
    pub cad: u8,
    pub bdl: u64,
    pub bdl_bytes: usize,
    pub buf: u64,
    pub buf_bytes: usize,
    pub cbl: u32,
    pub lvi: u16,
    pub fmt: u16,
    pub tag: u32,
    pub paths: &'a [Path],
    pub chan: &'a [u8],
}

/// One member's codec state. Power words keep the raw byte (bits 7:4 actual, 3:0 set). [HDA-SPEC §7.3.3.10]
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct Mem {
    pub dac: u8, pub pin: u8, pub conv: u16, pub sc: u8, pub dal: u8, pub dar: u8, pub dpwr: u8,
    pub pinctl: u8, pub pamp: u8, pub eapd: u8, pub ppwr: u8,
}

/// One snapshot: the stream descriptor, every member's codec path, the AFG, the PCI coherence bits and a
/// CPU-side checksum of the first 4 KiB of the buffer.
#[derive(Clone, Copy, Default)]
pub(super) struct Snap {
    pub ctl: u32, pub sts: u8, pub lpib: u32, pub cbl: u32, pub lvi: u16, pub fmt: u16, pub bdl: u64,
    pub m: [Mem; PAIR_MAX], pub nm: usize,
    pub afg_pwr: u8, pub gd: u8, pub gdir: u8, pub gpen: u8,
    pub nosnoop: u8, pub tcsel: u8, pub sum: u32,
}

/// What [`rearm`] reports back to its caller.
#[derive(Clone, Copy, Default)]
pub(super) struct Rearm {
    pub run: u32,
    pub stop_ok: bool,
    pub srst_set: bool,
    pub srst_clr: bool,
    pub fmt_match: bool,
    pub tag_match: bool,
    pub stable: bool,
    pub sdfmt_rd: u16,
}

static RUNS: AtomicU32 = AtomicU32::new(0);
static TONE_RUNS: AtomicU32 = AtomicU32::new(0);
/// (run number, post snapshot) of the previous tone run and the previous play arm.
/// AUDIO8 (B329) M3: (shape, run, pre, post) of the last run PER SHAPE — see [`shape`].
const SHAPES: usize = 8;
static LAST: spin::Mutex<[Option<(u64, u32, Snap, Snap)>; SHAPES]> = spin::Mutex::new([None; SHAPES]);
static LAST_RES: spin::Mutex<Rearm> = spin::Mutex::new(Rearm { run: 0, stop_ok: false, srst_set: false, srst_clr: false, fmt_match: false, tag_match: false, stable: false, sdfmt_rd: 0 });
/// The controller's PCI function, found once by matching BAR0 against `base`. 0 = not looked up.
static BDF_BASE: AtomicU64 = AtomicU64::new(0);
static BDF: AtomicU32 = AtomicU32::new(u32::MAX);

fn ctl24(base: u64, sd: u64) -> u32 {
    (r8(base, sd + SD_CTL) as u32) | ((r8(base, sd + SD_CTL + 1) as u32) << 8) | ((r8(base, sd + SD_CTL + 2) as u32) << 16)
}

fn afg_of(rings: &mut Rings, cad: u8, a: &mut Audit) -> u8 {
    match rings.cmd(cad, 0, VERB_GET_PARAMETER, PARAM_SUBNODE_COUNT, a) {
        Some(sub) => { a.verbs_get += 1; ((sub >> 16) & 0xFF) as u8 }
        None => 0x01,
    }
}

/// Get Amplifier Gain/Mute, OUTPUT side. Payload bit 15 = output, bit 13 = left (clear = right), bits 3:0
/// index. [HDA-SPEC §7.3.3.7] (The `0x80` payload the older reads used is input/right — a DAC answers 0.)
fn out_amp_rd(rings: &mut Rings, cad: u8, nid: u8, left: bool, a: &mut Audit) -> u8 {
    a.verbs_get += 1;
    let pl = if left { 0xA000 } else { 0x8000 };
    (rings.cmd16(cad, nid, VERB_GET_AMP_GAIN_MUTE, pl, a).unwrap_or(0xFF) & 0xFF) as u8
}

fn get8(rings: &mut Rings, cad: u8, nid: u8, verb: u32, a: &mut Audit) -> u8 {
    a.verbs_get += 1;
    (rings.cmd(cad, nid, verb, 0, a).unwrap_or(0xFF) & 0xFF) as u8
}

fn fnv(buf: u64, bytes: usize) -> u32 {
    if buf == 0 { return 0; }
    let mut h: u32 = 0x811C_9DC5;
    for i in 0..bytes.min(4096) {
        h ^= unsafe { core::ptr::read_volatile((buf + i as u64) as *const u8) } as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The controller's (bus, slot, func), by BAR0 match. None on a controller this scan cannot place.
fn bdf(base: u64) -> Option<(u8, u8, u8, u16)> {
    if BDF_BASE.load(Ordering::Relaxed) != base {
        let mut v = u32::MAX;
        for (b, s, f, vend, _d) in crate::drivers::pci::PciScanner::audio_inventory().iter().copied() {
            if crate::drivers::pci::PciScanner::get_bar_address(b, s, f) == base {
                v = ((b as u32) << 24) | ((s as u32) << 16) | ((f as u32) << 8) | (if vend == 0x8086 { 1 } else { 0 });
                break;
            }
        }
        BDF.store(v, Ordering::Relaxed);
        BDF_BASE.store(base, Ordering::Relaxed);
    }
    let v = BDF.load(Ordering::Relaxed);
    if v == u32::MAX { None } else { Some(((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, (v & 1) as u16)) }
}

/// The PCI Express capability's Device Control register offset (cap + 8), walked from the capability list.
fn devctl_off(b: u8, s: u8, f: u8) -> Option<u8> {
    let status = unsafe { crate::arch::pci::read_config_16(b, s, f, 0x06) };
    if status & 0x10 == 0 { return None; }
    let mut p = (unsafe { crate::arch::pci::read_config_32(b, s, f, 0x34) } & 0xFC) as u8;
    for _ in 0..48 {
        if p < 0x40 { return None; }
        let w = unsafe { crate::arch::pci::read_config_32(b, s, f, p & 0xFC) };
        if w & 0xFF == 0x10 { return Some(p + 8); }
        p = ((w >> 8) & 0xFC) as u8;
    }
    None
}

/// (nosnoop, tcsel): PCIe DevCtl bit 11 "Enable No Snoop" (0xFF = no PCIe capability) and TCSEL 0x44[2:0]
/// (0xFF = not an Intel controller, the register is Intel's).
fn coherence_rd(base: u64) -> (u8, u8) {
    let Some((b, s, f, intel)) = bdf(base) else { return (0xFF, 0xFF) };
    let ns = match devctl_off(b, s, f) {
        Some(o) => ((unsafe { crate::arch::pci::read_config_16(b, s, f, o) } >> 11) & 1) as u8,
        None => 0xFF,
    };
    let tc = if intel == 1 { (unsafe { crate::arch::pci::read_config_32(b, s, f, 0x44) } & 0x7) as u8 } else { 0xFF };
    (ns, tc)
}

/// Linux `hda_intel` does both on every Intel PCH (`AZX_DCAPS_SNOOP_TYPE(SCH)` clears DevCtl No-Snoop;
/// `azx_init_pci` clears TCSEL "to clear playback static"). Both are read back by the caller.
fn coherence_fix(base: u64, a: &mut Audit) {
    let Some((b, s, f, intel)) = bdf(base) else { return };
    if let Some(o) = devctl_off(b, s, f) {
        let v = unsafe { crate::arch::pci::read_config_16(b, s, f, o) };
        if v & (1 << 11) != 0 {
            unsafe { crate::arch::pci::write_config_16(b, s, f, o, v & !(1 << 11)) };
            a.cfg += 1;
        }
    }
    if intel == 1 {
        let v = unsafe { crate::arch::pci::read_config_32(b, s, f, 0x44) };
        if v & 0x7 != 0 {
            unsafe { crate::arch::pci::write_config_32(b, s, f, 0x44, v & !0x7) };
            a.cfg += 1;
        }
    }
}

/// Write every cache line of `[p, p + bytes)` back to memory, then fence. A snooped DMA does not need it;
/// a non-snooped one reads DRAM, which is otherwise whatever the heap held before. Returns bytes flushed.
pub(super) fn flush(p: u64, bytes: usize) -> usize {
    if p == 0 || bytes == 0 { return 0; }
    let start = p & !63;
    let end = p + bytes as u64;
    let mut l = start;
    while l < end {
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("clflush [{}]", in(reg) l, options(nostack, preserves_flags)) };
        l += 64;
    }
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("mfence", options(nostack, preserves_flags)) };
    core::sync::atomic::fence(Ordering::SeqCst);
    (end - start) as usize
}

/// M1 — read everything a run depends on. Read-only (codec GETs, MMIO and config reads).
pub(super) fn snapshot(rings: &mut Rings, p: &Params, a: &mut Audit) -> Snap {
    let (base, sd, cad) = (p.base, p.sd, p.cad);
    let mut s = Snap::default();
    s.ctl = ctl24(base, sd);
    s.sts = r8(base, sd + SD_STS);
    s.lpib = r32(base, sd + SD_LPIB);
    s.cbl = r32(base, sd + SD_CBL);
    s.lvi = r16(base, sd + SD_LVI);
    s.fmt = r16(base, sd + SD_FMT);
    s.bdl = ((r32(base, sd + SD_BDPU) as u64) << 32) | (r32(base, sd + SD_BDPL) as u64);
    s.nm = p.paths.len().min(PAIR_MAX);
    for i in 0..s.nm {
        let (dac, pin) = (p.paths[i].dac, p.paths[i].pin);
        let mut m = Mem { dac, pin, ..Mem::default() };
        a.verbs_get += 1;
        m.conv = (rings.cmd(cad, dac, VERB_GET_CONVERTER_FORMAT, 0, a).unwrap_or(0xFFFF) & 0xFFFF) as u16;
        m.sc = get8(rings, cad, dac, VERB_GET_STREAM_CHANNEL, a);
        m.dal = out_amp_rd(rings, cad, dac, true, a);
        m.dar = out_amp_rd(rings, cad, dac, false, a);
        m.dpwr = get8(rings, cad, dac, VERB_GET_POWER_STATE, a);
        m.pinctl = get8(rings, cad, pin, VERB_GET_PIN_CONTROL, a);
        m.pamp = out_amp_rd(rings, cad, pin, true, a);
        m.eapd = get8(rings, cad, pin, VERB_GET_EAPD, a);
        m.ppwr = get8(rings, cad, pin, VERB_GET_POWER_STATE, a);
        s.m[i] = m;
    }
    let afg = afg_of(rings, cad, a);
    s.afg_pwr = get8(rings, cad, afg, VERB_GET_POWER_STATE, a);
    s.gd = get8(rings, cad, afg, VERB_GET_GPIO_DATA, a);
    s.gdir = get8(rings, cad, afg, VERB_GET_GPIO_DIRECTION, a);
    s.gpen = get8(rings, cad, afg, VERB_GET_GPIO_ENABLE, a);
    let (ns, tc) = coherence_rd(base);
    s.nosnoop = ns;
    s.tcsel = tc;
    s.sum = fnv(p.buf, p.buf_bytes);
    s
}

fn d(pw: u8) -> u8 { (pw >> 4) & 0x0F }

/// ONE line per snapshot.
fn print(n: u32, who: &str, when: &str, s: &Snap) {
    let mut l = String::new();
    let _ = write!(l, "[hda] run={} {} {}: sdctl={:#08x} run={} srst={} stripe={} tag={} sts={:#04x} lpib={} cbl={} lvi={} fmt={:#06x} bdl={:#x}",
        n, who, when, s.ctl, (s.ctl >> 1) & 1, s.ctl & 1, (s.ctl >> 16) & 3, (s.ctl >> 20) & 0xF, s.sts, s.lpib, s.cbl, s.lvi, s.fmt, s.bdl);
    for i in 0..s.nm {
        let m = &s.m[i];
        let _ = write!(l, " ; m{} dac=0x{:02x} conv={:#06x} sc={:#04x} dacamp={:#04x}/{:#04x} dpwr=D{} pin=0x{:02x} pinctl={:#04x} pamp={:#04x} eapd={:#04x} ppwr=D{}",
            i, m.dac, m.conv, m.sc, m.dal, m.dar, d(m.dpwr), m.pin, m.pinctl, m.pamp, m.eapd, d(m.ppwr));
    }
    let _ = write!(l, " ; afg=D{} gpio={:#04x}/{:#04x}/{:#04x} ; nosnoop={} tcsel={} ; sum={:#010x}",
        d(s.afg_pwr), s.gd, s.gdir, s.gpen, s.nosnoop, s.tcsel, s.sum);
    serial_println!("{}", l);
}

/// The STATE fields that differ between two snapshots. Heap addresses (`bdl`), the position (`lpib`) and the
/// buffer checksum are meant to differ run to run and are not compared.
fn diff(a: &Snap, b: &Snap) -> String {
    let mut o = String::new();
    let mut f = |c: bool, name: &str| { if c { if !o.is_empty() { o.push(','); } o.push_str(name); } };
    f(a.ctl != b.ctl, "sdctl");
    f(a.sts != b.sts, "sts");
    f(a.cbl != b.cbl, "cbl");
    f(a.lvi != b.lvi, "lvi");
    f(a.fmt != b.fmt, "fmt");
    f(a.nm != b.nm, "members");
    for i in 0..a.nm.min(b.nm) {
        let (x, y) = (&a.m[i], &b.m[i]);
        if x != y {
            f(x.dac != y.dac || x.pin != y.pin, "m.path");
            f(x.conv != y.conv, "m.conv");
            f(x.sc != y.sc, "m.sc");
            f(x.dal != y.dal || x.dar != y.dar, "m.dacamp");
            f(x.dpwr != y.dpwr, "m.dpwr");
            f(x.pinctl != y.pinctl, "m.pinctl");
            f(x.pamp != y.pamp, "m.pamp");
            f(x.eapd != y.eapd, "m.eapd");
            f(x.ppwr != y.ppwr, "m.ppwr");
        }
    }
    f(a.afg_pwr != b.afg_pwr, "afg");
    f(a.gd != b.gd || a.gdir != b.gdir || a.gpen != b.gpen, "gpio");
    f(a.nosnoop != b.nosnoop, "nosnoop");
    f(a.tcsel != b.tcsel, "tcsel");
    o
}

/// The codec's 0 dB step from an amplifier-capability word: OFFSET (bits 6:0) IS the step that is 0 dB,
/// clamped to NUMSTEPS (bits 14:8). [HDA-SPEC §7.3.4.10] `tone::moderate_gain` returns the MAXIMUM when
/// OFFSET is 0 — on a part whose 0 dB is step 0 that is the loudest the amp goes; this never is.
fn zero_db(caps: u32) -> u8 {
    let steps = ((caps >> 8) & 0x7F) as u8;
    let off = (caps & 0x7F) as u8;
    off.min(steps)
}

fn power_d0(rings: &mut Rings, cad: u8, nid: u8, a: &mut Audit) -> u8 {
    if rings.cmd(cad, nid, VERB_SET_POWER_STATE, 0, a).is_some() { a.verbs_set += 1; }
    let t = crate::arch::now_cycles();
    loop {
        let act = (get8(rings, cad, nid, VERB_GET_POWER_STATE, a) >> 4) & 0x0F;
        if act == 0 || elapsed_ms(t) >= 50 { return act; }
        delay_us(500);
    }
}

/// M2 — THE one reset. Prints `pre`, resets and reprograms the descriptor and the codec path, prints the
/// `rearm` readback line and `post`, diffs against the previous run of the same `who`. Does not RUN.
pub(super) fn rearm(rings: &mut Rings, p: &Params, who: &'static str, a: &mut Audit) -> Rearm {
    let (base, sd, cad) = (p.base, p.sd, p.cad);
    let n = RUNS.fetch_add(1, Ordering::Relaxed) + 1;
    let pre = snapshot(rings, p, a);
    print(n, who, "pre", &pre);

    // 1. Stop: clear RUN (and IOCE), wait for the engine to report stopped. [HDA-SPEC §3.3.35]
    let c0 = r8(base, sd + SD_CTL);
    w8(base, sd + SD_CTL, c0 & !((SDCTL_RUN | SDCTL_IOCE) as u8));
    let stop_ok = wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_RUN as u8 == 0);
    // 2. SRST pulse: set, observe set, clear, observe clear.
    w8(base, sd + SD_CTL, SDCTL_SRST as u8);
    let srst_set = wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_SRST as u8 != 0);
    w8(base, sd + SD_CTL, 0);
    let srst_clr = wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_SRST as u8 == 0);
    // 3. Status W1C.
    w8(base, sd + SD_STS, SDSTS_BCIS | SDSTS_FIFOE | SDSTS_DESE);
    a.stream += 4;
    // 4. Coherence: the buffer and the BDL reach memory; the controller may not skip the snoop.
    let (ns0, tc0) = coherence_rd(base);
    coherence_fix(base, a);
    let (ns1, tc1) = coherence_rd(base);
    let flushed = flush(p.buf, p.buf_bytes) + flush(p.bdl, p.bdl_bytes);
    // 5. The descriptor: BDL, CBL, LVI, FMT, tag (stripe 0, priority 0), IOCE. [HDA-SPEC §3.3.35-43]
    w32(base, sd + SD_BDPL, (bus_addr(p.bdl) & 0xFFFF_FFFF) as u32);
    w32(base, sd + SD_BDPU, (bus_addr(p.bdl) >> 32) as u32);
    w32(base, sd + SD_CBL, p.cbl);
    w16(base, sd + SD_LVI, p.lvi);
    w16(base, sd + SD_FMT, p.fmt);
    w8(base, sd + SD_CTL + 2, ((p.tag & 0xF) << 4) as u8);
    w8(base, sd + SD_CTL, SDCTL_IOCE as u8);
    a.stream += 7;

    // 6. The codec path, every member.
    let afg = afg_of(rings, cad, a);
    let afg_act = power_d0(rings, cad, afg, a);
    let afg_amp = rings.cmd(cad, afg, VERB_GET_PARAMETER, PARAM_OUT_AMP_CAPS, a).unwrap_or(0);
    a.verbs_get += 1;
    let nm = p.paths.len().min(PAIR_MAX);
    let mut gains = [(0u8, 0u8); PAIR_MAX];
    for m in 0..nm {
        let path = p.paths[m];
        for i in 0..(path.len as usize).min(MAX_PATH_DEPTH) {
            let nid = path.nodes[i];
            let caps = rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_WIDGET_CAPS, a).unwrap_or(0);
            a.verbs_get += 1;
            let wt = ((caps >> WT_SHIFT) & 0xF) as u8;
            if caps & WCAP_POWER_CTL != 0 { let _ = power_d0(rings, cad, nid, a); }
            if (wt == WT_SELECTOR || wt == WT_PIN) && caps & WCAP_CONN_LIST != 0 && i + 1 < path.len as usize {
                let cl = rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_CONN_LEN, a).unwrap_or(0) & 0x7F;
                a.verbs_get += 1;
                if cl > 1 && rings.cmd(cad, nid, VERB_SET_CONNECTION_SELECT, path.sel[i] as u32, a).is_some() { a.verbs_set += 1; }
            }
            if caps & WCAP_OUT_AMP != 0 {
                let ac = if caps & WCAP_AMP_OVERRIDE != 0 {
                    a.verbs_get += 1;
                    rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_OUT_AMP_CAPS, a).unwrap_or(0)
                } else { afg_amp };
                let g = zero_db(ac);
                if nid == path.dac { gains[m] = (g, ((ac >> 8) & 0x7F) as u8); }
                if rings.cmd16(cad, nid, VERB_SET_AMP_GAIN_MUTE, amp_payload(true, 0, false, g), a).is_some() { a.verbs_set += 1; }
            }
        }
        // Converter: stream/channel FIRST, then the format (the order Linux `snd_hda_codec_setup_stream` uses).
        let ch = p.chan.get(m).copied().unwrap_or(0) as u32;
        if rings.cmd(cad, path.dac, VERB_SET_STREAM_CHANNEL, ((p.tag & 0xF) << 4) | (ch & 0xF), a).is_some() { a.verbs_set += 1; }
        if rings.cmd16(cad, path.dac, VERB_SET_CONVERTER_FORMAT, p.fmt as u32, a).is_some() { a.verbs_set += 1; }
        // Pin: OUT enable (+ HP drive where the pin declares it), EAPD where capable. [HDA-SPEC §7.3.3.13, .16]
        let pincap = rings.cmd(cad, path.pin, VERB_GET_PARAMETER, PARAM_PIN_CAPS, a).unwrap_or(0);
        a.verbs_get += 1;
        let mut pc = PINCTL_OUT_ENABLE;
        if pincap & PINCAP_HP_DRIVE != 0 { pc |= PINCTL_HP_ENABLE; }
        if rings.cmd(cad, path.pin, VERB_SET_PIN_CONTROL, pc as u32, a).is_some() { a.verbs_set += 1; }
        if pincap & PINCAP_EAPD != 0 && rings.cmd(cad, path.pin, VERB_SET_EAPD, EAPD_ENABLE as u32, a).is_some() { a.verbs_set += 1; }
    }

    // 7. Readback and score.
    let sdfmt_rd = r16(base, sd + SD_FMT);
    let tag_rd = (r8(base, sd + SD_CTL + 2) >> 4) as u32;
    let mut fmt_match = sdfmt_rd == p.fmt && nm > 0;
    let mut tag_match = tag_rd == (p.tag & 0xF) && nm > 0;
    let mut fl = String::new();
    for m in 0..nm {
        let conv = rings.cmd(cad, p.paths[m].dac, VERB_GET_CONVERTER_FORMAT, 0, a).unwrap_or(0xFFFF_FFFF) & 0xFFFF;
        let sc = rings.cmd(cad, p.paths[m].dac, VERB_GET_STREAM_CHANNEL, 0, a).unwrap_or(0xFFFF_FFFF) & 0xFF;
        a.verbs_get += 2;
        let ch = p.chan.get(m).copied().unwrap_or(0) as u32;
        if conv != sdfmt_rd as u32 { fmt_match = false; }
        if (sc >> 4) != (p.tag & 0xF) || (sc & 0xF) != ch { tag_match = false; }
        let _ = write!(fl, "{}m{}:conv={:#06x},sc={:#04x},gain={}/{}", if m == 0 { "" } else { " " }, m, conv, sc, gains[m].0, gains[m].1);
    }
    serial_println!(
        "[hda] rearm run={} who={} was_ctl={:#04x} stop={} srst={}/{} flushed={} nosnoop={}->{} tcsel={}->{} afg=D{} fmt={:#06x} sdfmt_rd={:#06x} tag={} tag_rd={} [{}] dac_fmt_match={} tag_match={}",
        n, who, c0, stop_ok as u8, srst_set as u8, srst_clr as u8, flushed, ns0, ns1, tc0, tc1, afg_act,
        p.fmt, sdfmt_rd, p.tag, tag_rd, fl, fmt_match as u8, tag_match as u8
    );

    let post = snapshot(rings, p, a);
    print(n, who, "post", &post);
    // AUDIO8 (B329) M3: like against like. FLIGHT20's hdaboth / hda220 / hda2 FAILs were `fields_stable=0` on
    // `post=[members]` / `[m.path]` only — each diffed against the previous run of a DIFFERENT shape (two members
    // vs one, DAC 0x03 vs 0x04). The previous run is now looked up by shape: who, SDxFMT, the member DAC/pin list.
    let key = shape(who, p);
    let mut g = LAST.lock();
    let slot = g.iter().position(|e| e.as_ref().map_or(false, |x| x.0 == key));
    let stable = match slot.and_then(|i| g[i].as_ref()) {
        Some((_, pn, ppre, ppost)) => {
            let (dp, dq) = (diff(ppre, &pre), diff(ppost, &post));
            serial_println!("[hda] run={} diff vs run={}: pre=[{}] post=[{}] stable={} shape={:#x}", n, pn, dp, dq, dq.is_empty() as u8, key);
            dq.is_empty()
        }
        None => { serial_println!("[hda] run={} diff vs run=-: pre=[] post=[] stable=1 (first {} run of shape={:#x})", n, who, key); true }
    };
    let i = slot.or_else(|| g.iter().position(|e| e.is_none())).unwrap_or((n as usize) % SHAPES);
    g[i] = Some((key, n, pre, post));
    drop(g);
    let r = Rearm { run: n, stop_ok, srst_set, srst_clr, fmt_match, tag_match, stable, sdfmt_rd };
    *LAST_RES.lock() = r;
    r
}

/// Set RUN (tag rewritten with it) and read it back, polling up to 2 ms. Returns (ctl readback, run bit).
pub(super) fn run(base: u64, sd: u64, tag: u32) -> (u32, bool) {
    w8(base, sd + SD_CTL + 2, ((tag & 0xF) << 4) as u8);
    w8(base, sd + SD_CTL, (SDCTL_IOCE | SDCTL_RUN) as u8);
    let t = crate::arch::now_cycles();
    loop {
        let c = ctl24(base, sd);
        if c & SDCTL_RUN != 0 || elapsed_ms(t) >= 2 { return (c, c & SDCTL_RUN != 0); }
        core::hint::spin_loop();
    }
}

/// M4 — the `tests hda` verdict, printed at the end of every tone run (`lpib_moved` from the run's walk).
pub(super) fn verdict_tone(lpib_moved: bool) {
    let r = *LAST_RES.lock();
    let n = TONE_RUNS.fetch_add(1, Ordering::Relaxed) + 1;
    let ok = r.stable && r.fmt_match && r.tag_match && lpib_moved && r.srst_set && r.srst_clr;
    serial_println!(
        ":: HDA: runs={} fields_stable={} dac_fmt_match={} tag_match={} lpib_moved={} -> {} :: run={} sdfmt={:#06x} srst={}/{} stop={} ::",
        n, r.stable as u8, r.fmt_match as u8, r.tag_match as u8, lpib_moved as u8, if ok { "PASS" } else { "FAIL" },
        r.run, r.sdfmt_rd, r.srst_set as u8, r.srst_clr as u8, r.stop_ok as u8
    );
}

/// The last rearm's readback (play's witness reads tag/format match from here).
pub(super) fn last() -> Rearm { *LAST_RES.lock() }

/// AUDIO8 (B329) M3 — a run's shape: who (bit 63 = play), SDxFMT (bits 47:32), then each member's DAC and pin
/// (8 bits each, member 0 lowest). Two runs are diffed only when their shapes are equal.
fn shape(who: &str, p: &Params) -> u64 {
    let mut k = ((who == "play") as u64) << 63 | (p.fmt as u64) << 32;
    for (m, path) in p.paths.iter().enumerate().take(2) {
        k |= ((path.dac as u64) | (path.pin as u64) << 8) << (16 * m);
    }
    k
}
