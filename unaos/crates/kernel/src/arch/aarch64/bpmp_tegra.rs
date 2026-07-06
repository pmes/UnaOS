// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// JB1b: the Tegra234 BPMP IVC channel, first light = MRQ_PING. The Orin USB/video arcs are gated
// behind BPMP MRQs (JX1: gated Tegra partitions are EL3-fatal to touch until ungated), and this
// module is the transport those MRQs ride on. Geometry comes from the firmware DTB, resolved on
// silicon by `fdt_tegra::bpmp_geometry` (metal-verified 2026-07-06: shmem TX 0x4007_0000 / RX
// 0x4007_1000 in SYSRAM — GiB 1, which mmu_tegra now maps Device-nGnRE — and the HSP doorbell
// block at 0x03c0_0000).
//
// Protocol facts (Linux drivers/firmware/tegra/{ivc.c,bpmp.c,bpmp-tegra186.c}, soc/tegra/
// bpmp-abi.h — the reference implementation for this ABI):
//   * Each IVC queue = a 128-byte header + num_frames*frame_size of data. The header's first
//     64-byte line is WRITER-owned {u32 count @+0, u32 state @+4}; the second 64-byte line is
//     READER-owned {u32 count @+64}. Queue empty <=> writer count == reader count.
//   * The BPMP protocol uses 128-byte frames, ONE frame per channel, channel stride =
//     128 (header) + 128 (frame) = 256 bytes, at the SAME index in the TX and RX areas. The
//     CPU-to-BPMP synchronous command channel is INDEX 3 (tegra186/194/234 soc table:
//     cpu_tx.offset = 3; the response comes back on the rx area's index-3 queue).
//   * Message frame: { u32 code (MRQ number out / ignored in), u32 flags (bit0 = MSG_ACK:
//     please respond), u8 data[120] }. Response frame data begins with
//     struct mrq_response { i32 err; u32 flags } then the payload.
//   * MRQ_PING = 0: request { u32 challenge }, response payload { u32 reply } where
//     reply = challenge << 1 (carry dropped).
//   * Channel (re-)establishment: writer-side header `state` field — 0 = ESTABLISHED, 1 = SYNC,
//     2 = ACK. We write SYNC + ring; the BPMP zeroes its counters and answers ACK (visible as
//     the state in OUR rx queue header); we zero our counters, write ESTABLISHED, ring. The BPMP
//     firmware is always up (UEFI used this very channel), so a fresh SYNC re-establishes.
//   * Doorbell: the HSP doorbell region sits at hsp_base + (1 + nSM/2 + nSS + nAS) * 0x10000,
//     where the counts come from HSP_INT_DIMENSIONING (hsp_base + 0x380, fields nSM [3:0],
//     nSS [7:4], nAS [11:8]). Doorbells are 0x100 apart; the BPMP's is INDEX 3 (Linux
//     tegra-hsp db_map: { "bpmp", master 19, index 3 }); ring = write 1 to TRIGGER (+0x0).
//
// EL3-fatal discipline (the JX1 lesson): every new MMIO address class gets a serial line BEFORE
// the first touch, so a dead boot's last line names the killer. HSP and SYSRAM are always-on
// fabric (UEFI itself runs this channel), so the risk is low — but the discipline stands.

use super::fdt_tegra::BpmpGeom;

const IVC_STATE_ESTABLISHED: u32 = 0;
const IVC_STATE_SYNC: u32 = 1;
const IVC_STATE_ACK: u32 = 2;

/// Channel stride: 128-byte header + one 128-byte frame.
const CH_STRIDE: u64 = 256;
/// Header field offsets within a queue.
const OFF_W_COUNT: u64 = 0;
const OFF_W_STATE: u64 = 4;
const OFF_R_COUNT: u64 = 64;
const OFF_FRAME: u64 = 128;
/// The CPU-to-BPMP synchronous command channel index (tegra186/194/234 soc table).
const CPU_TX_CH: u64 = 3;

const MSG_ACK: u32 = 1 << 0;
const MRQ_PING: u32 = 0;

const HSP_INT_DIMENSIONING: u64 = 0x380;
const HSP_DB_STRIDE: u64 = 0x100;
const HSP_DB_INDEX_BPMP: u64 = 3;
const HSP_DB_TRIGGER: u64 = 0x0;
const HSP_DB_ENABLE: u64 = 0x4;

#[inline]
fn r32(pa: u64) -> u32 {
    unsafe { core::ptr::read_volatile(pa as *const u32) }
}
#[inline]
fn w32(pa: u64, v: u32) {
    unsafe { core::ptr::write_volatile(pa as *mut u32, v) }
}
#[inline]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Bounded wait: spins until `f()` is true or ~`ms` milliseconds of CNTPCT have passed.
fn wait_ms(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let freq: u64;
    let start: u64;
    unsafe {
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) start, options(nomem, nostack, preserves_flags));
    }
    let budget = freq / 1000 * ms;
    loop {
        if f() {
            return true;
        }
        let now: u64;
        unsafe {
            core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) now, options(nomem, nostack, preserves_flags));
        }
        if now.wrapping_sub(start) > budget {
            return false;
        }
        core::hint::spin_loop();
    }
}

struct Doorbell {
    trigger: u64,
    enable: u64,
}

/// Locate the BPMP doorbell from the HSP dimensioning register. Prints the derivation (this is
/// the first HSP touch of the boot). Sanity-caps the field counts so a garbage read can't send
/// the doorbell pointer into the weeds.
fn doorbell(hsp_base: u64) -> Option<Doorbell> {
    serial_println!(":: tegra: JB1b — touching HSP @{:#x} (dimensioning) ::", hsp_base);
    let dim = r32(hsp_base + HSP_INT_DIMENSIONING);
    let n_sm = (dim & 0xf) as u64;
    let n_ss = ((dim >> 4) & 0xf) as u64;
    let n_as = ((dim >> 8) & 0xf) as u64;
    if n_sm > 16 || n_ss > 16 || n_as > 16 {
        serial_println!(":: tegra: JB1b — HSP dimensioning {:#x} implausible; STOP ::", dim);
        return None;
    }
    let db_base = hsp_base + (1 + n_sm / 2 + n_ss + n_as) * 0x10000;
    let db = db_base + HSP_DB_INDEX_BPMP * HSP_DB_STRIDE;
    serial_println!(
        ":: tegra: JB1b — HSP dim={:#x} (nSM={} nSS={} nAS={}) db_base={:#x} bpmp_db={:#x} enable={:#x} ::",
        dim,
        n_sm,
        n_ss,
        n_as,
        db_base,
        db,
        r32(db + HSP_DB_ENABLE),
    );
    Some(Doorbell { trigger: db + HSP_DB_TRIGGER, enable: db + HSP_DB_ENABLE })
}

/// JB1b: establish the IVC command channel and exchange one MRQ_PING. Pre-drop, EL2, polled.
/// Returns true iff the ping round-trip verified (reply == challenge << 1).
pub fn jb1b_ping(geom: &BpmpGeom) -> bool {
    serial_println!(
        ":: tegra: JB1b — geom: shmem_tx={:#x} shmem_rx={:#x} hsp={:#x} db_master={} ::",
        geom.shmem_tx,
        geom.shmem_rx,
        geom.hsp_base,
        geom.db_master,
    );
    let Some(db) = doorbell(geom.hsp_base) else { return false };
    let _ = db.enable; // read in the derivation print; kept for the follow-on RX-interrupt arc

    // Queue bases for the command channel (index 3, stride 256) — TX we write, RX the BPMP writes.
    let txq = geom.shmem_tx + CPU_TX_CH * CH_STRIDE;
    let rxq = geom.shmem_rx + CPU_TX_CH * CH_STRIDE;
    serial_println!(
        ":: tegra: JB1b — touching SYSRAM: txq={:#x} rxq={:#x} pre-sync counts/state tx=[{:#x} {:#x} {:#x}] rx=[{:#x} {:#x} {:#x}] ::",
        txq,
        rxq,
        r32(txq + OFF_W_COUNT),
        r32(txq + OFF_W_STATE),
        r32(txq + OFF_R_COUNT),
        r32(rxq + OFF_W_COUNT),
        r32(rxq + OFF_W_STATE),
        r32(rxq + OFF_R_COUNT),
    );

    // Re-establish the channel: SYNC -> (peer ACK | peer SYNC) -> ESTABLISHED. The BPMP answers
    // in microseconds; 100 ms budgets are generous.
    w32(txq + OFF_W_STATE, IVC_STATE_SYNC);
    dsb();
    w32(db.trigger, 1);
    let mut acked = false;
    let mut established = false;
    let ok = wait_ms(100, || match r32(rxq + OFF_W_STATE) {
        IVC_STATE_ACK => {
            // Peer cleared its side in response to our SYNC; clear ours, declare established.
            w32(txq + OFF_W_COUNT, 0);
            w32(rxq + OFF_R_COUNT, 0);
            dsb();
            w32(txq + OFF_W_STATE, IVC_STATE_ESTABLISHED);
            dsb();
            w32(db.trigger, 1);
            established = true;
            true
        }
        IVC_STATE_SYNC => {
            // Peer is (re)starting too: clear ours, answer ACK, wait for its ESTABLISHED.
            w32(txq + OFF_W_COUNT, 0);
            w32(rxq + OFF_R_COUNT, 0);
            dsb();
            w32(txq + OFF_W_STATE, IVC_STATE_ACK);
            dsb();
            w32(db.trigger, 1);
            acked = true;
            false
        }
        IVC_STATE_ESTABLISHED => {
            if established {
                true
            } else if acked {
                // The three-way tail (the JB1b first-boot lesson): we answered peer-SYNC with
                // ACK, the peer established — now WE establish and the channel is up. The first
                // metal run timed out exactly here, with the BPMP already at ESTABLISHED.
                w32(txq + OFF_W_STATE, IVC_STATE_ESTABLISHED);
                dsb();
                w32(db.trigger, 1);
                established = true;
                true
            } else {
                // Stale ESTABLISHED from before the peer notices our SYNC — keep waiting; the
                // BPMP always answers a SYNC.
                false
            }
        }
        _ => false,
    });
    if !ok && !established {
        serial_println!(
            ":: tegra: JB1b — IVC sync TIMEOUT (rx state={:#x} tx state={:#x} acked={}); STOP ::",
            r32(rxq + OFF_W_STATE),
            r32(txq + OFF_W_STATE),
            acked,
        );
        return false;
    }
    serial_println!(":: tegra: JB1b — IVC channel ESTABLISHED ::");

    // One MRQ_PING: frame = { code=MRQ_PING, flags=MSG_ACK, data[0..4]=challenge }.
    let challenge: u32 = 0x0055_AA33;
    let frame = txq + OFF_FRAME;
    w32(frame, MRQ_PING);
    w32(frame + 4, MSG_ACK);
    w32(frame + 8, challenge);
    dsb();
    w32(txq + OFF_W_COUNT, r32(txq + OFF_W_COUNT).wrapping_add(1));
    dsb();
    w32(db.trigger, 1);

    // Response ready when the rx queue's writer count moves past our reader count.
    let ready = wait_ms(100, || r32(rxq + OFF_W_COUNT) != r32(rxq + OFF_R_COUNT));
    if !ready {
        serial_println!(":: tegra: JB1b — MRQ_PING TIMEOUT (no response frame); STOP ::");
        return false;
    }
    // Inbound frame layout (Linux tegra_bpmp_channel_read): the mb_data CODE field carries the
    // mrq_response err, FLAGS its flags, and the MRQ payload starts at data[0] (+8).
    let rframe = rxq + OFF_FRAME;
    let err = r32(rframe) as i32;
    let reply = r32(rframe + 8);
    // Consume the frame so the channel stays balanced for the next MRQ.
    w32(rxq + OFF_R_COUNT, r32(rxq + OFF_R_COUNT).wrapping_add(1));
    dsb();
    let want = challenge << 1;
    let pass = err == 0 && reply == want;
    serial_println!(
        ":: tegra: JB1b — MRQ_PING err={} reply={:#x} (want {:#x}) -> {} ::",
        err,
        reply,
        want,
        if pass { "PASS" } else { "FAIL" },
    );
    pass
}
