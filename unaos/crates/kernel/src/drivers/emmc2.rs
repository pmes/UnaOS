// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BCM2711 EMMC2 / SDHCI microSD driver (M6g) — the block backend that lets the bare-metal Pi 4 load a
// program off the very card it booted from. Deliberately minimal, matching the arc scope: PIO only,
// single-block CMD17 reads, fully polled (IRPT_EN = 0), no DMA, and NO writes (the seam in
// `drivers::block` refuses a write on this backend). Every wait is bounded by a CNTPCT deadline — the
// mailbox driver's discipline (arch/aarch64/mailbox.rs) — so a missing/silent controller fails the probe
// and logs one clean line instead of hanging boot.
//
// Dual-base probe. QEMU's `raspi4b` attaches an `if=sd` card to the LEGACY Arasan SDHCI @0xFE30_0000;
// the EMMC2 controller @0xFE34_0000 has no card. On real Pi 4 silicon it is the reverse: the microSD
// slot routes to EMMC2 and the legacy Arasan carries the SDIO WiFi. So we probe **EMMC2 first, then fall
// back to the legacy base** — one driver, two candidate bases, and the metal path is the first tried.
// QEMU therefore always exercises the fallback (legacy) leg; the EMMC2 success leg runs on silicon only.
//
// Register map: the BCM2835-style names are 32-bit views of the standard SDHCI register block, which is
// exactly what QEMU's generic-sdhci and the real Arasan/EMMC2 both serve — so the same accessors drive
// both. Both bases sit in the peripheral GiB already mapped Device/XN by `boot::build_l1` (L1[3] @
// 0xC000_0000), so no MMU change is needed.

use spin::Mutex;

use crate::drivers::block::{BlockDeviceInfo, BlockError};

// --- Candidate controller bases (Pi 4 low-peripheral mode, peripheral base 0xFE00_0000). ---
const EMMC2_BASE: usize = 0xFE34_0000; // EMMC2 — the real microSD slot on metal; no card in QEMU
const LEGACY_BASE: usize = 0xFE30_0000; // legacy Arasan SDHCI — QEMU's `if=sd` card; SDIO WiFi on metal

// RPi firmware GET_CLOCK_RATE ids, used only if a base's own CAPABILITIES base-clock field reads zero.
const CLK_ID_EMMC2: u32 = 0xC; // EMMC2 clock (0xFE34 base)
const CLK_ID_EMMC: u32 = 0x1; // EMMC clock (0xFE30 legacy base)

// --- SDHCI register offsets (32-bit views). ---
const BLKSIZECNT: usize = 0x04; // [15:0] block size, [31:16] block count
const ARG1: usize = 0x08;
const CMDTM: usize = 0x0C; // command + transfer-mode; writing this ISSUES the command
const RESP0: usize = 0x10;
const RESP1: usize = 0x14;
const RESP2: usize = 0x18;
const RESP3: usize = 0x1C;
const DATA: usize = 0x20; // PIO FIFO, 32-bit LE
const STATUS: usize = 0x24;
const CONTROL0: usize = 0x28;
const CONTROL1: usize = 0x2C;
const INTERRUPT: usize = 0x30; // W1C
const IRPT_MASK: usize = 0x34; // status-ENABLE (bits latch into INTERRUPT only if set here)
const IRPT_EN: usize = 0x38; // signal-enable (kept 0 — polled)
const CAPABILITIES: usize = 0x40;

// --- STATUS (0x24) bits. ---
const ST_CMD_INHIBIT: u32 = 1 << 0;
const ST_DAT_INHIBIT: u32 = 1 << 1;

// --- CONTROL1 (0x2C) bits. ---
const C1_CLK_INTLEN: u32 = 1 << 0;
const C1_CLK_STABLE: u32 = 1 << 1;
const C1_CLK_EN: u32 = 1 << 2;
const C1_SRST_HC: u32 = 1 << 24;

// --- INTERRUPT (0x30) bits (W1C). ---
const INT_CMD_DONE: u32 = 1 << 0;
const INT_DATA_DONE: u32 = 1 << 1;
const INT_READ_RDY: u32 = 1 << 5;
const INT_ERR: u32 = 1 << 15; // summary of any error interrupt (bits 16+)
/// Any error: the error-summary bit OR any of the error-status bits [31:16].
const INT_ERR_ANY: u32 = INT_ERR | 0xFFFF_0000;

// --- CMDTM (0x0C) field builders. ---
const CMD_RESP_NONE: u32 = 0b00 << 16;
const CMD_RESP_136: u32 = 0b01 << 16;
const CMD_RESP_48: u32 = 0b10 << 16;
const CMD_RESP_48_BUSY: u32 = 0b11 << 16;
const CMD_CRCCHK: u32 = 1 << 19;
const CMD_IXCHK: u32 = 1 << 20;
const CMD_ISDATA: u32 = 1 << 21;
const CMD_DAT_DIR_READ: u32 = 1 << 4; // 1 = card -> host
#[inline]
const fn cmd(index: u32) -> u32 {
    index << 24
}

// --- Bounded-wait budgets (ms). Generous vs the microseconds QEMU/real cards take; the point is to fail
// a dead/absent controller cleanly rather than hang boot. ---
const CMD_TIMEOUT_MS: u64 = 100; // one command's completion
const ACMD41_TIMEOUT_MS: u64 = 1000; // the power-up (busy) polling loop
const RESET_TIMEOUT_MS: u64 = 100;
const CLK_STABLE_TIMEOUT_MS: u64 = 100;
const DATA_TIMEOUT_MS: u64 = 200;

/// The identified card: which base won the probe, its addressing mode, and its capacity. `read_block_512`
/// reads this. Behind a Mutex so a read serializes the (single) controller; on the Pi loader path only the
/// one loader task ever reads, but the lock keeps the driver correct if a second reader ever appears.
struct SdCard {
    base: usize,
    /// ccs (from ACMD41 bit30): true = SDHC/SDXC block addressing, false = SDSC byte addressing.
    block_addressing: bool,
    num_blocks: u64,
    csd_version: u8, // 1 or 2, for the identified line
}
static CARD: Mutex<Option<SdCard>> = Mutex::new(None);

#[inline]
fn read32(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}
#[inline]
fn write32(base: usize, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, val) }
}

/// A CNTPCT deadline `ms` milliseconds from now (the free-running counter is monotonic and won't wrap in
/// any boot window, so a plain `>=` compare is sound — same as `mbox_call`).
#[inline]
fn deadline_ms(ms: u64) -> u64 {
    crate::arch::timer::cntpct() + crate::arch::timer::cntfrq().saturating_mul(ms) / 1000
}
#[inline]
fn expired(deadline: u64) -> bool {
    crate::arch::timer::cntpct() >= deadline
}

/// Spin until every bit in `mask` at `off` reads 0, or the deadline expires. Returns whether it cleared.
fn wait_clear(base: usize, off: usize, mask: u32, ms: u64) -> bool {
    let dl = deadline_ms(ms);
    while read32(base, off) & mask != 0 {
        if expired(dl) {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}
/// Spin until any bit in `mask` at `off` reads 1, or the deadline expires. Returns whether it set.
fn wait_set(base: usize, off: usize, mask: u32, ms: u64) -> bool {
    let dl = deadline_ms(ms);
    while read32(base, off) & mask == 0 {
        if expired(dl) {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Issue one command (`cmdtm` built from `cmd()` + response/flag bits) with argument `arg`, and wait for
/// completion. For a data command (`CMD_ISDATA`) the caller sets `BLKSIZECNT` first. Returns Ok on
/// CMD_DONE with no error; Err on a command timeout / CRC / index error, or on our own bounded timeout —
/// the caller (an init step) treats any Err as "this base failed" and the probe falls back / gives up.
fn send_command(base: usize, cmdtm: u32, arg: u32) -> Result<(), ()> {
    // Wait for BOTH the command AND data lines to be free before issuing. Waiting on DAT_INHIBIT
    // unconditionally (not just before a data/busy command) is load-bearing on metal: a preceding R1b
    // command (CMD7 SELECT) leaves the card asserting BUSY on DAT0, which the controller reflects as
    // DAT_INHIBIT=1 until it clears. Issuing the next command (CMD16) while that busy is still asserted
    // is rejected by a real Arasan/EMMC2 (→ init fails → on metal, where EMMC2 is the only card, M6g
    // fails). QEMU deasserts busy instantly so this window is invisible there; the wait closes it.
    if !wait_clear(base, STATUS, ST_CMD_INHIBIT | ST_DAT_INHIBIT, CMD_TIMEOUT_MS) {
        return Err(());
    }
    write32(base, INTERRUPT, 0xFFFF_FFFF); // clear any stale status
    write32(base, ARG1, arg);
    write32(base, CMDTM, cmdtm); // issues the command

    // Wait for command-complete OR any error. A card that isn't there never sets CMD_DONE — QEMU flags
    // CMD_TIMEOUT (an error bit) for a response-expecting command with no card, and may also set CMD_DONE,
    // so we test the error bits regardless of CMD_DONE.
    if !wait_set(base, INTERRUPT, INT_CMD_DONE | INT_ERR_ANY, CMD_TIMEOUT_MS) {
        return Err(());
    }
    let int = read32(base, INTERRUPT);
    if int & INT_ERR_ANY != 0 {
        write32(base, INTERRUPT, int); // W1C what we saw
        return Err(());
    }
    write32(base, INTERRUPT, INT_CMD_DONE);
    Ok(())
}

/// Read the four response registers (for R2 / 136-bit responses; also used to read R1/R3/R6/R7's RESP0).
#[inline]
fn read_resp(base: usize) -> [u32; 4] {
    [read32(base, RESP0), read32(base, RESP1), read32(base, RESP2), read32(base, RESP3)]
}

/// Extract CSD bit range `[hi:lo]` from a 136-bit R2 response. SDHCI strips the CRC byte and shifts the
/// 120-bit content right 8, so CSD bit `b` (b >= 8) lands at overall response bit `b-8` (the classic
/// off-by-8). `resp[i]` holds response bits `[32i+31 : 32i]`.
fn csd_bits(resp: &[u32; 4], hi: u32, lo: u32) -> u64 {
    let mut val = 0u64;
    let mut b = hi;
    loop {
        let r = b - 8; // CSD bit b -> response bit r
        let bit = (resp[(r / 32) as usize] >> (r % 32)) & 1;
        val = (val << 1) | bit as u64;
        if b == lo {
            break;
        }
        b -= 1;
    }
    val
}

/// Resolve a candidate base's SD base clock in Hz: CAPABILITIES[15:8] MHz if nonzero, else the VideoCore
/// mailbox GET_CLOCK_RATE for this base's clock id, else assume 100 MHz (logged). QEMU reports 52 MHz in
/// CAP so the mailbox/assume legs never run there.
fn base_clock(base: usize, clock_id: u32) -> u32 {
    let cap_mhz = (read32(base, CAPABILITIES) >> 8) & 0xFF;
    if cap_mhz != 0 {
        return cap_mhz * 1_000_000;
    }
    if let Some(hz) = crate::arch::mailbox::get_clock_rate(clock_id) {
        return hz;
    }
    serial_println!(":: M6g: base clock unknown (CAP=0, mailbox failed) — assuming 100 MHz ::");
    100_000_000
}

/// Program the SD clock to (at most) `target_hz` using the SDHCI-3 10-bit divided-clock mode
/// (SDCLK = base/(2·DIV)). Disables the SD clock, sets the divider + data-timeout + internal clock, waits
/// for the internal clock to stabilise, then re-enables the SD clock. Returns whether the clock stabilised.
fn set_clock(base: usize, base_hz: u32, target_hz: u32) -> bool {
    // Disable the SD clock before changing the divider.
    let c1 = read32(base, CONTROL1) & !C1_CLK_EN;
    write32(base, CONTROL1, c1);

    // DIV = ceil(base / (2*target)), clamped into the 10-bit field (>=1 so we never select bypass).
    let denom = target_hz.saturating_mul(2).max(1);
    let div = base_hz.div_ceil(denom).clamp(1, 0x3FF);
    // Freq select: low 8 bits -> [15:8], high 2 bits -> [7:6].
    let freq = ((div & 0xFF) << 8) | (((div >> 8) & 0x3) << 6);

    let mut c1 = read32(base, CONTROL1);
    c1 &= !0x0000_FFC0; // clear the old freq-select field ([15:8] + [7:6])
    c1 &= !(0xF << 16); // clear DATA_TOUNIT
    c1 |= freq;
    c1 |= 0xE << 16; // DATA_TOUNIT = max
    c1 |= C1_CLK_INTLEN;
    write32(base, CONTROL1, c1);

    if !wait_set(base, CONTROL1, C1_CLK_STABLE, CLK_STABLE_TIMEOUT_MS) {
        return false;
    }
    let c1 = read32(base, CONTROL1) | C1_CLK_EN;
    write32(base, CONTROL1, c1);
    true
}

/// Run the full SDHCI init ladder against `base` at 400 kHz identification clock, and if a card answers,
/// select it, learn its capacity from the CSD, raise the clock to transfer speed, and return the card.
/// Silent on failure (returns None) — the caller emits the single fallback / no-card line, so a failing
/// base never adds an unaccounted serial line. `clock_id` is the mailbox fallback for the base clock.
fn try_init(base: usize, clock_id: u32) -> Option<SdCard> {
    // 1. Full-controller software reset; wait for it to self-clear.
    write32(base, CONTROL1, read32(base, CONTROL1) | C1_SRST_HC);
    if !wait_clear(base, CONTROL1, C1_SRST_HC, RESET_TIMEOUT_MS) {
        return None;
    }
    // 2. Enable status latching (must be set or STATUS bits never appear in INTERRUPT); keep signals off
    //    (polled); clear any stale status.
    write32(base, IRPT_MASK, 0xFFFF_FFFF);
    write32(base, IRPT_EN, 0);
    write32(base, INTERRUPT, 0xFFFF_FFFF);
    // 3. Bus power: 3.3 V select + bus power on (CONTROL0 bits[11:8] = 0xF).
    write32(base, CONTROL0, (read32(base, CONTROL0) & !(0xF << 8)) | (0xF << 8));

    // 4. 400 kHz identification clock (data-timeout unit folded in by set_clock).
    let base_hz = base_clock(base, clock_id);
    if !set_clock(base, base_hz, 400_000) {
        return None;
    }

    // 5. CMD0 GO_IDLE (no response).
    send_command(base, cmd(0) | CMD_RESP_NONE, 0).ok()?;
    // 6. CMD8 SEND_IF_COND (R7, CRC+index check): 0x1AA = 2.7-3.6 V + check pattern 0xAA. The echo is the
    //    discriminator — a base with no card cannot echo 0x1AA (it times out or returns garbage).
    send_command(base, cmd(8) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0x1AA).ok()?;
    if read32(base, RESP0) & 0xFFF != 0x1AA {
        return None;
    }
    // 7. ACMD41 loop (bounded ~1 s): CMD55 (APP_CMD, R1) then ACMD41 (SD_SEND_OP_COND, R3 — CRC AND index
    //    check DISABLED; the OCR has no valid CRC/index) with HCS + the 3.3 V window, until the card's
    //    power-up-busy bit (RESP0[31]) clears. ccs = RESP0[30] -> block vs byte addressing.
    let acmd41_deadline = deadline_ms(ACMD41_TIMEOUT_MS);
    let mut ocr;
    loop {
        send_command(base, cmd(55) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0).ok()?;
        send_command(base, cmd(41) | CMD_RESP_48, 0x40FF_8000).ok()?;
        ocr = read32(base, RESP0);
        if ocr & (1 << 31) != 0 {
            break; // powered up
        }
        if expired(acmd41_deadline) {
            return None;
        }
        core::hint::spin_loop();
    }
    let block_addressing = ocr & (1 << 30) != 0; // ccs

    // 8. CMD2 ALL_SEND_CID (R2, CRC on, index off) — moves the card to identification state.
    send_command(base, cmd(2) | CMD_RESP_136 | CMD_CRCCHK, 0).ok()?;
    // 9. CMD3 SEND_RELATIVE_ADDR (R6) -> rca in RESP0[31:16].
    send_command(base, cmd(3) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0).ok()?;
    let rca = read32(base, RESP0) >> 16;
    let rca_arg = rca << 16;

    // 10. CMD9 SEND_CSD (R2) — the card must be in stand-by (post-CMD3, pre-CMD7). Parse capacity.
    send_command(base, cmd(9) | CMD_RESP_136 | CMD_CRCCHK, rca_arg).ok()?;
    let resp = read_resp(base);
    let csd_structure = csd_bits(&resp, 127, 126);
    let (num_blocks, csd_version) = if csd_structure == 1 {
        // CSD v2 (SDHC/SDXC): C_SIZE = CSD[69:48]; capacity = (C_SIZE+1) * 512 KiB -> blocks = (C_SIZE+1)*1024.
        let c_size = csd_bits(&resp, 69, 48);
        ((c_size + 1) * 1024, 2u8)
    } else {
        // CSD v1 (SDSC): standard formula. blocks(512) = (C_SIZE+1) * 2^(C_SIZE_MULT+2) * 2^READ_BL_LEN / 512.
        let read_bl_len = csd_bits(&resp, 83, 80) as u32;
        let c_size = csd_bits(&resp, 73, 62);
        let c_size_mult = csd_bits(&resp, 49, 47) as u32;
        let mult = 1u64 << (c_size_mult + 2);
        let block_len = 1u64 << read_bl_len;
        let capacity = (c_size + 1) * mult * block_len;
        (capacity / 512, 1u8)
    };
    if num_blocks == 0 {
        return None;
    }

    // 11. CMD7 SELECT_CARD (R1b) -> transfer state.
    send_command(base, cmd(7) | CMD_RESP_48_BUSY | CMD_CRCCHK | CMD_IXCHK, rca_arg).ok()?;
    // 12. CMD16 SET_BLOCKLEN 512 (R1; SDSC semantics, harmless on SDHC where 512 is fixed).
    send_command(base, cmd(16) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 512).ok()?;
    // 13. Raise to transfer clock (<= 25 MHz).
    if !set_clock(base, base_hz, 25_000_000) {
        return None;
    }

    Some(SdCard { base, block_addressing, num_blocks, csd_version })
}

/// Probe the microSD: EMMC2 first (the metal path), then the legacy base (QEMU's card). On success,
/// register the block backend and log the identified line. Emits the fallback line at the EMMC2->legacy
/// transition (absent on metal, where EMMC2 wins) and one clean failure line if neither base answers.
/// Called once, synchronously on the BSP, after `start_aps` — single-threaded mailbox use, deterministic
/// serial placement (its lines land early, before the M6b demo). Never hangs boot: every wait is bounded.
pub fn probe() {
    if let Some(card) = try_init(EMMC2_BASE, CLK_ID_EMMC2) {
        finish(card);
        return;
    }
    // EMMC2 has no card in QEMU; announce the fallback (on metal EMMC2 succeeds and this line is absent).
    serial_println!(
        ":: M6g: EMMC2 @{:#x}: no card — falling back to SDHCI @{:#x} ::",
        EMMC2_BASE,
        LEGACY_BASE
    );
    if let Some(card) = try_init(LEGACY_BASE, CLK_ID_EMMC) {
        finish(card);
        return;
    }
    serial_println!(":: M6g: no SD card on either base — no block device registered ::");
}

/// Announce an identified card and register it as the block backend (read-only). The vendor/product tags
/// are cosmetic (shown only by the shell) — the loader path uses only the geometry.
fn finish(card: SdCard) {
    let mib = card.num_blocks * 512 / (1024 * 1024);
    serial_println!(
        ":: M6g: SD card @0x{:x} identified — {} blocks ({} MiB, CSD v{}) ::",
        card.base,
        card.num_blocks,
        mib,
        card.csd_version
    );
    let info = BlockDeviceInfo {
        slot_id: 0,
        block_size: 512,
        num_blocks: card.num_blocks,
        vendor: *b"BCM-SD  ",
        product: *b"microSD Card    ",
    };
    *CARD.lock() = Some(card);
    crate::drivers::block::register_sd(info);
}

/// Read one 512-byte block at `lba` into `buf` (>= 512 bytes) via a polled single-block CMD17. Returns
/// the number of bytes copied (512) on success. Backs `drivers::block::read_block` on the SD backend.
/// No cache maintenance: PIO into a normal kernel buffer, no DMA.
pub fn read_block_512(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let guard = CARD.lock();
    let card = guard.as_ref().ok_or(BlockError::NotReady)?;
    if lba >= card.num_blocks {
        return Err(BlockError::BadLba);
    }
    // SDSC uses byte addressing; guard that the byte offset fits the 32-bit ARG1 register.
    let arg = if card.block_addressing {
        lba as u32
    } else {
        let byte_off = lba.checked_mul(512).filter(|&b| b <= u32::MAX as u64).ok_or(BlockError::BadLba)?;
        byte_off as u32
    };
    let base = card.base;

    write32(base, INTERRUPT, 0xFFFF_FFFF); // clear stale status
    write32(base, BLKSIZECNT, (1 << 16) | 512); // one block, 512 bytes

    // CMD17 READ_SINGLE_BLOCK: R1 (CRC+index check) + data present + card->host direction.
    send_command(
        base,
        cmd(17) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK | CMD_ISDATA | CMD_DAT_DIR_READ,
        arg,
    )
    .map_err(|_| BlockError::Io)?;

    // PIO in: wait for the block to be buffered, read 128 little-endian words, then wait transfer-complete.
    if !wait_set(base, INTERRUPT, INT_READ_RDY, DATA_TIMEOUT_MS) {
        return Err(BlockError::Io);
    }
    write32(base, INTERRUPT, INT_READ_RDY); // W1C
    let n = buf.len().min(512);
    for i in 0..128usize {
        let word = read32(base, DATA);
        let bytes = word.to_le_bytes();
        let off = i * 4;
        for (k, &b) in bytes.iter().enumerate() {
            if off + k < n {
                buf[off + k] = b;
            }
        }
    }
    if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
        return Err(BlockError::Io);
    }
    let int = read32(base, INTERRUPT);
    write32(base, INTERRUPT, int); // W1C everything we saw
    if int & INT_ERR_ANY != 0 {
        return Err(BlockError::Io);
    }
    Ok(n)
}
