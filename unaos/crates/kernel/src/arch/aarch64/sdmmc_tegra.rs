// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-SDMMC-1 — Tegra234 microSD-slot SDMMC controller bring-up, READ-ONLY recon (`sdmmc` gated).
// The installer line's FIRST rung (docs/../unaos-installer.md): the Orin devkit is the "mule" whose
// microSD slot we ultimately want to write from a booted UnaOS. Before any write path (a later arc,
// SDMMC-2, behind its own arm flag) we census the card the NET-1 house pattern demands: resolve the
// controller from the live DTB, bring the SDHCI engine up to card-identification, read CID/CSD/capacity,
// and read sector 0 to classify its partition signature. NOTHING here writes to the card.
//
// ## Ground truth / the model this mirrors
//
// The Tegra234 SDMMC controllers are SDHCI-compatible (compatible `nvidia,tegra234-sdhci`) with NVIDIA
// vendor registers stacked above the standard 0x00..0xFF SDHCI block. The Orin Nano devkit routes the
// user-facing microSD slot to `sdmmc1` (`mmc@3400000` on the L4T tree); the on-module eMMC (when fitted)
// is `sdmmc4`, marked `non-removable`. We do NOT hardcode the base — the DTB decides (compatible / node
// name match), and the removable (non-`non-removable`) enabled instance is the microSD slot. Every
// candidate node found is logged.
//
// The in-tree Pi 4 `drivers::emmc2` SDHCI driver is the proven register/bit model this mirrors — the
// BCM2711 "32-bit view" register names ARE the standard SDHCI block (BLKSIZECNT 0x04, ARG1 0x08, CMDTM
// 0x0C, RESP 0x10.., DATA 0x20, PRESENT-STATE 0x24, CONTROL0 0x28, CONTROL1 0x2C, INTERRUPT 0x30, CAPS
// 0x40), which Tegra234 serves identically. Mirrored, NOT copied: the Pi driver dual-probes two fixed
// bases; here the base is DTB-resolved, and the Tegra vendor-quirk assumptions are documented below.
//
// ## Code-complete-prior-to-metal (by design)
//
// QEMU models no Tegra234 SDMMC controller, so the whole driver is `tegra`-gated at the MMIO layer (the
// net4 pattern): a `sdmmc`-standalone (virt) build does zero MMIO and prints one honest compiled-present
// witness line (`sdmmc_census` below); only `UNAOS_SDMMC=1 UNAOS_TEGRA=1` on real Orin silicon touches
// the controller. Correctness comes from `arroyo check`, the QEMU regression non-regression (the tegra
// code is compiled out on virt), and faithful adherence to the SD Physical Layer / SDHCI spec.
//
// ## The Tegra vendor-quirk assumptions this READ-ONLY recon relies on (documented; metal-pending)
//
//  1. The firmware/BPMP has already ENABLED the sdmmc1 module clock + pad power and left the slot's
//     rails up (the bootloader read the card to boot). We do NOT program the CAR/BPMP clock or the
//     Tegra vendor pad-control registers (>= 0x100) — we drive ONLY the standard SDHCI internal-clock
//     divider (CONTROL1) off whatever base clock the controller already has running. If metal shows the
//     internal clock never stabilises (CLK_STABLE never sets), the diagnosis is "the input clock is
//     gated" and the fix (a BPMP clock MRQ) is a later arc — surfaced, never worked around.
//  2. The CAPABILITIES base-clock field: if it reads 0 (some Tegra SKUs report base clock via the DT
//     `clock-frequency` / assigned-clock-rates instead of CAPS[15:8]), we assume a documented 200 MHz
//     and log it. Identification runs at 400 kHz then 25 MHz default-speed, so an inexact base only
//     changes the divider, never correctness.
//  3. Bus width / speed: identification runs 1-bit at default speed (<= 25 MHz). 4-bit / high-speed
//     negotiation (ACMD6 / CMD6) is deferred — it is not needed to census the card and keeps this rung
//     minimal. The reported width/speed is "1-bit, default-speed (25 MHz)".
//
// ## Read-only-by-construction
//
// This module issues ONLY the identification ladder + CMD17 single-block READ: CMD0, CMD8, CMD55/ACMD41,
// CMD2, CMD3, CMD9, CMD7, CMD16, CMD17. There is NO CMD24/WRITE_SINGLE_BLOCK, no CMD25, no ACMD6
// bus-width write, no erase, no CMD6 switch. The controller-register writes it does make (SRST, clock,
// power, command issue) are the SDHCI machinery every read needs; NONE of them are a WRITE to the card's
// storage. `grep` the diff for `cmd(24)` / `write_block` / `WRITE` and find nothing that targets card
// storage — read-only by construction (see review/unaos-orin-sdmmc1-LANDING.md).

#![cfg(feature = "sdmmc")]

/// Stable serial prefix so the operator (and the bench runbook) can grep the whole SDMMC recon as one
/// block, exactly as NET-4 uses `:: PCIE4:`.
const PS: &str = ":: SDMMC:";

// ── The witness half (virt / non-tegra build): one honest line, zero MMIO ──────────────────────────

/// The QEMU-safe witness: on a `sdmmc`-but-not-`tegra` build (QEMU models no Tegra234 SDMMC controller),
/// there is no controller to census, so print one line recording that the driver is compiled-present but
/// its recon is metal-only, and return before any MMIO. This keeps the GICv3 virt regression runs
/// unperturbed. Mirrors `net4_bringup`'s `not(tegra)` witness.
#[cfg(not(feature = "tegra"))]
pub fn sdmmc_census(_dtb_addr: u64, _dtb_size: usize, _ram_gib_mask: u64) {
    serial_println!(
        "{} ORIN-SDMMC-1 Tegra234 microSD recon compiled; no Tegra234 SDMMC on this build (QEMU virt) — recon is metal-only (UNAOS_SDMMC=1 UNAOS_TEGRA=1) ::",
        PS
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// The metal recon (`sdmmc` + `tegra`) — DTB census (M1), SDHCI identification (M2), sector-0 read (M3).
// ══════════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "tegra")]
pub use metal::sdmmc_census;

#[cfg(feature = "tegra")]
mod metal {
    use super::PS;
    use crate::arch::aarch64::fdt_tegra::{Fdt, PropWords};

    // ── SDHCI register offsets (32-bit views — identical to the Pi `emmc2` model, standard SDHCI) ──
    const BLKSIZECNT: u64 = 0x04; // [15:0] block size, [31:16] block count
    const ARG1: u64 = 0x08;
    const CMDTM: u64 = 0x0c; // transfer-mode + command; writing this ISSUES the command
    const RESP0: u64 = 0x10;
    const RESP1: u64 = 0x14;
    const RESP2: u64 = 0x18;
    const RESP3: u64 = 0x1c;
    const DATA: u64 = 0x20; // PIO FIFO, 32-bit LE
    const STATUS: u64 = 0x24; // Present State
    const CONTROL0: u64 = 0x28; // Host Control 1 + Power + BlockGap + Wakeup
    const CONTROL1: u64 = 0x2c; // Clock Control + Timeout + Software Reset
    const INTERRUPT: u64 = 0x30; // Normal + Error Interrupt Status (W1C)
    const IRPT_MASK: u64 = 0x34; // status-ENABLE (bits latch into INTERRUPT only if set here)
    const IRPT_EN: u64 = 0x38; // signal-enable (kept 0 — polled)
    const CAPABILITIES: u64 = 0x40;
    const HOST_VERSION: u64 = 0xfc; // [31:16] = Host Controller Version register (0xFE)

    // ── Present State (0x24) bits. ──
    const ST_CMD_INHIBIT: u32 = 1 << 0;
    const ST_DAT_INHIBIT: u32 = 1 << 1;
    const ST_CARD_INSERTED: u32 = 1 << 16;

    // ── CONTROL1 (0x2C) bits. ──
    const C1_CLK_INTLEN: u32 = 1 << 0;
    const C1_CLK_STABLE: u32 = 1 << 1;
    const C1_CLK_EN: u32 = 1 << 2;
    const C1_SRST_HC: u32 = 1 << 24; // Software Reset For All (reg 0x2F bit 0)

    // ── INTERRUPT (0x30) bits (W1C). ──
    const INT_CMD_DONE: u32 = 1 << 0;
    const INT_DATA_DONE: u32 = 1 << 1;
    const INT_READ_RDY: u32 = 1 << 5;
    const INT_ERR: u32 = 1 << 15;
    /// Any error: the error-summary bit OR any of the error-status bits [31:16].
    const INT_ERR_ANY: u32 = INT_ERR | 0xffff_0000;

    // ── R1 card-status error bits (SD Physical Layer §4.10.1); same mask the Pi emmc2 driver uses. ──
    const R1_ERROR_MASK: u32 = 0xfff9_8008;

    // ── CMDTM (0x0C) field builders. ──
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

    // ── Bounded-wait budgets (ms). Generous vs the microseconds real cards take; the point is to fail a
    //    dead/absent controller cleanly rather than hang boot. ──
    const CMD_TIMEOUT_MS: u64 = 100;
    const ACMD41_TIMEOUT_MS: u64 = 1000;
    const RESET_TIMEOUT_MS: u64 = 100;
    const CLK_STABLE_TIMEOUT_MS: u64 = 100;
    const DATA_TIMEOUT_MS: u64 = 200;

    /// Base-clock fallback (Hz) when CAPABILITIES[15:8] reads 0 (see the vendor-quirk note in the file
    /// header): a conservative Tegra234 sdmmc1 assumption, logged when used.
    const ASSUMED_BASE_HZ: u32 = 200_000_000;

    /// Poison patterns that mean ABSENT DECODE, never "present" (the NET-4b law + the PI-V3D-1 false-PASS
    /// lesson): `0xffffffff` = master-abort / unclaimed / open bus; `0xdeadbeef` = firmware register/DRAM
    /// fill; `0xa5a5a5a5` = the Tegra carveout poison fill. A live SDHCI CAPABILITIES read is none of these.
    #[inline]
    fn is_poison(v: u32) -> bool {
        v == 0xffff_ffff || v == 0xdead_beef || v == 0xa5a5_a5a5
    }

    #[inline]
    fn read32(base: u64, off: u64) -> u32 {
        unsafe { core::ptr::read_volatile((base + off) as *const u32) }
    }
    #[inline]
    fn write32(base: u64, off: u64, val: u32) {
        unsafe { core::ptr::write_volatile((base + off) as *mut u32, val) }
    }

    /// A CNTPCT deadline `ms` milliseconds from now (the free-running counter is monotonic and won't wrap
    /// in any boot window, so a plain `>=` compare is sound — the mailbox/emmc2 discipline).
    #[inline]
    fn deadline_ms(ms: u64) -> u64 {
        crate::arch::timer::cntpct() + crate::arch::timer::cntfrq().saturating_mul(ms) / 1000
    }
    #[inline]
    fn expired(deadline: u64) -> bool {
        crate::arch::timer::cntpct() >= deadline
    }
    /// Spin until every bit in `mask` at `off` reads 0, or the deadline expires.
    fn wait_clear(base: u64, off: u64, mask: u32, ms: u64) -> bool {
        let dl = deadline_ms(ms);
        while read32(base, off) & mask != 0 {
            if expired(dl) {
                return false;
            }
            core::hint::spin_loop();
        }
        true
    }
    /// Spin until any bit in `mask` at `off` reads 1, or the deadline expires.
    fn wait_set(base: u64, off: u64, mask: u32, ms: u64) -> bool {
        let dl = deadline_ms(ms);
        while read32(base, off) & mask == 0 {
            if expired(dl) {
                return false;
            }
            core::hint::spin_loop();
        }
        true
    }

    /// Issue one command and wait for completion. Returns Ok on CMD_DONE with no error; Err on a command
    /// timeout / CRC / index error, or our own bounded timeout. Mirrors `emmc2::send_command` (incl. the
    /// unconditional DAT_INHIBIT wait, load-bearing on metal after an R1b command leaves DAT0 busy).
    fn send_command(base: u64, cmdtm: u32, arg: u32) -> Result<(), ()> {
        if !wait_clear(base, STATUS, ST_CMD_INHIBIT | ST_DAT_INHIBIT, CMD_TIMEOUT_MS) {
            return Err(());
        }
        write32(base, INTERRUPT, 0xffff_ffff); // clear any stale status
        write32(base, ARG1, arg);
        write32(base, CMDTM, cmdtm); // issues the command
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

    /// Read the four response registers (R2 / 136-bit; also holds R1/R3/R6/R7's RESP0).
    #[inline]
    fn read_resp(base: u64) -> [u32; 4] {
        [read32(base, RESP0), read32(base, RESP1), read32(base, RESP2), read32(base, RESP3)]
    }

    /// Extract bit range `[hi:lo]` from a 136-bit R2 response (CID or CSD). SDHCI strips the CRC byte and
    /// shifts the 120-bit content right 8, so register bit `b` (b >= 8) lands at overall response bit
    /// `b-8` (the classic off-by-8 — identical for CID and CSD). `resp[i]` holds response bits
    /// `[32i+31 : 32i]`.
    fn r2_bits(resp: &[u32; 4], hi: u32, lo: u32) -> u64 {
        let mut val = 0u64;
        let mut b = hi;
        loop {
            let r = b - 8;
            let bit = (resp[(r / 32) as usize] >> (r % 32)) & 1;
            val = (val << 1) | bit as u64;
            if b == lo {
                break;
            }
            b -= 1;
        }
        val
    }

    /// Resolve the SD base clock in Hz: CAPABILITIES[15:8] MHz if nonzero, else the documented assumed
    /// base (logged). There is no VideoCore mailbox on Tegra (the Pi's middle leg) — the assumption
    /// stands in for it, per the file-header vendor-quirk note.
    fn base_clock(base: u64) -> u32 {
        let cap_mhz = (read32(base, CAPABILITIES) >> 8) & 0xff;
        if cap_mhz != 0 {
            return cap_mhz * 1_000_000;
        }
        serial_println!(
            "{}   CAPABILITIES base-clock field = 0 — assuming {} MHz (Tegra reports it via DT clocks; see quirk note) ::",
            PS,
            ASSUMED_BASE_HZ / 1_000_000
        );
        ASSUMED_BASE_HZ
    }

    /// Program the SD clock to (at most) `target_hz` using the SDHCI-3 10-bit divided-clock mode
    /// (SDCLK = base/(2·DIV)). Identical to `emmc2::set_clock`. Returns whether the internal clock
    /// stabilised (false ⇒ the input clock is gated — the metal-pending BPMP-clock diagnosis).
    fn set_clock(base: u64, base_hz: u32, target_hz: u32) -> bool {
        let c1 = read32(base, CONTROL1) & !C1_CLK_EN;
        write32(base, CONTROL1, c1);

        let denom = target_hz.saturating_mul(2).max(1);
        let div = base_hz.div_ceil(denom).clamp(1, 0x3ff);
        let freq = ((div & 0xff) << 8) | (((div >> 8) & 0x3) << 6);

        let mut c1 = read32(base, CONTROL1);
        c1 &= !0x0000_ffc0; // clear old freq-select field ([15:8] + [7:6])
        c1 &= !(0xf << 16); // clear DATA_TOUNIT
        c1 |= freq;
        c1 |= 0xe << 16; // DATA_TOUNIT = max
        c1 |= C1_CLK_INTLEN;
        write32(base, CONTROL1, c1);

        if !wait_set(base, CONTROL1, C1_CLK_STABLE, CLK_STABLE_TIMEOUT_MS) {
            return false;
        }
        let c1 = read32(base, CONTROL1) | C1_CLK_EN;
        write32(base, CONTROL1, c1);
        true
    }

    /// The identified card, learned by the M2 ladder — enough to census + read sector 0.
    struct Card {
        block_addressing: bool, // ccs: true = SDHC/SDXC block addressing, false = SDSC byte
        num_blocks: u64,
        csd_version: u8,
    }

    /// M2: run the SDHCI identification ladder (READ-ONLY to the card) against `base`, learning the CID,
    /// CSD-derived capacity, and RCA. Prints the CID (manufacturer/OEM/product/serial) and capacity.
    /// Returns the identified card, or None on any absent-card / timeout / decode failure (the caller
    /// prints the honest cause). NO card write anywhere in this ladder.
    fn identify(base: u64) -> Option<Card> {
        // 1. Full-controller software reset; wait for it to self-clear.
        serial_println!("{}   M2: SRST_ALL (controller software reset) ::", PS);
        write32(base, CONTROL1, read32(base, CONTROL1) | C1_SRST_HC);
        if !wait_clear(base, CONTROL1, C1_SRST_HC, RESET_TIMEOUT_MS) {
            serial_println!("{}   M2: SRST did not self-clear (controller not responding) — STOP ::", PS);
            return None;
        }
        // 2. Enable status latching (must be set or STATUS bits never appear in INTERRUPT); keep signals
        //    off (polled); clear any stale status.
        write32(base, IRPT_MASK, 0xffff_ffff);
        write32(base, IRPT_EN, 0);
        write32(base, INTERRUPT, 0xffff_ffff);
        // 3. Bus power: 3.3 V select + bus power on (CONTROL0 bits[11:8] = 0xF).
        write32(base, CONTROL0, (read32(base, CONTROL0) & !(0xf << 8)) | (0xf << 8));

        // 4. Card-detect: with power + reset settled, is a card seated? (Present State bit 16.)
        let present = read32(base, STATUS);
        if present & ST_CARD_INSERTED == 0 {
            serial_println!(
                "{}   M2: no card seated (Present State {:#010x}, Card-Inserted clear) — census done, nothing to identify ::",
                PS, present
            );
            return None;
        }
        serial_println!("{}   M2: card detected (Present State {:#010x}) ::", PS, present);

        // 5. 400 kHz identification clock.
        let base_hz = base_clock(base);
        if !set_clock(base, base_hz, 400_000) {
            serial_println!(
                "{}   M2: internal clock never stabilised at 400 kHz — the input clock is gated (BPMP-clock diagnosis; see quirk note) — STOP ::",
                PS
            );
            return None;
        }

        // 6. CMD0 GO_IDLE (no response).
        send_command(base, cmd(0) | CMD_RESP_NONE, 0).ok()?;
        // 7. CMD8 SEND_IF_COND (R7): 0x1AA = 2.7-3.6 V + check pattern 0xAA. The echo is the discriminator.
        send_command(base, cmd(8) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0x1aa).ok()?;
        let sdhc_capable = read32(base, RESP0) & 0xfff == 0x1aa;
        if !sdhc_capable {
            serial_println!("{}   M2: CMD8 echo mismatch — legacy/SDSC card (or no v2 support) ::", PS);
        }
        // 8. ACMD41 loop (bounded ~1 s): CMD55 (APP_CMD) then ACMD41 (SD_SEND_OP_COND) with HCS + the
        //    3.3 V window, until power-up-busy (RESP0[31]) clears. ccs = RESP0[30].
        let acmd41_deadline = deadline_ms(ACMD41_TIMEOUT_MS);
        let mut ocr;
        loop {
            send_command(base, cmd(55) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0).ok()?;
            send_command(base, cmd(41) | CMD_RESP_48, 0x40ff_8000).ok()?;
            ocr = read32(base, RESP0);
            if ocr & (1 << 31) != 0 {
                break;
            }
            if expired(acmd41_deadline) {
                serial_println!("{}   M2: ACMD41 power-up timed out (card never left busy) — STOP ::", PS);
                return None;
            }
            core::hint::spin_loop();
        }
        let block_addressing = ocr & (1 << 30) != 0; // ccs

        // 9. CMD2 ALL_SEND_CID (R2) -> identification state. Decode + print the CID.
        send_command(base, cmd(2) | CMD_RESP_136 | CMD_CRCCHK, 0).ok()?;
        let cid = read_resp(base);
        print_cid(&cid);

        // 10. CMD3 SEND_RELATIVE_ADDR (R6) -> rca in RESP0[31:16].
        send_command(base, cmd(3) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0).ok()?;
        let rca = read32(base, RESP0) >> 16;
        let rca_arg = rca << 16;

        // 11. CMD9 SEND_CSD (R2) — card must be in stand-by (post-CMD3, pre-CMD7). Parse capacity.
        send_command(base, cmd(9) | CMD_RESP_136 | CMD_CRCCHK, rca_arg).ok()?;
        let csd = read_resp(base);
        let csd_structure = r2_bits(&csd, 127, 126);
        let (num_blocks, csd_version) = if csd_structure == 1 {
            // CSD v2 (SDHC/SDXC): C_SIZE = CSD[69:48]; blocks = (C_SIZE+1)*1024.
            let c_size = r2_bits(&csd, 69, 48);
            ((c_size + 1) * 1024, 2u8)
        } else {
            // CSD v1 (SDSC): blocks(512) = (C_SIZE+1) * 2^(C_SIZE_MULT+2) * 2^READ_BL_LEN / 512.
            let read_bl_len = r2_bits(&csd, 83, 80) as u32;
            let c_size = r2_bits(&csd, 73, 62);
            let c_size_mult = r2_bits(&csd, 49, 47) as u32;
            let mult = 1u64 << (c_size_mult + 2);
            let block_len = 1u64 << read_bl_len;
            ((c_size + 1) * mult * block_len / 512, 1u8)
        };
        if num_blocks == 0 {
            serial_println!("{}   M2: CSD decoded 0 capacity — STOP (won't census a zero-size card) ::", PS);
            return None;
        }
        let mib = num_blocks * 512 / (1024 * 1024);
        serial_println!(
            "{}   M2: capacity {} blocks ({} MiB, CSD v{}), addressing {}, {} ::",
            PS, num_blocks, mib, csd_version,
            if block_addressing { "block (SDHC/SDXC)" } else { "byte (SDSC)" },
            if sdhc_capable { "v2 (CMD8 ok)" } else { "legacy" }
        );

        // 12. CMD7 SELECT_CARD (R1b) -> transfer state.
        send_command(base, cmd(7) | CMD_RESP_48_BUSY | CMD_CRCCHK | CMD_IXCHK, rca_arg).ok()?;
        // 13. CMD16 SET_BLOCKLEN 512 (R1; SDSC semantics, harmless on SDHC where 512 is fixed).
        send_command(base, cmd(16) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 512).ok()?;
        // 14. Raise to default transfer clock (<= 25 MHz). 1-bit bus (4-bit/HS deferred — quirk note).
        if !set_clock(base, base_hz, 25_000_000) {
            serial_println!("{}   M2: could not raise to 25 MHz transfer clock — STOP ::", PS);
            return None;
        }
        serial_println!(
            "{}   M2: identified — RCA {:#06x}, bus 1-bit, default-speed (<=25 MHz) [4-bit/HS negotiation deferred] ::",
            PS, rca
        );

        Some(Card { block_addressing, num_blocks, csd_version })
    }

    /// Decode + print the CID (SD Physical Layer §5.1): MID (manufacturer), OID (2 ASCII), PNM (5 ASCII
    /// product name), PRV (revision), PSN (32-bit serial), MDT (manufacture date). Read from the 136-bit
    /// CMD2 R2 response with the SDHCI off-by-8 shift.
    fn print_cid(cid: &[u32; 4]) {
        let mid = r2_bits(cid, 127, 120) as u8;
        let oid = [r2_bits(cid, 119, 112) as u8, r2_bits(cid, 111, 104) as u8];
        let pnm = [
            r2_bits(cid, 103, 96) as u8,
            r2_bits(cid, 95, 88) as u8,
            r2_bits(cid, 87, 80) as u8,
            r2_bits(cid, 79, 72) as u8,
            r2_bits(cid, 71, 64) as u8,
        ];
        let prv = r2_bits(cid, 63, 56) as u8;
        let psn = r2_bits(cid, 55, 24) as u32;
        let mdt = r2_bits(cid, 19, 8) as u16; // [11:8]=month within, [19:12]=year-2000
        let year = 2000 + ((mdt >> 4) & 0xff) as u32;
        let month = (mdt & 0xf) as u32;
        let ascii = |b: u8| if b.is_ascii_graphic() { b } else { b'?' };
        serial_println!(
            "{}   M2: CID manufacturer(MID)={:#04x} OEM(OID)='{}{}' product(PNM)='{}{}{}{}{}' rev={:#x} serial(PSN)={:#010x} date={}/{} ::",
            PS, mid,
            ascii(oid[0]) as char, ascii(oid[1]) as char,
            ascii(pnm[0]) as char, ascii(pnm[1]) as char, ascii(pnm[2]) as char,
            ascii(pnm[3]) as char, ascii(pnm[4]) as char,
            prv, psn, month, year
        );
    }

    /// M3: read sector 0 (LBA 0) via a polled single-block CMD17 READ into `buf` (512 bytes). READ-ONLY
    /// (a read command; no card write). Returns whether the block was read. Mirrors `emmc2::read_block_512`
    /// (PIO, no DMA, no cache maintenance).
    fn read_sector0(base: u64, card: &Card, buf: &mut [u8; 512]) -> bool {
        // SDSC uses byte addressing; LBA 0 -> arg 0 in both modes, so no overflow concern here.
        let arg = if card.block_addressing { 0u32 } else { 0u32 };

        write32(base, INTERRUPT, 0xffff_ffff);
        write32(base, BLKSIZECNT, (1 << 16) | 512); // one block, 512 bytes

        // CMD17 READ_SINGLE_BLOCK: R1 + data present + card->host direction.
        if send_command(
            base,
            cmd(17) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK | CMD_ISDATA | CMD_DAT_DIR_READ,
            arg,
        )
        .is_err()
        {
            serial_println!("{}   M3: CMD17 (READ sector 0) failed at the link layer ::", PS);
            return false;
        }
        // The card's R1 verdict before touching the FIFO.
        let r1 = read32(base, RESP0);
        if r1 & R1_ERROR_MASK != 0 {
            serial_println!("{}   M3: CMD17 R1 error status {:#010x} ::", PS, r1);
            return false;
        }
        if !wait_set(base, INTERRUPT, INT_READ_RDY, DATA_TIMEOUT_MS) {
            serial_println!("{}   M3: read buffer never became ready (READ_RDY timeout) ::", PS);
            return false;
        }
        write32(base, INTERRUPT, INT_READ_RDY); // W1C
        for i in 0..128usize {
            let word = read32(base, DATA);
            let bytes = word.to_le_bytes();
            let off = i * 4;
            buf[off..off + 4].copy_from_slice(&bytes);
        }
        if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
            serial_println!("{}   M3: transfer-complete timeout after buffer read ::", PS);
            return false;
        }
        let int = read32(base, INTERRUPT);
        write32(base, INTERRUPT, int); // W1C everything we saw
        if int & INT_ERR_ANY != 0 {
            serial_println!("{}   M3: data-transfer error status {:#010x} ::", PS, int);
            return false;
        }
        true
    }

    /// Classify sector 0 by signature: GPT-protective MBR, FAT boot sector, generic MBR, or unknown.
    fn classify_sector0(buf: &[u8; 512]) -> &'static str {
        let sig_55aa = buf[510] == 0x55 && buf[511] == 0xaa;
        // FAT boot sector: a jump opcode (0xEB xx 90 / 0xE9) at byte 0 and a "FAT" filesystem-type string
        // (FAT12/16 @ 0x36, FAT32 @ 0x52).
        let fat_jump = buf[0] == 0xeb || buf[0] == 0xe9;
        let fat_str = &buf[0x36..0x39] == b"FAT" || &buf[0x52..0x55] == b"FAT";
        if sig_55aa {
            // First partition entry's type byte lives at 446 + 4 = 450.
            if buf[450] == 0xee {
                return "GPT-protective MBR (0xEE partition; GPT header at LBA 1)";
            }
            if fat_jump && fat_str {
                return "FAT boot sector (0x55AA + jump + FAT type string)";
            }
            return "MBR (0x55AA boot signature; classic partition table)";
        }
        if fat_jump && fat_str {
            return "FAT boot sector (jump + FAT type string; no 0x55AA)";
        }
        "unknown (no recognised signature)"
    }

    // ── M1: FDT census — enumerate SDMMC-compatible nodes, pick the enabled removable microSD slot ──

    const MAX_CAND: usize = 8;
    const PATH_CAP: usize = 160;

    /// A resolved SDMMC candidate node: its path, reg base/size, enabled/removable flags.
    struct Candidate {
        path: [u8; PATH_CAP],
        plen: usize,
        base: u64,
        size: u64,
        enabled: bool,
        removable: bool,
    }

    /// Read the 64-bit base + size from a `reg` whose cells are (addr:2, size:2) — the /bus@0 children
    /// shape on Tegra234. Returns (base, size); (0,0) if too short.
    fn reg_base_size(reg: &PropWords) -> (u64, u64) {
        if reg.n >= 4 {
            let base = ((reg.words[0] as u64) << 32) | reg.words[1] as u64;
            let size = ((reg.words[2] as u64) << 32) | reg.words[3] as u64;
            (base, size)
        } else if reg.n >= 2 {
            // Single (addr:1,size:1) fallback (unusual, but honest).
            (reg.words[0] as u64, reg.words[1] as u64)
        } else {
            (0, 0)
        }
    }

    /// A bounded ASCII view of a `compatible` NUL-joined string list, for the serial line.
    fn compat_ascii(fdt: &Fdt, node: &[u8], out: &mut [u8; 48]) -> usize {
        let compat = fdt.prop_at(node, b"compatible");
        let mut cl = 0usize;
        'fill: for wi in 0..compat.n {
            for b in compat.words[wi].to_be_bytes() {
                if cl >= out.len() {
                    break 'fill;
                }
                out[cl] = if b == 0 {
                    b'|'
                } else if b.is_ascii_graphic() {
                    b
                } else {
                    b'?'
                };
                cl += 1;
            }
        }
        cl
    }

    /// Is the node's `compatible` an SDHCI/SDMMC controller? (Tegra: `nvidia,tegra234-sdhci`; also match
    /// the generic `sdhci`/`mmc` tokens for robustness across firmware DT revisions.)
    fn is_sdmmc_node(fdt: &Fdt, node: &[u8], leaf: &[u8]) -> bool {
        let mut buf = [0u8; 48];
        let n = compat_ascii(fdt, node, &mut buf);
        let has = |needle: &[u8]| buf[..n].windows(needle.len()).any(|w| w == needle);
        has(b"sdhci") || has(b"tegra234-sdhci") || leaf.starts_with(b"mmc@") || leaf.starts_with(b"sdhci@")
    }

    /// Return the node status as "okay" (true) — absent status ⇒ okay per the DT spec.
    fn node_okay(fdt: &Fdt, node: &[u8]) -> bool {
        let status = fdt.prop_at(node, b"status");
        if !status.found {
            return true;
        }
        // status is a small string riding the BE words; check for "okay"/"ok".
        let mut sb = [0u8; 12];
        let mut sl = 0usize;
        'fill: for wi in 0..status.n {
            for b in status.words[wi].to_be_bytes() {
                if b == 0 || sl >= sb.len() {
                    break 'fill;
                }
                sb[sl] = b;
                sl += 1;
            }
        }
        &sb[..sl.min(4)] == b"okay" || &sb[..sl.min(2)] == b"ok"
    }

    /// M1: walk the DTB for every SDMMC-compatible node, log each candidate, and pick the enabled
    /// removable (microSD-slot) instance. Returns its (base, size), or None (with a printed reason).
    /// READ-ONLY RAM walk — no MMIO.
    fn resolve_microsd(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<(u64, u64)> {
        if dtb_addr == 0 || dtb_size == 0 {
            serial_println!("{}   M1: no DTB handed off — cannot resolve the SDMMC controller ::", PS);
            return None;
        }
        let g_lo = dtb_addr >> 30;
        let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
        let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
        if !mapped(g_lo) || !mapped(g_hi) {
            serial_println!("{}   M1: DTB GiB unmapped (mask {:#x}) — cannot walk it ::", PS, ram_gib_mask);
            return None;
        }
        let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
        let Some(fdt) = Fdt::new(blob) else {
            serial_println!("{}   M1: bad DTB header — cannot walk it ::", PS);
            return None;
        };

        // Collect unique candidate node paths (bounded). A node "is a candidate" when it carries a
        // `reg` AND its compatible/name marks it an SDMMC/SDHCI controller.
        let mut paths = [[0u8; PATH_CAP]; MAX_CAND];
        let mut lens = [0usize; MAX_CAND];
        let mut n = 0usize;
        fdt.for_each_prop(|e| {
            if n >= MAX_CAND || e.name != b"reg" {
                return;
            }
            let leaf = match e.path.iter().rposition(|&b| b == b'/') {
                Some(i) => &e.path[i + 1..],
                None => e.path,
            };
            // Cheap pre-filter on the node name so we only prop_at() plausible nodes.
            let name_hint = leaf.starts_with(b"mmc@")
                || leaf.starts_with(b"sdhci@")
                || leaf.windows(4).any(|w| w == b"mmc@")
                || leaf.windows(6).any(|w| w == b"sdmmc");
            if !name_hint && !is_sdmmc_node(&fdt, e.path, leaf) {
                return;
            }
            let l = e.path.len().min(PATH_CAP);
            let dup = (0..n).any(|i| lens[i] == l && paths[i][..l] == e.path[..l]);
            if !dup {
                paths[n][..l].copy_from_slice(&e.path[..l]);
                lens[n] = l;
                n += 1;
            }
        });

        if n == 0 {
            serial_println!("{}   M1: no SDMMC/SDHCI-compatible node found in the DTB ::", PS);
            return None;
        }

        // Resolve + log each candidate; remember the best (enabled + removable) microSD slot.
        let mut best: Option<Candidate> = None;
        let mut first_enabled: Option<Candidate> = None;
        for i in 0..n {
            let node = &paths[i][..lens[i]];
            // Confirm it really is an SDMMC controller (the name pre-filter can be loose).
            let leaf = match node.iter().rposition(|&b| b == b'/') {
                Some(k) => &node[k + 1..],
                None => node,
            };
            if !is_sdmmc_node(&fdt, node, leaf) {
                continue;
            }
            let reg = fdt.prop_at(node, b"reg");
            let (base, size) = reg_base_size(&reg);
            let enabled = node_okay(&fdt, node);
            let removable = !fdt.prop_at(node, b"non-removable").found;
            let has_cd = fdt.prop_at(node, b"cd-gpios").found;
            let mut cbuf = [0u8; 48];
            let cn = compat_ascii(&fdt, node, &mut cbuf);
            serial_println!(
                "{}   M1: candidate {} reg={:#010x}(size {:#x}) status={} {} {} compat='{}' ::",
                PS,
                core::str::from_utf8(node).unwrap_or("?"),
                base, size,
                if enabled { "okay" } else { "disabled" },
                if removable { "removable" } else { "non-removable" },
                if has_cd { "cd-gpios" } else { "no-cd" },
                core::str::from_utf8(&cbuf[..cn]).unwrap_or("?"),
            );
            if base == 0 {
                continue;
            }
            let cand = Candidate {
                path: paths[i],
                plen: lens[i],
                base,
                size,
                enabled,
                removable,
            };
            if enabled && removable && best.is_none() {
                best = Some(cand);
            } else if enabled && first_enabled.is_none() {
                first_enabled = Some(cand);
            }
        }

        // Prefer the enabled removable slot (the microSD). Fall back to the first enabled instance
        // (documented — e.g. a DT that doesn't mark the slot removable), else refuse.
        let pick = best.or(first_enabled)?;
        let _ = (pick.enabled, pick.removable); // (all fields consumed for logging clarity)
        let node = &pick.path[..pick.plen];
        serial_println!(
            "{}   M1: picked {} @ {:#010x} (size {:#x}) as the microSD slot ::",
            PS,
            core::str::from_utf8(node).unwrap_or("?"),
            pick.base, pick.size
        );
        Some((pick.base, pick.size))
    }

    /// ORIN-SDMMC-1 entry point (metal): FDT census (M1) -> map + poison-honest CAPS probe -> SDHCI
    /// identification (M2) -> sector-0 read + classify (M3). READ-ONLY to the card throughout. Graceful
    /// on any missing/foreign DTB, absent decode, or absent card (records and returns).
    pub fn sdmmc_census(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
        serial_println!(
            "{} ORIN-SDMMC-1 Tegra234 microSD READ-ONLY recon (DTB @{:#x} size={:#x}) ::",
            PS, dtb_addr, dtb_size
        );

        // ── M1: resolve the microSD-slot controller from the live DTB ──
        let Some((base, size)) = resolve_microsd(dtb_addr, dtb_size, ram_gib_mask) else {
            serial_println!("{}   recon SKIPPED (no resolvable microSD-slot SDMMC controller) ::", PS);
            return;
        };

        // The Tegra234 SDMMC controllers live in the GiB-0 device window that `mmu_tegra` already maps
        // Device-nGnRE at boot (sdmmc1 @ 0x0340_0000 « 0x4000_0000). Confirm the register window sits in a
        // MAPPED GiB (GiB-0 device window, or a RAM GiB from the mask) before dereferencing it — never
        // deref an unmapped address. A controller outside the already-mapped windows would need the
        // pcie2 `map_mmio_window` path (not pulled by this standalone feature) ⇒ honest refusal.
        let map_size = if size == 0 { 0x1000usize } else { size as usize };
        let g_lo = base >> 30;
        let g_hi = (base + map_size as u64 - 1) >> 30;
        let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
        if !mapped(g_lo) || !mapped(g_hi) {
            serial_println!(
                "{}   M1: controller window {:#010x}(+{:#x}) is outside the already-mapped GiB windows (mask {:#x}) — recon SKIPPED (no unmapped deref) ::",
                PS, base, map_size, ram_gib_mask
            );
            return;
        }
        serial_println!(
            "{}   M1: controller window {:#010x}(+{:#x}) is in the GiB-0 device window (already Device-nGnRE) ::",
            PS, base, map_size
        );

        // ── M1: POISON-HONEST probe read (CAPABILITIES + Host Version) BEFORE anything else (NET-4b law) ──
        let caps = read32(base, CAPABILITIES);
        if is_poison(caps) {
            serial_println!(
                "{}   M1: CAPABILITIES[{:#x}] = {:#010x} — POISON (open bus / carveout / firmware fill); the window is NOT a live SDHCI — recon REFUSED (no reset, no writes) ::",
                PS, CAPABILITIES, caps
            );
            return;
        }
        let hcver = (read32(base, HOST_VERSION) >> 16) & 0xff; // SDHCI spec version = version reg [7:0]
        serial_println!(
            "{}   M1: live SDHCI — CAPABILITIES={:#010x} (base-clk {} MHz, 8-bit={}, ADMA2={}), spec-version reg={:#04x} (SDHCI {}) ::",
            PS, caps,
            (caps >> 8) & 0xff,
            (caps >> 18) & 1,
            (caps >> 19) & 1,
            hcver,
            match hcver { 0 => "1.0", 1 => "2.0", 2 => "3.0", 3 => "4.0", _ => "?" }
        );

        // ── M2: SDHCI identification ladder (READ-ONLY) ──
        let Some(card) = identify(base) else {
            serial_println!("{} ORIN-SDMMC-1 recon done at M2 (no identified card / honest stop) ::", PS);
            return;
        };

        // ── M3: sector-0 read census + signature classification ──
        let mut sec0 = [0u8; 512];
        if !read_sector0(base, &card, &mut sec0) {
            serial_println!("{} ORIN-SDMMC-1 recon STOPPED at M3 (sector-0 read failed) ::", PS);
            return;
        }
        // Dump the first 16 bytes hex.
        let h = &sec0[..16];
        serial_println!(
            "{}   M3: sector 0 first 16 bytes = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            PS, h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]
        );
        let class = classify_sector0(&sec0);
        serial_println!("{}   M3: sector-0 signature = {} ::", PS, class);
        serial_println!(
            "{} ORIN-SDMMC-1 DONE — microSD censused: {} blocks ({} MiB, CSD v{}), sector-0 {} (READ-ONLY; no card write) ::",
            PS, card.num_blocks, card.num_blocks * 512 / (1024 * 1024), card.csd_version, class
        );
    }
}
