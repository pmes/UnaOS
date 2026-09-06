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
