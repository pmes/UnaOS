// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TCURX (orin 14, `tcuprobe` knob, DEFAULT OFF): the Tegra234 HSP shared-mailbox side of the serial
// console — a READ-ONLY probe of the Tegra Combined UART (TCU) RX mailbox, so render5/6 can say
// whether the SPE forwards console input into the mailbox the CCPLEX is meant to read (A16's
// competing reader) or whether nothing arrives there at all. No HSP or UART register is written; no
// IRQ is enabled. Design + provenance: `docs/dev/evidence/orin14/TCURX-DESIGN.md`.
//
// FACTS THIS FILE RESTS ON (each with its source; no GPL driver was read — orin-ledger D3,
// CLEAN_ROOM_POLICY §6):
//   * HSP block layout — `HSP_INT_DIMENSIONING` at block + 0x380 (nSM [3:0], nSS [7:4], nAS [11:8]),
//     a 64 KiB common region, then the shared mailboxes at a 32 KiB (1 << 15) stride:
//     SM i = block + 0x10000 + (i << 15). Source: edk2-nvidia `HspDoorbellPrivate.h`
//     (BSD-2-Clause-Patent; `HSP_DIMENSIONING 0x380`, `HSP_COMMON_REGION_SIZE SIZE_64KB`,
//     `HSP_MAILBOX_SHIFT_SIZE 15`). Consistent with this tree's METAL-VERIFIED doorbell derivation in
//     `bpmp_tegra.rs` (db region = block + (1 + nSM/2 + nSS + nAS) * 0x10000 — nSM/2 because two
//     32 KiB mailboxes share one 64 KiB step; render4 read nSM=8 -> db_base 0x3c90000 and MRQ_PING
//     answered over it).
//   * TCU mailbox WORD — bits [23:0] = up to three payload bytes (byte 0 in [7:0]), bits [25:24] =
//     number of valid bytes, bit 26 = flush, bit 27 = hw-flush, bit 31 = data present (the mailbox
//     FULL/tag bit). The reader consumes by WRITING the word back with the count decremented (or 0).
//     Source: edk2-nvidia `TegraCombinedSerialPortLib.c` (BSD-2-Clause-Patent). THIS PROBE NEVER
//     WRITES — it only samples bit 31 and the payload, so a byte the SPE parks there stays parked.
//   * DTB contract — the console node whose `compatible` names `tcu` carries `mboxes` =
//     <hsp-phandle type index> x2 with `mbox-names = "rx", "tx"`; the HSP node has `#mbox-cells = 2`,
//     `reg = <hi lo hi lo>`; the type cell's bits [7:0] = mailbox class (shared mailbox), bits [15:8] =
//     data-size flags; the index cell's bit 31 = direction (1 = producer/TX, 0 = consumer/RX), bits
//     [23:0] = the shared-mailbox index. Source: the public DT bindings `nvidia,tegra186-hsp.yaml`
//     (GPL-2.0-only OR BSD-2-Clause) and `nvidia,tegra194-tcu.yaml`. The LIVE values come from the
//     DTB the firmware hands this kernel (`JB1a — DTB @0x25f501000`), never from a dtsi.
//
// Witness tokens are subsystem-named: `[tcu]` (the console-over-mailbox protocol) and `[hsp]` (the
// block). Every failure to resolve the DTB is one `[tcu] STOP` line and the probe stays unarmed.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::fdt_tegra::{Fdt, MAX_PATH};

/// `HSP_INT_DIMENSIONING`: block + 0x380 (edk2-nvidia `HSP_DIMENSIONING`; `bpmp_tegra.rs` twin).
const HSP_INT_DIMENSIONING: u64 = 0x380;
/// The common region ahead of the first shared mailbox (edk2-nvidia `HSP_COMMON_REGION_SIZE`).
const HSP_COMMON_REGION: u64 = 0x1_0000;
/// Shared-mailbox stride = 1 << `HSP_MAILBOX_SHIFT_SIZE` (15) = 32 KiB (edk2-nvidia).
const HSP_SM_SHIFT: u64 = 15;
/// TCU word: bit 31 = data present / mailbox full (edk2-nvidia `TEGRA_COMBINED_UART_PIO.Interrupt`).
const TCU_FULL: u32 = 1 << 31;
/// TCU word: bits [25:24] = number of valid payload bytes.
const TCU_NBYTES_SHIFT: u32 = 24;
/// TCU word: bit 26 = flush, bit 27 = hw-flush.
const TCU_FLUSH: u32 = 1 << 26;
const TCU_HWFLUSH: u32 = 1 << 27;
/// DT index cell: bit 31 = direction (1 = TX), bits [23:0] = shared-mailbox index.
const MBOX_DIR_TX: u32 = 1 << 31;
const MBOX_INDEX_MASK: u32 = 0x00ff_ffff;

/// The resolved RX / TX mailbox word addresses (0 = unresolved; the task never runs then).
static RX_MBOX: AtomicU64 = AtomicU64::new(0);
static TX_MBOX: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
/// Sampler counters (the task samples on every pass; the census prints them once a second).
static POLLS: AtomicU64 = AtomicU64::new(0);
static FULL_EDGES: AtomicU64 = AtomicU64::new(0);
static CHANGES: AtomicU64 = AtomicU64::new(0);
static LAST_FULL_RAW: AtomicU64 = AtomicU64::new(0);
static CENSUS: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn r32(pa: u64) -> u32 {
    unsafe { core::ptr::read_volatile(pa as *const u32) }
}

#[inline(never)]
fn stop(why: &str, a: u64, b: u64) {
    serial_println!("[tcu] STOP — {} = {:#x} / {:#x}; probe NOT armed ::", why, a, b);
}

/// One decoded `mboxes` entry: the HSP node it names and the mailbox it selects.
struct MboxRef {
    hsp_path: [u8; MAX_PATH],
    hsp_len: usize,
    hsp_base: u64,
    mbox_cells: u32,
    ty: u32,
    idx: u32,
}

impl MboxRef {
    fn path(&self) -> &str {
        core::str::from_utf8(&self.hsp_path[..self.hsp_len]).unwrap_or("?")
    }
    fn sm_index(&self) -> u64 {
        (self.idx & MBOX_INDEX_MASK) as u64
    }
    fn is_tx(&self) -> bool {
        self.idx & MBOX_DIR_TX != 0
    }
    /// The mailbox WORD address: block + common region + index * 32 KiB (facts above).
    fn word_pa(&self) -> u64 {
        self.hsp_base + HSP_COMMON_REGION + (self.sm_index() << HSP_SM_SHIFT)
    }
}

fn resolve(fdt: &Fdt<'_>, ph: u32, ty: u32, idx: u32) -> Option<MboxRef> {
    let mut buf = [0u8; MAX_PATH];
    let n = fdt.path_of_phandle(ph, &mut buf);
    if n == 0 {
        stop("mboxes phandle resolves to no node; phandle / type", ph as u64, ty as u64);
        return None;
    }
    let reg = fdt.prop_at(&buf[..n], b"reg");
    if reg.n < 2 {
        stop("HSP node reg too short (need hi lo); reg.n / phandle", reg.n as u64, ph as u64);
        return None;
    }
    let cells = fdt.prop_at(&buf[..n], b"#mbox-cells");
    Some(MboxRef {
        hsp_path: buf,
        hsp_len: n,
        hsp_base: ((reg.words[0] as u64) << 32) | reg.words[1] as u64,
        mbox_cells: if cells.n >= 1 { cells.words[0] } else { 0 },
        ty,
        idx,
    })
}

/// TCURX arm: resolve the TCU console node + its two HSP mailboxes from the live DTB, print the
/// `[tcu] hsp` witness, take ONE read-only sample of the RX mailbox word (after announcing the
/// address — the EL3-fatal discipline), and spawn the boot-core sampler task. Read-only throughout.
pub fn tcuprobe_arm(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    if dtb_addr == 0 || dtb_size == 0 {
        return stop("no DTB handed off (addr / size)", dtb_addr, dtb_size as u64);
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return stop("DTB GiB unmapped (GiB lo / RAM-GiB-mask)", g_lo, ram_gib_mask);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return stop("bad DTB header at Fdt::new (addr / size)", dtb_addr, dtb_size as u64);
    };

    // 1. The console node: the first node whose `compatible` string list contains "tcu".
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen == 0 && e.name == b"compatible" && e.val_off + e.val_len <= blob.len() {
            let v = &blob[e.val_off..e.val_off + e.val_len];
            if v.windows(3).any(|w| w == b"tcu") {
                let l = e.path.len().min(MAX_PATH);
                path[..l].copy_from_slice(&e.path[..l]);
                plen = l;
            }
        }
    });
    if plen == 0 {
        return stop("no node whose compatible names `tcu`", 0, 0);
    }
    let node = &path[..plen];
    let mboxes = fdt.prop_at(node, b"mboxes");
    if mboxes.n < 6 {
        return stop("tcu mboxes too short (need 2 x <phandle type index>); cells", mboxes.n as u64, 0);
    }
    // `mbox-names`: which entry is "rx" (the binding puts rx first; read it rather than assume).
    let mut rx_entry = 0usize;
    let mut names_seen = false;
    fdt.for_each_prop(|e| {
        if !names_seen && e.path == node && e.name == b"mbox-names" && e.val_off + e.val_len <= blob.len() {
            names_seen = true;
            let v = &blob[e.val_off..e.val_off + e.val_len];
            let mut k = 0usize;
            for s in v.split(|&b| b == 0) {
                if s == b"rx" {
                    rx_entry = k;
                }
                if !s.is_empty() {
                    k += 1;
                }
            }
        }
    });
    let tx_entry = 1 - rx_entry.min(1);
    let (rp, rt, ri) = (mboxes.words[3 * rx_entry], mboxes.words[3 * rx_entry + 1], mboxes.words[3 * rx_entry + 2]);
    let (tp, tt, ti) = (mboxes.words[3 * tx_entry], mboxes.words[3 * tx_entry + 1], mboxes.words[3 * tx_entry + 2]);
    let Some(rx) = resolve(&fdt, rp, rt, ri) else { return };
    let Some(tx) = resolve(&fdt, tp, tt, ti) else { return };

    // 2. The witness: the brief's shape (`top0` = the HSP the RX entry names, `aon` = the TX entry's —
    //    the binding's assignment; the paths beside them say whether the live DTB agrees).
    serial_println!(
        "[tcu] hsp top0={:#x} aon={:#x} tx-mbox={} rx-mbox={} | node={} names={} rx={} cells=[{:#x} {:#x} {:#x}] tx={} cells=[{:#x} {:#x} {:#x}] #mbox-cells={}/{} dir(rx)={} dir(tx)={} -> rx-word={:#x} tx-word={:#x} (block+0x10000+(i<<15))",
        rx.hsp_base,
        tx.hsp_base,
        tx.sm_index(),
        rx.sm_index(),
        core::str::from_utf8(node).unwrap_or("?"),
        if names_seen { "read" } else { "ASSUMED(rx,tx)" },
        rx.path(),
        rp,
        rx.ty,
        ri,
        tx.path(),
        tp,
        tx.ty,
        ti,
        rx.mbox_cells,
        tx.mbox_cells,
        if rx.is_tx() { "TX?!" } else { "rx" },
        if tx.is_tx() { "tx" } else { "RX?!" },
        rx.word_pa(),
        tx.word_pa(),
    );

    // 3. Bound the RX mailbox index by the block's own dimensioning before the first touch of the
    //    shared-mailbox window (read-only; `bpmp_tegra` reads this same register on top0 every boot).
    serial_println!("[hsp] touching {:#x} (dimensioning, read-only) ::", rx.hsp_base + HSP_INT_DIMENSIONING);
    let dim = r32(rx.hsp_base + HSP_INT_DIMENSIONING);
    let n_sm = (dim & 0xf) as u64;
    if n_sm == 0 || n_sm > 15 || rx.sm_index() >= n_sm {
        return stop("rx mailbox index outside the block's nSM (index / dimensioning)", rx.sm_index(), dim as u64);
    }
    serial_println!(
        "[hsp] dim={:#x} (nSM={} nSS={} nAS={}) rx sm{} @ {:#x} — touching (one read-only sample) ::",
        dim,
        n_sm,
        (dim >> 4) & 0xf,
        (dim >> 8) & 0xf,
        rx.sm_index(),
        rx.word_pa()
    );
    let raw = r32(rx.word_pa());
    serial_println!("[tcu] rx-mbox raw={:#010x} full={} {} (arm sample)", raw, (raw & TCU_FULL != 0) as u8, decode(raw));

    RX_MBOX.store(rx.word_pa(), Ordering::Release);
    TX_MBOX.store(tx.word_pa(), Ordering::Release);
    ARMED.store(true, Ordering::Release);
    super::sched::spawn("tcu-probe", sampler, 0, 0);
    serial_println!("[tcu] sampler task spawned (boot core; samples every pass, census ~1 s; read-only) ::");
}

/// The TCU word decoded (no consumption: the SPE's parked byte stays parked).
fn decode(raw: u32) -> Decoded {
    Decoded(raw)
}
struct Decoded(u32);
impl core::fmt::Display for Decoded {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let r = self.0;
        write!(
            f,
            "nbytes={} data=[{:02x} {:02x} {:02x}] flush={} hwflush={}",
            (r >> TCU_NBYTES_SHIFT) & 0b11,
            r & 0xff,
            (r >> 8) & 0xff,
            (r >> 16) & 0xff,
            (r & TCU_FLUSH != 0) as u8,
            (r & TCU_HWFLUSH != 0) as u8
        )
    }
}

#[inline(always)]
fn cntpct() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}
#[inline(always)]
fn cntfrq() -> u64 {
    let f: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) f, options(nomem, nostack, preserves_flags)) };
    if f == 0 { 62_500_000 } else { f }
}

/// The boot-core sampler: one read-only read of the RX mailbox word per pass (the same cadence the
/// console pump polls UARTC at), latching FULL edges and value changes so a byte parked between two
/// census lines is never missed; `[tcu] rx-mbox` census once a second. Cooperative — yields every pass.
fn sampler(_arg: usize) {
    let pa = RX_MBOX.load(Ordering::Acquire);
    if pa == 0 || !ARMED.load(Ordering::Acquire) {
        return;
    }
    let period = cntfrq();
    let mut last_census = cntpct();
    let mut last_raw = r32(pa);
    let mut was_full = last_raw & TCU_FULL != 0;
    loop {
        let raw = r32(pa);
        POLLS.fetch_add(1, Ordering::Relaxed);
        if raw != last_raw {
            CHANGES.fetch_add(1, Ordering::Relaxed);
            last_raw = raw;
        }
        let full = raw & TCU_FULL != 0;
        if full {
            LAST_FULL_RAW.store(raw as u64, Ordering::Relaxed);
            if !was_full {
                FULL_EDGES.fetch_add(1, Ordering::Relaxed);
            }
        }
        was_full = full;
        if cntpct().wrapping_sub(last_census) >= period {
            last_census = cntpct();
            let n = CENSUS.fetch_add(1, Ordering::Relaxed) + 1;
            let edges = FULL_EDGES.load(Ordering::Relaxed);
            let lf = LAST_FULL_RAW.load(Ordering::Relaxed) as u32;
            serial_println!(
                "[tcu] rx-mbox raw={:#010x} full={} {} | census={} polls={} full-edges={} changes={} last-full={:#010x} -> {}",
                raw,
                full as u8,
                decode(raw),
                n,
                POLLS.load(Ordering::Relaxed),
                edges,
                CHANGES.load(Ordering::Relaxed),
                lf,
                if full {
                    "FULL-NOW (the SPE parked a byte in the RX mailbox and nobody consumed it)"
                } else if edges > 0 {
                    "FULL-SEEN (the SPE forwards RX into the mailbox; a consumer cleared it)"
                } else {
                    "FULL-NEVER (nothing has arrived on the mailbox so far)"
                }
            );
        }
        super::sched::yield_now();
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// TCURX2 (orin 15, `tcurx` knob = tcuprobe + orinrx + tegra, DEFAULT OFF) — RUNG 2: the CONSUMER.
//
// Everything above this line is the read-only probe of rung 1. Rung 1 flew on render6
// (2026-09-06T01:28Z): a burst `tste\r` into the board left UARTC delivering `s`,`t`,`\r` and the
// probe printing `[tcu] rx-mbox raw=0x82006574 full=1 nbytes=2 data=[74 65 00] … full-edges=1
// changes=1` — bit31 set, [25:24]=2, byte0=0x74 't', byte1=0x65 'e', i.e. EXACTLY the two bytes
// UARTC lost — and it STAYED full for the rest of the boot because the probe deliberately never
// consumes. `TCURX-DESIGN.md` §7 row 1: "TCURX rung 2: replace serialrx::drain's LSR/RBR poll with
// the mailbox read + write-back (§4)". This block is that write-back. Per R19 the RBR poll is NOT
// removed — the mailbox is ADDED as a second source in `drain`, so the UARTC path stays open.
//
// THE ONE WRITE THIS ARC INTRODUCES, and its whole extent: the 32-bit store to the RX mailbox WORD
// at `RX_MBOX` — the slot the TCU protocol makes the CONSUMER's to clear (§4, edk2-nvidia
// `TegraCombinedSerialPortLib.c`, BSD-2-Clause-Patent). No other HSP register, no UART register, no
// doorbell, no interrupt enable. The address is the same one rung 1 resolved from the LIVE DTB and
// bounded by the block's own `HSP_INT_DIMENSIONING` before its first touch; if that resolution
// failed, `ARMED` is false and this path is inert.
//
// PROTOCOL (§4), implemented literally: read the word; bit 31 clear -> nothing to take. Bit 31 set
// -> `n` = bits [25:24]. Take byte 0 (bits [7:0]); write the word back with the remaining `n-1`
// bytes shifted down 8 and the count decremented, keeping bit 31 set while bytes remain, and
// writing 0 when none do — the zero word is what tells the SPE the slot is free. A FULL word with
// n == 0 is a pure flush/hw-flush tag: it carries no byte, so it is consumed with a 0 write and
// reported as "no byte" rather than injecting a phantom key.
//
// THE RUNG-1 SAMPLER STAYS, UNCHANGED AND STILL READ-ONLY. It does not fight this consumer: it only
// ever `r32`s the word, so the two racing on the same address can at worst make the sampler print a
// word this consumer has already replaced. Expect its census to read `full=0` most of the time once
// `tcurx` is on (that is the fix working) with `full-edges`/`changes` still climbing as the SPE
// refills — and DO NOT read `full=0` there as "nothing arrived"; `[serialrx] … mbox=` is the count
// that answers that. The sampler never writes; consumption happens only here.
/// Take ONE byte from the TCU RX mailbox and consume it per §4. `None` = nothing pending (or the
/// probe never armed). Called from `serial::serialrx::drain` off the `SERIAL_PORT` lock, so the
/// per-byte witness below is safe to print from here.
#[cfg(feature = "tcurx")]
pub fn rx_mbox_take() -> Option<u8> {
    let pa = RX_MBOX.load(Ordering::Acquire);
    if pa == 0 || !ARMED.load(Ordering::Acquire) {
        return None;
    }
    let raw = r32(pa);
    if raw & TCU_FULL == 0 {
        return None;
    }
    let n = (raw >> TCU_NBYTES_SHIFT) & 0b11;
    if n == 0 {
        w32(pa, 0);
        TOOK_TAGS.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let b = (raw & 0xff) as u8;
    let left = n - 1;
    // Remaining bytes shift down one lane; count decremented; bit 31 held while any remain, cleared
    // (whole word 0) when none do. `left` is 0..=2, so `8 * left` is 0/8/16 — never a wide shift.
    let word = if left == 0 { 0 } else { ((raw >> 8) & ((1u32 << (8 * left)) - 1)) | (left << TCU_NBYTES_SHIFT) | TCU_FULL };
    w32(pa, word);
    let took = TOOK.fetch_add(1, Ordering::Relaxed) + 1;
    serial_println!(
        "[tcurx] took={:#04x} '{}' left={} word={:#010x} <- raw={:#010x} @ {:#x} n={} took-total={} tags={}",
        b,
        if (0x20u8..0x7f).contains(&b) { b as char } else { '.' },
        left,
        word,
        raw,
        pa,
        n,
        took,
        TOOK_TAGS.load(Ordering::Relaxed)
    );
    Some(b)
}

/// Bytes this consumer has taken out of the RX mailbox this boot (the `mbox=` field's source).
#[cfg(feature = "tcurx")]
static TOOK: AtomicU64 = AtomicU64::new(0);
/// FULL words with nbytes == 0 (flush / hw-flush tags) consumed without yielding a byte.
#[cfg(feature = "tcurx")]
static TOOK_TAGS: AtomicU64 = AtomicU64::new(0);

/// Bytes taken from the RX mailbox so far — read by `serial::serialrx::census` for `mbox=`.
#[cfg(feature = "tcurx")]
pub fn rx_mbox_took() -> u64 {
    TOOK.load(Ordering::Relaxed)
}

/// The ONLY write in this file, and the only one the `tcurx` knob adds anywhere: the RX mailbox
/// word. Kept beside the read helpers so a reviewer sees the whole write surface in one place.
#[cfg(feature = "tcurx")]
#[inline(always)]
fn w32(pa: u64, v: u32) {
    unsafe { core::ptr::write_volatile(pa as *mut u32, v) };
}

/// Is the RX mailbox path live — i.e. did rung 1 resolve the word address out of the LIVE DTB and
/// arm the probe? `serial::serialrx`'s RXMERGE arbitration (A37) reads this to decide which of the
/// two RX readers owns the console: an armed mailbox means the SPE's forward is available and the
/// direct UARTC RBR poll must be PARKED, because that read pops the byte out of the RX FIFO the SPE
/// is reading too. False = the DTB never resolved, `rx_mbox_take` is inert, and the RBR poll stays
/// the only source (R19: the path that failed under conditions keeps its code and its fallback).
#[cfg(feature = "tcurx")]
pub fn rx_mbox_armed() -> bool {
    ARMED.load(Ordering::Acquire) && RX_MBOX.load(Ordering::Acquire) != 0
}

// ═══ RXBURST (A16, orin 17) — DRAIN THE WORD, NOT THE BYTE ═══════════════════════════════════════
//
// THE DEFECT, from render8 2026-09-06 (`~/unaos-bench/scratch/orin17/render8-boot.log`, slice lines
// 3238-3256, `policy=mbox-only`). The burst `tste` + CR was injected as ONE 5-byte write and the
// mailbox delivered THREE bytes:
//
//   :3238  [tcurx] took=0x74 't' left=2 word=0x82007473 <- raw=0x83747374 @ 0x3c10000 n=3 took-total=1
//   :3240  [tcurx] took=0x73 's' left=1 word=0x81000074 <- raw=0x82007473 @ 0x3c10000 n=2 took-total=2
//   :3242  [tcurx] took=0x74 't' left=0 word=0x00000000 <- raw=0x81000074 @ 0x3c10000 n=1 took-total=3
//   :3249  [tcu] rx-mbox raw=0x00000000 full=0 … full-edges=1 changes=3 last-full=0x81000074
//   :3255  [serialrx] rx=3 (+3) polls=0 refused=0 ovrf=0 … mbox=3
//   :3256  [rxmerge] census policy=mbox-only seq=3 uartc=0 mbox=3 dup=0 reorder=0 parked=9098202
//
// `e` and CR reached nothing. Under `mbox-only` the UARTC RBR is parked BY POLICY (A37), so those
// two bytes exist nowhere — not in a FIFO, not in the mailbox, not in the key path. The same five
// bytes paced at 200 ms/byte arrived 5 of 5. `full-edges=1` for the whole 76-second boot: the SPE
// posted ONE word, ever.
//
// MECHANISM, and it is OURS, not the SPE's. `rx_mbox_take` consumed ONE BYTE per call and wrote the
// word back with bit 31 STILL ASSERTED while bytes remained — that is what `word=0x82007473` and
// `word=0x81000074` above are — and then printed its ~95-character witness, after which
// `serialrx::deliver` printed a ~65-character `[rxmerge]` line. This console is polled 115200 8N1
// (`tegra::write_byte` spins on THRE), so ~160 characters is ~14 ms of BLOCKING transmit per byte
// taken. The rung-1 sampler's own trace measures the wall time independently: it polls at 45 Hz
// (:3231 `polls=9098327` -> :3249 `polls=9098372`, one census second apart) and it caught BOTH
// intermediate words — `changes=3`, `last-full=0x81000074`, and it never saw the original
// 0x83747374 at all — which takes ~14 ms between takes. The slot was therefore held ~28 ms after
// the SPE posted it. At 115200 the SPE has its next three-byte word ready 0.26 ms after the first,
// and the CCPLEX's slot is ONE WORD DEEP. It had nowhere to put `e` and CR.
//
// Neither the cadence claim nor the ack-ordering claim survives the same evidence: the pump drains
// ~50x/s (render8 `parked=` deltas 28/55/56/60 per census second) but the drain ALREADY looped to
// empty, taking all three bytes in one pass; and the driver clears FULL strictly AFTER copying the
// byte out of the word it read, never before. What was wrong is the SIZE OF THE OCCUPANCY WINDOW.
//
// THE FIX, below. Take the WHOLE WORD and free the slot in the same breath: read the word, copy its
// up-to-three payload bytes into a local, `w32(pa, 0)` immediately, and only then print and deliver.
// The window shrinks from ~28 ms to the handful of cycles between the read and the write, and the
// write-back becomes the edk2 protocol's plain "the consumer clears the slot" rather than the
// partial re-assertion of bit 31 the per-byte shape needed. Then loop: while the word reads FULL
// take the next one, and for a bounded [`BURST_REFILL_US`] window after each word keep re-reading an
// EMPTY word, so the SPE's next post is collected in microseconds instead of at the pump's next
// pass. That refill spin is what covers the 0.26 ms inter-word gap; it is entered ONLY after a word
// has actually been taken, so an idle boot never spins.
//
// WHY NOT THE 250 Hz TICK (A21 `[orinbsptick]`, live on this image). It would not have helped: the
// gap this defect turns on is 0.26 ms and a 250 Hz tick is 4 ms, so the second word would still
// have been dropped before the first harvest. It also costs what the refill spin does not — MMIO
// and `pal::push_event` from IRQ context, against a pump that owns the event queue at task level.
// The pump's cadence is not the bottleneck; the 28 ms of witness printing inside the window was.
//
// WHAT THE NEXT FLIGHT SCORES. `drained_words=` is this burst's word count and `refill=` counts the
// words it took during the refill spin — words the old shape could not have reached at all. A burst
// leg that returns `drained_words=2` with five bytes delivered says occupancy was the whole cause
// (`refill=1` beside it says the second word arrived AFTER we freed the slot, i.e. the SPE retried
// and only the spin could have caught it; `refill=0` says it was already queued). One that still
// returns `drained_words=1` with three bytes says the SPE drops a word it cannot post, with no
// retry at all, and `mbox-only` needs the HSP RX doorbell interrupt (a further rung), not a faster
// poll. Either way A37's per-byte witnesses are unchanged in shape and token set —
// one `[tcurx] took=` and one `[rxmerge] src=mbox` line per delivered byte, in order.
//
// EXACTLY-ONCE is preserved by construction: a word is copied out before it is cleared, it is
// cleared exactly once, and every byte copied is delivered exactly once by the caller's `for` loop.
// `rx_mbox_take` (the per-byte shape) is KEPT and still correct — R19: a path that failed under
// conditions keeps its code — it simply has no caller on this policy.

/// Payload bytes one TCU word carries (bits [23:0], byte 0 in [7:0]).
#[cfg(feature = "tcurx")]
const TCU_WORD_BYTES: usize = 3;
/// Words one [`rx_mbox_drain`] call takes before handing the pump back its pass. A CAP, never a
/// drop: the bound is checked BEFORE the word is read, so a capped burst leaves the slot exactly as
/// the SPE left it and the next pass takes it.
#[cfg(feature = "tcurx")]
const BURST_MAX_WORDS: usize = 16;
#[cfg(feature = "tcurx")]
const BURST_MAX_BYTES: usize = BURST_MAX_WORDS * TCU_WORD_BYTES;
/// How long the drain keeps re-reading an EMPTY word after taking one. Sized off the wire: at
/// 115200 8N1 the SPE fills a fresh three-byte word every 0.26 ms, so a window a few words wide
/// carries a burst across intact while never being entered on an idle pass.
#[cfg(feature = "tcurx")]
const BURST_REFILL_US: u64 = 2_000;

/// Words taken out of the RX mailbox this boot, and how many of those the refill spin caught.
#[cfg(feature = "tcurx")]
static WORDS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "tcurx")]
static REFILL_WORDS: AtomicU64 = AtomicU64::new(0);
/// Drains that took at least one word, and drains that hit [`BURST_MAX_WORDS`].
#[cfg(feature = "tcurx")]
static BURSTS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "tcurx")]
static CAPPED: AtomicU64 = AtomicU64::new(0);

/// One drain's harvest: the bytes, and enough provenance for the per-byte `[tcurx]` witness to say
/// which word each came out of. Lives on the caller's stack (~250 bytes) and is never shared.
#[cfg(feature = "tcurx")]
pub struct RxBurst {
    buf: [u8; BURST_MAX_BYTES],
    raws: [u32; BURST_MAX_BYTES],
    n: usize,
    words: usize,
    refill: usize,
    tags: usize,
    capped: bool,
    pa: u64,
    took_base: u64,
}

#[cfg(feature = "tcurx")]
impl RxBurst {
    const fn empty() -> Self {
        Self {
            buf: [0; BURST_MAX_BYTES],
            raws: [0; BURST_MAX_BYTES],
            n: 0,
            words: 0,
            refill: 0,
            tags: 0,
            capped: false,
            pa: 0,
            took_base: 0,
        }
    }
    /// Bytes harvested (0 on an empty mailbox, or when the probe never armed).
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// Words taken from the mailbox by this drain — the `drained_words=` field.
    pub fn words(&self) -> usize {
        self.words
    }
    /// Byte `i`, clamped rather than panicking: a `Location` in a console drain is not a diagnostic.
    pub fn byte(&self, i: usize) -> u8 {
        self.buf[if i < BURST_MAX_BYTES { i } else { BURST_MAX_BYTES - 1 }]
    }

    /// A16's per-byte witness, printed AFTER the mailbox is already clear — that ordering is the
    /// whole fix, so it is stated here and not left to the caller's discretion. Token set is
    /// render8's plus `drained_words=` / `refill=`; `word=` stays, and is now always the 0 this
    /// consumer writes, because the slot is released in one write instead of three.
    pub fn witness(&self, i: usize) {
        if i >= self.n {
            return;
        }
        let b = self.buf[i];
        let raw = self.raws[i];
        serial_println!(
            "[tcurx] took={:#04x} '{}' left={} word={:#010x} <- raw={:#010x} @ {:#x} n={} took-total={} tags={} drained_words={} refill={} burst_bytes={} capped={} words-total={} refill-total={} bursts={}",
            b,
            if (0x20u8..0x7f).contains(&b) { b as char } else { '.' },
            self.n - i - 1,
            0u32,
            raw,
            self.pa,
            (raw >> TCU_NBYTES_SHIFT) & 0b11,
            self.took_base + i as u64 + 1,
            TOOK_TAGS.load(Ordering::Relaxed),
            self.words,
            self.refill,
            self.n,
            self.capped as u8,
            WORDS.load(Ordering::Relaxed),
            REFILL_WORDS.load(Ordering::Relaxed),
            BURSTS.load(Ordering::Relaxed)
        );
    }
}

/// THE DRAIN. Take whole words out of the TCU RX mailbox until it is empty, clearing the slot in
/// the same breath as the read, and return the bytes for the caller to print and deliver off the
/// hot path. Bounded three ways: [`BURST_MAX_WORDS`] words, a [`BURST_REFILL_US`] refill window
/// that is only ever entered after a word has been taken, and the FULL bit itself — every iteration
/// either clears bit 31 or leaves the loop, so it cannot spin on a stuck word.
#[cfg(feature = "tcurx")]
pub fn rx_mbox_drain() -> RxBurst {
    let mut b = RxBurst::empty();
    let pa = RX_MBOX.load(Ordering::Acquire);
    if pa == 0 || !ARMED.load(Ordering::Acquire) {
        return b;
    }
    b.pa = pa;
    b.took_base = TOOK.load(Ordering::Relaxed);
    let window = (cntfrq() / 1_000_000).max(1) * BURST_REFILL_US;
    let mut since_word = cntpct();
    // Did the loop read an EMPTY word since the last one it took? That is what separates a word the
    // SPE posted while we were still here (`refill=`, only reachable because the slot was freed in
    // microseconds) from one that was already queued behind the first.
    let mut spun = false;
    loop {
        if b.words >= BURST_MAX_WORDS {
            b.capped = true;
            CAPPED.fetch_add(1, Ordering::Relaxed);
            break;
        }
        let raw = r32(pa);
        if raw & TCU_FULL == 0 {
            // The refill window — the SPE's next word is 0.26 ms behind this one on a 115200 burst,
            // and the pump's next pass is ~20 ms away. Never entered on an idle drain.
            if b.words == 0 || cntpct().wrapping_sub(since_word) >= window {
                break;
            }
            spun = true;
            core::hint::spin_loop();
            continue;
        }
        // ── THE OCCUPANCY WINDOW, and its whole extent: the read above, the clear below. Nothing
        //    between them touches the serial port, a lock, or the event queue. render8's defect was
        //    ~28 ms of witness printing sitting in exactly this gap with bit 31 still asserted.
        let n = ((raw >> TCU_NBYTES_SHIFT) & 0b11) as usize;
        w32(pa, 0);
        // ── window closed; the SPE may post again from here on. Bookkeeping only, below.
        if spun {
            b.refill += 1;
            REFILL_WORDS.fetch_add(1, Ordering::Relaxed);
            spun = false;
        }
        b.words += 1;
        WORDS.fetch_add(1, Ordering::Relaxed);
        since_word = cntpct();
        if n == 0 {
            // A FULL word with nbytes == 0 is a pure flush / hw-flush tag: consumed, no byte.
            b.tags += 1;
            TOOK_TAGS.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let mut k = 0;
        while k < n && b.n < BURST_MAX_BYTES {
            b.buf[b.n] = ((raw >> (8 * k)) & 0xff) as u8;
            b.raws[b.n] = raw;
            b.n += 1;
            k += 1;
            TOOK.fetch_add(1, Ordering::Relaxed);
        }
    }
    if b.words > 0 {
        BURSTS.fetch_add(1, Ordering::Relaxed);
    }
    b
}
