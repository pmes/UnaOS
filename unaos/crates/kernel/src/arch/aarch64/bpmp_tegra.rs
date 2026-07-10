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
// The partition-ungate MRQs (soc/tegra/bpmp-abi.h): MRQ_RESET { cmd, reset_id } with
// CMD_RESET_DEASSERT = 2; MRQ_CLK single word cmd_and_id = subcommand[31:24] | clk_id[23:0]
// with CMD_CLK_ENABLE = 7; MRQ_PG { cmd = CMD_PG_SET_STATE = 1, id, state } with
// PG_STATE_ON = 1.
const MRQ_RESET: u32 = 20;
const CMD_RESET_DEASSERT: u32 = 2;
const MRQ_CLK: u32 = 22;
const CMD_CLK_ENABLE: u32 = 7;
// MRQ_CLK subcommand: read a clock's actual enabled state (soc/tegra/bpmp-abi.h CMD_CLK_IS_ENABLED).
// Pure query — response payload[0] = 1 (running) / 0 (gated). Used by jb7_clocks_query (arc A).
const CMD_CLK_IS_ENABLED: u32 = 6;
const MRQ_PG: u32 = 66;
const CMD_PG_SET_STATE: u32 = 1;
const PG_STATE_ON: u32 = 1;

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

/// An established IVC command channel: queue bases + the doorbell trigger. Single in-flight
/// message (the BPMP command channel is one frame deep).
pub struct Chan {
    txq: u64,
    rxq: u64,
    trigger: u64,
}

impl Chan {
    /// One synchronous MRQ round-trip: write the frame (code=mrq, MSG_ACK, payload words), ring,
    /// poll for the response, consume it. Returns (err, first-two payload words); `None` = timeout.
    pub fn transfer(&self, mrq: u32, payload: &[u32]) -> Option<(i32, [u32; 2])> {
        let frame = self.txq + OFF_FRAME;
        w32(frame, mrq);
        w32(frame + 4, MSG_ACK);
        for (i, &p) in payload.iter().enumerate().take(28) {
            w32(frame + 8 + (i as u64) * 4, p);
        }
        dsb();
        w32(self.txq + OFF_W_COUNT, r32(self.txq + OFF_W_COUNT).wrapping_add(1));
        dsb();
        w32(self.trigger, 1);
        if !wait_ms(100, || r32(self.rxq + OFF_W_COUNT) != r32(self.rxq + OFF_R_COUNT)) {
            return None;
        }
        let rframe = self.rxq + OFF_FRAME;
        let err = r32(rframe) as i32;
        let out = [r32(rframe + 8), r32(rframe + 12)];
        w32(self.rxq + OFF_R_COUNT, r32(self.rxq + OFF_R_COUNT).wrapping_add(1));
        dsb();
        Some((err, out))
    }
}

/// JB1b: establish the IVC command channel and exchange one MRQ_PING. Pre-drop, EL2, polled.
/// Returns the live channel iff the ping round-trip verified (reply == challenge << 1).
pub fn jb1b_ping(geom: &BpmpGeom) -> Option<Chan> {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags));
    }
    serial_println!(
        ":: tegra: JB1b — geom: shmem_tx={:#x} shmem_rx={:#x} hsp={:#x} db_master={} DAIF={:#06b} ::",
        geom.shmem_tx,
        geom.shmem_rx,
        geom.hsp_base,
        geom.db_master,
        (daif >> 6) & 0b1111,
    );
    let Some(db) = doorbell(geom.hsp_base) else { return None };
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
        return None;
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
        return None;
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
    if pass { Some(Chan { txq, rxq, trigger: db.trigger }) } else { None }
}

/// JB1c: ungate the XUSB host partition via the established BPMP channel, then re-probe the xHCI
/// capability block that was EL3-fatal to touch in JX1. Order per Linux xhci-tegra: power domains
/// ON (MRQ_PG), clocks ENABLE (MRQ_CLK), resets DEASSERT (MRQ_RESET) — every id read off the
/// firmware DTB (`fdt_tegra::xusb_ids`), every response err printed, and the probe announces
/// itself first (the JX1 EL3-fatal discipline: if the ungate was insufficient, the last line
/// names the touch).
///
/// Returns true iff the block came ALIVE (a sane capability word read back) — the gate the JB2b
/// xHCI attach hangs off: a false here means 0x0361_0000 must not be touched again this boot.
pub fn jb1c_ungate_xusb(chan: &Chan, ids: &super::fdt_tegra::XusbIds) -> bool {
    serial_println!(
        ":: tegra: JB1c — XUSB ids from DTB: {} clocks, {} resets, {} power-domains ::",
        ids.n_clocks,
        ids.n_resets,
        ids.n_pds,
    );
    for i in 0..ids.n_pds {
        let id = ids.pds[i];
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, id, PG_STATE_ON]) {
            Some((err, _)) => {
                serial_println!(":: tegra: JB1c — PG {} ON -> err={} ::", id, err)
            }
            None => {
                serial_println!(":: tegra: JB1c — PG {} ON -> TIMEOUT; STOP ::", id);
                return false;
            }
        }
    }
    for i in 0..ids.n_clocks {
        let id = ids.clocks[i];
        match chan.transfer(MRQ_CLK, &[(CMD_CLK_ENABLE << 24) | (id & 0x00ff_ffff)]) {
            Some((err, _)) => {
                serial_println!(":: tegra: JB1c — CLK {} enable -> err={} ::", id, err)
            }
            None => {
                serial_println!(":: tegra: JB1c — CLK {} enable -> TIMEOUT; STOP ::", id);
                return false;
            }
        }
    }
    for i in 0..ids.n_resets {
        let id = ids.resets[i];
        match chan.transfer(MRQ_RESET, &[CMD_RESET_DEASSERT, id]) {
            Some((err, _)) => {
                serial_println!(":: tegra: JB1c — RESET {} deassert -> err={} ::", id, err)
            }
            None => {
                serial_println!(":: tegra: JB1c — RESET {} deassert -> TIMEOUT; STOP ::", id);
                return false;
            }
        }
    }
    // The moment of truth: the read that was an EL3-fatal SError in JX1. Announce first.
    const XUSB_HOST: u64 = 0x0361_0000;
    serial_println!(":: tegra: JB1c — probing XUSB host @{:#x} (was EL3-fatal in JX1) ::", XUSB_HOST);
    let cap0 = r32(XUSB_HOST);
    if cap0 == 0xFFFF_FFFF || cap0 == 0 {
        serial_println!(":: tegra: JB1c — XUSB cap0={:#010x} (still gated/absent) ::", cap0);
        return false;
    }
    let caplength = (cap0 & 0xff) as u64;
    let hcs1 = r32(XUSB_HOST + 0x04);
    serial_println!(
        ":: tegra: JB1c — XUSB ALIVE: xHCI v{:x}.{:02x} CAPLENGTH={:#x} HCSPARAMS1={:#010x} HCCPARAMS1={:#010x} USBCMD={:#010x} USBSTS={:#010x} -> PASS ::",
        (cap0 >> 24) & 0xff,
        (cap0 >> 16) & 0xff,
        caplength,
        hcs1,
        r32(XUSB_HOST + 0x10),
        r32(XUSB_HOST + caplength),
        r32(XUSB_HOST + caplength + 0x04),
    );
    // JB2a: the port survey — pure PORTSC reads (xHCI op_base + 0x400 + 0x10*port), no writes, no
    // reset (the resident XUSB firmware state stays untouched). CCS (bit 0) = a device is
    // physically connected; PLS [8:5] = link state; speed [13:10] (1=FS 2=LS 3=HS 4=SS). With a
    // keyboard plugged into the Orin, one line here is the JB2 enumeration arc's first hardware
    // evidence. MaxPorts = HCSPARAMS1[31:24].
    let op = XUSB_HOST + caplength;
    let max_ports = ((hcs1 >> 24) & 0xff) as u64;
    for port in 0..max_ports.min(16) {
        let sc = r32(op + 0x400 + 0x10 * port);
        if sc & 1 != 0 {
            serial_println!(
                ":: tegra: JB2a — port {} CONNECTED: PORTSC={:#010x} (PLS={} speed={}) ::",
                port + 1,
                sc,
                (sc >> 5) & 0xf,
                (sc >> 10) & 0xf,
            );
        }
    }
    serial_println!(":: tegra: JB2a — port survey done ({} ports scanned) ::", max_ports);
    true
}

// JB2c step 1: the USB2 bias-pad tracking clock. TEGRA234_CLK_USB2_TRK = 165
// (dt-bindings/clock/tegra234-clock.h line 323: `#define TEGRA234_CLK_USB2_TRK 165U` — verified
// against live mainline 2026-07-06, and cross-corroborated by the JB0 PWM3 clk=107/reset=70 IDs
// reading out of the same headers). The bias pad's tracking pass (padctl BIAS_PAD_CTL1 USB2_PD_TRK
// -> TRK_COMPLETED) needs this gate running; NVIDIA's ExitBootServices teardown left it disabled
// along with the pad power-down. Linux `tegra186_utmi_bias_pad_power_on` enables it before tracking
// (and, since tegra234 `trk_hw_mode=false`, disables it again after — we leave it on: harmless for
// first light, one fewer MRQ on the scarce attended boot, and it does not re-trigger tracking once
// TRK_COMPLETED is W1C'd).
const CLK_USB2_TRK: u32 = 165;

/// JB2c step 1: enable the USB2 bias-pad tracking clock over the proven BPMP channel (same MRQ_CLK
/// transport as JB0/JB1c). Returns true iff the enable acked err=0. Best-effort: on timeout/err the
/// padctl helper still runs the power-up (tracking then just times out its warn-only poll and the
/// pads still come out of power-down), so the caller logs but does not gate on this.
pub fn jb2c_usb2_trk_clk(chan: &Chan) -> bool {
    match chan.transfer(MRQ_CLK, &[(CMD_CLK_ENABLE << 24) | (CLK_USB2_TRK & 0x00ff_ffff)]) {
        Some((err, _)) => {
            serial_println!(
                ":: tegra: JB2c — USB2_TRK clk {} enable -> err={} ::",
                CLK_USB2_TRK, err
            );
            err == 0
        }
        None => {
            serial_println!(":: tegra: JB2c — USB2_TRK clk enable TIMEOUT (proceeding) ::");
            false
        }
    }
}

// JB0 fan constants. The devkit cooling fan is driven by Tegra234 PWM controller PWM3 at MMIO
// base 0x032A_0000 (verified against tegra234.dtsi `pwm3: pwm@32a0000`; the `pwm-fan` node's
// `&pwm3` channel 0 = the fan; UEFI's own TegraPwmDxe drives this same block during boot).
// PWM_CONTROLLER_PWM_CSR_0 is one 32-bit register at base+0x0 (channel 0): bit[31] = ENABLE,
// DUTY = a fraction of 256 at PWM_DUTY_SHIFT=16, bits[12:0] = SCALE (output frequency only —
// irrelevant to on/off). Per drivers/pwm/pwm-tegra.c the duty count = round(fraction * 256):
// 256 (0x100) = 100% (overflows the nominal 8-bit field into bit 24; UEFI's "medium" is
// 0x8080_0000 = 128 = 50%). METAL-PROVEN 2026-07-06: 0x8100_0000 (100%) spun the fan full — far
// louder than a headless bring-up board needs. Regulated to ~40% (duty 0x66 = 102/256) =
// ENABLE | (0x66 << 16) = 0x8066_0000: ample cooling for the light CAPSTONE/xHCI load, much
// quieter (fan noise ~ RPM²). Retune: duty = round(pct/100 * 256), CSR = 0x8000_0000 |
// (duty << 16) — e.g. 25%->0x8040_0000, 50%->0x8080_0000, 100%->0x8100_0000. (A future arc could
// make this thermal-adaptive via MRQ_THERMAL Tj reads instead of a fixed duty.)
const PWM3_CSR: u64 = 0x032A_0000;
const PWM_FAN_DUTY: u32 = 0x8066_0000; // ENABLE + ~40% duty
// TEGRA234_CLK_PWM3 / TEGRA234_RESET_PWM3 (dt-bindings/{clock,reset}/tegra234-*.h). PWM3 sits in
// an always-on rail (its DT node has no `power-domains`), so — unlike the XUSB host block — this
// CSR is NOT a power-gated aperture: no MRQ_PG and no JX1-class EL3-fatal risk. It only needs its
// clock enabled + module reset deasserted (both undone by the UEFI ExitBootServices teardown)
// before the CSR output toggles.
const CLK_PWM3: u32 = 107;
const RESET_PWM3: u32 = 70;

/// JB0: turn the Orin cooling fan back on. NVIDIA's ExitBootServices teardown disables the fan
/// PWM's clock + module reset, so the fan stops the instant UnaOS takes over and the SoC heats up.
/// (A dead fan can't damage the die — BL31/BPMP arm an OS-independent hardware thermal net that
/// throttles at 103 C and PMIC-resets at 105 C — but restoring cooling keeps the SoC out of the
/// throttle band.) This is the cheapest teardown-restore: re-enable the PWM3 clock + deassert its
/// reset over the SAME BPMP channel JB1 proved, then drive the CSR to a regulated ~40% duty
/// (metal-proven: 100% ran the fan deafeningly). Runs FIRST on Orin, before the XUSB work.
/// Best-effort: a failed clock/reset MRQ prints and skips (no fan is not fatal); the CSR is
/// always-mapped (GiB-0 Device block, same as XUSB) and always-powered, so the write cannot
/// EL3-fault — the pre-touch line is JX1 discipline, not a real hazard here.
pub fn jb0_fan_on(chan: &Chan) {
    match chan.transfer(MRQ_CLK, &[(CMD_CLK_ENABLE << 24) | (CLK_PWM3 & 0x00ff_ffff)]) {
        Some((err, _)) => {
            serial_println!(":: tegra: JB0 — fan PWM3 clk {} enable -> err={} ::", CLK_PWM3, err)
        }
        None => {
            serial_println!(":: tegra: JB0 — fan PWM3 clk enable TIMEOUT; fan skipped ::");
            return;
        }
    }
    match chan.transfer(MRQ_RESET, &[CMD_RESET_DEASSERT, RESET_PWM3]) {
        Some((err, _)) => serial_println!(
            ":: tegra: JB0 — fan PWM3 reset {} deassert -> err={} ::",
            RESET_PWM3, err
        ),
        None => {
            serial_println!(":: tegra: JB0 — fan PWM3 reset deassert TIMEOUT; fan skipped ::");
            return;
        }
    }
    serial_println!(
        ":: tegra: JB0 — touching fan PWM3 CSR @{:#x} (always-on, not PG-gated) ::",
        PWM3_CSR
    );
    w32(PWM3_CSR, PWM_FAN_DUTY);
    dsb();
    serial_println!(
        ":: tegra: JB0 — fan ON (~40% duty): CSR<-{:#010x} (readback {:#010x}) -> PASS ::",
        PWM_FAN_DUTY,
        r32(PWM3_CSR),
    );
}

const CMD_PG_GET_STATE: u32 = 2; // MRQ_PG subcommand: read a power domain's current state
// t234 XUSB clock ids (dt-bindings/clock/tegra234-clock.h). FALCON = the Falcon core clock
// ("xusb_falcon_src", already enabled by JB1c); CORE_HOST = the host core clock ("xusb_host");
// FALCON_HOST/FALCON_SS = leaf clocks NOT in the DTB `clocks` list (re-asserted for completeness).
const CLK_XUSB_CORE_HOST: u32 = 267;
const CLK_XUSB_FALCON: u32 = 269;
const CLK_XUSB_FALCON_HOST: u32 = 270;
const CLK_XUSB_FALCON_SS: u32 = 271;

/// JB5 (attended bench): the read-only power-state guard for the raw-handoff Falcon witness.
/// MRQ_PG GET_STATE of every DTB power domain — a pure query, zero mutation — so the JB5 probe
/// touches XUSB MMIO only on a boot where the partition is provably ON (the JX1 gated-block rule).
/// Returns true iff every domain reads ON (0x1).
pub fn jb5_pg_on(chan: &Chan, ids: &super::fdt_tegra::XusbIds) -> bool {
    let mut all_on = true;
    for i in 0..ids.n_pds {
        let id = ids.pds[i];
        match chan.transfer(MRQ_PG, &[CMD_PG_GET_STATE, id]) {
            Some((err, out)) => {
                serial_println!(
                    ":: tegra: JB5 — PG {} GET_STATE (read-only) err={} = {:#x} ::",
                    id, err, out[0]
                );
                if err != 0 || out[0] != 1 {
                    all_on = false;
                }
            }
            None => {
                serial_println!(":: tegra: JB5 — PG {} GET_STATE -> TIMEOUT ::", id);
                all_on = false;
            }
        }
    }
    all_on
}

/// JB7 (arc A): read-only clock-state census for the halted-Falcon question. `jb1c_ungate_xusb`
/// only proves each MRQ_CLK ENABLE *acked* (err==0) — never that the clock is actually running.
/// This issues MRQ_CLK CMD_CLK_IS_ENABLED (a pure query, zero mutation) for every DTB clock the
/// XUSB host lists PLUS the four Falcon leaf clocks (267/269/270/271), so a gated core clock behind
/// an otherwise-powered partition shows as `=0`. If every clock reads `=1` while CPUCTL stays
/// 0xffffffff, clock-gating is RULED OUT — the Falcon core is held in reset, not unclocked. Runs at
/// raw handoff (before jb1c re-enables anything) so it reports the state UEFI actually left.
pub fn jb7_clocks_query(chan: &Chan, ids: &super::fdt_tegra::XusbIds) {
    let query = |id: u32, tag: &str| {
        match chan.transfer(MRQ_CLK, &[(CMD_CLK_IS_ENABLED << 24) | (id & 0x00ff_ffff)]) {
            Some((err, out)) => serial_println!(
                ":: tegra: JB7 — CLK {} IS_ENABLED ({}) err={} = {} ::",
                id, tag, err, out[0]
            ),
            None => serial_println!(":: tegra: JB7 — CLK {} IS_ENABLED ({}) -> TIMEOUT ::", id, tag),
        }
    };
    serial_println!(
        ":: tegra: JB7 — Falcon clock census (read-only MRQ_CLK IS_ENABLED): {} DTB + 4 leaf ::",
        ids.n_clocks
    );
    for i in 0..ids.n_clocks {
        query(ids.clocks[i], "dtb");
    }
    for id in [CLK_XUSB_CORE_HOST, CLK_XUSB_FALCON, CLK_XUSB_FALCON_HOST, CLK_XUSB_FALCON_SS] {
        query(id, "leaf");
    }
}

